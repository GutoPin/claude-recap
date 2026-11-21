mod config;
mod parse;
mod recap;
mod skills;
mod store;
mod tui;

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use chrono::{Local, Utc};
use clap::{Parser, Subcommand};

use store::{Folder, Library};

/// Browse your Claude Code sessions by folder and see what you did in each.
#[derive(Parser)]
#[command(name = "recap", version)]
struct Args {
    /// Claude Code data directory [default: $CLAUDE_CONFIG_DIR or ~/.claude]
    #[arg(long, global = true, value_name = "DIR")]
    dir: Option<PathBuf>,

    /// Print JSON instead of text
    #[arg(long, global = true)]
    json: bool,

    /// Read colours from this file [default: ~/.config/claude-recap/config.json]
    #[arg(long, global = true, value_name = "FILE")]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Recap of one folder [default: the current directory]
    Folder {
        /// A path, or just the folder name
        path: Option<String>,
    },
    /// Installed skills and how often each one was used
    Skills,
    /// Where the config file goes and what it can hold
    Config,
}

fn main() -> ExitCode {
    match run(Args::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("recap: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Args) -> Result<(), String> {
    let config_path = args.config.or_else(config::path);
    if matches!(args.command, Some(Command::Config)) {
        let path = config_path.map_or("(no home directory)".into(), |p| p.display().to_string());
        print!(
            "Config file: {path}\n\nDefaults, as the file would be written:\n\n{}",
            config::Config::default().json()
        );
        return Ok(());
    }
    let config = match &config_path {
        Some(path) => config::load(path)?,
        None => config::Config::default(),
    };
    let root = store::config_dir(args.dir)?;
    let lib = store::load(&root)?;
    let now = Utc::now();

    match args.command {
        None if args.json => print_json(&lib),
        None if std::io::stdout().is_terminal() => {
            tui::run(&lib, config.look(config::truecolor())).map_err(|e| e.to_string())
        }
        None => {
            print!("{}", recap::folders_table(&lib, now));
            Ok(())
        }
        Some(Command::Folder { path }) => {
            let folder = find_folder(&lib, path.as_deref())?;
            if args.json {
                return print_json(folder);
            }
            print!(
                "{}",
                recap::to_text(&recap::folder_rows(folder, now, &Local))
            );
            Ok(())
        }
        Some(Command::Skills) if args.json => print_json(&lib.skills),
        Some(Command::Skills) => {
            print!("{}", recap::skills_table(&lib));
            Ok(())
        }
        Some(Command::Config) => Ok(()),
    }
}

fn print_json<T: serde::Serialize + ?Sized>(value: &T) -> Result<(), String> {
    let json = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    println!("{json}");
    Ok(())
}

/// Finds a folder by path, or by name when the argument is not a path with
/// recorded sessions.
fn find_folder<'a>(lib: &'a Library, arg: Option<&str>) -> Result<&'a Folder, String> {
    let wanted = arg.unwrap_or(".");
    let absolute =
        std::fs::canonicalize(wanted).unwrap_or_else(|_| Path::new(wanted).to_path_buf());
    let absolute = absolute.to_string_lossy();
    lib.folders
        .iter()
        .find(|f| f.path == absolute.as_ref())
        .or_else(|| lib.folders.iter().find(|f| f.name() == wanted))
        .ok_or_else(|| format!("no sessions recorded for {absolute} (run `recap` to see the folders that have some)"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{load, tests::fixture};

    #[test]
    fn finds_a_folder_by_path_or_name() {
        let fx = fixture();
        let lib = load(&fx.0).unwrap();
        assert_eq!(
            find_folder(&lib, Some("/work/shop")).unwrap().path,
            "/work/shop"
        );
        assert_eq!(
            find_folder(&lib, Some("notes")).unwrap().path,
            "/work/notes"
        );
        let err = find_folder(&lib, Some("/work/unknown")).unwrap_err();
        assert!(
            err.contains("no sessions recorded for /work/unknown"),
            "{err}"
        );
    }
}
