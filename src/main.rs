mod claude_cli;
mod mcp;
mod sessions;

use std::{
    io,
    path::PathBuf,
    sync::mpsc::{Receiver, TryRecvError},
    time::{Duration, Instant},
};

use mcp::{McpResult, McpServer, McpStatus};
use ratatui::{
    DefaultTerminal, Frame,
    crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Gauge, HighlightSpacing, List, ListItem, ListState, Padding,
        Paragraph, Wrap,
    },
};
use sessions::Session;

const TICK_RATE: Duration = Duration::from_millis(250);
/// How often sessions are reloaded (only files that changed are re-parsed).
const SESSIONS_REFRESH: Duration = Duration::from_secs(5);

/// Default context window; override with `--context-limit` or
/// `CLAUDASH_CONTEXT_LIMIT`.
const DEFAULT_CONTEXT_LIMIT: u64 = 1_000_000;
const CONTEXT_LIMIT_ENV: &str = "CLAUDASH_CONTEXT_LIMIT";

/// How long a footer notice stays visible.
const FLASH_DURATION: Duration = Duration::from_secs(4);

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

struct App {
    /// Context window size the gauge is measured against.
    context_limit: u64,
    sessions: Vec<Session>,
    /// Indices into `sessions` that match the filter; the selection indexes this.
    visible: Vec<usize>,
    session_state: ListState,
    /// Search text (empty = no filter).
    filter: String,
    /// `true` while typing the filter after pressing `/`.
    searching: bool,
    /// Session to resume; handled by `run`, which owns the terminal.
    pending_resume: Option<(String, PathBuf)>,
    /// Temporary footer notice: (text, is_error, shown_at).
    flash: Option<(String, bool, Instant)>,
    /// Message shown in the panel if sessions could not be loaded.
    sessions_error: Option<String>,
    mcp_servers: Vec<McpServer>,
    mcp_error: Option<String>,
    /// Channel for the check in progress (`claude mcp list` takes several seconds).
    mcp_pending: Option<Receiver<McpResult>>,
    mcp_checked_at: Option<Instant>,
    sessions_loaded_at: Instant,
    ticks: usize,
    should_quit: bool,
}

impl App {
    fn new(context_limit: u64) -> Self {
        let mut app = Self {
            context_limit,
            sessions: Vec::new(),
            visible: Vec::new(),
            session_state: ListState::default(),
            filter: String::new(),
            searching: false,
            pending_resume: None,
            flash: None,
            sessions_error: None,
            mcp_servers: Vec::new(),
            mcp_error: None,
            mcp_pending: None,
            mcp_checked_at: None,
            sessions_loaded_at: Instant::now(),
            ticks: 0,
            should_quit: false,
        };
        app.reload_sessions();
        app.refresh_mcp();
        app
    }

    /// Starts a new MCP check unless one is already running.
    fn refresh_mcp(&mut self) {
        if self.mcp_pending.is_none() {
            self.mcp_pending = Some(mcp::spawn_check());
        }
    }

