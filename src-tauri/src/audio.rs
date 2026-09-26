//! Audio devices for the settings (ADR-043)
//!
//! Lists the audio devices on offer. Choosing one, like every other audio
//! setting, goes through [`crate::settings::settings_change`]; the choice in
//! effect is the saved config.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use jamjam::audio::{bounded, list_input_devices, list_output_devices, LIST_TIMEOUT};

/// Audio device information for IPC
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioDeviceInfo {
    pub id: String,
    pub name: String,
    pub supported_sample_rates: Vec<u32>,
    pub supported_channels: Vec<u16>,
    pub is_default: bool,
    pub is_asio: bool,
}

/// One direction's device list, and the call that has hung, if one has
///
/// A driver that has hung never answers the listing, so the settings could not
/// be read while it lasted. The call is made on a thread of its own and waited
/// for a limited time, and what the last successful listing said is shown
/// instead. While a call that hung is still out, no other is made: it would
/// hang the same way.
struct Listing {
    /// How long a listing is waited for
    timeout: Duration,
    /// Set by the call that hung when it finally returns
    hung: Mutex<Option<Arc<AtomicBool>>>,
    last: Mutex<Option<Vec<AudioDeviceInfo>>>,
}

static INPUTS: Listing = Listing::new(LIST_TIMEOUT);
static OUTPUTS: Listing = Listing::new(LIST_TIMEOUT);

/// Marks the listing call as ended when it is dropped, a panic included
struct Ends(Arc<AtomicBool>);

impl Drop for Ends {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

impl Listing {
    const fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            hung: Mutex::new(None),
            last: Mutex::new(None),
        }
    }

    fn list(
        &'static self,
        direction: &'static str,
        enumerate: fn() -> Result<Vec<AudioDeviceInfo>, String>,
    ) -> Result<Vec<AudioDeviceInfo>, String> {
        let mut hung = self.hung.lock().unwrap_or_else(|e| e.into_inner());
        if hung
            .as_ref()
            .is_some_and(|ended| !ended.load(Ordering::SeqCst))
        {
            drop(hung);
            return self.last_listing(direction);
        }
        *hung = None;
        drop(hung);

        let ended = Arc::new(AtomicBool::new(false));
        let ends = Ends(ended.clone());
        let outcome = bounded(
            &format!("{} device listing", direction),
            self.timeout,
            move || {
                let _ends = ends;
                let listed = enumerate();
                // Kept by the call itself, so a listing that comes back late
                // still becomes the last one
                if let Ok(devices) = &listed {
                    *self.last.lock().unwrap_or_else(|e| e.into_inner()) = Some(devices.clone());
                }
                listed
            },
        );
        match outcome {
            Ok(listed) => listed,
            Err(_) => {
                *self.hung.lock().unwrap_or_else(|e| e.into_inner()) = Some(ended);
                self.last_listing(direction)
            }
        }
    }

    fn last_listing(&self, direction: &str) -> Result<Vec<AudioDeviceInfo>, String> {
        match self.last.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            Some(devices) => {
                tracing::warn!(
                    "The audio driver is not answering: showing the {} devices as last listed",
                    direction
                );
                Ok(devices)
            }
            None => Err(format!(
                "The {} devices could not be listed: the audio driver is not answering",
                direction
            )),
        }
    }
}

/// Available input (microphone) devices
pub fn input_devices() -> Result<Vec<AudioDeviceInfo>, String> {
    INPUTS.list("input", list_inputs)
}

/// Available output (speaker) devices
pub fn output_devices() -> Result<Vec<AudioDeviceInfo>, String> {
    OUTPUTS.list("output", list_outputs)
}

fn list_inputs() -> Result<Vec<AudioDeviceInfo>, String> {
    let devices = list_input_devices().map_err(|e| {
        tracing::error!("Listing input devices failed: {}", e);
        e.to_string()
    })?;

    let devices: Vec<AudioDeviceInfo> = devices
        .into_iter()
        .map(|d| AudioDeviceInfo {
            id: d.id.0,
            name: d.name,
            supported_sample_rates: d.supported_sample_rates,
            supported_channels: d.supported_channels,
            is_default: d.is_default,
            is_asio: d.is_asio,
        })
        .collect();
    log_devices("input", &devices);
    Ok(devices)
}

