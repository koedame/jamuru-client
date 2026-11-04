//! Asking another participant to buffer one's audio for longer (ADR-097)
//!
//! A participant whose own uplink is the shaky one hears from the others as
//! gaps. The listener's buffer for them is the listener's to set, so they ask:
//! "hold the buffer you play my audio from to at least this long". The
//! listener's app asks its user (or, when the user said so, answers by itself),
//! applies it to that one buffer and says what it applied. The ask is not kept:
//! when the asker withdraws it, leaves, or the listener restores it, the buffer
//! goes back to what it was.
//!
//! [`BufferRequests`] is both sides as a state machine with no I/O, like
//! [`super::settings_help::Help`]: it takes what happened (a local action, a
//! message from a peer, the clock, a peer leaving) and says what to do about
//! it ([`Out`]) - messages to send, events for the UI, and changes to a buffer
//! for the caller to make. Time is passed in, so a test needs no waiting.
//!
//! The other app is not trusted to follow the protocol. An ask outside what
//! [`Terms::is_valid`] allows is dropped. A peer that sends over and over gets
//! one answer in [`REPLY_INTERVAL`], so peers cannot make this app exceed the
//! server's message limit and lose the room.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::audio::{BufferTerms, GrantView};

/// While no answer has come, the asker sends the ask again this often.
pub const RESEND_ASK_EVERY: Duration = Duration::from_secs(2);
/// An asker that has heard nothing for this long takes the other as unreachable.
pub const UNREACHABLE_AFTER: Duration = Duration::from_secs(20);
/// Once the ask is with the listener, the asker asks how it stands this often.
pub const CHECK_EVERY: Duration = Duration::from_secs(10);
/// The listener says it accepted or restored this many times in all, a second
/// apart, because one message that is lost leaves the asker not knowing.
pub const REPLY_REPEATS: u32 = 3;
/// How far apart those repeats are.
pub const REPEAT_REPLY_EVERY: Duration = Duration::from_secs(1);
/// What a peer sends unasked (an ask, a check) is answered at most this often.
pub const REPLY_INTERVAL: Duration = Duration::from_secs(1);

/// What the two apps say to each other over the room's signaling. Travels as
/// the body of a peer message, under [`super::settings_help::PeerBody::BufferRequest`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BufferMessage {
    /// Asker to listener: hold the buffer you play my audio from to this.
    Ask { terms: BufferTerms },
    /// Asker to listener: I no longer ask. Give the buffer back.
    Withdraw,
    /// Asker to listener: how does my ask stand?
    Check,
    /// Listener to asker: this is how it stands. `applied` is what the buffer
    /// was set to, once it is.
    Reply {
        answer: Answer,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        applied: Option<GrantView>,
    },
}

/// What the listener answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Answer {
    /// The ask is here and its user has not decided
    Pending,
    /// Accepted; the buffer is set
    Accepted,
    /// The user said no
    Declined,
    /// The settings say asks are not taken
    DeclinedBySetting,
    /// The buffer is back to what it was: the user put it back, or there is
    /// no ask of yours to hold
    Restored,
    /// The ask was withdrawn, as asked
    Withdrawn,
}

/// How the asker's ask stands, as the asker sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Standing {
    /// Sent, no answer yet
    Sending,
    /// The other has it and has not decided
    Pending,
    /// Accepted; the buffer is as `applied` says
    Granted { applied: GrantView },
    /// The other said no
    Declined,
    /// The other's settings do not take asks
    DeclinedBySetting,
    /// Accepted once, and the buffer is back to what it was
    Restored,
    /// No answer came for as long as one may take
    Unreachable,
}

impl Standing {
    /// Whether nothing more will come of it unless the asker asks again.
    pub fn is_over(&self) -> bool {
        matches!(
            self,
            Standing::Declined
                | Standing::DeclinedBySetting
                | Standing::Restored
                | Standing::Unreachable
        )
    }
}

/// Why a held ask was given up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Release {
    /// This app's user put the buffer back
    Restored,
    /// The asker withdrew
    Withdrawn,
    /// The asker asked for something else; the earlier ask ends until the user answers
    Replaced,
    /// The asker left the room
    PeerLeft,
    /// This app left the room or lost the connection
    Stopped,
}

/// Something the UI shows.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BufferEvent {
    /// `peer` asks this app to hold the buffer for them; answer with
    /// [`BufferRequests::accept`] or [`BufferRequests::decline`].
    Asked {
        peer: Uuid,
        peer_name: String,
        terms: BufferTerms,
    },
    /// The ask waiting for an answer is gone
    AskGone { peer: Uuid },
    /// This app holds the buffer for `peer`, as `applied` says
    Held {
        peer: Uuid,
        terms: BufferTerms,
        applied: GrantView,
    },
    /// This app no longer holds the buffer for `peer`
    Released { peer: Uuid, reason: Release },
    /// How this app's ask of `peer` stands; `None` once it is dropped
    Standing {
        peer: Uuid,
        standing: Option<Standing>,
    },
}

/// What to do about something that happened.
#[derive(Debug, Clone, PartialEq)]
pub enum Out {
    Send {
        to: Uuid,
        message: BufferMessage,
    },
    Event(BufferEvent),
    /// Set the receive buffer of `peer` to hold to `terms`, or back to what it
    /// was (`None`). Then say what it came to with [`BufferRequests::applied`].
    Apply {
        peer: Uuid,
        terms: Option<BufferTerms>,
    },
}

/// Whether this app takes asks, and whether it asks about each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Policy {
    /// The user is asked about each, unless they accepted one from the same
    /// participant already in this session
    #[default]
    Ask,
    /// No ask is taken
    Never,
}

/// Why a local action could not be taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BufferRequestError {
    /// The terms are outside what an ask may say
    InvalidTerms,
    /// There is no ask from that participant waiting, or held
    NotActive,
    /// The ask changed after the user was shown it
    Changed,
}

impl std::fmt::Display for BufferRequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BufferRequestError::InvalidTerms => f.write_str("The ask is outside what may be asked"),
            BufferRequestError::NotActive => f.write_str("No ask from that participant"),
            BufferRequestError::Changed => f.write_str("The ask changed after it was shown"),
        }
    }
}

impl std::error::Error for BufferRequestError {}

/// This app asking one participant.
#[derive(Debug, Clone)]
struct Asking {
    terms: BufferTerms,
    standing: Standing,
    /// When the other was last heard from (the ask was made, until it is)
    heard_at: Instant,
    /// When the last ask or check went out
    sent_at: Instant,
}

/// One participant's ask of this app.
#[derive(Debug, Clone)]
enum Heard {
    /// The user has not decided
    Prompting { name: String, terms: BufferTerms },
    /// The buffer is held
    Holding {
        terms: BufferTerms,
        /// What the buffer came to; `None` until the caller has said
        applied: Option<GrantView>,
        /// Messages saying so still to go, and when the next is due
        repeats: Option<(Answer, u32, Instant)>,
    },
}

