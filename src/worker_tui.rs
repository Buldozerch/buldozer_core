//! Ready-to-use worker TUI preset.
//!
//! This preset is meant to keep projects small:
//! - provides Main + DB Actions menus
//! - handles DB init (with optional SQLCipher password prompt)
//! - runs your project logic via a provided async function

use crate::tui_logger::LogLine;
use crate::tui_shell::{MenuApp, MenuView, ShellContext, ShellParams, TaskOutcome};
use crate::wallet_db::{WalletDb, WalletDbConfig};
use ratatui::prelude::*;
use tokio::sync::mpsc::UnboundedReceiver;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Main,
    DbActions,
}

#[derive(Debug, Clone)]
pub struct WorkerHeader {
    /// Line shown as an italic green tagline (spinner will be appended).
    pub tagline: String,
    /// Extra lines shown in blue.
    pub extra_lines: Vec<String>,
}

impl Default for WorkerHeader {
    fn default() -> Self {
        Self {
            tagline: "Grind Boys".to_string(),
            extra_lines: vec![
                "Soft for private community".to_string(),
                "Thanks for your trust".to_string(),
            ],
        }
    }
}

pub struct WorkerTuiParams {
    pub title: &'static str,
    pub check_git_updates: bool,
    pub db_url: String,
    pub db_encryption: bool,
    pub db_max_connections: u32,
    pub wallet_db_config: WalletDbConfig,
    pub header: WorkerHeader,
    pub actions: Vec<String>,
}

impl WorkerTuiParams {
    /// Creates params with reasonable defaults (10 DB connections).
    pub fn new(
        title: &'static str,
        check_git_updates: bool,
        db_url: String,
        db_encryption: bool,
        wallet_db_config: WalletDbConfig,
    ) -> Self {
        Self {
            title,
            check_git_updates,
            db_url,
            db_encryption,
            db_max_connections: 10,
            wallet_db_config,
            header: WorkerHeader::default(),
            actions: Vec::new(),
        }
    }

    /// Overrides header text.
    pub fn with_header(mut self, header: WorkerHeader) -> Self {
        self.header = header;
        self
    }

    /// Sets the list of project actions shown in the main menu.
    pub fn with_actions(mut self, actions: Vec<String>) -> Self {
        self.actions = actions;
        self
    }
}

/// Starts the preset worker TUI.
///
/// `run_fn` is called when the user selects the "Run" menu item.
/// The `WalletDb` instance is initialized lazily and cached in the app state.
pub async fn start_worker_tui<RunFn, RunFut>(
    params: WorkerTuiParams,
    log_rx: UnboundedReceiver<LogLine>,
    run_fn: RunFn,
) -> Result<(), Box<dyn std::error::Error>>
where
    RunFn: Fn(usize, WalletDb) -> RunFut + Send + Sync + 'static,
    RunFut: std::future::Future<Output = Result<(), String>> + Send + 'static,
{
    let state = WorkerState {
        screen: Screen::Main,
        main_selected: 0,
        db_selected: 0,
        db: None,
        params,
        run_fn: std::sync::Arc::new(run_fn),
    };

    let shell_params = ShellParams {
        title: state.params.title,
        check_git_updates: state.params.check_git_updates,
    };

    crate::tui_shell::start(shell_params, state, log_rx, WorkerApp).await
}

struct WorkerState<RunFn> {
    screen: Screen,
    main_selected: usize,
    db_selected: usize,

    db: Option<WalletDb>,
    params: WorkerTuiParams,
    run_fn: std::sync::Arc<RunFn>,
}

struct WorkerApp;

