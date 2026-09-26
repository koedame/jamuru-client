//! Audio diagnostics for jamjam
//!
//! Provides audio device checks including:
//! - Input/output device detection and capabilities
//! - Low-latency support detection (ASIO, CoreAudio exclusive)
//! - Buffer size compatibility
//! - Sample rate support

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::warn;

use super::{DiagnosticGrade, DiagnosticProblem, ProblemCode, ProblemSeverity};
use crate::audio::{
    bounded, list_input_devices, list_output_devices, resolve_input_device, resolve_output_device,
    stable_device_id, AudioDevice, AudioError, DeviceId, LIST_TIMEOUT,
};

/// Diagnostics for a single audio device
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceDiagnostics {
    /// Device ID
    pub id: String,
    /// Device name
    pub name: String,
    /// Whether this is the default device
    pub is_default: bool,
    /// Whether this is an ASIO device (Windows)
    pub is_asio: bool,
    /// Supported sample rates
    pub supported_sample_rates: Vec<u32>,
    /// Whether 48kHz is supported (required for jamjam)
    pub supports_48khz: bool,
    /// Supported channel counts
    pub supported_channels: Vec<u16>,
    /// Diagnostic grade for this device
    pub grade: DiagnosticGrade,
}

impl DeviceDiagnostics {
    /// Create diagnostics from an audio device
    fn from_audio_device(info: &AudioDevice) -> Self {
        let supports_48khz = info.supported_sample_rates.contains(&48000);

        // Calculate grade based on capabilities
        // On macOS, CoreAudio is the low-latency API (equivalent to ASIO on Windows)
        // On Linux, PipeWire/JACK provide low-latency audio
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        let grade = if supports_48khz {
            // 48kHz support is sufficient for A grade on macOS/Linux
            DiagnosticGrade::A
        } else {
            // Doesn't support 48kHz
            DiagnosticGrade::C
        };

        #[cfg(target_os = "windows")]
        let grade = if info.is_asio && supports_48khz {
            // ASIO with 48kHz support is ideal on Windows
            DiagnosticGrade::A
        } else if supports_48khz {
            // Non-ASIO but supports 48kHz - WASAPI shared mode
            DiagnosticGrade::B
        } else {
            // Doesn't support 48kHz
            DiagnosticGrade::C
        };

        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        let grade = if supports_48khz {
            DiagnosticGrade::B
        } else {
            DiagnosticGrade::C
        };

        DeviceDiagnostics {
            id: info.id.0.clone(),
            name: info.name.clone(),
            is_default: info.is_default,
            is_asio: info.is_asio,
            supported_sample_rates: info.supported_sample_rates.clone(),
            supports_48khz,
            supported_channels: info.supported_channels.clone(),
            grade,
        }
    }
}

/// Low-latency support detection
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LowLatencySupport {
    /// Whether ASIO is available (Windows)
    pub asio_available: bool,
    /// List of available ASIO devices
    pub asio_devices: Vec<String>,
    /// Whether the selected device supports 32-sample buffers
    pub supports_32_samples: bool,
    /// Whether the selected device supports 64-sample buffers
    pub supports_64_samples: bool,
    /// Whether the selected device supports 128-sample buffers
    pub supports_128_samples: bool,
    /// Minimum supported buffer size in samples
    pub min_buffer_size: Option<u32>,
    /// Estimated minimum latency in milliseconds (at 48kHz)
    pub estimated_min_latency_ms: Option<f64>,
}

impl Default for LowLatencySupport {
    fn default() -> Self {
        Self {
            asio_available: false,
            asio_devices: Vec::new(),
            supports_32_samples: false,
            supports_64_samples: false,
            supports_128_samples: true, // Assume most devices support 128
            min_buffer_size: Some(128),
            estimated_min_latency_ms: Some(2.67), // 128 samples at 48kHz
        }
    }
}

/// Where a diagnosed device came from
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeviceSource {
    /// Matches the device id configured in Settings
    Configured,
    /// Settings has no device configured, so the OS default was used
    OsDefault,
}

/// Complete audio diagnostics result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioDiagnosticsResult {
    /// Available input devices
    pub input_devices: Vec<DeviceDiagnostics>,
    /// Available output devices
    pub output_devices: Vec<DeviceDiagnostics>,
    /// Diagnostics for the input device streaming would actually use
    pub selected_input: Option<DeviceDiagnostics>,
    /// Diagnostics for the output device streaming would actually use
    pub selected_output: Option<DeviceDiagnostics>,
    /// Whether `selected_input` is the configured device or the OS default
    pub input_source: DeviceSource,
    /// Whether `selected_output` is the configured device or the OS default
    pub output_source: DeviceSource,
    /// Low-latency support information
    pub low_latency_support: LowLatencySupport,
    /// Overall audio grade
    pub overall_grade: DiagnosticGrade,
    /// Detected problems
    pub problems: Vec<DiagnosticProblem>,
}

