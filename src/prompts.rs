//! Your prompt history across projects, from Claude Code's `history.jsonl`
//! (the file behind the up-arrow in the prompt box). Internal format: lines that
//! don't parse are skipped.

use std::{
    fs,
    io::{self, BufRead, BufReader, Write},
};

use chrono::{DateTime, Local, TimeZone};
use serde::Deserialize;

#[derive(Clone, Debug, PartialEq)]
pub struct Prompt {
    pub text: String,
    pub project: String,
    pub session_id: String,
    pub at: Option<DateTime<Local>>,
}

#[derive(Deserialize)]
struct Line {
    display: Option<String>,
    project: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    /// Epoch milliseconds.
    timestamp: Option<i64>,
}

/// Every prompt, newest first. Pasted content is left out.
pub fn load() -> Vec<Prompt> {
    let Some(file) = crate::paths::claude_home().map(|h| h.join("history.jsonl")) else {
        return Vec::new();
    };
    let Ok(file) = fs::File::open(file) else {
        return Vec::new();
    };
    let mut prompts: Vec<Prompt> = BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|l| serde_json::from_str::<Line>(&l).ok())
        .filter_map(|l| {
            let text = l.display?.trim().to_string();
            (!text.is_empty()).then(|| Prompt {
                text,
                project: l
                    .project
                    .map(|p| crate::paths::display(std::path::Path::new(&p)))
                    .unwrap_or_default(),
                session_id: l.session_id.unwrap_or_default(),
                at: l
                    .timestamp
                    .and_then(|t| Local.timestamp_millis_opt(t).single()),
            })
        })
        .collect();
    prompts.reverse();
    prompts
}

/// Indices of prompts matching `query` (case-insensitive, text or project).
pub fn search(prompts: &[Prompt], query: &str) -> Vec<usize> {
    let q = query.to_lowercase();
    prompts
        .iter()
        .enumerate()
        .filter(|(_, p)| {
            q.is_empty()
                || p.text.to_lowercase().contains(&q)
                || p.project.to_lowercase().contains(&q)
        })
        .map(|(i, _)| i)
        .collect()
}

/// Copies text to the clipboard with the OSC 52 terminal sequence, which most
/// modern terminals support, over SSH too.
pub fn copy_to_clipboard(text: &str) -> io::Result<()> {
    let mut out = io::stdout();
    write!(out, "\x1b]52;c;{}\x07", base64(text.as_bytes()))?;
    out.flush()
}

fn base64(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_base64() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64("héllo".as_bytes()), "aMOpbGxv");
    }

    #[test]
    fn searches_text_and_project() {
        let p = |text: &str, project: &str| Prompt {
            text: text.into(),
            project: project.into(),
            session_id: String::new(),
            at: None,
        };
        let prompts = [p("Fix the login test", "~/api"), p("write docs", "~/web")];
        assert_eq!(search(&prompts, "LOGIN"), [0]);
        assert_eq!(search(&prompts, "web"), [1]);
        assert_eq!(search(&prompts, ""), [0, 1]);
    }
}
