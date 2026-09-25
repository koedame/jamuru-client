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
//!
//! The other app is not trusted to follow the protocol. What the helped side
//! approves is the question it numbered and showed itself, never a number the
//! helper chose; a proposal that arrives while one waits, or that names a
//! device this help never showed, is dropped. The only answer a peer can get
//! without this app's user acting is "busy", at most once per participant in
//! [`BUSY_REPLY_INTERVAL`], so peers cannot make this app exceed the server's
//! message limit and lose the room.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::settings::{AudioSettings, SettingChange, SettingsError};

/// A participant asking while this app is already helped hears "busy" at
/// most once in this time; asking again sooner gets no answer.
pub const BUSY_REPLY_INTERVAL: Duration = Duration::from_secs(10);

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
    /// Helper to helped: please apply this change. `id` names it in the
    /// answer. One at a time: a proposal sent while another waits is dropped.
    Propose { id: u64, change: SettingChange },
    /// Helped to helper: what became of proposal `id`.
    Answered { id: u64, answer: Answer },
    /// Helped to helper: my settings changed for another reason (I changed
    /// them myself); these are them now.
    Settings { settings: AudioSettings },
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

/// What the helped side did with a proposal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Answer {
    /// Approved and in effect; the settings as they are now.
    Applied { settings: Box<AudioSettings> },
    /// Approved, but the app refused the value.
    Refused { reason: Refusal },
    /// Not approved.
    Declined,
}

/// Why the helped app refused a change its user approved. A closed set rather
/// than the error's text, which can name a device id or a file path; the
/// helper's app says it in its own language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    /// The device is no longer offered (it was unplugged).
    DeviceGone,
    /// The value does not fit (a channel the device lacks, a size not offered).
    InvalidValue,
    /// The settings could not be read or saved.
    Unavailable,
}

impl From<&SettingsError> for Refusal {
    fn from(error: &SettingsError) -> Self {
        match error {
            SettingsError::DeviceNotOffered(_) => Refusal::DeviceGone,
            SettingsError::ChannelOutOfRange { .. }
            | SettingsError::NoLeftChannel
            | SettingsError::InvalidTransmitChannels(_)
            | SettingsError::InvalidBufferSize(_)
            | SettingsError::InvalidSampleRate(_) => Refusal::InvalidValue,
            SettingsError::Unavailable(_) => Refusal::Unavailable,
        }
    }
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
    /// The peer stopped it (or withdrew the request).
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
    /// The helper proposes `change` (naming this app's own device ids).
    /// `id` is this app's number for the question; decide with
    /// [`Help::take_proposal`]. `device_name` is the name the helper saw for
    /// the device it proposes, if the change is to a device.
    Proposed {
        id: u64,
        change: SettingChange,
        device_name: Option<String>,
    },
    /// The helped side answered this app's proposal `id`.
    Answered { id: u64, answer: Answer },
    /// The helped side's settings, as they are now.
    Settings { settings: AudioSettings },
    /// Help with `peer` is over; a waiting proposal is dropped.
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
    /// There is no help going on in the role the action needs, or the
    /// request being answered is not the one waiting.
    NotActive,
    /// The last proposal has not been answered yet.
    Waiting,
    /// No question by that number is waiting.
    NoSuchProposal(u64),
}

impl std::fmt::Display for HelpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HelpError::AlreadyHelping => f.write_str("Already helping someone with their settings"),
            HelpError::NotActive => f.write_str("Not helping with settings"),
            HelpError::Waiting => f.write_str("The last change is still waiting for an answer"),
            HelpError::NoSuchProposal(id) => write!(f, "No settings change {} is waiting", id),
        }
    }
}

impl std::error::Error for HelpError {}

/// This app helping someone.
#[derive(Debug, Clone, PartialEq)]
enum Giving {
    /// Asked `peer`, no answer yet.
    Asking { peer: Uuid },
    /// Helping `peer`; `waiting` is the proposal not answered yet.
    Active {
        peer: Uuid,
        next_id: u64,
        waiting: Option<u64>,
    },
}

impl Giving {
    fn peer(&self) -> Uuid {
        match self {
            Giving::Asking { peer } | Giving::Active { peer, .. } => *peer,
        }
    }
}

