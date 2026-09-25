//! The window someone helping works in (ADR-044 §5): the helped app's own
//! screen, drawn in the helper's app from the helped app's state.
//!
//! What it shows is the helped app's: its own channel strip is the helped
//! person's microphone, not the helper's.

use crate::pom::driver::{Driver, DriverResult};
use crate::pom::element::Element;

const OWN_MUTE: &str = concat!(
    "[data-testid='channel-strip'][data-channel-type='local']",
    " [data-testid='channel-mute']"
);
const OWN_PEAK: &str = concat!(
    "[data-testid='channel-strip'][data-channel-type='local']",
    " [data-testid='channel-peak']"
);

pub struct HelperScreen<'a> {
    driver: &'a Driver,
    label: String,
}

impl<'a> HelperScreen<'a> {
    pub(crate) fn new(driver: &'a Driver, label: String) -> Self {
        Self { driver, label }
    }

    fn element(&self, selector: &'static str, name: &'static str) -> Element<'a> {
        Element::new(self.driver, selector, Some(self.label.clone()), name)
    }

    /// The line that says whose app this window works in.
    pub fn banner(&self) -> Element<'a> {
        self.element(
            "[data-testid='settings-help-window-banner']",
            "helper window banner",
        )
    }

    /// The helped app's session screen: its room, participants and mixer.
    pub fn session(&self) -> Element<'a> {
        self.element("[data-testid='session']", "helped app's session screen")
    }

    pub fn room_code(&self) -> DriverResult<String> {
        Ok(self
            .element("[data-testid='room-code']", "room code")
            .text()?
            .trim()
            .to_string())
    }

    /// Mute of the helped person's own channel: what they hear of themselves
    /// sent on.
    pub fn own_mute_button(&self) -> Element<'a> {
        self.element(OWN_MUTE, "mute button")
    }

    pub fn own_channel_is_muted(&self) -> DriverResult<bool> {
        Ok(self
            .driver
            .query(
                &format!("{}[aria-pressed='true']", OWN_MUTE),
                Some(&self.label),
            )?
            .exists)
    }

    /// Raw 0-100 peak of the helped person's own meter, as it reads in this window.
    pub fn own_channel_peak(&self) -> DriverResult<f32> {
        let raw = self
            .element(OWN_PEAK, "channel peak read-out")
            .attribute("data-peak")?
            .ok_or_else(|| format!("no channel meter matched {}", OWN_PEAK))?;
        raw.trim()
            .parse::<f32>()
            .map_err(|e| format!("data-peak {:?} is not a number: {}", raw, e))
    }

    /// What a helper is not offered on the helped app's screen.
    pub fn leave_button(&self) -> Element<'a> {
        self.element("[data-testid='leave-room']", "leave button")
    }

    pub fn chat_input(&self) -> Element<'a> {
        self.element("[data-testid='chat-input']", "chat input")
    }

    /// The header's settings button, which opens the helped app's audio
    /// settings beside the screen.
    pub fn settings_button(&self) -> Element<'a> {
        self.element(".main-header__icon-btn", "settings button")
    }

    pub fn settings_panel(&self) -> Element<'a> {
        self.element(
            "[data-testid='settings-help-panel']",
            "helper's settings panel",
        )
    }

    pub fn buffer_size_select(&self) -> Element<'a> {
        self.element(
            "[data-testid='settings-help-panel'] #buffer-size",
            "helper's buffer size dropdown",
        )
    }

    /// What the panel says about the last change (why it could not be applied).
    pub fn settings_status(&self) -> Element<'a> {
        self.element(
            "[data-testid='settings-help-panel-status']",
            "helper's panel status",
        )
    }

    /// Calls `method` on the helped app the way this window's screen does, so a
    /// scenario can ask for what the screen does not offer and see it refused.
    /// The inner error is the helped app's answer (`denied`, ...).
    pub fn call_helped(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> DriverResult<Result<serde_json::Value, String>> {
        self.driver.invoke_in(
            "help_call",
            &serde_json::json!({ "method": method, "params": params }),
            Some(&self.label),
        )
    }
}
