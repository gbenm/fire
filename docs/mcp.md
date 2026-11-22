# MCP server

`fire cli mcp` starts a [Model Context Protocol](https://modelcontextprotocol.io) server over
stdio. It gives agents two things:

1. **The commands you already configured.** Every available `fire` command, with its invocation
   path, description, and `exec`/`eval` templates, so the agent runs `fire ...` instead of guessing
   or writing its own build/test/run commands. Commands marked `long_running: true` say so, so
   the agent knows to start them with `start_session`.
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
- The command, and everything under it, is left out of `list_commands` and `search_commands`. A
  subcommand can't opt back in under a group marked `agents: false`.
- `start_session` refuses to run it, with an error telling the agent not to run it any other way
  and to ask you instead.
- The server's instructions tell agents that these commands are off-limits, including from their
  own shell.
- `fire` help marks them as `[no agents]`, so you can see what's restricted without opening the
  YAML.

You can still run these commands yourself, including with `fire cli bg`.

This steers well-behaved agents. It is not a sandbox: an agent with shell access can still read
your config or type `fire deploy prod`. To hard-block that, also add a deny rule to your agent's
own permissions (for example, in Claude Code: `"deny": ["Bash(fire deploy:*)"]`).

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
`command`, `note` (the short reason it was started), `cwd`, `status` (`running`, `exited`,
`stopped` when someone stopped it on purpose, `lost`),
`pid`, `exit_code`, and timestamps.

### `session_output`
Takes an `id` and optional `lines` (default 100). Returns the session plus the last lines of its
terminal `output` as plain text: stdout and stderr merged (sessions run on a pty), escape
sequences removed and `\r` progress redraws collapsed.

### `start_session`
Takes `command` (the fire path plus arguments as a token array, without `fire`, e.g.
`["api", "dev"]`), a required short `note`, and an optional `cwd` (project directory, defaulting to
the server's). Same as `fire cli bg -m "<note>" <command...>`, except that it refuses commands
marked `agents: false`. Returns the session id in `stdout`. One command in one `cwd` is one
session: if it is already running, the tool returns that id and replaces its note; if it ended, it
starts again under the same id. So a restart is `signal_session` followed by `start_session`, and
the session list doesn't fill up with copies of the same server.

### `send_input`
Takes `id` and `text`, and types the text followed by Enter into the session's terminal
(`fire cli send`). With `raw: true`, sends `text` as exact keys without Enter, interpreting `\r`,
`\n`, `\t`, `\e`, `\\` and `\xHH`: e.g. `q`, `\x03` (Ctrl+C) or `\e[A` (up arrow)
(`fire cli send --raw`).

### `signal_session`
Takes `id` and an optional `signal` (name like `INT`, `HUP`, `USR1`, or a number), which is sent to
the session's foreground process group, as a keyboard signal would be. Without `signal`, it stops the session (`TERM`, then `KILL` after 5s), or
removes the record of an already-exited one (`fire cli kill`).

Management tools return `{ ok, stdout, stderr }` from the equivalent CLI command.

## Testing manually
Use the [MCP inspector](https://github.com/modelcontextprotocol/inspector) to poke at the server
without wiring up a full agent host:
```bash
npx @modelcontextprotocol/inspector -- fire cli mcp
```
