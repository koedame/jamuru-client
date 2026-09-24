//! Which channels of a multi-channel audio interface the app reads and writes.
//!
//! The settings count channels from 1, the way an interface labels its
//! inputs. Everything here counts from 0, the way a frame is indexed.

/// The channels capture delivers, as indexes into a device frame.
///
/// One channel for a mono transmit, two for stereo. A stereo transmit with no
/// right channel chosen sends the left one on both sides.
pub fn capture_picks(left: u32, right: Option<u32>, wanted: u16) -> Vec<usize> {
    let left = index(left);
    if wanted >= 2 {
        vec![left, right.map_or(left, index)]
    } else {
        vec![left]
    }
}

/// What to try, in order, when opening a device for capture.
///
/// The selection the user made comes first. A device that has no such channel
/// (the interface was swapped for a laptop microphone) falls back to the first
/// channels at the same width, and a device that will not open two channels
/// falls back to its first one, so a stale setting never leaves a session
/// without input.
pub fn capture_attempts(left: u32, right: Option<u32>, wanted: u16) -> Vec<Vec<usize>> {
    let mut attempts = vec![capture_picks(left, right, wanted)];
    let first_channels: &[Vec<usize>] = if wanted >= 2 {
        &[vec![0, 1], vec![0]]
    } else {
        &[vec![0]]
    };
    for fallback in first_channels {
        if !attempts.contains(fallback) {
            attempts.push(fallback.clone());
        }
    }
    attempts
}

/// Copies the `picks` channels out of interleaved frames of `device_channels`
/// into `out`, interleaved in the order of `picks`.
///
/// `out` keeps its allocation between calls, so this allocates only if a
/// device hands over more than it has before.
pub fn pick_channels(input: &[f32], device_channels: usize, picks: &[usize], out: &mut Vec<f32>) {
    out.clear();
    for frame in input.chunks_exact(device_channels) {
        out.extend(picks.iter().map(|&channel| frame[channel]));
    }
}

/// Where the stereo the app plays lands on an output device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputRoute {
    /// Device channel the left side plays on.
    pub left: usize,
    /// Device channel the right side plays on. `None` plays both sides mixed
    /// on `left`, for an output that is a single channel.
    pub right: Option<usize>,
}

impl Default for OutputRoute {
    /// The first two channels, the way a stereo device is normally heard.
    fn default() -> Self {
        Self {
            left: 0,
            right: Some(1),
        }
    }
}

impl OutputRoute {
    /// The route the output channel settings describe.
    pub fn from_settings(left: u32, right: Option<u32>) -> Self {
        Self {
            left: index(left),
            right: right.map(index),
        }
    }

    /// Fewest channels a device needs to play this route.
    pub fn channels_needed(&self) -> usize {
        self.left.max(self.right.unwrap_or(0)) + 1
    }

    /// Whether the device frame is the stereo frame, channel for channel.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// Writes interleaved stereo `wire` to `out`, frames of `device_channels`,
/// with each side on the channel `route` names and every other channel silent.
pub fn place_stereo(wire: &[f32], out: &mut [f32], device_channels: usize, route: OutputRoute) {
    out.fill(0.0);
    for (frame, pair) in out
        .chunks_exact_mut(device_channels)
        .zip(wire.as_chunks::<2>().0)
    {
        match route.right {
            Some(right) if right != route.left => {
                frame[route.left] = pair[0];
                frame[right] = pair[1];
            }
            _ => frame[route.left] = (pair[0] + pair[1]) * 0.5,
        }
    }
}

/// The smallest channel count in `offered` that holds `needed` channels: what
/// a device is opened with to read or write channel number `needed`.
pub fn smallest_channel_count(offered: &[u16], needed: usize) -> Option<u16> {
    offered
        .iter()
        .copied()
        .filter(|&count| count as usize >= needed)
        .min()
}

fn index(setting: u32) -> usize {
    setting.max(1) as usize - 1
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Frames of 8 channels where every sample says which channel it is on, so
    /// what was picked can be read off the values.
    fn eight_channel_frames(frames: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|_| (0..8).map(|channel| channel as f32))
            .collect()
    }

    /// Verifies: REQ-AUD-118
    #[test]
    fn test_input_channels_5_and_6_are_read_from_the_fifth_and_sixth_channel() {
        let input = eight_channel_frames(3);
        let picks = capture_picks(5, Some(6), 2);
        let mut out = Vec::new();

        pick_channels(&input, 8, &picks, &mut out);

        assert_eq!(out, [4.0, 5.0, 4.0, 5.0, 4.0, 5.0]);
    }

