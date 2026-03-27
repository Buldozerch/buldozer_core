use std::collections::VecDeque;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::prelude::*;
use ratatui::widgets::*;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::tui_logger::LogLine;
use crate::update::{self, UpdateInfo};

pub enum TaskOutcome<S> {
    Done,
    UpdateState(Box<dyn FnOnce(&mut S) + Send>),
    Quit,
    Restart,
}

pub type ShellFuture<S> = Pin<Box<dyn Future<Output = Result<TaskOutcome<S>, String>> + Send>>;

struct RunningTask<S> {
    label: &'static str,
    handle: tokio::task::JoinHandle<Result<TaskOutcome<S>, String>>,
}

struct ShellState<S> {
    // External state
    state: S,

    // Running async task
    running: Option<RunningTask<S>>,

    // Lifecycle
    quit: bool,
    restart: bool,

    // Update modal
    update_prompt: Option<UpdateInfo>,

    // Secret prompt modal.
    secret_prompt: Option<SecretPrompt<S>>,

    // UI
    header_tick: u64,
    logs: VecDeque<LogLine>,
    log_scroll: u16,
    follow_bottom: bool,
}

struct SecretPrompt<S> {
    title: &'static str,
    action: String,
    buf: String,
    label: &'static str,
    build: Option<Box<dyn FnOnce(String) -> ShellFuture<S> + Send>>,
}

pub struct ShellContext<'a, S> {
    inner: &'a mut ShellState<S>,
}

impl<'a, S> ShellContext<'a, S> {
    pub fn state(&mut self) -> &mut S {
        &mut self.inner.state
    }

    pub fn state_ref(&self) -> &S {
        &self.inner.state
    }

    pub fn running_label(&self) -> Option<&'static str> {
        self.inner.running.as_ref().map(|r| r.label)
    }

    pub fn quit(&mut self) {
        self.inner.quit = true;
    }

    pub fn restart(&mut self) {
        self.inner.restart = true;
        self.inner.quit = true;
    }

    pub fn spawn_task<Fut>(&mut self, label: &'static str, fut: Fut)
    where
        Fut: std::future::Future<Output = Result<TaskOutcome<S>, String>> + Send + 'static,
        S: Send + 'static,
    {
        if self.inner.running.is_some() {
            return;
        }
        log::info!("{}: start", label);
        let handle = tokio::spawn(fut);
        self.inner.running = Some(RunningTask { label, handle });
    }

    pub fn prompt_secret<F>(&mut self, label: &'static str, title: &'static str, action: String, f: F)
    where
        F: FnOnce(String) -> ShellFuture<S> + Send + 'static,
        S: Send + 'static,
    {
        if self.inner.secret_prompt.is_some() {
            return;
        }
        self.inner.secret_prompt = Some(SecretPrompt {
            title,
            action,
            buf: String::new(),
            label,
            build: Some(Box::new(f)),
        });
    }
}

pub struct ShellParams {
    pub title: &'static str,
    pub check_git_updates: bool,
}

pub struct MenuView {
    pub title: String,
    pub items: Vec<Line<'static>>,
    pub selected: usize,
}

pub trait MenuApp<S>: Send {
    fn header_lines(&self, state: &S, tick: u64) -> Vec<Line<'static>>;

    fn menu_view(&self, state: &S, running_label: Option<&'static str>) -> MenuView;

    fn menu_selected_mut<'a>(&mut self, state: &'a mut S) -> &'a mut usize;

    // If returns true, shell will not run default menu navigation.
    fn on_key(&mut self, _code: KeyCode, _ctx: &mut ShellContext<'_, S>) -> bool {
        false
    }

