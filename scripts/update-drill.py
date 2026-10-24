#!/usr/bin/env python3
"""The update drill: installs real packages over each other and checks that an
update that cannot start is taken out again (ADR-088).

Two builds of this source are made with the `update-drill` feature: version A
(0.0.1) and version B (0.0.2), both trusting a key made for the drill and
looking for updates on this machine. Each scenario installs A, serves B as the
newer release, starts A and waits for what must follow:

  good        A installs B and restarts into it; B starts and is kept.
  crash       B ends right after starting; A is put back, started again, and
              does not install B again by itself.
  hang        B never says it is up; it is ended, A is put back as above.
  never-runs  B ends before it starts at all; A is put back as above.

A scenario passes only when the installed program is exactly B's (good) or
exactly A's again (the rest), the update's files are cleaned up, and the
version given up on is remembered.

    # The two builds (each a `cargo tauri build` of this source):
    scripts/update-drill.py build --out drill

    # The drill (on Linux, under a display: `xvfb-run -a ...`):
    scripts/update-drill.py run --a drill/a --b drill/b

The versions are below every release, so what the drill leaves behind cannot
hold back a real one. On Linux everything goes under a throwaway home. On
macOS and Windows the app's own places in the user's profile are used and the
Windows build is installed where the app installs, so the drill runs there
only on a machine that is thrown away afterwards (CI).
"""

from __future__ import annotations

import argparse
import hashlib
import http.server
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
import uuid
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path

VERSION_A = "0.0.1"
VERSION_B = "0.0.2"
DEFAULT_PORT = 18765
SCENARIOS = ("good", "crash", "hang", "never-runs")
REPO = Path(__file__).resolve().parent.parent

# How long each outcome may take. The watcher gives a new version 180 s to say
# it is up, and the Windows installer runs inside the 180 s it gives a start.
OUTCOME_LIMIT = {"good": 300, "crash": 240, "hang": 360, "never-runs": 360}
# How long a version put back is watched for installing the given-up one again.
NO_REINSTALL_WATCH = 40


def say(line: str) -> None:
    print(f"[drill {datetime.now().strftime('%H:%M:%S')}] {line}", flush=True)


class DrillFailed(Exception):
    pass


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def one(pattern: str, under: Path) -> Path:
    found = sorted(under.glob(pattern))
    if len(found) != 1:
        raise DrillFailed(f"expected one {pattern} under {under}, found {[str(p) for p in found]}")
    return found[0]


def terms_version() -> int:
    text = (REPO / "src" / "config.rs").read_text(encoding="utf-8")
    found = re.search(r"pub const TERMS_VERSION: u32 = (\d+);", text)
    if not found:
        raise DrillFailed("TERMS_VERSION not found in src/config.rs")
    return int(found.group(1))


# --- where things are, per system -----------------------------------------


@dataclass
class Places:
    home: Path
    config: Path
    store: Path
    log: Path
    # Where the package is installed and how it is started.
    installed_program: Path

    @staticmethod
    def of(home: Path, work: Path) -> "Places":
        if sys.platform == "darwin":
            support = home / "Library" / "Application Support" / "jamuru"
            app = work / "apps" / "jamuru.app"
            return Places(
                home=home,
                config=support / "config.toml",
                store=support / "update",
                log=home / "Library" / "Logs" / "me.koeda.jamuru" / "jamuru.log",
                installed_program=app / "Contents" / "MacOS" / "jamuru-app",
            )
        if sys.platform == "win32":
            local = Path(os.environ["LOCALAPPDATA"])
            roaming = Path(os.environ["APPDATA"])
            return Places(
                home=home,
                config=roaming / "jamuru" / "config" / "config.toml",
                store=local / "jamuru" / "data" / "update",
                log=local / "me.koeda.jamuru" / "logs" / "jamuru.log",
                installed_program=local / "jamuru" / "jamuru-app.exe",
            )
        return Places(
            home=home,
            config=home / ".config" / "jamuru" / "config.toml",
            store=home / ".local" / "share" / "jamuru" / "update",
            log=home / ".local" / "share" / "me.koeda.jamuru" / "logs" / "jamuru.log",
            installed_program=work / "apps" / "jamuru.AppImage",
        )


