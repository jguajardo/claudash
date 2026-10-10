//! Projects (cards, then a page per project with a menu of sections) and
//! Insights (a menu of sections): their state and keys.

use std::path::{Path, PathBuf};

use ratatui::{crossterm::event::KeyCode, widgets::TableState};

use super::{
    ActivityFocus, Alert, App, Confirm, InsightsSection, Popup, Problem, ProjectPage, Section,
    Target, View, paths, specs, token_savers,
};
use crate::mcp::{McpServer, McpStatus};
use crate::{
    git,
    projects::{Checkout, Repo},
    sessions::Session,
    ui::plural,
    worktree::{self, Lock, Obstacles, OpenSession, Plan},
};

/// A project in the Projects grid: a repository or a folder outside git.
pub struct Card {
    /// The main checkout, or the folder.
    pub dir: PathBuf,
    pub name: String,
    /// Index into `projects.repos`, for repositories.
    pub repo: Option<usize>,
}

impl App {
    /// Everything Now flags, in order: problems across projects, then MCP
    /// servers checked so far that failed or need you to sign in.
    pub fn alerts(&self) -> Vec<Alert> {
        let mut alerts: Vec<Alert> = self.problems().into_iter().map(Alert::Problem).collect();
        let mut checked: Vec<(&PathBuf, &super::McpSnapshot)> = self.mcp_cache.iter().collect();
        checked.sort_by_key(|(dir, _)| dir.as_path());
        for (dir, snapshot) in checked {
            let Ok(servers) = &snapshot.result else {
                continue;
            };
            let mut sign_in: Vec<&McpServer> = Vec::new();
            for s in servers {
                match s.status {
                    McpStatus::Failed(_) => alerts.push(Alert::Mcp {
                        dir: dir.clone(),
                        name: s.name.clone(),
                        full_name: s.full_name.clone(),
                    }),
                    McpStatus::NeedsAuth => sign_in.push(s),
                    _ => {}
                }
            }
            if let Some(first) = sign_in.first() {
                let mut names: Vec<String> = Vec::new();
                for s in &sign_in {
                    if !names.contains(&s.name) {
                        names.push(s.name.clone());
                    }
                }
                alerts.push(Alert::McpSignIn {
                    dir: dir.clone(),
                    names,
                    first: first.full_name.clone(),
                });
            }
        }
        let stopped = self.stopped();
        if !stopped.is_empty() {
            let now = chrono::Utc::now().timestamp();
            let closed: Vec<_> = stopped.iter().filter(|s| !s.open).collect();
            alerts.insert(
                0,
                Alert::LimitStopped {
                    closed: closed.len(),
                    ready: closed.iter().filter(|s| s.reset(now)).count(),
                    open: stopped.len() - closed.len(),
                    next_reset: closed
                        .iter()
                        .filter(|s| !s.reset(now))
                        .map(|s| s.stop.resets_at)
                        .min(),
                    queued: self.continue_at_reset || self.auto_continue,
                },
            );
        }
        alerts
    }

    /// Where Enter on an alert goes.
    pub fn alert_target(&self, alert: &Alert) -> Option<Target> {
        let project = |dir: &Path, section, select: Option<String>| {
            Some(Target::Project {
                dir: dir.to_path_buf(),
                section,
                select,
            })
        };
        match alert {
            Alert::LimitStopped { closed, .. } if *closed > 0 => Some(Target::ContinueStopped),
            Alert::LimitStopped { .. } => None,
            Alert::Mcp { dir, full_name, .. }
            | Alert::McpSignIn {
                dir,
                first: full_name,
                ..
            } => project(dir, Section::Mcp, Some(full_name.clone())),
            Alert::Problem(p) => match p {
                Problem::SharedFolder { dir, .. } => project(dir, Section::Sessions, None),
                Problem::SameFile { .. } => None,
                Problem::IdleWorktree { path, .. } | Problem::MissingWorktree { path } => project(
                    path,
                    Section::Worktrees,
                    Some(path.to_string_lossy().into_owned()),
                ),
                Problem::Secrets { session_id, .. } if session_id.is_empty() => {
                    Some(Target::InsightsSecurity)
                }
                Problem::Secrets { session_id, .. } | Problem::Risky { session_id, .. } => {
                    Some(Target::Session(session_id.clone()))
                }
                Problem::StaleChange { dir, id, .. } | Problem::FinishedChange { dir, id, .. } => {
                    project(dir, Section::Specs, Some(id.clone()))
                }
            },
        }
    }

