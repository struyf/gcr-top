mod client;
mod models;
mod ui;

use anyhow::Result;
use clap::Parser;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use models::{LogEntry, Revision, Service};
use ratatui::{backend::CrosstermBackend, widgets::TableState, Terminal};
use std::{collections::HashMap, io, sync::Arc, time::Duration};
use tokio::sync::mpsc;
use ui::{
    BannerType, RevisionTrafficItem, TrafficModalState, TrafficModalStatus, UiState,
};

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
    ServicesError(String),
    LogsUpdated {
        service_name: String,
        logs: Vec<LogEntry>,
    },
    LogError {
        service_name: String,
        error: String,
    },
    RevisionsFetched {
        service_name: String,
        result: Result<Vec<Revision>, String>,
    },
    TrafficSplitResult {
        service_name: String,
        result: Result<(), String>,
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

    let res = run_app(&mut terminal, client, &args.project, &args.region).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        eprintln!("Error running gcr-top: {:?}", err);
    }

    Ok(())
}

fn spawn_fetch_services(client: Arc<client::GcpClient>, tx: mpsc::Sender<AppEvent>) {
    tokio::spawn(async move {
        match client.list_services().await {
            Ok(fetched) => {
                let _ = tx.send(AppEvent::ServicesUpdated(fetched)).await;
            }
            Err(e) => {
                let _ = tx.send(AppEvent::ServicesError(e.to_string())).await;
            }
        }
    });
}

fn spawn_fetch_logs(client: Arc<client::GcpClient>, tx: mpsc::Sender<AppEvent>, service_name: String) {
    tokio::spawn(async move {
        match client.fetch_recent_logs(&service_name).await {
            Ok(entries) => {
                let _ = tx.send(AppEvent::LogsUpdated {
                    service_name,
                    logs: entries,
                }).await;
            }
            Err(e) => {
                let _ = tx.send(AppEvent::LogError {
                    service_name,
                    error: e.to_string(),
                }).await;
            }
        }
    });
}

fn spawn_fetch_revisions(client: Arc<client::GcpClient>, tx: mpsc::Sender<AppEvent>, service_name: String) {
    tokio::spawn(async move {
        match client.list_revisions(&service_name).await {
            Ok(revs) => {
                let _ = tx.send(AppEvent::RevisionsFetched {
                    service_name,
                    result: Ok(revs),
                }).await;
            }
            Err(e) => {
                let _ = tx.send(AppEvent::RevisionsFetched {
                    service_name,
                    result: Err(e.to_string()),
                }).await;
            }
        }
    });
}

fn spawn_set_traffic_split(
    client: Arc<client::GcpClient>,
    tx: mpsc::Sender<AppEvent>,
    service_name: String,
    splits: Vec<(String, i32)>,
) {
    tokio::spawn(async move {
        let splits_ref: Vec<(&str, i32)> = splits.iter().map(|(r, p)| (r.as_str(), *p)).collect();
        match client.set_traffic_split(&service_name, splits_ref).await {
            Ok(()) => {
                let _ = tx.send(AppEvent::TrafficSplitResult {
                    service_name,
                    result: Ok(()),
                }).await;
            }
            Err(e) => {
                let _ = tx.send(AppEvent::TrafficSplitResult {
                    service_name,
                    result: Err(e.to_string()),
                }).await;
            }
        }
    });
}

fn open_traffic_modal(
    services: &[Service],
    table_state: &TableState,
    client: &Arc<client::GcpClient>,
    tx: &mpsc::Sender<AppEvent>,
) -> Option<TrafficModalState> {
    let idx = table_state.selected()?;
    let svc = services.get(idx)?;
    let svc_name = svc.short_name().to_string();

    let mut modal = TrafficModalState::new(svc_name.clone());

    for (rev, pct, tag) in svc.traffic_details() {
        modal.revisions.push(RevisionTrafficItem {
            revision_name: rev,
            percent: pct,
            tag,
        });
    }

    spawn_fetch_revisions(Arc::clone(client), tx.clone(), svc_name);
    Some(modal)
}

