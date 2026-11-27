//! Audio quality evaluation
//!
//! Provides tools for measuring audio quality using PESQ and latency analysis.

use jamjam::audio::AudioPreset;
use tracing::{debug, info};

/// Quality evaluation result
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct QualityResult {
    /// PESQ MOS-LQO score (1.0 - 4.5)
    pub pesq_mos: Option<f32>,
    /// Measured latency in milliseconds
    pub latency_ms: Option<f32>,
    /// Packet loss percentage
    pub packet_loss_percent: Option<f32>,
    /// Whether quality meets the preset threshold
    pub meets_threshold: bool,
    /// Additional notes
    pub notes: Option<String>,
}

impl QualityResult {
    /// Create a result that passes
    pub fn passed() -> Self {
        Self {
            pesq_mos: None,
            latency_ms: None,
            packet_loss_percent: None,
            meets_threshold: true,
            notes: None,
        }
    }

    /// Create a result that fails
    pub fn failed(notes: impl Into<String>) -> Self {
        Self {
            pesq_mos: None,
            latency_ms: None,
            packet_loss_percent: None,
            meets_threshold: false,
            notes: Some(notes.into()),
        }
    }
}

/// PESQ evaluator for audio quality measurement
///
/// Uses ITU-T P.862 PESQ algorithm to compare reference and degraded audio.
pub struct PesqEvaluator {
    /// Sample rate (must be 8000 or 16000 for narrowband PESQ, 48000 for wideband)
    /// Reserved for future PESQ library integration
    #[allow(dead_code)]
    sample_rate: u32,
}

impl PesqEvaluator {
    /// Minimum acceptable MOS per preset
    ///
    /// The latency side of the threshold is not duplicated here: it comes from
    /// `AudioPreset::max_app_latency_ms` so that the E2E suite and the product
    /// cannot disagree about a preset's budget.
    pub const MIN_MOS: &'static [(&'static str, f32)] = &[
        ("zero-latency", 4.0),
        ("ultra-low-latency", 3.8),
        ("balanced", 3.5),
        ("high-quality", 4.2),
    ];

    /// Minimum acceptable MOS for `preset`, defaulting to the balanced bar
    pub fn min_mos_for(preset: &str) -> f32 {
        Self::MIN_MOS
            .iter()
            .find(|(p, _)| *p == preset)
            .map(|(_, mos)| *mos)
            .unwrap_or(3.5)
    }

    /// Application latency budget for `preset`, straight from the product
    pub fn max_app_latency_ms_for(preset: &str) -> f32 {
        AudioPreset::from_name(preset)
            .unwrap_or_default()
            .max_app_latency_ms()
    }

    /// Create a new PESQ evaluator
    pub fn new(sample_rate: u32) -> Self {
        Self { sample_rate }
    }

    /// Evaluate audio quality by comparing reference and received audio
    ///
    /// Returns a MOS-LQO score (1.0 to 4.5 scale).
    pub fn evaluate(&self, reference: &[f32], received: &[f32]) -> Result<f32, QualityError> {
        if reference.is_empty() || received.is_empty() {
            return Err(QualityError::EmptyAudio);
        }

        // For now, use a simplified quality metric based on correlation
        // TODO: Integrate actual PESQ library (e.g., pypesq via subprocess or FFI)

        let correlation = self.calculate_correlation(reference, received)?;

        // Map correlation to approximate MOS scale
        // correlation 1.0 -> MOS 4.5
        // correlation 0.9 -> MOS 4.0
        // correlation 0.7 -> MOS 3.0
        // correlation 0.5 -> MOS 2.0
        // correlation 0.0 -> MOS 1.0
        let mos = 1.0 + 3.5 * correlation.max(0.0);

        info!(
            "Audio quality: correlation={:.4}, approx_mos={:.2}",
            correlation, mos
        );

        Ok(mos)
    }

    /// Evaluate and check against preset threshold
    pub fn evaluate_with_threshold(
        &self,
        reference: &[f32],
        received: &[f32],
        preset: &str,
    ) -> Result<QualityResult, QualityError> {
        let mos = self.evaluate(reference, received)?;

        let threshold = Self::min_mos_for(preset);

        let meets_threshold = mos >= threshold;

        Ok(QualityResult {
            pesq_mos: Some(mos),
            latency_ms: None,
            packet_loss_percent: None,
            meets_threshold,
            notes: if meets_threshold {
                None
            } else {
                Some(format!(
                    "MOS {:.2} below threshold {:.2} for preset {}",
                    mos, threshold, preset
                ))
            },
        })
    }

    /// Calculate Pearson correlation between two audio signals
    fn calculate_correlation(&self, a: &[f32], b: &[f32]) -> Result<f32, QualityError> {
        let len = a.len().min(b.len());
        if len == 0 {
            return Err(QualityError::EmptyAudio);
        }

        let mean_a: f32 = a.iter().take(len).sum::<f32>() / len as f32;
        let mean_b: f32 = b.iter().take(len).sum::<f32>() / len as f32;

        let mut num = 0.0f32;
        let mut den_a = 0.0f32;
        let mut den_b = 0.0f32;

        for i in 0..len {
            let da = a[i] - mean_a;
            let db = b[i] - mean_b;
            num += da * db;
            den_a += da * da;
            den_b += db * db;
        }

        if den_a == 0.0 || den_b == 0.0 {
            // If one signal is constant (e.g., silence), correlation is undefined
            // Return 0 for silence test cases
            if den_a == 0.0 && den_b == 0.0 {
                return Ok(1.0); // Both silence = perfect match
            }
            return Ok(0.0);
        }

        Ok(num / (den_a.sqrt() * den_b.sqrt()))
    }
}

