//! Where a plan window went: the usage of every session, project and model
//! inside the current 5-hour and 7-day windows.
//!
//! The windows come from Claude Code's status line (`rate_limits`, with their
//! reset times and how much is used) when it's set up. Without it, the
//! 5-hour window is estimated the way it's commonly understood: it starts with
//! the first request after the previous one ended, at the top of that hour.
//!
//! How much of the limit each session took is an estimate: Anthropic doesn't
//! publish how requests count against plan limits, so claudash splits the used
//! percentage in proportion to each session's API-equivalent cost, which
//! weighs models and cache use the way prices do.

use std::collections::HashMap;

use crate::{
    sessions::{Session, Usage},
    statusline::Store,
};

const HOUR: i64 = 3600;
const FIVE_HOURS: i64 = 5 * HOUR;
const SEVEN_DAYS: i64 = 7 * 24 * HOUR;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Window {
    /// Epoch seconds.
    pub start: i64,
    pub end: i64,
    /// Share of the plan limit used, when Claude Code reported it.
    pub used: Option<f64>,
    /// Whether the bounds came from Claude Code rather than an estimate.
    pub reported: bool,
}

/// One session's part of a window.
#[derive(Clone, Debug, PartialEq)]
pub struct Share {
    pub session_id: String,
    pub title: String,
    pub project: String,
    pub usage: Usage,
    /// Fraction of the window's API-equivalent cost, 0–1.
    pub fraction: f64,
}

