//! Self-diagnosis module for jamjam
//!
//! Provides environment diagnostics for users to verify their setup before
//! joining a session. Includes network, audio, and CPU performance checks.

mod audio;
mod cpu;
mod network;

pub use audio::{
    AudioDiagnostics, AudioDiagnosticsResult, DeviceDiagnostics, DeviceSource, LowLatencySupport,
};
pub use cpu::{CpuDiagnostics, CpuDiagnosticsResult};
pub use network::{
    ConnectionStability, IpSupport, NatType, NetworkDiagnostics, NetworkDiagnosticsResult,
    SignalingDiagnostics,
};

use serde::{Deserialize, Serialize};

/// Grade for diagnostic results (A is best, C is worst)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiagnosticGrade {
    /// Excellent - optimal for zero-latency mode
    A,
    /// Good - suitable for low-latency operation
    B,
    /// Fair - may experience issues, adjustments recommended
    C,
    /// Unknown - could not determine
    Unknown,
}

impl DiagnosticGrade {
    /// Convert grade to a numeric score (100, 75, 50, 0)
    pub fn to_score(self) -> u32 {
        match self {
            DiagnosticGrade::A => 100,
            DiagnosticGrade::B => 75,
            DiagnosticGrade::C => 50,
            DiagnosticGrade::Unknown => 0,
        }
    }
}

/// Recommended preset based on diagnostics
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecommendedPreset {
    /// Optimal for stable, low-jitter connections
    ZeroLatency,
    /// Good for LAN or stable connections
    UltraLowLatency,
    /// Balanced for typical internet connections
    Balanced,
    /// For unstable connections or recording
    HighQuality,
}

impl RecommendedPreset {
    /// Get the preset name as a string
    pub fn as_str(&self) -> &'static str {
        match self {
            RecommendedPreset::ZeroLatency => "zero-latency",
            RecommendedPreset::UltraLowLatency => "ultra-low-latency",
            RecommendedPreset::Balanced => "balanced",
            RecommendedPreset::HighQuality => "high-quality",
        }
    }
}

/// Problem detected during diagnostics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticProblem {
    /// Problem severity
    pub severity: ProblemSeverity,
    /// Problem category
    pub category: String,
    /// Problem type, with any data needed to render its message. The UI
    /// builds the localized message/suggestion text from this instead of
    /// receiving pre-rendered English strings.
    pub code: ProblemCode,
}

/// Kind of problem detected during diagnostics, carrying only the data the UI
/// needs to build a localized message (no natural-language text).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ProblemCode {
    /// No IPv4 or IPv6 connectivity at all
    NoConnectivity,
    /// IPv6 is not available (IPv4-only)
    NoIpv6,
    /// Symmetric NAT detected, making P2P harder
    SymmetricNat,
    /// Connection stability graded C
    UnstableConnection,
    /// Jitter above the acceptable threshold
    HighJitter {
        /// Measured jitter in milliseconds
        jitter_ms: f64,
    },
    /// Could not connect to the signaling server
    SignalingUnreachable {
        /// Signaling server URL that was unreachable
        url: String,
        /// Underlying error, if any (not localized; comes from the OS/library)
        error: Option<String>,
    },
    /// Failed to enumerate input audio devices
    InputEnumerationFailed {
        /// Underlying error (not localized)
        error: String,
    },
    /// Failed to enumerate output audio devices
    OutputEnumerationFailed {
        /// Underlying error (not localized)
        error: String,
    },
    /// The input side of the audio driver did not answer within the limit
    InputDeviceUnresponsive,
    /// The output side of the audio driver did not answer within the limit
    OutputDeviceUnresponsive,
    /// No input devices found
    NoInputDevices,
    /// No output devices found
    NoOutputDevices,
    /// Selected input device may not support 48kHz
    InputNot48kHz {
        /// Device name
        device_name: String,
    },
    /// Selected output device may not support 48kHz
    OutputNot48kHz {
        /// Device name
        device_name: String,
    },
    /// Neither 32- nor 64-sample buffers are supported
    LowBufferUnsupported,
    /// No ASIO devices detected (Windows)
    NoAsioDevices,
    /// CPU benchmark shows insufficient headroom for real-time processing
    InsufficientRealtimeHeadroom,
    /// CPU usage above the acceptable threshold
    HighCpuUsage {
        /// CPU usage as a percentage (0-100)
        usage_percent: f64,
    },
    /// Available memory below the acceptable threshold
    LowMemory {
        /// Available memory in megabytes
        available_mb: u64,
    },
    /// Smallest buffer size may cause audio glitches
    SmallBufferGlitchRisk,
}

/// Severity of a diagnostic problem
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProblemSeverity {
    /// Critical issue that may prevent operation
    Error,
    /// Issue that may cause degraded performance
    Warning,
    /// Informational note
    Info,
}

