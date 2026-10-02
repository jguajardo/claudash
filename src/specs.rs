//! Spec-driven development frameworks in a project (OpenSpec, spec-kit,
//! Kiro/cc-sdd, Task Master and others): which one it uses, its open changes
//! and how far along their tasks are, and the framework's own slash command
//! for the next step. claudash only reads their files; the frameworks and
//! Claude Code do the work.

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    time::SystemTime,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Framework {
    OpenSpec,
    SpecKit,
    Kiro,
    SpecWorkflow,
    TaskMaster,
    Gsd,
    Bmad,
    AgentOs,
}

impl Framework {
    pub fn title(self) -> &'static str {
        match self {
            Framework::OpenSpec => "OpenSpec",
            Framework::SpecKit => "spec-kit",
            Framework::Kiro => "Kiro specs",
            Framework::SpecWorkflow => "spec-workflow",
            Framework::TaskMaster => "Task Master",
            Framework::Gsd => "GSD",
            Framework::Bmad => "BMAD",
            Framework::AgentOs => "Agent OS",
        }
    }
}

/// Where a change stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// Being specified: no tasks yet.
    Planning,
    /// Tasks left to do.
    Implementing,
    /// Every task done.
    Complete,
}

impl Stage {
    pub fn label(self) -> &'static str {
        match self {
            Stage::Planning => "planning",
            Stage::Implementing => "implementing",
            Stage::Complete => "complete",
        }
    }
}

/// A change, feature spec or task list.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub framework: Framework,
    pub id: String,
    /// Its folder (or file).
    pub path: PathBuf,
    pub done: usize,
    pub total: usize,
    pub stage: Stage,
    /// Newest modification of its files.
    pub modified: SystemTime,
    /// The framework's slash command for the next step, when the project has it.
    pub next: Option<String>,
    /// What `next` is for, e.g. "write the plan".
    pub next_label: Option<&'static str>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProjectSpecs {
    pub frameworks: Vec<Framework>,
    pub changes: Vec<Change>,
}

/// `(done, total)` from Markdown checkboxes: `- [ ]`, `- [x]`, `* [X]`, `- [-]`
/// (in progress, not done).
pub fn count_tasks(text: &str) -> (usize, usize) {
    let mut done = 0;
    let mut total = 0;
    for line in text.lines() {
        let line = line.trim_start();
        let Some(rest) = line
            .strip_prefix("- [")
            .or_else(|| line.strip_prefix("* ["))
        else {
            continue;
        };
        let mut chars = rest.chars();
        let (Some(mark), Some(']')) = (chars.next(), chars.next()) else {
            continue;
        };
        match mark {
            'x' | 'X' => {
                done += 1;
                total += 1;
            }
            ' ' | '-' => total += 1,
            _ => {}
        }
    }
    (done, total)
}

/// Slash commands available in `dir`: project and user commands
/// (`commands/a/b.md` is `/a:b`) and skills (`skills/<name>/SKILL.md` is `/<name>`).
pub fn slash_commands(dir: &Path) -> HashSet<String> {
    let mut roots = vec![dir.join(".claude")];
    // Tests only see the project's own commands.
    if let Some(home) = crate::paths::claude_home().filter(|_| !cfg!(test)) {
        roots.push(home);
    }
    let mut names = HashSet::new();
    for root in roots {
        collect_commands(&root.join("commands"), "", &mut names, 0);
        for entry in fs::read_dir(root.join("skills"))
            .into_iter()
            .flatten()
            .flatten()
        {
            if entry.path().join("SKILL.md").is_file() {
                names.insert(entry.file_name().to_string_lossy().into_owned());
            }
        }
    }
    names
}

fn collect_commands(dir: &Path, prefix: &str, names: &mut HashSet<String>, depth: usize) {
    if depth > 3 {
        return;
    }
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            collect_commands(&path, &format!("{prefix}{name}:"), names, depth + 1);
        } else if let Some(stem) = name.strip_suffix(".md") {
            names.insert(format!("{prefix}{stem}"));
        }
    }
}

/// The first candidate command the project has, with its arguments.
fn pick(available: &HashSet<String>, candidates: &[&str], args: &str) -> Option<String> {
    candidates
        .iter()
        .find(|c| available.contains(**c))
        .map(|c| {
            if args.is_empty() {
                format!("/{c}")
            } else {
                format!("/{c} {args}")
            }
        })
}

