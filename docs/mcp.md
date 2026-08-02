# MCP server

`fire cli mcp` starts a [Model Context Protocol](https://modelcontextprotocol.io) server over
stdio. It exposes every `fire` command available to the agent — names, descriptions, invocation
paths, and the underlying `exec`/`eval` templates — so the agent can call `fire ...` directly
instead of guessing.

The server is **context-only**: it does not execute `fire` commands on the agent's behalf. Tools
are read-only introspection over the same merged config Fire itself resolves commands against
(local `fire.yml` / `*.fire.yml` plus installed global directories; see
[Overview](./overview.md#file-discovery-and-merge-order)) — whatever directory the host happens
to launch it from, no setup needed on your part.

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

## Testing manually
Use the [MCP inspector](https://github.com/modelcontextprotocol/inspector) to poke at the server
without wiring up a full agent host:
```bash
npx @modelcontextprotocol/inspector -- fire cli mcp
```
