<div align="center">

<img src="packaging/icon.png" alt="Agentory" width="120">

# Agentory

**Many Claude Code sessions. One window.**

A native Windows desktop app that runs the real [Claude Code](https://claude.com/claude-code) TUI in
as many parallel sessions as you need, each optionally isolated in its own git worktree, with
per-project skill, docs, git and GitHub management built in.

[![CI](https://github.com/EvilPrime98/agentory/actions/workflows/ci.yml/badge.svg)](https://github.com/EvilPrime98/agentory/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/EvilPrime98/agentory)](https://github.com/EvilPrime98/agentory/releases)
![Platform](https://img.shields.io/badge/platform-Windows-0078D4)
![Rust](https://img.shields.io/badge/built%20with-Rust%20%2B%20GPUI-orange)
[![License](https://img.shields.io/badge/license-GPL--3.0--or--later-blue)](LICENSE)

<img width="1445" height="946" alt="image" src="https://github.com/user-attachments/assets/c80f2968-6738-4874-9750-01b848f56bce" />

</div>

## Why Agentory

Running Claude Code on more than one task usually means a pile of terminal windows and no idea which
one is waiting on you. Agentory puts them in one place and tells you.

- **Real Claude Code, not a wrapper.** Each session is the actual `claude` TUI in an embedded PTY
  (ConPTY), rendered by a `vt100` terminal grid. Permission prompts, slash commands and everything
  else behave exactly as in your terminal.
- **Project sidebar with live status.** Sessions are grouped under their project, and a status dot
  shows whether each one is working, waiting for input, or done. The eye button on the sidebar
  hides the page rows (Skills, Settings, Docs, Git, GitHub, Workflows, Graphify, Office, History) you
  don't use.
- **Worktree isolation.** Start a session in a fresh `git worktree` on its own branch
  (`agentory/<id>`) so parallel agents never step on each other's files.
- **Skills, managed per project.** A built-in skills page reads `spm.json` and `.claude/skills`,
  installs, updates and removes skills with a built-in `spm` (same `spm.json` and cache as the
  [`spm`](https://github.com/EvilPrime98/claude-spm) CLI, which keeps working on the same projects),
  searches the SkillsMP market (set `SKILLSMP_API_KEY` for a higher quota), installs market skills pinned to
  the commit you reviewed, updates them to the latest commit after a new review, installs into the global
  skills folder, and shows each skill's context cost, usage and overlaps.
- **Claude Code settings editor.** The **Settings** row under **Global** edits
  `~/.claude/settings.json` (or the one in `CLAUDE_CONFIG_DIR`): every option usable in that file,
  with search, typed controls, JSON editors for the complex keys and masked secrets in `env`. The
  list of options ships with the app and is refreshed with
  `python scripts/gen-claude-settings-catalog.py`.
- **Agent office.** A pixel-art office page shows each agent session as a worker whose pose follows its
  status. Click a worker to steer it, drag a skill onto one, or use the shared task board: a foreman
  drafts a task plan from a GitHub issue or markdown file for one-click approval, workers claim
  tasks, and a finished write task opens a PR into `dev`.
- **Voice dictation.** Optional, fully local (Whisper); toggle it with `Ctrl+Alt+M`.
- **Leaves your setup alone.** Status comes from hooks injected with
  `claude --settings <generated file>`, so tracking a session edits none of your Claude settings, and
  sessions started outside Agentory are unaffected.
- **Small and native.** One Rust binary built on [GPUI](https://github.com/zed-industries/zed), with
  no Electron and no web runtime.

<img width="1440" height="943" alt="image" src="https://github.com/user-attachments/assets/c7ddc63d-0ee0-4695-920a-c8c652ad58ad" />

## Install

**Installer (recommended).** Download the latest `Agentory-Setup-<version>.exe` from the
[Releases page](https://github.com/EvilPrime98/agentory/releases) and run it.

**From source.**

```
git clone https://github.com/EvilPrime98/agentory.git
cd agentory
cargo run --release
```

Requirements: Windows 10 1809 or later (ConPTY), a recent stable Rust toolchain, and `claude` and `git`
on your `PATH`.

## Usage

Add a project folder from the sidebar, then press **+** to start a session, either in the project
folder or in a new worktree.

| Keys | Action |
| --- | --- |
| `Ctrl+Alt+B` | Toggle the sidebar |
| `Ctrl+Alt+S` | Open the skills page |
| `Ctrl+Alt+T` | Open a terminal in the selected project |
| `Ctrl+Alt+M` | Toggle voice dictation in the active session |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | Next / previous session |
| `F11` | Fullscreen |
| `Ctrl+Shift+V` or `Shift+Insert` | Paste |
| `Ctrl+Shift+C` | Copy the selection |
| `Ctrl+C` | Copy while text is selected, otherwise interrupt |

Everything else goes straight to the PTY.

App data lives in `%APPDATA%\Agentory` (preferences, projects) and `%LOCALAPPDATA%\Agentory` (logs,
status files, worktrees).

<details>
<summary>Developer environment variables</summary>

```
AGENTORY_SHELL=1       # PowerShell instead of claude
AGENTORY_OPEN=settings # open a page at startup (settings or sessions)
AGENTORY_AUTOSTART=1   # start a session in the selected project
AGENTORY_CONFIG_DIR=<dir> # use another preferences/projects folder
AGENTORY_GRAPHIFY=<exe>   # graphify binary to run instead of the one on PATH
```

</details>

## Self-building (experimental, opt-in)

Agentory can change itself. When you enable **self-build**, it clones its own public source at your
installed version's tag. You then open a **Modify session**: an agent CLI such as Claude Code edits
that clone inside a worktree, on your behalf.

Nothing is installed until you approve it:

1. You review the diff and press **Apply**.
2. Agentory merges it into your local branch, builds it, and runs the **gate**: the test suite plus
   a smoke launch that the new exe must survive for 10 seconds.
3. A separate **helper** binary, which agents cannot edit, swaps in the new exe and relaunches. If
   the new build fails to start, the helper reverts to the last known-good exe.

Self-build is off by default and aimed at developers with Rust, git and an agent CLI installed.

## Status

Agentory is early (0.1) and Windows-only.

## License

[GPL-3.0-or-later](LICENSE)
