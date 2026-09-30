//! claudash's own records about sessions, kept in its data directory
//! (`~/.local/share/claudash` on Linux) and never in Claude Code's files:
//!
//! - `library.json`: tags, a note and a star per session ID.
//! - `trash/<id>/`: sessions moved out of Claude Code's directories, with a
//!   `manifest.json` saying where each file came from, so they can be restored.

use std::{
    collections::HashMap,
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::sessions::Session;

/// Trashed sessions are deleted for good after this many days.
pub const TRASH_DAYS: i64 = 30;

pub fn data_dir() -> Option<PathBuf> {
    // Tests never touch the real data directory; they pass explicit paths.
    if cfg!(test) {
        return None;
    }
    dirs::data_dir().map(|dir| dir.join("claudash"))
}

// ---- Tags, notes and stars ---------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Meta {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub starred: bool,
}

impl Meta {
    fn is_empty(&self) -> bool {
        *self == Meta::default()
    }
}

#[derive(Default)]
pub struct Library {
    pub meta: HashMap<String, Meta>,
    file: Option<PathBuf>,
}

impl Library {
    pub fn load() -> Self {
        let file = data_dir().map(|d| d.join("library.json"));
        Self::load_from(file)
    }

    fn load_from(file: Option<PathBuf>) -> Self {
        let meta = file
            .as_ref()
            .and_then(|f| fs::read(f).ok())
            .and_then(|raw| serde_json::from_slice(&raw).ok())
            .unwrap_or_default();
        Library { meta, file }
    }

    pub fn get(&self, id: &str) -> Option<&Meta> {
        self.meta.get(id)
    }

    /// Changes a session's record and saves the library.
    pub fn update(&mut self, id: &str, change: impl FnOnce(&mut Meta)) -> io::Result<()> {
        let entry = self.meta.entry(id.to_string()).or_default();
        change(entry);
        if entry.is_empty() {
            self.meta.remove(id);
        }
        self.save()
    }

    fn save(&self) -> io::Result<()> {
        let Some(file) = &self.file else {
            return Ok(());
        };
        if let Some(dir) = file.parent() {
            fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(&self.meta).map_err(io::Error::other)?;
        let tmp = file.with_extension("json.tmp");
        fs::write(&tmp, text)?;
        fs::rename(tmp, file)
    }
}

/// "api, #urgent,  bug" -> ["api", "urgent", "bug"]: trimmed, no `#`, no
/// duplicates, lowercase.
pub fn parse_tags(input: &str) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    for tag in input.split([',', ' ']) {
        let tag = tag.trim().trim_start_matches('#').to_lowercase();
        if !tag.is_empty() && !tags.contains(&tag) {
            tags.push(tag);
        }
    }
    tags
}

// ---- Trash -------------------------------------------------------------------

/// Where a trashed file or directory lived, and its name inside the trash.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Moved {
    original: PathBuf,
    stored: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Trashed {
    pub id: String,
    pub title: String,
    pub project: String,
    /// Epoch seconds.
    pub trashed_at: i64,
    pub size: u64,
    moved: Vec<Moved>,
}

fn trash_dir() -> io::Result<PathBuf> {
    data_dir()
        .map(|d| d.join("trash"))
        .ok_or_else(|| io::Error::other("could not find the data directory"))
}

/// Session IDs are UUIDs; never build a path from anything else.
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// Everything Claude Code keeps for a session: its transcript, the sibling
/// directory with subagents and tool results, and its `file-history` and
/// `session-env` directories.
fn session_paths(session: &Session) -> Vec<PathBuf> {
    let mut paths = vec![session.path.clone(), session.path.with_extension("")];
    if let Some(home) = crate::paths::claude_home()
        && valid_id(&session.id)
    {
        paths.push(home.join("file-history").join(&session.id));
        paths.push(home.join("session-env").join(&session.id));
    }
    paths.into_iter().filter(|p| p.exists()).collect()
}

/// Renames, or copies and deletes when source and target are on different
/// file systems.
fn move_path(from: &Path, to: &Path) -> io::Result<()> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    if fs::rename(from, to).is_ok() {
        return Ok(());
    }
    copy_recursive(from, to)?;
    if from.is_dir() {
        fs::remove_dir_all(from)
    } else {
        fs::remove_file(from)
    }
}