/// Which side of the audio path a device read is for
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Input,
    Output,
}

impl Direction {
    fn reading(self) -> &'static str {
        match self {
            Direction::Input => "reading the input devices",
            Direction::Output => "reading the output devices",
        }
    }

    fn enumeration_failed(self, error: String) -> ProblemCode {
        match self {
            Direction::Input => ProblemCode::InputEnumerationFailed { error },
            Direction::Output => ProblemCode::OutputEnumerationFailed { error },
        }
    }

    fn unresponsive(self) -> ProblemCode {
        match self {
            Direction::Input => ProblemCode::InputDeviceUnresponsive,
            Direction::Output => ProblemCode::OutputDeviceUnresponsive,
        }
    }
}

/// What one side's calls into the audio driver gave: the devices, and the id
/// of the one streaming would use.
type DeviceRead = (Result<Vec<AudioDevice>, AudioError>, Option<String>);

/// Asks the audio driver for one side's devices, given the configured id
/// (`None`: the OS default). Every call in it can hang with the driver.
type ReadDevices = Arc<dyn Fn(Direction, Option<DeviceId>) -> DeviceRead + Send + Sync>;

fn read_devices_from_driver(direction: Direction, configured: Option<DeviceId>) -> DeviceRead {
    match direction {
        Direction::Input => (
            list_input_devices(),
            resolve_input_device(configured.as_ref())
                .ok()
                .and_then(|device| stable_device_id(&device)),
        ),
        Direction::Output => (
            list_output_devices(),
            resolve_output_device(configured.as_ref())
                .ok()
                .and_then(|device| stable_device_id(&device)),
        ),
    }
}

/// Reads one side's devices on a thread of its own, giving up at `timeout`,
/// so a driver that has hung costs neither this task nor the runtime's
/// threads more than that.
async fn read_devices_within(
    read: ReadDevices,
    direction: Direction,
    configured: Option<DeviceId>,
    timeout: Duration,
) -> Result<DeviceRead, AudioError> {
    tokio::task::spawn_blocking(move || {
        bounded(direction.reading(), timeout, move || {
            read(direction, configured)
        })
    })
    .await
    .unwrap_or_else(|e| Err(AudioError::StreamError(e.to_string())))
}

/// Audio diagnostics runner
pub struct AudioDiagnostics;

impl AudioDiagnostics {
    /// Run all audio diagnostics against the devices Settings has configured
    /// (`None` means Settings has nothing configured, so the OS default is
    /// used). Resolves through `resolve_input_device`/`resolve_output_device`,
    /// the same functions `streaming_start` uses, so the diagnostics tab
    /// reports on the device a call would actually use, not a different one.
    ///
    /// The calls into the audio driver are bounded (REQ-AUD-123): a side
    /// whose driver does not answer within [`LIST_TIMEOUT`] is reported as
    /// unresponsive, not as having no devices.
    pub async fn run(
        configured_input: Option<&str>,
        configured_output: Option<&str>,
    ) -> AudioDiagnosticsResult {
        Self::run_with(
            Arc::new(read_devices_from_driver),
            LIST_TIMEOUT,
            configured_input,
            configured_output,
        )
        .await
    }

