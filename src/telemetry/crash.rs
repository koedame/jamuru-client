//! Where the app crashed, kept for the next launch.
//!
//! A panic cannot count on the network, so the hook writes a small file and
//! the next launch sends it. Only the place is kept - file, line, function.
//! The panic message is not, as it can carry paths, room codes or addresses.

use std::fs;
use std::panic;
use std::path::Path;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use super::collector::UsageReporter;
use super::event::{format_ts, Crash};
use super::install::{is_valid_id, CRASH_FILE};

/// The crash record on disk.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct CrashFile {
    pub ts: String,
    /// The launch that crashed, so its `app_start` and this line can be joined.
    pub launch_id: String,
    pub app_version: String,
    pub file: String,
    pub line: u32,
    pub function: Option<String>,
}

impl CrashFile {
    /// The `crash` event, or `None` when the file names a place that is not
    /// a plain source location (a hand-edited file must not be a way to send
    /// free text).
    pub(crate) fn event(&self) -> Option<Crash> {
        Some(Crash {
            file: clean_file(&self.file)?,
            line: self.line,
            function: self.function.as_deref().and_then(clean_function),
        })
    }

    pub(crate) fn has_valid_ids(&self) -> bool {
        is_valid_id(&self.launch_id)
            && !self.app_version.is_empty()
            && self.app_version.len() <= 32
            && self
                .app_version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"+.-".contains(&b))
    }
}

pub(crate) fn write(dir: &Path, crash: &CrashFile) {
    let Ok(json) = serde_json::to_string(crash) else {
        return;
    };
    let _ = fs::create_dir_all(dir);
    let _ = fs::write(dir.join(CRASH_FILE), json);
}

/// Reads and removes the pending crash record.
pub(crate) fn take(dir: &Path) -> Option<CrashFile> {
    let path = dir.join(CRASH_FILE);
    let content = fs::read_to_string(&path).ok();
    let _ = fs::remove_file(&path);
    serde_json::from_str(&content?).ok()
}

/// The file as it may be sent: relative to the crate for our own code, the
/// bare file name for anything outside it (an absolute path names the user's
/// home directory).
fn clean_file(path: &str) -> Option<String> {
    let path = path.replace('\\', "/");
    let shown = if path.starts_with('/') || path.chars().nth(1) == Some(':') {
        path.rsplit('/').next()?.to_string()
    } else {
        path.trim_start_matches("./").to_string()
    };
    let ok = !shown.is_empty()
        && shown.len() <= 120
        && !shown.contains("..")
        && shown
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_./-".contains(&b));
    ok.then_some(shown)
}

/// A function path with only the characters the schema allows: the
/// `{{closure}}` marker and generic brackets are reduced to their letters.
fn clean_function(name: &str) -> Option<String> {
    let cleaned: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '<' | '>'))
        .take(200)
        .collect();
    (!cleaned.is_empty()).then_some(cleaned)
}

/// The innermost frame of our own code in a captured backtrace, without the
/// hash the compiler appends.
fn function_from_backtrace(backtrace: &str) -> Option<String> {
    backtrace.lines().find_map(|line| {
        let (index, symbol) = line.trim().split_once(": ")?;
        if index.is_empty() || !index.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let ours = symbol.starts_with("jamjam::") || symbol.starts_with("jamjam_app_lib::");
        if !ours || symbol.contains("::telemetry::") {
            return None;
        }
        let without_hash = match symbol.rsplit_once("::h") {
            Some((head, hash))
                if hash.len() == 16 && hash.bytes().all(|b| b.is_ascii_hexdigit()) =>
            {
                head
            }
            _ => symbol,
        };
        Some(without_hash.to_string())
    })
}

/// Makes a panic leave a crash record, while the reporter is on. The hook
/// that was there before still runs, so the diagnostic log keeps its line.
pub(crate) fn install_hook(reporter: &UsageReporter) {
    let reporter = reporter.clone();
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        if let (true, Some(dir), Some(location)) =
            (reporter.is_enabled(), reporter.state_dir(), info.location())
        {
            let backtrace = std::backtrace::Backtrace::force_capture().to_string();
            write(
                dir,
                &CrashFile {
                    ts: format_ts(Utc::now()),
                    launch_id: reporter.launch_id().to_string(),
                    app_version: reporter.app_version().to_string(),
                    file: location.file().to_string(),
                    line: location.line(),
                    function: function_from_backtrace(&backtrace),
                },
            );
        }
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_is_the_bare_name_when_the_path_is_absolute() {
        assert_eq!(
            clean_file("/home/alice/.cargo/registry/src/x/tokio-1/src/io.rs").as_deref(),
            Some("io.rs")
        );
        assert_eq!(
            clean_file(r"C:\Users\alice\.cargo\lib.rs").as_deref(),
            Some("lib.rs")
        );
    }

    #[test]
    fn the_file_stays_relative_when_it_is_in_the_crate() {
        assert_eq!(
            clean_file("src/network/connection.rs").as_deref(),
            Some("src/network/connection.rs")
        );
    }

    #[test]
    fn the_file_is_rejected_when_it_is_not_a_plain_path() {
        assert_eq!(clean_file("src/../secret.rs"), None);
        assert_eq!(clean_file("send me @ alice.rs"), None);
        assert_eq!(clean_file(""), None);
    }

    #[test]
    fn the_function_loses_the_compiler_hash_and_the_closure_braces() {
        let backtrace = "   0: std::panicking::begin_panic\n   \
                         1: jamjam::network::connection::poll::{{closure}}::h0123456789abcdef\n             \
                         at ./src/network/connection.rs:10:5\n";
        let function = function_from_backtrace(backtrace).unwrap();
        assert_eq!(function, "jamjam::network::connection::poll::{{closure}}");
        assert_eq!(
            clean_function(&function).as_deref(),
            Some("jamjam::network::connection::poll::closure")
        );
    }

    #[test]
    fn the_function_is_none_when_no_frame_is_ours() {
        assert_eq!(
            function_from_backtrace("   0: std::panicking::begin_panic\n   1: main\n"),
            None
        );
    }
}
