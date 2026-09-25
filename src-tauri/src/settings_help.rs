//! Helping another participant with their audio settings (ADR-043)
//!
//! One participant (the helper) asks to help another (the helped). Once the
//! helped side accepts, the helper sees their audio settings and proposes
//! changes one at a time; each one takes effect only when the helped side
//! approves it. Either side can stop at any time, and every approved change
//! is announced to the whole room for the chat.
//!
//! [`Help`] is the protocol as a state machine with no I/O: it takes what
//! happened (a local action, a message from a peer, a peer leaving) and says
//! what to do about it ([`Out`]) - messages to send and events for the UI.
//! The caller carries those out over the signaling relay and applies approved
//! changes through [`crate::settings::apply`], the same path as a change made
//! in the settings window.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::settings::{AudioSettings, SettingChange};

/// What the two apps say to each other. Travels as the body of a peer
/// message, under [`PeerBody::SettingsHelp`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HelpMessage {
    /// Helper to helped: may I help with your settings?
    Request,
    /// Helped to helper: yes, and these are my settings now.
    Accepted { settings: AudioSettings },
    /// Helped to helper: no. `busy` when someone else is already helping.
    Declined { busy: bool },
    /// Helper to helped: please apply this change. `id` names it in the answer.
    Propose { id: u64, change: SettingChange },
    /// Helped to helper: what became of proposal `id`.
    Answered { id: u64, answer: Answer },
    /// Helped to helper: my settings changed for another reason (I changed
    /// them myself); these are them now.
    Settings { settings: AudioSettings },
    /// Either side to the other: the help is over.
    Stop,
    /// Helped to everyone in the room, for the chat. The helped participant
    /// is the sender, which the server stamps; the notice names the helper.
    Notice {
        event: NoticeEvent,
        helper: Uuid,
        helper_name: String,
        /// The setting that changed (`SettingChange`'s name), for `Changed`
        #[serde(default, skip_serializing_if = "Option::is_none")]
        setting: Option<String>,
    },
}

/// What the helped side did with a proposal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Answer {
    /// Approved and in effect; the settings as they are now.
    Applied { settings: AudioSettings },
    /// Approved, but the app refused the value (a device that went away).
    Refused { error: String },
    /// Not approved.
    Declined,
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
    /// This app stopped it.
    Stopped,
    /// The peer stopped it (or declined to start).
    PeerStopped,
    /// The peer left the room.
    PeerLeft,
}

/// Something the UI shows.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HelpEvent {
    /// `peer` asks to help this app; answer with [`Help::answer_request`].
    Requested { peer: Uuid, peer_name: String },
    /// Help with `peer` began. `settings` are the helped side's, for the helper.
    Started {
        role: Role,
        peer: Uuid,
        settings: Option<AudioSettings>,
    },
    /// `peer` declined this app's offer to help.
    Declined { peer: Uuid, busy: bool },
    /// The helper proposes `change` (naming this app's own device ids);
    /// decide with [`Help::take_proposal`].
    Proposed { id: u64, change: SettingChange },
    /// The helped side answered proposal `id`.
    Answered { id: u64, answer: Answer },
    /// The helped side's settings, as they are now.
    Settings { settings: AudioSettings },
    /// Help with `peer` is over; pending proposals are dropped.
    Ended {
        role: Role,
        peer: Uuid,
        reason: EndReason,
    },
    /// For the chat: `helper` did `event` to the settings of `helped`.
    Notice {
        event: NoticeEvent,
        helper_name: String,
        helped_name: String,
        setting: Option<String>,
    },
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
}

/// Why a local action could not be taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelpError {
    /// This app is already helping someone (or asking to).
    AlreadyHelping,
    /// There is no help going on in the role the action needs.
    NotActive,
    /// No proposal by that id is waiting.
    NoSuchProposal(u64),
    /// A proposal named a device handle this help never gave out.
    UnknownDevice(String),
}

impl std::fmt::Display for HelpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HelpError::AlreadyHelping => f.write_str("Already helping someone with their settings"),
            HelpError::NotActive => f.write_str("Not helping with settings"),
            HelpError::NoSuchProposal(id) => write!(f, "No settings change {} is waiting", id),
            HelpError::UnknownDevice(handle) => write!(f, "Device not found: {}", handle),
        }
    }
}

