//! Git state of the folders sessions run in, from git's machine-readable
//! (`--porcelain`) output.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    /// Current branch; `None` when HEAD is detached.
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    /// Changed, staged, untracked or conflicted paths.
    pub changed: usize,
    /// How many of those git doesn't track.
    pub untracked: usize,
    /// Top of this checkout (a worktree's own root for linked worktrees).
    pub root: PathBuf,
    /// The repository's shared `.git` directory: equal for all its worktrees.
    pub common_dir: PathBuf,
    /// A linked worktree rather than the main checkout.
    pub linked_worktree: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub locked: bool,
    /// Why it's locked, as given to `git worktree lock --reason`.
    pub lock_reason: Option<String>,
    /// Git considers it removable: its directory is gone.
    pub prunable: bool,
}

/// What removing a worktree would lose: the paths `git status` lists there.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Changes {
    /// Tracked files that are modified, staged or conflicted.
    pub modified: Vec<String>,
    /// Files and folders git doesn't track.
    pub untracked: Vec<String>,
    /// Files and folders its `.gitignore` rules ignore (build output,
    /// `.env`): git removes them with the worktree without asking.
    pub ignored: Vec<String>,
}

impl Changes {
    /// Nothing git would refuse to remove the worktree over.
    pub fn is_empty(&self) -> bool {
        self.modified.is_empty() && self.untracked.is_empty()
    }
}

/// How far removing a worktree may go beyond what git does by default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Removal {
    /// Unlock it first.
    pub unlock: bool,
    /// Delete its modified and untracked files (`--force`).
    pub discard: bool,
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Status of the checkout containing `dir`, or `None` outside a repository.
pub fn status(dir: &Path) -> Option<Status> {
    let paths = git(
        dir,
        &[
            "rev-parse",
            "--show-toplevel",
            "--git-common-dir",
            "--git-dir",
        ],
    )?;
    let mut lines = paths.lines();
    let root = PathBuf::from(lines.next()?);
    let absolute = |p: &str| {
        let p = PathBuf::from(p);
        if p.is_absolute() { p } else { dir.join(p) }
    };
    let common_dir = absolute(lines.next()?);
    let git_dir = absolute(lines.next()?);
    let porcelain = git(dir, &["status", "--porcelain=v2", "--branch"])?;
    let mut status = parse_status(&porcelain);
    status.linked_worktree = common_dir.canonicalize().ok() != git_dir.canonicalize().ok();
    status.common_dir = common_dir.canonicalize().unwrap_or(common_dir);
    status.root = root;
    Some(status)
}

fn parse_status(porcelain: &str) -> Status {
    let mut s = Status::default();
    for line in porcelain.lines() {
        if let Some(head) = line.strip_prefix("# branch.head ") {
            s.branch = (head != "(detached)").then(|| head.to_string());
        } else if let Some(up) = line.strip_prefix("# branch.upstream ") {
            s.upstream = Some(up.to_string());
        } else if let Some(ab) = line.strip_prefix("# branch.ab ") {
            let mut parts = ab.split_whitespace();
            s.ahead = parts
                .next()
                .and_then(|a| a.trim_start_matches('+').parse().ok())
                .unwrap_or(0);
            s.behind = parts
                .next()
                .and_then(|b| b.trim_start_matches('-').parse().ok())
                .unwrap_or(0);
        } else if !line.starts_with('#') && !line.is_empty() {
            s.changed += 1;
            if line.starts_with("? ") {
                s.untracked += 1;
            }
        }
    }
    s
}

/// All worktrees of the repository containing `dir`, main checkout first.
pub fn worktrees(dir: &Path) -> Vec<Worktree> {
    git(dir, &["worktree", "list", "--porcelain"])
        .map(|out| parse_worktrees(&out))
        .unwrap_or_default()
}

fn parse_worktrees(porcelain: &str) -> Vec<Worktree> {
    let mut list = Vec::new();
    let mut current: Option<Worktree> = None;
    for line in porcelain.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            list.extend(current.take());
            current = Some(Worktree {
                path: PathBuf::from(path),
                ..Default::default()
            });
        } else if let Some(w) = current.as_mut() {
            if let Some(branch) = line.strip_prefix("branch ") {
                w.branch = Some(branch.trim_start_matches("refs/heads/").to_string());
            } else if let Some(reason) = line.strip_prefix("locked") {
                w.locked = true;
                w.lock_reason = Some(reason.trim().to_string()).filter(|r| !r.is_empty());
            } else if line.starts_with("prunable") {
                w.prunable = true;
            }
        }
    }
    list.extend(current);
    list
}

/// The modified, untracked and ignored paths of the checkout at `worktree`.
pub fn worktree_changes(worktree: &Path) -> Changes {
    let mut changes = Changes::default();
    let Some(out) = git(worktree, &["status", "--porcelain=v1", "-z", "--ignored"]) else {
        return changes;
    };
    let mut entries = out.split('\0').filter(|e| !e.is_empty());
    while let Some(entry) = entries.next() {
        let Some((state, path)) = entry.split_at_checked(3) else {
            continue;
        };
        if state.starts_with("??") {
            changes.untracked.push(path.to_string());
        } else if state.starts_with("!!") {
            changes.ignored.push(path.to_string());
        } else {
            changes.modified.push(path.to_string());
            // A rename or copy is followed by the path it came from.
            if state.contains(['R', 'C']) {
                entries.next();
            }
        }
    }
    changes
}

