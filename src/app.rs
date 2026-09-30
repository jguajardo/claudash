//! Application state, key handling and background work.

use std::{
    collections::{HashMap, HashSet},
    io,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
    time::{Duration, Instant},
};

use ratatui::{
    DefaultTerminal,
    crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    widgets::ListState,
};

use crate::{
    claude_cli::{self, PromptReply},
    ecosystem::{self, Ecosystem},
    history::History,
    hooks::{self, Activity},
    instructions::{self, Instructions},
    library::{self, Library, Trashed},
    mcp::{self, McpResult, McpStatus},
    notify, paths,
    sessions::{self, Session},
    statusline,
    transcript::{self, Entry, Hit},
    ui,
};

const TICK_RATE: Duration = Duration::from_millis(250);
/// How often sessions, status line data and live sessions are refreshed
/// (only transcript files that changed are re-parsed).
const REFRESH_EVERY: Duration = Duration::from_secs(5);
/// How long the selected project must stay the same before its MCP servers are
/// checked, so scrolling through sessions doesn't start servers for every project.
const MCP_DEBOUNCE: Duration = Duration::from_millis(600);
/// How long a footer notice stays visible.
const FLASH_DURATION: Duration = Duration::from_secs(4);
/// Plan usage percentages that trigger an alert, once per window.
const PLAN_ALERTS: [f64; 2] = [80.0, 95.0];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum View {
    Dashboard,
    Ecosystem,
    Usage,
    /// A session's conversation, full screen.
    Transcript,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sessions,
    Mcp,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EcoTab {
    Skills,
    Agents,
    Commands,
    Hooks,
    Plugins,
}

impl EcoTab {
    pub const ALL: [EcoTab; 5] = [
        EcoTab::Skills,
        EcoTab::Agents,
        EcoTab::Commands,
        EcoTab::Hooks,
        EcoTab::Plugins,
    ];

    pub fn title(self) -> &'static str {
        match self {
            EcoTab::Skills => "Skills",
            EcoTab::Agents => "Agents",
            EcoTab::Commands => "Commands",
            EcoTab::Hooks => "Hooks",
            EcoTab::Plugins => "Plugins",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }
}

pub enum Popup {
    /// Scrollable text: MCP logs, prompt replies, details, help.
    Text {
        title: String,
        lines: Vec<String>,
        scroll: u16,
    },
    ConfirmDelete {
        id: String,
        title: String,
    },
    /// Trashed sessions. `purge` holds the ID awaiting a second `x`.
    Trash {
        items: Vec<Trashed>,
        state: ListState,
        purge: Option<String>,
    },
    /// Bulk cleanup: move every session matching a preset to the trash.
    Cleanup {
        preset: usize,
    },
    /// Matches of a search through every session's conversation.
    Results {
        query: String,
        hits: Vec<Hit>,
        state: ListState,
    },
}

/// A line being typed in the footer.
pub enum Input {
    Search,
    /// Search inside the open conversation.
    TranscriptSearch {
        text: String,
    },
    /// Search through every session's conversation.
    FindAll {
        text: String,
    },
    /// Tags for a session, comma or space separated.
    Tags {
        id: String,
        text: String,
    },
    /// A note for a session.
    Note {
        id: String,
        text: String,
    },
    Prompt {
        id: String,
        cwd: PathBuf,
        text: String,
    },
}

/// One screen row of the conversation view.
pub struct TranscriptRow {
    /// Index of the entry the row belongs to.
    pub entry: usize,
    pub text: String,
    pub style: ratatui::style::Style,
}

/// The conversation view's state.
pub struct TranscriptView {
    pub session_id: String,
    pub title: String,
    pub path: PathBuf,
    pub project: String,
    pub branch: Option<String>,
    pub entries: Vec<Entry>,
    /// Show tool output and compaction summaries in full.
    pub show_output: bool,
    /// Last search inside the conversation, and the entries that match it.
    pub query: Option<String>,
    pub matches: Vec<usize>,
    pub match_pos: usize,
    /// First visible row.
    pub scroll: usize,
    /// Whether the last draw showed the end of the conversation.
    pub at_end: bool,
    /// Entry to bring into view on the next draw.
    pub jump_to: Option<usize>,
    /// Rows for (width, show_output, query); rebuilt when that changes.
    pub rows: Vec<TranscriptRow>,
    pub rows_key: Option<(u16, bool, Option<String>)>,
}

impl TranscriptView {
    fn search(&mut self, query: &str) {
        let needle = query.to_lowercase();
        self.matches = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                transcript::searchable(e).is_some_and(|t| t.to_lowercase().contains(&needle))
            })
            .map(|(i, _)| i)
            .collect();
        self.query = (!query.is_empty()).then(|| query.to_string());
        self.match_pos = 0;
        self.jump_to = self.matches.first().copied();
    }

    fn step_match(&mut self, forward: bool) {
        if self.matches.is_empty() {
            return;
        }
        let n = self.matches.len();
        self.match_pos = if forward {
            (self.match_pos + 1) % n
        } else {
            (self.match_pos + n - 1) % n
        };
        self.jump_to = Some(self.matches[self.match_pos]);
    }
}

/// Bulk cleanup presets: which sessions to move to the trash.
#[derive(Clone, Copy)]
pub enum Preset {
    OlderThanDays(u64),
    LargerThanMb(u64),
}

pub const CLEANUP_PRESETS: [Preset; 5] = [
    Preset::OlderThanDays(7),
    Preset::OlderThanDays(14),
    Preset::OlderThanDays(21),
    Preset::LargerThanMb(5),
    Preset::LargerThanMb(20),
];

impl Preset {
    pub fn label(self) -> String {
        match self {
            Preset::OlderThanDays(d) => format!("not used for {d} days"),
            Preset::LargerThanMb(mb) => format!("larger than {mb} MB"),
        }
    }

    fn matches(self, session: &Session) -> bool {
        match self {
            Preset::OlderThanDays(days) => session
                .modified
                .elapsed()
                .is_ok_and(|age| age.as_secs() > days * 86_400),
            Preset::LargerThanMb(mb) => session.size > mb * 1_000_000,
        }
    }
}

pub struct Project {
    pub cwd: PathBuf,
    pub instructions: Instructions,
}

pub struct McpSnapshot {
    pub result: McpResult,
    pub checked_at: Instant,
}

/// A value computed on a background thread.
struct Job<T> {
    rx: Receiver<T>,
}

impl<T: Send + 'static> Job<T> {
    fn spawn(f: impl FnOnce() -> T + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let _ = tx.send(f());
        });
        Job { rx }
    }

    /// `Some` once the job has finished; `Err` if the thread died.
    fn poll(&self) -> Option<Result<T, ()>> {
        match self.rx.try_recv() {
            Ok(value) => Some(Ok(value)),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(())),
        }
    }
}

pub struct App {
    /// Context window size used when the status line hasn't reported one.
    pub context_limit: u64,
    /// Desktop notifications and bell for sessions that need you and plan alerts.
    notify: bool,
    pub view: View,
    pub focus: Focus,
    pub popup: Option<Popup>,
    pub input: Option<Input>,
    pub should_quit: bool,
    pub ticks: usize,
    /// Temporary footer notice: (text, is_error, shown_at).
    pub flash: Option<(String, bool, Instant)>,