impl<RunFn, RunFut> MenuApp<WorkerState<RunFn>> for WorkerApp
where
    RunFn: Fn(usize, WalletDb) -> RunFut + Send + Sync + 'static,
    RunFut: std::future::Future<Output = Result<(), String>> + Send + 'static,
{
    fn header_lines(&self, state: &WorkerState<RunFn>, tick: u64) -> Vec<Line<'static>> {
        let spinner = match tick % 4 {
            0 => "|",
            1 => "/",
            2 => "-",
            _ => "\\",
        };

        let tagline = format!("{} {spinner}", state.params.header.tagline);

        let mut out = Vec::with_capacity(4 + state.params.header.extra_lines.len());
        out.push(Line::from(Span::styled(
            "----------------------------------------",
            Style::default().fg(Color::LightGreen),
        )));
        out.push(Line::from(Span::styled(
            state.params.title,
            Style::default()
                .fg(Color::LightGreen)
                .add_modifier(Modifier::BOLD),
        )));
        out.push(Line::from(Span::styled(
            tagline,
            Style::default().fg(Color::Green).add_modifier(Modifier::ITALIC),
        )));
        out.push(Line::from(""));

        for l in &state.params.header.extra_lines {
            out.push(Line::from(Span::styled(
                l.clone(),
                Style::default().fg(Color::Blue),
            )));
        }

        out.push(Line::from(Span::styled(
            "----------------------------------------",
            Style::default().fg(Color::LightGreen),
        )));
        out
    }

    fn menu_view(
        &self,
        state: &WorkerState<RunFn>,
        _running_label: Option<&str>,
    ) -> MenuView {
        match state.screen {
            Screen::Main => MenuView {
                title: "Menu".to_string(),
                items: {
                    let mut items = Vec::with_capacity(2 + state.params.actions.len());
                    items.push(Line::from("DB Actions"));
                    for a in &state.params.actions {
                        items.push(Line::from(a.clone()));
                    }
                    items.push(Line::from("Exit"));
                    items
                },
                selected: state.main_selected,
            },
            Screen::DbActions => MenuView {
                title: "DB Actions".to_string(),
                items: vec![
                    Line::from("Import Wallets to DB"),
                    Line::from("Sync Wallets in DB"),
                    Line::from("Back"),
                ],
                selected: state.db_selected,
            },
        }
    }

    fn menu_selected_mut<'a>(&mut self, state: &'a mut WorkerState<RunFn>) -> &'a mut usize {
        let screen = state.screen;
        match screen {
            Screen::Main => &mut state.main_selected,
            Screen::DbActions => &mut state.db_selected,
        }
    }

    fn on_enter(&mut self, ctx: &mut ShellContext<'_, WorkerState<RunFn>>) {
        let (screen, main_selected, db_selected) = {
            let st = ctx.state_ref();
            (st.screen, st.main_selected, st.db_selected)
        };

        match screen {
            Screen::Main => match main_selected {
                0 => ctx.state().screen = Screen::DbActions,
                x => {
                    let actions_len = ctx.state_ref().params.actions.len();
                    // Actions are [1..=actions_len], Exit is actions_len + 1.
                    if x >= 1 && x <= actions_len {
                        spawn_run_idx(ctx, x - 1);
                    } else {
                        ctx.quit();
                    }
                }
            },
            Screen::DbActions => match db_selected {
                0 => spawn_db_import(ctx),
                1 => spawn_db_sync(ctx),
                _ => ctx.state().screen = Screen::Main,
            },
        }
    }

    fn on_esc(&mut self, ctx: &mut ShellContext<'_, WorkerState<RunFn>>) {
        if ctx.state_ref().screen == Screen::DbActions {
            ctx.state().screen = Screen::Main;
        }
    }
}

