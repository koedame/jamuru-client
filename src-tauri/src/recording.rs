//! Recording the room (ADR-095).
//!
//! The screen starts and stops it and shows how long it has run; the files
//! are written by the core's recorder. Which folder they go to is not the
//! screen's to say: it is `<the user's music folder>/jamuru`, so the screen
//! cannot make the app write somewhere else, and what it reads back are
//! file names, not paths.

use std::path::{Path, PathBuf};

use jamjam::audio::{FinishedRecording, RecordingEnd, RecordingReport, RecordingStartError};
use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::signaling::SignalingState;
use crate::streaming::StreamingState;

/// The folder under the music folder the recordings go to.
const FOLDER: &str = "jamuru";

/// How a recording that ended came to an end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EndedBy {
    /// The person stopped it
    Stopped,
    /// The audio session ended (the room was left, the devices were reopened)
    SessionEnded,
    /// A file got as big as a WAV can be
    SizeLimit,
    /// Writing failed (the disk is full, the folder is gone)
    WriteFailed,
}

/// A recording that ended.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Ended {
    /// A new number for each recording that ends, so the screen shows each ending once
    pub id: u64,
    pub file_name: String,
    /// The microphone's file, if it was recorded on its own
    pub self_file_name: Option<String>,
    pub seconds: f32,
    /// Stereo frames missing from the file because the disk writer fell behind
    pub dropped_frames: u64,
    pub ended_by: EndedBy,
    /// What went wrong, for `WriteFailed`
    pub error: Option<String>,
}

/// What the screen shows of the recording.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Status {
    pub recording: bool,
    pub seconds: f32,
    pub dropped_frames: u64,
    pub file_name: Option<String>,
    pub last: Option<Ended>,
    /// The participants who say they record the room, by id
    pub peers: Vec<String>,
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn ended(finished: FinishedRecording) -> Ended {
    let (ended_by, error) = match finished.reason {
        RecordingEnd::Stopped => (EndedBy::Stopped, None),
        RecordingEnd::SessionEnded => (EndedBy::SessionEnded, None),
        RecordingEnd::SizeLimit => (EndedBy::SizeLimit, None),
        RecordingEnd::WriteFailed(error) => (EndedBy::WriteFailed, Some(error)),
    };
    Ended {
        id: finished.id,
        file_name: file_name(&finished.mix_path),
        self_file_name: finished.self_path.as_deref().map(file_name),
        seconds: finished.seconds,
        dropped_frames: finished.dropped_frames,
        ended_by,
        error,
    }
}

fn status(report: RecordingReport, peers: Vec<String>) -> Status {
    Status {
        peers,
        recording: report.recording,
        seconds: report.seconds,
        dropped_frames: report.dropped_frames,
        file_name: report.mix_path.as_deref().map(file_name),
        last: report.last.map(ended),
    }
}

/// `<music folder>/jamuru`
fn folder<R: tauri::Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    let base = app
        .path()
        .audio_dir()
        .or_else(|_| app.path().document_dir())
        .map_err(|e| format!("The folder to record to is unknown: {}", e))?;
    Ok(base.join(FOLDER))
}

/// The name a recording started now gets, without the extension:
/// `jamuru-2026-11-04-153012`.
fn stem(now: chrono::DateTime<chrono::Local>) -> String {
    now.format("jamuru-%Y-%m-%d-%H%M%S").to_string()
}

/// Starts recording the room: what the person hears, and with `with_self` the
/// microphone as well, in a file of its own.
#[tauri::command]
pub async fn recording_start(
    with_self: bool,
    app: AppHandle,
    state: tauri::State<'_, StreamingState>,
    signaling: tauri::State<'_, SignalingState>,
) -> Result<Status, String> {
    crate::terms::require_accepted(&app)?;
    let sample_rate = state
        .running_sample_rate()
        .ok_or("There is no audio to record: the room has not been entered")?;
    let dir = folder(&app)?;
    let report = state
        .recorder()
        .start(&dir, &stem(chrono::Local::now()), sample_rate, with_self)
        .map_err(|e| match e {
            RecordingStartError::Already => "A recording is already running".to_string(),
            RecordingStartError::Io(e) => {
                format!(
                    "The recording file could not be made in {}: {}",
                    dir.display(),
                    e
                )
            }
        })?;
    tracing::info!(
        "Recording started at {} Hz (microphone separately: {})",
        sample_rate,
        with_self
    );
    Ok(status(report, signaling.recording_peers()))
}

/// Stops the recording, if one runs.
#[tauri::command]
pub async fn recording_stop(
    state: tauri::State<'_, StreamingState>,
    signaling: tauri::State<'_, SignalingState>,
) -> Result<Status, String> {
    if let Some(finished) = state.recorder().stop(RecordingEnd::Stopped) {
        tracing::info!("Recording stopped after {:.1} s", finished.seconds);
    }
    Ok(status(
        state.recorder().report(),
        signaling.recording_peers(),
    ))
}

/// How the recording is doing.
#[tauri::command]
pub async fn recording_status(
    state: tauri::State<'_, StreamingState>,
    signaling: tauri::State<'_, SignalingState>,
) -> Result<Status, String> {
    Ok(status(
        state.recorder().report(),
        signaling.recording_peers(),
    ))
}

/// Opens the folder the recordings are in, in the file manager.
#[tauri::command]
pub fn recording_open_folder(app: AppHandle) -> Result<(), String> {
    let dir = folder(&app)?;
    std::fs::create_dir_all(&dir).map_err(|e| {
        format!(
            "Could not make the recordings folder {}: {}",
            dir.display(),
            e
        )
    })?;
    std::process::Command::new(crate::logging::desktop_opener())
        .arg(&dir)
        .spawn()
        .map(|_| ())
        .map_err(|e| {
            format!(
                "Could not open the recordings folder {}: {}",
                dir.display(),
                e
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// Verifies: REQ-AUD-046
    #[test]
    fn when_a_recording_starts_its_name_is_the_time_it_started() {
        let now = chrono::Local
            .with_ymd_and_hms(2026, 11, 4, 15, 30, 12)
            .unwrap();

        assert_eq!(stem(now), "jamuru-2026-11-04-153012");
    }

    /// Verifies: REQ-AUD-046
    #[test]
    fn when_a_recording_ended_the_screen_is_given_file_names_not_paths() {
        let finished = FinishedRecording {
            id: 3,
            mix_path: PathBuf::from("/home/someone/Music/jamuru/jamuru-a.wav"),
            self_path: Some(PathBuf::from(
                "/home/someone/Music/jamuru/jamuru-a-self.wav",
            )),
            seconds: 61.5,
            dropped_frames: 0,
            reason: RecordingEnd::WriteFailed("disk full".into()),
        };

        let shown = ended(finished);

        assert_eq!(shown.file_name, "jamuru-a.wav");
        assert_eq!(shown.self_file_name.as_deref(), Some("jamuru-a-self.wav"));
        assert_eq!(shown.ended_by, EndedBy::WriteFailed);
        assert_eq!(shown.error.as_deref(), Some("disk full"));
        assert!(!serde_json::to_string(&shown).unwrap().contains("someone"));
    }
}
