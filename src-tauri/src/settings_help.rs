//! Helping another participant with their audio settings (ADR-044 §5)
//!
//! One participant (the helper) asks to help another (the helped). The helped
//! side answers once. On yes it opens a connection to the server's relay under
//! a number only it chose and tells the helper the number; the helper opens the
//! same number and from then on operates the helped app through the relay, as a
//! portal whose range the permission table fixes (`rpc::spec`). Nothing more is
//! asked of the person helped. Either side can stop at any time, and the help
//! ends when either leaves the room or the connection to it is lost.
//!
//! [`Help`] is the offer and its end as a state machine with no I/O: it takes
//! what happened (a local action, a message from a peer, a peer leaving, the
//! relay connection ending) and says what to do about it ([`Out`]) - messages
//! to send, events for the UI, and the relay connection the helper is to open.
//! The caller carries those out over the signaling relay and the app's own
//! relay connection.
//!
//! The other app is not trusted to follow the protocol. What the helped side
//! accepts is the request it showed itself, from the participant it named; a
//! number from anyone the helper did not ask, or one the relay would refuse, is
//! dropped. The only answer a peer can get without this app's user acting is
//! "busy", at most once per participant in [`BUSY_REPLY_INTERVAL`], so peers
//! cannot make this app exceed the server's message limit and lose the room.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use jamjam::network::is_help_session;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A participant asking while this app is already helped hears "busy" at
/// most once in this time; asking again sooner gets no answer.
pub const BUSY_REPLY_INTERVAL: Duration = Duration::from_secs(10);

/// What the two apps say to each other over the room's signaling. Travels as
/// the body of a peer message, under [`PeerBody::SettingsHelp`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HelpMessage {
    /// Helper to helped: may I help with your settings?
    Request,
    /// Helped to helper: yes. The helped app is waiting on the relay under
    /// `session`; open the same number there.
    Accepted { session: String },
    /// Helped to helper: no. `busy` when someone else is already helping.
    Declined { busy: bool },
    /// Either side to the other: the help is over. `role` is the sender's
    /// side of it, so help the two give each other the other way goes on.
    Stop { role: Role },
    /// Helped to everyone in the room, for the chat. The helped participant
    /// is the sender, which the server stamps; each app names the helper
    /// from its own list of the room's participants.
    Notice {
        event: NoticeEvent,
        helper: Uuid,
        /// The setting that changed (`SettingChange`'s name), for `Changed`
        #[serde(default, skip_serializing_if = "Option::is_none")]
        setting: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeEvent {
    Started,
    Changed,
    Ended,
}

/// The body of a peer message, by topic. Only settings help exists; a body
/// on another topic (from a newer app) does not parse and is ignored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeerBody {
    SettingsHelp(HelpMessage),
}

/// Which side of a help this app is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// This app helps the peer.
    Helper,
    /// The peer helps this app.
    Helped,
}

/// Why a help ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    /// This app left the room or lost the connection.
    Stopped,
    /// The peer stopped it (or withdrew the request), or their relay
    /// connection ended.
    PeerStopped,
    /// The peer left the room.
    PeerLeft,
}

/// Something the UI shows.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HelpEvent {
    /// `peer` asks to help this app; answer with [`Help::accept`] or
    /// [`Help::decline`].
    Requested { peer: Uuid, peer_name: String },
    /// Help with `peer` began.
    Started {
        role: Role,
        peer: Uuid,
        peer_name: String,
    },
    /// `peer` declined this app's offer to help.
    Declined { peer: Uuid, busy: bool },
    /// Help with `peer` is over.
    Ended {
        role: Role,
        peer: Uuid,
        reason: EndReason,
    },
    /// For the chat: `helper` did `event` to the settings of `helped_name`
    /// (the sender, as the server stamped it). The caller names the helper
    /// from the room's participants.
    Notice {
        event: NoticeEvent,
        helper: Uuid,
        helped_name: String,
        setting: Option<String>,
    },
}

/// The relay connection the helper is to open: the helped side has accepted
/// and is waiting on `session`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Link {
    pub peer: Uuid,
    pub peer_name: String,
    pub session: String,
}

/// What to do about something that happened.
#[derive(Debug, Clone, PartialEq)]
pub enum Out {
    Send {
        to: Uuid,
        message: HelpMessage,
    },
    /// To everyone in the room who accepts peer messages, this app included.
    Broadcast(HelpMessage),
    Event(HelpEvent),
    /// Open the helper's end of the relay and the window the help is done in.
    Link(Link),
}

/// Why a local action could not be taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelpError {
    /// This app is already helping someone (or asking to).
    AlreadyHelping,
    /// There is no help going on in the role the action needs, or the
    /// request being answered is not the one waiting.
    NotActive,
}

