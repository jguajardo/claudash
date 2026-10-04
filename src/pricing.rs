//! What each request would cost at Claude API prices, from the usage Claude
//! Code records with every response: input, 5-minute and 1-hour cache writes,
//! cache reads, output, fast mode, US-only inference and web searches.
//!
//! On a Pro or Max plan you don't pay this; it's the API-equivalent value of
//! what you used. Prices: <https://platform.claude.com/docs/en/about-claude/pricing>.

use serde::Deserialize;

use crate::sessions::Usage;

/// USD per million tokens: input, 5-minute cache write, 1-hour cache write,
/// cache read, output.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Price {
    pub input: f64,
    pub write_5m: f64,
    pub write_1h: f64,
    pub read: f64,
    pub output: f64,
}

const fn price(input: f64, write_5m: f64, write_1h: f64, read: f64, output: f64) -> Price {
    Price {
        input,
        write_5m,
        write_1h,
        read,
        output,
    }
}

/// Model ID prefixes, most specific first, as of 2026-10-04.
const PRICES: &[(&str, Price)] = &[
    ("claude-fable-5-1", price(10.0, 12.5, 20.0, 0.25, 50.0)),
    ("claude-mythos-5-1", price(10.0, 12.5, 20.0, 0.25, 50.0)),
    ("claude-fable-5", price(10.0, 12.5, 20.0, 1.0, 50.0)),
    ("claude-mythos-5", price(10.0, 12.5, 20.0, 1.0, 50.0)),
    ("claude-opus-5-5", price(4.0, 5.0, 8.0, 0.20, 20.0)),
    ("claude-opus-5", price(5.0, 6.25, 10.0, 0.5, 25.0)),
    ("claude-opus-4-8", price(5.0, 6.25, 10.0, 0.5, 25.0)),
    ("claude-opus-4-7", price(5.0, 6.25, 10.0, 0.5, 25.0)),
    ("claude-opus-4-6", price(5.0, 6.25, 10.0, 0.5, 25.0)),
    ("claude-opus-4-5", price(5.0, 6.25, 10.0, 0.5, 25.0)),
    ("claude-opus-4-1", price(15.0, 18.75, 30.0, 1.5, 75.0)),
    ("claude-opus-4", price(15.0, 18.75, 30.0, 1.5, 75.0)),
    ("claude-sonnet-5-5", price(2.0, 2.5, 4.0, 0.2, 10.0)),
    ("claude-sonnet-5", price(2.0, 2.5, 4.0, 0.2, 10.0)),
    ("claude-sonnet-4-6", price(3.0, 3.75, 6.0, 0.3, 15.0)),
    ("claude-sonnet-4-5", price(3.0, 3.75, 6.0, 0.3, 15.0)),
    ("claude-sonnet-4", price(3.0, 3.75, 6.0, 0.3, 15.0)),
    ("claude-haiku-4-5", price(1.0, 1.25, 2.0, 0.1, 5.0)),
    ("claude-3-5-haiku", price(0.8, 1.0, 1.6, 0.08, 4.0)),
    ("claude-haiku-3-5", price(0.8, 1.0, 1.6, 0.08, 4.0)),
];

/// Fast mode input and output prices; cache prices scale with input.
const FAST: &[(&str, f64, f64)] = &[
    ("claude-opus-5-5", 8.0, 40.0),
    ("claude-opus-5", 10.0, 50.0),
    ("claude-opus-4-8", 10.0, 50.0),
];

/// USD per web search.
const WEB_SEARCH: f64 = 0.01;
/// US-only inference (`inference_geo: "us"`).
const US_ONLY: f64 = 1.1;

/// The price of a model ID such as `claude-opus-5-5` or
/// `claude-haiku-4-5-20251001`.
pub fn lookup(model: &str) -> Option<Price> {
    PRICES
        .iter()
        .find(|(prefix, _)| {
            model == *prefix
                || model
                    .strip_prefix(prefix)
                    .is_some_and(|rest| rest.starts_with('-') && !next_is_version(rest))
        })
        .map(|(_, p)| *p)
}

