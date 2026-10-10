//! Branch reviews: claudash fetches, finds the branch and its base, checks it
//! out in a worktree of its own (your checkout is never touched), and hands
//! the review to Claude Code. Claude does the reviewing; claudash shows the
//! findings as file, line and comment, and posting them is up to you.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use serde::{Deserialize, Serialize};

use crate::claude_cli;

/// Bases tried in order, on the repository's remote.
const BASES: [&str; 3] = ["develop", "main", "master"];
/// Branches listed at most, most recently committed first.
const MAX_BRANCHES: usize = 300;

/// Runs git without ever prompting: a password or passphrase prompt would
/// write over the dashboard. Fetches that need one fail instead.
fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null());
    if std::env::var_os("GIT_SSH_COMMAND").is_none() {
        cmd.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    }
    let output = cmd
        .output()
        .map_err(|e| format!("Could not run git: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Branch {
    /// Without the remote: `feature/login`.
    pub name: String,
    /// With it: `origin/feature/login`.
    pub reference: String,
    pub subject: String,
    pub author: String,
    /// Epoch seconds of the last commit.
    pub when: i64,
    /// Commits on the branch that the base doesn't have.
    pub ahead: u32,
    /// The branch checked out in your own checkout (its committed work),
    /// to review before pushing. `reference` is then its commit.
    pub local: bool,
}

/// What the branch picker shows for a repository.
#[derive(Clone, Debug, Default)]
pub struct Listing {
    /// `origin/develop`, else `origin/main`, else `origin/master`.
    pub base: Option<String>,
    pub branches: Vec<Branch>,
    /// Why `git fetch` failed; the branches are then the ones known locally.
    pub fetch_error: Option<String>,
}

/// Fetches and lists the remote's branches. Slow (network): call it off the
/// UI thread.
pub fn list(repo: &Path, checkout: &Path) -> Result<Listing, String> {
    let remotes = git(repo, &["remote"])?;
    let remote = remotes
        .lines()
        .find(|r| *r == "origin")
        .or_else(|| remotes.lines().next())
        .ok_or("This repository has no remote to fetch branches from")?
        .to_string();
    let fetch_error = git(repo, &["fetch", "--prune", "--quiet", &remote]).err();
    let base = BASES.iter().map(|b| format!("{remote}/{b}")).find(|b| {
        git(
            repo,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/remotes/{b}"),
            ],
        )
        .is_ok()
    });
    let refs = git(
        repo,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            "--format=%(refname:short)%09%(subject)%09%(authorname)%09%(committerdate:unix)",
            &format!("refs/remotes/{remote}"),
        ],
    )?;
    let mut branches = Vec::new();
    for line in refs.lines() {
        let mut parts = line.splitn(4, '\t');
        let (Some(reference), Some(subject), Some(author), Some(when)) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let Some(name) = reference.strip_prefix(&format!("{remote}/")) else {
            continue;
        };
        if name == "HEAD" || Some(reference) == base.as_deref() || reference == remote {
            continue;
        }
        let ahead = base
            .as_ref()
            .and_then(|b| git(repo, &["rev-list", "--count", &format!("{b}..{reference}")]).ok())
            .and_then(|n| n.trim().parse().ok())
            .unwrap_or(0);
        branches.push(Branch {
            name: name.to_string(),
            reference: reference.to_string(),
            subject: subject.to_string(),
            author: author.to_string(),
            when: when.trim().parse().unwrap_or(0),
            ahead,
            local: false,
        });
        if branches.len() >= MAX_BRANCHES {
            break;
        }
    }
    if let Some(base) = &base
        && let Some(local) = local_branch(checkout, base)
    {
        branches.insert(0, local);
    }
    Ok(Listing {
        base,
        branches,
        fetch_error,
    })
}

/// The branch checked out in `checkout`, when it has commits the base lacks.
fn local_branch(checkout: &Path, base: &str) -> Option<Branch> {
    let name = git(checkout, &["rev-parse", "--abbrev-ref", "HEAD"]).ok()?;
    let name = name.trim();
    if name == "HEAD" {
        return None; // Detached.
    }
    let ahead: u32 = git(checkout, &["rev-list", "--count", &format!("{base}..HEAD")])
        .ok()?
        .trim()
        .parse()
        .ok()?;
    if ahead == 0 {
        return None;
    }
    let info = git(checkout, &["log", "-1", "--format=%H%x09%s%x09%an%x09%ct"]).ok()?;
    let mut parts = info.trim().splitn(4, '\t');
    Some(Branch {
        name: name.to_string(),
        reference: parts.next()?.to_string(),
        subject: parts.next()?.to_string(),
        author: parts.next()?.to_string(),
        when: parts.next()?.parse().unwrap_or(0),
        ahead,
        local: true,
    })
}