impl std::fmt::Display for HelpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HelpError::AlreadyHelping => f.write_str("Already helping someone with their settings"),
            HelpError::NotActive => f.write_str("Not helping with settings"),
        }
    }
}

impl std::error::Error for HelpError {}

/// This app helping someone.
#[derive(Debug, Clone, PartialEq)]
enum Giving {
    /// Asked `peer`, no answer yet.
    Asking { peer: Uuid },
    /// Helping `peer` through the relay connection numbered `session`.
    Active { peer: Uuid, session: String },
}

impl Giving {
    fn peer(&self) -> Uuid {
        match self {
            Giving::Asking { peer } | Giving::Active { peer, .. } => *peer,
        }
    }
}

/// Someone helping this app.
#[derive(Debug, Clone, PartialEq)]
enum Receiving {
    /// `peer` asked; this app has not answered.
    Asked { peer: Uuid, name: String },
    /// `peer` is helping, through the relay connection numbered `session`.
    Active {
        peer: Uuid,
        name: String,
        session: String,
    },
}

impl Receiving {
    fn peer(&self) -> Uuid {
        match self {
            Receiving::Asked { peer, .. } | Receiving::Active { peer, .. } => *peer,
        }
    }
}

/// The help this app gives and receives. The two are independent: an app can
/// be helped by one participant while it helps another (or the same one),
/// but gives and receives help with one participant at a time.
#[derive(Debug, Default)]
pub struct Help {
    giving: Option<Giving>,
    receiving: Option<Receiving>,
    /// When each participant was last told this app is busy
    busy_replies: HashMap<Uuid, Instant>,
}

impl Help {
    pub fn new() -> Self {
        Self::default()
    }

    /// Who is helping this app, if anyone.
    #[cfg(test)]
    fn helper(&self) -> Option<Uuid> {
        match &self.receiving {
            Some(Receiving::Active { peer, .. }) => Some(*peer),
            _ => None,
        }
    }

    /// Asks `peer` to let this app help with their settings.
    pub fn request(&mut self, peer: Uuid) -> Result<Vec<Out>, HelpError> {
        if self.giving.is_some() {
            return Err(HelpError::AlreadyHelping);
        }
        self.giving = Some(Giving::Asking { peer });
        Ok(vec![Out::Send {
            to: peer,
            message: HelpMessage::Request,
        }])
    }

    /// Whether `peer` is the participant whose request the user was shown, and
    /// has not been answered.
    pub fn is_asked_by(&self, peer: Uuid) -> bool {
        matches!(&self.receiving, Some(Receiving::Asked { peer: asking, .. }) if *asking == peer)
    }

    /// Declines `peer`'s request, the one the user was shown.
    pub fn decline(&mut self, peer: Uuid) -> Result<Vec<Out>, HelpError> {
        if !self.is_asked_by(peer) {
            return Err(HelpError::NotActive);
        }
        self.receiving = None;
        Ok(vec![Out::Send {
            to: peer,
            message: HelpMessage::Declined { busy: false },
        }])
    }

    /// Accepts `peer`'s request, the one the user was shown, now that this app
    /// waits on the relay under `session`. The helper is told the number and the
    /// room hears that the help started.
    pub fn accept(&mut self, peer: Uuid, session: String) -> Result<Vec<Out>, HelpError> {
        let name = match &self.receiving {
            Some(Receiving::Asked { peer: asking, name }) if *asking == peer => name.clone(),
            _ => return Err(HelpError::NotActive),
        };
        self.receiving = Some(Receiving::Active {
            peer,
            name: name.clone(),
            session: session.clone(),
        });
        Ok(vec![
            Out::Send {
                to: peer,
                message: HelpMessage::Accepted { session },
            },
            Out::Broadcast(notice(NoticeEvent::Started, peer, None)),
            Out::Event(HelpEvent::Started {
                role: Role::Helped,
                peer,
                peer_name: name,
            }),
        ])
    }

    /// Stops the help in `role`. No event: the UI that asked to stop has already
    /// cleared the help, and an end that reached it later could land on a help
    /// it began in the meantime.
    pub fn stop(&mut self, role: Role) -> Result<Vec<Out>, HelpError> {
        let (peer, announce) = match role {
            Role::Helper => (
                self.giving.take().ok_or(HelpError::NotActive)?.peer(),
                false,
            ),
            Role::Helped => {
                let receiving = self.receiving.take().ok_or(HelpError::NotActive)?;
                let active = matches!(receiving, Receiving::Active { .. });
                (receiving.peer(), active)
            }
        };
        let mut out = vec![Out::Send {
            to: peer,
            message: HelpMessage::Stop { role },
        }];
        if announce {
            out.push(Out::Broadcast(notice(NoticeEvent::Ended, peer, None)));
        }
        Ok(out)
    }