    // Sessions.
    pub sessions: Vec<Session>,
    /// Indices into `sessions` that match the filter; the selection indexes this.
    pub visible: Vec<usize>,
    pub session_state: ListState,
    /// Search text (empty = no filter).
    pub filter: String,
    /// Message shown in the panel if sessions could not be loaded.
    pub sessions_error: Option<String>,
    refreshed_at: Instant,
    /// Session to resume; handled by `run`, which owns the terminal.
    pending_resume: Option<(String, PathBuf)>,
    /// Open sessions (ID -> "busy"/"idle") from `claude agents --json`.
    pub live: HashMap<String, String>,
    live_job: Option<Job<Result<HashMap<String, String>, String>>>,
    pub statusline: statusline::Store,
    /// Last hook event per session, from `claudash hook`.
    pub hook_states: HashMap<String, hooks::State>,
    /// `true` once any hook has run, i.e. `claudash setup --apply` was used.
    pub hooks_configured: bool,
    /// Activity seen at the previous check, to notify on changes only.
    seen_activity: Option<HashMap<String, Activity>>,
    /// Plan alerts already sent: (window label, resets_at, threshold).
    plan_alerts_sent: HashSet<(&'static str, i64, u64)>,

    // Project context.
    /// Project folder of the selected session and the instructions Claude loads there.
    pub project: Option<Project>,
    /// When `project` last changed; used to debounce MCP checks.
    project_since: Instant,
    /// Last MCP check per project folder; `r` clears the selected one.
    pub mcp_cache: HashMap<PathBuf, McpSnapshot>,
    /// Check in progress and its folder.
    mcp_job: Option<(PathBuf, Job<McpResult>)>,
    pub mcp_state: ListState,

    // Ecosystem view.
    pub eco: Option<(Option<PathBuf>, Ecosystem)>,
    eco_job: Option<(Option<PathBuf>, Job<Ecosystem>)>,
    pub eco_tab: EcoTab,
    pub eco_states: [ListState; 5],
    /// `claude plugin details` output per plugin ID.
    pub plugin_details: HashMap<String, Result<String, String>>,
    details_job: Option<(String, Job<Result<String, String>>)>,
    toggle_job: Option<(String, Job<Result<String, String>>)>,

    // Quick prompt.
    prompt_job: Option<(String, Job<Result<PromptReply, String>>)>,

    // Conversations.
    pub transcript: Option<TranscriptView>,
    find_job: Option<(String, Job<Vec<Hit>>)>,

    // Organization.
    pub library: Library,
    pub history: History,
    /// Usage view shows months instead of days.
    pub monthly: bool,
}

impl App {
    pub fn new(context_limit: u64, notify: bool) -> Self {
        let mut app = Self {
            context_limit,
            notify,
            view: View::Dashboard,
            focus: Focus::Sessions,
            popup: None,
            input: None,
            should_quit: false,
            ticks: 0,
            flash: None,
            sessions: Vec::new(),
            visible: Vec::new(),
            session_state: ListState::default(),
            filter: String::new(),
            sessions_error: None,
            refreshed_at: Instant::now(),
            pending_resume: None,
            live: HashMap::new(),
            live_job: None,
            statusline: statusline::Store::default(),
            hook_states: HashMap::new(),
            hooks_configured: false,
            seen_activity: None,
            plan_alerts_sent: HashSet::new(),
            project: None,
            project_since: Instant::now(),
            mcp_cache: HashMap::new(),
            mcp_job: None,
            mcp_state: ListState::default(),
            eco: None,
            eco_job: None,
            eco_tab: EcoTab::Skills,
            eco_states: Default::default(),
            plugin_details: HashMap::new(),
            details_job: None,
            toggle_job: None,
            prompt_job: None,
            transcript: None,
            find_job: None,
            library: Library::load(),
            history: History::load(),
            monthly: false,
        };
        let expired = library::purge_expired();
        if expired > 0 {
            app.show_flash(
                format!(
                    "Emptied {expired} session(s) older than {} days from the trash",
                    library::TRASH_DAYS
                ),
                false,
            );
        }
        app.refresh();
        app.update_project();
        app.skip_mcp_debounce();
        app
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        let mut last_tick = Instant::now();
        while !self.should_quit {
            self.update_project();
            terminal.draw(|frame| ui::draw(frame, self))?;

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

    pub fn on_tick(&mut self) {
        self.ticks = self.ticks.wrapping_add(1);
        if self
            .flash
            .as_ref()
            .is_some_and(|(_, _, at)| at.elapsed() >= FLASH_DURATION)
        {
            self.flash = None;
        }
        if self.refreshed_at.elapsed() >= REFRESH_EVERY {
            self.refresh();
        } else if self.ticks.is_multiple_of(4) {
            // Hook state is a few tiny files: check it every second so
            // "needs you" shows up (and notifies) right away.
            self.reload_hook_states();
        }
        self.poll_jobs();
        self.maybe_start_mcp_check();
        if self.view == View::Ecosystem {
            self.ensure_ecosystem();
            self.ensure_plugin_details();
        }
    }

    pub fn show_flash(&mut self, msg: impl Into<String>, is_error: bool) {
        self.flash = Some((msg.into(), is_error, Instant::now()));
    }

    pub fn spinner(&self) -> &'static str {
        const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        SPINNER[self.ticks % SPINNER.len()]
    }

    // ---- Sessions ----------------------------------------------------------

    /// Reloads sessions and status line data, and asks which sessions are open.
    fn refresh(&mut self) {
        self.reload_sessions();
        // Best effort: a failed write only means the chart misses this refresh.
        let _ = self.history.merge(&self.sessions);
        self.statusline = statusline::load();
        self.check_plan_alerts();
        self.reload_hook_states();
        if self.live_job.is_none() {
            self.live_job = Some(Job::spawn(claude_cli::live_sessions));
        }
        self.refreshed_at = Instant::now();
    }

    fn reload_hook_states(&mut self) {
        (self.hook_states, self.hooks_configured) = hooks::load();
        self.check_activity_changes();
    }

    /// What an open session is doing: from its hooks when set up, otherwise
    /// from `claude agents --json`. `None` for sessions that aren't open.
    pub fn activity(&self, id: &str) -> Option<Activity> {
        let live = self.live.get(id)?;
        match self.hook_states.get(id).map(hooks::State::activity) {
            Some(Activity::Ended) | None => Some(match live.as_str() {
                "busy" => Activity::Working,
                _ => Activity::Waiting,
            }),
            Some(activity) => Some(activity),
        }
    }

    /// Notifies when an open session starts needing you or finishes a reply.
    fn check_activity_changes(&mut self) {
        let now: HashMap<String, Activity> = self
            .live
            .keys()
            .filter_map(|id| self.activity(id).map(|a| (id.clone(), a)))
            .collect();
        // The first check only records the starting point.
        let Some(before) = self.seen_activity.replace(now.clone()) else {
            return;
        };
        for (id, activity) in now {
            // Only changes are news: a session seen for the first time (for
            // instance when claudash starts) doesn't notify.
            let Some(previous) = before.get(&id).copied() else {
                continue;
            };
            let message = match (previous, activity) {
                (p, Activity::NeedsYou) if p != Activity::NeedsYou => "needs you",
                (Activity::Working, Activity::Waiting) => "finished",
                _ => continue,
            };
            let title = self
                .sessions
                .iter()
                .find(|s| s.id == id)
                .map(|s| s.title.clone())
                .unwrap_or_else(|| "A session".into());
            self.alert(&format!("Claude Code {message}"), &title);
        }
    }

    /// Alerts once per window when plan usage crosses a threshold.
    fn check_plan_alerts(&mut self) {
        let Some((limits, _)) = self.statusline.rate_limits.clone() else {
            return;
        };
        for (label, window) in [("5-hour", limits.five_hour), ("7-day", limits.seven_day)] {
            let Some(w) = window.filter(statusline::Window::is_current) else {
                continue;
            };
            let crossed = PLAN_ALERTS
                .iter()
                .rev()
                .find(|&&t| w.used_percentage >= t)
                .copied();
            if let Some(threshold) = crossed
                && self
                    .plan_alerts_sent
                    .insert((label, w.resets_at, threshold as u64))
            {
                self.alert(
                    &format!("Claude plan: {label} limit at {:.0}%", w.used_percentage),
                    "Usage is getting close to the limit",
                );
            }
        }
    }

    fn alert(&mut self, title: &str, body: &str) {
        self.show_flash(format!("{title}: {body}"), false);
        if self.notify {
            notify::send(title, body);
        }
    }

    fn reload_sessions(&mut self) {
        let selected_id = self.selected_session().map(|s| s.id.clone());
        let result = paths::projects_dir()
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
    }

    /// Recomputes `visible`, keeping the selection on `keep_id` if still visible.
    fn apply_filter(&mut self, keep_id: Option<String>) {
        let query = self.filter.to_lowercase();
        self.visible = self
            .sessions
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                query.is_empty()
                    || session_matches(s, &query)
                    || self
                        .library
                        .get(&s.id)
                        .is_some_and(|m| meta_matches(m, &query))
            })
            .map(|(i, _)| i)
            .collect();
        let index = keep_id
            .and_then(|id| self.visible.iter().position(|&i| self.sessions[i].id == id))
            .or((!self.visible.is_empty()).then_some(0));
        self.session_state.select(index);
    }