/// Size of what a review would read: `base...branch`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DiffStat {
    pub commits: u32,
    pub files: u32,
    pub insertions: u32,
    pub deletions: u32,
}

pub fn diff_stat(repo: &Path, base: &str, reference: &str) -> Result<DiffStat, String> {
    let commits = git(
        repo,
        &["rev-list", "--count", &format!("{base}..{reference}")],
    )?
    .trim()
    .parse()
    .unwrap_or(0);
    let short = git(
        repo,
        &["diff", "--shortstat", &format!("{base}...{reference}")],
    )?;
    Ok(parse_shortstat(&short, commits))
}

/// " 8 files changed, 340 insertions(+), 90 deletions(-)"
fn parse_shortstat(text: &str, commits: u32) -> DiffStat {
    let mut stat = DiffStat {
        commits,
        ..Default::default()
    };
    for part in text.split(',') {
        let mut words = part.split_whitespace();
        let n = words.next().and_then(|n| n.parse().ok()).unwrap_or(0);
        match words.next() {
            Some(w) if w.starts_with("file") => stat.files = n,
            Some(w) if w.starts_with("insertion") => stat.insertions = n,
            Some(w) if w.starts_with("deletion") => stat.deletions = n,
            _ => {}
        }
    }
    stat
}

/// Where claudash keeps review worktrees: outside the repository, so they
/// never show up in its `git status`.
pub fn worktrees_dir() -> Option<PathBuf> {
    // Tests never make worktrees in the real cache.
    if cfg!(test) {
        return None;
    }
    crate::paths::claudash_cache().map(|dir| dir.join("reviews"))
}

fn slug(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

/// What tells one review worktree from another: two reviews with the same
/// key use the same folder of the cache.
pub fn worktree_key(repo_name: &str, branch: &str) -> (String, String) {
    (slug(repo_name), slug(branch))
}

/// Where the review of `branch` of the repository called `repo_name` has its
/// worktree.
pub fn worktree_for(repo_name: &str, branch: &str) -> Option<PathBuf> {
    let (repo, branch) = worktree_key(repo_name, branch);
    Some(worktrees_dir()?.join(repo).join(branch))
}

/// A file the tab of an interactive review creates when its session ends.
/// Its folder is made here, so the tab only has to write the file.
pub fn ended_marker(session_id: &str) -> Option<PathBuf> {
    let dir = worktrees_dir()?.parent()?.join("ended");
    fs::create_dir_all(&dir).ok()?;
    Some(dir.join(slug(session_id)))
}

/// A worktree with the branch checked out (detached, so no local branch is
/// created), made or updated for this review.
pub fn prepare_worktree(
    repo: &Path,
    repo_name: &str,
    branch: &str,
    reference: &str,
) -> Result<PathBuf, String> {
    let dir = worktree_for(repo_name, branch).ok_or("Could not find the cache directory")?;
    if dir.join(".git").exists() {
        git(&dir, &["checkout", "--quiet", "--detach", reference])?;
    } else {
        if let Some(parent) = dir.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let path = dir.to_string_lossy().into_owned();
        git(
            repo,
            &["worktree", "add", "--quiet", "--detach", &path, reference],
        )?;
    }
    Ok(dir)
}

/// How far the review goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    /// Claude reads the changes and the code; nothing runs. In the background.
    Static,
    /// Also runs the tests, in an interactive session that asks first.
    Tests,
    /// Also starts the project and checks it, in an interactive session.
    Run,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Static, Mode::Tests, Mode::Run];

    pub fn title(self) -> &'static str {
        match self {
            Mode::Static => "Static review",
            Mode::Tests => "Review and run the tests",
            Mode::Run => "Review and start the project",
        }
    }

    pub fn detail(self) -> &'static str {
        match self {
            Mode::Static => {
                "Claude reads the commits, the diff and the code around it; nothing runs and no \
                 file changes. Runs in the background, and several can run at once; the findings \
                 open when it's done, or wait under B while you do something else."
            }
            Mode::Tests => {
                "Opens Claude Code in the review worktree: it reviews, finds how the project runs \
                 its tests and runs them, asking you before each command. Findings show when \
                 that session ends."
            }
            Mode::Run => {
                "Opens Claude Code in the review worktree: it reviews, finds how to start the \
                 project, starts it and checks it works, asking you before each command. Findings \
                 show when that session ends."
            }
        }
    }
}