fn spawn_db_import<RunFn, RunFut>(ctx: &mut ShellContext<'_, WorkerState<RunFn>>)
where
    RunFn: Fn(usize, WalletDb) -> RunFut + Send + Sync + 'static,
    RunFut: std::future::Future<Output = Result<(), String>> + Send + 'static,
{
    if let Some(db) = ctx.state_ref().db.clone() {
        ctx.spawn_task("DB Import", async move {
            db.import_from_files().await.map_err(|e| e.to_string())?;
            Ok(TaskOutcome::Done)
        });
        return;
    }

    let url = ctx.state_ref().params.db_url.clone();
    let cfg = ctx.state_ref().params.wallet_db_config.clone();
    let max = ctx.state_ref().params.db_max_connections;

    if ctx.state_ref().params.db_encryption {
        ctx.prompt_secret(
            "DB Import",
            "DB Password",
            "Import Wallets to DB".to_string(),
            move |pw| {
                Box::pin(async move {
                    let db = WalletDb::init(&url, Some(&pw), max, cfg)
                        .await
                        .map_err(|e| e.to_string())?;
                    db.import_from_files().await.map_err(|e| e.to_string())?;
                    Ok(TaskOutcome::UpdateState(Box::new(move |st: &mut WorkerState<RunFn>| {
                        st.db = Some(db);
                    })))
                })
            },
        );
        return;
    }

    ctx.spawn_task("DB Import", async move {
        let db = WalletDb::init(&url, None, max, cfg)
            .await
            .map_err(|e| e.to_string())?;
        db.import_from_files().await.map_err(|e| e.to_string())?;
        Ok(TaskOutcome::UpdateState(Box::new(move |st: &mut WorkerState<RunFn>| {
            st.db = Some(db);
        })))
    });
}

fn spawn_db_sync<RunFn, RunFut>(ctx: &mut ShellContext<'_, WorkerState<RunFn>>)
where
    RunFn: Fn(usize, WalletDb) -> RunFut + Send + Sync + 'static,
    RunFut: std::future::Future<Output = Result<(), String>> + Send + 'static,
{
    if let Some(db) = ctx.state_ref().db.clone() {
        ctx.spawn_task("DB Sync", async move {
            db.sync_from_files().await.map_err(|e| e.to_string())?;
            Ok(TaskOutcome::Done)
        });
        return;
    }

    let url = ctx.state_ref().params.db_url.clone();
    let cfg = ctx.state_ref().params.wallet_db_config.clone();
    let max = ctx.state_ref().params.db_max_connections;

    if ctx.state_ref().params.db_encryption {
        ctx.prompt_secret(
            "DB Sync",
            "DB Password",
            "Sync Wallets in DB".to_string(),
            move |pw| {
                Box::pin(async move {
                    let db = WalletDb::init(&url, Some(&pw), max, cfg)
                        .await
                        .map_err(|e| e.to_string())?;
                    db.sync_from_files().await.map_err(|e| e.to_string())?;
                    Ok(TaskOutcome::UpdateState(Box::new(move |st: &mut WorkerState<RunFn>| {
                        st.db = Some(db);
                    })))
                })
            },
        );
        return;
    }

    ctx.spawn_task("DB Sync", async move {
        let db = WalletDb::init(&url, None, max, cfg)
            .await
            .map_err(|e| e.to_string())?;
        db.sync_from_files().await.map_err(|e| e.to_string())?;
        Ok(TaskOutcome::UpdateState(Box::new(move |st: &mut WorkerState<RunFn>| {
            st.db = Some(db);
        })))
    });
}

fn spawn_run_idx<RunFn, RunFut>(ctx: &mut ShellContext<'_, WorkerState<RunFn>>, action_idx: usize)
where
    RunFn: Fn(usize, WalletDb) -> RunFut + Send + Sync + 'static,
    RunFut: std::future::Future<Output = Result<(), String>> + Send + 'static,
{
    let action_name = ctx
        .state_ref()
        .params
        .actions
        .get(action_idx)
        .cloned()
        .unwrap_or_else(|| format!("Action {}", action_idx + 1));

    if let Some(db) = ctx.state_ref().db.clone() {
        let run_fn = std::sync::Arc::clone(&ctx.state_ref().run_fn);
        ctx.spawn_task(action_name, async move {
            (*run_fn)(action_idx, db).await?;
            Ok(TaskOutcome::Done)
        });
        return;
    }

    let url = ctx.state_ref().params.db_url.clone();
    let cfg = ctx.state_ref().params.wallet_db_config.clone();
    let max = ctx.state_ref().params.db_max_connections;
    let run_fn = std::sync::Arc::clone(&ctx.state_ref().run_fn);

    if ctx.state_ref().params.db_encryption {
        ctx.prompt_secret(action_name, "DB Password", "Run".to_string(), move |pw| {
            Box::pin(async move {
                let db = WalletDb::init(&url, Some(&pw), max, cfg)
                    .await
                    .map_err(|e| e.to_string())?;
                (*run_fn)(action_idx, db.clone()).await?;
                Ok(TaskOutcome::UpdateState(Box::new(move |st: &mut WorkerState<RunFn>| {
                    st.db = Some(db);
                })))
            })
        });
        return;
    }

    ctx.spawn_task(action_name, async move {
        let db = WalletDb::init(&url, None, max, cfg)
            .await
            .map_err(|e| e.to_string())?;
        (*run_fn)(action_idx, db.clone()).await?;
        Ok(TaskOutcome::UpdateState(Box::new(move |st: &mut WorkerState<RunFn>| {
            st.db = Some(db);
        })))
    });
}