/// Removes a worktree; its branch stays. Without `how.discard` git refuses
/// when it has changes or untracked files, and without `how.unlock` when it's
/// locked. The error is git's own message.
pub fn remove_worktree(main_checkout: &Path, worktree: &Path, how: Removal) -> Result<(), String> {
    let run = |args: &[&str]| {
        let output = Command::new("git")
            .arg("-C")
            .arg(main_checkout)
            .args(args)
            .arg(worktree)
            .output()
            .map_err(|e| format!("Could not run git: {e}"))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
        }
    };
    if how.unlock {
        run(&["worktree", "unlock"])?;
    }
    if how.discard {
        run(&["worktree", "remove", "--force"])
    } else {
        run(&["worktree", "remove"])
    }
}

/// Drops the records of worktrees whose directory is gone (`git worktree prune`).
pub fn prune_worktrees(main_checkout: &Path) -> Result<(), String> {
    git(main_checkout, &["worktree", "prune"])
        .map(|_| ())
        .ok_or_else(|| "git worktree prune failed".to_string())
}

/// Commits made today in the repository at `dir`, one line each.
pub fn commits_today(dir: &Path) -> Vec<String> {
    git(
        dir,
        &["log", "--since=midnight", "--format=%h %s", "--no-merges"],
    )
    .map(|out| out.lines().map(str::to_owned).collect())
    .unwrap_or_default()
}

/// Repositories for tests that need a real one.
#[cfg(test)]
pub(crate) mod testing {
    use std::{path::PathBuf, process::Command};

    /// A repository in a fresh temporary directory, and a way to run git in it.
    /// `None` when this machine has no git.
    pub fn scratch_repo(name: &str) -> Option<(PathBuf, impl Fn(&[&str]) -> bool)> {
        let root = std::env::temp_dir().join(format!("claudash-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // Git prints worktree paths with symbolic links resolved (macOS keeps
        // its temporary directory behind one).
        #[cfg(unix)]
        let root = root.canonicalize().unwrap();
        let dir = root.clone();
        let run = move |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(&dir)
                // Whatever the machine's own git settings say about signing.
                .args(["-c", "user.email=t@t", "-c", "user.name=t"])
                .args(["-c", "commit.gpgsign=false"])
                .args(args)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        };
        run(&["init", "-q"]).then_some((root, run))
    }
}

#[cfg(test)]
mod tests {
    use super::{testing::scratch_repo, *};

    #[test]
    fn parses_branch_counts_and_changes() {
        let s = parse_status(
            "# branch.oid abc\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +2 -1\n\
             1 .M N... 100644 100644 100644 a b src/x.rs\n? new.txt\n",
        );
        assert_eq!(s.branch.as_deref(), Some("main"));
        assert_eq!((s.ahead, s.behind, s.changed), (2, 1, 2));
        assert_eq!(parse_status("# branch.head (detached)\n").branch, None);
    }

    #[test]
    fn parses_worktree_list() {
        let list = parse_worktrees(
            "worktree /r\nHEAD a\nbranch refs/heads/main\n\n\
             worktree /r/.claude/worktrees/x\nHEAD b\nbranch refs/heads/worktree-x\nlocked claude\n\n\
             worktree /gone\nHEAD c\ndetached\nprunable gitdir file points to non-existent location\n",
        );
        assert_eq!(list.len(), 3);
        assert_eq!(list[1].branch.as_deref(), Some("worktree-x"));
        assert!(list[1].locked && list[2].prunable);
    }

    #[test]
    fn reads_this_repository() {
        // The crate is developed in git; CI checks it out with git too.
        let here = Path::new(env!("CARGO_MANIFEST_DIR"));
        if let Some(s) = status(here) {
            assert!(!s.linked_worktree || s.root != s.common_dir);
            assert!(!worktrees(here).is_empty());
        }
    }

    #[test]
    fn counts_untracked_files_apart() {
        let s = parse_status(
            "# branch.head main\n1 .M N... 100644 100644 100644 a b src/x.rs\n? new.txt\n? other.txt\n",
        );
        assert_eq!((s.changed, s.untracked), (3, 2));
    }

    #[test]
    fn keeps_why_a_worktree_is_locked() {
        let list = parse_worktrees(
            "worktree /r\nHEAD a\nbranch refs/heads/main\n\n\
             worktree /r/x\nHEAD b\nbranch refs/heads/x\nlocked claude agent agent-a1 (pid 4242)\n\n\
             worktree /r/y\nHEAD c\nbranch refs/heads/y\nlocked\n",
        );
        assert!(!list[0].locked && list[0].lock_reason.is_none());
        assert_eq!(
            list[1].lock_reason.as_deref(),
            Some("claude agent agent-a1 (pid 4242)")
        );
        // Locked by hand, with no reason given.
        assert!(list[2].locked && list[2].lock_reason.is_none());
    }

