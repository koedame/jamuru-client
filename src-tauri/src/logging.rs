//! Diagnostic log file (ADR-036).
//!
//! Every build, including the one users install, writes `jamuru.log` to the
//! OS log directory. A release app has no console and no developer tools, so
//! this file is the only way to see what the app did when something does not
//! work. Rust `tracing` events, the core library's events and the webview's
//! `console` output all end up in it.
//!
//! The webview reaches the file through [`log_frontend`], an app command, and
//! not through tauri-plugin-log's own JavaScript command: plugin commands need
//! a capability, and the log has to keep working when the capabilities are
//! exactly what is broken.

use std::collections::HashMap;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{plugin::TauriPlugin, AppHandle, Manager, Runtime};
use tauri_plugin_log::{log, RotationStrategy, Target, TargetKind};

use crate::config::{AppConfig, ConfigState};

/// `jamuru.log` in the OS log directory.
const LOG_FILE_STEM: &str = "jamuru";

/// A file is rotated once a write would push it past this size.
const MAX_FILE_BYTES: u128 = 5 * 1024 * 1024;

/// The active file plus two older generations.
const KEPT_FILES: usize = 3;

/// Overrides the levels, e.g. `JAMURU_LOG=trace` or `JAMURU_LOG=info,jamuru=trace`.
pub const ENV_VAR: &str = "JAMURU_LOG";

/// Target of the lines that come from the webview.
const WEBVIEW_TARGET: &str = "webview";

/// Targets that are this project's own code (the core library, this crate and
/// the webview). They log at debug by default, dependencies at info.
const OWN_TARGETS: [&str; 3] = ["jamuru", "jamjam_app_lib", WEBVIEW_TARGET];

/// One console call can carry a whole array or DOM dump. Without a cap a single
/// line could rotate the useful history out of the file.
const MAX_WEBVIEW_MESSAGE_BYTES: usize = 8 * 1024;

/// One call site (a `tracing` macro in the source) writes at most this many
/// records per window; the rest are counted. A stream that fails on every
/// packet would otherwise write hundreds of lines a second and rotate the
/// useful history out of the file within a minute.
const SITE_WINDOW: Duration = Duration::from_secs(10);
const RECORDS_PER_SITE_WINDOW: u32 = 5;

const MASK: &str = "***";

/// Keys whose values must never reach the file.
const SECRET_KEYS: [&str; 4] = ["password", "passwd", "secret", "token"];

/// Set to `off` to write STUN/candidate addresses and audio device ids
/// unmasked, for when a NAT/LAN fallback bug needs the real values.
/// Default is to mask them: the file is the thing users are told to attach
/// to a bug report, most often a public GitHub issue.
pub const REDACT_ENV_VAR: &str = "JAMURU_LOG_REDACT";

pub(crate) fn redaction_enabled() -> bool {
    parse_redact_flag(std::env::var(REDACT_ENV_VAR).ok().as_deref())
}

fn parse_redact_flag(value: Option<&str>) -> bool {
    !value.is_some_and(|v| v.eq_ignore_ascii_case("off"))
}

/// Which targets log at which level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogSpec {
    default: log::LevelFilter,
    targets: Vec<(String, log::LevelFilter)>,
    /// Parts of `JAMURU_LOG` that are not a level or `target=level`.
    ignored: Vec<String>,
}

impl LogSpec {
    /// Own code at debug, everything else at info.
    pub fn defaults() -> Self {
        Self {
            default: log::LevelFilter::Info,
            targets: OWN_TARGETS
                .iter()
                .map(|t| (t.to_string(), log::LevelFilter::Debug))
                .collect(),
            ignored: Vec::new(),
        }
    }

    pub fn from_env() -> Self {
        Self::parse(std::env::var(ENV_VAR).ok().as_deref())
    }

    /// Comma-separated parts. A bare level sets every target, this project's own
    /// included; `target=level` sets one target and wins over the bare level
    /// wherever the two are written (`jamuru=trace,info` and `info,jamuru=trace`
    /// mean the same).
    pub fn parse(spec: Option<&str>) -> Self {
        let mut parsed = Self::defaults();
        let mut bare = None;
        let mut named = Vec::new();
        for part in spec.unwrap_or("").split(',').map(str::trim) {
            if part.is_empty() {
                continue;
            }
            match part.split_once('=') {
                None => match part.parse::<log::LevelFilter>() {
                    Ok(level) => bare = Some(level),
                    Err(_) => parsed.ignored.push(part.to_string()),
                },
                Some((target, level)) => match level.trim().parse::<log::LevelFilter>() {
                    Ok(level) if !target.trim().is_empty() => {
                        named.push((target.trim().to_string(), level))
                    }
                    _ => parsed.ignored.push(part.to_string()),
                },
            }
        }
        if let Some(level) = bare {
            parsed.default = level;
            for (_, own) in parsed.targets.iter_mut() {
                *own = level;
            }
        }
        for (target, level) in named {
            parsed.set_target(&target, level);
        }
        parsed
    }

    fn set_target(&mut self, target: &str, level: log::LevelFilter) {
        match self.targets.iter_mut().find(|(t, _)| t == target) {
            Some((_, existing)) => *existing = level,
            None => self.targets.push((target.to_string(), level)),
        }
    }

    fn describe(&self) -> String {
        let targets: Vec<String> = self
            .targets
            .iter()
            .map(|(target, level)| format!("{}={}", target, level))
            .collect();
        format!("default={} {}", self.default, targets.join(" "))
    }
}

struct Site {
    window_started: Duration,
    written: u32,
    held_back: u32,
}

/// Limits how often one call site can write (see [`SITE_WINDOW`]).
///
/// The webview's lines are not limited here: the webview has its own limiter
/// (`ui/src/lib/logging.ts`) that also saves the IPC call.
struct SiteLimiter {
    inner: Box<dyn log::Log>,
    now: Box<dyn Fn() -> Duration + Send + Sync>,
    sites: Mutex<HashMap<(String, u32, log::Level), Site>>,
}

impl SiteLimiter {
    fn new(inner: Box<dyn log::Log>) -> Self {
        let started = Instant::now();
        Self::with_clock(inner, Box::new(move || started.elapsed()))
    }

