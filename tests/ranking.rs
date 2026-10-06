//! End-to-end ranking tests on generated transcripts. Each case mirrors a
//! real failure of the old grep-based search.

use ccs::index::{self, Doc};
use ccs::rank::{search, Query};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

struct Sessions {
    dir: tempfile::TempDir,
    next: u32,
}

impl Sessions {
    fn new() -> Sessions {
        Sessions { dir: tempfile::tempdir().unwrap(), next: 1 }
    }

    /// Write a session under a project dir named after `cwd`; returns its id.
    fn add(&mut self, cwd: &str, records: Vec<Value>) -> String {
        let sid = format!("{:08x}-0000-4000-8000-{:012x}", self.next, self.next);
        self.next += 1;
        let project = self.dir.path().join("projects").join(cwd.replace('/', "-"));
        std::fs::create_dir_all(&project).unwrap();
        let body: String = records
            .into_iter()
            .map(|mut r| {
                if r.get("cwd").is_none() && matches!(r["type"].as_str(), Some("user" | "assistant")) {
                    r["cwd"] = json!(cwd);
                }
                format!("{r}\n")
            })
            .collect();
        std::fs::write(project.join(format!("{sid}.jsonl")), body).unwrap();
        sid
    }

    fn path(&self, sid: &str) -> PathBuf {
        let projects = self.dir.path().join("projects");
        for p in std::fs::read_dir(&projects).unwrap().flatten() {
            let f = p.path().join(format!("{sid}.jsonl"));
            if f.exists() {
                return f;
            }
        }
        panic!("no session {sid}");
    }

    fn load(&self) -> Vec<Doc> {
        index::load_from(&self.dir.path().join("projects"), &self.dir.path().join("cache/index.bin"), true)
    }

    fn top(&self, query: &str) -> Vec<String> {
        let docs = self.load();
        search(&docs, &Query::parse(query), 0).iter().map(|r| r.doc.sid.clone()).collect()
    }
}

fn user(text: &str) -> Value {
    json!({"type": "user", "timestamp": "2026-10-01T10:00:00Z", "message": {"role": "user", "content": text}})
}

fn claude(text: &str) -> Value {
    json!({"type": "assistant", "timestamp": "2026-10-01T10:00:01Z",
           "message": {"role": "assistant", "content": [{"type": "text", "text": text}]}})
}

fn tool_output(text: &str) -> Value {
    json!({"type": "user", "timestamp": "2026-10-01T10:00:02Z",
           "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t", "content": text}]}})
}

fn filler() -> Vec<Value> {
    vec![user("please refactor the billing service"), claude("I refactored the billing service and ran the tests.")]
}

const CWD: &str = "/Users/me/code/app";

#[test]
fn reminders_tool_lists_and_attachments_do_not_match() {
    let mut s = Sessions::new();
    let real = s.add(CWD, vec![user("how do I set up the MCP gateway?"), claude("Here is how.")]);
    let noise = s.add(
        CWD,
        vec![
            json!({"type": "attachment", "attachment": {"type": "deferred_tools", "text": "mcp__slack__send mcp__linear__get"}}),
            user("fix the login bug<system-reminder>Available: mcp__claude_ai_Linear__save_issue</system-reminder>"),
            json!({"type": "user", "isMeta": true, "message": {"content": "MCP server instructions"}}),
            json!({"type": "user", "isSidechain": true, "message": {"content": "MCP from a subagent"}}),
            claude("Fixed the login bug."),
        ],
    );
    let top = s.top("mcp");
    assert_eq!(top, [real.clone()], "noise session {noise} must not match");
}

#[test]
fn base64_and_images_do_not_produce_tokens() {
    let mut s = Sessions::new();
    let b64 = "SnwYKEAgSGAI4AUIIdGhpbmtpbmcSDJQeXv89DcC9af1QlBoMv6/p0/TAcKN/wBAIIjBwd1bt7+seKMfQUiJG1rWAQmfz4p0";
    s.add(
        CWD,
        vec![
            tool_output(&format!("thinking signature: {b64}")),
            json!({"type": "user", "message": {"content": [
                {"type": "image", "source": {"type": "base64", "data": b64}},
                {"type": "text", "text": "what is in this screenshot"}]}}),
        ],
    );
    let real = s.add(CWD, vec![user("which tickets are P0 right now?")]);
    assert_eq!(s.top("p0"), [real]);
}

#[test]
fn title_and_project_outrank_passing_mentions() {
    let mut s = Sessions::new();
    let casual = s.add(
        CWD,
        vec![user("look at the gateway"), claude("The MCP layer forwards calls. MCP again. And MCP. MCP. MCP.")],
    );
    let subject = s.add(
        "/Users/me/code/repo-worktrees/mcp-toolkit",
        vec![json!({"type": "ai-title", "aiTitle": "MCP toolkit contracts"}), user("generate the tool contracts"), claude("Done.")],
    );
    assert_eq!(s.top("mcp"), [subject, casual]);
}