    /// A help message arrived from `from` (stamped by the server, so it is who
    /// it says), whose room name is `from_name`.
    ///
    /// Anything that does not fit the current state - an answer to a request
    /// never made, a stop from someone who is not helping - is dropped.
    pub fn receive(&mut self, from: Uuid, from_name: &str, message: HelpMessage) -> Vec<Out> {
        self.receive_at(from, from_name, message, Instant::now())
    }

    /// [`Self::receive`], at `now`.
    fn receive_at(
        &mut self,
        from: Uuid,
        from_name: &str,
        message: HelpMessage,
        now: Instant,
    ) -> Vec<Out> {
        match message {
            HelpMessage::Request => match &self.receiving {
                None => {
                    self.receiving = Some(Receiving::Asked {
                        peer: from,
                        name: from_name.to_string(),
                    });
                    vec![Out::Event(HelpEvent::Requested {
                        peer: from,
                        peer_name: from_name.to_string(),
                    })]
                }
                Some(current) if current.peer() == from => Vec::new(),
                Some(_) => {
                    self.busy_replies
                        .retain(|_, at| now.duration_since(*at) < BUSY_REPLY_INTERVAL);
                    if self.busy_replies.contains_key(&from) {
                        return Vec::new();
                    }
                    self.busy_replies.insert(from, now);
                    vec![Out::Send {
                        to: from,
                        message: HelpMessage::Declined { busy: true },
                    }]
                }
            },
            HelpMessage::Accepted { session } => match &self.giving {
                Some(Giving::Asking { peer }) if *peer == from && is_help_session(&session) => {
                    self.giving = Some(Giving::Active {
                        peer: from,
                        session: session.clone(),
                    });
                    vec![
                        Out::Event(HelpEvent::Started {
                            role: Role::Helper,
                            peer: from,
                            peer_name: from_name.to_string(),
                        }),
                        Out::Link(Link {
                            peer: from,
                            peer_name: from_name.to_string(),
                            session,
                        }),
                    ]
                }
                _ => Vec::new(),
            },
            HelpMessage::Declined { busy } => match &self.giving {
                Some(Giving::Asking { peer }) if *peer == from => {
                    self.giving = None;
                    vec![Out::Event(HelpEvent::Declined { peer: from, busy })]
                }
                _ => Vec::new(),
            },
            // The sender stopped helping this app...
            HelpMessage::Stop { role: Role::Helper } => {
                self.end_receiving_with(from, EndReason::PeerStopped, true)
            }
            // ...or stopped being helped by it.
            HelpMessage::Stop { role: Role::Helped } => {
                self.end_giving_with(from, EndReason::PeerStopped)
            }
            HelpMessage::Notice {
                event,
                helper,
                setting,
            } => vec![Out::Event(HelpEvent::Notice {
                event,
                helper,
                helped_name: from_name.to_string(),
                setting,
            })],
        }
    }

    /// `peer` left the room: any help with them is over. The room already
    /// hears that they left, so no notice goes out.
    pub fn peer_left(&mut self, peer: Uuid) -> Vec<Out> {
        let mut out = self.end_giving_with(peer, EndReason::PeerLeft);
        out.extend(self.end_receiving_with(peer, EndReason::PeerLeft, false));
        out
    }

    /// The relay connection numbered `session` ended, whichever side closed it
    /// (or this app could not open it): the help it carried is over. The peer is
    /// told as well, in case the relay could not, and the room is told when this
    /// app was the helped side, as it is when the help is stopped.
    pub fn link_closed(&mut self, role: Role, session: &str) -> Vec<Out> {
        let peer = match (role, &self.giving, &self.receiving) {
            (
                Role::Helper,
                Some(Giving::Active {
                    peer,
                    session: held,
                }),
                _,
            ) if held == session => *peer,
            (
                Role::Helped,
                _,
                Some(Receiving::Active {
                    peer,
                    session: held,
                    ..
                }),
            ) if held == session => *peer,
            _ => return Vec::new(),
        };
        let mut out = vec![Out::Send {
            to: peer,
            message: HelpMessage::Stop { role },
        }];
        out.extend(match role {
            Role::Helper => self.end_giving_with(peer, EndReason::PeerStopped),
            Role::Helped => self.end_receiving_with(peer, EndReason::PeerStopped, true),
        });
        out
    }