/// Complete diagnostics result combining all categories
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompleteDiagnosticsResult {
    /// Network diagnostics
    pub network: NetworkDiagnosticsResult,
    /// Audio diagnostics
    pub audio: AudioDiagnosticsResult,
    /// CPU diagnostics
    pub cpu: CpuDiagnosticsResult,
    /// Overall score (0-100)
    pub overall_score: u32,
    /// Recommended preset
    pub recommended_preset: RecommendedPreset,
    /// Zero-latency mode compatibility
    pub zero_latency_compatible: bool,
    /// Detected problems
    pub problems: Vec<DiagnosticProblem>,
}

/// Run complete diagnostics. `configured_input`/`configured_output` are the
/// device ids Settings has configured (`None` for "use the OS default"),
/// passed through to `AudioDiagnostics::run`.
pub async fn run_complete_diagnostics(
    signaling: &crate::network::SignalingClient,
    configured_input: Option<&str>,
    configured_output: Option<&str>,
) -> CompleteDiagnosticsResult {
    // Run diagnostics (network is async, others are sync)
    let network = NetworkDiagnostics::run(signaling).await;
    let audio = AudioDiagnostics::run(configured_input, configured_output).await;
    let cpu = CpuDiagnostics::run();

    // Calculate overall score (weighted average)
    // Network: 40%, Audio: 40%, CPU: 20%
    let network_score = network.connection_stability.to_score();
    let audio_score = audio.overall_grade.to_score();
    let cpu_score = cpu.grade.to_score();
    let overall_score = (network_score * 40 + audio_score * 40 + cpu_score * 20) / 100;

    // Determine recommended preset based on diagnostics
    let recommended_preset = determine_recommended_preset(&network, &audio, &cpu);

    // Check zero-latency compatibility
    let zero_latency_compatible = check_zero_latency_compatibility(&network, &audio, &cpu);

    // Collect all problems
    let mut problems = Vec::new();
    problems.extend(network.problems.clone());
    problems.extend(audio.problems.clone());
    problems.extend(cpu.problems.clone());

    CompleteDiagnosticsResult {
        network,
        audio,
        cpu,
        overall_score,
        recommended_preset,
        zero_latency_compatible,
        problems,
    }
}

/// Determine recommended preset based on diagnostic results
fn determine_recommended_preset(
    network: &NetworkDiagnosticsResult,
    audio: &AudioDiagnosticsResult,
    cpu: &CpuDiagnosticsResult,
) -> RecommendedPreset {
    // If network is unstable, recommend high-quality (more buffering)
    if network.connection_stability == DiagnosticGrade::C {
        return RecommendedPreset::HighQuality;
    }

    // If jitter is high, recommend balanced
    if let Some(jitter_ms) = network.jitter_ms {
        if jitter_ms > 10.0 {
            return RecommendedPreset::HighQuality;
        }
        if jitter_ms > 3.0 {
            return RecommendedPreset::Balanced;
        }
    }

    // If audio device doesn't support low buffer sizes, limit to balanced
    if !audio.low_latency_support.supports_32_samples {
        if !audio.low_latency_support.supports_64_samples {
            return RecommendedPreset::Balanced;
        }
        return RecommendedPreset::UltraLowLatency;
    }

    // If CPU is struggling, recommend balanced
    if cpu.grade == DiagnosticGrade::C {
        return RecommendedPreset::Balanced;
    }

    // If network is excellent (jitter < 1ms), recommend zero-latency
    if let Some(jitter_ms) = network.jitter_ms {
        if jitter_ms < 1.0 && network.connection_stability == DiagnosticGrade::A {
            return RecommendedPreset::ZeroLatency;
        }
    }

    // Default to ultra-low-latency for good conditions
    RecommendedPreset::UltraLowLatency
}