/// One comment worth leaving on the pull request.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    pub file: String,
    #[serde(default)]
    pub line: Option<u32>,
    #[serde(default)]
    pub severity: String,
    pub comment: String,
    #[serde(default)]
    pub suggestion: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Findings {
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub findings: Vec<Finding>,
}

/// A finished review, kept in claudash's data directory.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Review {
    pub session_id: String,
    pub repo: PathBuf,
    pub branch: String,
    pub base: String,
    pub worktree: PathBuf,
    pub mode: Mode,
    /// Epoch seconds.
    pub at: i64,
    #[serde(flatten)]
    pub result: Findings,
}

const SCHEMA: &str = r#"{"type":"object","properties":{"summary":{"type":"string"},"findings":{"type":"array","items":{"type":"object","properties":{"file":{"type":"string"},"line":{"type":"integer"},"severity":{"type":"string","enum":["high","medium","low"]},"comment":{"type":"string"},"suggestion":{"type":"string"}},"required":["file","severity","comment"]}}},"required":["summary","findings"]}"#;

/// What Claude is asked to do.
pub fn prompt(mode: Mode, branch: &str, base: &str) -> String {
    let mut text = format!(
        "Review the branch `{branch}` against `{base}` the way a senior engineer reviews a pull \
         request. This folder is a checkout of the branch. Use `git log --oneline {base}..HEAD` \
         and `git diff {base}...HEAD` to see its commits and changes, and read the code around \
         them where it helps. Don't change any tracked file.\n\n\
         Report only what deserves a comment on the pull request: bugs, security problems, \
         missing error handling, risky or unclear code, missing tests. For each one give the \
         file (relative to the repository root), the line in the new version of the file, a \
         severity (high, medium or low), the comment to leave, addressed to the author, and a \
         suggested change when it helps. No praise and nothing a linter would catch. If nothing \
         deserves a comment, return no findings. Also write a summary of the branch in two or \
         three sentences. Write the summary and comments in the language of the branch's \
         commit messages."
    );
    match mode {
        Mode::Static => {}
        Mode::Tests => text.push_str(
            "\n\nThen find out how this project runs its tests and run them, asking me before \
             running anything. Add failing tests, and problems running them, as findings.",
        ),
        Mode::Run => text.push_str(
            "\n\nThen find out how to start this project locally, start it, check that it comes \
             up and that what the branch changed works, and stop it when you're done. Ask me \
             before running anything. Add what goes wrong as findings.",
        ),
    }
    if mode != Mode::Static {
        text.push_str(&format!(
            "\n\nEnd your last reply with the review as a ```json code block matching this \
             JSON Schema, so claudash can list it:\n{SCHEMA}"
        ));
    }
    text
}

/// Arguments for an interactive `claude` in the worktree (tests and run modes).
pub fn interactive_args(mode: Mode, branch: &str, base: &str, session_id: &str) -> Vec<String> {
    vec![
        "--session-id".into(),
        session_id.into(),
        "--name".into(),
        format!("Review {branch}"),
        prompt(mode, branch, base),
    ]
}

