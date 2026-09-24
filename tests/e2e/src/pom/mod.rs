//! Page object model for the jamjam desktop app (ADR-025).
//!
//! Scenarios describe what a *user* does and sees. They go through the
//! screens in [`screens`]; they never see a CSS selector, a window label or
//! an HTTP call. That indirection is the point: a UI restructure should
//! change page objects, not tests.
//!
//! Requires the app to be built with the `e2e-control` feature - see
//! [`App::launch`].

pub mod driver;
pub mod element;
pub mod loopback_audio;
pub mod screens;

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use driver::{Driver, DriverResult};
use screens::{ConnectionScreen, SessionScreen, SettingsScreen};

/// How long to wait for the app to answer its first health check. Generous
/// because it covers process start plus webview creation on a cold cache.
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(30);

/// Default for waits on UI transitions.
pub const UI_TIMEOUT: Duration = Duration::from_secs(10);

/// A running jamjam app under test.
pub struct App {
    driver: Driver,
    process: Child,
    /// Isolated `$HOME` for this run; dropped (and deleted) with the app.
    _home: tempfile::TempDir,
}

impl App {
    /// Builds nothing - it launches the binary at `binary` and waits for the
    /// control channel.
    ///
    /// The caller must have built that binary **with the `e2e-control`
    /// feature**. A plain `cargo build` overwrites the same path without the
    /// feature, and the app then starts normally but never opens the port,
    /// which looks like a hang. [`Self::launch`] resolves and checks the path
    /// so that failure mode reports itself.
    pub fn launch_binary(binary: &Path) -> DriverResult<Self> {
        Self::launch_binary_with_devices(binary, None, None)
    }

    /// [`Self::launch_binary`], pinning the audio devices in the seeded config.
    pub fn launch_binary_with_devices(
        binary: &Path,
        input_device: Option<&str>,
        output_device: Option<&str>,
    ) -> DriverResult<Self> {
        Self::launch_binary_with_env(binary, input_device, output_device, &[])
    }

    /// [`Self::launch_binary_with_devices`], also setting environment variables
    /// for the app (for example `JAMJAM_LOG`).
    pub fn launch_binary_with_env(
        binary: &Path,
        input_device: Option<&str>,
        output_device: Option<&str>,
        env: &[(&str, &str)],
    ) -> DriverResult<Self> {
        Self::launch_binary_seeded(binary, input_device, output_device, "", None, &[], env)
    }

    /// Launches the release build with the jamjam server pinned to
    /// `server_url` and `args` on its command line (an invite link, as the OS
    /// passes it).
    ///
    /// A release build uses the server it was built with unless the config
    /// says otherwise, and the scenarios that need the app connected use a
    /// local one. Build it first, with any remote server - the config replaces it:
    /// `JAMJAM_SERVER_URL=https://jamjam.example.com cargo build --release --manifest-path src-tauri/Cargo.toml --features e2e-control`
    pub fn launch_release(server_url: &str, args: &[&str]) -> DriverResult<Self> {
        Self::launch_binary_seeded(
            &release_binary_path(),
            None,
            None,
            &server_url_setting(server_url),
            None,
            args,
            &[],
        )
    }

    fn launch_binary_seeded(
        binary: &Path,
        input_device: Option<&str>,
        output_device: Option<&str>,
        settings: &str,
        identity: Option<&[u8; 32]>,
        args: &[&str],
        env: &[(&str, &str)],
    ) -> DriverResult<Self> {
        if !binary.exists() {
            return Err(format!(
                "app binary not found at {}. Build it first:\n  \
                 cargo build --manifest-path src-tauri/Cargo.toml --features e2e-control \
                 (add --release for the release build)",
                binary.display()
            ));
        }

        let port = free_port()?;
        // A throwaway $HOME keeps the run from reading or writing the
        // developer's real config.toml and device_identity.json - the
        // `directories` crate derives the app data dir from it. Product code
        // needs no test-only override for this.
        let home =
            tempfile::tempdir().map_err(|e| format!("could not create a temp HOME: {}", e))?;
        seed_config(home.path(), input_device, output_device, settings)?;
        if let Some(secret) = identity {
            seed_identity(home.path(), secret)?;
        }

        let process = Command::new(binary)
            .args(args)
            .env("JAMJAM_E2E_CONTROL_PORT", port.to_string())
            .env("HOME", home.path())
            // Where the log file lands on Linux; an inherited value would put
            // it outside the throwaway $HOME.
            .env_remove("XDG_DATA_HOME")
            .envs(env.iter().copied())
            .spawn()
            .map_err(|e| format!("could not start {}: {}", binary.display(), e))?;

        let app = Self {
            driver: Driver::new(port),
            process,
            _home: home,
        };

        wait_for(LAUNCH_TIMEOUT, || app.driver.is_healthy()).map_err(|_| {
            format!(
                "app did not open its control channel on port {} within {:?}. \
                 Was it built with --features e2e-control?",
                port, LAUNCH_TIMEOUT
            )
        })?;

        Ok(app)
    }

