//! The settings window. A separate Tauri window, so every lookup targets
//! the `settings` label rather than the main window.

use crate::pom::driver::{Driver, DriverResult};
use crate::pom::element::Element;

use super::windows;

const ROOT: &str = "[data-testid='settings-panel']";

/// The tabs a user can switch between.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    General,
    Profile,
    Devices,
    Diagnostics,
}

impl SettingsTab {
    /// Matches the `id` the tab list renders (`tab-<id>` in VerticalTabs).
    fn id(self) -> &'static str {
        match self {
            SettingsTab::General => "general",
            SettingsTab::Profile => "profile",
            SettingsTab::Devices => "devices",
            SettingsTab::Diagnostics => "diagnostics",
        }
    }

    pub fn all() -> [SettingsTab; 4] {
        [
            SettingsTab::General,
            SettingsTab::Profile,
            SettingsTab::Devices,
            SettingsTab::Diagnostics,
        ]
    }
}

pub struct SettingsScreen<'a> {
    driver: &'a Driver,
}

impl<'a> SettingsScreen<'a> {
    pub(crate) fn new(driver: &'a Driver) -> Self {
        Self { driver }
    }

    fn element(&self, selector: impl Into<String>, name: &'static str) -> Element<'a> {
        Element::new(
            self.driver,
            selector,
            Some(windows::SETTINGS.to_string()),
            name,
        )
    }

    /// Whether the settings window exists at all. Checked before touching
    /// its contents, because querying a window that was never opened is a
    /// different failure than an empty window.
    pub fn is_open(&self) -> DriverResult<bool> {
        Ok(self
            .driver
            .open_windows()?
            .iter()
            .any(|label| label == windows::SETTINGS))
    }

    pub fn wait_until_open(&self, timeout: std::time::Duration) -> DriverResult<()> {
        crate::pom::wait_for(timeout, || self.is_open().unwrap_or(false))
            .map_err(|_| format!("settings window did not open within {:?}", timeout))?;
        self.element(ROOT, "settings panel")
            .wait_until_visible(timeout)
    }

    pub fn is_displayed(&self) -> DriverResult<bool> {
        self.element(ROOT, "settings panel").is_visible()
    }

    // --- tabs ---

    pub fn tab(&self, tab: SettingsTab) -> Element<'a> {
        self.element(format!("#tab-{}", tab.id()), "settings tab")
    }

    pub fn select_tab(&self, tab: SettingsTab) -> DriverResult<()> {
        self.tab(tab).click()
    }

    /// Which tab the user currently has selected, read from `aria-selected`
    /// so it reflects what assistive tech (and the user) perceives.
    pub fn selected_tab(&self) -> DriverResult<Option<SettingsTab>> {
        for tab in SettingsTab::all() {
            let selector = format!("#tab-{}[aria-selected='true']", tab.id());
            if self
                .driver
                .query(&selector, Some(windows::SETTINGS))?
                .exists
            {
                return Ok(Some(tab));
            }
        }
        Ok(None)
    }

    /// Text of the panel currently shown next to the tab list.
    pub fn visible_text(&self) -> DriverResult<String> {
        self.element(ROOT, "settings panel").text()
    }

    pub fn window_html(&self) -> DriverResult<String> {
        self.driver.dom(Some(windows::SETTINGS))
    }

    /// Controls of the General tab. Select that tab before using them.
    pub fn general_tab(&self) -> GeneralTab<'a> {
        GeneralTab::new(self.driver)
    }

    /// Controls of the Devices tab. Select that tab before using them.
    pub fn devices_tab(&self) -> DevicesTab<'a> {
        DevicesTab::new(self.driver)
    }

    /// Controls of the Diagnostics tab. Select that tab before using them.
    pub fn diagnostics_tab(&self) -> DiagnosticsTab<'a> {
        DiagnosticsTab::new(self.driver)
    }
}

/// The Diagnostics tab's log file section (ADR-036).
pub struct DiagnosticsTab<'a> {
    driver: &'a Driver,
}

impl<'a> DiagnosticsTab<'a> {
    pub(crate) fn new(driver: &'a Driver) -> Self {
        Self { driver }
    }

    fn element(&self, selector: &'static str, name: &'static str) -> Element<'a> {
        Element::new(
            self.driver,
            selector,
            Some(windows::SETTINGS.to_string()),
            name,
        )
    }

    pub fn open_log_folder_button(&self) -> Element<'a> {
        self.element(
            "[data-testid='diagnostics-open-log-folder']",
            "open log folder button",
        )
    }

    /// The usage reporting switch. `data-checked` says whether it is on.
    pub fn usage_reporting_switch(&self) -> Element<'a> {
        self.element(
            "[data-testid='diagnostics-usage-toggle']",
            "usage reporting switch",
        )
    }

    pub fn is_usage_reporting_on(&self) -> DriverResult<bool> {
        Ok(self
            .usage_reporting_switch()
            .attribute("data-checked")?
            .as_deref()
            == Some("true"))
    }

    /// The button that reads what would be sent.
    pub fn show_usage_button(&self) -> Element<'a> {
        self.element(
            "[data-testid='diagnostics-usage-show']",
            "show what is sent button",
        )
    }

    /// What the button revealed: the lines, or the note that there are none.
    pub fn usage_preview(&self) -> Element<'a> {
        self.element("[data-testid='diagnostics-usage-preview']", "what is sent")
    }

    /// The button, the folder that was opened and the error if it could not be.
    pub fn log_file_section(&self) -> Element<'a> {
        self.element("[data-testid='diagnostics-log-file']", "log file section")
    }
}

/// The General tab's controls. Separate from [`SettingsScreen`] because these
/// only exist while that tab is selected.
pub struct GeneralTab<'a> {
    driver: &'a Driver,
}

impl<'a> GeneralTab<'a> {
    pub(crate) fn new(driver: &'a Driver) -> Self {
        Self { driver }
    }

    fn element(&self, selector: &'static str, name: &'static str) -> Element<'a> {
        Element::new(
            self.driver,
            selector,
            Some(windows::SETTINGS.to_string()),
            name,
        )
    }

    /// The signaling server override field. Empty clears it (uses the build
    /// default).
    pub fn server_url_input(&self) -> Element<'a> {
        self.element("#server-url-input", "server URL field")
    }

    /// The language dropdown ("ja" / "en").
    pub fn language_select(&self) -> Element<'a> {
        self.element("#language-select", "language dropdown")
    }
}

/// The Devices tab's controls. Separate from [`SettingsScreen`] because these
/// only exist while that tab is selected, so treating them as always-present
/// would make a wrong-tab mistake look like a missing element.
pub struct DevicesTab<'a> {
    driver: &'a Driver,
}

impl<'a> DevicesTab<'a> {
    pub(crate) fn new(driver: &'a Driver) -> Self {
        Self { driver }
    }

    fn element(&self, selector: &'static str, name: &'static str) -> Element<'a> {
        Element::new(
            self.driver,
            selector,
            Some(windows::SETTINGS.to_string()),
            name,
        )
    }

    pub fn input_device_select(&self) -> Element<'a> {
        self.element("#input-device", "input device dropdown")
    }

    pub fn output_device_select(&self) -> Element<'a> {
        self.element("#output-device", "output device dropdown")
    }

    pub fn sample_rate_select(&self) -> Element<'a> {
        self.element("#sample-rate", "sample rate dropdown")
    }

    pub fn buffer_size_select(&self) -> Element<'a> {
        self.element("#buffer-size", "buffer size dropdown")
    }
}
