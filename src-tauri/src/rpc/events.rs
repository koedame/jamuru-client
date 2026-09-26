//! Events the app publishes to the portals that listen (ADR-044).
//!
//! The backend announces a change by emitting a Tauri event; the screen hears
//! it through the webview. A remote portal hears the same events here, but
//! only those the table lets its portal receive.

use serde_json::Value;
use tauri::{AppHandle, Listener, Manager, Runtime};
use tokio::sync::broadcast;

use super::spec::Access;

/// An event a portal may be sent, and which portals may.
pub struct EventSpec {
    pub name: &'static str,
    pub access: Access,
}

pub const EVENTS: &[EventSpec] = &[
    EventSpec {
        name: "audio:config-changed",
        access: Access::ALL,
    },
    // The language of the person's own windows: a helper's screen keeps the
    // helper's.
    EventSpec {
        name: "i18n:language-changed",
        access: Access::NO_HELP,
    },
    EventSpec {
        name: crate::session::CHANGED_EVENT,
        access: Access::ALL,
    },
    EventSpec {
        name: crate::mixer::CHANGED_EVENT,
        access: Access::ALL,
    },
    // The person's own settings help, drawn on their own screen.
    EventSpec {
        name: crate::session::HELP_EVENT,
        access: Access::NO_HELP,
    },
];

/// One event as it was emitted.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub name: &'static str,
    pub payload: Value,
}

/// Fans the app's events out to whoever subscribes. A subscriber that falls
/// behind loses the oldest events rather than holding the app up.
pub struct EventHub {
    sender: broadcast::Sender<Event>,
}

impl EventHub {
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.sender.subscribe()
    }
}

/// Starts listening for the events in [`EVENTS`] and makes the hub available
/// as managed state. Call once, from setup.
pub fn install<R: Runtime>(app: &AppHandle<R>) {
    let (sender, _) = broadcast::channel(64);
    for spec in EVENTS {
        let sender = sender.clone();
        app.listen_any(spec.name, move |event| {
            let payload = serde_json::from_str(event.payload()).unwrap_or(Value::Null);
            // No subscriber is the normal case: nobody is connected.
            let _ = sender.send(Event {
                name: spec.name,
                payload,
            });
        });
    }
    app.manage(EventHub { sender });
}

#[cfg(test)]
mod tests {
    use tauri::Emitter;

    use super::*;
    use crate::rpc::spec::Portal;

    /// Verifies: REQ-RMT-022
    #[tokio::test]
    async fn an_emitted_event_reaches_a_subscriber() {
        let app = tauri::test::mock_app();
        let handle = app.handle().clone();
        install(&handle);
        let mut events = handle.state::<EventHub>().subscribe();

        handle
            .emit("audio:config-changed", serde_json::json!({ "revision": 3 }))
            .unwrap();

        let event = tokio::time::timeout(std::time::Duration::from_secs(1), events.recv())
            .await
            .expect("the event should arrive")
            .unwrap();
        assert_eq!(event.name, "audio:config-changed");
        assert_eq!(event.payload, serde_json::json!({ "revision": 3 }));
    }

    #[test]
    fn every_event_is_open_to_the_screen_and_the_portals_it_lists() {
        for spec in EVENTS {
            assert!(spec.access.allows(Portal::Screen), "{}", spec.name);
        }
    }
}
