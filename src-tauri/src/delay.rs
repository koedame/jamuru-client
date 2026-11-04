//! Measuring the delay of the audio devices from the settings (REQ-AUD-050).
//!
//! The measuring itself is `jamjam::audio::delay_meter`. This is the IPC
//! surface over it: start and cancel a run, read where it is, and keep the
//! figure in the config together with the devices it was measured on. The
//! room is told the figure from there (`signaling::SignalingState::set_own_delay`).

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use jamjam::audio::delay_meter::{self, Failure, Measurement, Stability, State};
use jamjam::config::DeviceDelay;

use crate::config::ConfigState;
use crate::streaming::StreamingState;

/// The event the screen hears when a run ends with a figure.
pub const MEASURED_EVENT: &str = "audio:delay-measured";

/// The delay kept for the devices selected now.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Figure {
    pub delay_ms: f64,
    pub spread_ms: f64,
    /// `steady`, `slightly_shaky` or `shaky`.
    pub stability: &'static str,
    /// Whether the delay is long enough to look at the device settings again.
    pub needs_review: bool,
    pub measured_at: u64,
}

/// Where the measuring is.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Status {
    /// Whether an audio session is running: the sound is played and heard
    /// through the session's own streams.
    pub session_running: bool,
    pub running: bool,
    /// `clicks` or `tone`, while running.
    pub stimulus: Option<&'static str>,
    pub step: Option<usize>,
    pub steps: usize,
    /// Why the last run gave no figure, if it did not: `no_sound`,
    /// `unsteady`, `too_long`, `no_stream` or `timed_out`.
    pub failure: Option<&'static str>,
    /// The figure for the devices selected now, if there is one.
    pub figure: Option<Figure>,
    /// The delays the others in the room told of their devices, in ms, by id.
    pub peers: std::collections::BTreeMap<String, f32>,
}

fn figure_of(delay: &DeviceDelay) -> Figure {
    Figure {
        delay_ms: delay.delay_ms,
        spread_ms: delay.spread_ms,
        stability: Stability::of_spread(delay.spread_ms).name(),
        needs_review: delay.delay_ms > delay_meter::REVIEW_ABOVE_MS,
        measured_at: delay.measured_at,
    }
}