/// The asks this app makes and the asks of it.
#[derive(Debug, Default)]
pub struct BufferRequests {
    policy: Policy,
    asking: HashMap<Uuid, Asking>,
    heard: HashMap<Uuid, Heard>,
    /// What this app's user accepted from each participant in this session: a
    /// later ask that does not go beyond it is taken without asking again
    accepted_before: HashMap<Uuid, BufferTerms>,
    /// When each participant was last answered unasked
    answered_at: HashMap<Uuid, Instant>,
    /// Participants the buffer was put back for, the messages saying so still to go and when
    /// the next is due
    repeat_restored: HashMap<Uuid, (u32, Instant)>,
}

impl BufferRequests {
    pub fn new(policy: Policy) -> Self {
        Self {
            policy,
            ..Self::default()
        }
    }

    /// Whether asks are taken.
    pub fn policy(&self) -> Policy {
        self.policy
    }

    /// Changes whether asks are taken. An ask waiting for an answer is declined
    /// when asks are no longer taken; one already held stays until the user
    /// puts it back.
    pub fn set_policy(&mut self, policy: Policy) -> Vec<Out> {
        self.policy = policy;
        if policy != Policy::Never {
            return Vec::new();
        }
        let waiting: Vec<Uuid> = self
            .heard
            .iter()
            .filter(|(_, heard)| matches!(heard, Heard::Prompting { .. }))
            .map(|(peer, _)| *peer)
            .collect();
        let mut out = Vec::new();
        for peer in waiting {
            self.heard.remove(&peer);
            out.push(Out::Event(BufferEvent::AskGone { peer }));
            out.push(Out::Send {
                to: peer,
                message: reply(Answer::DeclinedBySetting, None),
            });
        }
        out
    }

    // ----- the asker -----

    /// Asks `peer` to hold the buffer for this app's audio to `terms`. Asking
    /// again, with the same terms or others, replaces the ask.
    pub fn ask(
        &mut self,
        peer: Uuid,
        terms: BufferTerms,
        now: Instant,
    ) -> Result<Vec<Out>, BufferRequestError> {
        if !terms.is_valid() {
            return Err(BufferRequestError::InvalidTerms);
        }
        self.asking.insert(
            peer,
            Asking {
                terms,
                standing: Standing::Sending,
                heard_at: now,
                sent_at: now,
            },
        );
        Ok(vec![
            Out::Send {
                to: peer,
                message: BufferMessage::Ask { terms },
            },
            standing_event(peer, Some(Standing::Sending)),
        ])
    }

    /// Takes the ask of `peer` back, or drops what is shown of one that ended.
    pub fn withdraw(&mut self, peer: Uuid) -> Result<Vec<Out>, BufferRequestError> {
        let Some(asking) = self.asking.remove(&peer) else {
            return Err(BufferRequestError::NotActive);
        };
        let mut out = Vec::new();
        if !asking.standing.is_over() {
            out.push(Out::Send {
                to: peer,
                message: BufferMessage::Withdraw,
            });
        }
        out.push(standing_event(peer, None));
        Ok(out)
    }

    /// The asks this app made, with what was asked and how each stands.
    pub fn asked(&self) -> Vec<(Uuid, BufferTerms, Standing)> {
        self.asking
            .iter()
            .map(|(peer, asking)| (*peer, asking.terms, asking.standing))
            .collect()
    }

    /// How this app's ask of `peer` stands.
    pub fn standing(&self, peer: Uuid) -> Option<Standing> {
        self.asking.get(&peer).map(|asking| asking.standing)
    }

    /// What this app asked of `peer`.
    pub fn asked_terms(&self, peer: Uuid) -> Option<BufferTerms> {
        self.asking.get(&peer).map(|asking| asking.terms)
    }

    // ----- the listener -----

    /// The asks waiting for this app's user, with the name of who asks.
    pub fn prompts(&self) -> Vec<(Uuid, String, BufferTerms)> {
        self.heard
            .iter()
            .filter_map(|(peer, heard)| match heard {
                Heard::Prompting { name, terms } => Some((*peer, name.clone(), *terms)),
                Heard::Holding { .. } => None,
            })
            .collect()
    }

    /// The asks this app holds a buffer for.
    pub fn held(&self) -> Vec<(Uuid, BufferTerms)> {
        self.heard
            .iter()
            .filter_map(|(peer, heard)| match heard {
                Heard::Holding { terms, .. } => Some((*peer, *terms)),
                Heard::Prompting { .. } => None,
            })
            .collect()
    }

    /// The user accepts the ask of `peer`, which they saw as `shown`. An ask that
    /// changed after it was shown is not taken; the new one is up for an answer.
    pub fn accept(
        &mut self,
        peer: Uuid,
        shown: BufferTerms,
        now: Instant,
    ) -> Result<Vec<Out>, BufferRequestError> {
        let Some(Heard::Prompting { terms, .. }) = self.heard.get(&peer).cloned() else {
            return Err(BufferRequestError::NotActive);
        };
        if terms != shown {
            return Err(BufferRequestError::Changed);
        }
        self.accepted_before.insert(peer, terms);
        Ok(self.take(peer, terms, now))
    }

    /// The user turns down the ask of `peer`.
    pub fn decline(&mut self, peer: Uuid) -> Result<Vec<Out>, BufferRequestError> {
        if !matches!(self.heard.get(&peer), Some(Heard::Prompting { .. })) {
            return Err(BufferRequestError::NotActive);
        }
        self.heard.remove(&peer);
        Ok(vec![
            Out::Event(BufferEvent::AskGone { peer }),
            Out::Send {
                to: peer,
                message: reply(Answer::Declined, None),
            },
        ])
    }

    /// The user puts the buffer for `peer` back. Their next ask is asked about
    /// again, however many were accepted before.
    pub fn restore(&mut self, peer: Uuid, now: Instant) -> Result<Vec<Out>, BufferRequestError> {
        if !matches!(self.heard.get(&peer), Some(Heard::Holding { .. })) {
            return Err(BufferRequestError::NotActive);
        }
        self.accepted_before.remove(&peer);
        let mut out = self.let_go(peer, Release::Restored);
        out.push(Out::Send {
            to: peer,
            message: reply(Answer::Restored, None),
        });
        // One message lost must not leave the asker thinking the buffer is held
        self.answered_at.insert(peer, now);
        self.repeat_restored
            .insert(peer, (REPLY_REPEATS - 1, now + REPEAT_REPLY_EVERY));
        Ok(out)
    }

