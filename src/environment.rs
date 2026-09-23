//! What the machine and its audio stack look like
//!
//! Coarse facts that explain why a setup behaves the way it does: OS version,
//! installed memory, which audio host is in use and how small a buffer a device
//! accepts. Every probe answers `None` when the platform can't tell, so one
//! missing fact never costs the others.
//!
//! Deliberately left out: CPU model, host name, user name and MAC address.

use cpal::traits::DeviceTrait;
use cpal::SupportedBufferSize;

/// OS version as `major.minor` (`14.6`), or just `major` where the OS has no
/// minor (`11` on Windows). The build number is dropped.
pub fn os_version() -> Option<String> {
    sysinfo::System::os_version().and_then(|raw| major_minor(&raw))
}

/// Installed memory in GB, rounded to a power of two (`16`, not `15.6`).
pub fn ram_gb() -> Option<u32> {
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    round_ram_gb(system.total_memory())
}

/// Name of the audio host jamjam opens devices through: `coreaudio`, `wasapi`,
/// `asio`, `alsa`, `jack`, ...
pub fn audio_host() -> String {
    cpal::default_host().id().to_string()
}

/// Smallest buffer, in frames, the device accepts for capture. `None` when the
/// driver doesn't report a range.
pub fn min_input_buffer_frames(device: &cpal::Device) -> Option<u32> {
    let configs = device.supported_input_configs().ok()?;
    smallest_buffer_frames(configs.map(|c| *c.buffer_size()))
}

/// Smallest buffer, in frames, the device accepts for playback. `None` when the
/// driver doesn't report a range.
pub fn min_output_buffer_frames(device: &cpal::Device) -> Option<u32> {
    let configs = device.supported_output_configs().ok()?;
    smallest_buffer_frames(configs.map(|c| *c.buffer_size()))
}

/// Keeps the leading numeric dot-separated parts of a version, at most two.
/// Anything after the first space (Windows appends the build as `11 (22631)`)
/// is dropped, and so is a version with no leading number (`rolling`).
fn major_minor(raw: &str) -> Option<String> {
    let version = raw.split_whitespace().next()?;
    let parts: Vec<&str> = version
        .split('.')
        .map_while(|part| {
            (!part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())).then_some(part)
        })
        .take(2)
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("."))
    }
}

/// Nearest power of two to the installed memory, measured on a log scale. The
/// OS reports a little less than what is installed (15.6 GB for a 16 GB
/// machine), and integrated graphics take a share of it, so a plain rounding to
/// the nearest GB would name a size nobody bought.
fn round_ram_gb(total_bytes: u64) -> Option<u32> {
    if total_bytes == 0 {
        return None;
    }
    let gb = total_bytes as f64 / (1u64 << 30) as f64;
    let exponent = gb.log2().round().max(0.0) as u32;
    2u32.checked_pow(exponent)
}

