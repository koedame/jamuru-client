//! Whether this machine goes out to the network over Wi-Fi.
//!
//! Wi-Fi is not recommended for a session: its delay varies, and a stream of
//! audio packets rides it badly. The diagnostics warn about it the way the
//! Devices tab warns about a Bluetooth device. The question is about the
//! interface the default route leaves by, not about whether a Wi-Fi adapter
//! exists: a laptop on a cable with Wi-Fi switched on is not on Wi-Fi.

/// Whether the default route leaves by a Wi-Fi interface. `None` when that
/// could not be found out, so the caller says nothing instead of guessing.
pub fn default_route_is_wifi() -> Option<bool> {
    platform::default_route_is_wifi()
}

/// The interface the IPv4 default route leaves by, out of the text of
/// Linux's `/proc/net/route`. Of several default routes the one with the
/// lowest metric wins, as the kernel picks it.
#[cfg(any(target_os = "linux", test))]
fn linux_default_interface(route_table: &str) -> Option<&str> {
    route_table
        .lines()
        .skip(1)
        .filter_map(|line| {
            let mut columns = line.split_whitespace();
            let interface = columns.next()?;
            let destination = columns.next()?;
            let _gateway = columns.next()?;
            let flags = u32::from_str_radix(columns.next()?, 16).ok()?;
            let _ref_count = columns.next()?;
            let _use = columns.next()?;
            let metric: u32 = columns.next()?.parse().ok()?;
            let mask = columns.next()?;
            let is_up = flags & 0x1 != 0;
            let is_default = destination == "00000000" && mask == "00000000";
            (is_up && is_default).then_some((metric, interface))
        })
        .min_by_key(|(metric, _)| *metric)
        .map(|(_, interface)| interface)
}

/// The interface `route -n get default` says the default route leaves by.
#[cfg(any(target_os = "macos", test))]
fn macos_default_interface(route_output: &str) -> Option<&str> {
    route_output
        .lines()
        .find_map(|line| line.trim().strip_prefix("interface:"))
        .map(str::trim)
        .filter(|interface| !interface.is_empty())
}

/// Whether `networksetup -listallhardwareports` lists `interface` as Wi-Fi.
/// The port's name is "Wi-Fi" (or "AirPort" before macOS 10.8).
#[cfg(any(target_os = "macos", test))]
fn macos_interface_is_wifi(hardware_ports: &str, interface: &str) -> bool {
    let mut port_is_wifi = false;
    for line in hardware_ports.lines() {
        if let Some(port) = line.trim().strip_prefix("Hardware Port:") {
            let port = port.trim();
            port_is_wifi = port == "Wi-Fi" || port == "AirPort";
        } else if let Some(device) = line.trim().strip_prefix("Device:") {
            if device.trim() == interface {
                return port_is_wifi;
            }
        }
    }
    false
}

#[cfg(target_os = "linux")]
mod platform {
    use std::path::Path;

