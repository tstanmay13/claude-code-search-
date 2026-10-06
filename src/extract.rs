//! Turns a Claude Code JSONL transcript into the text a person would
//! recognize as the conversation, split by who wrote it.
//!
//! Transcripts also store system reminders, tool and skill lists, thinking
//! blocks and images. Those appear in nearly every session, so including them
//! made every query match every session. They are dropped here.

use serde::Deserialize;
use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Title = 0,
    Project = 1,
    User = 2,
    Assistant = 3,
    /// Messages injected into the session by other agents and background tasks.
    Other = 4,
    ToolInput = 5,
    ToolOutput = 6,
}

pub const NUM_FIELDS: usize = 7;
pub const FIELDS: [Field; NUM_FIELDS] =
    [Field::Title, Field::Project, Field::User, Field::Assistant, Field::Other, Field::ToolInput, Field::ToolOutput];

impl Field {
    pub fn label(self) -> &'static str {
        match self {
            Field::Title => "title",
            Field::Project => "proj",
            Field::User => "you",
            Field::Assistant => "claude",
            Field::Other => "agents",
            Field::ToolInput => "tools",
            Field::ToolOutput => "output",
        }
    }
}

/// One message (or one part of a message) with the time it was written.
#[derive(Clone, Debug)]
pub struct Segment {
    pub field: Field,
    pub ts: String,
    pub text: String,
}

#[derive(Debug, Default)]
pub struct Transcript {
    pub cwd: String,
    pub branches: Vec<String>,
    /// Display title: custom title, else latest AI title, else agent name, else first prompt.
    pub title: String,
    /// Every distinct explicit title, for the title field.
    pub titles: Vec<String>,
    pub segments: Vec<Segment>,
}

const TOOL_OUTPUT_HEAD: usize = 6000;
const TOOL_OUTPUT_TAIL: usize = 2000;
const TOOL_INPUT_MAX: usize = 4000;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    is_meta: bool,
    #[serde(default)]
    is_sidechain: bool,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    git_branch: Option<String>,
    #[serde(default)]
    timestamp: String,
    #[serde(default)]
    message: Option<Message>,
    #[serde(default)]
    custom_title: Option<String>,
    #[serde(default)]
    ai_title: Option<String>,
    #[serde(default)]
    agent_name: Option<String>,
}

#[derive(Deserialize)]
struct Message {
    #[serde(default)]
    content: Value,
}

pub fn read(path: &Path) -> std::io::Result<Transcript> {
    let file = std::fs::File::open(path)?;
    let mut t = Transcript::default();
    let (mut custom, mut ai, mut agent) = (None, None, None);
    let mut first_prompt: Option<String> = None;

    for line in BufReader::new(file).lines() {
        let line = line?;
        // Cheap prefilter: skip attachments, snapshots and other bookkeeping
        // records without paying for a full JSON parse.
        if !(line.contains("\"type\":\"user\"")
            || line.contains("\"type\":\"assistant\"")
            || line.contains("-title\"")
            || line.contains("\"type\":\"agent-name\""))
        {
            continue;
        }
        let Ok(rec) = serde_json::from_str::<Record>(&line) else { continue };
        match rec.kind.as_str() {
            "custom-title" | "ai-title" | "agent-name" => {
                let (slot, value) = match rec.kind.as_str() {
                    "custom-title" => (&mut custom, rec.custom_title),
                    "ai-title" => (&mut ai, rec.ai_title),
                    _ => (&mut agent, rec.agent_name),
                };
                if let Some(v) = value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty()) {
                    if !t.titles.contains(&v) {
                        t.titles.push(v.clone());
                    }
                    *slot = Some(v);
                }
            }
            "user" | "assistant" if !rec.is_meta && !rec.is_sidechain => {
                if t.cwd.is_empty() {
                    if let Some(cwd) = &rec.cwd {
                        t.cwd = cwd.clone();
                    }
                }
                if let Some(b) = rec.git_branch.as_deref().filter(|b| !b.is_empty() && *b != "HEAD") {
                    if !t.branches.iter().any(|x| x == b) {
                        t.branches.push(b.to_owned());
                    }
                }
                let Some(msg) = rec.message else { continue };
                let before = t.segments.len();
                if rec.kind == "user" {
                    user_content(&msg.content, &rec.timestamp, &mut t.segments);
                } else {
                    assistant_content(&msg.content, &rec.timestamp, &mut t.segments);
                }
                if first_prompt.is_none() {
                    first_prompt = t.segments[before..]
                        .iter()
                        .find(|s| s.field == Field::User && is_real_prompt(&s.text))
                        .map(|s| s.text.clone());
                }
            }
            _ => {}
        }
    }

    t.title = custom
        .or(ai)
        .or(agent)
        .or(first_prompt)
        .map(|s| one_line(&s, 200))
        .unwrap_or_default();
    Ok(t)
}

/// A bare slash command like "/model" or "/clear" says nothing about the session.
fn is_real_prompt(s: &str) -> bool {
    let bare_command = s.starts_with('/') && !s.contains(char::is_whitespace);
    !bare_command && s.chars().any(char::is_alphanumeric)
}

