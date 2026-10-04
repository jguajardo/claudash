//! Claude Code's permission rules from every settings file that applies to a
//! project, merged into one list, with the ones worth a second look flagged:
//! rules so broad they skip every prompt, rules that contradict each other,
//! and a default mode that never asks.

use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::audit::Severity;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Allow,
    Ask,
    Deny,
    /// `permissions.defaultMode`.
    Mode,
    /// A rule claudash suggests adding: a safe command you run often that no
    /// rule allows, so Claude Code asks every time.
    Suggest,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Allow => "allow",
            Kind::Ask => "ask",
            Kind::Deny => "deny",
            Kind::Mode => "mode",
            Kind::Suggest => "add?",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub kind: Kind,
    pub rule: String,
    /// "managed", "user", "project" or "local".
    pub scope: &'static str,
    pub file: PathBuf,
    pub flag: Option<(Severity, String)>,
}

/// Commands that, allowed for any arguments, let Claude do almost anything
/// without asking: run code, reach the network, delete or publish.
const POWERFUL: [&str; 22] = [
    "curl",
    "wget",
    "rm",
    "sudo",
    "ssh",
    "scp",
    "git push",
    "npm publish",
    "cargo publish",
    "python",
    "python3",
    "node",
    "bash",
    "sh",
    "zsh",
    "eval",
    "docker",
    "kubectl",
    "aws",
    "gcloud",
    "terraform",
    "chmod",
];

/// What deserves a second look about an allow rule, if anything.
fn judge_allow(rule: &str) -> Option<(Severity, String)> {
    let rule = rule.trim();
    if matches!(rule, "Bash" | "Bash(*)" | "Bash(*:*)" | "Bash(:*)") {
        return Some((
            Severity::High,
            "runs any shell command without asking".into(),
        ));
    }
    if let Some(inner) = rule.strip_prefix("Bash(").and_then(|r| r.strip_suffix(')')) {
        let prefix = inner
            .trim_end_matches(":*")
            .trim_end_matches(" *")
            .trim_end_matches('*')
            .trim();
        if POWERFUL.contains(&prefix) {
            return Some((
                Severity::Medium,
                format!("runs any `{prefix}` command without asking"),
            ));
        }
    }
    if rule == "WebFetch" {
        return Some((Severity::Low, "fetches any website without asking".into()));
    }
    None
}

