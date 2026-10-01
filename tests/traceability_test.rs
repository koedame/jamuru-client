//! Requirement traceability checks (ADR-018)
//!
//! Enforces the one rule that keeps the right-hand side of the V from going
//! hollow: every requirement marked `@must` has at least one test that actually
//! asserts something, and every `Verifies:` annotation points at a requirement
//! that exists.
//!
//! Requirements are defined in two places:
//!
//! - `docs-spec/requirements.md` - product and design requirements (`-0NN`)
//! - `docs-spec/behavior/*.feature` - behaviour scenarios (`-1NN`), tagged
//!   `@REQ-...` plus `@must` or `@should`
//!
//! Tests declare what they cover with `Verifies: REQ-XXX-NNN` in the doc comment
//! immediately above the test function.
//!
//! Some requirements can only be verified by connecting clients through a
//! running signaling server, which this repository does not contain. Those are
//! listed under [`SERVER_VERIFIED_HEADING`] in `docs-spec/requirements.md` and
//! are verified by connection tests kept next to the server; here they are
//! exempt from the `must` check and marked as such in the matrix.
//!
//! The generated matrix is written to `docs-spec/traceability.md` and compared
//! against the committed copy, so any change in coverage has to be reviewed.
//! Regenerate with:
//!
//! ```text
//! JAMJAM_UPDATE_TRACEABILITY=1 cargo test --test traceability_test
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

/// Trees scanned for `Verifies:` annotations, relative to the repository root
const SOURCE_ROOTS: &[(&str, &[&str])] = &[
    ("src", &["rs"]),
    ("src-tauri/src", &["rs"]),
    ("tests", &["rs"]),
    ("tests/e2e/src", &["rs"]),
    ("ui/src", &["ts", "tsx"]),
];

/// Directories never scanned: build output and vendored dependencies
const SKIP_DIRS: &[&str] = &["target", "node_modules", "dist", "fixtures"];

/// Tokens that count as an assertion in Rust
const RUST_ASSERTIONS: &[&str] = &[
    "assert!",
    "assert_eq!",
    "assert_ne!",
    "assert_matches!",
    "panic!",
    "unreachable!",
];

/// Tokens that count as an assertion in TypeScript
const TS_ASSERTIONS: &[&str] = &["expect(", "assert("];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Criticality {
    Must,
    Should,
}

impl Criticality {
    fn parse(token: &str) -> Option<Self> {
        match token {
            "must" => Some(Criticality::Must),
            "should" => Some(Criticality::Should),
            _ => None,
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            Criticality::Must => "must",
            Criticality::Should => "should",
        }
    }
}

#[derive(Debug, Clone)]
struct Requirement {
    id: String,
    criticality: Criticality,
    description: String,
    /// Repo-relative path of the file that defines the requirement
    defined_in: String,
}

#[derive(Debug, Clone)]
struct Verification {
    requirement_id: String,
    /// Repo-relative path of the file holding the test
    file: String,
    /// Name of the test function, or the `it(...)` description
    test_name: String,
    /// Whether the test body contains an assertion
    has_assertion: bool,
    /// 1-indexed line of the annotation, for error messages
    line: usize,
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("cannot read {}: {}", path.display(), e))
}

fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

// ---------------------------------------------------------------------------
// Parsing the left-hand side of the V
// ---------------------------------------------------------------------------

fn looks_like_requirement_id(token: &str) -> bool {
    let mut parts = token.split('-');
    parts.next() == Some("REQ")
        && parts.next().is_some_and(|domain| {
            !domain.is_empty()
                && domain
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        })
        && parts
            .next()
            .is_some_and(|number| number.len() == 3 && number.chars().all(|c| c.is_ascii_digit()))
        && parts.next().is_none()
}

/// Heading in `docs-spec/requirements.md` whose `- REQ-...` list names the
/// requirements verified by connection tests run against a signaling server.
const SERVER_VERIFIED_HEADING: &str = "## シグナリングサーバーを立てて検証する要求";

