# MCP server

`fire cli mcp` starts a [Model Context Protocol](https://modelcontextprotocol.io) server over
stdio, exposing configured commands, managed long-running sessions, and version-matched Fire
documentation to agents.

It gives agents three things:

1. **The commands you already configured.** Every available `fire` command, with its exact
   invocation path and description, so the agent uses `run_command` instead of guessing or
   writing its own build/test/run commands. Commands marked `long_running: true` say so, so the
   agent knows to start them with `start_session`.
2. **Managed long-running processes.** Agents start dev servers, watchers, and similar processes
   as [detached sessions](./sessions.md), each with a short note explaining why. You and every
   other agent can then see them from that project (or explicitly list every project), read their
   output, send input or signals, and stop them, and you can attach with `fire cli fg <id>`.
3. **Version-matched Fire documentation.** The `get_docs` tool exposes its full topic index in the
   tool description and lets an agent read any embedded guide or the config schema, so it can
   author configuration without relying on documentation for a different release.

The server's instructions are a short decision guide, on one line with the critical rule first
(some clients only show the first line, or a truncated prefix):

1. Long-running commands always go through `start_session`, never the agent's own shell.
2. Find the command with `search_commands` before writing one.
3. Check `list_sessions` and reuse what is already running.
4. Long-running (`long_running: true`, or it doesn't exit on its own): `start_session` with a
   note, then `session_output`. Finite (build, test, lint): `run_command`.
5. Manage sessions with `session_output`, `send_input`, `signal_session`, `stop_session` and
   `forget_session`; never run `agents: false` commands; if the project has no config, use
   `init_project`, then use `get_docs` to edit it; change global directories only when the user
   asks.

Tool descriptions say what each tool does and when to use it. Hints meant for you stay here:
attach to any agent's session with `fire cli fg <id>` (list them with `fire cli ps`), and start
your own detached sessions with `fire --bg <command>`. Commands resolve against the same merged
config Fire uses (local `fire.yaml` / `*.fire.yaml` plus registered global directories; see
[Overview](./overview.md#file-discovery-and-merge-order)).

Pass your working directory as `cwd` to see and run that project's local commands when the MCP
server was started elsewhere.

## Usage
Register Fire at user scope so it is available in every project; Fire picks the project from the
directory the host starts it in:

```bash
fire cli mcp install <host>
```

Hosts are `claude`, `claude-desktop`, `cursor`, `vscode`, `codex`, `gemini`, and `windsurf`.
Omit `<host>` in a terminal to select from detected hosts. In a script or other non-interactive
input, specify the host explicitly. Use `--print` to inspect the command or file change without
applying it, and remove a registration with `fire cli mcp uninstall <host>`.

Fire uses the host's CLI for Claude Code, VS Code, and Codex when it is on `PATH`. File-based hosts,
and VS Code or Codex without their CLI, use the host's user configuration. Fire preserves unrelated
entries and writes the previous file to a neighboring `.bak` file; repeating a file-based
installation makes no change. If the Claude Code CLI is missing, Fire prints the command to run
after installing it. When `fire` isn't on `PATH`, generated configuration uses the absolute path of
the running binary.

The manual commands and file formats below are fallbacks for hosts that you do not want Fire to
configure.

**Claude Code**
```bash
claude mcp add --scope user fire -- fire cli mcp
```

**Codex**
```bash
codex mcp add fire -- fire cli mcp
```
Or in `~/.codex/config.toml`:
```toml
[mcp_servers.fire]
command = "fire"
args = ["cli", "mcp"]
```

**VS Code (Copilot agent mode)**
```bash
code --add-mcp '{"name":"fire","command":"fire","args":["cli","mcp"]}'
```
Or in a workspace `.vscode/mcp.json` (note the `servers` key):
```json
{
  "servers": {
    "fire": { "command": "fire", "args": ["cli", "mcp"] }
  }
}
```

**Cursor, Claude Desktop, Gemini CLI, Windsurf** and most other hosts use the same `mcpServers`
shape:
```json
{
  "mcpServers": {
    "fire": { "command": "fire", "args": ["cli", "mcp"] }
  }
}
```

| Host | File |
|---|---|
| Cursor | `~/.cursor/mcp.json` (or `.cursor/mcp.json` in a project) |
| Claude Desktop | macOS: `~/Library/Application Support/Claude/claude_desktop_config.json`; Linux: `~/.config/Claude/claude_desktop_config.json` |
| VS Code | macOS: `~/Library/Application Support/Code/User/mcp.json`; Linux: `~/.config/Code/User/mcp.json` |
| Gemini CLI | `~/.gemini/settings.json` (or `.gemini/settings.json` in a project) |
| Windsurf | `~/.codeium/windsurf/mcp_config.json` |

Tips:
- GUI apps on macOS may not inherit your shell's `PATH`. If the host can't find `fire`, use the
  absolute path from `which fire` as `command`.
- Hosts that aren't tied to a project directory (Claude Desktop, for example) only see the
  commands from your [registered global directories](./overview.md#file-discovery-and-merge-order).
- Check it works by asking the agent "what fire commands are available here?".

## Keeping commands away from agents
Some commands should stay human-only: production deploys, destructive database tasks, anything
that spends money. Mark them with `agents: false`:
```yaml
commands:
  deploy:
    description: Deploy to production
    agents: false
    commands:
      prod: ./scripts/deploy prod
  db:
    commands:
      migrate: ./scripts/migrate
      drop:
        agents: false
        exec: ./scripts/drop-db
```
- The command, and everything under it, is left out of `search_commands`. A
  subcommand can't opt back in under a group marked `agents: false`.
- `run_command` and `start_session` refuse to run it, with an error telling the agent not to run
  it any other way and to ask you instead.
- A command exposed to agents is also refused before it starts if any command in its `needs` graph
  has `agents: false`; dependencies cannot bypass the restriction.
- The server's instructions tell agents that these commands are off-limits, including from their
  own shell.
- `fire` help marks them as `[no agents]`, so you can see what's restricted without opening the
  YAML.

You can still run these commands yourself, including with `fire --bg`.

This steers well-behaved agents. It is not a sandbox: an agent with shell access can still read
your config or type `fire deploy prod`. To hard-block that, also add a deny rule to your agent's
own permissions (for example, in Claude Code: `"deny": ["Bash(fire deploy:*)"]`).

## Tools

### `init_project`
Creates a minimal config in the required `cwd`. Optional `file` is a base name that follows the
same naming convention as `fire cli init`; optional `namespace` and `group` values are preserved
exactly. The structured result includes `ok`, the absolute `path`, and an `error` on failure. It
never overwrites: if the target exists, the result names that path. After creation, call
`get_docs` for the format and schema, edit the file, then pass the same `cwd` to
`search_commands`; changes are discovered immediately. Global registration is separate and
should only happen when the user asks.

### `get_docs`
Omit `topic` to return the complete index of embedded documentation. Pass one of the topic ids
shown in the tool description and its `topic` enum to return that Markdown document; `schema`
returns `fire.schema.json`. The index, topic enum, completion, and CLI all come from the same
embedded files.

### `search_commands`
The single command-discovery tool. Omit `query` (or pass an empty string) to return every command,
or provide a case-insensitive substring to match against `path`, `description`, `exec`, or `eval`.
Pass `cwd` to discover local commands as if Fire were run from that directory; without it, Fire
uses the MCP server's directory. Relative paths resolve from the server's directory.
Each flat result puts `path` first: it is the exact canonical invocation to run, such as
`"fire ex backend logs"`. Results also include `description`, `runnable`, `source` (`local` or
`global`), a custom `placeholder` when configured, and `long_running: true` for commands that
must use `start_session`. A `needs` array names the commands that Fire runs first.

Underlying `exec` and `eval` templates are omitted by default. Pass `include_exec: true` only when
their implementation is useful as context; run the returned `path`, never a copied template.

### `run_command`
Runs a finite configured command to completion. Pass `path` exactly as returned by
`search_commands` (including the leading `fire`) and optional `args` as an array; each argument is
appended as one unchanged token. Pass the same `cwd` used for discovery so the command resolves
and runs in that project; without it, Fire uses the MCP server's directory. The command is stopped
after 60 seconds. Fire runs the command's complete `needs` graph under the same timeout. For longer
builds or tests, run Fire from your shell; commands intended to keep running must be marked
`long_running: true` and started with `start_session`.

The structured result contains `exit_code`, separate `stdout` and `stderr`, `timed_out`, and
`stdout_truncated` / `stderr_truncated`. Each output stream is capped at 64 KiB. `exit_code` is
null when the command times out, cannot start, has no executable action, is unknown, is marked
`long_running: true`, or is unavailable because of `agents: false`. A long-running refusal points
the agent to `start_session`.

### `list_sessions`
Returns detached sessions started in the current Fire project by default, including sessions
started from another directory that resolves to the same project. Pass `cwd` to select another
project (relative paths resolve from the MCP server's directory), or `all: true` to return
sessions from every project; `all: true` ignores `cwd`. Sessions may have been started by any
agent or by you. Each result contains `id`, `command`, `note` (the short reason it was started),
`cwd`, `status` (`running`, `exited`, `stopped` when someone stopped it on purpose, `lost`),
`pid`, `exit_code`, and timestamps.

### `session_output`
Takes an `id` and optional `lines` (default 100). Returns the session plus the last lines of its
terminal `output` as plain text: stdout and stderr merged (sessions run on a pty), escape
sequences removed and `\r` progress redraws collapsed.

### `start_session`
Takes `command` (the fire path plus arguments as a token array, without `fire`, e.g.
`["api", "dev"]`), a required short `note`, and an optional `cwd` (project directory, defaulting to
the server's). Same as `fire --bg -m "<note>" <command...>`, except that it refuses commands
marked `agents: false`. Returns the session id in `session_id` (and, for compatibility, `stdout`).
One command in one `cwd` is one session: if it is already running, the tool returns that id and
replaces its note; if it ended, it starts again under the same id. So a restart is
`stop_session` followed by `start_session`, and the session list doesn't fill up with copies of
the same server.

The result is MCP structured content (with an output schema), using the same CLI envelope as the
other management tools and adding `session_id` on success. MCP also includes its JSON text form
for clients that do not read structured content:

```json
{
  "ok": true,
  "session_id": "api-dev-a1b2c3",
  "stdout": "api-dev-a1b2c3",
  "stderr": ""
}
```

### `send_input`
Takes `id` and `text`, and types the text followed by Enter into the session's terminal
(`fire cli send`). With `raw: true`, sends `text` as exact keys without Enter, interpreting `\r`,
`\n`, `\t`, `\e`, `\\` and `\xHH`: e.g. `q`, `\x03` (Ctrl+C) or `\e[A` (up arrow)
(`fire cli send --raw`).

### `stop_session`
Takes `id` and stops that running session gracefully (`TERM`, then `KILL` after 5s if needed).

### `signal_session`
Takes `id` and a required `signal` (name like `INT`, `HUP`, `USR1`, or a number), which is sent to
the running session's foreground process group, as a keyboard signal would be.

### `forget_session`
Takes `id` and removes the saved record of an exited, stopped, or lost session. It refuses to
remove a running session; stop it first with `stop_session` if that is the intent.

Management tools return `{ ok, stdout, stderr }` as MCP structured content. `start_session` also
returns `session_id` on success.

### Global directories
The MCP counterpart of [`fire cli global`](./overview.md#global-directories), with the same semantics.

- `list_global` (read-only): every registered directory with its absolute `path`, whether it
  still `exists`, and its `namespaces`. Each namespace (`null` for commands without one) lists the
  absolute paths of the config `files` that define it, so an agent can open the right file to edit
  a command, and whether it is `merged`. The top-level `merged` lists merged namespaces.
- `add_global` (`path`): register a directory.
- `remove_global` (`namespace` or `path`, exactly one): unregister it. Files are never touched.
- `prune_global`: unregister directories that no longer exist.
- `merge_global` / `unmerge_global` (`namespace`): call a namespace's commands without (or again
  with) the namespace word.

Paths must be absolute (a leading `~` means the home directory): a relative path would resolve
against the server's directory, so it is rejected instead of guessed. These tools change the
user's global setup; their descriptions tell agents to use them only when the user asks. They
return structured results: `ok`, `error`, the affected `path`, and per tool `registered` /
`already_registered` / `has_config`, `removed`, `pruned`, `merged` / `already_merged`. The CLI's
first-use registration prompt never appears through MCP.

## Testing manually
Use the [MCP inspector](https://github.com/modelcontextprotocol/inspector) to poke at the server
without wiring up a full agent host:
```bash
npx @modelcontextprotocol/inspector -- fire cli mcp
```
