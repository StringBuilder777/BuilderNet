use std::{collections::BTreeMap, net::Ipv4Addr, process::Command, thread};

use crate::models::{Device, LocalNetwork, NetworkScan, PingScanResult, TRACE_TARGET, TraceHop};

const SCAN_WORKERS: usize = 32;
const TRACE_MAX_HOPS: u8 = 12;

pub(crate) fn default_route() -> (String, String) {
    if cfg!(target_os = "macos") {
        if let Ok(output) = Command::new("route")
            .args(["-n", "get", "default"])
            .output()
        {
            let text = String::from_utf8_lossy(&output.stdout);
            let gateway = value_after_label(&text, "gateway:").unwrap_or("192.168.1.1");
            let interface = value_after_label(&text, "interface:").unwrap_or("desconocida");
            return (gateway.into(), interface.into());
        }
    } else if let Ok(output) = Command::new("ip")
        .args(["route", "show", "default"])
        .output()
    {
        let text = String::from_utf8_lossy(&output.stdout);
        let words: Vec<&str> = text.split_whitespace().collect();
        let gateway = word_after(&words, "via").unwrap_or("192.168.1.1");
        let interface = word_after(&words, "dev").unwrap_or("desconocida");
        return (gateway.into(), interface.into());
    }
    ("192.168.1.1".into(), "desconocida".into())
}

pub(crate) fn ping_once(host: &str) -> Option<f64> {
    let mut command = Command::new("ping");
    command.args(["-c", "1"]);
    if cfg!(target_os = "macos") {
        command.args(["-W", "500"]);
    } else {
        command.args(["-W", "1"]);
    }
    let output = command.arg(host).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_ping_latency(&text).or(Some(0.5))
}

fn ping_broadcast_once(broadcast: &str) -> Option<f64> {
    let mut command = Command::new("ping");
    if cfg!(target_os = "linux") {
        command.arg("-b");
    }
    command.args(["-c", "1"]);
    if cfg!(target_os = "macos") {
        command.args(["-W", "500"]);
    } else {
        command.args(["-W", "1"]);
    }
    let output = command.arg(broadcast).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_ping_latency(&text).or(Some(0.5))
}

fn parse_ping_latency(text: &str) -> Option<f64> {
    for token in text.split_whitespace() {
        if let Some(raw) = token.strip_prefix("time=") {
            return raw.trim_end_matches("ms").parse().ok();
        }
        if token == "time<1" || token == "time<1ms" {
            return Some(0.5);
        }
    }
    None
}

pub(crate) fn scan_local_network(gateway: &str) -> NetworkScan {
    let trace_worker = thread::spawn(|| trace_route(TRACE_TARGET));
    let devices = if let Some(network) = local_network_from_gateway(gateway) {
        let _ = ping_broadcast_once(&network.broadcast);
        let ping_results = ping_sweep(&network);
        merge_scan_results(ping_results, arp_devices())
    } else {
        arp_devices()
    };
    let trace_hops = trace_worker.join().unwrap_or_default();
    let reachable_devices = devices.iter().filter(|device| device.reachable).count();
    let message = scan_message(devices.len(), reachable_devices, trace_hops.len());

    NetworkScan {
        devices,
        trace_hops,
        message,
    }
}

fn scan_message(device_count: usize, reachable_count: usize, trace_count: usize) -> String {
    if device_count == 0 && trace_count == 0 {
        "Sin resultados. Revisa permisos de red o comandos ping/arp/traceroute.".into()
    } else {
        format!(
            "{device_count} dispositivos, {reachable_count} con ping, {trace_count} saltos a {TRACE_TARGET}"
        )
    }
}

fn local_network_from_gateway(gateway: &str) -> Option<LocalNetwork> {
    let octets = gateway.parse::<Ipv4Addr>().ok()?.octets();
    Some(LocalNetwork {
        prefix: format!("{}.{}.{}.", octets[0], octets[1], octets[2]),
        broadcast: format!("{}.{}.{}.255", octets[0], octets[1], octets[2]),
    })
}

fn ping_sweep(network: &LocalNetwork) -> Vec<PingScanResult> {
    let mut workers = Vec::new();
    for start in 1..=SCAN_WORKERS {
        let prefix = network.prefix.clone();
        workers.push(thread::spawn(move || {
            let mut results = Vec::new();
            for last_octet in (start..=254).step_by(SCAN_WORKERS) {
                let ip = format!("{prefix}{last_octet}");
                if let Some(latency_ms) = ping_once(&ip) {
                    results.push(PingScanResult { ip, latency_ms });
                }
            }
            results
        }));
    }

    let mut results = Vec::new();
    for worker in workers {
        if let Ok(mut worker_results) = worker.join() {
            results.append(&mut worker_results);
        }
    }
    results
}