    /// The caller has set the buffer for `peer` as `Out::Apply` said and it came
    /// to `applied`. The first time, this is also what the asker is told.
    pub fn applied(&mut self, peer: Uuid, applied: GrantView, now: Instant) -> Vec<Out> {
        let Some(Heard::Holding {
            terms,
            applied: held,
            repeats,
        }) = self.heard.get_mut(&peer)
        else {
            return Vec::new();
        };
        let first = held.is_none();
        *held = Some(applied);
        let terms = *terms;
        let mut out = Vec::new();
        if first {
            *repeats = Some((
                Answer::Accepted,
                REPLY_REPEATS - 1,
                now + REPEAT_REPLY_EVERY,
            ));
            self.answered_at.insert(peer, now);
            out.push(Out::Send {
                to: peer,
                message: reply(Answer::Accepted, Some(applied)),
            });
        }
        out.push(Out::Event(BufferEvent::Held {
            peer,
            terms,
            applied,
        }));
        out
    }

    /// Records how the buffer for `peer` stands now, for the next check the
    /// asker makes. Says nothing to anyone.
    pub fn refresh(&mut self, peer: Uuid, applied: GrantView) {
        if let Some(Heard::Holding { applied: held, .. }) = self.heard.get_mut(&peer) {
            if held.is_some() {
                *held = Some(applied);
            }
        }
    }

    fn take(&mut self, peer: Uuid, terms: BufferTerms, _now: Instant) -> Vec<Out> {
        self.heard.insert(
            peer,
            Heard::Holding {
                terms,
                applied: None,
                repeats: None,
            },
        );
        vec![
            Out::Event(BufferEvent::AskGone { peer }),
            Out::Apply {
                peer,
                terms: Some(terms),
            },
        ]
    }

    fn let_go(&mut self, peer: Uuid, reason: Release) -> Vec<Out> {
        let was_held = matches!(self.heard.remove(&peer), Some(Heard::Holding { .. }));
        if !was_held {
            return Vec::new();
        }
        vec![
            Out::Apply { peer, terms: None },
            Out::Event(BufferEvent::Released { peer, reason }),
        ]
    }

    // ----- both -----

    /// A message arrived from `from` (stamped by the server, so it is who it
    /// says), whose room name is `from_name`.
    ///
    /// Anything that does not fit the current state - an answer to an ask never
    /// made - is dropped.
    pub fn receive(
        &mut self,
        from: Uuid,
        from_name: &str,
        message: BufferMessage,
        now: Instant,
    ) -> Vec<Out> {
        match message {
            BufferMessage::Ask { terms } => self.heard_ask(from, from_name, terms, now),
            BufferMessage::Withdraw => {
                let waiting = matches!(self.heard.get(&from), Some(Heard::Prompting { .. }));
                let mut out = if waiting {
                    self.heard.remove(&from);
                    vec![Out::Event(BufferEvent::AskGone { peer: from })]
                } else {
                    self.let_go(from, Release::Withdrawn)
                };
                if !out.is_empty() && self.may_answer(from, now) {
                    out.push(Out::Send {
                        to: from,
                        message: reply(Answer::Withdrawn, None),
                    });
                }
                out
            }
            BufferMessage::Check => {
                if !self.may_answer(from, now) {
                    return Vec::new();
                }
                let message = match self.heard.get(&from) {
                    Some(Heard::Prompting { .. }) => reply(Answer::Pending, None),
                    Some(Heard::Holding { applied, .. }) => reply(Answer::Accepted, *applied),
                    // There is none to hold; the asker finds out it is not held
                    None => reply(Answer::Restored, None),
                };
                vec![Out::Send { to: from, message }]
            }
            BufferMessage::Reply { answer, applied } => {
                self.heard_reply(from, answer, applied, now)
            }
        }
    }

    fn may_answer(&mut self, peer: Uuid, now: Instant) -> bool {
        self.answered_at
            .retain(|_, at| now.duration_since(*at) < REPLY_INTERVAL);
        if self.answered_at.contains_key(&peer) {
            return false;
        }
        self.answered_at.insert(peer, now);
        true
    }

    fn heard_ask(&mut self, from: Uuid, name: &str, terms: BufferTerms, now: Instant) -> Vec<Out> {
        if !terms.is_valid() {
            return Vec::new();
        }
        if self.policy == Policy::Never {
            if !self.may_answer(from, now) {
                return Vec::new();
            }
            return vec![Out::Send {
                to: from,
                message: reply(Answer::DeclinedBySetting, None),
            }];
        }
        match self.heard.get(&from) {
            Some(Heard::Prompting { terms: shown, .. }) if *shown == terms => {
                // The ask sent again while it waits
                if !self.may_answer(from, now) {
                    return Vec::new();
                }
                return vec![Out::Send {
                    to: from,
                    message: reply(Answer::Pending, None),
                }];
            }
            Some(Heard::Holding {
                terms: held,
                applied,
                ..
            }) if *held == terms => {
                // The ask sent again after it was accepted: the answer was lost
                let applied = *applied;
                if applied.is_none() || !self.may_answer(from, now) {
                    return Vec::new();
                }
                return vec![Out::Send {
                    to: from,
                    message: reply(Answer::Accepted, applied),
                }];
            }
            _ => {}
        }
        if self
            .accepted_before
            .get(&from)
            .is_some_and(|accepted| goes_no_further(terms, *accepted))
        {
            return self.take(from, terms, now);
        }
        // An ask accepted earlier ends here: the buffer goes back until this one is answered
        let mut out = self.let_go(from, Release::Replaced);
        self.heard.insert(
            from,
            Heard::Prompting {
                name: name.to_string(),
                terms,
            },
        );
        out.push(Out::Event(BufferEvent::Asked {
            peer: from,
            peer_name: name.to_string(),
            terms,
        }));
        if self.may_answer(from, now) {
            out.push(Out::Send {
                to: from,
                message: reply(Answer::Pending, None),
            });
        }
        out
    }

    fn heard_reply(
        &mut self,
        from: Uuid,
        answer: Answer,
        applied: Option<GrantView>,
        now: Instant,
    ) -> Vec<Out> {
        let Some(asking) = self.asking.get_mut(&from) else {
            return Vec::new();
        };
        asking.heard_at = now;
        let standing = match answer {
            Answer::Pending if asking.standing.is_over() => return Vec::new(),
            Answer::Pending => Standing::Pending,
            Answer::Accepted if asking.standing.is_over() => return Vec::new(),
            Answer::Accepted => Standing::Granted {
                applied: applied.unwrap_or_default(),
            },
            Answer::Declined => Standing::Declined,
            Answer::DeclinedBySetting => Standing::DeclinedBySetting,
            Answer::Restored => Standing::Restored,
            // What the asker asked for; the ask is already gone from here
            Answer::Withdrawn => return Vec::new(),
        };
        if asking.standing == standing {
            return Vec::new();
        }
        asking.standing = standing;
        vec![standing_event(from, Some(standing))]
    }