    fn poll_mcp(&mut self) {
        let Some(rx) = &self.mcp_pending else {
            return;
        };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("The MCP check ended without a result".into()),
        };
        self.mcp_pending = None;
        self.mcp_checked_at = Some(Instant::now());
        match result {
            Ok(mut servers) => {
                // Online servers first, unconfigured ones last.
                servers.sort_by_key(|s| status_rank(&s.status));
                self.mcp_servers = servers;
                self.mcp_error = None;
            }
            Err(e) => self.mcp_error = Some(e),
        }
    }

    fn reload_sessions(&mut self) {
        let selected_id = self.selected_session().map(|s| s.id.clone());

        let result = sessions::projects_dir()
            .ok_or_else(|| "Could not find the home directory".to_string())
            .and_then(|dir| {
                sessions::load_sessions(&dir, &self.sessions)
                    .map_err(|e| format!("Could not read {}: {e}", dir.display()))
            });
        match result {
            Ok(sessions) => {
                self.sessions = sessions;
                self.sessions_error = None;
            }
            Err(e) => {
                self.sessions.clear();
                self.sessions_error = Some(e);
            }
        }

        self.apply_filter(selected_id);
        self.sessions_loaded_at = Instant::now();
    }

    /// Recomputes `visible`, keeping the selection on `keep_id` if still visible.
    fn apply_filter(&mut self, keep_id: Option<String>) {
        let query = self.filter.to_lowercase();
        self.visible = self
            .sessions
            .iter()
            .enumerate()
            .filter(|(_, s)| query.is_empty() || session_matches(s, &query))
            .map(|(i, _)| i)
            .collect();
        let index = keep_id
            .and_then(|id| self.visible.iter().position(|&i| self.sessions[i].id == id))
            .or((!self.visible.is_empty()).then_some(0));
        self.session_state.select(index);
    }

    fn show_flash(&mut self, msg: impl Into<String>, is_error: bool) {
        self.flash = Some((msg.into(), is_error, Instant::now()));
    }

    /// Queues the selected session to be resumed with `claude --resume`.
    fn request_resume(&mut self) {
        let Some(session) = self.selected_session() else {
            return;
        };
        match &session.cwd {
            Some(cwd) if cwd.is_dir() => {
                self.pending_resume = Some((session.id.clone(), cwd.clone()));
            }
            Some(cwd) => {
                let msg = format!("Folder {} no longer exists", cwd.display());
                self.show_flash(msg, true);
            }
            None => self.show_flash("This session has no recorded folder", true),
        }
    }

    /// Suspends the TUI, runs `claude --resume <id>` in the project folder and
    /// returns to the dashboard when Claude Code exits.
    fn resume_session(
        &mut self,
        terminal: &mut DefaultTerminal,
        id: &str,
        cwd: &PathBuf,
    ) -> io::Result<()> {
        ratatui::restore();
        let status = claude_cli::command()
            .args(["--resume", id])
            .current_dir(cwd)
            .status();
        // A fresh terminal instead of `terminal.clear()`: clear() queries the cursor
        // position, which not every terminal answers, and a new one redraws everything.
        *terminal = ratatui::try_init()?;

        match status {
            Ok(s) if s.success() => self.show_flash("Claude Code session ended", false),
            Ok(s) => self.show_flash(format!("claude exited with {s}"), true),
            Err(e) => self.show_flash(format!("Could not run `claude`: {e}"), true),
        }
        self.reload_sessions();
        Ok(())
    }

    fn run(&mut self, terminal: &mut DefaultTerminal) -> std::io::Result<()> {
        let mut last_tick = Instant::now();
        while !self.should_quit {
            terminal.draw(|frame| self.draw(frame))?;

            let timeout = TICK_RATE.saturating_sub(last_tick.elapsed());
            if event::poll(timeout)?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                self.handle_key(key);
            }
            if let Some((id, cwd)) = self.pending_resume.take() {
                self.resume_session(terminal, &id, &cwd)?;
            }
            if last_tick.elapsed() >= TICK_RATE {
                self.on_tick();
                last_tick = Instant::now();
            }
        }
        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }
        if self.searching {
            self.handle_search_key(key.code);
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            // Esc clears the filter first; with no filter it quits.
            KeyCode::Esc if !self.filter.is_empty() => self.set_filter(String::new()),
            KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('/') => self.searching = true,
            KeyCode::Enter => self.request_resume(),
            KeyCode::Down | KeyCode::Char('j') => self.session_state.select_next(),
            KeyCode::Up | KeyCode::Char('k') => self.session_state.select_previous(),
            KeyCode::Home => self.session_state.select_first(),
            KeyCode::End => self.session_state.select_last(),
            KeyCode::Char('r') => {
                self.reload_sessions();
                self.refresh_mcp();
            }
            _ => {}
        }
    }

    fn handle_search_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Enter => self.searching = false,
            KeyCode::Esc => {
                self.searching = false;
                self.set_filter(String::new());
            }
            KeyCode::Backspace => {
                let mut filter = self.filter.clone();
                filter.pop();
                self.set_filter(filter);
            }
            KeyCode::Char(c) => {
                let filter = format!("{}{c}", self.filter);
                self.set_filter(filter);
            }
            KeyCode::Down => self.session_state.select_next(),
            KeyCode::Up => self.session_state.select_previous(),
            _ => {}
        }
    }

    fn set_filter(&mut self, filter: String) {
        let keep = self.selected_session().map(|s| s.id.clone());
        self.filter = filter;
        self.apply_filter(keep);
    }

    fn on_tick(&mut self) {
        self.ticks = self.ticks.wrapping_add(1);
        self.poll_mcp();
        if self
            .flash
            .as_ref()
            .is_some_and(|(_, _, at)| at.elapsed() >= FLASH_DURATION)
        {
            self.flash = None;
        }
        if self.sessions_loaded_at.elapsed() >= SESSIONS_REFRESH {
            self.reload_sessions();
        }
    }

    fn selected_session(&self) -> Option<&Session> {
        self.session_state
            .selected()
            .and_then(|i| self.visible.get(i))
            .map(|&i| &self.sessions[i])
    }

    fn draw(&mut self, frame: &mut Frame) {
        let [header, body, tokens, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(8),
            Constraint::Length(6),
            Constraint::Length(1),
        ])
        .areas(frame.area());

        let [sessions, mcp] =
            Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
                .areas(body);

        self.draw_header(frame, header);
        self.draw_sessions(frame, sessions);
        self.draw_mcp(frame, mcp);
        self.draw_tokens(frame, tokens);
        self.draw_footer(frame, footer);
    }

    fn draw_header(&self, frame: &mut Frame, area: Rect) {
        let title = Line::from(vec![
            Span::styled(
                " ◆ claudash ",
                Style::new().fg(Color::Black).bg(Color::LightMagenta).bold(),
            ),
            Span::raw("  Control dashboard for Claude Code").dark_gray(),
        ]);
        frame.render_widget(Paragraph::new(title), area);
    }

    fn draw_sessions(&mut self, frame: &mut Frame, area: Rect) {
        let count = if self.filter.is_empty() {
            format!(" {} sessions ", self.sessions.len())
        } else {
            format!(" {}/{} sessions ", self.visible.len(), self.sessions.len())
        };
        let mut block =
            panel("Sessions", Color::LightMagenta).title_bottom(Line::from(count).right_aligned());
        if !self.filter.is_empty() {
            block = block.title(Line::from(format!(" filter: {} ", self.filter)).fg(Color::Yellow));
        }

        if self.visible.is_empty() {
            let msg = match &self.sessions_error {
                Some(e) => e.as_str(),
                None if !self.sessions.is_empty() => "No sessions match the filter.",
                None => "No sessions in ~/.claude/projects yet.",
            };
            let empty = Paragraph::new(msg)
                .style(Style::new().fg(Color::DarkGray).italic())
                .wrap(Wrap { trim: true })
                .block(block);
            frame.render_widget(empty, area);
            return;
        }

        let items: Vec<ListItem> = self
            .visible
            .iter()
            .map(|&i| &self.sessions[i])
            .map(|s| {
                let mut detail = vec![Span::styled(
                    format!("  {}", s.project_path),
                    Style::new().fg(Color::Gray),
                )];
                if let Some(branch) = &s.git_branch {
                    detail.push(Span::styled(
                        format!("  {branch}"),
                        Style::new().fg(Color::Cyan),
                    ));
                }
                detail.push(Span::styled(
                    format!("  · {}", sessions::relative_age(s.modified)),
                    Style::new().fg(Color::DarkGray).italic(),
                ));
                ListItem::new(vec![
                    Line::from(Span::styled(s.title.as_str(), Style::new().bold())),
                    Line::from(detail),
                ])
            })
            .collect();

        let list = List::new(items)
            .block(block)
            .highlight_style(
                Style::new()
                    .bg(Color::Rgb(60, 40, 70))
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▶ ")
            .highlight_spacing(HighlightSpacing::Always);

        frame.render_stateful_widget(list, area, &mut self.session_state);
    }

    fn draw_mcp(&self, frame: &mut Frame, area: Rect) {
        let online = self
            .mcp_servers
            .iter()
            .filter(|s| s.status == McpStatus::Connected)
            .count();
        let footer = if self.mcp_pending.is_some() {
            format!(" {} checking… ", SPINNER[self.ticks % SPINNER.len()])
        } else {
            let age = self
                .mcp_checked_at
                .map(|t| format!(" · {}s ago", t.elapsed().as_secs()))
                .unwrap_or_default();
            format!(" {online}/{} online{age} ", self.mcp_servers.len())
        };
        let block =
            panel("MCP Status", Color::Cyan).title_bottom(Line::from(footer).right_aligned());

        if let Some(error) = &self.mcp_error {
            let msg = Paragraph::new(error.as_str())
                .style(Style::new().fg(Color::Red))
                .wrap(Wrap { trim: true })
                .block(block);
            frame.render_widget(msg, area);
            return;
        }
        if self.mcp_servers.is_empty() {
            let text = if self.mcp_pending.is_some() {
                "Running `claude mcp list`…"
            } else {
                "No MCP servers configured."
            };
            let msg = Paragraph::new(text)
                .style(Style::new().fg(Color::DarkGray).italic())
                .block(block);
            frame.render_widget(msg, area);
            return;
        }

        let items: Vec<ListItem> = self
            .mcp_servers
            .iter()
            .map(|s| {
                let (dot, label, color) = match &s.status {
                    McpStatus::Connected => ("●", "ONLINE", Color::Green),
                    McpStatus::NeedsAuth => ("◐", "AUTH", Color::Yellow),
                    McpStatus::Failed(_) => ("✘", "ERROR", Color::Red),
                    McpStatus::NotConfigured => ("○", "NOT SET", Color::DarkGray),
                    McpStatus::Unknown(_) => ("?", "UNKNOWN", Color::Gray),
                };
                let name: String = s.name.chars().take(18).collect();
                ListItem::new(Line::from(vec![
                    Span::styled(format!("{dot} "), Style::new().fg(color)),
                    Span::raw(format!("{name:<19}")),
                    Span::styled(format!("{label:<11}"), Style::new().fg(color).bold()),
                    Span::styled(format!(" {}", s.source), Style::new().fg(Color::DarkGray)),
                ]))
            })
            .collect();
        frame.render_widget(List::new(items).block(block), area);
    }

    /// Context window usage of the selected session, plus its totals.
    fn draw_tokens(&self, frame: &mut Frame, area: Rect) {
        let block = panel("Token Usage", Color::Yellow);
        let Some(session) = self.selected_session() else {
            let msg = Paragraph::new("Select a session to see its usage.")
                .style(Style::new().fg(Color::DarkGray).italic())
                .block(block);
            frame.render_widget(msg, area);
            return;
        };
        let t = &session.tokens;
        let block = block.title(
            Line::from(format!(" {} ", session.title))
                .fg(Color::DarkGray)
                .right_aligned(),
        );
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let [gauge_area, stats_area] =
            Layout::vertical([Constraint::Length(2), Constraint::Length(2)]).areas(inner);

        let ratio = t.context_used as f64 / self.context_limit as f64;
        let color = match ratio {
            r if r < 0.6 => Color::Green,
            r if r < 0.85 => Color::Yellow,
            _ => Color::Red,
        };
        let label = format!(
            "Context {:.1}%  ({} / {})",
            ratio * 100.0,
            sessions::human_tokens(t.context_used),
            sessions::human_tokens(self.context_limit),
        );
        let gauge = Gauge::default()
            .gauge_style(Style::new().fg(color).bg(Color::Rgb(40, 40, 40)))
            .ratio(ratio.clamp(0.0, 1.0))
            .label(Span::styled(label, Style::new().fg(Color::White).bold()))
            .use_unicode(true);
        frame.render_widget(gauge, gauge_area);

        let dim = Style::new().fg(Color::DarkGray);
        let value = Style::new().fg(Color::White).bold();
        let stat = |name: &'static str, n: u64| {
            [
                Span::styled(name, dim),
                Span::styled(sessions::human_tokens(n), value),
                Span::raw("   "),
            ]
        };
        let mut stats: Vec<Span> = [
            stat("input ", t.total.input_tokens),
            stat("cache write ", t.total.cache_creation_input_tokens),
            stat("cache read ", t.total.cache_read_input_tokens),
            stat("output ", t.total.output_tokens),
        ]
        .into_iter()
        .flatten()
        .collect();
        if let Some(cost) = t.cost_usd {
            stats.push(Span::styled("cost ", dim));
            stats.push(Span::styled(format!("${cost:.2}"), value.fg(Color::Green)));
        }
        let model = Line::from(vec![
            Span::styled("model ", dim),
            Span::styled(
                t.model.as_deref().unwrap_or("—"),
                Style::new().fg(Color::Cyan),
            ),
        ]);
        frame.render_widget(Paragraph::new(vec![Line::from(stats), model]), stats_area);
    }

    fn draw_footer(&self, frame: &mut Frame, area: Rect) {
        if self.searching {
            let line = Line::from(vec![
                Span::styled(" / ", Style::new().fg(Color::Black).bg(Color::Yellow)),
                Span::raw(format!(" {}", self.filter)),
                Span::styled("▌", Style::new().fg(Color::Yellow)),
                Span::raw("   Enter keep · Esc clear").dark_gray(),
            ]);
            frame.render_widget(Paragraph::new(line), area);
            return;
        }
        if let Some((msg, is_error, _)) = &self.flash {
            let color = if *is_error { Color::Red } else { Color::Green };
            let line = Line::from(Span::styled(
                format!(" {msg}"),
                Style::new().fg(color).bold(),
            ));
            frame.render_widget(Paragraph::new(line), area);
            return;
        }
        let key = |k: &'static str| Span::styled(k, Style::new().fg(Color::Black).bg(Color::Gray));
        let help = Line::from(vec![
            key(" ↑/↓ "),
            Span::raw(" navigate  ").dark_gray(),
            key(" Enter "),
            Span::raw(" resume  ").dark_gray(),
            key(" / "),
            Span::raw(" search  ").dark_gray(),
            key(" r "),
            Span::raw(" reload  ").dark_gray(),
            key(" q "),
            Span::raw(" quit").dark_gray(),
        ]);
        frame.render_widget(Paragraph::new(help), area);
    }
}