    /// Takes you where an alert points.
    pub fn go_to(&mut self, target: Target) {
        match target {
            Target::Session(id) => {
                self.select_session(&id);
                if self.selected_session().is_some_and(|s| s.id == id) {
                    self.inspect_scroll = 0;
                    self.inspect_sub = None;
                    self.enter_subview(View::Inspect);
                }
            }
            Target::ContinueStopped => {
                let now = chrono::Utc::now().timestamp();
                let stopped = self.stopped();
                let ready: Vec<String> = stopped
                    .iter()
                    .filter(|s| !s.open && s.reset(now))
                    .map(|s| format!("  • {} · {}", s.session.title, s.session.project_path))
                    .collect();
                if ready.is_empty() {
                    let next = stopped
                        .iter()
                        .filter(|s| !s.open)
                        .map(|s| s.stop.resets_at)
                        .min();
                    self.continue_at_reset = !self.continue_at_reset;
                    let msg = match (self.continue_at_reset, next) {
                        (true, Some(at)) => format!(
                            "They'll continue in the background at {} if claudash is still open",
                            crate::snapshots::when(at)
                        ),
                        _ => "They won't continue on their own; Enter again to change it".into(),
                    };
                    return self.show_flash(msg, false);
                }
                let mut lines = vec![
                    "Continue these sessions in the background? Each one runs".into(),
                    "`claude --bg --resume <id>` in its folder with the prompt:".into(),
                    format!("\"{}\"", crate::stopped::PROMPT),
                    String::new(),
                ];
                lines.extend(ready);
                lines.push(String::new());
                lines.push("Follow them under Background in Now (l: logs, Enter: attach).".into());
                self.popup = Some(Popup::Confirm {
                    title: "Continue after the limit".into(),
                    lines,
                    yes: "continue them".into(),
                    action: Confirm::ContinueStopped,
                });
            }
            Target::InsightsSecurity => {
                self.view = View::Insights;
                self.insights = InsightsSection::Security;
                self.insights_scroll = 0;
            }
            Target::Project {
                dir,
                section,
                select,
            } => {
                self.open_project_of(&dir);
                let Some(page) = &mut self.project_page else {
                    return;
                };
                page.section = section;
                let page_dir = page.dir.clone();
                self.enter_section();
                let Some(select) = select else {
                    return;
                };
                match section {
                    Section::Worktrees => {
                        let index = self.repo_at(&page_dir).and_then(|r| {
                            r.checkouts
                                .iter()
                                .position(|c| c.path.to_string_lossy() == select)
                        });
                        if let Some(page) = &mut self.project_page {
                            page.worktrees_state.select(Some(index.unwrap_or(0)));
                        }
                    }
                    Section::Specs => {
                        let index = self
                            .project_specs(&page_dir)
                            .and_then(|s| s.changes.iter().position(|c| c.id == select));
                        self.specs_state.select(Some(index.unwrap_or(0)));
                    }
                    Section::Mcp => {
                        let index = self
                            .project_servers()
                            .and_then(|s| s.iter().position(|m| m.full_name == select));
                        self.mcp_state.select(Some(index.unwrap_or(0)));
                    }
                    _ => {}
                }
            }
        }
    }

    pub(super) fn handle_alerts_key(&mut self, code: KeyCode) {
        let alerts = self.alerts();
        let n = alerts.len();
        match code {
            KeyCode::Esc => self.activity_focus = ActivityFocus::Open,
            KeyCode::Down | KeyCode::Char('j') if n > 0 => {
                self.alert_cursor = (self.alert_cursor + 1).min(n - 1)
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.alert_cursor = self.alert_cursor.saturating_sub(1)
            }
            KeyCode::Enter => {
                let Some(alert) = alerts.get(self.alert_cursor.min(n.saturating_sub(1))) else {
                    return;
                };
                match self.alert_target(alert) {
                    Some(target) => self.go_to(target),
                    None => self.show_flash("Look at the open sessions for this one", false),
                }
            }
            _ => {}
        }
    }

