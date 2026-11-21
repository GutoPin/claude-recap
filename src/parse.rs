//! Reads one session transcript (a `.jsonl` file) into a `Session`.
//!
//! The transcript format is not documented and changes between Claude Code
//! releases, so every field is optional and a line that does not fit is
//! skipped instead of failing the file.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

const MAX_PROMPT_CHARS: usize = 2000;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl Tokens {
    pub fn add(&mut self, other: &Tokens) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Prompt {
    pub at: Option<DateTime<Utc>>,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Session {
    pub id: String,
    #[serde(skip)]
    pub file: PathBuf,
    /// Directory the session was started in.
    pub cwd: Option<String>,
    /// Every directory the session worked in, with the number of records seen there.
    pub dirs: BTreeMap<String, u32>,
    pub branch: Option<String>,
    pub title: Option<String>,
    pub started: Option<DateTime<Utc>>,
    pub ended: Option<DateTime<Utc>>,
    pub prompts: Vec<Prompt>,
    pub replies: u32,
    pub tools: BTreeMap<String, u32>,
    pub files: BTreeMap<String, u32>,
    pub skills: BTreeMap<String, u32>,
    pub models: BTreeMap<String, u32>,
    pub tokens: Tokens,
    /// As recorded by Claude Code itself; absent in older transcripts.
    pub cost_usd: Option<f64>,
    pub active_ms: Option<u64>,
}

impl Session {
    pub fn name(&self) -> String {
        if let Some(title) = self.title.as_deref().filter(|t| !t.trim().is_empty()) {
            return title.trim().to_string();
        }
        match self.prompts.first() {
            Some(p) => one_line(&p.text),
            None => "(no prompts)".to_string(),
        }
    }

    pub fn model(&self) -> Option<&str> {
        self.models
            .iter()
            .max_by_key(|&(name, n)| (*n, std::cmp::Reverse(name)))
            .map(|(name, _)| name.as_str())
    }
}

pub fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Deserialize)]
struct Kind<'a> {
    #[serde(rename = "type", borrow)]
    kind: Option<Cow<'a, str>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    timestamp: Option<String>,
    cwd: Option<String>,
    git_branch: Option<String>,
    is_meta: Option<bool>,
    is_sidechain: Option<bool>,
    is_compact_summary: Option<bool>,
    prompt_source: Option<String>,
    message: Option<Message>,
    ai_title: Option<String>,
    custom_title: Option<String>,
    summary: Option<String>,
    #[serde(rename = "totalCostUSD")]
    total_cost_usd: Option<f64>,
    total_duration: Option<u64>,
}

#[derive(Deserialize)]
struct Message {
    id: Option<String>,
    model: Option<String>,
    usage: Option<Usage>,
    content: Option<Content>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Content {
    Text(String),
    Blocks(Vec<Block>),
}

#[derive(Deserialize)]
struct Block {
    #[serde(rename = "type")]
    kind: Option<String>,
    text: Option<String>,
    id: Option<String>,
    name: Option<String>,
    input: Option<Input>,
}

#[derive(Deserialize)]
struct Input {
    file_path: Option<String>,
    notebook_path: Option<String>,
    skill: Option<String>,
}

#[derive(Deserialize)]
struct Usage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
}

const EDITING_TOOLS: &[&str] = &["Edit", "Write", "MultiEdit", "NotebookEdit"];

/// Returns `None` when the file holds no conversation at all.
pub fn parse_session(path: &Path) -> Option<Session> {
    let file = File::open(path).ok()?;
    let mut session = Session {
        id: path.file_stem()?.to_string_lossy().into_owned(),
        file: path.to_path_buf(),
        ..Session::default()
    };
    let mut titles = Titles::default();
    // One reply is written as several records that repeat its token usage, so
    // usage is kept per message id and summed at the end.
    let mut usage: HashMap<String, Tokens> = HashMap::new();
    let mut tool_calls: HashSet<String> = HashSet::new();

    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { break };
        // Transcripts carry large records this tool has no use for. Reading
        // only the type first lets them go by without building anything.
        let kind = match serde_json::from_str::<Kind>(&line) {
            Ok(Kind { kind: Some(kind) }) => kind,
            _ => continue,
        };
        if !matches!(
            kind.as_ref(),
            "user" | "assistant" | "ai-title" | "custom-title" | "summary" | "cost-state"
        ) {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Record>(&line) else {
            continue;
        };
        match kind.as_ref() {
            "user" => {
                session.see(&record);
                if let Some(text) = prompt_text(&record) {
                    session.prompts.push(Prompt {
                        at: timestamp(&record),
                        text,
                    });
                }
            }
            "assistant" => {
                session.see(&record);
                if record.is_sidechain == Some(true) {
                    continue;
                }
                let Some(message) = record.message else {
                    continue;
                };
                session.reply(message, &mut usage, &mut tool_calls);
            }
            "ai-title" => titles.generated = record.ai_title.or(titles.generated),
            "custom-title" => titles.custom = record.custom_title.or(titles.custom),
            "summary" => titles.summary = record.summary.or(titles.summary),
            "cost-state" => {
                session.cost_usd = record.total_cost_usd.or(session.cost_usd);
                session.active_ms = record.total_duration.or(session.active_ms);
            }
            _ => {}
        }
    }

    session.replies = usage.len() as u32;
    for tokens in usage.values() {
        session.tokens.add(tokens);
    }
    session.title = titles.custom.or(titles.generated).or(titles.summary);
    (!session.prompts.is_empty() || session.replies > 0).then_some(session)
}