@dataclass
class Builds:
    """What the drill needs of the two builds' bundle directories."""

    a_package: Path
    b_update: Path
    b_signature: Path
    platform_keys: tuple[str, ...]

    @staticmethod
    def of(a: Path, b: Path) -> "Builds":
        if sys.platform == "darwin":
            arch = "aarch64" if os.uname().machine == "arm64" else "x86_64"
            update = one("macos/*.app.tar.gz", b)
            return Builds(
                a_package=one("macos/*.app", a),
                b_update=update,
                b_signature=Path(f"{update}.sig"),
                platform_keys=(f"darwin-{arch}-app", f"darwin-{arch}"),
            )
        if sys.platform == "win32":
            update = one("nsis/*-setup.exe", b)
            return Builds(
                a_package=one("nsis/*-setup.exe", a),
                b_update=update,
                b_signature=Path(f"{update}.sig"),
                platform_keys=("windows-x86_64-nsis", "windows-x86_64"),
            )
        update = one("appimage/*.AppImage", b)
        return Builds(
            a_package=one("appimage/*.AppImage", a),
            b_update=update,
            b_signature=Path(f"{update}.sig"),
            platform_keys=("linux-x86_64-appimage", "linux-x86_64"),
        )


# --- the release being offered --------------------------------------------


class Offer:
    """Serves B as the newer release on 127.0.0.1, as the release would."""

    def __init__(self, builds: Builds, port: int, work: Path):
        self.dir = work / "offer"
        self.dir.mkdir(parents=True, exist_ok=True)
        shutil.copy2(builds.b_update, self.dir / builds.b_update.name)
        url = f"http://127.0.0.1:{port}/{builds.b_update.name}"
        signature = builds.b_signature.read_text(encoding="utf-8").strip()
        manifest = {
            "version": VERSION_B,
            "pub_date": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
            "platforms": {key: {"url": url, "signature": signature} for key in builds.platform_keys},
        }
        (self.dir / "latest.json").write_text(json.dumps(manifest), encoding="utf-8")
        self.requests: list[str] = []
        offer = self

        class Handler(http.server.SimpleHTTPRequestHandler):
            def __init__(self, *args, **kwargs):
                super().__init__(*args, directory=str(offer.dir), **kwargs)

            def log_message(self, format, *args):  # noqa: A002 - the base class's name
                offer.requests.append(format % args)

        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def close(self) -> None:
        self.server.shutdown()
        self.server.server_close()


# --- the app ----------------------------------------------------------------


