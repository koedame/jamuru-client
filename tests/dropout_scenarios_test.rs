//! Whose line broke, as every participant of a room works it out together (REQ-CON-163, ADR-112)
//!
//! Several apps run the decision against audio links that stop hearing each other as a scenario
//! says, with the messages between them (what each hears, the server's answer to who is in the
//! room) delivered as the real thing would deliver them. Time is virtual: one step is 100 ms.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use jamjam::network::dropout::{
    Dropouts, Heard, Line, Network, Presence, Side, Standing, SERVER_PING_EVERY,
};
use uuid::Uuid;

const STEP: Duration = Duration::from_millis(100);
/// When the audio stops, after everyone has been heard for a while.
const CUT_AT: Duration = Duration::from_secs(3);
/// Long enough for a participant whose line broke to be a whole ping and its slack silent to the server.
const RUN_FOR: Duration = Duration::from_secs(30);

struct App {
    id: Uuid,
    dropouts: Dropouts,
    /// When each other participant's audio last arrived here.
    heard: HashMap<Uuid, Instant>,
    standing: HashMap<Uuid, Standing>,
    /// For each participant, how long after the cut the app first knew whose line it was
    /// (said any side but "checking").
    decided_after: HashMap<Uuid, Duration>,
}

/// What a scenario breaks.
#[derive(Default)]
struct Cuts {
    /// Apps whose own line is down: nothing reaches them and nothing they send reaches anyone.
    lines: HashSet<usize>,
    /// Pairs whose audio does not get through either way, though both lines are up.
    paths: HashSet<(usize, usize)>,
}

impl Cuts {
    fn path_cut(&self, a: usize, b: usize) -> bool {
        self.paths.contains(&(a, b)) || self.paths.contains(&(b, a))
    }
}

/// A room of `count` apps, run to the end of the scenario. The cuts happen at [`CUT_AT`].
fn run(count: usize, cuts: Cuts) -> Vec<App> {
    let start = Instant::now();
    let mut apps: Vec<App> = (0..count)
        .map(|_| App {
            id: Uuid::new_v4(),
            dropouts: Dropouts::new(),
            heard: HashMap::new(),
            standing: HashMap::new(),
            decided_after: HashMap::new(),
        })
        .collect();
    let ids: Vec<Uuid> = apps.iter().map(|app| app.id).collect();
    // The server's own record: when each app's connection last pinged it
    let mut last_ping: HashMap<Uuid, Instant> = HashMap::new();
    let mut next_ping = Duration::ZERO;

    let mut elapsed = Duration::ZERO;
    while elapsed <= RUN_FOR {
        let now = start + elapsed;
        let cut = elapsed >= CUT_AT;

        // The audio that gets through in this step
        for (a, app) in apps.iter_mut().enumerate() {
            for (b, id) in ids.iter().enumerate() {
                if a == b {
                    continue;
                }
                let gets_through = !cut
                    || (!cuts.lines.contains(&a)
                        && !cuts.lines.contains(&b)
                        && !cuts.path_cut(a, b));
                if gets_through {
                    app.heard.insert(*id, now);
                }
            }
        }
        // Every app that can reach the server pings it every [`SERVER_PING_EVERY`]
        if elapsed >= next_ping {
            for (i, id) in ids.iter().enumerate() {
                if !(cut && cuts.lines.contains(&i)) {
                    last_ping.insert(*id, now);
                }
            }
            next_ping += SERVER_PING_EVERY;
        }

        // Each app decides, and what it says reaches the others whose line is up
        let mut views = Vec::new();
        let mut asking_server = Vec::new();
        for (i, app) in apps.iter_mut().enumerate() {
            let links: Vec<Heard> = ids
                .iter()
                .filter(|id| **id != app.id)
                .map(|id| Heard {
                    peer: *id,
                    at: app.heard.get(id).copied(),
                })
                .collect();
            let tick = app.dropouts.tick(now, &links, Line::Up, Network::Unknown);
            app.standing = tick.standing;
            for (peer, standing) in &app.standing {
                let decided = standing
                    .dropout
                    .is_some_and(|dropout| dropout.side != Side::Checking);
                if decided && cut {
                    app.decided_after.entry(*peer).or_insert(elapsed - CUT_AT);
                }
            }
            let line_up = !(cut && cuts.lines.contains(&i));
            if let (Some(view), true) = (tick.share, line_up) {
                views.push((i, view));
            }
            if tick.ask_presence && line_up {
                asking_server.push(i);
            }
        }
        for (from, view) in views {
            for (to, app) in apps.iter_mut().enumerate() {
                let reaches = !(cut && cuts.lines.contains(&to));
                if to != from && reaches {
                    app.dropouts.view_received(now, ids[from], view.clone());
                }
            }
        }
        for asker in asking_server {
            let presence: Presence = ids
                .iter()
                .filter(|id| **id != ids[asker])
                .map(|id| {
                    (
                        *id,
                        last_ping
                            .get(id)
                            .map(|at| now.saturating_duration_since(*at)),
                    )
                })
                .collect();
            apps[asker].dropouts.presence_received(now, presence);
        }
        elapsed += STEP;
    }
    apps
}

/// How long after the cut app `of` knew whose line it was that stopped being heard from `about`.
fn decided_after(apps: &[App], of: usize, about: usize) -> Duration {
    apps[of].decided_after[&apps[about].id]
}