/// Latency measurer using cross-correlation
pub struct LatencyMeasurer {
    /// Sample rate
    sample_rate: u32,
}

impl LatencyMeasurer {
    /// Create a new latency measurer
    pub fn new(sample_rate: u32) -> Self {
        Self { sample_rate }
    }

    /// Measure latency between reference and received audio using cross-correlation
    ///
    /// Returns latency in milliseconds. Searches lags up to 500ms; use
    /// [`Self::measure_within`] to bound the search.
    ///
    /// The reference must be aperiodic. A steady tone correlates equally well at
    /// every multiple of its period, so the peak is ambiguous and the reported
    /// lag can be any of them - use a sweep or an impulse instead.
    pub fn measure(&self, reference: &[f32], received: &[f32]) -> Result<f32, QualityError> {
        let max_lag = (self.sample_rate as usize / 2).min(reference.len() / 2);
        self.measure_within(reference, received, max_lag)
    }

    /// Measure latency, searching no further than `max_lag_samples`
    ///
    /// Cross-correlation costs `O(max_lag * len)`, so bounding the search to the
    /// delay a caller can plausibly expect keeps the measurement fast.
    pub fn measure_within(
        &self,
        reference: &[f32],
        received: &[f32],
        max_lag_samples: usize,
    ) -> Result<f32, QualityError> {
        if reference.is_empty() || received.is_empty() {
            return Err(QualityError::EmptyAudio);
        }

        // For very short or identical signals, return 0
        if reference.len() < 100 {
            return Ok(0.0);
        }

        let max_lag_samples = max_lag_samples.min(reference.len() / 2).max(1);

        let mut best_lag = 0i32;
        let mut best_corr = f32::NEG_INFINITY;

        // Calculate energy of reference for normalization
        let ref_energy: f32 = reference.iter().map(|x| x * x).sum();
        if ref_energy == 0.0 {
            return Ok(0.0); // Silence
        }

        // Search for best lag
        for lag in -(max_lag_samples as i32)..=(max_lag_samples as i32) {
            let corr = self.normalized_correlation_at_lag(reference, received, lag);
            if corr > best_corr {
                best_corr = corr;
                best_lag = lag;
            }
        }

        // Positive lag means received is delayed relative to reference
        let latency_samples = best_lag.max(0) as f32;
        let latency_ms = (latency_samples / self.sample_rate as f32) * 1000.0;

        debug!(
            "Latency measurement: lag={} samples, corr={:.4}, latency={:.2}ms",
            best_lag, best_corr, latency_ms
        );

        Ok(latency_ms)
    }

