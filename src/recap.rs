//! Builds the recap text for a folder, a session or a skill. The same rows
//! feed the interactive view and the plain output.

use std::collections::BTreeMap;
use std::fmt::Display;

use chrono::{DateTime, Datelike, TimeZone, Utc};

use crate::parse::{Session, one_line};
use crate::skills::Skill;
use crate::store::{Folder, Library, home};

const TOP: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tone {
    Title,
    Section,
    Plain,
    Dim,
}

pub type Row = (Tone, String);

fn blank() -> Row {
    (Tone::Plain, String::new())
}

pub fn to_text(rows: &[Row]) -> String {
    rows.iter().map(|(_, text)| format!("{text}\n")).collect()
}

/// Replaces the home directory with `~`.
pub fn tilde(path: &str) -> String {
    let Some(home) = home() else {
        return path.to_string();
    };
    let home = home.to_string_lossy();
    match path.strip_prefix(home.as_ref()) {
        Some("") => "~".to_string(),
        Some(rest) if rest.starts_with('/') && home.len() > 1 => format!("~{rest}"),
        _ => path.to_string(),
    }
}

pub fn money(usd: f64) -> String {
    format!("${usd:.2}")
}

pub fn compact(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.1}k", n as f64 / 1e3),
        _ => format!("{:.1}M", n as f64 / 1e6),
    }
}

pub fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

pub fn span(ms: u64) -> String {
    let secs = ms / 1000;
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        _ => format!("{}h {}m", secs / 3600, secs % 3600 / 60),
    }
}

pub fn ago(now: DateTime<Utc>, then: DateTime<Utc>) -> String {
    let mins = (now - then).num_minutes();
    match mins {
        ..1 => "just now".to_string(),
        1..60 => format!("{mins}m ago"),
        60..1440 => format!("{}h ago", mins / 60),
        _ => format!("{}d ago", mins / 1440),
    }
}

/// "Mar 3", with the year when it is not the current one.
pub fn day<Tz: TimeZone>(at: DateTime<Utc>, now: DateTime<Utc>, tz: &Tz) -> String
where
    Tz::Offset: Display,
{
    let local = at.with_timezone(tz);
    let pattern = if local.year() == now.with_timezone(tz).year() {
        "%b %-d"
    } else {
        "%b %-d, %Y"
    };
    local.format(pattern).to_string()
}

fn clock<Tz: TimeZone>(at: DateTime<Utc>, tz: &Tz) -> String
where
    Tz::Offset: Display,
{
    at.with_timezone(tz).format("%H:%M").to_string()
}

/// Counts as "name 12 · other 3", highest first.
fn tally(counts: &BTreeMap<String, u32>) -> String {
    ranked(counts)
        .into_iter()
        .take(TOP)
        .map(|(name, n)| format!("{name} {n}"))
        .collect::<Vec<_>>()
        .join(" · ")
}

fn ranked(counts: &BTreeMap<String, u32>) -> Vec<(&str, u32)> {
    let mut all: Vec<(&str, u32)> = counts.iter().map(|(k, &v)| (k.as_str(), v)).collect();
    all.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    all
}

fn merged<'a>(maps: impl Iterator<Item = &'a BTreeMap<String, u32>>) -> BTreeMap<String, u32> {
    let mut total = BTreeMap::new();
    for map in maps {
        for (key, n) in map {
            *total.entry(key.clone()).or_default() += n;
        }
    }
    total
}

fn counted(rows: &mut Vec<Row>, heading: &str, counts: &BTreeMap<String, u32>, base: &str) {
    if counts.is_empty() {
        return;
    }
    let extra = counts.len().saturating_sub(TOP);
    rows.extend([blank(), (Tone::Section, heading.to_string())]);
    for (name, n) in ranked(counts).into_iter().take(TOP) {
        let shown = name
            .strip_prefix(base)
            .map(|rest| rest.trim_start_matches('/'))
            .filter(|r| !r.is_empty());
        rows.push((
            Tone::Plain,
            format!(
                "{n:>4}  {}",
                shown.map_or_else(|| tilde(name), str::to_string)
            ),
        ));
    }
    if extra > 0 {
        rows.push((Tone::Dim, format!("      and {extra} more")));
    }
}

