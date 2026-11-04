//! Telling the others in the room the delay of this app's audio devices
//! (REQ-CON-035).
//!
//! The delay is measured in the settings (`audio::delay_meter`) and is a
//! number of milliseconds for the sender's own input and output. It is all the
//! notice carries: not which devices, not when, not how steady.

use serde::{Deserialize, Serialize};

/// The longest delay a notice may state, in ms. The meter gives no figure
/// above [`crate::audio::delay_meter::MAX_DELAY_MS`].
pub const MAX_NOTICE_MS: f32 = 500.0;

/// The delay of the sender's devices; `None` when the sender has no figure
/// for the devices it uses now. Travels as the body of a peer message, under
/// [`super::settings_help::PeerBody::DeviceDelay`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DeviceDelayNotice {
    pub delay_ms: Option<f32>,
}

impl DeviceDelayNotice {
    /// The notice for a figure, to a tenth of a millisecond. Nothing for a
    /// figure that cannot be a delay.
    pub fn of(delay_ms: Option<f64>) -> Self {
        Self {
            delay_ms: delay_ms
                .map(|ms| (ms * 10.0).round() as f32 / 10.0)
                .filter(|ms| (0.0..=MAX_NOTICE_MS).contains(ms)),
        }
    }

    /// What a received notice says, if it says something a delay can be: a
    /// number outside the range is dropped, as an app that lies or errs.
    pub fn stated(&self) -> Option<f32> {
        self.delay_ms
            .filter(|ms| ms.is_finite() && (0.0..=MAX_NOTICE_MS).contains(ms))
    }
}

#[cfg(test)]
mod tests {
    use super::super::settings_help::PeerBody;
    use super::*;

    /// Verifies: REQ-CON-035
    #[test]
    fn when_the_notice_travels_it_is_a_peer_message_under_its_own_topic_with_only_the_number() {
        let body = serde_json::to_value(PeerBody::DeviceDelay(DeviceDelayNotice::of(Some(23.44))))
            .unwrap();

        assert_eq!(body, serde_json::json!({"device_delay": {"delay_ms": 23.4}}));
        let parsed: PeerBody = serde_json::from_value(body).unwrap();
        assert_eq!(
            parsed,
            PeerBody::DeviceDelay(DeviceDelayNotice {
                delay_ms: Some(23.4)
            })
        );
    }

    /// Verifies: REQ-CON-035
    #[test]
    fn when_the_sender_has_no_figure_the_notice_says_so() {
        let body = serde_json::to_value(PeerBody::DeviceDelay(DeviceDelayNotice::of(None))).unwrap();

        assert_eq!(body, serde_json::json!({"device_delay": {"delay_ms": null}}));
    }

    /// Verifies: REQ-CON-035
    #[test]
    fn a_number_that_cannot_be_a_delay_is_not_stated() {
        assert_eq!(DeviceDelayNotice::of(Some(-1.0)).delay_ms, None);
        assert_eq!(DeviceDelayNotice::of(Some(900.0)).delay_ms, None);
        let from_a_liar = DeviceDelayNotice {
            delay_ms: Some(1.0e9),
        };
        assert_eq!(from_a_liar.stated(), None);
        assert_eq!(DeviceDelayNotice::of(Some(0.0)).stated(), Some(0.0));
    }
}
