//! The in-session screen: room sidebar, mixer and chat, all in the main
//! window once the user is in a room.

use crate::pom::driver::{Driver, DriverResult};
use crate::pom::element::Element;

const ROOT: &str = "[data-testid='session']";

/// Strips are addressed by whose audio they carry, not by position: the
/// user's own channel and a peer's look alike in the DOM but mean opposite
/// things - one is the microphone, the other is what arrived over the network.
const OWN_PEAK: &str = concat!(
    "[data-testid='channel-strip'][data-channel-type='local']",
    " [data-testid='channel-peak']"
);
const PEER_PEAK: &str = concat!(
    "[data-testid='channel-strip'][data-channel-type='remote']",
    " [data-testid='channel-peak']"
);
const OWN_MUTE: &str = concat!(
    "[data-testid='channel-strip'][data-channel-type='local']",
    " [data-testid='channel-mute']"
);

pub struct SessionScreen<'a> {
    driver: &'a Driver,
}

impl<'a> SessionScreen<'a> {
    pub(crate) fn new(driver: &'a Driver) -> Self {
        Self { driver }
    }

    fn element(&self, selector: &'static str, name: &'static str) -> Element<'a> {
        Element::new(self.driver, selector, None, name)
    }

    pub fn is_displayed(&self) -> DriverResult<bool> {
        self.element(ROOT, "session screen").is_visible()
    }

    /// Joining involves a signaling round-trip and an audio start, so callers
    /// wait rather than assert immediately.
    pub fn wait_until_displayed(&self, timeout: std::time::Duration) -> DriverResult<()> {
        self.element(ROOT, "session screen")
            .wait_until_visible(timeout)
    }

    // --- room ---

    /// The invite code shown to the user, which is what they would share.
    pub fn room_code(&self) -> DriverResult<String> {
        Ok(self
            .element("[data-testid='room-code']", "room code")
            .text()?
            .trim()
            .to_string())
    }

    pub fn leave_button(&self) -> Element<'a> {
        self.element("[data-testid='leave-room']", "leave button")
    }

    /// The confirm button of the dialog the leave button opens.
    pub fn leave_confirm_button(&self) -> Element<'a> {
        self.element(".leave-dialog__confirm", "leave confirm button")
    }

    // --- participants ---

    /// How many people the sidebar shows, including the user themselves.
    pub fn participant_count(&self) -> DriverResult<usize> {
        self.element("[data-testid='participant-list'] > li", "participant")
            .count()
    }

    pub fn wait_for_participants(
        &self,
        expected: usize,
        timeout: std::time::Duration,
    ) -> DriverResult<()> {
        crate::pom::wait_until(timeout, || {
            self.participant_count().unwrap_or(0) == expected
        })
        .map_err(|_| {
            format!(
                "expected {} participants within {:?}, saw {:?}",
                expected,
                timeout,
                self.participant_count()
            )
        })
    }

    pub fn participant_names(&self) -> DriverResult<String> {
        self.element("[data-testid='participant-list']", "participant list")
            .text()
    }

    // --- mixer ---

    /// One strip per channel the user can adjust (their own plus each peer).
    pub fn channel_count(&self) -> DriverResult<usize> {
        self.element(".channel-strip", "channel strip").count()
    }

    pub fn wait_for_channels(
        &self,
        expected: usize,
        timeout: std::time::Duration,
    ) -> DriverResult<()> {
        crate::pom::wait_until(timeout, || self.channel_count().unwrap_or(0) == expected).map_err(
            |_| {
                format!(
                    "expected {} mixer channels within {:?}, saw {:?}",
                    expected,
                    timeout,
                    self.channel_count()
                )
            },
        )
    }

    /// Mute toggle of the user's own channel. `aria-pressed` reflects the
    /// state the user perceives, so scenarios assert on that rather than on
    /// CSS.
    pub fn own_mute_button(&self) -> Element<'a> {
        self.element(OWN_MUTE, "mute button")
    }

    pub fn own_channel_is_muted(&self) -> DriverResult<bool> {
        Ok(self
            .driver
            .query(&format!("{}[aria-pressed='true']", OWN_MUTE), None)?
            .exists)
    }

    /// Raw 0-100 peak level of the user's own meter: what their input device
    /// is picking up.
    ///
    /// Read from `data-peak` rather than the dB text, which is rounded and
    /// localised - and which shows an "-inf"-style value for silence, so
    /// parsing it would mean special-casing that.
    pub fn own_channel_peak(&self) -> DriverResult<f32> {
        self.channel_peak(OWN_PEAK)
    }

    /// Peak of the remote peer's channel: what is arriving over the network.
    ///
    /// This is the only meter that says audio actually crossed the connection.
    /// The user's own meter rises from their own capture whether or not a
    /// single packet ever left the machine.
    pub fn peer_channel_peak(&self) -> DriverResult<f32> {
        self.channel_peak(PEER_PEAK)
    }

    fn channel_peak(&self, selector: &'static str) -> DriverResult<f32> {
        let raw = self
            .element(selector, "channel peak read-out")
            .attribute("data-peak")?
            .ok_or_else(|| format!("no channel meter matched {}", selector))?;
        raw.trim()
            .parse::<f32>()
            .map_err(|e| format!("data-peak {:?} is not a number: {}", raw, e))
    }

    /// Waits until the user's own meter reads above `level`, returning what it
    /// read.
    ///
    /// Phrased as a threshold rather than "any signal" because the loopback
    /// device a scenario feeds cannot be returned to silence, so the meter has
    /// a floor - see [`crate::pom::loopback_audio`].
    pub fn wait_for_own_level_above(
        &self,
        level: f32,
        timeout: std::time::Duration,
    ) -> DriverResult<f32> {
        self.wait_for_level_above(OWN_PEAK, level, timeout)
    }

    /// Waits until the remote peer's channel reads above `level`.
    pub fn wait_for_peer_level_above(
        &self,
        level: f32,
        timeout: std::time::Duration,
    ) -> DriverResult<f32> {
        self.wait_for_level_above(PEER_PEAK, level, timeout)
    }

    fn wait_for_level_above(
        &self,
        selector: &'static str,
        level: f32,
        timeout: std::time::Duration,
    ) -> DriverResult<f32> {
        crate::pom::wait_until(timeout, || {
            self.channel_peak(selector)
                .map(|peak| peak > level)
                .unwrap_or(false)
        })
        .map_err(|_| {
            format!(
                "the meter at {} stayed at or below {} for {:?}; last peak {:?}",
                selector,
                level,
                timeout,
                self.channel_peak(selector)
            )
        })?;
        self.channel_peak(selector)
    }

    // --- chat ---

    pub fn chat_input(&self) -> Element<'a> {
        self.element("[data-testid='chat-input']", "chat input")
    }

    pub fn chat_send_button(&self) -> Element<'a> {
        self.element("[data-testid='chat-send']", "chat send button")
    }

    pub fn chat_message_count(&self) -> DriverResult<usize> {
        self.element("[data-testid='chat-message']", "chat message")
            .count()
    }

    /// Text of the whole chat column, not of one message.
    ///
    /// `query` reports the *first* match only, so asking a message selector
    /// for its text silently hides every message after the first - which made
    /// a two-way conversation look like a delivery failure.
    pub fn chat_text(&self) -> DriverResult<String> {
        self.chat_panel_text()
    }

    /// Sends a message the way a user would: type, then press send.
    pub fn send_chat(&self, message: &str) -> DriverResult<()> {
        self.chat_input().type_text(message)?;
        self.chat_send_button().click()
    }

    /// Waits for `text` to appear in any chat message. Delivery goes through
    /// the signaling server and a poll on the receiving side, so this cannot
    /// be asserted synchronously.
    pub fn wait_for_chat_containing(
        &self,
        text: &str,
        timeout: std::time::Duration,
    ) -> DriverResult<()> {
        crate::pom::wait_until(timeout, || {
            self.driver
                .query("[data-testid='chat-panel']", None)
                .ok()
                .and_then(|r| r.text)
                .map(|t| t.contains(text))
                .unwrap_or(false)
        })
        .map_err(|_| {
            format!(
                "no chat message contained {:?} within {:?}; saw {:?}",
                text,
                timeout,
                self.chat_text()
            )
        })
    }

    /// Whole-panel text, for assertions phrased over everything in the chat
    /// column rather than a single message.
    pub fn chat_panel_text(&self) -> DriverResult<String> {
        self.element("[data-testid='chat-panel']", "chat panel")
            .text()
    }
}