class App:
    def __init__(self, places: Places, builds: Builds, work: Path):
        self.places = places
        self.builds = builds
        self.work = work
        self.run_id = uuid.uuid4().hex
        self.output = work / "app-output.log"

    def env(self, breaking: str | None) -> dict[str, str]:
        env = dict(os.environ)
        env["HOME"] = str(self.places.home)
        # The places above are the ones a home of their own gives.
        for name in ("XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "XDG_STATE_HOME"):
            env.pop(name, None)
        # No FUSE on a CI runner or in a container: the AppImage unpacks itself.
        env["APPIMAGE_EXTRACT_AND_RUN"] = "1"
        # Every process of this drill carries this, to be found and ended.
        env["JAMJAM_UPDATE_DRILL_RUN"] = self.run_id
        env.pop("JAMJAM_UPDATE_DRILL_BREAK", None)
        if breaking:
            env["JAMJAM_UPDATE_DRILL_BREAK"] = f"{VERSION_B}:{breaking}"
        return env

    def install_a(self) -> None:
        program = self.places.installed_program
        if sys.platform == "win32":
            subprocess.run([str(self.builds.a_package), "/S"], check=True, timeout=300)
        elif sys.platform == "darwin":
            app = program.parent.parent.parent
            shutil.rmtree(app, ignore_errors=True)
            app.parent.mkdir(parents=True, exist_ok=True)
            subprocess.run(["ditto", str(self.builds.a_package), str(app)], check=True)
        else:
            program.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(self.builds.a_package, program)
            program.chmod(0o755)
        if not program.is_file():
            raise DrillFailed(f"A was installed but {program} is not there")

    def start(self, breaking: str | None) -> None:
        kwargs: dict = {}
        if sys.platform == "win32":
            kwargs["creationflags"] = subprocess.DETACHED_PROCESS | subprocess.CREATE_NEW_PROCESS_GROUP
        else:
            kwargs["start_new_session"] = True
        with self.output.open("ab") as out:
            subprocess.Popen(
                [str(self.places.installed_program)],
                env=self.env(breaking),
                stdin=subprocess.DEVNULL,
                stdout=out,
                stderr=subprocess.STDOUT,
                **kwargs,
            )

    def end_all(self) -> None:
        """Ends every process of the app and of its watcher."""
        if sys.platform == "win32":
            for name in ("jamuru-app.exe", "jamuru-update.exe"):
                subprocess.run(["taskkill", "/F", "/T", "/IM", name], capture_output=True)
        elif sys.platform == "darwin":
            # A version started again by the watcher is opened by the system
            # and does not carry the drill's environment; it is found by where
            # it runs from.
            for where in (str(self.work / "apps"), str(self.places.store)):
                subprocess.run(["pkill", "-9", "-f", where], capture_output=True)
        else:
            for pid in self.linux_pids():
                try:
                    os.kill(pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
        time.sleep(2)

    def linux_pids(self) -> list[int]:
        marker = f"JAMJAM_UPDATE_DRILL_RUN={self.run_id}".encode()
        pids = []
        for entry in Path("/proc").iterdir():
            if not entry.name.isdigit() or int(entry.name) == os.getpid():
                continue
            try:
                environ = (entry / "environ").read_bytes()
            except OSError:
                continue
            if marker in environ.split(b"\0"):
                pids.append(int(entry.name))
        return pids


# --- what is looked at ------------------------------------------------------


class Look:
    def __init__(self, places: Places):
        self.places = places

    def watch_log(self) -> str:
        return read(self.places.store / "watch.log")

    def app_log(self) -> str:
        return read(self.places.log)

    def starts(self, version: str) -> int:
        """How many times `version` got as far as the app's log."""
        return len(re.findall(rf"jamuru {re.escape(version)} starting", self.app_log()))

    def tried(self, version: str) -> int:
        """How many times `version` ran as the update being tried. A build
        broken to crash or hang does so before the app's log exists, so this
        is what says it ran at all."""
        return self.watch_log().count(f"{version} takes part in")

    def trial_open(self) -> bool:
        return (self.places.store / "trial.json").exists()

    def slot_left(self) -> bool:
        return (self.places.store / "slot").exists() or (self.places.store / "slot.partial").exists()

    def rejected(self) -> list[str]:
        text = read(self.places.store / "rejected.json")
        return json.loads(text) if text else []

    def installed(self) -> str | None:
        program = self.places.installed_program
        return sha256(program) if program.is_file() else None

    def report(self) -> str:
        store = self.places.store
        listing = sorted(str(p.relative_to(store)) for p in store.rglob("*")) if store.exists() else []
        return "\n".join(
            [
                f"--- store {store}: {listing[:40]}",
                "--- watch.log",
                self.watch_log()[-6000:],
                "--- app log (end)",
                self.app_log()[-6000:],
            ]
        )


def read(path: Path) -> str:
    try:
        return path.read_text(encoding="utf-8", errors="replace")
    except FileNotFoundError:
        return ""


def wait_for(what: str, limit: float, check) -> None:
    deadline = time.monotonic() + limit
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(1)
    raise DrillFailed(f"not within {limit:.0f} s: {what}")


# --- the scenarios ----------------------------------------------------------


def fresh(places: Places, app: App) -> None:
    """Before a scenario: nothing of a previous one is left."""
    app.end_all()
    shutil.rmtree(places.store, ignore_errors=True)
    if places.log.exists():
        places.log.unlink()
    places.config.parent.mkdir(parents=True, exist_ok=True)
    # Agreed to the terms (or the app waits on the consent screen and reaches
    # nothing), and updating by itself, as it is by default.
    places.config.write_text(f"terms_version = {terms_version()}\nauto_update = true\n", encoding="utf-8")
    app.install_a()


def run_scenario(name: str, places: Places, app: App, offer: Offer) -> None:
    say(f"scenario {name}")
    look = Look(places)
    fresh(places, app)
    a_hash = look.installed()
    breaking = None if name == "good" else name
    offer.requests.clear()
    app.start(breaking)
    wait_for("A starts", 60, lambda: look.starts(VERSION_A) >= 1)

    if name == "good":
        wait_for(
            f"{VERSION_B} is confirmed",
            OUTCOME_LIMIT[name],
            lambda: f"{VERSION_B} confirmed" in look.watch_log(),
        )
        wait_for("the trial is closed", 30, lambda: not look.trial_open() and not look.slot_left())
        expect(look.installed() not in (None, a_hash), "B is what is installed")
        expect(look.starts(VERSION_B) == 1, f"{VERSION_B} started once")
        expect(look.tried(VERSION_B) == 1, f"{VERSION_B} was tried once")
        expect(VERSION_B not in look.rejected(), f"{VERSION_B} is not remembered as given up on")
        # B is the newest release: it finds nothing newer and installs nothing.
        time.sleep(15)
        expect(not look.trial_open(), "B opened no trial of its own")
        expect(look.starts(VERSION_B) == 1, f"{VERSION_B} did not restart")
    else:
        wait_for(
            f"{VERSION_A} is put back",
            OUTCOME_LIMIT[name],
            lambda: f"{VERSION_A} is back" in look.watch_log(),
        )
        wait_for(f"{VERSION_A} is started again", 60, lambda: look.starts(VERSION_A) >= 2)
        expect(look.installed() == a_hash, "the installed program is A's again, byte for byte")
        expect(not look.trial_open() and not look.slot_left(), "the trial is closed")
        expect(look.rejected() == [VERSION_B], f"{VERSION_B} is remembered as given up on")
        # Broken where it was asked to break: one try, never as far as the app.
        expect(
            look.tried(VERSION_B) == (0 if name == "never-runs" else 1),
            f"{VERSION_B} was started only by the update",
        )
        expect(look.starts(VERSION_B) == 0, f"{VERSION_B} broke before the app opened")
        # No loop: A finds B again and leaves it alone.
        time.sleep(NO_REINSTALL_WATCH)
        expect(not look.trial_open(), "A did not install B again")
        expect(look.installed() == a_hash, "A is still what is installed")
        expect(
            f"Not installing {VERSION_B} by itself" in look.app_log(),
            "A says it leaves B alone",
        )
        expect(look.starts(VERSION_A) == 2, f"{VERSION_A} was not restarted again")
    app.end_all()
    say(f"scenario {name}: passed")


def expect(condition: bool, what: str) -> None:
    if not condition:
        raise DrillFailed(f"expected: {what}")


def run(args: argparse.Namespace) -> int:
    if sys.platform in ("darwin", "win32") and os.environ.get("CI") != "true":
        print(
            "The drill installs over the app's own places on this system; it runs only on CI "
            "(a machine thrown away afterwards).",
            file=sys.stderr,
        )
        return 2
    work = Path(tempfile.mkdtemp(prefix="update-drill-"))
    if sys.platform in ("darwin", "win32"):
        home = Path.home()
    else:
        home = work / "home"
        home.mkdir()
    places = Places.of(home, work)
    builds = Builds.of(Path(args.a), Path(args.b))
    app = App(places, builds, work)
    offer = Offer(builds, args.port, work)
    say(f"A {VERSION_A}: {builds.a_package}")
    say(f"B {VERSION_B}: {builds.b_update}")
    say(f"work: {work}")
    failed = []
    try:
        for name in args.scenario or SCENARIOS:
            try:
                run_scenario(name, places, app, offer)
            except DrillFailed as e:
                failed.append(name)
                say(f"scenario {name}: FAILED: {e}")
                print(Look(places).report(), flush=True)
                print("--- requests to the offer", *offer.requests[-20:], sep="\n", flush=True)
                print("--- app output (end)", read(app.output)[-4000:], sep="\n", flush=True)
    finally:
        app.end_all()
        offer.close()
    if failed:
        say(f"FAILED: {', '.join(failed)}")
        return 1
    say("all scenarios passed")
    return 0


def settings(version: str, pubkey: str, port: int) -> dict:
    """The settings one of the drill's builds is made with: its version, and
    updates looked for here and signed by the drill's key."""
    return {
        "version": version,
        "bundle": {"createUpdaterArtifacts": True},
        "plugins": {
            "updater": {
                "pubkey": pubkey,
                "endpoints": [f"http://127.0.0.1:{port}/latest.json"],
                "dangerousInsecureTransportProtocol": True,
                "requireSignedVersion": True,
                "windows": {"installMode": "quiet"},
            }
        },
    }


# What each system installs and updates from, as `cargo tauri build --bundles`
# names it, and the directory under `bundle/` it lands in.
BUNDLE = {"darwin": ("app", "macos"), "win32": ("nsis", "nsis")}.get(sys.platform, ("appimage", "appimage"))


def build(args: argparse.Namespace) -> int:
    out = Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    key = out / "drill.key"
    # A key of the drill's own: the release key never signs a build made to
    # fail.
    subprocess.run(
        ["cargo", "tauri", "signer", "generate", "--ci", "--force", "-p", "", "-w", str(key)],
        cwd=REPO,
        check=True,
        stdout=subprocess.DEVNULL,
    )
    pubkey = Path(f"{key}.pub").read_text(encoding="utf-8").strip()
    flag, subdir = BUNDLE
    target = REPO / "src-tauri" / "target"
    if args.target:
        target = target / args.target
    bundles = target / "release" / "bundle"
    env = dict(os.environ)
    env["TAURI_SIGNING_PRIVATE_KEY"] = key.read_text(encoding="utf-8")
    env["TAURI_SIGNING_PRIVATE_KEY_PASSWORD"] = ""
    env["APPIMAGE_EXTRACT_AND_RUN"] = "1"
    # A release build must be given its server. The drill's builds are given
    # one that never resolves (`.invalid`, RFC 2606): they reach no one.
    env["JAMJAM_SERVER_URL"] = "https://update-drill.invalid"
    for version, name in ((VERSION_A, "a"), (VERSION_B, "b")):
        conf = out / f"{name}.conf.json"
        conf.write_text(json.dumps(settings(version, pubkey, args.port), indent=2), encoding="utf-8")
        # Only this build's bundle is found there afterwards.
        shutil.rmtree(bundles, ignore_errors=True)
        command = ["cargo", "tauri", "build", "--config", str(conf), "--features", "update-drill", "--bundles", flag]
        if args.target:
            command += ["--target", args.target]
        say(f"building {version}: {' '.join(command)}")
        subprocess.run(command, cwd=REPO, check=True, env=env)
        shutil.rmtree(out / name, ignore_errors=True)
        shutil.copytree(bundles / subdir, out / name / subdir, symlinks=True)
    say(f"built: {out / 'a'} and {out / 'b'}")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    b = commands.add_parser("build", help=f"make the {VERSION_A} and {VERSION_B} builds")
    b.add_argument("--out", required=True, help="where the builds' bundles are put (<out>/a, <out>/b)")
    b.add_argument("--target", help="the Rust target, when it is not the host's")
    b.add_argument("--port", type=int, default=DEFAULT_PORT)
    b.set_defaults(func=build)
    r = commands.add_parser("run", help="run the drill")
    r.add_argument("--a", required=True, help=f"bundle directory of the {VERSION_A} build")
    r.add_argument("--b", required=True, help=f"bundle directory of the {VERSION_B} build")
    r.add_argument("--port", type=int, default=DEFAULT_PORT)
    r.add_argument("--scenario", action="append", choices=SCENARIOS, help="only these (repeatable)")
    r.set_defaults(func=run)
    args = parser.parse_args()
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main())
