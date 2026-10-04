//! claudash's own usage history (`usage-history.json` in its data directory).
//!
//! Claude Code deletes transcripts after `cleanupPeriodDays` (30 days by
//! default), which would take their usage out of the charts. Every refresh
//! copies each session's per-day, per-model usage here, so the Usage view keeps
//! months of history for sessions that are gone.

use std::{
    collections::{BTreeMap, HashMap},
    fs, io,
    path::PathBuf,
};

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::sessions::{Daily, Session, Usage};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Entry {
    project: String,
    daily: Daily,
}

#[derive(Default)]
pub struct History {
    sessions: BTreeMap<String, Entry>,
    file: Option<PathBuf>,
}

impl History {
    pub fn load() -> Self {
        Self::load_from(crate::library::data_dir().map(|d| d.join("usage-history.json")))
    }

    fn load_from(file: Option<PathBuf>) -> Self {
        let sessions = file
            .as_ref()
            .and_then(|f| fs::read(f).ok())
            .and_then(|raw| serde_json::from_slice(&raw).ok())
            .unwrap_or_default();
        History { sessions, file }
    }

    /// Records the current sessions' usage; saves only when something changed.
    pub fn merge(&mut self, current: &[Session]) -> io::Result<()> {
        let mut changed = false;
        for s in current.iter().filter(|s| !s.tokens.daily.is_empty()) {
            let entry = Entry {
                project: s.project_path.clone(),
                daily: s.tokens.daily.clone(),
            };
            if self.sessions.get(&s.id) != Some(&entry) {
                self.sessions.insert(s.id.clone(), entry);
                changed = true;
            }
        }
        if changed { self.save() } else { Ok(()) }
    }

    fn save(&self) -> io::Result<()> {
        let Some(file) = &self.file else {
            return Ok(());
        };
        if let Some(dir) = file.parent() {
            fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string(&self.sessions).map_err(io::Error::other)?;
        let tmp = file.with_extension("json.tmp");
        fs::write(&tmp, text)?;
        fs::rename(tmp, file)
    }

    /// Usage per day, all sessions and models together.
    pub fn per_day(&self) -> BTreeMap<NaiveDate, Usage> {
        let mut days: BTreeMap<NaiveDate, Usage> = BTreeMap::new();
        for entry in self.sessions.values() {
            for (day, models) in &entry.daily {
                let total = days.entry(*day).or_default();
                for usage in models.values() {
                    total.add(usage);
                }
            }
        }
        days
    }

    /// Usage per model from `since` on.
    pub fn per_model(&self, since: NaiveDate) -> Vec<(String, Usage)> {
        let mut models: HashMap<String, Usage> = HashMap::new();
        for entry in self.sessions.values() {
            for (_, day_models) in entry.daily.range(since..) {
                for (model, usage) in day_models {
                    let name = if model.is_empty() {
                        "unknown"
                    } else {
                        model.as_str()
                    };
                    models.entry(name.to_string()).or_default().add(usage);
                }
            }
        }
        let mut sorted: Vec<(String, Usage)> = models.into_iter().collect();
        sorted.sort_by_key(|(_, u)| std::cmp::Reverse(u.processed()));
        sorted
    }

    /// Usage per project from `since` on, most processed tokens first.
    pub fn per_project(&self, since: NaiveDate) -> Vec<(String, Usage)> {
        let mut projects: HashMap<&str, Usage> = HashMap::new();
        for entry in self.sessions.values() {
            let mut total = Usage::default();
            for usage in entry
                .daily
                .range(since..)
                .flat_map(|(_, models)| models.values())
            {
                total.add(usage);
            }
            if total.processed() > 0 {
                projects
                    .entry(entry.project.as_str())
                    .or_default()
                    .add(&total);
            }
        }
        let mut sorted: Vec<(String, Usage)> = projects
            .into_iter()
            .map(|(p, u)| (p.to_string(), u))
            .collect();
        sorted.sort_by_key(|(_, u)| std::cmp::Reverse(u.processed()));
        sorted
    }

    /// Earliest day with recorded usage.
    pub fn first_day(&self) -> Option<NaiveDate> {
        self.sessions
            .values()
            .filter_map(|e| e.daily.keys().next().copied())
            .min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    fn session(id: &str, day: NaiveDate, model: &str, output: u64) -> Session {
        let mut daily = Daily::new();
        daily.entry(day).or_default().insert(
            model.into(),
            Usage {
                output_tokens: output,
                ..Default::default()
            },
        );
        Session {
            id: id.into(),
            path: PathBuf::new(),
            title: String::new(),
            project_path: "~/p".into(),
            cwd: None,
            git_branch: None,
            modified: SystemTime::now(),
            size: 0,
            tokens: crate::sessions::SessionTokens {
                daily,
                ..Default::default()
            },
        }
    }

    #[test]
    fn keeps_sessions_that_disappear_and_aggregates() {
        let file =
            std::env::temp_dir().join(format!("claudash-history-{}.json", std::process::id()));
        let _ = fs::remove_file(&file);
        let d1 = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let d2 = NaiveDate::from_ymd_opt(2026, 1, 2).unwrap();

        let mut history = History::load_from(Some(file.clone()));
        history
            .merge(&[session("a", d1, "opus", 10), session("b", d2, "sonnet", 5)])
            .unwrap();
        // Session "a" is gone (cleaned up by Claude Code); "b" grew.
        history.merge(&[session("b", d2, "sonnet", 7)]).unwrap();

        let reloaded = History::load_from(Some(file.clone()));
        fs::remove_file(&file).unwrap();
        let days = reloaded.per_day();
        assert_eq!(days[&d1].output_tokens, 10);
        assert_eq!(days[&d2].output_tokens, 7);
        assert_eq!(reloaded.first_day(), Some(d1));
        let models = reloaded.per_model(d1);
        assert_eq!(models[0].0, "opus");
        let projects = reloaded.per_project(d2);
        assert_eq!(projects.len(), 1);
        assert_eq!(
            (projects[0].0.as_str(), projects[0].1.processed()),
            ("~/p", 7)
        );
    }
}