#[derive(Default)]
struct Titles {
    custom: Option<String>,
    generated: Option<String>,
    summary: Option<String>,
}

impl Session {
    /// Folds in what every conversation record carries: time, directory, branch.
    fn see(&mut self, record: &Record) {
        if let Some(at) = timestamp(record) {
            self.started = Some(self.started.map_or(at, |t| t.min(at)));
            self.ended = Some(self.ended.map_or(at, |t| t.max(at)));
        }
        if let Some(cwd) = &record.cwd {
            self.cwd.get_or_insert_with(|| cwd.clone());
            *self.dirs.entry(cwd.clone()).or_default() += 1;
        }
        if let Some(branch) = record
            .git_branch
            .as_deref()
            .filter(|b| !b.is_empty() && *b != "HEAD")
        {
            self.branch = Some(branch.to_string());
        }
    }

    fn reply(
        &mut self,
        message: Message,
        usage: &mut HashMap<String, Tokens>,
        tool_calls: &mut HashSet<String>,
    ) {
        // "<synthetic>" marks messages Claude Code wrote itself, such as API errors.
        let real = message
            .model
            .as_deref()
            .is_some_and(|m| !m.starts_with('<'));
        if let (Some(u), true) = (&message.usage, real) {
            let key = message
                .id
                .clone()
                .unwrap_or_else(|| format!("#{}", usage.len()));
            if !usage.contains_key(&key) {
                *self
                    .models
                    .entry(message.model.clone().unwrap_or_default())
                    .or_default() += 1;
            }
            usage.insert(
                key,
                Tokens {
                    input: u.input_tokens.unwrap_or(0),
                    output: u.output_tokens.unwrap_or(0),
                    cache_read: u.cache_read_input_tokens.unwrap_or(0),
                    cache_write: u.cache_creation_input_tokens.unwrap_or(0),
                },
            );
        }

        let Some(Content::Blocks(blocks)) = message.content else {
            return;
        };
        for block in blocks {
            if block.kind.as_deref() != Some("tool_use") {
                continue;
            }
            let Some(name) = block.name else { continue };
            if let Some(id) = block.id
                && !tool_calls.insert(id)
            {
                continue;
            }
            if let Some(input) = block.input {
                if name == "Skill" {
                    if let Some(skill) = input.skill {
                        *self.skills.entry(skill).or_default() += 1;
                    }
                } else if EDITING_TOOLS.contains(&name.as_str())
                    && let Some(path) = input.file_path.or(input.notebook_path)
                {
                    *self.files.entry(path).or_default() += 1;
                }
            }
            *self.tools.entry(name).or_default() += 1;
        }
    }
}

fn timestamp(record: &Record) -> Option<DateTime<Utc>> {
    let at = DateTime::parse_from_rfc3339(record.timestamp.as_deref()?).ok()?;
    Some(at.with_timezone(&Utc))
}

/// The text of a record if a person typed it. Most `user` records are tool
/// results, hook output or notes Claude Code adds on its own.
fn prompt_text(record: &Record) -> Option<String> {
    let skip = [
        record.is_meta,
        record.is_sidechain,
        record.is_compact_summary,
    ]
    .contains(&Some(true))
        || record.prompt_source.as_deref() == Some("system");
    if skip {
        return None;
    }
    let text = match record.message.as_ref()?.content.as_ref()? {
        Content::Text(text) => text.clone(),
        Content::Blocks(blocks) => {
            if blocks
                .iter()
                .any(|b| b.kind.as_deref() == Some("tool_result"))
            {
                return None;
            }
            blocks
                .iter()
                .filter_map(|b| b.text.as_deref())
                .collect::<Vec<_>>()
                .join("\n")
        }
    };
    clean_prompt(&text)
}