    async fn run_with(
        read: ReadDevices,
        timeout: Duration,
        configured_input: Option<&str>,
        configured_output: Option<&str>,
    ) -> AudioDiagnosticsResult {
        let mut problems = Vec::new();

        // Resolve the input/output device the same way `streaming_start`
        // would: the configured id if there is one, otherwise the OS default.
        let (input_read, output_read) = tokio::join!(
            read_devices_within(
                read.clone(),
                Direction::Input,
                configured_input.map(|id| DeviceId(id.to_string())),
                timeout,
            ),
            read_devices_within(
                read,
                Direction::Output,
                configured_output.map(|id| DeviceId(id.to_string())),
                timeout,
            ),
        );

        let (input_devices, selected_input_id, input_answered) =
            Self::diagnose_side(Direction::Input, input_read, &mut problems);
        let (output_devices, selected_output_id, output_answered) =
            Self::diagnose_side(Direction::Output, output_read, &mut problems);

        let selected_input =
            selected_input_id.and_then(|id| input_devices.iter().find(|d| d.id == id).cloned());
        let selected_output =
            selected_output_id.and_then(|id| output_devices.iter().find(|d| d.id == id).cloned());

        let input_source = if configured_input.is_some() {
            DeviceSource::Configured
        } else {
            DeviceSource::OsDefault
        };
        let output_source = if configured_output.is_some() {
            DeviceSource::Configured
        } else {
            DeviceSource::OsDefault
        };

        // Check for no devices. A side that did not answer has not said it
        // has none.
        if input_answered && input_devices.is_empty() {
            problems.push(DiagnosticProblem {
                severity: ProblemSeverity::Error,
                category: "audio".to_string(),
                code: ProblemCode::NoInputDevices,
            });
        }

        if output_answered && output_devices.is_empty() {
            problems.push(DiagnosticProblem {
                severity: ProblemSeverity::Error,
                category: "audio".to_string(),
                code: ProblemCode::NoOutputDevices,
            });
        }

        // Check 48kHz support
        if let Some(ref input) = selected_input {
            if !input.supports_48khz {
                problems.push(DiagnosticProblem {
                    severity: ProblemSeverity::Warning,
                    category: "audio".to_string(),
                    code: ProblemCode::InputNot48kHz {
                        device_name: input.name.clone(),
                    },
                });
            }
        }

        if let Some(ref output) = selected_output {
            if !output.supports_48khz {
                problems.push(DiagnosticProblem {
                    severity: ProblemSeverity::Warning,
                    category: "audio".to_string(),
                    code: ProblemCode::OutputNot48kHz {
                        device_name: output.name.clone(),
                    },
                });
            }
        }

        // Detect low-latency support
        let low_latency_support = Self::detect_low_latency_support(&input_devices, &output_devices);

        // Add problems for limited low-latency support
        if !low_latency_support.supports_32_samples && !low_latency_support.supports_64_samples {
            problems.push(DiagnosticProblem {
                severity: ProblemSeverity::Warning,
                category: "audio".to_string(),
                code: ProblemCode::LowBufferUnsupported,
            });
        }

        #[cfg(target_os = "windows")]
        if !low_latency_support.asio_available {
            problems.push(DiagnosticProblem {
                severity: ProblemSeverity::Info,
                category: "audio".to_string(),
                code: ProblemCode::NoAsioDevices,
            });
        }

        // Calculate overall grade
        let overall_grade =
            Self::calculate_overall_grade(&selected_input, &selected_output, &low_latency_support);

        AudioDiagnosticsResult {
            input_devices,
            output_devices,
            selected_input,
            selected_output,
            input_source,
            output_source,
            low_latency_support,
            overall_grade,
            problems,
        }
    }

    /// One side's devices and the id of the one streaming would use, from
    /// what its read gave. The last is whether the driver answered, which is
    /// what "no devices" can be said on.
    fn diagnose_side(
        direction: Direction,
        read: Result<DeviceRead, AudioError>,
        problems: &mut Vec<DiagnosticProblem>,
    ) -> (Vec<DeviceDiagnostics>, Option<String>, bool) {
        let (listed, selected_id) = match read {
            Ok(read) => read,
            Err(AudioError::DeviceUnresponsive(what)) => {
                warn!("The audio driver did not answer while {}", what);
                problems.push(DiagnosticProblem {
                    severity: ProblemSeverity::Error,
                    category: "audio".to_string(),
                    code: direction.unresponsive(),
                });
                return (Vec::new(), None, false);
            }
            Err(e) => (Err(e), None),
        };
        let devices = match listed {
            Ok(devices) => devices
                .iter()
                .map(DeviceDiagnostics::from_audio_device)
                .collect(),
            Err(e) => {
                warn!("Failed to list {:?} devices: {}", direction, e);
                problems.push(DiagnosticProblem {
                    severity: ProblemSeverity::Error,
                    category: "audio".to_string(),
                    code: direction.enumeration_failed(e.to_string()),
                });
                Vec::new()
            }
        };
        (devices, selected_id, true)
    }

