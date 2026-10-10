//! Removing worktrees: what stands in the way of each one, and the steps that
//! clear it.

use std::{
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use crate::{
    git::{self, Changes, Removal},
    projects::Checkout,
    ui::plural,
};

/// Paths listed in the question before the rest are only counted.
const LISTED: usize = 8;
/// How long a stopped session's process gets to end before giving up.
const STOP_WAIT: Duration = Duration::from_secs(8);

/// Who holds a worktree's lock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Holder {
    /// The process the lock names is running.
    Running(u32),
    /// The process the lock names has ended: the lock was left behind.
    Ended(u32),
    /// The lock names no process: set by hand, or by another tool.
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lock {
    /// As given to `git worktree lock --reason`.
    pub reason: Option<String>,
    pub holder: Holder,
}

impl Lock {
    /// Claude Code locks the worktrees of its agents with a reason that ends
    /// in "(pid N)"; `alive` says whether that process still runs.
    pub fn read(reason: Option<&str>, alive: impl Fn(u32) -> bool) -> Lock {
        let holder = match reason.and_then(lock_pid) {
            Some(pid) if alive(pid) => Holder::Running(pid),
            Some(pid) => Holder::Ended(pid),
            None => Holder::Unknown,
        };
        Lock {
            reason: reason.map(str::to_owned),
            holder,
        }
    }
}

/// The process ID in a lock reason such as "claude agent agent-a1 (pid 4242)".
pub fn lock_pid(reason: &str) -> Option<u32> {
    let (_, rest) = reason.rsplit_once("(pid ")?;
    rest.split_once(')')?.0.trim().parse().ok()
}

/// Whether a process with this ID exists. When that can't be found out, it
/// counts as running, so nothing is unlocked by mistake.
pub fn process_alive(pid: u32) -> bool {
    if cfg!(windows) {
        return match Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .output()
        {
            Ok(o) if o.status.success() => {
                String::from_utf8_lossy(&o.stdout).contains(&format!("\"{pid}\""))
            }
            _ => true,
        };
    }
    // Linux: the process table is a directory.
    let proc_dir = Path::new("/proc");
    if proc_dir.join("self").exists() {
        return proc_dir.join(pid.to_string()).exists();
    }
    // macOS and the BSDs: `ps -p` prints nothing and fails when there's no
    // such process. Any other failure (a `ps` without `-p`) settles nothing.
    match Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "pid="])
        .output()
    {
        Ok(o) if o.status.success() => true,
        Ok(o) => !(o.stdout.is_empty() && o.stderr.is_empty()),
        Err(_) => true,
    }
}

/// A session Claude Code has open in a worktree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenSession {
    pub title: String,
    /// The ID `claude stop` takes, when it's a background session.
    pub background: Option<String>,
}

/// What stands in the way of removing a worktree.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Obstacles {
    pub sessions: Vec<OpenSession>,
    pub lock: Option<Lock>,
    pub changes: Changes,
}

/// What clears the way, in the order it's done.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Steps {
    /// Background sessions to stop first (`claude stop <id>`).
    pub stop: Vec<String>,
    pub unlock: bool,
    /// Delete its modified and untracked files.
    pub discard: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    /// claudash can't clear the way; why, in one line.
    Blocked(String),
    Remove(Steps),
}

/// What it takes to remove a worktree with these obstacles.
pub fn plan(obstacles: &Obstacles) -> Plan {
    let mut interactive = obstacles.sessions.iter().filter(|s| s.background.is_none());
    if let Some(first) = interactive.next() {
        let others = match interactive.count() {
            0 => String::new(),
            n => format!(" and {n} more"),
        };
        return Plan::Blocked(format!(
            "\"{}\"{others} is open there in another terminal; close it, then remove the worktree",
            first.title
        ));
    }
    let stop: Vec<String> = obstacles
        .sessions
        .iter()
        .filter_map(|s| s.background.clone())
        .collect();
    if let Some(Lock {
        holder: Holder::Running(pid),
        ..
    }) = &obstacles.lock
        && stop.is_empty()
    {
        return Plan::Blocked(format!(
            "It's locked by a process that is still running (pid {pid}); stop that first"
        ));
    }
    Plan::Remove(Steps {
        stop,
        unlock: obstacles.lock.is_some(),
        discard: !obstacles.changes.is_empty(),
    })
}