fn clean_prompt(text: &str) -> Option<String> {
    let text = text.trim();
    if let Some(name) = between(text, "<command-name>", "</command-name>") {
        let args = between(text, "<command-args>", "</command-args>").unwrap_or_default();
        return Some(
            format!("{} {}", name.trim(), args.trim())
                .trim()
                .to_string(),
        );
    }
    let text = strip_tag(text, "pasted_content");
    let text = text.trim();
    if text.is_empty() || text.starts_with('<') || text.starts_with("[Request interrupted") {
        return None;
    }
    Some(text.chars().take(MAX_PROMPT_CHARS).collect())
}

fn between<'a>(text: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = text.find(open)? + open.len();
    let end = text[start..].find(close)? + start;
    Some(&text[start..end])
}

/// Removes `<name ...>` and `</name ...>` markers, keeping what they wrap.
fn strip_tag(text: &str, name: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let open = rest.find(&format!("<{name}"));
        let close = rest.find(&format!("</{name}"));
        let Some(at) = [open, close].into_iter().flatten().min() else {
            break;
        };
        let Some(end) = rest[at..].find('>') else {
            break;
        };
        out.push_str(&rest[..at]);
        rest = &rest[at + end + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use serde_json::{Value, json};

    pub fn user(at: &str, cwd: &str, text: &str) -> Value {
        json!({"type": "user", "timestamp": at, "cwd": cwd, "gitBranch": "main", "promptSource": "typed",
               "message": {"role": "user", "content": text}})
    }

    pub fn reply(at: &str, cwd: &str, id: &str, output: u64, blocks: Value) -> Value {
        json!({"type": "assistant", "timestamp": at, "cwd": cwd,
               "message": {"id": id, "model": "claude-opus-5", "role": "assistant", "content": blocks,
                           "usage": {"input_tokens": 10, "output_tokens": output,
                                     "cache_read_input_tokens": 1000, "cache_creation_input_tokens": 200}}})
    }

    pub fn tool(id: &str, name: &str, input: Value) -> Value {
        json!({"type": "tool_use", "id": id, "name": name, "input": input})
    }

    pub fn write_jsonl(path: &Path, records: &[Value]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let body: String = records.iter().map(|r| format!("{r}\n")).collect();
        std::fs::write(path, body).unwrap();
    }

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("claude-recap-parse-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn reads_a_session() {
        let dir = scratch("session");
        let file = dir.join("abc-123.jsonl");
        write_jsonl(
            &file,
            &[
                json!({"type": "mode", "mode": "normal"}),
                user("2026-03-01T10:00:00.000Z", "/work/shop", "add a cart page"),
                reply(
                    "2026-03-01T10:00:05.000Z",
                    "/work/shop",
                    "m1",
                    50,
                    json!([{"type": "thinking", "thinking": "hm"}]),
                ),
                reply(
                    "2026-03-01T10:00:06.000Z",
                    "/work/shop",
                    "m1",
                    80,
                    json!([tool("t1", "Read", json!({"file_path": "/work/shop/a.ts"}))]),
                ),
                reply(
                    "2026-03-01T10:00:07.000Z",
                    "/work/shop",
                    "m1",
                    120,
                    json!([tool(
                        "t2",
                        "Edit",
                        json!({"file_path": "/work/shop/cart.ts", "old_string": "a", "new_string": "b"})
                    )]),
                ),
                // The same tool call written twice must count once.
                reply(
                    "2026-03-01T10:00:07.500Z",
                    "/work/shop",
                    "m1",
                    120,
                    json!([tool(
                        "t2",
                        "Edit",
                        json!({"file_path": "/work/shop/cart.ts"})
                    )]),
                ),
                json!({"type": "user", "timestamp": "2026-03-01T10:00:08.000Z", "cwd": "/work/shop",
                       "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t2", "content": "ok"}]}}),
                reply(
                    "2026-03-01T10:01:00.000Z",
                    "/work/shop/api",
                    "m2",
                    30,
                    json!([tool("t3", "Skill", json!({"skill": "frontend-design"})), {"type": "text", "text": "done"}]),
                ),
                user(
                    "2026-03-01T10:30:00.000Z",
                    "/work/shop/api",
                    "now  the\ncheckout",
                ),
                json!({"type": "assistant", "timestamp": "2026-03-01T10:30:01.000Z",
                       "message": {"id": "err", "model": "<synthetic>", "content": [{"type": "text", "text": "API error"}],
                                   "usage": {"input_tokens": 0, "output_tokens": 0}}}),
                json!({"type": "ai-title", "aiTitle": "Draft"}),
                json!({"type": "ai-title", "aiTitle": "Cart and checkout"}),
                json!({"type": "cost-state", "totalCostUSD": 1.5, "totalDuration": 60000}),
                json!({"type": "cost-state", "totalCostUSD": 3.25, "totalDuration": 1_800_000}),
                json!({"type": "api-request-blob", "message": {"content": "x".repeat(5000)}}),
            ],
        );
        std::fs::OpenOptions::new()
            .append(true)
            .open(&file)
            .map(|mut f| std::io::Write::write_all(&mut f, b"{not json\n\n"))
            .unwrap()
            .unwrap();

        let s = parse_session(&file).unwrap();
        assert_eq!(s.id, "abc-123");
        assert_eq!(s.name(), "Cart and checkout");
        assert_eq!(s.cwd.as_deref(), Some("/work/shop"));
        assert_eq!(
            s.dirs,
            BTreeMap::from([
                ("/work/shop".to_string(), 6),
                ("/work/shop/api".to_string(), 2)
            ])
        );
        assert_eq!(s.branch.as_deref(), Some("main"));
        assert_eq!(s.started.unwrap().to_rfc3339(), "2026-03-01T10:00:00+00:00");
        assert_eq!(s.ended.unwrap().to_rfc3339(), "2026-03-01T10:30:01+00:00");
        let prompts: Vec<_> = s.prompts.iter().map(|p| p.text.as_str()).collect();
        assert_eq!(prompts, ["add a cart page", "now  the\ncheckout"]);
        assert_eq!(s.replies, 2);
        assert_eq!(
            s.tokens,
            Tokens {
                input: 20,
                output: 150,
                cache_read: 2000,
                cache_write: 400
            }
        );
        assert_eq!(
            s.tools,
            BTreeMap::from([
                ("Edit".to_string(), 1),
                ("Read".to_string(), 1),
                ("Skill".to_string(), 1)
            ])
        );
        assert_eq!(
            s.files,
            BTreeMap::from([("/work/shop/cart.ts".to_string(), 1)])
        );
        assert_eq!(
            s.skills,
            BTreeMap::from([("frontend-design".to_string(), 1)])
        );
        assert_eq!(s.model(), Some("claude-opus-5"));
        assert_eq!((s.cost_usd, s.active_ms), (Some(3.25), Some(1_800_000)));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn untitled_sessions_are_named_after_their_first_prompt() {
        let dir = scratch("untitled");
        let file = dir.join("s.jsonl");
        write_jsonl(
            &file,
            &[user("2026-03-01T10:00:00Z", "/w", "fix   the\n login bug")],
        );
        assert_eq!(parse_session(&file).unwrap().name(), "fix the login bug");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn files_without_a_conversation_are_ignored() {
        let dir = scratch("empty");
        let file = dir.join("s.jsonl");
        write_jsonl(
            &file,
            &[
                json!({"type": "mode", "mode": "normal"}),
                json!({"type": "ai-title", "aiTitle": "x"}),
            ],
        );
        assert!(parse_session(&file).is_none());
        assert!(parse_session(&dir.join("missing.jsonl")).is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn only_typed_text_counts_as_a_prompt() {
        let record = |value: Value| serde_json::from_value::<Record>(value).unwrap();
        let typed = |text: &str| prompt_text(&record(json!({"message": {"content": text}})));

        assert_eq!(typed("  hello  ").as_deref(), Some("hello"));
        assert_eq!(typed("<task-notification>done</task-notification>"), None);
        assert_eq!(typed("<system-reminder>x</system-reminder>"), None);
        assert_eq!(typed("[Request interrupted by user for tool use]"), None);
        assert_eq!(typed("   "), None);
        assert_eq!(
            typed("<command-message>review</command-message>\n<command-name>/review</command-name>\n<command-args>src/</command-args>").as_deref(),
            Some("/review src/")
        );
        assert_eq!(
            typed("<pasted_content id=\"a1\">\nbuild it\n</pasted_content id=\"a1\">").as_deref(),
            Some("build it")
        );

        for flag in ["isMeta", "isSidechain", "isCompactSummary"] {
            assert_eq!(
                prompt_text(&record(json!({flag: true, "message": {"content": "hi"}}))),
                None,
                "{flag}"
            );
        }
        assert_eq!(
            prompt_text(&record(
                json!({"promptSource": "system", "message": {"content": "hi"}})
            )),
            None
        );
        let image = json!({"message": {"content": [{"type": "image", "source": {}}, {"type": "text", "text": "see this"}]}});
        assert_eq!(prompt_text(&record(image)).as_deref(), Some("see this"));
    }

    #[test]
    fn long_prompts_are_capped() {
        let text = "é".repeat(MAX_PROMPT_CHARS + 50);
        assert_eq!(
            clean_prompt(&text).unwrap().chars().count(),
            MAX_PROMPT_CHARS
        );
    }
}
