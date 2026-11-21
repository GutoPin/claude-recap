//! Finds the Claude Code data directory and loads every session in it,
//! grouped by the folder it was started in.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::parse::{Session, Tokens, parse_session};
use crate::skills::{self, Skill};

#[derive(Debug, Serialize)]
pub struct Folder {
    /// Real path of the folder, or the encoded directory name when no
    /// session recorded one.
    pub path: String,
    /// Newest first.
    pub sessions: Vec<Session>,
}

impl Folder {
    pub fn name(&self) -> &str {
        self.path
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .filter(|n| !n.is_empty())
            .unwrap_or(&self.path)
    }

    pub fn last_active(&self) -> Option<DateTime<Utc>> {
        self.sessions.iter().filter_map(|s| s.ended).max()
    }

    pub fn cost_usd(&self) -> Option<f64> {
        let costs: Vec<f64> = self.sessions.iter().filter_map(|s| s.cost_usd).collect();
        (!costs.is_empty()).then(|| costs.iter().sum())
    }

    pub fn tokens(&self) -> Tokens {
        let mut total = Tokens::default();
        self.sessions.iter().for_each(|s| total.add(&s.tokens));
        total
    }
}

#[derive(Debug, Serialize)]
pub struct Library {
    #[serde(skip)]
    pub root: PathBuf,
    /// Most recently active first.
    pub folders: Vec<Folder>,
    pub skills: Vec<Skill>,
}

pub fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// `--dir`, then `CLAUDE_CONFIG_DIR`, then `~/.claude`.
pub fn config_dir(flag: Option<PathBuf>) -> Result<PathBuf, String> {
    let dir = flag
        .or_else(|| {
            std::env::var_os("CLAUDE_CONFIG_DIR")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        })
        .or_else(|| home().map(|h| h.join(".claude")))
        .ok_or("could not find a home directory, pass --dir")?;
    if !dir.join("projects").is_dir() {
        return Err(format!(
            "no Claude Code sessions in {} (pass --dir or set CLAUDE_CONFIG_DIR)",
            dir.display()
        ));
    }
    Ok(dir)
}

pub fn load(root: &Path) -> Result<Library, String> {
    let projects = root.join("projects");
    let entries =
        std::fs::read_dir(&projects).map_err(|e| format!("{}: {e}", projects.display()))?;

    let mut groups: Vec<(String, Vec<PathBuf>)> = Vec::new();
    for entry in entries.flatten() {
        let Ok(files) = std::fs::read_dir(entry.path()) else {
            continue;
        };
        let transcripts: Vec<PathBuf> = files
            .flatten()
            .map(|f| f.path())
            .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
            .collect();
        if !transcripts.is_empty() {
            groups.push((
                entry.file_name().to_string_lossy().into_owned(),
                transcripts,
            ));
        }
    }

    let all: Vec<&PathBuf> = groups.iter().flat_map(|(_, files)| files).collect();
    let mut parsed = parse_all(&all).into_iter();

    let mut folders: Vec<Folder> = groups
        .iter()
        .filter_map(|(key, files)| {
            let mut sessions: Vec<Session> = parsed.by_ref().take(files.len()).flatten().collect();
            if sessions.is_empty() {
                return None;
            }
            sessions.sort_by(|a, b| b.ended.cmp(&a.ended).then_with(|| a.id.cmp(&b.id)));
            // The directory name is the path with every separator turned into
            // a dash, which cannot be undone, so the path comes from a session.
            let path = sessions
                .iter()
                .find_map(|s| s.cwd.clone())
                .unwrap_or_else(|| key.clone());
            Some(Folder { path, sessions })
        })
        .collect();
    folders.sort_by(|a, b| {
        b.last_active()
            .cmp(&a.last_active())
            .then_with(|| a.path.cmp(&b.path))
    });

    let skills = skills::discover(root, &folders);
    Ok(Library {
        root: root.to_path_buf(),
        folders,
        skills,
    })
}

