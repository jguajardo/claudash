//! Where an interactive Claude Code session opens: in claudash's own terminal,
//! or in a new tab or pane of the terminal multiplexer claudash runs in, so
//! claudash stays on screen and several sessions can be open at once.

use std::{path::Path, process::Command};

/// Runs the session: `$1` is a file to create when it ends ("" for none),
/// the rest the command. When the command fails it waits for Enter before
/// the tab closes, so the error can be read. The short pause is for Zellij,
/// which leaves a dead tab behind when a command ends the instant it starts.
const SCRIPT: &str = r#"done=$1; shift; "$@"; code=$?; [ -n "$done" ] && : > "$done"; [ "$code" -eq 0 ] && sleep 0.2 || { printf '\nClaude Code ended with an error. Press Enter to close. '; read -r _; }"#;
/// Longest tab or pane name, in characters.
const NAME_LENGTH: usize = 22;

/// A session to open in a tab or pane.
#[derive(Clone, Copy, Debug)]
pub struct Launch<'a> {
    pub program: &'a str,
    pub args: &'a [String],
    pub cwd: &'a Path,
    /// The tab's or pane's name; see [`tab_name`].
    pub name: &'a str,
    /// Variables of claudash's environment the session needs too: the tab
    /// starts from the multiplexer's environment, not claudash's.
    pub env: &'a [(String, String)],
    /// A file to create when the session ends.
    pub done: Option<&'a Path>,
}

/// A terminal multiplexer claudash can ask for a new tab or pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mux {
    Zellij,
    Tmux,
}

impl Mux {
    /// The multiplexer this process runs in, from the environment (`var`).
    pub fn detect(var: impl Fn(&str) -> Option<String>) -> Option<Mux> {
        if var("TMUX").is_some() {
            Some(Mux::Tmux)
        } else if var("ZELLIJ").is_some() {
            Some(Mux::Zellij)
        } else {
            None
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Mux::Zellij => "Zellij",
            Mux::Tmux => "tmux",
        }
    }

    /// What this multiplexer calls the thing `place` opens.
    pub fn unit(self, place: Place) -> &'static str {
        match (self, place) {
            (_, Place::Pane) => "pane",
            (Mux::Tmux, _) => "window",
            (Mux::Zellij, _) => "tab",
        }
    }
}

/// Where sessions open (`open_in` in the settings file).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Place {
    /// A new tab (a window, in tmux).
    #[default]
    Tab,
    /// A new pane next to claudash.
    Pane,
    /// claudash's own terminal: claudash waits until the session ends.
    Here,
}

impl Place {
    pub fn parse(setting: Option<&str>) -> Place {
        match setting.map(|s| s.trim().to_lowercase()).as_deref() {
            Some("pane") => Place::Pane,
            Some("here") => Place::Here,
            _ => Place::Tab,
        }
    }
}

/// A word quoted for `sh`.
fn quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

/// A short, single-line name for a tab or pane.
pub fn tab_name(title: &str) -> String {
    let clean = title.split_whitespace().collect::<Vec<_>>().join(" ");
    // A leading dash would be read as an option of the multiplexer's.
    let clean = clean.trim_start_matches(['-', ' ']).to_string();
    if clean.is_empty() {
        return "claude".to_string();
    }
    if clean.chars().count() <= NAME_LENGTH {
        return clean;
    }
    let cut: String = clean.chars().take(NAME_LENGTH).collect();
    format!("{}…", cut.trim_end())
}

/// The command that opens `launch` in a new tab or pane of `mux`.
/// `Place::Here` has no such command: it gets a tab.
pub fn command(mux: Mux, place: Place, launch: &Launch) -> Vec<String> {
    let cwd = launch.cwd.to_string_lossy().into_owned();
    // What runs inside: the script, its name, the file to mark, the command.
    let mut inner = vec![
        "sh".to_string(),
        "-c".to_string(),
        SCRIPT.to_string(),
        "claudash".to_string(),
        launch
            .done
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
    ];
    if !launch.env.is_empty() {
        inner.push("env".to_string());
        inner.extend(launch.env.iter().map(|(k, v)| format!("{k}={v}")));
    }
    inner.push(launch.program.to_string());
    inner.extend(launch.args.iter().cloned());
    let words = |items: &[&str]| items.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    match mux {
        Mux::Zellij => {
            let mut command = match place {
                Place::Pane => words(&["zellij", "run"]),
                Place::Tab | Place::Here => words(&["zellij", "action", "new-tab"]),
            };
            command.extend(words(&[
                "--cwd",
                &cwd,
                "--name",
                launch.name,
                "--close-on-exit",
                "--",
            ]));
            command.extend(inner);
            command
        }
        Mux::Tmux => {
            // tmux expands `#…` formats in the folder; `##` is a literal `#`.
            let cwd = cwd.replace('#', "##");
            let mut command = match place {
                Place::Pane => words(&["tmux", "split-window", "-h", "-c", &cwd]),
                Place::Tab | Place::Here => {
                    words(&["tmux", "new-window", "-c", &cwd, "-n", launch.name])
                }
            };
            // One string: tmux before 3.2 passes it to the shell whole.
            let quoted: Vec<String> = inner[2..].iter().map(|w| quote(w)).collect();
            command.push(format!("sh -c {}", quoted.join(" ")));
            command
        }
    }
}

