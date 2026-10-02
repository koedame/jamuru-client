//! Every relative link in `docs/` and `docs-spec/` must point at a file that
//! exists.
//!
//! A renamed ADR or a moved guide leaves the links to it dangling, and nothing
//! else notices: the documents are plain Markdown that no build reads.

use std::path::{Path, PathBuf};

fn markdown_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {}", dir.display(), e)) {
        let path = entry.unwrap().path();
        if path.is_dir() {
            markdown_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "md") {
            out.push(path);
        }
    }
}

/// Link targets of the form `](target)` outside code fences, with the line
/// number they are on.
fn link_targets(text: &str) -> Vec<(usize, String)> {
    let mut targets = Vec::new();
    let mut in_fence = false;
    for (index, line) in text.lines().enumerate() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let mut rest = line;
        while let Some((_, after)) = rest.split_once("](") {
            let Some((target, tail)) = after.split_once(')') else {
                break;
            };
            targets.push((index + 1, target.to_string()));
            rest = tail;
        }
    }
    targets
}

#[test]
fn relative_links_in_docs_point_at_existing_files() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    markdown_files(&root.join("docs"), &mut files);
    markdown_files(&root.join("docs-spec"), &mut files);
    assert!(!files.is_empty(), "no Markdown files found");

    let mut broken = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file).unwrap();
        for (line, target) in link_targets(&text) {
            let path = target.split('#').next().unwrap();
            if path.is_empty() || path.contains("://") || path.starts_with("mailto:") {
                continue;
            }
            let resolved = file.parent().unwrap().join(path);
            if !resolved.exists() {
                broken.push(format!(
                    "{}:{}: {}",
                    file.strip_prefix(&root).unwrap().display(),
                    line,
                    target
                ));
            }
        }
    }
    assert!(
        broken.is_empty(),
        "broken links:\n  {}",
        broken.join("\n  ")
    );
}