/// Case-insensitive match on title, path and branch.
/// `query` must already be lowercase.
fn session_matches(session: &Session, query: &str) -> bool {
    session.title.to_lowercase().contains(query)
        || session.project_path.to_lowercase().contains(query)
        || session
            .git_branch
            .as_deref()
            .is_some_and(|b| b.to_lowercase().contains(query))
}

fn status_rank(status: &McpStatus) -> u8 {
    match status {
        McpStatus::Connected => 0,
        McpStatus::NeedsAuth => 1,
        McpStatus::Failed(_) => 2,
        McpStatus::Unknown(_) => 3,
        McpStatus::NotConfigured => 4,
    }
}

fn panel(title: &str, accent: Color) -> Block<'_> {
    Block::default()
        .title(Line::from(format!(" {title} ")).bold().fg(accent))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(accent))
        .padding(Padding::horizontal(1))
}

const HELP: &str = "\
claudash — control dashboard for Claude Code (unofficial project)

Usage: claudash [--context-limit <TOKENS>]

Options:
  --context-limit <TOKENS>  Context window size (e.g. 1M, 200k, 500000).
                            Also set with the CLAUDASH_CONTEXT_LIMIT variable.
                            Default: 1M.
  -h, --help                Print this help.

Keys: ↑/↓ navigate · Enter resume session · / search · r reload · q quit";