    /// Launches the app from its conventional build location.
    pub fn launch() -> DriverResult<Self> {
        Self::launch_with_devices(None, None)
    }

    /// Launches from the conventional build location with `env` set for the app.
    pub fn launch_with_env(env: &[(&str, &str)]) -> DriverResult<Self> {
        Self::launch_binary_with_env(&default_binary_path(), None, None, env)
    }

    /// Launches with the audio devices pinned in the app's own config file.
    ///
    /// The device is seeded through `config.toml` rather than the settings UI
    /// because streaming reads it at start-up; changing it afterwards would
    /// race the connect sequence. Writing into the throwaway `$HOME` keeps it
    /// scoped to this run.
    pub fn launch_with_devices(
        input_device: Option<&str>,
        output_device: Option<&str>,
    ) -> DriverResult<Self> {
        Self::launch_binary_with_devices(&default_binary_path(), input_device, output_device)
    }

    /// [`Self::launch_with_devices`], also writing `settings` (TOML lines such
    /// as `input_channel_l = 5`) into the app's `config.toml`.
    pub fn launch_with_devices_and_settings(
        input_device: Option<&str>,
        output_device: Option<&str>,
        settings: &str,
    ) -> DriverResult<Self> {
        Self::launch_binary_seeded(
            &default_binary_path(),
            input_device,
            output_device,
            settings,
            None,
            &[],
            &[],
        )
    }

    /// Launches the debug build with the jamjam server pinned to
    /// `server_url`, for scenarios about what the connection screen shows
    /// when that URL is wrong or unreachable (a leftover dev/test value in
    /// `config.toml`, for example). [`Self::launch_release`] is the
    /// release-build equivalent.
    pub fn launch_with_server_url(server_url: &str) -> DriverResult<Self> {
        Self::launch_binary_seeded(
            &default_binary_path(),
            None,
            None,
            &server_url_setting(server_url),
            None,
            &[],
            &[],
        )
    }

    /// Launches the debug build as the device whose secret key is `secret`
    /// (`jamjam::network::DeviceIdentity::from_secret_bytes`), so a scenario
    /// can tell the signaling server which device the app is before it
    /// connects - the app lists rooms as soon as it starts.
    pub fn launch_as_device(secret: &[u8; 32]) -> DriverResult<Self> {
        Self::launch_binary_seeded(
            &default_binary_path(),
            None,
            None,
            "",
            Some(secret),
            &[],
            &[],
        )
    }

    /// The app's log file inside the throwaway `$HOME`. The path mirrors what
    /// Tauri derives for the log directory of the app identifier on each platform.
    pub fn log_file_path(&self) -> PathBuf {
        let home = self._home.path();
        if cfg!(target_os = "macos") {
            home.join("Library/Logs/me.koeda.jamjam/jamjam.log")
        } else if cfg!(target_os = "windows") {
            home.join("AppData/Local/me.koeda.jamjam/logs/jamjam.log")
        } else {
            home.join(".local/share/me.koeda.jamjam/logs/jamjam.log")
        }
    }

    /// Where the app keeps the install ID and the record of a crash, inside the
    /// throwaway `$HOME` (`ProjectDirs` of `jamjam`, then `usage`).
    pub fn usage_state_dir(&self) -> PathBuf {
        let home = self._home.path();
        if cfg!(target_os = "macos") {
            home.join("Library/Application Support/jamjam/usage")
        } else if cfg!(target_os = "windows") {
            home.join("AppData/Local/jamjam/data/usage")
        } else {
            home.join(".local/share/jamjam/usage")
        }
    }

    /// What the app has written to its log file so far; empty if there is none yet.
    pub fn log_text(&self) -> String {
        std::fs::read_to_string(self.log_file_path()).unwrap_or_default()
    }

    pub fn connection_screen(&self) -> ConnectionScreen<'_> {
        ConnectionScreen::new(&self.driver)
    }

    /// The in-session screen: room sidebar, mixer and chat.
    pub fn session_screen(&self) -> SessionScreen<'_> {
        SessionScreen::new(&self.driver)
    }

    pub fn settings_screen(&self) -> SettingsScreen<'_> {
        SettingsScreen::new(&self.driver)
    }

    /// Labels of the windows the user currently has open.
    pub fn open_windows(&self) -> DriverResult<Vec<String>> {
        self.driver.open_windows()
    }

    /// Whole rendered document of the main window. For assertions phrased
    /// over the entire screen, such as "this string appears nowhere".
    pub fn main_window_html(&self) -> DriverResult<String> {
        self.driver.dom(None)
    }
}

