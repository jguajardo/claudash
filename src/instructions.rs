//! Which instruction files (CLAUDE.md / AGENTS.md) Claude Code loads for a project.
//!
//! Follows the rules documented at <https://code.claude.com/docs/en/memory>:
//!
//! - Always loaded: the managed-policy `CLAUDE.md` and the user's
//!   `~/.claude/CLAUDE.md`.
//! - From the working directory and every directory above it: `CLAUDE.md`,
//!   `.claude/CLAUDE.md` and `CLAUDE.local.md`.
//! - `AGENTS.md` / `.claude/AGENTS.md` in those same directories, depending on
//!   the "Project instructions" setting. By default (`claude-md-or-agents-md`)
//!   they are read only when none of the three project `CLAUDE.md` files exist
//!   in the working directory or above it.
//!
//! Only files that load at launch are listed; files in subdirectories load on
//! demand and are not shown.

use std::{
    fs,
    path::{Path, PathBuf},
};

/// Value of the "Project instructions" setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    ClaudeMdOrAgentsMd,
    ClaudeMdAndAgentsMd,
    ClaudeMd,
    ManagedOnly,
}

impl Mode {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "claude-md-or-agents-md" => Some(Self::ClaudeMdOrAgentsMd),
            "claude-md-and-agents-md" => Some(Self::ClaudeMdAndAgentsMd),
            "claude-md" => Some(Self::ClaudeMd),
            "managed-only" => Some(Self::ManagedOnly),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeMdOrAgentsMd => "claude-md-or-agents-md",
            Self::ClaudeMdAndAgentsMd => "claude-md-and-agents-md",
            Self::ClaudeMd => "claude-md",
            Self::ManagedOnly => "managed-only",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Managed,
    User,
    Project,
    Local,
    Agents,
}

