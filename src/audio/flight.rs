//! What went wrong with the received audio, and when.
//!
//! A gap in the output has a handful of possible causes that look alike in the
//! stream position but differ in timing: a packet that arrived late, a thread
//! that was away while packets waited, a play-out buffer another thread held
//! at the instant the device asked for it, the device reading faster than the
//! peer sends. Counters say how many; only the time of each says which. The
//! recorder keeps the last [`CAPACITY`] events with the microsecond each
//! happened at, so a debug call can show that the gaps come every 100.0 ms, or
//! follow a late arrival, or arrive with no late arrival at all.
//!
//! It is written from the audio callback, so it takes no lock and allocates
//! nothing: an event is one 64-bit word stored into a ring, and a count is one
//! relaxed add.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// Events kept. Old ones are overwritten.
pub const CAPACITY: usize = 512;

const KIND_BITS: u32 = 4;
const VALUE_BITS: u32 = 20;
const AT_BITS: u32 = 64 - KIND_BITS - VALUE_BITS;
const VALUE_MAX: u64 = (1 << VALUE_BITS) - 1;
const AT_MASK: u64 = (1 << AT_BITS) - 1;

/// What happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    /// A read found the buffer empty while playing and played silence. The
    /// position waits, so the delay grows by a frame. Value: none.
    Starved = 1,
    /// A read found a later frame but not the one due, and concealed it.
    Concealed = 2,
    /// A read played silence because the buffer was starting over (after a
    /// resynchronisation or a reconnect), not because a frame was missing.
    Primed = 3,
    /// A read played silence while the delay was being raised.
    Padded = 4,
    /// A read found the buffer held by the network side and played a frame of
    /// silence instead of waiting.
    Busy = 5,
    /// A frame arrived after its turn and was dropped. Value: none.
    LateFrame = 6,
    /// The stream jumped and the buffer started over. Value: how far it jumped,
    /// in frames.
    Resynced = 7,
    /// The gap between two received frames, when it was over two frames long.
    /// Value: the gap in microseconds. Measured where the network task hands a
    /// frame over, so a stalled thread shows here as well as a stalled link.
    LateArrival = 8,
    /// The thread that carries the network task was away for longer than two
    /// frames between two turns of its own loop. Value: microseconds.
    ThreadStall = 9,
    /// The device asked for a frame more than two frames' time after the
    /// previous ask: a callback came late. Value: microseconds.
    ReadGap = 10,
    /// The device asked again within a quarter of a frame's time: it asks for
    /// more than one frame per callback, so its callback is bigger than the
    /// frame. Value: microseconds.
    ReadBurst = 11,
    /// The buffer held more than its target for a whole stretch of reads, and
    /// the extra was discarded in one skip. Value: frames discarded.
    Trimmed = 12,
}

impl Kind {
    const ALL: [Kind; 12] = [
        Kind::Starved,
        Kind::Concealed,
        Kind::Primed,
        Kind::Padded,
        Kind::Busy,
        Kind::LateFrame,
        Kind::Resynced,
        Kind::LateArrival,
        Kind::ThreadStall,
        Kind::ReadGap,
        Kind::ReadBurst,
        Kind::Trimmed,
    ];

    fn from_bits(bits: u64) -> Option<Kind> {
        Kind::ALL.into_iter().find(|kind| *kind as u64 == bits)
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Starved => "starved",
            Kind::Concealed => "concealed",
            Kind::Primed => "primed",
            Kind::Padded => "padded",
            Kind::Busy => "busy",
            Kind::LateFrame => "late_frame",
            Kind::Resynced => "resynced",
            Kind::LateArrival => "late_arrival",
            Kind::ThreadStall => "thread_stall",
            Kind::ReadGap => "read_gap",
            Kind::ReadBurst => "read_burst",
            Kind::Trimmed => "trimmed",
        }
    }
}

/// One recorded event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Event {
    pub kind: Kind,
    /// Microseconds since the recorder was made.
    pub at_us: u64,
    /// See the kind; 0 when it has none. Saturates at about a second.
    pub value_us: u64,
}

/// What [`FlightRecorder::report`] returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// Microseconds since the recorder was made.
    pub now_us: u64,
    /// Reads the output side made: what the device asked for.
    pub reads: u64,
    /// Frames the network side stored: what the peer delivered.
    pub writes: u64,
    /// How many times each kind happened since the start, not only the ones
    /// still in the ring.
    pub counts: Vec<(Kind, u64)>,
    /// The most recent events, oldest first.
    pub events: Vec<Event>,
}

/// The recorder. Shared by the network side, the output callback and the loop
/// that carries the network task.
pub struct FlightRecorder {
    started: Instant,
    next: AtomicUsize,
    ring: Box<[AtomicU64]>,
    counts: [AtomicU64; Kind::ALL.len()],
    reads: AtomicU64,
    writes: AtomicU64,
}

