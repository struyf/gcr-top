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
    BannerType, RevisionTrafficItem, Toast, ToastKind, TrafficModalAction, TrafficModalState,
    TrafficModalStatus, TrafficSplitTarget, UiState,
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
    pub toasts: Vec<Toast>,
    pub table_state: TableState,
    pub spinner_tick: usize,
    pub search_query: String,
    pub is_searching: bool,
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
            toasts: Vec::new(),
            table_state,
            spinner_tick: 0,
            search_query: String::new(),
            is_searching: false,
        }
    }

    pub fn add_toast(&mut self, message: impl Into<String>, kind: ToastKind) {
        if self.toasts.len() >= 4 {
            self.toasts.remove(0);
        }
        self.toasts.push(Toast::new(message, kind));
    }

    pub fn prune_toasts(&mut self) {
        self.toasts.retain(|t| !t.is_expired());
    }

    pub fn filter_services<'a>(services: &'a [Service], query: &str) -> Vec<&'a Service> {
        let trimmed = query.trim().to_lowercase();
        if trimmed.is_empty() {
            return services.iter().collect();
        }
        services
            .iter()
            .filter(|s| {
                s.short_name().to_lowercase().contains(&trimmed)
                    || s.location()
                        .map(|l| l.to_lowercase().contains(&trimmed))
                        .unwrap_or(false)
            })
            .collect()
    }

    pub fn filtered_services(&self) -> Vec<&Service> {
        Self::filter_services(&self.services, &self.search_query)
    }

    pub fn clamp_selection(&mut self) {
        let count = self.filtered_services().len();
        if let Some(selected) = self.table_state.selected() {
            if count == 0 {
                if selected != 0 {
                    self.table_state.select(Some(0));
                }
            } else if selected >= count {
                self.table_state.select(Some(0));
            }
        } else {
            self.table_state.select(Some(0));
        }
    }

    pub fn handle_search_key(&mut self, key: KeyCode) {
        match key {
            KeyCode::Esc => {
                self.search_query.clear();
                self.is_searching = false;
                self.clamp_selection();
            }
            KeyCode::Enter => {
                self.is_searching = false;
            }
            KeyCode::Backspace => {
                self.search_query.pop();
                self.clamp_selection();
            }
            KeyCode::Char(c) => {
                self.search_query.push(c);
                self.clamp_selection();
            }
            _ => {}
        }
    }

    pub fn navigate_down(&mut self) -> Option<String> {
        self.clamp_selection();
        let (i, svc_name) = {
            let filtered = self.filtered_services();
            let count = filtered.len();
            if count == 0 {
                (0, None)
            } else {
                let next_i = match self.table_state.selected() {
                    Some(idx) => {
                        if idx >= count.saturating_sub(1) {
                            0
                        } else {
                            idx + 1
                        }
                    }
                    None => 0,
                };
                (next_i, filtered.get(next_i).map(|s| s.short_name().to_string()))
            }
        };
        self.table_state.select(Some(i));
        svc_name
    }

    pub fn navigate_up(&mut self) -> Option<String> {
        self.clamp_selection();
        let (i, svc_name) = {
            let filtered = self.filtered_services();
            let count = filtered.len();
            if count == 0 {
                (0, None)
            } else {
                let prev_i = match self.table_state.selected() {
                    Some(idx) => {
                        if idx == 0 || idx >= count {
                            count.saturating_sub(1)
                        } else {
                            idx - 1
                        }
                    }
                    None => 0,
                };
                (prev_i, filtered.get(prev_i).map(|s| s.short_name().to_string()))
            }
        };
        self.table_state.select(Some(i));
        svc_name
    }

    pub fn dismiss_oldest_toast(&mut self) -> bool {
        if !self.toasts.is_empty() {
            self.toasts.remove(0);
            true
        } else {
            false
        }
    }

    pub fn handle_event(&mut self, event: AppEvent) -> AppAction {
        match event {
            AppEvent::ServicesUpdated(updated) => {
                self.services = updated;
                self.is_fetching_services = false;
                self.services_error = None;
                self.is_auth_error = false;
                self.clamp_selection();
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
                    self.services_error = Some(err.clone());
                    self.is_auth_error = is_auth;
                } else {
                    self.banner_message =
                        Some((err.clone(), BannerType::Error, std::time::Instant::now()));
                    self.add_toast(err, ToastKind::Error);
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
                    self.log_error_msg = Some(error.clone());
                    self.is_fetching_logs = false;
                    self.add_toast(format!("Log error ({}): {}", service_name, error), ToastKind::Warning);
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
                                for (idx, rev) in fetched_revs.into_iter().enumerate() {
                                    let name = rev.short_name().to_string();
                                    let (pct, tag) = existing_map
                                        .get(&name)
                                        .cloned()
                                        .unwrap_or((0, "-".to_string()));
                                    items.push(RevisionTrafficItem {
                                        revision_name: name,
                                        percent: pct,
                                        tag,
                                        is_latest: idx == 0,
                                    });
                                }

                                // Retain any existing revisions that were not in fetched list
                                for item in &modal.revisions {
                                    if !items.iter().any(|i| i.revision_name == item.revision_name) {
                                        items.push(item.clone());
                                    }
                                }

                                let sum: i32 = items.iter().map(|i| i.percent).sum();
                                if sum == 0 && !items.is_empty() {
                                    items[0].percent = 100;
                                }

                                modal.revisions = items;
                                if modal.selected_index >= modal.revisions.len() {
                                    modal.selected_index = 0;
                                }
                            }
                            modal.status = TrafficModalStatus::Idle;
                        }
                        Err(err) => {
                            if modal.revisions.is_empty() {
                                modal.status = TrafficModalStatus::Error(format!(
                                    "Failed to list revisions: {}",
                                    err
                                ));
                                self.add_toast(
                                    format!("Failed to list revisions: {}", err),
                                    ToastKind::Error,
                                );
                            } else {
                                modal.status = TrafficModalStatus::Idle;
                                self.banner_message = Some((
                                    format!("Warning: Could not fetch all revisions: {}", err),
                                    BannerType::Info,
                                    std::time::Instant::now(),
                                ));
                                self.add_toast(
                                    format!("Could not fetch revisions: {}", err),
                                    ToastKind::Warning,
                                );
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
                            self.add_toast(
                                format!("Traffic split updated for {}", service_name),
                                ToastKind::Success,
                            );
                            action = AppAction::FetchServices;
                        }
                        Err(err) => {
                            modal.status = TrafficModalStatus::Error(format!(
                                "Failed to update traffic split: {}",
                                err
                            ));
                            self.add_toast(
                                format!("Failed to update traffic: {}", err),
                                ToastKind::Error,
                            );
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
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        let _ = disable_raw_mode();
        original_hook(panic_info);
    }));

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