fn read_scope(file: &Path, scope: &'static str, out: &mut Vec<Rule>) {
    let Ok(text) = fs::read_to_string(file) else {
        return;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    let permissions = &json["permissions"];
    for (kind, key) in [
        (Kind::Allow, "allow"),
        (Kind::Ask, "ask"),
        (Kind::Deny, "deny"),
    ] {
        for rule in permissions[key].as_array().into_iter().flatten() {
            let Some(rule) = rule.as_str() else {
                continue;
            };
            out.push(Rule {
                kind,
                rule: rule.to_string(),
                scope,
                file: file.to_path_buf(),
                flag: (kind == Kind::Allow).then(|| judge_allow(rule)).flatten(),
            });
        }
    }
    if let Some(mode) = permissions["defaultMode"].as_str() {
        let flag = match mode {
            "bypassPermissions" => Some((Severity::High, "never asks before anything".into())),
            "acceptEdits" => Some((Severity::Low, "edits files without asking".into())),
            _ => None,
        };
        out.push(Rule {
            kind: Kind::Mode,
            rule: mode.to_string(),
            scope,
            file: file.to_path_buf(),
            flag,
        });
    }
}

/// Every rule that applies in `project` (user rules alone without one).
pub fn load(project: Option<&Path>) -> Vec<Rule> {
    let mut rules = Vec::new();
    // Tests only read the project's own files.
    if !cfg!(test) {
        read_scope(
            &crate::paths::managed_dir().join("managed-settings.json"),
            "managed",
            &mut rules,
        );
        if let Some(home) = crate::paths::claude_home() {
            read_scope(&home.join("settings.json"), "user", &mut rules);
            read_scope(&home.join("settings.local.json"), "user local", &mut rules);
        }
    }
    if let Some(dir) = project.map(|p| p.join(".claude")) {
        read_scope(&dir.join("settings.json"), "project", &mut rules);
        read_scope(&dir.join("settings.local.json"), "local", &mut rules);
    }
    flag_conflicts(&mut rules);
    // Flagged first, worst first.
    rules.sort_by_key(|r| (r.flag.as_ref().map_or(3, |(s, _)| *s as u8), r.kind as u8));
    rules
}

/// Allow rules also denied (deny wins, so the allow does nothing) and rules
/// repeated in several files.
fn flag_conflicts(rules: &mut [Rule]) {
    let denied: Vec<String> = rules
        .iter()
        .filter(|r| r.kind == Kind::Deny)
        .map(|r| r.rule.clone())
        .collect();
    let mut seen: Vec<(Kind, String)> = Vec::new();
    for r in rules.iter_mut() {
        if r.kind == Kind::Allow && denied.contains(&r.rule) {
            r.flag = Some((
                Severity::Low,
                "also denied; deny wins, so this allow does nothing".into(),
            ));
        } else if r.kind != Kind::Mode && seen.contains(&(r.kind, r.rule.clone())) {
            r.flag
                .get_or_insert((Severity::Low, "repeated from another settings file".into()));
        }
        seen.push((r.kind, r.rule.clone()));
    }
}

/// Build, test and git-reading commands: the ones Claude Code usually asks
/// about and that are worth allowing. Plain shell utilities (ls, cat…) are
/// left out because Claude Code may already treat them as read-only, and
/// anything that deletes, publishes, reaches the network or runs arbitrary
/// code stays out whatever its count.
const SAFE_COMMANDS: [&str; 20] = [
    "cargo test",
    "cargo build",
    "cargo check",
    "cargo clippy",
    "cargo fmt",
    "npm test",
    "npm run",
    "pnpm test",
    "pnpm run",
    "yarn test",
    "go test",
    "go build",
    "go vet",
    "pytest",
    "make",
    "git status",
    "git diff",
    "git log",
    "git show",
    "git branch",
];

/// Whether an allow rule already lets `command` (a command key such as
/// `cargo test`) run without asking.
fn allowed(rules: &[Rule], command: &str) -> bool {
    rules.iter().filter(|r| r.kind == Kind::Allow).any(|r| {
        let rule = r.rule.trim();
        if rule == "Bash" {
            return true;
        }
        rule.strip_prefix("Bash(")
            .and_then(|i| i.strip_suffix(')'))
            .map(|inner| inner.trim_end_matches(":*").trim_end_matches('*').trim())
            .is_some_and(|prefix| {
                !prefix.is_empty()
                    && (command == prefix
                        || command.starts_with(&format!("{prefix} "))
                        || prefix.starts_with(&format!("{command} ")))
            })
    })
}

/// Rules worth adding: safe commands run at least `min_uses` times that no
/// allow rule covers, most used first. `uses` is (command key, times run).
pub fn suggestions(rules: &[Rule], uses: &[(String, u32)], min_uses: u32) -> Vec<Rule> {
    let defaults = rules.iter().any(|r| {
        r.kind == Kind::Mode && matches!(r.rule.as_str(), "bypassPermissions" | "dontAsk")
    });
    if defaults {
        return Vec::new();
    }
    let mut out: Vec<(u32, Rule)> = uses
        .iter()
        .filter(|(command, n)| {
            *n >= min_uses && SAFE_COMMANDS.contains(&command.as_str()) && !allowed(rules, command)
        })
        .map(|(command, n)| {
            (
                *n,
                Rule {
                    kind: Kind::Suggest,
                    rule: format!("Bash({command}:*)"),
                    scope: "suggested",
                    file: std::path::PathBuf::new(),
                    flag: Some((
                        Severity::Low,
                        format!(
                            "run {n} times in 30 days and no rule allows it, so each run asked"
                        ),
                    )),
                },
            )
        })
        .collect();
    out.sort_by_key(|(n, _)| std::cmp::Reverse(*n));
    out.into_iter().map(|(_, r)| r).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_broad_rules_conflicts_and_modes() {
        let dir = std::env::temp_dir().join(format!("claudash-perm-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".claude")).unwrap();
        fs::write(
            dir.join(".claude/settings.json"),
            r#"{"permissions":{"allow":["Bash(curl:*)","Bash(cargo test:*)","Read"],"deny":["Read"],"defaultMode":"bypassPermissions"}}"#,
        )
        .unwrap();
        fs::write(
            dir.join(".claude/settings.local.json"),
            r#"{"permissions":{"allow":["Bash","Bash(cargo test:*)"]}}"#,
        )
        .unwrap();
        let rules = load(Some(&dir));
        fs::remove_dir_all(&dir).unwrap();

        let flag = |rule: &str, scope: &str| {
            rules
                .iter()
                .find(|r| r.rule == rule && r.scope == scope)
                .and_then(|r| r.flag.clone())
                .map(|(s, _)| s)
        };
        assert_eq!(flag("Bash", "local"), Some(Severity::High));
        assert_eq!(flag("bypassPermissions", "project"), Some(Severity::High));
        assert_eq!(flag("Bash(curl:*)", "project"), Some(Severity::Medium));
        assert_eq!(flag("Bash(cargo test:*)", "project"), None);
        assert_eq!(flag("Bash(cargo test:*)", "local"), Some(Severity::Low));
        assert_eq!(flag("Read", "project"), Some(Severity::Low));
        // High first.
        assert_eq!(rules[0].flag.as_ref().unwrap().0, Severity::High);
    }

    #[test]
    fn suggests_rules_only_for_safe_commands_nothing_allows() {
        let rule = |rule: &str| Rule {
            kind: Kind::Allow,
            rule: rule.into(),
            scope: "user",
            file: PathBuf::new(),
            flag: None,
        };
        let uses = [
            ("cargo test".to_string(), 84),
            ("git diff".to_string(), 40),
            ("rm".to_string(), 90),
            ("ls".to_string(), 30),
            ("cargo build".to_string(), 12),
        ];
        let rules = [rule("Bash(git diff:*)")];
        let got: Vec<String> = suggestions(&rules, &uses, 10)
            .into_iter()
            .map(|r| r.rule)
            .collect();
        // git diff is allowed, rm is never safe, ls is left to Claude Code.
        assert_eq!(got, ["Bash(cargo test:*)", "Bash(cargo build:*)"]);
        assert!(suggestions(&[rule("Bash")], &uses, 10).is_empty());
    }
}
