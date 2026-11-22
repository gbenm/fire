# Fire compared to other task runners

Fire overlaps with tools you may already use as a command menu: `make`, [just](https://just.systems),
[Task](https://taskfile.dev), [mise](https://mise.jdx.dev) tasks, and `npm run`. This page explains
what Fire adds, and when another tool is the better fit.

## What Fire adds

| | Fire |
|---|---|
| **Config** | YAML (`fire.yml`, `*.fire.yml`), with a JSON Schema for editor validation |
| **Completion** | Generated from your config, nested groups included, with descriptions (zsh) |
| **Commands everywhere** | Register a directory once (`fire cli install`) and call its namespace from any directory |
| **Long-running processes** | Every command runs on a pty you can detach (Ctrl+Z) and reattach from any terminal (`fire cli fg <id>`) |
| **AI agents** | Built-in [MCP server](./mcp.md): agents discover your commands, start servers as shared sessions, and skip commands marked `agents: false` |
| **Missing tools** | `check` + `fallback_runner`: run the command in a container when the tool isn't installed locally |
| **Scripting** | `eval` / `compute` with JS/TS (node, deno) or Python snippets, next to plain shell |
| **Arguments** | Extra args are appended to the command by default; placeholders and macros when you need them |

## When another tool fits better

- **You build files, not run commands.** `make` rebuilds only what changed, based on file
  timestamps. Fire doesn't track inputs and outputs.
- **You need task dependencies or caching today.** just, Task, mise and make run dependencies
  before a task, and Task and mise can also skip work when sources haven't changed. Fire only has
  `before` for now ([dependencies are planned](https://github.com/gbenm/fire/issues/55)).
- **You need native Windows.** Fire runs on macOS and Linux (and Windows through WSL), because
  sessions rely on Unix ptys.
- **You want tool versions and tasks in one place.** mise also installs and pins the language and
  tool versions your tasks use.
- **It's a small JavaScript project.** `npm run` is already there, and every JS developer knows it.

## Using them together

Fire doesn't need to replace any of them. A common setup keeps the existing tool and lets Fire be
the entry point for people and agents:

```yaml
commands:
  build:
    description: Build everything
    exec: make build
  test:
    description: Run the test suite
    exec: just test
  dev:
    description: Start the web app
    long_running: true
    exec: npm run dev
```

You get completion with descriptions, detachable sessions, and agent access, without moving any
logic out of your `Makefile`, `justfile` or `package.json`.