fn spawn_fetch_revisions(
    client: Arc<client::GcpClient>,
    tx: mpsc::Sender<AppEvent>,
    service_name: String,
    location: String,
) {
    tokio::spawn(async move {
        match client.list_revisions(&service_name, &location).await {
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
    full_service_name: String,
    splits: Vec<TrafficSplitTarget>,
) {
    tokio::spawn(async move {
        let splits_ref: Vec<(&str, i32, Option<&str>)> = splits
            .iter()
            .map(|s| (s.revision.as_str(), s.percent, s.tag.as_deref()))
            .collect();
        match client.set_traffic_split_with_tags(&full_service_name, splits_ref).await {
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

trait AsService {
    fn as_service(&self) -> &Service;
}

impl AsService for Service {
    fn as_service(&self) -> &Service {
        self
    }
}

impl AsService for &Service {
    fn as_service(&self) -> &Service {
        self
    }
}

impl AsService for &&Service {
    fn as_service(&self) -> &Service {
        self
    }
}

fn open_traffic_modal<T: AsService>(
    services: &[T],
    table_state: &TableState,
    client: &Arc<client::GcpClient>,
    tx: &mpsc::Sender<AppEvent>,
) -> Option<TrafficModalState> {
    let idx = table_state.selected()?;
    let svc = services.get(idx)?.as_service();
    let svc_name = svc.short_name().to_string();

    let mut modal = TrafficModalState::new(svc_name.clone(), svc.name.clone());

    let primary = svc.primary_revision();
    let details = svc.traffic_details();
    for (i, (rev, pct, tag)) in details.into_iter().enumerate() {
        if rev == "-" {
            continue;
        }
        let is_latest = (rev == primary) || (i == 0);
        modal.revisions.push(RevisionTrafficItem {
            revision_name: rev,
            percent: pct,
            tag,
            is_latest,
        });
    }

    let location = svc
        .concrete_location()
        .unwrap_or_else(|| {
            let reg = client.region();
            if reg != "-" {
                reg
            } else {
                ""
            }
        })
        .to_string();

    spawn_fetch_revisions(Arc::clone(client), tx.clone(), svc_name, location);
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
        app_state.prune_toasts();

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
            app_state.clamp_selection();
            let filtered = AppState::filter_services(&app_state.services, &app_state.search_query);
            let banner_ref = app_state
                .banner_message
                .as_ref()
                .map(|(msg, b_type, _)| (msg.as_str(), b_type));

            ui::render_ui(
                f,
                UiState {
                    project,
                    region,
                    services: &filtered,
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
                    toasts: &app_state.toasts,
                    spinner_tick: app_state.spinner_tick,
                    search_query: &app_state.search_query,
                    is_searching: app_state.is_searching,
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
                            modal.full_service_name.clone(),
                            splits,
                        );
                    }
                    TrafficModalAction::None => {}
                }
                continue;
            }

            // Search mode keystrokes take precedence over main view
            if app_state.is_searching {
                app_state.handle_search_key(key.code);
                continue;
            }

            // Main view interactions
            match key.code {
                KeyCode::Char('/') => {
                    app_state.is_searching = true;
                }
                KeyCode::Char('q') => return Ok(()),
                KeyCode::Esc => {
                    if app_state.banner_message.is_some() {
                        app_state.banner_message = None;
                    } else if app_state.dismiss_oldest_toast() {
                        // Dismissed oldest toast
                    } else if app_state.show_logs {
                        app_state.show_logs = false;
                        app_state.log_error_msg = None;
                    } else if !app_state.search_query.is_empty() {
                        app_state.search_query.clear();
                        app_state.clamp_selection();
                    }
                }
                KeyCode::Char('s') => {
                    app_state.clamp_selection();
                    let filtered = app_state.filtered_services();
                    if let Some(modal) = open_traffic_modal(
                        &filtered,
                        &app_state.table_state,
                        &client,
                        &tx,
                    ) {
                        app_state.traffic_modal = Some(modal);
                    } else {
                        app_state.add_toast("No service selected", ToastKind::Info);
                        app_state.banner_message = Some((
                            "No service selected to adjust traffic split".to_string(),
                            BannerType::Info,
                            std::time::Instant::now(),
                        ));
                    }
                }
                KeyCode::Char('l') => {
                    app_state.clamp_selection();
                    let filtered = app_state.filtered_services();
                    if let Some(idx) = app_state.table_state.selected()
                        && let Some(svc) = filtered.get(idx)
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
                    app_state.clamp_selection();
                    let filtered = app_state.filtered_services();
                    if let Some(idx) = app_state.table_state.selected()
                        && let Some(svc) = filtered.get(idx)
                        && let Some(ref uri) = svc.uri
                    {
                        let _ = open::that(uri);
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if let Some(svc_name) = app_state.navigate_down()
                        && app_state.show_logs
                    {
                        app_state.selected_service_name = svc_name;
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
                    if let Some(svc_name) = app_state.navigate_up()
                        && app_state.show_logs
                    {
                        app_state.selected_service_name = svc_name;
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
    fn test_search_and_filtering() {
        let mut state = AppState::new();
        state.services = vec![
            Service {
                name: "projects/p/locations/us-central1/services/web-frontend".to_string(),
                uri: Some("https://web.run.app".to_string()),
                latest_ready_revision: Some("rev-1".to_string()),
                conditions: None,
                traffic_statuses: None,
            },
            Service {
                name: "projects/p/locations/europe-west1/services/api-server".to_string(),
                uri: Some("https://api.run.app".to_string()),
                latest_ready_revision: Some("rev-2".to_string()),
                conditions: None,
                traffic_statuses: None,
            },
            Service {
                name: "projects/p/locations/us-east1/services/worker-queue".to_string(),
                uri: None,
                latest_ready_revision: Some("rev-3".to_string()),
                conditions: None,
                traffic_statuses: None,
            },
        ];

        // 1. Empty query returns all services
        assert_eq!(state.filtered_services().len(), 3);

        // 2. Case-insensitive match on short_name
        state.search_query = "API".to_string();
        let filtered = state.filtered_services();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].short_name(), "api-server");

        state.search_query = "Frontend".to_string();
        let filtered = state.filtered_services();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].short_name(), "web-frontend");

        // 3. Case-insensitive match on location
        state.search_query = "US-".to_string();
        let filtered = state.filtered_services();
        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].short_name(), "web-frontend");
        assert_eq!(filtered[1].short_name(), "worker-queue");

        state.search_query = "europe".to_string();
        let filtered = state.filtered_services();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].short_name(), "api-server");

        // 4. No matches
        state.search_query = "nonexistent".to_string();
        assert_eq!(state.filtered_services().len(), 0);
    }

    #[test]
    fn test_search_key_handling_and_mode_transitions() {
        let mut state = AppState::new();
        state.services = vec![
            Service {
                name: "projects/p/locations/us-central1/services/alpha".to_string(),
                uri: None,
                latest_ready_revision: None,
                conditions: None,
                traffic_statuses: None,
            },
            Service {
                name: "projects/p/locations/us-central1/services/beta".to_string(),
                uri: None,
                latest_ready_revision: None,
                conditions: None,
                traffic_statuses: None,
            },
        ];

        // Activate searching
        state.is_searching = true;

        // Type characters
        state.handle_search_key(KeyCode::Char('a'));
        state.handle_search_key(KeyCode::Char('l'));
        state.handle_search_key(KeyCode::Char('p'));
        assert_eq!(state.search_query, "alp");
        assert!(state.is_searching);
        assert_eq!(state.filtered_services().len(), 1);

        // Backspace deletes char
        state.handle_search_key(KeyCode::Backspace);
        assert_eq!(state.search_query, "al");
        assert!(state.is_searching);

        // Enter keeps query and exits search mode
        state.handle_search_key(KeyCode::Enter);
        assert_eq!(state.search_query, "al");
        assert!(!state.is_searching);

        // Reactivate and Esc clears query and exits search mode
        state.is_searching = true;
        state.handle_search_key(KeyCode::Esc);
        assert_eq!(state.search_query, "");
        assert!(!state.is_searching);
        assert_eq!(state.filtered_services().len(), 2);
    }

    #[test]
    fn test_clamping_and_navigation_on_filtered_services() {
        let mut state = AppState::new();
        state.services = vec![
            Service {
                name: "projects/p/locations/us-central1/services/svc-0".to_string(),
                uri: None,
                latest_ready_revision: None,
                conditions: None,
                traffic_statuses: None,
            },
            Service {
                name: "projects/p/locations/us-central1/services/svc-1".to_string(),
                uri: None,
                latest_ready_revision: None,
                conditions: None,
                traffic_statuses: None,
            },
            Service {
                name: "projects/p/locations/us-central1/services/svc-2".to_string(),
                uri: None,
                latest_ready_revision: None,
                conditions: None,
                traffic_statuses: None,
            },
        ];

        // Select last index (2)
        state.table_state.select(Some(2));

        // Filter down so only 1 service matches (index 0)
        state.search_query = "svc-0".to_string();
        state.clamp_selection();
        // Index 2 shrank past filtered length (1), reset to 0
        assert_eq!(state.table_state.selected(), Some(0));

        // Navigation wraps around filtered list
        state.search_query = "".to_string();
        state.table_state.select(Some(0));
        assert_eq!(state.navigate_down(), Some("svc-1".to_string()));
        assert_eq!(state.table_state.selected(), Some(1));
        assert_eq!(state.navigate_down(), Some("svc-2".to_string()));
        assert_eq!(state.table_state.selected(), Some(2));
        assert_eq!(state.navigate_down(), Some("svc-0".to_string())); // wrap to 0
        assert_eq!(state.table_state.selected(), Some(0));

        assert_eq!(state.navigate_up(), Some("svc-2".to_string())); // wrap to 2
        assert_eq!(state.table_state.selected(), Some(2));
    }

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
        assert!(state.toasts.is_empty());
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

        // When services already exist, error appears in banner and toast instead
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
        assert_eq!(state.toasts.len(), 1);
        assert_eq!(state.toasts[0].kind, ToastKind::Error);
    }

    #[test]
    fn test_app_event_revisions_fetched_success() {
        let mut state = AppState::new();
        let mut modal = TrafficModalState::new("web-service".to_string(), "projects/p/locations/l/services/web-service".to_string());
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
        assert!(modal.revisions[0].is_latest);
        assert_eq!(modal.revisions[1].percent, 0);
        assert!(!modal.revisions[1].is_latest);
        assert_eq!(modal.selected_index, 0);
    }

    #[test]
    fn test_revisions_fetched_populates_zero_percent_traffic_revisions() {
        let mut state = AppState::new();
        let mut modal = TrafficModalState::new("web-service".to_string(), "projects/p/locations/l/services/web-service".to_string());
        modal.revisions.push(RevisionTrafficItem {
            revision_name: "rev-001".to_string(),
            percent: 100,
            tag: "active".to_string(),
            is_latest: false,
        });
        modal.status = TrafficModalStatus::FetchingRevisions;
        state.traffic_modal = Some(modal);

        let revs = vec![
            Revision {
                name: "projects/p/locations/l/services/web-service/revisions/rev-002".to_string(),
                conditions: None,
                create_time: None,
            },
            Revision {
                name: "projects/p/locations/l/services/web-service/revisions/rev-001".to_string(),
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
        // rev-002 is latest fetched, gets 0% traffic since it had no prior split
        assert_eq!(modal.revisions[0].revision_name, "rev-002");
        assert_eq!(modal.revisions[0].percent, 0);
        assert!(modal.revisions[0].is_latest);
        // rev-001 retains its active 100% split and tag
        assert_eq!(modal.revisions[1].revision_name, "rev-001");
        assert_eq!(modal.revisions[1].percent, 100);
        assert_eq!(modal.revisions[1].tag, "active");
        assert_eq!(modal.total_percent(), 100);
    }

    #[test]
    fn test_app_event_revisions_fetched_error_with_no_prior_revisions() {
        let mut state = AppState::new();
        let mut modal = TrafficModalState::new("web-service".to_string(), "projects/p/locations/l/services/web-service".to_string());
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
        let mut modal = TrafficModalState::new("web-service".to_string(), "projects/p/locations/l/services/web-service".to_string());
        modal.revisions.push(RevisionTrafficItem {
            revision_name: "rev-current".to_string(),
            percent: 100,
            tag: "-".to_string(),
            is_latest: true,
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
        assert_eq!(state.toasts.len(), 1);
        assert_eq!(state.toasts[0].kind, ToastKind::Warning);
    }

    #[test]
    fn test_app_event_traffic_split_result_success_and_error() {
        let mut state = AppState::new();
        let mut modal = TrafficModalState::new("api-svc".to_string(), "projects/p/locations/l/services/api-svc".to_string());
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
        assert!(state.toasts.iter().any(|t| t.kind == ToastKind::Success));

        // Error transition
        let mut modal = TrafficModalState::new("api-svc".to_string(), "projects/p/locations/l/services/api-svc".to_string());
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
        assert!(state.toasts.iter().any(|t| t.kind == ToastKind::Error));
    }

    #[tokio::test]
    async fn test_mock_integration_full_user_journey() {
        let server = wiremock::MockServer::start().await;
        let client = Arc::new(
            client::GcpClient::with_base_urls(
                "test-project".to_string(),
                "us-central1".to_string(),
                server.uri(),
                server.uri(),
            )
            .unwrap()
            .with_token("test-mock-token"),
        );

        // 1. Mock list_revisions
        let revs_response = serde_json::json!({
            "revisions": [
                {
                    "name": "projects/test-project/locations/us-central1/services/web/revisions/web-00002",
                    "createTime": "2024-01-02T00:00:00Z"
                },
                {
                    "name": "projects/test-project/locations/us-central1/services/web/revisions/web-00001",
                    "createTime": "2024-01-01T00:00:00Z"
                }
            ]
        });

        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/v2/projects/test-project/locations/us-central1/services/web/revisions"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(revs_response))
            .mount(&server)
            .await;

        // 2. Mock patch traffic split
        wiremock::Mock::given(wiremock::matchers::method("PATCH"))
            .and(wiremock::matchers::path("/v2/projects/test-project/locations/us-central1/services/web"))
            .and(wiremock::matchers::query_param("updateMask", "traffic"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
            .mount(&server)
            .await;

        let mut app_state = AppState::new();
        app_state.services = vec![Service {
            name: "projects/test-project/locations/us-central1/services/web".to_string(),
            uri: Some("https://web.run.app".to_string()),
            latest_ready_revision: Some("web-00002".to_string()),
            conditions: None,
            traffic_statuses: None,
        }];

        // User opens modal ('s')
        let (tx, mut rx) = mpsc::channel(16);
        let modal = open_traffic_modal(&app_state.services, &app_state.table_state, &client, &tx).unwrap();
        app_state.traffic_modal = Some(modal);
        assert_eq!(app_state.traffic_modal.as_ref().unwrap().status, TrafficModalStatus::FetchingRevisions);

        // Background task returns revisions
        let event = rx.recv().await.expect("Expected RevisionsFetched event");
        let action = app_state.handle_event(event);
        assert_eq!(action, AppAction::None);

        let modal = app_state.traffic_modal.as_mut().unwrap();
        assert_eq!(modal.status, TrafficModalStatus::Idle);
        assert_eq!(modal.revisions.len(), 2);
        assert_eq!(modal.revisions[0].revision_name, "web-00002");
        assert!(modal.revisions[0].is_latest);

        // Adjust traffic splits via simulated key presses
        // Rev 0: Clear with 'c' -> 0%
        modal.handle_key(KeyCode::Char('c'));
        assert_eq!(modal.revisions[0].percent, 0);

        // Enter 80% on rev 0: type '8', '0'
        modal.handle_key(KeyCode::Char('8'));
        modal.handle_key(KeyCode::Char('0'));
        assert_eq!(modal.revisions[0].percent, 80);

        // Assign tag 'candidate' on rev 0: press 't', type 'c','a','n','d','i','d','a','t','e', Enter
        modal.handle_key(KeyCode::Char('t'));
        assert!(modal.editing_tag);
        for ch in "candidate".chars() {
            modal.handle_key(KeyCode::Char(ch));
        }
        modal.handle_key(KeyCode::Enter);
        assert!(!modal.editing_tag);
        assert_eq!(modal.revisions[0].tag, "candidate");

        // Move to rev 1: 'j'
        modal.handle_key(KeyCode::Char('j'));
        assert_eq!(modal.selected_index, 1);

        // Enter 20% on rev 1: type '2', '0'
        modal.handle_key(KeyCode::Char('2'));
        modal.handle_key(KeyCode::Char('0'));
        assert_eq!(modal.revisions[1].percent, 20);

        assert_eq!(modal.total_percent(), 100);

        // Submitting modal
        // 1st Enter -> Confirming
        let action = modal.handle_key(KeyCode::Enter);
        assert_eq!(action, TrafficModalAction::None);
        assert_eq!(modal.status, TrafficModalStatus::Confirming);

        // 2nd Enter -> Submit action
        let action = modal.handle_key(KeyCode::Enter);
        let splits = match action {
            TrafficModalAction::Submit(s) => s,
            other => panic!("Expected Submit, got {:?}", other),
        };
        assert_eq!(splits.len(), 2);
        assert_eq!(splits[0].revision, "web-00002");
        assert_eq!(splits[0].percent, 80);
        assert_eq!(splits[0].tag, Some("candidate".to_string()));
        assert_eq!(splits[1].revision, "web-00001");
        assert_eq!(splits[1].percent, 20);
        assert_eq!(splits[1].tag, None);

        // Execute submission against mock client
        spawn_set_traffic_split(Arc::clone(&client), tx.clone(), "web".to_string(), "projects/test-project/locations/us-central1/services/web".to_string(), splits);

        let result_event = rx.recv().await.expect("Expected TrafficSplitResult");
        let app_action = app_state.handle_event(result_event);
        assert_eq!(app_action, AppAction::FetchServices);

        let modal = app_state.traffic_modal.as_ref().unwrap();
        match &modal.status {
            TrafficModalStatus::Success(msg) => {
                assert!(msg.contains("Traffic split updated successfully"));
            }
            other => panic!("Expected Success status, got {:?}", other),
        }
        assert!(app_state.toasts.iter().any(|t| t.kind == ToastKind::Success));
    }

    #[tokio::test]
    async fn test_mock_integration_aggregate_region_fetches_revisions_with_concrete_location() {
        let server = wiremock::MockServer::start().await;
        // Client configured with aggregate region "-"
        let client = Arc::new(
            client::GcpClient::with_base_urls(
                "test-project".to_string(),
                "-".to_string(),
                server.uri(),
                server.uri(),
            )
            .unwrap()
            .with_token("test-mock-token"),
        );

        // Mock list_revisions on the concrete location europe-west1
        let revs_response = serde_json::json!({
            "revisions": [
                {
                    "name": "projects/test-project/locations/europe-west1/services/api/revisions/api-00002",
                    "createTime": "2024-01-02T00:00:00Z"
                },
                {
                    "name": "projects/test-project/locations/europe-west1/services/api/revisions/api-00001",
                    "createTime": "2024-01-01T00:00:00Z"
                }
            ]
        });

        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/v2/projects/test-project/locations/europe-west1/services/api/revisions"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(revs_response))
            .mount(&server)
            .await;

        let mut app_state = AppState::new();
        app_state.services = vec![Service {
            name: "projects/test-project/locations/europe-west1/services/api".to_string(),
            uri: Some("https://api.run.app".to_string()),
            latest_ready_revision: Some("api-00002".to_string()),
            conditions: None,
            traffic_statuses: None,
        }];

        let (tx, mut rx) = mpsc::channel(16);
        let modal = open_traffic_modal(&app_state.services, &app_state.table_state, &client, &tx).unwrap();
        app_state.traffic_modal = Some(modal);
        assert_eq!(app_state.traffic_modal.as_ref().unwrap().status, TrafficModalStatus::FetchingRevisions);

        let event = rx.recv().await.expect("Expected RevisionsFetched event");
        let action = app_state.handle_event(event);
        assert_eq!(action, AppAction::None);

        let modal = app_state.traffic_modal.as_ref().unwrap();
        assert_eq!(modal.status, TrafficModalStatus::Idle);
        assert_eq!(modal.revisions.len(), 2);
        assert_eq!(modal.revisions[0].revision_name, "api-00002");
        assert_eq!(modal.revisions[1].revision_name, "api-00001");
    }

    #[tokio::test]
    async fn test_mock_integration_aggregate_region_malformed_service_name_emits_error() {
        let server = wiremock::MockServer::start().await;
        let client = Arc::new(
            client::GcpClient::with_base_urls(
                "test-project".to_string(),
                "-".to_string(),
                server.uri(),
                server.uri(),
            )
            .unwrap()
            .with_token("test-mock-token"),
        );

        let mut app_state = AppState::new();
        app_state.services = vec![Service {
            name: "malformed-name".to_string(),
            uri: None,
            latest_ready_revision: None,
            conditions: None,
            traffic_statuses: None,
        }];

        let (tx, mut rx) = mpsc::channel(16);
        let modal = open_traffic_modal(&app_state.services, &app_state.table_state, &client, &tx).unwrap();
        app_state.traffic_modal = Some(modal);

        let event = rx.recv().await.expect("Expected RevisionsFetched event");
        let action = app_state.handle_event(event);
        assert_eq!(action, AppAction::None);

        let modal = app_state.traffic_modal.as_ref().unwrap();
        match &modal.status {
            TrafficModalStatus::Error(err) => {
                assert!(err.contains("concrete location is required"));
            }
            other => panic!("Expected Error status, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_mock_submission_failure_403_permission_denied() {
        let server = wiremock::MockServer::start().await;
        let client = Arc::new(
            client::GcpClient::with_base_urls(
                "test-project".to_string(),
                "us-central1".to_string(),
                server.uri(),
                server.uri(),
            )
            .unwrap()
            .with_token("test-mock-token"),
        );

        wiremock::Mock::given(wiremock::matchers::method("PATCH"))
            .and(wiremock::matchers::path("/v2/projects/test-project/locations/us-central1/services/web"))
            .respond_with(wiremock::ResponseTemplate::new(403).set_body_json(serde_json::json!({
                "error": {
                    "code": 403,
                    "message": "The caller does not have permission",
                    "status": "PERMISSION_DENIED"
                }
            })))
            .mount(&server)
            .await;

        let mut app_state = AppState::new();
        let mut modal = TrafficModalState::new("web".to_string(), "projects/test-project/locations/us-central1/services/web".to_string());
        modal.status = TrafficModalStatus::Submitting;
        app_state.traffic_modal = Some(modal);

        let (tx, mut rx) = mpsc::channel(16);
        let splits = vec![TrafficSplitTarget::new("web-001", 100, None)];
        spawn_set_traffic_split(client, tx, "web".to_string(), "projects/test-project/locations/us-central1/services/web".to_string(), splits);

        let event = rx.recv().await.expect("Expected TrafficSplitResult");
        let action = app_state.handle_event(event);
        assert_eq!(action, AppAction::None);

        let modal = app_state.traffic_modal.as_ref().unwrap();
        match &modal.status {
            TrafficModalStatus::Error(err) => {
                assert!(err.contains("Permission Denied"));
                assert!(err.contains("Cloud Run Developer"));
            }
            other => panic!("Expected Error status, got {:?}", other),
        }
        assert!(app_state.toasts.iter().any(|t| t.kind == ToastKind::Error));
    }

    #[tokio::test]
    async fn test_mock_submission_failure_409_conflict() {
        let server = wiremock::MockServer::start().await;
        let client = Arc::new(
            client::GcpClient::with_base_urls(
                "test-project".to_string(),
                "us-central1".to_string(),
                server.uri(),
                server.uri(),
            )
            .unwrap()
            .with_token("test-mock-token"),
        );

        wiremock::Mock::given(wiremock::matchers::method("PATCH"))
            .and(wiremock::matchers::path("/v2/projects/test-project/locations/us-central1/services/web"))
            .respond_with(wiremock::ResponseTemplate::new(409).set_body_string("Resource version conflict"))
            .mount(&server)
            .await;

        let mut app_state = AppState::new();
        let mut modal = TrafficModalState::new("web".to_string(), "projects/test-project/locations/us-central1/services/web".to_string());
        modal.status = TrafficModalStatus::Submitting;
        app_state.traffic_modal = Some(modal);

        let (tx, mut rx) = mpsc::channel(16);
        let splits = vec![TrafficSplitTarget::new("web-001", 100, None)];
        spawn_set_traffic_split(client, tx, "web".to_string(), "projects/test-project/locations/us-central1/services/web".to_string(), splits);

        let event = rx.recv().await.expect("Expected TrafficSplitResult");
        let action = app_state.handle_event(event);
        assert_eq!(action, AppAction::None);

        let modal = app_state.traffic_modal.as_ref().unwrap();
        match &modal.status {
            TrafficModalStatus::Error(err) => {
                assert!(err.contains("Conflict detected"));
                assert!(err.contains("concurrently"));
            }
            other => panic!("Expected Error status, got {:?}", other),
        }
        assert!(app_state.toasts.iter().any(|t| t.kind == ToastKind::Error));
    }

    #[tokio::test]
    async fn test_mock_submission_failure_429_rate_limited() {
        let server = wiremock::MockServer::start().await;
        let client = Arc::new(
            client::GcpClient::with_base_urls(
                "test-project".to_string(),
                "us-central1".to_string(),
                server.uri(),
                server.uri(),
            )
            .unwrap()
            .with_token("test-mock-token"),
        );

        wiremock::Mock::given(wiremock::matchers::method("PATCH"))
            .and(wiremock::matchers::path("/v2/projects/test-project/locations/us-central1/services/web"))
            .respond_with(wiremock::ResponseTemplate::new(429).set_body_string("Rate limit exceeded"))
            .mount(&server)
            .await;

        let mut app_state = AppState::new();
        let mut modal = TrafficModalState::new("web".to_string(), "projects/test-project/locations/us-central1/services/web".to_string());
        modal.status = TrafficModalStatus::Submitting;
        app_state.traffic_modal = Some(modal);

        let (tx, mut rx) = mpsc::channel(16);
        let splits = vec![TrafficSplitTarget::new("web-001", 100, None)];
        spawn_set_traffic_split(client, tx, "web".to_string(), "projects/test-project/locations/us-central1/services/web".to_string(), splits);

        let event = rx.recv().await.expect("Expected TrafficSplitResult");
        let action = app_state.handle_event(event);
        assert_eq!(action, AppAction::None);

        let modal = app_state.traffic_modal.as_ref().unwrap();
        match &modal.status {
            TrafficModalStatus::Error(err) => {
                assert!(err.contains("rate limit exceeded"));
            }
            other => panic!("Expected Error status, got {:?}", other),
        }
        assert!(app_state.toasts.iter().any(|t| t.kind == ToastKind::Error));
    }

    #[tokio::test]
    async fn test_mock_integration_aggregate_region_traffic_split_with_concrete_location() {
        let server = wiremock::MockServer::start().await;
        let client = Arc::new(
            client::GcpClient::with_base_urls(
                "test-project".to_string(),
                "-".to_string(),
                server.uri(),
                server.uri(),
            )
            .unwrap()
            .with_token("test-mock-token"),
        );

        // Mock list_revisions to the concrete location europe-west1
        let revs_response = serde_json::json!({
            "revisions": [
                {
                    "name": "projects/test-project/locations/europe-west1/services/api/revisions/api-00002",
                    "createTime": "2024-01-02T00:00:00Z"
                }
            ]
        });
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/v2/projects/test-project/locations/europe-west1/services/api/revisions"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(revs_response))
            .mount(&server)
            .await;

        // Mock PATCH to the concrete location europe-west1
        wiremock::Mock::given(wiremock::matchers::method("PATCH"))
            .and(wiremock::matchers::path("/v2/projects/test-project/locations/europe-west1/services/api"))
            .and(wiremock::matchers::query_param("updateMask", "traffic"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "name": "projects/test-project/locations/europe-west1/services/api"
            })))
            .mount(&server)
            .await;

        let mut app_state = AppState::new();
        app_state.services = vec![Service {
            name: "projects/test-project/locations/europe-west1/services/api".to_string(),
            uri: Some("https://api.run.app".to_string()),
            latest_ready_revision: Some("api-00002".to_string()),
            conditions: None,
            traffic_statuses: None,
        }];

        let (tx, mut rx) = mpsc::channel(16);
        let modal = open_traffic_modal(&app_state.services, &app_state.table_state, &client, &tx).unwrap();
        assert_eq!(modal.service_name, "api");
        assert_eq!(modal.full_service_name, "projects/test-project/locations/europe-west1/services/api");

        app_state.traffic_modal = Some(modal);

        // Receive background fetch revisions event
        let rev_event = rx.recv().await.expect("Expected RevisionsFetched event");
        let rev_action = app_state.handle_event(rev_event);
        assert_eq!(rev_action, AppAction::None);

        let splits = vec![TrafficSplitTarget::new("api-00002", 100, None)];
        let modal_ref = app_state.traffic_modal.as_ref().unwrap();
        spawn_set_traffic_split(
            Arc::clone(&client),
            tx.clone(),
            modal_ref.service_name.clone(),
            modal_ref.full_service_name.clone(),
            splits,
        );

        let event = rx.recv().await.expect("Expected TrafficSplitResult");
        match &event {
            AppEvent::TrafficSplitResult { service_name, result } => {
                assert_eq!(service_name, "api");
                assert!(result.is_ok());
            }
            other => panic!("Expected TrafficSplitResult, got {:?}", other),
        }

        let action = app_state.handle_event(event);
        assert_eq!(action, AppAction::FetchServices);
        let modal = app_state.traffic_modal.as_ref().unwrap();
        match &modal.status {
            TrafficModalStatus::Success(msg) => {
                assert!(msg.contains("Traffic split updated successfully"));
            }
            other => panic!("Expected Success status, got {:?}", other),
        }
    }
}
