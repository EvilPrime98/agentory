# Agentory

A native Windows desktop app (GPUI) that hosts several [Claude Code](https://claude.com/claude-code)
sessions side by side and manages each project's skills through the
[`spm`](https://github.com/EvilPrime98/mySkills) CLI. Design notes: [`docs/agentory.md`](docs/agentory.md).

- **Sessions**: each one runs the real `claude` TUI in an embedded PTY (ConPTY), drawn by a `vt100`
  terminal grid. Tabs per session; a project sidebar with status dots.
- **Isolation**: optionally start a session in a fresh `git worktree` + branch (`agentory/<id>`);
  `spm install` runs there so `.claude/skills` is present.
- **Status**: sessions are launched with `claude --settings <generated file>` whose hooks
  (`Notification`, `Stop`, `UserPromptSubmit`) write to `%APPDATA%\Agentory\status\<id>.status`.
  No project or global settings are edited.
- **Skills drawer**: reads `spm.json` / `.claude/skills`, shells out to `spm` for install, uninstall,
  update and repo changes, and shows `spm doctor --json` context cost.

## Run

```
cargo run                      # needs `claude`, `spm` and `git` on PATH
AGENTORY_SHELL=1 cargo run     # dev aid: PowerShell instead of claude
AGENTORY_OPEN=settings         # dev aid: open a page at startup
AGENTORY_AUTOSTART=1           # dev aid: start a session in the selected project
```

Keys: `Ctrl+Alt+B` sidebar, `Ctrl+Alt+S` skills drawer, `F11` fullscreen,
`Ctrl+Shift+V` paste, `Ctrl+Shift+C` copy the visible screen. Everything else goes to the PTY.

App data: `%APPDATA%\Agentory` (prefs, projects), `%LOCALAPPDATA%\Agentory` (logs, status, worktrees).

## Not done yet

Split panes, mouse selection in the terminal, session persistence across restarts, worktree cleanup
on close, Linux/macOS builds. Licence: GPL-3.0-or-later.