/// The question to ask before carrying `steps` out: what will be stopped,
/// unlocked and deleted, and what stays.
pub fn explain(
    place: &str,
    branch: Option<&str>,
    unpushed: u32,
    obstacles: &Obstacles,
    steps: &Steps,
) -> Vec<String> {
    let mut lines = vec![format!("Remove the worktree {place}?")];
    for session in obstacles.sessions.iter().filter(|s| s.background.is_some()) {
        let id = session.background.as_deref().unwrap_or_default();
        lines.push(String::new());
        lines.push(format!(
            "A background session runs there: \"{}\". claudash stops it first (claude stop {id}); \
             its conversation is kept.",
            session.title
        ));
    }
    if let Some(lock) = obstacles.lock.as_ref().filter(|_| steps.unlock) {
        lines.push(String::new());
        lines.push(match (&lock.holder, &lock.reason) {
            (Holder::Running(pid), _) => format!(
                "It's locked by that session's process (pid {pid}); claudash will unlock it once \
                 the session has stopped."
            ),
            (Holder::Ended(pid), _) => format!(
                "It's locked by a process that is no longer running (pid {pid}); claudash will \
                 unlock it."
            ),
            (Holder::Unknown, Some(reason)) => {
                format!("It's locked (\"{reason}\"); claudash will unlock it.")
            }
            (Holder::Unknown, None) => {
                "It's locked, with no reason given; claudash will unlock it.".to_string()
            }
        });
    }
    let changes = &obstacles.changes;
    if steps.discard {
        lines.push(String::new());
        lines.push("These exist only there and will be deleted:".into());
        let paths = changes
            .modified
            .iter()
            .map(|p| format!("  modified   {p}"))
            .chain(
                changes
                    .untracked
                    .iter()
                    .map(|p| format!("  untracked  {p}")),
            );
        lines.extend(paths.take(LISTED));
        let total = changes.modified.len() + changes.untracked.len();
        if total > LISTED {
            lines.push(format!("  … and {} more", total - LISTED));
        }
    } else {
        lines.push(String::new());
        lines.push("This runs `git worktree remove`.".into());
    }
    if !changes.ignored.is_empty() {
        const NAMED: usize = 4;
        let mut names = changes.ignored[..changes.ignored.len().min(NAMED)].join(", ");
        if changes.ignored.len() > NAMED {
            names.push_str(&format!(" and {} more", changes.ignored.len() - NAMED));
        }
        lines.push(String::new());
        lines.push(format!("What git ignores there goes with it: {names}."));
    }
    if let Some(branch) = branch {
        lines.push(String::new());
        lines.push(if unpushed > 0 {
            format!(
                "Its branch {branch} is kept, with its {}.",
                plural(u64::from(unpushed), "unpushed commit")
            )
        } else {
            format!("Its branch {branch} is kept.")
        });
    }
    lines
}

/// The worktrees among `checkouts` that can go without losing anything or
/// interrupting anyone: no changes, no session open (`open`), and no lock
/// other than one left behind by a process that ended.
pub fn idle(checkouts: &[Checkout], open: impl Fn(&Path) -> bool) -> Vec<(PathBuf, Steps)> {
    checkouts
        .iter()
        .filter(|c| !c.main && !c.prunable)
        .filter(|c| c.status.as_ref().is_some_and(|s| s.changed == 0))
        .filter(|c| !open(&c.path))
        .filter_map(|c| {
            let unlock = match c.lock.as_ref().map(|l| &l.holder) {
                None => false,
                Some(Holder::Ended(_)) => true,
                Some(Holder::Running(_) | Holder::Unknown) => return None,
            };
            let steps = Steps {
                unlock,
                ..Default::default()
            };
            Some((c.path.clone(), steps))
        })
        .collect()
}