impl FlightRecorder {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            started: Instant::now(),
            next: AtomicUsize::new(0),
            ring: (0..CAPACITY).map(|_| AtomicU64::new(0)).collect(),
            counts: Default::default(),
            reads: AtomicU64::new(0),
            writes: AtomicU64::new(0),
        })
    }

    /// Microseconds since the recorder was made.
    pub fn now_us(&self) -> u64 {
        self.started.elapsed().as_micros() as u64 & AT_MASK
    }

    /// Records that `kind` happened now. `value_us` is what the kind says it
    /// is, or 0.
    pub fn note(&self, kind: Kind, value_us: u64) {
        self.counts[kind as usize - 1].fetch_add(1, Ordering::Relaxed);
        let word = (kind as u64) << (VALUE_BITS + AT_BITS)
            | value_us.min(VALUE_MAX) << AT_BITS
            | self.now_us();
        let slot = self.next.fetch_add(1, Ordering::Relaxed) % CAPACITY;
        self.ring[slot].store(word, Ordering::Relaxed);
    }

    /// Counts a read the device asked for.
    pub fn count_read(&self) {
        self.reads.fetch_add(1, Ordering::Relaxed);
    }

    /// Counts a frame the network side stored.
    pub fn count_write(&self) {
        self.writes.fetch_add(1, Ordering::Relaxed);
    }

    /// How many times `kind` happened.
    pub fn count(&self, kind: Kind) -> u64 {
        self.counts[kind as usize - 1].load(Ordering::Relaxed)
    }

    /// The counts and the events still in the ring, oldest first.
    pub fn report(&self) -> Report {
        let mut events: Vec<Event> = self
            .ring
            .iter()
            .filter_map(|slot| {
                let word = slot.load(Ordering::Relaxed);
                Some(Event {
                    kind: Kind::from_bits(word >> (VALUE_BITS + AT_BITS))?,
                    at_us: word & AT_MASK,
                    value_us: (word >> AT_BITS) & VALUE_MAX,
                })
            })
            .collect();
        events.sort_by_key(|event| event.at_us);
        Report {
            now_us: self.now_us(),
            reads: self.reads.load(Ordering::Relaxed),
            writes: self.writes.load(Ordering::Relaxed),
            counts: Kind::ALL.iter().map(|k| (*k, self.count(*k))).collect(),
            events,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_event_comes_back_with_its_kind_and_value() {
        let recorder = FlightRecorder::new();

        recorder.note(Kind::LateArrival, 4_321);

        let report = recorder.report();
        assert_eq!(report.events.len(), 1);
        assert_eq!(report.events[0].kind, Kind::LateArrival);
        assert_eq!(report.events[0].value_us, 4_321);
        assert!(report.events[0].at_us <= report.now_us);
    }

    #[test]
    fn a_value_too_big_to_store_saturates_instead_of_spilling_into_the_kind() {
        let recorder = FlightRecorder::new();

        recorder.note(Kind::ThreadStall, u64::MAX);

        let event = recorder.report().events[0];
        assert_eq!(event.kind, Kind::ThreadStall);
        assert_eq!(event.value_us, VALUE_MAX);
    }

    #[test]
    fn counts_include_events_the_ring_has_already_overwritten() {
        let recorder = FlightRecorder::new();

        for _ in 0..CAPACITY + 10 {
            recorder.note(Kind::Starved, 0);
        }

        let report = recorder.report();
        assert_eq!(report.events.len(), CAPACITY);
        assert_eq!(recorder.count(Kind::Starved), CAPACITY as u64 + 10);
    }

    #[test]
    fn the_ring_keeps_the_newest_events_in_time_order() {
        let recorder = FlightRecorder::new();

        for value in 0..CAPACITY as u64 + 5 {
            recorder.note(Kind::LateArrival, value);
        }

        let events = recorder.report().events;
        assert_eq!(events.len(), CAPACITY);
        assert!(events.windows(2).all(|pair| pair[0].at_us <= pair[1].at_us));
        assert!(
            events.iter().all(|event| event.value_us >= 5),
            "the five oldest were overwritten"
        );
    }

    #[test]
    fn reads_and_writes_are_counted_separately_from_events() {
        let recorder = FlightRecorder::new();

        recorder.count_read();
        recorder.count_read();
        recorder.count_write();

        let report = recorder.report();
        assert_eq!((report.reads, report.writes), (2, 1));
        assert!(report.events.is_empty());
    }

    #[test]
    fn every_kind_has_a_distinct_name_and_survives_the_round_trip() {
        let recorder = FlightRecorder::new();
        let mut names: Vec<&str> = Kind::ALL.iter().map(|k| k.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), Kind::ALL.len());

        for kind in Kind::ALL {
            recorder.note(kind, 1);
        }

        let kinds: Vec<Kind> = recorder.report().events.iter().map(|e| e.kind).collect();
        for kind in Kind::ALL {
            assert!(kinds.contains(&kind), "{:?} was lost", kind);
        }
    }
}
