use std::{
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

use crate::{
    models::{Device, MAX_HISTORY, NetworkScan, PingTarget, Screen, TraceHop},
    network::{default_route, ping_once, scan_local_network},
};

pub(crate) enum WorkerMessage {
    PingResult { host: String, latency: Option<f64> },
    ScanResult(NetworkScan),
}

pub(crate) struct App {
    pub(crate) screen: Screen,
    pub(crate) welcome_index: usize,
    pub(crate) selected_target: usize,
    pub(crate) selected_device: usize,
    pub(crate) trace_scroll: usize,
    pub(crate) targets: Vec<PingTarget>,
    pub(crate) devices: Vec<Device>,
    pub(crate) trace_hops: Vec<TraceHop>,
    pub(crate) gateway: String,
    pub(crate) interface: String,
    pub(crate) input: String,
    pub(crate) input_mode: bool,
    pub(crate) scanning: bool,
    pub(crate) scan_message: String,
    pub(crate) tx: Sender<WorkerMessage>,
    pub(crate) rx: Receiver<WorkerMessage>,
    pub(crate) last_ping_cycle: Instant,
    pub(crate) should_quit: bool,
}

impl App {
    pub(crate) fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        let (gateway, interface) = default_route();
        Self {
            screen: Screen::Welcome,
            welcome_index: 0,
            selected_target: 0,
            selected_device: 0,
            trace_scroll: 0,
            targets: vec![
                PingTarget::new(gateway.clone()),
                PingTarget::new("1.1.1.1"),
                PingTarget::new("8.8.8.8"),
            ],
            devices: Vec::new(),
            trace_hops: Vec::new(),
            gateway,
            interface,
            input: String::new(),
            input_mode: false,
            scanning: false,
            scan_message: "Sin escaneo. Abre Topologia o pulsa s.".into(),
            tx,
            rx,
            last_ping_cycle: Instant::now() - Duration::from_secs(5),
            should_quit: false,
        }
    }

    pub(crate) fn tick(&mut self) {
        self.process_worker_messages();

        if self.last_ping_cycle.elapsed() >= Duration::from_secs(2) {
            self.run_ping_cycle();
            self.last_ping_cycle = Instant::now();
        }
    }

    pub(crate) fn process_worker_messages(&mut self) {
        while let Ok(message) = self.rx.try_recv() {
            match message {
                WorkerMessage::PingResult { host, latency } => {
                    if let Some(target) = self.targets.iter_mut().find(|target| target.host == host)
                    {
                        target.sent += 1;
                        target.online = latency.is_some();
                        target.latency_ms = latency;
                        target.last_checked = Some(Instant::now());
                        if let Some(value) = latency {
                            target.received += 1;
                            target.history.push_back(value);
                            while target.history.len() > MAX_HISTORY {
                                target.history.pop_front();
                            }
                        }
                    }
                }
                WorkerMessage::ScanResult(scan) => {
                    self.devices = scan.devices;
                    self.trace_hops = scan.trace_hops;
                    self.scan_message = scan.message;
                    self.selected_device = self
                        .selected_device
                        .min(self.devices.len().saturating_sub(1));
                    self.trace_scroll = self
                        .trace_scroll
                        .min(self.trace_hops.len().saturating_sub(1));
                    self.scanning = false;
                }
            }
        }
    }

    fn run_ping_cycle(&self) {
        for target in &self.targets {
            let tx = self.tx.clone();
            let host = target.host.clone();
            thread::spawn(move || {
                let latency = ping_once(&host);
                let _ = tx.send(WorkerMessage::PingResult { host, latency });
            });
        }
    }

    pub(crate) fn scan(&mut self) {
        if !self.start_scan_state() {
            return;
        }
        let tx = self.tx.clone();
        let gateway = self.gateway.clone();
        thread::spawn(move || {
            let scan = scan_local_network(&gateway);
            let _ = tx.send(WorkerMessage::ScanResult(scan));
        });
    }

    pub(crate) fn start_scan_state(&mut self) -> bool {
        if self.scanning {
            return false;
        }
        self.scanning = true;
        self.scan_message = "Escaneando LAN /24 y traceando 1.1.1.1...".into();
        true
    }

    pub(crate) fn open_topology(&mut self) {
        self.screen = Screen::Topology;
        if self.needs_initial_scan() {
            self.scan();
        }
    }

    pub(crate) fn needs_initial_scan(&self) -> bool {
        self.devices.is_empty() && self.trace_hops.is_empty() && !self.scanning
    }

    pub(crate) fn previous_device(&mut self) {
        self.selected_device = self.selected_device.saturating_sub(1);
    }

    pub(crate) fn next_device(&mut self) {
        self.selected_device = (self.selected_device + 1).min(self.devices.len().saturating_sub(1));
    }

    pub(crate) fn scroll_trace_up(&mut self, amount: usize) {
        self.trace_scroll = self.trace_scroll.saturating_sub(amount);
    }

    pub(crate) fn scroll_trace_down(&mut self, amount: usize) {
        self.trace_scroll =
            (self.trace_scroll + amount).min(self.trace_hops.len().saturating_sub(1));
    }
}

