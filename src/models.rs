use std::{collections::VecDeque, time::Instant};

pub(crate) const MAX_HISTORY: usize = 50;
pub(crate) const TRACE_TARGET: &str = "1.1.1.1";

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Screen {
    Welcome,
    Monitor,
    MultiPing,
    Topology,
}

#[derive(Clone)]
pub(crate) struct PingTarget {
    pub(crate) host: String,
    pub(crate) online: bool,
    pub(crate) latency_ms: Option<f64>,
    pub(crate) history: VecDeque<f64>,
    pub(crate) sent: u64,
    pub(crate) received: u64,
    pub(crate) last_checked: Option<Instant>,
}

impl PingTarget {
    pub(crate) fn new(host: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            online: false,
            latency_ms: None,
            history: VecDeque::new(),
            sent: 0,
            received: 0,
            last_checked: None,
        }
    }

    pub(crate) fn loss(&self) -> f64 {
        if self.sent == 0 {
            0.0
        } else {
            100.0 * (self.sent - self.received) as f64 / self.sent as f64
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DiscoverySource {
    Arp,
    Ping,
    PingAndArp,
}

impl DiscoverySource {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Arp => "arp",
            Self::Ping => "ping",
            Self::PingAndArp => "ping+arp",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Device {
    pub(crate) ip: String,
    pub(crate) mac: String,
    pub(crate) reachable: bool,
    pub(crate) latency_ms: Option<f64>,
    pub(crate) source: DiscoverySource,
}

impl Device {
    pub(crate) fn arp(ip: impl Into<String>, mac: impl Into<String>) -> Self {
        Self {
            ip: ip.into(),
            mac: mac.into(),
            reachable: false,
            latency_ms: None,
            source: DiscoverySource::Arp,
        }
    }

    pub(crate) fn ping(ip: impl Into<String>, latency_ms: f64) -> Self {
        Self {
            ip: ip.into(),
            mac: "desconocida".into(),
            reachable: true,
            latency_ms: Some(latency_ms),
            source: DiscoverySource::Ping,
        }
    }

    pub(crate) fn mark_reachable(&mut self, latency_ms: f64) {
        self.reachable = true;
        self.latency_ms = Some(latency_ms);
        self.source = match self.source {
            DiscoverySource::Arp => DiscoverySource::PingAndArp,
            DiscoverySource::Ping => DiscoverySource::Ping,
            DiscoverySource::PingAndArp => DiscoverySource::PingAndArp,
        };
    }

    pub(crate) fn status_label(&self) -> &'static str {
        if self.reachable { "PING" } else { "ARP" }
    }

    pub(crate) fn source_label(&self) -> &'static str {
        self.source.label()
    }

    pub(crate) fn latency_label(&self) -> String {
        self.latency_ms
            .map(|value| format!("{value:.1}"))
            .unwrap_or_else(|| "-".into())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PingScanResult {
    pub(crate) ip: String,
    pub(crate) latency_ms: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LocalNetwork {
    pub(crate) prefix: String,
    pub(crate) broadcast: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TraceHop {
    pub(crate) hop: u8,
    pub(crate) address: String,
    pub(crate) latency_ms: Option<f64>,
}

impl TraceHop {
    pub(crate) fn new(hop: u8, address: impl Into<String>, latency_ms: Option<f64>) -> Self {
        Self {
            hop,
            address: address.into(),
            latency_ms,
        }
    }

    pub(crate) fn status_label(&self) -> &'static str {
        if self.latency_ms.is_some() {
            "OK"
        } else {
            "SIN RESP."
        }
    }

    pub(crate) fn latency_label(&self) -> String {
        self.latency_ms
            .map(|value| format!("{value:.1}"))
            .unwrap_or_else(|| "-".into())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NetworkScan {
    pub(crate) devices: Vec<Device>,
    pub(crate) trace_hops: Vec<TraceHop>,
    pub(crate) message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_zero_packet_loss_before_first_ping() {
        assert_eq!(PingTarget::new("1.1.1.1").loss(), 0.0);
    }

    #[test]
    fn computes_partial_packet_loss() {
        let mut target = PingTarget::new("1.1.1.1");
        target.sent = 4;
        target.received = 3;
        assert_eq!(target.loss(), 25.0);
    }

    #[test]
    fn computes_total_packet_loss() {
        let mut target = PingTarget::new("1.1.1.1");
        target.sent = 4;
        assert_eq!(target.loss(), 100.0);
    }

    #[test]
    fn device_labels_explain_reachability_and_source() {
        let arp = Device::arp("192.168.1.10", "aa:bb:cc:dd:ee:ff");
        let ping = Device::ping("192.168.1.20", 7.25);

        assert_eq!(arp.status_label(), "ARP");
        assert_eq!(arp.latency_label(), "-");
        assert_eq!(arp.source_label(), "arp");
        assert_eq!(ping.status_label(), "PING");
        assert_eq!(ping.latency_label(), "7.2");
        assert_eq!(ping.source_label(), "ping");
    }

    #[test]
    fn trace_hop_labels_explain_status() {
        let ok = TraceHop::new(1, "192.168.1.1", Some(2.25));
        let timeout = TraceHop::new(2, "*", None);

        assert_eq!(ok.status_label(), "OK");
        assert_eq!(ok.latency_label(), "2.2");
        assert_eq!(timeout.status_label(), "SIN RESP.");
        assert_eq!(timeout.latency_label(), "-");
    }
}