/// Parses transcripts on all cores, keeping the input order.
fn parse_all(files: &[&PathBuf]) -> Vec<Option<Session>> {
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
    let chunk = files.len().div_ceil(workers).max(1);
    std::thread::scope(|scope| {
        let handles: Vec<_> = files
            .chunks(chunk)
            .map(|batch| {
                scope.spawn(move || batch.iter().map(|f| parse_session(f)).collect::<Vec<_>>())
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect()
    })
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::parse::tests::{reply, tool, user, write_jsonl};
    use serde_json::json;
    use std::sync::atomic::{AtomicU32, Ordering};

    static NEXT: AtomicU32 = AtomicU32::new(0);

    /// A made-up Claude Code data directory that removes itself.
    pub struct Fixture(pub PathBuf);

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    pub fn fixture() -> Fixture {
        let id = NEXT.fetch_add(1, Ordering::SeqCst);
        let root =
            std::env::temp_dir().join(format!("claude-recap-store-{}-{id}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let shop = root.join("projects/-work-shop");

        write_jsonl(
            &shop.join("aaaa-1111.jsonl"),
            &[
                user("2026-03-01T10:00:00Z", "/work/shop", "add a cart page"),
                reply(
                    "2026-03-01T10:00:05Z",
                    "/work/shop",
                    "m1",
                    100,
                    json!([
                        tool("t1", "Edit", json!({"file_path": "/work/shop/src/cart.ts"})),
                        tool("t2", "Bash", json!({"command": "npm test"})),
                        tool("t3", "Skill", json!({"skill": "frontend-design"})),
                    ]),
                ),
                user("2026-03-01T10:20:00Z", "/work/shop", "now the checkout"),
                reply(
                    "2026-03-01T10:20:30Z",
                    "/work/shop",
                    "m2",
                    300,
                    json!([
                        tool("t4", "Edit", json!({"file_path": "/work/shop/src/cart.ts"})),
                        tool(
                            "t5",
                            "Write",
                            json!({"file_path": "/work/shop/src/checkout.ts"})
                        ),
                    ]),
                ),
                json!({"type": "ai-title", "aiTitle": "Cart and checkout"}),
                json!({"type": "cost-state", "totalCostUSD": 3.25, "totalDuration": 1_500_000}),
            ],
        );
        write_jsonl(
            &shop.join("bbbb-2222.jsonl"),
            &[
                user(
                    "2026-03-03T09:00:00Z",
                    "/work/shop",
                    "why is the build failing?",
                ),
                reply(
                    "2026-03-03T09:00:10Z",
                    "/work/shop",
                    "m1",
                    40,
                    json!([
                        tool("t1", "Skill", json!({"skill": "tools:debugging"})),
                        tool("t2", "Skill", json!({"skill": "old-skill"})),
                    ]),
                ),
                json!({"type": "cost-state", "totalCostUSD": 0.75, "totalDuration": 20_000}),
            ],
        );
        write_jsonl(
            &root.join("projects/-work-notes/cccc-3333.jsonl"),
            &[
                user("2026-02-10T18:00:00Z", "/work/notes", "summarize my notes"),
                reply(
                    "2026-02-10T18:00:20Z",
                    "/work/notes",
                    "m1",
                    500,
                    json!([{"type": "text", "text": "here"}]),
                ),
            ],
        );
        write_jsonl(
            &root.join("projects/-work-empty/dddd-4444.jsonl"),
            &[json!({"type": "mode", "mode": "normal"})],
        );
        std::fs::create_dir_all(root.join("projects/-work-shop/memory")).unwrap();

        let skill = |dir: PathBuf, body: &str| {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("SKILL.md"), body).unwrap();
        };
        skill(
            root.join("skills/frontend-design"),
            "---\nname: frontend-design\ndescription: Guidance for distinctive visual design.\n---\n\n# Body\n",
        );
        // A skill symlinked into place, the way installers often leave them.
        let linked = root.join("elsewhere/linked");
        skill(
            linked.clone(),
            "---\nname: linked\ndescription: Reached through a symlink.\n---\n",
        );
        #[cfg(unix)]
        std::os::unix::fs::symlink(&linked, root.join("skills/linked")).unwrap();
        skill(
            root.join("skills/unused"),
            "---\nname: unused\ndescription: >\n  Never invoked,\n  spans two lines.\n---\n",
        );
        let plugin = root.join("plugins/cache/market/tools/1.0.0");
        skill(
            plugin.join("skills/debugging"),
            "---\nname: debugging\ndescription: \"Find the root cause: first.\"\n---\n",
        );
        std::fs::write(
            root.join("plugins/installed_plugins.json"),
            json!({"version": 2, "plugins": {"tools@market": [
                {"scope": "user", "installPath": plugin},
                {"scope": "project", "installPath": plugin},
            ]}})
            .to_string(),
        )
        .unwrap();
        Fixture(root)
    }

    #[test]
    fn groups_sessions_by_folder_newest_first() {
        let fx = fixture();
        let lib = load(&fx.0).unwrap();

        let folders: Vec<_> = lib
            .folders
            .iter()
            .map(|f| (f.path.as_str(), f.name(), f.sessions.len()))
            .collect();
        assert_eq!(
            folders,
            [("/work/shop", "shop", 2), ("/work/notes", "notes", 1)]
        );

        let shop = &lib.folders[0];
        let titles: Vec<_> = shop.sessions.iter().map(|s| s.name()).collect();
        assert_eq!(titles, ["why is the build failing?", "Cart and checkout"]);
        assert_eq!(shop.cost_usd(), Some(4.0));
        assert_eq!(shop.tokens().output, 440);
        assert_eq!(
            shop.last_active().unwrap().to_rfc3339(),
            "2026-03-03T09:00:10+00:00"
        );
        assert_eq!(lib.folders[1].cost_usd(), None);
    }

    #[test]
    fn config_dir_must_hold_sessions() {
        let fx = fixture();
        assert_eq!(config_dir(Some(fx.0.clone())), Ok(fx.0.clone()));
        let err = config_dir(Some(fx.0.join("nope"))).unwrap_err();
        assert!(err.contains("no Claude Code sessions"), "{err}");
    }
}