    /// This app left the room or lost the connection: every help is over, and
    /// there is no one left to tell.
    pub fn reset(&mut self) -> Vec<Out> {
        let mut out = Vec::new();
        if let Some(giving) = self.giving.take() {
            out.push(Out::Event(HelpEvent::Ended {
                role: Role::Helper,
                peer: giving.peer(),
                reason: EndReason::Stopped,
            }));
        }
        if let Some(receiving) = self.receiving.take() {
            out.push(Out::Event(HelpEvent::Ended {
                role: Role::Helped,
                peer: receiving.peer(),
                reason: EndReason::Stopped,
            }));
        }
        out
    }

    fn end_giving_with(&mut self, peer: Uuid, reason: EndReason) -> Vec<Out> {
        if self.giving.as_ref().map(Giving::peer) != Some(peer) {
            return Vec::new();
        }
        self.giving = None;
        vec![Out::Event(HelpEvent::Ended {
            role: Role::Helper,
            peer,
            reason,
        })]
    }

    fn end_receiving_with(&mut self, peer: Uuid, reason: EndReason, announce: bool) -> Vec<Out> {
        if self.receiving.as_ref().map(Receiving::peer) != Some(peer) {
            return Vec::new();
        }
        let mut out = Vec::new();
        if let Some(Receiving::Active { .. }) = self.receiving.take() {
            if announce {
                out.push(Out::Broadcast(notice(NoticeEvent::Ended, peer, None)));
            }
        }
        out.push(Out::Event(HelpEvent::Ended {
            role: Role::Helped,
            peer,
            reason,
        }));
        out
    }
}

/// The notice the room hears of a help: `event`, by `helper`, about `setting`
/// when a setting changed.
pub fn notice(event: NoticeEvent, helper: Uuid, setting: Option<String>) -> HelpMessage {
    HelpMessage::Notice {
        event,
        helper,
        setting,
    }
}