/// Check if zero-latency mode is compatible with current environment
fn check_zero_latency_compatibility(
    network: &NetworkDiagnosticsResult,
    audio: &AudioDiagnosticsResult,
    cpu: &CpuDiagnosticsResult,
) -> bool {
    // Jitter must be < 1ms
    let jitter_ok = network.jitter_ms.map(|j| j < 1.0).unwrap_or(false);

    // Network must be stable (grade A)
    let network_ok = network.connection_stability == DiagnosticGrade::A;

    // Audio must support 32 samples buffer
    let audio_ok = audio.low_latency_support.supports_32_samples;

    // CPU must be adequate (grade A or B)
    let cpu_ok = cpu.grade == DiagnosticGrade::A || cpu.grade == DiagnosticGrade::B;

    jitter_ok && network_ok && audio_ok && cpu_ok
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::audio::{DeviceDiagnostics, LowLatencySupport};
    use crate::diagnostics::cpu::{CpuBenchmarkResult, SystemResources};
    use crate::diagnostics::network::{
        ConnectionStability, IpSupport, NatType, SignalingDiagnostics,
    };

    /// Network diagnostics for a link with the given jitter and stability.
    fn network_with(jitter_ms: f64, stability: DiagnosticGrade) -> NetworkDiagnosticsResult {
        NetworkDiagnosticsResult {
            ip_support: IpSupport {
                ipv4_available: true,
                ipv6_available: false,
                ipv4_addresses: vec!["192.0.2.10".to_string()],
                ipv6_addresses: Vec::new(),
                public_ipv4: Some("198.51.100.10".to_string()),
                public_ipv6: None,
            },
            nat_type: NatType::FullCone,
            connection_stability: stability,
            jitter_ms: Some(jitter_ms),
            stability_metrics: ConnectionStability {
                avg_rtt_ms: Some(20.0),
                min_rtt_ms: Some(18.0),
                max_rtt_ms: Some(22.0),
                successful_probes: 10,
                failed_probes: 0,
                packet_loss_rate: 0.0,
            },
            signaling: SignalingDiagnostics {
                connected: true,
                connection_time_ms: Some(30),
                error: None,
            },
            problems: Vec::new(),
        }
    }

    /// Audio diagnostics for an interface that does or does not reach 32 samples.
    fn audio_with(supports_32: bool) -> AudioDiagnosticsResult {
        AudioDiagnosticsResult {
            input_devices: Vec::<DeviceDiagnostics>::new(),
            output_devices: Vec::new(),
            selected_input: None,
            selected_output: None,
            input_source: DeviceSource::OsDefault,
            output_source: DeviceSource::OsDefault,
            low_latency_support: LowLatencySupport {
                supports_32_samples: supports_32,
                supports_64_samples: true,
                min_buffer_size: Some(if supports_32 { 32 } else { 64 }),
                ..Default::default()
            },
            overall_grade: DiagnosticGrade::A,
            problems: Vec::new(),
        }
    }

    fn cpu_with(grade: DiagnosticGrade) -> CpuDiagnosticsResult {
        CpuDiagnosticsResult {
            benchmarks: vec![CpuBenchmarkResult {
                processing_time_us: 100.0,
                frame_duration_us: 667.0,
                realtime_factor: 0.15,
                buffer_size: 32,
            }],
            system: SystemResources {
                cpu_cores: 8,
                cpu_usage: Some(0.2),
                available_memory_mb: Some(8192),
            },
            grade,
            realtime_capable: true,
            problems: Vec::new(),
        }
    }

    /// Given jitter of 0.5ms on a stable link
    /// Then zero-latency is the recommended preset
    ///
    /// Verifies: REQ-LAT-104
    #[test]
    fn low_jitter_recommends_zero_latency() {
        let network = network_with(0.5, DiagnosticGrade::A);
        let audio = audio_with(true);
        let cpu = cpu_with(DiagnosticGrade::A);

        assert_eq!(
            determine_recommended_preset(&network, &audio, &cpu),
            RecommendedPreset::ZeroLatency
        );
        assert!(
            check_zero_latency_compatibility(&network, &audio, &cpu),
            "a 0.5ms jitter link with 32-sample support must be zero-latency capable"
        );
    }

    /// Given jitter of 15ms on an unstable link
    /// Then a preset with more buffering is recommended instead of zero-latency
    ///
    /// Verifies: REQ-LAT-105
    #[test]
    fn high_jitter_recommends_more_buffering() {
        let network = network_with(15.0, DiagnosticGrade::A);
        let audio = audio_with(true);
        let cpu = cpu_with(DiagnosticGrade::A);

        let recommended = determine_recommended_preset(&network, &audio, &cpu);
        assert_ne!(
            recommended,
            RecommendedPreset::ZeroLatency,
            "15ms of jitter must not recommend zero-latency"
        );
        assert!(
            matches!(
                recommended,
                RecommendedPreset::Balanced | RecommendedPreset::HighQuality
            ),
            "15ms of jitter should recommend a buffered preset, got {:?}",
            recommended
        );
        assert!(
            !check_zero_latency_compatibility(&network, &audio, &cpu),
            "zero-latency must be reported as incompatible at 15ms of jitter"
        );
    }

    /// Moderate jitter sits between the two extremes: buffered, but not the
    /// most conservative preset.
    #[test]
    fn moderate_jitter_recommends_balanced() {
        let network = network_with(5.0, DiagnosticGrade::A);
        assert_eq!(
            determine_recommended_preset(
                &network,
                &audio_with(true),
                &cpu_with(DiagnosticGrade::A)
            ),
            RecommendedPreset::Balanced
        );
    }
}
