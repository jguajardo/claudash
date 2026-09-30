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
    /// Git considers it removable: its directory is gone.
    pub prunable: bool,
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
            } else if line.starts_with("locked") {
                w.locked = true;
            } else if line.starts_with("prunable") {
                w.prunable = true;
            }
        }
    }
    list.extend(current);
    list
}

/// Removes a worktree the safe way: git refuses when it has changes or
/// untracked files, or is locked.
pub fn remove_worktree(main_checkout: &Path, worktree: &Path) -> Result<(), String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(main_checkout)
        .args(["worktree", "remove"])
        .arg(worktree)
        .output()
        .map_err(|e| format!("Could not run git: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// Drops the records of worktrees whose directory is gone (`git worktree prune`).
pub fn prune_worktrees(main_checkout: &Path) -> Result<(), String> {
    git(main_checkout, &["worktree", "prune"])
        .map(|_| ())
        .ok_or_else(|| "git worktree prune failed".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

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

        assert!(remove_worktree(&root, &root.join("dirty")).is_err());
        assert!(root.join("dirty/file.txt").exists());
        assert!(remove_worktree(&root, &root.join("clean")).is_ok());
        assert!(!root.join("clean").exists());

        // A worktree whose directory vanished is pruned from git's records.
        std::fs::remove_dir_all(root.join("dirty")).unwrap();
        assert!(worktrees(&root).iter().any(|w| w.prunable));
        prune_worktrees(&root).unwrap();
        assert_eq!(worktrees(&root).len(), 1);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
