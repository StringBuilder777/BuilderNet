use std::{
    collections::{BTreeMap, VecDeque},
    io,
    net::Ipv4Addr,
    process::Command,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    symbols,
    text::{Line, Span},
    widgets::{
        Axis, Block, Borders, Chart, Dataset, GraphType, List, ListItem, Paragraph, Row, Scrollbar,
        ScrollbarOrientation, ScrollbarState, Table, Wrap,
    },
};

const MAX_HISTORY: usize = 50;
const SCAN_WORKERS: usize = 32;
const TRACE_TARGET: &str = "1.1.1.1";
const TRACE_MAX_HOPS: u8 = 12;

#[derive(Clone, Copy, PartialEq)]
enum Screen {
    Welcome,
    Monitor,
    MultiPing,
    Topology,
}

#[derive(Clone)]
struct PingTarget {
    host: String,
    online: bool,
    latency_ms: Option<f64>,
    history: VecDeque<f64>,
    sent: u64,
    received: u64,
    last_checked: Option<Instant>,
}

impl PingTarget {
    fn new(host: impl Into<String>) -> Self {
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

    fn loss(&self) -> f64 {
        if self.sent == 0 {
            0.0
        } else {
            100.0 * (self.sent - self.received) as f64 / self.sent as f64
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DiscoverySource {
    Arp,
    Ping,
    PingAndArp,
}

impl DiscoverySource {
    fn label(self) -> &'static str {
        match self {
            Self::Arp => "arp",
            Self::Ping => "ping",
            Self::PingAndArp => "ping+arp",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Device {
    ip: String,
    mac: String,
    reachable: bool,
    latency_ms: Option<f64>,
    source: DiscoverySource,
}

impl Device {
    fn arp(ip: impl Into<String>, mac: impl Into<String>) -> Self {
        Self {
            ip: ip.into(),
            mac: mac.into(),
            reachable: false,
            latency_ms: None,
            source: DiscoverySource::Arp,
        }
    }

    fn ping(ip: impl Into<String>, latency_ms: f64) -> Self {
        Self {
            ip: ip.into(),
            mac: "desconocida".into(),
            reachable: true,
            latency_ms: Some(latency_ms),
            source: DiscoverySource::Ping,
        }
    }

    fn mark_reachable(&mut self, latency_ms: f64) {
        self.reachable = true;
        self.latency_ms = Some(latency_ms);
        self.source = match self.source {
            DiscoverySource::Arp => DiscoverySource::PingAndArp,
            DiscoverySource::Ping => DiscoverySource::Ping,
            DiscoverySource::PingAndArp => DiscoverySource::PingAndArp,
        };
    }

    fn status_label(&self) -> &'static str {
        if self.reachable { "PING" } else { "ARP" }
    }

    fn source_label(&self) -> &'static str {
        self.source.label()
    }

    fn latency_label(&self) -> String {
        self.latency_ms
            .map(|value| format!("{value:.1}"))
            .unwrap_or_else(|| "-".into())
    }
}

#[derive(Clone, Debug, PartialEq)]
struct PingScanResult {
    ip: String,
    latency_ms: f64,
}

#[derive(Clone, Debug, PartialEq)]
struct LocalNetwork {
    prefix: String,
    broadcast: String,
}

#[derive(Clone, Debug, PartialEq)]
struct TraceHop {
    hop: u8,
    address: String,
    latency_ms: Option<f64>,
}

impl TraceHop {
    fn new(hop: u8, address: impl Into<String>, latency_ms: Option<f64>) -> Self {
        Self {
            hop,
            address: address.into(),
            latency_ms,
        }
    }

    fn status_label(&self) -> &'static str {
        if self.latency_ms.is_some() {
            "OK"
        } else {
            "SIN RESP."
        }
    }

    fn latency_label(&self) -> String {
        self.latency_ms
            .map(|value| format!("{value:.1}"))
            .unwrap_or_else(|| "-".into())
    }
}

#[derive(Clone, Debug, PartialEq)]
struct NetworkScan {
    devices: Vec<Device>,
    trace_hops: Vec<TraceHop>,
    message: String,
}

enum WorkerMessage {
    PingResult { host: String, latency: Option<f64> },
    ScanResult(NetworkScan),
}

struct App {
    screen: Screen,
    welcome_index: usize,
    selected_target: usize,
    selected_device: usize,
    trace_scroll: usize,
    targets: Vec<PingTarget>,
    devices: Vec<Device>,
    trace_hops: Vec<TraceHop>,
    gateway: String,
    interface: String,
    input: String,
    input_mode: bool,
    scanning: bool,
    scan_message: String,
    tx: Sender<WorkerMessage>,
    rx: Receiver<WorkerMessage>,
    last_ping_cycle: Instant,
    should_quit: bool,
}

impl App {
    fn new() -> Self {
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

    fn tick(&mut self) {
        self.process_worker_messages();

        if self.last_ping_cycle.elapsed() >= Duration::from_secs(2) {
            self.run_ping_cycle();
            self.last_ping_cycle = Instant::now();
        }
    }

    fn process_worker_messages(&mut self) {
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

    fn scan(&mut self) {
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

    fn start_scan_state(&mut self) -> bool {
        if self.scanning {
            return false;
        }
        self.scanning = true;
        self.scan_message = "Escaneando LAN /24 y traceando 1.1.1.1...".into();
        true
    }

    fn handle_key(&mut self, code: KeyCode) {
        if self.input_mode {
            match code {
                KeyCode::Esc => {
                    self.input_mode = false;
                    self.input.clear();
                }
                KeyCode::Enter => {
                    let host = self.input.trim();
                    if !host.is_empty() && !self.targets.iter().any(|t| t.host == host) {
                        self.targets.push(PingTarget::new(host));
                        self.selected_target = self.targets.len() - 1;
                    }
                    self.input_mode = false;
                    self.input.clear();
                }
                KeyCode::Backspace => {
                    self.input.pop();
                }
                KeyCode::Char(c) => self.input.push(c),
                _ => {}
            }
            return;
        }

        match code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Esc => self.screen = Screen::Welcome,
            KeyCode::Char('1') => self.screen = Screen::Monitor,
            KeyCode::Char('2') => self.screen = Screen::MultiPing,
            KeyCode::Char('3') => self.open_topology(),
            KeyCode::Char('s') => self.scan(),
            KeyCode::Char('a') if self.screen == Screen::MultiPing => {
                self.input_mode = true;
            }
            KeyCode::Char('d') if self.screen == Screen::MultiPing => {
                if self.targets.len() > 1 {
                    self.targets.remove(self.selected_target);
                    self.selected_target = self.selected_target.min(self.targets.len() - 1);
                }
            }
            KeyCode::Up => match self.screen {
                Screen::Welcome => self.welcome_index = self.welcome_index.saturating_sub(1),
                Screen::MultiPing => {
                    self.selected_target = self.selected_target.saturating_sub(1);
                }
                Screen::Topology => self.previous_device(),
                _ => {}
            },
            KeyCode::Down => match self.screen {
                Screen::Welcome => self.welcome_index = (self.welcome_index + 1).min(2),
                Screen::MultiPing => {
                    self.selected_target =
                        (self.selected_target + 1).min(self.targets.len().saturating_sub(1));
                }
                Screen::Topology => self.next_device(),
                _ => {}
            },
            KeyCode::PageUp if self.screen == Screen::Topology => self.scroll_trace_up(5),
            KeyCode::PageDown if self.screen == Screen::Topology => self.scroll_trace_down(5),
            KeyCode::Home if self.screen == Screen::Topology => {
                self.selected_device = 0;
                self.trace_scroll = 0;
            }
            KeyCode::End if self.screen == Screen::Topology => {
                self.selected_device = self.devices.len().saturating_sub(1);
                self.trace_scroll = self.trace_hops.len().saturating_sub(1);
            }
            KeyCode::Enter if self.screen == Screen::Welcome => {
                self.screen = match self.welcome_index {
                    0 => Screen::Monitor,
                    1 => Screen::MultiPing,
                    _ => Screen::Topology,
                };
                if self.screen == Screen::Topology {
                    self.scan();
                }
            }
            _ => {}
        }
    }

    fn open_topology(&mut self) {
        self.screen = Screen::Topology;
        if self.needs_initial_scan() {
            self.scan();
        }
    }

    fn needs_initial_scan(&self) -> bool {
        self.devices.is_empty() && self.trace_hops.is_empty() && !self.scanning
    }

    fn previous_device(&mut self) {
        self.selected_device = self.selected_device.saturating_sub(1);
    }

    fn next_device(&mut self) {
        self.selected_device = (self.selected_device + 1).min(self.devices.len().saturating_sub(1));
    }

    fn scroll_trace_up(&mut self, amount: usize) {
        self.trace_scroll = self.trace_scroll.saturating_sub(amount);
    }

    fn scroll_trace_down(&mut self, amount: usize) {
        self.trace_scroll =
            (self.trace_scroll + amount).min(self.trace_hops.len().saturating_sub(1));
    }
}

fn main() -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let mut app = App::new();

    let result = run(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn run(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App) -> io::Result<()> {
    while !app.should_quit {
        app.tick();
        terminal.draw(|frame| draw(frame, app))?;
        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            app.handle_key(key.code);
        }
    }
    Ok(())
}

fn draw(frame: &mut Frame, app: &App) {
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .split(frame.area());

    draw_header(frame, areas[0], app);
    match app.screen {
        Screen::Welcome => draw_welcome(frame, areas[1], app),
        Screen::Monitor => draw_monitor(frame, areas[1], app),
        Screen::MultiPing => draw_multi_ping(frame, areas[1], app),
        Screen::Topology => draw_topology(frame, areas[1], app),
    }
    draw_footer(frame, areas[2], app);
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let internet = app.targets.iter().skip(1).any(|target| target.online);
    let reachable_devices = app.devices.iter().filter(|device| device.reachable).count();
    let status = if internet { "ONLINE" } else { "COMPROBANDO" };
    let color = if internet {
        Color::Green
    } else {
        Color::Yellow
    };
    let title = Line::from(vec![
        Span::styled(
            " RED TUI ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(
            "  interfaz: {}  gateway: {}  red: {}/{}  internet: ",
            app.interface,
            app.gateway,
            reachable_devices,
            app.devices.len()
        )),
        Span::styled(
            status,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(title).block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn draw_welcome(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(25),
            Constraint::Length(11),
            Constraint::Min(3),
        ])
        .split(area);

    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                "Bienvenido al monitor de red",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from("Estado local, conectividad a Internet y descubrimiento"),
        ])
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true }),
        chunks[0],
    );

    let options = [
        ("1", "Monitor de red", "Estado general, gateway y destinos"),
        ("2", "Pings multiples", "Latencia y perdida por destino"),
        ("3", "Grafico de red", "Topologia LAN y trace WAN"),
    ];
    let items: Vec<ListItem> = options
        .iter()
        .enumerate()
        .map(|(index, (key, title, description))| {
            let marker = if index == app.welcome_index { ">" } else { " " };
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{marker} [{key}] {title:<18} "),
                    Style::default()
                        .fg(if index == app.welcome_index {
                            Color::Black
                        } else {
                            Color::Cyan
                        })
                        .bg(if index == app.welcome_index {
                            Color::Cyan
                        } else {
                            Color::Reset
                        })
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(*description),
            ]))
        })
        .collect();
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .title(" Selecciona un modulo ")
                .borders(Borders::ALL),
        ),
        chunks[1],
    );
}