pub fn folder_rows<Tz: TimeZone>(folder: &Folder, now: DateTime<Utc>, tz: &Tz) -> Vec<Row>
where
    Tz::Offset: Display,
{
    let mut facts = vec![plural(folder.sessions.len(), "session", "sessions")];
    if let Some(at) = folder.last_active() {
        facts.push(format!("last active {}", ago(now, at)));
    }
    if let Some(cost) = folder.cost_usd() {
        facts.push(money(cost));
    }
    facts.push(format!("{} output tokens", compact(folder.tokens().output)));

    let mut rows = vec![
        (Tone::Title, tilde(&folder.path)),
        (Tone::Dim, facts.join(" · ")),
        blank(),
        (Tone::Section, "Sessions".to_string()),
    ];
    for session in &folder.sessions {
        let when = session.ended.map(|at| day(at, now, tz)).unwrap_or_default();
        rows.push((Tone::Plain, format!("{when:<8}{}", session.name())));
    }

    let files = merged(folder.sessions.iter().map(|s| &s.files));
    counted(&mut rows, "Most edited files", &files, &folder.path);
    let tools = merged(folder.sessions.iter().map(|s| &s.tools));
    if !tools.is_empty() {
        rows.extend([
            blank(),
            (Tone::Section, "Tools".to_string()),
            (Tone::Plain, tally(&tools)),
        ]);
    }
    let skills = merged(folder.sessions.iter().map(|s| &s.skills));
    if !skills.is_empty() {
        rows.extend([
            blank(),
            (Tone::Section, "Skills".to_string()),
            (Tone::Plain, tally(&skills)),
        ]);
    }
    rows
}

pub fn session_rows<Tz: TimeZone>(session: &Session, now: DateTime<Utc>, tz: &Tz) -> Vec<Row>
where
    Tz::Offset: Display,
{
    let mut rows = vec![(Tone::Title, session.name())];

    let same_day = match (session.started, session.ended) {
        (Some(a), Some(b)) => a.with_timezone(tz).date_naive() == b.with_timezone(tz).date_naive(),
        _ => true,
    };
    let mut facts = Vec::new();
    if let (Some(start), Some(end)) = (session.started, session.ended) {
        facts.push(if same_day {
            format!(
                "{}, {} to {}",
                day(start, now, tz),
                clock(start, tz),
                clock(end, tz)
            )
        } else {
            format!(
                "{} {} to {} {}",
                day(start, now, tz),
                clock(start, tz),
                day(end, now, tz),
                clock(end, tz)
            )
        });
    }
    if let Some(ms) = session.active_ms {
        facts.push(format!("active {}", span(ms)));
    }
    if let Some(model) = session.model() {
        facts.push(model.to_string());
    }
    if !facts.is_empty() {
        rows.push((Tone::Dim, facts.join(" · ")));
    }

    let mut usage = Vec::new();
    if let Some(cost) = session.cost_usd {
        usage.push(money(cost));
    }
    usage.push(plural(session.prompts.len(), "prompt", "prompts"));
    let t = &session.tokens;
    usage.push(format!(
        "tokens {} in, {} out, {} cache read, {} cache write",
        compact(t.input),
        compact(t.output),
        compact(t.cache_read),
        compact(t.cache_write)
    ));
    rows.push((Tone::Dim, usage.join(" · ")));

    let mut resume = format!("claude --resume {}", session.id);
    if let Some(branch) = &session.branch {
        resume = format!("branch {branch} · {resume}");
    }
    rows.push((Tone::Dim, resume));

    if !session.prompts.is_empty() {
        rows.extend([blank(), (Tone::Section, "Prompts".to_string())]);
        for prompt in &session.prompts {
            let at = match prompt.at {
                Some(at) if same_day => clock(at, tz),
                Some(at) => format!("{} {}", day(at, now, tz), clock(at, tz)),
                None => String::new(),
            };
            rows.push((Tone::Plain, format!("{at}  {}", one_line(&prompt.text))));
        }
    }

    let base = session.cwd.as_deref().unwrap_or_default();
    counted(&mut rows, "Files edited", &session.files, base);
    if !session.tools.is_empty() {
        rows.extend([
            blank(),
            (Tone::Section, "Tools".to_string()),
            (Tone::Plain, tally(&session.tools)),
        ]);
    }
    if !session.skills.is_empty() {
        rows.extend([
            blank(),
            (Tone::Section, "Skills".to_string()),
            (Tone::Plain, tally(&session.skills)),
        ]);
    }
    if session.dirs.len() > 1 {
        counted(&mut rows, "Worked in", &session.dirs, "");
    }
    rows
}

