//! Which audio links have gone quiet, and on whose side of the line (ADR-112).
//!
//! A participant's audio that stops arriving, with no word that they left, is one of three
//! things: this app's own line broke, theirs did, or the path between the two did. The first is
//! for this person to fix, the second is for waiting out, so the screen says which.
//!
//! [`Dropouts`] decides; it does no I/O, like [`super::roster::Roster`]. It is told what each link
//! last heard, how this app stands with the signaling server, and what the others and the server
//! said, and answers with who is quiet, since when, on whose side, and what to send the room. Time
//! is passed in, so a test needs no waiting.
//!
//! The evidence, in the order it is used:
//!
//! 1. This app's own line: the machine is offline, the signaling connection is down, or the
//!    server does not answer. Or every link with audio went quiet at about the same time.
//! 2. What the other participants hear of them ([`LinkView`]). Each says how long ago it last heard
//!    each of them; a participant who heard them after this app stopped hearing them shows their
//!    line is up, and one who stopped at the same time shows it is down.
//! 3. What the server knows ([`Presence`]): the room still lists them, and how long ago it last
//!    heard from their connection. The only witness in a room of two.
//!
//! Every fact is compared with when this app itself stopped hearing the participant, so an answer
//! that arrives late or was made before the audio stopped cannot point the wrong way.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A link that has received nothing for this long is quiet. The links send a keep-alive every
/// second, muted or not, so a link that is merely silent is not quiet.
pub const DROPOUT_AFTER: Duration = Duration::from_millis(1500);

/// Links that all went quiet within this of each other went quiet together.
pub const SIMULTANEOUS_WITHIN: Duration = Duration::from_secs(2);

/// How far back dropouts are counted to call a link unstable.
pub const UNSTABLE_WINDOW: Duration = Duration::from_secs(15 * 60);

/// A link that went quiet this many times within [`UNSTABLE_WINDOW`] is unstable.
pub const UNSTABLE_DROPOUTS: usize = 2;

/// While anyone is quiet, this app tells the room what it hears this often.
pub const VIEW_EVERY: Duration = Duration::from_secs(2);

/// This app tells the room what it hears no more often than this, whoever asks.
pub const VIEW_MIN_GAP: Duration = Duration::from_secs(1);

/// While a side is not known and nobody else can tell it, the server is asked this often.
pub const PRESENCE_EVERY: Duration = Duration::from_secs(2);

/// A server that has not answered in this long is out of reach.
pub const PRESENCE_UNANSWERED_AFTER: Duration = Duration::from_secs(3);

/// How often an app pings the signaling server while in a room. The server keeps when it last
/// heard from each connection, which is what [`Presence`] reports.
pub const SERVER_PING_EVERY: Duration = Duration::from_secs(4);

/// How late a ping may be before the participant's line is taken as dead. Together with
/// [`SERVER_PING_EVERY`] it stays under the time an audio link waits before it gives up and the
/// room is entered again (10 s), so a room of two names the broken side before the link is
/// started over.
pub const SERVER_PING_SLACK: Duration = Duration::from_secs(2);

/// A signaling connection that has heard nothing from the server for this long is dead: the
/// server answers a ping at once, so this is several pings long.
pub const CONNECTION_DEAD_AFTER: Duration = Duration::from_secs(15);

/// A participant heard from this long after this app stopped hearing them has a line that is up.
const HEARD_AFTER: Duration = Duration::from_secs(1);

/// After this long quiet with nothing to tell the side by, the side is given as unknown.
const GIVE_UP_AFTER: Duration = Duration::from_secs(40);

/// At most this many participants are in a view: a room is full at ten.
const MAX_VIEW_LINKS: usize = 16;

/// Whose line it is that broke.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    /// This app's own line.
    Mine,
    /// The participant's own line.
    Theirs,
    /// Neither end's line: the path between the two.
    Between,
    /// Not known yet; the others and the server are being asked.
    Checking,
    /// Not known, and nobody could tell.
    Unknown,
}