    fn with_clock(inner: Box<dyn log::Log>, now: Box<dyn Fn() -> Duration + Send + Sync>) -> Self {
        Self {
            inner,
            now,
            sites: Mutex::new(HashMap::new()),
        }
    }

    /// Whether this record may be written, and how many records from the same
    /// site were held back before it.
    fn admit(&self, record: &log::Record) -> Option<u32> {
        let key = (
            record.file().unwrap_or_default().to_string(),
            record.line().unwrap_or_default(),
            record.level(),
        );
        let now = (self.now)();
        let mut sites = self.sites.lock().unwrap_or_else(|e| e.into_inner());
        let site = sites.entry(key).or_insert(Site {
            window_started: now,
            written: 0,
            held_back: 0,
        });

        if now.saturating_sub(site.window_started) >= SITE_WINDOW {
            let held = std::mem::take(&mut site.held_back);
            site.window_started = now;
            site.written = 1;
            return Some(held);
        }
        if site.written < RECORDS_PER_SITE_WINDOW {
            site.written += 1;
            return Some(0);
        }
        site.held_back += 1;
        None
    }
}

impl log::Log for SiteLimiter {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        self.inner.enabled(metadata)
    }

    fn log(&self, record: &log::Record) {
        if record.target() == WEBVIEW_TARGET {
            return self.inner.log(record);
        }
        let Some(held_back) = self.admit(record) else {
            return;
        };
        if held_back > 0 {
            // `record.file()` is the compiler's `file!()`, which on a CI
            // build is the absolute path of the build machine's checkout
            // (e.g. `/Users/runner/work/.../connection.rs`). That path means
            // nothing to whoever reads a pasted log, so only the file name
            // is kept.
            let file_name = record
                .file()
                .and_then(|f| f.rsplit(['/', '\\']).next())
                .unwrap_or("?");
            self.inner.log(
                &log::Record::builder()
                    .args(format_args!(
                        "{} similar record(s) from {}:{} were held back",
                        held_back,
                        file_name,
                        record.line().unwrap_or(0)
                    ))
                    .level(record.level())
                    .target(record.target())
                    .build(),
            );
        }
        self.inner.log(record);
    }

    fn flush(&self) {
        self.inner.flush();
    }
}

fn builder(spec: &LogSpec) -> tauri_plugin_log::Builder {
    let file = Target::new(TargetKind::LogDir {
        file_name: Some(LOG_FILE_STEM.to_string()),
    });
    // A console exists for `cargo tauri dev` and for the GUI E2E harness, which
    // reads the app's stderr when a scenario fails (ADR-025).
    #[cfg(any(debug_assertions, feature = "e2e-control"))]
    let targets = vec![file, Target::new(TargetKind::Stderr)];
    #[cfg(not(any(debug_assertions, feature = "e2e-control")))]
    let targets = vec![file];

    let redact = redaction_enabled();
    let mut builder = tauri_plugin_log::Builder::new()
        .targets(targets)
        .rotation_strategy(RotationStrategy::KeepSome(KEPT_FILES))
        .max_file_size(MAX_FILE_BYTES)
        .level(spec.default)
        // UTC with a `Z` and milliseconds: unambiguous when a user pastes the
        // file, and precise enough to read connect timings from it.
        .format(move |out, message, record| {
            let message = redact_network_identifiers(&message.to_string(), redact);
            let message = redact_personal_identifiers(&message, redact);
            out.finish(format_args!(
                "{} {:<5} [{}] {}",
                chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                record.level(),
                record.target(),
                message
            ))
        });
    for (target, level) in &spec.targets {
        builder = builder.level_for(target.clone(), *level);
    }
    builder
}

/// Installs the logger while the app is being built.
///
/// Not tauri-plugin-log's own plugin: that one aborts the app when the log
/// directory cannot be created, and a diagnostic feature must not be able to
/// keep the app from starting. Without a log file the app runs as before.
pub fn init<R: Runtime>(spec: LogSpec) -> TauriPlugin<R> {
    tauri::plugin::Builder::new("jamuru-log")
        .setup(move |app, _api| {
            match builder(&spec).split(app) {
                Ok((_, max_level, logger)) => {
                    let logger = Box::new(SiteLimiter::new(logger));
                    if let Err(e) = tauri_plugin_log::attach_logger(max_level, logger) {
                        eprintln!("jamuru: another logger is already installed: {}", e);
                    } else {
                        install_panic_hook();
                    }
                }
                Err(e) => eprintln!("jamuru: could not open the log file: {}", e),
            }
            Ok(())
        })
        .build()
}

/// A panic in a command or a task otherwise only prints to a stderr nobody
/// sees in an installed app.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        tracing::error!(
            "panic in thread '{}': {}",
            thread.name().unwrap_or("<unnamed>"),
            info
        );
        previous(info);
    }));
}