pub fn skill_rows(skill: &Skill, now: DateTime<Utc>) -> Vec<Row> {
    let mut facts = vec![skill.source.label()];
    facts.push(match skill.uses {
        0 => "never used".to_string(),
        n => format!("used {}", plural(n as usize, "time", "times")),
    });
    if let Some(at) = skill.last_used {
        facts.push(format!("last used {}", ago(now, at)));
    }
    let mut rows = vec![
        (Tone::Title, skill.name.clone()),
        (Tone::Dim, facts.join(" · ")),
    ];
    if !skill.description.is_empty() {
        rows.extend([blank(), (Tone::Plain, skill.description.clone())]);
    }
    counted(&mut rows, "Used in", &skill.used_in, "");
    if let Some(file) = &skill.file {
        rows.extend([blank(), (Tone::Dim, tilde(&file.to_string_lossy()))]);
    }
    rows
}

fn cut(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let kept: String = text.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}…")
}

/// The folder list printed when output is piped.
pub fn folders_table(lib: &Library, now: DateTime<Utc>) -> String {
    if lib.folders.is_empty() {
        return format!("No sessions in {}\n", lib.root.display());
    }
    let paths: Vec<String> = lib.folders.iter().map(|f| tilde(&f.path)).collect();
    let width = paths
        .iter()
        .map(|p| p.chars().count())
        .max()
        .unwrap_or(6)
        .max(6);
    let mut out = format!(
        "{:<width$}  {:>8}  {:>11}  {:>9}\n",
        "FOLDER", "SESSIONS", "LAST ACTIVE", "COST"
    );
    for (folder, path) in lib.folders.iter().zip(paths) {
        out.push_str(&format!(
            "{path:<width$}  {:>8}  {:>11}  {:>9}\n",
            folder.sessions.len(),
            folder
                .last_active()
                .map(|at| ago(now, at))
                .unwrap_or_default(),
            folder
                .cost_usd()
                .map(money)
                .unwrap_or_else(|| "-".to_string()),
        ));
    }
    out
}

