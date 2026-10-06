# claude-code-search

Personal CLI helpers for [Claude Code](https://claude.com/claude-code).

## Install

```bash
git clone https://github.com/tstanmay13/claude-code-search.git
cd claude-code-search && ./install.sh
```

`install.sh` builds the Rust binary (`cargo build --release`) and symlinks it
into `~/.local/bin`, so `git pull && ./install.sh` updates the live command.

**Dependencies:** a Rust toolchain to build ([rustup](https://rustup.rs)),
[fzf](https://github.com/junegunn/fzf) (`brew install fzf`), and the `claude` CLI.

## `ccs` — Claude Code Search

You remember discussing something with Claude — an error message, a library,
a ticket, a decision — but not *which* session or *which* project it was in.
`ccs` ranks **all** Claude Code sessions on this machine for your query, lets
you pick one in fzf, and resumes it from its original directory.

```bash
ccs "connection timeout"   # find the session where you debugged that error
ccs mcp                    # sessions about MCP, the mcp-* worktree first
ccs                        # no query: browse all sessions, newest first
ccs --print p0             # print ranked results with scores, no picker
```

### What gets searched

Only the conversation: your prompts, Claude's replies, tool calls and tool
output, plus each session's title (custom title, else AI title, else first
real prompt) and project directory and branch. System reminders, tool and skill
lists, thinking blocks, images and base64 blobs are skipped. They are stored in
every transcript, so including them made every query match every session.

### How results are ranked

1. **Exact match first.** Sessions containing the whole query as a phrase
   (for one word: as a whole word, so `mcp` beats `mcpServers`).
2. **Then by how many query words appear.**
3. **Then a field-weighted BM25 score.** More mentions rank higher, with
   diminishing returns per field. Weights: title 12, project 8, your prompts 4,
   Claude's replies 2, agent/task messages 0.75, tool calls 0.5, tool output 0.25.
   A recency bonus of at most 10% breaks near-ties.

The last word also matches as a prefix, so results update sensibly while you
type. Each row shows why it matched, e.g. `title proj you:6 claude:91`, and the
preview shows the best-matching messages with who wrote them and when.

### Speed

Extracted text is cached in `~/.cache/ccs/index.bin` and only changed
transcripts are re-read. fzf only displays results: every keystroke re-runs
`ccs --rank`, which takes about 40 ms on ~100 sessions. `ccs --reindex` rebuilds
the cache from scratch.

fzf keys: type to re-rank, `↑/↓` to move (preview follows), `Enter` to
resume, `Esc` to cancel.
