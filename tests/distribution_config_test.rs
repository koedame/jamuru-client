//! Guards the app settings that break or cannot be changed once the app is
//! in users' hands (ADR-029).
//!
//! None of these show up while developing: `cargo tauri dev` runs with the
//! terminal's microphone permission, the identifier only matters once data
//! has been written under it, and a missing CSP changes nothing until
//! something hostile is rendered. So they are pinned here, on every
//! `cargo test`, by reading the files the bundler reads.

use std::path::{Path, PathBuf};

use jamjam::config::{DEFAULT_SERVER_URL, DEV_SERVER_URL, RELEASE_SERVER_URL};
use serde_json::Value;

/// The rule the app's build script enforces on the server a release is given.
#[path = "../src-tauri/release_server_url.rs"]
mod release_server_url;
use release_server_url::release_server_url_problem;

fn src_tauri(file: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src-tauri")
        .join(file);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("could not read {}: {}", path.display(), e))
}

fn tauri_conf() -> Value {
    serde_json::from_str(&src_tauri("tauri.conf.json")).expect("tauri.conf.json is not valid JSON")
}

/// Returns the `<string>` value paired with `key` in a flat plist `<dict>`.
fn plist_string(plist: &str, key: &str) -> Option<String> {
    let after_key = plist.split_once(&format!("<key>{}</key>", key))?.1;
    let value = after_key.trim_start().strip_prefix("<string>")?;
    value.split_once("</string>").map(|(v, _)| v.to_string())
}

/// Returns the sources listed for `directive`, whichever form the CSP takes.
fn csp_sources(csp: &Value, directive: &str) -> Vec<String> {
    let value = match csp {
        Value::Object(map) => map.get(directive).cloned(),
        Value::String(policy) => policy.split(';').map(str::trim).find_map(|d| {
            d.strip_prefix(directive)
                .map(|rest| Value::from(rest.trim()))
        }),
        _ => None,
    };
    match value {
        Some(Value::String(s)) => s.split_whitespace().map(str::to_string).collect(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

/// macOS terminates an app that touches the microphone without this
/// description. Tauri merges `src-tauri/Info.plist` into the bundle.
///
/// Verifies: REQ-DIST-001
#[test]
fn macos_bundle_declares_why_it_uses_the_microphone() {
    let description = plist_string(&src_tauri("Info.plist"), "NSMicrophoneUsageDescription")
        .expect("src-tauri/Info.plist does not declare NSMicrophoneUsageDescription");

    assert!(
        !description.trim().is_empty(),
        "NSMicrophoneUsageDescription is empty; macOS shows it in the permission prompt"
    );
}

/// Verifies: REQ-DIST-002
#[test]
fn identifier_is_the_one_released_under() {
    assert_eq!(
        tauri_conf()["identifier"],
        "me.koeda.jamjam",
        "the identifier decides where the webview keeps user data; changing it \
         after release loses that data"
    );
}

/// Verifies: REQ-DIST-003
#[test]
fn csp_is_enabled() {
    let csp = &tauri_conf()["app"]["security"]["csp"];

    assert!(
        !csp.is_null(),
        "app.security.csp is null, so the webview runs without a CSP"
    );
    assert_eq!(csp_sources(csp, "default-src"), vec!["'self'"]);
}

/// Scripts are the part that matters: an injected script can call every
/// Tauri command. Only the bundled ones may run.
///
/// Verifies: REQ-DIST-003
#[test]
fn csp_allows_only_bundled_scripts() {
    let csp = &tauri_conf()["app"]["security"]["csp"];

    assert_eq!(csp_sources(csp, "script-src"), vec!["'self'"]);
    assert_eq!(csp_sources(csp, "object-src"), vec!["'none'"]);
}

/// Nothing in the webview talks to the network - signalling and audio run
/// in the Rust side - so no directive may name a remote origin. The only
/// URL allowed is Tauri's IPC endpoint.
///
/// Verifies: REQ-DIST-003
#[test]
fn csp_names_no_remote_origin() {
    let csp = &tauri_conf()["app"]["security"]["csp"];
    let directives = [
        "default-src",
        "script-src",
        "style-src",
        "font-src",
        "img-src",
        "connect-src",
    ];

    for directive in directives {
        for source in csp_sources(csp, directive) {
            let remote = source == "*"
                || source.starts_with("http:")
                || source.starts_with("https:")
                || source.starts_with("ws:")
                || source.starts_with("wss:")
                || source.contains("unsafe-");
            assert!(
                !remote || source == "http://ipc.localhost",
                "{} allows {:?}",
                directive,
                source
            );
        }
    }
    assert!(
        csp_sources(csp, "connect-src").contains(&"ipc:".to_string()),
        "connect-src must keep Tauri's IPC reachable, or every command fails"
    );
}

/// The UI imports `@tauri-apps/api` as modules; exposing the same API as a
/// global only widens what a stray script could reach.
///
/// Verifies: REQ-DIST-004
#[test]
fn tauri_api_is_not_exposed_as_a_global() {
    assert_eq!(tauri_conf()["app"]["withGlobalTauri"], false);
}

/// Every capability file the bundler picks up from `src-tauri/capabilities/`.
fn capabilities() -> Vec<(String, Value)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src-tauri/capabilities");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("could not read {}: {}", dir.display(), e))
        .map(|entry| entry.expect("could not list capabilities").path())
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|path| {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("could not read {}: {}", path.display(), e));
            let value = serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("{name} is not valid JSON: {e}"));
            (name, value)
        })
        .collect()
}

