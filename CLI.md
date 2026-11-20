# Agentory CLI

Agentory ships a small command-line interface on the same binary as the desktop app. It lets you (or an agent running inside an Agentory session) read and change app settings and manage project workflows without opening the UI. Changes apply live: the running app picks them up automatically.

```
agentory settings <command>
agentory workflow <command>
agentory graphify <command>
agentory commands <command>
```

Any other invocation of `agentory` launches the desktop app.

Agent sessions started by Agentory get the CLI on their `PATH` (a small `agentory` / `agentory.cmd` shim is written to `<config dir>/cli`), so an agent can call it directly.

## Conventions

| Topic | Behavior |
| --- | --- |
| Output | Human-readable on a terminal. JSON when stdout is not a terminal, or when `--json` is passed. |
| `--help`, `-h` | Print help for the area and exit `0`. Running an area with no command prints help and exits `2`. |
| Errors | Written to stderr (as `{"error": {"code", "message", "allowed"}}` in JSON mode). |
| Exit codes | `0` success, `1` file/IO error, `2` bad usage, invalid value or invalid workflow. |

## `agentory settings`

Read and change the app settings stored in `prefs.json` (in the Agentory config directory, e.g. `%APPDATA%\Agentory` on Windows).

| Command | Description |
| --- | --- |
| `agentory settings list` | Print every setting and its current value. With `--json`, also includes default, type, allowed values and description. |
| `agentory settings get KEY` | Print the current value of one setting. |
| `agentory settings set KEY VALUE` | Validate and save a new value. |
| `agentory settings reset KEY` | Restore the default value. |

```sh
agentory settings list
agentory settings set theme dark
agentory settings set terminal-font-size 15
agentory settings get notifications
agentory settings reset claude-command
```

### Settings

| Key | Type | Allowed | Default | Description |
| --- | --- | --- | --- | --- |
| `theme` | choice | `system`, `light`, `dark` | `system` | Light or dark palette. `system` follows the OS appearance. |
| `language` | choice | `system`, `english`, `spanish` | `system` | Interface language. `system` follows the OS locale. |
| `claude-command` | string | any | empty | Program started in each agent session. Empty means `claude` from `PATH`. |
| `ask-worktree` | bool | `true`, `false` | `true` | Offer a git worktree when starting a session in a git project. |
| `send-doc-prompt` | bool | `true`, `false` | `false` | Submit the prompt, not just type it, when running a doc from the Docs page. |
| `doc-prompt` | string | any | `@docs/{doc} -> implement` | Prompt for running a doc from the Docs page. `{doc}` is the doc path under `docs/`. Empty restores the default. |
| `notifications` | bool | `true`, `false` | `true` | Toast when a session needs input or finishes. |
| `fullscreen-agents` | bool | `true`, `false` | `true` | Launch agent sessions in Claude Code's fullscreen renderer. |
| `terminal-font-size` | number | `9` to `28` | `13` | Terminal font size in points. |

The exact list is always available from `agentory settings --help`.

## `agentory workflow`

Manage the workflows of a project: multi-step prompt pipelines stored as JSON under `<project>/.agentory/workflows/`.

The project is the current directory, or the nearest parent containing a `.agentory` folder. Pass `--project PATH` to point elsewhere.

| Command | Description |
| --- | --- |
| `agentory workflow list` | List workflows (id, name, step count). |
| `agentory workflow show ID` | Show a workflow with all its steps. |
| `agentory workflow create NAME --prompt TEXT [--step-name N] [--model M] [--skill S]...` | Create a workflow with a first step. `--prompt` is required. The id is derived from the name and made unique. |
| `agentory workflow apply ID\|- [--file PATH]` | Replace a whole definition from JSON (stdin by default). `-` takes the id from the JSON. Unknown ids are created. |
| `agentory workflow rename ID NAME` | Rename a workflow. |
| `agentory workflow delete ID [--yes]` | Delete a workflow. `--yes` is required when running on a terminal. |
| `agentory workflow step add ID [--at POS] [--name N] [--prompt P] [--model M] [--skill S]...` | Insert a step (appended unless `--at` is given). |
| `agentory workflow step set ID POS [--name N] [--prompt P] [--model M] [--skill S]... [--clear-skills]` | Edit fields of one step. Only the flags you pass change. |
| `agentory workflow step rm ID POS` | Remove a step. |
| `agentory workflow step move ID FROM TO` | Move a step to another position. |