fn newest(path: &Path, depth: usize) -> SystemTime {
    let mut newest = fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    if depth > 0 && path.is_dir() {
        for entry in fs::read_dir(path).into_iter().flatten().flatten() {
            newest = newest.max(self::newest(&entry.path(), depth - 1));
        }
    }
    newest
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
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

fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Stage from task counts: no tasks is still planning.
fn stage(done: usize, total: usize) -> Stage {
    match (done, total) {
        (_, 0) => Stage::Planning,
        (d, t) if d >= t => Stage::Complete,
        _ => Stage::Implementing,
    }
}

/// Reads every framework found in `dir`.
pub fn scan(dir: &Path) -> ProjectSpecs {
    let mut specs = ProjectSpecs::default();
    let available = slash_commands(dir);

    // OpenSpec: openspec/changes/<id>/{proposal,design,tasks}.md.
    if dir.join("openspec").is_dir() {
        specs.frameworks.push(Framework::OpenSpec);
        for path in subdirs(&dir.join("openspec/changes")) {
            let id = name_of(&path);
            if id == "archive" {
                continue;
            }
            let (done, total) = count_tasks(&read(&path.join("tasks.md")));
            let stage = stage(done, total);
            let (candidates, label): (&[&str], _) = match stage {
                Stage::Planning => (
                    &["opsx:continue", "opsx:ff", "opsx:update"],
                    "finish planning it",
                ),
                Stage::Implementing => (&["opsx:apply", "openspec:apply"], "implement the tasks"),
                Stage::Complete => (&["opsx:archive", "openspec:archive"], "archive it"),
            };
            let next = pick(&available, candidates, &id);
            specs.changes.push(Change {
                framework: Framework::OpenSpec,
                next_label: next.as_ref().map(|_| label),
                next,
                modified: newest(&path, 3),
                id,
                path,
                done,
                total,
                stage,
            });
        }
    }

    // spec-kit: .specify/ and specs/NNN-slug/{spec,plan,tasks}.md.
    if dir.join(".specify").is_dir() {
        specs.frameworks.push(Framework::SpecKit);
        for path in subdirs(&dir.join("specs")) {
            let id = name_of(&path);
            let (done, total) = count_tasks(&read(&path.join("tasks.md")));
            let (candidates, label): (&[&str], _) = if !path.join("plan.md").is_file() {
                (&["speckit-plan", "speckit.plan"][..], "write the plan")
            } else if !path.join("tasks.md").is_file() {
                (
                    &["speckit-tasks", "speckit.tasks"][..],
                    "break it into tasks",
                )
            } else if done < total {
                (
                    &["speckit-implement", "speckit.implement"][..],
                    "implement the tasks",
                )
            } else {
                (&[][..], "")
            };
            // spec-kit commands act on the feature of the current git branch.
            let next = pick(&available, candidates, "");
            specs.changes.push(Change {
                framework: Framework::SpecKit,
                next_label: next.as_ref().map(|_| label),
                next,
                modified: newest(&path, 2),
                stage: stage(done, total),
                id,
                path,
                done,
                total,
            });
        }
    }

    // Kiro and cc-sdd: .kiro/specs/<feature>/{requirements,design,tasks}.md.
    if dir.join(".kiro/specs").is_dir() {
        specs.frameworks.push(Framework::Kiro);
        for path in subdirs(&dir.join(".kiro/specs")) {
            let id = name_of(&path);
            let (done, total) = count_tasks(&read(&path.join("tasks.md")));
            let (candidates, label): (&[&str], _) = if !path.join("requirements.md").is_file() {
                (
                    &["kiro-spec-requirements", "kiro:spec-requirements"][..],
                    "write the requirements",
                )
            } else if !path.join("design.md").is_file() {
                (
                    &["kiro-spec-design", "kiro:spec-design"][..],
                    "write the design",
                )
            } else if !path.join("tasks.md").is_file() {
                (
                    &["kiro-spec-tasks", "kiro:spec-tasks"][..],
                    "break it into tasks",
                )
            } else if done < total {
                (&["kiro-impl", "kiro:spec-impl"][..], "implement the tasks")
            } else {
                (&[][..], "")
            };
            let next = pick(&available, candidates, &id);
            specs.changes.push(Change {
                framework: Framework::Kiro,
                next_label: next.as_ref().map(|_| label),
                next,
                modified: newest(&path, 2),
                stage: stage(done, total),
                id,
                path,
                done,
                total,
            });
        }
    }

    // spec-workflow-mcp: .spec-workflow/specs/<feature>/tasks.md (driven over MCP).
    if dir.join(".spec-workflow").is_dir() {
        specs.frameworks.push(Framework::SpecWorkflow);
        for path in subdirs(&dir.join(".spec-workflow/specs")) {
            let (done, total) = count_tasks(&read(&path.join("tasks.md")));
            specs.changes.push(Change {
                framework: Framework::SpecWorkflow,
                id: name_of(&path),
                modified: newest(&path, 2),
                stage: stage(done, total),
                path,
                done,
                total,
                next: None,
                next_label: None,
            });
        }
    }

    // Task Master: .taskmaster/tasks/tasks.json, one task list per tag.
    let tasks_json = dir.join(".taskmaster/tasks/tasks.json");
    if dir.join(".taskmaster").is_dir() {
        specs.frameworks.push(Framework::TaskMaster);
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&read(&tasks_json)) {
            let modified = newest(&tasks_json, 0);
            // Tagged format: {"master": {"tasks": [...]}}; legacy: {"tasks": [...]}.
            let lists: Vec<(String, &serde_json::Value)> = if value["tasks"].is_array() {
                vec![("tasks".into(), &value["tasks"])]
            } else {
                value
                    .as_object()
                    .into_iter()
                    .flatten()
                    .filter(|(_, v)| v["tasks"].is_array())
                    .map(|(k, v)| (k.clone(), &v["tasks"]))
                    .collect()
            };
            for (tag, tasks) in lists {
                let tasks: Vec<&serde_json::Value> = tasks
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|t| t["status"].as_str() != Some("cancelled"))
                    .collect();
                let done = tasks
                    .iter()
                    .filter(|t| t["status"].as_str() == Some("done"))
                    .count();
                specs.changes.push(Change {
                    framework: Framework::TaskMaster,
                    id: tag,
                    path: tasks_json.clone(),
                    done,
                    total: tasks.len(),
                    stage: stage(done, tasks.len()),
                    modified,
                    next: None,
                    next_label: None,
                });
            }
        }
    }

    // GSD: .planning/ with a ROADMAP.md of phases.
    if dir.join(".planning/STATE.md").is_file() || dir.join(".planning/ROADMAP.md").is_file() {
        specs.frameworks.push(Framework::Gsd);
        let roadmap = dir.join(".planning/ROADMAP.md");
        let (done, total) = count_tasks(&read(&roadmap));
        if total > 0 {
            let next = pick(&available, &["gsd-progress", "gsd:progress"], "");
            specs.changes.push(Change {
                framework: Framework::Gsd,
                id: "roadmap".into(),
                modified: newest(&dir.join(".planning"), 2),
                path: roadmap,
                done,
                total,
                stage: stage(done, total),
                next_label: next.as_ref().map(|_| "see where it stands"),
                next,
            });
        }
    }

    // Detected only: their layouts change between versions.
    if dir.join("_bmad").is_dir() {
        specs.frameworks.push(Framework::Bmad);
    }
    if dir.join("agent-os").is_dir() {
        specs.frameworks.push(Framework::AgentOs);
    }

    // Unfinished work first, then most recently touched.
    specs
        .changes
        .sort_by_key(|c| (c.stage == Stage::Complete, std::cmp::Reverse(c.modified)));
    specs
}