fn user_content(content: &Value, ts: &str, out: &mut Vec<Segment>) {
    match content {
        Value::String(s) => user_text(s, ts, out),
        Value::Array(items) => {
            for item in items {
                match item.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(s) = item.get("text").and_then(Value::as_str) {
                            user_text(s, ts, out);
                        }
                    }
                    Some("tool_result") => {
                        let text = tool_result_text(item.get("content"));
                        let text = strip_system_reminders(&text);
                        if !text.trim().is_empty() {
                            out.push(seg(Field::ToolOutput, ts, truncate_middle(&text)));
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

/// Classify a user-role string. Many of these are not typed by the user:
/// slash-command wrappers, background task notifications, and messages
/// relayed from other sessions.
fn user_text(raw: &str, ts: &str, out: &mut Vec<Segment>) {
    let s = strip_system_reminders(raw);
    let s = s.trim();
    if s.is_empty() || s.starts_with("<local-command-caveat>") || s.starts_with("[Request interrupted") {
        return;
    }
    if s.starts_with("<task-notification>") || s.starts_with("Another Claude session sent a message") {
        out.push(seg(Field::Other, ts, s.to_owned()));
    } else if ["<local-command-stdout>", "<bash-stdout>", "<bash-stderr>", "<persisted-output>"]
        .iter()
        .any(|p| s.starts_with(p))
    {
        out.push(seg(Field::ToolOutput, ts, truncate_middle(s)));
    } else if s.contains("<command-name>") {
        let name = between(s, "<command-name>", "</command-name>").unwrap_or("");
        let args = between(s, "<command-args>", "</command-args>").unwrap_or("");
        let text = format!("{} {}", name.trim(), args.trim());
        if !text.trim().is_empty() {
            out.push(seg(Field::User, ts, text.trim().to_owned()));
        }
    } else if let Some(cmd) = between(s, "<bash-input>", "</bash-input>") {
        out.push(seg(Field::User, ts, format!("! {}", cmd.trim())));
    } else {
        out.push(seg(Field::User, ts, s.to_owned()));
    }
}

fn assistant_content(content: &Value, ts: &str, out: &mut Vec<Segment>) {
    let Value::Array(items) = content else {
        if let Value::String(s) = content {
            out.push(seg(Field::Assistant, ts, s.clone()));
        }
        return;
    };
    for item in items {
        match item.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(s) = item.get("text").and_then(Value::as_str) {
                    if !s.trim().is_empty() {
                        out.push(seg(Field::Assistant, ts, s.to_owned()));
                    }
                }
            }
            Some("tool_use") => {
                let mut text = String::new();
                if let Some(input) = item.get("input") {
                    collect_strings(input, &mut text);
                }
                if !text.trim().is_empty() {
                    out.push(seg(Field::ToolInput, ts, truncate_chars(&text, TOOL_INPUT_MAX)));
                }
            }
            _ => {}
        }
    }
}

fn tool_result_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .filter(|i| i.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|i| i.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn collect_strings(v: &Value, out: &mut String) {
    match v {
        Value::String(s) => {
            out.push_str(s);
            out.push('\n');
        }
        Value::Array(a) => a.iter().for_each(|x| collect_strings(x, out)),
        Value::Object(o) => o.values().for_each(|x| collect_strings(x, out)),
        _ => {}
    }
}

fn seg(field: Field, ts: &str, text: String) -> Segment {
    Segment { field, ts: ts.to_owned(), text }
}

pub fn strip_system_reminders(s: &str) -> String {
    const OPEN: &str = "<system-reminder>";
    const CLOSE: &str = "</system-reminder>";
    if !s.contains(OPEN) {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find(OPEN) {
        out.push_str(&rest[..start]);
        match rest[start..].find(CLOSE) {
            Some(end) => rest = &rest[start + end + CLOSE.len()..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

fn between<'a>(s: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = s.find(open)? + open.len();
    let end = s[start..].find(close)? + start;
    Some(&s[start..end])
}

fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => s[..i].to_owned(),
        None => s.to_owned(),
    }
}

/// Keep the head and tail of long tool output; errors tend to be at either end.
fn truncate_middle(s: &str) -> String {
    let n = s.chars().count();
    if n <= TOOL_OUTPUT_HEAD + TOOL_OUTPUT_TAIL {
        return s.to_owned();
    }
    let head: String = s.chars().take(TOOL_OUTPUT_HEAD).collect();
    let tail: String = s.chars().skip(n - TOOL_OUTPUT_TAIL).collect();
    format!("{head}\n…\n{tail}")
}

pub fn one_line(s: &str, max: usize) -> String {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(&flat, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(s: &str) -> Vec<Segment> {
        let mut out = vec![];
        user_text(s, "", &mut out);
        out
    }

    #[test]
    fn classifies_injected_user_messages() {
        assert!(user("<local-command-caveat>Caveat: ...</local-command-caveat>").is_empty());
        assert_eq!(user("Another Claude session sent a message:\n<x>")[0].field, Field::Other);
        assert_eq!(user("<task-notification><id>1</id></task-notification>")[0].field, Field::Other);
        assert_eq!(user("<local-command-stdout>ok</local-command-stdout>")[0].field, Field::ToolOutput);
        let cmd = user("<command-message>explain</command-message><command-name>/explain</command-name><command-args>the MCP gateway</command-args>");
        assert_eq!((cmd[0].field, cmd[0].text.as_str()), (Field::User, "/explain the MCP gateway"));
        assert_eq!(user("<bash-input>pwd</bash-input>")[0].text, "! pwd");
    }

    #[test]
    fn strips_system_reminders_inside_messages() {
        let s = user("fix the bug<system-reminder>mcp__slack tools available</system-reminder> please");
        assert_eq!(s[0].text, "fix the bug please");
        assert!(user("<system-reminder>only a reminder</system-reminder>").is_empty());
    }
}