    fn on_enter(&mut self, _ctx: &mut ShellContext<'_, S>) {}

    fn on_esc(&mut self, _ctx: &mut ShellContext<'_, S>) {}

    fn render_overlays(&self, _f: &mut Frame<'_>, _area: Rect, _state: &S) {}
}

pub async fn start<S, A>(
    params: ShellParams,
    state: S,
    mut log_rx: UnboundedReceiver<LogLine>,
    mut app: A,
) -> Result<(), Box<dyn std::error::Error>>
where
    S: Send + 'static,
    A: MenuApp<S> + 'static,
{
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut shell = ShellState {
        state,
        running: None,
        quit: false,
        restart: false,
        update_prompt: None,
        secret_prompt: None,
        header_tick: 0,
        logs: VecDeque::with_capacity(2048),
        log_scroll: 0,
        follow_bottom: true,
    };

    if params.check_git_updates {
        match update::check_update() {
            Ok(Some(info)) => {
                if info.ff_possible {
                    log::warn!(
                        "Update available: {} -> {} (behind={}, ahead={})",
                        info.local_hash,
                        info.remote_hash,
                        info.behind,
                        info.ahead
                    );
                    shell.update_prompt = Some(info);
                } else {
                    log::warn!(
                        "Repo diverged (ahead={}, behind={}); ff-only update is not possible",
                        info.ahead,
                        info.behind
                    );
                }
            }
            Ok(None) => log::info!("OK up to date"),
            Err(e) => log::debug!("update check failed: {e}"),
        }
    }

    let res = run_loop(&mut terminal, &mut shell, &mut log_rx, &mut app, &params).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    if shell.restart {
        update::restart_self()?;
        std::process::exit(0);
    }

    res
}

async fn run_loop<S, A>(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    shell: &mut ShellState<S>,
    log_rx: &mut UnboundedReceiver<LogLine>,
    app: &mut A,
    params: &ShellParams,
) -> Result<(), Box<dyn std::error::Error>>
where
    S: Send + 'static,
    A: MenuApp<S>,
{
    loop {
        if shell.quit {
            return Ok(());
        }

        shell.header_tick = shell.header_tick.wrapping_add(1);

        while let Ok(line) = log_rx.try_recv() {
            push_log(shell, line);
        }

        if let Some(running) = shell.running.as_mut()
            && running.handle.is_finished()
        {
            let task = shell.running.take().unwrap();
            match task.handle.await {
                Ok(Ok(TaskOutcome::Done)) => log::info!("DONE {}", task.label),
                Ok(Ok(TaskOutcome::UpdateState(f))) => {
                    f(&mut shell.state);
                    log::info!("DONE {}", task.label);
                }
                Ok(Ok(TaskOutcome::Quit)) => {
                    log::info!("DONE {}", task.label);
                    shell.quit = true;
                }
                Ok(Ok(TaskOutcome::Restart)) => {
                    log::info!("DONE {}", task.label);
                    shell.restart = true;
                    shell.quit = true;
                }
                Ok(Err(e)) => log::error!("{}: {e}", task.label),
                Err(e) => log::error!("{}: join error: {e}", task.label),
            }
        }

        terminal.draw(|f| ui(f, shell, app, params))?;

        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        let ev = event::read()?;
        let Event::Key(key) = ev else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        // Global keys.
        match key.code {
            KeyCode::Char('q') => return Ok(()),
            KeyCode::Char('c') => {
                if let Some(task) = shell.running.take() {
                    task.handle.abort();
                    log::warn!("{}: cancelled", task.label);
                }
            }
            KeyCode::PageUp => {
                shell.follow_bottom = false;
                shell.log_scroll = shell.log_scroll.saturating_sub(5);
            }
            KeyCode::PageDown => {
                shell.log_scroll = shell.log_scroll.saturating_add(5);
            }
            KeyCode::End => {
                shell.follow_bottom = true;
            }
            _ => {}
        }

        if shell.running.is_some() {
            continue;
        }

        // Update modal has priority.
        if shell.update_prompt.is_some() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    let mut ctx = ShellContext { inner: shell };
                    ctx.spawn_task("Update", async move {
                        update::pull_ff_only().map_err(|e| e.to_string())?;
                        Ok(TaskOutcome::Restart)
                    });
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    log::warn!("update skipped");
                    shell.update_prompt = None;
                }
                _ => {}
            }
            continue;
        }

        // Secret modal has priority.
        if shell.secret_prompt.is_some() {
            handle_secret_prompt(shell, key.code);
            continue;
        }

        let mut ctx = ShellContext { inner: shell };

        // App-specific key handling (modals, etc.).
        if app.on_key(key.code, &mut ctx) {
            continue;
        }

        let view = app.menu_view(ctx.state_ref(), ctx.running_label());
        let items = view.items.len();

        match key.code {
            KeyCode::Up => {
                if items > 0 {
                    let s = app.menu_selected_mut(ctx.state());
                    *s = s.saturating_sub(1);
                }
            }
            KeyCode::Down => {
                if items > 0 {
                    let s = app.menu_selected_mut(ctx.state());
                    *s = (*s + 1).min(items.saturating_sub(1));
                }
            }
            KeyCode::Enter => app.on_enter(&mut ctx),
            KeyCode::Esc => app.on_esc(&mut ctx),
            _ => {}
        }
    }
}