/// `-5` after `claude-opus` is another version, not a date suffix.
fn next_is_version(rest: &str) -> bool {
    let part = rest.trim_start_matches('-').split('-').next().unwrap_or("");
    !part.is_empty() && part.len() <= 2 && part.chars().all(|c| c.is_ascii_digit())
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
struct CacheCreation {
    #[serde(default)]
    ephemeral_5m_input_tokens: u64,
    #[serde(default)]
    ephemeral_1h_input_tokens: u64,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
struct ServerTools {
    #[serde(default)]
    web_search_requests: u64,
}

/// A response's `message.usage`, with what pricing needs.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct RawUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    cache_creation: Option<CacheCreation>,
    server_tool_use: Option<ServerTools>,
    speed: Option<String>,
    inference_geo: Option<String>,
}

impl RawUsage {
    /// Token counts and their API-equivalent cost for `model`.
    pub fn priced(&self, model: &str) -> Usage {
        let mut usage = Usage {
            input_tokens: self.input_tokens,
            cache_creation_input_tokens: self.cache_creation_input_tokens,
            cache_read_input_tokens: self.cache_read_input_tokens,
            output_tokens: self.output_tokens,
            cost: 0.0,
        };
        let Some(mut p) = lookup(model) else {
            return usage;
        };
        if self.speed.as_deref() == Some("fast")
            && let Some((_, input, output)) = FAST.iter().find(|(m, _, _)| model.starts_with(m))
        {
            let scale = input / p.input;
            p = Price {
                input: *input,
                write_5m: p.write_5m * scale,
                write_1h: p.write_1h * scale,
                read: p.read * scale,
                output: *output,
            };
        }
        // Older transcripts don't split cache writes; Claude Code's default is 5 minutes.
        let (write_5m, write_1h) = match self.cache_creation {
            Some(c) if c.ephemeral_5m_input_tokens + c.ephemeral_1h_input_tokens > 0 => {
                (c.ephemeral_5m_input_tokens, c.ephemeral_1h_input_tokens)
            }
            _ => (self.cache_creation_input_tokens, 0),
        };
        let mut cost = (self.input_tokens as f64 * p.input
            + write_5m as f64 * p.write_5m
            + write_1h as f64 * p.write_1h
            + self.cache_read_input_tokens as f64 * p.read
            + self.output_tokens as f64 * p.output)
            / 1e6;
        if self.inference_geo.as_deref() == Some("us") {
            cost *= US_ONLY;
        }
        cost += self.server_tool_use.map_or(0, |s| s.web_search_requests) as f64 * WEB_SEARCH;
        usage.cost = cost;
        usage
    }
}

/// "$0.42", "$12.40", "$1,284".
pub fn format_usd(usd: f64) -> String {
    if usd >= 1000.0 {
        let whole = usd.round() as u64;
        let s = whole.to_string();
        let mut out = String::new();
        for (i, c) in s.chars().enumerate() {
            if i > 0 && (s.len() - i).is_multiple_of(3) {
                out.push(',');
            }
            out.push(c);
        }
        format!("${out}")
    } else {
        format!("${usd:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(json: &str) -> RawUsage {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn finds_prices_by_model_id() {
        assert_eq!(lookup("claude-opus-5-5").unwrap().output, 20.0);
        assert_eq!(lookup("claude-opus-5").unwrap().output, 25.0);
        assert_eq!(lookup("claude-haiku-4-5-20251001").unwrap().input, 1.0);
        assert_eq!(lookup("claude-opus-4-1-20250805").unwrap().input, 15.0);
        // "claude-opus-4" must not match Opus 4.8.
        assert_eq!(lookup("claude-opus-4-8").unwrap().input, 5.0);
        assert!(lookup("gpt-5").is_none());
        assert!(lookup("claude-opus-9").is_none());
    }

    #[test]
    fn prices_cache_writes_by_duration_and_fast_mode() {
        let one_hour = raw(
            r#"{"input_tokens":1000000,"cache_creation_input_tokens":1000000,"cache_read_input_tokens":1000000,"output_tokens":1000000,"cache_creation":{"ephemeral_5m_input_tokens":0,"ephemeral_1h_input_tokens":1000000}}"#,
        );
        // Opus 5.5: 4 input + 8 (1h write) + 0.2 read + 20 output.
        assert!((one_hour.priced("claude-opus-5-5").cost - 32.2).abs() < 1e-9);
        // Without the split, writes count as 5-minute ones: 4 + 5 + 0.2 + 20.
        let unsplit = raw(
            r#"{"input_tokens":1000000,"cache_creation_input_tokens":1000000,"cache_read_input_tokens":1000000,"output_tokens":1000000}"#,
        );
        assert!((unsplit.priced("claude-opus-5-5").cost - 29.2).abs() < 1e-9);
        let fast = raw(r#"{"input_tokens":1000000,"output_tokens":1000000,"speed":"fast"}"#);
        assert!((fast.priced("claude-opus-5-5").cost - 48.0).abs() < 1e-9);
        let searches = raw(r#"{"server_tool_use":{"web_search_requests":3}}"#);
        assert!((searches.priced("claude-sonnet-5").cost - 0.03).abs() < 1e-9);
        let us = raw(r#"{"output_tokens":1000000,"inference_geo":"us"}"#);
        assert!((us.priced("claude-sonnet-5").cost - 11.0).abs() < 1e-9);
        assert_eq!(raw(r#"{"output_tokens":5}"#).priced("unknown").cost, 0.0);
    }

    #[test]
    fn formats_dollars() {
        assert_eq!(format_usd(0.4242), "$0.42");
        assert_eq!(format_usd(12.4), "$12.40");
        assert_eq!(format_usd(1284.4), "$1,284");
        assert_eq!(format_usd(1_234_567.0), "$1,234,567");
    }
}
