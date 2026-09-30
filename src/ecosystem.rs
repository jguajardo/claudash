//! What Claude Code has available: skills, subagents, slash commands, hooks and
//! plugins, at user, project and plugin scope.
//!
//! File locations and frontmatter follow the docs:
//! <https://code.claude.com/docs/en/skills>, <https://code.claude.com/docs/en/sub-agents>,
//! <https://code.claude.com/docs/en/hooks> and
//! <https://code.claude.com/docs/en/plugins/manifest-reference>.
//! Plugins come from `claude plugin list --json`; each plugin's components are
//! read from its install path using the standard layout (`skills/`, `agents/`,
//! `commands/`, `hooks/hooks.json`). Plugins that declare custom component
//! paths in their manifest may show fewer components than they have.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

#[derive(Clone, Debug, Default)]
pub struct Item {
    pub name: String,
    pub description: String,
    /// "user", "project", "claude.ai" or the plugin name.
    pub source: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Plugin {
    pub id: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(rename = "installPath")]
    pub install_path: Option<PathBuf>,
    #[serde(rename = "mcpServers", default)]
    pub mcp_servers: serde_json::Map<String, serde_json::Value>,
    #[serde(skip)]
    pub description: String,
}

impl Plugin {
    /// `superpowers@claude-plugins-official` -> `superpowers`.
    pub fn short_name(&self) -> &str {
        self.id.split('@').next().unwrap_or(&self.id)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Ecosystem {
    pub skills: Vec<Item>,
    pub agents: Vec<Item>,
    pub commands: Vec<Item>,
    pub hooks: Vec<Item>,
    pub plugins: Vec<Plugin>,
    /// Why the plugin list couldn't be read, if it couldn't.
    pub plugin_error: Option<String>,
}

/// Scans everything available for a session running in `project`. Runs
/// `claude plugin list --json`, so call it off the UI thread.
pub fn load(project: Option<&Path>) -> Ecosystem {
    let mut eco = Ecosystem::default();
    let home = crate::paths::claude_home();

    let (plugins, plugin_error) = match list_plugins() {
        Ok(plugins) => (plugins, None),
        Err(e) => (Vec::new(), Some(e)),
    };
    eco.plugin_error = plugin_error;

    let mut roots: Vec<(PathBuf, String)> = Vec::new();
    if let Some(home) = &home {
        roots.push((home.clone(), "user".into()));
    }
    if let Some(project) = project {
        roots.push((project.join(".claude"), "project".into()));
    }

    for (root, source) in &roots {
        eco.skills.extend(scan_skills(&root.join("skills"), source));
        eco.agents
            .extend(scan_markdown(&root.join("agents"), source));
        eco.commands
            .extend(scan_markdown(&root.join("commands"), source));
    }
    // Hooks live in settings files; project ones in both shared and local settings.
    if let Some(home) = &home {
        eco.hooks
            .extend(scan_hooks(&home.join("settings.json"), "user"));
    }
    if let Some(project) = project {
        let dir = project.join(".claude");
        eco.hooks
            .extend(scan_hooks(&dir.join("settings.json"), "project"));
        eco.hooks
            .extend(scan_hooks(&dir.join("settings.local.json"), "local"));
    }

    for plugin in plugins.iter().filter(|p| p.enabled) {
        let Some(dir) = &plugin.install_path else {
            continue;
        };
        let source = plugin.short_name().to_string();
        eco.skills.extend(scan_skills(&dir.join("skills"), &source));
        eco.agents
            .extend(scan_markdown(&dir.join("agents"), &source));
        eco.commands
            .extend(scan_markdown(&dir.join("commands"), &source));
        eco.hooks
            .extend(scan_hooks(&dir.join("hooks").join("hooks.json"), &source));
    }
    eco.plugins = plugins;

    for list in [&mut eco.skills, &mut eco.agents, &mut eco.commands] {
        list.sort_by_key(|a| a.name.to_lowercase());
    }
    eco
}

fn list_plugins() -> Result<Vec<Plugin>, String> {
    let output = crate::claude_cli::command()
        .args(["plugin", "list", "--json"])
        .output()
        .map_err(|e| format!("Could not run `claude plugin list`: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "`claude plugin list` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let mut plugins: Vec<Plugin> = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("Unexpected `claude plugin list --json` output: {e}"))?;
    for plugin in &mut plugins {
        plugin.description = plugin
            .install_path
            .as_ref()
            .and_then(|dir| fs::read_to_string(dir.join(".claude-plugin/plugin.json")).ok())
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
            .and_then(|json| json["description"].as_str().map(str::to_owned))
            .unwrap_or_default();
    }
    plugins.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(plugins)
}

/// Skills are `<dir>/<name>/SKILL.md`, one level deep: some plugins ship extra
/// `SKILL.md` copies in subfolders (`upstream/`, `v1/`) that aren't skills of
/// their own. Skills synced from claude.ai sit at `skills/synced/<org>/<name>/`.
fn scan_skills(dir: &Path, source: &str) -> Vec<Item> {
    let mut items = Vec::new();
    for child in sorted_dirs(dir) {
        if child.file_name().is_some_and(|n| n == "synced") {
            for org in sorted_dirs(&child) {
                items.extend(
                    sorted_dirs(&org)
                        .iter()
                        .filter_map(|d| skill(d, "claude.ai")),
                );
            }
        } else {
            items.extend(skill(&child, source));
        }
    }
    items
}

fn skill(dir: &Path, source: &str) -> Option<Item> {
    let path = dir.join("SKILL.md");
    if !path.is_file() {
        return None;
    }
    let meta = frontmatter(&path);
    let fallback = dir.file_name()?.to_string_lossy().into_owned();
    Some(Item {
        name: meta.name.unwrap_or(fallback),
        description: meta.description.unwrap_or_default(),
        source: source.into(),
        path,
    })
}

fn sorted_dirs(dir: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs
}

/// Subagents and commands are Markdown files, possibly in subdirectories.
fn scan_markdown(dir: &Path, source: &str) -> Vec<Item> {
    find_files(dir, 3, &|p| p.extension().is_some_and(|e| e == "md"))
        .into_iter()
        .map(|path| {
            let meta = frontmatter(&path);
            let stem = path
                .file_stem()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            Item {
                name: meta.name.unwrap_or(stem),
                description: meta.description.unwrap_or_default(),
                source: source.into(),
                path,
            }
        })
        .collect()
}

/// Hooks from a settings file or a plugin's `hooks.json`:
/// `{"hooks": {"<Event>": [{"matcher": "...", "hooks": [{"type": "command", "command": "..."}]}]}}`.
fn scan_hooks(file: &Path, source: &str) -> Vec<Item> {
    let Some(json) = fs::read_to_string(file)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
    else {
        return Vec::new();
    };
    let Some(events) = json["hooks"].as_object() else {
        return Vec::new();
    };
    let mut items = Vec::new();
    for (event, groups) in events {
        for group in groups.as_array().into_iter().flatten() {
            let matcher = group["matcher"].as_str().filter(|m| !m.is_empty());
            for hook in group["hooks"].as_array().into_iter().flatten() {
                let action = hook["command"]
                    .as_str()
                    .or_else(|| hook["url"].as_str())
                    .or_else(|| hook["prompt"].as_str())
                    .unwrap_or_else(|| hook["type"].as_str().unwrap_or("?"));
                let description = match matcher {
                    Some(m) => format!("[{m}] {action}"),
                    None => action.to_string(),
                };
                items.push(Item {
                    name: event.clone(),
                    description,
                    source: source.into(),
                    path: file.to_path_buf(),
                });
            }
        }
    }
    items
}

fn find_files(dir: &Path, depth: usize, wanted: &dyn Fn(&Path) -> bool) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return found;
    };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            if depth > 0 {
                found.extend(find_files(&path, depth - 1, wanted));
            }
        } else if wanted(&path) {
            found.push(path);
        }
    }
    found
}