/// The requirement IDs listed under [`SERVER_VERIFIED_HEADING`]
fn parse_server_verified(root: &Path) -> Vec<String> {
    let contents = read(&root.join("docs-spec/requirements.md"));
    contents
        .lines()
        .skip_while(|line| line.trim() != SERVER_VERIFIED_HEADING)
        .skip(1)
        .take_while(|line| !line.starts_with("## "))
        .filter_map(|line| line.trim().strip_prefix("- "))
        .filter_map(|item| item.split_whitespace().next())
        .map(|id| id.trim_matches('`').to_string())
        .collect()
}

/// Requirements defined by the markdown tables in `docs-spec/requirements.md`
fn parse_requirements_md(root: &Path) -> Vec<Requirement> {
    let path = root.join("docs-spec/requirements.md");
    let contents = read(&path);
    let defined_in = rel(root, &path);
    let mut requirements = Vec::new();

    for line in contents.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("| REQ-") {
            continue;
        }

        let cells: Vec<&str> = trimmed
            .trim_matches('|')
            .split('|')
            .map(str::trim)
            .collect();
        let Some(id) = cells.first() else { continue };
        if !looks_like_requirement_id(id) {
            continue;
        }

        let criticality = cells
            .iter()
            .find_map(|cell| Criticality::parse(cell))
            .unwrap_or_else(|| {
                panic!(
                    "{}: requirement {} has no must/should column",
                    defined_in, id
                )
            });

        requirements.push(Requirement {
            id: (*id).to_string(),
            criticality,
            description: cells.get(1).copied().unwrap_or_default().to_string(),
            defined_in: defined_in.clone(),
        });
    }

    requirements
}

/// Requirements defined by tagged Scenarios in `docs-spec/behavior/*.feature`
///
/// Also returns any tagging problems found, so they can all be reported at once.
fn parse_feature_files(root: &Path) -> (Vec<Requirement>, Vec<String>) {
    let dir = root.join("docs-spec/behavior");
    let mut requirements = Vec::new();
    let mut problems = Vec::new();

    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {}", dir.display(), e))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "feature"))
        .collect();
    files.sort();

    for path in files {
        let contents = read(&path);
        let defined_in = rel(root, &path);
        let lines: Vec<&str> = contents.lines().collect();

        // Pending tags accumulated from the line(s) above a Scenario.
        let mut pending: Option<(Vec<String>, Vec<Criticality>, usize)> = None;

        for (index, line) in lines.iter().enumerate() {
            let trimmed = line.trim();

            if trimmed.starts_with('@') {
                let mut ids = Vec::new();
                let mut criticalities = Vec::new();
                for token in trimmed.split_whitespace() {
                    let token = token.trim_start_matches('@');
                    if looks_like_requirement_id(token) {
                        ids.push(token.to_string());
                    } else if let Some(c) = Criticality::parse(token) {
                        criticalities.push(c);
                    } else {
                        problems.push(format!(
                            "{}:{}: unknown tag @{}",
                            defined_in,
                            index + 1,
                            token
                        ));
                    }
                }
                pending = Some((ids, criticalities, index + 1));
                continue;
            }

            if trimmed.starts_with("Scenario:") {
                let title = trimmed.trim_start_matches("Scenario:").trim().to_string();
                match pending.take() {
                    None => problems.push(format!(
                        "{}:{}: Scenario \"{}\" has no @REQ-* tag",
                        defined_in,
                        index + 1,
                        title
                    )),
                    Some((ids, criticalities, tag_line)) => {
                        if ids.len() != 1 {
                            problems.push(format!(
                                "{}:{}: Scenario \"{}\" must carry exactly one @REQ-* tag, found {}",
                                defined_in,
                                tag_line,
                                title,
                                ids.len()
                            ));
                        }
                        if criticalities.len() != 1 {
                            problems.push(format!(
                                "{}:{}: Scenario \"{}\" must carry exactly one @must/@should tag, found {}",
                                defined_in,
                                tag_line,
                                title,
                                criticalities.len()
                            ));
                        }
                        if let (Some(id), Some(criticality)) = (ids.first(), criticalities.first())
                        {
                            requirements.push(Requirement {
                                id: id.clone(),
                                criticality: *criticality,
                                description: title,
                                defined_in: defined_in.clone(),
                            });
                        }
                    }
                }
                continue;
            }

            // A tag block must be immediately followed by the Scenario it
            // annotates; anything else means the tag is orphaned.
            if !trimmed.is_empty() && pending.is_some() && !trimmed.starts_with('#') {
                let (ids, _, tag_line) = pending.take().expect("checked above");
                problems.push(format!(
                    "{}:{}: tag {:?} is not attached to a Scenario",
                    defined_in, tag_line, ids
                ));
            }
        }
    }

    (requirements, problems)
}