fn merge_scan_results(ping_results: Vec<PingScanResult>, arp_devices: Vec<Device>) -> Vec<Device> {
    let mut devices = BTreeMap::new();

    for device in arp_devices {
        if let Ok(ip) = device.ip.parse::<Ipv4Addr>() {
            devices.insert(ip, device);
        }
    }

    for PingScanResult { ip, latency_ms } in ping_results {
        if let Ok(address) = ip.parse::<Ipv4Addr>() {
            devices
                .entry(address)
                .and_modify(|device: &mut Device| device.mark_reachable(latency_ms))
                .or_insert_with(|| Device::ping(ip, latency_ms));
        }
    }

    devices.into_values().collect()
}

fn trace_route(host: &str) -> Vec<TraceHop> {
    let max_hops = TRACE_MAX_HOPS.to_string();
    let output = Command::new("traceroute")
        .args(["-n", "-m", max_hops.as_str(), "-w", "1", host])
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_traceroute_output(&text)
}

fn parse_traceroute_output(text: &str) -> Vec<TraceHop> {
    text.lines().filter_map(parse_traceroute_line).collect()
}

fn parse_traceroute_line(line: &str) -> Option<TraceHop> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    let hop = tokens.first()?.parse::<u8>().ok()?;
    let address = tokens
        .iter()
        .skip(1)
        .find(|token| **token != "*" && token.parse::<f64>().is_err())
        .copied()
        .unwrap_or("*");
    let latency_ms = first_latency_ms(&tokens[1..]);
    Some(TraceHop::new(hop, address, latency_ms))
}

fn first_latency_ms(tokens: &[&str]) -> Option<f64> {
    tokens.iter().enumerate().find_map(|(index, token)| {
        token
            .trim_end_matches("ms")
            .parse::<f64>()
            .ok()
            .filter(|_| token.ends_with("ms") || tokens.get(index + 1) == Some(&"ms"))
    })
}

fn value_after_label<'a>(text: &'a str, label: &str) -> Option<&'a str> {
    text.lines()
        .map(str::trim)
        .find_map(|line| line.strip_prefix(label).map(str::trim))
}

fn word_after<'a>(words: &'a [&str], needle: &str) -> Option<&'a str> {
    words
        .windows(2)
        .find(|pair| pair[0] == needle)
        .map(|pair| pair[1])
}

fn arp_devices() -> Vec<Device> {
    let output = Command::new("arp").arg("-a").output();
    let Ok(output) = output else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines().filter_map(parse_arp_line).collect()
}

