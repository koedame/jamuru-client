//! Telling a refusal that came from this computer's clock
//!
//! A device identity signs the current time (ADR-024), and the server refuses
//! a time that is too far from its own with the same 401 it gives a forged
//! identity. The refusal says nothing about the time, so the app measures it:
//! every answer of the server carries the server's time in its `Date` header,
//! and the gap to this computer's clock is what the person can act on.

use std::time::{SystemTime, UNIX_EPOCH};

use super::error::NetworkError;

/// A gap this large or larger between this computer's clock and the server's
/// is reported as the clock being wrong, rather than as an unexplained refusal.
pub const CLOCK_SKEW_NOTICE_SECS: i64 = 30;

/// Current Unix time in whole seconds, the timestamp a device identity signs
/// when connecting.
pub(super) fn now_unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// How far this computer's clock is ahead of the server's, in seconds
/// (negative when it is behind), from the `Date` header of one of the
/// server's answers. `None` when the header is not a date.
pub fn clock_offset_secs(date_header: &str, now_secs: i64) -> Option<i64> {
    let server = chrono::DateTime::parse_from_rfc2822(date_header.trim()).ok()?;
    Some(now_secs - server.timestamp())
}

/// The error a refusal is, when the server refused the device (`status` 401)
/// and its `Date` header shows this computer's clock to be off by
/// [`CLOCK_SKEW_NOTICE_SECS`] or more. `None` leaves the refusal as it is.
pub(super) fn clock_skew_of_refusal(
    status: u16,
    date_header: Option<&str>,
) -> Option<NetworkError> {
    if status != 401 {
        return None;
    }
    let offset_secs = clock_offset_secs(date_header?, now_unix_secs())?;
    (offset_secs.abs() >= CLOCK_SKEW_NOTICE_SECS).then_some(NetworkError::ClockSkew { offset_secs })
}

/// The sentence a [`NetworkError::ClockSkew`] is. The screen reads the size of
/// the gap back out of it, so the wording is part of the contract with
/// `ui/src/lib/errorMessages.ts`.
pub(super) fn clock_skew_message(offset_secs: i64) -> String {
    let side = if offset_secs >= 0 {
        "ahead of"
    } else {
        "behind"
    };
    format!(
        "This computer's clock is {} the server's by {} seconds",
        side,
        offset_secs.unsigned_abs()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const DATE: &str = "Sat, 26 Sep 2026 17:44:57 GMT";
    const DATE_SECS: i64 = 1_790_444_697;

    #[test]
    fn when_the_clock_runs_ahead_of_the_servers_date_the_offset_is_positive() {
        assert_eq!(clock_offset_secs(DATE, DATE_SECS + 375), Some(375));
    }

    #[test]
    fn when_the_clock_runs_behind_the_servers_date_the_offset_is_negative() {
        assert_eq!(clock_offset_secs(DATE, DATE_SECS - 90), Some(-90));
    }

    #[test]
    fn when_the_date_header_is_not_a_date_there_is_no_offset() {
        assert_eq!(clock_offset_secs("soon", DATE_SECS), None);
    }

    #[test]
    fn when_the_message_names_the_gap_it_names_the_side_and_the_size() {
        assert_eq!(
            clock_skew_message(375),
            "This computer's clock is ahead of the server's by 375 seconds"
        );
        assert_eq!(
            clock_skew_message(-90),
            "This computer's clock is behind the server's by 90 seconds"
        );
    }

    #[test]
    fn when_the_refusal_is_not_a_401_it_is_not_put_down_to_the_clock() {
        let date = chrono::DateTime::from_timestamp(now_unix_secs() - 3600, 0)
            .unwrap()
            .format("%a, %d %b %Y %H:%M:%S GMT")
            .to_string();
        assert!(clock_skew_of_refusal(403, Some(&date)).is_none());
    }

    #[test]
    fn when_the_servers_time_is_close_to_the_clock_a_401_is_not_put_down_to_it() {
        let date = chrono::DateTime::from_timestamp(now_unix_secs() - 5, 0)
            .unwrap()
            .format("%a, %d %b %Y %H:%M:%S GMT")
            .to_string();
        assert!(clock_skew_of_refusal(401, Some(&date)).is_none());
    }

    #[test]
    fn when_a_401_carries_no_date_it_is_not_put_down_to_the_clock() {
        assert!(clock_skew_of_refusal(401, None).is_none());
    }

    #[test]
    fn when_the_servers_time_is_far_from_the_clock_a_401_is_the_clock() {
        let date = chrono::DateTime::from_timestamp(now_unix_secs() - 375, 0)
            .unwrap()
            .format("%a, %d %b %Y %H:%M:%S GMT")
            .to_string();
        let error = clock_skew_of_refusal(401, Some(&date)).expect("clock skew");
        assert!(
            matches!(error, NetworkError::ClockSkew { offset_secs } if (374..=376).contains(&offset_secs)),
            "{error:?}"
        );
    }
}