impl std::error::Error for HelpError {}

/// This app helping someone.
#[derive(Debug, Clone, PartialEq)]
enum Giving {
    /// Asked `peer`, no answer yet.
    Asking { peer: Uuid },
    /// Helping `peer`; `pending` are proposals not answered yet.
    Active {
        peer: Uuid,
        next_id: u64,
        pending: Vec<u64>,
    },
}

/// Stands in for device ids while someone helps: an id can carry a serial
/// number (it is masked in `jamjam.log` for that reason), so the helper sees
/// a handle instead and the helped side turns it back. A device keeps its
/// handle for the whole help, whichever list it appears in.
#[derive(Debug, Default, Clone, PartialEq)]
struct DeviceHandles {
    ids: Vec<String>,
}

impl DeviceHandles {
    const PREFIX: &'static str = "device-";

    fn handle(&mut self, id: &str) -> String {
        let index = match self.ids.iter().position(|known| known == id) {
            Some(index) => index,
            None => {
                self.ids.push(id.to_string());
                self.ids.len() - 1
            }
        };
        format!("{}{}", Self::PREFIX, index + 1)
    }

    fn id(&self, handle: &str) -> Option<&str> {
        let number: usize = handle.strip_prefix(Self::PREFIX)?.parse().ok()?;
        self.ids.get(number.checked_sub(1)?).map(String::as_str)
    }

    /// `settings` as the helper may see them.
    fn hide(&mut self, mut settings: AudioSettings) -> AudioSettings {
        for device in settings
            .input_devices
            .iter_mut()
            .chain(settings.output_devices.iter_mut())
        {
            device.id = self.handle(&device.id);
        }
        settings.input_device_id = settings.input_device_id.map(|id| self.handle(&id));
        settings.output_device_id = settings.output_device_id.map(|id| self.handle(&id));
        settings
    }

    /// `change` with the device it names turned back into its id.
    fn reveal(&self, change: SettingChange) -> Result<SettingChange, HelpError> {
        let id = |handle: String| {
            self.id(&handle)
                .map(String::from)
                .ok_or(HelpError::UnknownDevice(handle))
        };
        Ok(match change {
            SettingChange::InputDevice { device_id } => SettingChange::InputDevice {
                device_id: id(device_id)?,
            },
            SettingChange::OutputDevice { device_id } => SettingChange::OutputDevice {
                device_id: id(device_id)?,
            },
            other => other,
        })
    }
}

/// Someone helping this app.
#[derive(Debug, Clone, PartialEq)]
enum Receiving {
    /// `peer` asked; this app has not answered.
    Asked { peer: Uuid, name: String },
    /// `peer` is helping; `proposals` wait for this app's decision, oldest first.
    Active {
        peer: Uuid,
        name: String,
        proposals: Vec<(u64, SettingChange)>,
        handles: DeviceHandles,
    },
}

impl Receiving {
    fn peer(&self) -> Uuid {
        match self {
            Receiving::Asked { peer, .. } | Receiving::Active { peer, .. } => *peer,
        }
    }
}

impl Giving {
    fn peer(&self) -> Uuid {
        match self {
            Giving::Asking { peer } | Giving::Active { peer, .. } => *peer,
        }
    }
}

/// The help this app gives and receives. The two are independent: an app can
/// be helped by one participant while it helps another, but gives and
/// receives help with one participant at a time.
#[derive(Debug, Default)]
pub struct Help {
    giving: Option<Giving>,
    receiving: Option<Receiving>,
}