// ---------------------------------------------------------------------------
// Parsing the right-hand side of the V
// ---------------------------------------------------------------------------

fn collect_source_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();

    for (dir, extensions) in SOURCE_ROOTS {
        let base = root.join(dir);
        if base.is_dir() {
            walk(&base, extensions, &mut files);
        }
    }

    files.sort();
    files.dedup();
    files
}

fn walk(dir: &Path, extensions: &[&str], out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();

        if path.is_dir() {
            if !SKIP_DIRS.contains(&name.as_str()) && !name.starts_with('.') {
                walk(&path, extensions, out);
            }
        } else if path
            .extension()
            .is_some_and(|ext| extensions.contains(&ext.to_string_lossy().as_ref()))
        {
            out.push(path);
        }
    }
}

/// How far below an annotation the test declaration may sit
///
/// An annotation belongs to the test immediately below it, allowing for the rest
/// of a doc comment and any attributes. Without a bound, a stray `Verifies:` in
/// prose would bind to some distant function and be credited with that
/// function's assertions.
const MAX_LINES_TO_DECLARATION: usize = 12;

/// Name of the test that follows `start`, plus whether its body asserts anything
///
/// Relies on rustfmt/prettier formatting: the body ends at the first line that
/// closes the block at the same indentation as the declaration.
fn test_after(lines: &[&str], start: usize, is_rust: bool) -> Option<(String, bool)> {
    let declaration = lines
        .iter()
        .enumerate()
        .skip(start)
        .take(MAX_LINES_TO_DECLARATION)
        .find(|(_, line)| {
            let trimmed = line.trim_start();
            if is_rust {
                trimmed.starts_with("fn ") || trimmed.starts_with("async fn ")
            } else {
                trimmed.starts_with("it(") || trimmed.starts_with("it.each")
            }
        })?;

    let (declaration_index, declaration_line) = declaration;
    let indent = declaration_line.len() - declaration_line.trim_start().len();

    let name = if is_rust {
        declaration_line
            .trim_start()
            .trim_start_matches("async ")
            .trim_start_matches("fn ")
            .split('(')
            .next()
            .unwrap_or_default()
            .trim()
            .to_string()
    } else {
        // Take the first quoted string at or after the declaration. That is the
        // description in `it('name', ...)`, in `it.each(cases)('name', ...)`, and
        // in the multi-line `it.each([\n  ...\n])('name', ...)` form, where the
        // declaration line itself holds no quote.
        lines
            .iter()
            .skip(declaration_index)
            .take(MAX_LINES_TO_DECLARATION)
            .find_map(|line| {
                let start = line.find(['\'', '"', '`'])?;
                let quote = line.as_bytes()[start] as char;
                Some(
                    line[start + 1..]
                        .split(quote)
                        .next()
                        .unwrap_or_default()
                        .to_string(),
                )
            })
            .unwrap_or_else(|| declaration_line.trim_start().to_string())
    };

    let assertions = if is_rust {
        RUST_ASSERTIONS
    } else {
        TS_ASSERTIONS
    };
    let mut has_assertion = false;

    for line in lines.iter().skip(declaration_index + 1) {
        let trimmed = line.trim();
        let line_indent = line.len() - line.trim_start().len();

        // End of the block at the declaration's own indentation.
        if !trimmed.is_empty()
            && line_indent == indent
            && (trimmed == "}" || trimmed == "});" || trimmed == "}," || trimmed == "})")
        {
            break;
        }

        if assertions.iter().any(|token| trimmed.contains(token)) {
            has_assertion = true;
        }
    }

    Some((name, has_assertion))
}