/// A link that is quiet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dropout {
    /// How long nothing has arrived.
    pub silent_for: Duration,
    pub side: Side,
}

/// How one participant's link stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Standing {
    pub dropout: Option<Dropout>,
    /// Quiet [`UNSTABLE_DROPOUTS`] times or more within [`UNSTABLE_WINDOW`]
    pub unstable: bool,
}

/// When a link last received anything.
#[derive(Debug, Clone, Copy)]
pub struct Heard {
    pub peer: Uuid,
    /// `None` while nothing has arrived: the link is being set up, which is not a dropout.
    pub at: Option<Instant>,
}

/// How this app's own signaling connection stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line {
    Up,
    /// Closed, or not answering its pings.
    Down,
}

/// What the operating system says of this machine's network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Network {
    Online,
    /// No route out.
    Offline,
    /// Not found out on this system.
    Unknown,
}

/// What an app tells the room of its links: how long ago each last received anything.
///
/// Travels as the body of a peer message, under
/// [`super::settings_help::PeerBody::LinkView`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkView {
    pub links: Vec<LinkAge>,
    /// Someone is quiet here and asks the others to say what they hear.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ask: bool,
}

/// One participant's link in a [`LinkView`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkAge {
    pub peer: Uuid,
    pub ago_ms: u64,
}

/// What the server answered of the room: for each participant listed, how long ago it last
/// heard from their connection, or `None` when their connection never pinged it (an app that does
/// not, or one that has only just come in). A participant who is not listed is not in the room.
pub type Presence = Vec<(Uuid, Option<Duration>)>;

/// What to do after a [`Dropouts::tick`].
#[derive(Debug, Clone, PartialEq)]
pub struct Tick {
    pub standing: HashMap<Uuid, Standing>,
    /// Tell the room this.
    pub share: Option<LinkView>,
    /// Ask the server who is in the room and how long ago it last heard from each.
    pub ask_presence: bool,
}

/// What the server said, and when it was received.
#[derive(Debug)]
struct Said {
    at: Instant,
    idle: HashMap<Uuid, Option<Duration>>,
}

/// What a participant said they hear, and when it was received.
#[derive(Debug)]
struct Told {
    at: Instant,
    ago: HashMap<Uuid, Duration>,
}

/// Which links are quiet and whose line it is. See the module documentation.
#[derive(Debug, Default)]
pub struct Dropouts {
    /// The links that are quiet now, with when each last received anything.
    quiet: HashMap<Uuid, Instant>,
    /// When each link went quiet, within [`UNSTABLE_WINDOW`].
    history: HashMap<Uuid, VecDeque<Instant>>,
    told: HashMap<Uuid, Told>,
    said: Option<Said>,
    /// When the oldest ask the server has not answered was made; cleared when everything is
    /// heard again.
    asked_at: Option<Instant>,
    /// When the server was last asked.
    last_ask: Option<Instant>,
    shared: Option<(Instant, Vec<Uuid>)>,
    /// Someone asked what this app hears.
    owes_a_view: bool,
}

impl Dropouts {
    pub fn new() -> Self {
        Self::default()
    }

    /// A participant told what they hear.
    pub fn view_received(&mut self, now: Instant, from: Uuid, view: LinkView) {
        let ago = view
            .links
            .iter()
            .take(MAX_VIEW_LINKS)
            .map(|link| (link.peer, Duration::from_millis(link.ago_ms)))
            .collect();
        self.told.insert(from, Told { at: now, ago });
        if view.ask {
            self.owes_a_view = true;
        }
    }

    /// The server answered who is in the room.
    pub fn presence_received(&mut self, now: Instant, presence: Presence) {
        self.said = Some(Said {
            at: now,
            idle: presence.into_iter().collect(),
        });
    }