/// A static review with `claude -p`, allowed only to read files and run
/// `git log`, `git diff` and `git show`. Slow: call it off the UI thread.
pub fn run_static(
    worktree: &Path,
    branch: &str,
    base: &str,
    session_id: &str,
) -> Result<Findings, String> {
    let mut child = claude_cli::command()
        .args([
            "-p",
            "--session-id",
            session_id,
            "--name",
            &format!("Review {branch}"),
            "--tools",
            "Read,Grep,Glob,Bash",
            "--output-format",
            "json",
            "--json-schema",
            SCHEMA,
            "--allowedTools",
            "Bash(git log:*)",
            "Bash(git diff:*)",
            "Bash(git show:*)",
        ])
        .current_dir(worktree)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not run `claude -p`: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(prompt(Mode::Static, branch, base).as_bytes())
            .map_err(|e| format!("Could not send the prompt: {e}"))?;
    }
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).map_err(|_| {
        let stderr = String::from_utf8_lossy(&output.stderr);
        format!("`claude -p` failed: {}", stderr.trim())
    })?;
    if json["is_error"].as_bool() == Some(true) {
        return Err(json["result"]
            .as_str()
            .unwrap_or("Claude reported an error")
            .to_string());
    }
    let structured = &json["structured_output"];
    if structured.is_object() {
        serde_json::from_value(structured.clone()).map_err(|e| e.to_string())
    } else {
        from_reply(json["result"].as_str().unwrap_or_default())
            .ok_or_else(|| "Claude's reply had no findings to list".to_string())
    }
}

/// Findings from the last ```json block of a reply (interactive reviews), or
/// from the reply itself when it's only JSON.
pub fn from_reply(text: &str) -> Option<Findings> {
    if let Ok(f) = serde_json::from_str::<Findings>(text.trim()) {
        return Some(f);
    }
    let start = text.rfind("```json")? + "```json".len();
    let end = text[start..].find("```")? + start;
    serde_json::from_str(text[start..end].trim()).ok()
}

// ---- Saved reviews -----------------------------------------------------------

fn store_dir() -> Option<PathBuf> {
    crate::library::data_dir().map(|dir| dir.join("reviews"))
}

pub fn save(review: &Review) -> Result<(), String> {
    let dir = store_dir().ok_or("Could not find the data directory")?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let text = serde_json::to_vec_pretty(review).map_err(|e| e.to_string())?;
    fs::write(dir.join(format!("{}.json", slug(&review.session_id))), text)
        .map_err(|e| e.to_string())
}

/// Every saved review, newest first.
fn all() -> Vec<Review> {
    let mut reviews: Vec<Review> = store_dir()
        .and_then(|dir| fs::read_dir(dir).ok())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| fs::read(e.path()).ok())
        .filter_map(|raw| serde_json::from_slice::<Review>(&raw).ok())
        .collect();
    reviews.sort_by_key(|r| std::cmp::Reverse(r.at));
    reviews
}

/// Saved reviews of a repository, newest first.
pub fn saved(repo: &Path) -> Vec<Review> {
    let mut reviews = all();
    reviews.retain(|r| r.repo == repo);
    reviews
}

/// The latest saved reviews of every repository, newest first.
pub fn recent(limit: usize) -> Vec<Review> {
    let mut reviews = all();
    reviews.truncate(limit);
    reviews
}

/// The review as Markdown, to paste into the pull request or keep.
pub fn markdown(review: &Review) -> String {
    let mut out = format!(
        "# Review of `{}` against `{}`\n\n{}\n",
        review.branch, review.base, review.result.summary
    );
    if review.result.findings.is_empty() {
        out.push_str("\nNothing worth a comment.\n");
    }
    for f in &review.result.findings {
        let place = match f.line {
            Some(line) => format!("{}:{line}", f.file),
            None => f.file.clone(),
        };
        out.push_str(&format!(
            "\n## `{place}` · {}\n\n{}\n",
            f.severity, f.comment
        ));
        if let Some(s) = f.suggestion.as_deref().filter(|s| !s.trim().is_empty()) {
            out.push_str(&format!("\n```suggestion\n{}\n```\n", s.trim_end()));
        }
    }
    out
}