    fn set_filter(&mut self, filter: String) {
        let keep = self.selected_session().map(|s| s.id.clone());
        self.filter = filter;
        self.apply_filter(keep);
    }

    pub fn selected_session(&self) -> Option<&Session> {
        self.session_state
            .selected()
            .and_then(|i| self.visible.get(i))
            .map(|&i| &self.sessions[i])
    }

    /// Why an action on the selected session isn't possible while it's open in
    /// Claude Code: two processes writing one transcript interleave messages.
    fn open_elsewhere(&self, session: &Session, action: &str) -> Option<String> {
        self.live
            .contains_key(&session.id)
            .then(|| format!("This session is open in Claude Code; close it before you {action}"))
    }

    /// Queues the selected session to be resumed with `claude --resume`.
    fn request_resume(&mut self) {
        let (id, cwd) = match self
            .selected_session()
            .map(|s| (s, self.open_elsewhere(s, "resume it here")))
        {
            None => return,
            Some((_, Some(msg))) => return self.show_flash(msg, true),
            Some((s, None)) => (s.id.clone(), self.selected_project_dir()),
        };
        match cwd {
            Ok(cwd) => self.pending_resume = Some((id, cwd.to_path_buf())),
            Err(msg) => self.show_flash(msg, true),
        }
    }

    /// Suspends the TUI, runs `claude --resume <id>` in the project folder and
    /// returns to the dashboard when Claude Code exits.
    fn resume_session(
        &mut self,
        terminal: &mut DefaultTerminal,
        id: &str,
        cwd: &Path,
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
        self.refresh();
        Ok(())
    }

    fn request_delete(&mut self) {
        let Some(session) = self.selected_session() else {
            return;
        };
        if let Some(msg) = self.open_elsewhere(session, "delete it") {
            return self.show_flash(msg, true);
        }
        self.popup = Some(Popup::ConfirmDelete {
            id: session.id.clone(),
            title: session.title.clone(),
        });
    }

    fn delete_session(&mut self, id: &str) {
        let Some(session) = self.sessions.iter().find(|s| s.id == id) else {
            return;
        };
        match library::trash(session) {
            Ok(()) => self.show_flash(
                format!("Moved \"{}\" to the trash (T to restore)", session.title),
                false,
            ),
            Err(e) => self.show_flash(
                format!("Could not move the session to the trash: {e}"),
                true,
            ),
        }
        self.reload_sessions();
    }

    fn start_prompt_input(&mut self) {
        let Some(session) = self.selected_session() else {
            return;
        };
        if let Some(msg) = self.open_elsewhere(session, "prompt it from here") {
            return self.show_flash(msg, true);
        }
        if self.prompt_job.is_some() {
            return self.show_flash("A prompt is already running", true);
        }
        let id = session.id.clone();
        match self.selected_project_dir() {
            Ok(cwd) => {
                self.input = Some(Input::Prompt {
                    id,
                    cwd: cwd.to_path_buf(),
                    text: String::new(),
                })
            }
            Err(msg) => self.show_flash(msg, true),
        }
    }

