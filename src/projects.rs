//! The repositories sessions ran in, each with all its checkouts (the main one
//! and its worktrees) and their git state.

use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
};

use crate::git::{self, Status};

#[derive(Clone, Debug, PartialEq)]
pub struct Checkout {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub status: Option<Status>,
    /// The repository's main checkout (not a linked worktree).
    pub main: bool,
    pub locked: bool,
    /// Its directory is gone; `git worktree prune` would drop it.
    pub prunable: bool,
    /// Created by Claude Code (`claude --worktree`, subagents, background sessions).
    pub claude_created: bool,
}

impl Checkout {
    /// Uncommitted or unpushed work that removing it would lose.
    pub fn has_work(&self) -> bool {
        self.status
            .as_ref()
            .is_some_and(|s| s.changed > 0 || s.ahead > 0)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Repo {
    pub name: String,
    pub checkouts: Vec<Checkout>,
}

/// Git status per session folder (`None` outside a repository).
pub type Statuses = HashMap<PathBuf, Option<Status>>;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Model {
    pub repos: Vec<Repo>,
    /// Session folders outside any git repository.
    pub loose: Vec<PathBuf>,
}

impl Model {
    pub fn checkout(&self, path: &Path) -> Option<&Checkout> {
        self.repos
            .iter()
            .flat_map(|r| &r.checkouts)
            .find(|c| c.path == path)
    }
}

/// Builds the model for the given session folders. Also returns the git status
/// of each folder, for the Dashboard. Runs git several times per repository:
/// call it off the UI thread.
pub fn build(dirs: &[PathBuf]) -> (Model, Statuses) {
    let mut statuses: HashMap<PathBuf, Option<Status>> = HashMap::new();
    // Repositories keyed by their shared .git directory.
    let mut repos: BTreeMap<PathBuf, PathBuf> = BTreeMap::new();
    let mut model = Model::default();
    for dir in dirs {
        let status = git::status(dir);
        match &status {
            Some(s) => {
                repos
                    .entry(s.common_dir.clone())
                    .or_insert_with(|| dir.clone());
            }
            None => model.loose.push(dir.clone()),
        }
        statuses.insert(dir.clone(), status);
    }

    for any_dir in repos.values() {
        let worktrees = git::worktrees(any_dir);
        let checkouts: Vec<Checkout> = worktrees
            .iter()
            .enumerate()
            .map(|(i, w)| {
                let status = if w.prunable {
                    None
                } else {
                    statuses
                        .get(&w.path)
                        .cloned()
                        .unwrap_or_else(|| git::status(&w.path))
                };
                Checkout {
                    path: w.path.clone(),
                    branch: w.branch.clone(),
                    status,
                    main: i == 0,
                    locked: w.locked,
                    prunable: w.prunable,
                    claude_created: w.path.to_string_lossy().contains("/.claude/worktrees/"),
                }
            })
            .collect();
        let name = checkouts
            .first()
            .and_then(|c| c.path.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        model.repos.push(Repo { name, checkouts });
    }
    model.repos.sort_by_key(|r| r.name.to_lowercase());
    model.loose.sort();
    (model, statuses)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_this_repository_and_loose_folders() {
        let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let outside = std::env::temp_dir();
        let (model, statuses) = build(&[here.clone(), outside.clone()]);
        if statuses[&here].is_some() {
            let repo = model
                .repos
                .iter()
                .find(|r| r.checkouts.iter().any(|c| here.starts_with(&c.path)))
                .expect("the crate's repository");
            assert!(repo.checkouts[0].main);
        }
        // The temp dir is normally not a repository.
        if statuses[&outside].is_none() {
            assert!(model.loose.contains(&outside));
        }
    }
}