fn collect_verifications(root: &Path) -> Vec<Verification> {
    let mut verifications = Vec::new();

    for path in collect_source_files(root) {
        // The checker itself mentions the annotation format in prose.
        if path.ends_with("traceability_test.rs") {
            continue;
        }

        let contents = read(&path);
        if !contents.contains("Verifies:") {
            continue;
        }

        let file = rel(root, &path);
        let is_rust = path.extension().is_some_and(|ext| ext == "rs");
        let lines: Vec<&str> = contents.lines().collect();

        for (index, line) in lines.iter().enumerate() {
            // Only a comment line that *begins* with the marker is an
            // annotation; prose that merely mentions it is not.
            let stripped = line
                .trim()
                .trim_start_matches("///")
                .trim_start_matches("//!")
                .trim_start_matches("//")
                .trim_start_matches("/**")
                .trim_start_matches('*')
                .trim_end_matches("*/")
                .trim();
            let Some(after) = stripped.strip_prefix("Verifies:") else {
                continue;
            };

            let ids: Vec<&str> = after
                .split(|c: char| c == ',' || c.is_whitespace())
                .map(str::trim)
                .filter(|token| !token.is_empty())
                .collect();

            for id in ids {
                if !looks_like_requirement_id(id) {
                    panic!(
                        "{}:{}: \"{}\" is not a requirement ID; write `Verifies: REQ-DOMAIN-NNN`",
                        file,
                        index + 1,
                        id
                    );
                }

                let (test_name, has_assertion) = test_after(&lines, index, is_rust)
                    .unwrap_or_else(|| ("<no test found>".to_string(), false));

                verifications.push(Verification {
                    requirement_id: id.to_string(),
                    file: file.clone(),
                    test_name,
                    has_assertion,
                    line: index + 1,
                });
            }
        }
    }

    verifications
}

// ---------------------------------------------------------------------------
// Matrix rendering
// ---------------------------------------------------------------------------