    /// Time has passed: asks go out again, how they stand is asked, the
    /// answers still to repeat are repeated, and an ask nobody answers ends.
    pub fn tick(&mut self, now: Instant) -> Vec<Out> {
        let mut out = Vec::new();
        for (peer, asking) in self.asking.iter_mut() {
            if asking.standing.is_over() {
                continue;
            }
            if now.duration_since(asking.heard_at) >= UNREACHABLE_AFTER {
                asking.standing = Standing::Unreachable;
                out.push(standing_event(*peer, Some(Standing::Unreachable)));
                continue;
            }
            let (every, message) = match asking.standing {
                Standing::Sending => (
                    RESEND_ASK_EVERY,
                    BufferMessage::Ask {
                        terms: asking.terms,
                    },
                ),
                _ => (CHECK_EVERY, BufferMessage::Check),
            };
            if now.duration_since(asking.sent_at) >= every {
                asking.sent_at = now;
                out.push(Out::Send { to: *peer, message });
            }
        }
        for (peer, heard) in self.heard.iter_mut() {
            if let Heard::Holding {
                applied, repeats, ..
            } = heard
            {
                if let Some((answer, left, due)) = repeats {
                    if now >= *due {
                        out.push(Out::Send {
                            to: *peer,
                            message: reply(*answer, *applied),
                        });
                        if *left <= 1 {
                            *repeats = None;
                        } else {
                            *left -= 1;
                            *due = now + REPEAT_REPLY_EVERY;
                        }
                    }
                }
            }
        }
        for (peer, (left, due)) in self.repeat_restored.clone() {
            if now >= due {
                out.push(Out::Send {
                    to: peer,
                    message: reply(Answer::Restored, None),
                });
                if left <= 1 {
                    self.repeat_restored.remove(&peer);
                } else {
                    self.repeat_restored
                        .insert(peer, (left - 1, now + REPEAT_REPLY_EVERY));
                }
            }
        }
        out
    }

    /// `peer` left the room: what was asked of it, and its ask of this app, are
    /// over. The room already hears that they left, so nothing is sent.
    pub fn peer_left(&mut self, peer: Uuid) -> Vec<Out> {
        self.answered_at.remove(&peer);
        self.accepted_before.remove(&peer);
        self.repeat_restored.remove(&peer);
        let mut out = Vec::new();
        if self.asking.remove(&peer).is_some() {
            out.push(standing_event(peer, None));
        }
        if matches!(self.heard.get(&peer), Some(Heard::Prompting { .. })) {
            self.heard.remove(&peer);
            out.push(Out::Event(BufferEvent::AskGone { peer }));
        } else {
            out.extend(self.let_go(peer, Release::PeerLeft));
        }
        out
    }

    /// This app left the room or lost the connection: every ask is over, and
    /// every buffer goes back.
    pub fn reset(&mut self) -> Vec<Out> {
        let mut out = Vec::new();
        let peers: Vec<Uuid> = self.asking.keys().copied().collect();
        for peer in peers {
            self.asking.remove(&peer);
            out.push(standing_event(peer, None));
        }
        let peers: Vec<Uuid> = self.heard.keys().copied().collect();
        for peer in peers {
            if matches!(self.heard.get(&peer), Some(Heard::Prompting { .. })) {
                self.heard.remove(&peer);
                out.push(Out::Event(BufferEvent::AskGone { peer }));
            } else {
                out.extend(self.let_go(peer, Release::Stopped));
            }
        }
        self.accepted_before.clear();
        self.answered_at.clear();
        self.repeat_restored.clear();
        out
    }
}

/// Whether an ask holds the buffer no longer and no harder than one the user
/// already accepted: not a higher cap, not a fixed buffer where the one accepted
/// followed the link, and not a stricter count of missing frames where both follow it.
fn goes_no_further(ask: BufferTerms, accepted: BufferTerms) -> bool {
    ask.max_ms10 <= accepted.max_ms10
        && match (ask.auto, accepted.auto) {
            (true, true) => ask.tolerance_tenths >= accepted.tolerance_tenths,
            (false, true) => false,
            (_, false) => true,
        }
}

fn reply(answer: Answer, applied: Option<GrantView>) -> BufferMessage {
    BufferMessage::Reply { answer, applied }
}

fn standing_event(peer: Uuid, standing: Option<Standing>) -> Out {
    Out::Event(BufferEvent::Standing { peer, standing })
}

#[cfg(test)]
mod tests {
    use super::super::settings_help::PeerBody;
    use super::*;

    fn terms() -> BufferTerms {
        BufferTerms {
            max_ms10: 300,
            tolerance_tenths: 10,
            auto: false,
        }
    }

    fn applied() -> GrantView {
        GrantView {
            applied_ms10: 293,
            rounded: true,
            at_lowest: false,
            auto_running: false,
        }
    }

    /// Two apps and the room between them: what one sends the other receives,
    /// and a buffer change is made at once and reported back.
    struct Room {
        asker: BufferRequests,
        listener: BufferRequests,
        asker_id: Uuid,
        listener_id: Uuid,
        /// What the listener's buffer for the asker was told, in order
        applies: Vec<Option<BufferTerms>>,
        /// What the UIs were shown, per app
        asker_events: Vec<BufferEvent>,
        listener_events: Vec<BufferEvent>,
        /// Messages the listener sent, by what they said
        sent_by_listener: Vec<BufferMessage>,
        sent_by_asker: Vec<BufferMessage>,
        /// Whether messages get through
        up: bool,
    }

    impl Room {
        fn new(policy: Policy) -> Self {
            Self {
                asker: BufferRequests::new(Policy::Ask),
                listener: BufferRequests::new(policy),
                asker_id: Uuid::new_v4(),
                listener_id: Uuid::new_v4(),
                applies: Vec::new(),
                asker_events: Vec::new(),
                listener_events: Vec::new(),
                sent_by_listener: Vec::new(),
                sent_by_asker: Vec::new(),
                up: true,
            }
        }

        /// Carries `outs` of the asker (`from_asker`) or the listener through the room
        fn run(&mut self, from_asker: bool, outs: Vec<Out>, now: Instant) {
            let mut queue = vec![(from_asker, outs)];
            while let Some((from_asker, outs)) = queue.pop() {
                for out in outs {
                    match out {
                        Out::Event(event) => {
                            if from_asker {
                                self.asker_events.push(event);
                            } else {
                                self.listener_events.push(event);
                            }
                        }
                        Out::Apply { peer, terms } => {
                            assert!(!from_asker, "only the listener's buffers are set");
                            assert_eq!(peer, self.asker_id);
                            self.applies.push(terms);
                            if terms.is_some() {
                                let more = self.listener.applied(peer, applied(), now);
                                queue.push((false, more));
                            }
                        }
                        Out::Send { message, .. } if from_asker => {
                            self.sent_by_asker.push(message.clone());
                            if self.up {
                                let name = "Asker";
                                let more = self.listener.receive(self.asker_id, name, message, now);
                                queue.push((false, more));
                            }
                        }
                        Out::Send { message, .. } => {
                            self.sent_by_listener.push(message.clone());
                            if self.up {
                                let more =
                                    self.asker
                                        .receive(self.listener_id, "Listener", message, now);
                                queue.push((true, more));
                            }
                        }
                    }
                }
            }
        }

