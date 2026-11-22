# Fire CLI 🔥

Configuration-first CLI with dynamic completion, runtime eval, and container-friendly runners.

## Quick start (5 minutes)
1) Install via Homebrew (tap required):
```bash
brew tap gbenm/labs
brew install fire
```

2) execute `fire cli init` to create a `fire.yml`:
```yaml
commands:
  hello: echo "Hello Fire"
```

3) Run it:
```bash
fire hello world
# => Hello Fire world
```

4) Enable completion (zsh + bash):
```bash
fire cli completion install
```

## Configuration at a glance
Fire loads, merges, and executes commands defined in YAML (`fire.yml`, `fire.yaml`, `*.fire.yml`, `*.fire.yaml`). Commands can:
- run shell `exec` steps
- evaluate runtime snippets via `eval` (node/deno/python)
- rewrite arguments with `compute`
- use runners, fallback runners, and pre-run `before` hooks
- expose nested subcommands with greedy resolution

Reserved name: `cli` cannot be defined under `commands`.

## Completion
Install or refresh completion anytime:
```bash
fire cli completion install
```
- zsh shows values + descriptions
- bash uses `complete -C` values

Details: see [docs/completion.md](./docs/completion.md).

## Schema & editor support
Associate your YAML with https://raw.githubusercontent.com/gbenm/fire/main/schemas/fire.schema.json for validation (`fire cli init` does it):
- Add `$schema` to the first line: `# yaml-language-server: $schema=https://raw.githubusercontent.com/gbenm/fire/main/schemas/fire.schema.json`
- Or map file names in `.vscode/settings.json` (`yaml.schemas`).

## MCP server for agents
Let AI agents use the commands you already configured, and launch long-running processes as
managed sessions you can inspect, signal, and attach to:
```bash
fire cli mcp
```
Mark commands agents shouldn't touch (deploys, destructive DB tasks) with `agents: false`.
Details: see [docs/mcp.md](./docs/mcp.md).

## Sessions
In a terminal, every `fire` command runs on a pty under a small supervisor. It behaves exactly as if
run directly (colors, prompts, Ctrl+C, exit code), but **Ctrl+Z detaches it** while it keeps
running: `fg` brings it back, and `fire cli fg <id>` attaches to it from any other terminal. You and
your agents can list sessions from any project, read their output, type into them, signal or stop
them:
```bash
fire cli bg -m "api for auth tests" api dev   # start one detached
fire cli ps                                   # running ones; -a/--failed/--stopped/--exited for the rest
fire cli fg <id>
```
Mark servers and watchers with `long_running: true` so they detach on their own when an agent or a
script runs them without a terminal. Details: see [docs/sessions.md](./docs/sessions.md).

## Documentation map
- [Overview](./docs/overview.md)
- [Commands & arguments](./docs/commands-and-args.md)
- [Runtimes & eval](./docs/runtimes-and-eval.md)
- [Shell completion](./docs/completion.md)
- [MCP server](./docs/mcp.md)
- [Sessions](./docs/sessions.md)
- [Progressive examples](./docs/examples.md)
- [Troubleshooting](./docs/troubleshooting.md)

## If you enjoyed this and want to buy me a coffee ☕️
- Buy Me a Coffee: https://buymeacoffee.com/gbenm
- Bitcoin (BTC): `bc1q0l8na5gr6jgpnq6r7fj0nl3hqx2dm77l2vearuxx20s5ktes45vsxmksfu`
- USDC (Ethereum ERC20): `0x26D01f6fBa520D343CDAF309ba04FB9a7B26b4DE`