impl Help {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether `peer` is the one helping this app.
    #[cfg(test)]
    fn is_helped_by(&self, peer: Uuid) -> bool {
        matches!(&self.receiving, Some(Receiving::Active { peer: p, .. }) if *p == peer)
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

    /// Answers the pending request. On yes, `settings` go to the helper and the
    /// room hears that the help started (the server stamps this app as the
    /// notice's sender, so it names only the helper).
    pub fn answer_request(
        &mut self,
        accept: bool,
        settings: AudioSettings,
    ) -> Result<Vec<Out>, HelpError> {
        let Some(Receiving::Asked { peer, name }) = self.receiving.take() else {
            return Err(HelpError::NotActive);
        };
        if !accept {
            return Ok(vec![Out::Send {
                to: peer,
                message: HelpMessage::Declined { busy: false },
            }]);
        }
        let mut handles = DeviceHandles::default();
        let settings = handles.hide(settings);
        self.receiving = Some(Receiving::Active {
            peer,
            name: name.clone(),
            proposals: Vec::new(),
            handles,
        });
        Ok(vec![
            Out::Send {
                to: peer,
                message: HelpMessage::Accepted { settings },
            },
            Out::Broadcast(HelpMessage::Notice {
                event: NoticeEvent::Started,
                helper: peer,
                helper_name: name,
                setting: None,
            }),
            Out::Event(HelpEvent::Started {
                role: Role::Helped,
                peer,
                settings: None,
            }),
        ])
    }

    /// Proposes `change` to the participant this app is helping. Returns the
    /// proposal's id alongside what to send.
    pub fn propose(&mut self, change: SettingChange) -> Result<(u64, Vec<Out>), HelpError> {
        let Some(Giving::Active {
            peer,
            next_id,
            pending,
        }) = &mut self.giving
        else {
            return Err(HelpError::NotActive);
        };
        let id = *next_id;
        *next_id += 1;
        pending.push(id);
        Ok((
            id,
            vec![Out::Send {
                to: *peer,
                message: HelpMessage::Propose { id, change },
            }],
        ))
    }

    /// Takes proposal `id` off the waiting list for this app to decide on.
    /// Returns the change to apply when approving (its device, if any, is
    /// this app's id); the caller reports how it went with [`Self::report`],
    /// or declines with [`Self::decline`].
    pub fn take_proposal(&mut self, id: u64) -> Result<SettingChange, HelpError> {
        let Some(Receiving::Active { proposals, .. }) = &mut self.receiving else {
            return Err(HelpError::NotActive);
        };
        let index = proposals
            .iter()
            .position(|(pending, _)| *pending == id)
            .ok_or(HelpError::NoSuchProposal(id))?;
        Ok(proposals.remove(index).1)
    }

    /// Declines proposal `id`, taken with [`Self::take_proposal`].
    pub fn decline(&mut self, id: u64) -> Result<Vec<Out>, HelpError> {
        let Some(Receiving::Active { peer, .. }) = &self.receiving else {
            return Err(HelpError::NotActive);
        };
        Ok(vec![Out::Send {
            to: *peer,
            message: HelpMessage::Answered {
                id,
                answer: Answer::Declined,
            },
        }])
    }

    /// Reports how applying proposal `id` went. An applied change is announced
    /// to the room for the chat.
    pub fn report(
        &mut self,
        id: u64,
        change: &SettingChange,
        result: Result<AudioSettings, String>,
    ) -> Result<Vec<Out>, HelpError> {
        let Some(Receiving::Active {
            peer,
            name,
            handles,
            ..
        }) = &mut self.receiving
        else {
            return Err(HelpError::NotActive);
        };
        let mut out = Vec::new();
        let answer = match result {
            Ok(settings) => {
                out.push(Out::Broadcast(HelpMessage::Notice {
                    event: NoticeEvent::Changed,
                    helper: *peer,
                    helper_name: name.clone(),
                    setting: Some(setting_name(change)),
                }));
                Answer::Applied {
                    settings: handles.hide(settings),
                }
            }
            Err(error) => Answer::Refused { error },
        };
        out.insert(
            0,
            Out::Send {
                to: *peer,
                message: HelpMessage::Answered { id, answer },
            },
        );
        Ok(out)
    }

    /// This app's settings changed while someone helps it: the helper's view
    /// follows.
    pub fn settings_changed(&mut self, settings: AudioSettings) -> Vec<Out> {
        match &mut self.receiving {
            Some(Receiving::Active { peer, handles, .. }) => vec![Out::Send {
                to: *peer,
                message: HelpMessage::Settings {
                    settings: handles.hide(settings),
                },
            }],
            _ => Vec::new(),
        }
    }

    /// Stops the help in `role`. Pending proposals are dropped.
    pub fn stop(&mut self, role: Role) -> Result<Vec<Out>, HelpError> {
        match role {
            Role::Helper => {
                let giving = self.giving.take().ok_or(HelpError::NotActive)?;
                let peer = giving.peer();
                Ok(vec![
                    Out::Send {
                        to: peer,
                        message: HelpMessage::Stop,
                    },
                    Out::Event(HelpEvent::Ended {
                        role,
                        peer,
                        reason: EndReason::Stopped,
                    }),
                ])
            }
            Role::Helped => {
                let receiving = self.receiving.take().ok_or(HelpError::NotActive)?;
                let peer = receiving.peer();
                let mut out = vec![Out::Send {
                    to: peer,
                    message: HelpMessage::Stop,
                }];
                if let Receiving::Active { name, .. } = receiving {
                    out.push(Out::Broadcast(ended_notice(peer, name)));
                }
                out.push(Out::Event(HelpEvent::Ended {
                    role,
                    peer,
                    reason: EndReason::Stopped,
                }));
                Ok(out)
            }
        }
    }

    /// A help message arrived from `from` (stamped by the server, so it is who
    /// it says), whose room name is `from_name`.
    ///
    /// Anything that does not fit the current state - an answer to a request
    /// never made, a proposal from someone who is not helping - is dropped.
    pub fn receive(&mut self, from: Uuid, from_name: &str, message: HelpMessage) -> Vec<Out> {
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
                Some(_) => vec![Out::Send {
                    to: from,
                    message: HelpMessage::Declined { busy: true },
                }],
            },
            HelpMessage::Accepted { settings } => match &self.giving {
                Some(Giving::Asking { peer }) if *peer == from => {
                    self.giving = Some(Giving::Active {
                        peer: from,
                        next_id: 1,
                        pending: Vec::new(),
                    });
                    vec![Out::Event(HelpEvent::Started {
                        role: Role::Helper,
                        peer: from,
                        settings: Some(settings),
                    })]
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
            HelpMessage::Propose { id, change } => match &mut self.receiving {
                Some(Receiving::Active {
                    peer,
                    proposals,
                    handles,
                    ..
                }) if *peer == from => match handles.reveal(change) {
                    // Asked about as this app's own device, so the question
                    // can name it.
                    Ok(change) => {
                        proposals.push((id, change.clone()));
                        vec![Out::Event(HelpEvent::Proposed { id, change })]
                    }
                    // A handle this help never gave out: nothing to ask about.
                    Err(e) => vec![Out::Send {
                        to: from,
                        message: HelpMessage::Answered {
                            id,
                            answer: Answer::Refused {
                                error: e.to_string(),
                            },
                        },
                    }],
                },
                _ => Vec::new(),
            },
            HelpMessage::Answered { id, answer } => match &mut self.giving {
                Some(Giving::Active { peer, pending, .. }) if *peer == from => {
                    let Some(index) = pending.iter().position(|p| *p == id) else {
                        return Vec::new();
                    };
                    pending.remove(index);
                    vec![Out::Event(HelpEvent::Answered { id, answer })]
                }
                _ => Vec::new(),
            },
            HelpMessage::Settings { settings } => match &self.giving {
                Some(Giving::Active { peer, .. }) if *peer == from => {
                    vec![Out::Event(HelpEvent::Settings { settings })]
                }
                _ => Vec::new(),
            },
            HelpMessage::Stop => self.end_with(from, EndReason::PeerStopped),
            HelpMessage::Notice {
                event,
                helper_name,
                setting,
                ..
            } => vec![Out::Event(HelpEvent::Notice {
                event,
                helper_name,
                helped_name: from_name.to_string(),
                setting,
            })],
        }
    }

    /// `peer` left the room: any help with them is over.
    pub fn peer_left(&mut self, peer: Uuid) -> Vec<Out> {
        self.end_with(peer, EndReason::PeerLeft)
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

    fn end_with(&mut self, peer: Uuid, reason: EndReason) -> Vec<Out> {
        let mut out = Vec::new();
        if self.giving.as_ref().is_some_and(|g| g.peer() == peer) {
            self.giving = None;
            out.push(Out::Event(HelpEvent::Ended {
                role: Role::Helper,
                peer,
                reason,
            }));
        }
        if self.receiving.as_ref().is_some_and(|r| r.peer() == peer) {
            if let Some(Receiving::Active { name, .. }) = self.receiving.take() {
                out.push(Out::Broadcast(ended_notice(peer, name)));
            }
            self.receiving = None;
            out.push(Out::Event(HelpEvent::Ended {
                role: Role::Helped,
                peer,
                reason,
            }));
        }
        out
    }
}

fn ended_notice(helper: Uuid, helper_name: String) -> HelpMessage {
    HelpMessage::Notice {
        event: NoticeEvent::Ended,
        helper,
        helper_name,
        setting: None,
    }
}

/// The name a change travels under (`"input_device"`, `"buffer_size"`, ...),
/// which the chat turns into the setting's label.
pub fn setting_name(change: &SettingChange) -> String {
    serde_json::to_value(change)
        .ok()
        .and_then(|v| v.get("setting").and_then(|s| s.as_str()).map(String::from))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::ChannelPair;

    fn settings(buffer_size: u32) -> AudioSettings {
        AudioSettings {
            revision: 0,
            input_devices: vec![],
            output_devices: vec![],
            input_device_id: None,
            output_device_id: None,
            input_channel_count: None,
            output_channel_count: None,
            input_channels: ChannelPair {
                left: 1,
                right: Some(2),
            },
            output_channels: ChannelPair {
                left: 1,
                right: Some(2),
            },
            transmit_channels: 2,
            buffer_size,
            buffer_sizes: vec![32, 64, 128, 256],
            sample_rate: 48000,
            sample_rates: vec![],
        }
    }

    fn buffer(samples: u32) -> SettingChange {
        SettingChange::BufferSize { samples }
    }

    const HELPER_NAME: &str = "Aki";
    const HELPED_NAME: &str = "Bo";

    /// A helper app and a helped app with the help under way, and the ids of
    /// the two participants.
    fn active() -> (Help, Help, Uuid, Uuid) {
        let helper_id = Uuid::new_v4();
        let helped_id = Uuid::new_v4();
        let mut helper = Help::new();
        let mut helped = Help::new();
        helper.request(helped_id).unwrap();
        helped.receive(helper_id, HELPER_NAME, HelpMessage::Request);
        helped.answer_request(true, settings(64)).unwrap();
        helper.receive(
            helped_id,
            HELPED_NAME,
            HelpMessage::Accepted {
                settings: settings(64),
            },
        );
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

    /// Verifies: REQ-RMT-001
    #[test]
    fn when_the_helped_side_accepts_the_helper_receives_their_settings_and_the_room_hears_it() {
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

        let out = helped.answer_request(true, settings(64)).unwrap();
        assert_eq!(
            sent(&out),
            vec![(
                helper_id,
                HelpMessage::Accepted {
                    settings: settings(64)
                }
            )]
        );
        assert_eq!(
            broadcast(&out),
            vec![HelpMessage::Notice {
                event: NoticeEvent::Started,
                helper: helper_id,
                helper_name: HELPER_NAME.into(),
                setting: None,
            }]
        );

        let started = helper.receive(
            helped_id,
            HELPED_NAME,
            HelpMessage::Accepted {
                settings: settings(64),
            },
        );
        assert_eq!(
            events(&started),
            vec![HelpEvent::Started {
                role: Role::Helper,
                peer: helped_id,
                settings: Some(settings(64))
            }]
        );
        assert!(helped.is_helped_by(helper_id));
    }

    /// Verifies: REQ-RMT-001
    #[test]
    fn when_the_helped_side_declines_no_help_starts_and_proposals_are_not_possible() {
        let helper_id = Uuid::new_v4();
        let helped_id = Uuid::new_v4();
        let mut helper = Help::new();
        let mut helped = Help::new();
        helper.request(helped_id).unwrap();
        helped.receive(helper_id, HELPER_NAME, HelpMessage::Request);

        let out = helped.answer_request(false, settings(64)).unwrap();
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
        assert_eq!(helper.propose(buffer(128)), Err(HelpError::NotActive));
        assert!(!helped.is_helped_by(helper_id));
    }

    /// The helped side sees what is proposed and nothing changes until it
    /// approves - the safeguard against the helper's mistakes.
    ///
    /// Verifies: REQ-RMT-002
    #[test]
    fn a_proposed_change_reaches_the_helped_side_as_a_question_and_is_not_applied_by_itself() {
        let (mut helper, mut helped, _, helped_id) = active();

        let (id, out) = helper.propose(buffer(128)).unwrap();
        let [(to, message)] = sent(&out).try_into().unwrap();
        assert_eq!(to, helped_id);

        let arrived = helped.receive(Uuid::nil(), "", message.clone());
        assert!(
            arrived.is_empty(),
            "a proposal from someone who is not helping is dropped"
        );
        let helper_id = helped.helper().unwrap();
        let arrived = helped.receive(helper_id, HELPER_NAME, message);
        assert_eq!(
            events(&arrived),
            vec![HelpEvent::Proposed {
                id,
                change: buffer(128)
            }]
        );
        assert!(
            sent(&arrived).is_empty() && broadcast(&arrived).is_empty(),
            "the change waits for a decision"
        );
    }

    /// Verifies: REQ-RMT-002
    /// Verifies: REQ-RMT-004
    #[test]
    fn an_approved_change_is_answered_with_the_new_settings_and_announced_to_the_room() {
        let (mut helper, mut helped, helper_id, helped_id) = active();
        let (id, out) = helper.propose(buffer(128)).unwrap();
        let [(_, message)] = sent(&out).try_into().unwrap();
        helped.receive(helper_id, HELPER_NAME, message);

        let change = helped.take_proposal(id).unwrap();
        assert_eq!(change, buffer(128));
        let out = helped.report(id, &change, Ok(settings(128))).unwrap();

        assert_eq!(
            sent(&out),
            vec![(
                helper_id,
                HelpMessage::Answered {
                    id,
                    answer: Answer::Applied {
                        settings: settings(128)
                    }
                }
            )]
        );
        let [notice] = broadcast(&out).try_into().unwrap();
        assert_eq!(
            notice,
            HelpMessage::Notice {
                event: NoticeEvent::Changed,
                helper: helper_id,
                helper_name: HELPER_NAME.into(),
                setting: Some("buffer_size".into()),
            }
        );

        // Every app in the room, helper and helped included, turns it into
        // a chat line naming both.
        assert_eq!(
            events(&Help::new().receive(helped_id, HELPED_NAME, notice)),
            vec![HelpEvent::Notice {
                event: NoticeEvent::Changed,
                helper_name: HELPER_NAME.into(),
                helped_name: HELPED_NAME.into(),
                setting: Some("buffer_size".into()),
            }]
        );
    }

    /// Verifies: REQ-RMT-002
    #[test]
    fn a_declined_change_is_answered_as_declined_and_the_room_hears_nothing() {
        let (mut helper, mut helped, helper_id, helped_id) = active();
        let (id, out) = helper.propose(buffer(128)).unwrap();
        let [(_, message)] = sent(&out).try_into().unwrap();
        helped.receive(helper_id, HELPER_NAME, message);

        helped.take_proposal(id).unwrap();
        let out = helped.decline(id).unwrap();

        let [(_, answer)] = sent(&out).try_into().unwrap();
        assert!(broadcast(&out).is_empty());
        assert_eq!(
            events(&helper.receive(helped_id, HELPED_NAME, answer)),
            vec![HelpEvent::Answered {
                id,
                answer: Answer::Declined
            }]
        );
        assert_eq!(
            helped.take_proposal(id),
            Err(HelpError::NoSuchProposal(id)),
            "a decided proposal cannot be applied later"
        );
    }

    /// Verifies: REQ-RMT-003
    #[test]
    fn when_the_helper_stops_the_helped_side_ends_too_and_later_proposals_go_nowhere() {
        let (mut helper, mut helped, helper_id, helped_id) = active();
        let (id, out) = helper.propose(buffer(128)).unwrap();
        let [(_, proposal)] = sent(&out).try_into().unwrap();
        helped.receive(helper_id, HELPER_NAME, proposal.clone());

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
            vec![ended_notice(helper_id, HELPER_NAME.into())]
        );
        assert_eq!(
            helped.take_proposal(id),
            Err(HelpError::NotActive),
            "the proposal waiting when help ended is dropped"
        );
        assert!(helped.receive(helper_id, HELPER_NAME, proposal).is_empty());
        assert_eq!(helper.propose(buffer(64)), Err(HelpError::NotActive));
    }

    /// Verifies: REQ-RMT-003
    #[test]
    fn when_the_helped_side_stops_the_helper_ends_and_the_room_hears_it() {
        let (mut helper, mut helped, helper_id, helped_id) = active();

        let out = helped.stop(Role::Helped).unwrap();
        assert_eq!(
            broadcast(&out),
            vec![ended_notice(helper_id, HELPER_NAME.into())]
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
        assert_eq!(helper.propose(buffer(64)), Err(HelpError::NotActive));
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
            helped.answer_request(true, settings(64)),
            Err(HelpError::NotActive),
            "a withdrawn request can no longer be accepted"
        );
    }

    /// Verifies: REQ-RMT-003
    #[test]
    fn when_the_helper_leaves_the_room_the_help_ends_on_the_helped_side() {
        let (_, mut helped, helper_id, _) = active();

        let out = helped.peer_left(helper_id);

        assert!(events(&out).contains(&HelpEvent::Ended {
            role: Role::Helped,
            peer: helper_id,
            reason: EndReason::PeerLeft
        }));
        assert!(!helped.is_helped_by(helper_id));
    }

    /// Verifies: REQ-RMT-005
    #[test]
    fn while_someone_is_helping_a_second_request_is_turned_down_as_busy() {
        let (_, mut helped, helper_id, _) = active();
        let other = Uuid::new_v4();

        let out = helped.receive(other, "Cy", HelpMessage::Request);

        assert_eq!(
            sent(&out),
            vec![(other, HelpMessage::Declined { busy: true })]
        );
        assert!(events(&out).is_empty(), "the user is not asked about it");
        assert!(helped.is_helped_by(helper_id));
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

    /// The helper only sees answers and settings from the one it helps.
    ///
    /// Verifies: REQ-RMT-002
    #[test]
    fn answers_and_settings_from_anyone_but_the_helped_side_are_dropped() {
        let (mut helper, _, _, helped_id) = active();
        let (id, _) = helper.propose(buffer(128)).unwrap();
        let stranger = Uuid::new_v4();

        assert!(helper
            .receive(
                stranger,
                "Cy",
                HelpMessage::Answered {
                    id,
                    answer: Answer::Declined
                }
            )
            .is_empty());
        assert!(helper
            .receive(
                stranger,
                "Cy",
                HelpMessage::Settings {
                    settings: settings(32)
                }
            )
            .is_empty());
        assert!(helper
            .receive(
                stranger,
                "Cy",
                HelpMessage::Accepted {
                    settings: settings(32)
                }
            )
            .is_empty());
        assert_eq!(
            events(&helper.receive(
                helped_id,
                HELPED_NAME,
                HelpMessage::Answered {
                    id,
                    answer: Answer::Declined
                }
            ))
            .len(),
            1
        );
    }

    #[test]
    fn a_change_the_helped_side_makes_itself_reaches_the_helper() {
        let (_, mut helped, helper_id, _) = active();

        assert_eq!(
            sent(&helped.settings_changed(settings(256))),
            vec![(
                helper_id,
                HelpMessage::Settings {
                    settings: settings(256)
                }
            )]
        );
        assert!(Help::new().settings_changed(settings(256)).is_empty());
    }

    #[test]
    fn leaving_the_room_ends_every_help_without_sending_anything() {
        let (mut helper, mut helped, _, _) = active();

        for out in [helper.reset(), helped.reset()] {
            assert!(sent(&out).is_empty() && broadcast(&out).is_empty());
            assert_eq!(events(&out).len(), 1);
        }
        assert_eq!(helper.propose(buffer(64)), Err(HelpError::NotActive));
    }

    /// Settings offering the devices `ids`, named "Device 1", "Device 2", ...
    fn with_devices(ids: &[&str]) -> AudioSettings {
        let device = |(i, id): (usize, &&str)| crate::audio::AudioDeviceInfo {
            id: id.to_string(),
            name: format!("Device {}", i + 1),
            supported_sample_rates: vec![48000],
            supported_channels: vec![2],
            is_default: false,
            is_asio: false,
        };
        AudioSettings {
            input_devices: ids.iter().enumerate().map(device).collect(),
            output_devices: ids.iter().enumerate().map(device).collect(),
            input_device_id: Some(ids[0].to_string()),
            output_device_id: Some(ids[0].to_string()),
            ..settings(64)
        }
    }

    const SERIAL: &str = "20221310";

    /// Device ids can carry serial numbers; the helper sees the names and a
    /// handle per device, never the id.
    ///
    /// Verifies: REQ-RMT-006
    #[test]
    fn the_helper_sees_device_names_but_never_device_ids() {
        let helper_id = Uuid::new_v4();
        let mut helped = Help::new();
        helped.receive(helper_id, HELPER_NAME, HelpMessage::Request);
        let id = format!("coreaudio:AG06:{SERIAL}:1,2");

        let out = helped
            .answer_request(true, with_devices(&[&id, "alsa:builtin"]))
            .unwrap();

        let [(_, accepted)] = sent(&out).try_into().unwrap();
        let json = serde_json::to_string(&accepted).unwrap();
        assert!(!json.contains(SERIAL), "{}", json);
        assert!(json.contains("Device 1"), "the names are shown: {}", json);
        let HelpMessage::Accepted { settings } = accepted else {
            panic!("not accepted");
        };
        assert_eq!(
            settings.input_device_id,
            settings.input_devices[0].id.clone().into(),
            "the selection points at the same handle as its device"
        );
        assert_eq!(
            settings.input_devices[0].id, settings.output_devices[0].id,
            "a device keeps one handle in both lists"
        );
    }

    /// Verifies: REQ-RMT-006
    #[test]
    fn a_proposed_device_is_asked_about_as_the_helped_sides_own_and_an_unknown_one_is_refused() {
        let helper_id = Uuid::new_v4();
        let mut helped = Help::new();
        helped.receive(helper_id, HELPER_NAME, HelpMessage::Request);
        let id = format!("coreaudio:AG06:{SERIAL}:1,2");
        let out = helped
            .answer_request(true, with_devices(&["alsa:builtin", &id]))
            .unwrap();
        let [(_, HelpMessage::Accepted { settings })] = <[_; 1]>::try_from(sent(&out)).unwrap()
        else {
            panic!("not accepted");
        };
        let handle = settings.input_devices[1].id.clone();

        let asked = helped.receive(
            helper_id,
            HELPER_NAME,
            HelpMessage::Propose {
                id: 1,
                change: SettingChange::InputDevice { device_id: handle },
            },
        );
        let unknown = helped.receive(
            helper_id,
            HELPER_NAME,
            HelpMessage::Propose {
                id: 2,
                change: SettingChange::OutputDevice {
                    device_id: "device-99".to_string(),
                },
            },
        );

        let real = SettingChange::InputDevice { device_id: id };
        assert_eq!(
            events(&asked),
            vec![HelpEvent::Proposed {
                id: 1,
                change: real.clone()
            }],
            "the user is asked about their own device"
        );
        assert_eq!(helped.take_proposal(1), Ok(real));
        assert!(events(&unknown).is_empty(), "the user is not asked");
        assert_eq!(
            sent(&unknown),
            vec![(
                helper_id,
                HelpMessage::Answered {
                    id: 2,
                    answer: Answer::Refused {
                        error: "Device not found: device-99".to_string()
                    }
                }
            )]
        );
        assert_eq!(helped.take_proposal(2), Err(HelpError::NoSuchProposal(2)));
    }

    /// The wire form is part of the protocol between two apps of different
    /// versions, so it is pinned.
    #[test]
    fn a_help_message_travels_under_its_topic_and_kind() {
        let body = PeerBody::SettingsHelp(HelpMessage::Propose {
            id: 3,
            change: buffer(128),
        });
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "settings_help": {
                    "kind": "propose",
                    "id": 3,
                    "change": { "setting": "buffer_size", "samples": 128 }
                }
            })
        );
        assert_eq!(serde_json::from_value::<PeerBody>(json).unwrap(), body);
        assert!(
            serde_json::from_value::<PeerBody>(serde_json::json!({ "other_topic": {} })).is_err(),
            "a topic this app does not know does not parse"
        );
    }
}