        fn ask(&mut self, terms: BufferTerms, now: Instant) {
            let outs = self.asker.ask(self.listener_id, terms, now).unwrap();
            self.run(true, outs, now);
        }

        fn tick(&mut self, now: Instant) {
            let outs = self.asker.tick(now);
            self.run(true, outs, now);
            let outs = self.listener.tick(now);
            self.run(false, outs, now);
        }

        fn standing(&self) -> Option<Standing> {
            self.asker.standing(self.listener_id)
        }
    }

    fn seconds(base: Instant, s: u64) -> Instant {
        base + Duration::from_secs(s)
    }

    /// Verifies: REQ-CON-035
    #[test]
    fn when_an_ask_is_made_it_goes_out_once_and_shows_as_sending() {
        let (mut room, t0) = (Room::new(Policy::Ask), Instant::now());
        room.up = false;

        room.ask(terms(), t0);

        assert_eq!(room.sent_by_asker, [BufferMessage::Ask { terms: terms() }]);
        assert_eq!(room.standing(), Some(Standing::Sending));
    }

    /// Verifies: REQ-CON-035
    #[test]
    fn when_no_answer_comes_the_ask_goes_out_every_two_seconds_and_after_twenty_it_is_unreachable()
    {
        let (mut room, t0) = (Room::new(Policy::Ask), Instant::now());
        room.up = false;
        room.ask(terms(), t0);

        for s in 1..=19 {
            room.tick(seconds(t0, s));
        }
        let asks = room
            .sent_by_asker
            .iter()
            .filter(|m| matches!(m, BufferMessage::Ask { .. }))
            .count();
        assert_eq!(asks, 10, "the first, and one more every two seconds");
        assert_eq!(room.standing(), Some(Standing::Sending));

        room.tick(seconds(t0, 20));

        assert_eq!(room.standing(), Some(Standing::Unreachable));
        let before = room.sent_by_asker.len();
        room.tick(seconds(t0, 30));
        assert_eq!(
            room.sent_by_asker.len(),
            before,
            "nothing more is sent once it is over"
        );
    }

    /// Verifies: REQ-CON-035
    #[test]
    fn when_the_listener_has_the_ask_it_says_so_and_the_asker_stops_sending_it_again() {
        let (mut room, t0) = (Room::new(Policy::Ask), Instant::now());

        room.ask(terms(), t0);

        assert_eq!(room.standing(), Some(Standing::Pending));
        assert!(
            room.applies.is_empty(),
            "nothing is changed before the user decides"
        );
        assert!(matches!(
            room.listener_events[..],
            [BufferEvent::Asked { terms: shown, .. }] if shown == terms()
        ));
        for s in 1..=9 {
            room.tick(seconds(t0, s));
        }
        let asks = room
            .sent_by_asker
            .iter()
            .filter(|m| matches!(m, BufferMessage::Ask { .. }))
            .count();
        assert_eq!(asks, 1);
    }

    /// Verifies: REQ-CON-035
    #[test]
    fn when_the_user_accepts_the_buffer_is_set_and_the_asker_is_told_what_it_came_to() {
        let (mut room, t0) = (Room::new(Policy::Ask), Instant::now());
        room.ask(terms(), t0);

        let outs = room.listener.accept(room.asker_id, terms(), t0).unwrap();
        room.run(false, outs, t0);

        assert_eq!(room.applies, [Some(terms())]);
        assert_eq!(
            room.standing(),
            Some(Standing::Granted { applied: applied() })
        );
        assert_eq!(room.listener.held(), [(room.asker_id, terms())]);
    }

    /// Verifies: REQ-CON-035
    #[test]
    fn when_the_user_accepts_the_answer_is_sent_three_times_a_second_apart() {
        let (mut room, t0) = (Room::new(Policy::Ask), Instant::now());
        room.ask(terms(), t0);
        let outs = room.listener.accept(room.asker_id, terms(), t0).unwrap();
        room.run(false, outs, t0);
        let accepted = |room: &Room| {
            room.sent_by_listener
                .iter()
                .filter(|m| {
                    matches!(
                        m,
                        BufferMessage::Reply {
                            answer: Answer::Accepted,
                            ..
                        }
                    )
                })
                .count()
        };
        assert_eq!(accepted(&room), 1);

        room.tick(seconds(t0, 1));
        assert_eq!(accepted(&room), 2);
        room.tick(seconds(t0, 2));
        assert_eq!(accepted(&room), 3);
        room.tick(seconds(t0, 3));
        room.tick(seconds(t0, 4));
        assert_eq!(accepted(&room), 3, "and no more");
    }

    /// Verifies: REQ-CON-035
    #[test]
    fn when_the_accepted_answer_is_lost_the_asker_finds_out_from_its_check_within_ten_seconds() {
        let (mut room, t0) = (Room::new(Policy::Ask), Instant::now());
        room.ask(terms(), t0);
        room.up = false;
        let outs = room.listener.accept(room.asker_id, terms(), t0).unwrap();
        room.run(false, outs, t0);
        for s in 1..=4 {
            room.tick(seconds(t0, s));
        }
        assert_eq!(
            room.standing(),
            Some(Standing::Pending),
            "none of the answers got through"
        );

        room.up = true;
        for s in 5..=11 {
            room.tick(seconds(t0, s));
        }

        assert_eq!(
            room.standing(),
            Some(Standing::Granted { applied: applied() })
        );
    }

    /// Verifies: REQ-CON-035
    #[test]
    fn when_the_ask_is_granted_the_asker_checks_how_it_stands_every_ten_seconds() {
        let (mut room, t0) = (Room::new(Policy::Ask), Instant::now());
        room.ask(terms(), t0);
        let outs = room.listener.accept(room.asker_id, terms(), t0).unwrap();
        room.run(false, outs, t0);

        for s in 1..=25 {
            room.tick(seconds(t0, s));
        }

        let checks = room
            .sent_by_asker
            .iter()
            .filter(|m| **m == BufferMessage::Check)
            .count();
        assert_eq!(checks, 2, "at 10 and 20 seconds");
        assert_eq!(
            room.standing(),
            Some(Standing::Granted { applied: applied() })
        );
    }

