//! `claudash statusline`: a Claude Code status line command that records what
//! Claude Code reports, so the dashboard can show it.
//!
//! Claude Code runs the configured status line command after each assistant
//! message and pipes a documented JSON object to its stdin
//! (<https://code.claude.com/docs/en/statusline>). That JSON carries data that
//! isn't available anywhere else: plan usage (`rate_limits`) and the real
//! context window size. This command saves it to
//! `<cache dir>/claudash/statusline/<session_id>.json` and prints a short status
//! line, or the output of another status line command given after `--`.
//!
//! Plan usage is also appended to `history.jsonl` in the same directory, one
//! sample per change, so the dashboard can tell how fast usage is growing and
//! forecast when a limit will be reached.

use std::{
    fs::{self, OpenOptions},
    io::{self, BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, SystemTime},
};

use serde::{Deserialize, Serialize};

/// Files older than this are deleted when a new one is written.
const MAX_AGE: Duration = Duration::from_secs(30 * 24 * 3600);
/// Plan usage history is trimmed to its newer half past this size.
const HISTORY_MAX_BYTES: u64 = 512 * 1024;

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Snapshot {
    pub session_id: Option<String>,
    #[serde(default)]
    pub model: Model,
    #[serde(default)]
    pub context_window: ContextWindow,
    #[serde(default)]
    pub cost: Cost,
    pub rate_limits: Option<RateLimits>,
    pub prompt_cache: Option<PromptCache>,
}