/// Accepts "1M", "200k", "500000" (any case, optional '_' separators).
fn parse_token_count(input: &str) -> Result<u64, String> {
    let clean = input.trim().replace('_', "").to_lowercase();
    let (digits, multiplier) = match clean.strip_suffix('m') {
        Some(d) => (d, 1_000_000.0),
        None => match clean.strip_suffix('k') {
            Some(d) => (d, 1_000.0),
            None => (clean.as_str(), 1.0),
        },
    };
    match digits.parse::<f64>() {
        Ok(n) if n > 0.0 => Ok((n * multiplier).round() as u64),
        _ => Err(format!(
            "invalid context window value: '{input}' (use e.g. 1M, 200k or 500000)"
        )),
    }
}

/// Precedence: `--context-limit` flag > environment variable > default.
fn context_limit_from(
    mut args: impl Iterator<Item = String>,
    env: Option<String>,
) -> Result<u64, String> {
    let mut flag = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Err(HELP.to_string()),
            "--context-limit" => {
                flag = Some(args.next().ok_or("missing value for --context-limit")?);
            }
            other => match other.strip_prefix("--context-limit=") {
                Some(value) => flag = Some(value.to_string()),
                None => return Err(format!("unknown argument: '{other}'\n\n{HELP}")),
            },
        }
    }
    flag.or(env)
        .map_or(Ok(DEFAULT_CONTEXT_LIMIT), |v| parse_token_count(&v))
}

