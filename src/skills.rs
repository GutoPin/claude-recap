//! Lists installed skills and counts how often each one was used.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::store::{Folder, par_map};

const PLUGIN_SEARCH_DEPTH: usize = 4;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    User,
    Plugin(String),
    Project(String),
    /// Used in a session but not installed any more, or built into Claude Code.
    Missing,
}

impl Source {
    pub fn label(&self) -> String {
        match self {
            Source::User => "user".to_string(),
            Source::Plugin(name) => format!("plugin {name}"),
            Source::Project(name) => format!("project {name}"),
            Source::Missing => "built in or removed".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub source: Source,
    pub file: Option<PathBuf>,
    pub uses: u32,
    pub last_used: Option<DateTime<Utc>>,
    /// Folder name to number of uses there.
    pub used_in: BTreeMap<String, u32>,
}

#[derive(Deserialize)]
struct Installed {
    #[serde(default)]
    plugins: BTreeMap<String, Vec<Install>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Install {
    install_path: Option<PathBuf>,
}

/// Most used first, then by name.
pub fn discover(root: &Path, folders: &[Folder]) -> Vec<Skill> {
    // Collected first and read together, because project folders can sit on
    // a slow mount (a Windows drive under WSL) where each file costs ~20 ms.
    let mut wanted: Vec<(PathBuf, String, Source)> = Vec::new();
    for file in skill_files(&root.join("skills"), 1) {
        wanted.push((file, String::new(), Source::User));
    }
    let installed = std::fs::read_to_string(root.join("plugins/installed_plugins.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Installed>(&text).ok());
    for (key, installs) in installed.map(|i| i.plugins).unwrap_or_default() {
        let plugin = key.split('@').next().unwrap_or(&key).to_string();
        for dir in installs.into_iter().filter_map(|i| i.install_path) {
            for file in skill_files(&dir, PLUGIN_SEARCH_DEPTH) {
                wanted.push((file, format!("{plugin}:"), Source::Plugin(plugin.clone())));
            }
        }
    }
    let project_files = par_map(folders, |folder| {
        skill_files(&Path::new(&folder.path).join(".claude/skills"), 1)
    });
    for (folder, files) in folders.iter().zip(project_files) {
        for file in files {
            wanted.push((
                file,
                String::new(),
                Source::Project(folder.name().to_string()),
            ));
        }
    }
    let texts = par_map(&wanted, |(file, _, _)| std::fs::read_to_string(file).ok());

    let mut found: BTreeMap<String, Skill> = BTreeMap::new();
    for ((file, prefix, source), text) in wanted.into_iter().zip(texts) {
        let Some(text) = text else { continue };
        let (name, description) = front_matter(&text);
        let name = name.unwrap_or_else(|| {
            file.parent()
                .and_then(|d| d.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
        let name = format!("{prefix}{name}");
        found.entry(name.clone()).or_insert(Skill {
            name,
            description: description.unwrap_or_default(),
            source,
            file: Some(file),
            uses: 0,
            last_used: None,
            used_in: BTreeMap::new(),
        });
    }

    for folder in folders {
        for session in &folder.sessions {
            for (name, &uses) in &session.skills {
                let skill = found.entry(name.clone()).or_insert_with(|| Skill {
                    name: name.clone(),
                    description: String::new(),
                    source: Source::Missing,
                    file: None,
                    uses: 0,
                    last_used: None,
                    used_in: BTreeMap::new(),
                });
                skill.uses += uses;
                skill.last_used = skill.last_used.max(session.ended);
                *skill.used_in.entry(folder.name().to_string()).or_default() += uses;
            }
        }
    }

    let mut skills: Vec<Skill> = found.into_values().collect();
    skills.sort_by(|a, b| b.uses.cmp(&a.uses).then_with(|| a.name.cmp(&b.name)));
    skills
}

/// Every `SKILL.md` that sits `depth` or fewer directories below `dir`.
fn skill_files(dir: &Path, depth: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        // Skills are often symlinked into place, so links are followed.
        .filter(|e| e.path().is_dir())
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            // Some plugins ship their skills under `.claude/skills`.
            (!name.starts_with('.') || name == ".claude") && name != "node_modules"
        })
        .map(|e| e.path())
        .collect();
    dirs.sort();
    for sub in dirs {
        let file = sub.join("SKILL.md");
        if file.is_file() {
            out.push(file);
        } else if depth > 1 {
            out.extend(skill_files(&sub, depth - 1));
        }
    }
    out
}

/// Reads `name` and `description` from the YAML block that opens a skill file.
/// This handles the shapes skills use in practice (plain, quoted and folded
/// values) without pulling in a YAML parser.
fn front_matter(text: &str) -> (Option<String>, Option<String>) {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return (None, None);
    }
    let block: Vec<&str> = lines.take_while(|l| l.trim() != "---").collect();

    let value = |key: &str| -> Option<String> {
        let at = block
            .iter()
            .position(|l| l.starts_with(&format!("{key}:")))?;
        let first = block[at][key.len() + 1..].trim();
        let text = if first.is_empty() || first.starts_with('>') || first.starts_with('|') {
            block[at + 1..]
                .iter()
                .take_while(|l| l.starts_with(' ') || l.starts_with('\t') || l.trim().is_empty())
                .map(|l| l.trim())
                .filter(|l| !l.is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            let unquoted = first
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .or_else(|| first.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')));
            unquoted.unwrap_or(first).to_string()
        };
        (!text.is_empty()).then_some(text)
    };
    (value("name"), value("description"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{load, tests::fixture};

    #[test]
    fn lists_installed_skills_with_their_usage() {
        let fx = fixture();
        let lib = load(&fx.0).unwrap();
        let rows: Vec<_> = lib
            .skills()
            .iter()
            .map(|s| (s.name.as_str(), s.source.label(), s.uses))
            .collect();
        assert_eq!(
            rows,
            [
                ("frontend-design", "user".to_string(), 1),
                ("old-skill", "built in or removed".to_string(), 1),
                ("tools:debugging", "plugin tools".to_string(), 1),
                ("linked", "user".to_string(), 0),
                ("unused", "user".to_string(), 0),
            ]
        );

        let design = &lib.skills()[0];
        assert_eq!(
            design.description,
            "Guidance for distinctive visual design."
        );
        assert_eq!(
            design.last_used.unwrap().to_rfc3339(),
            "2026-03-01T10:20:30+00:00"
        );
        assert_eq!(design.used_in, BTreeMap::from([("shop".to_string(), 1)]));
        assert!(
            design
                .file
                .as_ref()
                .unwrap()
                .ends_with("skills/frontend-design/SKILL.md")
        );
        assert_eq!(lib.skills()[2].description, "Find the root cause: first.");
        assert_eq!(
            lib.skills()[4].description,
            "Never invoked, spans two lines."
        );
        assert!(lib.skills()[1].file.is_none());
    }

    #[test]
    fn front_matter_shapes() {
        assert_eq!(front_matter("# No front matter\nname: x\n"), (None, None));
        assert_eq!(
            front_matter("---\nname: 'a b'\n---\n"),
            (Some("a b".into()), None)
        );
        assert_eq!(
            front_matter("---\ndescription: |\n  one\n\n  two\nname: n\n---\nname: body\n"),
            (Some("n".into()), Some("one two".into()))
        );
        assert_eq!(
            front_matter("---\nname:\ndescription: d\n---\n"),
            (None, Some("d".into()))
        );
    }

    #[test]
    fn a_missing_data_directory_has_no_skills() {
        assert!(discover(Path::new("/nonexistent/claude-recap"), &[]).is_empty());
    }
}