    /// Takes in what the links hear now and says who is quiet, what to tell the room, and whether
    /// to ask the server.
    pub fn tick(&mut self, now: Instant, links: &[Heard], line: Line, network: Network) -> Tick {
        let known: Vec<Uuid> = links.iter().map(|link| link.peer).collect();
        self.quiet.retain(|peer, _| known.contains(peer));
        self.history.retain(|peer, _| known.contains(peer));
        self.told.retain(|peer, _| known.contains(peer));

        let heard: Vec<(Uuid, Instant)> = links
            .iter()
            .filter_map(|link| link.at.map(|at| (link.peer, at)))
            .collect();
        let quiet_now: Vec<(Uuid, Instant)> = heard
            .iter()
            .copied()
            .filter(|(_, at)| now.saturating_duration_since(*at) >= DROPOUT_AFTER)
            .collect();

        self.quiet
            .retain(|peer, _| quiet_now.iter().any(|(quiet, _)| quiet == peer));
        for &(peer, at) in &quiet_now {
            if self.quiet.insert(peer, at).is_none() {
                self.history.entry(peer).or_default().push_back(now);
            }
        }
        for dropouts in self.history.values_mut() {
            while dropouts
                .front()
                .is_some_and(|at| now.saturating_duration_since(*at) > UNSTABLE_WINDOW)
            {
                dropouts.pop_front();
            }
        }
        if quiet_now.is_empty() {
            self.asked_at = None;
            self.last_ask = None;
            self.said = None;
        }

        let server_out_of_reach = self.server_out_of_reach(now);
        let own_line_down =
            network == Network::Offline || line == Line::Down || server_out_of_reach;
        let everyone_quiet_together = heard.len() >= 2
            && quiet_now.len() == heard.len()
            && span(quiet_now.iter().map(|(_, at)| *at)) <= SIMULTANEOUS_WITHIN;

        let mut ask_presence = false;
        let mut standing = HashMap::new();
        for link in links {
            let dropouts = self.history.get(&link.peer).map_or(0, VecDeque::len);
            let dropout = self.quiet.get(&link.peer).map(|&last| {
                let side = if own_line_down || everyone_quiet_together {
                    Side::Mine
                } else {
                    let (side, wants_presence) = self.side_of(now, link.peer, last);
                    ask_presence |= wants_presence;
                    side
                };
                Dropout {
                    silent_for: now.saturating_duration_since(last),
                    side,
                }
            });
            standing.insert(
                link.peer,
                Standing {
                    dropout,
                    unstable: dropouts >= UNSTABLE_DROPOUTS,
                },
            );
        }

        // A server that does not answer is asked again, so that it is noticed when it does
        let ask_presence = (ask_presence || server_out_of_reach)
            && self
                .last_ask
                .is_none_or(|at| now.saturating_duration_since(at) >= PRESENCE_EVERY);
        if ask_presence {
            self.last_ask = Some(now);
            let outstanding = self
                .asked_at
                .is_some_and(|asked| !self.said.as_ref().is_some_and(|said| said.at >= asked));
            if !outstanding {
                self.asked_at = Some(now);
            }
        }

        let share = self.view_to_share(now, &heard, &quiet_now);
        Tick {
            standing,
            share,
            ask_presence,
        }
    }

    /// The server was asked and has not answered for [`PRESENCE_UNANSWERED_AFTER`].
    fn server_out_of_reach(&self, now: Instant) -> bool {
        let Some(asked) = self.asked_at else {
            return false;
        };
        let answered = self.said.as_ref().is_some_and(|said| said.at >= asked);
        !answered && now.saturating_duration_since(asked) >= PRESENCE_UNANSWERED_AFTER
    }

