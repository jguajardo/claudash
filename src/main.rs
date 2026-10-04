mod analysis;
mod app;
mod audit;
mod claude_cli;
mod config;
mod doctor;
mod ecosystem;
mod git;
mod help;
mod history;
mod hooks;
mod instructions;
mod keys;
mod library;
mod mcp;
mod notify;
mod paths;
mod permissions;
mod plan;
mod pricing;
mod projects;
mod prompts;
mod quota;
mod report;
mod review;
mod sessions;
mod setup;
mod snapshots;
mod specs;
mod statusline;
mod stopped;
mod summary;
mod transcript;
mod ui;
mod wrapped;

/// Context window used when the status line hasn't reported one; override with
/// `--context-limit` or `CLAUDASH_CONTEXT_LIMIT`.
const DEFAULT_CONTEXT_LIMIT: u64 = 1_000_000;
const CONTEXT_LIMIT_ENV: &str = "CLAUDASH_CONTEXT_LIMIT";

const HELP: &str = "\
claudash — the control room for Claude Code (unofficial project)

Usage:
  claudash [OPTIONS]                     open the dashboard
  claudash status [--json]               one line for status bars: sessions that
                                         need you, working, waiting; plan usage
  claudash summary [-o FILE]             today's summary as Markdown
  claudash usage [daily|monthly|projects|models|sessions] [--since DATE] [--json]
                                         tokens and API-equivalent dollars
  claudash wrapped [week|month] [--redact]  your week or month on one card to share
                                         (--plain without colors)
  claudash quota [--json]                where the current 5-hour and 7-day windows went,
                                         by session, project and model
  claudash plan [--json]                 is your plan worth it: your use at API prices
                                         against its price, and how often each plan
                                         would have stopped you
  claudash continue [--wait] [--dry-run] continue sessions a plan limit stopped, in
                                         the background, once it has reset; --wait
                                         waits for the reset first
  claudash export <SESSION-ID> [-o FILE] a conversation as Markdown (stdout
                                         without -o)
  claudash setup [--apply | --remove]    connect claudash to Claude Code (status
                                         line + hooks); shows the changes first
  claudash doctor                        check claudash's connection to Claude Code
  claudash config [--init]               show the settings file and its values;
                                         --init writes a commented one
  claudash statusline [-- <COMMAND>...]  status line command for Claude Code
  claudash hook                          hook command for Claude Code

Options:
  --view <NAME>             View to open on: now (default), sessions, projects or
                            insights.
  --context-limit <TOKENS>  Context window size used when Claude Code's status
                            line hasn't reported one (e.g. 1M, 200k, 500000).
                            Also set with CLAUDASH_CONTEXT_LIMIT. Default: 1M.
  --no-notify               No desktop notifications or bell.
  -h, --help                Print this help.
  -V, --version             Print the version.

Flags win over CLAUDASH_CONTEXT_LIMIT, which wins over the settings file.
In the dashboard, press : to find any action by name, ? for help.";