impl Scope {
    pub fn label(self) -> &'static str {
        match self {
            Self::Managed => "managed",
            Self::User => "user",
            Self::Project => "project",
            Self::Local => "local",
            Self::Agents => "agents",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstructionFile {
    pub path: PathBuf,
    pub scope: Scope,
    /// `None` when Claude Code loads it; otherwise why it is skipped.
    pub skipped: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instructions {
    pub mode: Mode,
    pub files: Vec<InstructionFile>,
}

impl Instructions {
    /// Whether any project-level file (not managed or user) is loaded.
    pub fn has_project_instructions(&self) -> bool {
        self.files
            .iter()
            .any(|f| f.skipped.is_none() && !matches!(f.scope, Scope::Managed | Scope::User))
    }
}

/// Resolves the instruction files for a session that runs in `cwd`.
pub fn resolve(cwd: &Path) -> Instructions {
    let claude_home = crate::paths::claude_home();
    let managed_dir = crate::paths::managed_dir();
    let mode = read_mode(claude_home.as_deref(), &managed_dir);
    resolve_with(cwd, claude_home.as_deref(), &managed_dir, mode)
}

fn resolve_with(
    cwd: &Path,
    claude_home: Option<&Path>,
    managed_dir: &Path,
    mode: Mode,
) -> Instructions {
    let user_file = claude_home.map(|dir| dir.join("CLAUDE.md"));

    // Root first, working directory last: the order Claude Code concatenates them in.
    let mut ancestors: Vec<&Path> = cwd.ancestors().collect();
    ancestors.reverse();

    let mut project = Vec::new();
    let mut agents = Vec::new();
    for dir in ancestors {
        for (name, scope) in [
            ("CLAUDE.md", Scope::Project),
            (".claude/CLAUDE.md", Scope::Project),
            ("CLAUDE.local.md", Scope::Local),
        ] {
            let path = dir.join(name);
            // `~/.claude/CLAUDE.md` is the user file, not a project one.
            if user_file.as_ref() != Some(&path) && path.is_file() {
                project.push((path, scope));
            }
        }
        for name in ["AGENTS.md", ".claude/AGENTS.md"] {
            let path = dir.join(name);
            if path.is_file() {
                agents.push(path);
            }
        }
    }

    let managed_only = (mode == Mode::ManagedOnly).then_some("managed-only");
    let agents_skipped = match mode {
        Mode::ClaudeMdOrAgentsMd if !project.is_empty() => Some("CLAUDE.md wins"),
        Mode::ClaudeMdOrAgentsMd | Mode::ClaudeMdAndAgentsMd => None,
        Mode::ClaudeMd => Some("setting: claude-md"),
        Mode::ManagedOnly => managed_only,
    };

    let mut files = Vec::new();
    let managed_file = managed_dir.join("CLAUDE.md");
    if managed_file.is_file() {
        files.push(InstructionFile {
            path: managed_file,
            scope: Scope::Managed,
            skipped: None,
        });
    }
    if let Some(path) = user_file.filter(|p| p.is_file()) {
        files.push(InstructionFile {
            path,
            scope: Scope::User,
            skipped: managed_only,
        });
    }
    files.extend(project.into_iter().map(|(path, scope)| InstructionFile {
        path,
        scope,
        skipped: managed_only,
    }));
    files.extend(agents.into_iter().map(|path| InstructionFile {
        path,
        scope: Scope::Agents,
        skipped: agents_skipped,
    }));

    Instructions { mode, files }
}

/// Reads the "Project instructions" setting. Claude Code honors it in user and
/// managed settings only (not project or local ones); managed settings win.
fn read_mode(claude_home: Option<&Path>, managed_dir: &Path) -> Mode {
    let from = |path: PathBuf| -> Option<Mode> {
        let json: serde_json::Value = serde_json::from_str(&fs::read_to_string(path).ok()?).ok()?;
        let value = json
            .pointer("/pluginConfigs/agents-md@builtin/options/instructionFiles")?
            .as_str()?;
        Mode::parse(value)
    };
    from(managed_dir.join("managed-settings.json"))
        .or_else(|| claude_home.and_then(|dir| from(dir.join("settings.json"))))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway directory tree: `root/home/.claude`, `root/managed`, `root/repo/app`.
    struct Tree {
        root: PathBuf,
    }

    impl Tree {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "claudash-instructions-{tag}-{}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&root);
            for dir in ["home/.claude", "managed", "repo/app/.claude"] {
                fs::create_dir_all(root.join(dir)).unwrap();
            }
            Self { root }
        }
        fn touch(&self, rel: &str) -> PathBuf {
            let path = self.root.join(rel);
            fs::write(&path, "x").unwrap();
            path
        }
        fn resolve(&self, mode: Mode) -> Instructions {
            resolve_with(
                &self.root.join("repo/app"),
                Some(&self.root.join("home/.claude")),
                &self.root.join("managed"),
                mode,
            )
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn skipped_of(i: &Instructions, path: &Path) -> Option<Option<&'static str>> {
        i.files.iter().find(|f| f.path == path).map(|f| f.skipped)
    }

    #[test]
    fn agents_md_loads_when_there_is_no_claude_md() {
        let t = Tree::new("agents-only");
        let agents = t.touch("repo/app/AGENTS.md");
        let user = t.touch("home/.claude/CLAUDE.md");

        let i = t.resolve(Mode::ClaudeMdOrAgentsMd);
        // The user file doesn't count as a project CLAUDE.md.
        assert_eq!(skipped_of(&i, &agents), Some(None));
        assert_eq!(skipped_of(&i, &user), Some(None));
        assert!(i.has_project_instructions());
    }

    #[test]
    fn claude_md_in_a_parent_directory_hides_agents_md() {
        let t = Tree::new("parent-claude");
        let parent = t.touch("repo/CLAUDE.md");
        let agents = t.touch("repo/app/AGENTS.md");

        let i = t.resolve(Mode::ClaudeMdOrAgentsMd);
        assert_eq!(skipped_of(&i, &parent), Some(None));
        assert_eq!(skipped_of(&i, &agents), Some(Some("CLAUDE.md wins")));

        let both = t.resolve(Mode::ClaudeMdAndAgentsMd);
        assert_eq!(skipped_of(&both, &agents), Some(None));
    }

    #[test]
    fn claude_local_md_counts_and_files_are_ordered_root_first() {
        let t = Tree::new("local");
        let parent = t.touch("repo/CLAUDE.md");
        let dot_claude = t.touch("repo/app/.claude/CLAUDE.md");
        let local = t.touch("repo/app/CLAUDE.local.md");
        let agents = t.touch("repo/app/.claude/AGENTS.md");

        let i = t.resolve(Mode::ClaudeMdOrAgentsMd);
        let paths: Vec<&PathBuf> = i.files.iter().map(|f| &f.path).collect();
        assert_eq!(paths, [&parent, &dot_claude, &local, &agents]);
        assert_eq!(i.files[2].scope, Scope::Local);
        assert!(i.files[3].skipped.is_some());
    }

    #[test]
    fn managed_only_skips_everything_but_the_managed_file() {
        let t = Tree::new("managed-only");
        let managed = t.touch("managed/CLAUDE.md");
        let user = t.touch("home/.claude/CLAUDE.md");
        let project = t.touch("repo/app/CLAUDE.md");

        let i = t.resolve(Mode::ManagedOnly);
        assert_eq!(skipped_of(&i, &managed), Some(None));
        assert!(skipped_of(&i, &user).unwrap().is_some());
        assert!(skipped_of(&i, &project).unwrap().is_some());
        assert!(!i.has_project_instructions());
    }

    #[test]
    fn reads_mode_from_settings_with_managed_taking_precedence() {
        let t = Tree::new("mode");
        let home = t.root.join("home/.claude");
        let managed = t.root.join("managed");
        let setting = |v: &str| {
            format!(
                r#"{{"pluginConfigs":{{"agents-md@builtin":{{"options":{{"instructionFiles":"{v}"}}}}}}}}"#
            )
        };

        assert_eq!(read_mode(Some(&home), &managed), Mode::ClaudeMdOrAgentsMd);
        fs::write(
            home.join("settings.json"),
            setting("claude-md-and-agents-md"),
        )
        .unwrap();
        assert_eq!(read_mode(Some(&home), &managed), Mode::ClaudeMdAndAgentsMd);
        fs::write(managed.join("managed-settings.json"), setting("claude-md")).unwrap();
        assert_eq!(read_mode(Some(&home), &managed), Mode::ClaudeMd);
    }
}