/// Stands in for device ids while someone helps: an id can carry a serial
/// number (it is masked in `jamjam.log` for that reason), so the helper sees
/// a handle instead and the helped side turns it back. Inputs and outputs are
/// numbered apart, so a handle from one list cannot pick a device in the other.
#[derive(Debug, Default, Clone, PartialEq)]
struct DeviceHandles {
    input: Vec<Shown>,
    output: Vec<Shown>,
}

/// A device the helper was shown: its id, and the name it was listed under
/// (none for a chosen device the list did not hold).
#[derive(Debug, Clone, PartialEq)]
struct Shown {
    id: String,
    name: Option<String>,
}

impl DeviceHandles {
    const INPUT: &'static str = "input-";
    const OUTPUT: &'static str = "output-";

    fn handle(shown: &mut Vec<Shown>, prefix: &str, id: &str, name: Option<&str>) -> String {
        let index = match shown.iter().position(|known| known.id == id) {
            Some(index) => index,
            None => {
                shown.push(Shown {
                    id: id.to_string(),
                    name: None,
                });
                shown.len() - 1
            }
        };
        if let Some(name) = name {
            shown[index].name = Some(name.to_string());
        }
        format!("{}{}", prefix, index + 1)
    }

    fn find<'s>(shown: &'s [Shown], prefix: &str, handle: &str) -> Option<&'s Shown> {
        let number: usize = handle.strip_prefix(prefix)?.parse().ok()?;
        shown.get(number.checked_sub(1)?)
    }

    /// `settings` as the helper may see them.
    fn hide(&mut self, mut settings: AudioSettings) -> AudioSettings {
        for device in settings.input_devices.iter_mut() {
            device.id = Self::handle(&mut self.input, Self::INPUT, &device.id, Some(&device.name));
        }
        for device in settings.output_devices.iter_mut() {
            device.id = Self::handle(
                &mut self.output,
                Self::OUTPUT,
                &device.id,
                Some(&device.name),
            );
        }
        settings.input_device_id = settings
            .input_device_id
            .map(|id| Self::handle(&mut self.input, Self::INPUT, &id, None));
        settings.output_device_id = settings
            .output_device_id
            .map(|id| Self::handle(&mut self.output, Self::OUTPUT, &id, None));
        settings
    }

    /// `change` with the device it names turned back into its id, and that
    /// device's name as the helper saw it; `None` for a handle this help
    /// never showed in that list.
    fn reveal(&self, change: SettingChange) -> Option<(SettingChange, Option<String>)> {
        Some(match change {
            SettingChange::InputDevice { device_id } => {
                let shown = Self::find(&self.input, Self::INPUT, &device_id)?;
                (
                    SettingChange::InputDevice {
                        device_id: shown.id.clone(),
                    },
                    shown.name.clone(),
                )
            }
            SettingChange::OutputDevice { device_id } => {
                let shown = Self::find(&self.output, Self::OUTPUT, &device_id)?;
                (
                    SettingChange::OutputDevice {
                        device_id: shown.id.clone(),
                    },
                    shown.name.clone(),
                )
            }
            other => (other, None),
        })
    }
}

/// The proposal the helped side is asking its user about.
#[derive(Debug, Clone, PartialEq)]
struct Question {
    /// This app's number for it: what the UI answers with
    number: u64,
    /// The helper's id for it: what the answer names
    id: u64,
    change: SettingChange,
}

/// Someone helping this app.
#[derive(Debug, Clone, PartialEq)]
enum Receiving {
    /// `peer` asked; this app has not answered.
    Asked { peer: Uuid, name: String },
    /// `peer` is helping.
    Active {
        peer: Uuid,
        name: String,
        /// Which help this is (each accepted request gets the next number):
        /// an answer belongs to the help its question came from
        help: u64,
        question: Option<Question>,
        /// The number the last question got
        asked: u64,
        handles: DeviceHandles,
        /// The revision of the settings the helper was last sent
        shown_revision: u64,
    },
}

impl Receiving {
    fn peer(&self) -> Uuid {
        match self {
            Receiving::Asked { peer, .. } | Receiving::Active { peer, .. } => *peer,
        }
    }
}