/// The first lines of a session: what build this is, where it runs, where the
/// log is and which settings were in effect.
pub fn log_startup<R: Runtime>(app: &AppHandle<R>, spec: &LogSpec) {
    tracing::info!(
        "jamuru {} starting (os={} arch={})",
        app.package_info().version,
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    match app.path().app_log_dir() {
        Ok(dir) => tracing::info!(
            "log file: {} (levels: {})",
            dir.join(format!("{}.log", LOG_FILE_STEM)).display(),
            spec.describe()
        ),
        Err(e) => tracing::warn!("log directory is unknown: {}", e),
    }
    for part in &spec.ignored {
        tracing::warn!("Ignoring '{}' in {}", part, ENV_VAR);
    }
    match app.state::<ConfigState>().get() {
        Ok(config) => tracing::info!("config: {}", summarize_config(&config)),
        Err(e) => tracing::error!("config could not be read: {}", e),
    }
}

/// Lists the settings worth reading in a bug report. Named fields rather than
/// the config's `Debug` output: a field added to the config later must not
/// reach the file until someone decides it belongs there.
fn summarize_config(config: &AppConfig) -> String {
    let redact = redaction_enabled();
    let device = |id: &Option<String>| {
        let name = id.clone().unwrap_or_else(|| "system default".to_string());
        redact_device_id(&name, redact)
    };
    let channel = |l: u32, r: Option<u32>| match r {
        Some(r) => format!("{}/{}", l, r),
        None => format!("{}", l),
    };
    redact_secrets(&format!(
        "server={} input={:?} output={:?} buffer={} sample_rate={} preset={:?} \
         input_channels={} output_channels={} transmit_channels={}",
        strip_userinfo(config.effective_server_url()),
        device(&config.input_device_id),
        device(&config.output_device_id),
        config.buffer_size,
        config.sample_rate,
        config.preset,
        channel(config.input_channel_l, config.input_channel_r),
        channel(config.output_channel_l, config.output_channel_r),
        config.transmit_channels,
    ))
}

/// `scheme://user:pass@host/x` -> `scheme://host/x`.
pub fn strip_userinfo(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    match rest[..authority_end].rfind('@') {
        Some(at) => format!("{}://{}", scheme, &rest[at + 1..]),
        None => url.to_string(),
    }
}

/// Replaces the value of every `password`, `passwd`, `secret` and `token` key
/// (`password=x`, `"roomPassword": "x"`, `?token=x`) with `***`.
///
/// The webview can print anything, including a room password inside an object
/// it logs. The file is the thing users send to someone else, so this is
/// applied where the text enters it. It recognises `key: value` and `key=value`
/// only: free text such as `password hunter2` is left alone, because masking
/// the word after "password" in every sentence would hide the messages this
/// file exists to keep. The desktop app has no way to enter a room password
/// today; this is here for when it does.
pub fn redact_secrets(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut search_from = 0;
    while let Some(key_end) = next_secret_key_end(&lower, search_from) {
        match secret_value_range(text, key_end) {
            Some((start, end)) => {
                out.push_str(&text[copied..start]);
                out.push_str(MASK);
                copied = end;
                search_from = end;
            }
            None => search_from = key_end,
        }
    }
    out.push_str(&text[copied..]);
    out
}

fn next_secret_key_end(lower: &str, from: usize) -> Option<usize> {
    SECRET_KEYS
        .iter()
        .filter_map(|key| lower[from..].find(key).map(|at| from + at + key.len()))
        .min()
}

/// Where the value of a secret key sits, given the end of the key.
fn secret_value_range(text: &str, key_end: usize) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut i = key_end;
    // The rest of the name (`password_hash`, `roomPasswordConfirm`) and the
    // closing quote of a JSON key.
    while i < bytes.len()
        && (bytes[i].is_ascii_alphanumeric()
            || matches!(bytes[i], b'_' | b'-' | b'"' | b'\'' | b'\\'))
    {
        i += 1;
    }
    while bytes.get(i) == Some(&b' ') {
        i += 1;
    }
    if !matches!(bytes.get(i), Some(b':' | b'=')) {
        return None;
    }
    i += 1;
    while bytes.get(i) == Some(&b' ') {
        i += 1;
    }

    let (start, end) = match bytes.get(i) {
        Some(&quote) if quote == b'"' || quote == b'\'' => {
            let start = i + 1;
            let mut end = start;
            while end < bytes.len() && bytes[end] != quote {
                end += if bytes[end] == b'\\' { 2 } else { 1 };
            }
            (start, end.min(bytes.len()))
        }
        _ => {
            let mut end = i;
            while end < bytes.len()
                && !matches!(
                    bytes[end],
                    b' ' | b',' | b';' | b'&' | b')' | b'}' | b']' | b'\n'
                )
            {
                end += 1;
            }
            (i, end)
        }
    };
    (start < end).then_some((start, end))
}