/// Prompt cache statistics for the main conversation (Claude Code v2.1.251+).
#[derive(Clone, Debug, Default, Deserialize)]
pub struct PromptCache {
    pub ttl: Option<String>,
    /// Epoch seconds when the cached prefix goes cold.
    pub expires_at: Option<i64>,
    /// Cache reads as a fraction of all input tokens, main conversation only.
    pub hit_ratio: Option<f64>,
    pub misses: Option<u64>,
    pub last_miss_cause: Option<MissCause>,
    /// Tokens the next request re-caches if the cache has gone cold by then.
    pub recache_tokens_if_cold: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct MissCause {
    #[serde(default)]
    pub causes: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Model {
    pub display_name: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ContextWindow {
    pub context_window_size: Option<u64>,
    pub used_percentage: Option<f64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Cost {
    pub total_cost_usd: Option<f64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct RateLimits {
    pub five_hour: Option<Window>,
    pub seven_day: Option<Window>,
}

#[derive(Clone, Copy, Debug, Deserialize)]
pub struct Window {
    pub used_percentage: f64,
    /// Unix epoch seconds.
    pub resets_at: i64,
}

impl Window {
    /// A window whose reset time has passed no longer applies.
    pub fn is_current(&self) -> bool {
        self.resets_at > chrono::Utc::now().timestamp()
    }
}

fn store_dir() -> Option<PathBuf> {
    crate::paths::claudash_cache().map(|dir| dir.join("statusline"))
}

/// One plan usage reading, as stored in `history.jsonl`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    /// Epoch seconds when the reading was taken.
    pub at: i64,
    pub five_hour: Option<(f64, i64)>,
    pub seven_day: Option<(f64, i64)>,
}

impl Sample {
    fn from_limits(limits: &RateLimits, at: i64) -> Self {
        let pair = |w: Option<Window>| w.map(|w| (w.used_percentage, w.resets_at));
        Sample {
            at,
            five_hour: pair(limits.five_hour),
            seven_day: pair(limits.seven_day),
        }
    }

    fn same_reading(&self, other: &Sample) -> bool {
        self.five_hour == other.five_hour && self.seven_day == other.seven_day
    }
}

/// Entry point for `claudash statusline [-- <command> [args...]]`.
///
/// Never fails loudly: a broken status line would clutter Claude Code's UI, so
/// errors only reach stderr.
pub fn run(wrapped: &[String]) -> io::Result<()> {
    let mut input = Vec::new();
    io::stdin().read_to_end(&mut input)?;

    let snapshot: Snapshot = serde_json::from_slice(&input).unwrap_or_default();
    if let Err(e) = save(&snapshot, &input) {
        eprintln!("claudash statusline: {e}");
    }

    let mut out = io::stdout().lock();
    match wrapped.split_first() {
        Some((program, args)) => {
            let output = run_wrapped(program, args, &input)?;
            out.write_all(&output)
        }
        None => writeln!(out, "{}", render(&snapshot)),
    }
}

fn save(snapshot: &Snapshot, raw: &[u8]) -> io::Result<()> {
    let (Some(dir), Some(id)) = (store_dir(), snapshot.session_id.as_deref()) else {
        return Ok(());
    };
    // Session IDs are UUIDs; refuse anything that could escape the directory.
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Ok(());
    }
    fs::create_dir_all(&dir)?;
    // Write then rename, so the dashboard never reads a half-written file.
    let tmp = dir.join(format!(".{id}.{}.tmp", std::process::id()));
    fs::write(&tmp, raw)?;
    fs::rename(&tmp, dir.join(format!("{id}.json")))?;
    if let Some(limits) = &snapshot.rate_limits {
        append_sample(
            &dir.join("history.jsonl"),
            Sample::from_limits(limits, now()),
        )?;
    }
    prune(&dir);
    Ok(())
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Appends a reading unless it repeats the last one; trims the file when big.
fn append_sample(file: &Path, sample: Sample) -> io::Result<()> {
    if read_samples(file)
        .last()
        .is_some_and(|last| last.same_reading(&sample))
    {
        return Ok(());
    }
    let mut line = serde_json::to_string(&sample).map_err(io::Error::other)?;
    line.push('\n');
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)?
        .write_all(line.as_bytes())?;
    if fs::metadata(file)?.len() > HISTORY_MAX_BYTES {
        let samples = read_samples(file);
        let keep = &samples[samples.len() / 2..];
        let text: String = keep
            .iter()
            .filter_map(|s| serde_json::to_string(s).ok())
            .map(|l| l + "\n")
            .collect();
        fs::write(file, text)?;
    }
    Ok(())
}

fn read_samples(file: &Path) -> Vec<Sample> {
    let Ok(f) = fs::File::open(file) else {
        return Vec::new();
    };
    BufReader::new(f)
        .lines()
        .map_while(Result::ok)
        .filter_map(|l| serde_json::from_str(&l).ok())
        .collect()
}

/// When a limit will be reached at the current pace.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Forecast {
    /// Reaches 100% at this epoch second, before the window resets.
    LimitAt(i64),
    /// Won't reach 100% before the window resets.
    Safe,
}

/// Selects one window's `(used_percentage, resets_at)` from a sample.
pub type Pick = fn(&Sample) -> Option<(f64, i64)>;

/// Forecasts one window from the readings taken during it. `pick` selects the
/// window from a sample; `lookback` is how far back the pace is measured.
pub fn forecast(samples: &[Sample], pick: Pick, lookback: i64, now: i64) -> Option<Forecast> {
    let (current, resets_at) = samples.last().and_then(pick)?;
    if resets_at <= now {
        return None;
    }
    // Readings from this same window (same reset time) inside the lookback.
    let window: Vec<(i64, f64)> = samples
        .iter()
        .filter_map(|s| {
            pick(s)
                .filter(|(_, r)| *r == resets_at)
                .map(|(p, _)| (s.at, p))
        })
        .filter(|(at, _)| now - at <= lookback)
        .collect();
    let (first_at, first_pct) = *window.first()?;
    let (last_at, _) = *window.last()?;
    let elapsed = (last_at - first_at) as f64;
    // Too little data to call a pace: less than 5 minutes of readings.
    if elapsed < 300.0 {
        return None;
    }
    let per_second = (current - first_pct) / elapsed;
    if per_second <= 0.0 {
        return Some(Forecast::Safe);
    }
    let at = now + ((100.0 - current).max(0.0) / per_second) as i64;
    Some(if at < resets_at {
        Forecast::LimitAt(at)
    } else {
        Forecast::Safe
    })
}

fn prune(dir: &PathBuf) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age > MAX_AGE);
        if old {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn run_wrapped(program: &str, args: &[String], input: &[u8]) -> io::Result<Vec<u8>> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        // The command may exit without reading everything; that's fine.
        let _ = stdin.write_all(input);
    }
    Ok(child.wait_with_output()?.stdout)
}

/// The default one-line status: model, context and plan usage.
fn render(s: &Snapshot) -> String {
    let mut parts = Vec::new();
    if let Some(name) = &s.model.display_name {
        parts.push(name.clone());
    }
    if let Some(pct) = s.context_window.used_percentage {
        parts.push(format!("ctx {pct:.0}%"));
    }
    if let Some(limits) = &s.rate_limits {
        for (label, window) in [("5h", limits.five_hour), ("7d", limits.seven_day)] {
            if let Some(w) = window.filter(Window::is_current) {
                parts.push(format!("{label} {:.0}%", w.used_percentage));
            }
        }
    }
    if let Some(cost) = s.cost.total_cost_usd {
        parts.push(format!("${cost:.2}"));
    }
    parts.join(" · ")
}

/// What the dashboard knows from the status line.
#[derive(Default)]
pub struct Store {
    /// Latest snapshot per session ID.
    pub sessions: std::collections::HashMap<String, Snapshot>,
    /// Plan usage from the most recently written snapshot that has it.
    pub rate_limits: Option<(RateLimits, SystemTime)>,
    /// `true` once any snapshot has been found, i.e. the status line is set up.
    pub configured: bool,
    /// Plan usage readings, oldest first.
    pub samples: Vec<Sample>,
}