fn draw_monitor(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(area);

    let online = app.targets.iter().filter(|target| target.online).count();
    let reachable_devices = app.devices.iter().filter(|device| device.reachable).count();
    let summary = vec![
        Line::from(vec![
            Span::styled("Interfaz: ", Style::default().fg(Color::DarkGray)),
            Span::styled(&app.interface, Style::default().fg(Color::Cyan)),
        ]),
        Line::from(vec![
            Span::styled("Gateway:  ", Style::default().fg(Color::DarkGray)),
            Span::raw(&app.gateway),
        ]),
        Line::from(vec![
            Span::styled("Destinos: ", Style::default().fg(Color::DarkGray)),
            Span::raw(format!("{} activos de {}", online, app.targets.len())),
        ]),
        Line::from(vec![
            Span::styled("Vecinos:  ", Style::default().fg(Color::DarkGray)),
            Span::raw(format!("{} detectados", app.devices.len())),
        ]),
        Line::from(vec![
            Span::styled("Ping LAN: ", Style::default().fg(Color::DarkGray)),
            Span::raw(format!("{reachable_devices} alcanzables")),
        ]),
        Line::from(vec![
            Span::styled("Metodo:   ", Style::default().fg(Color::DarkGray)),
            Span::raw("broadcast + barrido /24 + ARP + trace"),
        ]),
        Line::from(vec![
            Span::styled("Escaneo:  ", Style::default().fg(Color::DarkGray)),
            Span::raw(app.scan_message.as_str()),
        ]),
        Line::from(""),
        Line::from("Pulsa [2] para ver latencia detallada."),
        Line::from("Pulsa [3] para ver la topologia."),
        Line::from("Pulsa [s] para escanear la LAN y tracear 1.1.1.1."),
    ];
    frame.render_widget(
        Paragraph::new(summary).block(
            Block::default()
                .title(" Resumen de conectividad ")
                .borders(Borders::ALL),
        ),
        chunks[0],
    );

    let items: Vec<ListItem> = app
        .targets
        .iter()
        .map(|target| {
            let (icon, color) = if target.online {
                ("●", Color::Green)
            } else {
                ("○", Color::Red)
            };
            let latency = target
                .latency_ms
                .map(|value| format!("{value:.1} ms"))
                .unwrap_or_else(|| "---".into());
            ListItem::new(Line::from(vec![
                Span::styled(format!("{icon} "), Style::default().fg(color)),
                Span::raw(format!("{:<18} {:>9}", target.host, latency)),
            ]))
        })
        .collect();
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .title(" Destinos vigilados ")
                .borders(Borders::ALL),
        ),
        chunks[1],
    );
}

