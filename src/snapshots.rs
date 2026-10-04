//! Snapshots: an opt-in safety net for what `/rewind` can't undo.
//!
//! Claude Code's checkpoints don't cover files changed by Bash commands or by
//! subagents. With `snapshots = true` in claudash's settings, `claudash hook`
//! commits the project's working tree to a separate "shadow" git repository in
//! claudash's data directory before each prompt and after each reply. The
//! project's own `.git` is never touched; its `.gitignore` rules apply, so
//! ignored files (build output, `.env`) are not copied.

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// One snapshot of a project's working tree.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub sha: String,
    /// Epoch seconds.
    pub at: i64,
    /// "before prompt" or "after reply", or "before restore".
    pub label: String,
    pub session_id: String,
    /// Files that changed since the previous snapshot.
    pub files: u32,
}

/// "Oct 04 14:20" for a snapshot's time.
pub fn when(at: i64) -> String {
    chrono::DateTime::from_timestamp(at, 0)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%b %d %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

/// Whether snapshots are turned on in the settings file.
pub fn enabled() -> bool {
    crate::config::load()
        .ok()
        .and_then(|c| c.snapshots)
        .unwrap_or(false)
}

fn store() -> Option<PathBuf> {
    crate::library::data_dir().map(|d| d.join("snapshots"))
}

/// The shadow repository for the work tree at `root`.
fn shadow(store: &Path, root: &Path) -> PathBuf {
    let name: String = root
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    store.join(name.trim_matches('-'))
}

/// git with the shadow repository and the project as its work tree.
fn git(shadow: &Path, root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("--git-dir")
        .arg(shadow)
        .arg("--work-tree")
        .arg(root)
        .args([
            "-c",
            "user.name=claudash",
            "-c",
            "user.email=claudash@localhost",
        ])
        .args(["-c", "core.autocrlf=false", "-c", "gc.auto=200"])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// The top of the git checkout `dir` is in; snapshots only cover checkouts.
pub fn project_root(dir: &Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "--show-toplevel"])
        .stdin(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| PathBuf::from(String::from_utf8_lossy(&output.stdout).trim()))
}

/// Commits the work tree at `root` if anything changed since the last
/// snapshot. Returns the new snapshot's sha.
pub fn take(root: &Path, session_id: &str, label: &str) -> Result<Option<String>, String> {
    let store = store().ok_or("no data directory")?;
    take_in(&store, root, session_id, label)
}

fn take_in(
    store: &Path,
    root: &Path,
    session_id: &str,
    label: &str,
) -> Result<Option<String>, String> {
    let shadow = shadow(store, root);
    if !shadow.join("HEAD").exists() {
        std::fs::create_dir_all(&shadow).map_err(|e| e.to_string())?;
        let init = Command::new("git")
            .args(["init", "-q", "--bare"])
            .arg(&shadow)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("could not run git: {e}"))?;
        if !init.status.success() {
            return Err(String::from_utf8_lossy(&init.stderr).trim().to_string());
        }
        // The project's own repository is never part of a snapshot.
        std::fs::create_dir_all(shadow.join("info")).map_err(|e| e.to_string())?;
        std::fs::write(shadow.join("info/exclude"), ".git\n").map_err(|e| e.to_string())?;
    }
    git(&shadow, root, &["add", "-A", "."])?;
    let has_head = git(&shadow, root, &["rev-parse", "-q", "--verify", "HEAD"]).is_ok();
    if has_head && git(&shadow, root, &["diff", "--cached", "--quiet"]).is_ok() {
        return Ok(None); // Nothing changed.
    }
    git(
        &shadow,
        root,
        &[
            "commit",
            "-q",
            "--no-verify",
            "-m",
            &format!("{label}\n\nsession {session_id}"),
        ],
    )?;
    git(&shadow, root, &["rev-parse", "HEAD"]).map(|s| Some(s.trim().to_string()))
}

/// Snapshots of the work tree at `root`, newest first.
pub fn list(root: &Path) -> Vec<Snapshot> {
    store().map(|s| list_in(&s, root)).unwrap_or_default()
}