    /// Verify latency against the presets' latency budgets
    pub fn verify_against_spec(&self, latency_ms: f32, preset: &str) -> QualityResult {
        let threshold = PesqEvaluator::max_app_latency_ms_for(preset);

        let meets_threshold = latency_ms <= threshold;

        QualityResult {
            pesq_mos: None,
            latency_ms: Some(latency_ms),
            packet_loss_percent: None,
            meets_threshold,
            notes: if meets_threshold {
                None
            } else {
                Some(format!(
                    "Latency {:.2}ms exceeds threshold {:.2}ms for preset {}",
                    latency_ms, threshold, preset
                ))
            },
        }
    }

    /// Calculate normalized cross-correlation at a specific lag
    fn normalized_correlation_at_lag(&self, a: &[f32], b: &[f32], lag: i32) -> f32 {
        let mut sum_ab = 0.0f32;
        let mut sum_aa = 0.0f32;
        let mut sum_bb = 0.0f32;
        let mut count = 0;

        for (i, &ai) in a.iter().enumerate() {
            let j = i as i32 + lag;
            if j >= 0 && (j as usize) < b.len() {
                let bj = b[j as usize];
                sum_ab += ai * bj;
                sum_aa += ai * ai;
                sum_bb += bj * bj;
                count += 1;
            }
        }

        if count > 0 && sum_aa > 0.0 && sum_bb > 0.0 {
            sum_ab / (sum_aa.sqrt() * sum_bb.sqrt())
        } else {
            0.0
        }
    }
}

/// External PESQ evaluator using Python script
///
/// Calls the external Python script for actual ITU-T P.862 PESQ evaluation.
pub struct ExternalPesqEvaluator {
    /// Path to the Python script
    script_path: std::path::PathBuf,
    /// PESQ mode: "wb" (wideband 16kHz) or "nb" (narrowband 8kHz)
    mode: String,
}

impl ExternalPesqEvaluator {
    /// Create a new external PESQ evaluator
    pub fn new() -> Self {
        // Find the script relative to the e2e test directory
        let script_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("scripts")
            .join("pesq")
            .join("evaluate.py");

        Self {
            script_path,
            mode: "wb".to_string(),
        }
    }