#[derive(Default, Debug, PartialEq)]
struct Frontmatter {
    name: Option<String>,
    description: Option<String>,
}

fn frontmatter(path: &Path) -> Frontmatter {
    fs::read_to_string(path)
        .map(|text| parse_frontmatter(&text))
        .unwrap_or_default()
}

/// Reads `name` and `description` from YAML frontmatter. Handles plain,
/// quoted and folded (`>` / `|`) values, which is all these files use.
fn parse_frontmatter(text: &str) -> Frontmatter {
    let mut meta = Frontmatter::default();
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return meta;
    }
    let body: Vec<&str> = lines.take_while(|l| l.trim() != "---").collect();
    let mut i = 0;
    while i < body.len() {
        let line = body[i];
        i += 1;
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if line.starts_with([' ', '\t']) || !matches!(key.trim(), "name" | "description") {
            continue;
        }
        let mut value = value.trim().to_string();
        if matches!(value.as_str(), ">" | "|" | ">-" | "|-" | "") {
            // Block scalar: the indented lines that follow.
            let mut parts = Vec::new();
            while i < body.len() && (body[i].starts_with([' ', '\t']) || body[i].is_empty()) {
                parts.push(body[i].trim());
                i += 1;
            }
            value = parts.join(" ").trim().to_string();
        }
        let value = value
            .trim_matches(|c| c == '"' || c == '\'')
            .replace("\\\"", "\"");
        match key.trim() {
            "name" => meta.name = Some(value),
            _ => meta.description = Some(value),
        }
    }
    meta
}

