//! The mixer: where the faders stand (ADR-044 §6).
//!
//! The backend owns this, not the screen. Each strip - the microphone, and
//! each of the others in the room - has a volume, a pan and (for the others) a
//! mute. The screen reads [`Snapshot`] with `mixer_get`, hears `mixer:changed`
//! when it changes, and acts with one command per operation, so a window that
//! draws this app's screen from another place (the one of someone helping)
//! starts from the faders as they are and follows them.
//!
//! What is heard follows the strips: the microphone's and the strip of the
//! participant the audio goes to are set on the audio engine as they change and
//! again whenever a session starts.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::session::SessionState;
use crate::streaming::StreamingState;

/// Announces a change of the mixer. The payload is the new [`Snapshot`].
pub const CHANGED_EVENT: &str = "mixer:changed";

/// The fader position that is 0 dB: the gain the audio plays at unchanged.
const UNITY_VOLUME: u32 = 80;
/// The top of the fader.
const MAX_VOLUME: u32 = 100;
const MAX_PAN: i32 = 100;

/// The microphone's strip. Whether it is muted, and heard back, are the audio
/// engine's (`streaming_set_mute`, `streaming_set_monitoring`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Strip {
    /// 0 to 100; 80 is 0 dB.
    pub volume: u32,
    /// -100 (left) to 100 (right).
    pub pan: i32,
}

impl Default for Strip {
    fn default() -> Self {
        Self {
            volume: UNITY_VOLUME,
            pan: 0,
        }
    }
}

/// The strip of another participant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PeerStrip {
    pub volume: u32,
    pub pan: i32,
    /// Silences them without moving the fader, so unmuting brings it back.
    pub muted: bool,
}

impl Default for PeerStrip {
    fn default() -> Self {
        let Strip { volume, pan } = Strip::default();
        Self {
            volume,
            pan,
            muted: false,
        }
    }
}

impl PeerStrip {
    /// The gain to play them at, in percent of unity gain.
    fn gain_percent(self) -> u32 {
        if self.muted {
            0
        } else {
            gain_percent(self.volume)
        }
    }
}

/// The gain, in percent of unity gain, a fader at `volume` plays at.
fn gain_percent(volume: u32) -> u32 {
    (volume * 100 + UNITY_VOLUME / 2) / UNITY_VOLUME
}

/// The whole mixer at one moment.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Snapshot {
    /// Rises with every change, so a reader that hears an old snapshot after a
    /// newer one can tell.
    pub revision: u64,
    pub local: Strip,
    /// One strip for each participant of the room, by their id.
    pub peers: BTreeMap<String, PeerStrip>,
}

#[derive(Default)]
struct Inner {
    revision: u64,
    local: Strip,
    peers: BTreeMap<String, PeerStrip>,
}

impl Inner {
    fn view(&self) -> Snapshot {
        Snapshot {
            revision: self.revision,
            local: self.local,
            peers: self.peers.clone(),
        }
    }
}

/// The mixer, managed by Tauri.
#[derive(Default)]
pub struct MixerState {
    inner: Mutex<Inner>,
}

impl MixerState {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn snapshot(&self) -> Snapshot {
        self.lock().view()
    }

    fn local(&self) -> Strip {
        self.lock().local
    }

    fn peer(&self, id: &str) -> PeerStrip {
        self.lock().peers.get(id).copied().unwrap_or_default()
    }

    /// Changes the mixer with `change`, which says whether it changed anything,
    /// and announces the result when it did.
    fn change<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        change: impl FnOnce(&mut Inner) -> bool,
    ) -> Snapshot {
        let (snapshot, changed) = {
            let mut inner = self.lock();
            let changed = change(&mut inner);
            if changed {
                inner.revision += 1;
            }
            (inner.view(), changed)
        };
        if changed {
            if let Err(e) = app.emit(CHANGED_EVENT, &snapshot) {
                tracing::warn!("Could not announce the mixer change: {}", e);
            }
        }
        snapshot
    }

    /// Gives each of `ids` a strip and drops the strips of anyone else: a
    /// participant who is new stands at the default, and one who left leaves
    /// nothing behind.
    pub fn keep_only<R: Runtime>(&self, app: &AppHandle<R>, ids: &[String]) {
        self.change(app, |inner| {
            let before = inner.peers.len();
            inner.peers.retain(|id, _| ids.contains(id));
            let mut changed = inner.peers.len() != before;
            for id in ids {
                if !inner.peers.contains_key(id) {
                    inner.peers.insert(id.clone(), PeerStrip::default());
                    changed = true;
                }
            }
            changed
        });
    }
}

/// Sets `audio` to the microphone's strip and to `peer`'s, the participant the
/// audio is about to go to. A session starts with the faders where they stand.
pub async fn set_on_audio<R: Runtime>(app: &AppHandle<R>, peer: &str) {
    let mixer = app.state::<MixerState>();
    apply_local(app, mixer.local()).await;
    apply_peer(app, mixer.peer(peer)).await;
}