#[test]
fn more_mentions_rank_higher() {
    let mut s = Sessions::new();
    let once = s.add(CWD, vec![user("is this a p0?"), claude("No.")]);
    let mut many = vec![user("list the p0 tickets"), claude("There are four p0 tickets: the p0 auth bug, the p0 deploy, and two p0 data issues.")];
    many.push(user("close the first p0 and escalate the other p0"));
    let many = s.add(CWD, many);
    let mut rest = filler();
    rest.push(user("nothing relevant"));
    s.add(CWD, rest);
    assert_eq!(s.top("p0"), [many, once]);
}

#[test]
fn exact_phrase_beats_scattered_words() {
    let mut s = Sessions::new();
    let scattered = s.add(
        CWD,
        vec![
            user("the connection pool is too small, raise the connection limit and check every connection"),
            claude("I raised the timeout and the request timeout. Connection count is fine; timeout now 30s."),
        ],
    );
    // Only in tool output, the lowest-weight field, but as the exact phrase.
    let phrase = s.add(CWD, vec![user("run the integration tests"), tool_output("Error: connection timeout after 30000ms")]);
    assert_eq!(s.top("connection timeout"), [phrase.clone(), scattered.clone()]);
    assert_eq!(s.top("connection timeout "), [phrase.clone(), scattered.clone()]);
    // The phrase still wins while the last word is half typed.
    assert_eq!(s.top("connection time")[0], phrase);
}

#[test]
fn whole_word_beats_prefix_but_prefix_still_finds() {
    let mut s = Sessions::new();
    let prefix_only = s.add(CWD, vec![user("edit mcpServers in settings, mcpServers again, mcpServers")]);
    let whole = s.add(CWD, vec![claude("this uses MCP")]);
    assert_eq!(s.top("mcp"), [whole.clone(), prefix_only.clone()]);
    // A finished word (trailing space) does not prefix-match.
    assert_eq!(s.top("mcp "), [whole]);
    assert_eq!(s.top("mcpserv"), [prefix_only]);
}

#[test]
fn heavy_tool_output_cannot_beat_a_title_hit() {
    let mut s = Sessions::new();
    let noisy = s.add(CWD, vec![user("tail the logs"), tool_output(&"deploy step ok\n".repeat(300))]);
    let titled = s.add(CWD, vec![json!({"type": "custom-title", "customTitle": "deploy fix"}), user("fix it"), claude("ok")]);
    assert_eq!(s.top("deploy"), [titled, noisy]);
}

#[test]
fn title_prefers_custom_then_ai_then_first_real_prompt() {
    let mut s = Sessions::new();
    let fallback = s.add(
        CWD,
        vec![
            user("<local-command-caveat>Caveat: the messages below were generated</local-command-caveat>"),
            user("<command-name>/model</command-name><command-args></command-args>"),
            user("Another Claude session sent a message:\n<cross-session-message>hi</cross-session-message>"),
            user("fix the login bug on the settings page"),
        ],
    );
    let named = s.add(
        CWD,
        vec![
            json!({"type": "ai-title", "aiTitle": "Old AI title"}),
            json!({"type": "custom-title", "customTitle": "my name"}),
            json!({"type": "ai-title", "aiTitle": "Newer AI title"}),
            user("hello"),
        ],
    );
    let docs = s.load();
    let title = |sid: &str| docs.iter().find(|d| d.sid == sid).unwrap().title.clone();
    assert_eq!(title(&fallback), "fix the login bug on the settings page");
    assert_eq!(title(&named), "my name");
    // Every title is searchable, not only the displayed one.
    assert_eq!(s.top("old ai title"), [named]);
}

#[test]
fn cache_picks_up_changed_and_deleted_sessions() {
    let mut s = Sessions::new();
    let a = s.add(CWD, vec![user("first topic")]);
    let b = s.add(CWD, vec![user("second topic")]);
    assert!(s.top("zebra").is_empty());

    let path = s.path(&a);
    let mut body = std::fs::read_to_string(&path).unwrap();
    body.push_str(&format!("{}\n", user("now about a zebra")));
    std::fs::write(&path, body).unwrap();
    assert_eq!(s.top("zebra"), [a]);

    std::fs::remove_file(s.path(&b)).unwrap();
    assert_eq!(s.load().len(), 1);
    assert!(Path::new(&s.dir.path().join("cache/index.bin")).exists());
}

#[test]
fn empty_query_lists_every_session() {
    let mut s = Sessions::new();
    s.add(CWD, vec![user("one")]);
    s.add(CWD, vec![user("two")]);
    assert_eq!(s.top("").len(), 2);
}
