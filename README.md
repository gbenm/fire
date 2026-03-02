# Fire CLI

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

## Documentation map
- [Overview](./docs/overview.md)
- [Commands & arguments](./docs/commands-and-args.md)
- [Runtimes & eval](./docs/runtimes-and-eval.md)
- [Shell completion](./docs/completion.md)
- [Progressive examples](./docs/examples.md)
- [Troubleshooting](./docs/troubleshooting.md)

