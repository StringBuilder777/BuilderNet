use std::{
    collections::VecDeque,
    io,
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
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    symbols,
    text::{Line, Span},
    widgets::{
        Axis, Block, Borders, Chart, Dataset, GraphType, List, ListItem, Paragraph, Row, Table,
        Wrap,
    },
};

const MAX_HISTORY: usize = 50;

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

#[derive(Clone)]
struct Device {
    ip: String,
    mac: String,
}

enum WorkerMessage {
    PingResult { host: String, latency: Option<f64> },
    ScanResult(Vec<Device>),
}

struct App {
    screen: Screen,
    welcome_index: usize,
    selected_target: usize,
    targets: Vec<PingTarget>,
    devices: Vec<Device>,
    gateway: String,
    interface: String,
    input: String,
    input_mode: bool,
    scanning: bool,
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
            targets: vec![
                PingTarget::new(gateway.clone()),
                PingTarget::new("1.1.1.1"),
                PingTarget::new("8.8.8.8"),
            ],
            devices: demo_devices(&gateway),
            gateway,
            interface,
            input: String::new(),
            input_mode: false,
            scanning: false,
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
                WorkerMessage::ScanResult(devices) => {
                    if !devices.is_empty() {
                        self.devices = devices;
                    }
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
        if self.scanning {
            return;
        }
        self.scanning = true;
        let tx = self.tx.clone();
        let gateway = self.gateway.clone();
        thread::spawn(move || {
            populate_arp_table(&gateway);
            let devices = arp_devices();
            let _ = tx.send(WorkerMessage::ScanResult(devices));
        });
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
            KeyCode::Char('3') => self.screen = Screen::Topology,
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
                _ => {}
            },
            KeyCode::Down => match self.screen {
                Screen::Welcome => self.welcome_index = (self.welcome_index + 1).min(2),
                Screen::MultiPing => {
                    self.selected_target =
                        (self.selected_target + 1).min(self.targets.len().saturating_sub(1));
                }
                _ => {}
            },
            KeyCode::Enter if self.screen == Screen::Welcome => {
                self.screen = match self.welcome_index {
                    0 => Screen::Monitor,
                    1 => Screen::MultiPing,
                    _ => Screen::Topology,
                }
            }
            _ => {}
        }
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
            "  interfaz: {}  gateway: {}  internet: ",
            app.interface, app.gateway
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
        ("3", "Grafico de red", "Escaneo y topologia local"),
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
        Line::from(""),
        Line::from("Pulsa [2] para ver latencia detallada."),
        Line::from("Pulsa [3] para ver la topologia."),
        Line::from("Pulsa [s] para realizar un escaneo ARP."),
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

    let mut lines = vec![
        Line::from(Span::styled(
            "                         INTERNET",
            Style::default()
                .fg(Color::Blue)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("                            │"),
        Line::from(format!("                    ┌─ {} ─┐", app.gateway)),
        Line::from(format!(
            "                    │ Router / {} │",
            app.interface
        )),
        Line::from("                    └──────┬──────┘"),
        Line::from("                           LAN"),
    ];
    for (index, device) in app.devices.iter().take(8).enumerate() {
        let branch = if index + 1 == app.devices.len().min(8) {
            "└──"
        } else {
            "├──"
        };
        lines.push(Line::from(format!(
            "                         {branch} {:<15} {}",
            device.ip, device.mac
        )));
    }
    if app.devices.is_empty() {
        lines.push(Line::from(
            "                         └── sin vecinos detectados",
        ));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(
                Block::default()
                    .title(" Topologia descubierta ")
                    .borders(Borders::ALL),
            )
            .wrap(Wrap { trim: false }),
        chunks[0],
    );

    let items: Vec<ListItem> = app
        .devices
        .iter()
        .map(|device| ListItem::new(format!("{}  {}", device.ip, device.mac)))
        .collect();
    let scan_title = if app.scanning {
        " Escaneando... "
    } else {
        " Vecinos ARP "
    };
    frame.render_widget(
        List::new(items).block(Block::default().title(scan_title).borders(Borders::ALL)),
        chunks[1],
    );
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let help = match app.screen {
        Screen::Welcome => "↑↓ seleccionar  Enter abrir  1/2/3 acceso rapido  q salir",
        Screen::Monitor => "1 monitor  2 multiping  3 topologia  s escanear  Esc inicio  q salir",
        Screen::MultiPing => {
            "↑↓ seleccionar  a agregar  d eliminar  1/2/3 modulos  Esc inicio  q salir"
        }
        Screen::Topology => "s escanear  1/2/3 modulos  Esc inicio  q salir",
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

fn populate_arp_table(gateway: &str) {
    let octets: Vec<&str> = gateway.split('.').collect();
    if octets.len() != 4 {
        return;
    }
    let prefix = format!("{}.{}.{}.", octets[0], octets[1], octets[2]);
    let mut workers = Vec::new();
    for start in 1..=16 {
        let prefix = prefix.clone();
        workers.push(thread::spawn(move || {
            for last_octet in (start..=254).step_by(16) {
                let _ = ping_once(&format!("{prefix}{last_octet}"));
            }
        }));
    }
    for worker in workers {
        let _ = worker.join();
    }
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
        Some(Device {
            ip: ip.into(),
            mac: mac.into(),
        })
    } else {
        None
    }
}

fn demo_devices(gateway: &str) -> Vec<Device> {
    vec![
        Device {
            ip: gateway.into(),
            mac: "gateway".into(),
        },
        Device {
            ip: "192.168.1.20".into(),
            mac: "pc-demo".into(),
        },
        Device {
            ip: "192.168.1.30".into(),
            mac: "wifi-demo".into(),
        },
    ]
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
            targets: vec![PingTarget::new("192.168.1.1"), PingTarget::new("1.1.1.1")],
            devices: Vec::new(),
            gateway: "192.168.1.1".into(),
            interface: "test0".into(),
            input: String::new(),
            input_mode: false,
            scanning: false,
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
            .send(WorkerMessage::ScanResult(vec![Device {
                ip: "192.168.1.50".into(),
                mac: "aa:bb:cc:dd:ee:ff".into(),
            }]))
            .unwrap();
        app.process_worker_messages();

        assert!(!app.scanning);
        assert_eq!(app.devices.len(), 1);
        assert_eq!(app.devices[0].ip, "192.168.1.50");
    }

    #[test]
    fn keeps_existing_devices_when_scan_result_is_empty() {
        let mut app = test_app();
        app.devices = demo_devices(&app.gateway);
        app.scanning = true;
        app.tx.send(WorkerMessage::ScanResult(Vec::new())).unwrap();
        app.process_worker_messages();

        assert!(!app.scanning);
        assert_eq!(app.devices.len(), 3);
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
}