fn status(
    session_running: bool,
    state: &State,
    current: Option<&DeviceDelay>,
    peers: std::collections::HashMap<String, f32>,
) -> Status {
    let (running, stimulus, step) = match state {
        State::Running(step) => (true, Some(step.stimulus.name()), Some(step.step)),
        _ => (false, None, None),
    };
    let failure = match state {
        State::Done(Err(Failure::Cancelled)) | State::Done(Ok(_)) => None,
        State::Done(Err(failure)) => Some(failure.name()),
        _ => None,
    };
    Status {
        session_running,
        running,
        stimulus,
        step,
        steps: delay_meter::LEVELS_DB.len(),
        failure,
        figure: current.map(figure_of),
        peers: peers.into_iter().collect(),
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Keeps a figure with the devices it was measured on. `measured_on` is the
/// config as it was when the run began: if a device was changed meanwhile, the
/// figure is kept for the old devices and is not the current one.
pub fn keep(
    config: &ConfigState,
    measured_on: &jamjam::config::AppConfig,
    found: &Measurement,
) -> Result<(), String> {
    config
        .modify(|next| {
            next.device_delay = Some(DeviceDelay {
                delay_ms: found.delay_ms,
                spread_ms: found.spread_ms,
                input_device_id: measured_on.input_device_id.clone(),
                output_device_id: measured_on.output_device_id.clone(),
                buffer_size: measured_on.buffer_size,
                sample_rate: measured_on.sample_rate,
                measured_at: now_secs(),
            });
        })
        .map(|_| ())
}

/// Starts measuring the delay of the devices the session runs on. The sound
/// takes the place of what is played for up to 90 seconds, and the room hears
/// silence from this app meanwhile.
#[tauri::command]
pub fn delay_measure_start(
    app: AppHandle,
    streaming: tauri::State<'_, StreamingState>,
    config: tauri::State<'_, ConfigState>,
) -> Result<(), String> {
    crate::terms::require_accepted(&app)?;
    if !streaming.is_active() {
        return Err("The delay is measured on the devices the room uses: enter a room first".into());
    }
    let measured_on = config.get()?;
    let handle = app.clone();
    delay_meter::start(move |result| {
        let Ok(found) = result else {
            return;
        };
        tracing::info!(
            "Measured the device delay: {:.2} ms (spread {:.2} ms, {} of {} kept, {})",
            found.delay_ms,
            found.spread_ms,
            found.kept,
            found.heard,
            found.stimulus.name()
        );
        if let Err(e) = keep(&handle.state::<ConfigState>(), &measured_on, found) {
            tracing::warn!("Could not keep the measured device delay: {}", e);
            return;
        }
        let _ = handle.emit(MEASURED_EVENT, found.delay_ms);
    })
    .map_err(String::from)
}

/// Stops the measuring under way.
#[tauri::command]
pub fn delay_measure_cancel() {
    delay_meter::cancel();
}

/// Where the measuring is, and the figure for the devices selected now.
#[tauri::command]
pub fn delay_measure_status(
    streaming: tauri::State<'_, StreamingState>,
    config: tauri::State<'_, ConfigState>,
    signaling: tauri::State<'_, crate::signaling::SignalingState>,
) -> Result<Status, String> {
    let config = config.get()?;
    Ok(status(
        streaming.is_active(),
        &delay_meter::state(),
        config.current_device_delay(),
        signaling.peer_delays(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use jamjam::audio::delay_meter::{Step, Stimulus};

    fn delay(delay_ms: f64, spread_ms: f64) -> DeviceDelay {
        DeviceDelay {
            delay_ms,
            spread_ms,
            input_device_id: None,
            output_device_id: None,
            buffer_size: 64,
            sample_rate: 48_000,
            measured_at: 1,
        }
    }

    /// Verifies: REQ-AUD-050
    #[test]
    fn while_measuring_the_status_says_which_sound_and_which_step() {
        let state = State::Running(Step {
            stimulus: Stimulus::Tone,
            step: 3,
            steps: 8,
        });

        let status = status(true, &state, None, Default::default());

        assert!(status.running);
        assert_eq!(status.stimulus, Some("tone"));
        assert_eq!(status.step, Some(3));
        assert_eq!(status.steps, 8);
        assert_eq!(status.failure, None);
    }

    /// Verifies: REQ-AUD-050
    #[test]
    fn when_a_run_found_nothing_the_status_says_why_and_a_cancel_says_nothing() {
        let failed = status(true, &State::Done(Err(Failure::NoSound)), None, Default::default());
        let cancelled = status(true, &State::Done(Err(Failure::Cancelled)), None, Default::default());

        assert_eq!(failed.failure, Some("no_sound"));
        assert!(!failed.running);
        assert_eq!(cancelled.failure, None);
    }

    /// Verifies: REQ-AUD-050
    #[test]
    fn the_figure_carries_how_steady_it_is_and_whether_the_settings_want_a_look() {
        let steady = status(true, &State::Idle, Some(&delay(23.0, 0.5)), Default::default());
        let long = status(true, &State::Idle, Some(&delay(140.0, 4.0)), Default::default());

        let steady = steady.figure.unwrap();
        assert_eq!(steady.stability, "steady");
        assert!(!steady.needs_review);
        let long = long.figure.unwrap();
        assert_eq!(long.stability, "slightly_shaky");
        assert!(long.needs_review);
    }

    /// Verifies: REQ-CON-035
    #[test]
    fn the_status_lists_the_delays_the_others_told() {
        let peers = [("p1".to_string(), 23.5_f32)].into_iter().collect();

        let status = status(true, &State::Idle, None, peers);

        assert_eq!(status.peers.get("p1"), Some(&23.5));
    }

    /// Verifies: REQ-AUD-050
    #[test]
    fn a_figure_is_kept_with_the_devices_it_was_measured_on_and_is_current_only_for_them() {
        let dir = tempfile::tempdir().unwrap();
        let on = jamjam::config::AppConfig {
            input_device_id: Some("in-a".into()),
            ..Default::default()
        };
        let config = ConfigState::at(dir.path().join("config.toml"), on.clone());
        let found = Measurement {
            delay_ms: 31.5,
            spread_ms: 0.4,
            stability: Stability::Steady,
            kept: 5,
            heard: 5,
            stimulus: Stimulus::Clicks,
            level_db: -48.0,
        };

        keep(&config, &on, &found).unwrap();

        let saved = config.get().unwrap();
        assert_eq!(saved.current_device_delay().map(|d| d.delay_ms), Some(31.5));
        // The person changed the input while it ran
        let changed = jamjam::config::AppConfig {
            input_device_id: Some("in-b".into()),
            ..saved
        };
        assert_eq!(changed.current_device_delay(), None);
    }
}