#[derive(Debug, PartialEq)]
enum Cli {
    Dashboard {
        context_limit: u64,
        notify: bool,
        /// Index into `app::VIEW_KEYS`.
        view: usize,
    },
    Config {
        init: bool,
    },
    Status {
        json: bool,
    },
    Summary {
        output: Option<String>,
    },
    Usage(report::UsageArgs),
    Quota {
        json: bool,
    },
    Continue {
        wait: bool,
        dry_run: bool,
    },
    Plan {
        json: bool,
    },
    Wrapped {
        month: bool,
        redact: bool,
        plain: bool,
    },
    Help,
    Version,
    Setup(setup::Mode),
    Statusline {
        wrapped: Vec<String>,
    },
    Hook,
    Doctor,
    Export {
        id: String,
        output: Option<String>,
    },
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

/// Precedence: flags > environment variable > settings file > defaults.
fn parse_args(
    args: impl IntoIterator<Item = String>,
    env: Option<String>,
    config: &config::Config,
) -> Result<Cli, String> {
    let mut args = args.into_iter();
    let mut flag = None;
    let mut notify = config.notify.unwrap_or(true);
    let mut view = config.view.as_deref().map_or(Ok(0), config::view_index)?;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(Cli::Help),
            "-V" | "--version" => return Ok(Cli::Version),
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
            "doctor" => return Ok(Cli::Doctor),
            "config" => {
                let rest: Vec<String> = args.collect();
                return match rest.iter().map(String::as_str).collect::<Vec<_>>()[..] {
                    [] => Ok(Cli::Config { init: false }),
                    ["--init"] => Ok(Cli::Config { init: true }),
                    _ => Err(format!("usage: claudash config [--init]\n\n{HELP}")),
                };
            }
            "status" => {
                let rest: Vec<String> = args.collect();
                return match rest.iter().map(String::as_str).collect::<Vec<_>>()[..] {
                    [] => Ok(Cli::Status { json: false }),
                    ["--json"] => Ok(Cli::Status { json: true }),
                    _ => Err(format!("usage: claudash status [--json]\n\n{HELP}")),
                };
            }
            "summary" => {
                let rest: Vec<String> = args.collect();
                return match rest.iter().map(String::as_str).collect::<Vec<_>>()[..] {
                    [] => Ok(Cli::Summary { output: None }),
                    ["-o", file] => Ok(Cli::Summary {
                        output: Some(file.to_string()),
                    }),
                    _ => Err(format!("usage: claudash summary [-o FILE]\n\n{HELP}")),
                };
            }
            "wrapped" => {
                let (mut month, mut redact, mut plain) = (false, false, false);
                for arg in args.by_ref() {
                    match arg.as_str() {
                        "week" => month = false,
                        "month" => month = true,
                        "--redact" => redact = true,
                        "--plain" => plain = true,
                        _ => {
                            return Err(format!(
                                "usage: claudash wrapped [week|month] [--redact] [--plain]\n\n{HELP}"
                            ));
                        }
                    }
                }
                return Ok(Cli::Wrapped {
                    month,
                    redact,
                    plain,
                });
            }
            "quota" => {
                let rest: Vec<String> = args.collect();
                return match rest.iter().map(String::as_str).collect::<Vec<_>>()[..] {
                    [] => Ok(Cli::Quota { json: false }),
                    ["--json"] => Ok(Cli::Quota { json: true }),
                    _ => Err(format!("usage: claudash quota [--json]\n\n{HELP}")),
                };
            }
            "plan" => {
                let rest: Vec<String> = args.collect();
                return match rest.iter().map(String::as_str).collect::<Vec<_>>()[..] {
                    [] => Ok(Cli::Plan { json: false }),
                    ["--json"] => Ok(Cli::Plan { json: true }),
                    _ => Err(format!("usage: claudash plan [--json]\n\n{HELP}")),
                };
            }
            "continue" => {
                let (mut wait, mut dry_run) = (false, false);
                for arg in args {
                    match arg.as_str() {
                        "--wait" => wait = true,
                        "--dry-run" => dry_run = true,
                        _ => {
                            return Err(format!(
                                "usage: claudash continue [--wait] [--dry-run]\n\n{HELP}"
                            ));
                        }
                    }
                }
                return Ok(Cli::Continue { wait, dry_run });
            }
            "usage" => {
                let rest: Vec<String> = args.collect();
                return report::UsageArgs::parse(&rest)
                    .map(Cli::Usage)
                    .map_err(|e| format!("{e}\n\n{HELP}"));
            }
            "--view" => {
                view = config::view_index(&args.next().ok_or("missing value for --view")?)?;
            }
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
            other => {
                if let Some(value) = other.strip_prefix("--context-limit=") {
                    flag = Some(value.to_string());
                } else if let Some(value) = other.strip_prefix("--view=") {
                    view = config::view_index(value)?;
                } else {
                    return Err(format!("unknown argument: '{other}'\n\n{HELP}"));
                }
            }
        }
    }
    let context_limit = flag
        .or(env)
        .or_else(|| config.context_limit.as_ref().map(config::Limit::as_text))
        .map_or(Ok(DEFAULT_CONTEXT_LIMIT), |v| parse_token_count(&v))?;
    Ok(Cli::Dashboard {
        context_limit,
        notify,
        view,
    })
}

