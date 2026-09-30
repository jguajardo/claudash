mod app;
mod claude_cli;
mod ecosystem;
mod history;
mod hooks;
mod instructions;
mod library;
mod mcp;
mod notify;
mod paths;
mod sessions;
mod setup;
mod statusline;
mod transcript;
mod ui;

/// Context window used when the status line hasn't reported one; override with
/// `--context-limit` or `CLAUDASH_CONTEXT_LIMIT`.
const DEFAULT_CONTEXT_LIMIT: u64 = 1_000_000;
const CONTEXT_LIMIT_ENV: &str = "CLAUDASH_CONTEXT_LIMIT";

const HELP: &str = "\
claudash — control dashboard for Claude Code (unofficial project)

Usage:
  claudash [OPTIONS]                     open the dashboard
  claudash setup [--apply | --remove]    connect claudash to Claude Code (status
                                         line + hooks); shows the changes first
  claudash statusline [-- <COMMAND>...]  status line command for Claude Code
  claudash hook                          hook command for Claude Code
  claudash export <SESSION-ID> [-o FILE] a conversation as Markdown (stdout
                                         without -o)

Options:
  --context-limit <TOKENS>  Context window size used when Claude Code's status
                            line hasn't reported one (e.g. 1M, 200k, 500000).
                            Also set with CLAUDASH_CONTEXT_LIMIT. Default: 1M.
  --no-notify               No desktop notifications or bell.
  -h, --help                Print this help.

Views: 1 Dashboard · 2 Ecosystem · 3 Usage. Press ? in the dashboard for keys.";

#[derive(Debug, PartialEq)]
enum Cli {
    Dashboard { context_limit: u64, notify: bool },
    Help,
    Setup(setup::Mode),
    Statusline { wrapped: Vec<String> },
    Hook,
    Export { id: String, output: Option<String> },
}

/// Accepts "1M", "200k", "500000" (any case, optional '_' separators).
fn parse_token_count(input: &str) -> Result<u64, String> {
    let clean = input.trim().replace('_', "").to_lowercase();
    let (digits, multiplier) = match clean.strip_suffix('m') {
        Some(d) => (d, 1_000_000.0),
        None => match clean.strip_suffix('k') {
            Some(d) => (d, 1_000.0),
            None => (clean.as_str(), 1.0),
        },
    };
    match digits.parse::<f64>() {
        Ok(n) if n > 0.0 => Ok((n * multiplier).round() as u64),
        _ => Err(format!(
            "invalid context window value: '{input}' (use e.g. 1M, 200k or 500000)"
        )),
    }
}

/// Context limit precedence: `--context-limit` flag > environment variable > default.
fn parse_args(args: impl IntoIterator<Item = String>, env: Option<String>) -> Result<Cli, String> {
    let mut args = args.into_iter();
    let mut flag = None;
    let mut notify = true;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(Cli::Help),
            "--no-notify" => notify = false,
            "setup" => {
                let rest: Vec<String> = args.collect();
                return match rest.iter().map(String::as_str).collect::<Vec<_>>()[..] {
                    [] => Ok(Cli::Setup(setup::Mode::Show)),
                    ["--apply"] => Ok(Cli::Setup(setup::Mode::Apply)),
                    ["--remove"] => Ok(Cli::Setup(setup::Mode::Remove)),
                    _ => Err(format!("unexpected setup arguments\n\n{HELP}")),
                };
            }
            "hook" => return Ok(Cli::Hook),
            "export" => {
                let rest: Vec<String> = args.collect();
                return match rest.iter().map(String::as_str).collect::<Vec<_>>()[..] {
                    [id] => Ok(Cli::Export {
                        id: id.to_string(),
                        output: None,
                    }),
                    [id, "-o", file] | ["-o", file, id] => Ok(Cli::Export {
                        id: id.to_string(),
                        output: Some(file.to_string()),
                    }),
                    _ => Err(format!(
                        "usage: claudash export <SESSION-ID> [-o FILE]\n\n{HELP}"
                    )),
                };
            }
            "statusline" => {
                let rest: Vec<String> = args.collect();
                return match rest.split_first() {
                    None => Ok(Cli::Statusline {
                        wrapped: Vec::new(),
                    }),
                    // Kept from v0.2.0; `claudash setup` replaces it.
                    Some((first, [])) if first == "--setup" => Ok(Cli::Setup(setup::Mode::Show)),
                    Some((first, command)) if first == "--" && !command.is_empty() => {
                        Ok(Cli::Statusline {
                            wrapped: command.to_vec(),
                        })
                    }
                    Some(_) => Err(format!("unexpected statusline arguments\n\n{HELP}")),
                };
            }
            "--context-limit" => {
                flag = Some(args.next().ok_or("missing value for --context-limit")?);
            }
            other => match other.strip_prefix("--context-limit=") {
                Some(value) => flag = Some(value.to_string()),
                None => return Err(format!("unknown argument: '{other}'\n\n{HELP}")),
            },
        }
    }
    let context_limit = flag
        .or(env)
        .map_or(Ok(DEFAULT_CONTEXT_LIMIT), |v| parse_token_count(&v))?;
    Ok(Cli::Dashboard {
        context_limit,
        notify,
    })
}