/// The name a change travels under (`"input_device"`, `"buffer_size"`, ...),
/// which the chat turns into the setting's label.
pub fn setting_name(change: &crate::settings::SettingChange) -> String {
    serde_json::to_value(change)
        .ok()
        .and_then(|v| v.get("setting").and_then(|s| s.as_str()).map(String::from))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELPER_NAME: &str = "Aki";
    const HELPED_NAME: &str = "Bo";
    const SESSION: &str = "0123456789abcdef0123456789abcdef";

    /// A helper app and a helped app with the help under way, and the ids of
    /// the two participants.
    fn active() -> (Help, Help, Uuid, Uuid) {
        let helper_id = Uuid::new_v4();
        let helped_id = Uuid::new_v4();
        let mut helper = Help::new();
        let mut helped = Help::new();
        helper.request(helped_id).unwrap();
        helped.receive(helper_id, HELPER_NAME, HelpMessage::Request);
        let out = helped.accept(helper_id, SESSION.to_string()).unwrap();
        let [(_, accepted)] = sent(&out).try_into().unwrap();
        helper.receive(helped_id, HELPED_NAME, accepted);
        (helper, helped, helper_id, helped_id)
    }

    fn sent(out: &[Out]) -> Vec<(Uuid, HelpMessage)> {
        out.iter()
            .filter_map(|o| match o {
                Out::Send { to, message } => Some((*to, message.clone())),
                _ => None,
            })
            .collect()
    }

    fn broadcast(out: &[Out]) -> Vec<HelpMessage> {
        out.iter()
            .filter_map(|o| match o {
                Out::Broadcast(message) => Some(message.clone()),
                _ => None,
            })
            .collect()
    }

    fn events(out: &[Out]) -> Vec<HelpEvent> {
        out.iter()
            .filter_map(|o| match o {
                Out::Event(event) => Some(event.clone()),
                _ => None,
            })
            .collect()
    }

    fn links(out: &[Out]) -> Vec<Link> {
        out.iter()
            .filter_map(|o| match o {
                Out::Link(link) => Some(link.clone()),
                _ => None,
            })
            .collect()
    }

    /// A help is not kept anywhere: an app that has just started has none, in
    /// either role, and nothing to stop.
    ///
    /// Verifies: REQ-RMT-003
    #[test]
    fn an_app_that_has_just_started_is_helping_no_one_and_helped_by_no_one() {
        let mut help = Help::new();

        assert_eq!(help.helper(), None);
        assert_eq!(help.stop(Role::Helper), Err(HelpError::NotActive));
        assert_eq!(help.stop(Role::Helped), Err(HelpError::NotActive));
        assert!(help.reset().is_empty());
    }

    /// Verifies: REQ-RMT-001
    #[test]
    fn when_the_helped_side_accepts_the_helper_is_given_the_number_to_open_and_the_room_hears_it() {
        let helper_id = Uuid::new_v4();
        let helped_id = Uuid::new_v4();
        let mut helper = Help::new();
        let mut helped = Help::new();

        assert_eq!(
            sent(&helper.request(helped_id).unwrap()),
            vec![(helped_id, HelpMessage::Request)]
        );
        assert_eq!(
            events(&helped.receive(helper_id, HELPER_NAME, HelpMessage::Request)),
            vec![HelpEvent::Requested {
                peer: helper_id,
                peer_name: HELPER_NAME.into()
            }]
        );

        let out = helped.accept(helper_id, SESSION.to_string()).unwrap();
        let accepted = HelpMessage::Accepted {
            session: SESSION.to_string(),
        };
        assert_eq!(sent(&out), vec![(helper_id, accepted.clone())]);
        assert_eq!(
            broadcast(&out),
            vec![notice(NoticeEvent::Started, helper_id, None)]
        );
        assert_eq!(
            events(&out),
            vec![HelpEvent::Started {
                role: Role::Helped,
                peer: helper_id,
                peer_name: HELPER_NAME.into()
            }]
        );

        let started = helper.receive(helped_id, HELPED_NAME, accepted);
        assert_eq!(
            events(&started),
            vec![HelpEvent::Started {
                role: Role::Helper,
                peer: helped_id,
                peer_name: HELPED_NAME.into()
            }]
        );
        assert_eq!(
            links(&started),
            vec![Link {
                peer: helped_id,
                peer_name: HELPED_NAME.into(),
                session: SESSION.into()
            }],
            "the helper is to open the same number on the relay"
        );
        assert_eq!(helped.helper(), Some(helper_id));
    }

    /// Verifies: REQ-RMT-001
    #[test]
    fn when_the_helped_side_declines_no_help_starts() {
        let helper_id = Uuid::new_v4();
        let helped_id = Uuid::new_v4();
        let mut helper = Help::new();
        let mut helped = Help::new();
        helper.request(helped_id).unwrap();
        helped.receive(helper_id, HELPER_NAME, HelpMessage::Request);

        let out = helped.decline(helper_id).unwrap();
        assert_eq!(
            sent(&out),
            vec![(helper_id, HelpMessage::Declined { busy: false })]
        );
        assert!(
            broadcast(&out).is_empty(),
            "nothing started, nothing to tell the room"
        );

        let declined = helper.receive(
            helped_id,
            HELPED_NAME,
            HelpMessage::Declined { busy: false },
        );
        assert_eq!(
            events(&declined),
            vec![HelpEvent::Declined {
                peer: helped_id,
                busy: false
            }]
        );
        assert!(links(&declined).is_empty());
        assert_eq!(helped.helper(), None);
    }

    /// The user answers the request they were shown. If that one was
    /// withdrawn and someone else asked in the meantime, the answer does not
    /// go to the newcomer.
    ///
    /// Verifies: REQ-RMT-001
    #[test]
    fn an_answer_to_one_participants_request_does_not_accept_someone_elses() {
        let (first, second) = (Uuid::new_v4(), Uuid::new_v4());
        let mut helped = Help::new();
        helped.receive(first, "First", HelpMessage::Request);
        helped.receive(first, "First", HelpMessage::Stop { role: Role::Helper });
        helped.receive(second, "Second", HelpMessage::Request);

        assert_eq!(
            helped.accept(first, SESSION.to_string()),
            Err(HelpError::NotActive)
        );
        assert_eq!(helped.decline(first), Err(HelpError::NotActive));
        assert_eq!(helped.helper(), None, "the newcomer was not accepted");
        assert!(helped.is_asked_by(second), "the newcomer is still waiting");
    }

    /// Only a participant the helper asked can say yes, and only with a number
    /// the relay would take.
    ///
    /// Verifies: REQ-RMT-001
    #[test]
    fn an_acceptance_from_anyone_the_helper_did_not_ask_or_with_a_bad_number_is_dropped() {
        let helped_id = Uuid::new_v4();
        let stranger = Uuid::new_v4();
        let accepted = |session: &str| HelpMessage::Accepted {
            session: session.to_string(),
        };
        let mut helper = Help::new();
        assert!(helper
            .receive(helped_id, HELPED_NAME, accepted(SESSION))
            .is_empty());

        helper.request(helped_id).unwrap();
        assert!(helper.receive(stranger, "Cy", accepted(SESSION)).is_empty());
        for bad in ["", "abc", "../../admin", &"A".repeat(32)] {
            assert!(
                helper
                    .receive(helped_id, HELPED_NAME, accepted(bad))
                    .is_empty(),
                "{:?}",
                bad
            );
        }
        assert!(
            !events(&helper.receive(helped_id, HELPED_NAME, accepted(SESSION))).is_empty(),
            "the one asked, with a good number, is accepted"
        );
    }

    /// Verifies: REQ-RMT-003
    #[test]
    fn when_the_helper_stops_the_helped_side_ends_too() {
        let (mut helper, mut helped, helper_id, helped_id) = active();

        let out = helper.stop(Role::Helper).unwrap();
        let [(to, stop)] = sent(&out).try_into().unwrap();
        assert_eq!(to, helped_id);
        let ended = helped.receive(helper_id, HELPER_NAME, stop);

        assert!(events(&ended).contains(&HelpEvent::Ended {
            role: Role::Helped,
            peer: helper_id,
            reason: EndReason::PeerStopped
        }));
        assert_eq!(
            broadcast(&ended),
            vec![notice(NoticeEvent::Ended, helper_id, None)]
        );
        assert_eq!(helped.helper(), None);
        assert_eq!(helper.stop(Role::Helper), Err(HelpError::NotActive));
    }

    /// Verifies: REQ-RMT-003
    #[test]
    fn when_the_helped_side_stops_the_helper_ends_and_the_room_hears_it() {
        let (mut helper, mut helped, helper_id, helped_id) = active();

        let out = helped.stop(Role::Helped).unwrap();
        assert_eq!(
            broadcast(&out),
            vec![notice(NoticeEvent::Ended, helper_id, None)]
        );
        let [(to, stop)] = sent(&out).try_into().unwrap();
        assert_eq!(to, helper_id);

        assert_eq!(
            events(&helper.receive(helped_id, HELPED_NAME, stop)),
            vec![HelpEvent::Ended {
                role: Role::Helper,
                peer: helped_id,
                reason: EndReason::PeerStopped
            }]
        );
        assert_eq!(helper.stop(Role::Helper), Err(HelpError::NotActive));
    }

    /// Stopping gives the UI nothing to act on later: it has cleared the help
    /// itself, and an end delivered on its next poll could clear a help it
    /// began with the same person in the meantime.
    ///
    /// Verifies: REQ-RMT-003
    #[test]
    fn stopping_gives_the_ui_no_event_that_could_end_a_help_begun_after_it() {
        let (mut helper, mut helped, _, _) = active();

        for out in [
            helper.stop(Role::Helper).unwrap(),
            helped.stop(Role::Helped).unwrap(),
        ] {
            assert!(events(&out).is_empty(), "{:?}", out);
            assert_eq!(sent(&out).len(), 1, "the other side is still told");
        }
    }

    /// Two participants can help each other at the same time; stopping one
    /// direction leaves the other.
    ///
    /// Verifies: REQ-RMT-003
    #[test]
    fn stopping_help_one_way_leaves_the_help_the_other_way() {
        let (mut aki, mut bo, aki_id, bo_id) = active();
        // Bo helps Aki as well.
        let [(_, request)] = sent(&bo.request(aki_id).unwrap()).try_into().unwrap();
        aki.receive(bo_id, HELPED_NAME, request);
        let [(_, accepted)] = sent(&aki.accept(bo_id, "f".repeat(32)).unwrap())
            .try_into()
            .unwrap();
        bo.receive(aki_id, HELPER_NAME, accepted);

        // Aki stops helping Bo.
        let [(_, stop)] = sent(&aki.stop(Role::Helper).unwrap()).try_into().unwrap();
        let ended = bo.receive(aki_id, HELPER_NAME, stop);

        assert_eq!(
            events(&ended),
            vec![HelpEvent::Ended {
                role: Role::Helped,
                peer: aki_id,
                reason: EndReason::PeerStopped
            }]
        );
        assert!(bo.stop(Role::Helper).is_ok(), "Bo still helps Aki");
        assert_eq!(aki.helper(), Some(bo_id));
    }

    /// Verifies: REQ-RMT-003
    #[test]
    fn a_stop_from_someone_not_in_the_help_changes_nothing() {
        let (mut helper, mut helped, helper_id, _) = active();
        let stranger = Uuid::new_v4();

        assert!(helped
            .receive(stranger, "Cy", HelpMessage::Stop { role: Role::Helper })
            .is_empty());
        assert!(helper
            .receive(stranger, "Cy", HelpMessage::Stop { role: Role::Helped })
            .is_empty());
        assert_eq!(helped.helper(), Some(helper_id));
        assert!(helper.stop(Role::Helper).is_ok());
    }

    /// Verifies: REQ-RMT-003
    #[test]
    fn the_helper_can_withdraw_a_request_before_it_is_answered() {
        let helper_id = Uuid::new_v4();
        let helped_id = Uuid::new_v4();
        let mut helper = Help::new();
        let mut helped = Help::new();
        helper.request(helped_id).unwrap();
        helped.receive(helper_id, HELPER_NAME, HelpMessage::Request);

        let [(_, stop)] = sent(&helper.stop(Role::Helper).unwrap())
            .try_into()
            .unwrap();
        let ended = helped.receive(helper_id, HELPER_NAME, stop);

        assert!(broadcast(&ended).is_empty(), "nothing had started");
        assert_eq!(
            helped.accept(helper_id, SESSION.to_string()),
            Err(HelpError::NotActive),
            "a withdrawn request can no longer be accepted"
        );
    }

    /// Verifies: REQ-RMT-003
    #[test]
    fn when_the_helper_leaves_the_room_the_help_ends_on_the_helped_side_without_a_notice() {
        let (_, mut helped, helper_id, _) = active();

        let out = helped.peer_left(helper_id);

        assert_eq!(
            events(&out),
            vec![HelpEvent::Ended {
                role: Role::Helped,
                peer: helper_id,
                reason: EndReason::PeerLeft
            }]
        );
        assert!(
            broadcast(&out).is_empty(),
            "the room already hears that they left"
        );
        assert_eq!(helped.helper(), None);
    }

    /// Verifies: REQ-RMT-003
    #[test]
    fn when_the_relay_connection_ends_the_help_ends_and_the_helped_side_tells_the_room() {
        let (mut helper, mut helped, helper_id, helped_id) = active();

        let on_helped = helped.link_closed(Role::Helped, SESSION);
        assert_eq!(
            events(&on_helped),
            vec![HelpEvent::Ended {
                role: Role::Helped,
                peer: helper_id,
                reason: EndReason::PeerStopped
            }]
        );
        assert_eq!(
            broadcast(&on_helped),
            vec![notice(NoticeEvent::Ended, helper_id, None)]
        );
        assert_eq!(
            sent(&on_helped),
            vec![(helper_id, HelpMessage::Stop { role: Role::Helped })],
            "the peer is told too, in case the relay could not"
        );
        assert_eq!(helped.helper(), None);

        let on_helper = helper.link_closed(Role::Helper, SESSION);
        assert_eq!(
            events(&on_helper),
            vec![HelpEvent::Ended {
                role: Role::Helper,
                peer: helped_id,
                reason: EndReason::PeerStopped
            }]
        );
        assert_eq!(
            sent(&on_helper),
            vec![(helped_id, HelpMessage::Stop { role: Role::Helper })]
        );
        assert!(broadcast(&on_helper).is_empty());
        assert_eq!(helper.stop(Role::Helper), Err(HelpError::NotActive));
    }

    /// A connection that ends after its help did, and a help begun since, are
    /// different helps: the old connection's end does not stop the new help.
    ///
    /// Verifies: REQ-RMT-003
    #[test]
    fn the_end_of_an_earlier_relay_connection_does_not_end_a_later_help() {
        let (mut helper, mut helped, helper_id, helped_id) = active();
        helped.stop(Role::Helped).unwrap();
        helper.stop(Role::Helper).unwrap();
        let later = "b".repeat(32);
        let [(_, request)] = sent(&helper.request(helped_id).unwrap())
            .try_into()
            .unwrap();
        helped.receive(helper_id, HELPER_NAME, request);
        let [(_, accepted)] = sent(&helped.accept(helper_id, later.clone()).unwrap())
            .try_into()
            .unwrap();
        helper.receive(helped_id, HELPED_NAME, accepted);

        assert!(helped.link_closed(Role::Helped, SESSION).is_empty());
        assert!(helper.link_closed(Role::Helper, SESSION).is_empty());
        assert_eq!(helped.helper(), Some(helper_id));
        assert!(helper.stop(Role::Helper).is_ok());
        assert!(!helped.link_closed(Role::Helped, &later).is_empty());
    }

    /// Verifies: REQ-RMT-005
    #[test]
    fn while_someone_is_helping_a_participant_asking_hears_busy_at_most_once_in_the_interval() {
        let (_, mut helped, helper_id, _) = active();
        let other = Uuid::new_v4();
        let start = Instant::now();
        let busy = vec![(other, HelpMessage::Declined { busy: true })];

        let out = helped.receive_at(other, "Cy", HelpMessage::Request, start);
        assert_eq!(sent(&out), busy);
        assert!(events(&out).is_empty(), "the user is not asked about it");
        assert_eq!(helped.helper(), Some(helper_id));

        for i in 1..50 {
            let at = start + BUSY_REPLY_INTERVAL * i / 50;
            assert!(
                helped
                    .receive_at(other, "Cy", HelpMessage::Request, at)
                    .is_empty(),
                "asking again sooner gets no answer"
            );
        }
        assert_eq!(
            sent(&helped.receive_at(
                other,
                "Cy",
                HelpMessage::Request,
                start + BUSY_REPLY_INTERVAL
            )),
            busy,
            "after the interval, one more"
        );
    }

    /// Participants working together cannot earn more answers by one of them
    /// asking and withdrawing again and again.
    ///
    /// Verifies: REQ-RMT-005
    #[test]
    fn asking_and_withdrawing_again_and_again_earns_no_more_busy_answers() {
        let (x, y) = (Uuid::new_v4(), Uuid::new_v4());
        let mut helped = Help::new();
        let now = Instant::now();
        helped.receive_at(x, "X", HelpMessage::Request, now);
        assert_eq!(
            sent(&helped.receive_at(y, "Y", HelpMessage::Request, now)).len(),
            1
        );

        for _ in 0..20 {
            helped.receive_at(x, "X", HelpMessage::Stop { role: Role::Helper }, now);
            helped.receive_at(x, "X", HelpMessage::Request, now);
            assert!(helped
                .receive_at(y, "Y", HelpMessage::Request, now)
                .is_empty());
        }
    }

    /// Verifies: REQ-RMT-005
    #[test]
    fn an_app_already_helping_someone_cannot_ask_to_help_another() {
        let (mut helper, _, _, _) = active();

        assert_eq!(
            helper.request(Uuid::new_v4()),
            Err(HelpError::AlreadyHelping)
        );
    }

    /// Verifies: REQ-RMT-003
    #[test]
    fn leaving_the_room_ends_every_help_without_sending_anything() {
        let (mut aki, mut bo, aki_id, bo_id) = active();
        let [(_, request)] = sent(&bo.request(aki_id).unwrap()).try_into().unwrap();
        aki.receive(bo_id, HELPED_NAME, request);
        aki.accept(bo_id, "c".repeat(32)).unwrap();

        let out = aki.reset();

        assert!(sent(&out).is_empty() && broadcast(&out).is_empty());
        assert_eq!(
            events(&out),
            vec![
                HelpEvent::Ended {
                    role: Role::Helper,
                    peer: bo_id,
                    reason: EndReason::Stopped
                },
                HelpEvent::Ended {
                    role: Role::Helped,
                    peer: bo_id,
                    reason: EndReason::Stopped
                }
            ]
        );
        assert!(aki.reset().is_empty());
    }

    /// Verifies: REQ-RMT-004
    #[test]
    fn a_notice_reaches_the_room_naming_the_helped_participant_as_the_server_stamped_them() {
        let helper_id = Uuid::new_v4();
        let mut anyone = Help::new();

        let out = anyone.receive(
            Uuid::new_v4(),
            HELPED_NAME,
            notice(
                NoticeEvent::Changed,
                helper_id,
                Some("buffer_size".to_string()),
            ),
        );

        assert_eq!(
            events(&out),
            vec![HelpEvent::Notice {
                event: NoticeEvent::Changed,
                helper: helper_id,
                helped_name: HELPED_NAME.to_string(),
                setting: Some("buffer_size".to_string())
            }]
        );
    }

    #[test]
    fn a_help_message_travels_under_its_topic_and_kind() {
        let body = serde_json::to_value(PeerBody::SettingsHelp(HelpMessage::Accepted {
            session: SESSION.to_string(),
        }))
        .unwrap();
        assert_eq!(
            body,
            serde_json::json!({"settings_help": {"kind": "accepted", "session": SESSION}})
        );
        let parsed: PeerBody = serde_json::from_value(serde_json::json!(
            {"settings_help": {"kind": "stop", "role": "helped"}}
        ))
        .unwrap();
        assert_eq!(
            parsed,
            PeerBody::SettingsHelp(HelpMessage::Stop { role: Role::Helped })
        );
        assert!(
            serde_json::from_value::<PeerBody>(serde_json::json!({"other_topic": {}})).is_err(),
            "a topic this app does not know is not a help message"
        );
    }

    /// A message of the earlier way of helping (a change to approve one at a
    /// time) does not parse, so it is dropped as one from another version.
    #[test]
    fn a_message_of_the_change_by_change_help_is_not_a_help_message() {
        for kind in ["propose", "answered", "settings"] {
            assert!(
                serde_json::from_value::<PeerBody>(
                    serde_json::json!({"settings_help": {"kind": kind}})
                )
                .is_err(),
                "{}",
                kind
            );
        }
    }
}