fn parse_arp_line(line: &str) -> Option<Device> {
    if line
        .split_whitespace()
        .any(|word| word == "(incomplete)" || word == "incomplete")
    {
        return None;
    }
    let ip = line
        .split('(')
        .nth(1)
        .and_then(|value| value.split(')').next())
        .or_else(|| line.split_whitespace().next())?;
    let mac = line
        .split_whitespace()
        .find(|word| word.matches(':').count() == 5 || word.matches('-').count() == 5)
        .unwrap_or("desconocida");
    if ip.contains('.') {
        Some(Device::arp(ip, mac))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::DiscoverySource;

    #[test]
    fn parses_normal_ping_latency() {
        let output = "64 bytes from 1.1.1.1: icmp_seq=0 ttl=57 time=12.34 ms";
        assert_eq!(parse_ping_latency(output), Some(12.34));
    }

    #[test]
    fn parses_submillisecond_ping_latency() {
        assert_eq!(
            parse_ping_latency("64 bytes from localhost: time<1 ms"),
            Some(0.5)
        );
        assert_eq!(
            parse_ping_latency("64 bytes from localhost: time<1ms"),
            Some(0.5)
        );
    }

    #[test]
    fn returns_none_when_ping_output_has_no_latency() {
        assert_eq!(parse_ping_latency("PING host: no timing token"), None);
    }

    #[test]
    fn parses_macos_arp_line() {
        let device = parse_arp_line("? (192.168.1.10) at aa:bb:cc:dd:ee:ff on en0").unwrap();
        assert_eq!(device.ip, "192.168.1.10");
        assert_eq!(device.mac, "aa:bb:cc:dd:ee:ff");
    }

    #[test]
    fn parses_linux_arp_line() {
        let device = parse_arp_line("192.168.1.20 ether aa:bb:cc:dd:ee:ff C eth0").unwrap();
        assert_eq!(device.ip, "192.168.1.20");
        assert_eq!(device.mac, "aa:bb:cc:dd:ee:ff");
    }

    #[test]
    fn rejects_incomplete_arp_entries() {
        assert!(parse_arp_line("? (192.168.1.30) at (incomplete) on en0").is_none());
        assert!(parse_arp_line("192.168.1.30 (incomplete) eth0").is_none());
    }

    #[test]
    fn rejects_invalid_arp_entries() {
        assert!(parse_arp_line("not-an-ip ether aa:bb:cc:dd:ee:ff C eth0").is_none());
        assert!(parse_arp_line("").is_none());
    }

    #[test]
    fn extracts_macos_route_labels() {
        let route = "   route to: default\n gateway: 192.168.1.1\ninterface: en0\n";
        assert_eq!(value_after_label(route, "gateway:"), Some("192.168.1.1"));
        assert_eq!(value_after_label(route, "interface:"), Some("en0"));
        assert_eq!(value_after_label(route, "missing:"), None);
    }

    #[test]
    fn extracts_linux_route_words() {
        let words: Vec<&str> = "default via 192.168.1.1 dev eth0 proto dhcp"
            .split_whitespace()
            .collect();
        assert_eq!(word_after(&words, "via"), Some("192.168.1.1"));
        assert_eq!(word_after(&words, "dev"), Some("eth0"));
        assert_eq!(word_after(&words, "missing"), None);
    }

    #[test]
    fn derives_local_24_network_from_gateway() {
        let network = local_network_from_gateway("10.20.30.1").unwrap();
        assert_eq!(network.prefix, "10.20.30.");
        assert_eq!(network.broadcast, "10.20.30.255");
    }

    #[test]
    fn rejects_invalid_gateway_for_local_network() {
        assert!(local_network_from_gateway("not-an-ip").is_none());
        assert!(local_network_from_gateway("example.com").is_none());
    }

    #[test]
    fn merges_ping_and_arp_scan_results() {
        let devices = merge_scan_results(
            vec![
                PingScanResult {
                    ip: "192.168.1.20".into(),
                    latency_ms: 3.5,
                },
                PingScanResult {
                    ip: "192.168.1.10".into(),
                    latency_ms: 1.2,
                },
            ],
            vec![
                Device::arp("192.168.1.20", "aa:bb:cc:dd:ee:ff"),
                Device::arp("192.168.1.30", "11:22:33:44:55:66"),
            ],
        );

        assert_eq!(
            devices
                .iter()
                .map(|device| device.ip.as_str())
                .collect::<Vec<_>>(),
            vec!["192.168.1.10", "192.168.1.20", "192.168.1.30"]
        );
        assert_eq!(devices[0].source, DiscoverySource::Ping);
        assert!(devices[0].reachable);
        assert_eq!(devices[1].source, DiscoverySource::PingAndArp);
        assert_eq!(devices[1].mac, "aa:bb:cc:dd:ee:ff");
        assert_eq!(devices[1].latency_ms, Some(3.5));
        assert_eq!(devices[2].source, DiscoverySource::Arp);
        assert!(!devices[2].reachable);
    }

    #[test]
    fn parses_traceroute_hops_with_latency_and_timeouts() {
        let output = "\
traceroute to 1.1.1.1 (1.1.1.1), 12 hops max
 1  192.168.1.1  1.234 ms  1.100 ms  1.090 ms
 2  * * *
 3  203.0.113.1  8.750 ms * 9.000 ms
";

        let hops = parse_traceroute_output(output);

        assert_eq!(
            hops,
            vec![
                TraceHop::new(1, "192.168.1.1", Some(1.234)),
                TraceHop::new(2, "*", None),
                TraceHop::new(3, "203.0.113.1", Some(8.75)),
            ]
        );
    }

    #[test]
    fn scan_message_reports_empty_and_successful_results() {
        assert_eq!(
            scan_message(0, 0, 0),
            "Sin resultados. Revisa permisos de red o comandos ping/arp/traceroute."
        );
        assert_eq!(
            scan_message(12, 4, 3),
            "12 dispositivos, 4 con ping, 3 saltos a 1.1.1.1"
        );
    }
}