/// A proposal taken off the helped side's list to be applied or declined.
/// Kept by the caller while the change is applied, so the answer still goes
/// out (and the room still hears of an applied change) if the help ended
/// meanwhile.
#[derive(Debug, Clone, PartialEq)]
pub struct Taken {
    helper: Uuid,
    help: u64,
    id: u64,
    pub change: SettingChange,
}

/// The help this app gives and receives. The two are independent: an app can
/// be helped by one participant while it helps another (or the same one),
/// but gives and receives help with one participant at a time.
#[derive(Debug, Default)]
pub struct Help {
    giving: Option<Giving>,
    receiving: Option<Receiving>,
    /// The number the last accepted request got
    helps: u64,
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

    /// Answers `peer`'s request, the one the user was shown. On yes,
    /// `settings` go to the helper and the room hears that the help started.
    pub fn answer_request(
        &mut self,
        peer: Uuid,
        accept: bool,
        settings: AudioSettings,
    ) -> Result<Vec<Out>, HelpError> {
        let name = match &self.receiving {
            Some(Receiving::Asked { peer: asking, name }) if *asking == peer => name.clone(),
            _ => return Err(HelpError::NotActive),
        };
        if !accept {
            self.receiving.take();
            return Ok(vec![Out::Send {
                to: peer,
                message: HelpMessage::Declined { busy: false },
            }]);
        }
        let mut handles = DeviceHandles::default();
        let shown_revision = settings.revision;
        let settings = handles.hide(settings);
        self.helps += 1;
        self.receiving = Some(Receiving::Active {
            peer,
            name,
            help: self.helps,
            question: None,
            asked: 0,
            handles,
            shown_revision,
        });
        Ok(vec![
            Out::Send {
                to: peer,
                message: HelpMessage::Accepted { settings },
            },
            Out::Broadcast(notice(NoticeEvent::Started, peer, None)),
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
            waiting,
        }) = &mut self.giving
        else {
            return Err(HelpError::NotActive);
        };
        if waiting.is_some() {
            return Err(HelpError::Waiting);
        }
        let id = *next_id;
        *next_id += 1;
        *waiting = Some(id);
        Ok((
            id,
            vec![Out::Send {
                to: *peer,
                message: HelpMessage::Propose { id, change },
            }],
        ))
    }

    /// Takes the question numbered `number` - the one the user answered - for
    /// this app to apply or decline. The change it returns names this app's
    /// own device ids; pass it back to [`Self::report`] or [`Self::decline`].
    pub fn take_proposal(&mut self, number: u64) -> Result<Taken, HelpError> {
        let Some(Receiving::Active {
            peer,
            help,
            question,
            ..
        }) = &mut self.receiving
        else {
            return Err(HelpError::NotActive);
        };
        if question.as_ref().map(|q| q.number) != Some(number) {
            return Err(HelpError::NoSuchProposal(number));
        }
        let question = question.take().expect("checked above");
        Ok(Taken {
            helper: *peer,
            help: *help,
            id: question.id,
            change: question.change,
        })
    }

    /// Declines `taken`. Nothing to say if the help ended meanwhile.
    pub fn decline(&mut self, taken: Taken) -> Vec<Out> {
        if !self.still_in(&taken) {
            return Vec::new();
        }
        vec![Out::Send {
            to: taken.helper,
            message: HelpMessage::Answered {
                id: taken.id,
                answer: Answer::Declined,
            },
        }]
    }

    /// Reports how applying `taken` went. An applied change is announced to
    /// the room for the chat even if the help ended while it was applied: it
    /// did change the settings. The helper hears the answer only in the same
    /// help - not in a later one the two started meanwhile, where the id
    /// would name another proposal.
    pub fn report(&mut self, taken: Taken, result: Result<AudioSettings, Refusal>) -> Vec<Out> {
        let mut out = Vec::new();
        if let Some(Receiving::Active {
            help,
            handles,
            shown_revision,
            ..
        }) = &mut self.receiving
        {
            if *help == taken.help {
                let answer = match &result {
                    Ok(settings) => {
                        *shown_revision = (*shown_revision).max(settings.revision);
                        Answer::Applied {
                            settings: Box::new(handles.hide(settings.clone())),
                        }
                    }
                    Err(reason) => Answer::Refused { reason: *reason },
                };
                out.push(Out::Send {
                    to: taken.helper,
                    message: HelpMessage::Answered {
                        id: taken.id,
                        answer,
                    },
                });
            }
        }
        if result.is_ok() {
            out.push(Out::Broadcast(notice(
                NoticeEvent::Changed,
                taken.helper,
                Some(setting_name(&taken.change)),
            )));
        }
        out
    }

