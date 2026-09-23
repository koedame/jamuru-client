//! Which settings go into the log.
//!
//! There is no list of settings that are sent: the whole settings file goes
//! in, so a setting added later is sent without anyone remembering to add it.
//! What is left out is named instead, and those names are the whole privacy
//! boundary for settings.
//!
//! When a setting holds a name, a URL, an ID or a path, add it to
//! [`LEFT_OUT`]. `left_out_settings_never_reach_the_line` is the test that
//! shows what a reader of the log would see for those.

use serde_json::{Map, Value};

use crate::config::AppConfig;

/// Settings that are never sent.
///
/// - `peer_name`: the user's display name
/// - `connection_history`: rooms the user has been in
/// - `server_url`: a server of their own, which can carry credentials
/// - `input_device_id` / `output_device_id`: identify the user's audio setup
pub const LEFT_OUT: [&str; 5] = [
    "peer_name",
    "connection_history",
    "server_url",
    "input_device_id",
    "output_device_id",
];

/// Longest string value that is sent (the schema's limit).
const MAX_STRING_LEN: usize = 128;

/// The settings as sent: every item of the settings file except [`LEFT_OUT`].
///
/// The log holds flat scalars only, so an item that is a list or a table, or
/// a string longer than the schema allows, is dropped instead of making the
/// whole line invalid.
pub fn settings_for_report(config: &AppConfig) -> Map<String, Value> {
    let Ok(Value::Object(items)) = serde_json::to_value(config) else {
        return Map::new();
    };
    items
        .into_iter()
        .filter(|(name, _)| !LEFT_OUT.contains(&name.as_str()))
        .filter(|(_, value)| match value {
            Value::String(s) => s.chars().count() <= MAX_STRING_LEN,
            Value::Array(_) | Value::Object(_) => false,
            _ => true,
        })
        .collect()
}
