//! The terms of use and the agreement to them on first launch.
//!
//! The text is `docs/terms.md`, bundled into the app so it can be read
//! offline, next to the LICENSE. Until the user has agreed to the version in
//! force (`jamjam::config::TERMS_VERSION`), nothing the app does on its own
//! reaches the network: the loops that connect, check for updates and report
//! usage wait on [`TermsState::wait_accepted`], and the commands that connect
//! refuse ([`require_accepted`]).

use jamjam::config::TERMS_VERSION;
use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime};
use tokio::sync::watch;

use crate::config::ConfigState;

/// The terms of use, as published in `docs/terms.md`.
const TERMS: &str = include_str!("../../docs/terms.md");

/// The license of the software, as shipped in `LICENSE`.
const LICENSE: &str = include_str!("../../LICENSE");

/// Where the privacy and security page lives, opened in the browser.
const PRIVACY_URL: &str =
    "https://github.com/koedame/jamjam-client/blob/main/docs/getting-started/privacy.md";

/// Where notices about ending or interrupting the service are posted, opened
/// in the browser.
const ANNOUNCEMENTS_URL: &str =
    "https://github.com/koedame/jamjam-client/blob/main/docs/announcements.md";

/// Tauri-managed state: whether the user has agreed to the terms in force.
pub struct TermsState {
    accepted: watch::Sender<bool>,
}

impl TermsState {
    pub fn new(accepted: bool) -> Self {
        Self {
            accepted: watch::channel(accepted).0,
        }
    }

    pub fn is_accepted(&self) -> bool {
        *self.accepted.borrow()
    }

    pub(crate) fn accept(&self) {
        self.accepted.send_replace(true);
    }

    /// Returns once the terms are agreed to; at once if they already are.
    pub async fn wait_accepted(&self) {
        let mut accepted = self.accepted.subscribe();
        // The sender lives as long as `self`, so waiting cannot fail.
        let _ = accepted.wait_for(|accepted| *accepted).await;
    }
}

/// Waits until the user has agreed to the terms. For the loops that start
/// with the app and talk to the network on their own.
pub async fn wait_accepted<R: Runtime>(app: &AppHandle<R>) {
    app.state::<TermsState>().wait_accepted().await;
}

/// Refuses a command that would talk to the network before the user has
/// agreed to the terms.
pub fn require_accepted<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    if app.state::<TermsState>().is_accepted() {
        Ok(())
    } else {
        Err("The terms of use have not been agreed to".to_string())
    }
}

/// What the screen needs to ask for agreement.
#[derive(Debug, Clone, Serialize)]
pub struct TermsInfo {
    /// The version in force.
    pub version: u32,
    /// Whether the user has agreed to that version on this device.
    pub accepted: bool,
    /// The text, in Markdown.
    pub text: &'static str,
}

/// The terms in force and whether they were agreed to.
#[tauri::command]
pub fn terms_get(state: tauri::State<'_, TermsState>) -> TermsInfo {
    TermsInfo {
        version: TERMS_VERSION,
        accepted: state.is_accepted(),
        text: TERMS,
    }
}

/// The text of the LICENSE.
#[tauri::command]
pub fn terms_get_license() -> &'static str {
    LICENSE
}

/// Records that the user agreed to terms version `version`, then lets what
/// waited for it start. `version` is the one the screen showed, so an
/// agreement to an older text is not taken for the one in force.
#[tauri::command]
pub fn terms_accept(
    version: u32,
    config: tauri::State<'_, ConfigState>,
    state: tauri::State<'_, TermsState>,
) -> Result<(), String> {
    accept(version, &config, &state)
}

fn accept(version: u32, config: &ConfigState, state: &TermsState) -> Result<(), String> {
    if version != TERMS_VERSION {
        return Err(format!(
            "The terms in force are version {}, not {}",
            TERMS_VERSION, version
        ));
    }
    let accepted_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .ok();
    config.modify(|config| {
        config.terms_version = Some(TERMS_VERSION);
        config.terms_accepted_at = accepted_at;
    })?;
    state.accept();
    tracing::info!("Agreed to the terms of use, version {}", TERMS_VERSION);
    Ok(())
}

/// A page the settings screen opens in the browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Page {
    Privacy,
    Announcements,
}

impl Page {
    fn url(self) -> &'static str {
        match self {
            Page::Privacy => PRIVACY_URL,
            Page::Announcements => ANNOUNCEMENTS_URL,
        }
    }
}