    /// Every project, repositories first, then folders outside git.
    pub fn cards(&self) -> Vec<Card> {
        let mut cards: Vec<Card> = self
            .projects
            .repos
            .iter()
            .enumerate()
            .filter_map(|(i, r)| {
                Some(Card {
                    dir: r.checkouts.first()?.path.clone(),
                    name: r.name.clone(),
                    repo: Some(i),
                })
            })
            .collect();
        cards.extend(self.projects.loose.iter().map(|dir| Card {
            dir: dir.clone(),
            name: paths::display(dir),
            repo: None,
        }));
        cards
    }

    /// The repository whose main checkout is `dir`.
    pub fn repo_at(&self, dir: &Path) -> Option<&Repo> {
        self.projects
            .repos
            .iter()
            .find(|r| r.checkouts.first().is_some_and(|c| c.path == dir))
    }

    /// Folders that belong to the project at `dir`: every checkout of a
    /// repository, or the folder itself.
    pub fn project_folders(&self, dir: &Path) -> Vec<PathBuf> {
        match self.repo_at(dir) {
            Some(repo) => repo.checkouts.iter().map(|c| c.path.clone()).collect(),
            None => vec![dir.to_path_buf()],
        }
    }

    /// Sessions of a project, newest first.
    pub fn project_sessions(&self, dir: &Path) -> Vec<&Session> {
        let folders = self.project_folders(dir);
        self.sessions
            .iter()
            .filter(|s| s.cwd.as_ref().is_some_and(|c| folders.contains(c)))
            .collect()
    }

    /// Spec changes of a project (from its main checkout, or the first
    /// checkout that has any).
    pub fn project_specs(&self, dir: &Path) -> Option<&specs::ProjectSpecs> {
        self.project_folders(dir)
            .iter()
            .find_map(|f| self.projects.specs.get(f))
    }

    /// Opens the page of the project `dir` belongs to.
    pub fn open_project_of(&mut self, dir: &Path) {
        let cards = self.cards();
        let index = cards
            .iter()
            .position(|c| c.dir == dir || self.project_folders(&c.dir).iter().any(|f| f == dir));
        let Some(index) = index else {
            return self.show_flash(
                "That folder isn't in Projects yet; try again in a moment",
                true,
            );
        };
        self.project_cursor = index;
        self.open_page(cards[index].dir.clone());
    }

    fn open_page(&mut self, dir: PathBuf) {
        self.view = View::Projects;
        self.project_page = Some(ProjectPage {
            dir,
            section: Section::Overview,
            in_content: false,
            sessions_state: TableState::default().with_selected(Some(0)),
            worktrees_state: TableState::default().with_selected(Some(0)),
            snapshots_state: TableState::default().with_selected(Some(0)),
            scroll: 0,
        });
        self.specs_state.select(Some(0));
        self.mcp_state.select(Some(0));
        self.update_project();
    }

    pub(super) fn handle_projects_key(&mut self, code: KeyCode) {
        if self.project_page.is_none() {
            return self.handle_grid_key(code);
        }
        let (section, in_content) = self
            .project_page
            .as_ref()
            .map(|p| (p.section, p.in_content))
            .unwrap_or((Section::Overview, false));
        if !in_content {
            return self.handle_menu_key(code);
        }
        match section {
            Section::Sessions => self.handle_project_sessions_key(code),
            Section::Specs => self.handle_specs_key(code),
            Section::Worktrees => self.handle_worktrees_key(code),
            Section::Snapshots => self.handle_snapshots_key(code),
            Section::Mcp => self.handle_mcp_key(code),
            Section::Setup => self.handle_eco_key(code),
            Section::Overview | Section::Security => self.leave_section(),
        }
    }

    fn handle_grid_key(&mut self, code: KeyCode) {
        let n = self.cards().len();
        if n == 0 {
            return;
        }
        let columns = self.project_columns.max(1);
        let i = self.project_cursor.min(n - 1);
        self.project_cursor = match code {
            KeyCode::Right => (i + 1).min(n - 1),
            KeyCode::Left => i.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => (i + columns).min(n - 1),
            KeyCode::Up | KeyCode::Char('k') => i.saturating_sub(columns),
            KeyCode::Home => 0,
            KeyCode::End => n - 1,
            KeyCode::Enter => {
                let dir = self.cards()[i].dir.clone();
                self.open_page(dir);
                i
            }
            KeyCode::Char('b') => {
                match self.cards()[i].repo {
                    Some(r) => self.open_branches(r, 0),
                    None => self.show_flash("This folder isn't a git repository", true),
                }
                i
            }
            _ => i,
        };
    }

