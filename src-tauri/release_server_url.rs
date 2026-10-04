//! The rule for the jamuru server a release build is given (ADR-030).
//!
//! Shared by the app's build script, which enforces it, and
//! `tests/distribution_config_test.rs`, which pins it down. Only `std`, as a
//! build script has nothing else.

use std::net::IpAddr;

/// Why `url` cannot be the server a release build uses, or `None` if it can:
/// it must be `https://` and name a host other than the user's own machine.
pub fn release_server_url_problem(url: &str) -> Option<&'static str> {
    if url.is_empty() {
        return Some(
            "a release build needs JAMJAM_SERVER_URL: the jamuru server the app asks for its signaling server",
        );
    }
    let Some(rest) = url.strip_prefix("https://") else {
        return Some("JAMJAM_SERVER_URL must be an https:// URL");
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host_port = authority.rsplit('@').next().unwrap_or_default();
    let host = if let Some(bracketed) = host_port.strip_prefix('[') {
        bracketed.split(']').next().unwrap_or_default()
    } else {
        host_port.split(':').next().unwrap_or_default()
    };
    let host = host.to_ascii_lowercase();
    let host = host.trim_end_matches('.');
    if host.is_empty() {
        return Some("JAMJAM_SERVER_URL names no host");
    }
    let this_machine = host == "localhost"
        || host.ends_with(".localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|ip| ip.is_loopback() || ip.is_unspecified());
    // A shortened IPv4 form (`127.1`) is not an `IpAddr` here, yet resolvers
    // read it as one - so a host of only digits and dots must be a full address.
    let numeric_but_not_an_address =
        host.chars().all(|c| c.is_ascii_digit() || c == '.') && host.parse::<IpAddr>().is_err();
    if this_machine || numeric_but_not_an_address {
        return Some("JAMJAM_SERVER_URL must name a server other than this machine");
    }
    None
}
