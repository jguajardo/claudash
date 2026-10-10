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
    launch::{self, Launch, Mux, Place},
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
    worktree::{self, Steps},
};

mod pages;
pub use pages::Card;

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
/// From this much of a window used, claudash tells you when it resets.
const LIMIT_RESET_ALERT: f64 = 90.0;
/// Plan usage percentages that trigger an alert, once per window.
const PLAN_ALERTS: [f64; 2] = [80.0, 95.0];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// Every session, the selected one's project and its usage.
    Sessions,
    /// What needs you now: open sessions, alerts, plan usage, live feed.
    Now,
    /// Every project as a card; Enter opens its page.
    Projects,
    /// Plan usage, where tokens go, and security.
    Insights,
    /// MCP server and background session logs.
    Logs,
    /// A session's conversation, full screen.
    Transcript,
    /// Everything claudash does, and whether it's set up.
    Help,
    /// One session in depth: context over time, tools, files, subagents.
    Inspect,
}

/// Views in the order of their number keys, `1` to `4`.
pub const VIEW_KEYS: [View; 4] = [View::Now, View::Sessions, View::Projects, View::Insights];

impl View {
    pub fn title(self) -> &'static str {
        match self {
            View::Now => "Now",
            View::Sessions => "Sessions",
            View::Projects => "Projects",
            View::Insights => "Insights",
            View::Logs => "Logs",
            View::Transcript => "Conversation",
            View::Help => "Help",
            View::Inspect => "Inspector",
        }
    }
}

/// The sections of a project's page, in its menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Overview,
    Sessions,
    Specs,
    Worktrees,
    Snapshots,
    Mcp,
    Setup,
    Security,
}

impl Section {
    pub const ALL: [Section; 8] = [
        Section::Overview,
        Section::Sessions,
        Section::Specs,
        Section::Worktrees,
        Section::Snapshots,
        Section::Mcp,
        Section::Setup,
        Section::Security,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Section::Overview => "Overview",
            Section::Sessions => "Sessions",
            Section::Specs => "Specs",
            Section::Worktrees => "Worktrees",
            Section::Snapshots => "Snapshots",
            Section::Mcp => "MCP servers",
            Section::Setup => "Skills & plugins",
            Section::Security => "Security",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Section::Overview => "◉",
            Section::Sessions => "≡",
            Section::Specs => "▤",
            Section::Worktrees => "⎇",
            Section::Snapshots => "↺",
            Section::Mcp => "⌁",
            Section::Setup => "⚙",
            Section::Security => "◈",
        }
    }
}

/// A project's page: its folder (the main checkout, or a folder outside git)
/// and where you are in it.
pub struct ProjectPage {
    pub dir: PathBuf,
    pub section: Section,
    /// Keys go to the section's content instead of the menu.
    pub in_content: bool,
    pub sessions_state: ratatui::widgets::TableState,
    pub worktrees_state: ratatui::widgets::TableState,
    pub snapshots_state: ratatui::widgets::TableState,
    pub scroll: u16,
}

/// The sections of Insights, in its menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InsightsSection {
    Usage,
    Quota,
    Plan,
    Tokens,
    Security,
}

impl InsightsSection {
    pub const ALL: [InsightsSection; 5] = [
        InsightsSection::Usage,
        InsightsSection::Quota,
        InsightsSection::Plan,
        InsightsSection::Tokens,
        InsightsSection::Security,
    ];