    #[test]
    fn lists_the_files_a_worktree_would_lose() {
        let Some((root, run)) = scratch_repo("changes") else {
            return;
        };
        std::fs::write(root.join("tracked.txt"), "one").unwrap();
        std::fs::write(root.join(".gitignore"), ".env\nbuild/\n").unwrap();
        assert!(run(&["add", "tracked.txt", ".gitignore"]));
        assert!(run(&["commit", "-q", "-m", "init"]));
        assert!(run(&["worktree", "add", "-q", "wt", "-b", "a"]));
        let wt = root.join("wt");
        assert_eq!(worktree_changes(&wt), Changes::default());

        std::fs::write(wt.join("tracked.txt"), "two").unwrap();
        std::fs::write(wt.join("new file.txt"), "x").unwrap();
        std::fs::create_dir_all(wt.join("notes")).unwrap();
        std::fs::write(wt.join("notes/a.md"), "x").unwrap();
        // Ignored files don't stop git from removing a worktree, but go with it.
        std::fs::write(wt.join(".env"), "KEY=1").unwrap();
        std::fs::create_dir_all(wt.join("build")).unwrap();
        std::fs::write(wt.join("build/out"), "x").unwrap();
        let changes = worktree_changes(&wt);
        assert_eq!(changes.modified, ["tracked.txt"]);
        assert_eq!(changes.untracked, ["new file.txt", "notes/"]);
        assert_eq!(changes.ignored, [".env", "build/"]);
        let only_ignored = Changes {
            ignored: changes.ignored.clone(),
            ..Default::default()
        };
        assert!(
            only_ignored.is_empty(),
            "ignored files don't need discarding"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn deletes_a_worktrees_files_only_when_told_to_discard_them() {
        let Some((root, run)) = scratch_repo("discard") else {
            return;
        };
        assert!(run(&["commit", "-q", "--allow-empty", "-m", "init"]));
        assert!(run(&["worktree", "add", "-q", "dirty", "-b", "a"]));
        let dirty = root.join("dirty");
        std::fs::write(dirty.join("file.txt"), "work").unwrap();

        assert!(remove_worktree(&root, &dirty, Removal::default()).is_err());
        assert!(dirty.join("file.txt").exists());
        let discard = Removal {
            discard: true,
            ..Default::default()
        };
        assert_eq!(remove_worktree(&root, &dirty, discard), Ok(()));
        assert!(!dirty.exists());
        // Its branch stays.
        assert!(run(&["rev-parse", "--verify", "-q", "refs/heads/a"]));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn removes_a_locked_worktree_only_when_told_to_unlock_it() {
        let Some((root, run)) = scratch_repo("locked") else {
            return;
        };
        assert!(run(&["commit", "-q", "--allow-empty", "-m", "init"]));
        assert!(run(&["worktree", "add", "-q", "held", "-b", "a"]));
        assert!(run(&[
            "worktree",
            "lock",
            "--reason",
            "claude agent x (pid 1)",
            "held"
        ]));
        let held = root.join("held");
        std::fs::write(held.join("file.txt"), "work").unwrap();
        let unlock = Removal {
            unlock: true,
            ..Default::default()
        };

        assert!(remove_worktree(&root, &held, Removal::default()).is_err());
        // Unlocking alone never deletes work.
        assert!(remove_worktree(&root, &held, unlock).is_err());
        assert!(held.join("file.txt").exists());
        std::fs::remove_file(held.join("file.txt")).unwrap();
        assert!(run(&["worktree", "lock", "held"]));
        assert_eq!(remove_worktree(&root, &held, unlock), Ok(()));
        assert!(!held.exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn removes_clean_worktrees_and_refuses_dirty_ones() {
        let root = std::env::temp_dir().join(format!("claudash-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let run = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["-c", "user.email=t@t", "-c", "user.name=t"])
                .args(args)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        };
        if !run(&["init", "-q"]) {
            return; // No git on this machine.
        }
        assert!(run(&["commit", "-q", "--allow-empty", "-m", "init"]));
        assert!(run(&["worktree", "add", "-q", "clean", "-b", "a"]));
        assert!(run(&["worktree", "add", "-q", "dirty", "-b", "b"]));
        std::fs::write(root.join("dirty/file.txt"), "work").unwrap();

        assert!(remove_worktree(&root, &root.join("dirty"), Removal::default()).is_err());
        assert!(root.join("dirty/file.txt").exists());
        assert!(remove_worktree(&root, &root.join("clean"), Removal::default()).is_ok());
        assert!(!root.join("clean").exists());

        // A worktree whose directory vanished is pruned from git's records.
        std::fs::remove_dir_all(root.join("dirty")).unwrap();
        assert!(worktrees(&root).iter().any(|w| w.prunable));
        prune_worktrees(&root).unwrap();
        assert_eq!(worktrees(&root).len(), 1);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