    /// This app's settings changed while someone helps it: the helper's view
    /// follows. Settings the helper has already been sent are not sent again.
    pub fn settings_changed(&mut self, settings: AudioSettings) -> Vec<Out> {
        match &mut self.receiving {
            Some(Receiving::Active {
                peer,
                handles,
                shown_revision,
                ..
            }) if settings.revision > *shown_revision => {
                *shown_revision = settings.revision;
                vec![Out::Send {
                    to: *peer,
                    message: HelpMessage::Settings {
                        settings: handles.hide(settings),
                    },
                }]
            }
            _ => Vec::new(),
        }
    }

    /// Stops the help in `role`. A waiting proposal is dropped.
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
        out.push(Out::Event(HelpEvent::Ended {
            role,
            peer,
            reason: EndReason::Stopped,
        }));
        Ok(out)
    }

    /// A help message arrived from `from` (stamped by the server, so it is who
    /// it says), whose room name is `from_name`.
    ///
    /// Anything that does not fit the current state - an answer to a request
    /// never made, a proposal from someone who is not helping - is dropped.
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
            HelpMessage::Accepted { settings } => match &self.giving {
                Some(Giving::Asking { peer }) if *peer == from => {
                    self.giving = Some(Giving::Active {
                        peer: from,
                        next_id: 1,
                        waiting: None,
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
                    question: question @ None,
                    asked,
                    handles,
                    ..
                }) if *peer == from => match handles.reveal(change) {
                    // Asked about as this app's own device, under the name
                    // the helper saw.
                    Some((change, device_name)) => {
                        *asked += 1;
                        *question = Some(Question {
                            number: *asked,
                            id,
                            change: change.clone(),
                        });
                        vec![Out::Event(HelpEvent::Proposed {
                            id: *asked,
                            change,
                            device_name,
                        })]
                    }
                    // A handle this help never showed: the helper's app
                    // does not send one, so there is no one to explain to.
                    None => Vec::new(),
                },
                _ => Vec::new(),
            },
            HelpMessage::Answered { id, answer } => match &mut self.giving {
                Some(Giving::Active { peer, waiting, .. })
                    if *peer == from && *waiting == Some(id) =>
                {
                    *waiting = None;
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

    /// Whether the help `taken` came from is still going on.
    fn still_in(&self, taken: &Taken) -> bool {
        matches!(&self.receiving, Some(Receiving::Active { help, .. }) if *help == taken.help)
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

fn notice(event: NoticeEvent, helper: Uuid, setting: Option<String>) -> HelpMessage {
    HelpMessage::Notice {
        event,
        helper,
        setting,
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

    fn at_revision(revision: u64, settings: AudioSettings) -> AudioSettings {
        AudioSettings {
            revision,
            ..settings
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
        active_with(settings(64))
    }

    fn active_with(helped_settings: AudioSettings) -> (Help, Help, Uuid, Uuid) {
        let helper_id = Uuid::new_v4();
        let helped_id = Uuid::new_v4();
        let mut helper = Help::new();
        let mut helped = Help::new();
        helper.request(helped_id).unwrap();
        helped.receive(helper_id, HELPER_NAME, HelpMessage::Request);
        let out = helped
            .answer_request(helper_id, true, helped_settings)
            .unwrap();
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

    /// The number the helped side gave the question `out` raised.
    fn question(out: &[Out]) -> u64 {
        match events(out).as_slice() {
            [HelpEvent::Proposed { id, .. }] => *id,
            other => panic!("expected one question, got {:?}", other),
        }
    }

    /// Proposes `change` from `helper` and delivers it to `helped`; returns
    /// the helper's id for it and the helped side's question number.
    fn propose(helper: &mut Help, helped: &mut Help, change: SettingChange) -> (u64, u64) {
        let helper_id = helped.helper().unwrap();
        let (id, out) = helper.propose(change).unwrap();
        let [(_, message)] = sent(&out).try_into().unwrap();
        (
            id,
            question(&helped.receive(helper_id, HELPER_NAME, message)),
        )
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

        let out = helped
            .answer_request(helper_id, true, settings(64))
            .unwrap();
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
            vec![notice(NoticeEvent::Started, helper_id, None)]
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
        assert_eq!(helped.helper(), Some(helper_id));
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

        let out = helped
            .answer_request(helper_id, false, settings(64))
            .unwrap();
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
        assert_eq!(helped.helper(), None);
    }

    /// The user answers the request they were shown. If that one was
    /// withdrawn and someone else asked in the meantime, the answer does not
    /// go to the newcomer.
    ///
    /// Verifies: REQ-RMT-001
    #[test]
    fn an_answer_to_one_participants_request_does_not_accept_someone_elses() {
        let aki = Uuid::new_v4();
        let cy = Uuid::new_v4();
        let mut helped = Help::new();
        helped.receive(aki, HELPER_NAME, HelpMessage::Request);
        helped.receive(aki, HELPER_NAME, HelpMessage::Stop { role: Role::Helper });
        helped.receive(cy, "Cy", HelpMessage::Request);

        assert_eq!(
            helped.answer_request(aki, true, settings(64)),
            Err(HelpError::NotActive)
        );
        assert_eq!(helped.helper(), None, "Cy was not let in");
    }

    /// The helped side sees what is proposed and nothing changes until it
    /// approves - the safeguard against the helper's mistakes.
    ///
    /// Verifies: REQ-RMT-002
    #[test]
    fn a_proposed_change_reaches_the_helped_side_as_a_question_and_is_not_applied_by_itself() {
        let (mut helper, mut helped, helper_id, helped_id) = active();

        let (_, out) = helper.propose(buffer(128)).unwrap();
        let [(to, message)] = sent(&out).try_into().unwrap();
        assert_eq!(to, helped_id);

        let arrived = helped.receive(Uuid::new_v4(), "Cy", message.clone());
        assert!(
            arrived.is_empty(),
            "a proposal from someone who is not helping is dropped"
        );
        let arrived = helped.receive(helper_id, HELPER_NAME, message);
        assert!(matches!(
            events(&arrived).as_slice(),
            [HelpEvent::Proposed { change, .. }] if *change == buffer(128)
        ));
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
        let (id, number) = propose(&mut helper, &mut helped, buffer(128));

        let taken = helped.take_proposal(number).unwrap();
        assert_eq!(taken.change, buffer(128));
        let out = helped.report(taken, Ok(at_revision(1, settings(128))));

        let [(to, answer)] = sent(&out).try_into().unwrap();
        assert_eq!(to, helper_id);
        assert_eq!(
            answer,
            HelpMessage::Answered {
                id,
                answer: Answer::Applied {
                    settings: Box::new(at_revision(1, settings(128)))
                }
            }
        );
        let [notice] = broadcast(&out).try_into().unwrap();
        assert_eq!(
            notice,
            HelpMessage::Notice {
                event: NoticeEvent::Changed,
                helper: helper_id,
                setting: Some("buffer_size".into()),
            }
        );
        assert!(matches!(
            events(&helper.receive(helped_id, HELPED_NAME, answer)).as_slice(),
            [HelpEvent::Answered { id: answered, answer: Answer::Applied { .. } }] if *answered == id
        ));

        // Every app in the room, helper and helped included, turns it into
        // a chat line; the helped side is the sender the server stamped.
        assert_eq!(
            events(&Help::new().receive(helped_id, HELPED_NAME, notice)),
            vec![HelpEvent::Notice {
                event: NoticeEvent::Changed,
                helper: helper_id,
                helped_name: HELPED_NAME.into(),
                setting: Some("buffer_size".into()),
            }]
        );
    }

    /// Verifies: REQ-RMT-002
    #[test]
    fn a_declined_change_is_answered_as_declined_and_the_room_hears_nothing() {
        let (mut helper, mut helped, _, helped_id) = active();
        let (id, number) = propose(&mut helper, &mut helped, buffer(128));

        let taken = helped.take_proposal(number).unwrap();
        let out = helped.decline(taken);

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
            helped.take_proposal(number),
            Err(HelpError::NoSuchProposal(number)),
            "a decided proposal cannot be applied later"
        );
    }

    /// The helper cannot choose what the helped side's approval applies: a
    /// proposal while another waits is dropped, and the approval names the
    /// question the helped side numbered, so reusing an id changes nothing.
    ///
    /// Verifies: REQ-RMT-002
    #[test]
    fn only_the_change_the_helped_side_was_asked_about_is_applied_whatever_ids_the_helper_sends() {
        let (_, mut helped, helper_id, _) = active();
        let propose = |helped: &mut Help, id: u64, change: SettingChange| {
            helped.receive(helper_id, HELPER_NAME, HelpMessage::Propose { id, change })
        };

        let first = question(&propose(&mut helped, 7, buffer(128)));
        assert!(
            propose(&mut helped, 7, buffer(32)).is_empty(),
            "a second proposal while one waits is dropped, not queued"
        );
        assert_eq!(helped.take_proposal(first).unwrap().change, buffer(128));

        let second = question(&propose(&mut helped, 7, buffer(256)));
        assert_ne!(first, second, "each question gets its own number");
        assert_eq!(
            helped.take_proposal(first),
            Err(HelpError::NoSuchProposal(first)),
            "an answer to the old question does not approve the new one"
        );
        assert_eq!(helped.take_proposal(second).unwrap().change, buffer(256));
    }

    /// Verifies: REQ-RMT-002
    #[test]
    fn the_helper_proposes_the_next_change_only_after_the_last_is_answered() {
        let (mut helper, mut helped, _, helped_id) = active();
        let (_, number) = propose(&mut helper, &mut helped, buffer(128));

        assert_eq!(helper.propose(buffer(32)), Err(HelpError::Waiting));

        let taken = helped.take_proposal(number).unwrap();
        let [(_, answer)] = sent(&helped.decline(taken)).try_into().unwrap();
        helper.receive(helped_id, HELPED_NAME, answer);
        assert!(helper.propose(buffer(32)).is_ok());
    }

    /// An approved change took effect even if the helper stopped while it was
    /// being applied, so the room still hears of it.
    ///
    /// Verifies: REQ-RMT-004
    #[test]
    fn a_change_applied_after_the_help_ended_is_still_announced_but_not_answered() {
        let (mut helper, mut helped, helper_id, _) = active();
        let (_, number) = propose(&mut helper, &mut helped, buffer(128));
        let taken = helped.take_proposal(number).unwrap();

        helped.receive(
            helper_id,
            HELPER_NAME,
            HelpMessage::Stop { role: Role::Helper },
        );
        let out = helped.report(taken, Ok(settings(128)));

        assert!(
            sent(&out).is_empty(),
            "no one is helping to hear the answer"
        );
        assert_eq!(
            broadcast(&out),
            vec![notice(
                NoticeEvent::Changed,
                helper_id,
                Some("buffer_size".into())
            )]
        );
    }

    /// Verifies: REQ-RMT-003
    #[test]
    fn when_the_helper_stops_the_helped_side_ends_too_and_later_proposals_go_nowhere() {
        let (mut helper, mut helped, helper_id, helped_id) = active();
        let (_, number) = propose(&mut helper, &mut helped, buffer(128));

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
        assert_eq!(
            helped.take_proposal(number),
            Err(HelpError::NotActive),
            "the proposal waiting when help ended is dropped"
        );
        assert!(helped
            .receive(
                helper_id,
                HELPER_NAME,
                HelpMessage::Propose {
                    id: 9,
                    change: buffer(64)
                }
            )
            .is_empty());
        assert_eq!(helper.propose(buffer(64)), Err(HelpError::NotActive));
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
        assert_eq!(helper.propose(buffer(64)), Err(HelpError::NotActive));
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
        let [(_, accepted)] = sent(&aki.answer_request(bo_id, true, settings(64)).unwrap())
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
        assert!(bo.propose(buffer(128)).is_ok(), "Bo still helps Aki");
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
        assert!(helper.propose(buffer(128)).is_ok());
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
            helped.answer_request(helper_id, true, settings(64)),
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

    /// An apply can outlast its help: stopped, asked again and accepted, the
    /// two are in a new help whose proposal ids start over. The old change's
    /// answer must not land on the new proposal.
    ///
    /// Verifies: REQ-RMT-002
    #[test]
    fn a_change_from_an_earlier_help_is_not_answered_into_a_later_one() {
        let (mut helper, mut helped, helper_id, helped_id) = active();
        let (_, number) = propose(&mut helper, &mut helped, buffer(128));
        let taken = helped.take_proposal(number).unwrap();

        let [(_, stop)] = sent(&helper.stop(Role::Helper).unwrap())
            .try_into()
            .unwrap();
        helped.receive(helper_id, HELPER_NAME, stop);
        let [(_, request)] = sent(&helper.request(helped_id).unwrap())
            .try_into()
            .unwrap();
        helped.receive(helper_id, HELPER_NAME, request);
        let [(_, accepted)] = sent(
            &helped
                .answer_request(helper_id, true, settings(128))
                .unwrap(),
        )
        .try_into()
        .unwrap();
        helper.receive(helped_id, HELPED_NAME, accepted);
        propose(&mut helper, &mut helped, buffer(32));

        let out = helped.report(taken, Ok(at_revision(1, settings(128))));

        assert!(
            sent(&out).is_empty(),
            "no answer into the new help: {:?}",
            out
        );
        assert_eq!(
            broadcast(&out).len(),
            1,
            "the room still hears of the change"
        );
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
        assert!(
            helper
                .receive(
                    helped_id,
                    HELPED_NAME,
                    HelpMessage::Answered {
                        id: id + 1,
                        answer: Answer::Declined
                    }
                )
                .is_empty(),
            "an answer to a proposal not made is dropped"
        );
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
    fn a_change_the_helped_side_makes_itself_reaches_the_helper_once() {
        let (_, mut helped, helper_id, _) = active();

        assert_eq!(
            sent(&helped.settings_changed(at_revision(1, settings(256)))),
            vec![(
                helper_id,
                HelpMessage::Settings {
                    settings: at_revision(1, settings(256))
                }
            )]
        );
        assert!(
            helped
                .settings_changed(at_revision(1, settings(256)))
                .is_empty(),
            "settings the helper has already been sent are not sent again"
        );
        assert!(Help::new().settings_changed(settings(256)).is_empty());
    }

    #[test]
    fn settings_an_approved_change_already_reported_are_not_sent_again() {
        let (mut helper, mut helped, _, _) = active();
        let (_, number) = propose(&mut helper, &mut helped, buffer(128));
        let taken = helped.take_proposal(number).unwrap();
        helped.report(taken, Ok(at_revision(1, settings(128))));

        assert!(helped
            .settings_changed(at_revision(1, settings(128)))
            .is_empty());
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

    /// A refusal says why in a word the helper's app translates, never the
    /// error's text (which can carry a device id or a path).
    ///
    /// Verifies: REQ-RMT-006
    #[test]
    fn a_refused_change_is_answered_with_a_reason_not_the_error_text() {
        let (mut helper, mut helped, _, _) = active();
        let (_, number) = propose(&mut helper, &mut helped, buffer(128));
        let taken = helped.take_proposal(number).unwrap();
        let error = SettingsError::DeviceNotOffered(format!("coreaudio:AG06:{SERIAL}:1,2"));

        let out = helped.report(taken, Err(Refusal::from(&error)));

        let json = serde_json::to_string(&sent(&out)[0].1).unwrap();
        assert!(!json.contains(SERIAL), "{}", json);
        assert_eq!(
            sent(&out)[0].1,
            HelpMessage::Answered {
                id: 1,
                answer: Answer::Refused {
                    reason: Refusal::DeviceGone
                }
            }
        );
        assert!(broadcast(&out).is_empty(), "nothing changed");
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

    const SERIAL: &str = "Y8XJ2KA0123456";

    /// Device ids can carry serial numbers; the helper sees the names and a
    /// handle per device, never the id - when help starts, in an answer, and
    /// when the helped side's settings change.
    ///
    /// Verifies: REQ-RMT-006
    #[test]
    fn the_helper_sees_device_names_but_never_device_ids() {
        let id = format!("coreaudio:AG06:{SERIAL}:1,2");
        let devices = with_devices(&[&id, "alsa:builtin"]);
        let helper_id = Uuid::new_v4();
        let mut helped = Help::new();
        helped.receive(helper_id, HELPER_NAME, HelpMessage::Request);

        let accepted = helped
            .answer_request(helper_id, true, devices.clone())
            .unwrap();
        let taken = {
            let proposal = helped.receive(
                helper_id,
                HELPER_NAME,
                HelpMessage::Propose {
                    id: 1,
                    change: buffer(128),
                },
            );
            helped.take_proposal(question(&proposal)).unwrap()
        };
        let applied = helped.report(taken, Ok(at_revision(1, devices.clone())));
        let changed = helped.settings_changed(at_revision(2, devices));

        for out in [&accepted, &applied, &changed] {
            let [(_, message)] = sent(out).try_into().unwrap();
            let json = serde_json::to_string(&message).unwrap();
            assert!(!json.contains(SERIAL), "{}", json);
            assert!(json.contains("Device 1"), "the names are shown: {}", json);
        }
        let [(_, HelpMessage::Accepted { settings })] =
            <[_; 1]>::try_from(sent(&accepted)).unwrap()
        else {
            panic!("not accepted");
        };
        assert_eq!(
            settings.input_device_id,
            settings.input_devices[0].id.clone().into(),
            "the selection points at the same handle as its device"
        );
    }

    /// Verifies: REQ-RMT-006
    #[test]
    fn a_proposed_device_is_asked_about_as_the_helped_sides_own_and_one_never_shown_is_dropped() {
        let helper_id = Uuid::new_v4();
        let mut helped = Help::new();
        helped.receive(helper_id, HELPER_NAME, HelpMessage::Request);
        let id = format!("coreaudio:AG06:{SERIAL}:1,2");
        let out = helped
            .answer_request(
                helper_id,
                true,
                AudioSettings {
                    output_devices: vec![],
                    output_device_id: None,
                    ..with_devices(&["alsa:builtin", &id])
                },
            )
            .unwrap();
        let [(_, HelpMessage::Accepted { settings })] = <[_; 1]>::try_from(sent(&out)).unwrap()
        else {
            panic!("not accepted");
        };
        let handle = settings.input_devices[1].id.clone();
        let propose = |helped: &mut Help, change: SettingChange| {
            helped.receive(
                helper_id,
                HELPER_NAME,
                HelpMessage::Propose { id: 1, change },
            )
        };

        for never_shown in [
            SettingChange::InputDevice {
                device_id: "input-99".to_string(),
            },
            // An input's handle does not name an output.
            SettingChange::OutputDevice {
                device_id: handle.clone(),
            },
        ] {
            assert!(
                propose(&mut helped, never_shown.clone()).is_empty(),
                "{:?}: not asked, not answered",
                never_shown
            );
        }
        let asked = propose(
            &mut helped,
            SettingChange::InputDevice { device_id: handle },
        );

        let real = SettingChange::InputDevice { device_id: id };
        assert_eq!(
            events(&asked),
            vec![HelpEvent::Proposed {
                id: 1,
                change: real.clone(),
                device_name: Some("Device 2".to_string()),
            }],
            "the user is asked about their own device, by the name the helper saw"
        );
        assert_eq!(helped.take_proposal(question(&asked)).unwrap().change, real);
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
        assert_eq!(
            serde_json::to_value(HelpMessage::Stop { role: Role::Helped }).unwrap(),
            serde_json::json!({ "kind": "stop", "role": "helped" })
        );
        assert!(
            serde_json::from_value::<PeerBody>(serde_json::json!({ "other_topic": {} })).is_err(),
            "a topic this app does not know does not parse"
        );
    }
}