    /// Verifies: REQ-CON-035
    #[test]
    fn when_the_listener_goes_quiet_for_twenty_seconds_after_granting_the_ask_is_unreachable() {
        let (mut room, t0) = (Room::new(Policy::Ask), Instant::now());
        room.ask(terms(), t0);
        let outs = room.listener.accept(room.asker_id, terms(), t0).unwrap();
        room.run(false, outs, t0);
        room.up = false;

        for s in 1..=25 {
            room.tick(seconds(t0, s));
        }

        assert_eq!(room.standing(), Some(Standing::Unreachable));
    }

    /// Verifies: REQ-CON-035
    #[test]
    fn when_the_user_declines_the_asker_is_told_and_nothing_is_changed() {
        let (mut room, t0) = (Room::new(Policy::Ask), Instant::now());
        room.ask(terms(), t0);

        let outs = room.listener.decline(room.asker_id).unwrap();
        room.run(false, outs, t0);

        assert_eq!(room.standing(), Some(Standing::Declined));
        assert!(room.applies.is_empty());
        assert!(room.listener.prompts().is_empty());
    }

    /// Verifies: REQ-CON-036
    #[test]
    fn when_the_settings_take_no_asks_the_asker_is_told_and_the_user_is_not_asked() {
        let (mut room, t0) = (Room::new(Policy::Never), Instant::now());

        room.ask(terms(), t0);

        assert_eq!(room.standing(), Some(Standing::DeclinedBySetting));
        assert!(room.listener_events.is_empty());
        assert!(room.applies.is_empty());
    }

    /// Verifies: REQ-CON-036
    #[test]
    fn when_the_settings_change_to_take_no_asks_the_ask_waiting_is_declined_and_one_held_stays() {
        let (mut room, t0) = (Room::new(Policy::Ask), Instant::now());
        room.ask(terms(), t0);
        let other = Uuid::new_v4();
        let outs = room
            .listener
            .receive(other, "Other", BufferMessage::Ask { terms: terms() }, t0);
        assert!(!outs.is_empty());
        let outs = room.listener.accept(room.asker_id, terms(), t0).unwrap();
        room.run(false, outs, t0);

        let outs = room.listener.set_policy(Policy::Never);

        assert_eq!(
            outs.iter()
                .filter(|o| matches!(o, Out::Send { message: BufferMessage::Reply { answer: Answer::DeclinedBySetting, .. }, to } if *to == other))
                .count(),
            1
        );
        assert!(room.listener.prompts().is_empty());
        assert_eq!(
            room.listener.held(),
            [(room.asker_id, terms())],
            "the held one stays"
        );
    }

    /// Verifies: REQ-CON-036
    #[test]
    fn when_a_participant_was_accepted_once_a_next_ask_that_goes_no_further_is_taken_without_asking_the_user(
    ) {
        let (mut room, t0) = (Room::new(Policy::Ask), Instant::now());
        room.ask(terms(), t0);
        let outs = room.listener.accept(room.asker_id, terms(), t0).unwrap();
        room.run(false, outs, t0);
        let shorter = BufferTerms {
            max_ms10: terms().max_ms10 - 50,
            ..terms()
        };

        room.ask(shorter, seconds(t0, 5));

        assert_eq!(room.applies, [Some(terms()), Some(shorter)]);
        assert_eq!(room.listener.held(), [(room.asker_id, shorter)]);
        assert!(
            !room
                .listener_events
                .iter()
                .skip(2)
                .any(|e| matches!(e, BufferEvent::Asked { .. })),
            "the user was asked once"
        );
    }

    /// Verifies: REQ-CON-036
    #[test]
    fn when_a_participant_accepted_once_asks_for_more_the_user_is_asked_again_and_the_earlier_buffer_ends_until_they_answer(
    ) {
        let following = BufferTerms {
            auto: true,
            ..terms()
        };
        let longer = BufferTerms {
            max_ms10: terms().max_ms10 + 50,
            ..following
        };
        let held_fixed = BufferTerms {
            auto: false,
            ..following
        };

        let stricter = BufferTerms {
            tolerance_tenths: following.tolerance_tenths - 5,
            ..following
        };

        for (accepted, ask) in [
            (terms(), longer),
            (following, held_fixed),
            (following, stricter),
        ] {
            let (mut room, t0) = (Room::new(Policy::Ask), Instant::now());
            room.ask(accepted, t0);
            let outs = room.listener.accept(room.asker_id, accepted, t0).unwrap();
            room.run(false, outs, t0);

            let outs = room.listener.receive(
                room.asker_id,
                "Asker",
                BufferMessage::Ask { terms: ask },
                seconds(t0, 5),
            );

            assert!(
                outs.iter()
                    .any(|o| matches!(o, Out::Event(BufferEvent::Asked { .. }))),
                "the user is asked about {ask:?} after {accepted:?}"
            );
            assert!(
                !outs
                    .iter()
                    .any(|o| matches!(o, Out::Apply { terms: Some(_), .. })),
                "nothing new is applied for {ask:?} before the user answers"
            );
            assert!(
                outs.iter()
                    .any(|o| matches!(o, Out::Apply { terms: None, .. })),
                "the buffer taken earlier is put back for {ask:?} while the user decides"
            );

            // Turning it down leaves nothing held that restore or leaving could not reach
            room.listener.decline(room.asker_id).unwrap();
            assert!(room.listener.held().is_empty());
        }
    }

    /// Verifies: REQ-CON-036
    #[test]
    fn when_the_ask_changed_after_the_user_was_shown_it_accepting_what_they_saw_is_refused() {
        let (mut room, t0) = (Room::new(Policy::Ask), Instant::now());
        room.ask(terms(), t0);
        let longer = BufferTerms {
            max_ms10: 500,
            ..terms()
        };
        room.ask(longer, seconds(t0, 1));

        let refused = room.listener.accept(room.asker_id, terms(), seconds(t0, 2));

        assert_eq!(refused, Err(BufferRequestError::Changed));
        assert_eq!(
            room.listener.prompts(),
            [(room.asker_id, "Asker".to_string(), longer)],
            "the new ask is still up for an answer"
        );
        assert!(room.listener.held().is_empty());
        assert!(!room.listener.accepted_before.contains_key(&room.asker_id));
    }

    /// Verifies: REQ-CON-035
    #[test]
    fn when_a_participant_asks_and_withdraws_in_turn_it_is_answered_at_most_once_a_second() {
        let (mut listener, t0) = (BufferRequests::default(), Instant::now());
        let asker = Uuid::new_v4();
        let mut answers = 0;

        for round in 0..6 {
            let at = t0 + Duration::from_millis(100 * round);
            for message in [
                BufferMessage::Ask { terms: terms() },
                BufferMessage::Withdraw,
            ] {
                answers += listener
                    .receive(asker, "Asker", message, at)
                    .iter()
                    .filter(|o| matches!(o, Out::Send { .. }))
                    .count();
            }
        }

        assert_eq!(answers, 1, "six asks and six withdrawals in 0.6 s");
    }

