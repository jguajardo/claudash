//! Is your plan worth it: what your Claude Code use would cost at API prices
//! against what the plan costs, and how often each plan would have stopped
//! you at its 5-hour limit.
//!
//! Anthropic doesn't publish plan limits in tokens. Max gives "5x or 20x more
//! usage per 5-hour session than Pro", so one calibration point is enough:
//! how much API-equivalent cost fit in a 5-hour window of your plan, taken
//! from the times the limit actually stopped you (or the status line's
//! reading of the current window). Every 5-hour window of the last 30 days is
//! then compared with what each plan would hold. It's an estimate, and says so.

use std::collections::BTreeSet;

use crate::{sessions::Session, statusline::Store};

const HOUR: i64 = 3600;
const FIVE_HOURS: i64 = 5 * HOUR;
const DAY: i64 = 24 * HOUR;
/// The period looked at.
pub const DAYS: i64 = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    Pro,
    Max5,
    Max20,
    /// Pay as you go: the API-equivalent cost is what you pay.
    Api,
}

impl Plan {
    pub const SUBSCRIPTIONS: [Plan; 3] = [Plan::Pro, Plan::Max5, Plan::Max20];

    pub fn name(self) -> &'static str {
        match self {
            Plan::Pro => "Pro",
            Plan::Max5 => "Max 5x",
            Plan::Max20 => "Max 20x",
            Plan::Api => "API",
        }
    }

    /// Monthly price in USD (monthly billing), from claude.com/pricing.
    pub fn price(self) -> Option<f64> {
        match self {
            Plan::Pro => Some(20.0),
            Plan::Max5 => Some(100.0),
            Plan::Max20 => Some(200.0),
            Plan::Api => None,
        }
    }

    /// Usage per 5-hour window, relative to Pro.
    fn multiplier(self) -> f64 {
        match self {
            Plan::Pro => 1.0,
            Plan::Max5 => 5.0,
            Plan::Max20 => 20.0,
            Plan::Api => f64::INFINITY,
        }
    }

    /// `plan = "max5x"` in the settings file, or Claude Code's own tier name
    /// ("default_claude_max_5x", "claude_pro").
    pub fn parse(text: &str) -> Option<Plan> {
        let t = text.to_ascii_lowercase().replace([' ', '-', '_'], "");
        if t.contains("20x") {
            Some(Plan::Max20)
        } else if t.contains("5x") || t == "max" {
            Some(Plan::Max5)
        } else if t.contains("pro") {
            Some(Plan::Pro)
        } else if t == "api" {
            Some(Plan::Api)
        } else {
            None
        }
    }
}

/// Your plan and where claudash learned it.
pub fn detect(setting: Option<&str>) -> Option<(Plan, &'static str)> {
    if let Some(plan) = setting.and_then(Plan::parse) {
        return Some((plan, "the settings file"));
    }
    // Claude Code keeps the account's tier in `.claude.json`, next to its
    // config directory. Undocumented, so only a fallback.
    let file = match std::env::var_os("CLAUDE_CONFIG_DIR") {
        Some(dir) if !dir.is_empty() => std::path::PathBuf::from(dir).join(".claude.json"),
        _ => dirs::home_dir()?.join(".claude.json"),
    };
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(file).ok()?).ok()?;
    let account = json.get("oauthAccount")?;
    [
        "organizationRateLimitTier",
        "userRateLimitTier",
        "organizationType",
    ]
    .iter()
    .filter_map(|k| account.get(*k).and_then(|v| v.as_str()))
    .find_map(Plan::parse)
    .map(|plan| (plan, "Claude Code's account info"))
}

