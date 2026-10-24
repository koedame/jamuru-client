//! What a listing of ASIO drivers answers while a session holds one open.
//!
//! Listing a driver means creating and initializing it. Doing that to the driver a session is
//! running on takes it from the session: on ASIO4ALL the session's callbacks drop to zero and do
//! not come back. So while a session holds a driver, a listing is answered from what is known,
//! and no driver is opened for it.

use super::device::AudioDevice;

/// The drivers last found, and the one a session holds open.
pub(crate) struct Roster {
    listed: Vec<AudioDevice>,
    open: Option<Open>,
}

/// A driver a session is opening or has open. Its description is known once it has initialized.
struct Open {
    device: Option<AudioDevice>,
}

impl Roster {
    pub(crate) const fn new() -> Self {
        Self {
            listed: Vec::new(),
            open: None,
        }
    }

    /// Whether a session holds a driver, so that none may be opened to list it.
    pub(crate) fn holds_a_driver(&self) -> bool {
        self.open.is_some()
    }

    /// A session starts opening a driver.
    pub(crate) fn opening(&mut self) {
        self.open = Some(Open { device: None });
    }

    /// The driver the session is opening has initialized and says what it offers.
    pub(crate) fn opened(&mut self, device: AudioDevice) {
        if let Some(open) = &mut self.open {
            open.device = Some(device);
        }
    }

    /// The session let go of its driver, or did not manage to open it.
    pub(crate) fn closed(&mut self) {
        self.open = None;
    }

    /// Every driver was opened and described: this is what the listing found.
    pub(crate) fn found(&mut self, found: Vec<AudioDevice>) {
        self.listed = found;
    }

    /// The answer to a listing while a session holds a driver: the last listing, with the
    /// session's driver in it as it described itself.
    pub(crate) fn while_open(&self) -> Vec<AudioDevice> {
        let mut answer = self.listed.clone();
        if let Some(device) = self.open.as_ref().and_then(|open| open.device.as_ref()) {
            match answer.iter_mut().find(|d| d.id == device.id) {
                Some(listed) => *listed = device.clone(),
                None => answer.push(device.clone()),
            }
        }
        answer
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::DeviceId;

    fn driver(name: &str, channels: u16) -> AudioDevice {
        AudioDevice {
            id: DeviceId(format!("asio:{name}")),
            name: name.to_string(),
            supported_sample_rates: vec![48000],
            supported_channels: vec![channels],
            is_default: false,
            is_asio: true,
            is_bluetooth: false,
        }
    }

    fn names(devices: &[AudioDevice]) -> Vec<&str> {
        devices.iter().map(|d| d.name.as_str()).collect()
    }

    /// Verifies: REQ-AUD-130
    #[test]
    fn no_session_holds_a_driver_before_one_is_opened_and_after_it_is_closed() {
        let mut roster = Roster::new();
        assert!(!roster.holds_a_driver());

        roster.opening();
        assert!(roster.holds_a_driver());

        roster.closed();
        assert!(!roster.holds_a_driver());
    }

    /// Verifies: REQ-AUD-130
    #[test]
    fn a_driver_still_being_opened_already_stops_the_drivers_being_opened_for_a_listing() {
        let mut roster = Roster::new();
        roster.opening();

        assert!(roster.holds_a_driver());
        assert!(roster.while_open().is_empty());
    }

    /// Verifies: REQ-AUD-130
    #[test]
    fn while_a_session_holds_a_driver_the_listing_has_it_and_the_others_as_last_found() {
        let mut roster = Roster::new();
        roster.found(vec![
            driver("ASIO4ALL v2", 2),
            driver("Focusrite USB ASIO", 18),
        ]);
        roster.opening();
        roster.opened(driver("ASIO4ALL v2", 2));

        assert_eq!(
            names(&roster.while_open()),
            ["ASIO4ALL v2", "Focusrite USB ASIO"]
        );
    }

    /// Verifies: REQ-AUD-130
    #[test]
    fn a_driver_never_listed_before_the_session_opened_it_is_in_the_listing_all_the_same() {
        let mut roster = Roster::new();
        roster.opening();
        roster.opened(driver("ASIO4ALL v2", 2));

        assert_eq!(names(&roster.while_open()), ["ASIO4ALL v2"]);
    }

    /// Verifies: REQ-AUD-130
    #[test]
    fn the_session_s_driver_is_listed_as_it_described_itself_when_it_opened() {
        let mut roster = Roster::new();
        roster.found(vec![driver("ASIO4ALL v2", 2)]);
        roster.opening();
        roster.opened(driver("ASIO4ALL v2", 8));

        let listed = roster.while_open();

        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].supported_channels, [8]);
    }

    /// Verifies: REQ-AUD-130
    #[test]
    fn a_new_listing_replaces_what_was_last_found_once_the_session_has_closed() {
        let mut roster = Roster::new();
        roster.found(vec![driver("ASIO4ALL v2", 2)]);
        roster.opening();
        roster.opened(driver("ASIO4ALL v2", 2));
        roster.closed();

        assert!(!roster.holds_a_driver());

        roster.found(vec![driver("Focusrite USB ASIO", 18)]);
        roster.opening();
        assert_eq!(names(&roster.while_open()), ["Focusrite USB ASIO"]);
    }
}