async fn apply_local<R: Runtime>(app: &AppHandle<R>, strip: Strip) {
    let audio = app.state::<StreamingState>();
    audio.set_local_volume(gain_percent(strip.volume)).await;
    audio.set_local_pan(strip.pan).await;
}

async fn apply_peer<R: Runtime>(app: &AppHandle<R>, strip: PeerStrip) {
    let audio = app.state::<StreamingState>();
    audio.set_peer_volume(strip.gain_percent()).await;
    audio.set_peer_pan(strip.pan).await;
}

/// Whether the audio goes to `peer` at the moment.
fn is_heard<R: Runtime>(app: &AppHandle<R>, peer: &str) -> bool {
    app.try_state::<SessionState>()
        .is_some_and(|session| session.snapshot().streaming_peer_id.as_deref() == Some(peer))
}

async fn set_local<R: Runtime>(app: &AppHandle<R>, edit: impl FnOnce(&mut Strip)) -> Snapshot {
    let mixer = app.state::<MixerState>();
    let snapshot = mixer.change(app, |inner| {
        let before = inner.local;
        edit(&mut inner.local);
        inner.local != before
    });
    apply_local(app, snapshot.local).await;
    snapshot
}

async fn set_peer<R: Runtime>(
    app: &AppHandle<R>,
    peer: &str,
    edit: impl FnOnce(&mut PeerStrip),
) -> Result<Snapshot, String> {
    let mixer = app.state::<MixerState>();
    let mut known = true;
    let snapshot = mixer.change(app, |inner| match inner.peers.get_mut(peer) {
        Some(strip) => {
            let before = *strip;
            edit(strip);
            *strip != before
        }
        None => {
            known = false;
            false
        }
    });
    if !known {
        return Err(format!("There is no participant {} in the room", peer));
    }
    if is_heard(app, peer) {
        apply_peer(app, snapshot.peers[peer]).await;
    }
    Ok(snapshot)
}

// =============================================================================
// Commands
// =============================================================================

/// The mixer as it is.
#[tauri::command]
pub fn mixer_get(state: tauri::State<'_, MixerState>) -> Snapshot {
    state.snapshot()
}

/// Moves the microphone's fader.
#[tauri::command]
pub async fn mixer_set_local_volume(volume: u32, app: AppHandle) -> Snapshot {
    set_local(&app, |strip| strip.volume = volume.min(MAX_VOLUME)).await
}

/// Moves the microphone's pan.
#[tauri::command]
pub async fn mixer_set_local_pan(pan: i32, app: AppHandle) -> Snapshot {
    set_local(&app, |strip| strip.pan = pan.clamp(-MAX_PAN, MAX_PAN)).await
}

/// Moves the fader of the participant `peer_id`.
#[tauri::command]
pub async fn mixer_set_peer_volume(
    peer_id: String,
    volume: u32,
    app: AppHandle,
) -> Result<Snapshot, String> {
    set_peer(&app, &peer_id, |strip| strip.volume = volume.min(MAX_VOLUME)).await
}

/// Moves the pan of the participant `peer_id`.
#[tauri::command]
pub async fn mixer_set_peer_pan(
    peer_id: String,
    pan: i32,
    app: AppHandle,
) -> Result<Snapshot, String> {
    set_peer(&app, &peer_id, |strip| {
        strip.pan = pan.clamp(-MAX_PAN, MAX_PAN)
    })
    .await
}

