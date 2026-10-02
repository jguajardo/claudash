//! Application state, key handling and background work.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
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
    analysis::{self, Analysis},
    claude_cli::{self, LiveSession, PromptReply},
    doctor,
    ecosystem::{self, Ecosystem},
    git,
    history::History,
    hooks::{self, Activity},
    instructions::{self, Instructions},
    keys::{self, Binding, Context},
    library::{self, Library, Trashed},
    mcp::{self, McpResult, McpStatus},
    notify, paths,
    projects::{self, Model as ProjectsModel},
    prompts::{self, Prompt},
    review::{self, Branch, DiffStat, Findings, Listing, Mode, Review},
    sessions::{self, Session},
    specs, statusline,
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
/// How often git state is refreshed for session folders.
const GIT_EVERY: Duration = Duration::from_secs(15);
/// Window for "used N times" counts in the Ecosystem view.
pub const USAGE_WINDOW_DAYS: u64 = 30;

/// How long a footer notice stays visible.
const FLASH_DURATION: Duration = Duration::from_secs(4);
/// Plan usage percentages that trigger an alert, once per window.
const PLAN_ALERTS: [f64; 2] = [80.0, 95.0];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// Every session, the selected one's project and its usage.
    Sessions,
    /// Repositories, their checkouts and worktrees, and what's wrong.
    Projects,
    /// What open sessions are doing right now.
    Activity,
    /// MCP server and background session logs.
    Logs,
    Ecosystem,
    Usage,
    /// A session's conversation, full screen.
    Transcript,
    /// Everything claudash does, and whether it's set up.
    Help,
    /// One session in depth: context over time, tools, files, subagents.
    Inspect,
}

/// Views in the order of their number keys, `1` to `6`.
pub const VIEW_KEYS: [View; 6] = [
    View::Sessions,
    View::Activity,
    View::Projects,
    View::Logs,
    View::Usage,
    View::Ecosystem,
];

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
    /// A yes/no question before something that changes things.
    Confirm {
        title: String,
        lines: Vec<String>,
        /// What `y` does.
        yes: String,
        action: Confirm,
    },
    /// Trashed sessions. `purge` holds the ID awaiting a second `D`.
    Trash {
        items: Vec<Trashed>,
        state: ListState,
        purge: Option<String>,
    },
    /// Bulk cleanup: move every session matching a preset to the trash.
    Cleanup { preset: usize },
    /// Your earlier prompts, filtered as you type.
    Prompts {
        query: String,
        matches: Vec<usize>,
        state: ListState,
    },
    /// Today's summary, exportable to Markdown.
    Summary { markdown: String, scroll: u16 },
    /// A repository's remote branches, to pick one to review.
    Branches {
        repo: PathBuf,
        repo_name: String,
        listing: Listing,
        /// When each branch was last reviewed (epoch seconds).
        reviewed: HashMap<String, i64>,
        query: String,
        matches: Vec<usize>,
        state: ListState,
    },
    /// How to review the chosen branch.
    ReviewSetup {
        task: ReviewTask,
        stat: Result<DiffStat, String>,
        last: Option<Review>,
        /// Index into the options: the modes, then "show the last review".
        choice: usize,
    },
    /// A review's findings; `detail` shows the selected one in full.
    Findings {
        review: Review,
        state: ListState,
        detail: Option<u16>,
    },
    /// Every action, found by name and run by pressing its key.
    Palette {
        commands: Vec<&'static Binding>,
        query: String,
        matches: Vec<usize>,
        state: ListState,
    },
    /// Matches of a search through every session's conversation.
    Results {
        query: String,
        hits: Vec<Hit>,
        state: ListState,
    },
}

type BranchesJob = Job<Result<Listing, String>>;

/// The worktree, and the findings when Claude ran headless.
type ReviewJob = Job<Result<(PathBuf, Option<Findings>), String>>;

/// A branch review being set up or running.
#[derive(Clone, Debug, PartialEq)]
pub struct ReviewTask {
    /// The repository's main checkout.
    pub repo: PathBuf,
    pub repo_name: String,
    pub branch: String,
    /// `origin/<branch>`.
    pub reference: String,
    pub base: String,
    pub mode: Mode,
    pub session_id: String,
}

