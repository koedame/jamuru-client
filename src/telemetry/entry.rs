//! What an app tells the server about itself when it enters a room.
//!
//! The GUI and the CLI both call [`client_info`], so what is sent and when the
//! audio devices are left out is decided in one place. The GUI adds what only
//! it knows (the WebView version) to the `machine` it passes in.

use std::sync::Arc;
use std::time::Duration;

use crate::audio::bounded;
use crate::config::AppConfig;
use crate::network::ClientInfo;

use super::event::{AppStart, AudioEnv};

/// Reads the `audio_env` for the devices with these ids (`None`: the OS
/// default).
pub type ReadAudioEnv = Arc<dyn Fn(Option<&str>, Option<&str>) -> AudioEnv + Send + Sync>;

/// The `audio_env` for the devices with these ids, or `None` when the drivers
/// do not answer within `timeout`. Asking a driver that hung never returns, so
/// it is asked on a thread of its own and what is not known is not reported.
///
/// Blocks for up to `timeout`: call it from a blocking thread.
pub fn read_audio_env_within(
    read: ReadAudioEnv,
    input_id: Option<String>,
    output_id: Option<String>,
    timeout: Duration,
) -> Option<AudioEnv> {
    bounded("reading the audio devices", timeout, move || {
        read(input_id.as_deref(), output_id.as_deref())
    })
    .ok()
}

/// What the app tells the server when it enters a room ([`ClientInfo`]),
/// whether or not usage reporting is on. Read fresh, so it shows the settings
/// and devices of this moment. A driver that does not answer within `timeout`
/// leaves the devices out; the rest is still sent.
///
/// The audio devices (their names and IDs, which can carry a person's name)
/// are part of it only when `share_devices` is set, which is while usage
/// reporting is on (ADR-065). The display name is never part of it.
///
/// Blocks for up to `timeout`: call it from a blocking thread.
pub fn client_info(
    app_version: &str,
    machine: AppStart,
    config: &AppConfig,
    share_devices: bool,
    read: ReadAudioEnv,
    timeout: Duration,
) -> ClientInfo {
    let audio = if share_devices {
        read_audio_env_within(
            read,
            config.input_device_id.clone(),
            config.output_device_id.clone(),
            timeout,
        )
        .unwrap_or(AudioEnv {
            input: None,
            output: None,
            input_id: config.input_device_id.clone(),
            output_id: config.output_device_id.clone(),
        })
    } else {
        AudioEnv {
            input: None,
            output: None,
            input_id: None,
            output_id: None,
        }
    };
    ClientInfo::new(app_version, machine, audio)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::{Device, DeviceKind};

    fn microphone_named(name: &str) -> ReadAudioEnv {
        let name = name.to_string();
        Arc::new(move |input_id, _| AudioEnv {
            input: Some(Device {
                name: name.clone(),
                kind: DeviceKind::Usb,
                channels: 2,
                sample_rates: vec![48000],
                min_buffer_frames: None,
                is_default: false,
            }),
            output: None,
            input_id: input_id.map(str::to_string),
            output_id: None,
        })
    }

    fn config() -> AppConfig {
        AppConfig {
            input_device_id: Some("alsa:mic".to_string()),
            ..AppConfig::default()
        }
    }

    fn info(share_devices: bool, read: ReadAudioEnv, timeout: Duration) -> serde_json::Value {
        serde_json::to_value(client_info(
            "1.2.3",
            AppStart::default(),
            &config(),
            share_devices,
            read,
            timeout,
        ))
        .unwrap()
    }

    /// Verifies: REQ-CON-032
    #[test]
    fn when_the_devices_are_not_to_be_shared_the_room_entry_info_has_no_device_name_or_id() {
        let info = info(
            false,
            microphone_named("Taro's AirPods"),
            Duration::from_secs(1),
        );

        assert_eq!(info["app_version"], "1.2.3");
        assert_eq!(info["os"], std::env::consts::OS);
        assert!(
            info["input"].is_null() && info["output"].is_null(),
            "{info}"
        );
        assert!(
            info.get("input_id").is_none() && info.get("output_id").is_none(),
            "{info}"
        );
        assert!(!info.to_string().contains("AirPods"), "{info}");
    }

    /// Verifies: REQ-CON-032
    #[test]
    fn when_the_devices_are_to_be_shared_the_room_entry_info_has_their_name_and_id() {
        let info = info(true, microphone_named("Studio mic"), Duration::from_secs(1));

        assert_eq!(info["input"]["name"], "Studio mic");
        assert_eq!(info["input_id"], "alsa:mic");
    }

    /// Verifies: REQ-CON-032
    #[test]
    fn when_a_driver_does_not_answer_in_time_the_room_entry_info_keeps_the_ids_and_leaves_the_devices_out(
    ) {
        let (release, hold) = std::sync::mpsc::channel::<()>();
        let hold = std::sync::Mutex::new(hold);
        let hung: ReadAudioEnv = Arc::new(move |_, _| {
            let _ = hold.lock().unwrap().recv();
            AudioEnv {
                input: None,
                output: None,
                input_id: None,
                output_id: None,
            }
        });

        let info = info(true, hung, Duration::from_millis(50));
        drop(release);

        assert!(info["input"].is_null(), "{info}");
        assert_eq!(info["input_id"], "alsa:mic");
    }
}