    /// Verifies: REQ-AUD-118
    #[test]
    fn test_a_mono_transmit_reads_only_the_left_input_channel() {
        let input = eight_channel_frames(2);
        let picks = capture_picks(3, Some(4), 1);
        let mut out = Vec::new();

        pick_channels(&input, 8, &picks, &mut out);

        assert_eq!(out, [2.0, 2.0]);
    }

    /// Verifies: REQ-AUD-118
    #[test]
    fn test_a_stereo_transmit_with_no_right_channel_sends_the_left_on_both_sides() {
        assert_eq!(capture_picks(3, None, 2), [2, 2]);
    }

    /// Verifies: REQ-AUD-118
    #[test]
    fn test_the_default_input_setting_reads_the_first_two_channels() {
        assert_eq!(capture_picks(1, Some(2), 2), [0, 1]);
        assert_eq!(capture_picks(1, Some(2), 1), [0]);
    }

    /// Verifies: REQ-AUD-118
    #[test]
    fn test_a_selection_the_device_lacks_falls_back_to_its_first_channels() {
        let attempts = capture_attempts(5, Some(6), 2);

        assert_eq!(attempts, [vec![4, 5], vec![0, 1], vec![0]]);
    }

    /// Verifies: REQ-AUD-118
    #[test]
    fn test_the_default_selection_falls_back_only_to_mono() {
        assert_eq!(capture_attempts(1, Some(2), 2), [vec![0, 1], vec![0]]);
        assert_eq!(capture_attempts(1, Some(2), 1), [vec![0]]);
    }

    /// Verifies: REQ-AUD-119
    #[test]
    fn test_output_channels_5_and_6_play_the_left_and_right_on_the_fifth_and_sixth() {
        let wire = [0.5, -0.5, 0.25, -0.25];
        let mut out = [f32::NAN; 16];
        let route = OutputRoute::from_settings(5, Some(6));

        place_stereo(&wire, &mut out, 8, route);

        assert_eq!(
            out,
            [
                0.0, 0.0, 0.0, 0.0, 0.5, -0.5, 0.0, 0.0, //
                0.0, 0.0, 0.0, 0.0, 0.25, -0.25, 0.0, 0.0,
            ]
        );
    }

    /// Verifies: REQ-AUD-119
    #[test]
    fn test_an_output_with_no_right_channel_plays_both_sides_mixed_on_the_left() {
        let wire = [0.5, -0.25];
        let mut out = [f32::NAN; 4];
        let route = OutputRoute::from_settings(3, None);

        place_stereo(&wire, &mut out, 4, route);

        assert_eq!(out, [0.0, 0.0, 0.125, 0.0]);
    }

    /// Verifies: REQ-AUD-119
    #[test]
    fn test_the_default_output_setting_is_the_stereo_frame_itself() {
        let route = OutputRoute::from_settings(1, Some(2));
        let wire = [0.5, -0.5, 0.25, -0.25];
        let mut out = [f32::NAN; 4];

        place_stereo(&wire, &mut out, 2, route);

        assert!(route.is_default());
        assert_eq!(out, wire);
    }

    /// Verifies: REQ-AUD-119
    #[test]
    fn test_a_route_needs_a_device_with_its_highest_channel() {
        assert_eq!(OutputRoute::from_settings(5, Some(6)).channels_needed(), 6);
        assert_eq!(OutputRoute::from_settings(3, None).channels_needed(), 3);
        assert_eq!(OutputRoute::default().channels_needed(), 2);
    }

    /// Verifies: REQ-AUD-118
    #[test]
    fn test_a_device_is_opened_with_the_smallest_channel_count_that_reaches_the_channel() {
        assert_eq!(smallest_channel_count(&[2, 8, 4], 3), Some(4));
        assert_eq!(smallest_channel_count(&[2], 2), Some(2));
        assert_eq!(smallest_channel_count(&[2], 6), None);
    }

    /// A setting of 0 cannot come from the app (the commands refuse it) but a
    /// hand-edited config can hold it; it reads as channel 1.
    #[test]
    fn test_a_channel_setting_of_zero_is_read_as_the_first_channel() {
        assert_eq!(capture_picks(0, Some(0), 2), [0, 0]);
    }
}