fn main() -> std::io::Result<()> {
    // A broken settings file must never break the status line or hooks, which
    // Claude Code runs; only the dashboard and `claudash config` report it.
    let settings = config::load();
    let defaults = config::Config::default();
    // Parsed before taking over the terminal so errors print normally.
    let cli = parse_args(
        std::env::args().skip(1),
        std::env::var(CONTEXT_LIMIT_ENV).ok(),
        settings.as_ref().unwrap_or(&defaults),
    );
    if let (Err(e), Ok(Cli::Dashboard { .. } | Cli::Config { init: false })) = (&settings, &cli) {
        eprintln!("claudash: {e}");
        std::process::exit(2);
    }
    let (context_limit, notify, view) = match cli {
        Ok(Cli::Dashboard {
            context_limit,
            notify,
            view,
        }) => (context_limit, notify, view),
        Ok(Cli::Config { init: true }) => {
            return exit_on_error(
                "config",
                config::init().map(|path| println!("Wrote {}", path.display())),
            );
        }
        Ok(Cli::Config { init: false }) => {
            show_config();
            return Ok(());
        }
        Ok(Cli::Status { json }) => return exit_on_error("status", report::status(json)),
        Ok(Cli::Summary { output }) => {
            return exit_on_error("summary", report::summary(output.as_deref()));
        }
        Ok(Cli::Usage(usage)) => return exit_on_error("usage", report::usage(&usage)),
        Ok(Cli::Quota { json }) => return exit_on_error("quota", report::quota(json)),
        Ok(Cli::Plan { json }) => return exit_on_error("plan", report::plan(json)),
        Ok(Cli::Continue { wait, dry_run }) => {
            return exit_on_error("continue", report::continue_stopped(wait, dry_run));
        }
        Ok(Cli::Wrapped {
            month,
            redact,
            plain,
        }) => {
            let plain = plain || std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
            return exit_on_error("wrapped", report::wrapped(month, redact, plain));
        }
        Ok(Cli::Version) => {
            println!("claudash {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Ok(Cli::Help) => {
            println!("{HELP}");
            return Ok(());
        }
        Ok(Cli::Setup(mode)) => return exit_on_error("setup", setup::run(mode)),
        Ok(Cli::Statusline { wrapped }) => return statusline::run(&wrapped),
        Ok(Cli::Hook) => return hooks::run(),
        Ok(Cli::Doctor) => {
            doctor::print();
            return Ok(());
        }
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
    let mut app = app::App::new(context_limit, notify);
    // https://no-color.org: any non-empty NO_COLOR turns colors off.
    app.no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty())
        || settings.as_ref().is_ok_and(|s| s.colors == Some(false));
    app.plan = crate::plan::detect(settings.as_ref().ok().and_then(|s| s.plan.as_deref()));
    app.auto_continue = settings
        .as_ref()
        .is_ok_and(|s| s.auto_continue == Some(true));
    app.switch_view(app::VIEW_KEYS[view]);
    let result = app.run(&mut terminal);
    ratatui::restore();
    result
}

/// `claudash config`: where the settings file is and what's in effect.
fn show_config() {
    let path = config::path();
    match &path {
        Some(p) if p.exists() => println!("Settings file: {}", p.display()),
        Some(p) => println!(
            "Settings file: {} (not created; `claudash config --init` writes a commented one)",
            p.display()
        ),
        None => println!("Settings file: none (no config directory)"),
    }
    let defaults = config::Config::default();
    let settings = config::load().unwrap_or(defaults);
    if let Ok(Cli::Dashboard {
        context_limit,
        notify,
        view,
    }) = parse_args(Vec::new(), std::env::var(CONTEXT_LIMIT_ENV).ok(), &settings)
    {
        println!();
        println!("  view           {}", config::VIEW_NAMES[view]);
        println!("  context_limit  {}", sessions::human_tokens(context_limit));
        println!("  notify         {}", if notify { "on" } else { "off" });
    }
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
    let (markdown, removed) = audit::redact(&app::session_markdown(session)?);
    if removed > 0 {
        eprintln!("Removed {removed} secret(s) from the export (API keys, tokens or private keys)");
    }
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
        match parse_args(
            args(list),
            env.map(str::to_owned),
            &config::Config::default(),
        )? {
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
    fn settings_file_loses_to_flags_and_environment() {
        let settings = config::Config {
            view: Some("sessions".into()),
            context_limit: Some(config::Limit::Text("200k".into())),
            notify: Some(false),
            colors: None,
            snapshots: None,
            auto_continue: None,
            plan: None,
        };
        let parse = |list: &[&str], env: Option<&str>| {
            parse_args(args(list), env.map(str::to_owned), &settings)
        };
        assert_eq!(
            parse(&[], None),
            Ok(Cli::Dashboard {
                context_limit: 200_000,
                notify: false,
                view: 1,
            })
        );
        assert!(matches!(
            parse(&["--view", "insights"], Some("300k")),
            Ok(Cli::Dashboard {
                context_limit: 300_000,
                view: 3,
                ..
            })
        ));
        assert!(matches!(
            parse(&["--context-limit", "2M"], Some("300k")),
            Ok(Cli::Dashboard {
                context_limit: 2_000_000,
                ..
            })
        ));
    }

    #[test]
    fn parses_subcommands() {
        let parse = |list: &[&str]| parse_args(args(list), None, &config::Config::default());
        assert_eq!(parse(&["--help"]), Ok(Cli::Help));
        assert_eq!(parse(&["--version"]), Ok(Cli::Version));
        assert_eq!(
            parse(&["--no-notify"]),
            Ok(Cli::Dashboard {
                context_limit: DEFAULT_CONTEXT_LIMIT,
                notify: false,
                view: 0,
            })
        );
        assert!(matches!(
            parse(&["--view", "projects"]),
            Ok(Cli::Dashboard { view: 2, .. })
        ));
        assert!(matches!(
            parse(&["--view=insights"]),
            Ok(Cli::Dashboard { view: 3, .. })
        ));
        assert!(parse(&["--view", "nope"]).is_err());
        assert_eq!(parse(&["config"]), Ok(Cli::Config { init: false }));
        assert_eq!(parse(&["config", "--init"]), Ok(Cli::Config { init: true }));
        assert_eq!(parse(&["status", "--json"]), Ok(Cli::Status { json: true }));
        assert_eq!(
            parse(&["summary", "-o", "day.md"]),
            Ok(Cli::Summary {
                output: Some("day.md".into())
            })
        );
        assert!(parse(&["summary", "extra"]).is_err());
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
        assert_eq!(parse(&["doctor"]), Ok(Cli::Doctor));
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