#[cfg(test)]
pub(crate) fn test_app() -> App {
    let (tx, rx) = mpsc::channel();
    App {
        screen: Screen::Welcome,
        welcome_index: 0,
        selected_target: 0,
        selected_device: 0,
        trace_scroll: 0,
        targets: vec![PingTarget::new("192.168.1.1"), PingTarget::new("1.1.1.1")],
        devices: Vec::new(),
        trace_hops: Vec::new(),
        gateway: "192.168.1.1".into(),
        interface: "test0".into(),
        input: String::new(),
        input_mode: false,
        scanning: false,
        scan_message: "Sin escaneo. Abre Topologia o pulsa s.".into(),
        tx,
        rx,
        last_ping_cycle: Instant::now(),
        should_quit: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Device, TraceHop};

    #[test]
    fn processes_successful_and_failed_ping_results() {
        let mut app = test_app();
        app.tx
            .send(WorkerMessage::PingResult {
                host: "1.1.1.1".into(),
                latency: Some(10.0),
            })
            .unwrap();
        app.process_worker_messages();

        let target = &app.targets[1];
        assert!(target.online);
        assert_eq!(target.sent, 1);
        assert_eq!(target.received, 1);
        assert_eq!(target.latency_ms, Some(10.0));
        assert_eq!(target.history.back(), Some(&10.0));
        assert!(target.last_checked.is_some());

        app.tx
            .send(WorkerMessage::PingResult {
                host: "1.1.1.1".into(),
                latency: None,
            })
            .unwrap();
        app.process_worker_messages();

        let target = &app.targets[1];
        assert!(!target.online);
        assert_eq!(target.sent, 2);
        assert_eq!(target.received, 1);
        assert_eq!(target.latency_ms, None);
        assert_eq!(target.history.len(), 1);
    }

    #[test]
    fn limits_ping_history() {
        let mut app = test_app();
        for latency in 0..=MAX_HISTORY {
            app.tx
                .send(WorkerMessage::PingResult {
                    host: "1.1.1.1".into(),
                    latency: Some(latency as f64),
                })
                .unwrap();
        }
        app.process_worker_messages();

        let history = &app.targets[1].history;
        assert_eq!(history.len(), MAX_HISTORY);
        assert_eq!(history.front(), Some(&1.0));
        assert_eq!(history.back(), Some(&(MAX_HISTORY as f64)));
    }

    #[test]
    fn processes_scan_results_and_clears_scanning_state() {
        let mut app = test_app();
        app.scanning = true;
        app.tx
            .send(WorkerMessage::ScanResult(NetworkScan {
                devices: vec![Device::arp("192.168.1.50", "aa:bb:cc:dd:ee:ff")],
                trace_hops: vec![TraceHop::new(1, "192.168.1.1", Some(1.4))],
                message: "1 dispositivos, 0 con ping, 1 saltos a 1.1.1.1".into(),
            }))
            .unwrap();
        app.process_worker_messages();

        assert!(!app.scanning);
        assert_eq!(app.devices.len(), 1);
        assert_eq!(app.devices[0].ip, "192.168.1.50");
        assert_eq!(app.trace_hops.len(), 1);
        assert_eq!(app.trace_hops[0].address, "192.168.1.1");
        assert_eq!(
            app.scan_message,
            "1 dispositivos, 0 con ping, 1 saltos a 1.1.1.1"
        );
    }

    #[test]
    fn clears_existing_devices_when_scan_result_is_empty() {
        let mut app = test_app();
        app.devices = vec![
            Device::arp("192.168.1.254", "aa:bb:cc:dd:ee:ff"),
            Device::arp("192.168.1.20", "11:22:33:44:55:66"),
        ];
        app.scanning = true;
        app.tx
            .send(WorkerMessage::ScanResult(NetworkScan {
                devices: Vec::new(),
                trace_hops: vec![TraceHop::new(1, "*", None)],
                message: "0 dispositivos, 0 con ping, 1 saltos a 1.1.1.1".into(),
            }))
            .unwrap();
        app.process_worker_messages();

        assert!(!app.scanning);
        assert!(app.devices.is_empty());
        assert_eq!(app.trace_hops.len(), 1);
    }

    #[test]
    fn topology_needs_initial_scan_only_before_results_exist() {
        let mut app = test_app();

        assert!(app.needs_initial_scan());

        app.scanning = true;
        assert!(!app.needs_initial_scan());
        app.scanning = false;
        app.devices
            .push(Device::arp("192.168.1.50", "aa:bb:cc:dd:ee:ff"));
        assert!(!app.needs_initial_scan());
        app.devices.clear();
        app.trace_hops
            .push(TraceHop::new(1, "192.168.1.1", Some(1.0)));
        assert!(!app.needs_initial_scan());
    }

    #[test]
    fn scan_sets_visible_running_message() {
        let mut app = test_app();

        assert!(app.start_scan_state());

        assert!(app.scanning);
        assert_eq!(
            app.scan_message,
            "Escaneando LAN /24 y traceando 1.1.1.1..."
        );
        assert!(!app.start_scan_state());
    }
}
