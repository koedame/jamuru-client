#!/usr/bin/env python3
"""Build the third-party license notices shipped with the app.

  scripts/third-party-licenses.py          regenerate the two files below
  scripts/third-party-licenses.py --check  fail when they are out of date

Outputs:
  THIRD_PARTY_LICENSES.md              the repository's copy
  src-tauri/resources/LICENSES.txt     bundled into every installer

What goes in:
  - every Rust crate of the root crate and of src-tauri (cargo-about, all
    features, every release platform in about.toml), with the license text
    found in the crate itself
  - the npm packages the UI ships (ui/package-lock.json, not dev-only), with
    the license file from node_modules
  - the files in packaging/third-party/ (code copied into the source)

`--check` does not run cargo-about (it takes minutes). Both outputs carry a
hash of everything they are built from; the check recomputes that hash and
the hash of the output body, so it fails when an input changed without a
regeneration and when a generated file was edited by hand.
Regenerating needs cargo-about, network access for `cargo fetch`, and
`npm ci` in ui/.
"""

import hashlib
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MD_OUT = ROOT / "THIRD_PARTY_LICENSES.md"
TXT_OUT = ROOT / "src-tauri/resources/LICENSES.txt"

MANIFESTS = [ROOT / "Cargo.toml", ROOT / "src-tauri/Cargo.toml"]
INPUTS = [
    "Cargo.toml",
    "Cargo.lock",
    "src-tauri/Cargo.toml",
    "src-tauri/Cargo.lock",
    "ui/package.json",
    "ui/package-lock.json",
    "about.toml",
    "deny.toml",
    "scripts/third-party-licenses.py",
]
EXTRA_DIR = ROOT / "packaging/third-party"
HEADER_KEY = "inputs-sha256"
BODY_KEY = "body-sha256"

LINUX_NOTICE_ASSET = "jamjam-linux-bundled-libraries.txt"
RELEASES_URL = "https://github.com/koedame/jamjam-client/releases"

# Static: what is not derived from a lockfile.
EXTRA_SOURCES = {
    "lucide-LICENSE.txt": (
        "Lucide icons",
        "https://github.com/lucide-icons/lucide",
        "The SVG path data of the icons in ui/src/lib/icons.tsx and in the "
        "component files is copied from Lucide.",
    ),
}

JAMJAM_TXT_HEADER = """\
jamjam - License Information

jamjam is licensed under the jamjam Source Available License.
Copyright (c) 2024 koedame

TERMS AND CONDITIONS

1. PERMITTED USES
   - View and read the source code for educational or transparency purposes
   - Use Official Binaries distributed by the copyright holder

2. RESTRICTIONS
   - Modification, adaptation, or creating derivative works is NOT permitted
   - Redistribution, sublicensing, or transfer is NOT permitted
   - Commercial use is NOT permitted
   - Building, compiling, or executing from source code is NOT permitted
   - Creating competing products or services is NOT permitted

3. NO WARRANTY
   THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND.

For full license terms, see: https://github.com/koedame/jamjam-client/blob/HEAD/LICENSE
"""


def normalize(text: str) -> str:
    """Same bytes everywhere: LF line ends, no trailing blank lines."""
    return text.replace("\r\n", "\n").strip("\n")


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def inputs_hash() -> str:
    h = hashlib.sha256()
    paths = [ROOT / p for p in INPUTS] + sorted(EXTRA_DIR.glob("*"))
    for p in paths:
        h.update(str(p.relative_to(ROOT)).encode() + b"\0")
        h.update(p.read_bytes() + b"\0")
    return h.hexdigest()


def check_policy_sync() -> None:
    """about.toml accepts exactly what deny.toml allows."""
    import tomllib  # Python 3.11+; only regenerating needs it, `--check` runs on older ones too

    about = tomllib.loads((ROOT / "about.toml").read_text())
    deny = tomllib.loads((ROOT / "deny.toml").read_text())
    pairs = [
        ("licenses", sorted(about["accepted"]), sorted(deny["licenses"]["allow"])),
        ("targets", sorted(about["targets"]), sorted(deny["graph"]["targets"])),
    ]
    for name, a, d in pairs:
        if a != d:
            sys.exit(
                f"about.toml and deny.toml disagree on {name}:\n"
                f"  only in about.toml: {sorted(set(a) - set(d))}\n"
                f"  only in deny.toml:  {sorted(set(d) - set(a))}"
            )


