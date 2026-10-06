# ccs v2: Rust ranked search

## Problem (measured 2026-10-06, 623 sessions)
- `ccs` greps raw JSONL. Tool lists, system reminders and base64 images live in every file.
  - "MCP": 618 raw matches vs 28 sessions that mention it in real conversation text.
  - "p0": 557 raw matches (base64 noise) vs 8 real. Top real hit has 11 mentions.
- Rows are sorted by mtime, not relevance. fzf then re-sorts by its own fuzzy score.
- Titles are junk (`<local-command-caveat>`, "Another Claude session sent a message:").
  `sessions-index.json` doesn't exist; `custom-title` / `ai-title` records are ignored.
- Project path (e.g. worktree `mcp-toolkit-public-api`) isn't used as a signal.

## Design
- Rust binary `ccs` (serde_json, no search-engine crate; ~600 docs fits an in-memory scan).
- Extract per session: title, project path + branch, user prompts, assistant text,
  tool inputs, tool outputs. Skip attachments, system reminders, thinking, images, isMeta.
- Tokenizer: lowercase, split on non-alphanumerics; drop tokens > 40 chars (base64/hashes).
- Cache extracted docs in `~/.cache/ccs/index` keyed by path + mtime + size; re-parse only changed files.
- Score: BM25F with field weights — title 10, project 6, user 4, assistant 1.5,
  tool input 0.5, tool output 0.25 — plus exact-phrase bonus and a small recency tiebreak.
  Last query term also matches as a prefix (live typing), at reduced weight.
- Title: custom-title > latest ai-title > agent-name > first clean user prompt.
- fzf as display only: `--disabled`, `change:reload:ccs --rank {q}`, preview `ccs --preview <sid> {q}`
  showing matching messages (role, time, highlighted snippet).
- Row shows hit counts per field (e.g. `title✓ proj✓ you:3 claude:11`).
- Resume flow unchanged (cd to original cwd, `claude --resume`).

## Steps
- [x] Cargo scaffold, JSONL extractor + title logic, unit tests on fixture transcripts
- [x] Tokenizer + cache
- [x] Ranker (BM25F) + ranking tests: MCP-worktree session first; p0 session with 11 mentions first;
      tool-list-only and base64-only sessions excluded
- [x] CLI: `--rank`, `--preview`, fzf wiring, resume
- [x] install.sh builds with cargo and symlinks; README update
- [x] Verify on real data: MCP + p0 queries, timing (cold index, warm query)

## Review
- 95 resumable sessions (529 more .jsonl files are subagent transcripts, skipped as before).
  Old grep matched 91/95 for "mcp" and 76/95 for "p0"; now 57 and 15, ranked.
- Changed from plan: per-field BM25 saturation (summed BM25F saturated at one title hit),
  B=0.3 with length ratio capped at 4x, K1=2. Phrase/whole-word tier above everything.
- `mcp`: mcp-toolkit worktree #1 (22.9 vs 11.8). `p0`: session where the user typed p0 #1,
  orchestrator (10 Claude mentions) #3. `connection timeout`: phrase sessions first.
- 9 unit + 10 integration tests pass; mutating title weight / tier order fails 3 of them.
- Index build 0.5s cold, 42 MB cache; --rank ~40 ms. fzf flow verified in a pty with a stub claude.