    /// Verifies: REQ-CON-036
    #[test]
    fn when_another_participant_asks_it_is_asked_about_even_if_someone_was_accepted_before() {
        let (mut room, t0) = (Room::new(Policy::Ask), Instant::now());
        room.ask(terms(), t0);
        let outs = room.listener.accept(room.asker_id, terms(), t0).unwrap();
        room.run(false, outs, t0);
        let other = Uuid::new_v4();

        let outs = room
            .listener
            .receive(other, "Other", BufferMessage::Ask { terms: terms() }, t0);

        assert!(outs
            .iter()
            .any(|o| matches!(o, Out::Event(BufferEvent::Asked { peer, .. }) if *peer == other)));
        assert!(!outs.iter().any(|o| matches!(o, Out::Apply { .. })));
    }

    /// Verifies: REQ-CON-036
    #[test]
    fn when_the_user_puts_the_buffer_back_the_asker_is_told_and_the_next_ask_is_asked_about() {
        let (mut room, t0) = (Room::new(Policy::Ask), Instant::now());
        room.ask(terms(), t0);
        let outs = room.listener.accept(room.asker_id, terms(), t0).unwrap();
        room.run(false, outs, t0);

        let outs = room
            .listener
            .restore(room.asker_id, seconds(t0, 3))
            .unwrap();
        room.run(false, outs, seconds(t0, 3));

        assert_eq!(room.applies, [Some(terms()), None]);
        assert_eq!(room.standing(), Some(Standing::Restored));
        assert!(room.listener.held().is_empty());
        room.ask(terms(), seconds(t0, 6));
        assert_eq!(
            room.standing(),
            Some(Standing::Pending),
            "the user is asked again"
        );
        assert_eq!(room.applies.len(), 2);
    }

    /// Verifies: REQ-CON-036
    #[test]
    fn when_the_buffer_is_put_back_the_asker_is_told_three_times_a_second_apart() {
        let (mut room, t0) = (Room::new(Policy::Ask), Instant::now());
        room.ask(terms(), t0);
        let outs = room.listener.accept(room.asker_id, terms(), t0).unwrap();
        room.run(false, outs, t0);
        let outs = room
            .listener
            .restore(room.asker_id, seconds(t0, 3))
            .unwrap();
        room.run(false, outs, seconds(t0, 3));
        let restored = |room: &Room| {
            room.sent_by_listener
                .iter()
                .filter(|m| {
                    matches!(
                        m,
                        BufferMessage::Reply {
                            answer: Answer::Restored,
                            ..
                        }
                    )
                })
                .count()
        };
        assert_eq!(restored(&room), 1);

        for s in 4..=8 {
            room.tick(seconds(t0, s));
        }

        assert_eq!(restored(&room), 3);
    }

    /// Verifies: REQ-CON-036
    #[test]
    fn when_the_asker_withdraws_the_buffer_goes_back_and_the_listener_is_told_why() {
        let (mut room, t0) = (Room::new(Policy::Ask), Instant::now());
        room.ask(terms(), t0);
        let outs = room.listener.accept(room.asker_id, terms(), t0).unwrap();
        room.run(false, outs, t0);

        let outs = room.asker.withdraw(room.listener_id).unwrap();
        room.run(true, outs, seconds(t0, 2));

        assert_eq!(room.applies, [Some(terms()), None]);
        assert_eq!(room.standing(), None);
        assert!(room.listener_events.contains(&BufferEvent::Released {
            peer: room.asker_id,
            reason: Release::Withdrawn
        }));
    }

    /// Verifies: REQ-CON-036
    #[test]
    fn when_the_asker_withdraws_before_the_user_decides_the_prompt_goes_away() {
        let (mut room, t0) = (Room::new(Policy::Ask), Instant::now());
        room.ask(terms(), t0);

        let outs = room.asker.withdraw(room.listener_id).unwrap();
        room.run(true, outs, t0);

        assert!(room.listener.prompts().is_empty());
        assert!(room.listener_events.contains(&BufferEvent::AskGone {
            peer: room.asker_id
        }));
        assert!(room.applies.is_empty());
    }

    /// Verifies: REQ-CON-036
    #[test]
    fn when_the_asker_leaves_the_room_the_buffer_goes_back_and_nothing_is_sent() {
        let (mut room, t0) = (Room::new(Policy::Ask), Instant::now());
        room.ask(terms(), t0);
        let outs = room.listener.accept(room.asker_id, terms(), t0).unwrap();
        room.run(false, outs, t0);
        let sent = room.sent_by_listener.len();

        let outs = room.listener.peer_left(room.asker_id);
        room.run(false, outs, seconds(t0, 1));

        assert_eq!(room.applies, [Some(terms()), None]);
        assert_eq!(room.sent_by_listener.len(), sent);
        assert!(room.listener_events.contains(&BufferEvent::Released {
            peer: room.asker_id,
            reason: Release::PeerLeft
        }));
        room.tick(seconds(t0, 2));
        assert_eq!(
            room.sent_by_listener.len(),
            sent,
            "and it does not go on repeating"
        );
    }

    /// Verifies: REQ-CON-036
    #[test]
    fn when_the_listener_leaves_the_room_the_asker_drops_its_ask() {
        let (mut room, t0) = (Room::new(Policy::Ask), Instant::now());
        room.ask(terms(), t0);

        let outs = room.asker.peer_left(room.listener_id);

        assert_eq!(room.standing(), None);
        assert_eq!(
            outs,
            [Out::Event(BufferEvent::Standing {
                peer: room.listener_id,
                standing: None
            })]
        );
    }

    /// Verifies: REQ-CON-036
    #[test]
    fn when_this_app_leaves_the_room_every_buffer_goes_back_and_nothing_is_remembered() {
        let (mut room, t0) = (Room::new(Policy::Ask), Instant::now());
        room.ask(terms(), t0);
        let outs = room.listener.accept(room.asker_id, terms(), t0).unwrap();
        room.run(false, outs, t0);

        let outs = room.listener.reset();
        room.run(false, outs, seconds(t0, 1));

        assert_eq!(room.applies, [Some(terms()), None]);
        room.ask(terms(), seconds(t0, 2));
        assert_eq!(
            room.standing(),
            Some(Standing::Pending),
            "the user is asked again in the next session"
        );
    }

    /// Verifies: REQ-CON-035
    #[test]
    fn when_the_ask_is_sent_again_after_it_was_accepted_the_answer_is_sent_again_and_nothing_is_set_twice(
    ) {
        let (mut room, t0) = (Room::new(Policy::Ask), Instant::now());
        room.ask(terms(), t0);
        let outs = room.listener.accept(room.asker_id, terms(), t0).unwrap();
        room.run(false, outs, t0);

        let outs = room.listener.receive(
            room.asker_id,
            "Asker",
            BufferMessage::Ask { terms: terms() },
            seconds(t0, 5),
        );

        assert_eq!(room.applies.len(), 1);
        assert_eq!(
            outs,
            [Out::Send {
                to: room.asker_id,
                message: reply(Answer::Accepted, Some(applied()))
            }]
        );
    }