fn list_in(store: &Path, root: &Path) -> Vec<Snapshot> {
    let shadow = shadow(store, root);
    if !shadow.join("HEAD").exists() {
        return Vec::new();
    }
    let Ok(log) = git(
        &shadow,
        root,
        &[
            "log",
            "-200",
            "--format=@%H%x09%ct%x09%s%x09%b",
            "--shortstat",
        ],
    ) else {
        return Vec::new();
    };
    let mut out: Vec<Snapshot> = Vec::new();
    for line in log.lines() {
        if let Some(rest) = line.strip_prefix('@') {
            let mut parts = rest.split('\t');
            let (Some(sha), Some(at), Some(label)) = (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let session_id = parts
                .next()
                .and_then(|b| b.strip_prefix("session "))
                .unwrap_or_default()
                .to_string();
            out.push(Snapshot {
                sha: sha.to_string(),
                at: at.parse().unwrap_or(0),
                label: label.to_string(),
                session_id,
                files: 0,
            });
        } else if let Some(last) = out.last_mut()
            && let Some(n) = line.trim().split(' ').next().and_then(|n| n.parse().ok())
        {
            last.files = n;
        }
    }
    out
}

/// What changed in a snapshot: summary and patch against the one before.
pub fn diff(root: &Path, sha: &str) -> Result<String, String> {
    let store = store().ok_or("no data directory")?;
    git(
        &shadow(&store, root),
        root,
        &["show", "--stat", "--patch", "--format=%s%n", sha],
    )
}

/// Puts the files of a snapshot back in the work tree, after taking a
/// snapshot of the current state so the restore can be undone too. Files
/// created after the snapshot are left alone.
pub fn restore(root: &Path, sha: &str) -> Result<(), String> {
    let store = store().ok_or("no data directory")?;
    restore_in(&store, root, sha)
}

fn restore_in(store: &Path, root: &Path, sha: &str) -> Result<(), String> {
    take_in(store, root, "claudash", "before restore")?;
    git(&shadow(store, root), root, &["checkout", sha, "--", "."]).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn snapshots_and_restores_without_touching_the_project_repo() {
        let base = std::env::temp_dir().join(format!("claudash-snap-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let (root, store) = (base.join("project"), base.join("store"));
        fs::create_dir_all(&root).unwrap();
        let ok = Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["init", "-q"])
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            return; // No git on this machine.
        }
        fs::write(root.join(".gitignore"), "secret.env\n").unwrap();
        fs::write(root.join("secret.env"), "KEY=1").unwrap();
        fs::write(root.join("a.txt"), "one").unwrap();

        let first = take_in(&store, &root, "s1", "before prompt").unwrap();
        assert!(first.is_some());
        // Nothing changed: no new snapshot.
        assert_eq!(take_in(&store, &root, "s1", "after reply").unwrap(), None);
        // A Bash command deletes a file and rewrites another.
        fs::write(root.join("a.txt"), "broken").unwrap();
        fs::write(root.join("b.txt"), "new").unwrap();
        take_in(&store, &root, "s1", "after reply")
            .unwrap()
            .unwrap();

        let list = list_in(&store, &root);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].label, "after reply");
        assert_eq!(list[0].session_id, "s1");
        assert_eq!(list[0].files, 2);

        // More edits after the last snapshot: restoring snapshots them first.
        fs::write(root.join("a.txt"), "worse").unwrap();
        restore_in(&store, &root, first.as_deref().unwrap()).unwrap();
        assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "one");
        // The restore itself was snapshotted first.
        assert_eq!(list_in(&store, &root)[0].label, "before restore");
        // The project's own repository has no commits; ignored files weren't copied.
        let log = Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["log"])
            .output()
            .unwrap();
        assert!(!log.status.success());
        let tracked = git(&shadow(&store, &root), &root, &["ls-files"]).unwrap();
        assert!(!tracked.contains("secret.env") && tracked.contains("a.txt"));
        fs::remove_dir_all(&base).unwrap();
    }
}
