mod app;
mod claude_cli;
mod ecosystem;
mod instructions;
mod mcp;
mod paths;
mod sessions;
mod statusline;
mod ui;

/// Context window used when the status line hasn't reported one; override with
/// `--context-limit` or `CLAUDASH_CONTEXT_LIMIT`.
const DEFAULT_CONTEXT_LIMIT: u64 = 1_000_000;
const CONTEXT_LIMIT_ENV: &str = "CLAUDASH_CONTEXT_LIMIT";

const HELP: &str = "\
claudash — control dashboard for Claude Code (unofficial project)

Usage:
  claudash [--context-limit <TOKENS>]    open the dashboard
  claudash statusline [-- <COMMAND>...]  status line command for Claude Code
  claudash statusline --setup            how to enable plan usage in claudash

Options:
  --context-limit <TOKENS>  Context window size used when Claude Code's status
                            line hasn't reported one (e.g. 1M, 200k, 500000).
                            Also set with CLAUDASH_CONTEXT_LIMIT. Default: 1M.
  -h, --help                Print this help.

Views: 1 Dashboard · 2 Ecosystem · 3 Usage. Press ? in the dashboard for keys.";

#[derive(Debug, PartialEq)]
enum Cli {
    Dashboard { context_limit: u64 },
    Help,
    Statusline { setup: bool, wrapped: Vec<String> },
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
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return Ok(Cli::Help),
            "statusline" => {
                let rest: Vec<String> = args.collect();
                return match rest.split_first() {
                    None => Ok(Cli::Statusline {
                        setup: false,
                        wrapped: Vec::new(),
                    }),
                    Some((first, [])) if first == "--setup" => Ok(Cli::Statusline {
                        setup: true,
                        wrapped: Vec::new(),
                    }),
                    Some((first, command)) if first == "--" && !command.is_empty() => {
                        Ok(Cli::Statusline {
                            setup: false,
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
    Ok(Cli::Dashboard { context_limit })
}

fn main() -> std::io::Result<()> {
    // Parsed before taking over the terminal so errors print normally.
    let cli = parse_args(
        std::env::args().skip(1),
        std::env::var(CONTEXT_LIMIT_ENV).ok(),
    );
    let context_limit = match cli {
        Ok(Cli::Dashboard { context_limit }) => context_limit,
        Ok(Cli::Help) => {
            println!("{HELP}");
            return Ok(());
        }
        Ok(Cli::Statusline { setup: true, .. }) => {
            println!("{}", statusline::SETUP_HELP);
            return Ok(());
        }
        Ok(Cli::Statusline { wrapped, .. }) => return statusline::run(&wrapped),
        Err(msg) => {
            eprintln!("claudash: {msg}");
            std::process::exit(2);
        }
    };

    // ratatui::init enables raw mode + the alternate screen and installs a panic hook
    // that restores the terminal; ratatui::restore leaves it clean on exit.
    let mut terminal = ratatui::init();
    let result = app::App::new(context_limit).run(&mut terminal);
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn limit(list: &[&str], env: Option<&str>) -> Result<u64, String> {
        match parse_args(args(list), env.map(str::to_owned))? {
            Cli::Dashboard { context_limit } => Ok(context_limit),
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
    fn parses_statusline_subcommand() {
        let parse = |list: &[&str]| parse_args(args(list), None);
        assert_eq!(parse(&["--help"]), Ok(Cli::Help));
        assert_eq!(
            parse(&["statusline"]),
            Ok(Cli::Statusline {
                setup: false,
                wrapped: vec![]
            })
        );
        assert_eq!(
            parse(&["statusline", "--setup"]),
            Ok(Cli::Statusline {
                setup: true,
                wrapped: vec![]
            })
        );
        assert_eq!(
            parse(&["statusline", "--", "my.sh", "-x"]),
            Ok(Cli::Statusline {
                setup: false,
                wrapped: args(&["my.sh", "-x"])
            })
        );
        assert!(parse(&["statusline", "--bogus"]).is_err());
        assert!(parse(&["statusline", "--"]).is_err());
    }
}
