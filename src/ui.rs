use crossterm::event::KeyCode;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, TableState, Wrap},
    Frame,
};
use std::time::Duration;

use crate::models::{LogEntry, Service};

pub const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn spinner(tick: usize) -> &'static str {
    SPINNER_FRAMES[tick % SPINNER_FRAMES.len()]
}

pub fn animated_dots(tick: usize) -> &'static str {
    match (tick / 2) % 4 {
        0 => "",
        1 => ".",
        2 => "..",
        _ => "...",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BannerType {
    Info,
    Success,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub message: String,
    pub kind: ToastKind,
    pub created_at: std::time::Instant,
    pub duration: Duration,
}

impl Toast {
    pub fn new(message: impl Into<String>, kind: ToastKind) -> Self {
        let duration = match kind {
            ToastKind::Error => Duration::from_secs(8),
            ToastKind::Warning => Duration::from_secs(6),
            ToastKind::Info | ToastKind::Success => Duration::from_secs(4),
        };
        Self {
            message: message.into(),
            kind,
            created_at: std::time::Instant::now(),
            duration,
        }
    }

    pub fn is_expired(&self) -> bool {
        self.created_at.elapsed() > self.duration
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionTrafficItem {
    pub revision_name: String,
    pub percent: i32,
    pub tag: String,
    pub is_latest: bool,
}

impl RevisionTrafficItem {
    pub fn new(name: impl Into<String>, percent: i32, tag: impl Into<String>) -> Self {
        Self {
            revision_name: name.into(),
            percent,
            tag: tag.into(),
            is_latest: false,
        }
    }

    pub fn with_latest(mut self, is_latest: bool) -> Self {
        self.is_latest = is_latest;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrafficSplitTarget {
    pub revision: String,
    pub percent: i32,
    pub tag: Option<String>,
}

impl TrafficSplitTarget {
    pub fn new(revision: impl Into<String>, percent: i32, tag: Option<String>) -> Self {
        Self {
            revision: revision.into(),
            percent,
            tag,
        }
    }
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
    Submit(Vec<TrafficSplitTarget>),
}

#[derive(Debug, Clone)]
pub struct TrafficModalState {
    pub service_name: String,
    pub full_service_name: String,
    pub revisions: Vec<RevisionTrafficItem>,
    pub selected_index: usize,
    pub input_buffer: String,
    pub status: TrafficModalStatus,
    pub editing_tag: bool,
    pub tag_input_buffer: String,
}

impl TrafficModalState {
    pub fn new(service_name: String, full_service_name: String) -> Self {
        Self {
            service_name,
            full_service_name,
            revisions: Vec::new(),
            selected_index: 0,
            input_buffer: String::new(),
            status: TrafficModalStatus::FetchingRevisions,
            editing_tag: false,
            tag_input_buffer: String::new(),
        }
    }

    pub fn total_percent(&self) -> i32 {
        self.revisions.iter().map(|r| r.percent).sum()
    }

    pub fn is_latest_zero_traffic(&self) -> bool {
        let latest = self
            .revisions
            .iter()
            .find(|r| r.is_latest)
            .or_else(|| self.revisions.first());
        if let Some(item) = latest {
            item.percent == 0
        } else {
            false
        }
    }

    pub fn latest_revision_name(&self) -> Option<&str> {
        self.revisions
            .iter()
            .find(|r| r.is_latest)
            .or_else(|| self.revisions.first())
            .map(|r| r.revision_name.as_str())
    }

    pub fn handle_key(&mut self, key: KeyCode) -> TrafficModalAction {
        if matches!(self.status, TrafficModalStatus::Submitting | TrafficModalStatus::FetchingRevisions) {
            if key == KeyCode::Esc {
                return TrafficModalAction::Close;
            }
            return TrafficModalAction::None;
        }

        if self.editing_tag {
            match key {
                KeyCode::Esc => {
                    self.editing_tag = false;
                    self.tag_input_buffer.clear();
                }
                KeyCode::Enter => {
                    if let Some(item) = self.revisions.get_mut(self.selected_index) {
                        let trimmed = self.tag_input_buffer.trim();
                        item.tag = if trimmed.is_empty() || trimmed == "-" {
                            "-".to_string()
                        } else {
                            trimmed.to_string()
                        };
                    }
                    self.editing_tag = false;
                    self.tag_input_buffer.clear();
                }
                KeyCode::Backspace => {
                    self.tag_input_buffer.pop();
                }
                KeyCode::Char(c)
                    if (c.is_ascii_alphanumeric() || c == '-')
                        && self.tag_input_buffer.len() < 63 =>
                {
                    self.tag_input_buffer.push(c);
                }
                _ => {}
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
                        let splits: Vec<TrafficSplitTarget> = self
                            .revisions
                            .iter()
                            .filter(|r| r.percent > 0)
                            .map(|r| {
                                let tag = if r.tag.is_empty() || r.tag == "-" {
                                    None
                                } else {
                                    Some(r.tag.clone())
                                };
                                TrafficSplitTarget::new(r.revision_name.clone(), r.percent, tag)
                            })
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
            KeyCode::Char('t') | KeyCode::Char('T') => {
                if !self.revisions.is_empty() {
                    self.reset_confirm_or_error();
                    self.editing_tag = true;
                    let current_tag = &self.revisions[self.selected_index].tag;
                    self.tag_input_buffer = if current_tag == "-" {
                        String::new()
                    } else {
                        current_tag.clone()
                    };
                }
                TrafficModalAction::None
            }
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
    pub services: &'a [&'a Service],
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
    pub toasts: &'a [Toast],
    pub spinner_tick: usize,
    pub search_query: &'a str,
    pub is_searching: bool,
}

pub fn render_ui(f: &mut Frame, state: UiState) {
    let dots = animated_dots(state.spinner_tick);

    // If services are empty and there is an error, show the startup error screen
    if state.services.is_empty()
        && state.search_query.is_empty()
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
    let mut constraints = if banner_active {
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

    if state.is_searching {
        constraints.push(Constraint::Length(3));
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(f.area());

    let mut chunk_idx = 0;

    // 1. Header Banner
    let header_chunk = chunks[chunk_idx];
    chunk_idx += 1;

    let fetching_indicator = if state.is_fetching_services {
        format!(" [ {} Fetching services{} ]", spinner(state.spinner_tick), dots)
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

    let filter_indicator = if !state.search_query.is_empty() {
        format!(" [Filter: \"{}\"]", state.search_query)
    } else {
        String::new()
    };

    let table_title = if state.is_fetching_services {
        format!(" Services [ {} Fetching{} ]{} ", spinner(state.spinner_tick), dots, filter_indicator)
    } else {
        format!(" Services ({}){} ", state.services.len(), filter_indicator)
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
    .row_highlight_style(Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD));

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
            Paragraph::new(format!(" {} Streaming logs from Cloud Logging{} ", spinner(state.spinner_tick), dots))
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
                    " Live Logs: {} [ {} Refreshing{} ] ",
                    state.selected_service_name,
                    spinner(state.spinner_tick),
                    dots
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
        let selected_svc = state.table_state.selected().and_then(|idx| state.services.get(idx)).copied();
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
    chunk_idx += 1;

    let footer_text = if let Some(modal) = state.traffic_modal {
        if modal.editing_tag {
            " [Enter]: Save tag | [Esc]: Cancel tag edit | Type a-z, 0-9, hyphen "
        } else if modal.status == TrafficModalStatus::Confirming {
            " [Enter]: Confirm traffic deployment | [Esc] / arrows: Cancel confirmation "
        } else {
            " [↑/↓]: Select Revision | [0-9]: Enter % | [←/→] or [+/-]: Adjust | [t]: Edit Tag | [c]: Clear | [C]: Clear all | [Enter]: Confirm | [Esc]: Cancel "
        }
    } else if state.is_searching {
        " [Enter]: Keep filter | [Esc]: Cancel search | [Backspace]: Delete "
    } else if state.show_logs {
        " [Esc]: Close Logs | [/]: Filter | [j/k]: Select Service | [s]: Traffic Split | [o]: Open URL | [q]: Quit "
    } else {
        " [s]: Traffic Split | [l]: View Logs | [/]: Filter | [j/k]: Select Service | [o]: Open URL | [r]: Refresh | [q]: Quit "
    };

    let footer = Paragraph::new(footer_text)
        .style(Style::default().fg(Color::DarkGray))
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(footer, footer_chunk);

    // 5. Search Bar (conditionally rendered when is_searching is true)
    if state.is_searching {
        let search_chunk = chunks[chunk_idx];
        let search_bar = Paragraph::new(format!("/{}", state.search_query))
            .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Search ")
                    .border_style(Style::default().fg(Color::Yellow)),
            );
        f.render_widget(search_bar, search_chunk);
    }

    // 6. Traffic Split Modal (renders on top using Clear widget)
    if let Some(modal) = state.traffic_modal {
        render_traffic_modal(f, modal, state.spinner_tick);
    }

    // 7. Global Floating Toast Notifications
    render_toasts(f, state.toasts);
}

pub fn render_toasts(f: &mut Frame, toasts: &[Toast]) {
    if toasts.is_empty() {
        return;
    }
    let area = f.area();
    let toast_width = 46.min(area.width.saturating_sub(4));
    let mut top_y = 1u16;

    for toast in toasts.iter().rev().take(3) {
        let toast_height = 3u16;
        if top_y + toast_height >= area.height.saturating_sub(3) {
            break;
        }
        let toast_area = Rect {
            x: area.width.saturating_sub(toast_width + 2),
            y: top_y,
            width: toast_width,
            height: toast_height,
        };

        let (border_color, icon) = match toast.kind {
            ToastKind::Error => (Color::Red, "✗ Error: "),
            ToastKind::Warning => (Color::Yellow, "⚠ Warning: "),
            ToastKind::Success => (Color::Green, "✓ Success: "),
            ToastKind::Info => (Color::Cyan, "ℹ Info: "),
        };

        f.render_widget(Clear, toast_area);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color))
            .title(format!(" {} ", icon.trim()));

        let text = Paragraph::new(toast.message.as_str())
            .style(Style::default().fg(Color::White))
            .wrap(Wrap { trim: true })
            .block(block);

        f.render_widget(text, toast_area);
        top_y += toast_height;
    }
}

pub fn render_traffic_modal(f: &mut Frame, modal: &TrafficModalState, tick: usize) {
    let area = centered_rect(75, 75, f.area());
    f.render_widget(Clear, area);

    let dots = animated_dots(tick);
    let total = modal.total_percent();

    let border_color = match &modal.status {
        TrafficModalStatus::Submitting => Color::Cyan,
        TrafficModalStatus::FetchingRevisions => Color::Yellow,
        TrafficModalStatus::Success(_) => Color::Green,
        TrafficModalStatus::Error(_) => Color::Red,
        TrafficModalStatus::Confirming => {
            if modal.is_latest_zero_traffic() {
                Color::Yellow
            } else {
                Color::Green
            }
        }
        TrafficModalStatus::Idle => {
            if total == 100 {
                Color::Green
            } else {
                Color::Red
            }
        }
    };

    let modal_title = match &modal.status {
        TrafficModalStatus::FetchingRevisions => {
            format!(" Traffic Split Allocation: {} [ {} Fetching revisions{} ] ", modal.service_name, spinner(tick), dots)
        }
        TrafficModalStatus::Submitting => {
            format!(" Traffic Split Allocation: {} [ {} Submitting{} ] ", modal.service_name, spinner(tick), dots)
        }
        TrafficModalStatus::Success(_) => {
            format!(" Traffic Split Allocation: {} [ ✓ Updated ] ", modal.service_name)
        }
        TrafficModalStatus::Error(_) => {
            format!(" Traffic Split Allocation: {} [ ✗ Error ] ", modal.service_name)
        }
        TrafficModalStatus::Confirming => {
            format!(" Traffic Split Allocation: {} [ ⚠ Confirmation Pending ] ", modal.service_name)
        }
        TrafficModalStatus::Idle => {
            if total == 100 {
                format!(" Traffic Split Allocation: {} [ 100/100% Valid ] ", modal.service_name)
            } else {
                format!(" Traffic Split Allocation: {} [ {}/100% ] ", modal.service_name, total)
            }
        }
    };

    let modal_block = Block::default()
        .borders(Borders::ALL)
        .title(modal_title)
        .border_style(Style::default().fg(border_color));

    let inner_area = modal_block.inner(area);
    f.render_widget(modal_block, area);

    let modal_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // Top description
            Constraint::Min(5),    // Revisions list
            Constraint::Length(3), // Visual Total Allocation Indicator & Validation
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
                Cell::from(format!("{} Fetching revisions from Cloud Run API{} please wait", spinner(tick), dots)),
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

                let rev_display = if item.is_latest {
                    format!("{} [latest]", item.revision_name)
                } else {
                    item.revision_name.clone()
                };

                let rev_style = if is_selected {
                    Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
                } else if item.is_latest {
                    Style::default().fg(Color::Cyan)
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

                let tag_display = if is_selected && modal.editing_tag {
                    format!("[ {}_ ]", modal.tag_input_buffer)
                } else if item.tag.is_empty() || item.tag == "-" {
                    "-".to_string()
                } else {
                    item.tag.clone()
                };

                let tag_style = if is_selected && modal.editing_tag {
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else if item.tag != "-" && !item.tag.is_empty() {
                    Style::default().fg(Color::Cyan)
                } else {
                    Style::default().fg(Color::DarkGray)
                };

                Row::new(vec![
                    Cell::from(marker).style(marker_style),
                    Cell::from(rev_display).style(rev_style),
                    Cell::from(pct_text).style(pct_style),
                    Cell::from(bar).style(Style::default().fg(bar_color)),
                    Cell::from(tag_display).style(tag_style),
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

    // 3. Dynamic Visual Total Allocation Indicator & Validation
    let bar_len = 20usize;
    let filled_len = ((total.clamp(0, 100) as usize) * bar_len) / 100;
    let empty_len = bar_len.saturating_sub(filled_len);

    let progress_bar_str = if total > 100 {
        format!("[{} +{}% OVER]", "█".repeat(bar_len), total - 100)
    } else {
        format!("[{}{}]", "█".repeat(filled_len), "░".repeat(empty_len))
    };

    let (bar_color, status_text, status_style) = if total == 100 {
        (
            Color::Green,
            " ✓ Valid allocation (100/100%)".to_string(),
            Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
        )
    } else if total < 100 {
        (
            Color::Red,
            format!(" ✗ Incomplete: {}/100% (Need +{}%)", total, 100 - total),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )
    } else {
        (
            Color::Red,
            format!(" ✗ Overallocated: {}/100% (Excess +{}%)", total, total - 100),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )
    };

    let total_fraction_style = if total == 100 {
        Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
    };

    let line1 = Line::from(vec![
        Span::styled(" Total Allocation: ", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(format!("{:>3}/100% ", total), total_fraction_style),
        Span::styled(progress_bar_str, Style::default().fg(bar_color).add_modifier(Modifier::BOLD)),
        Span::styled(status_text, status_style),
    ]);

    let line2 = match &modal.status {
        TrafficModalStatus::Confirming => {
            if modal.is_latest_zero_traffic() {
                Line::from(vec![
                    Span::styled(
                        format!(
                            " ⚠ SAFETY WARNING: 0% traffic to latest revision ('{}')! Blackout risk. Press [Enter] to apply.",
                            modal.latest_revision_name().unwrap_or("latest")
                        ),
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                    ),
                ])
            } else {
                Line::from(vec![
                    Span::styled(
                        " ⚠ CONFIRMATION REQUIRED: Press [Enter] again to apply traffic split immediately.",
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                    ),
                ])
            }
        }
        _ => {
            if modal.is_latest_zero_traffic() && total == 100 {
                Line::from(vec![
                    Span::styled(
                        format!(
                            " ⚠ Notice: Latest revision ('{}') has 0% traffic. Safety blackout guardrail active.",
                            modal.latest_revision_name().unwrap_or("latest")
                        ),
                        Style::default().fg(Color::Yellow),
                    ),
                ])
            } else {
                Line::from(vec![
                    Span::styled(
                        " Use Left/Right arrows or digits to allocate exactly 100% across revisions.",
                        Style::default().fg(Color::DarkGray),
                    ),
                ])
            }
        }
    };

    let sum_border_color = if total == 100 { Color::Green } else { Color::Red };
    let sum_widget = Paragraph::new(vec![line1, line2])
        .block(Block::default().borders(Borders::BOTTOM).border_style(Style::default().fg(sum_border_color)));
    f.render_widget(sum_widget, modal_chunks[2]);

    // 4. Status / error message (wrapped to prevent clipping)
    let status_lines: Vec<Line> = match &modal.status {
        TrafficModalStatus::FetchingRevisions => vec![Line::from(vec![
            Span::styled(
                format!(" {} Fetching revisions from Cloud Run API{} please wait", spinner(tick), dots),
                Style::default().fg(Color::Yellow),
            ),
        ])],
        TrafficModalStatus::Confirming => vec![Line::from(vec![
            Span::styled(
                " ⚠ Confirm deployment: Press [Enter] again to apply split immediately, or [Esc] / arrows to cancel.",
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ),
        ])],
        TrafficModalStatus::Submitting => vec![Line::from(vec![
            Span::styled(
                format!(" {} Submitting traffic split to Google Cloud API{} applying changes", spinner(tick), dots),
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
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
        TrafficModalStatus::Idle => {
            if modal.editing_tag {
                vec![Line::from(Span::styled(
                    format!(" Editing Tag for revision '{}'. Enter letters, numbers, or hyphens. Press [Enter] to save, [Esc] to cancel.", modal.revisions.get(modal.selected_index).map(|r| r.revision_name.as_str()).unwrap_or("")),
                    Style::default().fg(Color::Yellow),
                ))]
            } else {
                vec![Line::from(Span::styled(
                    " Tip: Type numbers (0-100), Left/Right to adjust, 't' to edit tag, 'c' to clear, 'C' to clear all.",
                    Style::default().fg(Color::DarkGray),
                ))]
            }
        }
    };
    let status_paragraph = Paragraph::new(status_lines).wrap(Wrap { trim: true });
    f.render_widget(status_paragraph, modal_chunks[3]);

    // 5. Navigation hints
    let nav_text = match &modal.status {
        TrafficModalStatus::Success(_) => " [Enter] or [Esc]: Return to main view ",
        TrafficModalStatus::Confirming => " [Enter]: Confirm deployment | [Esc] or navigation keys: Cancel confirmation ",
        TrafficModalStatus::Submitting => " Updating traffic split... please wait ",
        TrafficModalStatus::FetchingRevisions => " Loading revisions from GCP... please wait ",
        _ => {
            if modal.editing_tag {
                " [Enter]: Save tag | [Esc]: Cancel tag edit | Type a-z, 0-9, hyphen "
            } else {
                " [↑/↓]: Select | [0-9]: Enter % | [←/→] or [+/-]: ±5% | [t]: Edit Tag | [c]: Clear | [C]: Clear all | [Enter]: Apply | [Esc]: Cancel "
            }
        }
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
pub mod tests {
    use super::*;

    #[test]
    fn test_traffic_modal_state_new() {
        let modal = TrafficModalState::new(
            "web-service".to_string(),
            "projects/p/locations/l/services/web-service".to_string(),
        );
        assert_eq!(modal.service_name, "web-service");
        assert_eq!(modal.full_service_name, "projects/p/locations/l/services/web-service");
        assert!(modal.revisions.is_empty());
        assert_eq!(modal.selected_index, 0);
        assert_eq!(modal.input_buffer, "");
        assert_eq!(modal.status, TrafficModalStatus::FetchingRevisions);
        assert_eq!(modal.total_percent(), 0);
        assert!(!modal.editing_tag);
        assert_eq!(modal.tag_input_buffer, "");
    }

    #[test]
    fn test_traffic_modal_state_total_percent() {
        let mut modal = TrafficModalState::new("web-service".to_string(), "projects/p/locations/l/services/web-service".to_string());
        assert_eq!(modal.total_percent(), 0);

        modal.revisions.push(RevisionTrafficItem::new("rev-1", 70, "-"));
        modal.revisions.push(RevisionTrafficItem::new("rev-2", 30, "-"));
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
        let mut modal = TrafficModalState::new("svc".to_string(), "projects/p/locations/l/services/svc".to_string());
        modal.status = TrafficModalStatus::Idle;

        // Empty revisions list navigation does not crash
        assert_eq!(modal.handle_key(KeyCode::Up), TrafficModalAction::None);
        assert_eq!(modal.selected_index, 0);
        assert_eq!(modal.handle_key(KeyCode::Down), TrafficModalAction::None);
        assert_eq!(modal.selected_index, 0);

        // Add 3 revisions (indices 0, 1, 2)
        modal.revisions = vec![
            RevisionTrafficItem::new("rev-0", 50, "-"),
            RevisionTrafficItem::new("rev-1", 30, "-"),
            RevisionTrafficItem::new("rev-2", 20, "-"),
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
        let mut modal = TrafficModalState::new("svc".to_string(), "projects/p/locations/l/services/svc".to_string());
        modal.status = TrafficModalStatus::Idle;
        modal.revisions = vec![RevisionTrafficItem::new("rev-0", 0, "-")];

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
        let mut modal = TrafficModalState::new("svc".to_string(), "projects/p/locations/l/services/svc".to_string());
        modal.status = TrafficModalStatus::Idle;
        modal.revisions = vec![RevisionTrafficItem::new("rev-0", 80, "-")];

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
        let mut modal = TrafficModalState::new("svc".to_string(), "projects/p/locations/l/services/svc".to_string());
        modal.status = TrafficModalStatus::Idle;
        modal.revisions = vec![
            RevisionTrafficItem::new("rev-0", 50, "-"),
            RevisionTrafficItem::new("rev-1", 30, "-"),
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
        let mut modal = TrafficModalState::new("svc".to_string(), "projects/p/locations/l/services/svc".to_string());
        modal.status = TrafficModalStatus::Idle;
        modal.revisions = vec![
            RevisionTrafficItem::new("rev-0", 70, "-"),
            RevisionTrafficItem::new("rev-1", 30, "-"),
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
                TrafficSplitTarget::new("rev-0", 70, None),
                TrafficSplitTarget::new("rev-1", 30, None),
            ])
        );
        assert_eq!(modal.status, TrafficModalStatus::Submitting);
    }

    #[test]
    fn test_confirmation_canceled_by_esc_or_edit() {
        let mut modal = TrafficModalState::new("svc".to_string(), "projects/p/locations/l/services/svc".to_string());
        modal.status = TrafficModalStatus::Idle;
        modal.revisions = vec![RevisionTrafficItem::new("rev-0", 100, "-")];

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
        let mut modal = TrafficModalState::new("svc".to_string(), "projects/p/locations/l/services/svc".to_string());
        modal.status = TrafficModalStatus::Idle;
        modal.revisions = vec![
            RevisionTrafficItem::new("rev-0", 60, "-"),
            RevisionTrafficItem::new("rev-1", 40, "-"),
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
        let mut modal = TrafficModalState::new("svc".to_string(), "projects/p/locations/l/services/svc".to_string());
        modal.status = TrafficModalStatus::Submitting;
        modal.revisions = vec![RevisionTrafficItem::new("rev-0", 100, "-")];

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
        let mut modal = TrafficModalState::new("svc".to_string(), "projects/p/locations/l/services/svc".to_string());
        modal.status = TrafficModalStatus::Success("Done".to_string());

        assert_eq!(modal.handle_key(KeyCode::Enter), TrafficModalAction::Close);
        assert_eq!(modal.handle_key(KeyCode::Esc), TrafficModalAction::Close);
    }

    #[test]
    fn test_tag_editing_flow() {
        let mut modal = TrafficModalState::new("svc".to_string(), "projects/p/locations/l/services/svc".to_string());
        modal.status = TrafficModalStatus::Idle;
        modal.revisions = vec![
            RevisionTrafficItem::new("rev-0", 100, "-"),
            RevisionTrafficItem::new("rev-1", 0, "existing-tag"),
        ];

        // Press 't' on rev-0: starts editing with empty buffer
        modal.selected_index = 0;
        modal.handle_key(KeyCode::Char('t'));
        assert!(modal.editing_tag);
        assert_eq!(modal.tag_input_buffer, "");

        // Type 'canary'
        for c in "canary".chars() {
            modal.handle_key(KeyCode::Char(c));
        }
        assert_eq!(modal.tag_input_buffer, "canary");

        // Backspace pops last char
        modal.handle_key(KeyCode::Backspace);
        assert_eq!(modal.tag_input_buffer, "canar");

        // Type 'y' back
        modal.handle_key(KeyCode::Char('y'));
        assert_eq!(modal.tag_input_buffer, "canary");

        // Enter saves tag
        modal.handle_key(KeyCode::Enter);
        assert!(!modal.editing_tag);
        assert_eq!(modal.revisions[0].tag, "canary");

        // Edit rev-1: pressing 't' loads existing tag
        modal.selected_index = 1;
        modal.handle_key(KeyCode::Char('t'));
        assert!(modal.editing_tag);
        assert_eq!(modal.tag_input_buffer, "existing-tag");

        // Esc cancels without changing
        modal.handle_key(KeyCode::Char('-'));
        modal.handle_key(KeyCode::Char('2'));
        assert_eq!(modal.tag_input_buffer, "existing-tag-2");
        modal.handle_key(KeyCode::Esc);
        assert!(!modal.editing_tag);
        assert_eq!(modal.revisions[1].tag, "existing-tag");
    }

    #[test]
    fn test_safety_guardrail_latest_zero_traffic() {
        let mut modal = TrafficModalState::new("svc".to_string(), "projects/p/locations/l/services/svc".to_string());
        modal.status = TrafficModalStatus::Idle;
        modal.revisions = vec![
            RevisionTrafficItem::new("rev-latest", 0, "-").with_latest(true),
            RevisionTrafficItem::new("rev-previous", 100, "-"),
        ];

        assert!(modal.is_latest_zero_traffic());
        assert_eq!(modal.latest_revision_name(), Some("rev-latest"));

        // Enter transitions to Confirming with guardrail
        let action = modal.handle_key(KeyCode::Enter);
        assert_eq!(action, TrafficModalAction::None);
        assert_eq!(modal.status, TrafficModalStatus::Confirming);
        assert!(modal.is_latest_zero_traffic());

        // Esc cancels confirmation
        let action = modal.handle_key(KeyCode::Esc);
        assert_eq!(action, TrafficModalAction::None);
        assert_eq!(modal.status, TrafficModalStatus::Idle);

        // Adjust latest to 50, previous to 50
        modal.revisions[0].percent = 50;
        modal.revisions[1].percent = 50;
        assert!(!modal.is_latest_zero_traffic());
    }

    #[test]
    fn test_visual_allocation_computation() {
        let mut modal = TrafficModalState::new("svc".to_string(), "projects/p/locations/l/services/svc".to_string());
        modal.revisions = vec![
            RevisionTrafficItem::new("rev-1", 40, "-"),
            RevisionTrafficItem::new("rev-2", 40, "-"),
        ];
        assert_eq!(modal.total_percent(), 80);

        modal.revisions[1].percent = 60;
        assert_eq!(modal.total_percent(), 100);

        modal.revisions[1].percent = 80;
        assert_eq!(modal.total_percent(), 120);
    }

    #[test]
    fn test_toast_lifecycle_and_expiration() {
        let toast = Toast::new("Sample info", ToastKind::Info);
        assert_eq!(toast.message, "Sample info");
        assert_eq!(toast.kind, ToastKind::Info);
        assert!(!toast.is_expired());
    }

    #[test]
    fn test_render_traffic_modal_render_all_states() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let backend = TestBackend::new(100, 35);
        let mut terminal = Terminal::new(backend).unwrap();

        let mut modal = TrafficModalState::new("web-svc".to_string(), "projects/p/locations/l/services/web-svc".to_string());
        modal.status = TrafficModalStatus::Idle;
        modal.revisions = vec![
            RevisionTrafficItem::new("rev-1", 70, "candidate").with_latest(true),
            RevisionTrafficItem::new("rev-2", 30, "-"),
        ];

        terminal.draw(|f| {
            render_traffic_modal(f, &modal, 0);
        }).unwrap();

        // Also test with editing_tag
        modal.editing_tag = true;
        modal.tag_input_buffer = "my-tag".to_string();
        terminal.draw(|f| {
            render_traffic_modal(f, &modal, 1);
        }).unwrap();

        // Test with Confirming & zero latest traffic
        modal.editing_tag = false;
        modal.revisions[0].percent = 0;
        modal.revisions[1].percent = 100;
        modal.status = TrafficModalStatus::Confirming;
        terminal.draw(|f| {
            render_traffic_modal(f, &modal, 2);
        }).unwrap();

        // Test with Submitting
        modal.status = TrafficModalStatus::Submitting;
        terminal.draw(|f| {
            render_traffic_modal(f, &modal, 3);
        }).unwrap();

        // Test with Error
        modal.status = TrafficModalStatus::Error("Permission Denied (HTTP 403)".to_string());
        terminal.draw(|f| {
            render_traffic_modal(f, &modal, 4);
        }).unwrap();

        // Test with Success
        modal.status = TrafficModalStatus::Success("All done".to_string());
        terminal.draw(|f| {
            render_traffic_modal(f, &modal, 5);
        }).unwrap();
    }

    #[test]
    fn test_render_toasts() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();

        let toasts = vec![
            Toast::new("Error occurred", ToastKind::Error),
            Toast::new("Warning message", ToastKind::Warning),
            Toast::new("Success notice", ToastKind::Success),
            Toast::new("Info message", ToastKind::Info),
        ];

        terminal.draw(|f| {
            render_toasts(f, &toasts);
        }).unwrap();
    }
    #[test]
    fn test_render_ui_with_filter_and_searching() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let backend = TestBackend::new(120, 35);
        let mut terminal = Terminal::new(backend).unwrap();

        let services = vec![Service {
            name: "projects/p/locations/us-central1/services/web-frontend".to_string(),
            uri: Some("https://web.run.app".to_string()),
            latest_ready_revision: Some("rev-1".to_string()),
            conditions: None,
            traffic_statuses: None,
        }];
        let svc_refs: Vec<&Service> = services.iter().collect();
        let mut table_state = TableState::default();
        table_state.select(Some(0));

        // 1. Render when searching is true
        terminal
            .draw(|f| {
                render_ui(
                    f,
                    UiState {
                        project: "test-proj",
                        region: "us-central1",
                        services: &svc_refs,
                        table_state: &mut table_state,
                        is_fetching_services: false,
                        services_error: None,
                        is_auth_error: false,
                        show_logs: false,
                        logs: &[],
                        selected_service_name: "web-frontend",
                        is_fetching_logs: false,
                        log_error_msg: None,
                        banner_message: None,
                        traffic_modal: None,
                        toasts: &[],
                        spinner_tick: 0,
                        search_query: "frontend",
                        is_searching: true,
                    },
                );
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let buffer_str: String = buffer.content().iter().map(|c| c.symbol()).collect();
        assert!(buffer_str.contains("[Filter: \"frontend\"]"));
        assert!(buffer_str.contains("/frontend"));
        assert!(buffer_str.contains("web-frontend"));

        // 2. Render when filter is active but searching is false
        terminal
            .draw(|f| {
                render_ui(
                    f,
                    UiState {
                        project: "test-proj",
                        region: "us-central1",
                        services: &svc_refs,
                        table_state: &mut table_state,
                        is_fetching_services: false,
                        services_error: None,
                        is_auth_error: false,
                        show_logs: false,
                        logs: &[],
                        selected_service_name: "web-frontend",
                        is_fetching_logs: false,
                        log_error_msg: None,
                        banner_message: None,
                        traffic_modal: None,
                        toasts: &[],
                        spinner_tick: 0,
                        search_query: "frontend",
                        is_searching: false,
                    },
                );
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let buffer_str: String = buffer.content().iter().map(|c| c.symbol()).collect();
        assert!(buffer_str.contains("[Filter: \"frontend\"]"));
        assert!(!buffer_str.contains("/frontend"));
    }
}