/// A few numbered lines of `file` around `line`, from the review worktree.
pub fn context(worktree: &Path, file: &str, line: u32, around: u32) -> Vec<String> {
    let Ok(text) = fs::read_to_string(worktree.join(file)) else {
        return Vec::new();
    };
    let first = line.saturating_sub(around).max(1);
    text.lines()
        .enumerate()
        .map(|(i, l)| (i as u32 + 1, l))
        .filter(|(n, _)| *n >= first && *n <= line + around)
        .map(|(n, l)| format!("{}{n:>5}  {l}", if n == line { "▶" } else { " " }))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_diff_sizes() {
        let s = parse_shortstat(" 8 files changed, 340 insertions(+), 90 deletions(-)\n", 12);
        assert_eq!(
            s,
            DiffStat {
                commits: 12,
                files: 8,
                insertions: 340,
                deletions: 90
            }
        );
        assert_eq!(
            parse_shortstat(" 1 file changed, 1 deletion(-)", 1).deletions,
            1
        );
    }

    #[test]
    fn reads_findings_from_a_reply() {
        let reply = "Done.\n\n```json\n{\"summary\":\"Adds login.\",\"findings\":[{\"file\":\"src/a.rs\",\"line\":42,\"severity\":\"high\",\"comment\":\"Unchecked token.\"}]}\n```\n";
        let f = from_reply(reply).unwrap();
        assert_eq!(f.findings[0].line, Some(42));
        assert_eq!(
            from_reply("{\"summary\":\"s\",\"findings\":[]}")
                .unwrap()
                .summary,
            "s"
        );
        assert!(from_reply("no json here").is_none());
    }

    #[test]
    fn prompts_ask_interactive_sessions_for_the_json() {
        assert!(!prompt(Mode::Static, "b", "origin/develop").contains("```json"));
        let tests = prompt(Mode::Tests, "b", "origin/develop");
        assert!(tests.contains("run them") && tests.contains("```json"));
        assert!(tests.contains("git diff origin/develop...HEAD"));
        assert_eq!(slug("origin/feature/x y"), "origin-feature-x-y");
    }

    /// A real repository with a remote: lists branches against develop, makes
    /// a detached worktree outside the repository, and leaves the checkout alone.
    #[test]
    fn lists_branches_and_prepares_a_worktree() {
        let root = std::env::temp_dir().join(format!("claudash-review-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (origin, clone) = (root.join("origin"), root.join("clone"));
        fs::create_dir_all(&origin).unwrap();
        let run = |dir: &Path, args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(["-c", "user.email=t@t", "-c", "user.name=t"])
                .args(args)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        };
        if !run(&origin, &["init", "-q", "-b", "main"]) {
            return; // No git on this machine.
        }
        assert!(run(
            &origin,
            &["commit", "-q", "--allow-empty", "-m", "init"]
        ));
        assert!(run(&origin, &["branch", "develop"]));
        assert!(run(&origin, &["checkout", "-q", "-b", "feature/x"]));
        fs::write(origin.join("a.txt"), "one\ntwo\n").unwrap();
        assert!(run(&origin, &["add", "a.txt"]));
        assert!(run(&origin, &["commit", "-q", "-m", "Add a"]));
        assert!(run(&origin, &["checkout", "-q", "main"]));
        assert!(run(
            &root,
            &["clone", "-q", &origin.to_string_lossy(), "clone"]
        ));

        let listing = list(&clone, &clone).unwrap();
        assert_eq!(listing.base.as_deref(), Some("origin/develop"));
        let branch = &listing.branches[0];
        assert_eq!((branch.name.as_str(), branch.ahead), ("feature/x", 1));
        assert!(listing.branches.iter().all(|b| b.name != "develop"));

        // A local branch with unpushed commits comes first.
        assert!(run(&clone, &["checkout", "-q", "-b", "mine"]));
        fs::write(clone.join("b.txt"), "b\n").unwrap();
        assert!(run(&clone, &["add", "b.txt"]));
        assert!(run(&clone, &["commit", "-q", "-m", "Add b"]));
        let with_local = list(&clone, &clone).unwrap();
        assert!(with_local.branches[0].local);
        assert_eq!(with_local.branches[0].name, "mine");
        assert!(run(&clone, &["checkout", "-q", "-"]));

        let stat = diff_stat(&clone, "origin/develop", "origin/feature/x").unwrap();
        assert_eq!((stat.commits, stat.files, stat.insertions), (1, 1, 2));

        // In tests the cache directory is the real one, so use a path in root.
        let wt = root.join("wt");
        let wt_path = wt.to_string_lossy().into_owned();
        git(
            &clone,
            &[
                "worktree",
                "add",
                "--quiet",
                "--detach",
                &wt_path,
                "origin/feature/x",
            ],
        )
        .unwrap();
        assert_eq!(context(&wt, "a.txt", 2, 1), ["     1  one", "▶    2  two"]);
        // The clone's own checkout is untouched.
        assert!(!clone.join("a.txt").exists());
        fs::remove_dir_all(&root).unwrap();
    }
}