def cargo_licenses() -> dict:
    """(license id, name, text) -> {(crate, version): repository}"""
    groups: dict = {}
    for manifest in MANIFESTS:
        subprocess.run(
            ["cargo", "fetch", "--locked", "--manifest-path", str(manifest)],
            check=True,
            cwd=ROOT,
        )
        out = subprocess.run(
            [
                "cargo", "about", "generate",
                "--manifest-path", str(manifest),
                "--all-features", "--locked", "--offline",
                "--format", "json",
            ],
            check=True,
            cwd=ROOT,
            capture_output=True,
            text=True,
        ).stdout
        for lic in json.loads(out)["licenses"]:
            key = (lic["id"], lic["name"], normalize(lic["text"]))
            users = groups.setdefault(key, {})
            for u in lic["used_by"]:
                c = u["crate"]
                users[(c["name"], c["version"])] = c.get("repository") or ""
    return groups


# Packages that ship only an SPDX tag file; their license texts are the ones
# of the project they come from.
NPM_LICENSE_FROM = {"@tauri-apps/plugin-deep-link": "@tauri-apps/api"}


def npm_license_files(pdir: Path) -> list:
    return [
        f for f in sorted(pdir.iterdir())
        if f.is_file()
        and f.suffix.lower() != ".spdx"
        and f.name.upper().startswith(("LICENSE", "LICENCE", "COPYING"))
    ]


def npm_licenses() -> dict:
    lock = json.loads((ROOT / "ui/package-lock.json").read_text())
    groups: dict = {}
    for path, pkg in lock["packages"].items():
        if not path or pkg.get("dev") or pkg.get("devOptional") or pkg.get("optional"):
            continue
        pdir = ROOT / "ui" / path
        meta = json.loads((pdir / "package.json").read_text())
        files = npm_license_files(pdir)
        if not files and meta["name"] in NPM_LICENSE_FROM:
            files = npm_license_files(ROOT / "ui/node_modules" / NPM_LICENSE_FROM[meta["name"]])
        if not files:
            sys.exit(f"npm package {path} has no license file; run `npm ci` in ui/")
        repo = meta.get("repository")
        repo = repo.get("url", "") if isinstance(repo, dict) else (repo or "")
        repo = repo.removeprefix("git+").removesuffix(".git")
        lid = pkg.get("license") or meta.get("license") or "UNKNOWN"
        for f in files:
            name = lid if len(files) == 1 else f.name
            users = groups.setdefault((lid, name, normalize(f.read_text(encoding="utf-8"))), {})
            users[(meta["name"], meta["version"])] = repo
    return groups


def extra_licenses() -> list:
    out = []
    for f in sorted(EXTRA_DIR.glob("*-LICENSE.txt")):
        title, url, note = EXTRA_SOURCES[f.name]
        out.append((title, url, note, normalize(f.read_text(encoding="utf-8"))))
    return out


def fence(text: str) -> str:
    return "````" if "```" in text else "```"


def sorted_groups(groups: dict) -> list:
    return sorted(groups.items(), key=lambda kv: (kv[0][0], sha256_bytes(kv[0][2].encode())))


def users_line(users: dict) -> str:
    return ", ".join(f"{n} {v}" for (n, v) in sorted(users))


def source_lines(license_id: str, users: dict, kind: str) -> list:
    """MPL-2.0 asks for the location of the source of the covered files."""
    if "MPL" not in license_id:
        return []
    base = "https://crates.io/crates" if kind == "crate" else "https://www.npmjs.com/package"
    return [f"{base}/{n}/{v}" for (n, v) in sorted(users)]


def summary(groups: dict) -> list:
    counts: dict = {}
    for (lid, _, _), users in groups.items():
        counts.setdefault(lid, set()).update(users)
    return sorted((lid, len(u)) for lid, u in counts.items())