/// Smallest lower bound among the reported buffer ranges. A range starting at 0
/// is what a driver reports when it doesn't know, so it doesn't count.
fn smallest_buffer_frames(sizes: impl IntoIterator<Item = SupportedBufferSize>) -> Option<u32> {
    sizes
        .into_iter()
        .filter_map(|size| match size {
            SupportedBufferSize::Range { min, .. } if min > 0 => Some(min),
            _ => None,
        })
        .min()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpal::traits::HostTrait;

    #[test]
    fn macos_style_version_with_patch_level_keeps_major_and_minor() {
        assert_eq!(major_minor("14.6.1").as_deref(), Some("14.6"));
    }

    #[test]
    fn version_that_already_is_major_minor_stays_as_it_is() {
        assert_eq!(major_minor("24.04").as_deref(), Some("24.04"));
    }

    #[test]
    fn windows_style_version_with_build_number_keeps_only_the_major() {
        assert_eq!(major_minor("11 (22631)").as_deref(), Some("11"));
    }

    #[test]
    fn version_with_only_a_major_stays_a_major() {
        assert_eq!(major_minor("12").as_deref(), Some("12"));
    }

    #[test]
    fn version_that_is_not_a_number_yields_nothing() {
        assert_eq!(major_minor("rolling"), None);
        assert_eq!(major_minor(""), None);
        assert_eq!(major_minor("   "), None);
    }

    #[test]
    fn version_with_a_non_numeric_tail_keeps_the_numeric_head() {
        assert_eq!(major_minor("6.1.rc2").as_deref(), Some("6.1"));
    }

    #[test]
    fn memory_a_little_under_a_power_of_two_rounds_up_to_it() {
        let fifteen_point_six_gb = (15.6 * (1u64 << 30) as f64) as u64;
        assert_eq!(round_ram_gb(fifteen_point_six_gb), Some(16));
    }

    #[test]
    fn memory_of_exactly_a_power_of_two_stays_put() {
        assert_eq!(round_ram_gb(8 << 30), Some(8));
        assert_eq!(round_ram_gb(32 << 30), Some(32));
    }

    #[test]
    fn memory_between_two_powers_rounds_to_the_nearer_one_on_a_log_scale() {
        // 18 GB (an unusual Mac configuration) is nearer to 16 than to 32.
        assert_eq!(round_ram_gb(18 << 30), Some(16));
        // 24 GB is past the geometric midpoint of 16 and 32 (22.6 GB).
        assert_eq!(round_ram_gb(24 << 30), Some(32));
    }

    #[test]
    fn memory_under_one_gb_rounds_to_one() {
        assert_eq!(round_ram_gb(512 << 20), Some(1));
    }

    #[test]
    fn memory_reported_as_zero_yields_nothing() {
        assert_eq!(round_ram_gb(0), None);
    }

    #[test]
    fn device_reporting_several_ranges_gives_the_smallest_lower_bound() {
        let sizes = [
            SupportedBufferSize::Range {
                min: 128,
                max: 4096,
            },
            SupportedBufferSize::Range { min: 32, max: 1024 },
            SupportedBufferSize::Range { min: 64, max: 2048 },
        ];
        assert_eq!(smallest_buffer_frames(sizes), Some(32));
    }

    #[test]
    fn device_reporting_no_buffer_range_gives_nothing() {
        assert_eq!(
            smallest_buffer_frames([SupportedBufferSize::Unknown, SupportedBufferSize::Unknown]),
            None
        );
        assert_eq!(smallest_buffer_frames([]), None);
    }

    #[test]
    fn range_starting_at_zero_is_ignored_in_favour_of_a_real_one() {
        let sizes = [
            SupportedBufferSize::Range { min: 0, max: 4096 },
            SupportedBufferSize::Unknown,
            SupportedBufferSize::Range {
                min: 256,
                max: 4096,
            },
        ];
        assert_eq!(smallest_buffer_frames(sizes), Some(256));
    }

    #[test]
    fn os_version_when_reported_is_major_or_major_minor_only() {
        // A container may have no os-release version, so `None` is fine; what
        // must never come back is a build number or a free-form string.
        if let Some(version) = os_version() {
            let parts: Vec<&str> = version.split('.').collect();
            assert!(
                (1..=2).contains(&parts.len())
                    && parts
                        .iter()
                        .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())),
                "unexpected os_version {version:?}"
            );
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    #[test]
    fn ram_gb_on_a_desktop_os_is_a_power_of_two() {
        let gb = ram_gb().expect("desktop OSes report installed memory");
        assert!(gb.is_power_of_two(), "{gb} is not a power of two");
    }

    #[test]
    fn audio_host_on_this_os_is_its_native_host() {
        #[cfg(target_os = "linux")]
        assert_eq!(audio_host(), "alsa");
        #[cfg(target_os = "macos")]
        assert_eq!(audio_host(), "coreaudio");
        #[cfg(target_os = "windows")]
        assert_eq!(audio_host(), "wasapi");
    }

    /// CI runners have no audio hardware, so the default devices may not exist;
    /// asking for them must just come back empty, not fail.
    #[test]
    fn min_buffer_frames_of_the_default_devices_never_fails_and_is_never_zero() {
        let host = cpal::default_host();
        let inputs = host
            .default_input_device()
            .and_then(|d| min_input_buffer_frames(&d));
        let outputs = host
            .default_output_device()
            .and_then(|d| min_output_buffer_frames(&d));
        for frames in inputs.into_iter().chain(outputs) {
            assert!(frames > 0);
        }
    }
}
