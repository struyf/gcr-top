#![allow(dead_code, unused_variables)]

mod client;
mod models;

use anyhow::Result;
use clap::Parser;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use models::{LogEntry, Service};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState},
    Terminal,
};
use std::{io, sync::Arc, time::Duration};
use tokio::sync::mpsc;

#[derive(Parser, Debug)]
#[command(name = "gcr-top", about = "Blazingly fast TUI for Google Cloud Run")]
struct Args {
    #[arg(short, long, env = "CLOUDSDK_CORE_PROJECT")]
    project: String,

    #[arg(short, long, default_value = "-")]
    region: String,
}

enum AppEvent {
    ServicesUpdated(Vec<Service>),
    LogsUpdated {
        service_name: String,
        logs: Vec<LogEntry>,
    },
    LogError {
        service_name: String,
        error: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let client = Arc::new(client::GcpClient::new(args.project.clone(), args.region.clone())?);

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let res = run_app(&mut terminal, client, &args.project).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        eprintln!("Error running gcr-top: {:?}", err);
    }

    Ok(())
}

async fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    client: Arc<client::GcpClient>,
    project: &str,
) -> Result<()> {
    let (tx, mut rx) = mpsc::channel::<AppEvent>(32);

    let mut table_state = TableState::default();
    table_state.select(Some(0));

    let mut services: Vec<Service> = Vec::new();
    let mut show_logs = false;
    let mut logs: Vec<LogEntry> = Vec::new();
    let mut log_error_msg: Option<String> = None;
    let mut selected_service_name = String::new();
    let mut is_fetching_logs = false;

    // Background service polling loop
    let client_svc = Arc::clone(&client);
    let tx_svc = tx.clone();
    tokio::spawn(async move {
        loop {
            if let Ok(fetched) = client_svc.list_services().await {
                let _ = tx_svc.send(AppEvent::ServicesUpdated(fetched)).await;
            }
            tokio::time::sleep(Duration::from_secs(20)).await;
        }
    });

    let mut last_log_poll = std::time::Instant::now();

    loop {
        // Handle non-blocking async events
        while let Ok(event) = rx.try_recv() {
            match event {
                AppEvent::ServicesUpdated(updated) => {
                    services = updated;
                }
                AppEvent::LogsUpdated { service_name, logs: new_logs } => {
                    if service_name == selected_service_name {
                        logs = new_logs;
                        log_error_msg = None;
                        is_fetching_logs = false;
                    }
                }
                AppEvent::LogError { service_name, error } => {
                    if service_name == selected_service_name {
                        log_error_msg = Some(error);
                        is_fetching_logs = false;
                    }
                }
            }
        }

        // Periodic background log polling
        if show_logs && !selected_service_name.is_empty() && last_log_poll.elapsed() > Duration::from_secs(7) {
            let client_log = Arc::clone(&client);
            let tx_log = tx.clone();
            let svc_name = selected_service_name.clone();
            tokio::spawn(async move {
                match client_log.fetch_recent_logs(&svc_name).await {
                    Ok(entries) => {
                        let _ = tx_log.send(AppEvent::LogsUpdated {
                            service_name: svc_name,
                            logs: entries,
                        }).await;
                    }
                    Err(e) => {
                        let _ = tx_log.send(AppEvent::LogError {
                            service_name: svc_name,
                            error: e.to_string(),
                        }).await;
                    }
                }
            });
            last_log_poll = std::time::Instant::now();
        }

        terminal.draw(|f| {
            let chunks = if show_logs {
                Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3),
                        Constraint::Percentage(40),
                        Constraint::Percentage(45),
                        Constraint::Length(3),
                    ])
                    .split(f.area())
            } else {
                Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3),
                        Constraint::Min(6),
                        Constraint::Length(6),
                        Constraint::Length(3),
                    ])
                    .split(f.area())
            };

            // 1. Header Banner
            let header = Paragraph::new(format!(
                " gcr-top | Project: {} | [l]: Logs | [Esc]: Close | [r]: Refresh | [q]: Quit",
                project
            ))
            .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
            .block(Block::default().borders(Borders::ALL).title(" Google Cloud Run Monitor "));
            f.render_widget(header, chunks[0]);

            // 2. Services Table with Traffic Column
            let rows: Vec<Row> = services
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
            .block(Block::default().borders(Borders::ALL).title(" Services "))
            .highlight_style(Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD));

            f.render_stateful_widget(table, chunks[1], &mut table_state);

            // 3. Middle Pane: either Live Logs or Traffic Allocation Inspector
            if show_logs {
                let pane_height = chunks[2].height.saturating_sub(2) as usize; // Aftrek van boven- en onderborder

                let log_widget = if let Some(ref err) = log_error_msg {
                    Paragraph::new(vec![
                        Line::from(Span::styled("API Error retrieving logs:", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))),
                        Line::from(Span::raw(err)),
                    ])
                    .block(Block::default().borders(Borders::ALL).title(" Error "))
                } else if is_fetching_logs && logs.is_empty() {
                    Paragraph::new("Streaming logs from Cloud Logging...")
                        .style(Style::default().fg(Color::DarkGray))
                        .block(Block::default().borders(Borders::ALL).title(format!(" Logs: {} ", selected_service_name)))
                } else if logs.is_empty() {
                    Paragraph::new("No entries found.")
                        .style(Style::default().fg(Color::DarkGray))
                        .block(Block::default().borders(Borders::ALL).title(format!(" Logs: {} ", selected_service_name)))
                } else {
                    let total_lines = logs.len();
                    // Bereken de verticale offset zodat de laatste regel altijd onderaan staat
                    let scroll_offset = if total_lines > pane_height {
                        (total_lines - pane_height) as u16
                    } else {
                        0
                    };

                    let log_lines: Vec<Line> = logs
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

                    Paragraph::new(log_lines)
                        .scroll((scroll_offset, 0))
                        .block(Block::default().borders(Borders::ALL).title(format!(" Live Logs: {} (Tail mode, auto-refresh 3s) ", selected_service_name)))
                };

                f.render_widget(log_widget, chunks[2]);
            } else {
                // Traffic Allocations Inspector for currently selected service
                let selected_svc = table_state.selected().and_then(|idx| services.get(idx));
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
                f.render_widget(traffic_widget, chunks[2]);
            }

            // 4. Footer
            let footer_text = if show_logs {
                " [Esc]: Close Logs | [j/k] or [↑/↓]: Select Service | [q]: Quit"
            } else {
                " [l]: View Logs | [j/k] or [↑/↓]: Select Service | [q]: Quit"
            };
            let footer = Paragraph::new(footer_text)
                .style(Style::default().fg(Color::DarkGray))
                .block(Block::default().borders(Borders::ALL));
            f.render_widget(footer, chunks[3]);
        })?;

        // Non-blocking keyboard event polling (30ms)
        if event::poll(Duration::from_millis(30))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') => return Ok(()),
                        KeyCode::Esc => {
                            show_logs = false;
                            log_error_msg = None;
                        }
                        KeyCode::Char('l') => {
                            if let Some(idx) = table_state.selected() {
                                if let Some(svc) = services.get(idx) {
                                    selected_service_name = svc.short_name().to_string();
                                    show_logs = true;
                                    logs.clear();
                                    log_error_msg = None;
                                    is_fetching_logs = true;

                                    let client_log = Arc::clone(&client);
                                    let tx_log = tx.clone();
                                    let svc_name = selected_service_name.clone();
                                    tokio::spawn(async move {
                                        match client_log.fetch_recent_logs(&svc_name).await {
                                            Ok(entries) => {
                                                let _ = tx_log.send(AppEvent::LogsUpdated {
                                                    service_name: svc_name,
                                                    logs: entries,
                                                }).await;
                                            }
                                            Err(e) => {
                                                let _ = tx_log.send(AppEvent::LogError {
                                                    service_name: svc_name,
                                                    error: e.to_string(),
                                                }).await;
                                            }
                                        }
                                    });
                                    last_log_poll = std::time::Instant::now();
                                }
                            }
                        }
                        KeyCode::Char('r') => {
                            let client_svc = Arc::clone(&client);
                            let tx_svc = tx.clone();
                            tokio::spawn(async move {
                                if let Ok(fetched) = client_svc.list_services().await {
                                    let _ = tx_svc.send(AppEvent::ServicesUpdated(fetched)).await;
                                }
                            });
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            let i = match table_state.selected() {
                                Some(i) => {
                                    if services.is_empty() || i >= services.len().saturating_sub(1) {
                                        0
                                    } else {
                                        i + 1
                                    }
                                }
                                None => 0,
                            };
                            table_state.select(Some(i));
                            if show_logs {
                                if let Some(svc) = services.get(i) {
                                    selected_service_name = svc.short_name().to_string();
                                    logs.clear();
                                    is_fetching_logs = true;
                                    let client_log = Arc::clone(&client);
                                    let tx_log = tx.clone();
                                    let svc_name = selected_service_name.clone();
                                    tokio::spawn(async move {
                                        match client_log.fetch_recent_logs(&svc_name).await {
                                            Ok(entries) => {
                                                let _ = tx_log.send(AppEvent::LogsUpdated {
                                                    service_name: svc_name,
                                                    logs: entries,
                                                }).await;
                                            }
                                            Err(e) => {
                                                let _ = tx_log.send(AppEvent::LogError {
                                                    service_name: svc_name,
                                                    error: e.to_string(),
                                                }).await;
                                            }
                                        }
                                    });
                                    last_log_poll = std::time::Instant::now();
                                }
                            }
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            let i = match table_state.selected() {
                                Some(i) => {
                                    if i == 0 {
                                        services.len().saturating_sub(1)
                                    } else {
                                        i - 1
                                    }
                                }
                                None => 0,
                            };
                            table_state.select(Some(i));
                            if show_logs {
                                if let Some(svc) = services.get(i) {
                                    selected_service_name = svc.short_name().to_string();
                                    logs.clear();
                                    is_fetching_logs = true;
                                    let client_log = Arc::clone(&client);
                                    let tx_log = tx.clone();
                                    let svc_name = selected_service_name.clone();
                                    tokio::spawn(async move {
                                        match client_log.fetch_recent_logs(&svc_name).await {
                                            Ok(entries) => {
                                                let _ = tx_log.send(AppEvent::LogsUpdated {
                                                    service_name: svc_name,
                                                    logs: entries,
                                                }).await;
                                            }
                                            Err(e) => {
                                                let _ = tx_log.send(AppEvent::LogError {
                                                    service_name: svc_name,
                                                    error: e.to_string(),
                                                }).await;
                                            }
                                        }
                                    });
                                    last_log_poll = std::time::Instant::now();
                                }
                            }
                        }
                        KeyCode::Char('o') => {
                            if let Some(idx) = table_state.selected() {
                                if let Some(svc) = services.get(idx) {
                                    if let Some(ref uri) = svc.uri {
                                        let _ = open::that(uri);
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}