fn render_matrix(
    requirements: &BTreeMap<String, Requirement>,
    by_requirement: &BTreeMap<String, Vec<Verification>>,
    server_verified: &BTreeSet<String>,
) -> String {
    let mut out = String::new();

    out.push_str("# トレーサビリティ対応表\n\n");
    out.push_str("> このファイルは `tests/traceability_test.rs` が生成する。手で編集しない。\n");
    out.push_str(
        "> 再生成: `JAMJAM_UPDATE_TRACEABILITY=1 cargo test --test traceability_test`\n\n",
    );
    out.push_str("要求 ID の体系と criticality の定義は [requirements.md](./requirements.md)、");
    out.push_str(
        "運用ルールは [ADR-018](./adr/ADR-018-iterative-v-model-traceability.md) を参照。\n\n",
    );

    let total = requirements.len();
    let musts = requirements
        .values()
        .filter(|r| r.criticality == Criticality::Must)
        .count();
    let verified = requirements
        .keys()
        .filter(|id| by_requirement.contains_key(*id))
        .count();
    let verified_with_a_server = requirements
        .keys()
        .filter(|id| !by_requirement.contains_key(*id) && server_verified.contains(*id))
        .count();
    let gaps: Vec<&Requirement> = requirements
        .values()
        .filter(|r| !by_requirement.contains_key(&r.id) && !server_verified.contains(&r.id))
        .collect();

    out.push_str("## 集計\n\n");
    out.push_str("| 項目 | 件数 |\n|------|------|\n");
    let _ = writeln!(out, "| 要求 総数 | {} |", total);
    let _ = writeln!(out, "| うち must | {} |", musts);
    let _ = writeln!(out, "| うち should | {} |", total - musts);
    let _ = writeln!(out, "| 検証済み | {} |", verified);
    let _ = writeln!(
        out,
        "| サーバーを立てた接続テストでのみ検証 | {} |",
        verified_with_a_server
    );
    let _ = writeln!(out, "| 未検証（should のみ許容） | {} |", gaps.len());
    out.push('\n');

    out.push_str("## 対応表\n\n");
    out.push_str("| 要求 ID | criticality | 内容 | 定義元 | 検証 |\n");
    out.push_str("|---------|-------------|------|--------|------|\n");

    for (id, requirement) in requirements {
        let verification = match by_requirement.get(id) {
            None if server_verified.contains(id) => {
                "サーバーを立てた接続テスト（このリポジトリの外）".to_string()
            }
            None => "**未検証**".to_string(),
            Some(list) => {
                let mut rendered: Vec<String> = list
                    .iter()
                    .map(|v| format!("`{}`::{}", v.file, v.test_name))
                    .collect();
                rendered.sort();
                rendered.dedup();
                rendered.join("<br/>")
            }
        };

        let _ = writeln!(
            out,
            "| {} | {} | {} | `{}` | {} |",
            id,
            requirement.criticality.as_str(),
            requirement.description,
            requirement.defined_in,
            verification
        );
    }

    out.push('\n');
    out.push_str("## 未検証の要求（ギャップ）\n\n");

    if gaps.is_empty() {
        out.push_str("なし。\n");
    } else {
        out.push_str("`should` の要求は未検証を許容する。解消は Plans.md で管理する。\n");
        out.push_str(
            "実環境（実回線・別マシン・実機）を要するものは Plans.md「実環境待ち」にまとめてある。\n\n",
        );
        out.push_str("| 要求 ID | 内容 | 定義元 |\n|---------|------|--------|\n");
        for requirement in gaps {
            let _ = writeln!(
                out,
                "| {} | {} | `{}` |",
                requirement.id, requirement.description, requirement.defined_in
            );
        }
    }

    out
}

// ---------------------------------------------------------------------------
// The checks
// ---------------------------------------------------------------------------

struct Traceability {
    requirements: BTreeMap<String, Requirement>,
    by_requirement: BTreeMap<String, Vec<Verification>>,
    server_verified: BTreeSet<String>,
    verifications: Vec<Verification>,
    problems: Vec<String>,
}

fn load() -> Traceability {
    let root = repo_root();

    let mut problems = Vec::new();
    let (feature_requirements, feature_problems) = parse_feature_files(&root);
    problems.extend(feature_problems);

    let mut requirements: BTreeMap<String, Requirement> = BTreeMap::new();
    for requirement in parse_requirements_md(&root)
        .into_iter()
        .chain(feature_requirements)
    {
        if let Some(existing) = requirements.get(&requirement.id) {
            problems.push(format!(
                "{} is defined twice: {} and {}",
                requirement.id, existing.defined_in, requirement.defined_in
            ));
            continue;
        }
        requirements.insert(requirement.id.clone(), requirement);
    }

    let verifications = collect_verifications(&root);
    let mut by_requirement: BTreeMap<String, Vec<Verification>> = BTreeMap::new();
    for verification in &verifications {
        if !requirements.contains_key(&verification.requirement_id) {
            continue; // reported separately as a dangling reference
        }
        if !verification.has_assertion {
            continue; // reported separately as a hollow test
        }
        by_requirement
            .entry(verification.requirement_id.clone())
            .or_default()
            .push(verification.clone());
    }

    let mut server_verified = BTreeSet::new();
    for id in parse_server_verified(&root) {
        if !requirements.contains_key(&id) {
            problems.push(format!(
                "{} is listed under \"{}\" but is not defined",
                id, SERVER_VERIFIED_HEADING
            ));
        }
        if !server_verified.insert(id.clone()) {
            problems.push(format!(
                "{} is listed twice under \"{}\"",
                id, SERVER_VERIFIED_HEADING
            ));
        }
    }

    Traceability {
        requirements,
        by_requirement,
        server_verified,
        verifications,
        problems,
    }
}

