//! The fzf preview pane: the messages in one session that best match the
//! query, with who wrote them and when, and the matched words highlighted.

use crate::extract::{self, Field, Segment};
use crate::index::project_label;
use crate::rank::{Query, WEIGHTS};
use crate::text::FieldBuilder;
use std::fmt::Write;
use std::path::Path;

const MAX_SEGMENTS: usize = 12;
const SNIPPET_CHARS: usize = 360;
const HL: &str = "\x1b[1;31m";
const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const RESET: &str = "\x1b[0m";

pub fn render(path: &Path, query: &str) -> String {
    let Ok(t) = extract::read(path) else { return format!("cannot read {}", path.display()) };
    let q = Query::parse(query);
    let mut out = String::new();
    let _ = writeln!(out, "{BOLD}{}{RESET}", t.title);
    let branches = if t.branches.is_empty() { String::new() } else { format!("  [{}]", t.branches.join(", ")) };
    let _ = writeln!(out, "{DIM}{}{branches}  {} messages{RESET}\n", project_label(&t.cwd), t.segments.len());

    if q.is_empty() {
        let prompts: Vec<&Segment> = t.segments.iter().filter(|s| s.field == Field::User).collect();
        for s in prompts.iter().rev().take(MAX_SEGMENTS).rev() {
            let _ = writeln!(out, "{DIM}{} · {}{RESET}\n{}\n", who(s.field), time(&s.ts), snippet(&s.text, &q));
        }
        return out;
    }

    // Rank each message the same way sessions are ranked: exact phrase first,
    // then words matched, then field weight times hit count.
    let mut scored: Vec<(bool, usize, f64, &Segment)> = t
        .segments
        .iter()
        .filter_map(|s| {
            let mut fb = FieldBuilder::default();
            fb.push(&s.text);
            let mut fields: [String; extract::NUM_FIELDS] = Default::default();
            fields[s.field as usize] = fb.text;
            let m = q.match_fields(&fields);
            let matched = m.matched_terms();
            if matched == 0 {
                return None;
            }
            let hits: u32 = m.terms.iter().map(|h| h[s.field as usize].whole + h[s.field as usize].prefix).sum();
            Some((m.exact(&q), matched, WEIGHTS[s.field as usize] * f64::from(hits), s))
        })
        .collect();
    scored.sort_by(|a, b| (b.0, b.1).cmp(&(a.0, a.1)).then(b.2.total_cmp(&a.2)));

    let total = scored.len();
    let _ = writeln!(out, "{DIM}{total} matching messages{RESET}\n");
    for (_, _, _, s) in scored.into_iter().take(MAX_SEGMENTS) {
        let _ = writeln!(out, "{DIM}{} · {}{RESET}\n{}\n", who(s.field), time(&s.ts), snippet(&s.text, &q));
    }
    out
}

fn who(f: Field) -> &'static str {
    match f {
        Field::User => "you",
        Field::Assistant => "claude",
        Field::Other => "agent/task message",
        Field::ToolInput => "tool call",
        Field::ToolOutput => "tool output",
        Field::Title | Field::Project => "",
    }
}

fn time(ts: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(ts)
        .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

/// A window of the message around the first hit, with all query words highlighted.
fn snippet(text: &str, q: &Query) -> String {
    let flat: Vec<char> = extract::one_line(text, usize::MAX).chars().collect();
    let lower: Vec<char> = flat.iter().map(|c| c.to_lowercase().next().unwrap_or(*c)).collect();
    let mut marks = vec![false; flat.len()];
    for (i, term) in q.terms.iter().enumerate() {
        let tc: Vec<char> = term.chars().collect();
        let prefix_ok = q.last_prefix && i + 1 == q.terms.len();
        let mut pos = 0;
        while pos + tc.len() <= lower.len() {
            let starts = pos == 0 || !lower[pos - 1].is_alphanumeric();
            let ends = pos + tc.len() == lower.len() || !lower[pos + tc.len()].is_alphanumeric();
            if starts && (ends || prefix_ok) && lower[pos..pos + tc.len()] == tc[..] {
                marks[pos..pos + tc.len()].iter_mut().for_each(|m| *m = true);
                pos += tc.len();
            } else {
                pos += 1;
            }
        }
    }
    let first = marks.iter().position(|&m| m).unwrap_or(0);
    let start = first.saturating_sub(SNIPPET_CHARS / 3);
    let end = (start + SNIPPET_CHARS).min(flat.len());
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    let mut on = false;
    for i in start..end {
        if marks[i] != on {
            out.push_str(if marks[i] { HL } else { RESET });
            on = marks[i];
        }
        out.push(flat[i]);
    }
    if on {
        out.push_str(RESET);
    }
    if end < flat.len() {
        out.push('…');
    }
    out
}