fn push_log<S>(shell: &mut ShellState<S>, line: LogLine) {
    if shell.logs.len() == 2000 {
        shell.logs.pop_front();
    }
    shell.logs.push_back(line);
}

fn ui<S, A>(f: &mut Frame<'_>, shell: &mut ShellState<S>, app: &A, params: &ShellParams)
where
    A: MenuApp<S>,
{
    let size = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(9),
            Constraint::Min(10),
            Constraint::Length(7),
        ])
        .split(size);

    render_header(f, chunks[0], shell, app, params.title);
    render_logs(f, chunks[1], shell);
    render_menu(f, chunks[2], shell, app);

    if let Some(info) = &shell.update_prompt {
        render_update_prompt(f, info);
    }

    if let Some(p) = shell.secret_prompt.as_ref() {
        render_secret_prompt(f, p);
    }

    app.render_overlays(f, size, &shell.state);
}

fn handle_secret_prompt<S>(shell: &mut ShellState<S>, code: KeyCode)
where
    S: Send + 'static,
{
    let Some(p) = shell.secret_prompt.as_mut() else {
        return;
    };

    match code {
        KeyCode::Esc => {
            shell.secret_prompt = None;
        }
        KeyCode::Backspace => {
            p.buf.pop();
        }
        KeyCode::Enter => {
            let secret = p.buf.trim().to_string();
            if secret.is_empty() {
                log::error!("empty secret");
                return;
            }

            let label = p.label;
            let build = p.build.take();
            shell.secret_prompt = None;
            let Some(build) = build else {
                return;
            };

            log::info!("{}: start", label);
            let handle = tokio::spawn(build(secret));
            shell.running = Some(RunningTask { label, handle });
        }
        KeyCode::Char(c) => {
            if !c.is_control() {
                p.buf.push(c);
            }
        }
        _ => {}
    }
}

fn render_secret_prompt<S>(f: &mut Frame<'_>, p: &SecretPrompt<S>) {
    let area = centered_rect(70, 30, f.area());
    let block = Block::default()
        .title(Span::styled(
            p.title,
            Style::default().fg(Color::LightGreen).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::LightGreen));

    let masked = "*".repeat(p.buf.chars().count());
    let body = vec![
        Line::from(format!("Action: {}", p.action)),
        Line::from(""),
        Line::from("Enter secret (hidden):"),
        Line::from(Span::styled(masked, Style::default().fg(Color::Yellow))),
        Line::from(""),
        Line::from(Span::styled(
            "Enter to confirm, Esc to cancel",
            Style::default().fg(Color::Gray),
        )),
    ];

    let p = Paragraph::new(Text::from(body))
        .block(block)
        .wrap(Wrap { trim: true });
    f.render_widget(Clear, area);
    f.render_widget(p, area);
}

