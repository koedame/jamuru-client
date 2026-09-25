//! A single UI element, addressed the way a user perceives it.
//!
//! Every method answers a question a user could answer by looking at the
//! screen ("is it there?", "what does it say?", "can I click it?"), which is
//! what keeps scenarios readable and free of selectors.

use super::driver::{Driver, DriverResult};

pub struct Element<'a> {
    driver: &'a Driver,
    selector: String,
    window: Option<String>,
    /// Human-readable name used in failure messages, e.g. "Create Room button".
    name: &'static str,
}

impl<'a> Element<'a> {
    pub(crate) fn new(
        driver: &'a Driver,
        selector: impl Into<String>,
        window: Option<String>,
        name: &'static str,
    ) -> Self {
        Self {
            driver,
            selector: selector.into(),
            window,
            name,
        }
    }

    fn window(&self) -> Option<&str> {
        self.window.as_deref()
    }

    pub fn exists(&self) -> DriverResult<bool> {
        Ok(self.driver.query(&self.selector, self.window())?.exists)
    }

    /// Present in the DOM *and* rendered. `exists` alone would pass for an
    /// element hidden behind `display: none`, which the user cannot see.
    pub fn is_visible(&self) -> DriverResult<bool> {
        let result = self.driver.query(&self.selector, self.window())?;
        Ok(result.exists && result.visible)
    }

    /// Whether a user could operate it: present, rendered, and not disabled.
    pub fn is_enabled(&self) -> DriverResult<bool> {
        let result = self.driver.query(&self.selector, self.window())?;
        Ok(result.exists && result.visible && !result.disabled)
    }

    pub fn text(&self) -> DriverResult<String> {
        let result = self.driver.query(&self.selector, self.window())?;
        result
            .text
            .ok_or_else(|| format!("{} is not present", self.name))
    }

    pub fn value(&self) -> DriverResult<String> {
        let result = self.driver.query(&self.selector, self.window())?;
        result
            .value
            .ok_or_else(|| format!("{} has no value", self.name))
    }

    /// Labels of the choices a `<select>` offers, in order. A disabled
    /// option (a "Select device" placeholder) is not a choice and is left out.
    pub fn option_labels(&self) -> DriverResult<Vec<String>> {
        Ok(self
            .driver
            .query(&self.selector, self.window())?
            .options
            .into_iter()
            .filter(|o| !o.disabled)
            .map(|o| o.label)
            .collect())
    }

    /// Values of the choices a `<select>` offers, in order. Values are what
    /// `select_value` takes - labels are for humans. Disabled options are
    /// left out: a user cannot pick them.
    pub fn option_values(&self) -> DriverResult<Vec<String>> {
        Ok(self
            .driver
            .query(&self.selector, self.window())?
            .options
            .into_iter()
            .filter(|o| !o.disabled)
            .map(|o| o.value)
            .collect())
    }

    /// Picks an option by value. Fails if the `<select>` does not offer it,
    /// because assigning an absent value leaves the old selection in place -
    /// which would look like a successful change.
    pub fn select_value(&self, value: &str) -> DriverResult<()> {
        self.driver
            .input(&self.selector, value, self.window())
            .map_err(|e| format!("could not choose {:?} in {}: {}", value, self.name, e))
    }

    /// One attribute of the element. Used where a component publishes a
    /// machine-readable value next to the text it renders for the user.
    pub fn attribute(&self, name: &str) -> DriverResult<Option<String>> {
        self.driver.attribute(&self.selector, name, self.window())
    }

    pub fn count(&self) -> DriverResult<usize> {
        Ok(self.driver.query(&self.selector, self.window())?.count)
    }

    pub fn click(&self) -> DriverResult<()> {
        self.driver
            .click(&self.selector, self.window())
            .map_err(|e| format!("could not click {}: {}", self.name, e))
    }

    pub fn type_text(&self, value: &str) -> DriverResult<()> {
        self.driver
            .input(&self.selector, value, self.window())
            .map_err(|e| format!("could not type into {}: {}", self.name, e))
    }

    /// Waits until the element is visible. UI transitions are asynchronous
    /// (IPC round-trips, React re-renders), so polling is the only honest
    /// way to assert on a post-action state.
    pub fn wait_until_visible(&self, timeout: std::time::Duration) -> DriverResult<()> {
        super::wait_for(timeout, || self.is_visible().unwrap_or(false))
            .map_err(|_| format!("{} did not become visible within {:?}", self.name, timeout))
    }

    pub fn wait_until_gone(&self, timeout: std::time::Duration) -> DriverResult<()> {
        super::wait_for(timeout, || !self.is_visible().unwrap_or(true))
            .map_err(|_| format!("{} was still visible after {:?}", self.name, timeout))
    }
}
