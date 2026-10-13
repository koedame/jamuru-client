//! What this build of the app is and what it runs on, for the screen to read.
//!
//! The same coarse facts the usage report carries (`jamjam::environment`):
//! nothing that names the machine or its user, and the CPU model is left out for
//! the same reason it is there. It stays on the machine; the dev and beta
//! element picker (`ui/src/lib/elementReport.ts`) is what shows it.

use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct AppEnvironment {
    app_version: String,
    /// "release", "beta" (remote debugging built in), "e2e" or "dev" (a debug build)
    build: &'static str,
    os: &'static str,
    arch: &'static str,
    os_version: Option<String>,
    ram_gb: Option<u32>,
    audio_host: String,
}

fn build_channel() -> &'static str {
    if cfg!(feature = "e2e-control") {
        "e2e"
    } else if cfg!(feature = "debug-remote") {
        "beta"
    } else if cfg!(debug_assertions) {
        "dev"
    } else {
        "release"
    }
}

fn environment(app_version: String) -> AppEnvironment {
    AppEnvironment {
        app_version,
        build: build_channel(),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        os_version: jamjam::environment::os_version(),
        ram_gb: jamjam::environment::ram_gb(),
        audio_host: jamjam::environment::audio_host(),
    }
}

#[tauri::command]
pub fn app_environment(app: tauri::AppHandle) -> AppEnvironment {
    environment(app.package_info().version.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_carries_the_version_it_is_given_and_the_platform_it_runs_on() {
        let environment = environment("0.1.0-107".to_string());

        assert_eq!(environment.app_version, "0.1.0-107");
        assert_eq!(environment.os, std::env::consts::OS);
        assert_eq!(environment.arch, std::env::consts::ARCH);
        assert!(!environment.audio_host.is_empty());
    }

    #[test]
    fn a_build_without_the_debug_features_is_named_for_its_profile() {
        if cfg!(any(feature = "e2e-control", feature = "debug-remote")) {
            return;
        }

        let expected = if cfg!(debug_assertions) {
            "dev"
        } else {
            "release"
        };
        assert_eq!(build_channel(), expected);
    }
}