    fn handle_menu_key(&mut self, code: KeyCode) {
        let Some(page) = &mut self.project_page else {
            return;
        };
        let i = Section::ALL
            .iter()
            .position(|s| *s == page.section)
            .unwrap_or(0);
        match code {
            KeyCode::Down | KeyCode::Char('j') => {
                page.section = Section::ALL[(i + 1).min(Section::ALL.len() - 1)];
                page.scroll = 0;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                page.section = Section::ALL[i.saturating_sub(1)];
                page.scroll = 0;
            }
            KeyCode::PageDown => page.scroll = page.scroll.saturating_add(10),
            KeyCode::PageUp => page.scroll = page.scroll.saturating_sub(10),
            KeyCode::Enter | KeyCode::Right | KeyCode::Tab => self.enter_section(),
            KeyCode::Esc | KeyCode::Left => {
                self.project_page = None;
                self.update_project();
            }
            KeyCode::Char('b') => {
                let dir = page.dir.clone();
                match self
                    .projects
                    .repos
                    .iter()
                    .position(|r| r.checkouts.first().is_some_and(|c| c.path == dir))
                {
                    Some(r) => self.open_branches(r, 0),
                    None => self.show_flash("This folder isn't a git repository", true),
                }
            }
            _ => {}
        }
        if matches!(
            self.project_page.as_ref().map(|p| p.section),
            Some(Section::Setup | Section::Security)
        ) {
            self.ensure_ecosystem();
        }
    }

    /// Moves the keys into the selected section, when it has anything to select.
    pub(super) fn enter_section(&mut self) {
        let Some(page) = &mut self.project_page else {
            return;
        };
        if matches!(page.section, Section::Overview | Section::Security) {
            return;
        }
        page.in_content = true;
        if page.section == Section::Mcp && self.mcp_state.selected().is_none() {
            self.mcp_state.select(Some(0));
        }
        if page.section == Section::Specs && self.specs_state.selected().is_none() {
            self.specs_state.select(Some(0));
        }
    }

    /// Back from a section's content to the menu.
    pub(super) fn leave_section(&mut self) {
        if let Some(page) = &mut self.project_page {
            page.in_content = false;
        }
    }

    /// Whether `co` is the worktree of a review of `repo` that is running.
    fn reviewing_in(&self, repo: &Repo, co: &Checkout) -> bool {
        co.review
            && self
                .reviews_in_progress()
                .filter(|task| task.repo == repo.checkouts[0].path)
                .any(|task| {
                    crate::review::worktree_for(&task.repo_name, &task.branch).as_ref()
                        == Some(&co.path)
                })
    }

    /// The sessions Claude Code has open in `dir` or a folder below it, by
    /// where Claude Code says they are or, failing that, where their
    /// transcript started.
    fn sessions_open_under(&self, dir: &Path) -> Vec<OpenSession> {
        self.live
            .iter()
            .filter_map(|(id, live)| {
                let session = self.sessions.iter().find(|s| &s.id == id);
                let folder = live
                    .cwd
                    .as_deref()
                    .map(Path::new)
                    .or_else(|| session.and_then(|s| s.cwd.as_deref()))?;
                folder.starts_with(dir).then(|| OpenSession {
                    title: session
                        .map(|s| s.title.clone())
                        .or_else(|| live.name.clone())
                        .unwrap_or_else(|| "A session".to_string()),
                    // Without its short ID there is nothing to pass to `claude stop`.
                    background: live.id.clone().filter(|_| live.is_background()),
                })
            })
            .collect()
    }

    /// What stands in the way of removing `co`, as it is right now.
    fn worktree_obstacles(&self, co: &Checkout) -> Obstacles {
        Obstacles {
            sessions: self.sessions_open_under(&co.path),
            // The process that held the lock may have ended since the last refresh.
            lock: co
                .lock
                .as_ref()
                .map(|lock| Lock::read(lock.reason.as_deref(), worktree::process_alive)),
            changes: git::worktree_changes(&co.path),
        }
    }

    fn page_dir(&self) -> Option<PathBuf> {
        self.project_page.as_ref().map(|p| p.dir.clone())
    }