fn main() -> std::io::Result<()> {
    // Validated before taking over the terminal so errors print normally.
    let context_limit = match context_limit_from(
        std::env::args().skip(1),
        std::env::var(CONTEXT_LIMIT_ENV).ok(),
    ) {
        Ok(limit) => limit,
        Err(msg) if msg == HELP => {
            println!("{HELP}");
            return Ok(());
        }
        Err(msg) => {
            eprintln!("claudash: {msg}");
            std::process::exit(2);
        }
    };

    // ratatui::init enables raw mode + the alternate screen and installs a panic hook
    // that restores the terminal; ratatui::restore leaves it clean on exit.
    let mut terminal = ratatui::init();
    let result = App::new(context_limit).run(&mut terminal);
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> impl Iterator<Item = String> {
        list.iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn parses_token_counts() {
        assert_eq!(parse_token_count("1M"), Ok(1_000_000));
        assert_eq!(parse_token_count("200k"), Ok(200_000));
        assert_eq!(parse_token_count("1.5m"), Ok(1_500_000));
        assert_eq!(parse_token_count("500_000"), Ok(500_000));
        assert!(parse_token_count("lots").is_err());
        assert!(parse_token_count("0").is_err());
    }

    #[test]
    fn context_limit_precedence() {
        assert_eq!(
            context_limit_from(args(&[]), None),
            Ok(DEFAULT_CONTEXT_LIMIT)
        );
        assert_eq!(
            context_limit_from(args(&[]), Some("200k".into())),
            Ok(200_000)
        );
        assert_eq!(
            context_limit_from(args(&["--context-limit", "500k"]), Some("200k".into())),
            Ok(500_000)
        );
        assert_eq!(
            context_limit_from(args(&["--context-limit=2M"]), None),
            Ok(2_000_000)
        );
        assert!(context_limit_from(args(&["--context-limit"]), None).is_err());
        assert!(context_limit_from(args(&["--foo"]), None).is_err());
    }
}