fn main() -> std::io::Result<()> {
    // Parsed before taking over the terminal so errors print normally.
    let cli = parse_args(
        std::env::args().skip(1),
        std::env::var(CONTEXT_LIMIT_ENV).ok(),
    );
    let (context_limit, notify) = match cli {
        Ok(Cli::Dashboard {
            context_limit,
            notify,
        }) => (context_limit, notify),
        Ok(Cli::Help) => {
            println!("{HELP}");
            return Ok(());
        }
        Ok(Cli::Setup(mode)) => return exit_on_error("setup", setup::run(mode)),
        Ok(Cli::Statusline { wrapped }) => return statusline::run(&wrapped),
        Ok(Cli::Hook) => return hooks::run(),
        Ok(Cli::Export { id, output }) => {
            return exit_on_error("export", export(&id, output.as_deref()));
        }
        Err(msg) => {
            eprintln!("claudash: {msg}");
            std::process::exit(2);
        }
    };

    // ratatui::init enables raw mode + the alternate screen and installs a panic hook
    // that restores the terminal; ratatui::restore leaves it clean on exit.
    let mut terminal = ratatui::init();
    let result = app::App::new(context_limit, notify).run(&mut terminal);
    ratatui::restore();
    result
}

/// Prints a subcommand's error once, plainly, and exits with status 1.
fn exit_on_error(command: &str, result: std::io::Result<()>) -> std::io::Result<()> {
    if let Err(e) = result {
        eprintln!("claudash {command}: {e}");
        std::process::exit(1);
    }
    Ok(())
}

/// `claudash export`: finds the session by ID (or ID prefix) and writes it.
fn export(id: &str, output: Option<&str>) -> std::io::Result<()> {
    let dir = paths::projects_dir()
        .ok_or_else(|| std::io::Error::other("could not find Claude Code's config directory"))?;
    let all = sessions::load_sessions(&dir, &[])?;
    let matches: Vec<&sessions::Session> = all.iter().filter(|s| s.id.starts_with(id)).collect();
    let session = match matches[..] {
        [one] => one,
        [] => return Err(std::io::Error::other(format!("no session with ID {id}"))),
        _ => {
            return Err(std::io::Error::other(format!(
                "{id} matches several sessions"
            )));
        }
    };
    let markdown = app::session_markdown(session)?;
    match output {
        Some(file) => {
            std::fs::write(file, markdown)?;
            eprintln!("Wrote {file}");
            Ok(())
        }
        None => {
            use std::io::Write;
            std::io::stdout().write_all(markdown.as_bytes())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn limit(list: &[&str], env: Option<&str>) -> Result<u64, String> {
        match parse_args(args(list), env.map(str::to_owned))? {
            Cli::Dashboard { context_limit, .. } => Ok(context_limit),
            other => Err(format!("{other:?}")),
        }
    }

    #[test]
    fn parses_token_counts() {
        assert_eq!(parse_token_count("1M"), Ok(1_000_000));
        assert_eq!(parse_token_count("200k"), Ok(200_000));
        assert_eq!(parse_token_count("1.5m"), Ok(1_500_000));
        assert_eq!(parse_token_count("500_000"), Ok(500_000));
        assert!(parse_token_count("lots").is_err());
        assert!(parse_token_count("0").is_err());
    }

    #[test]
    fn context_limit_precedence() {
        assert_eq!(limit(&[], None), Ok(DEFAULT_CONTEXT_LIMIT));
        assert_eq!(limit(&[], Some("200k")), Ok(200_000));
        assert_eq!(
            limit(&["--context-limit", "500k"], Some("200k")),
            Ok(500_000)
        );
        assert_eq!(limit(&["--context-limit=2M"], None), Ok(2_000_000));
        assert!(limit(&["--context-limit"], None).is_err());
        assert!(limit(&["--foo"], None).is_err());
    }

    #[test]
    fn parses_subcommands() {
        let parse = |list: &[&str]| parse_args(args(list), None);
        assert_eq!(parse(&["--help"]), Ok(Cli::Help));
        assert_eq!(
            parse(&["--no-notify"]),
            Ok(Cli::Dashboard {
                context_limit: DEFAULT_CONTEXT_LIMIT,
                notify: false
            })
        );
        assert_eq!(parse(&["setup"]), Ok(Cli::Setup(setup::Mode::Show)));
        assert_eq!(
            parse(&["setup", "--apply"]),
            Ok(Cli::Setup(setup::Mode::Apply))
        );
        assert_eq!(
            parse(&["setup", "--remove"]),
            Ok(Cli::Setup(setup::Mode::Remove))
        );
        assert!(parse(&["setup", "--bogus"]).is_err());
        assert_eq!(parse(&["hook"]), Ok(Cli::Hook));
        assert_eq!(
            parse(&["export", "abc"]),
            Ok(Cli::Export {
                id: "abc".into(),
                output: None
            })
        );
        assert_eq!(
            parse(&["export", "abc", "-o", "x.md"]),
            Ok(Cli::Export {
                id: "abc".into(),
                output: Some("x.md".into())
            })
        );
        assert!(parse(&["export"]).is_err());
        assert_eq!(
            parse(&["statusline"]),
            Ok(Cli::Statusline { wrapped: vec![] })
        );
        assert_eq!(
            parse(&["statusline", "--setup"]),
            Ok(Cli::Setup(setup::Mode::Show))
        );
        assert_eq!(
            parse(&["statusline", "--", "my.sh", "-x"]),
            Ok(Cli::Statusline {
                wrapped: args(&["my.sh", "-x"])
            })
        );
        assert!(parse(&["statusline", "--bogus"]).is_err());
        assert!(parse(&["statusline", "--"]).is_err());
    }
}