impl Drop for App {
    fn drop(&mut self) {
        // Kill unconditionally: a scenario that panicked mid-test must not
        // leave a GUI process behind holding the audio device.
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

/// The `config.toml` line that pins the jamjam server.
fn server_url_setting(server_url: &str) -> String {
    format!("server_url = {:?}\n", server_url)
}

/// Writes a `config.toml` into the throwaway `$HOME` so the app starts with
/// the devices a scenario needs.
///
/// The path mirrors what `directories`' `ProjectDirs` derives for the app on
/// this platform. Only written when a scenario asks for specific devices -
/// otherwise the app should see a first-run state.
fn seed_config(
    home: &Path,
    input_device: Option<&str>,
    output_device: Option<&str>,
    settings: &str,
) -> DriverResult<()> {
    if input_device.is_none() && output_device.is_none() && settings.is_empty() {
        return Ok(());
    }

    let config_dir = config_dir(home)?;

    let mut toml = String::new();
    if let Some(device) = input_device {
        // The app stores cpal's stable device id, not the display name a
        // scenario passes in here - resolve it the same way the app does
        // (see `loopback_audio::resolve_device_id`).
        let id = loopback_audio::resolve_device_id(device)?;
        toml.push_str(&format!("input_device_id = {:?}\n", id));
    }
    if let Some(device) = output_device {
        let id = loopback_audio::resolve_device_id(device)?;
        toml.push_str(&format!("output_device_id = {:?}\n", id));
    }
    toml.push_str(settings);

    let path = config_dir.join("config.toml");
    std::fs::write(&path, toml).map_err(|e| format!("could not write {}: {}", path.display(), e))
}

/// Writes the device identity the app loads at start-up, in the file format
/// of `src-tauri/src/device_identity.rs`, next to `config.toml`.
fn seed_identity(home: &Path, secret: &[u8; 32]) -> DriverResult<()> {
    let stored = serde_json::json!({
        "version": 1,
        "secret_key": data_encoding::BASE64.encode(secret),
    });
    let path = config_dir(home)?.join("device_identity.json");
    std::fs::write(&path, stored.to_string())
        .map_err(|e| format!("could not write {}: {}", path.display(), e))
}

/// The app's config directory under the throwaway `$HOME`, created if missing.
fn config_dir(home: &Path) -> DriverResult<PathBuf> {
    let dir = if cfg!(target_os = "macos") {
        home.join("Library/Application Support/jamjam")
    } else if cfg!(target_os = "windows") {
        home.join("AppData/Roaming/jamjam/config")
    } else {
        home.join(".config/jamjam")
    };
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("could not create {}: {}", dir.display(), e))?;
    Ok(dir)
}

/// Path the app binary lands at, relative to this crate.
fn default_binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../src-tauri/target/debug")
        .join(if cfg!(windows) {
            "jamjam-app.exe"
        } else {
            "jamjam-app"
        })
}

/// Path the release build of the app lands at, relative to this crate.
fn release_binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../src-tauri/target/release")
        .join(if cfg!(windows) {
            "jamjam-app.exe"
        } else {
            "jamjam-app"
        })
}

/// Asks the OS for an unused port. Bound and released immediately, so there
/// is a small race window before the app claims it - acceptable for a test
/// harness, and far simpler than a port file the app has to write.
fn free_port() -> DriverResult<u16> {
    let listener =
        TcpListener::bind("127.0.0.1:0").map_err(|e| format!("could not reserve a port: {}", e))?;
    listener
        .local_addr()
        .map(|addr| addr.port())
        .map_err(|e| format!("could not read the reserved port: {}", e))
}

/// The condition never held before the deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimedOut;

impl std::fmt::Display for TimedOut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "timed out")
    }
}

impl std::error::Error for TimedOut {}

/// Polls `condition` until it holds or `timeout` elapses. Public so
/// scenarios can wait on a state no page object models directly.
pub fn wait_until(timeout: Duration, condition: impl FnMut() -> bool) -> Result<(), TimedOut> {
    wait_for(timeout, condition)
}

/// Polls `condition` until it holds or `timeout` elapses.
pub(crate) fn wait_for(
    timeout: Duration,
    mut condition: impl FnMut() -> bool,
) -> Result<(), TimedOut> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if condition() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    if condition() {
        Ok(())
    } else {
        Err(TimedOut)
    }
}