    /// Detect low-latency support
    fn detect_low_latency_support(
        input_devices: &[DeviceDiagnostics],
        output_devices: &[DeviceDiagnostics],
    ) -> LowLatencySupport {
        // Check for ASIO devices
        let asio_devices: Vec<String> = input_devices
            .iter()
            .chain(output_devices.iter())
            .filter(|d| d.is_asio)
            .map(|d| d.name.clone())
            .collect();
        let asio_available = !asio_devices.is_empty();

        // Determine buffer size support
        // This is platform-dependent and device-dependent
        // For now, we make conservative estimates

        #[cfg(target_os = "macos")]
        let (supports_32, supports_64, min_buffer) = {
            // macOS CoreAudio generally supports low buffer sizes
            (true, true, Some(32u32))
        };

        #[cfg(target_os = "windows")]
        let (supports_32, supports_64, min_buffer) = {
            if asio_available {
                // ASIO typically supports very low buffer sizes
                (true, true, Some(32u32))
            } else {
                // WASAPI shared mode has higher minimum
                (false, true, Some(64u32))
            }
        };

        #[cfg(target_os = "linux")]
        let (supports_32, supports_64, min_buffer) = {
            // Linux with PipeWire/JACK can support low latency
            (true, true, Some(32u32))
        };

        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        let (supports_32, supports_64, min_buffer) = {
            // Unknown platform, conservative estimate
            (false, false, Some(128u32))
        };

        // Calculate estimated minimum latency
        let estimated_min_latency_ms = min_buffer.map(|b| b as f64 / 48.0); // samples / (kHz)

        LowLatencySupport {
            asio_available,
            asio_devices,
            supports_32_samples: supports_32,
            supports_64_samples: supports_64,
            supports_128_samples: true,
            min_buffer_size: min_buffer,
            estimated_min_latency_ms,
        }
    }

    /// Calculate overall audio grade
    fn calculate_overall_grade(
        selected_input: &Option<DeviceDiagnostics>,
        selected_output: &Option<DeviceDiagnostics>,
        low_latency_support: &LowLatencySupport,
    ) -> DiagnosticGrade {
        // If we have no devices, grade is C
        if selected_input.is_none() || selected_output.is_none() {
            return DiagnosticGrade::C;
        }

        let input_grade = selected_input
            .as_ref()
            .map(|d| d.grade)
            .unwrap_or(DiagnosticGrade::C);
        let output_grade = selected_output
            .as_ref()
            .map(|d| d.grade)
            .unwrap_or(DiagnosticGrade::C);

        // Low-latency bonus
        let latency_bonus = if low_latency_support.supports_32_samples {
            20
        } else if low_latency_support.supports_64_samples {
            10
        } else {
            0
        };

        let total_score = (input_grade.to_score() + output_grade.to_score()) / 2 + latency_bonus;

        if total_score >= 90 {
            DiagnosticGrade::A
        } else if total_score >= 70 {
            DiagnosticGrade::B
        } else {
            DiagnosticGrade::C
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_low_latency_support_default() {
        let support = LowLatencySupport::default();
        assert!(support.supports_128_samples);
        assert!(!support.asio_available);
    }

    #[test]
    fn test_diagnostic_grade_scoring() {
        assert_eq!(DiagnosticGrade::A.to_score(), 100);
        assert_eq!(DiagnosticGrade::B.to_score(), 75);
        assert_eq!(DiagnosticGrade::C.to_score(), 50);
    }

    /// Reports the source as configured/OS-default independent of whether any
    /// audio hardware is present, so this runs everywhere.
    #[tokio::test]
    async fn test_run_reports_configured_source_when_a_device_id_is_given() {
        let result = AudioDiagnostics::run(Some("nonexistent-device-id"), None).await;
        assert_eq!(result.input_source, DeviceSource::Configured);
        assert_eq!(result.output_source, DeviceSource::OsDefault);
    }

    #[tokio::test]
    async fn test_run_reports_os_default_source_when_no_device_is_configured() {
        let result = AudioDiagnostics::run(None, None).await;
        assert_eq!(result.input_source, DeviceSource::OsDefault);
        assert_eq!(result.output_source, DeviceSource::OsDefault);
    }

    /// Proves diagnostics reports the *configured* device, not always the OS
    /// default, which is the regression this test guards against. Needs at
    /// least one non-default device to make the two distinguishable, so it
    /// skips on machines/CI runners with only one device or none - the GUI
    /// e2e suite covers this with the ALSA/PipeWire virtual devices set up by
    /// `tests/e2e/scripts/setup-virtual-audio-linux.sh`.
    #[tokio::test]
    async fn test_run_selects_configured_device_over_os_default_when_they_differ() {
        let inputs = list_input_devices().unwrap_or_default();
        let Some(non_default) = inputs.iter().find(|d| !d.is_default) else {
            eprintln!("skipping: no non-default input device available in this environment");
            return;
        };

        let result = AudioDiagnostics::run(Some(non_default.id.0.as_str()), None).await;

        assert_eq!(
            result.selected_input.map(|d| d.id),
            Some(non_default.id.0.clone()),
            "diagnostics must report the configured device, not the OS default"
        );
        assert_eq!(result.input_source, DeviceSource::Configured);
    }

    /// Reads answered from a table: `hang` makes that side never come back
    /// until the returned sender is dropped, as a hung driver does not.
    fn reader(hang: Option<Direction>) -> (ReadDevices, std::sync::mpsc::Sender<()>) {
        let (release, released) = std::sync::mpsc::channel::<()>();
        let released = std::sync::Mutex::new(released);
        let read: ReadDevices = Arc::new(move |direction, _| {
            if hang == Some(direction) {
                let _ = released.lock().unwrap().recv();
            }
            let device = AudioDevice {
                id: DeviceId(format!("{:?}", direction)),
                name: format!("{:?} device", direction),
                supported_sample_rates: vec![48000],
                supported_channels: vec![2],
                is_default: true,
                is_asio: false,
            };
            (Ok(vec![device]), Some(format!("{:?}", direction)))
        });
        (read, release)
    }

    fn problem_codes(result: &AudioDiagnosticsResult) -> Vec<String> {
        result
            .problems
            .iter()
            .map(|p| format!("{:?}", p.code))
            .collect()
    }

    /// The point of the limit: a task of the runtime that asks a hung driver
    /// must not be the one that waits, or the runtime stops with it. With a
    /// single worker, a ticker that keeps ticking shows nothing was blocked.
    ///
    /// Verifies: REQ-AUD-123
    #[tokio::test(flavor = "current_thread")]
    async fn when_the_driver_hangs_while_the_devices_are_read_the_runtime_keeps_running() {
        let (read, release) = reader(Some(Direction::Input));
        let ticks = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let ticker = {
            let ticks = ticks.clone();
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    ticks.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
            })
        };

        let result = AudioDiagnostics::run_with(read, Duration::from_millis(200), None, None).await;

        ticker.abort();
        drop(release);
        assert!(
            ticks.load(std::sync::atomic::Ordering::SeqCst) >= 10,
            "the runtime was blocked while the driver hung: {} ticks",
            ticks.load(std::sync::atomic::Ordering::SeqCst)
        );
        assert!(
            problem_codes(&result).contains(&"InputDeviceUnresponsive".to_string()),
            "a hung driver must be reported: {:?}",
            problem_codes(&result)
        );
    }