fn draw_multi_ping(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(area);

    let rows: Vec<Row> = app
        .targets
        .iter()
        .enumerate()
        .map(|(index, target)| {
            let selected = index == app.selected_target;
            let status = if target.online { "ONLINE" } else { "SIN RESP." };
            let latency = target
                .latency_ms
                .map(|value| format!("{value:.1}"))
                .unwrap_or_else(|| "-".into());
            Row::new(vec![
                if selected { ">" } else { " " }.to_string(),
                target.host.clone(),
                status.into(),
                latency,
                format!("{:.0}%", target.loss()),
            ])
            .style(if selected {
                Style::default().bg(Color::DarkGray)
            } else {
                Style::default()
            })
        })
        .collect();
    let table = Table::new(
        rows,
        [
            Constraint::Length(2),
            Constraint::Min(14),
            Constraint::Length(10),
            Constraint::Length(9),
            Constraint::Length(7),
        ],
    )
    .header(
        Row::new(["", "Destino", "Estado", "ms", "Perdida"]).style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
    )
    .block(
        Block::default()
            .title(" Pings multiples ")
            .borders(Borders::ALL),
    )
    .column_spacing(1);
    frame.render_widget(table, chunks[0]);

    let target = &app.targets[app.selected_target];
    let points: Vec<(f64, f64)> = target
        .history
        .iter()
        .enumerate()
        .map(|(i, value)| (i as f64, *value))
        .collect();
    let max_y = points
        .iter()
        .map(|(_, y)| *y)
        .fold(10.0_f64, f64::max)
        .ceil()
        + 5.0;
    let dataset = Dataset::default()
        .name(target.host.clone())
        .marker(symbols::Marker::Braille)
        .graph_type(GraphType::Line)
        .style(Style::default().fg(Color::Cyan))
        .data(&points);
    let chart = Chart::new(vec![dataset])
        .block(
            Block::default()
                .title(" Historial de latencia ")
                .borders(Borders::ALL),
        )
        .x_axis(
            Axis::default()
                .bounds([0.0, MAX_HISTORY as f64])
                .labels(["-50", "-25", "ahora"]),
        )
        .y_axis(Axis::default().bounds([0.0, max_y]).labels([
            Line::from("0"),
            Line::from(format!("{:.0} ms", max_y / 2.0)),
            Line::from(format!("{max_y:.0} ms")),
        ]));
    frame.render_widget(chart, chunks[1]);

    if app.input_mode {
        let popup = centered_rect(60, 20, area);
        frame.render_widget(ratatui::widgets::Clear, popup);
        frame.render_widget(
            Paragraph::new(app.input.as_str()).block(
                Block::default()
                    .title(" Agregar IP o dominio: Enter confirma, Esc cancela ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan)),
            ),
            popup,
        );
        frame.set_cursor_position((popup.x + app.input.len() as u16 + 1, popup.y + 1));
    }
}

fn draw_topology(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
        .split(area);
    let left_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
        .split(chunks[0]);

    let lines = topology_scene_lines(app, left_chunks[0]);
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(" Topologia LAN / vista isometrica ")
                    .borders(Borders::ALL),
            )
            .wrap(Wrap { trim: false }),
        left_chunks[0],
    );

    draw_trace(frame, left_chunks[1], app);

    let table_visible = visible_table_rows(chunks[1]);
    let table_offset = scroll_offset(app.devices.len(), app.selected_device, table_visible);
    let rows: Vec<Row> = app
        .devices
        .iter()
        .enumerate()
        .skip(table_offset)
        .take(table_visible)
        .map(|(index, device)| {
            let selected = index == app.selected_device;
            Row::new(vec![
                if selected { ">" } else { " " }.to_string(),
                device.status_label().to_string(),
                device.ip.clone(),
                device.mac.clone(),
                device.latency_label(),
                device.source_label().into(),
            ])
            .style(if selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else if device.reachable {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::DarkGray)
            })
        })
        .collect();
    let scan_title = if app.scanning {
        " Escaneando... "
    } else {
        " Dispositivos LAN alcanzados "
    };
    let table = Table::new(
        rows,
        [
            Constraint::Length(2),
            Constraint::Length(7),
            Constraint::Length(15),
            Constraint::Min(13),
            Constraint::Length(7),
            Constraint::Length(9),
        ],
    )
    .header(
        Row::new(["", "Estado", "IP", "MAC", "ms", "Via"]).style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
    )
    .block(Block::default().title(scan_title).borders(Borders::ALL))
    .column_spacing(1);
    frame.render_widget(table, chunks[1]);
    render_scrollbar(frame, chunks[1], app.devices.len(), table_offset);
}

