//! ccs — Claude Code Search: rank all local Claude Code sessions for a query,
//! pick one in fzf, and resume it from its original directory.

use ccs::{index, preview, rank};
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

const USAGE: &str = "\
ccs — search all Claude Code sessions and resume one

usage:
  ccs [query...]          interactive picker (fzf), ranked as you type
  ccs --print [query...]  print the top results with scores, no picker
  ccs --reindex           rebuild the cache from scratch

Ranking: exact phrase first, then how many query words match, then a
field-weighted score (title > project > your prompts > Claude's replies >
tool calls > tool output). The last word also matches as a prefix.";

/// Rows handed to fzf. More than this is never useful in a picker.
const MAX_ROWS: usize = 300;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("-h" | "--help") => println!("{USAGE}"),
        // Internal: fzf reload. The cache is refreshed in memory but not
        // rewritten, because this runs on every keystroke.
        Some("--rank") => print_rows(&index::load(false), args.get(1).map_or("", String::as_str)),
        Some("--preview") => {
            let (path, q) = (args.get(1).map_or("", String::as_str), args.get(2).map_or("", String::as_str));
            print!("{}", preview::render(Path::new(path), q));
        }
        Some("--print") => print_scores(&index::load(true), &args[1..].join(" ")),
        Some("--reindex") => {
            let _ = std::fs::remove_file(index::cache_path());
            let docs = index::load(true);
            println!("indexed {} sessions into {}", docs.len(), index::cache_path().display());
        }
        _ => interactive(&args.join(" ")),
    }
}

fn now_secs() -> i64 {
    chrono::Local::now().timestamp()
}

fn short_home(p: &str) -> String {
    let home = index::home();
    let home = home.to_string_lossy();
    match p.strip_prefix(home.as_ref()) {
        Some(rest) if !home.is_empty() => format!("~{rest}"),
        _ => p.to_owned(),
    }
}

fn date(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        format!("{s:<n$}")
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    }
}

/// fzf rows: sid \t cwd \t path \t display
fn print_rows(docs: &[index::Doc], query: &str) {
    let q = rank::Query::parse(query);
    let ranked = rank::search(docs, &q, now_secs());
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    for r in ranked.iter().take(MAX_ROWS) {
        // Stop quietly when fzf closes the pipe on a newer keystroke.
        if writeln!(out, "{}", row_for(r)).is_err() {
            return;
        }
    }
}

fn print_scores(docs: &[index::Doc], query: &str) {
    let q = rank::Query::parse(query);
    let ranked = rank::search(docs, &q, now_secs());
    println!("{} sessions indexed, {} match {:?}", docs.len(), ranked.len(), query);
    for (i, r) in ranked.iter().take(20).enumerate() {
        println!(
            "{:>2}. {} m={} {:>7.2}  {}  {}  {}  [{}]",
            i + 1,
            if r.exact { "E" } else { " " },
            r.matched,
            r.score,
            date(r.doc.mtime_secs),
            truncate(&index::project_label(&r.doc.cwd), 34),
            truncate(&r.doc.title, 60).trim_end(),
            rank::badges(r),
        );
    }
}

fn interactive(query: &str) {
    if Command::new("fzf").arg("--version").stdout(Stdio::null()).status().is_err() {
        eprintln!("ccs: fzf is required (brew install fzf)");
        std::process::exit(1);
    }
    // Refresh and persist the cache once up front; reloads then only re-read
    // the few transcripts that change while the picker is open.
    let docs = index::load(true);
    if docs.is_empty() {
        eprintln!("ccs: no sessions found in {}", index::projects_dir().display());
        std::process::exit(1);
    }

    let exe = std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|_| "ccs".into());
    let exe = shell_quote(&exe);
    let reload = format!("reload({exe} --rank {{q}})");

    let mut child = Command::new("fzf")
        .args([
            "--ansi",
            "--disabled",
            "--no-multi",
            "--delimiter=\t",
            "--with-nth=4",
            "--query",
            query,
            "--prompt=ccs> ",
            "--header=ranked: exact phrase > words matched > title/project/you/claude/tools • Enter resumes",
            "--preview-window=down,45%,wrap",
        ])
        .arg(format!("--bind=change:{reload}"))
        .arg(format!("--preview={exe} --preview {{3}} {{q}}"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| {
            eprintln!("ccs: failed to start fzf: {e}");
            std::process::exit(1);
        });

    // Initial rows come from the docs already loaded, so the first screen is instant.
    {
        let stdin = child.stdin.take().expect("fzf stdin");
        let mut w = std::io::BufWriter::new(stdin);
        let q = rank::Query::parse(query);
        for r in rank::search(&docs, &q, now_secs()).iter().take(MAX_ROWS) {
            let _ = writeln!(w, "{}", row_for(r));
        }
    }
    let output = child.wait_with_output().unwrap_or_else(|e| {
        eprintln!("ccs: fzf failed: {e}");
        std::process::exit(1);
    });
    let selected = String::from_utf8_lossy(&output.stdout);
    let mut cols = selected.trim_end_matches('\n').split('\t');
    let (Some(sid), Some(cwd)) = (cols.next().filter(|s| !s.is_empty()), cols.next()) else {
        std::process::exit(130);
    };

    if Path::new(cwd).is_dir() {
        let _ = std::env::set_current_dir(cwd);
    } else {
        eprintln!("ccs: original cwd '{cwd}' no longer exists; resuming from $HOME");
        let _ = std::env::set_current_dir(index::home());
    }
    let here = std::env::current_dir().map(|p| short_home(&p.to_string_lossy())).unwrap_or_default();
    eprintln!("Resuming {sid} in {here}");
    let err = Command::new("claude").args(["--resume", sid]).exec();
    eprintln!("ccs: failed to run claude: {err}");
    std::process::exit(1);
}

fn row_for(r: &rank::Ranked) -> String {
    let d = r.doc;
    let badges = rank::badges(r);
    let badges = if badges.is_empty() { String::new() } else { format!("  \x1b[33m{badges}\x1b[0m") };
    format!(
        "{}\t{}\t{}\t\x1b[2m{}\x1b[0m  \x1b[36m{}\x1b[0m  {}{}",
        d.sid,
        d.cwd,
        d.path,
        date(d.mtime_secs),
        truncate(&index::project_label(&d.cwd), 34),
        truncate(&d.title, 90).trim_end(),
        badges,
    )
    .replace('\n', " ")
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}