    pub fn title(self) -> &'static str {
        match self {
            InsightsSection::Usage => "Plan & usage",
            InsightsSection::Quota => "Where the limit went",
            InsightsSection::Plan => "Is your plan worth it",
            InsightsSection::Tokens => "Where tokens go",
            InsightsSection::Security => "Security",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            InsightsSection::Usage => "▆",
            InsightsSection::Quota => "◑",
            InsightsSection::Plan => "$",
            InsightsSection::Tokens => "◔",
            InsightsSection::Security => "◈",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EcoTab {
    Skills,
    Agents,
    Commands,
    Hooks,
    Plugins,
    Permissions,
}

impl EcoTab {
    pub const ALL: [EcoTab; 6] = [
        EcoTab::Skills,
        EcoTab::Agents,
        EcoTab::Commands,
        EcoTab::Hooks,
        EcoTab::Plugins,
        EcoTab::Permissions,
    ];

    pub fn title(self) -> &'static str {
        match self {
            EcoTab::Skills => "Skills",
            EcoTab::Agents => "Agents",
            EcoTab::Commands => "Commands",
            EcoTab::Hooks => "Hooks",
            EcoTab::Plugins => "Plugins",
            EcoTab::Permissions => "Permissions",
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
    /// The week or month on one card.
    Wrapped { month: bool, redact: bool },
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
    /// Branch reviews of every project: running, waiting to be read and
    /// `saved` (the earlier ones, read from disk when the list opens).
    Reviews {
        state: ListState,
        saved: Vec<Review>,
    },
}

type BranchesJob = Job<Result<Listing, String>>;

/// Reviews that may run at once: each is a Claude Code session on your plan.
const MAX_REVIEWS: usize = 4;
/// Finished reviews the list shows besides the ones waiting to be read.
const REVIEWS_LISTED: usize = 30;

/// A branch review being set up or run.
struct RunningReview {
    task: ReviewTask,
    started: Instant,
    job: ReviewJob,
}

/// A review Claude does in an interactive session, which the user drives.
struct PendingReview {
    task: ReviewTask,
    worktree: PathBuf,
    /// The session runs in a tab or pane of its own, not in claudash's terminal.
    elsewhere: bool,
    /// A file that tab creates when the session ends.
    done: Option<PathBuf>,
    /// Claude Code has reported the session open at least once.
    seen_open: bool,
}

/// A `claude` command waiting for claudash's own terminal.
struct PendingCommand {
    args: Vec<String>,
    cwd: PathBuf,
    /// The interactive review this session is, by session ID.
    review: Option<String>,
}

/// A line of the list of reviews (`B`).
pub enum ReviewRow<'a> {
    Running {
        task: &'a ReviewTask,
        /// `None` for a review you drive in a session of its own.
        started: Option<Instant>,
    },
    Done {
        review: &'a Review,
        /// Finished and not opened yet.
        unread: bool,
    },
}

/// `launch::open`.
type Launcher = fn(Mux, Place, &Launch) -> Result<(), String>;

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
    /// Remove one worktree, after clearing what stands in its way.
    RemoveWorktree {
        main: PathBuf,
        path: PathBuf,
        steps: Steps,
    },
    /// Turn snapshots on or off in the settings file.
    Snapshots {
        on: bool,
    },
    /// Remove the worktrees nobody is using, each with its own steps.
    RemoveWorktrees {
        main: PathBuf,
        items: Vec<(PathBuf, Steps)>,
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
    /// Continue the sessions a plan limit stopped, now that it has reset.
    ContinueStopped,
    /// Put a snapshot's files back in a work tree.
    RestoreSnapshot {
        root: PathBuf,
        sha: String,
    },
}

impl Confirm {
    /// The key that says yes. Deleting files that exist nowhere else takes a
    /// different one from the questions that lose nothing.
    pub fn key(&self) -> char {
        match self {
            Confirm::RemoveWorktree { steps, .. } if steps.discard => 'D',
            _ => 'y',
        }
    }
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
    /// The alerts in Now.
    Alerts,
}

/// Where an alert takes you.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// A session's inspector, where its audit and secrets are.
    Session(String),
    /// Insights › Security (credentials in the prompt history).
    InsightsSecurity,
    /// Continue the sessions a plan limit stopped, or have them continue at
    /// the reset.
    ContinueStopped,
    /// A section of a project's page, with something in it selected: a
    /// checkout path, a spec change ID or an MCP server's full name.
    Project {
        dir: PathBuf,
        section: Section,
        select: Option<String>,
    },
}

/// Something in Now that needs a look, and where Enter takes you.
#[derive(Clone, Debug, PartialEq)]
pub enum Alert {
    Problem(Problem),
    /// An MCP server checked in `dir` that failed to connect.
    Mcp {
        dir: PathBuf,
        name: String,
        full_name: String,
    },
    /// MCP servers checked in `dir` that need you to sign in: one alert for
    /// all of them, names once each.
    McpSignIn {
        dir: PathBuf,
        names: Vec<String>,
        /// The first one's full name, selected when you go there.
        first: String,
    },
    /// Sessions a plan limit stopped.
    LimitStopped {
        /// Stopped and not open in Claude Code.
        closed: usize,
        /// Of those, how many can be continued now.
        ready: usize,
        /// Open in Claude Code: continued from their own terminal.
        open: usize,
        /// The next reset of a closed one that hasn't reset yet.
        next_reset: Option<i64>,
        /// They'll be continued at the reset.
        queued: bool,
    },
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
    /// API keys or tokens in a session's transcript (or in your prompt history
    /// when `session` is empty).
    Secrets {
        session: String,
        /// Empty for the prompt history.
        session_id: String,
        found: Vec<String>,
    },
    /// Something risky a session did lately.
    Risky {
        session: String,
        session_id: String,
        what: &'static str,
        detail: String,
    },
    /// A spec change with tasks left that nobody touched for a while.
    StaleChange {
        framework: &'static str,
        /// The folder whose specs have it.
        dir: PathBuf,
        id: String,
        done: usize,
        total: usize,
        days: u64,
    },
    /// A spec change whose tasks are all done, with the command to wrap it up.
    FinishedChange {
        framework: &'static str,
        dir: PathBuf,
        id: String,
        next: String,
    },
}

/// Risky commands listed in Problems at most; the inspector has them all.
const MAX_RISKY_PROBLEMS: usize = 5;

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
    /// `claude` commands for claudash's own terminal, in the order asked
    /// for; `run`, which owns the terminal, takes them one at a time.
    pending_commands: std::collections::VecDeque<PendingCommand>,
    /// The terminal multiplexer claudash runs in, if any.
    pub mux: Option<Mux>,
    /// Where sessions open when there is one (`open_in` in the settings).
    pub open_in: Place,
    /// Asks the multiplexer for a tab or pane; replaced in tests.
    launcher: Launcher,
    /// Open sessions (ID -> "busy"/"idle") from `claude agents --json`.
    pub live: HashMap<String, LiveSession>,
    /// Background sessions, finished ones included (`claude agents --json --all`).
    pub background: Vec<LiveSession>,
    pub activity_focus: ActivityFocus,
    pub background_state: ListState,
    /// `claude` commands running in the background, by what they do
    /// ("stop bg7k2", "continue", "mcp logout linear"): any number at once,
    /// never the same one twice.
    background_jobs: Vec<(String, Job<Result<String, String>>)>,
    /// Worktrees being removed; each ends with what to tell the user.
    removals: Vec<Job<Result<String, String>>>,
    pub logs: LogsView,
    live_job: Option<Job<Result<HashMap<String, LiveSession>, String>>>,
    pub statusline: statusline::Store,
    /// Last hook event per session, from `claudash hook`.
    pub hook_states: HashMap<String, hooks::State>,
    /// `true` once any hook has run, i.e. `claudash setup --apply` was used.
    pub hooks_configured: bool,
    /// Activity seen at the previous check, to notify on changes only.
    seen_activity: Option<HashMap<String, Activity>>,
    /// A window that came close to its limit, and when it resets.
    limit_reset_due: Option<(&'static str, i64)>,
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
    pub eco_states: [ListState; 6],
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
    /// Selected card in Projects.
    pub project_cursor: usize,
    /// Cards per row the last time Projects was drawn, for moving up and down.
    pub project_columns: usize,
    /// The open project page, if any.
    pub project_page: Option<ProjectPage>,
    pub specs_state: ListState,
    pub insights: InsightsSection,
    pub insights_scroll: u16,
    /// Selected alert in Now.
    pub alert_cursor: usize,
    /// Your Claude plan and where it was learned (see `plan::detect`).
    pub plan: Option<(crate::plan::Plan, &'static str)>,
    /// `auto_continue = true`: continue stopped sessions at the reset.
    pub auto_continue: bool,
    /// Enter on the stopped-sessions alert before the reset: continue them
    /// when it comes.
    pub continue_at_reset: bool,
    /// Sessions already continued, so they aren't continued twice while
    /// their transcripts catch up.
    continued: HashSet<String>,
    /// Snapshots of the open project's checkouts, newest first.
    pub snapshots: Vec<(PathBuf, crate::snapshots::Snapshot)>,
    /// `snapshots = true` in the settings file: the hook takes them.
    pub snapshots_on: bool,
    /// Which project they were read for, and when.
    snapshots_at: Option<(PathBuf, Instant)>,
    /// No colors (`NO_COLOR` or `colors = false` in the settings file).
    pub no_color: bool,
    /// Where Esc goes from a conversation, the inspector or the logs.
    pub return_view: View,
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
    /// Reviews preparing their worktree or (static mode) being done by Claude.
    review_jobs: Vec<RunningReview>,
    /// Finished reviews nobody has opened yet, oldest first.
    pub reviews_ready: Vec<Review>,
    /// What each session that needs you is waiting on, by session ID, with
    /// the transcript size it was read at.
    pub pending: HashMap<String, (u64, transcript::PendingTool)>,
    /// Credentials found in `~/.claude/history.jsonl`, checked once at start.
    pub history_secrets: Vec<crate::audit::Secret>,
    history_secrets_job: Option<Job<Vec<crate::audit::Secret>>>,
    /// Re-check the project's MCP servers once the running command ends.
    mcp_recheck: bool,
    /// Interactive reviews whose session hasn't ended.
    pending_reviews: Vec<PendingReview>,

    // Prompt history and daily summary.
    pub prompts: Vec<Prompt>,
    summary_job: Option<Job<String>>,

    // Organization.
    pub library: Library,
    pub history: History,
    /// Usage view shows months instead of days.
    pub monthly: bool,
    /// Usage chart in API-equivalent dollars instead of tokens.
    pub dollars: bool,
    /// Token savers installed (caveman, rtk), found when that page opens.
    pub token_savers: Option<Vec<String>>,
}

impl App {
    pub fn new(context_limit: u64, notify: bool) -> Self {
        let mut app = Self {
            context_limit,
            notify,
            view: View::Now,
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
            pending_commands: Default::default(),
            mux: None,
            open_in: Place::default(),
            launcher: launch::open,
            live: HashMap::new(),
            background: Vec::new(),
            activity_focus: ActivityFocus::Open,
            background_state: ListState::default(),
            background_jobs: Vec::new(),
            removals: Vec::new(),
            logs: LogsView::default(),
            live_job: None,
            statusline: statusline::Store::default(),
            hook_states: HashMap::new(),
            hooks_configured: false,
            seen_activity: None,
            plan_alerts_sent: HashSet::new(),
            limit_reset_due: None,
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
            project_cursor: 0,
            project_columns: 1,
            project_page: None,
            specs_state: ListState::default(),
            insights: InsightsSection::Usage,
            insights_scroll: 0,
            return_view: View::Sessions,
            alert_cursor: 0,
            plan: None,
            auto_continue: false,
            continue_at_reset: false,
            continued: HashSet::new(),
            snapshots: Vec::new(),
            snapshots_on: crate::snapshots::enabled(),
            snapshots_at: None,
            no_color: false,
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
            review_jobs: Vec::new(),
            reviews_ready: Vec::new(),
            pending_reviews: Vec::new(),
            mcp_recheck: false,
            pending: HashMap::new(),
            history_secrets: Vec::new(),
            history_secrets_job: None,
            library: Library::load(),
            history: History::load(),
            monthly: false,
            token_savers: None,
            dollars: false,
        };
        app.doctor_job = Some(Job::spawn(doctor::run));
        app.history_secrets_job = Some(Job::spawn(|| {
            let mut found = Vec::new();
            if let Some(file) = paths::claude_home().map(|h| h.join("history.jsonl"))
                && let Ok(text) = std::fs::read_to_string(file)
            {
                for line in text.lines() {
                    crate::audit::find_secrets(line, &mut found);
                }
            }
            found
        }));
        app.maybe_welcome();
        let expired = library::purge_expired();
        if expired > 0 {
            app.show_flash(
                format!(
                    "Emptied {} older than {} days from the trash",
                    crate::ui::plural(expired as u64, "session"),
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
            if let Some(command) = self.pending_commands.pop_front() {
                self.run_claude(terminal, &command.args, &command.cwd)?;
                if let Some(session_id) = &command.review {
                    self.settle_review_here(session_id);
                }
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
        } else if self.view == View::Now && self.ticks.is_multiple_of(8) {
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
        self.maybe_read_snapshots();
        if self.ticks.is_multiple_of(20) {
            self.maybe_continue_stopped();
        }
        if self.project_page.as_ref().is_some_and(|p| {
            self.view == View::Projects && matches!(p.section, Section::Setup | Section::Security)
        }) {
            self.ensure_ecosystem();
            self.ensure_plugin_details();
        }
    }

    /// Sessions a plan limit stopped that haven't been continued yet.
    pub fn stopped(&self) -> Vec<crate::stopped::Stopped<'_>> {
        let now = chrono::Utc::now().timestamp();
        crate::stopped::stopped(&self.sessions, &self.live, now)
            .into_iter()
            .filter(|s| !self.continued.contains(&s.session.id))
            .collect()
    }

    /// Continues, in the background, every stopped session whose limit has
    /// reset and that isn't open.
    fn continue_stopped(&mut self) {
        let now = chrono::Utc::now().timestamp();
        let ready = crate::stopped::ready(&self.stopped(), now);
        if ready.is_empty() {
            return self.show_flash("No stopped session can be continued yet", true);
        }
        if self.background_running("continue") {
            return self.show_flash("Those sessions are already being continued", true);
        }
        self.continued
            .extend(ready.iter().map(|(id, _, _)| id.clone()));
        self.background_jobs.push((
            "continue".into(),
            Job::spawn(move || crate::stopped::continue_all(&ready)),
        ));
    }

    /// At the reset, continues stopped sessions when you asked for it (Enter
    /// on the alert) or `auto_continue` is on. Waits half a minute past the
    /// reset so the new window is open.
    fn maybe_continue_stopped(&mut self) {
        if !(self.continue_at_reset || self.auto_continue) || self.background_running("continue") {
            return;
        }
        let later = chrono::Utc::now().timestamp() - 30;
        if crate::stopped::ready(&self.stopped(), later).is_empty() {
            return;
        }
        self.continue_at_reset = false;
        self.continue_stopped();
    }

    /// Whether `snapshots` were read for the project at `dir`.
    pub fn snapshots_for(&self, dir: &Path) -> bool {
        self.snapshots_at.as_ref().is_some_and(|(d, _)| d == dir)
    }

    /// Reads the open project's snapshots again every few seconds while its
    /// Snapshots section is shown.
    fn maybe_read_snapshots(&mut self) {
        let Some(page) = self
            .project_page
            .as_ref()
            .filter(|p| self.view == View::Projects && p.section == Section::Snapshots)
        else {
            return;
        };
        if self
            .snapshots_at
            .as_ref()
            .is_some_and(|(dir, at)| *dir == page.dir && at.elapsed() < REFRESH_EVERY)
        {
            return;
        }
        let dir = page.dir.clone();
        let mut list: Vec<(PathBuf, crate::snapshots::Snapshot)> = self
            .project_folders(&dir)
            .into_iter()
            .flat_map(|root| {
                crate::snapshots::list(&root)
                    .into_iter()
                    .map(move |s| (root.clone(), s))
            })
            .collect();
        list.sort_by_key(|(_, s)| std::cmp::Reverse(s.at));
        self.snapshots = list;
        self.snapshots_at = Some((dir, Instant::now()));
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
            // Open sessions → background sessions → alerts → open sessions,
            // skipping what's empty.
            let has_alerts = !self.alerts().is_empty();
            self.activity_focus = match self.activity_focus {
                ActivityFocus::Open if !self.background.is_empty() => {
                    if self.background_state.selected().is_none() {
                        self.background_state.select(Some(0));
                    }
                    ActivityFocus::Background
                }
                ActivityFocus::Open | ActivityFocus::Background if has_alerts => {
                    ActivityFocus::Alerts
                }
                _ => ActivityFocus::Open,
            };
            return;
        }
        if self.activity_focus == ActivityFocus::Alerts {
            return self.handle_alerts_key(code);
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
            KeyCode::Esc => {}
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
                    self.enter_subview(View::Inspect);
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
        self.enter_subview(View::Transcript);
    }

    /// Carries out a confirmed action.
    fn confirmed(&mut self, action: Confirm) {
        match action {
            Confirm::Trash(id) => self.delete_session(&id),
            Confirm::StopBackground(id) => self.start_background_command("stop", &id),
            Confirm::RespawnBackground(id) => self.start_background_command("respawn", &id),
            Confirm::RemoveWorktree { main, path, steps } => {
                let place = paths::display(&path);
                self.show_flash(format!("Removing {place}…"), false);
                self.removals.push(Job::spawn(move || {
                    worktree::remove(&main, &path, &steps)
                        .map(|()| format!("Removed worktree {place}"))
                        .map_err(|e| format!("{place} was not removed: {e}"))
                }));
            }
            Confirm::RemoveWorktrees { main, items } => {
                self.show_flash(
                    format!("Removing {}…", ui::plural(items.len() as u64, "worktree")),
                    false,
                );
                self.removals.push(Job::spawn(move || {
                    let mut removed = 0;
                    let mut refused = Vec::new();
                    for (path, steps) in &items {
                        match worktree::remove(&main, path, steps) {
                            Ok(()) => removed += 1,
                            Err(e) => refused.push(format!("{}: {e}", paths::display(path))),
                        }
                    }
                    let done = format!("Removed {}", ui::plural(removed, "worktree"));
                    match refused.first() {
                        None => Ok(done),
                        Some(first) => Err(format!("{done}; {} left ({first})", refused.len())),
                    }
                }));
            }
            Confirm::ContinueStopped => self.continue_stopped(),
            Confirm::Snapshots { on } => {
                let value = if on { "true" } else { "false" };
                // The setting first: without it the hook has nothing to do.
                let done = crate::config::set("snapshots", value).and_then(|_| {
                    let (statusline, hooked) = crate::setup::installed();
                    if on && (!statusline || hooked < hooks::EVENTS.len()) {
                        crate::setup::apply(crate::setup::Mode::Apply).map(|_| ())
                    } else {
                        Ok(())
                    }
                });
                match done {
                    Ok(()) => {
                        self.snapshots_on = on;
                        self.show_flash(
                            if on {
                                "Snapshots are on: the first is taken with your next prompt in \
                                 a session of a git project"
                            } else {
                                "Snapshots are off; the ones already taken are kept"
                            },
                            false,
                        );
                    }
                    Err(e) => self.show_flash(
                        format!(
                            "Could not turn snapshots {}: {e}",
                            if on { "on" } else { "off" }
                        ),
                        true,
                    ),
                }
            }
            Confirm::RestoreSnapshot { root, sha } => {
                match crate::snapshots::restore(&root, &sha) {
                    Ok(()) => self.show_flash(
                        format!(
                            "Restored {} to snapshot {}; the state before it is a snapshot too",
                            paths::display(&root),
                            &sha[..sha.len().min(8)]
                        ),
                        false,
                    ),
                    Err(e) => self.show_flash(format!("Could not restore: {e}"), true),
                }
                self.snapshots_at = None;
                self.git_at = None;
            }
            Confirm::McpLogout { server, cwd } => {
                let what = format!("mcp logout {server}");
                if self.background_running(&what) {
                    return self.show_flash(format!("claude {what} is already running"), true);
                }
                self.background_jobs.push((
                    what,
                    Job::spawn(move || claude_cli::mcp_logout(&server, &cwd)),
                ));
            }
            Confirm::RemoveReviewWorktree { main, path } => {
                match git::remove_worktree(&main, &path, git::Removal::default()) {
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
        let what = format!("{command} {id}");
        if self.background_running(&what) {
            return self.show_flash(format!("claude {what} is already running"), true);
        }
        let job_id = id.to_string();
        self.background_jobs.push((
            what,
            Job::spawn(move || claude_cli::background(command, &job_id)),
        ));
    }

    fn background_running(&self, what: &str) -> bool {
        self.background_jobs.iter().any(|(w, _)| w == what)
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
                let title = bg.name.clone().unwrap_or_else(|| id.clone());
                self.open_claude(vec!["attach".into(), id], cwd, &title);
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
        self.enter_subview(View::Logs);
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
            KeyCode::Esc => self.view = self.return_view,
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

        // Credentials in transcripts and in the prompt history.
        for s in &self.sessions {
            if let Some(a) = self.analysis(s)
                && !a.secrets.is_empty()
            {
                let kinds: std::collections::BTreeSet<&str> =
                    a.secrets.iter().map(|x| x.kind).collect();
                problems.push(Problem::Secrets {
                    session: s.title.clone(),
                    session_id: s.id.clone(),
                    found: kinds.into_iter().map(str::to_owned).collect(),
                });
            }
        }
        if !self.history_secrets.is_empty() {
            problems.push(Problem::Secrets {
                session: String::new(),
                session_id: String::new(),
                found: self
                    .history_secrets
                    .iter()
                    .map(|x| x.kind)
                    .collect::<std::collections::BTreeSet<&str>>()
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
            });
        }
        // The most severe things done in the last week.
        let week = chrono::Local::now() - chrono::Duration::days(7);
        let mut risky = 0;
        for s in &self.sessions {
            let Some(a) = self.analysis(s) else {
                continue;
            };
            for e in a.audit.iter().filter(|e| {
                e.severity == crate::audit::Severity::High && e.at.is_some_and(|t| t >= week)
            }) {
                if risky < MAX_RISKY_PROBLEMS {
                    problems.push(Problem::Risky {
                        session: s.title.clone(),
                        session_id: s.id.clone(),
                        what: e.what,
                        detail: e.detail.clone(),
                    });
                }
                risky += 1;
            }
        }

        // Spec changes: once per change id, even when worktrees repeat it.
        let mut seen = HashSet::new();
        for (dir, specs) in &self.projects.specs {
            for c in &specs.changes {
                if !seen.insert((c.framework, c.id.clone())) {
                    continue;
                }
                let days = c.modified.elapsed().map_or(0, |d| d.as_secs() / 86_400);
                match c.stage {
                    specs::Stage::Implementing if days >= STALE_CHANGE_DAYS => {
                        problems.push(Problem::StaleChange {
                            framework: c.framework.title(),
                            dir: dir.clone(),
                            id: c.id.clone(),
                            done: c.done,
                            total: c.total,
                            days,
                        })
                    }
                    specs::Stage::Complete if c.next.is_some() => {
                        problems.push(Problem::FinishedChange {
                            framework: c.framework.title(),
                            dir: dir.clone(),
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
        // A window that ran close to its limit has reset: say so once.
        let now = chrono::Utc::now().timestamp();
        if let Some((label, resets_at)) = self.limit_reset_due
            && now >= resets_at
        {
            self.limit_reset_due = None;
            self.alert(
                &format!("Claude plan: {label} limit reset"),
                "You can pick up where you stopped",
            );
        }
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
            if w.used_percentage >= LIMIT_RESET_ALERT
                && self.limit_reset_due.is_none_or(|(_, at)| at != w.resets_at)
            {
                self.limit_reset_due = Some((label, w.resets_at));
            }
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
        let (id, title, cwd) = match self
            .selected_session()
            .map(|s| (s, self.open_elsewhere(s, "resume it here")))
        {
            None => return,
            Some((_, Some(msg))) => return self.show_flash(msg, true),
            Some((s, None)) => (s.id.clone(), s.title.clone(), self.selected_project_dir()),
        };
        match cwd {
            Ok(cwd) => {
                let cwd = cwd.to_path_buf();
                self.open_claude(vec!["--resume".into(), id], cwd, &title);
            }
            Err(msg) => self.show_flash(msg, true),
        }
    }

    /// Opens an interactive `claude <args>` in `cwd`: in a new tab or pane of
    /// the multiplexer claudash runs in, so claudash stays on screen and other
    /// sessions can be opened meanwhile; otherwise in claudash's own terminal.
    pub(crate) fn open_claude(&mut self, args: Vec<String>, cwd: PathBuf, title: &str) {
        if !self.open_elsewhere_in(&args, &cwd, title, None) {
            self.pending_commands.push_back(PendingCommand {
                args,
                cwd,
                review: None,
            });
        }
    }

    /// Asks the multiplexer, when there is one and the settings allow it, for
    /// a tab or pane running `claude <args>`; `done` is a file for it to
    /// create when the session ends. `false` means the session has to run in
    /// claudash's own terminal.
    fn open_elsewhere_in(
        &mut self,
        args: &[String],
        cwd: &Path,
        title: &str,
        done: Option<&Path>,
    ) -> bool {
        let Some(mux) = self.mux.filter(|_| self.open_in != Place::Here) else {
            return false;
        };
        let program = claude_cli::path()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "claude".to_string());
        let name = launch::tab_name(title);
        let unit = mux.unit(self.open_in);
        // The tab inherits the multiplexer's environment, not claudash's.
        let env: Vec<(String, String)> = std::env::var("CLAUDE_CONFIG_DIR")
            .ok()
            .filter(|dir| !dir.is_empty())
            .map(|dir| ("CLAUDE_CONFIG_DIR".to_string(), dir))
            .into_iter()
            .collect();
        let launch = Launch {
            program: &program,
            args,
            cwd,
            name: &name,
            env: &env,
            done,
        };
        match (self.launcher)(mux, self.open_in, &launch) {
            Ok(()) => {
                self.show_flash(
                    format!("Opened \"{name}\" in a new {} {unit}", mux.name()),
                    false,
                );
                true
            }
            Err(e) => {
                self.show_flash(
                    format!("No new {} {unit} ({e}); opening it here", mux.name()),
                    true,
                );
                false
            }
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
            // Not every command is a session: `claude mcp login` only signs in.
            Ok(s) if s.success() && args.first().is_some_and(|a| a == "mcp") => self.show_flash(
                "Back from Claude Code; checking the MCP servers again",
                false,
            ),
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
        self.enter_subview(View::Transcript);
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
        match export_markdown(view) {
            Ok((path, 0)) => {
                self.show_flash(format!("Exported to {}", paths::display(&path)), false)
            }
            Ok((path, n)) => self.show_flash(
                format!(
                    "Exported to {}, with {} removed",
                    paths::display(&path),
                    plural_secrets(n)
                ),
                false,
            ),
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
            "Moved {} to the trash, {:.1} MB (T to restore)",
            crate::ui::plural(moved as u64, "session"),
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

    /// What the status line says while Claude reviews in the background.
    pub fn reviewing(&self) -> Option<String> {
        let branches: Vec<&str> = self
            .review_jobs
            .iter()
            .filter(|r| r.task.mode == Mode::Static)
            .map(|r| r.task.branch.as_str())
            .collect();
        match branches.as_slice() {
            [] => None,
            [one] => Some(format!("Claude is reviewing {one}…")),
            many => Some(format!("Claude is reviewing {} branches…", many.len())),
        }
    }

    /// Reviews being set up, run by Claude, or driven by you in a session.
    pub(crate) fn reviews_in_progress(&self) -> impl Iterator<Item = &ReviewTask> {
        self.review_jobs
            .iter()
            .map(|r| &r.task)
            .chain(self.pending_reviews.iter().map(|p| &p.task))
    }

    /// The list of reviews: the ones running, the ones waiting to be read
    /// (newest first), then earlier ones when the list is open.
    pub fn review_rows(&self) -> Vec<ReviewRow<'_>> {
        let saved: &[Review] = match &self.popup {
            Some(Popup::Reviews { saved, .. }) => saved,
            _ => &[],
        };
        let waiting = |review: &Review| {
            self.reviews_ready
                .iter()
                .any(|r| r.session_id == review.session_id)
        };
        let running = self
            .review_jobs
            .iter()
            .map(|r| (&r.task, r.started))
            .map(|(task, started)| (task, Some(started)))
            .chain(self.pending_reviews.iter().map(|p| (&p.task, None)))
            .map(|(task, started)| ReviewRow::Running { task, started });
        let unread = self
            .reviews_ready
            .iter()
            .rev()
            .map(|review| ReviewRow::Done {
                review,
                unread: true,
            });
        let read = saved
            .iter()
            .filter(|review| !waiting(review))
            .map(|review| ReviewRow::Done {
                review,
                unread: false,
            });
        running.chain(unread).chain(read).collect()
    }

    /// `B`: every project's reviews, running and finished.
    fn open_reviews(&mut self) {
        let saved = review::recent(REVIEWS_LISTED);
        if saved.is_empty()
            && self.reviews_ready.is_empty()
            && self.reviews_in_progress().next().is_none()
        {
            return self.show_flash(
                "No reviews yet: b on a project reviews one of its branches",
                false,
            );
        }
        self.popup = Some(Popup::Reviews {
            state: ListState::default().with_selected(Some(0)),
            saved,
        });
    }

    fn handle_reviews_key(&mut self, code: KeyCode) {
        let rows = self.review_rows().len();
        let Some(Popup::Reviews { state, .. }) = &mut self.popup else {
            return;
        };
        let selected = state.selected().unwrap_or(0).min(rows.saturating_sub(1));
        match code {
            KeyCode::Esc | KeyCode::Char('q') => self.popup = None,
            KeyCode::Down | KeyCode::Char('j') if rows > 0 => {
                state.select(Some((selected + 1).min(rows - 1)))
            }
            KeyCode::Up | KeyCode::Char('k') => state.select(Some(selected.saturating_sub(1))),
            KeyCode::Enter => {
                let opened = match self.review_rows().get(selected) {
                    Some(ReviewRow::Done { review, .. }) => Ok((*review).clone()),
                    Some(ReviewRow::Running { task, .. }) => {
                        Err(format!("{} is still being reviewed", task.branch))
                    }
                    None => return,
                };
                match opened {
                    Ok(review) => {
                        self.reviews_ready
                            .retain(|r| r.session_id != review.session_id);
                        self.popup = Some(Popup::Findings {
                            review,
                            state: ListState::default().with_selected(Some(0)),
                            detail: None,
                        });
                    }
                    Err(msg) => self.show_flash(msg, false),
                }
            }
            _ => {}
        }
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
        // Reviews that would share a worktree in the cache can't overlap: the
        // second would check its branch out under the first.
        let folder = review::worktree_key(&task.repo_name, &task.branch);
        let same = |t: &ReviewTask| review::worktree_key(&t.repo_name, &t.branch) == folder;
        if self.review_jobs.iter().any(|r| same(&r.task))
            || self.pending_reviews.iter().any(|p| same(&p.task))
        {
            return self.show_flash(format!("{} is already being reviewed", task.branch), true);
        }
        if self.review_jobs.len() >= MAX_REVIEWS {
            return self.show_flash(
                format!("{MAX_REVIEWS} reviews are running; wait for one to end"),
                true,
            );
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
            let others = match self.review_jobs.len() {
                0 => String::new(),
                n => format!(" ({} running; B lists them)", n + 1),
            };
            self.show_flash(
                format!("Reviewing {} in the background…{others}", task.branch),
                false,
            );
        }
        self.review_jobs.push(RunningReview {
            task,
            started: Instant::now(),
            job,
        });
    }

    /// Saves a finished review. Its findings open right away only when it's
    /// the one thing going on (`alone`, nothing on screen, nothing waiting);
    /// otherwise it waits in the list (`B`), so reviews don't open over each
    /// other or over what you're doing.
    fn deliver_review(
        &mut self,
        task: &ReviewTask,
        worktree: PathBuf,
        result: Findings,
        alone: bool,
    ) {
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
        let saved = review::save(&review);
        self.git_at = None;
        if alone && self.popup.is_none() && self.reviews_ready.is_empty() {
            self.alert(
                "Review ready",
                &format!("{} has been reviewed", task.branch),
            );
            self.popup = Some(Popup::Findings {
                review,
                state: ListState::default().with_selected(Some(0)),
                detail: None,
            });
        } else {
            self.reviews_ready.push(review);
            self.alert(
                "Review ready",
                &format!("{} in {} · B lists reviews", task.branch, task.repo_name),
            );
        }
        if let Err(e) = saved {
            self.show_flash(format!("Could not save the review: {e}"), true);
        }
    }

    /// Interactive reviews in a tab or pane of their own whose session has
    /// ended: the tab marked it, or Claude Code, asked successfully
    /// (`live_known`), no longer reports a session it once did. A poll that
    /// failed says nothing about which sessions are open.
    fn settle_reviews(&mut self, live_known: bool) {
        for mut pending in std::mem::take(&mut self.pending_reviews) {
            let open = self.live.contains_key(&pending.task.session_id);
            pending.seen_open |= open;
            let marked = pending.done.as_ref().is_some_and(|file| file.exists());
            let gone = live_known && pending.seen_open && !open;
            if pending.elsewhere && (marked || gone) {
                if let Some(file) = &pending.done {
                    let _ = std::fs::remove_file(file);
                }
                self.reload_sessions();
                self.finish_review(pending, false);
            } else {
                self.pending_reviews.push(pending);
            }
        }
    }

    /// The interactive review that ran in claudash's own terminal just ended.
    fn settle_review_here(&mut self, session_id: &str) {
        if let Some(at) = self
            .pending_reviews
            .iter()
            .position(|p| !p.elsewhere && p.task.session_id == session_id)
        {
            let pending = self.pending_reviews.remove(at);
            self.finish_review(pending, true);
        }
    }

    /// The findings of an interactive review, read from its session's last
    /// reply. `alone`: nothing else was going on while it ran.
    fn finish_review(&mut self, pending: PendingReview, alone: bool) {
        let PendingReview { task, worktree, .. } = pending;
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
            Some(result) => self.deliver_review(&task, worktree, result, alone),
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
            let (text, _) = crate::audit::redact(&review::markdown(review));
            std::fs::write(&path, text).map(|()| path)
        });
        match result {
            Ok(path) => self.show_flash(format!("Exported to {}", paths::display(&path)), false),
            Err(e) => self.show_flash(format!("Could not export: {e}"), true),
        }
    }

    /// Opens a sub-view (conversation, inspector, logs); Esc there returns to
    /// the view it was opened from.
    pub(crate) fn enter_subview(&mut self, view: View) {
        if !matches!(
            self.view,
            View::Transcript | View::Inspect | View::Logs | View::Help
        ) {
            self.return_view = self.view;
        }
        self.view = view;
    }

    /// The last week's or month's numbers for the wrapped card.
    pub fn wrapped_stats(&self, month: bool) -> crate::wrapped::Stats {
        let period = if month {
            crate::wrapped::Period::Month
        } else {
            crate::wrapped::Period::Week
        };
        let (from, to) = period.range(chrono::Local::now().date_naive());
        crate::wrapped::stats(&self.sessions, |s| self.analysis(s), from, to)
    }

    /// Shows a view, doing what it needs when it opens.
    pub fn switch_view(&mut self, view: View) {
        match view {
            View::Logs => self.open_logs(None),
            View::Now => {
                self.view = View::Now;
                self.start_analysis();
            }
            _ => self.view = view,
        }
    }

    /// Where keys go right now, as named in `keys::BINDINGS`.
    pub fn context(&self) -> Context {
        match self.view {
            View::Sessions => Context::Sessions,
            View::Now if self.activity_focus == ActivityFocus::Background => Context::Background,
            View::Now if self.activity_focus == ActivityFocus::Alerts => Context::Alerts,
            View::Now => Context::Now,
            View::Projects => match &self.project_page {
                None => Context::Projects,
                Some(page) if !page.in_content => Context::ProjectMenu,
                Some(page) => match page.section {
                    Section::Sessions => Context::ProjectSessions,
                    Section::Specs => Context::Specs,
                    Section::Worktrees => Context::Worktrees,
                    Section::Snapshots => Context::Snapshots,
                    Section::Mcp => Context::Mcp,
                    Section::Setup => Context::Setup,
                    Section::Overview | Section::Security => Context::ProjectMenu,
                },
            },
            View::Insights => Context::Insights,
            View::Logs => Context::Logs,
            View::Inspect => Context::Inspect,
            View::Transcript => Context::Conversation,
            View::Help => Context::Help,
        }
    }

    /// `:` or Ctrl+P: every action that makes sense from here, found by name.
    fn open_palette(&mut self) {
        let mut available = vec![
            Context::Now,
            Context::Sessions,
            Context::Projects,
            Context::Insights,
        ];
        if !self.background.is_empty() {
            available.push(Context::Background);
        }
        // A project's sections, while its page is open.
        if self.project_page.is_some() {
            available.extend([
                Context::ProjectMenu,
                Context::ProjectSessions,
                Context::Specs,
                Context::Worktrees,
                Context::Snapshots,
                Context::Mcp,
                Context::Setup,
            ]);
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
            let section = match binding.context {
                Context::ProjectSessions => Some(Section::Sessions),
                Context::Specs => Some(Section::Specs),
                Context::Worktrees => Some(Section::Worktrees),
                Context::Snapshots => Some(Section::Snapshots),
                Context::Mcp => Some(Section::Mcp),
                Context::Setup => Some(Section::Setup),
                _ => None,
            };
            match binding.context {
                Context::Sessions => self.view = View::Sessions,
                Context::Now => {
                    self.switch_view(View::Now);
                    self.activity_focus = ActivityFocus::Open;
                }
                Context::Background => {
                    self.switch_view(View::Now);
                    self.activity_focus = ActivityFocus::Background;
                    if self.background_state.selected().is_none() {
                        self.background_state.select(Some(0));
                    }
                }
                Context::Projects => {
                    self.view = View::Projects;
                    self.project_page = None;
                }
                Context::ProjectMenu => {
                    self.view = View::Projects;
                    if let Some(page) = &mut self.project_page {
                        page.in_content = false;
                    }
                }
                Context::Insights => self.view = View::Insights,
                Context::Logs => self.switch_view(View::Logs),
                _ if section.is_some() => {
                    self.view = View::Projects;
                    if let Some(page) = &mut self.project_page {
                        page.section = section.unwrap_or(Section::Overview);
                        page.in_content = true;
                    }
                    self.enter_section();
                }
                _ => {}
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
        // On a project's page, that project.
        if self.view == View::Projects
            && let Some(page) = &self.project_page
        {
            return Ok(&page.dir);
        }
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
            EcoTab::Permissions => eco.permissions.len(),
        }
    }

    /// Adds rule suggestions to the loaded permissions: safe commands this
    /// project's sessions ran often in the last 30 days that no rule allows.
    fn suggest_rules(&mut self) {
        let Some((project, eco)) = &self.eco else {
            return;
        };
        let since = chrono::Local::now().date_naive() - chrono::Days::new(USAGE_WINDOW_DAYS);
        let mut uses: HashMap<String, u32> = HashMap::new();
        for s in &self.sessions {
            if let Some(dir) = project
                && !s.cwd.as_deref().is_some_and(|c| c.starts_with(dir))
            {
                continue;
            }
            let Some(a) = self.analysis(s) else {
                continue;
            };
            for ((day, label), stat) in &a.tool_output {
                if *day >= since
                    && let Some(command) = label.strip_prefix("Bash: ")
                {
                    *uses.entry(command.to_string()).or_default() += stat.calls;
                }
            }
        }
        let uses: Vec<(String, u32)> = uses.into_iter().collect();
        let rules: Vec<crate::permissions::Rule> = eco
            .permissions
            .iter()
            .filter(|r| r.kind != crate::permissions::Kind::Suggest)
            .cloned()
            .collect();
        let suggested = crate::permissions::suggestions(&rules, &uses, 10);
        if let Some((_, eco)) = &mut self.eco {
            eco.permissions = rules;
            eco.permissions.extend(suggested);
        }
    }

    /// Whether an allow rule matched anything in the last 30 days; `None` when
    /// claudash can't tell (file and domain patterns).
    pub fn rule_used(&self, rule: &str) -> Option<bool> {
        let window = Duration::from_secs(USAGE_WINDOW_DAYS * 86_400);
        let recent = self
            .sessions
            .iter()
            .filter(|s| s.modified.elapsed().is_ok_and(|age| age <= window))
            .filter_map(|s| self.analysis(s));
        if let Some(inner) = rule.strip_prefix("Bash(").and_then(|r| r.strip_suffix(')')) {
            let prefix = inner.trim_end_matches(":*").trim_end_matches('*').trim();
            if prefix.is_empty() {
                return None;
            }
            let mut recent = recent;
            return Some(recent.any(|a| {
                a.tool_output.keys().any(|(_, label)| {
                    label.strip_prefix("Bash: ").is_some_and(|key| {
                        key == prefix
                            || key.starts_with(&format!("{prefix} "))
                            || prefix.starts_with(&format!("{key} "))
                    })
                })
            }));
        }
        if rule.contains('(') {
            return None;
        }
        let mut recent = recent;
        Some(recent.any(|a| {
            a.tools.keys().any(|t| {
                t == rule || (rule.ends_with('*') && t.starts_with(rule.trim_end_matches('*')))
            })
        }))
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
        if let Some(job) = &self.history_secrets_job
            && let Some(result) = job.poll()
        {
            self.history_secrets = result.unwrap_or_default();
            self.history_secrets_job = None;
        }
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

        let mut ended = Vec::new();
        let mut i = 0;
        while i < self.review_jobs.len() {
            match self.review_jobs[i].job.poll() {
                Some(result) => {
                    let task = self.review_jobs.remove(i).task;
                    ended.push((task, result.unwrap_or_else(|()| Err("No result".into()))));
                }
                None => i += 1,
            }
        }
        // One that ends while nothing else runs may open its findings itself.
        let alone = ended.len() == 1 && self.review_jobs.is_empty();
        for (task, result) in ended {
            match result {
                Ok((worktree, Some(findings))) => {
                    self.reload_sessions();
                    self.deliver_review(&task, worktree, findings, alone);
                }
                Ok((worktree, None)) => {
                    let args = review::interactive_args(
                        task.mode,
                        &task.branch,
                        &task.base,
                        &task.session_id,
                    );
                    let title = format!("review {}", task.branch);
                    let done = review::ended_marker(&task.session_id);
                    let elsewhere =
                        self.open_elsewhere_in(&args, &worktree, &title, done.as_deref());
                    if !elsewhere {
                        self.pending_commands.push_back(PendingCommand {
                            args,
                            cwd: worktree.clone(),
                            review: Some(task.session_id.clone()),
                        });
                    }
                    self.pending_reviews.push(PendingReview {
                        task,
                        worktree,
                        elsewhere,
                        done: done.filter(|_| elsewhere),
                        seen_open: false,
                    });
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

        let mut removed = Vec::new();
        self.removals.retain(|job| match job.poll() {
            Some(result) => {
                removed.push(result.unwrap_or_else(|()| Err("No result".into())));
                false
            }
            None => true,
        });
        for result in removed {
            match result {
                Ok(msg) => self.show_flash(msg, false),
                Err(e) => self.show_flash(e, true),
            }
            // Checkouts changed, and a session may have been stopped.
            self.git_at = None;
            self.live_job = None;
            self.refreshed_at = Instant::now() - REFRESH_EVERY;
        }

        let mut finished = Vec::new();
        let mut i = 0;
        while i < self.background_jobs.len() {
            match self.background_jobs[i].1.poll() {
                Some(result) => finished.push((self.background_jobs.remove(i).0, result)),
                None => i += 1,
            }
        }
        for (what, result) in finished {
            match result.unwrap_or_else(|()| Err("No result".into())) {
                Ok(msg) if what == "continue" => {
                    self.alert("Claude plan: limit reset", &msg);
                    self.show_flash(format!("{msg}; they're under Background in Now"), false)
                }
                Err(e) if what == "continue" => self.show_flash(e, true),
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
            self.suggest_rules();
        }
        if let Some(job) = &self.git_job
            && let Some(result) = job.poll()
        {
            if let Ok((model, git)) = result {
                self.projects = model;
                self.git = git;
                let cards = self.cards().len();
                self.project_cursor = self.project_cursor.min(cards.saturating_sub(1));
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
            let all = result.ok().and_then(Result::ok);
            let live_known = all.is_some();
            let all = all.unwrap_or_default();
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
            self.settle_reviews(live_known);
        }

        if let Some((project, job)) = &self.eco_job
            && let Some(result) = job.poll()
        {
            let eco = result.unwrap_or_default();
            self.eco = Some((project.clone(), eco));
            self.eco_job = None;
            self.suggest_rules();
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
                            "{} denied: headless runs can't ask for permission",
                            crate::ui::plural(reply.permission_denials as u64, "tool call")
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
            KeyCode::Char('w') => {
                self.popup = Some(Popup::Wrapped {
                    month: false,
                    redact: false,
                });
                return;
            }
            KeyCode::Char('r') => return self.refresh_all(),
            KeyCode::Char('B') => return self.open_reviews(),
            KeyCode::Char('?') => return self.toggle_help(),
            _ => {}
        }
        match self.view {
            View::Sessions => self.handle_sessions_key(key.code),
            View::Insights => self.handle_insights_key(key.code),
            View::Transcript => self.handle_transcript_key(key.code),
            View::Projects => self.handle_projects_key(key.code),
            View::Now => self.handle_activity_key(key.code),
            View::Logs => self.handle_logs_key(key.code),
            View::Inspect => match key.code {
                KeyCode::Esc => self.view = self.return_view,
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
            KeyCode::Esc => self.view = self.return_view,
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
                    self.enter_subview(View::Inspect);
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
                if let Some(dir) = self.selected_session().and_then(|s| s.cwd.clone()) {
                    self.open_project_of(&dir);
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
            KeyCode::Esc | KeyCode::Left => self.leave_section(),
            KeyCode::Enter => self.open_mcp_log(),
            // The full Logs view on this server: follows new lines, filters.
            KeyCode::Char('l') => {
                let (Some(project), Some(i)) = (&self.project, self.mcp_state.selected()) else {
                    return;
                };
                let Some(dir) = self
                    .project_servers()
                    .and_then(|s| s.get(i))
                    .and_then(|s| mcp::log_dir(&project.cwd, &s.full_name))
                else {
                    return;
                };
                self.open_logs(Some(LogKind::Mcp(dir)));
            }
            KeyCode::Char('a') | KeyCode::Char('L') => {
                let (Some(project), Some(i)) = (&self.project, self.mcp_state.selected()) else {
                    return;
                };
                let Some(server) = self.project_servers().and_then(|s| s.get(i)) else {
                    return;
                };
                let (name, cwd) = (server.full_name.clone(), project.cwd.clone());
                if code == KeyCode::Char('a') {
                    self.pending_commands.push_back(PendingCommand {
                        args: vec!["mcp".into(), "login".into(), name],
                        cwd,
                        review: None,
                    });
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
            KeyCode::Esc | KeyCode::Left => self.leave_section(),
            KeyCode::Tab => {
                self.eco_tab = EcoTab::ALL[(tab + 1) % EcoTab::ALL.len()];
            }
            KeyCode::BackTab => {
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
        if matches!(self.popup, Some(Popup::Reviews { .. })) {
            return self.handle_reviews_key(code);
        }
        match &mut self.popup {
            // Handled above: its rows are computed from the whole app.
            Some(Popup::Reviews { .. }) => {}
            Some(Popup::Wrapped { month, redact }) => match code {
                KeyCode::Esc | KeyCode::Char('q') => self.popup = None,
                KeyCode::Tab => *month = !*month,
                KeyCode::Char('x') => *redact = !*redact,
                KeyCode::Char('e') => {
                    let (month, redact) = (*month, *redact);
                    let stats = self.wrapped_stats(month);
                    let result = exports_dir().and_then(|dir| {
                        let path = dir.join(format!(
                            "{}-claudash-wrapped-{}.md",
                            chrono::Local::now().format("%Y-%m-%d"),
                            if month { "month" } else { "week" }
                        ));
                        let text = format!("```\n{}```\n", crate::wrapped::plain(&stats, redact));
                        std::fs::write(&path, text).map(|()| path)
                    });
                    match result {
                        Ok(path) => {
                            self.show_flash(format!("Exported to {}", paths::display(&path)), false)
                        }
                        Err(e) => self.show_flash(format!("Could not export: {e}"), true),
                    }
                }
                _ => {}
            },
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
                let yes = action.key();
                if matches!(code, KeyCode::Char(c) if c == yes || (yes == 'y' && c == 'Y')) {
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
/// Where tokens went over the last days, from every session's analysis.
#[derive(Default)]
pub struct TokenReport {
    /// Tool output that entered the context, by tool (Bash by command).
    pub outputs: Vec<(String, analysis::OutputStat)>,
    /// The most expensive prompts: session title, project, prompt.
    pub prompts: Vec<(String, String, analysis::PromptCost)>,
    /// (replies, output tokens) in the window and in the 30 days before it.
    pub replies: ((u32, u64), (u32, u64)),
    /// (tool calls, output bytes) in the window and in the 30 days before it.
    pub tool_calls: ((u32, u64), (u32, u64)),
}

impl App {
    pub fn token_report(&self, days: i64) -> TokenReport {
        let today = chrono::Local::now().date_naive();
        let start = today - chrono::Days::new(days as u64 - 1);
        let prior_start = start - chrono::Days::new(30);
        let mut report = TokenReport::default();
        let mut outputs: HashMap<String, analysis::OutputStat> = HashMap::new();
        for session in &self.sessions {
            let Some(a) = self.analysis(session) else {
                continue;
            };
            for ((day, label), stat) in &a.tool_output {
                let slot = if *day >= start {
                    let o = outputs.entry(label.clone()).or_default();
                    o.calls += stat.calls;
                    o.bytes += stat.bytes;
                    &mut report.tool_calls.0
                } else if *day >= prior_start {
                    &mut report.tool_calls.1
                } else {
                    continue;
                };
                slot.0 += stat.calls;
                slot.1 += stat.bytes;
            }
            for (day, (n, output)) in &a.replies {
                let slot = if *day >= start {
                    &mut report.replies.0
                } else if *day >= prior_start {
                    &mut report.replies.1
                } else {
                    continue;
                };
                slot.0 += n;
                slot.1 += output;
            }
            for p in &a.prompts {
                if p.at.is_some_and(|t| t.date_naive() >= start) {
                    report.prompts.push((
                        session.title.clone(),
                        session.project_path.clone(),
                        p.clone(),
                    ));
                }
            }
        }
        report.outputs = outputs.into_iter().collect();
        report
            .outputs
            .sort_by_key(|(_, s)| std::cmp::Reverse(s.bytes));
        report
            .prompts
            .sort_by_key(|(_, _, p)| std::cmp::Reverse(p.usage.processed()));
        report.prompts.truncate(15);
        report
    }
}

/// Token savers that are installed: caveman (a skill or plugin) and rtk (a
/// command, often wired in as a hook).
fn token_savers() -> Vec<String> {
    let mut found = Vec::new();
    let home = paths::claude_home();
    let mentions = |file: PathBuf, what: &str| {
        std::fs::read_to_string(file).is_ok_and(|t| t.to_lowercase().contains(what))
    };
    let caveman = home.as_ref().is_some_and(|h| {
        std::fs::read_dir(h.join("skills"))
            .into_iter()
            .flatten()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().contains("caveman"))
            || mentions(h.join("plugins/installed_plugins.json"), "caveman")
            || mentions(h.join("settings.json"), "caveman")
    });
    if caveman {
        found.push("caveman".to_string());
    }
    let rtk_hook = home
        .as_ref()
        .is_some_and(|h| mentions(h.join("settings.json"), "rtk "));
    if claude_cli::which("rtk").is_some() || rtk_hook {
        found.push(if rtk_hook { "rtk (hook)" } else { "rtk" }.to_string());
    }
    found
}

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
/// "1 secret" / "3 secrets".
fn plural_secrets(n: usize) -> String {
    if n == 1 {
        "1 secret".into()
    } else {
        format!("{n} secrets")
    }
}

/// Writes the conversation with any credentials masked; returns how many.
fn export_markdown(view: &TranscriptView) -> io::Result<(PathBuf, usize)> {
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
    let (text, removed) = crate::audit::redact(&markdown_for(view));
    std::fs::write(&path, text)?;
    Ok((path, removed))
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
        app.specs_state.select(Some(0));
        app.snapshots = ["/r", "/r/.claude/worktrees/w"]
            .map(|root| {
                (
                    PathBuf::from(root),
                    crate::snapshots::Snapshot {
                        sha: "0123456789abcdef".into(),
                        at: 1_759_000_000,
                        label: "after reply".into(),
                        session_id: "x".into(),
                        files: 3,
                    },
                )
            })
            .to_vec();
        app.snapshots_at = Some((PathBuf::from("/r"), Instant::now()));
        let hit = Hit {
            session_id: "x".into(),
            title: "t".into(),
            entry: 0,
            snippet: "…a snippet…".into(),
            at: None,
        };
        for (w, h) in [(1, 1), (10, 3), (20, 5), (40, 10), (80, 24), (200, 60)] {
            // Every screen: the views, each project section and each Insights section.
            let mut screens: Vec<(View, Option<Section>, InsightsSection)> = [
                View::Now,
                View::Sessions,
                View::Projects,
                View::Logs,
                View::Transcript,
                View::Help,
                View::Inspect,
            ]
            .into_iter()
            .map(|v| (v, None, InsightsSection::Usage))
            .collect();
            screens.extend(Section::ALL.map(|s| (View::Projects, Some(s), InsightsSection::Usage)));
            screens.extend(InsightsSection::ALL.map(|i| (View::Insights, None, i)));
            for (k, (view, section, insights)) in screens.into_iter().enumerate() {
                let tabs: &[EcoTab] = if section == Some(Section::Setup) {
                    &EcoTab::ALL
                } else {
                    &EcoTab::ALL[..1]
                };
                for &tab in tabs {
                    for popup in 0..6 {
                        app.view = view;
                        app.insights = insights;
                        app.project_page = section.map(|section| ProjectPage {
                            dir: PathBuf::from("/r"),
                            section,
                            in_content: popup % 2 == 1,
                            sessions_state: Default::default(),
                            worktrees_state: Default::default(),
                            snapshots_state: Default::default(),
                            scroll: u16::MAX,
                        });
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
                            4 if k % 2 == 0 => Some(Popup::Wrapped {
                                month: k % 4 == 0,
                                redact: k % 3 == 0,
                            }),
                            4 => Some(Popup::Results {
                                query: "q".into(),
                                hits: vec![hit.clone()],
                                state: ListState::default(),
                            }),
                            5 if k % 6 == 0 => Some(Popup::Summary {
                                markdown: "# Title\n\n- a line\n".repeat(20),
                                scroll: u16::MAX,
                            }),
                            5 if k % 6 == 1 => Some(Popup::Branches {
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
                            5 if k % 6 == 2 || k % 6 == 3 => {
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
                                    detail: (k % 6 == 3).then_some(u16::MAX),
                                })
                            }
                            5 if k % 6 == 4 => Some(Popup::ReviewSetup {
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
                            5 if k % 6 == 5 => {
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
                        app.no_color = k % 2 == 0;
                        app.activity_focus = if popup == 3 {
                            ActivityFocus::Alerts
                        } else {
                            ActivityFocus::Open
                        };
                        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
                        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
                    }
                }
            }
        }
    }

    #[test]
    fn draws_worktrees_and_the_list_of_reviews_at_any_size() {
        use crate::{
            projects::{Checkout, Repo},
            worktree::{Holder, Lock},
        };
        use ratatui::{Terminal, backend::TestBackend};
        let mut app = App::new(1_000_000, false);
        let checkout = |path: &str, holder: Option<Holder>, prunable: bool| Checkout {
            path: PathBuf::from(path),
            branch: Some("a-branch-with-quite-a-long-name/for-a-narrow-terminal".into()),
            status: (!prunable).then(|| git::Status {
                changed: 12,
                untracked: 5,
                ahead: 3,
                ..Default::default()
            }),
            main: path == "/r",
            lock: holder.map(|holder| Lock {
                reason: Some("claude agent agent-a1 (pid 4242)".into()),
                holder,
            }),
            prunable,
            claude_created: path.contains("/.claude/"),
            review: path.contains("/reviews/"),
        };
        app.projects.repos = vec![Repo {
            name: "r".into(),
            checkouts: vec![
                checkout("/r", None, false),
                checkout("/r/.claude/worktrees/a", Some(Holder::Running(4242)), false),
                checkout("/r/.claude/worktrees/b", Some(Holder::Ended(4242)), false),
                checkout("/cache/reviews/r/c", Some(Holder::Unknown), false),
                checkout("/gone", None, true),
            ],
        }];
        let done = Review {
            session_id: "s1".into(),
            repo: PathBuf::from("/r"),
            branch: "feature/with-a-long-name".into(),
            base: "origin/main".into(),
            worktree: PathBuf::from("/cache/reviews/r/c"),
            mode: Mode::Static,
            at: 1_759_000_000,
            result: Findings {
                summary: "A summary long enough to run past the edge of a small terminal.".into(),
                findings: vec![review::Finding {
                    file: "src/lib.rs".into(),
                    severity: "high".into(),
                    comment: "A comment.".into(),
                    ..Default::default()
                }],
            },
        };
        app.reviews_ready = vec![done.clone()];
        app.review_jobs.push(RunningReview {
            task: review_task("/r", "feat/x"),
            started: Instant::now(),
            job: Job::spawn(|| Err("never polled".into())),
        });
        for (w, h) in [(1, 1), (10, 3), (20, 5), (40, 10), (80, 24), (200, 60)] {
            for popup in [false, true] {
                app.view = View::Projects;
                app.project_page = Some(ProjectPage {
                    dir: PathBuf::from("/r"),
                    section: Section::Worktrees,
                    in_content: true,
                    sessions_state: Default::default(),
                    worktrees_state: ratatui::widgets::TableState::default().with_selected(Some(4)),
                    snapshots_state: Default::default(),
                    scroll: 0,
                });
                app.popup = popup.then(|| Popup::Reviews {
                    state: ListState::default().with_selected(Some(2)),
                    saved: vec![Review {
                        session_id: "s2".into(),
                        ..done.clone()
                    }],
                });
                let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
                terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
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
        for c in "insights usage".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.popup.is_none());
        assert!(app.view == View::Insights);

        // A command of another view goes there first.
        app.handle_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
        for c in "days or months".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        app.view = View::Sessions;
        let before = app.monthly;
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.view == View::Insights);
        assert_ne!(app.monthly, before);

        // Number keys follow the tab order.
        app.handle_key(KeyEvent::new(KeyCode::Char('3'), KeyModifiers::NONE));
        assert!(app.view == View::Projects);
        app.handle_key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE));
        // Esc goes back but never quits.
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.view == View::Sessions && !app.should_quit);
    }

    #[test]
    fn alerts_take_you_where_they_point() {
        let mut app = App::new(1_000_000, false);
        app.projects.loose = vec![PathBuf::from("/r")];
        let change = |id: &str| specs::Change {
            framework: specs::Framework::OpenSpec,
            id: id.into(),
            path: PathBuf::from("/r/openspec/changes").join(id),
            done: 1,
            total: 2,
            stage: specs::Stage::Implementing,
            modified: std::time::SystemTime::UNIX_EPOCH,
            next: None,
            next_label: None,
        };
        app.projects.specs.insert(
            PathBuf::from("/r"),
            specs::ProjectSpecs {
                frameworks: vec![specs::Framework::OpenSpec],
                changes: vec![change("a"), change("b")],
            },
        );

        // A stale change is among the alerts and goes to its Specs, selected.
        let alerts = app.alerts();
        let stale = alerts
            .iter()
            .position(|a| matches!(a, Alert::Problem(Problem::StaleChange { id, .. }) if id == "b"))
            .expect("stale change alert");
        app.view = View::Now;
        app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert!(app.activity_focus == ActivityFocus::Alerts);
        app.alert_cursor = stale;
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.view == View::Projects);
        let page = app.project_page.as_ref().unwrap();
        assert_eq!((page.section, page.in_content), (Section::Specs, true));
        assert_eq!(app.specs_state.selected(), Some(1));

        // Credentials in the prompt history go to Insights › Security.
        let history = Alert::Problem(Problem::Secrets {
            session: String::new(),
            session_id: String::new(),
            found: vec![],
        });
        assert_eq!(app.alert_target(&history), Some(Target::InsightsSecurity));
        app.go_to(Target::InsightsSecurity);
        assert!(app.view == View::Insights && app.insights == InsightsSection::Security);
    }

    #[test]
    fn says_when_a_window_that_ran_close_resets() {
        let mut app = App::new(1_000_000, false);
        let now = chrono::Utc::now().timestamp();
        app.statusline.rate_limits = Some((
            statusline::RateLimits {
                five_hour: Some(statusline::Window {
                    used_percentage: 96.0,
                    resets_at: now + 600,
                }),
                seven_day: None,
            },
            std::time::SystemTime::now(),
        ));
        app.check_plan_alerts();
        assert_eq!(app.limit_reset_due, Some(("5-hour", now + 600)));
        // Ten minutes later the window has reset.
        app.statusline.rate_limits = None;
        app.limit_reset_due = Some(("5-hour", now - 1));
        app.check_plan_alerts();
        assert!(app.limit_reset_due.is_none());
        assert!(
            app.flash
                .as_ref()
                .is_some_and(|f| f.0.contains("5-hour limit reset"))
        );
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
    fn offers_to_continue_sessions_a_limit_stopped() {
        let mut app = App::new(1_000_000, false);
        let now = chrono::Utc::now().timestamp();
        let stopped = |id: &str, resets_at: i64| Session {
            id: id.into(),
            path: PathBuf::from(format!("/t/{id}.jsonl")),
            title: format!("title {id}"),
            project_path: "~/repo".into(),
            cwd: Some(std::env::temp_dir()),
            git_branch: None,
            modified: std::time::SystemTime::now(),
            size: 0,
            tokens: crate::sessions::SessionTokens {
                limit_stop: Some(crate::sessions::LimitStop {
                    at: now - 600,
                    resets_at,
                    window: "five_hour".into(),
                }),
                ..Default::default()
            },
        };
        // Before the reset: Enter has them continue when it comes.
        app.sessions = vec![stopped("a", now + 3600)];
        let alerts = app.alerts();
        assert!(matches!(
            alerts[0],
            Alert::LimitStopped {
                closed: 1,
                ready: 0,
                open: 0,
                next_reset: Some(_),
                queued: false
            }
        ));
        app.go_to(app.alert_target(&alerts[0]).unwrap());
        assert!(app.continue_at_reset && app.popup.is_none());
        // After it: Enter asks before continuing them.
        app.continue_at_reset = false;
        app.sessions = vec![stopped("a", now - 60)];
        app.go_to(Target::ContinueStopped);
        assert!(matches!(
            app.popup,
            Some(Popup::Confirm {
                action: Confirm::ContinueStopped,
                ..
            })
        ));
        // Continued sessions drop out of the alert.
        app.continued.insert("a".into());
        assert!(app.alerts().is_empty());
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
            lock: None,
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

    #[test]
    fn opens_sessions_in_a_tab_when_a_multiplexer_is_there() {
        use crate::launch::{Mux, Place};
        let mut app = App::new(1_000_000, false);
        let args = vec!["--resume".to_string(), "abc".to_string()];
        let queued = |app: &App| -> Vec<Vec<String>> {
            app.pending_commands
                .iter()
                .map(|c| c.args.clone())
                .collect()
        };

        // No multiplexer: claudash hands its own terminal over, as always.
        app.open_claude(args.clone(), PathBuf::from("/tmp"), "Fix login");
        assert_eq!(queued(&app), std::slice::from_ref(&args));
        // A second one waits its turn instead of replacing the first.
        let other = vec!["attach".to_string(), "bg1".to_string()];
        app.open_claude(other.clone(), PathBuf::from("/tmp"), "Background");
        assert_eq!(queued(&app), [args.clone(), other]);

        // In Zellij it asks for a tab and stays on screen.
        app.pending_commands.clear();
        app.mux = Some(Mux::Zellij);
        app.launcher = |_, _, _| Ok(());
        app.open_claude(args.clone(), PathBuf::from("/tmp"), "Fix login");
        assert!(app.pending_commands.is_empty());
        let (message, is_error, _) = app.flash.clone().expect("says where it went");
        assert!(!is_error && message.contains("Fix login") && message.contains("Zellij tab"));

        // `open_in = "here"` keeps the old way.
        app.open_in = Place::Here;
        app.open_claude(args.clone(), PathBuf::from("/tmp"), "Fix login");
        assert_eq!(queued(&app), std::slice::from_ref(&args));

        // The multiplexer refused: it opens here and says why.
        app.pending_commands.clear();
        app.open_in = Place::Tab;
        app.launcher = |_, _, _| Err("no active session".into());
        app.open_claude(args.clone(), PathBuf::from("/tmp"), "Fix login");
        assert_eq!(queued(&app), std::slice::from_ref(&args));
        let (message, is_error, _) = app.flash.clone().expect("says why");
        assert!(
            is_error && message.contains("no active session"),
            "{message}"
        );
    }

    fn review_task(repo: &str, branch: &str) -> ReviewTask {
        ReviewTask {
            repo: PathBuf::from(repo),
            repo_name: repo.trim_start_matches('/').to_string(),
            branch: branch.into(),
            reference: format!("origin/{branch}"),
            base: "origin/main".into(),
            mode: Mode::Static,
            session_id: format!("session-{branch}"),
        }
    }

    /// A static review of `branch` that has just finished, with one finding.
    fn finished_review(app: &mut App, repo: &str, branch: &str) {
        let findings = Findings {
            summary: format!("About {branch}."),
            findings: vec![review::Finding {
                file: "src/lib.rs".into(),
                comment: "A comment.".into(),
                ..Default::default()
            }],
        };
        app.review_jobs.push(RunningReview {
            task: review_task(repo, branch),
            started: Instant::now(),
            job: Job::spawn(move || Ok((PathBuf::from("/wt"), Some(findings)))),
        });
    }

    fn finish_reviews(app: &mut App) {
        let started = Instant::now();
        while !app.review_jobs.is_empty() {
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "review never finished"
            );
            std::thread::sleep(Duration::from_millis(10));
            app.poll_jobs();
        }
    }

    #[test]
    fn reviews_several_branches_at_once_but_each_only_once() {
        let mut app = App::new(1_000_000, false);
        app.start_review(review_task("/api", "feat/x"));
        app.start_review(review_task("/web", "feat/y"));
        assert_eq!(app.review_jobs.len(), 2);

        // The same branch of the same repository shares one worktree.
        app.start_review(review_task("/api", "feat/x"));
        assert_eq!(app.review_jobs.len(), 2);
        let (message, is_error, _) = app.flash.clone().expect("says why not");
        assert!(is_error && message.contains("feat/x"), "{message}");

        // So do two that would land in the same folder of the cache: another
        // repository called the same, a branch whose name differs in a slash.
        let twin = ReviewTask {
            repo: PathBuf::from("/elsewhere/api"),
            branch: "feat-x".into(),
            ..review_task("/api", "feat/x")
        };
        app.start_review(twin);
        assert_eq!(app.review_jobs.len(), 2);

        for branch in ["a", "b"] {
            app.start_review(review_task("/cli", branch));
        }
        assert_eq!(app.review_jobs.len(), MAX_REVIEWS);
        app.start_review(review_task("/cli", "c"));
        assert_eq!(app.review_jobs.len(), MAX_REVIEWS);
    }

    #[test]
    fn a_review_that_ends_alone_opens_and_the_rest_wait_in_the_list() {
        let mut app = App::new(1_000_000, false);
        finished_review(&mut app, "/api", "feat/x");
        finish_reviews(&mut app);
        assert!(
            matches!(&app.popup, Some(Popup::Findings { review, .. }) if review.branch == "feat/x")
        );
        assert!(app.reviews_ready.is_empty());

        // Two more end while that one is being read: nothing opens over it.
        finished_review(&mut app, "/api", "feat/y");
        finished_review(&mut app, "/web", "fix/z");
        finish_reviews(&mut app);
        assert!(
            matches!(&app.popup, Some(Popup::Findings { review, .. }) if review.branch == "feat/x")
        );
        assert_eq!(app.reviews_ready.len(), 2);

        // `B` lists them; Enter opens one, which stops waiting.
        app.popup = None;
        press(&mut app, 'B');
        let Some(Popup::Reviews { state, .. }) = &app.popup else {
            panic!("the list of reviews");
        };
        assert_eq!(state.selected(), Some(0));
        assert_eq!(app.review_rows().len(), 2);
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(&app.popup, Some(Popup::Findings { .. })));
        assert_eq!(app.reviews_ready.len(), 1);
    }

    #[test]
    fn two_reviews_running_together_both_wait_to_be_read() {
        let mut app = App::new(1_000_000, false);
        finished_review(&mut app, "/api", "feat/x");
        finished_review(&mut app, "/web", "fix/z");
        finish_reviews(&mut app);
        // Neither takes the screen from the other: both wait in the list.
        assert!(app.popup.is_none());
        assert_eq!(app.reviews_ready.len(), 2);
    }

    #[test]
    fn background_commands_run_side_by_side_but_not_twice() {
        let mut app = App::new(1_000_000, false);
        app.start_background_command("stop", "bg1");
        app.start_background_command("stop", "bg2");
        assert_eq!(app.background_jobs.len(), 2);

        // The same command for the same session is already on its way.
        app.start_background_command("stop", "bg1");
        assert_eq!(app.background_jobs.len(), 2);
        let (message, is_error, _) = app.flash.clone().expect("says why not");
        assert!(is_error && message.contains("bg1"), "{message}");
    }

    #[test]
    fn snapshots_are_turned_on_and_off_from_their_section() {
        let mut app = App::new(1_000_000, false);
        app.view = View::Projects;
        app.project_page = Some(ProjectPage {
            dir: PathBuf::from("/r"),
            section: Section::Snapshots,
            in_content: true,
            sessions_state: Default::default(),
            worktrees_state: Default::default(),
            snapshots_state: Default::default(),
            scroll: 0,
        });
        assert!(!app.snapshots_on);

        press(&mut app, 'T');
        let Some(Popup::Confirm { lines, action, .. }) = &app.popup else {
            panic!("it asks first");
        };
        assert_eq!(*action, Confirm::Snapshots { on: true });
        let text = lines.join("\n");
        assert!(text.contains("before each prompt") && text.contains("config.toml"));

        // Tests have no settings file to write: it says so and stays off.
        press(&mut app, 'y');
        assert!(!app.snapshots_on);
        assert!(app.flash.as_ref().is_some_and(|(_, is_error, _)| *is_error));

        // With no snapshot to open, Enter offers the same.
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(
            &app.popup,
            Some(Popup::Confirm {
                action: Confirm::Snapshots { on: true },
                ..
            })
        ));

        // On: the same key turns them off, and says the ones taken are kept.
        app.popup = None;
        app.snapshots_on = true;
        press(&mut app, 'T');
        let Some(Popup::Confirm { lines, action, .. }) = &app.popup else {
            panic!("it asks first");
        };
        assert_eq!(*action, Confirm::Snapshots { on: false });
        assert!(lines.join("\n").contains("are kept"));
    }

    /// An app on the Worktrees of the repository at `root`, the cursor on
    /// the checkout whose folder is called `name`.
    fn app_on_worktree(root: &Path, name: &str) -> App {
        let mut app = App::new(1_000_000, false);
        let (model, statuses) = crate::projects::build(&[root.to_path_buf()]);
        let selected = model.repos[0]
            .checkouts
            .iter()
            .position(|c| c.path.ends_with(name))
            .expect("a checkout with that name");
        // The folder as git spells it, which is how the app knows projects.
        let dir = model.repos[0].checkouts[0].path.clone();
        app.projects = model;
        app.git = statuses;
        app.view = View::Projects;
        app.project_page = Some(ProjectPage {
            dir,
            section: Section::Worktrees,
            in_content: true,
            sessions_state: Default::default(),
            worktrees_state: ratatui::widgets::TableState::default().with_selected(Some(selected)),
            snapshots_state: Default::default(),
            scroll: 0,
        });
        app
    }

    fn press(app: &mut App, key: char) {
        app.handle_key(KeyEvent::new(KeyCode::Char(key), KeyModifiers::NONE));
    }

    fn finish_removals(app: &mut App) {
        let started = Instant::now();
        while !app.removals.is_empty() {
            assert!(
                started.elapsed() < Duration::from_secs(20),
                "removal never finished"
            );
            std::thread::sleep(Duration::from_millis(20));
            app.poll_jobs();
        }
    }

    #[test]
    fn removing_a_worktree_with_work_lists_it_and_asks_with_another_key() {
        let Some((root, run)) = crate::git::testing::scratch_repo("app-remove") else {
            return;
        };
        assert!(run(&["commit", "-q", "--allow-empty", "-m", "init"]));
        assert!(run(&["worktree", "add", "-q", "wt", "-b", "spike"]));
        std::fs::write(root.join("wt/notes.txt"), "work").unwrap();
        let mut app = app_on_worktree(&root, "wt");

        press(&mut app, 'D');
        let Some(Popup::Confirm { lines, action, .. }) = &app.popup else {
            panic!("it asks first");
        };
        assert!(lines.iter().any(|l| l.contains("untracked  notes.txt")));
        assert!(lines.iter().any(|l| l.contains("spike")));
        assert_eq!(action.key(), 'D');
        // `y` answers questions that lose nothing; this one deletes a file.
        press(&mut app, 'y');
        assert!(app.popup.is_some() && root.join("wt/notes.txt").exists());
        press(&mut app, 'D');
        assert!(app.popup.is_none());
        finish_removals(&mut app);
        assert!(!root.join("wt").exists());
        let (message, is_error, _) = app.flash.as_ref().expect("says what happened");
        assert!(!is_error && message.contains("Removed"), "{message}");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_worktree_open_in_another_terminal_names_the_session() {
        let Some((root, run)) = crate::git::testing::scratch_repo("app-open") else {
            return;
        };
        assert!(run(&["commit", "-q", "--allow-empty", "-m", "init"]));
        assert!(run(&["worktree", "add", "-q", "wt", "-b", "spike"]));
        let mut app = app_on_worktree(&root, "wt");
        let worktree = app.projects.repos[0]
            .checkouts
            .iter()
            .find(|c| c.path.ends_with("wt"))
            .map(|c| c.path.clone())
            .unwrap();
        app.sessions = vec![Session {
            id: "s1".into(),
            path: PathBuf::from("/t/s1.jsonl"),
            title: "Add OAuth login".into(),
            project_path: "~/repo".into(),
            cwd: Some(worktree),
            git_branch: None,
            modified: std::time::SystemTime::now(),
            size: 0,
            tokens: Default::default(),
        }];
        let live = LiveSession {
            session_id: "s1".into(),
            kind: "interactive".into(),
            status: "idle".into(),
            pid: Some(std::process::id()),
            ..Default::default()
        };
        app.live = HashMap::from([("s1".to_string(), live)]);

        press(&mut app, 'D');
        assert!(app.popup.is_none());
        let (message, is_error, _) = app.flash.as_ref().expect("says why not");
        assert!(
            *is_error && message.contains("Add OAuth login"),
            "{message}"
        );
        assert!(root.join("wt").exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_session_claude_code_reports_inside_a_worktree_keeps_it() {
        let Some((root, run)) = crate::git::testing::scratch_repo("app-live") else {
            return;
        };
        assert!(run(&["commit", "-q", "--allow-empty", "-m", "init"]));
        assert!(run(&["worktree", "add", "-q", "wt", "-b", "spike"]));
        let mut app = app_on_worktree(&root, "wt");
        let worktree = app.projects.repos[0]
            .checkouts
            .iter()
            .find(|c| c.path.ends_with("wt"))
            .map(|c| c.path.clone())
            .unwrap();
        // No transcript loaded for it yet, and it works in a folder below.
        let live = LiveSession {
            session_id: "s9".into(),
            kind: "interactive".into(),
            status: "busy".into(),
            pid: Some(std::process::id()),
            name: Some("Port the importer".into()),
            cwd: Some(worktree.join("src").to_string_lossy().into_owned()),
            ..Default::default()
        };
        app.live = HashMap::from([("s9".to_string(), live)]);

        press(&mut app, 'D');
        assert!(app.popup.is_none());
        let (message, is_error, _) = app.flash.clone().expect("says why not");
        assert!(
            is_error && message.contains("Port the importer"),
            "{message}"
        );
        // The cleanup leaves it out too.
        press(&mut app, 'C');
        assert!(app.popup.is_none() && root.join("wt").exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_review_in_a_tab_ends_with_its_session_not_with_a_failed_poll() {
        let mut app = App::new(1_000_000, false);
        let dir = std::env::temp_dir().join(format!("claudash-ended-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let pending = |done: Option<PathBuf>, seen_open: bool| PendingReview {
            task: review_task("/api", "feat/x"),
            worktree: PathBuf::from("/wt"),
            elsewhere: true,
            done,
            seen_open,
        };

        // Seen open before; now Claude Code couldn't say which sessions are
        // open, which settles nothing.
        app.pending_reviews.push(pending(None, true));
        app.settle_reviews(false);
        assert_eq!(app.pending_reviews.len(), 1);
        // It could, and the session isn't among them: it ended.
        app.settle_reviews(true);
        assert!(app.pending_reviews.is_empty());

        // The session's own tab says when it ends, whatever the polls say.
        let done = dir.join("session-feat-x");
        app.pending_reviews.push(pending(Some(done.clone()), false));
        app.settle_reviews(true);
        assert_eq!(
            app.pending_reviews.len(),
            1,
            "never seen open, not marked ended"
        );
        std::fs::write(&done, "").unwrap();
        app.settle_reviews(false);
        assert!(app.pending_reviews.is_empty() && !done.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_long_question_keeps_its_keys_on_screen() {
        use ratatui::{Terminal, backend::TestBackend};
        let mut app = App::new(1_000_000, false);
        app.popup = Some(Popup::Confirm {
            title: "Remove worktree".into(),
            lines: (0..40)
                .map(|i| format!("line {i} of a long list"))
                .collect(),
            yes: "delete them and remove".into(),
            action: Confirm::Trash("x".into()),
        });
        let mut terminal = Terminal::new(TestBackend::new(80, 16)).unwrap();
        terminal.draw(|f| ui::draw(f, &mut app)).unwrap();
        let screen: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(screen.contains("delete them and remove") && screen.contains("cancel"));
    }

    #[test]
    fn cleans_up_the_worktrees_nobody_uses_in_one_go() {
        let Some((root, run)) = crate::git::testing::scratch_repo("app-cleanup") else {
            return;
        };
        assert!(run(&["commit", "-q", "--allow-empty", "-m", "init"]));
        for name in ["one", "two", "busy"] {
            assert!(run(&["worktree", "add", "-q", name, "-b", name]));
        }
        std::fs::write(root.join("busy/notes.txt"), "work").unwrap();
        let mut app = app_on_worktree(&root, "one");

        press(&mut app, 'C');
        let Some(Popup::Confirm { lines, action, .. }) = &app.popup else {
            panic!("it asks first");
        };
        let text = lines.join("\n");
        assert!(text.contains("2 worktrees") && text.contains("one") && text.contains("two"));
        assert!(!text.contains("busy"));
        assert_eq!(action.key(), 'y');
        press(&mut app, 'y');
        finish_removals(&mut app);
        assert!(!root.join("one").exists() && !root.join("two").exists());
        assert!(root.join("busy/notes.txt").exists());
        let (message, is_error, _) = app.flash.as_ref().expect("says what happened");
        assert!(
            !is_error && message.contains("Removed 2 worktrees"),
            "{message}"
        );

        // Nothing left that is free: it says so instead of asking.
        let mut app = app_on_worktree(&root, "busy");
        press(&mut app, 'C');
        assert!(app.popup.is_none() && app.flash.is_some());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