/// How app `of` stands towards app `about` at the end of the scenario.
fn side(apps: &[App], of: usize, about: usize) -> Option<Side> {
    apps[of]
        .standing
        .get(&apps[about].id)
        .and_then(|standing| standing.dropout)
        .map(|dropout| dropout.side)
}

/// Test: Own line cut, in a room of three
/// Given A, B and C hearing each other
/// When A's own line goes down, so that A hears neither B nor C
/// Then A takes it for its own line, and B and C each take A's line for the one that broke
/// Verifies: REQ-CON-163
#[test]
fn when_a_participants_own_line_is_cut_in_a_room_of_three_they_see_it_is_their_own_line_and_the_others_see_theirs(
) {
    let apps = run(
        3,
        Cuts {
            lines: HashSet::from([0]),
            ..Default::default()
        },
    );

    assert_eq!(side(&apps, 0, 1), Some(Side::Mine));
    assert_eq!(side(&apps, 0, 2), Some(Side::Mine));
    assert_eq!(side(&apps, 1, 0), Some(Side::Theirs));
    assert_eq!(side(&apps, 2, 0), Some(Side::Theirs));
    assert_eq!(side(&apps, 1, 2), None, "B still hears C");
    eprintln!(
        "own line cut, three: A {:?}, B {:?}",
        decided_after(&apps, 0, 1),
        decided_after(&apps, 1, 0)
    );
    assert!(decided_after(&apps, 0, 1) <= Duration::from_secs(4));
    assert!(decided_after(&apps, 1, 0) <= Duration::from_secs(8));
}

/// Test: One participant's line cut, in a room of three
/// Given A, B and C hearing each other
/// When C's line goes down
/// Then A and B each take C's line for the one that broke, and C takes its own
/// Verifies: REQ-CON-163
#[test]
fn when_one_of_three_has_their_line_cut_the_other_two_see_their_line_and_they_see_their_own() {
    let apps = run(
        3,
        Cuts {
            lines: HashSet::from([2]),
            ..Default::default()
        },
    );

    assert_eq!(side(&apps, 0, 2), Some(Side::Theirs));
    assert_eq!(side(&apps, 1, 2), Some(Side::Theirs));
    assert_eq!(side(&apps, 2, 0), Some(Side::Mine));
    assert_eq!(side(&apps, 2, 1), Some(Side::Mine));
    assert_eq!(side(&apps, 0, 1), None, "A still hears B");
    eprintln!(
        "one cut, three: A {:?}, C {:?}",
        decided_after(&apps, 0, 2),
        decided_after(&apps, 2, 0)
    );
    assert!(decided_after(&apps, 0, 2) <= Duration::from_secs(8));
    assert!(decided_after(&apps, 2, 0) <= Duration::from_secs(4));
}

/// Test: The path between two of three cut
/// Given A, B and C hearing each other, every line up
/// When the audio between A and B stops, and C still hears both
/// Then A and B each see that it is the path between the two, and C sees nothing
/// Verifies: REQ-CON-163
#[test]
fn when_the_path_between_two_of_three_is_cut_the_two_see_the_path_between_them_broke() {
    let apps = run(
        3,
        Cuts {
            paths: HashSet::from([(0, 1)]),
            ..Default::default()
        },
    );

    assert_eq!(side(&apps, 0, 1), Some(Side::Between));
    assert_eq!(side(&apps, 1, 0), Some(Side::Between));
    assert_eq!(side(&apps, 2, 0), None);
    assert_eq!(side(&apps, 2, 1), None);
    eprintln!(
        "path cut, three: A {:?}, B {:?}",
        decided_after(&apps, 0, 1),
        decided_after(&apps, 1, 0)
    );
    assert!(decided_after(&apps, 0, 1) <= Duration::from_secs(8));
}

/// Test: The other one's line cut, in a room of two
/// Given A and B hearing each other, and both pinging the server
/// When B's line goes down, so that B stops pinging the server
/// Then A, with nobody else to ask, takes B's line for the one that broke
/// Verifies: REQ-CON-163
/// Verifies: REQ-CON-164
#[test]
fn when_in_a_room_of_two_the_other_ones_line_is_cut_the_server_not_hearing_them_says_it_is_their_line(
) {
    let apps = run(
        2,
        Cuts {
            lines: HashSet::from([1]),
            ..Default::default()
        },
    );

    assert_eq!(side(&apps, 0, 1), Some(Side::Theirs));
    eprintln!("line cut, two: A {:?}", decided_after(&apps, 0, 1));
    assert!(decided_after(&apps, 0, 1) <= Duration::from_secs(20));
}

/// Test: The path between the two cut, in a room of two
/// Given A and B hearing each other, and both pinging the server
/// When the audio between them stops while both lines stay up
/// Then both take it for the path between them, because the server still hears the other
/// Verifies: REQ-CON-163
/// Verifies: REQ-CON-164
#[test]
fn when_in_a_room_of_two_the_path_between_them_is_cut_the_server_still_hearing_them_says_it_is_the_path(
) {
    let apps = run(
        2,
        Cuts {
            paths: HashSet::from([(0, 1)]),
            ..Default::default()
        },
    );

    assert_eq!(side(&apps, 0, 1), Some(Side::Between));
    assert_eq!(side(&apps, 1, 0), Some(Side::Between));
    eprintln!("path cut, two: A {:?}", decided_after(&apps, 0, 1));
    assert!(decided_after(&apps, 0, 1) <= Duration::from_secs(8));
}
