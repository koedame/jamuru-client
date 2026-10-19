//! Opening an address that someone posted in the chat.
//!
//! The text comes from another participant, so the screen may ask for an
//! address to be opened but not for anything else to be run: only `http` and
//! `https` addresses go to the OS opener, never a local path, a `file:` URL or
//! a custom scheme that would start another program.

/// The longest address that is opened. Longer ones are refused rather than
/// handed to the OS opener.
const MAX_LEN: usize = 2048;

/// Opens `url` in the browser. Only a web address: see [`check`].
#[tauri::command]
pub fn chat_open_link(url: String) -> Result<(), String> {
    check(&url)?;
    crate::terms::open_url(&url)
}

/// Whether `url` is a web address that is safe to hand to the OS opener.
fn check(url: &str) -> Result<(), String> {
    if url.len() > MAX_LEN {
        return Err("The address is too long to open".to_string());
    }
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("The address has a space or a control character".to_string());
    }
    let lower = url.to_ascii_lowercase();
    let rest = lower
        .strip_prefix("https://")
        .or_else(|| lower.strip_prefix("http://"))
        .ok_or_else(|| "Only http and https addresses are opened".to_string())?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or("");
    if host.is_empty() || host.starts_with(':') {
        return Err("The address has no host".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: REQ-GUI-029
    #[test]
    fn when_the_address_is_http_or_https_it_is_accepted() {
        assert!(check("https://example.com/a?b=c#d").is_ok());
        assert!(check("HTTP://example.com").is_ok());
        assert!(check("https://user@example.com:8080/").is_ok());
    }

    /// Verifies: REQ-GUI-029
    #[test]
    fn when_the_address_is_not_a_web_address_it_is_refused() {
        for url in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "ms-msdt:/id",
            "smb://host/share",
            "C:\\Windows\\System32\\calc.exe",
            "/usr/bin/open",
            "example.com",
            "",
        ] {
            assert!(check(url).is_err(), "{url} should be refused");
        }
    }

    /// Verifies: REQ-GUI-029
    #[test]
    fn when_the_address_has_no_host_it_is_refused() {
        for url in ["https://", "https:///path", "http://:80/", "https://user@/"] {
            assert!(check(url).is_err(), "{url} should be refused");
        }
    }

    /// Verifies: REQ-GUI-029
    #[test]
    fn when_the_address_has_a_space_or_control_character_it_is_refused() {
        assert!(check("https://example.com/a b").is_err());
        assert!(check("https://example.com/\n--flag").is_err());
    }

    /// Verifies: REQ-GUI-029
    #[test]
    fn when_the_address_is_too_long_it_is_refused() {
        let url = format!("https://example.com/{}", "a".repeat(MAX_LEN));
        assert!(check(&url).is_err());
    }
}
