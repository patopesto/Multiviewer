// Hide the console window in release builds
// Use the --console flag to re-attach stdout/stderr when launching from a terminal for debugging.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod compositor;
mod config;
mod control;
mod engine;
mod platform;
mod secret;
mod session;
mod sources;
mod startup;
mod ui;

pub const APP_NAME: &str = "Multiviewer";
pub const PROJECT_FILE_EXTENSION: &str = "multiviewer";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    return startup::run(parse_cli(std::env::args().skip(1)));
}

/// Command-line options. Anything unrecognised is ignored.
#[derive(Default)]
struct Cli {
    pub(crate) console: bool,
    pub(crate) headless: bool,
    pub(crate) project: Option<std::path::PathBuf>,
    pub(crate) trace: Option<std::path::PathBuf>,
    pub(crate) exit_after: Option<std::time::Duration>,
}

fn parse_cli(args: impl IntoIterator<Item = String>) -> Cli {
    let mut cli = Cli::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--console" => cli.console = true,
            "--headless" => cli.headless = true,
            "--trace" => cli.trace = args.next().map(std::path::PathBuf::from),
            "--exit-after" => {
                let Some(value) = args.next() else {
                    eprintln!("--exit-after needs a duration in seconds");
                    continue;
                };
                match value.parse::<f32>() {
                    Ok(secs) if secs.is_finite() && secs >= 0.0 => {
                        cli.exit_after = Some(std::time::Duration::from_secs_f32(secs));
                    }
                    _ => eprintln!("ignoring --exit-after {value}: expected seconds"),
                }
            }
            other if cli.project.is_none() && !other.starts_with('-') => {
                cli.project = Some(std::path::PathBuf::from(other));
            }
            _ => {}
        }
    }
    return cli;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn cli(args: &[&str]) -> Cli {
        return parse_cli(args.iter().map(|arg| arg.to_string()));
    }

    #[test]
    fn parses_exit_after_and_project() {
        let parsed = cli(&["--exit-after", "2.5", "show.multiviewer"]);
        assert_eq!(parsed.exit_after, Some(Duration::from_secs_f32(2.5)));
        assert_eq!(
            parsed.project,
            Some(std::path::PathBuf::from("show.multiviewer"))
        );
    }

    #[test]
    fn parses_headless() {
        assert!(cli(&["--headless"]).headless);
        assert!(!cli(&[]).headless);
    }

    #[test]
    fn rejects_missing_or_invalid_exit_after() {
        assert_eq!(cli(&["--exit-after"]).exit_after, None);
        assert_eq!(cli(&["--exit-after", "soon"]).exit_after, None);
        assert_eq!(cli(&["--exit-after", "-1"]).exit_after, None);
        assert_eq!(cli(&["--exit-after", "inf"]).exit_after, None);
    }
}
