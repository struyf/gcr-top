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
    BannerType, RevisionTrafficItem, TrafficModalAction, TrafficModalState, TrafficModalStatus,
    UiState,
};

#[derive(Parser, Debug)]
#[command(name = "gcr-top", about = "Blazingly fast TUI for Google Cloud Run")]
struct Args {
    #[arg(short, long, env = "CLOUDSDK_CORE_PROJECT")]
    project: String,

    #[arg(short, long, default_value = "-")]
    region: String,
}

#[derive(Debug)]
pub enum AppEvent {
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

#[derive(Debug, PartialEq, Eq)]
pub enum AppAction {
    None,
    FetchServices,
}

pub struct AppState {
    pub services: Vec<Service>,
    pub is_fetching_services: bool,
    pub services_error: Option<String>,
    pub is_auth_error: bool,
    pub show_logs: bool,
    pub logs: Vec<LogEntry>,
    pub log_error_msg: Option<String>,
    pub selected_service_name: String,
    pub is_fetching_logs: bool,
    pub traffic_modal: Option<TrafficModalState>,
    pub banner_message: Option<(String, BannerType, std::time::Instant)>,
    pub table_state: TableState,
    pub spinner_tick: usize,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    pub fn new() -> Self {
        let mut table_state = TableState::default();
        table_state.select(Some(0));
        Self {
            services: Vec::new(),
            is_fetching_services: true,
            services_error: None,
            is_auth_error: false,
            show_logs: false,
            logs: Vec::new(),
            log_error_msg: None,
            selected_service_name: String::new(),
            is_fetching_logs: false,
            traffic_modal: None,
            banner_message: None,
            table_state,
            spinner_tick: 0,
        }
    }

    pub fn handle_event(&mut self, event: AppEvent) -> AppAction {
        match event {
            AppEvent::ServicesUpdated(updated) => {
                self.services = updated;
                self.is_fetching_services = false;
                self.services_error = None;
                self.is_auth_error = false;
                if self.table_state.selected().is_none() && !self.services.is_empty() {
                    self.table_state.select(Some(0));
                }
                AppAction::None
            }
            AppEvent::ServicesError(err) => {
                self.is_fetching_services = false;
                let is_auth = err.contains("gcloud auth login")
                    || err.contains("authentication")
                    || err.contains("HTTP 401")
                    || err.contains("Unauthenticated");

                if self.services.is_empty() {
                    self.services_error = Some(err);
                    self.is_auth_error = is_auth;
                } else {
                    self.banner_message =
                        Some((err, BannerType::Error, std::time::Instant::now()));
                }
                AppAction::None
            }
            AppEvent::LogsUpdated {
                service_name,
                logs: new_logs,
            } => {
                if service_name == self.selected_service_name {
                    self.logs = new_logs;
                    self.log_error_msg = None;
                    self.is_fetching_logs = false;
                }
                AppAction::None
            }
            AppEvent::LogError {
                service_name,
                error,
            } => {
                if service_name == self.selected_service_name {
                    self.log_error_msg = Some(error);
                    self.is_fetching_logs = false;
                }
                AppAction::None
            }
            AppEvent::RevisionsFetched {
                service_name,
                result,
            } => {
                if let Some(ref mut modal) = self.traffic_modal
                    && modal.service_name == service_name
                {
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
                                self.banner_message = Some((
                                    format!("Warning: Could not fetch all revisions: {}", err),
                                    BannerType::Info,
                                    std::time::Instant::now(),
                                ));
                            }
                        }
                    }
                }
                AppAction::None
            }
            AppEvent::TrafficSplitResult {
                service_name,
                result,
            } => {
                let mut action = AppAction::None;
                if let Some(ref mut modal) = self.traffic_modal
                    && modal.service_name == service_name
                {
                    match result {
                        Ok(()) => {
                            modal.status = TrafficModalStatus::Success(
                                "Traffic split updated successfully! Press [Enter] or [Esc] to return."
                                    .to_string(),
                            );
                            self.banner_message = Some((
                                format!("Traffic split updated for {}", service_name),
                                BannerType::Success,
                                std::time::Instant::now(),
                            ));
                            action = AppAction::FetchServices;
                        }
                        Err(err) => {
                            modal.status = TrafficModalStatus::Error(format!(
                                "Failed to update traffic split: {}",
                                err
                            ));
                        }
                    }
                }
                action
            }
        }
    }
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

async fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    client: Arc<client::GcpClient>,
    project: &str,
    region: &str,
) -> Result<()> {
    let (tx, mut rx) = mpsc::channel::<AppEvent>(64);
    let mut app_state = AppState::new();

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
        app_state.spinner_tick = app_state.spinner_tick.wrapping_add(1);

        // Auto-dismiss banner if timeout has expired
        if let Some((_, ref b_type, created_at)) = app_state.banner_message {
            let timeout = match b_type {
                BannerType::Info | BannerType::Success => Duration::from_secs(8),
                BannerType::Error => Duration::from_secs(12),
            };
            if created_at.elapsed() > timeout {
                app_state.banner_message = None;
            }
        }

        // Process all queued async events
        while let Ok(event) = rx.try_recv() {
            let action = app_state.handle_event(event);
            if let AppAction::FetchServices = action {
                spawn_fetch_services(Arc::clone(&client), tx.clone());
            }
        }

        // Periodic background log polling (7s)
        if app_state.show_logs
            && !app_state.selected_service_name.is_empty()
            && last_log_poll.elapsed() > Duration::from_secs(7)
        {
            spawn_fetch_logs(
                Arc::clone(&client),
                tx.clone(),
                app_state.selected_service_name.clone(),
            );
            last_log_poll = std::time::Instant::now();
        }

        terminal.draw(|f| {
            let banner_ref = app_state
                .banner_message
                .as_ref()
                .map(|(msg, b_type, _)| (msg.as_str(), b_type));

            ui::render_ui(
                f,
                UiState {
                    project,
                    region,
                    services: &app_state.services,
                    table_state: &mut app_state.table_state,
                    is_fetching_services: app_state.is_fetching_services,
                    services_error: app_state.services_error.as_deref(),
                    is_auth_error: app_state.is_auth_error,
                    show_logs: app_state.show_logs,
                    logs: &app_state.logs,
                    selected_service_name: &app_state.selected_service_name,
                    is_fetching_logs: app_state.is_fetching_logs,
                    log_error_msg: app_state.log_error_msg.as_deref(),
                    banner_message: banner_ref,
                    traffic_modal: app_state.traffic_modal.as_ref(),
                    spinner_tick: app_state.spinner_tick,
                },
            );
        })?;

        // Non-blocking keyboard event polling (30ms)
        if event::poll(Duration::from_millis(30))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            // Modal interactions take precedence
            if let Some(ref mut modal) = app_state.traffic_modal {
                let action = modal.handle_key(key.code);
                match action {
                    TrafficModalAction::Close => {
                        app_state.traffic_modal = None;
                    }
                    TrafficModalAction::Submit(splits) => {
                        spawn_set_traffic_split(
                            Arc::clone(&client),
                            tx.clone(),
                            modal.service_name.clone(),
                            splits,
                        );
                    }
                    TrafficModalAction::None => {}
                }
                continue;
            }

            // Main view interactions
            match key.code {
                KeyCode::Char('q') => return Ok(()),
                KeyCode::Esc => {
                    if app_state.banner_message.is_some() {
                        app_state.banner_message = None;
                    } else if app_state.show_logs {
                        app_state.show_logs = false;
                        app_state.log_error_msg = None;
                    }
                }
                KeyCode::Char('s') => {
                    if let Some(modal) = open_traffic_modal(
                        &app_state.services,
                        &app_state.table_state,
                        &client,
                        &tx,
                    ) {
                        app_state.traffic_modal = Some(modal);
                    } else {
                        app_state.banner_message = Some((
                            "No service selected".to_string(),
                            BannerType::Info,
                            std::time::Instant::now(),
                        ));
                    }
                }
                KeyCode::Char('l') => {
                    if let Some(idx) = app_state.table_state.selected()
                        && let Some(svc) = app_state.services.get(idx)
                    {
                        app_state.selected_service_name = svc.short_name().to_string();
                        app_state.show_logs = true;
                        app_state.logs.clear();
                        app_state.log_error_msg = None;
                        app_state.is_fetching_logs = true;
                        spawn_fetch_logs(
                            Arc::clone(&client),
                            tx.clone(),
                            app_state.selected_service_name.clone(),
                        );
                        last_log_poll = std::time::Instant::now();
                    }
                }
                KeyCode::Char('r') => {
                    app_state.is_fetching_services = true;
                    app_state.services_error = None;
                    app_state.banner_message = None;
                    spawn_fetch_services(Arc::clone(&client), tx.clone());
                    if app_state.show_logs && !app_state.selected_service_name.is_empty() {
                        app_state.is_fetching_logs = true;
                        spawn_fetch_logs(
                            Arc::clone(&client),
                            tx.clone(),
                            app_state.selected_service_name.clone(),
                        );
                        last_log_poll = std::time::Instant::now();
                    }
                }
                KeyCode::Char('o') => {
                    if let Some(idx) = app_state.table_state.selected()
                        && let Some(svc) = app_state.services.get(idx)
                        && let Some(ref uri) = svc.uri
                    {
                        let _ = open::that(uri);
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    let i = match app_state.table_state.selected() {
                        Some(i) => {
                            if app_state.services.is_empty()
                                || i >= app_state.services.len().saturating_sub(1)
                            {
                                0
                            } else {
                                i + 1
                            }
                        }
                        None => 0,
                    };
                    app_state.table_state.select(Some(i));
                    if app_state.show_logs
                        && let Some(svc) = app_state.services.get(i)
                    {
                        app_state.selected_service_name = svc.short_name().to_string();
                        app_state.logs.clear();
                        app_state.is_fetching_logs = true;
                        spawn_fetch_logs(
                            Arc::clone(&client),
                            tx.clone(),
                            app_state.selected_service_name.clone(),
                        );
                        last_log_poll = std::time::Instant::now();
                    }
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    let i = match app_state.table_state.selected() {
                        Some(i) => {
                            if i == 0 {
                                app_state.services.len().saturating_sub(1)
                            } else {
                                i - 1
                            }
                        }
                        None => 0,
                    };
                    app_state.table_state.select(Some(i));
                    if app_state.show_logs
                        && let Some(svc) = app_state.services.get(i)
                    {
                        app_state.selected_service_name = svc.short_name().to_string();
                        app_state.logs.clear();
                        app_state.is_fetching_logs = true;
                        spawn_fetch_logs(
                            Arc::clone(&client),
                            tx.clone(),
                            app_state.selected_service_name.clone(),
                        );
                        last_log_poll = std::time::Instant::now();
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_state_initial() {
        let state = AppState::new();
        assert!(state.services.is_empty());
        assert!(state.is_fetching_services);
        assert!(state.services_error.is_none());
        assert!(!state.is_auth_error);
        assert!(!state.show_logs);
        assert!(state.traffic_modal.is_none());
        assert!(state.banner_message.is_none());
        assert_eq!(state.table_state.selected(), Some(0));
    }

    #[test]
    fn test_app_event_services_updated() {
        let mut state = AppState::new();
        let services = vec![Service {
            name: "projects/p/locations/l/services/my-service".to_string(),
            uri: None,
            latest_ready_revision: Some("rev-1".to_string()),
            conditions: None,
            traffic_statuses: None,
        }];

        let action = state.handle_event(AppEvent::ServicesUpdated(services));
        assert_eq!(action, AppAction::None);
        assert_eq!(state.services.len(), 1);
        assert!(!state.is_fetching_services);
        assert!(state.services_error.is_none());
    }

    #[test]
    fn test_app_event_services_error_auth_vs_general() {
        let mut state = AppState::new();
        // Initial empty services -> sets services_error screen
        state.handle_event(AppEvent::ServicesError(
            "Google Cloud authentication expired (HTTP 401). Please run 'gcloud auth login'.".to_string(),
        ));
        assert!(state.is_auth_error);
        assert!(state.services_error.is_some());
        assert!(state.banner_message.is_none());

        // When services already exist, error appears in banner instead
        state.services.push(Service {
            name: "web".to_string(),
            uri: None,
            latest_ready_revision: None,
            conditions: None,
            traffic_statuses: None,
        });
        state.handle_event(AppEvent::ServicesError("Network failure".to_string()));
        assert!(state.banner_message.is_some());
        let (msg, b_type, _) = state.banner_message.unwrap();
        assert_eq!(msg, "Network failure");
        assert_eq!(b_type, BannerType::Error);
    }

    #[test]
    fn test_app_event_revisions_fetched_success() {
        let mut state = AppState::new();
        let mut modal = TrafficModalState::new("web-service".to_string());
        modal.status = TrafficModalStatus::FetchingRevisions;
        state.traffic_modal = Some(modal);

        let revs = vec![
            Revision {
                name: "projects/p/locations/l/services/web-service/revisions/rev-001".to_string(),
                conditions: None,
                create_time: None,
            },
            Revision {
                name: "projects/p/locations/l/services/web-service/revisions/rev-002".to_string(),
                conditions: None,
                create_time: None,
            },
        ];

        let action = state.handle_event(AppEvent::RevisionsFetched {
            service_name: "web-service".to_string(),
            result: Ok(revs),
        });
        assert_eq!(action, AppAction::None);

        let modal = state.traffic_modal.unwrap();
        assert_eq!(modal.status, TrafficModalStatus::Idle);
        assert_eq!(modal.revisions.len(), 2);
        assert_eq!(modal.revisions[0].revision_name, "rev-001");
        assert_eq!(modal.revisions[0].percent, 100); // Default first revision gets 100%
        assert_eq!(modal.revisions[1].percent, 0);
        assert_eq!(modal.selected_index, 0);
    }

    #[test]
    fn test_app_event_revisions_fetched_error_with_no_prior_revisions() {
        let mut state = AppState::new();
        let mut modal = TrafficModalState::new("web-service".to_string());
        modal.status = TrafficModalStatus::FetchingRevisions;
        state.traffic_modal = Some(modal);

        let action = state.handle_event(AppEvent::RevisionsFetched {
            service_name: "web-service".to_string(),
            result: Err("GCP API 404 Not Found".to_string()),
        });
        assert_eq!(action, AppAction::None);

        let modal = state.traffic_modal.unwrap();
        match modal.status {
            TrafficModalStatus::Error(msg) => {
                assert!(msg.contains("GCP API 404 Not Found"));
            }
            other => panic!("Expected Error status, got {:?}", other),
        }
    }

    #[test]
    fn test_app_event_revisions_fetched_error_with_existing_revisions_falls_back() {
        let mut state = AppState::new();
        let mut modal = TrafficModalState::new("web-service".to_string());
        modal.revisions.push(RevisionTrafficItem {
            revision_name: "rev-current".to_string(),
            percent: 100,
            tag: "-".to_string(),
        });
        modal.status = TrafficModalStatus::FetchingRevisions;
        state.traffic_modal = Some(modal);

        let action = state.handle_event(AppEvent::RevisionsFetched {
            service_name: "web-service".to_string(),
            result: Err("Temporary network outage".to_string()),
        });
        assert_eq!(action, AppAction::None);

        // Fallback keeps modal open in Idle state and sets warning banner
        let modal = state.traffic_modal.unwrap();
        assert_eq!(modal.status, TrafficModalStatus::Idle);
        assert_eq!(modal.revisions.len(), 1);
        assert!(state.banner_message.is_some());
        let (msg, b_type, _) = state.banner_message.unwrap();
        assert!(msg.contains("Temporary network outage"));
        assert_eq!(b_type, BannerType::Info);
    }

    #[test]
    fn test_app_event_traffic_split_result_success_and_error() {
        let mut state = AppState::new();
        let mut modal = TrafficModalState::new("api-svc".to_string());
        modal.status = TrafficModalStatus::Submitting;
        state.traffic_modal = Some(modal);

        // Success transition
        let action = state.handle_event(AppEvent::TrafficSplitResult {
            service_name: "api-svc".to_string(),
            result: Ok(()),
        });
        assert_eq!(action, AppAction::FetchServices);
        match &state.traffic_modal.as_ref().unwrap().status {
            TrafficModalStatus::Success(msg) => {
                assert!(msg.contains("Traffic split updated successfully"));
            }
            other => panic!("Expected Success, got {:?}", other),
        }
        assert!(state.banner_message.is_some());

        // Error transition
        let mut modal = TrafficModalState::new("api-svc".to_string());
        modal.status = TrafficModalStatus::Submitting;
        state.traffic_modal = Some(modal);

        let action = state.handle_event(AppEvent::TrafficSplitResult {
            service_name: "api-svc".to_string(),
            result: Err("Permission denied on Cloud Run service".to_string()),
        });
        assert_eq!(action, AppAction::None);
        match &state.traffic_modal.as_ref().unwrap().status {
            TrafficModalStatus::Error(msg) => {
                assert!(msg.contains("Permission denied on Cloud Run service"));
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }
}