    /// Verifies: REQ-AUD-123
    #[tokio::test]
    async fn when_the_input_driver_hangs_the_input_is_reported_unresponsive_and_the_output_is_still_read(
    ) {
        let (read, release) = reader(Some(Direction::Input));

        let result = AudioDiagnostics::run_with(read, Duration::from_millis(200), None, None).await;

        drop(release);
        let codes = problem_codes(&result);
        assert!(codes.contains(&"InputDeviceUnresponsive".to_string()));
        assert!(
            !codes.contains(&"NoInputDevices".to_string()),
            "a driver that did not answer has not said it has no devices: {:?}",
            codes
        );
        assert!(result.input_devices.is_empty());
        assert!(result.selected_input.is_none());
        assert_eq!(result.output_devices.len(), 1);
        assert_eq!(
            result.selected_output.map(|d| d.id),
            Some("Output".to_string())
        );
        assert!(!codes.contains(&"OutputDeviceUnresponsive".to_string()));
    }

    /// Verifies: REQ-AUD-123
    #[tokio::test]
    async fn when_the_output_driver_hangs_the_output_is_reported_unresponsive() {
        let (read, release) = reader(Some(Direction::Output));

        let result = AudioDiagnostics::run_with(read, Duration::from_millis(200), None, None).await;

        drop(release);
        let codes = problem_codes(&result);
        assert!(codes.contains(&"OutputDeviceUnresponsive".to_string()));
        assert!(!codes.contains(&"NoOutputDevices".to_string()));
        assert!(result.selected_output.is_none());
        assert_eq!(result.input_devices.len(), 1);
    }

    /// Verifies: REQ-AUD-123
    #[tokio::test]
    async fn when_the_driver_answers_no_unresponsive_problem_is_reported() {
        let (read, _release) = reader(None);

        let result = AudioDiagnostics::run_with(read, Duration::from_secs(5), None, None).await;

        let codes = problem_codes(&result);
        assert!(!codes.contains(&"InputDeviceUnresponsive".to_string()));
        assert!(!codes.contains(&"OutputDeviceUnresponsive".to_string()));
        assert_eq!(
            result.selected_input.map(|d| d.id),
            Some("Input".to_string())
        );
    }
}
