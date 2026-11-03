//! Telling the others in the room that this app records it (ADR-095).
//!
//! The room's terms ask everyone whose sound is recorded to agree before it
//! starts. The app cannot know who agreed, but it can make sure nobody records
//! without the others being told: starting a recording is announced to
//! everyone in the room, stopping it too, and a participant who comes in while
//! it runs is told on arrival. The announcement carries nothing but whether the
//! sender records: not where the file is, not what is in it.

use serde::{Deserialize, Serialize};

/// Whether the sender of the peer message records the room. Travels as the body
/// of a peer message, under [`super::settings_help::PeerBody::Recording`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingNotice {
    pub on: bool,
}

#[cfg(test)]
mod tests {
    use super::super::settings_help::PeerBody;
    use super::*;

    /// Verifies: REQ-CON-034
    #[test]
    fn when_the_notice_travels_it_is_a_peer_message_under_its_own_topic() {
        let body = serde_json::to_value(PeerBody::Recording(RecordingNotice { on: true })).unwrap();

        assert_eq!(body, serde_json::json!({"recording": {"on": true}}));
        let parsed: PeerBody = serde_json::from_value(body).unwrap();
        assert_eq!(parsed, PeerBody::Recording(RecordingNotice { on: true }));
    }

    /// Verifies: REQ-CON-034
    #[test]
    fn when_the_notice_says_more_than_whether_it_records_it_is_not_taken() {
        let wrong = serde_json::json!({"recording": {"on": "yes"}});

        assert!(serde_json::from_value::<PeerBody>(wrong).is_err());
    }
}