/// Masks IPv4 and IPv6 literals anywhere in a rendered log line: the last
/// IPv4 octet, and the IPv6 interface identifier (the lower 64 bits, which
/// an EUI-64 link-local address derives from the network card's MAC
/// address). The network prefix and the port number survive, since NAT/LAN
/// fallback issues are diagnosed from whether the candidate is global, LAN
/// or link-local, not from the exact host part.
///
/// `enabled` is threaded in rather than read from the environment here so
/// this stays a pure function: [`redaction_enabled`] reads `JAMURU_LOG_REDACT`.
fn redact_network_identifiers(text: &str, enabled: bool) -> String {
    if !enabled {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        if let Some((len, replacement)) = match_ipv6(text, i).or_else(|| match_ipv4(text, i)) {
            out.push_str(&replacement);
            i += len;
        } else {
            let ch = text[i..]
                .chars()
                .next()
                .expect("i is a char boundary within text");
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

fn match_ipv4(text: &str, start: usize) -> Option<(usize, String)> {
    let end = run_end(text, start, |c| c.is_ascii_digit() || c == '.');
    let addr: Ipv4Addr = text[start..end].parse().ok()?;
    let o = addr.octets();
    Some((end - start, format!("{}.{}.{}.x", o[0], o[1], o[2])))
}

fn match_ipv6(text: &str, start: usize) -> Option<(usize, String)> {
    let end = run_end(text, start, |c| c.is_ascii_hexdigit() || c == ':');
    let candidate = &text[start..end];
    if candidate.matches(':').count() < 2 {
        // Rejects timestamps and other digit:digit text before it is even
        // parsed: a real IPv6 literal always has at least two colons.
        return None;
    }
    let addr: Ipv6Addr = candidate.parse().ok()?;
    let s = addr.segments();
    Some((
        end - start,
        format!("{:x}:{:x}:{:x}:{:x}:x:x:x:x", s[0], s[1], s[2], s[3]),
    ))
}

/// End of the maximal run of `pred`-matching chars starting at `start`
/// (which must be a char boundary).
fn run_end(text: &str, start: usize, pred: impl Fn(char) -> bool) -> usize {
    text[start..]
        .char_indices()
        .find(|(_, c)| !pred(*c))
        .map(|(i, _)| start + i)
        .unwrap_or(text.len())
}

/// What the app has learned about the people and rooms it is talking to, so
/// that the file can leave their values out wherever they turn up in a line
/// (ADR-065). The log sites do not have to know which of their arguments are
/// somebody's: a value registered here is replaced wherever it appears.
#[derive(Default)]
struct Known {
    /// Room IDs and invite codes, kept as the server wrote them.
    rooms: Vec<String>,
    /// Display names of everybody in the room, this app's own included. A
    /// name is replaced by `peer<n>`, `n` being its place in this list.
    names: Vec<String>,
    /// Device IDs and names with what to write instead.
    devices: Vec<(String, String)>,
}

static KNOWN: Mutex<Known> = Mutex::new(Known {
    rooms: Vec::new(),
    names: Vec::new(),
    devices: Vec::new(),
});

fn known() -> std::sync::MutexGuard<'static, Known> {
    KNOWN.lock().unwrap_or_else(|e| e.into_inner())
}

/// The shortest registered value that is replaced anywhere in a line. A one
/// character name would rewrite ordinary words.
const MIN_KNOWN_CHARS: usize = 2;

/// A room ID or an invite code that must not reach the file whole.
pub fn register_room(room_id: &str, invite_code: Option<&str>) {
    let mut known = known();
    for value in std::iter::once(room_id).chain(invite_code) {
        if value.chars().count() >= MIN_KNOWN_CHARS && !known.rooms.iter().any(|r| r == value) {
            known.rooms.push(value.to_string());
        }
    }
}

/// A display name (the other participants' and this app's own).
pub fn register_participant_name(name: &str) {
    let name = name.trim();
    let mut known = known();
    if name.chars().count() >= MIN_KNOWN_CHARS && !known.names.iter().any(|n| n == name) {
        known.names.push(name.to_string());
    }
}

/// A device as the OS lists it. The raw ID and name are replaced by what
/// [`redact_device_id`] and [`redact_device_owner`] leave of them, wherever a
/// log site writes them.
pub fn register_device(id: &str, name: &str) {
    let mut known = known();
    let replacements = [
        (id, redact_device_id(id, true)),
        (name, redact_device_owner(name)),
    ];
    for (raw, masked) in replacements {
        if raw != masked
            && raw.chars().count() >= MIN_KNOWN_CHARS
            && !known.devices.iter().any(|(r, _)| r == raw)
        {
            known.devices.push((raw.to_string(), masked));
        }
    }
}

/// Keeps the first two characters of an invite code or a room ID.
fn shorten_room(value: &str) -> String {
    let head: String = value.chars().take(2).collect();
    format!("{}***", head)
}

/// `text` with every `needle` replaced. A needle made of ASCII letters and
/// digits only matches as a whole word, so that the name `ann` does not
/// rewrite `channel`.
fn replace_known(text: &str, needle: &str, replacement: &str) -> String {
    let whole_word = needle.chars().all(|c| c.is_ascii_alphanumeric());
    if !whole_word {
        return text.replace(needle, replacement);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(needle) {
        let before = rest[..at].chars().next_back();
        let after = rest[at + needle.len()..].chars().next();
        let inside_word =
            before.is_some_and(char::is_alphanumeric) || after.is_some_and(char::is_alphanumeric);
        out.push_str(&rest[..at]);
        out.push_str(if inside_word { needle } else { replacement });
        rest = &rest[at + needle.len()..];
    }
    out.push_str(rest);
    out
}

/// Leaves out what identifies a person from a rendered log line: the display
/// names, room IDs and invite codes the app has registered, the peer IDs
/// (UUIDs; the first four characters survive) and the user name in a home
/// folder path (`/Users/taro/Library` -> `~/Library`). What is diagnosed from
/// is the kind, the version and the shape of a value, never whose it is.
///
/// `enabled` is threaded in as in [`redact_network_identifiers`].
fn redact_personal_identifiers(text: &str, enabled: bool) -> String {
    if !enabled {
        return text.to_string();
    }
    let known = known();
    apply_known(text, &known)
}

fn apply_known(text: &str, known: &Known) -> String {
    let mut out = text.to_string();
    // Longest first: a name that contains another one is replaced whole.
    let mut names: Vec<(usize, &String)> = known.names.iter().enumerate().collect();
    names.sort_by_key(|(_, name)| std::cmp::Reverse(name.len()));
    for (index, name) in names {
        out = replace_known(&out, name, &format!("peer{}", index + 1));
    }
    for room in &known.rooms {
        out = replace_known(&out, room, &shorten_room(room));
    }
    for (raw, masked) in &known.devices {
        out = out.replace(raw.as_str(), masked);
    }
    redact_home_dir(&redact_device_identifiers(&redact_uuids(&out)))
}

fn redact_uuids(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut i = 0;
    while i + 36 <= bytes.len() {
        let standalone = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
        if standalone
            && is_uuid(&bytes[i..i + 36])
            && !bytes.get(i + 36).is_some_and(u8::is_ascii_alphanumeric)
        {
            out.push_str(&text[copied..i]);
            out.push_str(&text[i..i + 4]);
            out.push('…');
            i += 36;
            copied = i;
        } else {
            i += 1;
        }
    }
    out.push_str(&text[copied..]);
    out
}

/// This installation's device identifier (26 base32 characters, the one the
/// app presents to the signaling server): its first four characters stay.
fn redact_device_identifiers(text: &str) -> String {
    const LEN: usize = 26;
    let bytes = text.as_bytes();
    let is_base32 = |b: &u8| b.is_ascii_uppercase() || (b'2'..=b'7').contains(b);
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut i = 0;
    while i + LEN <= bytes.len() {
        let standalone = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
        if standalone
            && bytes[i..i + LEN].iter().all(is_base32)
            && !bytes.get(i + LEN).is_some_and(u8::is_ascii_alphanumeric)
        {
            out.push_str(&text[copied..i]);
            out.push_str(&text[i..i + 4]);
            out.push('…');
            i += LEN;
            copied = i;
        } else {
            i += 1;
        }
    }
    out.push_str(&text[copied..]);
    out
}

fn is_uuid(bytes: &[u8]) -> bool {
    bytes.iter().enumerate().all(|(i, b)| match i {
        8 | 13 | 18 | 23 => *b == b'-',
        _ => b.is_ascii_hexdigit(),
    })
}

/// `/Users/<name>/`, `/home/<name>/` and `C:\Users\<name>\` to `~/`.
fn redact_home_dir(text: &str) -> String {
    const MARKERS: [&str; 3] = ["/Users/", "/home/", "\\Users\\"];
    let mut out = text.to_string();
    for marker in MARKERS {
        let mut from = 0;
        while let Some(found) = out[from..].find(marker) {
            let at = from + found;
            let name_start = at + marker.len();
            // A Windows user name may hold a space; a macOS or Linux one does not.
            let windows = marker.starts_with('\\');
            let name_end = out[name_start..]
                .find(|c: char| {
                    matches!(c, '/' | '\\' | '"' | '\'' | ')' | '\n') || (!windows && c == ' ')
                })
                .map_or(out.len(), |n| name_start + n);
            if name_end == name_start {
                from = name_start;
                continue;
            }
            // The drive letter in front belongs to the path that is replaced.
            let bytes = out.as_bytes();
            let start = if marker.starts_with('\\')
                && at >= 2
                && bytes[at - 1] == b':'
                && bytes[at - 2].is_ascii_alphabetic()
            {
                at - 2
            } else {
                at
            };
            out.replace_range(start..name_end, "~");
            from = start + 1;
        }
    }
    out
}

/// The owner out of a device name: `Taro's AirPods` -> `***'s AirPods`,
/// `太郎のAirPods` -> `***のAirPods`. The device itself stays readable, since
/// what is diagnosed from is which product it is. Apple's own names for a
/// built-in device (`MacBook Proのスピーカー`) are not somebody's and stay.
pub(crate) fn redact_device_owner(name: &str) -> String {
    const PRODUCTS: [&str; 7] = [
        "macbook", "imac", "mac", "studio", "iphone", "ipad", "apple",
    ];
    for separator in ["'s ", "\u{2019}s ", "の"] {
        let Some((owner, rest)) = name.split_once(separator) else {
            continue;
        };
        let first_word = owner.split_whitespace().next().unwrap_or("").to_lowercase();
        if owner.is_empty() || rest.is_empty() || PRODUCTS.contains(&first_word.as_str()) {
            return name.to_string();
        }
        return format!("***{}{}", separator, rest);
    }
    name.to_string()
}

/// Reduces a cpal device id (`"{host}:{backend-specific value}"`, e.g.
/// `coreaudio:AppleUSBAudioEngine:Yamaha Corporation:AG06/AG03:20221310:1,2`
/// on macOS) to the product-looking segment, dropping the serial number and
/// channel counts the backend-specific value can embed. Strings that are
/// not colon-shaped (`"system default"`) are returned unchanged.
///
/// `enabled` is threaded in for the same reason as in
/// [`redact_network_identifiers`].
pub(crate) fn redact_device_id(raw: &str, enabled: bool) -> String {
    if !enabled {
        return raw.to_string();
    }
    let mut segments: Vec<&str> = raw.split(':').collect();
    if segments.len() < 2 {
        return redact_device_owner(raw);
    }
    while segments.len() > 1 && segments.last().is_some_and(|s| is_numeric_ish(s)) {
        segments.pop();
    }
    redact_device_owner(segments.last().copied().unwrap_or(raw))
}

fn is_numeric_ish(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit() || c == ',')
}

/// The text one webview message becomes in the file.
fn webview_line(label: &str, message: &str) -> String {
    let mut message = redact_secrets(message);
    if message.len() > MAX_WEBVIEW_MESSAGE_BYTES {
        let mut cut = MAX_WEBVIEW_MESSAGE_BYTES;
        while !message.is_char_boundary(cut) {
            cut -= 1;
        }
        let dropped = message.len() - cut;
        message.truncate(cut);
        message.push_str(&format!("... [{} more bytes dropped]", dropped));
    }
    format!("[{}] {}", label, message)
}

/// Writes a line from the webview into the log file.
///
/// `level` is the `console` method's name (`error`, `warn`, `info`, `debug`).
///
/// `async` so it runs on the async runtime: a synchronous command runs on the
/// main thread, where every line the webview logs would queue behind the
/// commands it is describing (device enumeration takes tens of milliseconds).
#[tauri::command]
pub async fn log_frontend(level: String, message: String, webview: tauri::Webview) {
    let level = match level.as_str() {
        "error" => log::Level::Error,
        "warn" => log::Level::Warn,
        "debug" => log::Level::Debug,
        _ => log::Level::Info,
    };
    log::log!(target: WEBVIEW_TARGET, level, "{}", webview_line(webview.label(), &message));
}

/// Opens the folder that holds `jamuru.log` in the OS file manager and returns
/// its path, so the settings screen can also show where it is.
#[tauri::command]
pub fn log_open_dir(app: AppHandle) -> Result<String, String> {
    let dir = app
        .path()
        .app_log_dir()
        .map_err(|e| format!("The log folder is unknown: {}", e))?;
    open_dir_with(desktop_opener(), &dir)
}

/// The OS command that opens a folder in the file manager or an address in
/// the browser.
pub(crate) fn desktop_opener() -> &'static str {
    if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    }
}

