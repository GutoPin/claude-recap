mod config;
mod parse;
mod recap;
mod skills;
mod store;
mod tui;

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::{Command as Process, ExitCode};

use chrono::{Local, Utc};
use clap::{Parser, Subcommand};

use parse::Session;
use store::{Folder, Library, Period, active_since};

/// Browse your Claude Code sessions by folder and see what you did in each.
#[derive(Parser)]
#[command(name = "claude-recap", version = env!("RECAP_VERSION"))]
struct Args {
    /// Claude Code data directory [default: $CLAUDE_CONFIG_DIR or ~/.claude]
    #[arg(long, global = true, value_name = "DIR")]
    dir: Option<PathBuf>,

    /// Print JSON instead of text
    #[arg(long, global = true)]
    json: bool,

    /// Only sessions active in this period; `t` changes it in the interface
    #[arg(long, global = true, value_name = "PERIOD")]
    since: Option<Period>,

    /// Print the recap as Markdown, with `folder` and `summary`
    #[arg(long, global = true)]
    markdown: bool,

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
    /// What you did across all folders in a period [default: --since week]
    Summary,
    /// Installed skills and how often each one was used
    Skills,
    /// Where the config file goes and what it can hold
    Config,
    /// Install the latest version from the main branch on GitHub
    Update,
}

fn main() -> ExitCode {
    match run(Args::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("claude-recap: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Args) -> Result<(), String> {
    if matches!(args.command, Some(Command::Update)) {
        return update();
    }
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
    let period = args.since.unwrap_or_default();
    let since = period.since(now, &Local);
    let render = |rows: Vec<recap::Row>| {
        let text = if args.markdown {
            recap::to_markdown(&rows)
        } else {
            recap::to_text(&rows)
        };
        print!("{text}");
        Ok(())
    };

    match args.command {
        None if args.json => {
            // Skills first: they are found from every folder, not only the ones in the period.
            lib.skills();
            print_json(&within(lib, since))
        }
        None if std::io::stdout().is_terminal() => {
            // Leaked so skills can be found on a thread of their own while the
            // list is already on screen; the process ends with the view anyway.
            let lib: &'static Library = Box::leak(Box::new(lib));
            std::thread::spawn(|| lib.skills());
            let look = config.look(config::truecolor());
            match tui::run(lib, look, period).map_err(|e| e.to_string())? {
                Some(session) => resume(&session, &lib.root),
                None => Ok(()),
            }
        }
        None => {
            print!("{}", recap::folders_table(&within(lib, since), now));
            Ok(())
        }
        Some(Command::Folder { path }) => {
            let found = find_folder(&lib, path.as_deref())?;
            let folder = active_since(std::slice::from_ref(found), since)
                .pop()
                .ok_or_else(|| {
                    format!(
                        "no sessions in {} {}",
                        recap::tilde(&found.path),
                        period.label()
                    )
                })?;
            if args.json {
                return print_json(&folder);
            }
            render(recap::folder_rows(&folder, now, &Local))
        }
        Some(Command::Summary) => {
            let period = args.since.unwrap_or(Period::Week);
            let folders = active_since(&lib.folders, period.since(now, &Local));
            if args.json {
                return print_json(&folders);
            }
            render(recap::period_rows(&folders, period.label(), now, &Local))
        }
        Some(Command::Skills) if args.json => print_json(lib.skills()),
        Some(Command::Skills) => {
            print!("{}", recap::skills_table(&lib));
            Ok(())
        }
        Some(Command::Config | Command::Update) => Ok(()),
    }
}

/// Hands the terminal to `claude --resume`. It runs in the folder the session
/// was started in, because Claude Code looks sessions up by folder.
fn resume(session: &Session, root: &Path) -> Result<(), String> {
    let dir = session
        .cwd
        .as_deref()
        .ok_or("this session has no folder recorded, so it cannot be resumed")?;
    if !Path::new(dir).is_dir() {
        return Err(format!(
            "{dir} no longer exists, so the session cannot be resumed there"
        ));
    }
    // Made absolute because claude runs in another directory.
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut claude = Process::new("claude");
    claude.args(["--resume", &session.id]).current_dir(dir);
    // Only when needed: setting it also moves where Claude Code keeps its login.
    if store::default_dir().and_then(|d| std::fs::canonicalize(d).ok()) != Some(root.clone()) {
        claude.env("CLAUDE_CONFIG_DIR", &root);
    }
    claude
        .status()
        .map(|_| ())
        .map_err(|e| format!("could not start claude: {e}"))
}

/// Reinstalls from the repository with cargo, which skips the build when the
/// installed copy is already at the newest commit.
fn update() -> Result<(), String> {
    println!("claude-recap {}", env!("RECAP_VERSION"));
    let status = Process::new("cargo")
        .args(["install", "--git", env!("CARGO_PKG_REPOSITORY")])
        .status()
        .map_err(|e| {
            format!("could not run cargo ({e}); updating needs Rust, see https://rustup.rs")
        })?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| "cargo install failed".to_string())
}

/// The library with only the sessions active since `since`.
fn within(mut lib: Library, since: Option<chrono::DateTime<Utc>>) -> Library {
    lib.folders = active_since(&lib.folders, since);
    lib
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
        .ok_or_else(|| format!("no sessions recorded for {absolute} (run `claude-recap` to see the folders that have some)"))
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
