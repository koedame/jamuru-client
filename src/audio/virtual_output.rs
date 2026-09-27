//! Recording/streaming virtual output device (Linux only; rm:1068).
//!
//! Publishes the self / peer / mix audio (`A` / `B` / `C`, 2ch each, 6ch
//! total) as a virtual device a DAW or OBS can pick as a recording input,
//! with no separate driver install. `pw-loopback` creates the device pair in
//! one child process: a sink face (`NODE_NAME_IN`, this app writes here) and
//! a source face (`NODE_NAME_OUT`, a DAW picks this), wired straight through.
//! Killing the process tears down both, so there is no node id to track or
//! `pw-cli destroy` to run.
//!
//! Getting the audio into the sink face goes through a second child process,
//! `pw-cat --playback`, fed over its stdin: linking `libpipewire` directly
//! (the `pipewire` crate) would need `libpipewire-0.3-dev` at build time,
//! which the desktop app's CI images do not carry today. `pw-cat` and
//! `pw-loopback` ship in the `pipewire` package already required for the
//! PipeWire backend used elsewhere (see `tests/e2e/scripts/setup-virtual-audio-linux.sh`).
//!
//! # Scope not covered here
//!
//! - macOS and Windows have no user-mode way to create a device at runtime;
//!   `start` returns [`AudioError::UnsupportedConfig`] there. Plans.md tracks
//!   the follow-up (a bundled, installer-provisioned Core Audio HAL plugin /
//!   virtual audio driver).
//! - This type only owns the child processes and the write pipe. Feeding it
//!   the `A`/`B`/`C` frames from the streaming session's audio callbacks is
//!   the next step (Plans.md).

use std::io::Write;
use std::process::{Child, Command, Stdio};

use super::error::AudioError;

/// Channels this device exposes: `A` (self, dry) + `B` (peer, dry) + `C`
/// (mix, post-fader), 2ch each.
pub const CHANNELS: usize = 6;

/// Node name a DAW or OBS picks as a recording input.
pub const NODE_NAME_OUT: &str = "jamjam-recording-output";

/// Node name this app writes to (the same virtual cable's other end).
const NODE_NAME_IN: &str = "jamjam-recording-output-in";

/// Owns the `pw-loopback` device pair and the `pw-cat` writer feeding it.
/// Dropping this kills both child processes, which tears down the PipeWire
/// nodes with them.
pub struct VirtualOutputSink {
    loopback: Child,
    writer: Child,
}

impl VirtualOutputSink {
    /// Starts the virtual device pair and the writer process. `sample_rate`
    /// must match what the streaming session's mix runs at: `pw-cat` is not
    /// told the source rate and cannot resample.
    #[cfg(target_os = "linux")]
    pub fn start(sample_rate: u32) -> Result<Self, AudioError> {
        let loopback = Command::new("pw-loopback")
            .arg("--name")
            .arg(NODE_NAME_OUT)
            .arg("--channels")
            .arg(CHANNELS.to_string())
            .arg("--capture-props")
            .arg(format!(
                "media.class=Audio/Sink node.name={NODE_NAME_IN} \
                 node.description=\"JamJam Recording Output\""
            ))
            .arg("--playback-props")
            .arg(format!(
                "media.class=Audio/Source node.name={NODE_NAME_OUT}"
            ))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| AudioError::PluginError(format!("pw-loopback did not start: {e}")))?;

