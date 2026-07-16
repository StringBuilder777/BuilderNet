use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    symbols,
    text::{Line, Span},
    widgets::{
        Axis, Block, Borders, Chart, Dataset, GraphType, List, ListItem, Paragraph, Row, Scrollbar,
        ScrollbarOrientation, ScrollbarState, Table, Wrap,
    },
};

use crate::{
    app::App,
    models::{MAX_HISTORY, Screen, TRACE_TARGET},
};

pub(crate) fn draw(frame: &mut Frame, app: &App) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_offset_keeps_selection_visible() {
        assert_eq!(scroll_offset(3, 0, 10), 0);
        assert_eq!(scroll_offset(20, 0, 5), 0);
        assert_eq!(scroll_offset(20, 10, 5), 8);
        assert_eq!(scroll_offset(20, 19, 5), 15);
    }
}
