# cc-tools

Personal CLI helpers for [Claude Code](https://claude.com/claude-code).

## Install

```bash
git clone https://github.com/tstanmay13/cc-tools.git
cd cc-tools && ./install.sh
```

`install.sh` symlinks everything in `bin/` into `~/.local/bin`, so a later
`git pull` updates the live commands automatically.

**Dependencies:** [ripgrep](https://github.com/BurntSushi/ripgrep), [fzf](https://github.com/junegunn/fzf), `jq`
(`brew install ripgrep fzf jq`), plus the `claude` CLI itself.

## Tools

### `ccs` — Claude Code Search

You remember discussing something with Claude — an error message, a library,
a ticket, a decision — but not *which* session or *which* project it was in.
`ccs` greps across **all** Claude Code sessions on this machine, lets you
pick the right one in fzf, and resumes it — in one command.

```bash
ccs "connection timeout"   # find the session where you debugged that error
ccs useReducer             # ...or discussed that API (rg smart-case regex)
ccs "JIRA-1234"            # ...or worked that ticket
ccs                        # no pattern: browse all sessions, newest first
```

What it does:

1. Searches `~/.claude/projects/**/*.jsonl` with `rg -l`, skipping
   non-resumable agent/sidechain transcripts
2. Shows an fzf picker — date, project dir, first prompt — newest first,
   with a preview pane showing the matching message text (readable, highlighted —
   not raw JSON)
3. On selection, reads the session's original `cwd` from the transcript,
   `cd`s there, and runs `claude --resume <session-id>` so resume finds
   the session in the right project

fzf keys: type to filter further, `↑/↓` to move (preview follows), `Enter`
to resume, `Esc` to cancel.