    /// Whose line it is that `peer` stopped being heard on, from what the others and the server
    /// said; and whether the server is worth asking. `last` is when this app last heard them.
    fn side_of(&self, now: Instant, peer: Uuid, last: Instant) -> (Side, bool) {
        let (mut alive, mut dead) = (0, 0);
        for (witness, told) in &self.told {
            if *witness == peer {
                continue;
            }
            let Some(ago) = told.ago.get(&peer) else {
                continue;
            };
            match seen(told.at, *ago, last, DROPOUT_AFTER) {
                Seen::Heard => alive += 1,
                Seen::Quiet => dead += 1,
                Seen::TooEarly => {}
            }
        }
        if alive + dead > 0 {
            return (
                if dead >= alive {
                    Side::Theirs
                } else {
                    Side::Between
                },
                false,
            );
        }

        let from_server = self.said.as_ref().map(|said| match said.idle.get(&peer) {
            None if said.at >= last => Seen::Quiet,
            None => Seen::TooEarly,
            Some(None) => Seen::TooEarly,
            Some(Some(idle)) => seen(said.at, *idle, last, SERVER_PING_EVERY + SERVER_PING_SLACK),
        });
        match from_server {
            Some(Seen::Heard) => (Side::Between, false),
            Some(Seen::Quiet) => (Side::Theirs, false),
            _ if now.saturating_duration_since(last) >= GIVE_UP_AFTER => (Side::Unknown, false),
            _ => (Side::Checking, true),
        }
    }

    /// What to tell the room of the links, if it is time.
    fn view_to_share(
        &mut self,
        now: Instant,
        heard: &[(Uuid, Instant)],
        quiet_now: &[(Uuid, Instant)],
    ) -> Option<LinkView> {
        let mut quiet_peers: Vec<Uuid> = quiet_now.iter().map(|(peer, _)| *peer).collect();
        quiet_peers.sort();
        let (since, told) = match &self.shared {
            Some((at, told)) => (Some(*at), told.as_slice()),
            None => (None, [].as_slice()),
        };
        let too_soon = since.is_some_and(|at| now.saturating_duration_since(at) < VIEW_MIN_GAP);
        let changed = quiet_peers != told;
        let still_quiet = !quiet_peers.is_empty()
            && since.is_none_or(|at| now.saturating_duration_since(at) >= VIEW_EVERY);
        if heard.is_empty() || too_soon || !(changed || still_quiet || self.owes_a_view) {
            return None;
        }
        self.owes_a_view = false;
        self.shared = Some((now, quiet_peers.clone()));
        Some(LinkView {
            links: heard
                .iter()
                .take(MAX_VIEW_LINKS)
                .map(|(peer, at)| LinkAge {
                    peer: *peer,
                    ago_ms: now.saturating_duration_since(*at).as_millis() as u64,
                })
                .collect(),
            ask: !quiet_peers.is_empty(),
        })
    }
}

/// What a witness's word comes to.
enum Seen {
    /// They heard the participant after this app stopped: the participant's line is up.
    Heard,
    /// They stopped hearing the participant when this app did, and it was told long enough after
    /// for the participant to have been heard if they were there.
    Quiet,
    /// Said before it could show either.
    TooEarly,
}

/// What a witness's word, taken at `at` and saying the participant was last heard `ago` before
/// that, shows of a participant this app last heard at `last`. `window` is how long after `last`
/// the word has to be taken to show a quiet line.
fn seen(at: Instant, ago: Duration, last: Instant, window: Duration) -> Seen {
    if at
        .checked_sub(ago)
        .is_some_and(|heard| heard > last + HEARD_AFTER)
    {
        Seen::Heard
    } else if at >= last + window {
        Seen::Quiet
    } else {
        Seen::TooEarly
    }
}