/// `claude plugin details <id>`: component inventory and projected token cost,
/// as printed by Claude Code.
pub fn plugin_details(id: &str) -> Result<String, String> {
    let output = crate::claude_cli::command()
        .args(["plugin", "details", id])
        .output()
        .map_err(|e| format!("Could not run `claude plugin details`: {e}"))?;
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    if output.status.success() {
        Ok(text)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// The "Always-on: ~840 tok" figure from `claude plugin details`.
pub fn always_on_tokens(details: &str) -> Option<String> {
    details.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("Always-on:")?;
        rest.split_whitespace().next().map(str::to_owned)
    })
}

/// Enables or disables a plugin with the official CLI.
pub fn set_plugin_enabled(id: &str, enable: bool) -> Result<String, String> {
    let action = if enable { "enable" } else { "disable" };
    let output = crate::claude_cli::command()
        .args(["plugin", action, id, "--json"])
        .output()
        .map_err(|e| format!("Could not run `claude plugin {action}`: {e}"))?;
    // `--json` prints one result line: {"command", "outcome", "message", ...}.
    let result: serde_json::Value =
        serde_json::from_slice(&output.stdout).unwrap_or(serde_json::Value::Null);
    let message = result["message"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| String::from_utf8_lossy(&output.stderr).trim().to_string());
    if output.status.success() && result["outcome"].as_str() != Some("failed") {
        Ok(message)
    } else {
        Err(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_quoted_and_folded_frontmatter() {
        let plain = "---\nname: deploy\ndescription: Deploy to production\n---\n# Body";
        assert_eq!(
            parse_frontmatter(plain),
            Frontmatter {
                name: Some("deploy".into()),
                description: Some("Deploy to production".into())
            }
        );

        let quoted =
            "---\nname: brainstorming\ndescription: \"You MUST use this: before work\"\n---";
        assert_eq!(
            parse_frontmatter(quoted).description.as_deref(),
            Some("You MUST use this: before work")
        );

        let folded =
            "---\nname: x\ndescription: >\n  First line\n  second line\nmodel: sonnet\n---";
        assert_eq!(
            parse_frontmatter(folded).description.as_deref(),
            Some("First line second line")
        );

        assert_eq!(
            parse_frontmatter("# No frontmatter"),
            Frontmatter::default()
        );
    }

    #[test]
    fn reads_hooks_from_settings() {
        let dir = std::env::temp_dir().join(format!("claudash-hooks-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("settings.json");
        fs::write(
            &file,
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"./check.sh"}]}],
                "SessionStart":[{"hooks":[{"type":"command","command":"echo hi"}]}]}}"#,
        )
        .unwrap();
        let hooks = scan_hooks(&file, "user");
        fs::remove_dir_all(&dir).unwrap();

        let described: Vec<(&str, &str)> = hooks
            .iter()
            .map(|h| (h.name.as_str(), h.description.as_str()))
            .collect();
        assert!(described.contains(&("PreToolUse", "[Bash] ./check.sh")));
        assert!(described.contains(&("SessionStart", "echo hi")));
    }

    #[test]
    fn extracts_always_on_tokens() {
        let details = "Projected token cost\n  Always-on:   ~840 tok   added to every session\n";
        assert_eq!(always_on_tokens(details).as_deref(), Some("~840"));
        assert_eq!(always_on_tokens("nothing here"), None);
    }

    #[test]
    fn skills_are_one_level_deep_except_synced() {
        let dir = std::env::temp_dir().join(format!("claudash-skills-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        for rel in ["deploy", "deploy/upstream", "synced/org-1/brand"] {
            fs::create_dir_all(dir.join(rel)).unwrap();
            fs::write(dir.join(rel).join("SKILL.md"), "---\nname: x\n---").unwrap();
        }
        fs::create_dir_all(dir.join("not-a-skill")).unwrap();
        let skills = scan_skills(&dir, "user");
        fs::remove_dir_all(&dir).unwrap();

        // Compare paths, not strings: the separator differs on Windows.
        let found: Vec<(PathBuf, &str)> = skills
            .iter()
            .map(|s| {
                (
                    s.path.strip_prefix(&dir).unwrap().to_path_buf(),
                    s.source.as_str(),
                )
            })
            .collect();
        assert_eq!(
            found,
            [
                (Path::new("deploy").join("SKILL.md"), "user"),
                (
                    Path::new("synced")
                        .join("org-1")
                        .join("brand")
                        .join("SKILL.md"),
                    "claude.ai"
                ),
            ]
        );
    }
}