/// A permission handed to the webview is something hostile markup could call
/// if it ever got past the CSP, so the list is pinned: granting another plugin
/// command means changing this test and saying why the UI needs it.
///
/// Verifies: REQ-DIST-006
#[test]
fn the_webview_is_granted_only_the_plugin_commands_the_ui_calls() {
    let mut granted: Vec<String> = capabilities()
        .iter()
        .flat_map(|(name, capability)| {
            capability["permissions"]
                .as_array()
                .unwrap_or_else(|| panic!("{name} has no permissions list"))
                .iter()
                .map(|permission| match permission {
                    Value::String(id) => id.clone(),
                    other => other["identifier"].as_str().unwrap_or_default().to_string(),
                })
                .collect::<Vec<_>>()
        })
        .collect();
    granted.sort();
    assert_eq!(
        granted,
        vec![
            "core:event:allow-listen",
            "core:event:allow-unlisten",
            "deep-link:allow-get-current",
        ]
    );
}

/// Verifies: REQ-DIST-006
#[test]
fn no_remote_page_is_given_ipc_access() {
    for (name, capability) in capabilities() {
        assert!(
            capability.get("remote").is_none(),
            "{name} opens the Tauri IPC to remote URLs"
        );
    }
}

fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    match authority.find(']') {
        Some(end) if authority.starts_with('[') => &authority[..=end],
        _ => authority.split(':').next().unwrap_or(authority),
    }
}

/// Every source file under `dir` with one of `extensions`.
fn source_files(dir: &Path, extensions: &[&str], out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("could not read {}: {}", dir.display(), e));
    for entry in entries {
        let path = entry.expect("unreadable directory entry").path();
        if path.is_dir() {
            source_files(&path, extensions, out);
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| extensions.contains(&e))
        {
            out.push(path);
        }
    }
}

/// A release build uses the server its builder passes in `JAMJAM_SERVER_URL`
/// and asks it where the signaling server is, so the core library - where the
/// default lives - carries no `https://` or `wss://` URL but example hosts.
///
/// Verifies: REQ-DIST-005
#[test]
fn the_source_names_no_production_server() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    source_files(&root.join("src"), &["rs"], &mut files);
    assert!(
        files.iter().any(|f| f.ends_with("src/config.rs")),
        "the scan did not reach src/config.rs"
    );

    for file in files {
        let source = std::fs::read_to_string(&file)
            .unwrap_or_else(|e| panic!("could not read {}: {}", file.display(), e));
        for url in urls_in(&source, &["https://", "wss://"]) {
            let host = host_of(&url);
            // A bare scheme (validation, messages) names no server.
            assert!(
                host.is_empty() || is_example_host(host),
                "{} names the server {:?}; a release takes it from JAMJAM_SERVER_URL",
                file.display(),
                url
            );
        }
    }
}

/// `src-tauri/build.rs` refuses to make a release without a server, or with
/// one that is not `https://` or that reaches the user's own machine however
/// it is spelled - the app would ship unable to connect.
///
/// Verifies: REQ-DIST-005
#[test]
fn a_release_is_only_given_a_remote_server_over_tls() {
    for url in [
        "https://jamjam.example.com",
        "https://jamjam.example.com:443/base",
        "https://[2001:db8::1]:8443",
    ] {
        assert_eq!(release_server_url_problem(url), None, "{} was refused", url);
    }
    for url in [
        "",
        "http://jamjam.example.com",
        "wss://jamjam.example.com",
        "https://",
        "https://localhost",
        "https://LOCALHOST:17890",
        "https://localhost./base",
        "https://app.localhost",
        "https://127.0.0.1",
        "https://127.0.0.2",
        "https://127.1",
        "https://0.0.0.0",
        "https://[::1]",
        "https://[0:0:0:0:0:0:0:1]:17890",
        "https://user@localhost",
    ] {
        assert!(
            release_server_url_problem(url).is_some(),
            "{:?} was accepted",
            url
        );
    }
}

