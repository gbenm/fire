# MCP server

`fire cli mcp` starts a [Model Context Protocol](https://modelcontextprotocol.io) server over
stdio. It gives agents two things:

1. **The commands you already configured.** Every available `fire` command, with its invocation
   path, description, and `exec`/`eval` templates, so the agent runs `fire ...` instead of guessing
   or writing its own build/test/run commands.
2. **Managed long-running processes.** Agents start dev servers, watchers, and similar processes
   as [detached sessions](./sessions.md), each with a short note explaining why. You and every
   other agent, in any project, can then see them, read their output, send input or signals, and
   stop them, and you can attach with `fire cli fg <id>`.

The server's instructions tell agents to do both: search configured commands first, and always
launch long-running processes through `start_session` (checking `list_sessions` first to avoid
duplicates). Commands resolve against the same merged config Fire uses (local `fire.yml` /
`*.fire.yml` plus installed global directories; see
[Overview](./overview.md#file-discovery-and-merge-order)).

## Usage
Point an MCP-capable host (Claude Code, Claude Desktop, Cursor, etc.) at:
```bash
fire cli mcp
```

Example host config (adjust to your tool's format):
```json
{
  "mcpServers": {
    "fire": {
      "command": "fire",
      "args": ["cli", "mcp"]
    }
  }
}
```

## Tools

### `list_commands`
Returns the full command tree as JSON: `fire_version`, the active `implicit_local_namespace` (if
any), usage notes, and a `commands` array. Each node has:
- `path` — the canonical invocation, e.g. `"fire ex backend logs"`
- `name`, `namespace`, `group`, `source` (`local` or `global`)
- `description`
- `runnable` — `false` for group-only nodes (invoke a subcommand instead)
- `exec` / `eval` — the underlying template(s), for context only
- `placeholder` — the argument placeholder pattern, if customized
- `subcommands` — nested nodes, same shape

### `search_commands`
Takes a `query` string and returns a flat list of matches (same fields as above, minus
`subcommands`) whose `path`, `description`, `exec`, or `eval` contain the query
(case-insensitive substring match). Useful once the tree is large enough that returning it whole
isn't practical.

### `list_sessions`
Returns every detached session (started by any agent or by you, from any project): `id`,
`command`, `note` (the short reason it was started), `cwd`, `status` (`running`, `exited`, `lost`),
`pid`, `exit_code`, and timestamps.

### `session_output`
Takes an `id` and optional `lines` (default 100). Returns the session plus the last lines of its
`stdout` and `stderr`.

### `start_session`
Takes `command` (the fire path plus arguments as a token array, without `fire`, e.g.
`["api", "dev"]`), a required short `note`, and an optional `cwd` (project directory, defaulting to
the server's). Same as `fire cli bg -m "<note>" <command...>`. Returns the session id in `stdout`.

### `send_input`
Takes `id` and `text`, and writes the text plus a newline to the session's stdin (`fire cli send`).

### `signal_session`
Takes `id` and an optional `signal` (name like `INT`, `HUP`, `USR1`, or a number), which is sent to
the process group. Without `signal`, it stops the session (`TERM`, then `KILL` after 5s), or
removes the record of an already-exited one (`fire cli kill`).

Management tools return `{ ok, stdout, stderr }` from the equivalent CLI command.

## Testing manually
Use the [MCP inspector](https://github.com/modelcontextprotocol/inspector) to poke at the server
without wiring up a full agent host:
```bash
npx @modelcontextprotocol/inspector -- fire cli mcp
```