fn draw_trace(frame: &mut Frame, area: Rect, app: &App) {
    let trace_visible = visible_table_rows(area);
    let trace_offset = app
        .trace_scroll
        .min(app.trace_hops.len().saturating_sub(trace_visible));
    let rows: Vec<Row> = app
        .trace_hops
        .iter()
        .skip(trace_offset)
        .take(trace_visible)
        .map(|hop| {
            Row::new(vec![
                hop.hop.to_string(),
                hop.status_label().to_string(),
                hop.address.clone(),
                hop.latency_label(),
            ])
            .style(if hop.latency_ms.is_some() {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::Yellow)
            })
        })
        .collect();
    let title = if app.scanning {
        format!(" Trace hacia {TRACE_TARGET}: ejecutando... ")
    } else if app.trace_hops.is_empty() {
        format!(" Trace hacia {TRACE_TARGET}: pulsa s ")
    } else {
        format!(" Trace hacia {TRACE_TARGET} ")
    };
    let table = Table::new(
        rows,
        [
            Constraint::Length(4),
            Constraint::Length(10),
            Constraint::Min(15),
            Constraint::Length(7),
        ],
    )
    .header(
        Row::new(["Hop", "Estado", "IP", "ms"]).style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
    )
    .block(Block::default().title(title).borders(Borders::ALL))
    .column_spacing(1);
    frame.render_widget(table, area);
    render_scrollbar(frame, area, app.trace_hops.len(), trace_offset);
}