    fn send_prompt(&mut self, id: String, cwd: PathBuf, text: String) {
        if text.trim().is_empty() {
            return;
        }
        let title = self
            .sessions
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.title.clone())
            .unwrap_or_default();
        let job = Job::spawn(move || claude_cli::run_prompt(&id, &cwd, &text));
        self.prompt_job = Some((title, job));
    }

    /// Opens the conversation of session `id`, optionally at an entry and
    /// with a search already applied.
    fn open_transcript(&mut self, id: &str, jump: Option<usize>, query: Option<&str>) {
        let Some(session) = self.sessions.iter().find(|s| s.id == id) else {
            return;
        };
        let entries = match transcript::load(&session.path) {
            Ok(entries) => entries,
            Err(e) => {
                return self.show_flash(format!("Could not read the conversation: {e}"), true);
            }
        };
        let mut view = TranscriptView {
            session_id: session.id.clone(),
            title: session.title.clone(),
            path: session.path.clone(),
            project: session.project_path.clone(),
            branch: session.git_branch.clone(),
            entries,
            show_output: false,
            query: None,
            matches: Vec::new(),
            match_pos: 0,
            scroll: 0,
            at_end: false,
            jump_to: None,
            rows: Vec::new(),
            rows_key: None,
        };
        // A match inside tool output or a compaction summary needs them shown.
        if jump.and_then(|i| view.entries.get(i)).is_some_and(|e| {
            matches!(
                e.kind,
                transcript::Kind::ToolResult { .. } | transcript::Kind::CompactSummary
            )
        }) {
            view.show_output = true;
        }
        if let Some(query) = query {
            view.search(query);
            if let Some(entry) = jump {
                view.match_pos = view.matches.iter().position(|&m| m == entry).unwrap_or(0);
            }
        }
        // Without a target, start at the end: the latest turns are the usual interest.
        view.jump_to = jump.or(view.jump_to).or(Some(usize::MAX));
        self.transcript = Some(view);
        self.view = View::Transcript;
    }

    fn start_find_all(&mut self, query: String) {
        let query = query.trim().to_string();
        if query.is_empty() {
            return;
        }
        if self.find_job.is_some() {
            return self.show_flash("A search is already running", true);
        }
        let targets: Vec<(String, String, PathBuf)> = self
            .sessions
            .iter()
            .map(|s| (s.id.clone(), s.title.clone(), s.path.clone()))
            .collect();
        let job_query = query.clone();
        self.find_job = Some((
            query,
            Job::spawn(move || transcript::search(&targets, &job_query)),
        ));
    }

    pub fn finding(&self) -> Option<&str> {
        self.find_job.as_ref().map(|(q, _)| q.as_str())
    }

    /// Writes the open conversation as Markdown to the exports folder.
    fn export_transcript(&mut self) {
        let Some(view) = &self.transcript else {
            return;
        };
        let result = export_markdown(view);
        match result {
            Ok(path) => self.show_flash(format!("Exported to {}", paths::display(&path)), false),
            Err(e) => self.show_flash(format!("Could not export: {e}"), true),
        }
    }

    /// Sessions a cleanup preset would move to the trash: never open or starred ones.
    pub fn cleanup_candidates(&self, preset: Preset) -> Vec<&Session> {
        self.sessions
            .iter()
            .filter(|s| preset.matches(s))
            .filter(|s| !self.live.contains_key(&s.id))
            .filter(|s| !self.library.get(&s.id).is_some_and(|m| m.starred))
            .collect()
    }

    fn run_cleanup(&mut self, preset: Preset) {
        let ids: Vec<String> = self
            .cleanup_candidates(preset)
            .iter()
            .map(|s| s.id.clone())
            .collect();
        let (mut moved, mut bytes, mut failed) = (0, 0, 0);
        for id in ids {
            let Some(session) = self.sessions.iter().find(|s| s.id == id) else {
                continue;
            };
            let size = session.size;
            match library::trash(session) {
                Ok(()) => {
                    moved += 1;
                    bytes += size;
                }
                Err(_) => failed += 1,
            }
        }
        let mut msg = format!(
            "Moved {moved} session(s) to the trash, {:.1} MB (T to restore)",
            bytes as f64 / 1e6
        );
        if failed > 0 {
            msg.push_str(&format!("; {failed} could not be moved"));
        }
        self.show_flash(msg, failed > 0);
        self.reload_sessions();
    }

    fn open_trash(&mut self) {
        let items = library::list_trash();
        let mut state = ListState::default();
        state.select((!items.is_empty()).then_some(0));
        self.popup = Some(Popup::Trash {
            items,
            state,
            purge: None,
        });
    }

    fn toggle_star(&mut self) {
        let Some(id) = self.selected_session().map(|s| s.id.clone()) else {
            return;
        };
        if let Err(e) = self.library.update(&id, |m| m.starred = !m.starred) {
            self.show_flash(format!("Could not save: {e}"), true);
        }
    }

    pub fn prompt_running(&self) -> bool {
        self.prompt_job.is_some()
    }

    // ---- Project context and MCP ------------------------------------------

    /// The selected session's project folder, or why there isn't a usable one.
    pub fn selected_project_dir(&self) -> Result<&Path, String> {
        let session = self.selected_session().ok_or("No session selected")?;
        match &session.cwd {
            Some(cwd) if cwd.is_dir() => Ok(cwd),
            Some(cwd) => Err(format!("Folder {} no longer exists", paths::display(cwd))),
            None => Err("This session has no recorded folder".into()),
        }
    }

    /// Follows the selection: when the project folder changes, resolves its
    /// instruction files and restarts the MCP debounce.
    pub fn update_project(&mut self) {
        let target = self.selected_project_dir().ok().map(Path::to_path_buf);
        if target.as_ref() == self.project.as_ref().map(|p| &p.cwd) {
            return;
        }
        self.project = target.map(|cwd| Project {
            instructions: instructions::resolve(&cwd),
            cwd,
        });
        self.project_since = Instant::now();
        self.mcp_state.select(None);
    }

    fn skip_mcp_debounce(&mut self) {
        self.project_since = Instant::now()
            .checked_sub(MCP_DEBOUNCE)
            .unwrap_or_else(Instant::now);
    }

    /// Starts `claude mcp list` for the selected project if it hasn't been
    /// checked yet, the selection has settled and no other check is running.
    fn maybe_start_mcp_check(&mut self) {
        let Some(project) = &self.project else {
            return;
        };
        if self.mcp_job.is_none()
            && !self.mcp_cache.contains_key(&project.cwd)
            && self.project_since.elapsed() >= MCP_DEBOUNCE
        {
            let cwd = project.cwd.clone();
            let job_cwd = cwd.clone();
            self.mcp_job = Some((cwd, Job::spawn(move || mcp::check(&job_cwd))));
        }
    }

    pub fn mcp_checking(&self, cwd: &Path) -> bool {
        self.mcp_job.as_ref().is_some_and(|(c, _)| c == cwd)
    }

    /// Servers of the selected project's last check, if it succeeded.
    pub fn project_servers(&self) -> Option<&Vec<mcp::McpServer>> {
        let project = self.project.as_ref()?;
        self.mcp_cache.get(&project.cwd)?.result.as_ref().ok()
    }

    fn open_mcp_log(&mut self) {
        let (Some(project), Some(i)) = (&self.project, self.mcp_state.selected()) else {
            return;
        };
        let Some(server) = self.project_servers().and_then(|s| s.get(i)) else {
            return;
        };
        let title = format!(" {} · log ", server.full_name);
        let lines = match mcp::read_log(&project.cwd, &server.full_name) {
            Ok((file, mut lines)) => {
                lines.insert(
                    0,
                    format!("{}  (newest log, last lines)", paths::display(&file)),
                );
                lines.insert(1, String::new());
                lines
            }
            Err(e) => vec![e],
        };
        // Start at the end (clamped to the last page when drawn): the latest
        // lines are the interesting ones.
        self.popup = Some(Popup::Text {
            title,
            lines,
            scroll: u16::MAX,
        });
    }

    // ---- Ecosystem ----------------------------------------------------------

    /// Loads the ecosystem for the selected project if it isn't loaded yet.
    fn ensure_ecosystem(&mut self) {
        let project = self.project.as_ref().map(|p| p.cwd.clone());
        let loaded_for = self.eco.as_ref().map(|(p, _)| p);
        let loading_for = self.eco_job.as_ref().map(|(p, _)| p);
        if loaded_for == Some(&project) || loading_for == Some(&project) {
            return;
        }
        if self.eco_job.is_none() {
            let dir = project.clone();
            let job = Job::spawn(move || ecosystem::load(dir.as_deref()));
            self.eco_job = Some((project, job));
        }
    }

    pub fn eco_loading(&self) -> bool {
        self.eco_job.is_some()
    }

    pub fn eco_len(&self, tab: EcoTab) -> usize {
        let Some((_, eco)) = &self.eco else {
            return 0;
        };
        match tab {
            EcoTab::Skills => eco.skills.len(),
            EcoTab::Agents => eco.agents.len(),
            EcoTab::Commands => eco.commands.len(),
            EcoTab::Hooks => eco.hooks.len(),
            EcoTab::Plugins => eco.plugins.len(),
        }
    }

    pub fn selected_plugin(&self) -> Option<&ecosystem::Plugin> {
        let (_, eco) = self.eco.as_ref()?;
        let i = self.eco_states[EcoTab::Plugins.index()].selected()?;
        eco.plugins.get(i)
    }

    /// Fetches `claude plugin details` for every plugin, one at a time, starting
    /// with the selected one, so the token cost column fills in.
    fn ensure_plugin_details(&mut self) {
        if self.details_job.is_some() {
            return;
        }
        let Some((_, eco)) = &self.eco else {
            return;
        };
        let selected = self.selected_plugin().map(|p| p.id.clone());
        let next = selected
            .into_iter()
            .chain(eco.plugins.iter().map(|p| p.id.clone()))
            .find(|id| !self.plugin_details.contains_key(id));
        if let Some(id) = next {
            let job_id = id.clone();
            self.details_job = Some((id, Job::spawn(move || ecosystem::plugin_details(&job_id))));
        }
    }

    fn toggle_plugin(&mut self) {
        if self.toggle_job.is_some() {
            return self.show_flash("A plugin change is already running", true);
        }
        let Some(plugin) = self.selected_plugin() else {
            return;
        };
        let (id, enable) = (plugin.id.clone(), !plugin.enabled);
        let job_id = id.clone();
        self.toggle_job = Some((
            id,
            Job::spawn(move || ecosystem::set_plugin_enabled(&job_id, enable)),
        ));
    }

    pub fn toggling_plugin(&self) -> Option<&str> {
        self.toggle_job.as_ref().map(|(id, _)| id.as_str())
    }

    fn open_eco_details(&mut self) {
        let Some((_, eco)) = &self.eco else {
            return;
        };
        let i = self.eco_states[self.eco_tab.index()].selected();
        let popup = match self.eco_tab {
            EcoTab::Plugins => {
                let Some(plugin) = i.and_then(|i| eco.plugins.get(i)) else {
                    return;
                };
                let body = match self.plugin_details.get(&plugin.id) {
                    Some(Ok(text)) => text.lines().map(str::to_owned).collect(),
                    Some(Err(e)) => vec![e.clone()],
                    None => vec!["Loading `claude plugin details`…".to_string()],
                };
                (format!(" {} ", plugin.id), body)
            }
            tab => {
                let list = match tab {
                    EcoTab::Skills => &eco.skills,
                    EcoTab::Agents => &eco.agents,
                    EcoTab::Commands => &eco.commands,
                    _ => &eco.hooks,
                };
                let Some(item) = i.and_then(|i| list.get(i)) else {
                    return;
                };
                let mut body = vec![
                    format!("Source: {}", item.source),
                    format!("File:   {}", paths::display(&item.path)),
                    String::new(),
                ];
                body.extend(textwrap(&item.description, 90));
                (format!(" {} ", item.name), body)
            }
        };
        self.popup = Some(Popup::Text {
            title: popup.0,
            lines: popup.1,
            scroll: 0,
        });
    }

    // ---- Background jobs ----------------------------------------------------

    fn poll_jobs(&mut self) {
        if let Some((query, job)) = &self.find_job
            && let Some(result) = job.poll()
        {
            let query = query.clone();
            let hits = result.unwrap_or_default();
            self.find_job = None;
            if hits.is_empty() {
                self.show_flash(format!("No conversation mentions \"{query}\""), true);
            } else {
                let mut state = ListState::default();
                state.select(Some(0));
                self.popup = Some(Popup::Results { query, hits, state });
            }
        }

        if let Some((cwd, job)) = &self.mcp_job
            && let Some(result) = job.poll()
        {
            let result = result
                .unwrap_or_else(|()| Err("The MCP check ended without a result".into()))
                .map(|mut servers| {
                    // Online servers first, unconfigured ones last.
                    servers.sort_by_key(|s| status_rank(&s.status));
                    servers
                });
            let snapshot = McpSnapshot {
                result,
                checked_at: Instant::now(),
            };
            self.mcp_cache.insert(cwd.clone(), snapshot);
            self.mcp_job = None;
        }

        if let Some(job) = &self.live_job
            && let Some(result) = job.poll()
        {
            // If `claude agents` fails (older Claude Code), just show no live marks.
            self.live = result.ok().and_then(Result::ok).unwrap_or_default();
            self.live_job = None;
            self.check_activity_changes();
        }

        if let Some((project, job)) = &self.eco_job
            && let Some(result) = job.poll()
        {
            let eco = result.unwrap_or_default();
            self.eco = Some((project.clone(), eco));
            self.eco_job = None;
            for (i, tab) in EcoTab::ALL.into_iter().enumerate() {
                let len = self.eco_len(tab);
                let state = &mut self.eco_states[i];
                let keep = state.selected().filter(|&s| s < len);
                state.select(keep.or((len > 0).then_some(0)));
            }
        }

        if let Some((id, job)) = &self.details_job
            && let Some(result) = job.poll()
        {
            let details = result.unwrap_or_else(|()| Err("No result".into()));
            self.plugin_details.insert(id.clone(), details);
            self.details_job = None;
        }

        if let Some((id, job)) = &self.toggle_job
            && let Some(result) = job.poll()
        {
            let id = id.clone();
            self.toggle_job = None;
            match result.unwrap_or_else(|()| Err("No result".into())) {
                Ok(msg) => self.show_flash(if msg.is_empty() { id } else { msg }, false),
                Err(msg) => self.show_flash(format!("{id}: {msg}"), true),
            }
            // Reload so the list, components and MCP servers reflect the change.
            self.eco = None;
            self.plugin_details.clear();
            self.mcp_cache.clear();
            self.skip_mcp_debounce();
        }

        if let Some((title, job)) = &self.prompt_job
            && let Some(result) = job.poll()
        {
            let title = format!(" Reply · {title} ");
            let lines = match result.unwrap_or_else(|()| Err("No result".into())) {
                Ok(reply) => {
                    let mut lines = textwrap(&reply.text, 100);
                    lines.push(String::new());
                    let mut info = Vec::new();
                    if reply.is_error {
                        info.push("finished with an error".to_string());
                    }
                    if let Some(cost) = reply.cost_usd {
                        info.push(format!("cost ${cost:.4}"));
                    }
                    if reply.permission_denials > 0 {
                        info.push(format!(
                            "{} tool call(s) denied: headless runs can't ask for permission",
                            reply.permission_denials
                        ));
                    }
                    lines.push(info.join(" · "));
                    lines
                }
                Err(e) => vec![e],
            };
            self.prompt_job = None;
            self.popup = Some(Popup::Text {
                title,
                lines,
                scroll: 0,
            });
            self.reload_sessions();
        }
    }

    /// `r`: reloads everything and re-checks the selected project from scratch.
    fn refresh_all(&mut self) {
        self.refresh();
        if let Some(project) = self.project.take() {
            self.mcp_cache.remove(&project.cwd);
        }
        self.update_project();
        self.skip_mcp_debounce();
        self.eco = None;
        self.plugin_details.clear();
        // Re-read an open conversation, so `r` follows a session that's running.
        if let Some(view) = &mut self.transcript
            && let Ok(entries) = transcript::load(&view.path)
        {
            view.entries = entries;
            view.rows_key = None;
            if view.at_end {
                view.jump_to = Some(usize::MAX);
            }
        }
    }

    // ---- Keys ---------------------------------------------------------------

    pub fn handle_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }
        if self.popup.is_some() {
            return self.handle_popup_key(key.code);
        }
        if self.input.is_some() {
            return self.handle_input_key(key.code);
        }
        match key.code {
            KeyCode::Char('q') => return self.should_quit = true,
            KeyCode::Char('1') => return self.view = View::Dashboard,
            KeyCode::Char('2') => return self.view = View::Ecosystem,
            KeyCode::Char('3') => return self.view = View::Usage,
            KeyCode::Char('r') => return self.refresh_all(),
            KeyCode::Char('?') => return self.popup = Some(help_popup()),
            _ => {}
        }
        match self.view {
            View::Dashboard => match self.focus {
                Focus::Sessions => self.handle_sessions_key(key.code),
                Focus::Mcp => self.handle_mcp_key(key.code),
            },
            View::Ecosystem => self.handle_eco_key(key.code),
            View::Usage => match key.code {
                KeyCode::Esc => self.view = View::Dashboard,
                KeyCode::Char('m') => self.monthly = !self.monthly,
                _ => {}
            },
            View::Transcript => self.handle_transcript_key(key.code),
        }
    }

    fn handle_transcript_key(&mut self, code: KeyCode) {
        if code == KeyCode::Char('e') {
            return self.export_transcript();
        }
        let Some(view) = &mut self.transcript else {
            self.view = View::Dashboard;
            return;
        };
        // Scrolling by hand wins over a jump still waiting for the next draw.
        if matches!(
            code,
            KeyCode::Down
                | KeyCode::Up
                | KeyCode::PageDown
                | KeyCode::PageUp
                | KeyCode::Home
                | KeyCode::End
                | KeyCode::Char('j' | 'k' | 'g' | 'G' | ' ')
        ) {
            view.jump_to = None;
        }
        match code {
            KeyCode::Esc => self.view = View::Dashboard,
            KeyCode::Down | KeyCode::Char('j') => view.scroll = view.scroll.saturating_add(1),
            KeyCode::Up | KeyCode::Char('k') => view.scroll = view.scroll.saturating_sub(1),
            KeyCode::PageDown | KeyCode::Char(' ') => view.scroll = view.scroll.saturating_add(20),
            KeyCode::PageUp => view.scroll = view.scroll.saturating_sub(20),
            KeyCode::Home | KeyCode::Char('g') => view.scroll = 0,
            KeyCode::End | KeyCode::Char('G') => view.scroll = usize::MAX,
            KeyCode::Char('o') => {
                // Keep the same entry at the top when rows change.
                let top = view.rows.get(view.scroll).map(|r| r.entry);
                view.show_output = !view.show_output;
                view.jump_to = top;
            }
            KeyCode::Char('/') => {
                let text = view.query.clone().unwrap_or_default();
                self.input = Some(Input::TranscriptSearch { text });
            }
            KeyCode::Char('n') => view.step_match(true),
            KeyCode::Char('N') => view.step_match(false),
            _ => {}
        }
    }

    fn handle_sessions_key(&mut self, code: KeyCode) {
        match code {
            // Esc clears the filter first; with no filter it quits.
            KeyCode::Esc if !self.filter.is_empty() => self.set_filter(String::new()),
            KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('/') => self.input = Some(Input::Search),
            KeyCode::Enter => self.request_resume(),
            KeyCode::Char('d') | KeyCode::Delete => self.request_delete(),
            KeyCode::Char('p') => self.start_prompt_input(),
            KeyCode::Char('v') => {
                if let Some(id) = self.selected_session().map(|s| s.id.clone()) {
                    self.open_transcript(&id, None, None);
                }
            }
            KeyCode::Char('f') => {
                self.input = Some(Input::FindAll {
                    text: String::new(),
                })
            }
            KeyCode::Char('*') => self.toggle_star(),
            KeyCode::Char('t') => {
                if let Some(id) = self.selected_session().map(|s| s.id.clone()) {
                    let text = self
                        .library
                        .get(&id)
                        .map(|m| m.tags.join(", "))
                        .unwrap_or_default();
                    self.input = Some(Input::Tags { id, text });
                }
            }
            KeyCode::Char('n') => {
                if let Some(id) = self.selected_session().map(|s| s.id.clone()) {
                    let text = self
                        .library
                        .get(&id)
                        .map(|m| m.note.clone())
                        .unwrap_or_default();
                    self.input = Some(Input::Note { id, text });
                }
            }
            KeyCode::Char('T') => self.open_trash(),
            KeyCode::Char('C') => self.popup = Some(Popup::Cleanup { preset: 0 }),
            KeyCode::Tab => {
                if self.project_servers().is_some_and(|s| !s.is_empty()) {
                    self.focus = Focus::Mcp;
                    if self.mcp_state.selected().is_none() {
                        self.mcp_state.select(Some(0));
                    }
                }
            }
            KeyCode::Down | KeyCode::Char('j') => self.session_state.select_next(),
            KeyCode::Up | KeyCode::Char('k') => self.session_state.select_previous(),
            KeyCode::Home => self.session_state.select_first(),
            KeyCode::End => self.session_state.select_last(),
            _ => {}
        }
    }

    fn handle_mcp_key(&mut self, code: KeyCode) {
        let len = self.project_servers().map_or(0, Vec::len);
        match code {
            KeyCode::Tab | KeyCode::Esc => self.focus = Focus::Sessions,
            KeyCode::Enter => self.open_mcp_log(),
            KeyCode::Down | KeyCode::Char('j') if len > 0 => {
                let next = self
                    .mcp_state
                    .selected()
                    .map_or(0, |i| (i + 1).min(len - 1));
                self.mcp_state.select(Some(next));
            }
            KeyCode::Up | KeyCode::Char('k') => self.mcp_state.select_previous(),
            _ => {}
        }
    }

    fn handle_eco_key(&mut self, code: KeyCode) {
        let tab = self.eco_tab.index();
        let len = self.eco_len(self.eco_tab);
        let state = &mut self.eco_states[tab];
        match code {
            KeyCode::Esc => self.view = View::Dashboard,
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => {
                self.eco_tab = EcoTab::ALL[(tab + 1) % EcoTab::ALL.len()];
            }
            KeyCode::Left | KeyCode::Char('h') | KeyCode::BackTab => {
                self.eco_tab = EcoTab::ALL[(tab + EcoTab::ALL.len() - 1) % EcoTab::ALL.len()];
            }
            KeyCode::Down | KeyCode::Char('j') if len > 0 => {
                let next = state.selected().map_or(0, |i| (i + 1).min(len - 1));
                state.select(Some(next));
            }
            KeyCode::Up | KeyCode::Char('k') => state.select_previous(),
            KeyCode::Home => state.select_first(),
            KeyCode::End if len > 0 => state.select(Some(len - 1)),
            KeyCode::Enter => self.open_eco_details(),
            KeyCode::Char(' ') if self.eco_tab == EcoTab::Plugins => self.toggle_plugin(),
            _ => {}
        }
    }

    fn handle_popup_key(&mut self, code: KeyCode) {
        match &mut self.popup {
            Some(Popup::Trash {
                items,
                state,
                purge,
            }) => {
                let selected = state.selected().and_then(|i| items.get(i)).cloned();
                match code {
                    KeyCode::Esc | KeyCode::Char('q') => self.popup = None,
                    KeyCode::Down | KeyCode::Char('j') if !items.is_empty() => {
                        let next = state.selected().map_or(0, |i| (i + 1).min(items.len() - 1));
                        state.select(Some(next));
                        *purge = None;
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        state.select_previous();
                        *purge = None;
                    }
                    KeyCode::Char('u') | KeyCode::Enter => {
                        if let Some(item) = selected {
                            match library::restore(&item) {
                                Ok(()) => {
                                    self.show_flash(format!("Restored \"{}\"", item.title), false)
                                }
                                Err(e) => self.show_flash(format!("Could not restore: {e}"), true),
                            }
                            self.reload_sessions();
                            self.open_trash();
                        }
                    }
                    KeyCode::Char('x') => {
                        let Some(item) = selected else {
                            return;
                        };
                        if purge.as_deref() == Some(item.id.as_str()) {
                            match library::purge(&item) {
                                Ok(()) => self.show_flash(
                                    format!("Deleted \"{}\" for good", item.title),
                                    false,
                                ),
                                Err(e) => self.show_flash(format!("Could not delete: {e}"), true),
                            }
                            self.open_trash();
                        } else {
                            *purge = Some(item.id.clone());
                        }
                    }
                    _ => *purge = None,
                }
            }
            Some(Popup::Cleanup { preset }) => match code {
                KeyCode::Esc | KeyCode::Char('q') => self.popup = None,
                KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => {
                    *preset = (*preset + 1) % CLEANUP_PRESETS.len();
                }
                KeyCode::Left | KeyCode::Char('h') => {
                    *preset = (*preset + CLEANUP_PRESETS.len() - 1) % CLEANUP_PRESETS.len();
                }
                KeyCode::Enter => {
                    let chosen = CLEANUP_PRESETS[*preset];
                    self.popup = None;
                    self.run_cleanup(chosen);
                }
                _ => {}
            },
            Some(Popup::Results { query, hits, state }) => match code {
                KeyCode::Esc | KeyCode::Char('q') => self.popup = None,
                KeyCode::Down | KeyCode::Char('j') => {
                    let next = state.selected().map_or(0, |i| (i + 1).min(hits.len() - 1));
                    state.select(Some(next));
                }
                KeyCode::Up | KeyCode::Char('k') => state.select_previous(),
                KeyCode::Enter => {
                    let (Some(i), query) = (state.selected(), query.clone()) else {
                        return;
                    };
                    let hit = hits[i].clone();
                    self.popup = None;
                    self.open_transcript(&hit.session_id, Some(hit.entry), Some(&query));
                }
                _ => {}
            },
            Some(Popup::ConfirmDelete { id, .. }) => {
                let id = id.clone();
                if matches!(code, KeyCode::Char('y') | KeyCode::Char('Y')) {
                    self.popup = None;
                    self.delete_session(&id);
                } else if matches!(code, KeyCode::Char('n') | KeyCode::Esc | KeyCode::Char('q')) {
                    self.popup = None;
                }
            }
            // Rendering clamps `scroll` to the last page, so it only needs to grow here.
            Some(Popup::Text { scroll, .. }) => match code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter => self.popup = None,
                KeyCode::Down | KeyCode::Char('j') => *scroll = scroll.saturating_add(1),
                KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
                KeyCode::PageDown | KeyCode::Char(' ') => *scroll = scroll.saturating_add(15),
                KeyCode::PageUp => *scroll = scroll.saturating_sub(15),
                KeyCode::Home | KeyCode::Char('g') => *scroll = 0,
                KeyCode::End | KeyCode::Char('G') => *scroll = u16::MAX,
                _ => {}
            },
            None => {}
        }
    }

    fn handle_input_key(&mut self, code: KeyCode) {
        match self.input.take() {
            Some(Input::Search) => {
                match code {
                    KeyCode::Enter => return,
                    KeyCode::Esc => return self.set_filter(String::new()),
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
                self.input = Some(Input::Search);
            }
            Some(Input::TranscriptSearch { mut text }) => match code {
                KeyCode::Enter => {
                    if let Some(view) = &mut self.transcript {
                        view.search(text.trim());
                        if view.matches.is_empty() && !text.trim().is_empty() {
                            let msg = format!("No matches for \"{}\"", text.trim());
                            self.show_flash(msg, true);
                        }
                    }
                }
                KeyCode::Esc => {}
                KeyCode::Backspace => {
                    text.pop();
                    self.input = Some(Input::TranscriptSearch { text });
                }
                KeyCode::Char(c) => {
                    text.push(c);
                    self.input = Some(Input::TranscriptSearch { text });
                }
                _ => self.input = Some(Input::TranscriptSearch { text }),
            },
            Some(Input::FindAll { mut text }) => match code {
                KeyCode::Enter => self.start_find_all(text),
                KeyCode::Esc => {}
                KeyCode::Backspace => {
                    text.pop();
                    self.input = Some(Input::FindAll { text });
                }
                KeyCode::Char(c) => {
                    text.push(c);
                    self.input = Some(Input::FindAll { text });
                }
                _ => self.input = Some(Input::FindAll { text }),
            },
            Some(Input::Tags { id, mut text }) => match code {
                KeyCode::Enter => {
                    let tags = library::parse_tags(&text);
                    if let Err(e) = self.library.update(&id, |m| m.tags = tags) {
                        self.show_flash(format!("Could not save the tags: {e}"), true);
                    }
                }
                KeyCode::Esc => {}
                KeyCode::Backspace => {
                    text.pop();
                    self.input = Some(Input::Tags { id, text });
                }
                KeyCode::Char(c) => {
                    text.push(c);
                    self.input = Some(Input::Tags { id, text });
                }
                _ => self.input = Some(Input::Tags { id, text }),
            },
            Some(Input::Note { id, mut text }) => match code {
                KeyCode::Enter => {
                    let note = text.trim().to_string();
                    if let Err(e) = self.library.update(&id, |m| m.note = note) {
                        self.show_flash(format!("Could not save the note: {e}"), true);
                    }
                }
                KeyCode::Esc => {}
                KeyCode::Backspace => {
                    text.pop();
                    self.input = Some(Input::Note { id, text });
                }
                KeyCode::Char(c) => {
                    text.push(c);
                    self.input = Some(Input::Note { id, text });
                }
                _ => self.input = Some(Input::Note { id, text }),
            },
            Some(Input::Prompt { id, cwd, mut text }) => match code {
                KeyCode::Enter => self.send_prompt(id, cwd, text),
                KeyCode::Esc => {}
                KeyCode::Backspace => {
                    text.pop();
                    self.input = Some(Input::Prompt { id, cwd, text });
                }
                KeyCode::Char(c) => {
                    text.push(c);
                    self.input = Some(Input::Prompt { id, cwd, text });
                }
                _ => self.input = Some(Input::Prompt { id, cwd, text }),
            },
            None => {}
        }
    }
}