/// Carries a plan out: stops the background sessions, waits for the process
/// that holds the lock to end, then removes the worktree. Its branch stays.
pub fn remove(main: &Path, path: &Path, steps: &Steps) -> Result<(), String> {
    for id in &steps.stop {
        crate::claude_cli::background("stop", id)?;
    }
    // The lock as it is now: stopping a session may have released it.
    let lock = || {
        git::worktrees(main)
            .into_iter()
            .find(|w| w.path == path)
            .filter(|w| w.locked)
            .map(|w| w.lock_reason.as_deref().and_then(lock_pid))
    };
    let mut held = lock();
    if steps.unlock
        && let Some(Some(pid)) = held
    {
        let asked = Instant::now();
        while !steps.stop.is_empty() && process_alive(pid) && asked.elapsed() < STOP_WAIT {
            std::thread::sleep(Duration::from_millis(250));
        }
        if process_alive(pid) {
            return Err(format!(
                "It's locked by a process that is still running (pid {pid}); nothing was removed"
            ));
        }
        held = lock();
    }
    let how = Removal {
        unlock: steps.unlock && held.is_some(),
        discard: steps.discard,
    };
    git::remove_worktree(main, path, how)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock(holder: Holder) -> Option<Lock> {
        Some(Lock {
            reason: Some("claude agent agent-a1 (pid 4242)".into()),
            holder,
        })
    }

    fn background(id: &str) -> OpenSession {
        OpenSession {
            title: format!("session {id}"),
            background: Some(id.into()),
        }
    }

    #[test]
    fn reads_the_process_a_lock_names() {
        assert_eq!(lock_pid("claude agent agent-a1 (pid 4242)"), Some(4242));
        assert_eq!(lock_pid("keep this one"), None);
        assert_eq!(lock_pid("(pid abc)"), None);

        let held = Lock::read(Some("claude agent agent-a1 (pid 4242)"), |_| true);
        assert_eq!(held.holder, Holder::Running(4242));
        let left_over = Lock::read(Some("claude agent agent-a1 (pid 4242)"), |_| false);
        assert_eq!(left_over.holder, Holder::Ended(4242));
        assert_eq!(Lock::read(None, |_| true).holder, Holder::Unknown);
    }

    #[test]
    fn tells_a_running_process_from_one_that_ended() {
        assert!(process_alive(std::process::id()));
        let mut child = if cfg!(windows) {
            let mut c = std::process::Command::new("cmd");
            c.args(["/C", "exit"]);
            c
        } else {
            std::process::Command::new("true")
        }
        .spawn()
        .unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert!(!process_alive(pid));
    }

    #[test]
    fn removes_a_free_worktree_as_git_does() {
        assert_eq!(plan(&Obstacles::default()), Plan::Remove(Steps::default()));
    }

    #[test]
    fn discards_files_and_clears_left_over_locks() {
        let dirty = Obstacles {
            changes: Changes {
                modified: vec!["a.rs".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        let discard = Steps {
            discard: true,
            ..Default::default()
        };
        assert_eq!(plan(&dirty), Plan::Remove(discard));

        let unlock = Steps {
            unlock: true,
            ..Default::default()
        };
        for holder in [Holder::Ended(4242), Holder::Unknown] {
            let locked = Obstacles {
                lock: lock(holder),
                ..Default::default()
            };
            assert_eq!(plan(&locked), Plan::Remove(unlock.clone()));
        }
    }

    #[test]
    fn stops_background_sessions_but_not_ones_in_another_terminal() {
        let running = Obstacles {
            sessions: vec![background("bg7k2")],
            lock: lock(Holder::Running(4242)),
            ..Default::default()
        };
        let stop = Steps {
            stop: vec!["bg7k2".into()],
            unlock: true,
            discard: false,
        };
        assert_eq!(plan(&running), Plan::Remove(stop));

        let interactive = Obstacles {
            sessions: vec![
                background("bg7k2"),
                OpenSession {
                    title: "Add OAuth login".into(),
                    background: None,
                },
            ],
            ..Default::default()
        };
        let Plan::Blocked(why) = plan(&interactive) else {
            panic!("an interactive session can't be closed from here");
        };
        assert!(why.contains("Add OAuth login") && why.contains("another terminal"));
    }

    #[test]
    fn leaves_a_lock_alone_while_its_process_runs() {
        let held = Obstacles {
            lock: lock(Holder::Running(4242)),
            ..Default::default()
        };
        let Plan::Blocked(why) = plan(&held) else {
            panic!("nothing here can stop that process");
        };
        assert!(why.contains("pid 4242"));
    }

    #[test]
    fn unlocks_only_when_the_locks_process_has_ended() {
        let Some((root, run)) = crate::git::testing::scratch_repo("worktree-remove") else {
            return;
        };
        assert!(run(&["commit", "-q", "--allow-empty", "-m", "init"]));
        let unlock = Steps {
            unlock: true,
            ..Default::default()
        };
        // The path as git reports it, which is how the app knows worktrees.
        let path_of = |name: &str| {
            git::worktrees(&root)
                .into_iter()
                .find(|w| w.path.ends_with(name))
                .map(|w| w.path)
                .expect("a worktree with that name")
        };

        // Held by a process that is running: this one.
        assert!(run(&["worktree", "add", "-q", "held", "-b", "a"]));
        let reason = format!("claude agent x (pid {})", std::process::id());
        assert!(run(&["worktree", "lock", "--reason", &reason, "held"]));
        let error = remove(&root, &path_of("held"), &unlock).unwrap_err();
        assert!(error.contains("still running"), "{error}");
        assert!(root.join("held").exists());
        assert!(git::worktrees(&root).iter().any(|w| w.locked));

        // A lock left by a process that ended.
        let mut child = std::process::Command::new(if cfg!(windows) { "cmd" } else { "true" })
            .args(if cfg!(windows) {
                &["/C", "exit"][..]
            } else {
                &[]
            })
            .spawn()
            .unwrap();
        let ended = child.id();
        child.wait().unwrap();
        assert!(run(&["worktree", "add", "-q", "left", "-b", "b"]));
        let reason = format!("claude agent y (pid {ended})");
        assert!(run(&["worktree", "lock", "--reason", &reason, "left"]));
        assert_eq!(remove(&root, &path_of("left"), &unlock), Ok(()));
        assert!(!root.join("left").exists());

        // Asked to unlock one that isn't locked any more: just removes it.
        assert!(run(&["worktree", "add", "-q", "free", "-b", "c"]));
        assert_eq!(remove(&root, &path_of("free"), &unlock), Ok(()));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn picks_the_worktrees_nobody_uses_for_a_cleanup() {
        let checkout = |path: &str, changed: usize, lock: Option<Lock>| Checkout {
            path: PathBuf::from(path),
            branch: None,
            status: Some(git::Status {
                changed,
                ..Default::default()
            }),
            main: false,
            lock,
            prunable: false,
            claude_created: true,
            review: false,
        };
        let list = vec![
            Checkout {
                main: true,
                ..checkout("/r", 0, None)
            },
            checkout("/r/free", 0, None),
            checkout("/r/dirty", 2, None),
            checkout("/r/open", 0, None),
            checkout("/r/left", 0, lock(Holder::Ended(1))),
            checkout("/r/held", 0, lock(Holder::Running(1))),
            // Locked by hand: someone wants it kept.
            checkout("/r/kept", 0, lock(Holder::Unknown)),
            Checkout {
                prunable: true,
                status: None,
                ..checkout("/r/gone", 0, None)
            },
        ];
        let unlock = Steps {
            unlock: true,
            ..Default::default()
        };
        assert_eq!(
            idle(&list, |path| path == Path::new("/r/open")),
            [
                (PathBuf::from("/r/free"), Steps::default()),
                (PathBuf::from("/r/left"), unlock),
            ]
        );
    }

    #[test]
    fn says_what_removing_will_do() {
        let obstacles = Obstacles {
            sessions: vec![background("bg7k2")],
            lock: lock(Holder::Ended(4242)),
            changes: Changes {
                modified: vec!["src/api.rs".into()],
                untracked: (1..=9).map(|i| format!("note{i}.txt")).collect(),
                ignored: vec![".env".into(), "target/".into()],
            },
        };
        let Plan::Remove(steps) = plan(&obstacles) else {
            panic!("removable");
        };
        let text = explain(
            "wt/pagination",
            Some("spike/pagination"),
            2,
            &obstacles,
            &steps,
        )
        .join("\n");
        assert!(text.contains("Remove the worktree wt/pagination?"));
        assert!(text.contains("session bg7k2") && text.contains("claude stop bg7k2"));
        assert!(text.contains("no longer running") && text.contains("unlock"));
        assert!(text.contains("modified   src/api.rs") && text.contains("untracked  note1.txt"));
        // Ten paths: eight shown, the rest counted.
        assert!(text.contains("and 2 more") && !text.contains("note9.txt"));
        assert!(text.contains("spike/pagination") && text.contains("2 unpushed commits"));

        // What git ignores isn't in the way, but it's said: it goes too.
        assert!(text.contains("git ignores") && text.contains(".env, target/"));

        let free = explain("wt/x", None, 0, &Obstacles::default(), &Steps::default()).join("\n");
        assert!(free.contains("git worktree remove") && !free.contains("deleted"));
        assert!(!free.contains("ignores") && !free.contains("nothing else"));
    }
}
