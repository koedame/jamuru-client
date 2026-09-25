//! Diagnostics IPC commands for Tauri
//!
//! Provides commands for running self-diagnostics from the frontend.

use tauri::State;

use jamjam::diagnostics::{
    AudioDiagnostics, AudioDiagnosticsResult, CompleteDiagnosticsResult, CpuDiagnostics,
    CpuDiagnosticsResult, NetworkDiagnostics, NetworkDiagnosticsResult,
};
use jamjam::network::SignalingClient;

use crate::audio::audio_get_current_devices;
use crate::config::ConfigState;
use crate::device_identity::DeviceIdentityState;

/// The client the signaling check connects with: the configured server and
/// this installation's identity, as `signaling_connect` uses (ADR-030).
fn signaling_client(
    config_state: &ConfigState,
    identity_state: &DeviceIdentityState,
) -> SignalingClient {
    SignalingClient::new(&config_state.server_url(), identity_state.identity())
}

/// Run complete diagnostics (network, audio, CPU)
#[tauri::command]
pub async fn diagnostics_run_complete(
    config_state: State<'_, ConfigState>,
    identity_state: State<'_, DeviceIdentityState>,
) -> Result<CompleteDiagnosticsResult, String> {
    let signaling = signaling_client(&config_state, &identity_state);
    let devices = audio_get_current_devices(config_state.clone())?;
    let result = jamjam::diagnostics::run_complete_diagnostics(
        &signaling,
        devices.input_device_id.as_deref(),
        devices.output_device_id.as_deref(),
    )
    .await;
    Ok(result)
}

/// Run network diagnostics only
#[tauri::command]
pub async fn diagnostics_run_network(
    config_state: State<'_, ConfigState>,
    identity_state: State<'_, DeviceIdentityState>,
) -> Result<NetworkDiagnosticsResult, String> {
    let signaling = signaling_client(&config_state, &identity_state);
    let result = NetworkDiagnostics::run(&signaling).await;
    Ok(result)
}

/// Run audio diagnostics only
#[tauri::command]
pub fn diagnostics_run_audio(
    config_state: State<'_, ConfigState>,
) -> Result<AudioDiagnosticsResult, String> {
    let devices = audio_get_current_devices(config_state.clone())?;
    let result = AudioDiagnostics::run(
        devices.input_device_id.as_deref(),
        devices.output_device_id.as_deref(),
    );
    Ok(result)
}

/// Run CPU diagnostics only
#[tauri::command]
pub fn diagnostics_run_cpu() -> Result<CpuDiagnosticsResult, String> {
    let result = CpuDiagnostics::run();
    Ok(result)
}

/// Get recommended preset based on current diagnostics
#[tauri::command]
pub async fn diagnostics_get_recommended_preset(
    config_state: State<'_, ConfigState>,
    identity_state: State<'_, DeviceIdentityState>,
) -> Result<String, String> {
    let signaling = signaling_client(&config_state, &identity_state);
    let devices = audio_get_current_devices(config_state.clone())?;
    let result = jamjam::diagnostics::run_complete_diagnostics(
        &signaling,
        devices.input_device_id.as_deref(),
        devices.output_device_id.as_deref(),
    )
    .await;
    Ok(result.recommended_preset.as_str().to_string())
}

/// Check if zero-latency mode is compatible with current environment
#[tauri::command]
pub async fn diagnostics_check_zero_latency(
    config_state: State<'_, ConfigState>,
    identity_state: State<'_, DeviceIdentityState>,
) -> Result<bool, String> {
    let signaling = signaling_client(&config_state, &identity_state);
    let devices = audio_get_current_devices(config_state.clone())?;
    let result = jamjam::diagnostics::run_complete_diagnostics(
        &signaling,
        devices.input_device_id.as_deref(),
        devices.output_device_id.as_deref(),
    )
    .await;
    Ok(result.zero_latency_compatible)
}