/// One plan and how it would have gone.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub plan: Plan,
    /// What it costs a month; for the API, your use at API prices.
    pub monthly: f64,
    /// 5-hour windows it would have stopped you in, when known: a range,
    /// from the most room a window of your plan showed to the least.
    pub stops: Option<(usize, usize)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Capacity {
    pub low: f64,
    pub high: f64,
    pub from: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Fit {
    pub plan: Option<(Plan, &'static str)>,
    /// Days with data, at most `DAYS`.
    pub days: i64,
    /// API-equivalent cost over those days.
    pub cost: f64,
    /// Times a 5-hour or 7-day limit stopped you (distinct windows).
    pub stopped_5h: usize,
    pub stopped_7d: usize,
    /// 5-hour windows you used.
    pub windows: usize,
    /// API-equivalent cost that fits in one 5-hour window of your plan: the
    /// least (where a limit stopped you, or the status line's reading) and the
    /// most (the busiest window that didn't stop you), and where the least
    /// was learned from.
    pub capacity: Option<Capacity>,
    /// Pro, Max 5x, Max 20x and the API.
    pub options: Vec<Choice>,
}

impl Fit {
    /// Your use at API prices for a 30-day month.
    pub fn monthly_cost(&self) -> f64 {
        self.cost * DAYS as f64 / self.days.max(1) as f64
    }

    /// How many times over the plan's price your use is worth.
    pub fn value(&self) -> Option<f64> {
        let price = self.plan?.0.price()?;
        Some(self.monthly_cost() / price)
    }

    /// The cheapest subscription that wouldn't have stopped you.
    pub fn cheapest_unstopped(&self) -> Option<&Choice> {
        self.options
            .iter()
            .filter(|o| o.plan != Plan::Api && o.stops.is_some_and(|(_, most)| most == 0))
            .min_by(|a, b| a.monthly.total_cmp(&b.monthly))
    }
}

/// The 5-hour windows of the requests in `timeline` (epoch seconds, cost),
/// sorted: each starts at the top of the hour of the first request after the
/// previous one ended. Returns each window's start and cost.
fn windows(timeline: &[(i64, f64)]) -> Vec<(i64, f64)> {
    let mut out: Vec<(i64, f64)> = Vec::new();
    for &(t, cost) in timeline {
        match out.last_mut() {
            Some((start, sum)) if t < *start + FIVE_HOURS => *sum += cost,
            _ => out.push((t - t.rem_euclid(HOUR), cost)),
        }
    }
    out
}

fn median(mut values: Vec<f64>) -> Option<f64> {
    values.sort_by(f64::total_cmp);
    values.get(values.len() / 2).copied()
}

pub fn fit(
    sessions: &[Session],
    store: &Store,
    plan: Option<(Plan, &'static str)>,
    now: i64,
) -> Fit {
    let from = now - DAYS * DAY;
    let mut timeline: Vec<(i64, f64)> = sessions
        .iter()
        .flat_map(|s| s.tokens.timeline.iter())
        .filter(|(at, _)| *at >= from && *at <= now)
        .map(|(at, u)| (*at, u.cost))
        .collect();
    timeline.sort_by_key(|(at, _)| *at);
    let cost: f64 = timeline.iter().map(|(_, c)| c).sum();
    let days = timeline
        .first()
        .map_or(0, |(first, _)| {
            ((now - first) as f64 / DAY as f64).ceil() as i64
        })
        .clamp(1, DAYS);
    let costs = windows(&timeline);

    // Distinct windows a limit stopped you in.
    let mut hits_5h = BTreeSet::new();
    let mut hits_7d = BTreeSet::new();
    for hit in sessions.iter().flat_map(|s| &s.tokens.limit_hits) {
        if hit.at < from {
            continue;
        }
        match hit.window.as_str() {
            "five_hour" => hits_5h.insert(hit.resets_at),
            _ => hits_7d.insert(hit.resets_at),
        };
    }
    let spent = |start: i64, end: i64| -> f64 {
        timeline
            .iter()
            .filter(|(at, _)| *at >= start && *at < end)
            .map(|(_, c)| c)
            .sum()
    };

    // How much fits in a window of your plan.
    let from_hits: Vec<f64> = hits_5h
        .iter()
        .map(|&resets| spent(resets - FIVE_HOURS, resets))
        .filter(|&c| c > 0.0)
        .collect();
    let usd = crate::pricing::format_usd;
    let low = match median(from_hits) {
        Some(c) if hits_5h.len() == 1 => Some((
            c,
            format!("The window where the 5-hour limit stopped you held {} at API prices", usd(c)),
        )),
        Some(c) => Some((
            c,
            format!(
                "The {} windows where the 5-hour limit stopped you held a median {} at API prices",
                hits_5h.len(),
                usd(c)
            ),
        )),
        None => crate::quota::five_hour(sessions, store, now)
            .filter(|w| w.reported && w.used.is_some_and(|u| u >= 10.0))
            .and_then(|w| {
                let cost = crate::quota::ledger(sessions, &w).total.cost;
                let used = w.used?;
                (cost > 0.0).then(|| {
                    (
                        cost / (used / 100.0),
                        format!(
                            "The current 5-hour window, {used:.0}% used, puts a full one at {} at API prices",
                            usd(cost / (used / 100.0))
                        ),
                    )
                })
            }),
    };
    // The busiest window no limit stopped you in held at least that much.
    let unstopped = costs
        .iter()
        .filter(|(start, _)| {
            !sessions
                .iter()
                .flat_map(|s| &s.tokens.limit_hits)
                .any(|h| h.at >= *start && h.at < start + FIVE_HOURS)
        })
        .map(|(_, c)| *c)
        .fold(0.0, f64::max);
    let capacity = low.map(|(low, from)| Capacity {
        low,
        high: unstopped.max(low),
        from,
    });

    let current = plan.map(|(p, _)| p).filter(|p| *p != Plan::Api);
    let monthly_cost = cost * DAYS as f64 / days as f64;
    let mut options: Vec<Choice> = Plan::SUBSCRIPTIONS
        .iter()
        .map(|&p| {
            let stops = match (capacity.as_ref(), current) {
                // Your own plan: the times it actually stopped you.
                (_, Some(c)) if c == p => Some((hits_5h.len(), hits_5h.len())),
                (Some(cap), Some(c)) => {
                    let over = |room: f64| {
                        let limit = room * p.multiplier() / c.multiplier();
                        costs.iter().filter(|(_, w)| *w > limit).count()
                    };
                    Some((over(cap.high), over(cap.low)))
                }
                _ => None,
            };
            Choice {
                plan: p,
                monthly: p.price().unwrap_or_default(),
                stops,
            }
        })
        .collect();
    options.push(Choice {
        plan: Plan::Api,
        monthly: monthly_cost,
        stops: Some((0, 0)),
    });
    Fit {
        plan,
        days,
        cost,
        stopped_5h: hits_5h.len(),
        stopped_7d: hits_7d.len(),
        windows: costs.len(),
        capacity,
        options,
    }
}

/// One sentence on whether your plan fits.
fn verdict(fit: &Fit) -> Option<String> {
    fit.capacity.as_ref()?;
    let (current, _) = fit.plan?;
    let price = current.price()?;
    let yours = fit.options.iter().find(|o| o.plan == current)?;
    let stopped = yours.stops.map_or(0, |(n, _)| n);
    let cheaper = fit
        .options
        .iter()
        .filter(|o| o.plan != Plan::Api && o.monthly < price)
        .filter(|o| o.stops.is_some_and(|(_, most)| most == 0))
        .min_by(|a, b| a.monthly.total_cmp(&b.monthly));
    let api = fit.monthly_cost();
    Some(if let Some(c) = cheaper {
        format!(
            "{} would have been enough: {} less a month.",
            c.plan.name(),
            whole(price - c.monthly)
        )
    } else if api < price {
        format!(
            "Paying the API by use would have cost about {} a month, less than the plan.",
            whole(api)
        )
    } else if stopped == 0 {
        format!(
            "{} fits: it never stopped you, and your use is worth {:.1}× its price.",
            current.name(),
            api / price
        )
    } else {
        match fit.cheapest_unstopped() {
            Some(bigger) => format!(
                "{} would likely have avoided {} for {} more a month; worth it only if {} cost you more than that.",
                bigger.plan.name(),
                if stopped == 1 {
                    "that stop".to_string()
                } else {
                    format!("those {stopped} stops")
                },
                whole(bigger.monthly - price),
                if stopped == 1 { "it" } else { "they" }
            ),
            None => {
                "Even Max 20x would likely have stopped you; the API has no 5-hour limit.".into()
            }
        }
    })
}

/// "$100" for whole-dollar amounts like plan prices.
fn whole(x: f64) -> String {
    format!("${:.0}", x.round())
}

/// The verdict as plain lines, for `claudash plan` and Insights.
pub fn report(fit: &Fit) -> Vec<String> {
    let usd = crate::pricing::format_usd;
    let mut lines = Vec::new();
    let period = if fit.days >= DAYS {
        format!("Last {DAYS} days")
    } else {
        format!("Last {} days (all there is)", fit.days)
    };
    match fit.plan {
        Some((Plan::Api, from)) => {
            lines.push(format!("Plan: API, pay as you go (from {from})"));
            lines.push(format!(
                "{period}: {} at API prices, which is what you paid; ≈{} a month",
                usd(fit.cost),
                whole(fit.monthly_cost())
            ));
        }
        Some((plan, from)) => {
            lines.push(format!(
                "Plan: {} ({}/month, from {from})",
                plan.name(),
                whole(plan.price().unwrap_or_default())
            ));
            lines.push(format!(
                "{period}: ≈{} of use at API prices (≈{} a month), {:.1}× what the plan costs",
                usd(fit.cost),
                whole(fit.monthly_cost()),
                fit.value().unwrap_or_default()
            ));
        }
        None => {
            lines.push(
                "Plan: unknown; set plan = \"pro\", \"max5x\", \"max20x\" or \"api\" in claudash's settings file"
                    .into(),
            );
            lines.push(format!(
                "{period}: ≈{} of use at API prices (≈{} a month)",
                usd(fit.cost),
                whole(fit.monthly_cost())
            ));
        }
    }
    let times = |n: usize| {
        if n == 1 {
            "1 time".to_string()
        } else {
            format!("{n} times")
        }
    };
    lines.push(format!(
        "Limits stopped you: 5-hour {}, 7-day {}, over {} windows of 5 hours you used",
        times(fit.stopped_5h),
        times(fit.stopped_7d),
        fit.windows
    ));
    lines.push(String::new());
    lines.push(format!(
        "{:<22}{:>10}   {}",
        "If you had", "a month", "5-hour stops"
    ));
    let current = fit.plan.map(|(p, _)| p);
    for o in &fit.options {
        let name = match o.plan {
            Plan::Api => "API (pay as you go)".to_string(),
            p if Some(p) == current => format!("{} (yours)", p.name()),
            p => p.name().to_string(),
        };
        let stops = match (o.plan, o.stops) {
            (Plan::Api, _) => "never".to_string(),
            (p, Some((n, _))) if Some(p) == current => format!("{n} (actual)"),
            (_, Some((a, b))) if a == b => format!("≈{a}"),
            (_, Some((a, b))) => format!("≈{a}–{b}"),
            (_, None) => "?".to_string(),
        };
        let monthly = if o.plan == Plan::Api {
            format!("≈{}", whole(o.monthly))
        } else {
            whole(o.monthly)
        };
        lines.push(format!("{name:<22}{monthly:>10}   {stops}"));
    }
    lines.push(String::new());
    lines.extend(verdict(fit));
    match &fit.capacity {
        Some(cap) => lines.push(format!(
            "How it's estimated. {}{}. Other plans are scaled by Anthropic's 5x and 20x{}. \
             Anthropic doesn't publish limits in tokens, and 7-day limits aren't modeled.",
            cap.from,
            if cap.high > cap.low * 1.05 {
                format!("; another held {} without stopping you", usd(cap.high))
            } else {
                String::new()
            },
            if cap.high > cap.low * 1.05 {
                ", so their stops are a range: limits don't follow API prices exactly"
            } else {
                ""
            }
        )),
        None if current.is_some_and(|p| p != Plan::Api) => lines.push(
            "Other plans can't be estimated yet: that needs one time a limit stopped you, or the \
             status line connected (claudash setup) during a busy window."
                .into(),
        ),
        None => {}
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::{LimitStop, SessionTokens, Usage};
    use std::{path::PathBuf, time::SystemTime};

    #[test]
    fn reads_plan_names() {
        assert_eq!(Plan::parse("default_claude_max_5x"), Some(Plan::Max5));
        assert_eq!(Plan::parse("default_claude_max_20x"), Some(Plan::Max20));
        assert_eq!(Plan::parse("claude_pro"), Some(Plan::Pro));
        assert_eq!(Plan::parse("Max 20x"), Some(Plan::Max20));
        assert_eq!(Plan::parse("api"), Some(Plan::Api));
        assert_eq!(Plan::parse("enterprise"), None);
    }

    #[test]
    fn calibrates_from_limit_stops_and_compares_plans() {
        let now = 40 * DAY;
        let usage = |cost: f64| Usage {
            cost,
            ..Default::default()
        };
        // Three windows: $40 (stopped there on Max 5x), $10 and $2.
        let w1 = now - 10 * DAY;
        let w2 = now - 5 * DAY;
        let w3 = now - DAY;
        let session = Session {
            id: "s".into(),
            path: PathBuf::new(),
            title: "s".into(),
            project_path: String::new(),
            cwd: None,
            git_branch: None,
            modified: SystemTime::UNIX_EPOCH,
            size: 0,
            tokens: SessionTokens {
                timeline: vec![
                    (w1, usage(30.0)),
                    (w1 + HOUR, usage(10.0)),
                    (w2, usage(10.0)),
                    (w3, usage(2.0)),
                ],
                limit_hits: vec![LimitStop {
                    at: w1 + 2 * HOUR,
                    resets_at: w1 + 3 * HOUR,
                    window: "five_hour".into(),
                }],
                ..Default::default()
            },
        };
        let fit = fit(
            &[session],
            &Store::default(),
            Some((Plan::Max5, "test")),
            now,
        );
        assert_eq!(fit.windows, 3);
        assert_eq!(fit.stopped_5h, 1);
        let cap = fit.capacity.clone().unwrap();
        assert_eq!((cap.low, cap.high), (40.0, 40.0));
        let stops: Vec<Option<(usize, usize)>> = fit.options.iter().map(|o| o.stops).collect();
        // Pro holds $8 a window: stopped in the $40 and $10 ones.
        assert_eq!(
            stops,
            [Some((2, 2)), Some((1, 1)), Some((0, 0)), Some((0, 0))]
        );
        assert_eq!(fit.cheapest_unstopped().map(|o| o.plan), Some(Plan::Max20));
        assert_eq!(fit.days, 10);
        assert!((fit.monthly_cost() - 156.0).abs() < 1e-9);
        assert!((fit.value().unwrap() - 1.56).abs() < 1e-9);
    }
}
