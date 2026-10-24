//! Which buffer size to open an ASIO driver at, and which one it ended up at.
//!
//! An ASIO driver accepts only the sizes inside the `[min, max]` range it
//! reports (on its grid, if it reports one). A request outside that range is
//! moved to the nearest end rather than handed to the driver's `preferred`
//! size: asking for 32 on a driver whose smallest buffer is 64 must open at
//! 64, not at the 256 the driver would rather have.

use std::sync::atomic::{AtomicU32, Ordering};

/// Which sizes between a driver's `min` and `max` it accepts (ASIO's
/// `granularity`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Steps {
    /// Every size in the range
    Any,
    /// `min`, `min + step`, `min + 2 * step`, ...
    Linear(i32),
    /// `min`, `min * 2`, `min * 4`, ...
    Doubling,
}

/// The size to open the driver at for a `requested` one: the request itself
/// when the driver accepts it, otherwise the nearest accepted size - rounding
/// up between two grid points, because a buffer smaller than the one asked for
/// risks the glitches the request was sized to avoid.
///
/// `preferred` is used only when the driver's range is unusable (empty or not
/// positive), where there is nothing to move the request to.
pub(crate) fn fit(requested: i32, min: i32, max: i32, preferred: i32, steps: Steps) -> i32 {
    if min <= 0 || max < min {
        return preferred;
    }
    let wanted = requested.clamp(min, max);
    let on_grid = |size: i32| match steps {
        Steps::Linear(step) if step > 0 => min + (size - min + step - 1) / step * step,
        Steps::Doubling => {
            let mut size_on_grid = min;
            while size_on_grid < size {
                size_on_grid = size_on_grid.saturating_mul(2);
            }
            size_on_grid
        }
        _ => size,
    };
    let rounded = on_grid(wanted);
    if rounded <= max {
        return rounded;
    }
    // The grid steps over `max`: the largest grid point under it.
    match steps {
        Steps::Linear(step) if step > 0 => min + (max - min) / step * step,
        Steps::Doubling => {
            let mut size_on_grid = min;
            while size_on_grid.saturating_mul(2) <= max {
                size_on_grid *= 2;
            }
            size_on_grid
        }
        _ => max,
    }
}

/// The size the running ASIO session opened at, in frames; 0 while none runs.
static OPENED: AtomicU32 = AtomicU32::new(0);

pub(crate) fn set_opened(frames: u32) {
    OPENED.store(frames, Ordering::Relaxed);
}

/// The buffer size, in frames, the running ASIO session actually opened at,
/// which is not always the one the settings ask for (the driver's smallest
/// size can be larger). `None` when no ASIO session is running.
pub fn opened_asio_buffer_frames() -> Option<u32> {
    match OPENED.load(Ordering::Relaxed) {
        0 => None,
        frames => Some(frames),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ASIO4ALL v2 as measured: min 64, max 2048, preferred 256, step 8.
    fn asio4all(requested: i32) -> i32 {
        fit(requested, 64, 2048, 256, Steps::Linear(8))
    }

    #[test]
    fn a_request_under_the_driver_minimum_opens_at_the_minimum_not_the_preferred_size() {
        assert_eq!(asio4all(32), 64);
    }

    #[test]
    fn a_request_over_the_driver_maximum_opens_at_the_maximum() {
        assert_eq!(asio4all(4096), 2048);
    }

    #[test]
    fn a_request_the_driver_accepts_opens_as_asked() {
        for requested in [64, 128, 256] {
            assert_eq!(asio4all(requested), requested);
        }
    }

    #[test]
    fn a_request_between_two_grid_points_opens_at_the_next_one_up() {
        assert_eq!(asio4all(100), 104);
        assert_eq!(fit(100, 64, 2048, 256, Steps::Doubling), 128);
    }

    #[test]
    fn a_grid_that_steps_over_the_maximum_opens_at_the_largest_grid_point_under_it() {
        assert_eq!(fit(500, 64, 500, 256, Steps::Linear(64)), 448);
        assert_eq!(fit(500, 64, 500, 256, Steps::Doubling), 256);
    }

    #[test]
    fn a_driver_with_one_fixed_size_opens_at_that_size() {
        assert_eq!(fit(32, 128, 128, 128, Steps::Any), 128);
    }

    #[test]
    fn a_driver_range_that_makes_no_sense_opens_at_the_preferred_size() {
        assert_eq!(fit(32, 0, 0, 256, Steps::Any), 256);
        assert_eq!(fit(32, 512, 64, 256, Steps::Any), 256);
    }

    #[test]
    fn the_opened_size_is_none_until_a_session_sets_it_and_none_again_once_cleared() {
        set_opened(0);
        assert_eq!(opened_asio_buffer_frames(), None);
        set_opened(64);
        assert_eq!(opened_asio_buffer_frames(), Some(64));
        set_opened(0);
        assert_eq!(opened_asio_buffer_frames(), None);
    }
}