/// Writes `view` as Markdown to `<documents>/claudash-exports/` (or
/// `~/claudash-exports/`) and returns the file.
fn export_markdown(view: &TranscriptView) -> io::Result<PathBuf> {
    let dir = dirs::document_dir()
        .or_else(dirs::home_dir)
        .ok_or_else(|| io::Error::other("no documents or home folder"))?
        .join("claudash-exports");
    std::fs::create_dir_all(&dir)?;
    let slug: String = view
        .title
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|p| !p.is_empty())
        .take(8)
        .collect::<Vec<_>>()
        .join("-");
    let date = chrono::Local::now().format("%Y-%m-%d");
    let path = dir.join(format!("{date}-{slug}.md"));
    std::fs::write(&path, markdown_for(view))?;
    Ok(path)
}

pub fn markdown_for(view: &TranscriptView) -> String {
    markdown(
        &view.title,
        &view.project,
        &view.session_id,
        view.branch.as_deref(),
        &view.entries,
    )
}

/// Markdown for a session straight from its transcript (`claudash export`).
pub fn session_markdown(session: &Session) -> io::Result<String> {
    let entries = transcript::load(&session.path)?;
    Ok(markdown(
        &session.title,
        &session.project_path,
        &session.id,
        session.git_branch.as_deref(),
        &entries,
    ))
}

