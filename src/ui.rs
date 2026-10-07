use crossterm::event::KeyCode;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState, Wrap},
    Frame,
};

use crate::models::{LogEntry, Service};

pub const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn spinner(tick: usize) -> &'static str {
    SPINNER_FRAMES[tick % SPINNER_FRAMES.len()]
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BannerType {
    Info,
    Success,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionTrafficItem {
    pub revision_name: String,
    pub percent: i32,
    pub tag: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrafficModalStatus {
    FetchingRevisions,
    Idle,
    Confirming,
    Submitting,
    Success(String),
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrafficModalAction {
    None,
    Close,
    Submit(Vec<(String, i32)>),
}

#[derive(Debug, Clone)]
pub struct TrafficModalState {
    pub service_name: String,
    pub revisions: Vec<RevisionTrafficItem>,
    pub selected_index: usize,
    pub input_buffer: String,
    pub status: TrafficModalStatus,
}

impl TrafficModalState {
    pub fn new(service_name: String) -> Self {
        Self {
            service_name,
            revisions: Vec::new(),
            selected_index: 0,
            input_buffer: String::new(),
            status: TrafficModalStatus::FetchingRevisions,
        }
    }

    pub fn total_percent(&self) -> i32 {
        self.revisions.iter().map(|r| r.percent).sum()
    }

    pub fn handle_key(&mut self, key: KeyCode) -> TrafficModalAction {
        if matches!(self.status, TrafficModalStatus::Submitting | TrafficModalStatus::FetchingRevisions) {
            if key == KeyCode::Esc {
                return TrafficModalAction::Close;
            }
            return TrafficModalAction::None;
        }

        match key {
            KeyCode::Esc => {
                if self.status == TrafficModalStatus::Confirming {
                    self.status = TrafficModalStatus::Idle;
                    TrafficModalAction::None
                } else {
                    TrafficModalAction::Close
                }
            }
            KeyCode::Enter => match &self.status {
                TrafficModalStatus::Success(_) => TrafficModalAction::Close,
                TrafficModalStatus::Submitting | TrafficModalStatus::FetchingRevisions => {
                    TrafficModalAction::None
                }
                TrafficModalStatus::Confirming => {
                    let total = self.total_percent();
                    if total != 100 {
                        self.status = TrafficModalStatus::Error(format!(
                            "Total traffic must equal exactly 100% (currently {}%)",
                            total
                        ));
                        TrafficModalAction::None
                    } else {
                        self.status = TrafficModalStatus::Submitting;
                        let splits: Vec<(String, i32)> = self
                            .revisions
                            .iter()
                            .filter(|r| r.percent > 0)
                            .map(|r| (r.revision_name.clone(), r.percent))
                            .collect();
                        TrafficModalAction::Submit(splits)
                    }
                }
                TrafficModalStatus::Idle | TrafficModalStatus::Error(_) => {
                    let total = self.total_percent();
                    if total != 100 {
                        self.status = TrafficModalStatus::Error(format!(
                            "Total traffic must equal exactly 100% (currently {}%)",
                            total
                        ));
                        TrafficModalAction::None
                    } else {
                        self.status = TrafficModalStatus::Confirming;
                        TrafficModalAction::None
                    }
                }
            },
            KeyCode::Up | KeyCode::Char('k') => {
                if self.status == TrafficModalStatus::Confirming {
                    self.status = TrafficModalStatus::Idle;
                }
                self.selected_index = self.selected_index.saturating_sub(1);
                self.input_buffer.clear();
                TrafficModalAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.status == TrafficModalStatus::Confirming {
                    self.status = TrafficModalStatus::Idle;
                }
                if !self.revisions.is_empty() && self.selected_index + 1 < self.revisions.len() {
                    self.selected_index += 1;
                }
                self.input_buffer.clear();
                TrafficModalAction::None
            }
            KeyCode::Right | KeyCode::Char('+') | KeyCode::Char('=') | KeyCode::Char('l') => {
                self.reset_confirm_or_error();
                if let Some(item) = self.revisions.get_mut(self.selected_index) {
                    item.percent = (item.percent + 5).min(100);
                    self.input_buffer = item.percent.to_string();
                }
                TrafficModalAction::None
            }
            KeyCode::Left | KeyCode::Char('-') | KeyCode::Char('h') => {
                self.reset_confirm_or_error();
                if let Some(item) = self.revisions.get_mut(self.selected_index) {
                    item.percent = (item.percent - 5).max(0);
                    self.input_buffer = item.percent.to_string();
                }
                TrafficModalAction::None
            }
            KeyCode::Char(']') => {
                self.reset_confirm_or_error();
                if let Some(item) = self.revisions.get_mut(self.selected_index) {
                    item.percent = (item.percent + 1).min(100);
                    self.input_buffer = item.percent.to_string();
                }
                TrafficModalAction::None
            }
            KeyCode::Char('[') => {
                self.reset_confirm_or_error();
                if let Some(item) = self.revisions.get_mut(self.selected_index) {
                    item.percent = (item.percent - 1).max(0);
                    self.input_buffer = item.percent.to_string();
                }
                TrafficModalAction::None
            }
            KeyCode::Char('c') => {
                self.reset_confirm_or_error();
                if let Some(item) = self.revisions.get_mut(self.selected_index) {
                    item.percent = 0;
                }
                self.input_buffer.clear();
                TrafficModalAction::None
            }
            KeyCode::Char('C') => {
                self.reset_confirm_or_error();
                for item in &mut self.revisions {
                    item.percent = 0;
                }
                self.input_buffer.clear();
                TrafficModalAction::None
            }
            KeyCode::Char(d) if d.is_ascii_digit() => {
                self.reset_confirm_or_error();
                if !self.revisions.is_empty() && self.input_buffer.len() < 3 {
                    self.input_buffer.push(d);
                    if let Ok(val) = self.input_buffer.parse::<i32>()
                        && let Some(item) = self.revisions.get_mut(self.selected_index)
                    {
                        item.percent = val.clamp(0, 100);
                    }
                }
                TrafficModalAction::None
            }
            KeyCode::Backspace => {
                self.reset_confirm_or_error();
                if !self.input_buffer.is_empty() {
                    self.input_buffer.pop();
                    let val = self.input_buffer.parse::<i32>().unwrap_or(0);
                    if let Some(item) = self.revisions.get_mut(self.selected_index) {
                        item.percent = val.clamp(0, 100);
                    }
                } else if let Some(item) = self.revisions.get_mut(self.selected_index) {
                    item.percent = 0;
                }
                TrafficModalAction::None
            }
            _ => TrafficModalAction::None,
        }
    }

    fn reset_confirm_or_error(&mut self) {
        if matches!(self.status, TrafficModalStatus::Confirming | TrafficModalStatus::Error(_)) {
            self.status = TrafficModalStatus::Idle;
        }
    }
}

pub fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let percent_x = percent_x.min(100);
    let percent_y = percent_y.min(100);

    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

pub struct UiState<'a> {
    pub project: &'a str,
    pub region: &'a str,
    pub services: &'a [Service],
    pub table_state: &'a mut TableState,
    pub is_fetching_services: bool,
    pub services_error: Option<&'a str>,
    pub is_auth_error: bool,
    pub show_logs: bool,
    pub logs: &'a [LogEntry],
    pub selected_service_name: &'a str,
    pub is_fetching_logs: bool,
    pub log_error_msg: Option<&'a str>,
    pub banner_message: Option<(&'a str, &'a BannerType)>,
    pub traffic_modal: Option<&'a TrafficModalState>,
    pub spinner_tick: usize,
}

pub fn render_ui(f: &mut Frame, state: UiState) {
    // If services are empty and there is an error, show the startup error screen
    if state.services.is_empty()
        && let Some(err) = state.services_error {
            if state.is_auth_error {
                render_auth_error(f, err);
                return;
            } else {
                render_startup_error(f, err);
                return;
            }
        }

    let banner_active = state.banner_message.is_some();

    // Compute main vertical layout
    let constraints = if banner_active {
        if state.show_logs {
            vec![
                Constraint::Length(3), // Header
                Constraint::Length(3), // Banner
                Constraint::Percentage(38), // Services table
                Constraint::Percentage(42), // Logs pane
                Constraint::Length(3), // Footer
            ]
        } else {
            vec![
                Constraint::Length(3), // Header
                Constraint::Length(3), // Banner
                Constraint::Min(6),    // Services table
                Constraint::Length(6), // Traffic allocations pane
                Constraint::Length(3), // Footer
            ]
        }
    } else if state.show_logs {
        vec![
            Constraint::Length(3), // Header
            Constraint::Percentage(40), // Services table
            Constraint::Percentage(45), // Logs pane
            Constraint::Length(3), // Footer
        ]
    } else {
        vec![
            Constraint::Length(3), // Header
            Constraint::Min(6),    // Services table
            Constraint::Length(6), // Traffic allocations pane
            Constraint::Length(3), // Footer
        ]
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(f.area());

    let mut chunk_idx = 0;

    // 1. Header Banner
    let header_chunk = chunks[chunk_idx];
    chunk_idx += 1;

    let fetching_indicator = if state.is_fetching_services {
        format!(" [ {} Fetching services... ]", spinner(state.spinner_tick))
    } else {
        String::new()
    };

    let header = Paragraph::new(format!(
        " gcr-top | Project: {} | Region: {}{} | [s]: Traffic Split | [l]: Logs | [r]: Refresh | [q]: Quit",
        state.project, state.region, fetching_indicator
    ))
    .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
    .block(Block::default().borders(Borders::ALL).title(" Google Cloud Run Monitor "));
    f.render_widget(header, header_chunk);

    // Optional Error / Info Banner
    if let Some((msg, b_type)) = state.banner_message {
        let banner_chunk = chunks[chunk_idx];
        chunk_idx += 1;

        let (fg_color, title) = match b_type {
            BannerType::Error => (Color::Red, " Alert "),
            BannerType::Success => (Color::Green, " Notice "),
            BannerType::Info => (Color::Cyan, " Info "),
        };

        let banner = Paragraph::new(format!(" {} | Press [Esc] to dismiss", msg))
            .style(Style::default().fg(fg_color).add_modifier(Modifier::BOLD))
            .wrap(Wrap { trim: true })
            .block(Block::default().borders(Borders::ALL).title(title).border_style(Style::default().fg(fg_color)));
        f.render_widget(banner, banner_chunk);
    }

    // 2. Services Table
    let table_chunk = chunks[chunk_idx];
    chunk_idx += 1;

    let rows: Vec<Row> = state
        .services
        .iter()
        .map(|s| {
            let status_style = if s.is_ready() {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::Red)
            };
            let status_text = if s.is_ready() { "RUNNING" } else { "DEGRADED" };

            let traffic_style = if s.traffic_summary().contains("(Split)") {
                Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            Row::new(vec![
                Cell::from(s.short_name().to_string()),
                Cell::from(status_text).style(status_style),
                Cell::from(s.traffic_summary()).style(traffic_style),
                Cell::from(s.primary_revision().to_string()),
                Cell::from(s.uri.clone().unwrap_or_else(|| "-".to_string())),
            ])
        })
        .collect();

    let table_title = if state.is_fetching_services {
        format!(" Services [ {} Fetching... ] ", spinner(state.spinner_tick))
    } else {
        format!(" Services ({}) ", state.services.len())
    };

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(20),
            Constraint::Percentage(12),
            Constraint::Percentage(18),
            Constraint::Percentage(22),
            Constraint::Percentage(28),
        ],
    )
    .header(
        Row::new(vec!["SERVICE", "STATUS", "TRAFFIC", "LATEST REVISION", "URI"])
            .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
    )
    .block(Block::default().borders(Borders::ALL).title(table_title))
    .highlight_style(Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD));

    f.render_stateful_widget(table, table_chunk, state.table_state);

    // 3. Middle Pane: Logs or Traffic Allocation Inspector
    let middle_chunk = chunks[chunk_idx];
    chunk_idx += 1;

    if state.show_logs {
        let pane_height = middle_chunk.height.saturating_sub(2) as usize;

        let log_widget = if let Some(err) = state.log_error_msg {
            Paragraph::new(vec![
                Line::from(Span::styled(
                    "API Error retrieving logs:",
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::raw(err)),
            ])
            .wrap(Wrap { trim: true })
            .block(Block::default().borders(Borders::ALL).title(" Error ").border_style(Style::default().fg(Color::Red)))
        } else if state.is_fetching_logs && state.logs.is_empty() {
            Paragraph::new(format!(" {} Streaming logs from Cloud Logging...", spinner(state.spinner_tick)))
                .style(Style::default().fg(Color::Yellow))
                .block(Block::default().borders(Borders::ALL).title(format!(" Logs: {} ", state.selected_service_name)))
        } else if state.logs.is_empty() {
            Paragraph::new("No entries found.")
                .style(Style::default().fg(Color::DarkGray))
                .block(Block::default().borders(Borders::ALL).title(format!(" Logs: {} ", state.selected_service_name)))
        } else {
            let total_lines = state.logs.len();
            let scroll_offset = if total_lines > pane_height {
                (total_lines - pane_height) as u16
            } else {
                0
            };

            let log_lines: Vec<Line> = state
                .logs
                .iter()
                .filter_map(|l| {
                    l.message().map(|msg| {
                        let sev = l.severity.as_deref().unwrap_or("INFO");
                        let sev_color = match sev {
                            "ERROR" | "CRITICAL" => Color::Red,
                            "WARNING" => Color::Yellow,
                            _ => Color::Green,
                        };
                        let time = l.timestamp.as_deref().unwrap_or("").split('.').next().unwrap_or("");
                        Line::from(vec![
                            Span::styled(format!("[{}] ", time), Style::default().fg(Color::DarkGray)),
                            Span::styled(format!("{:<7} ", sev), Style::default().fg(sev_color).add_modifier(Modifier::BOLD)),
                            Span::raw(msg),
                        ])
                    })
                })
                .collect();

            let title = if state.is_fetching_logs {
                format!(
                    " Live Logs: {} [ {} Refreshing... ] ",
                    state.selected_service_name,
                    spinner(state.spinner_tick)
                )
            } else {
                format!(
                    " Live Logs: {} (Tail mode, auto-refresh 7s) ",
                    state.selected_service_name
                )
            };

            Paragraph::new(log_lines)
                .scroll((scroll_offset, 0))
                .block(Block::default().borders(Borders::ALL).title(title))
        };

        f.render_widget(log_widget, middle_chunk);
    } else {
        // Traffic Allocations Inspector
        let selected_svc = state.table_state.selected().and_then(|idx| state.services.get(idx));
        let traffic_lines = match selected_svc {
            Some(svc) => {
                let details = svc.traffic_details();
                details
                    .into_iter()
                    .map(|(rev, pct, tag)| {
                        let pct_color = if pct == 100 { Color::Green } else { Color::Cyan };
                        Line::from(vec![
                            Span::raw("  • Revision: "),
                            Span::styled(format!("{:<28}", rev), Style::default().add_modifier(Modifier::BOLD)),
                            Span::raw(" Traffic: "),
                            Span::styled(format!("{:>3}%", pct), Style::default().fg(pct_color).add_modifier(Modifier::BOLD)),
                            Span::raw("  Tag: "),
                            Span::styled(tag, Style::default().fg(Color::Yellow)),
                        ])
                    })
                    .collect()
            }
            None => vec![Line::from(Span::raw("  No service selected"))],
        };

        let svc_name = selected_svc.map(|s| s.short_name()).unwrap_or("-");
        let traffic_widget = Paragraph::new(traffic_lines)
            .block(Block::default().borders(Borders::ALL).title(format!(" Traffic Allocations: {} ", svc_name)));
        f.render_widget(traffic_widget, middle_chunk);
    }

    // 4. Footer
    let footer_chunk = chunks[chunk_idx];
    let footer_text = if let Some(modal) = state.traffic_modal {
        if modal.status == TrafficModalStatus::Confirming {
            " [Enter]: Confirm traffic deployment | [Esc] / arrows: Cancel confirmation "
        } else {
            " [↑/↓]: Select Revision | [0-9]: Enter % | [←/→] or [+/-]: Adjust | [c]: Clear | [C]: Clear all | [Enter]: Confirm | [Esc]: Cancel "
        }
    } else if state.show_logs {
        " [Esc]: Close Logs | [j/k]: Select Service | [s]: Traffic Split | [o]: Open URL | [q]: Quit "
    } else {
        " [s]: Traffic Split | [l]: View Logs | [j/k]: Select Service | [o]: Open URL | [r]: Refresh | [q]: Quit "
    };

    let footer = Paragraph::new(footer_text)
        .style(Style::default().fg(Color::DarkGray))
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(footer, footer_chunk);

    // 5. Traffic Split Modal (renders on top using Clear widget)
    if let Some(modal) = state.traffic_modal {
        render_traffic_modal(f, modal, state.spinner_tick);
    }
}

pub fn render_traffic_modal(f: &mut Frame, modal: &TrafficModalState, tick: usize) {
    let area = centered_rect(72, 70, f.area());
    f.render_widget(Clear, area);

    let modal_block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Traffic Split Allocation: {} ", modal.service_name))
        .border_style(Style::default().fg(Color::Cyan));

    let inner_area = modal_block.inner(area);
    f.render_widget(modal_block, area);

    let modal_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // Top description
            Constraint::Min(5),    // Revisions list
            Constraint::Length(2), // Total / Sum validation
            Constraint::Length(3), // Status / error message (supports text wrapping)
            Constraint::Length(2), // Navigation hint
        ])
        .split(inner_area);

    // 1. Top description
    let desc = Paragraph::new(Line::from(vec![
        Span::raw("Allocate traffic across revisions for service "),
        Span::styled(&modal.service_name, Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Span::raw(". Sum must equal exactly 100%."),
    ]))
    .wrap(Wrap { trim: true });
    f.render_widget(desc, modal_chunks[0]);

    // 2. Revisions Table
    let rows: Vec<Row> = if modal.revisions.is_empty() {
        if modal.status == TrafficModalStatus::FetchingRevisions {
            vec![Row::new(vec![
                Cell::from(""),
                Cell::from(format!("{} Fetching revisions from Cloud Run API...", spinner(tick))),
                Cell::from(""),
                Cell::from(""),
                Cell::from(""),
            ])]
        } else {
            vec![Row::new(vec![
                Cell::from(""),
                Cell::from("No revisions found for service."),
                Cell::from(""),
                Cell::from(""),
                Cell::from(""),
            ])]
        }
    } else {
        modal
            .revisions
            .iter()
            .enumerate()
            .map(|(idx, item)| {
                let is_selected = idx == modal.selected_index;
                let marker = if is_selected { ">" } else { " " };

                let filled_blocks = (item.percent.clamp(0, 100) / 10) as usize;
                let empty_blocks = 10usize.saturating_sub(filled_blocks);
                let bar = format!("{}{}", "█".repeat(filled_blocks), "░".repeat(empty_blocks));

                let bar_color = if item.percent == 100 {
                    Color::Green
                } else if item.percent > 0 {
                    Color::Cyan
                } else {
                    Color::DarkGray
                };

                let rev_style = if is_selected {
                    Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Gray)
                };

                let pct_text = if is_selected && !modal.input_buffer.is_empty() {
                    format!("[ {:>3}% ]*", item.percent)
                } else if is_selected {
                    format!("[ {:>3}% ] ", item.percent)
                } else {
                    format!("  {:>3}%   ", item.percent)
                };

                let pct_style = if is_selected {
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else if item.percent > 0 {
                    Style::default().fg(Color::Cyan)
                } else {
                    Style::default().fg(Color::DarkGray)
                };

                let marker_style = if is_selected {
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };

                Row::new(vec![
                    Cell::from(marker).style(marker_style),
                    Cell::from(item.revision_name.clone()).style(rev_style),
                    Cell::from(pct_text).style(pct_style),
                    Cell::from(bar).style(Style::default().fg(bar_color)),
                    Cell::from(item.tag.clone()).style(Style::default().fg(Color::DarkGray)),
                ])
            })
            .collect()
    };

    let rev_table = Table::new(
        rows,
        [
            Constraint::Length(2),      // Marker
            Constraint::Percentage(45), // Revision name
            Constraint::Length(12),     // Percent
            Constraint::Length(14),     // Bar
            Constraint::Percentage(20), // Tag
        ],
    )
    .header(
        Row::new(vec!["", "REVISION", "PERCENT", "ALLOCATION", "TAG"])
            .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
    )
    .block(Block::default().borders(Borders::ALL).title(" Available Revisions "));

    f.render_widget(rev_table, modal_chunks[1]);

    // 3. Total / Sum validation line
    let total = modal.total_percent();
    let sum_line = match &modal.status {
        TrafficModalStatus::Confirming => Line::from(vec![
            Span::raw(" Total Split: "),
            Span::styled("100% ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            Span::styled("⚠ CONFIRMATION REQUIRED: Press [Enter] again to apply split", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        ]),
        _ => {
            if total == 100 {
                Line::from(vec![
                    Span::raw(" Total Split: "),
                    Span::styled("100% ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
                    Span::styled("✓ Valid allocation (Press Enter to apply)", Style::default().fg(Color::Green)),
                ])
            } else {
                let diff = 100 - total;
                let diff_str = if diff > 0 {
                    format!("(Remaining: {}%)", diff)
                } else {
                    format!("(Excess: {}%)", diff.abs())
                };
                Line::from(vec![
                    Span::raw(" Total Split: "),
                    Span::styled(format!("{}% ", total), Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                    Span::styled(format!("✗ Must equal 100% {}", diff_str), Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                ])
            }
        }
    };
    let sum_widget = Paragraph::new(sum_line).block(Block::default().borders(Borders::BOTTOM));
    f.render_widget(sum_widget, modal_chunks[2]);

    // 4. Status / error message (wrapped to prevent clipping)
    let status_lines: Vec<Line> = match &modal.status {
        TrafficModalStatus::FetchingRevisions => vec![Line::from(vec![
            Span::styled(format!(" {} Fetching revisions from Cloud Run...", spinner(tick)), Style::default().fg(Color::Yellow)),
        ])],
        TrafficModalStatus::Confirming => vec![Line::from(vec![
            Span::styled(
                " ⚠ Confirm deployment: Press [Enter] again to apply split immediately, or [Esc] / arrows to cancel.",
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ),
        ])],
        TrafficModalStatus::Submitting => vec![Line::from(vec![
            Span::styled(format!(" {} Submitting traffic split to Google Cloud API...", spinner(tick)), Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        ])],
        TrafficModalStatus::Success(msg) => vec![Line::from(vec![
            Span::styled(format!(" ✓ {}", msg), Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
        ])],
        TrafficModalStatus::Error(err) => err
            .lines()
            .enumerate()
            .map(|(i, l)| {
                let prefix = if i == 0 { " ✗ " } else { "   " };
                Line::from(Span::styled(
                    format!("{}{}", prefix, l),
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ))
            })
            .collect(),
        TrafficModalStatus::Idle => vec![
            Line::from(Span::styled(" Tip: Type numbers (0-100), use Left/Right arrows to adjust, 'c' to clear, 'C' to clear all.", Style::default().fg(Color::DarkGray))),
        ],
    };
    let status_paragraph = Paragraph::new(status_lines).wrap(Wrap { trim: true });
    f.render_widget(status_paragraph, modal_chunks[3]);

    // 5. Navigation hints
    let nav_text = match &modal.status {
        TrafficModalStatus::Success(_) => " [Enter] or [Esc]: Return to main view ",
        TrafficModalStatus::Confirming => " [Enter]: Confirm deployment | [Esc] or navigation keys: Cancel confirmation ",
        TrafficModalStatus::Submitting => " Updating traffic split... please wait ",
        _ => " [↑/↓]: Select | [0-9]: Enter % | [←/→] or [+/-]: ±5% | [c]: Clear | [C]: Clear all | [Enter]: Apply | [Esc]: Cancel ",
    };
    let nav_widget = Paragraph::new(nav_text).style(Style::default().fg(Color::DarkGray));
    f.render_widget(nav_widget, modal_chunks[4]);
}

pub fn render_auth_error(f: &mut Frame, error_msg: &str) {
    let area = centered_rect(75, 55, f.area());
    f.render_widget(Clear, area);

    let text = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  Google Cloud Authentication Required",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled("  Error details:", Style::default().fg(Color::Yellow))),
        Line::from(Span::styled(format!("    {}", error_msg), Style::default().fg(Color::White))),
        Line::from(""),
        Line::from(Span::styled(
            "  To authenticate, please run in a separate terminal:",
            Style::default().fg(Color::Cyan),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "    $ gcloud auth login",
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  Or for application default credentials:",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            "    $ gcloud auth application-default login",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
        Line::from(vec![
            Span::raw("  Press "),
            Span::styled("[r]", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(" to retry authentication  |  Press "),
            Span::styled("[q]", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(" to quit"),
        ]),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Authentication Error ")
        .border_style(Style::default().fg(Color::Red));

    let p = Paragraph::new(text).wrap(Wrap { trim: true }).block(block);
    f.render_widget(p, area);
}

pub fn render_startup_error(f: &mut Frame, error_msg: &str) {
    let area = centered_rect(70, 45, f.area());
    f.render_widget(Clear, area);

    let text = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  Failed to Connect to Google Cloud Run API",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled("  Error details:", Style::default().fg(Color::Yellow))),
        Line::from(Span::styled(format!("    {}", error_msg), Style::default().fg(Color::White))),
        Line::from(""),
        Line::from(vec![
            Span::raw("  Press "),
            Span::styled("[r]", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(" to retry  |  Press "),
            Span::styled("[q]", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(" to quit"),
        ]),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Connection Error ")
        .border_style(Style::default().fg(Color::Red));

    let p = Paragraph::new(text).wrap(Wrap { trim: true }).block(block);
    f.render_widget(p, area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_traffic_modal_state_new() {
        let modal = TrafficModalState::new("web-service".to_string());
        assert_eq!(modal.service_name, "web-service");
        assert!(modal.revisions.is_empty());
        assert_eq!(modal.selected_index, 0);
        assert_eq!(modal.input_buffer, "");
        assert_eq!(modal.status, TrafficModalStatus::FetchingRevisions);
        assert_eq!(modal.total_percent(), 0);
    }

    #[test]
    fn test_traffic_modal_state_total_percent() {
        let mut modal = TrafficModalState::new("web-service".to_string());
        assert_eq!(modal.total_percent(), 0);

        modal.revisions.push(RevisionTrafficItem {
            revision_name: "rev-1".to_string(),
            percent: 70,
            tag: "-".to_string(),
        });
        modal.revisions.push(RevisionTrafficItem {
            revision_name: "rev-2".to_string(),
            percent: 30,
            tag: "-".to_string(),
        });
        assert_eq!(modal.total_percent(), 100);

        modal.revisions[1].percent = 50;
        assert_eq!(modal.total_percent(), 120);
    }

    #[test]
    fn test_centered_rect_geometry() {
        let parent = Rect::new(0, 0, 100, 100);
        let centered = centered_rect(60, 40, parent);

        assert_eq!(centered.width, 60);
        assert_eq!(centered.height, 40);
        assert_eq!(centered.x, 20);
        assert_eq!(centered.y, 30);
        assert!(centered.x + centered.width <= parent.width);
        assert!(centered.y + centered.height <= parent.height);

        // 100% test
        let full = centered_rect(100, 100, parent);
        assert_eq!(full.width, 100);
        assert_eq!(full.height, 100);
        assert_eq!(full.x, 0);
        assert_eq!(full.y, 0);

        // Odd percentages
        let odd = centered_rect(73, 51, parent);
        assert!(odd.x + odd.width <= parent.width);
        assert!(odd.y + odd.height <= parent.height);
    }

    #[test]
    fn test_navigation_bounds() {
        let mut modal = TrafficModalState::new("svc".to_string());
        modal.status = TrafficModalStatus::Idle;

        // Empty revisions list navigation does not crash
        assert_eq!(modal.handle_key(KeyCode::Up), TrafficModalAction::None);
        assert_eq!(modal.selected_index, 0);
        assert_eq!(modal.handle_key(KeyCode::Down), TrafficModalAction::None);
        assert_eq!(modal.selected_index, 0);

        // Add 3 revisions (indices 0, 1, 2)
        modal.revisions = vec![
            RevisionTrafficItem {
                revision_name: "rev-0".to_string(),
                percent: 50,
                tag: "-".to_string(),
            },
            RevisionTrafficItem {
                revision_name: "rev-1".to_string(),
                percent: 30,
                tag: "-".to_string(),
            },
            RevisionTrafficItem {
                revision_name: "rev-2".to_string(),
                percent: 20,
                tag: "-".to_string(),
            },
        ];

        modal.input_buffer = "50".to_string();

        // Up at top bounds stays at 0 and clears input buffer
        assert_eq!(modal.handle_key(KeyCode::Up), TrafficModalAction::None);
        assert_eq!(modal.selected_index, 0);
        assert_eq!(modal.input_buffer, "");

        // Down moves down
        assert_eq!(modal.handle_key(KeyCode::Down), TrafficModalAction::None);
        assert_eq!(modal.selected_index, 1);

        // 'j' moves down
        assert_eq!(modal.handle_key(KeyCode::Char('j')), TrafficModalAction::None);
        assert_eq!(modal.selected_index, 2);

        // Down at bottom bounds stays at 2
        assert_eq!(modal.handle_key(KeyCode::Down), TrafficModalAction::None);
        assert_eq!(modal.selected_index, 2);
        assert_eq!(modal.handle_key(KeyCode::Char('j')), TrafficModalAction::None);
        assert_eq!(modal.selected_index, 2);

        // 'k' moves up
        assert_eq!(modal.handle_key(KeyCode::Char('k')), TrafficModalAction::None);
        assert_eq!(modal.selected_index, 1);

        // Up moves to 0
        assert_eq!(modal.handle_key(KeyCode::Up), TrafficModalAction::None);
        assert_eq!(modal.selected_index, 0);
    }

    #[test]
    fn test_percentage_clamping_and_digit_inputs() {
        let mut modal = TrafficModalState::new("svc".to_string());
        modal.status = TrafficModalStatus::Idle;
        modal.revisions = vec![RevisionTrafficItem {
            revision_name: "rev-0".to_string(),
            percent: 0,
            tag: "-".to_string(),
        }];

        // Digit inputs
        modal.handle_key(KeyCode::Char('5'));
        assert_eq!(modal.input_buffer, "5");
        assert_eq!(modal.revisions[0].percent, 5);

        modal.handle_key(KeyCode::Char('0'));
        assert_eq!(modal.input_buffer, "50");
        assert_eq!(modal.revisions[0].percent, 50);

        // Clamping to 100 on 3rd digit exceeding 100
        modal.handle_key(KeyCode::Char('0'));
        assert_eq!(modal.input_buffer, "500");
        assert_eq!(modal.revisions[0].percent, 100);

        // 4th digit is ignored
        modal.handle_key(KeyCode::Char('9'));
        assert_eq!(modal.input_buffer, "500");
        assert_eq!(modal.revisions[0].percent, 100);

        // Backspace removes last digit
        modal.handle_key(KeyCode::Backspace);
        assert_eq!(modal.input_buffer, "50");
        assert_eq!(modal.revisions[0].percent, 50);

        // Plus / Right arrow increments by 5
        modal.handle_key(KeyCode::Char('+'));
        assert_eq!(modal.revisions[0].percent, 55);
        assert_eq!(modal.input_buffer, "55");

        modal.handle_key(KeyCode::Right);
        assert_eq!(modal.revisions[0].percent, 60);

        // Saturating at 100
        for _ in 0..15 {
            modal.handle_key(KeyCode::Char('+'));
        }
        assert_eq!(modal.revisions[0].percent, 100);

        // Minus / Left arrow decrements by 5
        modal.handle_key(KeyCode::Char('-'));
        assert_eq!(modal.revisions[0].percent, 95);

        modal.handle_key(KeyCode::Left);
        assert_eq!(modal.revisions[0].percent, 90);

        // Floored at 0
        for _ in 0..25 {
            modal.handle_key(KeyCode::Char('-'));
        }
        assert_eq!(modal.revisions[0].percent, 0);

        // Step by 1: ']' and '['
        modal.handle_key(KeyCode::Char(']'));
        assert_eq!(modal.revisions[0].percent, 1);

        modal.handle_key(KeyCode::Char('['));
        assert_eq!(modal.revisions[0].percent, 0);

        modal.handle_key(KeyCode::Char('['));
        assert_eq!(modal.revisions[0].percent, 0);
    }

    #[test]
    fn test_input_buffer_backspacing_to_empty_resets_to_zero() {
        let mut modal = TrafficModalState::new("svc".to_string());
        modal.status = TrafficModalStatus::Idle;
        modal.revisions = vec![RevisionTrafficItem {
            revision_name: "rev-0".to_string(),
            percent: 80,
            tag: "-".to_string(),
        }];

        // When input buffer is empty, hitting Backspace immediately resets percent to 0
        assert_eq!(modal.input_buffer, "");
        modal.handle_key(KeyCode::Backspace);
        assert_eq!(modal.revisions[0].percent, 0);

        // Type '7'
        modal.handle_key(KeyCode::Char('7'));
        assert_eq!(modal.input_buffer, "7");
        assert_eq!(modal.revisions[0].percent, 7);

        // Backspace pops '7' -> buffer becomes "" and percent resets to 0
        modal.handle_key(KeyCode::Backspace);
        assert_eq!(modal.input_buffer, "");
        assert_eq!(modal.revisions[0].percent, 0);

        // Backspace again on empty buffer remains 0
        modal.handle_key(KeyCode::Backspace);
        assert_eq!(modal.input_buffer, "");
        assert_eq!(modal.revisions[0].percent, 0);
    }

    #[test]
    fn test_enter_rejected_when_total_not_100() {
        let mut modal = TrafficModalState::new("svc".to_string());
        modal.status = TrafficModalStatus::Idle;
        modal.revisions = vec![
            RevisionTrafficItem {
                revision_name: "rev-0".to_string(),
                percent: 50,
                tag: "-".to_string(),
            },
            RevisionTrafficItem {
                revision_name: "rev-1".to_string(),
                percent: 30,
                tag: "-".to_string(),
            },
        ];

        // Total is 80% (under 100%)
        let action = modal.handle_key(KeyCode::Enter);
        assert_eq!(action, TrafficModalAction::None);
        match &modal.status {
            TrafficModalStatus::Error(msg) => {
                assert!(msg.contains("80%"));
                assert!(msg.contains("must equal exactly 100%"));
            }
            other => panic!("Expected Error status, got {:?}", other),
        }

        // Over 100%
        modal.revisions[1].percent = 60; // total = 110%
        let action = modal.handle_key(KeyCode::Enter);
        assert_eq!(action, TrafficModalAction::None);
        match &modal.status {
            TrafficModalStatus::Error(msg) => {
                assert!(msg.contains("110%"));
            }
            other => panic!("Expected Error status, got {:?}", other),
        }
    }

    #[test]
    fn test_confirmation_step_flow() {
        let mut modal = TrafficModalState::new("svc".to_string());
        modal.status = TrafficModalStatus::Idle;
        modal.revisions = vec![
            RevisionTrafficItem {
                revision_name: "rev-0".to_string(),
                percent: 70,
                tag: "-".to_string(),
            },
            RevisionTrafficItem {
                revision_name: "rev-1".to_string(),
                percent: 30,
                tag: "-".to_string(),
            },
        ];

        // 1st Enter: total is 100% -> transitions to Confirming step
        let action = modal.handle_key(KeyCode::Enter);
        assert_eq!(action, TrafficModalAction::None);
        assert_eq!(modal.status, TrafficModalStatus::Confirming);

        // 2nd Enter: confirms and submits split
        let action = modal.handle_key(KeyCode::Enter);
        assert_eq!(
            action,
            TrafficModalAction::Submit(vec![
                ("rev-0".to_string(), 70),
                ("rev-1".to_string(), 30),
            ])
        );
        assert_eq!(modal.status, TrafficModalStatus::Submitting);
    }

    #[test]
    fn test_confirmation_canceled_by_esc_or_edit() {
        let mut modal = TrafficModalState::new("svc".to_string());
        modal.status = TrafficModalStatus::Idle;
        modal.revisions = vec![RevisionTrafficItem {
            revision_name: "rev-0".to_string(),
            percent: 100,
            tag: "-".to_string(),
        }];

        // Enter -> Confirming
        modal.handle_key(KeyCode::Enter);
        assert_eq!(modal.status, TrafficModalStatus::Confirming);

        // Esc cancels confirmation back to Idle without closing modal
        let action = modal.handle_key(KeyCode::Esc);
        assert_eq!(action, TrafficModalAction::None);
        assert_eq!(modal.status, TrafficModalStatus::Idle);

        // Esc again closes modal
        let action = modal.handle_key(KeyCode::Esc);
        assert_eq!(action, TrafficModalAction::Close);

        // Enter -> Confirming again
        modal.handle_key(KeyCode::Enter);
        assert_eq!(modal.status, TrafficModalStatus::Confirming);

        // Editing cancels confirmation back to Idle
        modal.handle_key(KeyCode::Char('-'));
        assert_eq!(modal.status, TrafficModalStatus::Idle);
        assert_eq!(modal.revisions[0].percent, 95);
    }

    #[test]
    fn test_bulk_actions_c_and_uppercase_c() {
        let mut modal = TrafficModalState::new("svc".to_string());
        modal.status = TrafficModalStatus::Idle;
        modal.revisions = vec![
            RevisionTrafficItem {
                revision_name: "rev-0".to_string(),
                percent: 60,
                tag: "-".to_string(),
            },
            RevisionTrafficItem {
                revision_name: "rev-1".to_string(),
                percent: 40,
                tag: "-".to_string(),
            },
        ];

        // 'c' clears selected revision
        modal.selected_index = 0;
        modal.handle_key(KeyCode::Char('c'));
        assert_eq!(modal.revisions[0].percent, 0);
        assert_eq!(modal.revisions[1].percent, 40);
        assert_eq!(modal.input_buffer, "");

        // 'C' clears all revisions
        modal.revisions[0].percent = 50;
        modal.revisions[1].percent = 50;
        modal.handle_key(KeyCode::Char('C'));
        assert_eq!(modal.revisions[0].percent, 0);
        assert_eq!(modal.revisions[1].percent, 0);
        assert_eq!(modal.total_percent(), 0);
        assert_eq!(modal.input_buffer, "");
    }

    #[test]
    fn test_submitting_and_fetching_lockout_keys() {
        let mut modal = TrafficModalState::new("svc".to_string());
        modal.status = TrafficModalStatus::Submitting;
        modal.revisions = vec![RevisionTrafficItem {
            revision_name: "rev-0".to_string(),
            percent: 100,
            tag: "-".to_string(),
        }];

        // Editing and Enter ignored while submitting
        assert_eq!(modal.handle_key(KeyCode::Enter), TrafficModalAction::None);
        assert_eq!(modal.handle_key(KeyCode::Char('5')), TrafficModalAction::None);
        assert_eq!(modal.handle_key(KeyCode::Char('+')), TrafficModalAction::None);
        assert_eq!(modal.revisions[0].percent, 100);

        // Esc allows closing modal
        assert_eq!(modal.handle_key(KeyCode::Esc), TrafficModalAction::Close);
    }

    #[test]
    fn test_success_status_close_on_enter_or_esc() {
        let mut modal = TrafficModalState::new("svc".to_string());
        modal.status = TrafficModalStatus::Success("Done".to_string());

        assert_eq!(modal.handle_key(KeyCode::Enter), TrafficModalAction::Close);
        assert_eq!(modal.handle_key(KeyCode::Esc), TrafficModalAction::Close);
    }
}