/// Opens `page` in the browser. Only the pages above: the screen cannot make
/// the app open another address.
#[tauri::command]
pub fn terms_open_page(page: Page) -> Result<(), String> {
    open_with(crate::logging::desktop_opener(), page.url())
}

fn open_with(opener: &str, url: &str) -> Result<(), String> {
    std::process::Command::new(opener)
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not open {}: {}", url, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// The version line that ends the terms: `制定: <日付>（版 N）`.
    fn version_in(text: &str) -> u32 {
        let line = text
            .lines()
            .find(|line| line.starts_with("制定:"))
            .expect("the terms end with a line giving the date and the version");
        let rest = line.split("版 ").nth(1).expect("the line names a version");
        rest.trim_end_matches('）')
            .trim()
            .parse()
            .expect("the version is a number")
    }

    /// Verifies: REQ-TRM-005
    #[test]
    fn when_the_terms_are_bundled_the_version_they_state_is_the_one_the_app_asks_for() {
        assert_eq!(version_in(TERMS), TERMS_VERSION);
    }

    /// Verifies: REQ-TRM-005
    #[test]
    fn when_the_terms_are_bundled_no_placeholder_is_left_in_the_text() {
        assert!(!TERMS.contains("{{"), "a template placeholder remains");
        assert!(!TERMS.contains("<!--"), "an editor's note remains");
    }

    #[test]
    fn when_the_license_is_bundled_it_is_not_empty() {
        assert!(LICENSE.contains("License"));
    }

    #[test]
    fn when_the_terms_are_not_agreed_to_the_state_says_so() {
        assert!(!TermsState::new(false).is_accepted());
        assert!(TermsState::new(true).is_accepted());
    }

    /// Verifies: REQ-TRM-003
    #[tokio::test(start_paused = true)]
    async fn when_the_terms_are_not_agreed_to_waiting_does_not_finish() {
        let state = TermsState::new(false);
        let waited = tokio::time::timeout(Duration::from_secs(3600), state.wait_accepted()).await;
        assert!(waited.is_err(), "the wait ended without an agreement");
    }

    /// Verifies: REQ-TRM-003
    #[tokio::test(start_paused = true)]
    async fn when_the_terms_are_agreed_to_while_waiting_the_wait_ends() {
        let state = std::sync::Arc::new(TermsState::new(false));
        let waiting = {
            let state = state.clone();
            tokio::spawn(async move { state.wait_accepted().await })
        };
        tokio::time::sleep(Duration::from_secs(60)).await;
        assert!(
            !waiting.is_finished(),
            "the wait ended before the agreement"
        );
        state.accept();
        tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .expect("the wait did not end after the agreement")
            .unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn when_the_terms_were_already_agreed_to_waiting_ends_at_once() {
        let state = TermsState::new(true);
        tokio::time::timeout(Duration::from_secs(1), state.wait_accepted())
            .await
            .expect("the wait did not end");
    }

    fn config_in(dir: &std::path::Path) -> ConfigState {
        ConfigState::at(
            dir.join("config.toml"),
            jamjam::config::AppConfig::default(),
        )
    }

    /// Verifies: REQ-TRM-002
    #[test]
    fn when_the_terms_in_force_are_agreed_to_the_version_is_saved_and_the_state_is_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let (config, state) = (config_in(dir.path()), TermsState::new(false));
        accept(TERMS_VERSION, &config, &state).unwrap();
        assert!(state.is_accepted());
        let saved = jamjam::config::load_config_from(&dir.path().join("config.toml")).unwrap();
        assert!(saved.terms_accepted());
        assert!(saved.terms_accepted_at.is_some());
    }

    /// Verifies: REQ-TRM-002
    #[test]
    fn when_another_version_than_the_one_in_force_is_agreed_to_nothing_is_saved() {
        let dir = tempfile::tempdir().unwrap();
        let (config, state) = (config_in(dir.path()), TermsState::new(false));
        assert!(accept(TERMS_VERSION + 1, &config, &state).is_err());
        assert!(!state.is_accepted());
        assert!(!config.get().unwrap().terms_accepted());
        assert!(!dir.path().join("config.toml").exists());
    }

    /// Verifies: REQ-TRM-004
    #[test]
    fn when_a_page_is_named_it_is_one_of_the_two_pages_the_app_opens() {
        for page in [Page::Privacy, Page::Announcements] {
            assert!(page.url().contains("/jamjam-client/blob/main/docs/"));
        }
        assert!(serde_json::from_str::<Page>("\"somewhere_else\"").is_err());
    }
}
