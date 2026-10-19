//! The audio session of a client: the devices, the thread that serves them, the send thread and
//! the audio links to the other participants. The app and the CLI run the same one; they differ
//! in who steers it and what they show of it (ADR-077).
//!
//! A session runs on a thread of its own, with its own current-thread runtime that carries the
//! network as well, at real-time priority. It is started with [`start`], steered with
//! [`Command`]s, and read through the [`Shared`] it publishes to. Opening a device, which a hung
//! driver can make take as long as it likes, is done off that thread (`audio::slot`).
//!
//! Performance optimizations for 32-sample buffers:
//! - Uses rtrb (real-time safe ring buffer) instead of tokio channels
//! - Zero allocation in audio callbacks
//! - Pre-allocated stereo conversion buffer

mod links;
mod run;
mod send;
mod shared;
mod stand_in;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc as std_mpsc;
use std::sync::Arc;
use std::thread::JoinHandle;

use uuid::Uuid;

use crate::audio::{AudioPreset, BurstProbe};
use crate::network::LinkIdentity;

pub use shared::{BandwidthSnapshot, Flights, LinkReport, LinkReports, Shared, MAX_LINKS};

/// A participant to open an audio link to
#[derive(Debug, Clone)]
pub struct LinkTarget {
    pub peer: Uuid,
    /// Their addresses, best first. All are raced, so one on the same network
    /// is reached directly instead of only through its public address
    /// (REQ-CON-113).
    pub candidates: Vec<SocketAddr>,
    /// Our key in the room and the key the server gave for them: with them
    /// the link is checked to be theirs (`Connection::verify_peer`). `None`
    /// for a peer reached without a room.
    pub link: Option<(LinkIdentity, Option<String>)>,
}

impl PartialEq for LinkTarget {
    fn eq(&self, other: &Self) -> bool {
        let keys = |target: &Self| {
            target
                .link
                .as_ref()
                .map(|(ours, theirs)| (ours.public_key(), theirs.clone()))
        };
        self.peer == other.peer && self.candidates == other.candidates && keys(self) == keys(other)
    }
}

/// Commands sent to the audio thread
#[derive(Debug, PartialEq)]
pub enum Command {
    Stop,
    SetInputDevice(Option<String>),
    /// Capture 1 (mono) or 2 (stereo) channels from now on
    SetTransmitChannels(u32),
    /// Capture from these input device channels (1-based settings) from now on
    SetInputChannels(u32, Option<u32>),
    /// Play on these output device channels (1-based settings) from now on
    SetOutputChannels(u32, Option<u32>),
    SetOutputDevice(Option<String>),
    SetMute(bool),
    SetMonitoring(bool),
    SetMasterVolume(f32),
    SetLocalVolume(f32),
    SetLocalPan(i32),
    /// Rebuild the receive jitter buffer at a new depth (ADR-020)
    SetJitterBufferFrames(u32),
    /// Resume probing on every link automatic recovery gave up on (REQ-CON-110)
    Reconnect,
    /// Open an audio link to a participant
    Link(Box<LinkTarget>),
    /// Close the audio link to a participant. The others go on.
    Unlink(Uuid),
}

/// What is captured in place of an input device
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InputSignal {
    /// A sine tone of this frequency in Hz
    Tone(f32),
    /// A short tone burst every 500 ms, whose round trip a [`BurstProbe`] times
    Bursts,
}

/// What a session is started with
pub struct Params {
    /// The socket every link goes through, the one whose address the room was told
    pub prepared_socket: std::net::UdpSocket,
    /// Id of the device to capture from, as the app saves it (`None`: the system default)
    pub input_device_id: Option<String>,
    /// Id of the device to play to (`None`: the system default)
    pub output_device_id: Option<String>,
    pub buffer_size: u32,
    pub preset: AudioPreset,
    pub sample_rate: u32,
    /// How many channels are sent: 1 (mono) or 2 (stereo)
    pub transmit_channels: u32,
    /// Which channels of a multi-channel interface are read (1-based)
    pub input_channels: (u32, Option<u32>),
    /// Which channels of a multi-channel interface are played on (1-based)
    pub output_channels: (u32, Option<u32>),
    /// Stand-in for the input device, to run where there is no sound card
    pub input_signal: Option<InputSignal>,
    /// Stand-in for the output device: the file what would be played is written to (raw 32-bit
    /// float, little-endian, stereo interleaved)
    pub output_file: Option<PathBuf>,
    /// Stamps the bursts sent and the bursts played out, to time the round trip
    pub probe: Option<Arc<BurstProbe>>,
}

/// A session that was started
pub struct Running {
    /// Where the commands go
    pub commands: std_mpsc::Sender<Command>,
    /// Says whether the session could open what it needs: `Ok` once it has, the reason once it
    /// could not. A session that failed to start has ended.
    pub started: std_mpsc::Receiver<Result<(), String>>,
    /// The audio thread. Ends after [`Command::Stop`].
    pub thread: JoinHandle<()>,
}

/// Starts an audio session on a thread of its own, with no links yet.
///
/// `shared` is marked active before the thread is spawned, and cleared by it as the very last
/// thing it does. `on_end` is told how the session ended, on that thread, before the links are
/// forgotten.
pub fn start(
    params: Params,
    shared: &Shared,
    on_end: impl FnOnce(&Result<(), String>) + Send + 'static,
) -> Running {
    let (commands, cmd_rx) = std_mpsc::channel::<Command>();
    let (ready_tx, started) = std_mpsc::channel::<Result<(), String>>();
    let shared = shared.clone();

    // Mark as active BEFORE spawning the thread to avoid a race condition
    shared.is_active.store(true, Ordering::SeqCst);

    let thread = std::thread::spawn(move || {
        // Set real-time priority immediately upon thread start
        crate::audio::priority::set_current_thread_realtime_priority("session");

        // Create tokio runtime for this thread
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to create tokio runtime");

        rt.block_on(async move {
            let result = run::run(params, cmd_rx, &shared, &ready_tx).await;
            if let Err(e) = &result {
                let _ = ready_tx.send(Err(e.clone()));
            }
            on_end(&result);
            // The links ended with the session
            if let Ok(mut links) = shared.links.write() {
                links.clear();
            }
            shared.is_active.store(false, Ordering::SeqCst);
        });
    });

    Running {
        commands,
        started,
        thread,
    }
}