/// The files worth reading for a change, in order.
pub fn files(change: &Change) -> Vec<PathBuf> {
    if change.path.is_file() {
        return vec![change.path.clone()];
    }
    [
        "proposal.md",
        "requirements.md",
        "spec.md",
        "design.md",
        "plan.md",
        "tasks.md",
    ]
    .iter()
    .map(|f| change.path.join(f))
    .filter(|p| p.is_file())
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_checkboxes() {
        let text = "## 1. Setup\n- [x] 1.1 Add\n- [ ] 1.2 Wire\n  * [X] nested\n- [-] going\n- not a task\n- [?] odd\n";
        assert_eq!(count_tasks(text), (2, 4));
    }

    #[test]
    fn reads_openspec_and_speckit_and_offers_the_next_command() {
        let root = std::env::temp_dir().join(format!("claudash-specs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let write = |p: &str, text: &str| {
            let path = root.join(p);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        };
        write("openspec/changes/add-login/proposal.md", "# Add login");
        write("openspec/changes/add-login/tasks.md", "- [x] a\n- [ ] b\n");
        write("openspec/changes/done-one/tasks.md", "- [x] a\n");
        write(
            "openspec/changes/archive/2026-01-01-old/tasks.md",
            "- [ ] a\n",
        );
        write(".claude/commands/opsx/apply.md", "apply");
        write(".specify/memory/constitution.md", "rules");
        write("specs/001-search/spec.md", "# Search");
        write(
            ".claude/skills/speckit-plan/SKILL.md",
            "---\nname: speckit-plan\n---",
        );

        let specs = scan(&root);
        assert_eq!(specs.frameworks, [Framework::OpenSpec, Framework::SpecKit]);
        let login = specs.changes.iter().find(|c| c.id == "add-login").unwrap();
        assert_eq!(
            (login.done, login.total, login.stage),
            (1, 2, Stage::Implementing)
        );
        assert_eq!(login.next.as_deref(), Some("/opsx:apply add-login"));
        // No /opsx:archive in this project: no next step is invented.
        let done = specs.changes.iter().find(|c| c.id == "done-one").unwrap();
        assert_eq!((done.stage, done.next.as_deref()), (Stage::Complete, None));
        assert!(specs.changes.iter().all(|c| c.id != "archive"));
        let search = specs.changes.iter().find(|c| c.id == "001-search").unwrap();
        assert_eq!(search.next.as_deref(), Some("/speckit-plan"));
        // Complete changes sort last.
        assert_eq!(specs.changes.last().unwrap().id, "done-one");
        assert_eq!(files(login).len(), 2);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn reads_task_master_tags() {
        let root = std::env::temp_dir().join(format!("claudash-tm-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join(".taskmaster/tasks")).unwrap();
        fs::write(
            root.join(".taskmaster/tasks/tasks.json"),
            r#"{"master":{"tasks":[{"status":"done"},{"status":"pending"},{"status":"cancelled"}]}}"#,
        )
        .unwrap();
        let specs = scan(&root);
        assert_eq!(specs.frameworks, [Framework::TaskMaster]);
        assert_eq!((specs.changes[0].done, specs.changes[0].total), (1, 2));
        fs::remove_dir_all(&root).unwrap();
    }
}