fn handle_modal_key(
    key: KeyCode,
    modal: &mut TrafficModalState,
    client: &Arc<client::GcpClient>,
    tx: &mpsc::Sender<AppEvent>,
) -> bool {
    match key {
        KeyCode::Esc => true,
        KeyCode::Enter => match &modal.status {
            TrafficModalStatus::Success(_) => true,
            TrafficModalStatus::Submitting | TrafficModalStatus::FetchingRevisions => false,
            TrafficModalStatus::Idle | TrafficModalStatus::Error(_) => {
                let total = modal.total_percent();
                if total != 100 {
                    modal.status = TrafficModalStatus::Error(format!(
                        "Total traffic must equal exactly 100% (currently {}%)",
                        total
                    ));
                } else {
                    modal.status = TrafficModalStatus::Submitting;
                    let splits: Vec<(String, i32)> = modal
                        .revisions
                        .iter()
                        .filter(|r| r.percent > 0)
                        .map(|r| (r.revision_name.clone(), r.percent))
                        .collect();

                    spawn_set_traffic_split(
                        Arc::clone(client),
                        tx.clone(),
                        modal.service_name.clone(),
                        splits,
                    );
                }
                false
            }
        },
        KeyCode::Up | KeyCode::Char('k') => {
            modal.selected_index = modal.selected_index.saturating_sub(1);
            modal.input_buffer.clear();
            false
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if modal.selected_index + 1 < modal.revisions.len() {
                modal.selected_index += 1;
            }
            modal.input_buffer.clear();
            false
        }
        KeyCode::Right | KeyCode::Char('+') | KeyCode::Char('l') => {
            if let Some(item) = modal.revisions.get_mut(modal.selected_index) {
                item.percent = (item.percent + 5).min(100);
                modal.input_buffer = item.percent.to_string();
            }
            false
        }
        KeyCode::Left | KeyCode::Char('-') | KeyCode::Char('h') => {
            if let Some(item) = modal.revisions.get_mut(modal.selected_index) {
                item.percent = item.percent.saturating_sub(5);
                modal.input_buffer = item.percent.to_string();
            }
            false
        }
        KeyCode::Char(']') => {
            if let Some(item) = modal.revisions.get_mut(modal.selected_index) {
                item.percent = (item.percent + 1).min(100);
                modal.input_buffer = item.percent.to_string();
            }
            false
        }
        KeyCode::Char('[') => {
            if let Some(item) = modal.revisions.get_mut(modal.selected_index) {
                item.percent = item.percent.saturating_sub(1);
                modal.input_buffer = item.percent.to_string();
            }
            false
        }
        KeyCode::Char('c') => {
            if let Some(item) = modal.revisions.get_mut(modal.selected_index) {
                item.percent = 0;
            }
            modal.input_buffer.clear();
            false
        }
        KeyCode::Char(d) if d.is_ascii_digit() => {
            if modal.input_buffer.len() < 3 {
                modal.input_buffer.push(d);
                if let Ok(val) = modal.input_buffer.parse::<i32>()
                    && let Some(item) = modal.revisions.get_mut(modal.selected_index) {
                        item.percent = val.clamp(0, 100);
                    }
            }
            false
        }
        KeyCode::Backspace => {
            if !modal.input_buffer.is_empty() {
                modal.input_buffer.pop();
                let val = modal.input_buffer.parse::<i32>().unwrap_or(0);
                if let Some(item) = modal.revisions.get_mut(modal.selected_index) {
                    item.percent = val;
                }
            } else if let Some(item) = modal.revisions.get_mut(modal.selected_index) {
                item.percent = 0;
            }
            false
        }
        _ => false,
    }
}

async fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    client: Arc<client::GcpClient>,
    project: &str,
    region: &str,
) -> Result<()> {
    let (tx, mut rx) = mpsc::channel::<AppEvent>(64);

    let mut table_state = TableState::default();
    table_state.select(Some(0));

    let mut services: Vec<Service> = Vec::new();
    let mut is_fetching_services = true;
    let mut services_error: Option<String> = None;
    let mut is_auth_error = false;

    let mut show_logs = false;
    let mut logs: Vec<LogEntry> = Vec::new();
    let mut log_error_msg: Option<String> = None;
    let mut selected_service_name = String::new();
    let mut is_fetching_logs = false;

    let mut traffic_modal: Option<TrafficModalState> = None;
    let mut banner_message: Option<(String, BannerType)> = None;
    let mut spinner_tick: usize = 0;

    // Initial fetch of services
    spawn_fetch_services(Arc::clone(&client), tx.clone());

    // Periodic background service polling loop (20s)
    let client_svc = Arc::clone(&client);
    let tx_svc = tx.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(20)).await;
            spawn_fetch_services(Arc::clone(&client_svc), tx_svc.clone());
        }
    });

    let mut last_log_poll = std::time::Instant::now();

    loop {
        spinner_tick = spinner_tick.wrapping_add(1);

        // Process all queued async events
        while let Ok(event) = rx.try_recv() {
            match event {
                AppEvent::ServicesUpdated(updated) => {
                    services = updated;
                    is_fetching_services = false;
                    services_error = None;
                    is_auth_error = false;
                    if table_state.selected().is_none() && !services.is_empty() {
                        table_state.select(Some(0));
                    }
                }
                AppEvent::ServicesError(err) => {
                    is_fetching_services = false;
                    let is_auth = err.contains("gcloud auth login")
                        || err.contains("authentication")
                        || err.contains("HTTP 401")
                        || err.contains("Unauthenticated");

                    if services.is_empty() {
                        services_error = Some(err);
                        is_auth_error = is_auth;
                    } else {
                        banner_message = Some((err, BannerType::Error));
                    }
                }
                AppEvent::LogsUpdated {
                    service_name,
                    logs: new_logs,
                } => {
                    if service_name == selected_service_name {
                        logs = new_logs;
                        log_error_msg = None;
                        is_fetching_logs = false;
                    }
                }
                AppEvent::LogError {
                    service_name,
                    error,
                } => {
                    if service_name == selected_service_name {
                        log_error_msg = Some(error);
                        is_fetching_logs = false;
                    }
                }
                AppEvent::RevisionsFetched {
                    service_name,
                    result,
                } => {
                    if let Some(ref mut modal) = traffic_modal
                        && modal.service_name == service_name {
                            match result {
                                Ok(fetched_revs) => {
                                    if !fetched_revs.is_empty() {
                                        let existing_map: HashMap<String, (i32, String)> = modal
                                            .revisions
                                            .iter()
                                            .map(|r| (r.revision_name.clone(), (r.percent, r.tag.clone())))
                                            .collect();

                                        let mut items = Vec::new();
                                        for rev in fetched_revs {
                                            let name = rev.short_name().to_string();
                                            let (pct, tag) = existing_map
                                                .get(&name)
                                                .cloned()
                                                .unwrap_or((0, "-".to_string()));
                                            items.push(RevisionTrafficItem {
                                                revision_name: name,
                                                percent: pct,
                                                tag,
                                            });
                                        }

                                        let sum: i32 = items.iter().map(|i| i.percent).sum();
                                        if sum == 0 && !items.is_empty() {
                                            items[0].percent = 100;
                                        }

                                        modal.revisions = items;
                                        modal.selected_index = 0;
                                    }
                                    modal.status = TrafficModalStatus::Idle;
                                }
                                Err(err) => {
                                    if modal.revisions.is_empty() {
                                        modal.status = TrafficModalStatus::Error(format!(
                                            "Failed to list revisions: {}",
                                            err
                                        ));
                                    } else {
                                        modal.status = TrafficModalStatus::Idle;
                                        banner_message = Some((
                                            format!("Warning: Could not fetch all revisions: {}", err),
                                            BannerType::Info,
                                        ));
                                    }
                                }
                            }
                        }
                }
                AppEvent::TrafficSplitResult {
                    service_name,
                    result,
                } => {
                    if let Some(ref mut modal) = traffic_modal
                        && modal.service_name == service_name {
                            match result {
                                Ok(()) => {
                                    modal.status = TrafficModalStatus::Success(
                                        "Traffic split updated successfully! Press [Enter] or [Esc] to return."
                                            .to_string(),
                                    );
                                    banner_message = Some((
                                        format!("Traffic split updated for {}", service_name),
                                        BannerType::Success,
                                    ));
                                    spawn_fetch_services(Arc::clone(&client), tx.clone());
                                }
                                Err(err) => {
                                    modal.status = TrafficModalStatus::Error(format!(
                                        "Failed to update traffic split: {}",
                                        err
                                    ));
                                }
                            }
                        }
                }
            }
        }

        // Periodic background log polling (7s)
        if show_logs && !selected_service_name.is_empty() && last_log_poll.elapsed() > Duration::from_secs(7) {
            spawn_fetch_logs(Arc::clone(&client), tx.clone(), selected_service_name.clone());
            last_log_poll = std::time::Instant::now();
        }

        terminal.draw(|f| {
            let banner_ref = banner_message.as_ref().map(|(msg, b_type)| (msg.as_str(), b_type));

            ui::render_ui(
                f,
                UiState {
                    project,
                    region,
                    services: &services,
                    table_state: &mut table_state,
                    is_fetching_services,
                    services_error: services_error.as_deref(),
                    is_auth_error,
                    show_logs,
                    logs: &logs,
                    selected_service_name: &selected_service_name,
                    is_fetching_logs,
                    log_error_msg: log_error_msg.as_deref(),
                    banner_message: banner_ref,
                    traffic_modal: traffic_modal.as_ref(),
                    spinner_tick,
                },
            );
        })?;

        // Non-blocking keyboard event polling (30ms)
        if event::poll(Duration::from_millis(30))?
            && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press {
                    // Modal interactions take precedence
                    if let Some(ref mut modal) = traffic_modal {
                        let should_close = handle_modal_key(key.code, modal, &client, &tx);
                        if should_close {
                            traffic_modal = None;
                        }
                        continue;
                    }

                    // Main view interactions
                    match key.code {
                        KeyCode::Char('q') => return Ok(()),
                        KeyCode::Esc => {
                            if banner_message.is_some() {
                                banner_message = None;
                            } else if show_logs {
                                show_logs = false;
                                log_error_msg = None;
                            }
                        }
                        KeyCode::Char('s') => {
                            if let Some(modal) = open_traffic_modal(&services, &table_state, &client, &tx) {
                                traffic_modal = Some(modal);
                            } else {
                                banner_message = Some(("No service selected".to_string(), BannerType::Info));
                            }
                        }
                        KeyCode::Char('l') => {
                            if let Some(idx) = table_state.selected()
                                && let Some(svc) = services.get(idx) {
                                    selected_service_name = svc.short_name().to_string();
                                    show_logs = true;
                                    logs.clear();
                                    log_error_msg = None;
                                    is_fetching_logs = true;
                                    spawn_fetch_logs(Arc::clone(&client), tx.clone(), selected_service_name.clone());
                                    last_log_poll = std::time::Instant::now();
                                }
                        }
                        KeyCode::Char('r') => {
                            is_fetching_services = true;
                            services_error = None;
                            banner_message = None;
                            spawn_fetch_services(Arc::clone(&client), tx.clone());
                            if show_logs && !selected_service_name.is_empty() {
                                is_fetching_logs = true;
                                spawn_fetch_logs(Arc::clone(&client), tx.clone(), selected_service_name.clone());
                                last_log_poll = std::time::Instant::now();
                            }
                        }
                        KeyCode::Char('o') => {
                            if let Some(idx) = table_state.selected()
                                && let Some(svc) = services.get(idx)
                                    && let Some(ref uri) = svc.uri {
                                        let _ = open::that(uri);
                                    }
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
                            if show_logs
                                && let Some(svc) = services.get(i) {
                                    selected_service_name = svc.short_name().to_string();
                                    logs.clear();
                                    is_fetching_logs = true;
                                    spawn_fetch_logs(Arc::clone(&client), tx.clone(), selected_service_name.clone());
                                    last_log_poll = std::time::Instant::now();
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
                            if show_logs
                                && let Some(svc) = services.get(i) {
                                    selected_service_name = svc.short_name().to_string();
                                    logs.clear();
                                    is_fetching_logs = true;
                                    spawn_fetch_logs(Arc::clone(&client), tx.clone(), selected_service_name.clone());
                                    last_log_poll = std::time::Instant::now();
                                }
                        }
                        _ => {}
                    }
                }
    }
}