fn markdown(
    title: &str,
    project: &str,
    id: &str,
    branch: Option<&str>,
    entries: &[Entry],
) -> String {
    let mut meta = vec![
        ("Project", project.to_string()),
        ("Session", id.to_string()),
    ];
    if let Some(branch) = branch {
        meta.push(("Branch", branch.to_string()));
    }
    meta.push((
        "Exported",
        chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
    ));
    transcript::to_markdown(title, &meta, entries)
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

/// Match on tags (with or without `#`) and the note. `query` is lowercase.
fn meta_matches(meta: &library::Meta, query: &str) -> bool {
    let tag_query = query.trim_start_matches('#');
    meta.tags.iter().any(|t| t.contains(tag_query)) || meta.note.to_lowercase().contains(query)
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

/// Greedy word wrap that keeps existing line breaks.
pub fn textwrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for paragraph in text.lines() {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
                out.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        out.push(line);
    }
    out
}

fn help_popup() -> Popup {
    let lines = [
        "Views",
        "  1  Dashboard    2  Ecosystem    3  Usage",
        "",
        "Everywhere",
        "  r  reload sessions, re-check MCP and ecosystem",
        "  ?  this help            q / Ctrl+C  quit",
        "",
        "Dashboard · sessions",
        "  ↑/↓ j/k   move                 /      search",
        "  Enter     resume in Claude Code",
        "  p         send a one-off prompt (claude -p --resume)",
        "  d         move the session to the trash (asks first)",
        "  t / n / * tags · note · star (search finds tags and notes)",
        "  T / C     trash (restore or delete for good) · bulk cleanup",
        "  v         read the conversation      f  search all conversations",
        "  Tab       move to the MCP list",
        "",
        "Dashboard · MCP",
        "  ↑/↓       move      Enter  show the server's latest log",
        "  Tab/Esc   back to sessions",
        "",
        "Ecosystem",
        "  ←/→ Tab   switch tab (Skills, Agents, Commands, Hooks, Plugins)",
        "  Enter     details           Space  enable/disable plugin",
        "",
        "Conversation (v)",
        "  ↑/↓ PgUp/PgDn g/G  scroll    o  show/hide tool output",
        "  /  search   n/N  next/previous match   e  export to Markdown",
        "",
        "Usage",
        "  m         days / months",
        "",
        "Popups",
        "  ↑/↓ PgUp/PgDn  scroll     Esc  close",
        "",
        "Plan usage and the real context window come from Claude Code's",
        "status line and hooks: run `claudash setup` to enable them.",
    ];
    Popup::Text {
        title: " Keys ".into(),
        lines: lines.iter().map(|s| s.to_string()).collect(),
        scroll: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_words_and_keeps_line_breaks() {
        assert_eq!(
            textwrap("one two three four\nfive", 9),
            ["one two", "three", "four", "five"]
        );
        assert!(textwrap("", 10).is_empty());
    }

    /// Every view, tab and popup must render at any terminal size, however
    /// small, whatever data (or lack of it) this machine has.
    #[test]
    fn renders_at_any_size_without_panicking() {
        use ratatui::{Terminal, backend::TestBackend};

        let mut app = App::new(1_000_000, false);
        app.eco = Some((None, Ecosystem::default()));
        let entry = |kind, text: &str| Entry {
            kind,
            at: None,
            text: text.into(),
        };
        app.transcript = Some(TranscriptView {
            session_id: "x".into(),
            title: "A conversation".into(),
            path: PathBuf::new(),
            project: "~/p".into(),
            branch: Some("main".into()),
            entries: vec![
                entry(transcript::Kind::User, "a long prompt ".repeat(40).as_str()),
                entry(
                    transcript::Kind::ToolUse {
                        name: "Bash".into(),
                    },
                    "cargo test",
                ),
                entry(
                    transcript::Kind::ToolResult { is_error: true },
                    "failed\nhere",
                ),
                entry(
                    transcript::Kind::Compaction {
                        pre_tokens: Some(1000),
                    },
                    "",
                ),
            ],
            show_output: true,
            query: Some("prompt".into()),
            matches: vec![0],
            match_pos: 0,
            scroll: 0,
            at_end: false,
            jump_to: Some(usize::MAX),
            rows: Vec::new(),
            rows_key: None,
        });
        let hit = Hit {
            session_id: "x".into(),
            title: "t".into(),
            entry: 0,
            snippet: "…a snippet…".into(),
            at: None,
        };
        for (w, h) in [(1, 1), (10, 3), (20, 5), (40, 10), (80, 24), (200, 60)] {
            for view in [
                View::Dashboard,
                View::Ecosystem,
                View::Usage,
                View::Transcript,
            ] {
                for tab in EcoTab::ALL {
                    for popup in 0..6 {
                        app.view = view;
                        app.eco_tab = tab;
                        app.monthly = popup % 2 == 0;
                        app.popup = match popup {
                            0 => None,
                            1 => Some(help_popup()),
                            2 => Some(Popup::Trash {
                                items: Vec::new(),
                                state: ListState::default(),
                                purge: None,
                            }),
                            3 => Some(Popup::Cleanup { preset: 4 }),
                            4 => Some(Popup::Results {
                                query: "q".into(),
                                hits: vec![hit.clone()],
                                state: ListState::default(),
                            }),
                            _ => Some(Popup::ConfirmDelete {
                                id: "x".into(),
                                title: "A session title far too long to fit".into(),
                            }),
                        };
                        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
                        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
                    }
                }
            }
        }
    }

    #[test]
    fn notifies_only_on_changes_of_known_sessions() {
        let mut app = App::new(1_000_000, false);
        let id = "11111111-2222-3333-4444-555555555555".to_string();
        let state = |event: &str, kind: Option<&str>| hooks::State {
            event: event.into(),
            notification_type: kind.map(Into::into),
            at: 0,
        };
        app.flash = None;
        app.seen_activity = Some(HashMap::new());
        app.live = HashMap::from([(id.clone(), "busy".into())]);
        app.hook_states =
            HashMap::from([(id.clone(), state("Notification", Some("permission_prompt")))]);
        // First time this session is seen: no alert, even though it needs you.
        app.check_activity_changes();
        assert!(app.flash.is_none());

        app.hook_states
            .insert(id.clone(), state("PostToolUse", None));
        app.check_activity_changes();
        assert!(app.flash.is_none());
        app.hook_states
            .insert(id.clone(), state("Notification", Some("permission_prompt")));
        app.check_activity_changes();
        assert!(
            app.flash
                .as_ref()
                .is_some_and(|f| f.0.contains("needs you"))
        );

        app.flash = None;
        app.hook_states
            .insert(id.clone(), state("UserPromptSubmit", None));
        app.check_activity_changes();
        app.hook_states.insert(id, state("Stop", None));
        app.check_activity_changes();
        assert!(app.flash.as_ref().is_some_and(|f| f.0.contains("finished")));
    }
}