### Flags

| Flag | Description |
| --- | --- |
| `--project PATH` | Project directory (default: auto-detected from the current directory). |
| `--json` | JSON output. |
| `--yes` | Confirm `delete` on a terminal. |
| `--skill S` | Repeatable. Setting skills on `step set` replaces the existing list. |
| `--clear-skills` | Remove all skills from a step. |

Step positions are **1-based**.

### Workflow JSON

```json
{
  "id": "review",
  "name": "Review",
  "steps": [
    { "name": "Analyze", "prompt": "Analyze: {{input}}", "model": "", "skills": [] },
    { "name": "Summarize", "prompt": "Summarize this:\n{{prev}}", "model": "", "skills": [] }
  ]
}
```

### Prompt variables

| Variable | Replaced with |
| --- | --- |
| `{{input}}` | The input given when the workflow is run. |
| `{{prev}}` | Output of the previous step. |
| `{{stepN}}` | Output of step `N`. Only earlier steps are allowed. |

### Validation

A workflow is rejected (exit `2`) when:

- the id contains anything other than letters, digits, `-` or `_`;
- the name is empty;
- there are no steps;
- a step has an empty name or prompt;
- a prompt references `{{stepN}}` for a step that has not run yet.

### Examples

```sh
# Create, then extend
agentory workflow create "Bug triage" --prompt "Reproduce: {{input}}" --step-name Reproduce
agentory workflow step add bug-triage --name Fix --prompt "Fix the bug found in:\n{{prev}}"
agentory workflow step move bug-triage 2 1

# Replace from a file or stdin
agentory workflow apply bug-triage --file triage.json
cat triage.json | agentory workflow apply -

# Work on another project
agentory workflow list --project ~/code/my-app

# Delete without a prompt
agentory workflow delete bug-triage --yes
```

## `agentory graphify`

Show and refresh a project's [graphify](https://github.com/safishamsi/graphify) knowledge graph. The project is the current directory (or the nearest parent with a `.agentory` folder); `--project PATH` overrides. It shells out to `graphify` on `PATH`.

| Command | Does |
| --- | --- |
| `agentory graphify status` | Installed version, graph present, node count, last built (unix seconds), stale |
| `agentory graphify update` | Runs `graphify update .` (AST only, no API cost) |

A full build uses an LLM, so it is started from the Graphify page in the app (or with `/graphify` in a session), not from the CLI.

`status` JSON: `{"installed": bool, "version": str|null, "graph": bool, "nodes": int|null, "built_at": int|null, "stale": bool, "report": path|null}`.

If `graphify` is missing, `status` reports `installed: false` and `update` fails with error code `not_installed` (exit 2).

```sh
agentory graphify status
agentory graphify update --project ~/code/my-app
```

## `agentory commands`

Manage a project's custom commands and default shell (the commands section of the sidebar). The project is the registered project containing the current directory; `--project PATH` overrides. The CLI never registers projects: add them in the app. Running a command stays in the app.

| Command | Does |
| --- | --- |
| `agentory commands list` | Custom commands, then detected ones (package.json, Cargo.toml, pyproject, Makefile, ...) |
| `agentory commands add NAME --command CMD [--shell S]` | Add a custom command; the id is derived from the name and made unique |
| `agentory commands set ID [--name N] [--command CMD] [--shell S] [--clear-shell]` | Edit a custom command |
| `agentory commands rm ID [--yes]` | Remove a custom command (`--yes` required on a terminal) |
| `agentory commands shell [NAME\|--clear]` | Read, set or clear the project's default shell |

Shell values are the ids available on this machine: `pwsh`, `powershell`, `cmd`, `git-bash`, `wsl` on Windows, `shell` elsewhere. Others fail with error code `invalid` and the allowed list.

`list` JSON: an array of `{"id", "name", "command", "shell", "source"}` where `source` is `custom` or `detected`. Detected commands have `id: null` and are read-only.

Errors (all exit `2`): `not_registered` (directory is not inside a registered project), `not_found` (no custom command with that id), `read_only` (the id given is a detected command), `invalid`, `usage`, `confirm_required`.

```sh
agentory commands add "Dev server" --command "npm run dev" --shell pwsh
agentory commands set dev-server --clear-shell
agentory commands shell git-bash
agentory commands rm dev-server --yes
```