    fn handle_project_sessions_key(&mut self, code: KeyCode) {
        let Some(dir) = self.page_dir() else {
            return;
        };
        let ids: Vec<String> = self
            .project_sessions(&dir)
            .iter()
            .map(|s| s.id.clone())
            .collect();
        let Some(page) = &mut self.project_page else {
            return;
        };
        let state = &mut page.sessions_state;
        let selected = state.selected().and_then(|i| ids.get(i)).cloned();
        match code {
            KeyCode::Esc | KeyCode::Left => page.in_content = false,
            KeyCode::Down | KeyCode::Char('j') if !ids.is_empty() => state.select(Some(
                state.selected().map_or(0, |i| (i + 1).min(ids.len() - 1)),
            )),
            KeyCode::Up | KeyCode::Char('k') => state.select_previous(),
            KeyCode::Enter => {
                if let Some(id) = selected {
                    self.select_session(&id);
                    self.request_resume();
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

    fn handle_specs_key(&mut self, code: KeyCode) {
        let Some(dir) = self.page_dir() else {
            return;
        };
        let Some(specs) = self.project_specs(&dir) else {
            return self.leave_section();
        };
        let n = specs.changes.len();
        let change = self
            .specs_state
            .selected()
            .and_then(|i| specs.changes.get(i))
            .cloned();
        // Commands run where the specs live.
        let folder = change
            .as_ref()
            .and_then(|c| {
                self.project_folders(&dir)
                    .into_iter()
                    .find(|f| c.path.starts_with(f))
            })
            .unwrap_or(dir);
        match code {
            KeyCode::Esc | KeyCode::Left => self.leave_section(),
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
                    let branch = crate::git::status(&folder).and_then(|s| s.branch);
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
                let title = next.clone();
                self.open_claude(vec![next], folder, &title);
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

    fn handle_snapshots_key(&mut self, code: KeyCode) {
        // The list on screen is this project's only once it has been read.
        let listed = self.page_dir().is_some_and(|dir| self.snapshots_for(&dir));
        let n = if listed { self.snapshots.len() } else { 0 };
        let Some(page) = &mut self.project_page else {
            return;
        };
        let state = &mut page.snapshots_state;
        let selected = self
            .snapshots
            .get(state.selected().unwrap_or(0).min(n.saturating_sub(1)))
            .filter(|_| listed)
            .cloned();
        match code {
            KeyCode::Esc | KeyCode::Left => page.in_content = false,
            KeyCode::Down | KeyCode::Char('j') if n > 0 => {
                state.select(Some(state.selected().map_or(0, |i| (i + 1).min(n - 1))))
            }
            KeyCode::Up | KeyCode::Char('k') => state.select_previous(),
            KeyCode::Char('T') => self.ask_snapshots(!self.snapshots_on),
            // Nothing to open while they're off: Enter turns them on.
            KeyCode::Enter if selected.is_none() && !self.snapshots_on => self.ask_snapshots(true),
            KeyCode::Enter => {
                let Some((root, snap)) = selected else {
                    return;
                };
                let lines = match crate::snapshots::diff(&root, &snap.sha) {
                    Ok(text) => text.lines().map(str::to_owned).collect(),
                    Err(e) => vec![format!("git: {e}")],
                };
                self.popup = Some(Popup::Text {
                    title: format!(" {} · {} ", snap.label, crate::snapshots::when(snap.at)),
                    lines,
                    scroll: 0,
                });
            }
            KeyCode::Char('U') => {
                let Some((root, snap)) = selected else {
                    return;
                };
                self.popup = Some(Popup::Confirm {
                    title: "Restore snapshot".into(),
                    lines: vec![
                        format!(
                            "Put the files of {} back as they were on {} ({})?",
                            paths::display(&root),
                            crate::snapshots::when(snap.at),
                            snap.label
                        ),
                        String::new(),
                        "The current state is snapshotted first, so this can be undone too.".into(),
                        "Files created after the snapshot are left where they are, and the".into(),
                        "project's own git repository is not touched.".into(),
                    ],
                    yes: "restore".into(),
                    action: Confirm::RestoreSnapshot {
                        root,
                        sha: snap.sha,
                    },
                });
            }
            _ => {}
        }
    }

    /// Asks before turning snapshots on or off, saying what that writes.
    fn ask_snapshots(&mut self, on: bool) {
        let file = crate::config::path()
            .map(|p| paths::display(&p))
            .unwrap_or_else(|| "claudash's config.toml".to_string());
        let (statusline, hooks) = crate::setup::installed();
        let setup_missing = !statusline || hooks < crate::hooks::EVENTS.len();
        self.popup = Some(Popup::Confirm {
            title: "Snapshots".into(),
            lines: snapshots_question(on, &file, setup_missing),
            yes: if on { "turn them on" } else { "turn them off" }.into(),
            action: Confirm::Snapshots { on },
        });
    }

    fn handle_worktrees_key(&mut self, code: KeyCode) {
        let Some(dir) = self.page_dir() else {
            return;
        };
        let Some(r) = self
            .projects
            .repos
            .iter()
            .position(|r| r.checkouts.first().is_some_and(|c| c.path == dir))
        else {
            if matches!(code, KeyCode::Esc | KeyCode::Left) {
                self.leave_section();
            }
            return;
        };
        let n = self.projects.repos[r].checkouts.len();
        let Some(page) = &mut self.project_page else {
            return;
        };
        let state = &mut page.worktrees_state;
        let c = state.selected().unwrap_or(0).min(n.saturating_sub(1));
        match code {
            KeyCode::Esc | KeyCode::Left => page.in_content = false,
            KeyCode::Down | KeyCode::Char('j') if n > 0 => state.select(Some((c + 1).min(n - 1))),
            KeyCode::Up | KeyCode::Char('k') => state.select_previous(),
            KeyCode::Enter => {
                let folder = self.projects.repos[r].checkouts[c].path.clone();
                self.folder_filter = Some(folder);
                self.filter.clear();
                self.apply_filter(None);
                self.view = View::Sessions;
            }
            KeyCode::Char('b') => self.open_branches(r, c),
            KeyCode::Char('D') => {
                let repo = &self.projects.repos[r];
                let co = &repo.checkouts[c];
                if co.main {
                    return self.show_flash("That's the main checkout, not a worktree", true);
                }
                if co.prunable {
                    return self.show_flash("Its directory is gone; P drops its record", false);
                }
                if self.reviewing_in(repo, co) {
                    return self.show_flash("A review is running in it; wait for it to end", true);
                }
                let obstacles = self.worktree_obstacles(co);
                let steps = match worktree::plan(&obstacles) {
                    Plan::Blocked(why) => return self.show_flash(why, true),
                    Plan::Remove(steps) => steps,
                };
                let unpushed = co.status.as_ref().map_or(0, |s| s.ahead);
                let lines = worktree::explain(
                    &paths::display(&co.path),
                    co.branch.as_deref(),
                    unpushed,
                    &obstacles,
                    &steps,
                );
                let yes = if steps.discard {
                    "delete them and remove"
                } else if !steps.stop.is_empty() {
                    "stop it and remove"
                } else {
                    "remove"
                };
                self.popup = Some(Popup::Confirm {
                    title: "Remove worktree".into(),
                    lines,
                    yes: yes.into(),
                    action: Confirm::RemoveWorktree {
                        main: repo.checkouts[0].path.clone(),
                        path: co.path.clone(),
                        steps,
                    },
                });
            }
            KeyCode::Char('C') => {
                let repo = &self.projects.repos[r];
                let busy = |path: &Path| {
                    !self.sessions_open_under(path).is_empty()
                        || repo
                            .checkouts
                            .iter()
                            .any(|co| co.path == path && self.reviewing_in(repo, co))
                };
                let items = worktree::idle(&repo.checkouts, busy);
                if items.is_empty() {
                    return self.show_flash(
                        "No worktree here is free of changes, sessions and locks; D on one says \
                         what's in the way",
                        false,
                    );
                }
                const LISTED: usize = 10;
                let mut lines = vec![
                    format!(
                        "Remove {} nobody is using?",
                        plural(items.len() as u64, "worktree")
                    ),
                    String::new(),
                ];
                lines.extend(
                    items
                        .iter()
                        .take(LISTED)
                        .map(|(path, _)| format!("  {}", paths::display(path))),
                );
                if items.len() > LISTED {
                    lines.push(format!("  … and {} more", items.len() - LISTED));
                }
                lines.push(String::new());
                lines.push(
                    "None has changes or an open session. Their branches are kept.".to_string(),
                );
                let locked = items.iter().filter(|(_, steps)| steps.unlock).count();
                if locked > 0 {
                    lines.push(format!(
                        "{} locked by a process that ended; claudash unlocks {}.",
                        if locked == 1 {
                            "One is".to_string()
                        } else {
                            format!("{locked} are")
                        },
                        if locked == 1 { "it" } else { "them" }
                    ));
                }
                self.popup = Some(Popup::Confirm {
                    title: "Clean up worktrees".into(),
                    lines,
                    yes: "remove them".into(),
                    action: Confirm::RemoveWorktrees {
                        main: repo.checkouts[0].path.clone(),
                        items,
                    },
                });
            }
            KeyCode::Char('P') => {
                let repo = &self.projects.repos[r];
                let missing = repo.checkouts.iter().filter(|c| c.prunable).count();
                if missing == 0 {
                    return self.show_flash("No missing worktrees in this repository", false);
                }
                self.popup = Some(Popup::Confirm {
                    title: "Prune worktrees".into(),
                    lines: vec![
                        format!(
                            "Drop {missing} worktree {} whose directory is gone?",
                            if missing == 1 { "record" } else { "records" }
                        ),
                        String::new(),
                        "This runs `git worktree prune`; no files are touched.".into(),
                    ],
                    yes: "prune".into(),
                    action: Confirm::PruneWorktrees(repo.checkouts[0].path.clone()),
                });
            }
            _ => {}
        }
    }

    pub(super) fn handle_insights_key(&mut self, code: KeyCode) {
        let i = InsightsSection::ALL
            .iter()
            .position(|s| *s == self.insights)
            .unwrap_or(0);
        match code {
            KeyCode::Down | KeyCode::Char('j') => {
                self.insights = InsightsSection::ALL[(i + 1).min(InsightsSection::ALL.len() - 1)];
                self.insights_scroll = 0;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.insights = InsightsSection::ALL[i.saturating_sub(1)];
                self.insights_scroll = 0;
            }
            KeyCode::PageDown => self.insights_scroll = self.insights_scroll.saturating_add(10),
            KeyCode::PageUp => self.insights_scroll = self.insights_scroll.saturating_sub(10),
            KeyCode::Char('m') => self.monthly = !self.monthly,
            KeyCode::Char('$') => self.dollars = !self.dollars,
            KeyCode::Esc => self.view = View::Now,
            _ => {}
        }
        if self.insights == InsightsSection::Tokens && self.token_savers.is_none() {
            self.token_savers = Some(token_savers());
        }
    }
}

/// What turning snapshots on or off does and writes, to ask about it first.
/// `setup_missing`: `claudash setup --apply` hasn't registered everything.
fn snapshots_question(on: bool, file: &str, setup_missing: bool) -> Vec<String> {
    if !on {
        return vec![
            "Turn snapshots off?".to_string(),
            String::new(),
            "No more copies are made. The snapshots already taken are kept: you can still look \
             at them and put files back from them."
                .to_string(),
            String::new(),
            format!("This writes `snapshots = false` to {file}."),
        ];
    }
    let mut lines = vec![
        "Turn snapshots on?".to_string(),
        String::new(),
        "From then on, claudash's hook saves a copy of a project's files before each prompt \
         you send and after each reply, when something changed. Only git projects."
            .to_string(),
        String::new(),
        "The copies go to a separate git repository in claudash's data folder. A project's \
         own .git is never touched, and files its .gitignore ignores aren't copied."
            .to_string(),
        String::new(),
        format!("This writes `snapshots = true` to {file}."),
    ];
    if setup_missing {
        lines.push(
            "It also does what `claudash setup --apply` does: registers claudash's status line \
             and hook in Claude Code's settings.json, after backing that file up."
                .to_string(),
        );
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_everything_turning_snapshots_on_writes() {
        let on = snapshots_question(true, "~/.config/claudash/config.toml", false).join("\n");
        assert!(on.contains("before each prompt") && on.contains("snapshots = true"));
        assert!(!on.contains("status line"));
        // Without claudash's setup it applies all of it, not only the hook.
        let with_setup = snapshots_question(true, "config.toml", true).join("\n");
        assert!(with_setup.contains("status line") && with_setup.contains("hook"));
        assert!(with_setup.contains("backing"));
        let off = snapshots_question(false, "config.toml", true).join("\n");
        assert!(off.contains("are kept") && !off.contains("status line"));
    }
}
