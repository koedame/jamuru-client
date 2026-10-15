//! Where the two people of a settings help are looking (ADR-044 §5).
//!
//! While a help goes on, each person's screen marks the part the other one is
//! pointing at and the part the other one has focused, as co-editing tools do.
//! What travels is as little as that needs: the name of a part, a place inside
//! it, and which part has the focus. Never the pointer's path, and nothing about
//! anything outside the window.
//!
//! The helper tells the helped app through the help portal ([`help_presence`]);
//! the helped app tells the helper by publishing an event the portal carries
//! ([`help_presence_publish`]). Each message is the whole picture as the sender
//! has it now, so one that is lost costs nothing the next does not mend.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

/// The event the helped app's screen hears the helper's presence as.
pub const FROM_HELPER_EVENT: &str = "help:presence-in";

/// The event the helped app's screen publishes and the help portal carries to
/// the helper's window.
pub const FROM_HELPED_EVENT: &str = "help:presence";

/// The window of the person helped that draws the helper's presence: the one
/// the help portal calls through.
const MAIN_WINDOW: &str = "main";

/// The longest name of a part that is accepted, in characters.
const MAX_ID_CHARS: usize = 200;

/// The least time between two messages from the helper that are passed on. The
/// helper's window sends ten a second at most; this is twice that, so network
/// jitter does not drop one a window sent in time, while a helper that does not
/// keep to the rate cannot load the screen of the person helped.
const MIN_INTERVAL: Duration = Duration::from_millis(50);

/// One part of the screen and a place in it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    /// The part's name: its `data-presence-id`, its `data-testid` or a path of
    /// tags that reads the same on both screens. Not its text, which is in
    /// each person's own language.
    pub id: String,
    /// Where in the part, from 0 (left) to 1 (right)
    pub x: f64,
    /// Where in the part, from 0 (top) to 1 (bottom)
    pub y: f64,
}

/// What one person's screen says of where they are.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Presence {
    /// The part the pointer is over, if it is over one that can be pressed or focused
    #[serde(default)]
    pub hover: Option<Target>,
    /// The part that has the focus
    #[serde(default)]
    pub focus: Option<Target>,
    /// Whether the helper has the settings panel open (the helper's message only)
    #[serde(default)]
    pub panel: Option<bool>,
}

impl Target {
    fn checked(mut self) -> Result<Self, &'static str> {
        let chars = self.id.chars().count();
        if chars == 0 || chars > MAX_ID_CHARS || self.id.chars().any(char::is_control) {
            return Err("not the name of a part");
        }
        if !self.x.is_finite() || !self.y.is_finite() {
            return Err("not a place in a part");
        }
        self.x = self.x.clamp(0.0, 1.0);
        self.y = self.y.clamp(0.0, 1.0);
        Ok(self)
    }
}

impl Presence {
    /// This message if it says what it may, with the places held to the part.
    pub fn checked(self) -> Result<Self, &'static str> {
        Ok(Self {
            hover: self.hover.map(Target::checked).transpose()?,
            focus: self.focus.map(Target::checked).transpose()?,
            panel: self.panel,
        })
    }
}

/// How lately the helper's last message was passed on.
#[derive(Default)]
pub struct PresenceState {
    last: Mutex<Option<Instant>>,
}

impl PresenceState {
    /// Whether a message arriving at `now` is passed on.
    fn admit(&self, now: Instant) -> bool {
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if last.is_some_and(|at| now.saturating_duration_since(at) < MIN_INTERVAL) {
            return false;
        }
        *last = Some(now);
        true
    }
}

/// What the helper's window is told of a message it sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PresenceAck {
    /// Whether it reached the screen of the person helped
    pub delivered: bool,
}

fn from_helper<R: Runtime>(
    app: &AppHandle<R>,
    state: &PresenceState,
    presence: Presence,
    now: Instant,
) -> Result<PresenceAck, String> {
    let presence = presence.checked().map_err(str::to_string)?;
    if !state.admit(now) {
        return Ok(PresenceAck { delivered: false });
    }
    let delivered = app.get_webview_window(MAIN_WINDOW).is_some_and(|_| {
        app.emit_to(MAIN_WINDOW, FROM_HELPER_EVENT, presence)
            .is_ok()
    });
    Ok(PresenceAck { delivered })
}

/// The helper tells the helped app where they are looking. Called through the
/// help portal, which is the only way in.
#[tauri::command]
pub fn help_presence(
    app: AppHandle,
    state: State<'_, PresenceState>,
    presence: Presence,
) -> Result<PresenceAck, String> {
    from_helper(&app, &state, presence, Instant::now())
}

