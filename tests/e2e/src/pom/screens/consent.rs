//! The screen a first launch starts on: the terms of use and the two boxes
//! that must be checked before the app goes on.

use crate::pom::driver::{Driver, DriverResult};
use crate::pom::element::Element;

pub struct ConsentScreen<'a> {
    driver: &'a Driver,
}

impl<'a> ConsentScreen<'a> {
    pub(crate) fn new(driver: &'a Driver) -> Self {
        Self { driver }
    }

    fn element(&self, selector: &'static str, name: &'static str) -> Element<'a> {
        Element::new(self.driver, selector, None, name)
    }

    pub fn wait_until_displayed(&self, timeout: std::time::Duration) -> DriverResult<()> {
        self.element("[data-testid='consent-screen']", "consent screen")
            .wait_until_visible(timeout)
    }

    pub fn is_displayed(&self) -> DriverResult<bool> {
        self.element("[data-testid='consent-screen']", "consent screen")
            .is_visible()
    }

    /// The line saying other people in the room can see the user's IP address.
    pub fn ip_notice(&self) -> Element<'a> {
        self.element("[data-testid='consent-ip-notice']", "IP address notice")
    }

    pub fn agree_box(&self) -> Element<'a> {
        self.element("[data-testid='consent-agree']", "agree box")
    }

    pub fn adult_box(&self) -> Element<'a> {
        self.element("[data-testid='consent-adult']", "age box")
    }

    pub fn start_button(&self) -> Element<'a> {
        self.element("[data-testid='consent-start']", "agree and start button")
    }

    /// Checks both boxes and presses the button, as a user who agrees does.
    pub fn agree(&self) -> DriverResult<()> {
        self.agree_box().click()?;
        self.adult_box().click()?;
        self.start_button().click()
    }
}