fn topology_scene_lines(app: &App, area: Rect) -> Vec<Line<'static>> {
    let reachable_devices = app.devices.iter().filter(|device| device.reachable).count();
    let trace_status = if app.trace_hops.is_empty() {
        "trace pendiente".into()
    } else {
        format!("{} saltos WAN", app.trace_hops.len())
    };
    let mut lines = vec![
        Line::from(Span::styled(
            "                  ╭───────── INTERNET ─────────╮",
            Style::default()
                .fg(Color::Blue)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(format!("                  │ {TRACE_TARGET:<27} │")),
        Line::from(format!(
            "                  ╰───────────┬───────── {trace_status}"
        )),
        Line::from("                              ╱ ╲"),
        Line::from(format!(
            "               ╔══════════ ROUTER / {:<8} ══════════╗",
            app.interface
        )),
        Line::from(format!(
            "               ║ gateway {:<15}  LAN {:>3}/{:<3} ping ║",
            app.gateway,
            reachable_devices,
            app.devices.len()
        )),
        Line::from("               ╚══════════════╤══════════════╤══════╝"),
    ];

    if let Some(device) = app.devices.get(app.selected_device) {
        lines.push(Line::from(vec![
            Span::styled(
                "               foco: ",
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                format!(
                    "{} {}  {}  {} ms",
                    if device.reachable { "●" } else { "○" },
                    device.ip,
                    device.source_label(),
                    device.latency_label()
                ),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
    } else if app.scanning {
        lines.push(Line::from(
            "               foco: escaneando dispositivos...",
        ));
    } else {
        lines.push(Line::from(
            "               foco: sin dispositivos detectados",
        ));
    }

    lines.push(Line::from(""));
    let available = area.height.saturating_sub(12) as usize;
    let visible = available.max(1);
    let offset = scroll_offset(app.devices.len(), app.selected_device, visible);
    for (index, device) in app.devices.iter().enumerate().skip(offset).take(visible) {
        let selected = index == app.selected_device;
        let branch = if selected { "╰─▶" } else { "├──" };
        let marker = if device.reachable { "●" } else { "○" };
        let style = if selected {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else if device.reachable {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        lines.push(Line::from(Span::styled(
            format!(
                "                    {branch} {marker} {:<15} {:<9} {:>6} ms",
                device.ip,
                device.source_label(),
                device.latency_label()
            ),
            style,
        )));
    }
    if app.devices.is_empty() {
        let text = if app.scanning {
            "                    ╰── escaneando..."
        } else {
            "                    ╰── sin dispositivos detectados"
        };
        lines.push(Line::from(text));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("Estado: ", Style::default().fg(Color::DarkGray)),
        Span::raw(app.scan_message.clone()),
    ]));
    lines
}

fn visible_table_rows(area: Rect) -> usize {
    usize::from(area.height.saturating_sub(3)).max(1)
}

fn scroll_offset(item_count: usize, selected: usize, visible_count: usize) -> usize {
    if item_count <= visible_count {
        return 0;
    }
    let half = visible_count / 2;
    selected
        .saturating_sub(half)
        .min(item_count.saturating_sub(visible_count))
}

fn render_scrollbar(frame: &mut Frame, area: Rect, item_count: usize, offset: usize) {
    if item_count <= visible_table_rows(area) {
        return;
    }
    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(Some("↑"))
        .end_symbol(Some("↓"));
    let mut state = ScrollbarState::new(item_count).position(offset);
    frame.render_stateful_widget(
        scrollbar,
        area.inner(Margin {
            vertical: 1,
            horizontal: 0,
        }),
        &mut state,
    );
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let help = match app.screen {
        Screen::Welcome => "↑↓ seleccionar  Enter abrir  1/2/3 acceso rapido  q salir",
        Screen::Monitor => "1 monitor  2 multiping  3 topologia  s LAN+trace  Esc inicio  q salir",
        Screen::MultiPing => {
            "↑↓ seleccionar  a agregar  d eliminar  1/2/3 modulos  Esc inicio  q salir"
        }
        Screen::Topology => {
            "↑↓ dispositivo  PgUp/PgDn trace  Home/End extremos  s escanear  Esc inicio  q salir"
        }
    };
    frame.render_widget(
        Paragraph::new(help)
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::DarkGray))
            .block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

fn ping_once(host: &str) -> Option<f64> {
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

fn scan_local_network(gateway: &str) -> NetworkScan {
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

fn default_route() -> (String, String) {
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

    fn test_app() -> App {
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
    fn keeps_welcome_and_target_navigation_in_bounds() {
        let mut app = test_app();

        app.handle_key(KeyCode::Up);
        assert_eq!(app.welcome_index, 0);
        for _ in 0..5 {
            app.handle_key(KeyCode::Down);
        }
        assert_eq!(app.welcome_index, 2);

        app.screen = Screen::MultiPing;
        app.handle_key(KeyCode::Up);
        assert_eq!(app.selected_target, 0);
        for _ in 0..5 {
            app.handle_key(KeyCode::Down);
        }
        assert_eq!(app.selected_target, 1);
    }

    #[test]
    fn opens_selected_screen_and_quits() {
        let mut app = test_app();
        app.welcome_index = 2;

        app.handle_key(KeyCode::Enter);
        assert!(app.screen == Screen::Topology);
        app.handle_key(KeyCode::Char('1'));
        assert!(app.screen == Screen::Monitor);
        app.handle_key(KeyCode::Char('q'));
        assert!(app.should_quit);
    }

    #[test]
    fn adds_trimmed_unique_target() {
        let mut app = test_app();
        app.screen = Screen::MultiPing;
        app.handle_key(KeyCode::Char('a'));
        for character in "  example.com  ".chars() {
            app.handle_key(KeyCode::Char(character));
        }
        app.handle_key(KeyCode::Enter);

        assert_eq!(app.targets.len(), 3);
        assert_eq!(app.targets[2].host, "example.com");
        assert_eq!(app.selected_target, 2);
        assert!(!app.input_mode);
        assert!(app.input.is_empty());

        app.handle_key(KeyCode::Char('a'));
        for character in "example.com".chars() {
            app.handle_key(KeyCode::Char(character));
        }
        app.handle_key(KeyCode::Enter);
        assert_eq!(app.targets.len(), 3);
    }

    #[test]
    fn cancels_target_input() {
        let mut app = test_app();
        app.screen = Screen::MultiPing;
        app.handle_key(KeyCode::Char('a'));
        app.handle_key(KeyCode::Char('x'));
        app.handle_key(KeyCode::Esc);

        assert!(!app.input_mode);
        assert!(app.input.is_empty());
        assert_eq!(app.targets.len(), 2);
    }

    #[test]
    fn deletes_selected_target_but_keeps_last_target() {
        let mut app = test_app();
        app.screen = Screen::MultiPing;
        app.selected_target = 1;

        app.handle_key(KeyCode::Char('d'));
        assert_eq!(app.targets.len(), 1);
        assert_eq!(app.selected_target, 0);
        app.handle_key(KeyCode::Char('d'));
        assert_eq!(app.targets.len(), 1);
    }

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

    #[test]
    fn topology_navigation_selects_devices_and_scrolls_trace() {
        let mut app = test_app();
        app.screen = Screen::Topology;
        app.devices = vec![
            Device::arp("192.168.1.10", "aa:bb:cc:dd:ee:ff"),
            Device::arp("192.168.1.20", "11:22:33:44:55:66"),
            Device::arp("192.168.1.30", "22:33:44:55:66:77"),
        ];
        app.trace_hops = (1..=8)
            .map(|hop| TraceHop::new(hop, format!("192.0.2.{hop}"), Some(f64::from(hop))))
            .collect();

        app.handle_key(KeyCode::Down);
        assert_eq!(app.selected_device, 1);
        app.handle_key(KeyCode::Down);
        app.handle_key(KeyCode::Down);
        assert_eq!(app.selected_device, 2);
        app.handle_key(KeyCode::Up);
        assert_eq!(app.selected_device, 1);

        app.handle_key(KeyCode::PageDown);
        assert_eq!(app.trace_scroll, 5);
        app.handle_key(KeyCode::PageUp);
        assert_eq!(app.trace_scroll, 0);
        app.handle_key(KeyCode::End);
        assert_eq!(app.selected_device, 2);
        assert_eq!(app.trace_scroll, 7);
        app.handle_key(KeyCode::Home);
        assert_eq!(app.selected_device, 0);
        assert_eq!(app.trace_scroll, 0);
    }

    #[test]
    fn scroll_offset_keeps_selection_visible() {
        assert_eq!(scroll_offset(3, 0, 10), 0);
        assert_eq!(scroll_offset(20, 0, 5), 0);
        assert_eq!(scroll_offset(20, 10, 5), 8);
        assert_eq!(scroll_offset(20, 19, 5), 15);
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
    fn trace_hop_labels_explain_status() {
        let ok = TraceHop::new(1, "192.168.1.1", Some(2.25));
        let timeout = TraceHop::new(2, "*", None);

        assert_eq!(ok.status_label(), "OK");
        assert_eq!(ok.latency_label(), "2.2");
        assert_eq!(timeout.status_label(), "SIN RESP.");
        assert_eq!(timeout.latency_label(), "-");
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
