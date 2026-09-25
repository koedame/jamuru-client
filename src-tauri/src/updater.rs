//! Self-update (ADR-041).
//!
//! A release published after the running version is downloaded, checked
//! against the public key in `tauri.conf.json` (`plugins.updater`), and
//! installed without the user doing anything. Nothing here ever reaches the
//! webview: no command, no event, no permission.
//!
//! Two rules keep it out of the way of the app's purpose. It never installs
//! while the user is in a session (an install ends with a restart, which would
//! cut the audio), and it never runs in a development build.

use std::time::Duration;

use tauri::{AppHandle, Runtime};
use tauri_plugin_updater::UpdaterExt;

use crate::config;
use crate::windows;

/// Left alone so the first frames and the audio device setup are not competing
/// with a network request.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(20);
const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const SESSION_POLL_INTERVAL: Duration = Duration::from_secs(30);

/// Why this run does not update itself, or `None` when it does.
pub(crate) fn skip_reason(
    auto_update: bool,
    development_build: bool,
    package_updates_itself: bool,
) -> Option<&'static str> {
    if development_build {
        Some("development build")
    } else if !package_updates_itself {
        Some("this package cannot replace itself")
    } else if !auto_update {
        Some("turned off in config.toml")
    } else {
        None
    }
}

/// A build made with `cargo tauri dev`, or the E2E build, must never replace
/// itself with a release: the E2E harness would find the app restarted under
/// it, and a developer would lose the checkout's build.
fn is_development_build() -> bool {
    cfg!(debug_assertions) || cfg!(feature = "e2e-control")
}

/// Only an AppImage can replace itself on Linux. A `.deb` install would need
/// `sudo` and a password prompt, which is not "without the user doing
/// anything", so it stays with the package manager.
fn package_updates_itself() -> bool {
    if cfg!(target_os = "linux") {
        matches!(
            tauri::utils::platform::bundle_type(),
            Some(tauri::utils::config::BundleType::AppImage)
        )
    } else {
        true
    }
}

/// Starts the background loop: check shortly after launch, then every six
/// hours. The setting is read again on every round, so turning it off in
/// `config.toml` takes effect without a restart.
pub(crate) fn spawn(app: AppHandle) {
    if is_development_build() {
        tracing::debug!("Self-update is off in a development build");
        return;
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK_DELAY).await;
        loop {
            let auto_update = config::load_config().unwrap_or_default().auto_update;
            match skip_reason(auto_update, false, package_updates_itself()) {
                Some(reason) => tracing::debug!("Not checking for an update: {}", reason),
                None => {
                    if let Err(e) = update_once(&app).await {
                        tracing::warn!("Self-update did not finish: {}", e);
                    }
                }
            }
            tokio::time::sleep(CHECK_INTERVAL).await;
        }
    });
}

async fn update_once(app: &AppHandle) -> tauri_plugin_updater::Result<()> {
    if let Some(version) = install_newer(app, true).await? {
        tracing::info!("Installed {}; restarting", version);
        // Windows: the installer has already ended this process. Elsewhere the
        // new version runs from the next start.
        app.restart();
    }
    Ok(())
}

/// Installs a newer release now, without waiting for the user to leave a
/// session: for someone debugging the app remotely (ADR-044), who restarts it
/// afterwards. Returns the version installed, or `None` when there was none.
#[cfg(feature = "debug-tools")]
pub(crate) async fn install_now<R: Runtime>(
    app: &AppHandle<R>,
) -> tauri_plugin_updater::Result<Option<String>> {
    install_newer(app, false).await
}

/// Downloads and installs a release newer than the running one, and returns
/// its version. With `wait_for_session` it holds back the install until the
/// user is out of a session, since the install ends in a restart.
async fn install_newer<R: Runtime>(
    app: &AppHandle<R>,
    wait_for_session: bool,
) -> tauri_plugin_updater::Result<Option<String>> {
    let Some(update) = app.updater()?.check().await? else {
        tracing::debug!("No newer release");
        return Ok(None);
    };
    tracing::info!(
        "Release {} is out (running {}); downloading",
        update.version,
        update.current_version
    );
    // `download` returns only bytes whose signature matches the public key
    // the app was built with.
    let bytes = update.download(|_, _| {}, || {}).await?;

    if wait_for_session {
        wait_until_no_session(windows::is_in_session, SESSION_POLL_INTERVAL).await;
        if !config::load_config().unwrap_or_default().auto_update {
            tracing::info!("Self-update was turned off while downloading; not installing");
            return Ok(None);
        }
    }

    update.install(bytes)?;
    Ok(Some(update.version.clone()))
}

/// Returns once `in_session` is false. The install ends in a restart, so it
/// waits for the user to leave the session rather than cutting the audio.
async fn wait_until_no_session(in_session: impl Fn() -> bool, poll: Duration) {
    while in_session() {
        tokio::time::sleep(poll).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Verifies: REQ-UPD-001
    #[test]
    fn when_the_setting_is_on_in_a_release_build_the_app_updates_itself() {
        assert_eq!(skip_reason(true, false, true), None);
    }

    /// Verifies: REQ-UPD-002
    #[test]
    fn when_the_setting_is_off_the_app_does_not_update_itself() {
        assert!(skip_reason(false, false, true).is_some());
    }

    /// Verifies: REQ-UPD-003
    #[test]
    fn when_it_is_a_development_build_the_app_does_not_update_itself() {
        assert!(skip_reason(true, true, true).is_some());
    }

    /// Verifies: REQ-UPD-004
    #[test]
    fn when_the_package_cannot_replace_itself_the_app_does_not_try() {
        assert!(skip_reason(true, false, false).is_some());
    }

    /// Verifies: REQ-UPD-005
    #[tokio::test]
    async fn when_the_user_is_in_a_session_the_install_waits_until_it_ends() {
        let asked = AtomicUsize::new(0);
        let in_session = || asked.fetch_add(1, Ordering::SeqCst) < 3;

        wait_until_no_session(in_session, Duration::from_millis(1)).await;

        assert_eq!(asked.load(Ordering::SeqCst), 4);
    }

    /// Verifies: REQ-UPD-005
    #[tokio::test]
    async fn when_the_user_is_not_in_a_session_the_install_does_not_wait() {
        let asked = AtomicUsize::new(0);
        let in_session = || {
            asked.fetch_add(1, Ordering::SeqCst);
            false
        };

        wait_until_no_session(in_session, Duration::from_secs(3600)).await;

        assert_eq!(asked.load(Ordering::SeqCst), 1);
    }
}