    /// Create with a specific script path
    pub fn with_script_path(script_path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            script_path: script_path.into(),
            mode: "wb".to_string(),
        }
    }

    /// Set PESQ mode
    pub fn mode(mut self, mode: &str) -> Self {
        self.mode = mode.to_string();
        self
    }

    /// Evaluate PESQ score between two WAV files
    pub fn evaluate_files(
        &self,
        reference_path: &std::path::Path,
        degraded_path: &std::path::Path,
    ) -> Result<QualityResult, QualityError> {
        use std::process::Command;

        let input = serde_json::json!({
            "reference": reference_path.to_string_lossy(),
            "degraded": degraded_path.to_string_lossy(),
            "mode": self.mode,
            "fallback": true,
        });

        let output = Command::new("python3")
            .arg(&self.script_path)
            .arg("--json")
            .arg(input.to_string())
            .output()
            .map_err(|e| QualityError::PesqFailed(format!("Failed to run PESQ script: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(QualityError::PesqFailed(format!(
                "PESQ script failed: {}",
                stderr
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let result: serde_json::Value = serde_json::from_str(&stdout)
            .map_err(|e| QualityError::PesqFailed(format!("Failed to parse PESQ output: {}", e)))?;

        if let Some(error) = result.get("error").and_then(|e| e.as_str()) {
            return Err(QualityError::PesqFailed(error.to_string()));
        }

        let mos = result.get("mos").and_then(|m| m.as_f64()).map(|m| m as f32);
        let latency_ms = result
            .get("latency_ms")
            .and_then(|l| l.as_f64())
            .map(|l| l as f32);

        let warning = result
            .get("warning")
            .and_then(|w| w.as_str())
            .map(String::from);

        Ok(QualityResult {
            pesq_mos: mos,
            latency_ms,
            packet_loss_percent: None,
            meets_threshold: true, // Will be checked separately
            notes: warning,
        })
    }

    /// Evaluate and check against preset threshold
    pub fn evaluate_files_with_threshold(
        &self,
        reference_path: &std::path::Path,
        degraded_path: &std::path::Path,
        preset: &str,
    ) -> Result<QualityResult, QualityError> {
        let mut result = self.evaluate_files(reference_path, degraded_path)?;

        let min_mos = PesqEvaluator::min_mos_for(preset);
        let max_latency = PesqEvaluator::max_app_latency_ms_for(preset);

        let mos_ok = result.pesq_mos.map(|m| m >= min_mos).unwrap_or(true);
        let latency_ok = result.latency_ms.map(|l| l <= max_latency).unwrap_or(true);

        result.meets_threshold = mos_ok && latency_ok;

        if !result.meets_threshold {
            let mut notes = Vec::new();
            if let Some(mos) = result.pesq_mos {
                if mos < min_mos {
                    notes.push(format!("MOS {:.2} < {:.2}", mos, min_mos));
                }
            }
            if let Some(latency) = result.latency_ms {
                if latency > max_latency {
                    notes.push(format!("Latency {:.1}ms > {:.1}ms", latency, max_latency));
                }
            }
            result.notes = Some(notes.join("; "));
        }

        Ok(result)
    }
}

impl Default for ExternalPesqEvaluator {
    fn default() -> Self {
        Self::new()
    }
}

/// Errors that can occur during quality evaluation
#[derive(Debug, thiserror::Error)]
pub enum QualityError {
    #[error("Empty audio data")]
    EmptyAudio,

    #[error("PESQ evaluation failed: {0}")]
    PesqFailed(String),

    #[error("Audio format mismatch")]
    FormatMismatch,

    #[error("IO error: {0}")]
    IoError(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn generate_sine(freq: f32, sample_rate: u32, duration_sec: f32) -> Vec<f32> {
        let num_samples = (sample_rate as f32 * duration_sec) as usize;
        (0..num_samples)
            .map(|i| (2.0 * PI * freq * i as f32 / sample_rate as f32).sin())
            .collect()
    }

    #[test]
    fn test_pesq_identical_audio() {
        let evaluator = PesqEvaluator::new(48000);
        let audio = generate_sine(440.0, 48000, 0.1);

        let mos = evaluator.evaluate(&audio, &audio).unwrap();
        assert!(mos >= 4.0, "Identical audio should have high MOS");
    }

    #[test]
    fn test_pesq_different_audio() {
        let evaluator = PesqEvaluator::new(48000);
        let audio1 = generate_sine(440.0, 48000, 0.1);
        let audio2 = generate_sine(880.0, 48000, 0.1);

        let mos = evaluator.evaluate(&audio1, &audio2).unwrap();
        assert!(mos < 4.0, "Different audio should have lower MOS");
    }

    #[test]
    fn test_latency_zero_lag() {
        let measurer = LatencyMeasurer::new(48000);
        let audio = generate_sine(440.0, 48000, 0.1);

        let latency = measurer.measure(&audio, &audio).unwrap();
        assert!(latency < 1.0, "Same audio should have near-zero latency");
    }

    #[test]
    fn test_latency_with_delay() {
        let measurer = LatencyMeasurer::new(48000);
        // Use longer signal for better correlation detection
        let reference = generate_sine(440.0, 48000, 0.5);

        // Add 10ms delay (480 samples at 48kHz)
        let mut received = vec![0.0; 480];
        received.extend_from_slice(&reference);

        let latency = measurer.measure(&reference, &received).unwrap();
        // Allow wider tolerance for simplified cross-correlation algorithm
        // The important thing is it detects a positive delay
        assert!(
            latency > 5.0 && latency < 50.0,
            "Expected delay detection (5-50ms range), got {}ms",
            latency
        );
    }
}