pub fn skills_table(lib: &Library) -> String {
    if lib.skills().is_empty() {
        return "No skills installed or used\n".to_string();
    }
    let width = lib
        .skills()
        .iter()
        .map(|s| s.name.chars().count())
        .max()
        .unwrap_or(5)
        .max(5);
    let mut out = format!(
        "{:>4}  {:<width$}  {:<24}  DESCRIPTION\n",
        "USES", "SKILL", "SOURCE"
    );
    for skill in lib.skills() {
        let line = format!(
            "{:>4}  {:<width$}  {:<24}  {}",
            skill.uses,
            skill.name,
            cut(&skill.source.label(), 24),
            cut(&skill.description, 70)
        );
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{load, tests::fixture};

    fn now() -> DateTime<Utc> {
        "2026-03-05T09:00:10Z".parse().unwrap()
    }

    #[test]
    fn folder_recap() {
        let fx = fixture();
        let lib = load(&fx.0).unwrap();
        let text = to_text(&folder_rows(&lib.folders[0], now(), &Utc));
        assert_eq!(
            text,
            "/work/shop\n\
             2 sessions · last active 2d ago · $4.00 · 440 output tokens\n\
             \n\
             Sessions\n\
             Mar 3   why is the build failing?\n\
             Mar 1   Cart and checkout\n\
             \n\
             Most edited files\n\
             \x20  2  src/cart.ts\n\
             \x20  1  src/checkout.ts\n\
             \n\
             Tools\n\
             Skill 3 · Edit 2 · Bash 1 · Write 1\n\
             \n\
             Skills\n\
             frontend-design 1 · old-skill 1 · tools:debugging 1\n"
        );
    }

    #[test]
    fn session_recap() {
        let fx = fixture();
        let lib = load(&fx.0).unwrap();
        let text = to_text(&session_rows(&lib.folders[0].sessions[1], now(), &Utc));
        assert_eq!(
            text,
            "Cart and checkout\n\
             Mar 1, 10:00 to 10:20 · active 25m · claude-opus-5\n\
             $3.25 · 2 prompts · tokens 20 in, 400 out, 2.0k cache read, 400 cache write\n\
             branch main · claude --resume aaaa-1111\n\
             \n\
             Prompts\n\
             10:00  add a cart page\n\
             10:20  now the checkout\n\
             \n\
             Files edited\n\
             \x20  2  src/cart.ts\n\
             \x20  1  src/checkout.ts\n\
             \n\
             Tools\n\
             Edit 2 · Bash 1 · Skill 1 · Write 1\n\
             \n\
             Skills\n\
             frontend-design 1\n"
        );
    }

    #[test]
    fn times_follow_the_given_zone() {
        let fx = fixture();
        let lib = load(&fx.0).unwrap();
        let lima = chrono::FixedOffset::west_opt(5 * 3600).unwrap();
        let text = to_text(&session_rows(&lib.folders[1].sessions[0], now(), &lima));
        assert!(text.contains("Feb 10, 13:00 to 13:00"), "{text}");
        assert!(text.contains("13:00  summarize my notes"), "{text}");
        assert!(!text.contains("active"), "{text}");
    }

    #[test]
    fn skill_recap() {
        let fx = fixture();
        let lib = load(&fx.0).unwrap();
        let text = to_text(&skill_rows(&lib.skills()[0], now()));
        assert!(text.starts_with(
            "frontend-design\nuser · used 1 time · last used 3d ago\n\nGuidance for distinctive visual design.\n\nUsed in\n   1  shop\n"
        ));
        assert!(text.trim_end().ends_with("skills/frontend-design/SKILL.md"));
        let unused = to_text(&skill_rows(&lib.skills()[4], now()));
        assert!(unused.contains("user · never used\n"), "{unused}");
    }

    #[test]
    fn tables() {
        let fx = fixture();
        let lib = load(&fx.0).unwrap();
        assert_eq!(
            folders_table(&lib, now()),
            "FOLDER       SESSIONS  LAST ACTIVE       COST\n\
             /work/shop          2       2d ago      $4.00\n\
             /work/notes         1      22d ago          -\n"
        );
        let skills = skills_table(&lib);
        let design = format!(
            "   1  frontend-design  {:<24}  Guidance for distinctive visual design.\n",
            "user"
        );
        assert!(skills.contains(&design), "{skills}");
        assert!(
            skills.contains(&format!("   1  {:<15}  built in or removed\n", "old-skill")),
            "{skills}"
        );
    }

    #[test]
    fn small_formatters() {
        assert_eq!(
            [compact(999), compact(1_500), compact(2_340_000)],
            ["999", "1.5k", "2.3M"]
        );
        assert_eq!(
            [span(45_000), span(600_000), span(17_857_301)],
            ["45s", "10m", "4h 57m"]
        );
        let then: DateTime<Utc> = "2026-03-05T08:00:00Z".parse().unwrap();
        assert_eq!(ago(now(), then), "1h ago");
        assert_eq!(ago(now(), now()), "just now");
        let last_year: DateTime<Utc> = "2025-12-31T23:00:00Z".parse().unwrap();
        assert_eq!(day(last_year, now(), &Utc), "Dec 31, 2025");
        assert_eq!(cut("abcdef", 4), "abc…");
    }
}