/// Actions that ask first.
#[derive(Clone, Debug, PartialEq)]
pub enum Confirm {
    Trash(String),
    StopBackground(String),
    RespawnBackground(String),
    RemoveWorktree {
        main: PathBuf,
        path: PathBuf,
    },
    PruneWorktrees(PathBuf),
    /// Remove a review's worktree.
    RemoveReviewWorktree {
        main: PathBuf,
        path: PathBuf,
    },
    /// Clear an MCP server's stored credentials.
    McpLogout {
        server: String,
        cwd: PathBuf,
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
    /// Show only log lines containing this text.
    LogFilter {
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ActivityFocus {
    #[default]
    Open,
    Background,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LogKind {
    /// An MCP server's log directory.
    Mcp(PathBuf),
    /// A background session, by short ID.
    Background(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct LogSource {
    pub label: String,
    pub group: String,
    pub kind: LogKind,
}

/// A log's lines, or why it couldn't be read.
pub type LogLines = Result<Vec<String>, String>;

/// The Logs view's state.
pub struct LogsView {
    pub sources: Vec<LogSource>,
    pub state: ListState,
    pub lines: LogLines,
    /// Which source `lines` belong to.
    pub loaded: Option<LogKind>,
    loaded_at: Option<Instant>,
    job: Option<(LogKind, Job<LogLines>)>,
    pub filter: String,
    pub errors_only: bool,
    /// Keep the view at the end and reload every 2 seconds.
    pub follow: bool,
    pub scroll: usize,
}

impl Default for LogsView {
    fn default() -> Self {
        LogsView {
            sources: Vec::new(),
            state: ListState::default(),
            lines: Ok(Vec::new()),
            loaded: None,
            loaded_at: None,
            job: None,
            filter: String::new(),
            errors_only: false,
            follow: true,
            scroll: usize::MAX,
        }
    }
}

impl LogsView {
    pub fn loading(&self) -> bool {
        self.job.is_some()
    }
}

/// One tool call in the activity feed.
pub struct FeedItem<'a> {
    pub session: &'a Session,
    pub event: &'a analysis::Event,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectRow {
    Repo(usize),
    Checkout(usize, usize),
    Loose(usize),
}

/// Something across projects that deserves attention.
#[derive(Clone, Debug, PartialEq)]
pub enum Problem {
    /// Two or more open sessions work in the same checkout.
    SharedFolder { dir: PathBuf, sessions: Vec<String> },
    /// Two or more open sessions edited the same file in the last day.
    SameFile { file: String, sessions: Vec<String> },
    /// A worktree with uncommitted or unpushed work and no open session.
    IdleWorktree {
        path: PathBuf,
        changed: usize,
        ahead: u32,
    },
    /// A worktree whose directory is gone.
    MissingWorktree { path: PathBuf },
    /// A spec change with tasks left that nobody touched for a while.
    StaleChange {
        framework: &'static str,
        id: String,
        done: usize,
        total: usize,
        days: u64,
    },
    /// A spec change whose tasks are all done, with the command to wrap it up.
    FinishedChange {
        framework: &'static str,
        id: String,
        next: String,
    },
}

/// Days without changes before a spec change with tasks left counts as stale.
const STALE_CHANGE_DAYS: u64 = 14;

/// Uses per skill, subagent type, MCP server and command.
#[derive(Default)]
pub struct UsageCounts {
    pub skills: HashMap<String, u32>,
    pub agents: HashMap<String, u32>,
    pub mcp_servers: HashMap<String, u32>,
    pub commands: HashMap<String, u32>,
}

impl UsageCounts {
    /// Uses of `name`, whether recorded plain or with a plugin prefix
    /// (`plugin:name`).
    pub fn count(map: &HashMap<String, u32>, name: &str) -> u32 {
        map.iter()
            .filter(|(k, _)| {
                let k = k.trim_start_matches('/');
                k == name || k.rsplit_once(':').is_some_and(|(_, n)| n == name)
            })
            .map(|(_, v)| v)
            .sum()
    }

    /// Everything a plugin contributed: its skills, subagents, commands and
    /// MCP servers (`plugin_<name>_<server>`).
    pub fn plugin(&self, plugin: &str) -> u32 {
        let prefix = format!("{plugin}:");
        let by_prefix = |map: &HashMap<String, u32>| -> u32 {
            map.iter()
                .filter(|(k, _)| k.trim_start_matches('/').starts_with(&prefix))
                .map(|(_, v)| v)
                .sum()
        };
        let mcp_prefix = format!("plugin_{plugin}_");
        let mcp: u32 = self
            .mcp_servers
            .iter()
            .filter(|(k, _)| k.starts_with(&mcp_prefix))
            .map(|(_, v)| v)
            .sum();
        by_prefix(&self.skills) + by_prefix(&self.agents) + by_prefix(&self.commands) + mcp
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
    pending_command: Option<(Vec<String>, PathBuf)>,
    /// Open sessions (ID -> "busy"/"idle") from `claude agents --json`.
    pub live: HashMap<String, LiveSession>,
    /// Background sessions, finished ones included (`claude agents --json --all`).
    pub background: Vec<LiveSession>,
    pub activity_focus: ActivityFocus,
    pub background_state: ListState,
    background_job: Option<(String, Job<Result<String, String>>)>,
    pub logs: LogsView,
    live_job: Option<Job<Result<HashMap<String, LiveSession>, String>>>,
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

    // Analysis of every transcript and git state of every session folder.
    pub analyses: analysis::Cache,
    analysis_job: Option<Job<analysis::Cache>>,
    pub git: HashMap<PathBuf, Option<git::Status>>,
    git_job: Option<Job<(ProjectsModel, projects::Statuses)>>,
    pub projects: ProjectsModel,
    pub projects_state: ListState,
    /// In Projects: the spec changes list has the keys instead of the tree.
    pub specs_focus: bool,
    pub specs_state: ListState,
    /// Show only sessions of this exact folder (set from the Projects view).
    pub folder_filter: Option<PathBuf>,
    git_at: Option<Instant>,
    /// Inspector scroll.
    pub inspect_scroll: u16,
    /// Subagent selected in the inspector (index into its analysis' list).
    pub inspect_sub: Option<usize>,
    pub activity_state: ListState,

    // Help.
    pub doctor: Option<Vec<doctor::Check>>,
    doctor_job: Option<Job<Vec<doctor::Check>>>,
    pub help_scroll: u16,
    /// View to return to when the help closes.
    help_from: View,

    // Branch reviews.
    /// Repository main checkout and name, and its branch listing.
    branches_job: Option<(PathBuf, String, BranchesJob)>,
    /// Preparing the worktree, then (static mode) Claude's review.
    review_job: Option<(ReviewTask, ReviewJob)>,
    /// What each session that needs you is waiting on, by session ID, with
    /// the transcript size it was read at.
    pub pending: HashMap<String, (u64, transcript::PendingTool)>,
    /// Re-check the project's MCP servers once the running command ends.
    mcp_recheck: bool,
    /// An interactive review whose session is running in the terminal.
    pending_review: Option<(ReviewTask, PathBuf)>,

    // Prompt history and daily summary.
    pub prompts: Vec<Prompt>,
    summary_job: Option<Job<String>>,

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
            view: View::Sessions,
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
            pending_command: None,
            live: HashMap::new(),
            background: Vec::new(),
            activity_focus: ActivityFocus::Open,
            background_state: ListState::default(),
            background_job: None,
            logs: LogsView::default(),
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
            analyses: analysis::Cache::new(),
            analysis_job: None,
            git: HashMap::new(),
            git_job: None,
            projects: ProjectsModel::default(),
            projects_state: ListState::default(),
            specs_focus: false,
            specs_state: ListState::default(),
            folder_filter: None,
            git_at: None,
            inspect_scroll: 0,
            inspect_sub: None,
            activity_state: ListState::default(),
            doctor: None,
            doctor_job: None,
            help_scroll: 0,
            help_from: View::Sessions,
            prompts: Vec::new(),
            summary_job: None,
            branches_job: None,
            review_job: None,
            pending_review: None,
            mcp_recheck: false,
            pending: HashMap::new(),
            library: Library::load(),
            history: History::load(),
            monthly: false,
        };
        app.doctor_job = Some(Job::spawn(doctor::run));
        app.maybe_welcome();
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
            if let Some((args, cwd)) = self.pending_command.take() {
                self.run_claude(terminal, &args, &cwd)?;
                self.finish_interactive_review();
                if std::mem::take(&mut self.mcp_recheck) {
                    self.recheck_mcp();
                }
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
        } else if self.view == View::Logs && self.logs.follow && self.ticks.is_multiple_of(8) {
            self.load_log();
        } else if self.view == View::Activity && self.ticks.is_multiple_of(8) {
            // Every 2 seconds: re-read what open sessions did.
            self.start_analysis();
            self.reload_hook_states();
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
        self.start_analysis();
        if self.git_at.is_none_or(|t| t.elapsed() >= GIT_EVERY) {
            self.start_git();
        }
        if self.live_job.is_none() {
            self.live_job = Some(Job::spawn(|| claude_cli::live_sessions(true)));
        }
        self.refreshed_at = Instant::now();
    }

    /// Re-analyzes transcripts that changed, in the background.
    fn start_analysis(&mut self) {
        if self.analysis_job.is_some() {
            return;
        }
        // Fresh file metadata: open sessions change between session reloads.
        let targets: Vec<(PathBuf, std::time::SystemTime, u64)> = self
            .sessions
            .iter()
            .map(|s| {
                let meta = std::fs::metadata(&s.path).ok();
                let modified = meta
                    .as_ref()
                    .and_then(|m| m.modified().ok())
                    .unwrap_or(s.modified);
                let size = meta.map_or(s.size, |m| m.len());
                (s.path.clone(), modified, size)
            })
            .collect();
        let previous = self.analyses.clone();
        self.analysis_job = Some(Job::spawn(move || {
            analysis::analyze_all(&targets, &previous)
        }));
    }

    /// Reads git state for every folder a session ran in, in the background.
    fn start_git(&mut self) {
        if self.git_job.is_some() {
            return;
        }
        let mut dirs: Vec<PathBuf> = self
            .sessions
            .iter()
            .filter_map(|s| s.cwd.clone())
            .filter(|d| d.is_dir())
            .collect();
        dirs.sort();
        dirs.dedup();
        self.git_at = Some(Instant::now());
        self.git_job = Some(Job::spawn(move || projects::build(&dirs)));
    }

    /// Open sessions, the ones that need you first, then working, then waiting.
    pub fn open_sessions(&self) -> Vec<&Session> {
        let mut open: Vec<&Session> = self
            .sessions
            .iter()
            .filter(|s| self.live.contains_key(&s.id))
            .collect();
        let rank = |s: &&Session| match self.activity(&s.id) {
            Some(Activity::NeedsYou) => 0,
            Some(Activity::Working) => 1,
            _ => 2,
        };
        open.sort_by_key(|s| (rank(s), std::cmp::Reverse(s.modified)));
        open
    }

    /// Recent tool calls across sessions active in the last hour, newest first.
    pub fn feed(&self, limit: usize) -> Vec<FeedItem<'_>> {
        let hour = Duration::from_secs(3_600);
        let mut items: Vec<FeedItem> = self
            .sessions
            .iter()
            .filter(|s| {
                s.modified.elapsed().is_ok_and(|age| age <= hour) || self.live.contains_key(&s.id)
            })
            .filter_map(|s| self.analysis(s).map(|a| (s, a)))
            .flat_map(|(s, a)| {
                a.recent.iter().map(move |e| FeedItem {
                    session: s,
                    event: e,
                })
            })
            .collect();
        items.sort_by_key(|i| std::cmp::Reverse(i.event.at));
        items.truncate(limit);
        items
    }

    /// Selects a session in the Dashboard list, clearing filters that hide it.
    fn select_session(&mut self, id: &str) {
        if !self.visible.iter().any(|&i| self.sessions[i].id == id) {
            self.filter.clear();
            self.folder_filter = None;
            self.apply_filter(None);
        }
        let index = self.visible.iter().position(|&i| self.sessions[i].id == id);
        self.session_state.select(index);
        self.update_project();
    }

    fn handle_activity_key(&mut self, code: KeyCode) {
        if code == KeyCode::Tab {
            self.activity_focus = match self.activity_focus {
                ActivityFocus::Open if !self.background.is_empty() => {
                    if self.background_state.selected().is_none() {
                        self.background_state.select(Some(0));
                    }
                    ActivityFocus::Background
                }
                _ => ActivityFocus::Open,
            };
            return;
        }
        if self.activity_focus == ActivityFocus::Background {
            match code {
                KeyCode::Esc => self.activity_focus = ActivityFocus::Open,
                KeyCode::Down | KeyCode::Char('j') if !self.background.is_empty() => {
                    let n = self.background.len();
                    let next = self
                        .background_state
                        .selected()
                        .map_or(0, |i| (i + 1).min(n - 1));
                    self.background_state.select(Some(next));
                }
                KeyCode::Up | KeyCode::Char('k') => self.background_state.select_previous(),
                _ => self.handle_background_key(code),
            }
            return;
        }
        let open: Vec<String> = self.open_sessions().iter().map(|s| s.id.clone()).collect();
        let selected = self
            .activity_state
            .selected()
            .and_then(|i| open.get(i))
            .cloned();
        match code {
            KeyCode::Esc => self.view = View::Sessions,
            KeyCode::Down | KeyCode::Char('j') if !open.is_empty() => {
                let next = self
                    .activity_state
                    .selected()
                    .map_or(0, |i| (i + 1).min(open.len() - 1));
                self.activity_state.select(Some(next));
            }
            KeyCode::Up | KeyCode::Char('k') => self.activity_state.select_previous(),
            KeyCode::Enter => {
                if let Some(id) = selected {
                    self.show_pending(&id);
                }
            }
            KeyCode::Char('v') => {
                if let Some(id) = selected {
                    self.select_session(&id);
                    self.open_transcript(&id, None, None);
                }
            }
            KeyCode::Char('i') => {
                if let Some(id) = selected {
                    self.select_session(&id);
                    self.inspect_scroll = 0;
                    self.inspect_sub = None;
                    self.view = View::Inspect;
                }
            }
            _ => {}
        }
    }

    /// Opens a subagent's own conversation.
    fn open_subagent(&mut self) {
        let Some(session) = self.selected_session() else {
            return;
        };
        let Some(sub) = self
            .inspect_sub
            .and_then(|i| self.analysis(session).and_then(|a| a.subagents.get(i)))
            .cloned()
        else {
            return;
        };
        let entries = match transcript::load(&sub.file) {
            Ok(entries) => entries,
            Err(e) => return self.show_flash(format!("Could not read the subagent: {e}"), true),
        };
        let title = if sub.description.is_empty() {
            format!("{} (subagent)", sub.agent_type)
        } else {
            format!("{} · {} (subagent)", sub.description, sub.agent_type)
        };
        self.transcript = Some(TranscriptView {
            session_id: session.id.clone(),
            title,
            path: sub.file.clone(),
            project: session.project_path.clone(),
            branch: session.git_branch.clone(),
            entries,
            show_output: false,
            query: None,
            matches: Vec::new(),
            match_pos: 0,
            scroll: 0,
            at_end: false,
            jump_to: Some(usize::MAX),
            rows: Vec::new(),
            rows_key: None,
        });
        self.view = View::Transcript;
    }

    /// Carries out a confirmed action.
    fn confirmed(&mut self, action: Confirm) {
        match action {
            Confirm::Trash(id) => self.delete_session(&id),
            Confirm::StopBackground(id) => self.start_background_command("stop", &id),
            Confirm::RespawnBackground(id) => self.start_background_command("respawn", &id),
            Confirm::RemoveWorktree { main, path } => {
                match git::remove_worktree(&main, &path) {
                    Ok(()) => self
                        .show_flash(format!("Removed worktree {}", paths::display(&path)), false),
                    Err(e) => self.show_flash(format!("git refused: {e}"), true),
                }
                self.git_at = None;
            }
            Confirm::McpLogout { server, cwd } => {
                if self.background_job.is_some() {
                    return self.show_flash("Another command is running", true);
                }
                let what = format!("mcp logout {server}");
                self.background_job = Some((
                    what,
                    Job::spawn(move || claude_cli::mcp_logout(&server, &cwd)),
                ));
            }
            Confirm::RemoveReviewWorktree { main, path } => {
                match git::remove_worktree(&main, &path) {
                    Ok(()) => self.show_flash("Removed the review worktree", false),
                    Err(e) => self.show_flash(format!("git refused: {e}"), true),
                }
                self.git_at = None;
            }
            Confirm::PruneWorktrees(main) => {
                match git::prune_worktrees(&main) {
                    Ok(()) => self.show_flash("Pruned worktrees whose directory is gone", false),
                    Err(e) => self.show_flash(e, true),
                }
                self.git_at = None;
            }
        }
    }

    /// `claude stop|respawn <id>` in the background; the result shows as a notice.
    fn start_background_command(&mut self, command: &'static str, id: &str) {
        if self.background_job.is_some() {
            return self.show_flash("Another background command is running", true);
        }
        let job_id = id.to_string();
        self.background_job = Some((
            format!("{command} {id}"),
            Job::spawn(move || claude_cli::background(command, &job_id)),
        ));
    }

    pub fn selected_background(&self) -> Option<&LiveSession> {
        self.background_state
            .selected()
            .and_then(|i| self.background.get(i))
    }

    fn handle_background_key(&mut self, code: KeyCode) {
        let Some(bg) = self.selected_background().cloned() else {
            return;
        };
        let Some(id) = bg.id.clone() else {
            return self.show_flash("This background session has no short ID", true);
        };
        match code {
            KeyCode::Char('l') => {
                self.open_logs(Some(LogKind::Background(id)));
            }
            KeyCode::Char('S') => {
                self.popup = Some(Popup::Confirm {
                    title: "Stop background session".into(),
                    lines: vec![
                        format!("Stop \"{}\"?", bg.name.clone().unwrap_or(id.clone())),
                        String::new(),
                        "Its conversation is kept; resume it later with".into(),
                        format!("claude attach {id}."),
                    ],
                    yes: "stop it".into(),
                    action: Confirm::StopBackground(id),
                });
            }
            KeyCode::Char('R') => {
                self.popup = Some(Popup::Confirm {
                    title: "Respawn background session".into(),
                    lines: vec![
                        format!("Respawn \"{}\"?", bg.name.clone().unwrap_or(id.clone())),
                        String::new(),
                        "This runs `claude respawn`: it restarts the session's process".into(),
                        "and continues the same conversation.".into(),
                    ],
                    yes: "respawn it".into(),
                    action: Confirm::RespawnBackground(id),
                });
            }
            KeyCode::Enter => {
                let cwd = bg
                    .cwd
                    .map(PathBuf::from)
                    .filter(|d| d.is_dir())
                    .or_else(dirs::home_dir)
                    .unwrap_or_else(|| PathBuf::from("."));
                self.pending_command = Some((vec!["attach".into(), id], cwd));
            }
            _ => {}
        }
    }

    /// Log sources: the selected project's MCP servers and every background session.
    fn log_sources(&self) -> Vec<LogSource> {
        let mut sources = Vec::new();
        if let Some(project) = &self.project {
            for (server, dir) in mcp::log_dirs(&project.cwd) {
                sources.push(LogSource {
                    label: format!("MCP · {server}"),
                    group: paths::display(&project.cwd),
                    kind: LogKind::Mcp(dir),
                });
            }
        }
        for bg in &self.background {
            if let Some(id) = &bg.id {
                sources.push(LogSource {
                    label: format!(
                        "{} · {id}",
                        bg.name.clone().unwrap_or_else(|| "background".into())
                    ),
                    group: "background sessions".into(),
                    kind: LogKind::Background(id.clone()),
                });
            }
        }
        sources
    }

    /// Opens the Logs view, optionally on a given source.
    fn open_logs(&mut self, select: Option<LogKind>) {
        self.logs.sources = self.log_sources();
        let index = select
            .and_then(|k| self.logs.sources.iter().position(|s| s.kind == k))
            .or((!self.logs.sources.is_empty()).then_some(0));
        self.logs.state.select(index);
        self.logs.loaded = None;
        self.view = View::Logs;
        self.load_log();
    }

    /// Reads the selected log source in the background.
    fn load_log(&mut self) {
        if self.logs.job.is_some() {
            return;
        }
        let Some(source) = self
            .logs
            .state
            .selected()
            .and_then(|i| self.logs.sources.get(i))
            .cloned()
        else {
            return;
        };
        let kind = source.kind.clone();
        self.logs.job = Some((
            kind.clone(),
            Job::spawn(move || match kind {
                LogKind::Mcp(dir) => mcp::read_log_dir(&dir).map(|(_, lines)| lines),
                LogKind::Background(id) => claude_cli::background("logs", &id)
                    .map(|text| text.lines().map(str::to_owned).collect()),
            }),
        ));
        self.logs.loaded_at = Some(Instant::now());
    }

    fn handle_logs_key(&mut self, code: KeyCode) {
        let n = self.logs.sources.len();
        match code {
            KeyCode::Esc => self.view = View::Sessions,
            KeyCode::Down | KeyCode::Char('j') if n > 0 => {
                let next = self.logs.state.selected().map_or(0, |i| (i + 1).min(n - 1));
                self.logs.state.select(Some(next));
                self.logs.loaded = None;
                self.logs.scroll = usize::MAX;
                self.load_log();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.logs.state.select_previous();
                self.logs.loaded = None;
                self.logs.scroll = usize::MAX;
                self.load_log();
            }
            KeyCode::PageUp => {
                self.logs.follow = false;
                self.logs.scroll = self.logs.scroll.saturating_sub(15);
            }
            KeyCode::PageDown => self.logs.scroll = self.logs.scroll.saturating_add(15),
            KeyCode::End | KeyCode::Char('G') => {
                self.logs.follow = true;
                self.logs.scroll = usize::MAX;
            }
            KeyCode::Char('x') => self.logs.errors_only = !self.logs.errors_only,
            KeyCode::Char('/') => {
                let text = self.logs.filter.clone();
                self.input = Some(Input::LogFilter { text });
            }
            _ => {}
        }
    }

    /// Rows of the Projects view: each repository followed by its checkouts,
    /// then folders outside git.
    pub fn project_rows(&self) -> Vec<ProjectRow> {
        let mut rows = Vec::new();
        for (r, repo) in self.projects.repos.iter().enumerate() {
            rows.push(ProjectRow::Repo(r));
            rows.extend((0..repo.checkouts.len()).map(|c| ProjectRow::Checkout(r, c)));
        }
        rows.extend((0..self.projects.loose.len()).map(ProjectRow::Loose));
        rows
    }

    /// Folder of a Projects row, when it has one.
    pub fn row_folder(&self, row: ProjectRow) -> Option<&Path> {
        match row {
            ProjectRow::Repo(r) => self.projects.repos[r]
                .checkouts
                .first()
                .map(|c| c.path.as_path()),
            ProjectRow::Checkout(r, c) => Some(&self.projects.repos[r].checkouts[c].path),
            ProjectRow::Loose(l) => Some(&self.projects.loose[l]),
        }
    }

    /// Sessions that ran in `dir`, newest first.
    pub fn sessions_in(&self, dir: &Path) -> Vec<&Session> {
        self.sessions
            .iter()
            .filter(|s| s.cwd.as_deref() == Some(dir))
            .collect()
    }

    /// Things worth your attention across projects.
    pub fn problems(&self) -> Vec<Problem> {
        let mut problems = Vec::new();
        let title = |id: &str| {
            self.sessions
                .iter()
                .find(|s| s.id == id)
                .map(|s| s.title.clone())
                .unwrap_or_else(|| id.chars().take(8).collect())
        };
        let open: Vec<&Session> = self
            .sessions
            .iter()
            .filter(|s| self.live.contains_key(&s.id))
            .collect();

        let mut by_folder: BTreeMap<&Path, Vec<String>> = BTreeMap::new();
        for s in &open {
            if let Some(dir) = &s.cwd {
                by_folder.entry(dir).or_default().push(title(&s.id));
            }
        }
        for (dir, sessions) in by_folder.into_iter().filter(|(_, v)| v.len() >= 2) {
            problems.push(Problem::SharedFolder {
                dir: dir.to_path_buf(),
                sessions,
            });
        }

        // The same file edited by two open sessions in the last day.
        let recent = chrono::Local::now() - chrono::Duration::hours(24);
        let mut by_file: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        for s in &open {
            let Some(a) = self.analysis(s) else {
                continue;
            };
            for (file, at) in &a.edits {
                if at.is_none_or(|t| t >= recent) {
                    by_file.entry(file).or_default().push(title(&s.id));
                }
            }
        }
        for (file, sessions) in by_file.into_iter().filter(|(_, v)| v.len() >= 2) {
            problems.push(Problem::SameFile {
                file: file.to_string(),
                sessions,
            });
        }

        // Spec changes: once per change id, even when worktrees repeat it.
        let mut seen = HashSet::new();
        for specs in self.projects.specs.values() {
            for c in &specs.changes {
                if !seen.insert((c.framework, c.id.clone())) {
                    continue;
                }
                let days = c.modified.elapsed().map_or(0, |d| d.as_secs() / 86_400);
                match c.stage {
                    specs::Stage::Implementing if days >= STALE_CHANGE_DAYS => {
                        problems.push(Problem::StaleChange {
                            framework: c.framework.title(),
                            id: c.id.clone(),
                            done: c.done,
                            total: c.total,
                            days,
                        })
                    }
                    specs::Stage::Complete if c.next.is_some() => {
                        problems.push(Problem::FinishedChange {
                            framework: c.framework.title(),
                            id: c.id.clone(),
                            next: c.next.clone().unwrap_or_default(),
                        })
                    }
                    _ => {}
                }
            }
        }

        for repo in &self.projects.repos {
            for c in repo.checkouts.iter().filter(|c| !c.main) {
                if c.prunable {
                    problems.push(Problem::MissingWorktree {
                        path: c.path.clone(),
                    });
                } else if c.has_work() && self.open_in_folder(&c.path) == 0 {
                    let s = c.status.as_ref();
                    problems.push(Problem::IdleWorktree {
                        path: c.path.clone(),
                        changed: s.map_or(0, |s| s.changed),
                        ahead: s.map_or(0, |s| s.ahead),
                    });
                }
            }
        }
        problems
    }

    /// The selected Projects row's folder and its spec changes, if any.
    pub fn selected_specs(&self) -> Option<(&Path, &specs::ProjectSpecs)> {
        let rows = self.project_rows();
        let row = self.projects_state.selected().and_then(|i| rows.get(i))?;
        let dir = self.row_folder(*row)?;
        self.projects.specs.get(dir).map(|s| (dir, s))
    }

    /// Sessions that worked on a change: they ran a command naming it, read or
    /// wrote its files, or ran on its branch (spec-kit names branches after
    /// features).
    pub fn change_sessions(&self, dir: &Path, change: &specs::Change) -> Vec<&Session> {
        let folder = change.path.to_string_lossy();
        self.sessions
            .iter()
            .filter(|s| {
                s.cwd
                    .as_deref()
                    .is_some_and(|c| c.starts_with(dir) || dir.starts_with(c))
            })
            .filter(|s| {
                s.git_branch.as_deref() == Some(change.id.as_str())
                    || self.analysis(s).is_some_and(|a| {
                        a.touched.iter().any(|f| f.starts_with(folder.as_ref()))
                            || a.invocations
                                .iter()
                                .any(|i| i.split_whitespace().skip(1).any(|w| w == change.id))
                    })
            })
            .collect()
    }

    fn handle_specs_key(&mut self, code: KeyCode) {
        let Some((dir, specs)) = self.selected_specs() else {
            self.specs_focus = false;
            return;
        };
        let n = specs.changes.len();
        let change = self
            .specs_state
            .selected()
            .and_then(|i| specs.changes.get(i))
            .cloned();
        let dir = dir.to_path_buf();
        match code {
            KeyCode::Tab | KeyCode::Esc => self.specs_focus = false,
            KeyCode::Down | KeyCode::Char('j') if n > 0 => {
                let next = self
                    .specs_state
                    .selected()
                    .map_or(0, |i| (i + 1).min(n - 1));
                self.specs_state.select(Some(next));
            }
            KeyCode::Up | KeyCode::Char('k') => self.specs_state.select_previous(),
            KeyCode::Enter => {
                let Some(change) = change else {
                    return;
                };
                let Some(next) = change.next.clone() else {
                    return self.show_flash(
                        format!(
                            "No next step to run: {} drives it, or this project lacks its command",
                            change.framework.title()
                        ),
                        false,
                    );
                };
                // spec-kit's commands act on the feature of the current branch.
                if change.framework == specs::Framework::SpecKit {
                    let branch = git::status(&dir).and_then(|s| s.branch);
                    if branch.as_deref() != Some(change.id.as_str()) {
                        return self.show_flash(
                            format!(
                                "spec-kit works on the current branch's feature; switch to branch {} first",
                                change.id
                            ),
                            true,
                        );
                    }
                }
                self.pending_command = Some((vec![next], dir));
            }
            KeyCode::Char('v') => {
                let Some(change) = change else {
                    return;
                };
                let mut lines = Vec::new();
                for file in specs::files(&change) {
                    lines.push(format!("── {} ──", paths::display(&file)));
                    lines.extend(
                        std::fs::read_to_string(&file)
                            .unwrap_or_default()
                            .lines()
                            .map(str::to_owned),
                    );
                    lines.push(String::new());
                }
                self.popup = Some(Popup::Text {
                    title: format!(" {} · {} ", change.framework.title(), change.id),
                    lines,
                    scroll: 0,
                });
            }
            _ => {}
        }
    }

    fn handle_projects_key(&mut self, code: KeyCode) {
        if self.specs_focus {
            return self.handle_specs_key(code);
        }
        if code == KeyCode::Tab {
            if self
                .selected_specs()
                .is_some_and(|(_, s)| !s.changes.is_empty())
            {
                self.specs_focus = true;
                if self.specs_state.selected().is_none() {
                    self.specs_state.select(Some(0));
                }
            }
            return;
        }
        let rows = self.project_rows();
        let row = self
            .projects_state
            .selected()
            .and_then(|i| rows.get(i))
            .copied();
        match code {
            KeyCode::Char('b') => {
                let (r, c) = match row {
                    Some(ProjectRow::Repo(r)) => (r, 0),
                    Some(ProjectRow::Checkout(r, c)) => (r, c),
                    _ => return self.show_flash("Select a repository first", true),
                };
                self.open_branches(r, c);
            }
            KeyCode::Char('D') => {
                let Some(ProjectRow::Checkout(r, c)) = row else {
                    return;
                };
                let repo = &self.projects.repos[r];
                let co = &repo.checkouts[c];
                if co.main {
                    return self.show_flash("That's the main checkout, not a worktree", true);
                }
                if self.open_in_folder(&co.path) > 0 {
                    return self
                        .show_flash("A session is open in this worktree; close it first", true);
                }
                let mut lines = vec![
                    format!("Remove the worktree {}?", paths::display(&co.path)),
                    String::new(),
                    "This runs `git worktree remove` without --force: git refuses when".into(),
                    "it has uncommitted changes or untracked files, or is locked.".into(),
                ];
                if co.status.as_ref().is_some_and(|s| s.ahead > 0) {
                    lines.push(String::new());
                    lines
                        .push("Its branch has unpushed commits; the branch itself is kept.".into());
                }
                self.popup = Some(Popup::Confirm {
                    title: "Remove worktree".into(),
                    lines,
                    yes: "remove".into(),
                    action: Confirm::RemoveWorktree {
                        main: repo.checkouts[0].path.clone(),
                        path: co.path.clone(),
                    },
                });
            }
            KeyCode::Char('P') => {
                let r = match row {
                    Some(ProjectRow::Repo(r) | ProjectRow::Checkout(r, _)) => r,
                    _ => return,
                };
                let repo = &self.projects.repos[r];
                let missing = repo.checkouts.iter().filter(|c| c.prunable).count();
                if missing == 0 {
                    return self.show_flash("No missing worktrees in this repository", false);
                }
                self.popup = Some(Popup::Confirm {
                    title: "Prune worktrees".into(),
                    lines: vec![
                        format!("Drop {missing} worktree record(s) whose directory is gone?"),
                        String::new(),
                        "This runs `git worktree prune`; no files are touched.".into(),
                    ],
                    yes: "prune".into(),
                    action: Confirm::PruneWorktrees(repo.checkouts[0].path.clone()),
                });
            }
            KeyCode::Esc => self.view = View::Sessions,
            KeyCode::Down | KeyCode::Char('j') if !rows.is_empty() => {
                let next = self
                    .projects_state
                    .selected()
                    .map_or(0, |i| (i + 1).min(rows.len() - 1));
                self.projects_state.select(Some(next));
            }
            KeyCode::Up | KeyCode::Char('k') => self.projects_state.select_previous(),
            KeyCode::Enter => {
                let Some(dir) = self
                    .projects_state
                    .selected()
                    .and_then(|i| rows.get(i))
                    .and_then(|&row| self.row_folder(row))
                    .map(Path::to_path_buf)
                else {
                    return;
                };
                self.folder_filter = Some(dir);
                self.filter.clear();
                self.apply_filter(None);
                self.view = View::Sessions;
                self.focus = Focus::Sessions;
            }
            _ => {}
        }
    }

    pub fn analysis(&self, session: &Session) -> Option<&Analysis> {
        self.analyses.get(&session.path).map(|(_, _, a)| a.as_ref())
    }

    pub fn git_status(&self, dir: &Path) -> Option<&git::Status> {
        self.git.get(dir).and_then(Option::as_ref)
    }

    /// How many open sessions work in `dir` (the same checkout).
    pub fn open_in_folder(&self, dir: &Path) -> usize {
        self.live
            .keys()
            .filter_map(|id| self.sessions.iter().find(|s| &s.id == id))
            .filter(|s| s.cwd.as_deref() == Some(dir))
            .count()
    }

    /// Median context size of the first request in sessions of `dir`: what a
    /// session there carries before any work (system prompt, tools, instructions).
    pub fn baseline_context(&self, dir: &Path) -> Option<(u64, usize)> {
        let mut firsts: Vec<u64> = self
            .sessions
            .iter()
            .filter(|s| s.cwd.as_deref() == Some(dir))
            .filter_map(|s| self.analysis(s).and_then(Analysis::first_context))
            .collect();
        if firsts.is_empty() {
            return None;
        }
        firsts.sort_unstable();
        Some((firsts[firsts.len() / 2], firsts.len()))
    }

    /// Uses of skills, subagent types, MCP servers and commands in sessions
    /// active during the last [`USAGE_WINDOW_DAYS`].
    pub fn recent_usage(&self) -> UsageCounts {
        let window = Duration::from_secs(USAGE_WINDOW_DAYS * 86_400);
        let mut counts = UsageCounts::default();
        for s in &self.sessions {
            if s.modified.elapsed().is_ok_and(|age| age > window) {
                continue;
            }
            let Some(a) = self.analysis(s) else {
                continue;
            };
            for (map, into) in [
                (&a.skills, &mut counts.skills),
                (&a.agents, &mut counts.agents),
                (&a.mcp_servers, &mut counts.mcp_servers),
                (&a.commands, &mut counts.commands),
            ] {
                for (k, v) in map {
                    *into.entry(k.clone()).or_default() += v;
                }
            }
        }
        counts
    }

    pub fn analyzing(&self) -> bool {
        self.analysis_job.is_some() && self.analyses.is_empty()
    }

    fn reload_hook_states(&mut self) {
        (self.hook_states, self.hooks_configured) = hooks::load();
        self.check_activity_changes();
        self.update_pending();
    }

    /// Reads what sessions that need you are waiting on (the end of their
    /// transcript, again only when it grew).
    fn update_pending(&mut self) {
        let waiting: Vec<(String, PathBuf, u64)> = self
            .sessions
            .iter()
            .filter(|s| self.activity(&s.id) == Some(Activity::NeedsYou))
            .map(|s| {
                let size = std::fs::metadata(&s.path).map_or(0, |m| m.len());
                (s.id.clone(), s.path.clone(), size)
            })
            .collect();
        self.pending
            .retain(|id, _| waiting.iter().any(|(w, _, _)| w == id));
        for (id, path, size) in waiting {
            if self.pending.get(&id).is_some_and(|(read, _)| *read == size) {
                continue;
            }
            match transcript::pending_tool(&path) {
                Some(tool) => {
                    self.pending.insert(id, (size, tool));
                }
                None => {
                    self.pending.remove(&id);
                }
            }
        }
    }

    pub fn pending_tool(&self, id: &str) -> Option<&transcript::PendingTool> {
        self.pending.get(id).map(|(_, tool)| tool)
    }

    /// Shows in full what a session that needs you is asking to do.
    fn show_pending(&mut self, id: &str) {
        let Some(tool) = self.pending_tool(id).cloned() else {
            return self.show_flash("It isn't waiting on a tool call", false);
        };
        let title = self
            .sessions
            .iter()
            .find(|s| s.id == id)
            .map_or(String::new(), |s| s.title.clone());
        let mut lines = vec![
            format!("{title} wants to use {}:", tool.name),
            String::new(),
        ];
        lines.extend(transcript::describe(&tool));
        lines.push(String::new());
        lines.push("Answer it in the session's own terminal.".into());
        self.popup = Some(Popup::Text {
            title: " What it's asking ".into(),
            lines,
            scroll: 0,
        });
    }

    /// What an open session is doing: from its hooks when set up, otherwise
    /// from `claude agents --json`. `None` for sessions that aren't open.
    pub fn activity(&self, id: &str) -> Option<Activity> {
        let live = self.live.get(id)?;
        Some(hooks::combine(live, self.hook_states.get(id)))
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
                self.folder_filter
                    .as_ref()
                    .is_none_or(|dir| s.cwd.as_ref() == Some(dir))
            })
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
            Ok(cwd) => {
                self.pending_command = Some((vec!["--resume".into(), id], cwd.to_path_buf()))
            }
            Err(msg) => self.show_flash(msg, true),
        }
    }

    /// Suspends the TUI, runs `claude <args>` (resume, attach) in `cwd` and
    /// returns to the dashboard when it exits.
    fn run_claude(
        &mut self,
        terminal: &mut DefaultTerminal,
        args: &[String],
        cwd: &Path,
    ) -> io::Result<()> {
        ratatui::restore();
        let status = claude_cli::command().args(args).current_dir(cwd).status();
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
        self.popup = Some(Popup::Confirm {
            title: "Move to trash".into(),
            lines: vec![
                format!("Move \"{}\" to the trash?", session.title),
                String::new(),
                "It moves with its subagents and checkpoints;".into(),
                "press T to restore it within 30 days.".into(),
            ],
            yes: "move to trash".into(),
            action: Confirm::Trash(session.id.clone()),
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

    /// The first time claudash opens without the status line or hooks connected,
    /// explain what it is and what `claudash setup` adds. Shown once.
    fn maybe_welcome(&mut self) {
        let Some(marker) = library::data_dir().map(|d| d.join("welcomed")) else {
            return;
        };
        if marker.exists() {
            return;
        }
        let (statusline, hooks) = crate::setup::installed();
        if statusline && hooks == hooks::EVENTS.len() {
            return;
        }
        let lines = [
            "claudash is a control panel for Claude Code: every session on this machine,",
            "what's open and what needs you, each project's instructions and MCP servers,",
            "your plan usage, conversations, plugins and more, on one screen.",
            "",
            "It can also act for you: resume or prompt a session, tag and note sessions,",
            "move old ones to a trash, search every conversation, toggle plugins.",
            "",
            "To get alerts when a session needs you, plan usage and cache diagnostics,",
            "connect it to Claude Code once from a terminal:",
            "",
            "    claudash setup           shows what it would change",
            "    claudash setup --apply   makes the change (backs up settings.json first)",
            "",
            "Press : to find any action by name, ? for everything claudash can do.",
            "Esc closes this.",
        ];
        self.popup = Some(Popup::Text {
            title: " Welcome to claudash ".into(),
            lines: lines.iter().map(|l| l.to_string()).collect(),
            scroll: 0,
        });
        if let Some(dir) = marker.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(marker, "");
    }

    // ---- Branch reviews ----------------------------------------------------

    /// `b` in Projects: fetches and lists the repository's branches.
    /// `checkout` is the one selected, whose own branch is offered first.
    fn open_branches(&mut self, repo_index: usize, checkout: usize) {
        if self.branches_job.is_some() {
            return;
        }
        let repo = &self.projects.repos[repo_index];
        let main = repo.checkouts[0].path.clone();
        let mine = repo.checkouts[checkout].path.clone();
        let name = repo.name.clone();
        let dir = main.clone();
        self.branches_job = Some((main, name, Job::spawn(move || review::list(&dir, &mine))));
    }

    pub fn fetching_branches(&self) -> bool {
        self.branches_job.is_some()
    }

    pub fn reviewing(&self) -> Option<&str> {
        self.review_job
            .as_ref()
            .filter(|(task, _)| task.mode == Mode::Static)
            .map(|(task, _)| task.branch.as_str())
    }

    /// Enter on a branch: its size against the base, and how to review it.
    fn choose_branch(&mut self, repo: PathBuf, repo_name: String, base: String, branch: &Branch) {
        let stat = review::diff_stat(&repo, &base, &branch.reference);
        let last = review::saved(&repo)
            .into_iter()
            .find(|r| r.branch == branch.name);
        self.popup = Some(Popup::ReviewSetup {
            task: ReviewTask {
                repo,
                repo_name,
                branch: branch.name.clone(),
                reference: branch.reference.clone(),
                base,
                mode: Mode::Static,
                session_id: uuid::Uuid::new_v4().to_string(),
            },
            stat,
            last,
            choice: 0,
        });
    }

    /// Makes the worktree and, in static mode, runs the review, off the UI thread.
    fn start_review(&mut self, task: ReviewTask) {
        if self.review_job.is_some() {
            return self.show_flash("A review is already running", true);
        }
        let t = task.clone();
        let job = Job::spawn(move || {
            let worktree =
                review::prepare_worktree(&t.repo, &t.repo_name, &t.branch, &t.reference)?;
            if t.mode != Mode::Static {
                return Ok((worktree, None));
            }
            review::run_static(&worktree, &t.branch, &t.base, &t.session_id)
                .map(|findings| (worktree, Some(findings)))
        });
        if task.mode == Mode::Static {
            self.show_flash(
                format!("Reviewing {} in the background…", task.branch),
                false,
            );
        }
        self.review_job = Some((task, job));
    }

    /// Saves a finished review and shows its findings.
    fn show_review(&mut self, task: &ReviewTask, worktree: PathBuf, result: Findings) {
        let review = Review {
            session_id: task.session_id.clone(),
            repo: task.repo.clone(),
            branch: task.branch.clone(),
            base: task.base.clone(),
            worktree,
            mode: task.mode,
            at: chrono::Utc::now().timestamp(),
            result,
        };
        if let Err(e) = review::save(&review) {
            self.show_flash(format!("Could not save the review: {e}"), true);
        }
        self.popup = Some(Popup::Findings {
            review,
            state: ListState::default().with_selected(Some(0)),
            detail: None,
        });
        self.git_at = None;
    }

    /// After an interactive review session: its findings, from its last reply.
    fn finish_interactive_review(&mut self) {
        let Some((task, worktree)) = self.pending_review.take() else {
            return;
        };
        let Some(path) = self
            .sessions
            .iter()
            .find(|s| s.id == task.session_id)
            .map(|s| s.path.clone())
        else {
            return self.show_flash("The review session wasn't saved; nothing to list", true);
        };
        let replies = transcript::load(&path).unwrap_or_default();
        let found = replies
            .iter()
            .rev()
            .filter(|e| e.kind == transcript::Kind::Assistant)
            .find_map(|e| review::from_reply(&e.text));
        match found {
            Some(result) => self.show_review(&task, worktree, result),
            None => self.show_flash(
                "No review found in the session's replies; read it with v in Sessions",
                true,
            ),
        }
    }

    fn copy_finding(&mut self, review: &Review, i: usize) {
        let Some(f) = review.result.findings.get(i) else {
            return;
        };
        let place = f.line.map_or(f.file.clone(), |l| format!("{}:{l}", f.file));
        let mut text = format!("{place}\n{}", f.comment);
        if let Some(s) = f.suggestion.as_deref().filter(|s| !s.trim().is_empty()) {
            text.push_str(&format!("\n\n```suggestion\n{}\n```", s.trim_end()));
        }
        match prompts::copy_to_clipboard(&text) {
            Ok(()) => self.show_flash(format!("Copied the comment on {place}"), false),
            Err(e) => self.show_flash(format!("Could not copy: {e}"), true),
        }
    }

    fn export_review(&mut self, review: &Review) {
        let result = exports_dir().and_then(|dir| {
            let path = dir.join(format!(
                "{}-review-{}.md",
                chrono::Local::now().format("%Y-%m-%d"),
                review.branch.replace('/', "-")
            ));
            std::fs::write(&path, review::markdown(review)).map(|()| path)
        });
        match result {
            Ok(path) => self.show_flash(format!("Exported to {}", paths::display(&path)), false),
            Err(e) => self.show_flash(format!("Could not export: {e}"), true),
        }
    }

    /// Shows a view, doing what it needs when it opens.
    pub fn switch_view(&mut self, view: View) {
        match view {
            View::Logs => self.open_logs(None),
            View::Activity => {
                self.view = View::Activity;
                self.start_analysis();
            }
            _ => self.view = view,
        }
    }

    /// Where keys go right now, as named in `keys::BINDINGS`.
    pub fn context(&self) -> Context {
        match self.view {
            View::Sessions if self.focus == Focus::Mcp => Context::Mcp,
            View::Sessions => Context::Sessions,
            View::Activity if self.activity_focus == ActivityFocus::Background => {
                Context::Background
            }
            View::Activity => Context::Activity,
            View::Projects if self.specs_focus => Context::Specs,
            View::Projects => Context::Projects,
            View::Logs => Context::Logs,
            View::Usage => Context::Usage,
            View::Ecosystem => Context::Ecosystem,
            View::Inspect => Context::Inspect,
            View::Transcript => Context::Conversation,
            View::Help => Context::Help,
        }
    }

    /// `:` or Ctrl+P: every action that makes sense from here, found by name.
    fn open_palette(&mut self) {
        let mut available = vec![
            Context::Sessions,
            Context::Activity,
            Context::Projects,
            Context::Logs,
            Context::Usage,
            Context::Ecosystem,
        ];
        if self.project_servers().is_some_and(|s| !s.is_empty()) {
            available.push(Context::Mcp);
        }
        if !self.background.is_empty() {
            available.push(Context::Background);
        }
        if self.view == View::Projects
            && self
                .selected_specs()
                .is_some_and(|(_, s)| !s.changes.is_empty())
        {
            available.push(Context::Specs);
        }
        let commands = keys::commands(self.context(), &available);
        let matches = (0..commands.len()).collect();
        self.popup = Some(Popup::Palette {
            commands,
            query: String::new(),
            matches,
            state: ListState::default().with_selected(Some(0)),
        });
    }

    /// Runs a command from the palette: goes where its key works and presses it.
    fn run_command(&mut self, binding: &Binding) {
        let Some(code) = binding.press else {
            return;
        };
        if binding.context != self.context() {
            match binding.context {
                Context::Sessions => {
                    self.view = View::Sessions;
                    self.focus = Focus::Sessions;
                }
                Context::Mcp => {
                    self.view = View::Sessions;
                    self.focus = Focus::Mcp;
                    if self.mcp_state.selected().is_none() {
                        self.mcp_state.select(Some(0));
                    }
                }
                Context::Activity => {
                    self.switch_view(View::Activity);
                    self.activity_focus = ActivityFocus::Open;
                }
                Context::Background => {
                    self.switch_view(View::Activity);
                    self.activity_focus = ActivityFocus::Background;
                    if self.background_state.selected().is_none() {
                        self.background_state.select(Some(0));
                    }
                }
                Context::Projects => {
                    self.switch_view(View::Projects);
                    self.specs_focus = false;
                }
                Context::Specs => {
                    self.switch_view(View::Projects);
                    self.specs_focus = true;
                    if self.specs_state.selected().is_none() {
                        self.specs_state.select(Some(0));
                    }
                }
                Context::Logs => self.switch_view(View::Logs),
                Context::Usage => self.switch_view(View::Usage),
                Context::Ecosystem => self.switch_view(View::Ecosystem),
                Context::Global | Context::Inspect | Context::Conversation | Context::Help => {}
            }
        }
        self.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn toggle_help(&mut self) {
        if self.view == View::Help {
            self.view = self.help_from;
        } else {
            self.help_from = self.view;
            self.help_scroll = 0;
            self.view = View::Help;
        }
    }

    fn open_prompts(&mut self) {
        self.prompts = prompts::load();
        let matches = prompts::search(&self.prompts, "");
        let mut state = ListState::default();
        state.select((!matches.is_empty()).then_some(0));
        self.popup = Some(Popup::Prompts {
            query: String::new(),
            matches,
            state,
        });
    }

    /// Builds today's summary in the background; it opens when ready.
    fn start_summary(&mut self) {
        if self.summary_job.is_some() {
            return;
        }
        let today = chrono::Local::now().date_naive();
        let inputs = crate::summary::inputs(&self.sessions, today);
        self.summary_job = Some(Job::spawn(move || crate::summary::build(inputs, today)));
    }

    pub fn summarizing(&self) -> bool {
        self.summary_job.is_some()
    }

    /// Writes today's summary to the exports folder.
    fn export_summary(&mut self, markdown: &str) {
        let result = exports_dir().and_then(|dir| {
            let path = dir.join(format!(
                "{}-claude-code-summary.md",
                chrono::Local::now().format("%Y-%m-%d")
            ));
            std::fs::write(&path, markdown).map(|()| path)
        });
        match result {
            Ok(path) => self.show_flash(format!("Exported to {}", paths::display(&path)), false),
            Err(e) => self.show_flash(format!("Could not export: {e}"), true),
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

    /// Checks the selected project's MCP servers again, now.
    fn recheck_mcp(&mut self) {
        if let Some(cwd) = self.project.as_ref().map(|p| p.cwd.clone()) {
            self.mcp_cache.remove(&cwd);
        }
        self.skip_mcp_debounce();
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
        if let Some((repo, name, job)) = &self.branches_job
            && let Some(result) = job.poll()
        {
            let (repo, repo_name) = (repo.clone(), name.clone());
            self.branches_job = None;
            match result.unwrap_or_else(|()| Err("No result".into())) {
                Ok(listing) => {
                    if let Some(e) = &listing.fetch_error {
                        self.show_flash(
                            format!("git fetch failed, showing known branches: {e}"),
                            true,
                        );
                    }
                    let reviewed = review::saved(&repo)
                        .into_iter()
                        .rev()
                        .map(|r| (r.branch, r.at))
                        .collect();
                    let matches = (0..listing.branches.len()).collect();
                    self.popup = Some(Popup::Branches {
                        repo,
                        repo_name,
                        listing,
                        reviewed,
                        query: String::new(),
                        matches,
                        state: ListState::default().with_selected(Some(0)),
                    });
                }
                Err(e) => self.show_flash(e, true),
            }
        }

        if let Some((task, job)) = &self.review_job
            && let Some(result) = job.poll()
        {
            let task = task.clone();
            self.review_job = None;
            match result.unwrap_or_else(|()| Err("No result".into())) {
                Ok((worktree, Some(findings))) => {
                    self.alert(
                        "Review ready",
                        &format!("{} has been reviewed", task.branch),
                    );
                    self.reload_sessions();
                    self.show_review(&task, worktree, findings);
                }
                Ok((worktree, None)) => {
                    let args = review::interactive_args(
                        task.mode,
                        &task.branch,
                        &task.base,
                        &task.session_id,
                    );
                    self.pending_command = Some((args, worktree.clone()));
                    self.pending_review = Some((task, worktree));
                }
                Err(e) => self.show_flash(format!("Review of {}: {e}", task.branch), true),
            }
        }

        if let Some(job) = &self.summary_job
            && let Some(result) = job.poll()
        {
            self.summary_job = None;
            match result {
                Ok(markdown) => {
                    self.popup = Some(Popup::Summary {
                        markdown,
                        scroll: 0,
                    })
                }
                Err(()) => self.show_flash("Could not build the summary", true),
            }
        }

        if let Some((what, job)) = &self.background_job
            && let Some(result) = job.poll()
        {
            let what = what.clone();
            self.background_job = None;
            match result.unwrap_or_else(|()| Err("No result".into())) {
                Ok(_) => self.show_flash(format!("claude {what}: done"), false),
                Err(e) => self.show_flash(format!("claude {what}: {e}"), true),
            }
            if what.starts_with("mcp ") {
                self.recheck_mcp();
            }
            self.live_job = None;
            self.refreshed_at = Instant::now() - REFRESH_EVERY;
        }
        if let Some((kind, job)) = &self.logs.job
            && let Some(result) = job.poll()
        {
            let kind = kind.clone();
            self.logs.job = None;
            let selected = self
                .logs
                .state
                .selected()
                .and_then(|i| self.logs.sources.get(i));
            if selected.is_some_and(|s| s.kind == kind) {
                self.logs.lines = result.unwrap_or_else(|()| Err("No result".into()));
                self.logs.loaded = Some(kind);
            }
        }

        if let Some(job) = &self.analysis_job
            && let Some(result) = job.poll()
        {
            if let Ok(cache) = result {
                self.analyses = cache;
            }
            self.analysis_job = None;
        }
        if let Some(job) = &self.git_job
            && let Some(result) = job.poll()
        {
            if let Ok((model, git)) = result {
                self.projects = model;
                self.git = git;
                let rows = self.project_rows().len();
                if self.projects_state.selected().is_none_or(|i| i >= rows) {
                    self.projects_state.select((rows > 0).then_some(0));
                }
            }
            self.git_job = None;
        }

        if let Some(job) = &self.doctor_job
            && let Some(result) = job.poll()
        {
            self.doctor = result.ok();
            self.doctor_job = None;
        }

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
            let all = result.ok().and_then(Result::ok).unwrap_or_default();
            let mut background: Vec<LiveSession> = all
                .values()
                .filter(|s| s.is_background())
                .cloned()
                .collect();
            background.sort_by(|a, b| a.name.cmp(&b.name));
            self.background = background;
            // Open means a process is running (finished background sessions have none).
            self.live = all.into_iter().filter(|(_, s)| s.is_running()).collect();
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
        if self.doctor_job.is_none() {
            self.doctor_job = Some(Job::spawn(doctor::run));
        }
        self.git_at = None;
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
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('p') {
            return self.open_palette();
        }
        // Keys that work everywhere; see `keys::BINDINGS`.
        match key.code {
            KeyCode::Char('q') => return self.should_quit = true,
            KeyCode::Char(c @ '1'..='6') => {
                return self.switch_view(VIEW_KEYS[c as usize - '1' as usize]);
            }
            KeyCode::Char(':') => return self.open_palette(),
            KeyCode::Char('f') => {
                self.input = Some(Input::FindAll {
                    text: String::new(),
                });
                return;
            }
            KeyCode::Char('h') => return self.open_prompts(),
            KeyCode::Char('s') => return self.start_summary(),
            KeyCode::Char('r') => return self.refresh_all(),
            KeyCode::Char('?') => return self.toggle_help(),
            _ => {}
        }
        match self.view {
            View::Sessions => match self.focus {
                Focus::Sessions => self.handle_sessions_key(key.code),
                Focus::Mcp => self.handle_mcp_key(key.code),
            },
            View::Ecosystem => self.handle_eco_key(key.code),
            View::Usage => match key.code {
                KeyCode::Esc => self.view = View::Sessions,
                KeyCode::Char('m') => self.monthly = !self.monthly,
                _ => {}
            },
            View::Transcript => self.handle_transcript_key(key.code),
            View::Projects => self.handle_projects_key(key.code),
            View::Activity => self.handle_activity_key(key.code),
            View::Logs => self.handle_logs_key(key.code),
            View::Inspect => match key.code {
                KeyCode::Esc => self.view = View::Sessions,
                KeyCode::Down | KeyCode::Char('j') => {
                    self.inspect_scroll = self.inspect_scroll.saturating_add(1)
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.inspect_scroll = self.inspect_scroll.saturating_sub(1)
                }
                KeyCode::PageDown | KeyCode::Char(' ') => {
                    self.inspect_scroll = self.inspect_scroll.saturating_add(15)
                }
                KeyCode::PageUp => self.inspect_scroll = self.inspect_scroll.saturating_sub(15),
                KeyCode::Char('v') => {
                    if let Some(id) = self.selected_session().map(|s| s.id.clone()) {
                        self.open_transcript(&id, None, None);
                    }
                }
                KeyCode::Tab | KeyCode::BackTab => {
                    let n = self
                        .selected_session()
                        .and_then(|s| self.analysis(s))
                        .map_or(0, |a| a.subagents.len());
                    if n > 0 {
                        let forward = key.code == KeyCode::Tab;
                        self.inspect_sub = Some(match (self.inspect_sub, forward) {
                            (None, true) => 0,
                            (None, false) => n - 1,
                            (Some(i), true) => (i + 1) % n,
                            (Some(i), false) => (i + n - 1) % n,
                        });
                    }
                }
                KeyCode::Enter => self.open_subagent(),
                _ => {}
            },
            View::Help => match key.code {
                KeyCode::Esc => self.toggle_help(),
                KeyCode::Down | KeyCode::Char('j') => {
                    self.help_scroll = self.help_scroll.saturating_add(1)
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.help_scroll = self.help_scroll.saturating_sub(1)
                }
                KeyCode::PageDown | KeyCode::Char(' ') => {
                    self.help_scroll = self.help_scroll.saturating_add(15)
                }
                KeyCode::PageUp => self.help_scroll = self.help_scroll.saturating_sub(15),
                KeyCode::Home | KeyCode::Char('g') => self.help_scroll = 0,
                KeyCode::End | KeyCode::Char('G') => self.help_scroll = u16::MAX,
                _ => {}
            },
        }
    }

    fn handle_transcript_key(&mut self, code: KeyCode) {
        if code == KeyCode::Char('e') {
            return self.export_transcript();
        }
        let Some(view) = &mut self.transcript else {
            self.view = View::Sessions;
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
            KeyCode::Esc => self.view = View::Sessions,
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
            KeyCode::Esc if self.folder_filter.is_some() => {
                self.folder_filter = None;
                let keep = self.selected_session().map(|s| s.id.clone());
                self.apply_filter(keep);
            }
            KeyCode::Char('/') => self.input = Some(Input::Search),
            KeyCode::Enter => self.request_resume(),
            KeyCode::Char('D') | KeyCode::Delete => self.request_delete(),
            KeyCode::Char('p') => self.start_prompt_input(),
            KeyCode::Char('v') => {
                if let Some(id) = self.selected_session().map(|s| s.id.clone()) {
                    self.open_transcript(&id, None, None);
                }
            }
            KeyCode::Char('*') => self.toggle_star(),
            KeyCode::Char('i') => {
                if self.selected_session().is_some() {
                    self.inspect_scroll = 0;
                    self.inspect_sub = None;
                    self.view = View::Inspect;
                }
            }
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
            KeyCode::Char('c') => {
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
            KeyCode::Enter | KeyCode::Char('l') => self.open_mcp_log(),
            KeyCode::Char('a') | KeyCode::Char('L') => {
                let (Some(project), Some(i)) = (&self.project, self.mcp_state.selected()) else {
                    return;
                };
                let Some(server) = self.project_servers().and_then(|s| s.get(i)) else {
                    return;
                };
                let (name, cwd) = (server.full_name.clone(), project.cwd.clone());
                if code == KeyCode::Char('a') {
                    self.pending_command = Some((vec!["mcp".into(), "login".into(), name], cwd));
                    self.mcp_recheck = true;
                } else {
                    self.popup = Some(Popup::Confirm {
                        title: "Sign out of MCP server".into(),
                        lines: vec![
                            format!("Sign out of {name}?"),
                            String::new(),
                            "This runs `claude mcp logout`, which clears its stored".into(),
                            "credentials. Sign in again with a.".into(),
                        ],
                        yes: "sign out".into(),
                        action: Confirm::McpLogout { server: name, cwd },
                    });
                }
            }
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
            KeyCode::Esc => self.view = View::Sessions,
            KeyCode::Right | KeyCode::Tab => {
                self.eco_tab = EcoTab::ALL[(tab + 1) % EcoTab::ALL.len()];
            }
            KeyCode::Left | KeyCode::BackTab => {
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
            Some(Popup::Branches {
                repo,
                repo_name,
                listing,
                query,
                matches,
                state,
                ..
            }) => match code {
                KeyCode::Esc => self.popup = None,
                KeyCode::Down if !matches.is_empty() => {
                    let next = state
                        .selected()
                        .map_or(0, |i| (i + 1).min(matches.len() - 1));
                    state.select(Some(next));
                }
                KeyCode::Up => state.select_previous(),
                KeyCode::Char(c) => {
                    query.push(c);
                    *matches = branch_matches(&listing.branches, query);
                    state.select((!matches.is_empty()).then_some(0));
                }
                KeyCode::Backspace => {
                    query.pop();
                    *matches = branch_matches(&listing.branches, query);
                    state.select((!matches.is_empty()).then_some(0));
                }
                KeyCode::Enter => {
                    let Some(branch) = state
                        .selected()
                        .and_then(|i| matches.get(i))
                        .map(|&i| listing.branches[i].clone())
                    else {
                        return;
                    };
                    let Some(base) = listing.base.clone() else {
                        return self.show_flash(
                            "No develop, main or master branch on the remote to review against",
                            true,
                        );
                    };
                    let (repo, repo_name) = (repo.clone(), repo_name.clone());
                    self.choose_branch(repo, repo_name, base, &branch);
                }
                _ => {}
            },
            Some(Popup::ReviewSetup {
                task, last, choice, ..
            }) => {
                let options = Mode::ALL.len() + usize::from(last.is_some());
                match code {
                    KeyCode::Esc | KeyCode::Char('q') => self.popup = None,
                    KeyCode::Down | KeyCode::Char('j') => *choice = (*choice + 1).min(options - 1),
                    KeyCode::Up | KeyCode::Char('k') => *choice = choice.saturating_sub(1),
                    KeyCode::Enter => {
                        if *choice < Mode::ALL.len() {
                            let mut task = task.clone();
                            task.mode = Mode::ALL[*choice];
                            self.popup = None;
                            self.start_review(task);
                        } else if let Some(review) = last.clone() {
                            self.popup = Some(Popup::Findings {
                                review,
                                state: ListState::default().with_selected(Some(0)),
                                detail: None,
                            });
                        }
                    }
                    _ => {}
                }
            }
            Some(Popup::Findings {
                review,
                state,
                detail,
            }) => {
                let n = review.result.findings.len();
                let selected = state.selected().unwrap_or(0);
                match (code, detail.as_mut()) {
                    (KeyCode::Esc | KeyCode::Char('q'), Some(_)) => *detail = None,
                    (KeyCode::Esc | KeyCode::Char('q'), None) => self.popup = None,
                    (KeyCode::Down | KeyCode::Char('j'), Some(scroll)) => {
                        *scroll = scroll.saturating_add(1)
                    }
                    (KeyCode::Up | KeyCode::Char('k'), Some(scroll)) => {
                        *scroll = scroll.saturating_sub(1)
                    }
                    (KeyCode::Down | KeyCode::Char('j'), None) if n > 0 => {
                        state.select(Some((selected + 1).min(n - 1)))
                    }
                    (KeyCode::Up | KeyCode::Char('k'), None) => state.select_previous(),
                    (KeyCode::Enter, None) if n > 0 => *detail = Some(0),
                    (KeyCode::Tab, _) => {
                        let review = review.clone();
                        self.copy_finding(&review, selected);
                    }
                    (KeyCode::Char('e'), _) => {
                        let review = review.clone();
                        self.export_review(&review);
                    }
                    (KeyCode::Char('v'), _) => {
                        let id = review.session_id.clone();
                        self.popup = None;
                        self.reload_sessions();
                        if self.sessions.iter().any(|s| s.id == id) {
                            self.open_transcript(&id, None, None);
                        } else {
                            self.show_flash("The review's session is gone", true);
                        }
                    }
                    (KeyCode::Char('D'), _) => {
                        let (main, path) = (review.repo.clone(), review.worktree.clone());
                        if !path.exists() {
                            return self.show_flash("Its worktree is already gone", false);
                        }
                        self.popup = Some(Popup::Confirm {
                            title: "Remove review worktree".into(),
                            lines: vec![
                                format!("Remove {}?", paths::display(&path)),
                                String::new(),
                                "The review and its session are kept. This runs".into(),
                                "`git worktree remove`, which refuses if files changed.".into(),
                            ],
                            yes: "remove".into(),
                            action: Confirm::RemoveReviewWorktree { main, path },
                        });
                    }
                    _ => {}
                }
            }
            Some(Popup::Palette {
                commands,
                query,
                matches,
                state,
            }) => match code {
                KeyCode::Esc => self.popup = None,
                KeyCode::Down if !matches.is_empty() => {
                    let next = state
                        .selected()
                        .map_or(0, |i| (i + 1).min(matches.len() - 1));
                    state.select(Some(next));
                }
                KeyCode::Up => state.select_previous(),
                KeyCode::Char(c) => {
                    query.push(c);
                    *matches = keys::search(commands, query);
                    state.select((!matches.is_empty()).then_some(0));
                }
                KeyCode::Backspace => {
                    query.pop();
                    *matches = keys::search(commands, query);
                    state.select((!matches.is_empty()).then_some(0));
                }
                KeyCode::Enter => {
                    let chosen = state
                        .selected()
                        .and_then(|i| matches.get(i))
                        .map(|&i| commands[i]);
                    self.popup = None;
                    if let Some(binding) = chosen {
                        self.run_command(binding);
                    }
                }
                _ => {}
            },
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
                    KeyCode::Char('D') => {
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
            Some(Popup::Prompts {
                query,
                matches,
                state,
            }) => match code {
                KeyCode::Esc => self.popup = None,
                KeyCode::Down if !matches.is_empty() => {
                    let next = state
                        .selected()
                        .map_or(0, |i| (i + 1).min(matches.len() - 1));
                    state.select(Some(next));
                }
                KeyCode::Up => state.select_previous(),
                KeyCode::Char(c) => {
                    query.push(c);
                    *matches = prompts::search(&self.prompts, query);
                    state.select((!matches.is_empty()).then_some(0));
                }
                KeyCode::Backspace => {
                    query.pop();
                    *matches = prompts::search(&self.prompts, query);
                    state.select((!matches.is_empty()).then_some(0));
                }
                KeyCode::Tab => {
                    if let Some(p) = state
                        .selected()
                        .and_then(|i| matches.get(i))
                        .map(|&i| &self.prompts[i])
                    {
                        let text = p.text.clone();
                        match prompts::copy_to_clipboard(&text) {
                            Ok(()) => self.show_flash("Copied to the clipboard", false),
                            Err(e) => self.show_flash(format!("Could not copy: {e}"), true),
                        }
                    }
                }
                KeyCode::Enter => {
                    let Some(text) = state
                        .selected()
                        .and_then(|i| matches.get(i))
                        .map(|&i| self.prompts[i].text.clone())
                    else {
                        return;
                    };
                    self.popup = None;
                    self.start_prompt_input();
                    if let Some(Input::Prompt { text: t, .. }) = &mut self.input {
                        *t = text;
                    }
                }
                _ => {}
            },
            Some(Popup::Summary { markdown, scroll }) => match code {
                KeyCode::Esc | KeyCode::Char('q') => self.popup = None,
                KeyCode::Down | KeyCode::Char('j') => *scroll = scroll.saturating_add(1),
                KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
                KeyCode::PageDown | KeyCode::Char(' ') => *scroll = scroll.saturating_add(15),
                KeyCode::PageUp => *scroll = scroll.saturating_sub(15),
                KeyCode::Char('e') => {
                    let markdown = markdown.clone();
                    self.export_summary(&markdown);
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
            Some(Popup::Confirm { action, .. }) => {
                let action = action.clone();
                if matches!(code, KeyCode::Char('y') | KeyCode::Char('Y')) {
                    self.popup = None;
                    self.confirmed(action);
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
            Some(Input::LogFilter { mut text }) => match code {
                KeyCode::Enter => self.logs.filter = text.trim().to_string(),
                KeyCode::Esc => self.logs.filter.clear(),
                KeyCode::Backspace => {
                    text.pop();
                    self.input = Some(Input::LogFilter { text });
                }
                KeyCode::Char(c) => {
                    text.push(c);
                    self.input = Some(Input::LogFilter { text });
                }
                _ => self.input = Some(Input::LogFilter { text }),
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

/// `<documents>/claudash-exports/` (or `~/claudash-exports/`), created if needed.
/// Branches whose name, subject or author contain every word of `query`.
fn branch_matches(branches: &[Branch], query: &str) -> Vec<usize> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    branches
        .iter()
        .enumerate()
        .filter(|(_, b)| {
            let text = format!("{} {} {}", b.name, b.subject, b.author).to_lowercase();
            words.iter().all(|w| text.contains(w.as_str()))
        })
        .map(|(i, _)| i)
        .collect()
}

fn exports_dir() -> io::Result<PathBuf> {
    let dir = dirs::document_dir()
        .or_else(dirs::home_dir)
        .ok_or_else(|| io::Error::other("no documents or home folder"))?
        .join("claudash-exports");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Writes `view` as Markdown to the exports folder and returns the file.
fn export_markdown(view: &TranscriptView) -> io::Result<PathBuf> {
    let dir = exports_dir()?;
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
        app.projects.loose = vec![PathBuf::from("/r")];
        app.projects.specs.insert(
            PathBuf::from("/r"),
            specs::ProjectSpecs {
                frameworks: vec![specs::Framework::OpenSpec],
                changes: vec![specs::Change {
                    framework: specs::Framework::OpenSpec,
                    id: "add-a-very-long-change-name-here".into(),
                    path: PathBuf::from("/r/openspec/changes/x"),
                    done: 3,
                    total: 7,
                    stage: specs::Stage::Implementing,
                    modified: std::time::SystemTime::UNIX_EPOCH,
                    next: Some("/opsx:apply x".into()),
                    next_label: Some("implement the tasks"),
                }],
            },
        );
        app.projects_state.select(Some(0));
        app.specs_focus = true;
        let hit = Hit {
            session_id: "x".into(),
            title: "t".into(),
            entry: 0,
            snippet: "…a snippet…".into(),
            at: None,
        };
        for (w, h) in [(1, 1), (10, 3), (20, 5), (40, 10), (80, 24), (200, 60)] {
            for view in [
                View::Sessions,
                View::Projects,
                View::Activity,
                View::Logs,
                View::Ecosystem,
                View::Usage,
                View::Transcript,
                View::Help,
                View::Inspect,
            ] {
                for tab in EcoTab::ALL {
                    for popup in 0..6 {
                        app.view = view;
                        app.eco_tab = tab;
                        app.monthly = popup % 2 == 0;
                        app.popup = match popup {
                            0 => None,
                            1 => Some(Popup::Text {
                                title: " t ".into(),
                                lines: vec!["a line".into(); 50],
                                scroll: u16::MAX,
                            }),
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
                            5 if view == View::Usage => Some(Popup::Summary {
                                markdown: "# Title\n\n- a line\n".repeat(20),
                                scroll: u16::MAX,
                            }),
                            5 if view == View::Projects => Some(Popup::Branches {
                                repo: PathBuf::from("/r"),
                                repo_name: "r".into(),
                                listing: Listing {
                                    base: Some("origin/develop".into()),
                                    branches: vec![Branch {
                                        name: "feature/a-very-long-branch-name".into(),
                                        reference: "origin/feature/x".into(),
                                        subject: "Add a thing".into(),
                                        author: "Someone".into(),
                                        when: 0,
                                        ahead: 3,
                                        local: true,
                                    }],
                                    fetch_error: None,
                                },
                                reviewed: HashMap::from([(
                                    "feature/a-very-long-branch-name".to_string(),
                                    0,
                                )]),
                                query: "feat".into(),
                                matches: vec![0],
                                state: ListState::default().with_selected(Some(0)),
                            }),
                            5 if view == View::Activity || view == View::Inspect => {
                                let review = Review {
                                    session_id: "s".into(),
                                    repo: PathBuf::from("/r"),
                                    branch: "feature/x".into(),
                                    base: "origin/develop".into(),
                                    worktree: PathBuf::from("/nonexistent"),
                                    mode: Mode::Static,
                                    at: 0,
                                    result: Findings {
                                        summary: "A summary that goes on. ".repeat(10),
                                        findings: vec![review::Finding {
                                            file: "src/a.rs".into(),
                                            line: Some(42),
                                            severity: "high".into(),
                                            comment: "A comment. ".repeat(30),
                                            suggestion: Some("let x = 1;\nlet y = 2;".into()),
                                        }],
                                    },
                                };
                                Some(Popup::Findings {
                                    review,
                                    state: ListState::default().with_selected(Some(0)),
                                    detail: (view == View::Inspect).then_some(u16::MAX),
                                })
                            }
                            5 if view == View::Ecosystem => Some(Popup::ReviewSetup {
                                task: ReviewTask {
                                    repo: PathBuf::from("/r"),
                                    repo_name: "r".into(),
                                    branch: "feature/x".into(),
                                    reference: "origin/feature/x".into(),
                                    base: "origin/develop".into(),
                                    mode: Mode::Static,
                                    session_id: "s".into(),
                                },
                                stat: Ok(DiffStat::default()),
                                last: None,
                                choice: 2,
                            }),
                            5 if view == View::Help => {
                                let commands = keys::commands(Context::Sessions, &[Context::Logs]);
                                let matches = (0..commands.len()).collect();
                                Some(Popup::Palette {
                                    commands,
                                    query: "a very long query".into(),
                                    matches,
                                    state: ListState::default().with_selected(Some(3)),
                                })
                            }
                            5 if view == View::Logs => Some(Popup::Prompts {
                                query: "q".into(),
                                matches: Vec::new(),
                                state: ListState::default(),
                            }),
                            _ => Some(Popup::Confirm {
                                title: "Move to trash".into(),
                                lines: vec!["A question far too long to fit anywhere".into()],
                                yes: "do it".into(),
                                action: Confirm::Trash("x".into()),
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
        app.live = HashMap::from([(
            id.clone(),
            LiveSession {
                session_id: id.clone(),
                status: "busy".into(),
                ..Default::default()
            },
        )]);
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

    #[test]
    fn palette_runs_commands_where_their_key_works() {
        let mut app = App::new(1_000_000, false);
        app.handle_key(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE));
        for c in "usage view".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.popup.is_none());
        assert!(app.view == View::Usage);

        // A command of another view goes there first.
        app.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
        for c in "days or months".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        app.view = View::Sessions;
        let before = app.monthly;
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.view == View::Usage);
        assert_ne!(app.monthly, before);

        // Number keys follow the tab order.
        app.handle_key(KeyEvent::new(KeyCode::Char('3'), KeyModifiers::NONE));
        assert!(app.view == View::Projects);
        // Esc goes back but never quits.
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.view == View::Sessions && !app.should_quit);
    }

    #[test]
    fn counts_uses_by_name_and_plugin() {
        let counts = UsageCounts {
            skills: HashMap::from([
                ("superpowers:writing-plans".into(), 3),
                ("release-notes".into(), 1),
            ]),
            mcp_servers: HashMap::from([
                ("plugin_context7_context7".into(), 2),
                ("playwright".into(), 5),
            ]),
            commands: HashMap::from([("/superpowers:brainstorm".into(), 1)]),
            ..Default::default()
        };
        assert_eq!(UsageCounts::count(&counts.skills, "writing-plans"), 3);
        assert_eq!(UsageCounts::count(&counts.skills, "release-notes"), 1);
        assert_eq!(counts.plugin("superpowers"), 4);
        assert_eq!(counts.plugin("context7"), 2);
        assert_eq!(counts.plugin("vercel"), 0);
    }

    #[test]
    fn finds_shared_folders_same_files_and_forgotten_worktrees() {
        use crate::projects::{Checkout, Repo};
        let mut app = App::new(1_000_000, false);
        let dir = PathBuf::from("/repo");
        let session = |id: &str| Session {
            id: id.into(),
            path: PathBuf::from(format!("/t/{id}.jsonl")),
            title: format!("title {id}"),
            project_path: "~/repo".into(),
            cwd: Some(dir.clone()),
            git_branch: None,
            modified: std::time::SystemTime::now(),
            size: 0,
            tokens: Default::default(),
        };
        app.sessions = vec![session("a"), session("b")];
        app.live = ["a", "b"]
            .iter()
            .map(|id| {
                let live = LiveSession {
                    session_id: id.to_string(),
                    status: "busy".into(),
                    ..Default::default()
                };
                (id.to_string(), live)
            })
            .collect();
        for id in ["a", "b"] {
            let mut analysis = Analysis::default();
            analysis.edits.insert("/repo/src/auth.rs".into(), None);
            app.analyses.insert(
                PathBuf::from(format!("/t/{id}.jsonl")),
                (
                    std::time::SystemTime::now(),
                    0,
                    std::sync::Arc::new(analysis),
                ),
            );
        }
        let checkout = |path: &str, main: bool, changed: usize, prunable: bool| Checkout {
            path: PathBuf::from(path),
            branch: None,
            status: (!prunable).then(|| git::Status {
                changed,
                ..Default::default()
            }),
            main,
            locked: false,
            prunable,
            claude_created: false,
            review: false,
        };
        app.projects.repos = vec![Repo {
            name: "repo".into(),
            checkouts: vec![
                checkout("/repo", true, 0, false),
                checkout("/repo/.claude/worktrees/x", false, 3, false),
                checkout("/gone", false, 0, true),
            ],
        }];

        let problems = app.problems();
        assert!(
            problems.iter().any(
                |p| matches!(p, Problem::SharedFolder { sessions, .. } if sessions.len() == 2)
            )
        );
        assert!(
            problems.iter().any(
                |p| matches!(p, Problem::SameFile { file, .. } if file == "/repo/src/auth.rs")
            )
        );
        assert!(
            problems
                .iter()
                .any(|p| matches!(p, Problem::IdleWorktree { changed: 3, .. }))
        );
        assert!(
            problems
                .iter()
                .any(|p| matches!(p, Problem::MissingWorktree { .. }))
        );
        assert_eq!(problems.len(), 4);
    }
}