/// Every error names the folder, so a user whose desktop has no file manager
/// can still open it by hand.
fn open_dir_with(opener: &str, dir: &Path) -> Result<String, String> {
    std::process::Command::new(opener)
        .arg(dir)
        .spawn()
        .map_err(|e| format!("Could not open the log folder {}: {}", dir.display(), e))?;
    Ok(dir.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::LevelFilter;

    fn level_of(spec: &LogSpec, target: &str) -> LevelFilter {
        spec.targets
            .iter()
            .find(|(t, _)| t == target)
            .map(|(_, l)| *l)
            .unwrap_or(spec.default)
    }

    /// Verifies: REQ-GUI-019
    #[test]
    fn env_var_unset_logs_own_code_at_debug_and_dependencies_at_info() {
        let spec = LogSpec::parse(None);

        assert_eq!(level_of(&spec, "jamuru"), LevelFilter::Debug);
        assert_eq!(level_of(&spec, "jamjam_app_lib"), LevelFilter::Debug);
        assert_eq!(level_of(&spec, "webview"), LevelFilter::Debug);
        assert_eq!(level_of(&spec, "tao"), LevelFilter::Info);
    }

    /// Verifies: REQ-GUI-019
    #[test]
    fn env_var_is_a_bare_level_applies_to_own_code_and_dependencies() {
        let spec = LogSpec::parse(Some("trace"));

        assert_eq!(level_of(&spec, "jamuru"), LevelFilter::Trace);
        assert_eq!(level_of(&spec, "tao"), LevelFilter::Trace);
    }

    /// Verifies: REQ-GUI-019
    #[test]
    fn env_var_names_a_target_only_that_target_changes() {
        let spec = LogSpec::parse(Some("info, tao=debug"));

        assert_eq!(level_of(&spec, "tao"), LevelFilter::Debug);
        assert_eq!(level_of(&spec, "wry"), LevelFilter::Info);
        assert_eq!(level_of(&spec, "jamuru"), LevelFilter::Info);
    }

    /// Verifies: REQ-GUI-019
    #[test]
    fn env_var_names_a_target_and_a_bare_level_the_target_wins_in_either_order() {
        for spec in ["jamuru=trace,info", "info,jamuru=trace"] {
            let spec = LogSpec::parse(Some(spec));

            assert_eq!(level_of(&spec, "jamuru"), LevelFilter::Trace);
            assert_eq!(level_of(&spec, "jamjam_app_lib"), LevelFilter::Info);
            assert_eq!(level_of(&spec, "tao"), LevelFilter::Info);
        }
    }

    /// Verifies: REQ-GUI-019
    #[test]
    fn env_var_has_an_invalid_part_the_rest_still_applies_and_the_part_is_reported() {
        let spec = LogSpec::parse(Some("loud,jamuru=trace,tao=,=debug"));

        assert_eq!(level_of(&spec, "jamuru"), LevelFilter::Trace);
        assert_eq!(spec.ignored, vec!["loud", "tao=", "=debug"]);
    }

    /// Records the lines that reached the file.
    struct Collect(std::sync::Arc<Mutex<Vec<String>>>);

    impl log::Log for Collect {
        fn enabled(&self, _: &log::Metadata) -> bool {
            true
        }
        fn log(&self, record: &log::Record) {
            self.0.lock().unwrap().push(record.args().to_string());
        }
        fn flush(&self) {}
    }

    /// Logs a warning as if it came from `connection.rs` at `line`.
    fn warn_from(limiter: &SiteLimiter, line: u32, message: &str) {
        log::Log::log(
            limiter,
            &log::Record::builder()
                .args(format_args!("{}", message))
                .level(log::Level::Warn)
                .target("jamuru")
                .file(Some("connection.rs"))
                .line(Some(line))
                .build(),
        );
    }

    #[test]
    fn a_call_site_repeats_within_the_window_the_excess_is_held_back_and_counted_after_it() {
        let written = std::sync::Arc::new(Mutex::new(Vec::new()));
        let clock = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let clock_reader = clock.clone();
        let limiter = SiteLimiter::with_clock(
            Box::new(Collect(written.clone())),
            Box::new(move || {
                Duration::from_secs(clock_reader.load(std::sync::atomic::Ordering::SeqCst))
            }),
        );

        for i in 0..20 {
            warn_from(&limiter, 10, &format!("bad frame {}", i));
        }
        warn_from(&limiter, 99, "another site is not limited");
        assert_eq!(
            written.lock().unwrap().len(),
            RECORDS_PER_SITE_WINDOW as usize + 1
        );

        clock.store(SITE_WINDOW.as_secs(), std::sync::atomic::Ordering::SeqCst);
        warn_from(&limiter, 10, "bad frame 20");

        let written = written.lock().unwrap();
        assert!(
            written
                .iter()
                .any(|line| line.contains("15 similar record(s) from connection.rs:10")),
            "{:?}",
            written
        );
        assert_eq!(written.last().unwrap(), "bad frame 20");
    }

    #[test]
    fn a_line_comes_from_the_webview_the_site_limiter_does_not_touch_it() {
        let written = std::sync::Arc::new(Mutex::new(Vec::new()));
        let limiter = SiteLimiter::new(Box::new(Collect(written.clone())));
        let from_webview = log::Record::builder()
            .args(format_args!("same"))
            .level(log::Level::Info)
            .target(WEBVIEW_TARGET)
            .build();

        for _ in 0..20 {
            log::Log::log(&limiter, &from_webview);
        }

        assert_eq!(written.lock().unwrap().len(), 20);
    }

    /// Verifies: REQ-GUI-018
    #[test]
    fn webview_message_holds_a_room_password_the_file_line_does_not() {
        let line = webview_line(
            "main",
            r#"join failed {"roomCode":"ABC-123","password":"hunter2 with spaces","peer":"x"} retry password=hunter3&x=1"#,
        );

        assert!(!line.contains("hunter2"), "{}", line);
        assert!(!line.contains("hunter3"), "{}", line);
        assert!(line.contains("ABC-123"), "{}", line);
        assert!(line.contains(r#""password":"***""#), "{}", line);
        assert!(line.starts_with("[main] "), "{}", line);
    }

    #[test]
    fn secret_is_in_a_camel_case_key_or_after_a_colon_the_value_is_masked() {
        assert_eq!(
            redact_secrets("roomPassword: abc123, other: 1"),
            "roomPassword: ***, other: 1"
        );
        assert_eq!(redact_secrets("?token=abc&room=1"), "?token=***&room=1");
    }

    #[test]
    fn text_has_no_secret_key_it_is_unchanged() {
        let text = "Connected in 120ms 接続しました has_password without value";
        assert_eq!(redact_secrets(text), text);
    }

    #[test]
    fn multibyte_text_surrounds_a_secret_the_value_is_masked_up_to_the_next_space() {
        // An unquoted value has no visible end in Japanese text, so everything
        // up to the next ASCII delimiter goes: too much is masked, never too little.
        assert_eq!(
            redact_secrets("参加できません password=あいう。 終わり"),
            "参加できません password=*** 終わり"
        );
    }

    #[test]
    fn webview_message_exceeds_the_cap_it_is_cut_on_a_char_boundary() {
        let line = webview_line("main", &"あ".repeat(MAX_WEBVIEW_MESSAGE_BYTES));

        assert!(line.contains("more bytes dropped"), "{}", line.len());
        assert!(line.len() < MAX_WEBVIEW_MESSAGE_BYTES + 64);
    }

    /// Verifies: REQ-GUI-022
    #[test]
    fn a_global_ipv4_address_is_in_the_line_only_the_last_octet_is_masked() {
        let line =
            redact_network_identifiers("STUN discovered public address: 203.0.113.67:61711", true);

        assert_eq!(line, "STUN discovered public address: 203.0.113.x:61711");
    }

    /// Verifies: REQ-GUI-022
    #[test]
    fn a_lan_ipv4_address_is_in_the_line_it_is_still_recognisable_as_lan() {
        let line = redact_network_identifiers("Added host candidate: 192.168.1.186:61711", true);

        assert_eq!(line, "Added host candidate: 192.168.1.x:61711");
    }

    /// Verifies: REQ-GUI-022
    #[test]
    fn a_link_local_ipv6_address_is_in_the_line_the_eui_64_interface_id_is_masked() {
        let line = redact_network_identifiers(
            "Added host candidate: [fe80::4037:96ff:fee1:fcde]:61711",
            true,
        );

        assert_eq!(line, "Added host candidate: [fe80:0:0:0:x:x:x:x]:61711");
        assert!(!line.contains("4037"), "{}", line);
        assert!(!line.contains("fcde"), "{}", line);
    }

    #[test]
    fn text_has_no_address_the_line_is_unchanged() {
        let text = "Connected in 120ms buffer=128 sample_rate=48000";
        assert_eq!(redact_network_identifiers(text, true), text);
    }

    /// Verifies: REQ-GUI-022
    #[test]
    fn redaction_is_disabled_addresses_reach_the_line_unmasked() {
        let text = "STUN discovered public address: 203.0.113.67:61711";
        assert_eq!(redact_network_identifiers(text, false), text);
    }

    #[test]
    fn env_var_is_off_the_flag_parses_to_disabled() {
        assert!(!parse_redact_flag(Some("off")));
        assert!(!parse_redact_flag(Some("OFF")));
        assert!(parse_redact_flag(Some("trace")));
        assert!(parse_redact_flag(None));
    }

    /// Verifies: REQ-GUI-022
    #[test]
    fn a_device_id_holds_a_serial_number_only_the_product_name_survives() {
        let name = redact_device_id(
            "coreaudio:AppleUSBAudioEngine:Yamaha Corporation:AG06/AG03:20221310:1,2",
            true,
        );

        assert_eq!(name, "AG06/AG03");
    }

    #[test]
    fn device_id_is_the_system_default_placeholder_it_is_unchanged() {
        assert_eq!(redact_device_id("system default", true), "system default");
    }

    /// Verifies: REQ-GUI-022
    #[test]
    fn device_id_redaction_is_disabled_the_raw_id_survives() {
        let raw = "coreaudio:AppleUSBAudioEngine:Yamaha Corporation:AG06/AG03:20221310:1,2";
        assert_eq!(redact_device_id(raw, false), raw);
    }

    fn known_with(rooms: &[&str], names: &[&str]) -> Known {
        Known {
            rooms: rooms.iter().map(|r| r.to_string()).collect(),
            names: names.iter().map(|n| n.to_string()).collect(),
            devices: Vec::new(),
        }
    }

    /// Verifies: REQ-GUI-022
    #[test]
    fn a_room_id_or_invite_code_is_in_a_line_only_its_first_two_characters_survive() {
        let known = known_with(&["ABC234XYZ"], &[]);

        let line = apply_known("Created room ABC234XYZ as host", &known);

        assert_eq!(line, "Created room AB*** as host");
    }

    /// Verifies: REQ-GUI-022
    #[test]
    fn a_display_name_is_in_a_line_it_becomes_a_numbered_peer() {
        let known = known_with(&[], &["Taro", "花子"]);

        let line = apply_known("Taro joined; 花子さん left; Taro again", &known);

        assert_eq!(line, "peer1 joined; peer2さん left; peer1 again");
    }

    /// Verifies: REQ-GUI-022
    #[test]
    fn a_display_name_is_part_of_another_word_the_word_is_left_alone() {
        let known = known_with(&[], &["ann"]);

        let line = apply_known("channel 2, ann, Joann", &known);

        assert_eq!(line, "channel 2, peer1, Joann");
    }

    /// Verifies: REQ-GUI-022
    #[test]
    fn a_name_contains_another_name_the_longer_one_is_replaced_whole() {
        let known = known_with(&[], &["Taro", "Taro Yamada"]);

        let line = apply_known("Taro Yamada joined", &known);

        assert_eq!(line, "peer2 joined");
    }

    /// Verifies: REQ-GUI-022
    #[test]
    fn a_peer_id_is_in_a_line_only_its_first_four_characters_survive() {
        let known = known_with(&[], &[]);

        let line = apply_known(
            "Peer 3f2a9c10-7b1e-4d55-9a02-0c6d5e8f1a77 joined the room",
            &known,
        );

        assert_eq!(line, "Peer 3f2a… joined the room");
    }

    /// Verifies: REQ-GUI-022
    #[test]
    fn a_device_identifier_is_in_a_line_only_its_first_four_characters_survive() {
        let known = known_with(&[], &[]);

        let line = apply_known("this device is ZV552SMKNDGMD56IKZ5CGND4LQ (beta)", &known);

        assert_eq!(line, "this device is ZV55… (beta)");
    }

    /// Verifies: REQ-GUI-022
    #[test]
    fn text_that_only_looks_like_a_device_identifier_it_is_left_alone() {
        let known = known_with(&[], &[]);

        for text in [
            "ZV552SMKNDGMD56IKZ5CGND4L",
            "ZV552SMKNDGMD56IKZ5CGND4LQX",
            "zv552smkndgmd56ikz5cgnd4lq",
            "ZV552SMKNDGMD56IKZ5CGND4L1",
        ] {
            assert_eq!(apply_known(text, &known), text);
        }
    }

    /// Verifies: REQ-GUI-022
    #[test]
    fn text_that_only_looks_like_a_peer_id_it_is_left_alone() {
        let known = known_with(&[], &[]);

        for text in [
            "3f2a9c10-7b1e-4d55-9a02-0c6d5e8f1a7",
            "x3f2a9c10-7b1e-4d55-9a02-0c6d5e8f1a77",
            "3f2a9c10-7b1e-4d55-9a02-0c6d5e8f1a77f",
            "zzzzzzzz-7b1e-4d55-9a02-0c6d5e8f1a77",
        ] {
            assert_eq!(apply_known(text, &known), text);
        }
    }

    /// Verifies: REQ-GUI-022
    #[test]
    fn a_path_is_in_a_home_folder_the_user_name_becomes_a_tilde() {
        let known = known_with(&[], &[]);

        for (text, expected) in [
            (
                "log file: /Users/taro/Library/Logs/jp.jamuru/jamuru.log",
                "log file: ~/Library/Logs/jp.jamuru/jamuru.log",
            ),
            (
                "opened /home/taro/.config/jamuru",
                "opened ~/.config/jamuru",
            ),
            (
                "C:\\Users\\Taro Y\\AppData\\Roaming\\jamuru",
                "~\\AppData\\Roaming\\jamuru",
            ),
            ("/usr/lib/libasound.so", "/usr/lib/libasound.so"),
        ] {
            assert_eq!(apply_known(text, &known), expected, "{text}");
        }
    }

    /// Verifies: REQ-GUI-022
    #[test]
    fn a_device_name_has_an_owner_the_owner_is_hidden_and_the_product_stays() {
        assert_eq!(
            redact_device_owner("Taro's AirPods Pro"),
            "***'s AirPods Pro"
        );
        assert_eq!(redact_device_owner("太郎のAirPods"), "***のAirPods");
        assert_eq!(redact_device_owner("Yamaha AG06/AG03"), "Yamaha AG06/AG03");
    }

    /// Verifies: REQ-GUI-022
    #[test]
    fn a_device_name_is_the_names_of_a_built_in_device_it_is_left_alone() {
        assert_eq!(
            redact_device_owner("MacBook Proのスピーカー"),
            "MacBook Proのスピーカー"
        );
        assert_eq!(redact_device_owner("iPhoneのマイク"), "iPhoneのマイク");
    }

    /// Verifies: REQ-GUI-022
    #[test]
    fn a_device_id_names_its_owner_the_owner_is_hidden_with_the_serial() {
        assert_eq!(
            redact_device_id("coreaudio:Taro's AirPods:20221310:1,2", true),
            "***'s AirPods"
        );
    }

    /// Verifies: REQ-GUI-022
    #[test]
    fn personal_redaction_is_turned_off_the_line_is_unchanged() {
        let line = "/Users/taro/x 3f2a9c10-7b1e-4d55-9a02-0c6d5e8f1a77";

        assert_eq!(redact_personal_identifiers(line, false), line);
    }

    /// Verifies: REQ-GUI-022
    #[test]
    fn a_registered_value_is_logged_the_line_in_the_file_does_not_hold_it() {
        register_room("QWE789RTY", None);
        register_participant_name("Registered Tester");
        register_device("coreaudio:Registered's Mic:99887766:1", "Registered's Mic");

        let line = redact_personal_identifiers(
            "room QWE789RTY, Registered Tester, coreaudio:Registered's Mic:99887766:1",
            true,
        );

        assert!(!line.contains("QWE789RTY"), "{line}");
        assert!(!line.contains("Registered Tester"), "{line}");
        assert!(!line.contains("99887766"), "{line}");
        assert!(line.contains("QW***"), "{line}");
    }

    #[test]
    fn a_held_back_notice_names_a_ci_build_path_only_the_file_name_survives() {
        let written = std::sync::Arc::new(Mutex::new(Vec::new()));
        let clock = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let clock_reader = clock.clone();
        let limiter = SiteLimiter::with_clock(
            Box::new(Collect(written.clone())),
            Box::new(move || {
                Duration::from_secs(clock_reader.load(std::sync::atomic::Ordering::SeqCst))
            }),
        );
        let ci_path = "/Users/runner/work/jamuru-client/jamuru-client/src/network/connection.rs";

        for i in 0..20 {
            let message = format!("bad frame {}", i);
            log::Log::log(
                &limiter,
                &log::Record::builder()
                    .args(format_args!("{}", message))
                    .level(log::Level::Warn)
                    .target("jamuru")
                    .file(Some(ci_path))
                    .line(Some(1057))
                    .build(),
            );
        }
        clock.store(SITE_WINDOW.as_secs(), std::sync::atomic::Ordering::SeqCst);
        log::Log::log(
            &limiter,
            &log::Record::builder()
                .args(format_args!("bad frame 20"))
                .level(log::Level::Warn)
                .target("jamuru")
                .file(Some(ci_path))
                .line(Some(1057))
                .build(),
        );

        let written = written.lock().unwrap();
        assert!(
            written
                .iter()
                .any(|line| line.contains("from connection.rs:1057")),
            "{:?}",
            written
        );
        assert!(
            !written.iter().any(|line| line.contains("/Users/runner")),
            "{:?}",
            written
        );
    }

    #[test]
    fn server_url_has_credentials_the_summary_omits_them() {
        assert_eq!(
            strip_userinfo("https://user:pass@signal.example.com/ws?x=1"),
            "https://signal.example.com/ws?x=1"
        );
        assert_eq!(
            strip_userinfo("https://signal.example.com/a@b"),
            "https://signal.example.com/a@b"
        );
    }

    /// Verifies: REQ-GUI-020
    #[test]
    fn opener_is_missing_the_error_names_the_log_folder() {
        let err =
            open_dir_with("jamuru-no-such-opener", Path::new("/logs/me.koeda.jamuru")).unwrap_err();

        assert!(err.contains("/logs/me.koeda.jamuru"), "{}", err);
    }

    #[cfg(unix)]
    /// Verifies: REQ-GUI-020
    #[test]
    fn opener_starts_the_result_is_the_log_folder_path() {
        let opened = open_dir_with("true", Path::new("/logs/me.koeda.jamuru"));

        assert_eq!(opened, Ok("/logs/me.koeda.jamuru".to_string()));
    }

    /// Verifies: REQ-GUI-018
    #[test]
    fn config_summary_lists_the_settings_and_omits_the_credentials_in_the_server_url() {
        let config = AppConfig {
            server_url: Some("https://user:hunter2@signal.example.com/ws".to_string()),
            input_device_id: Some("USB Interface".to_string()),
            buffer_size: 128,
            ..AppConfig::default()
        };

        let summary = summarize_config(&config);

        assert!(
            summary.contains("server=https://signal.example.com/ws"),
            "{}",
            summary
        );
        assert!(summary.contains(r#"input="USB Interface""#), "{}", summary);
        assert!(summary.contains("buffer=128"), "{}", summary);
        assert!(!summary.contains("hunter2"), "{}", summary);
    }

    /// Verifies: REQ-GUI-022
    #[test]
    fn config_summary_has_a_device_serial_number_only_the_product_name_survives() {
        let config = AppConfig {
            input_device_id: Some(
                "coreaudio:AppleUSBAudioEngine:Yamaha Corporation:AG06/AG03:20221310:1,2"
                    .to_string(),
            ),
            ..AppConfig::default()
        };

        let summary = summarize_config(&config);

        assert!(summary.contains(r#"input="AG06/AG03""#), "{}", summary);
        assert!(!summary.contains("20221310"), "{}", summary);
    }
}
