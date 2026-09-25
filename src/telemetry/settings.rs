//! Which settings go into the log.
//!
//! There is no list of settings that are sent: the whole settings file goes
//! in, so a setting added later is sent without anyone remembering to add it.
//! What is left out is named instead, and those names are the whole privacy
//! boundary for settings.
//!
//! The user has agreed to reporting, so what helps to find a fault is sent,
//! addresses and device IDs included. What is left out is what names the user
//! or a room they were in, and what is sent somewhere else. A URL goes in as its scheme, host and port only:
//! whatever sits in front of the host (credentials) or behind it (path, query,
//! fragment) can be a secret.
//!
//! When a setting holds a person's name or something that names a room, add it
//! to [`LEFT_OUT`]. `left_out_settings_never_reach_the_line` is the test that
//! shows what a reader of the log would see for those.

use serde_json::{Map, Value};

use crate::config::AppConfig;

/// Settings that are never sent.
///
/// - `peer_name`: the user's display name
/// - `connection_history`: rooms the user has been in
/// - `input_device_id` / `output_device_id`: sent in `audio_env` instead, so
///   that choosing a device is one change and one line, not two
pub const LEFT_OUT: [&str; 4] = [
    "peer_name",
    "connection_history",
    "input_device_id",
    "output_device_id",
];

/// The setting that holds a URL. It is sent as [`origin_of`] its value.
const SERVER_URL: &str = "server_url";

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
        .filter_map(|(name, value)| match (name.as_str(), &value) {
            (SERVER_URL, Value::String(url)) => {
                origin_of(url).map(|origin| (name, Value::String(origin)))
            }
            _ => Some((name, value)),
        })
        .filter(|(_, value)| match value {
            Value::String(s) => s.chars().count() <= MAX_STRING_LEN,
            Value::Array(_) | Value::Object(_) => false,
            _ => true,
        })
        .collect()
}

/// The scheme, host and port of `url`: `https://user:pw@example.com:8443/a?b#c`
/// becomes `https://example.com:8443`. `None` when it has no `scheme://` or no host.
pub(super) fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    (!scheme.is_empty() && !host.is_empty()).then(|| format!("{scheme}://{host}"))
}