impl Share {
    /// Estimated points of the plan limit, when the window's use is known.
    pub fn limit_points(&self, window: &Window) -> Option<f64> {
        window.used.map(|used| used * self.fraction)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ledger {
    pub sessions: Vec<Share>,
    /// Totals per project and per model, largest first.
    pub projects: Vec<(String, Usage)>,
    pub models: Vec<(String, Usage)>,
    pub total: Usage,
}

/// The current 5-hour window.
pub fn five_hour(sessions: &[Session], store: &Store, now: i64) -> Option<Window> {
    if let Some(w) = store
        .rate_limits
        .as_ref()
        .and_then(|(l, _)| l.five_hour)
        .filter(|w| w.resets_at > now)
    {
        return Some(Window {
            start: w.resets_at - FIVE_HOURS,
            end: now,
            used: Some(w.used_percentage),
            reported: true,
        });
    }
    let mut times: Vec<i64> = sessions
        .iter()
        .flat_map(|s| s.tokens.timeline.iter().map(|(at, _)| *at))
        .filter(|at| *at > now - 2 * SEVEN_DAYS)
        .collect();
    times.sort_unstable();
    let start = estimate_block(&times)?;
    (start + FIVE_HOURS > now).then_some(Window {
        start,
        end: now,
        used: None,
        reported: false,
    })
}

/// Start of the last 5-hour block: a block begins at the top of the hour of
/// the first request after the previous block ended.
fn estimate_block(times: &[i64]) -> Option<i64> {
    let mut start: Option<i64> = None;
    for &t in times {
        if start.is_none_or(|s| t >= s + FIVE_HOURS) {
            start = Some(t - t.rem_euclid(HOUR));
        }
    }
    start
}

/// The current 7-day window (the last seven days when it isn't reported).
pub fn seven_day(store: &Store, now: i64) -> Window {
    match store
        .rate_limits
        .as_ref()
        .and_then(|(l, _)| l.seven_day)
        .filter(|w| w.resets_at > now)
    {
        Some(w) => Window {
            start: w.resets_at - SEVEN_DAYS,
            end: now,
            used: Some(w.used_percentage),
            reported: true,
        },
        None => Window {
            start: now - SEVEN_DAYS,
            end: now,
            used: None,
            reported: false,
        },
    }
}

/// Who used what inside `window`. `models` gives each response's model, by
/// session: sessions only record per-day model totals, so models are split by
/// that day's proportions.
pub fn ledger(sessions: &[Session], window: &Window) -> Ledger {
    let mut ledger = Ledger::default();
    let mut projects: HashMap<String, Usage> = HashMap::new();
    let mut models: HashMap<String, Usage> = HashMap::new();
    for s in sessions {
        let mut usage = Usage::default();
        for (at, u) in &s.tokens.timeline {
            if *at >= window.start && *at <= window.end {
                usage.add(u);
            }
        }
        if usage.processed() == 0 && usage.cost == 0.0 {
            continue;
        }
        ledger.total.add(&usage);
        projects
            .entry(s.project_path.clone())
            .or_default()
            .add(&usage);
        // Split by the models used on the window's days.
        let first_day = chrono::DateTime::from_timestamp(window.start, 0)
            .map(|t| t.with_timezone(&chrono::Local).date_naive());
        let mut day_models: HashMap<&str, f64> = HashMap::new();
        let mut day_total = 0.0;
        for (day, per_model) in &s.tokens.daily {
            if first_day.is_some_and(|d| *day < d) {
                continue;
            }
            for (model, u) in per_model {
                *day_models.entry(model.as_str()).or_default() += u.cost;
                day_total += u.cost;
            }
        }
        for (model, cost) in day_models {
            if day_total > 0.0 {
                let f = cost / day_total;
                let entry = models.entry(model.to_string()).or_default();
                entry.cost += usage.cost * f;
                entry.output_tokens += (usage.output_tokens as f64 * f) as u64;
                entry.input_tokens += (usage.input_tokens as f64 * f) as u64;
                entry.cache_creation_input_tokens +=
                    (usage.cache_creation_input_tokens as f64 * f) as u64;
                entry.cache_read_input_tokens += (usage.cache_read_input_tokens as f64 * f) as u64;
            }
        }
        ledger.sessions.push(Share {
            session_id: s.id.clone(),
            title: s.title.clone(),
            project: s.project_path.clone(),
            usage,
            fraction: 0.0,
        });
    }
    let total_cost = ledger.total.cost;
    for share in &mut ledger.sessions {
        share.fraction = if total_cost > 0.0 {
            share.usage.cost / total_cost
        } else {
            0.0
        };
    }
    ledger
        .sessions
        .sort_by(|a, b| b.usage.cost.total_cmp(&a.usage.cost));
    ledger.projects = projects.into_iter().collect();
    ledger
        .projects
        .sort_by(|a, b| b.1.cost.total_cmp(&a.1.cost));
    ledger.models = models.into_iter().filter(|(m, _)| !m.is_empty()).collect();
    ledger.models.sort_by(|a, b| b.1.cost.total_cmp(&a.1.cost));
    ledger
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::SessionTokens;
    use std::{path::PathBuf, time::SystemTime};

    fn session(id: &str, project: &str, timeline: Vec<(i64, f64)>) -> Session {
        Session {
            id: id.into(),
            path: PathBuf::new(),
            title: id.into(),
            project_path: project.into(),
            cwd: None,
            git_branch: None,
            modified: SystemTime::now(),
            size: 0,
            tokens: SessionTokens {
                timeline: timeline
                    .into_iter()
                    .map(|(at, cost)| {
                        (
                            at,
                            Usage {
                                output_tokens: 100,
                                cost,
                                ..Default::default()
                            },
                        )
                    })
                    .collect(),
                ..Default::default()
            },
        }
    }

    #[test]
    fn estimates_five_hour_blocks_from_activity() {
        let h = HOUR;
        // 10:20 starts a block at 10:00; 14:59 is inside it; 15:10 starts a new one at 15:00.
        assert_eq!(
            estimate_block(&[10 * h + 1200, 14 * h + 3540]),
            Some(10 * h)
        );
        assert_eq!(
            estimate_block(&[10 * h + 1200, 15 * h + 600, 16 * h]),
            Some(15 * h)
        );
        assert_eq!(estimate_block(&[]), None);
    }

    #[test]
    fn splits_a_window_by_cost() {
        let sessions = [
            session("a", "~/api", vec![(100, 3.0), (5000, 1.0)]),
            session("b", "~/web", vec![(200, 1.0), (99_999, 50.0)]),
        ];
        let window = Window {
            start: 0,
            end: 10_000,
            used: Some(40.0),
            reported: true,
        };
        let ledger = ledger(&sessions, &window);
        assert_eq!(ledger.sessions[0].session_id, "a");
        assert!((ledger.sessions[0].fraction - 0.8).abs() < 1e-9);
        assert_eq!(ledger.sessions[0].limit_points(&window), Some(32.0));
        assert!((ledger.total.cost - 5.0).abs() < 1e-9);
        assert_eq!(ledger.projects[0].0, "~/api");
    }

    #[test]
    fn reported_windows_win_over_estimates() {
        let store = Store {
            rate_limits: Some((
                crate::statusline::RateLimits {
                    five_hour: Some(crate::statusline::Window {
                        used_percentage: 64.0,
                        resets_at: 20_000,
                    }),
                    seven_day: None,
                },
                SystemTime::now(),
            )),
            ..Default::default()
        };
        let w = five_hour(&[], &store, 10_000).unwrap();
        assert_eq!(
            (w.start, w.used, w.reported),
            (20_000 - FIVE_HOURS, Some(64.0), true)
        );
        let week = seven_day(&store, 10_000);
        assert!(!week.reported && week.used.is_none());
    }
}