    /// Verifies: REQ-CON-035
    #[test]
    fn when_the_ask_changes_while_the_user_has_not_decided_the_prompt_shows_the_new_one() {
        let (mut room, t0) = (Room::new(Policy::Ask), Instant::now());
        room.ask(terms(), t0);
        let longer = BufferTerms {
            max_ms10: 500,
            ..terms()
        };

        room.ask(longer, seconds(t0, 1));

        assert_eq!(
            room.listener.prompts(),
            [(room.asker_id, "Asker".to_string(), longer)]
        );
        let outs = room
            .listener
            .accept(room.asker_id, longer, seconds(t0, 2))
            .unwrap();
        room.run(false, outs, seconds(t0, 2));
        assert_eq!(room.applies, [Some(longer)]);
    }

    /// Verifies: REQ-CON-035
    #[test]
    fn when_an_ask_is_outside_what_may_be_asked_it_is_not_made_and_one_that_arrives_is_dropped() {
        let (mut room, t0) = (Room::new(Policy::Ask), Instant::now());
        let long = BufferTerms {
            max_ms10: 851,
            ..terms()
        };
        let none = BufferTerms {
            max_ms10: 0,
            ..terms()
        };
        let jumpy = BufferTerms {
            tolerance_tenths: 101,
            ..terms()
        };

        assert_eq!(
            room.asker.ask(room.listener_id, long, t0),
            Err(BufferRequestError::InvalidTerms)
        );
        assert_eq!(
            room.asker.ask(room.listener_id, none, t0),
            Err(BufferRequestError::InvalidTerms)
        );
        for bad in [long, none, jumpy] {
            let outs = room.listener.receive(
                room.asker_id,
                "Asker",
                BufferMessage::Ask { terms: bad },
                t0,
            );
            assert_eq!(outs, []);
        }
        assert!(room.listener.prompts().is_empty());
    }

    /// Verifies: REQ-CON-035
    #[test]
    fn when_a_peer_sends_asks_without_stopping_it_is_answered_once_a_second() {
        let (mut room, t0) = (Room::new(Policy::Ask), Instant::now());
        let mut answers = 0;

        for i in 0..50 {
            let at = t0 + Duration::from_millis(i * 20);
            let outs = room.listener.receive(
                room.asker_id,
                "Asker",
                BufferMessage::Ask { terms: terms() },
                at,
            );
            answers += outs
                .iter()
                .filter(|o| matches!(o, Out::Send { .. }))
                .count();
            let outs = room
                .listener
                .receive(room.asker_id, "Asker", BufferMessage::Check, at);
            answers += outs
                .iter()
                .filter(|o| matches!(o, Out::Send { .. }))
                .count();
        }

        assert_eq!(answers, 1, "in one second");
    }

    /// Verifies: REQ-CON-035
    #[test]
    fn when_a_check_comes_for_an_ask_this_app_does_not_hold_it_says_the_buffer_is_not_held() {
        let (mut room, t0) = (Room::new(Policy::Ask), Instant::now());
        room.ask(terms(), t0);
        let outs = room.listener.accept(room.asker_id, terms(), t0).unwrap();
        room.run(false, outs, t0);
        // The listener's app was started again: it knows nothing of the ask
        room.listener = BufferRequests::new(Policy::Ask);

        for s in 1..=10 {
            room.tick(seconds(t0, s));
        }

        assert_eq!(room.standing(), Some(Standing::Restored));
    }

    /// Verifies: REQ-CON-035
    #[test]
    fn when_an_answer_comes_for_an_ask_never_made_it_is_dropped() {
        let (mut room, t0) = (Room::new(Policy::Ask), Instant::now());

        let outs = room.asker.receive(
            room.listener_id,
            "Listener",
            reply(Answer::Accepted, Some(applied())),
            t0,
        );

        assert_eq!(outs, []);
        assert_eq!(room.standing(), None);
    }

    /// Verifies: REQ-CON-035
    #[test]
    fn when_a_late_pending_answer_arrives_after_the_ask_was_declined_the_standing_stays() {
        let (mut room, t0) = (Room::new(Policy::Ask), Instant::now());
        room.ask(terms(), t0);
        let outs = room.listener.decline(room.asker_id).unwrap();
        room.run(false, outs, t0);

        let outs = room.asker.receive(
            room.listener_id,
            "Listener",
            reply(Answer::Pending, None),
            t0,
        );

        assert_eq!(outs, []);
        assert_eq!(room.standing(), Some(Standing::Declined));
    }

    /// Verifies: REQ-CON-035
    #[test]
    fn when_the_buffer_changes_after_it_was_granted_the_next_check_tells_the_asker() {
        let (mut room, t0) = (Room::new(Policy::Ask), Instant::now());
        room.ask(terms(), t0);
        let outs = room.listener.accept(room.asker_id, terms(), t0).unwrap();
        room.run(false, outs, t0);
        let auto = GrantView {
            applied_ms10: 400,
            auto_running: true,
            ..applied()
        };

        room.listener.refresh(room.asker_id, auto);
        for s in 1..=10 {
            room.tick(seconds(t0, s));
        }

        assert_eq!(room.standing(), Some(Standing::Granted { applied: auto }));
    }

    /// Verifies: REQ-CON-035
    #[test]
    fn when_the_ask_travels_it_is_a_peer_message_under_its_own_topic() {
        let body = serde_json::to_value(PeerBody::BufferRequest(BufferMessage::Ask {
            terms: terms(),
        }))
        .unwrap();

        assert_eq!(
            body,
            serde_json::json!({"buffer_request": {"kind": "ask", "terms": {"max_ms10": 300, "tolerance_tenths": 10, "auto": false}}})
        );
        let parsed: PeerBody = serde_json::from_value(body).unwrap();
        assert_eq!(
            parsed,
            PeerBody::BufferRequest(BufferMessage::Ask { terms: terms() })
        );
    }

    /// Verifies: REQ-CON-035
    #[test]
    fn when_the_answer_travels_it_says_what_the_buffer_came_to() {
        let body = serde_json::to_value(PeerBody::BufferRequest(reply(
            Answer::Accepted,
            Some(applied()),
        )))
        .unwrap();

        assert_eq!(
            body,
            serde_json::json!({"buffer_request": {"kind": "reply", "answer": "accepted",
                "applied": {"applied_ms10": 293, "rounded": true, "at_lowest": false, "auto_running": false}}})
        );
        let parsed: PeerBody = serde_json::from_value(body).unwrap();
        assert_eq!(
            parsed,
            PeerBody::BufferRequest(reply(Answer::Accepted, Some(applied())))
        );
    }
}