/// Every Scenario carries exactly one requirement ID and one criticality, and
/// no ID is defined twice.
#[test]
fn requirement_definitions_are_well_formed() {
    let traceability = load();

    assert!(
        traceability.problems.is_empty(),
        "requirement definitions have problems:\n  {}",
        traceability.problems.join("\n  ")
    );
    assert!(
        !traceability.requirements.is_empty(),
        "no requirements were found - is docs-spec/requirements.md readable?"
    );
}

/// A `Verifies:` annotation must name a requirement that exists. A typo or a
/// deleted requirement would otherwise silently stop verifying anything.
#[test]
fn no_test_verifies_an_unknown_requirement() {
    let traceability = load();

    let dangling: Vec<String> = traceability
        .verifications
        .iter()
        .filter(|v| !traceability.requirements.contains_key(&v.requirement_id))
        .map(|v| {
            format!(
                "{}:{} ({}) references undefined {}",
                v.file, v.line, v.test_name, v.requirement_id
            )
        })
        .collect();

    assert!(
        dangling.is_empty(),
        "tests reference requirements that are not defined:\n  {}",
        dangling.join("\n  ")
    );
}

/// A test that claims to verify a requirement must actually assert something.
#[test]
fn no_verifying_test_is_hollow() {
    let traceability = load();

    let hollow: Vec<String> = traceability
        .verifications
        .iter()
        .filter(|v| !v.has_assertion)
        .map(|v| {
            format!(
                "{}:{} {} claims to verify {} but its body has no assertion",
                v.file, v.line, v.test_name, v.requirement_id
            )
        })
        .collect();

    assert!(
        hollow.is_empty(),
        "hollow verifications found (see .claude/rules/test-quality.md):\n  {}",
        hollow.join("\n  ")
    );
}

/// Every `must` requirement is verified by at least one asserting test, here
/// or - for the ones listed as needing a signaling server - by the connection
/// tests kept next to the server.
#[test]
fn every_must_requirement_is_verified() {
    let traceability = load();

    let unverified: Vec<String> = traceability
        .requirements
        .values()
        .filter(|r| r.criticality == Criticality::Must)
        .filter(|r| !traceability.by_requirement.contains_key(&r.id))
        .filter(|r| !traceability.server_verified.contains(&r.id))
        .map(|r| format!("{} ({}) defined in {}", r.id, r.description, r.defined_in))
        .collect();

    assert!(
        unverified.is_empty(),
        "must-level requirements without a verifying test:\n  {}\n\n\
         Either add a test annotated `Verifies: <ID>`, or downgrade the \
         requirement to @should and record the gap.",
        unverified.join("\n  ")
    );
}

/// The committed matrix matches what the code and specs currently say.
#[test]
fn traceability_matrix_is_up_to_date() {
    let root = repo_root();
    let traceability = load();
    let generated = render_matrix(
        &traceability.requirements,
        &traceability.by_requirement,
        &traceability.server_verified,
    );
    let path = root.join("docs-spec/traceability.md");

    if std::env::var("JAMJAM_UPDATE_TRACEABILITY").is_ok() {
        fs::write(&path, &generated).expect("cannot write traceability.md");
        return;
    }

    let committed = fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(
        committed.trim_end(),
        generated.trim_end(),
        "docs-spec/traceability.md is stale. Regenerate and review:\n  \
         JAMJAM_UPDATE_TRACEABILITY=1 cargo test --test traceability_test"
    );
}