/// `cargo tauri dev` and the GUI E2E suite build without optimisation and
/// must keep using a jamjam server on this machine, on the development port
/// 17890 that the GUI E2E suite's server occupies.
///
/// Verifies: REQ-DIST-005
#[test]
fn development_builds_use_the_local_server() {
    assert_eq!(host_of(DEV_SERVER_URL), "localhost");
    assert!(
        DEV_SERVER_URL.starts_with("http://") && DEV_SERVER_URL.ends_with(":17890"),
        "{:?} is not http:// on the development port 17890",
        DEV_SERVER_URL
    );
}

/// The default follows the build profile this test was compiled with, so
/// `cargo test` checks the development side and `cargo test --release` the
/// release side.
///
/// Verifies: REQ-DIST-005
#[test]
fn the_default_follows_the_build_profile() {
    let expected = if cfg!(debug_assertions) {
        DEV_SERVER_URL
    } else {
        RELEASE_SERVER_URL.unwrap_or("")
    };
    assert_eq!(DEFAULT_SERVER_URL, expected);
}

/// The server is decided in one place (`jamjam::config`), and the signaling
/// server by the server itself. A URL written into the UI or the Tauri
/// commands would be a second default that release builds could carry
/// unnoticed - which is how the UI once shipped a local signaling server.
///
/// Verifies: REQ-DIST-005
#[test]
fn no_other_source_names_a_server() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    source_files(&root.join("ui/src"), &["ts", "tsx"], &mut files);
    source_files(&root.join("src-tauri/src"), &["rs"], &mut files);
    assert!(
        files.len() > 10,
        "found only {} source files - the scan is not reading the app",
        files.len()
    );

    for file in files {
        let source = std::fs::read_to_string(&file)
            .unwrap_or_else(|e| panic!("could not read {}: {}", file.display(), e));
        for needle in ["ws://", "wss://", "VITE_SIGNALING_SERVER"] {
            assert!(
                !source.contains(needle),
                "{} contains {:?}; take the server from jamjam::config instead",
                file.display(),
                needle
            );
        }
        for url in urls_in(&source, &["http://", "https://"]) {
            let host = host_of(&url);
            assert!(
                is_example_host(host) || host == "www.w3.org",
                "{} names the server {:?}; take the server from jamjam::config instead",
                file.display(),
                url
            );
        }
    }
}

/// Every URL starting with one of `schemes` in `source`, up to the first
/// character that ends a URL in code or prose.
fn urls_in(source: &str, schemes: &[&str]) -> Vec<String> {
    let mut urls = Vec::new();
    for scheme in schemes {
        for (at, _) in source.match_indices(scheme) {
            urls.push(
                source[at..]
                    .chars()
                    .take_while(|c| !c.is_whitespace() && !matches!(c, '"' | '`' | '\'' | ')'))
                    .collect(),
            );
        }
    }
    urls
}

fn is_example_host(host: &str) -> bool {
    host == "example.com" || host.ends_with(".example.com")
}

#[test]
fn host_of_strips_scheme_port_and_path() {
    assert_eq!(
        host_of("wss://signaling.example.com"),
        "signaling.example.com"
    );
    assert_eq!(
        host_of("https://user@jamjam.example.com/base"),
        "jamjam.example.com"
    );
    assert_eq!(host_of("ws://localhost:17890/ws"), "localhost");
    assert_eq!(host_of("ws://[::1]:17890"), "[::1]");
}

/// Parser checks with hand-written input, so a change to the real files
/// cannot quietly make the assertions above vacuous.
#[test]
fn plist_string_reads_the_value_paired_with_the_key() {
    let plist = "<dict>\n\t<key>A</key>\n\t<string>first</string>\n\t<key>B</key>\n\t<string>second</string>\n</dict>";

    assert_eq!(plist_string(plist, "B").as_deref(), Some("second"));
    assert_eq!(plist_string(plist, "C"), None);
}

#[test]
fn csp_sources_reads_both_object_and_string_forms() {
    let object: Value = serde_json::json!({ "script-src": "'self' ipc:" });
    let string = Value::from("default-src 'self'; script-src 'self' ipc:");

    assert_eq!(csp_sources(&object, "script-src"), vec!["'self'", "ipc:"]);
    assert_eq!(csp_sources(&string, "script-src"), vec!["'self'", "ipc:"]);
    assert!(csp_sources(&object, "img-src").is_empty());
}