def render(cargo: dict, npm: dict, extra: list, fmt: str) -> str:
    md = fmt == "md"
    out: list = []

    def h1(t):
        out.append(f"# {t}\n" if md else f"{'=' * 78}\n{t}\n{'=' * 78}\n")

    def h2(t):
        out.append(f"## {t}\n" if md else f"\n{'=' * 78}\n{t}\n{'=' * 78}\n")

    def h3(t):
        out.append(f"### {t}\n" if md else f"\n{t}\n{'-' * min(len(t), 78)}\n")

    def text_block(t):
        if md:
            f = fence(t)
            out.append(f"{f}\n{t}\n{f}\n")
        else:
            out.append(t + "\n")

    h1("Third-Party Licenses")
    out.append(
        "jamjam includes the third-party software listed here. Each is used under its own\n"
        "license; the copyright notices and license texts below are reproduced as those\n"
        "licenses require. jamjam's own license is in LICENSE.\n"
        if md
        else "jamjam includes the third-party software listed here. Each is used under its\n"
        "own license; the copyright notices and license texts below are reproduced as\n"
        "those licenses require.\n"
    )
    out.append(
        "This list is generated by scripts/third-party-licenses.py from the lock files; do\n"
        "not edit it by hand.\n"
    )

    h2("Summary")
    for title, groups in (("Rust crates", cargo), ("npm packages", npm)):
        h3(title)
        for lid, n in summary(groups):
            out.append(f"- {lid}: {n}" if md else f"  {lid}: {n}")
        out.append("")

    for title, groups, kind in (
        ("Rust crates (the root crate and src-tauri; all features, all release platforms)", cargo, "crate"),
        ("npm packages shipped in the UI", npm, "npm"),
    ):
        h2(title)
        for (lid, name, text), users in sorted_groups(groups):
            h3(f"{name} ({lid})" if name != lid else lid)
            out.append("Used by: " + users_line(users) + "\n")
            src = source_lines(lid, users, kind)
            if src:
                out.append(
                    "Source code of these packages (MPL-2.0 requires saying where to get it):\n"
                    + "\n".join(f"- {s}" for s in src)
                    + "\n"
                )
            text_block(text)

    h2("Code copied into the source")
    for title, url, note, text in extra:
        h3(f"{title} ({url})")
        out.append(note + "\n")
        text_block(text)

    h2("Linux AppImage: libraries bundled from the system")
    out.append(
        "The Linux AppImage also carries shared libraries copied from the Ubuntu build\n"
        "environment (GTK, WebKitGTK, GLib, GStreamer and what they depend on). Many of\n"
        "them are under the LGPL. Which libraries they are depends on the build, so the\n"
        "list, the copyright file of each package, the license texts it refers to and\n"
        "where to get the source are generated from the AppImage itself and attached to\n"
        f"the same release as the AppImage, as {LINUX_NOTICE_ASSET}:\n"
        f"{RELEASES_URL}\n"
    )
    return "\n".join(out).rstrip("\n") + "\n"


def with_header(body: str, inputs: str, fmt: str) -> str:
    line = f"Generated by scripts/third-party-licenses.py. Do not edit. {HEADER_KEY}: {inputs} {BODY_KEY}: {sha256_bytes(body.encode())}"
    return (f"<!-- {line} -->\n" if fmt == "md" else f"{line}\n") + body


def parse_header(content: str, fmt: str):
    first, _, body = content.partition("\n")
    if fmt == "md":
        first = first.removeprefix("<!-- ").removesuffix(" -->")
    words = first.split()
    try:
        return words[words.index(HEADER_KEY + ":") + 1], words[words.index(BODY_KEY + ":") + 1], body
    except (ValueError, IndexError):
        return None, None, body


def txt_body(rendered: str) -> str:
    return JAMJAM_TXT_HEADER + "\n" + rendered


def check() -> int:
    want = inputs_hash()
    bad = []
    for path, fmt in ((MD_OUT, "md"), (TXT_OUT, "txt")):
        have_inputs, have_body, body = parse_header(path.read_bytes().decode("utf-8"), fmt)
        rel = path.relative_to(ROOT)
        if have_inputs != want:
            bad.append(f"{rel}: built from different inputs (lock files, about.toml, deny.toml, packaging/third-party or this script changed)")
        elif have_body != sha256_bytes(body.encode()):
            bad.append(f"{rel}: edited by hand")
    if bad:
        print("\n".join(bad), file=sys.stderr)
        print("Regenerate with: scripts/third-party-licenses.py", file=sys.stderr)
        return 1
    print("third-party license notices are up to date")
    return 0


def generate() -> int:
    check_policy_sync()
    npm, extra = npm_licenses(), extra_licenses()
    cargo = cargo_licenses()
    inputs = inputs_hash()
    MD_OUT.write_bytes(with_header(render(cargo, npm, extra, "md"), inputs, "md").encode("utf-8"))
    TXT_OUT.write_bytes(with_header(txt_body(render(cargo, npm, extra, "txt")), inputs, "txt").encode("utf-8"))
    for p in (MD_OUT, TXT_OUT):
        print(f"wrote {p.relative_to(ROOT)} ({p.stat().st_size} bytes)")
    return 0


if __name__ == "__main__":
    if sys.argv[1:] == ["--check"]:
        sys.exit(check())
    if sys.argv[1:]:
        sys.exit(__doc__)
    sys.exit(generate())