/// Reads every saved snapshot. Cheap: one small JSON file per recent session.
pub fn load() -> Store {
    let mut store = Store::default();
    let Some(dir) = store_dir() else {
        return store;
    };
    store.samples = read_samples(&dir.join("history.jsonl"));
    let Ok(entries) = fs::read_dir(dir) else {
        return store;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let Ok(snapshot) = fs::read(&path)
            .and_then(|raw| serde_json::from_slice::<Snapshot>(&raw).map_err(io::Error::other))
        else {
            continue;
        };
        store.configured = true;
        let modified = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        if let Some(limits) = &snapshot.rate_limits
            && store
                .rate_limits
                .as_ref()
                .is_none_or(|(_, t)| modified > *t)
        {
            store.rate_limits = Some((limits.clone(), modified));
        }
        if let Some(id) = snapshot.session_id.clone() {
            store.sessions.insert(id, snapshot);
        }
    }
    store
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "session_id": "abc-123",
        "model": {"id": "claude-opus-5-5", "display_name": "Opus"},
        "context_window": {"context_window_size": 1000000, "used_percentage": 23.4},
        "cost": {"total_cost_usd": 1.5},
        "rate_limits": {
            "five_hour": {"used_percentage": 41.2, "resets_at": 4102444800},
            "seven_day": {"used_percentage": 12.0, "resets_at": 1000}
        }
    }"#;

    #[test]
    fn parses_the_documented_fields_and_renders_a_line() {
        let s: Snapshot = serde_json::from_str(SAMPLE).unwrap();
        assert_eq!(s.context_window.context_window_size, Some(1_000_000));
        let limits = s.rate_limits.clone().unwrap();
        assert!(limits.five_hour.unwrap().is_current());
        // resets_at in the past: the window no longer applies and isn't shown.
        assert!(!limits.seven_day.unwrap().is_current());
        assert_eq!(render(&s), "Opus · ctx 23% · 5h 41% · $1.50");
    }

    #[test]
    fn tolerates_missing_fields() {
        let s: Snapshot = serde_json::from_str(r#"{"session_id":"x"}"#).unwrap();
        assert!(s.rate_limits.is_none());
        assert_eq!(render(&s), "");
    }

    fn sample(at: i64, five: f64) -> Sample {
        Sample {
            at,
            five_hour: Some((five, 20_000)),
            seven_day: None,
        }
    }

    #[test]
    fn forecasts_the_limit_from_the_pace_in_the_current_window() {
        let five = |s: &Sample| s.five_hour;
        // 10% -> 40% in 1500 s: 60% left at 0.02%/s takes 3000 s.
        let samples = [sample(1_000, 10.0), sample(2_500, 40.0)];
        assert_eq!(
            forecast(&samples, five, 3_600, 2_500),
            Some(Forecast::LimitAt(5_500))
        );
        // Same pace, but the window resets first.
        let early = [
            Sample {
                five_hour: Some((10.0, 4_000)),
                ..samples[0]
            },
            Sample {
                five_hour: Some((40.0, 4_000)),
                ..samples[1]
            },
        ];
        assert_eq!(forecast(&early, five, 3_600, 2_500), Some(Forecast::Safe));
        // Not enough data yet.
        assert_eq!(forecast(&samples[..1], five, 3_600, 2_500), None);
        // Readings from an earlier window (different reset) are ignored.
        let mixed = [
            Sample {
                five_hour: Some((90.0, 900)),
                ..sample(100, 0.0)
            },
            sample(2_400, 40.0),
            sample(2_500, 40.0),
        ];
        assert_eq!(forecast(&mixed, five, 3_600, 2_500), None);
    }

    #[test]
    fn history_skips_repeated_readings() {
        let file =
            std::env::temp_dir().join(format!("claudash-history-{}.jsonl", std::process::id()));
        let _ = fs::remove_file(&file);
        append_sample(&file, sample(1, 10.0)).unwrap();
        append_sample(&file, sample(2, 10.0)).unwrap();
        append_sample(&file, sample(3, 11.0)).unwrap();
        let samples = read_samples(&file);
        fs::remove_file(&file).unwrap();
        assert_eq!(samples.iter().map(|s| s.at).collect::<Vec<_>>(), [1, 3]);
    }

    #[test]
    fn parses_prompt_cache_fields() {
        let s: Snapshot = serde_json::from_str(
            r#"{"prompt_cache":{"ttl":"1h","expires_at":100,"hit_ratio":0.93,
                "misses":2,"last_miss_cause":{"causes":["ttl_expired_5m"]},"recache_tokens_if_cold":5000}}"#,
        )
        .unwrap();
        let cache = s.prompt_cache.unwrap();
        assert_eq!(cache.hit_ratio, Some(0.93));
        assert_eq!(cache.last_miss_cause.unwrap().causes, ["ttl_expired_5m"]);
    }
}