/// Opens `launch` in a new tab or pane. The error is the multiplexer's.
pub fn open(mux: Mux, place: Place, launch: &Launch) -> Result<(), String> {
    let command = command(mux, place, launch);
    let output = Command::new(&command[0])
        .args(&command[1..])
        .output()
        .map_err(|e| format!("could not run {}: {e}", command[0]))?;
    if output.status.success() {
        Ok(())
    } else {
        let error = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if error.is_empty() {
            format!("{} refused", command[0])
        } else {
            error
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn launch<'a>(args: &'a [String], cwd: &'a Path, name: &'a str) -> Launch<'a> {
        Launch {
            program: "/bin/claude",
            args,
            cwd,
            name,
            env: &[],
            done: None,
        }
    }

    #[test]
    fn finds_the_multiplexer_claudash_runs_in() {
        let env = |set: &'static [&'static str]| {
            move |name: &str| set.contains(&name).then(|| "1".to_string())
        };
        assert_eq!(Mux::detect(env(&["ZELLIJ"])), Some(Mux::Zellij));
        assert_eq!(Mux::detect(env(&["TMUX"])), Some(Mux::Tmux));
        // tmux inside Zellij: the innermost one is the one that can open tabs here.
        assert_eq!(Mux::detect(env(&["ZELLIJ", "TMUX"])), Some(Mux::Tmux));
        assert_eq!(Mux::detect(env(&["TERM"])), None);
    }

    #[test]
    fn reads_where_to_open_sessions_from_the_setting() {
        assert_eq!(Place::parse(None), Place::Tab);
        assert_eq!(Place::parse(Some("tab")), Place::Tab);
        assert_eq!(Place::parse(Some(" Pane ")), Place::Pane);
        assert_eq!(Place::parse(Some("here")), Place::Here);
        // An unknown value keeps the default instead of failing to start.
        assert_eq!(Place::parse(Some("window")), Place::Tab);
    }

    #[test]
    fn opens_a_zellij_tab_or_pane_that_stays_when_claude_fails() {
        let args = strings(&["--resume", "abc-123"]);
        let cwd = Path::new("/code/api server");
        let tab = command(Mux::Zellij, Place::Tab, &launch(&args, cwd, "Fix login"));
        assert_eq!(
            tab[..9],
            strings(&[
                "zellij",
                "action",
                "new-tab",
                "--cwd",
                "/code/api server",
                "--name",
                "Fix login",
                "--close-on-exit",
                "--",
            ])
        );
        // The session runs through a small script that waits when claude fails.
        assert_eq!(tab[9..11], strings(&["sh", "-c"]));
        assert!(tab[11].contains("\"$@\"") && tab[11].contains("read"));
        // Its arguments: a name for itself, no file to mark, then the command.
        assert_eq!(
            tab[12..],
            strings(&["claudash", "", "/bin/claude", "--resume", "abc-123"])
        );

        let pane = command(Mux::Zellij, Place::Pane, &launch(&args, cwd, "Fix login"));
        assert_eq!(pane[..2], strings(&["zellij", "run"]));
        assert!(pane.contains(&"--close-on-exit".to_string()));
        assert_eq!(
            pane[pane.len() - 3..],
            strings(&["/bin/claude", "--resume", "abc-123"])
        );
    }

    #[test]
    fn opens_a_tmux_window_or_split_with_one_quoted_command() {
        let args = strings(&["--resume", "abc-123"]);
        let cwd = Path::new("/code/api server");
        let tab = command(Mux::Tmux, Place::Tab, &launch(&args, cwd, "It's done"));
        assert_eq!(
            tab[..6],
            strings(&[
                "tmux",
                "new-window",
                "-c",
                "/code/api server",
                "-n",
                "It's done"
            ])
        );
        // Older tmux runs a single string through the shell, so it's quoted as one.
        assert_eq!(tab.len(), 7);
        assert!(
            tab[6].starts_with("sh -c '") && tab[6].ends_with("'/bin/claude' '--resume' 'abc-123'")
        );

        let pane = command(Mux::Tmux, Place::Pane, &launch(&args, cwd, "x"));
        assert_eq!(pane[..2], strings(&["tmux", "split-window"]));

        // tmux expands `#…` in the folder it's given; a folder may hold one.
        let odd = Path::new("/code/#{pane_id}/x");
        let tab = command(Mux::Tmux, Place::Tab, &launch(&args, odd, "x"));
        assert_eq!(tab[3], "/code/##{pane_id}/x");
    }

    #[test]
    fn carries_claudashs_own_settings_into_the_tab() {
        // The tab starts from the multiplexer's environment, not claudash's.
        let args = strings(&["--resume", "abc-123"]);
        let env = [(
            "CLAUDE_CONFIG_DIR".to_string(),
            "/tmp/other home".to_string(),
        )];
        let done = Path::new("/cache/ended/abc-123");
        let launch = Launch {
            env: &env,
            done: Some(done),
            ..launch(&args, Path::new("/code"), "x")
        };
        let tab = command(Mux::Zellij, Place::Tab, &launch);
        assert_eq!(
            tab[12..],
            strings(&[
                "claudash",
                "/cache/ended/abc-123",
                "env",
                "CLAUDE_CONFIG_DIR=/tmp/other home",
                "/bin/claude",
                "--resume",
                "abc-123",
            ])
        );
        // The script marks that file when the session ends, however it ends.
        assert!(tab[11].contains(": > \"$done\""));
    }

    #[test]
    fn quotes_words_for_the_shell() {
        assert_eq!(quote("plain"), "'plain'");
        assert_eq!(quote("it's"), "'it'\\''s'");
        assert_eq!(quote(""), "''");
    }

    #[test]
    fn names_tabs_briefly() {
        assert_eq!(tab_name("Fix login"), "Fix login");
        assert_eq!(
            tab_name("Add OAuth login to the public API server"),
            "Add OAuth login to the…"
        );
        assert_eq!(tab_name("line\nbreak\ttab"), "line break tab");
        assert_eq!(tab_name("  "), "claude");
        // A leading dash would read as an option of the multiplexer's.
        assert_eq!(tab_name("--help me"), "help me");
        assert_eq!(tab_name("-"), "claude");
    }
}
