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
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Allow => "allow",
            Kind::Ask => "ask",
            Kind::Deny => "deny",
            Kind::Mode => "mode",
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
}