fn list_outputs() -> Result<Vec<AudioDeviceInfo>, String> {
    let devices = list_output_devices().map_err(|e| {
        tracing::error!("Listing output devices failed: {}", e);
        e.to_string()
    })?;

    let devices: Vec<AudioDeviceInfo> = devices
        .into_iter()
        .map(|d| AudioDeviceInfo {
            id: d.id.0,
            name: d.name,
            supported_sample_rates: d.supported_sample_rates,
            supported_channels: d.supported_channels,
            is_default: d.is_default,
            is_asio: d.is_asio,
        })
        .collect();
    log_devices("output", &devices);
    Ok(devices)
}

/// What the OS offered, so a report can show "no interface was found" apart
/// from "the interface was found but not selected". Written when the list
/// differs from the one written last: the settings list the devices on every
/// change, and the same list again says nothing new.
fn log_devices(kind: &str, devices: &[AudioDeviceInfo]) {
    static LAST: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

    let names: Vec<String> = devices
        .iter()
        .map(|d| {
            if d.is_default {
                format!("{} (default)", d.name)
            } else {
                d.name.clone()
            }
        })
        .collect();
    let line = format!("{} {} device(s): {}", devices.len(), kind, names.join(", "));
    if let Ok(mut last) = LAST.lock() {
        match last.iter_mut().find(|(k, _)| k == kind) {
            Some((_, previous)) if *previous == line => return,
            Some((_, previous)) => *previous = line.clone(),
            None => last.push((kind.to_string(), line.clone())),
        }
    }
    tracing::info!("Found {}", line);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    static CALLS: AtomicUsize = AtomicUsize::new(0);
    static DRIVER_HUNG: AtomicBool = AtomicBool::new(false);

    fn a_driver_that_can_hang() -> Result<Vec<AudioDeviceInfo>, String> {
        CALLS.fetch_add(1, Ordering::SeqCst);
        while DRIVER_HUNG.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(vec![AudioDeviceInfo {
            id: "interface".to_string(),
            name: "Interface".to_string(),
            supported_sample_rates: vec![48000],
            supported_channels: vec![2],
            is_default: true,
            is_asio: false,
        }])
    }

    /// Verifies: REQ-AUD-123
    #[test]
    fn when_the_driver_hangs_the_devices_are_shown_as_last_listed_and_not_asked_again() {
        static LISTING: Listing = Listing::new(Duration::from_millis(100));

        let before = LISTING.list("input", a_driver_that_can_hang).unwrap();
        DRIVER_HUNG.store(true, Ordering::SeqCst);
        let started = std::time::Instant::now();
        let while_hung = LISTING.list("input", a_driver_that_can_hang).unwrap();
        let calls_after_the_first_hang = CALLS.load(Ordering::SeqCst);
        let asked_again = LISTING.list("input", a_driver_that_can_hang).unwrap();

        assert_eq!(while_hung, before);
        assert_eq!(asked_again, before);
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(
            CALLS.load(Ordering::SeqCst),
            calls_after_the_first_hang,
            "a listing made while one is hung would only hang too"
        );

        DRIVER_HUNG.store(false, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(300));
        LISTING.list("input", a_driver_that_can_hang).unwrap();
        assert!(
            CALLS.load(Ordering::SeqCst) > calls_after_the_first_hang,
            "once the hung call has returned the driver is asked again"
        );
    }

    /// Verifies: REQ-AUD-123
    #[test]
    fn when_the_driver_hangs_before_any_listing_it_says_so_instead_of_showing_no_devices() {
        static LISTING: Listing = Listing::new(Duration::from_millis(100));
        fn never() -> Result<Vec<AudioDeviceInfo>, String> {
            std::thread::sleep(Duration::from_secs(3));
            Ok(Vec::new())
        }

        let listed = LISTING.list("output", never);

        assert!(listed.unwrap_err().contains("not answering"));
    }
}