        let writer = Command::new("pw-cat")
            .arg("--playback")
            .arg("--target")
            .arg(NODE_NAME_IN)
            .arg("--channels")
            .arg(CHANNELS.to_string())
            .arg("--rate")
            .arg(sample_rate.to_string())
            .arg("--format")
            .arg("f32")
            .arg("--raw")
            .arg("-")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| AudioError::PluginError(format!("pw-cat did not start: {e}")))?;

        Ok(Self { loopback, writer })
    }

    /// Same signature on every platform; unsupported ones fail to start
    /// rather than silently doing nothing, so a caller logs it once instead
    /// of it going unnoticed (`AudioError::UnsupportedConfig`).
    #[cfg(not(target_os = "linux"))]
    pub fn start(_sample_rate: u32) -> Result<Self, AudioError> {
        Err(AudioError::UnsupportedConfig(
            "recording/streaming virtual output is Linux-only for now".into(),
        ))
    }

    /// Writes one block of interleaved `CHANNELS`-wide `f32` frames.
    /// Blocking: call this from a dedicated writer thread, never from an
    /// audio callback (see the audio thread rules in `docs-spec/architecture.md`).
    pub fn write_frames(&mut self, interleaved: &[f32]) -> Result<(), AudioError> {
        debug_assert_eq!(interleaved.len() % CHANNELS, 0);
        let stdin = self.writer.stdin.as_mut().expect("stdin piped at spawn");
        let bytes = f32_to_le_bytes(interleaved);
        stdin
            .write_all(&bytes)
            .map_err(|e| AudioError::StreamError(format!("virtual output write failed: {e}")))
    }
}

impl Drop for VirtualOutputSink {
    fn drop(&mut self) {
        // Order does not matter: each is its own process and neither waits
        // on the other to exit.
        let _ = self.writer.kill();
        let _ = self.writer.wait();
        let _ = self.loopback.kill();
        let _ = self.loopback.wait();
    }
}

/// `f32` samples as their little-endian bytes, the wire format `pw-cat
/// --format f32 --raw` expects.
fn f32_to_le_bytes(samples: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len() * 4);
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_f32_samples_as_little_endian_bytes() {
        let samples = [1.0_f32, -1.0, 0.0];
        let bytes = f32_to_le_bytes(&samples);
        assert_eq!(bytes.len(), 12);
        assert_eq!(&bytes[0..4], &1.0_f32.to_le_bytes());
        assert_eq!(&bytes[4..8], &(-1.0_f32).to_le_bytes());
        assert_eq!(&bytes[8..12], &0.0_f32.to_le_bytes());
    }

    #[test]
    #[cfg(not(target_os = "linux"))]
    fn refuses_to_start_on_an_unsupported_platform() {
        let result = VirtualOutputSink::start(48_000);
        assert!(matches!(result, Err(AudioError::UnsupportedConfig(_))));
    }

    /// Only runs where `pw-loopback` / `pw-cat` are actually installed and a
    /// PipeWire daemon is reachable (`PIPEWIRE_REMOTE` or the default
    /// `XDG_RUNTIME_DIR/pipewire-0` socket); skips everywhere else rather
    /// than failing CI machines with no audio server.
    #[test]
    #[cfg(target_os = "linux")]
    fn starts_and_stops_against_a_real_pipewire_daemon() {
        if which("pw-loopback").is_none() || which("pw-cat").is_none() {
            eprintln!("skipping: pw-loopback/pw-cat not on PATH");
            return;
        }
        if !pipewire_reachable() {
            eprintln!("skipping: no PipeWire daemon reachable");
            return;
        }

        let mut sink = match VirtualOutputSink::start(48_000) {
            Ok(sink) => sink,
            Err(e) => {
                eprintln!("skipping: {e}");
                return;
            }
        };
        // Give pw-loopback a moment to register its nodes before writing.
        std::thread::sleep(std::time::Duration::from_millis(300));
        let silence = vec![0.0_f32; CHANNELS * 480];
        sink.write_frames(&silence).expect("write to a live pw-cat");
        drop(sink);
    }

    #[cfg(target_os = "linux")]
    fn which(bin: &str) -> Option<()> {
        std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|dir| dir.join(bin))
                .find(|path| path.is_file())
                .map(|_| ())
        })
    }

    #[cfg(target_os = "linux")]
    fn pipewire_reachable() -> bool {
        std::process::Command::new("pw-cli")
            .arg("info")
            .arg("0")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }
}