/// The time between the earliest and the latest of `instants`.
fn span(instants: impl Iterator<Item = Instant>) -> Duration {
    let (mut earliest, mut latest): (Option<Instant>, Option<Instant>) = (None, None);
    for at in instants {
        earliest = Some(earliest.map_or(at, |e| e.min(at)));
        latest = Some(latest.map_or(at, |l| l.max(at)));
    }
    match (earliest, latest) {
        (Some(earliest), Some(latest)) => latest.duration_since(earliest),
        _ => Duration::ZERO,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// A room as one app in it sees it: `start` is the instant everything was last heard.
    struct Room {
        start: Instant,
        dropouts: Dropouts,
        /// When each link last received anything, as milliseconds after `start`
        heard: Vec<(Uuid, Option<u64>)>,
        line: Line,
        network: Network,
    }

    impl Room {
        fn of(peers: &[u128]) -> Self {
            Self {
                start: Instant::now(),
                dropouts: Dropouts::new(),
                heard: peers.iter().map(|p| (id(*p), Some(0))).collect(),
                line: Line::Up,
                network: Network::Online,
            }
        }

        fn at(&self, millis: u64) -> Instant {
            self.start + ms(millis)
        }

        fn hear(&mut self, peer: u128, at: u64) {
            for link in &mut self.heard {
                if link.0 == id(peer) {
                    link.1 = Some(at);
                }
            }
        }

        fn tick(&mut self, now: u64) -> Tick {
            let links: Vec<Heard> = self
                .heard
                .iter()
                .map(|(peer, at)| Heard {
                    peer: *peer,
                    at: at.map(|at| self.start + ms(at)),
                })
                .collect();
            self.dropouts
                .tick(self.start + ms(now), &links, self.line, self.network)
        }

        fn side(&mut self, peer: u128, now: u64) -> Option<Side> {
            self.tick(now).standing[&id(peer)].dropout.map(|d| d.side)
        }

        fn told(&mut self, from: u128, at: u64, hears: &[(u128, u64)]) {
            let view = LinkView {
                links: hears
                    .iter()
                    .map(|(peer, ago)| LinkAge {
                        peer: id(*peer),
                        ago_ms: *ago,
                    })
                    .collect(),
                ask: false,
            };
            self.dropouts.view_received(self.at(at), id(from), view);
        }

        fn presence(&mut self, at: u64, listed: &[(u128, u64)]) {
            self.dropouts.presence_received(
                self.at(at),
                listed
                    .iter()
                    .map(|(p, idle)| (id(*p), Some(ms(*idle))))
                    .collect(),
            );
        }
    }

    /// Verifies: REQ-CON-162
    #[test]
    fn when_a_link_has_received_nothing_for_less_than_one_and_a_half_seconds_it_is_not_quiet() {
        let mut room = Room::of(&[2]);
        assert_eq!(room.side(2, 1499), None);
    }

    /// Verifies: REQ-CON-162
    #[test]
    fn when_a_link_has_received_nothing_for_one_and_a_half_seconds_it_is_quiet_for_that_long() {
        let mut room = Room::of(&[2]);
        let quiet = room.tick(4000).standing[&id(2)].dropout.unwrap();
        assert_eq!(quiet.silent_for, ms(4000));
    }

    /// Verifies: REQ-CON-162
    #[test]
    fn when_a_link_never_received_anything_it_is_being_set_up_and_not_quiet() {
        let mut room = Room::of(&[2]);
        room.heard[0].1 = None;
        assert_eq!(room.side(2, 60_000), None);
    }

    /// Verifies: REQ-CON-162
    #[test]
    fn when_a_quiet_link_is_heard_again_it_is_no_longer_quiet() {
        let mut room = Room::of(&[2]);
        assert!(room.side(2, 2000).is_some());
        room.hear(2, 2500);
        assert_eq!(room.side(2, 2600), None);
    }

    /// Verifies: REQ-CON-162
    #[test]
    fn when_a_link_went_quiet_twice_within_fifteen_minutes_it_is_unstable() {
        let mut room = Room::of(&[2]);
        assert!(!room.tick(2000).standing[&id(2)].unstable);
        room.hear(2, 3000);
        assert!(!room.tick(3100).standing[&id(2)].unstable);
        assert!(room.tick(5000).standing[&id(2)].unstable);
        room.hear(2, 6000);
        assert!(room.tick(6100).standing[&id(2)].unstable);
    }

    /// Verifies: REQ-CON-162
    #[test]
    fn when_the_second_dropout_is_more_than_fifteen_minutes_after_the_first_it_is_not_unstable() {
        let mut room = Room::of(&[2]);
        room.tick(2000);
        room.hear(2, 3000);
        room.tick(3100);
        let later = 3000 + UNSTABLE_WINDOW.as_millis() as u64 + 1000;
        assert!(!room.tick(later).standing[&id(2)].unstable);
    }

    /// Verifies: REQ-CON-163
    #[test]
    fn when_every_participant_goes_quiet_together_the_line_that_broke_is_this_apps() {
        let mut room = Room::of(&[2, 3, 4]);
        for (peer, at) in [(2, 100), (3, 500), (4, 900)] {
            room.hear(peer, at);
        }
        for peer in [2, 3, 4] {
            assert_eq!(room.side(peer, 3000), Some(Side::Mine));
        }
    }

    /// Verifies: REQ-CON-163
    #[test]
    fn when_one_of_three_goes_quiet_and_the_other_hears_them_the_path_between_the_two_broke() {
        let mut room = Room::of(&[2, 3]);
        room.hear(3, 2900);
        // 3 last heard 2 at 2900 ms, long after this app stopped hearing it
        room.told(3, 3000, &[(2, 100), (1, 500)]);
        assert_eq!(room.side(2, 3100), Some(Side::Between));
        assert_eq!(room.side(3, 3100), None);
    }

    /// Verifies: REQ-CON-163
    #[test]
    fn when_one_of_three_goes_quiet_and_the_other_stopped_hearing_them_too_their_line_broke() {
        let mut room = Room::of(&[2, 3]);
        room.hear(3, 2900);
        // 3 last heard 2 at 0 ms, when this app did, and says so well after it
        room.told(3, 3000, &[(2, 3000)]);
        assert_eq!(room.side(2, 3100), Some(Side::Theirs));
    }

    /// Verifies: REQ-CON-163
    #[test]
    fn when_the_other_participants_say_what_they_heard_before_the_audio_could_show_it_they_are_not_believed(
    ) {
        let mut room = Room::of(&[2, 3]);
        room.hear(3, 2900);
        // Said 200 ms after this app stopped hearing 2: the participant may only have been slow to notice
        room.told(3, 200, &[(2, 200)]);
        assert_eq!(room.side(2, 3100), Some(Side::Checking));
    }

    /// Verifies: REQ-CON-163
    #[test]
    fn when_the_participants_split_on_whether_they_hear_them_the_majority_decides_and_a_tie_is_theirs(
    ) {
        let mut room = Room::of(&[2, 3, 4]);
        room.hear(3, 2900);
        room.hear(4, 2900);
        room.told(3, 3000, &[(2, 3000)]);
        room.told(4, 3000, &[(2, 100)]);
        assert_eq!(room.side(2, 3100), Some(Side::Theirs));
        room.told(3, 3200, &[(2, 100)]);
        assert_eq!(room.side(2, 3300), Some(Side::Between));
    }

    /// Verifies: REQ-CON-163
    #[test]
    fn when_in_a_room_of_two_the_participant_goes_quiet_the_server_is_asked() {
        let mut room = Room::of(&[2]);
        assert!(!room.tick(1000).ask_presence);
        let tick = room.tick(2000);
        assert!(tick.ask_presence);
        assert_eq!(tick.standing[&id(2)].dropout.unwrap().side, Side::Checking);
    }

    /// Verifies: REQ-CON-163
    #[test]
    fn when_in_a_room_of_two_the_server_has_heard_from_them_since_the_audio_stopped_the_path_between_broke(
    ) {
        let mut room = Room::of(&[2]);
        room.tick(2000);
        // Their connection pinged the server 300 ms ago, 4.7 s after this app last heard them
        room.presence(5000, &[(2, 300)]);
        assert_eq!(room.side(2, 5000), Some(Side::Between));
    }

    /// Verifies: REQ-CON-163
    #[test]
    fn when_in_a_room_of_two_the_server_last_heard_from_them_before_the_audio_stopped_the_answer_waits(
    ) {
        let mut room = Room::of(&[2]);
        room.tick(2000);
        room.presence(5000, &[(2, 5200)]);
        assert_eq!(room.side(2, 5000), Some(Side::Checking));
        assert!(room.tick(7000).ask_presence);
    }

    /// Verifies: REQ-CON-163
    #[test]
    fn when_in_a_room_of_two_the_server_has_not_heard_from_them_for_longer_than_a_ping_is_late_their_line_broke(
    ) {
        let mut room = Room::of(&[2]);
        room.tick(2000);
        let late = (SERVER_PING_EVERY + SERVER_PING_SLACK).as_millis() as u64;
        room.presence(late + 500, &[(2, late + 600)]);
        assert_eq!(room.side(2, late + 500), Some(Side::Theirs));
    }

    /// Verifies: REQ-CON-163
    #[test]
    fn when_in_a_room_of_two_their_line_broke_the_side_is_named_before_the_audio_link_gives_up() {
        // The link gives up after 10 s and the room is entered again, which starts the
        // question over: the server's answer has to be in before that.
        let gives_up = Duration::from_secs(10);
        assert!(SERVER_PING_EVERY + SERVER_PING_SLACK + PRESENCE_EVERY < gives_up);
    }

    /// Verifies: REQ-CON-163
    #[test]
    fn when_in_a_room_of_two_the_server_no_longer_lists_them_their_line_broke() {
        let mut room = Room::of(&[2]);
        room.tick(2000);
        room.presence(2500, &[]);
        assert_eq!(room.side(2, 2600), Some(Side::Theirs));
    }

    /// Verifies: REQ-CON-163
    #[test]
    fn when_in_a_room_of_two_the_server_does_not_answer_this_apps_own_line_broke() {
        let mut room = Room::of(&[2]);
        room.tick(2000);
        assert_eq!(room.side(2, 4000), Some(Side::Checking));
        assert_eq!(room.side(2, 5100), Some(Side::Mine));
    }

    /// Verifies: REQ-CON-163
    #[test]
    fn when_the_server_answers_after_not_answering_the_side_is_decided_again() {
        let mut room = Room::of(&[2]);
        room.tick(2000);
        assert_eq!(room.side(2, 5100), Some(Side::Mine));
        room.presence(5200, &[(2, 100)]);
        assert_eq!(room.side(2, 5300), Some(Side::Between));
    }

    /// Verifies: REQ-CON-163
    #[test]
    fn when_the_signaling_connection_is_down_this_apps_own_line_broke_whoever_is_quiet() {
        let mut room = Room::of(&[2, 3]);
        room.hear(3, 2900);
        room.line = Line::Down;
        assert_eq!(room.side(2, 3000), Some(Side::Mine));
    }

    /// Verifies: REQ-CON-163
    #[test]
    fn when_the_system_says_the_machine_is_offline_this_apps_own_line_broke() {
        let mut room = Room::of(&[2]);
        room.network = Network::Offline;
        assert_eq!(room.side(2, 2000), Some(Side::Mine));
    }

    /// Verifies: REQ-CON-163
    #[test]
    fn when_nothing_can_tell_the_side_for_a_long_time_it_is_given_as_unknown() {
        let mut room = Room::of(&[2]);
        room.tick(2000);
        // Their connection never pinged the server, so how long ago it last did says nothing
        room.dropouts
            .presence_received(room.at(41_000), vec![(id(2), None)]);
        assert_eq!(room.side(2, 41_000), Some(Side::Unknown));
    }

    /// Verifies: REQ-CON-163
    #[test]
    fn when_a_participant_is_heard_the_side_is_no_longer_asked_after() {
        let mut room = Room::of(&[2]);
        room.tick(2000);
        room.hear(2, 2100);
        assert!(!room.tick(2200).ask_presence);
        assert!(room.dropouts.asked_at.is_none());
    }

    /// Verifies: REQ-CON-163
    #[test]
    fn when_someone_goes_quiet_the_room_is_told_what_this_app_hears_and_asked_to_say_the_same() {
        let mut room = Room::of(&[2, 3]);
        room.hear(3, 1900);
        let view = room.tick(2000).share.unwrap();
        assert!(view.ask);
        let ages: HashMap<Uuid, u64> = view.links.iter().map(|l| (l.peer, l.ago_ms)).collect();
        assert_eq!(ages[&id(2)], 2000);
        assert_eq!(ages[&id(3)], 100);
    }

    /// Verifies: REQ-CON-163
    #[test]
    fn when_everyone_is_heard_the_room_is_told_nothing() {
        let mut room = Room::of(&[2, 3]);
        assert_eq!(room.tick(500).share, None);
        assert_eq!(room.tick(1000).share, None);
    }

    /// Verifies: REQ-CON-163
    #[test]
    fn when_someone_stays_quiet_the_room_is_told_again_every_two_seconds_and_no_more_often() {
        let mut room = Room::of(&[2, 3]);
        room.hear(3, 1900);
        assert!(room.tick(2000).share.is_some());
        room.hear(3, 2400);
        assert_eq!(room.tick(2500).share, None);
        room.hear(3, 3900);
        assert!(room.tick(4000).share.is_some());
    }

    /// Verifies: REQ-CON-163
    #[test]
    fn when_the_quiet_one_is_heard_again_the_room_is_told_so_once_without_asking() {
        let mut room = Room::of(&[2, 3]);
        room.hear(3, 1900);
        room.tick(2000);
        room.hear(2, 3000);
        room.hear(3, 3000);
        let view = room.tick(3100).share.unwrap();
        assert!(!view.ask);
        assert_eq!(room.tick(3200).share, None);
    }

    /// Verifies: REQ-CON-163
    #[test]
    fn when_a_participant_asks_what_this_app_hears_it_answers_at_once_but_not_more_than_once_a_second(
    ) {
        let mut room = Room::of(&[2, 3]);
        assert_eq!(room.tick(100).share, None);
        let ask = LinkView {
            links: vec![],
            ask: true,
        };
        room.dropouts
            .view_received(room.at(200), id(2), ask.clone());
        let answer = room.tick(300).share.unwrap();
        assert!(!answer.ask);
        room.dropouts.view_received(room.at(400), id(2), ask);
        assert_eq!(room.tick(500).share, None);
        assert!(room.tick(1400).share.is_some());
    }

    /// Verifies: REQ-CON-163
    #[test]
    fn when_a_view_names_more_participants_than_a_room_holds_the_rest_are_ignored() {
        let mut room = Room::of(&[2]);
        let links = (100..200)
            .map(|n| LinkAge {
                peer: id(n),
                ago_ms: 0,
            })
            .collect();
        room.dropouts
            .view_received(room.at(0), id(3), LinkView { links, ask: false });
        assert_eq!(room.dropouts.told[&id(3)].ago.len(), MAX_VIEW_LINKS);
    }

    /// Verifies: REQ-CON-163
    #[test]
    fn when_a_link_ends_what_was_known_of_it_is_forgotten() {
        let mut room = Room::of(&[2, 3]);
        room.hear(3, 1900);
        room.tick(2000);
        room.told(3, 2100, &[(2, 100)]);
        room.heard.retain(|(peer, _)| *peer != id(3));
        room.tick(2200);
        assert!(room.dropouts.told.is_empty());
    }

    /// Verifies: REQ-CON-163
    #[test]
    fn a_link_view_travels_as_a_peer_message_body_under_its_own_topic() {
        use crate::network::settings_help::PeerBody;
        let body = PeerBody::LinkView(LinkView {
            links: vec![LinkAge {
                peer: id(2),
                ago_ms: 40,
            }],
            ask: true,
        });
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"link_view": {"links": [{"peer": id(2), "ago_ms": 40}], "ask": true}})
        );
        assert_eq!(serde_json::from_value::<PeerBody>(json).unwrap(), body);
    }

    /// Verifies: REQ-CON-163
    #[test]
    fn a_link_view_that_asks_nothing_leaves_the_flag_out() {
        let view = LinkView {
            links: vec![],
            ask: false,
        };
        assert_eq!(
            serde_json::to_value(&view).unwrap(),
            serde_json::json!({"links": []})
        );
    }
}