fn render_header<S, A>(
    f: &mut Frame<'_>,
    area: Rect,
    shell: &ShellState<S>,
    app: &A,
    title: &'static str,
) where
    A: MenuApp<S>,
{
    let border = Style::default().fg(Color::LightGreen);
    let block = Block::default()
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(Color::LightGreen),
        ))
        .borders(Borders::ALL)
        .border_style(border);
    let lines = app.header_lines(&shell.state, shell.header_tick);
    let p = Paragraph::new(Text::from(lines))
        .block(block)
        .alignment(Alignment::Center);
    f.render_widget(p, area);
}

fn render_menu<S, A>(f: &mut Frame<'_>, area: Rect, shell: &ShellState<S>, app: &A)
where
    A: MenuApp<S>,
{
    let title = if let Some(r) = &shell.running {
        format!("{} (running)", r.label)
    } else {
        app.menu_view(&shell.state, None).title
    };
    let block = Block::default().title(title).borders(Borders::ALL);
    f.render_widget(block, area);

    let inner = area.inner(Margin {
        vertical: 1,
        horizontal: 2,
    });

    let view = app.menu_view(&shell.state, shell.running.as_ref().map(|r| r.label));
    let sel = if view.items.is_empty() {
        0
    } else {
        view.selected.min(view.items.len() - 1)
    };
    let items = view.items;
    let list = List::new(items)
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("> ");

    let mut state = ListState::default();
    state.select(Some(sel));
    f.render_stateful_widget(list, inner, &mut state);
}

fn render_logs<S>(f: &mut Frame<'_>, area: Rect, shell: &mut ShellState<S>) {
    let block = Block::default()
        .title("Logs (PgUp/PgDn scroll, End follow, c cancel task, q exit)")
        .borders(Borders::ALL);

    let inner = block.inner(area);
    let height = inner.height as usize;

    let total = shell.logs.len();
    let max_scroll = total.saturating_sub(height) as u16;
    if shell.follow_bottom || shell.log_scroll > max_scroll {
        shell.log_scroll = max_scroll;
    }

    let text = Text::from(shell.logs.iter().map(render_log_line).collect::<Vec<_>>());
    let p = Paragraph::new(text)
        .block(block)
        .scroll((shell.log_scroll, 0));
    f.render_widget(p, area);
}

fn render_log_line(l: &LogLine) -> Line<'static> {
    let mut style = match l.level {
        log::Level::Error => Style::default().fg(Color::Red),
        log::Level::Warn => Style::default().fg(Color::Yellow),
        log::Level::Info => Style::default(),
        log::Level::Debug => Style::default().fg(Color::Gray),
        log::Level::Trace => Style::default().fg(Color::DarkGray),
    };

    let msg = l.text.as_str();
    if msg.starts_with("OK") || msg.starts_with("DONE") {
        style = Style::default().fg(Color::Green);
    }

    Line::from(Span::styled(format!("[{}] {}", l.level, msg), style))
}

fn render_update_prompt(f: &mut Frame<'_>, info: &UpdateInfo) {
    let area = centered_rect(70, 40, f.area());
    let block = Block::default()
        .title(Span::styled(
            "Update Available",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    let body = vec![
        Line::from(format!("branch: {}", info.branch)),
        Line::from(format!("local:  {}", info.local_hash)),
        Line::from(format!("remote: {}", info.remote_hash)),
        Line::from(""),
        Line::from(format!("behind: {}  ahead: {}", info.behind, info.ahead)),
        Line::from(""),
        Line::from(Span::styled(
            format!("latest: {}", info.remote_subject),
            Style::default().fg(Color::White),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Press y to update (git pull --ff-only) or n to skip",
            Style::default().fg(Color::LightGreen),
        )),
    ];

    let p = Paragraph::new(Text::from(body))
        .block(block)
        .wrap(Wrap { trim: true });
    f.render_widget(Clear, area);
    f.render_widget(p, area);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
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