    pub fn default_route_is_wifi() -> Option<bool> {
        let route_table = std::fs::read_to_string("/proc/net/route").ok()?;
        let interface = super::linux_default_interface(&route_table)?;
        // The kernel adds a `wireless` directory to a Wi-Fi interface, and a
        // `phy80211` link to anything the wireless stack manages.
        let sys = Path::new("/sys/class/net").join(interface);
        Some(sys.join("wireless").exists() || sys.join("phy80211").exists())
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::process::Command;

    pub fn default_route_is_wifi() -> Option<bool> {
        let route = run("/sbin/route", &["-n", "get", "default"])?;
        let interface = super::macos_default_interface(&route)?;
        let ports = run("/usr/sbin/networksetup", &["-listallhardwareports"])?;
        Some(super::macos_interface_is_wifi(&ports, interface))
    }

    fn run(program: &str, args: &[&str]) -> Option<String> {
        let output = Command::new(program).args(args).output().ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

#[cfg(windows)]
mod platform {
    use windows::Win32::NetworkManagement::IpHelper::{GetBestInterface, GetIfEntry2, MIB_IF_ROW2};

    /// `IF_TYPE_IEEE80211`: the interface type Windows gives a Wi-Fi adapter.
    const IF_TYPE_IEEE80211: u32 = 71;

    pub fn default_route_is_wifi() -> Option<bool> {
        // Any public address will do: it is the interface that would carry a
        // packet to somewhere outside the local network that is asked for.
        let outside = u32::from_ne_bytes([8, 8, 8, 8]);
        let mut index = 0u32;
        // SAFETY: `index` is a live `u32` the call writes through.
        if unsafe { GetBestInterface(outside, &mut index) } != 0 {
            return None;
        }
        let mut row = MIB_IF_ROW2 {
            InterfaceIndex: index,
            ..Default::default()
        };
        // SAFETY: `row` is a live `MIB_IF_ROW2` with its index set, which the
        // call fills in.
        if unsafe { GetIfEntry2(&mut row) }.is_err() {
            return None;
        }
        Some(row.Type == IF_TYPE_IEEE80211)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod platform {
    pub fn default_route_is_wifi() -> Option<bool> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROUTES: &str =
        "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
wlan0\t00000000\t0100A8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0\n\
eth0\t00000000\t0100A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0\n\
eth0\t0000A8C0\t00000000\t0001\t0\t0\t100\t00FFFFFF\t0\t0\t0\n";

    /// Verifies: REQ-GUI-027
    #[test]
    fn when_two_default_routes_exist_the_one_with_the_lowest_metric_is_the_default_interface() {
        assert_eq!(linux_default_interface(ROUTES), Some("eth0"));
    }

    /// Verifies: REQ-GUI-027
    #[test]
    fn when_the_only_default_route_is_down_there_is_no_default_interface() {
        let routes =
            "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
wlan0\t00000000\t0100A8C0\t0002\t0\t0\t600\t00000000\t0\t0\t0\n";
        assert_eq!(linux_default_interface(routes), None);
    }

    /// Verifies: REQ-GUI-027
    #[test]
    fn when_only_a_local_network_route_exists_there_is_no_default_interface() {
        let routes =
            "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
eth0\t0000A8C0\t00000000\t0001\t0\t0\t100\t00FFFFFF\t0\t0\t0\n";
        assert_eq!(linux_default_interface(routes), None);
    }

    /// Verifies: REQ-GUI-027
    #[test]
    fn when_the_route_table_is_empty_there_is_no_default_interface() {
        assert_eq!(linux_default_interface(""), None);
    }

    /// Verifies: REQ-GUI-027
    #[test]
    fn when_route_prints_the_interface_it_is_the_default_interface() {
        let output = "   route to: default\ndestination: default\n       mask: default\n    gateway: 192.168.1.1\n  interface: en0\n      flags: <UP,GATEWAY,DONE,STATIC,PRCLONING,GLOBAL>\n";
        assert_eq!(macos_default_interface(output), Some("en0"));
    }

    /// Verifies: REQ-GUI-027
    #[test]
    fn when_route_prints_no_interface_there_is_no_default_interface() {
        assert_eq!(
            macos_default_interface("route: writing to routing socket: not in table\n"),
            None
        );
    }

    const PORTS: &str = "Hardware Port: Ethernet Adapter (en4)\nDevice: en4\nEthernet Address: aa:bb:cc:dd:ee:ff\n\nHardware Port: Wi-Fi\nDevice: en0\nEthernet Address: 11:22:33:44:55:66\n\nHardware Port: Thunderbolt Bridge\nDevice: bridge0\nEthernet Address: N/A\n";

    /// Verifies: REQ-GUI-027
    #[test]
    fn when_the_hardware_port_of_the_interface_is_wifi_the_interface_is_wifi() {
        assert!(macos_interface_is_wifi(PORTS, "en0"));
    }

    /// Verifies: REQ-GUI-027
    #[test]
    fn when_the_hardware_port_of_the_interface_is_ethernet_the_interface_is_not_wifi() {
        assert!(!macos_interface_is_wifi(PORTS, "en4"));
    }

    /// Verifies: REQ-GUI-027
    #[test]
    fn when_the_interface_is_not_listed_it_is_not_wifi() {
        assert!(!macos_interface_is_wifi(PORTS, "utun3"));
    }
}
