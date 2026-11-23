# Fire compared to other task runners

Fire overlaps with tools you may already use as a command menu: `make`, [just](https://just.systems),
[Task](https://taskfile.dev), [mise tasks](https://mise.jdx.dev/tasks/), and
[`npm run`](https://docs.npmjs.com/cli/commands/npm-run-script). The differences below are more
important than the syntax: Fire focuses on discoverable project commands, shared process sessions,
and a safe interface for agents.

## At a glance

| Capability | Fire | make | just | Task | mise tasks | npm scripts |
|---|---|---|---|---|---|---|
| **Config format** | YAML | Makefile syntax | justfile syntax | YAML | TOML or standalone task files | JSON |
| **Completion with descriptions** | Generated; descriptions in zsh, values in bash | Not built in | Generated completion; recipes can have doc comments | Generated completion; tasks have descriptions | Generated completion; tasks have descriptions | Script-name completion, without descriptions |
| **Namespaces / global commands** | Nested groups plus registered global namespaces | Separate makefiles and recursive make | Modules and imported justfiles | Included Taskfiles with namespaces | Included task files and global config | Workspace scripts |
| **Detach / reattach sessions** | Built in for every command | No | No | No | No | No |
| **Container fallback when a tool is missing** | Built-in `check` + `fallback_runner` | Write a recipe | Write a recipe | Write a task | Use mise tools or write a task | Write a script |
| **Task runtimes** | Shell plus JS, TS, Python `eval` / `compute` | Shell and make functions | Shell or shebang scripts | Shell | Shell and file tasks; also manages tool versions | Shell commands and JavaScript packages |
| **Agent / MCP interface** | Built in; individual commands can set `agents: false` | No | No | No | No | No |
| **Dependencies** | `needs`, sequential or parallel, deduplicated with cycle detection | Target prerequisites | Recipe dependencies | Task dependencies | Task dependencies | Lifecycle hooks or explicit script chaining |
| **Platforms** | macOS, Linux, and Windows through WSL | Wherever a compatible make is installed | macOS, Linux, Windows | macOS, Linux, Windows | macOS, Linux, Windows | Wherever Node.js runs |

The table describes built-in behavior. Any of these tools can call scripts or third-party utilities
to add capabilities outside its core.

## Where Fire is different

- **Commands stay discoverable.** Descriptions from `fire.yaml` appear in help and completion, and
  registering a directory makes its namespace callable from any directory.
- **Long-running commands are shared processes.** Every command runs on a pty. Detach with Ctrl+Z,
  reattach with `fire cli fg <id>`, or let a person and an agent inspect the same server and logs.
- **Agents use the same command contract as people.** The built-in [MCP server](./mcp.md) discovers
  configured commands without inventing shell commands, while `agents: false` keeps sensitive
  commands human-only.
- **Missing local tools can be expected.** A `check` can select a `fallback_runner`, commonly a
  container, instead of making every caller reproduce fallback logic.

## When another tool fits better

- **You build files, not just run commands.** `make` rebuilds only what changed based on targets,
  prerequisites, and file timestamps. Fire does not track inputs or outputs.
- **You need source-aware caching.** Task can use sources and generated files, and mise tasks can
  declare sources and outputs. Fire dependencies order work but do not cache it.
- **You need native Windows.** Fire sessions use Unix ptys, so Windows users need WSL. just, Task,
  mise, and npm all run natively on Windows.
- **You want tool versions and tasks in one place.** mise installs and pins tool versions as well as
  running tasks. Fire can use a container fallback, but it is not a general tool-version manager.
- **It is a small JavaScript project.** `npm run` is already available, needs no additional install,
  and is familiar to JavaScript developers.

## Using them together

Fire does not need to replace an existing runner. It can be the entry point for people and agents
while the current build logic stays where it is:

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

This adds completion with descriptions, detachable sessions, and agent access without moving logic
out of your `Makefile`, `justfile`, `Taskfile.yml`, mise config, or `package.json`.