/// The helped app's screen publishes where its person is looking; the help
/// portal carries it to the helper's window, and nothing carries it anywhere
/// when no help goes on.
#[tauri::command]
pub fn help_presence_publish(app: AppHandle, presence: Presence) -> Result<(), String> {
    let presence = presence.checked().map_err(str::to_string)?;
    app.emit(FROM_HELPED_EVENT, presence)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{Arc, Mutex as StdMutex};
    use tauri::Listener;

    fn target(id: &str, x: f64, y: f64) -> Target {
        Target {
            id: id.to_string(),
            x,
            y,
        }
    }

    /// Verifies: REQ-RMT-034
    #[test]
    fn a_message_with_anything_but_parts_and_places_is_not_accepted() {
        let with_a_trail = json!({
            "hover": {"id": "connection-panel-create-room", "x": 0.5, "y": 0.5, "trail": [[1, 2]]}
        });
        let with_a_screen_position = json!({"pointer": {"x": 10, "y": 20}});

        assert!(serde_json::from_value::<Presence>(with_a_trail).is_err());
        assert!(serde_json::from_value::<Presence>(with_a_screen_position).is_err());
    }

    /// Verifies: REQ-RMT-034
    #[test]
    fn a_place_outside_its_part_is_held_to_the_part() {
        let presence = Presence {
            hover: Some(target("mixer-mute", 1.8, -0.4)),
            ..Presence::default()
        };

        let held = presence.checked().unwrap().hover.unwrap();

        assert_eq!((held.x, held.y), (1.0, 0.0));
    }

    /// Verifies: REQ-RMT-034
    #[test]
    fn a_name_that_is_empty_too_long_or_not_text_is_refused() {
        for id in ["", &"a".repeat(MAX_ID_CHARS + 1), "line\nbreak"] {
            let presence = Presence {
                focus: Some(target(id, 0.5, 0.5)),
                ..Presence::default()
            };
            assert!(presence.checked().is_err(), "{:?} was accepted", id);
        }
        let not_a_number = Presence {
            hover: Some(target("mixer-mute", f64::NAN, 0.5)),
            ..Presence::default()
        };
        assert!(not_a_number.checked().is_err());
    }

    /// Verifies: REQ-RMT-034
    #[test]
    fn a_message_that_comes_sooner_than_ten_a_second_with_slack_is_not_passed_on() {
        let state = PresenceState::default();
        let start = Instant::now();

        assert!(state.admit(start));
        assert!(!state.admit(start + Duration::from_millis(10)));
        assert!(state.admit(start + Duration::from_millis(100)));
    }

    /// Verifies: REQ-RMT-034
    #[tokio::test]
    async fn the_helpers_message_reaches_the_main_window_of_the_person_helped() {
        let app = tauri::test::mock_app();
        let handle = app.handle().clone();
        tauri::WebviewWindowBuilder::new(&handle, "main", tauri::WebviewUrl::default())
            .build()
            .unwrap();
        let heard = Arc::new(StdMutex::new(Vec::<String>::new()));
        let sink = heard.clone();
        handle.listen_any(FROM_HELPER_EVENT, move |event| {
            sink.lock().unwrap().push(event.payload().to_string());
        });
        let state = PresenceState::default();
        let presence = Presence {
            hover: Some(target("mixer-mute", 0.25, 0.75)),
            focus: None,
            panel: Some(true),
        };

        let ack = from_helper(&handle, &state, presence, Instant::now()).unwrap();

        assert_eq!(ack, PresenceAck { delivered: true });
        tokio::time::sleep(Duration::from_millis(50)).await;
        let heard = heard.lock().unwrap();
        assert_eq!(heard.len(), 1);
        let payload: serde_json::Value = serde_json::from_str(&heard[0]).unwrap();
        assert_eq!(
            payload,
            json!({"hover": {"id": "mixer-mute", "x": 0.25, "y": 0.75}, "focus": null, "panel": true})
        );
    }

    /// Verifies: REQ-RMT-034
    #[test]
    fn when_there_is_no_main_window_the_message_is_not_said_to_have_arrived() {
        let app = tauri::test::mock_app();
        let state = PresenceState::default();

        let ack = from_helper(app.handle(), &state, Presence::default(), Instant::now()).unwrap();

        assert_eq!(ack, PresenceAck { delivered: false });
    }
}