fn copy_recursive(from: &Path, to: &Path) -> io::Result<()> {
    if from.is_dir() {
        fs::create_dir_all(to)?;
        for entry in fs::read_dir(from)?.flatten() {
            copy_recursive(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(from, to).map(|_| ())
    }
}

fn dir_size(path: &Path) -> u64 {
    if path.is_dir() {
        fs::read_dir(path)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| dir_size(&e.path()))
            .sum()
    } else {
        fs::metadata(path).map(|m| m.len()).unwrap_or(0)
    }
}

/// Moves a session out of Claude Code's directories into the trash.
pub fn trash(session: &Session) -> io::Result<()> {
    if !valid_id(&session.id) {
        return Err(io::Error::other("unexpected session ID"));
    }
    trash_into(&trash_dir()?, session, &session_paths(session))
}

fn trash_into(trash: &Path, session: &Session, paths: &[PathBuf]) -> io::Result<()> {
    let dir = trash.join(&session.id);
    fs::create_dir_all(&dir)?;
    let mut moved = Vec::new();
    let mut size = 0;
    for (i, original) in paths.iter().enumerate() {
        let stored = format!(
            "{i}-{}",
            original.file_name().unwrap_or_default().to_string_lossy()
        );
        size += dir_size(original);
        move_path(original, &dir.join(&stored))?;
        moved.push(Moved {
            original: original.clone(),
            stored,
        });
    }
    let item = Trashed {
        id: session.id.clone(),
        title: session.title.clone(),
        project: session.project_path.clone(),
        trashed_at: chrono::Utc::now().timestamp(),
        size,
        moved,
    };
    let manifest = serde_json::to_vec_pretty(&item).map_err(io::Error::other)?;
    fs::write(dir.join("manifest.json"), manifest)
}

/// Trashed sessions, newest first.
pub fn list_trash() -> Vec<Trashed> {
    trash_dir().map(|t| list_in(&t)).unwrap_or_default()
}

fn list_in(trash: &Path) -> Vec<Trashed> {
    let mut items: Vec<Trashed> = fs::read_dir(trash)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| fs::read(e.path().join("manifest.json")).ok())
        .filter_map(|raw| serde_json::from_slice(&raw).ok())
        .collect();
    items.sort_by_key(|t| std::cmp::Reverse(t.trashed_at));
    items
}

/// Puts a trashed session back where it was.
pub fn restore(item: &Trashed) -> io::Result<()> {
    restore_from(&trash_dir()?, item)
}

fn restore_from(trash: &Path, item: &Trashed) -> io::Result<()> {
    if !valid_id(&item.id) {
        return Err(io::Error::other("unexpected session ID"));
    }
    let dir = trash.join(&item.id);
    for m in &item.moved {
        if m.original.exists() {
            return Err(io::Error::other(format!(
                "{} already exists; not overwriting it",
                crate::paths::display(&m.original)
            )));
        }
    }
    for m in &item.moved {
        move_path(&dir.join(&m.stored), &m.original)?;
    }
    fs::remove_dir_all(dir)
}

/// Deletes a trashed session for good.
pub fn purge(item: &Trashed) -> io::Result<()> {
    if !valid_id(&item.id) {
        return Err(io::Error::other("unexpected session ID"));
    }
    fs::remove_dir_all(trash_dir()?.join(&item.id))
}

/// Deletes trashed sessions older than [`TRASH_DAYS`]. Returns how many.
pub fn purge_expired() -> usize {
    let cutoff = chrono::Utc::now().timestamp() - TRASH_DAYS * 86_400;
    list_trash()
        .iter()
        .filter(|t| t.trashed_at < cutoff)
        .filter(|t| purge(t).is_ok())
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    fn session(id: &str, path: PathBuf) -> Session {
        Session {
            id: id.into(),
            path,
            title: "A title".into(),
            project_path: "~/p".into(),
            cwd: None,
            git_branch: None,
            modified: SystemTime::now(),
            size: 0,
            tokens: Default::default(),
        }
    }

    #[test]
    fn tags_are_normalized() {
        assert_eq!(
            parse_tags("api, #Urgent  bug,api"),
            ["api", "urgent", "bug"]
        );
        assert!(parse_tags(" , ").is_empty());
    }

    #[test]
    fn library_round_trips_and_drops_empty_records() {
        let file = std::env::temp_dir().join(format!("claudash-lib-{}.json", std::process::id()));
        let mut lib = Library::load_from(Some(file.clone()));
        lib.update("s1", |m| {
            m.tags = vec!["api".into()];
            m.starred = true;
        })
        .unwrap();
        lib.update("s2", |m| m.note = "x".into()).unwrap();
        lib.update("s2", |m| m.note.clear()).unwrap();
        let again = Library::load_from(Some(file.clone()));
        fs::remove_file(&file).unwrap();
        assert!(again.get("s1").unwrap().starred);
        assert!(again.get("s2").is_none());
    }

    #[test]
    fn trash_and_restore_put_files_back() {
        let root = std::env::temp_dir().join(format!("claudash-trash-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let project = root.join("projects/p");
        let id = "0123abcd-0000-0000-0000-000000000000";
        fs::create_dir_all(project.join(id).join("subagents")).unwrap();
        fs::write(project.join(format!("{id}.jsonl")), "transcript").unwrap();
        fs::write(project.join(id).join("subagents/a.jsonl"), "sub").unwrap();
        let s = session(id, project.join(format!("{id}.jsonl")));
        let paths = vec![s.path.clone(), s.path.with_extension("")];
        let trash = root.join("trash");

        trash_into(&trash, &s, &paths).unwrap();
        assert!(!s.path.exists());
        let items = list_in(&trash);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].size, 13);

        restore_from(&trash, &items[0]).unwrap();
        assert_eq!(fs::read_to_string(&s.path).unwrap(), "transcript");
        assert!(project.join(id).join("subagents/a.jsonl").exists());
        assert!(list_in(&trash).is_empty());
        fs::remove_dir_all(&root).unwrap();
    }
}