/// Mutes or unmutes the participant `peer_id`.
#[tauri::command]
pub async fn mixer_set_peer_muted(
    peer_id: String,
    muted: bool,
    app: AppHandle,
) -> Result<Snapshot, String> {
    set_peer(&app, &peer_id, |strip| strip.muted = muted).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri::test::{mock_app, MockRuntime};
    use tauri::Listener;

    fn app() -> tauri::App<MockRuntime> {
        let app = mock_app();
        app.manage(MixerState::new());
        app.manage(StreamingState::new());
        app
    }

    fn ids(ids: &[&str]) -> Vec<String> {
        ids.iter().map(ToString::to_string).collect()
    }

    fn peer_ids(snapshot: Snapshot) -> Vec<String> {
        snapshot.peers.into_keys().collect()
    }

    fn heard_peer(app: &tauri::App<MockRuntime>) -> (u32, i32) {
        app.state::<StreamingState>().peer_gain()
    }

    /// Verifies: REQ-RMT-031
    #[test]
    fn a_fader_is_at_80_is_zero_db_and_plays_the_audio_unchanged() {
        assert_eq!(gain_percent(80), 100);
        assert_eq!(gain_percent(0), 0);
        assert_eq!(gain_percent(100), 125);
    }

    /// Verifies: REQ-RMT-031
    #[test]
    fn a_participant_is_in_the_room_gets_a_strip_at_the_default_and_one_who_left_loses_theirs() {
        let app = app();
        let mixer = app.state::<MixerState>();

        mixer.keep_only(app.handle(), &ids(&["a", "b"]));
        assert_eq!(peer_ids(mixer.snapshot()), ["a", "b"]);
        assert_eq!(mixer.snapshot().peers["a"], PeerStrip::default());

        mixer.keep_only(app.handle(), &ids(&["b"]));
        assert_eq!(peer_ids(mixer.snapshot()), ["b"]);
    }

    /// Verifies: REQ-RMT-031
    #[test]
    fn the_room_is_as_it_was_says_nothing_new() {
        let app = app();
        let mixer = app.state::<MixerState>();
        mixer.keep_only(app.handle(), &ids(&["a"]));
        let revision = mixer.snapshot().revision;

        mixer.keep_only(app.handle(), &ids(&["a"]));

        assert_eq!(mixer.snapshot().revision, revision);
    }

    /// Verifies: REQ-RMT-031
    #[tokio::test]
    async fn a_fader_is_moved_is_kept_and_announced_with_a_rising_revision() {
        let app = app();
        let handle = app.handle().clone();
        app.state::<MixerState>().keep_only(&handle, &ids(&["a"]));
        let heard = std::sync::Arc::new(Mutex::new(Vec::new()));
        let listener = heard.clone();
        handle.listen_any(CHANGED_EVENT, move |event| {
            listener.lock().unwrap().push(event.payload().to_string());
        });

        let first = set_peer(&handle, "a", |strip| strip.volume = 30)
            .await
            .unwrap();
        let second = set_peer(&handle, "a", |strip| strip.pan = -40)
            .await
            .unwrap();

        assert_eq!(second.peers["a"].volume, 30);
        assert_eq!(second.peers["a"].pan, -40);
        assert!(second.revision > first.revision);
        assert_eq!(app.state::<MixerState>().snapshot(), second);
        // Each move is announced once, with the strip it ended at.
        for _ in 0..50 {
            if heard.lock().unwrap().len() >= 2 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let heard = heard.lock().unwrap();
        assert_eq!(heard.len(), 2);
        assert!(heard[1].contains("\"pan\":-40"), "{}", heard[1]);
    }

    /// Verifies: REQ-RMT-031
    #[tokio::test]
    async fn a_fader_is_moved_to_where_it_is_announces_nothing() {
        let app = app();
        let handle = app.handle().clone();
        app.state::<MixerState>().keep_only(&handle, &ids(&["a"]));
        let revision = app.state::<MixerState>().snapshot().revision;

        set_peer(&handle, "a", |strip| strip.volume = UNITY_VOLUME)
            .await
            .unwrap();

        assert_eq!(app.state::<MixerState>().snapshot().revision, revision);
    }

    /// Verifies: REQ-RMT-031
    #[tokio::test]
    async fn a_participant_who_is_not_in_the_room_is_refused_and_changes_nothing() {
        let app = app();
        let handle = app.handle().clone();
        app.state::<MixerState>().keep_only(&handle, &ids(&["a"]));
        let before = app.state::<MixerState>().snapshot();

        let refused = set_peer(&handle, "nobody", |strip| strip.muted = true).await;

        assert!(refused.is_err());
        assert_eq!(app.state::<MixerState>().snapshot(), before);
    }

    /// Verifies: REQ-RMT-031
    #[tokio::test]
    async fn the_microphone_is_moved_is_set_on_the_audio_in_percent_of_unity_gain() {
        let app = app();
        let handle = app.handle().clone();

        set_local(&handle, |strip| {
            strip.volume = 40;
            strip.pan = 25;
        })
        .await;

        assert_eq!(app.state::<StreamingState>().local_gain(), (50, 25));
    }

    /// The audio goes to one participant, so only their strip is what is heard.
    ///
    /// Verifies: REQ-RMT-031
    #[tokio::test]
    async fn a_participant_the_audio_does_not_go_to_is_moved_leaves_what_is_heard_as_it_was() {
        let app = app();
        let handle = app.handle().clone();
        app.manage(SessionState::new());
        app.state::<MixerState>()
            .keep_only(&handle, &ids(&["a", "b"]));

        set_peer(&handle, "b", |strip| strip.volume = 10)
            .await
            .unwrap();

        assert_eq!(heard_peer(&app), (100, 0));
    }

    /// Verifies: REQ-RMT-031
    #[tokio::test]
    async fn a_session_starts_with_the_faders_where_they_stand_muted_being_silence() {
        let app = app();
        let handle = app.handle().clone();
        app.manage(SessionState::new());
        let mixer = app.state::<MixerState>();
        mixer.keep_only(&handle, &ids(&["a"]));
        set_local(&handle, |strip| strip.volume = 100).await;
        set_peer(&handle, "a", |strip| {
            strip.volume = 40;
            strip.pan = -30;
        })
        .await
        .unwrap();

        set_on_audio(&handle, "a").await;
        assert_eq!(heard_peer(&app), (50, -30));
        assert_eq!(app.state::<StreamingState>().local_gain().0, 125);

        set_peer(&handle, "a", |strip| strip.muted = true)
            .await
            .unwrap();
        set_on_audio(&handle, "a").await;
        assert_eq!(heard_peer(&app), (0, -30));
    }
}
