# Fire CLI

A CLI with dynamic completion powered by external configuration.

## Command Configuration
Fire loads YAML files from the current directory with these patterns:
- `fire.yaml`
- `fire.yml`
- `*.fire.yaml`
- `*.fire.yml`

Files are merged in this order:
1. `fire.yaml` / `fire.yml`
2. `*.fire.yaml` / `*.fire.yml` (lexicographic order)

If the same command name appears more than once, the last loaded definition wins.

Example:

```yaml
commands:
  run:
    description: Run npm scripts
    exec: npm run
    commands:
      build: npm run build
      test:
        description: Run tests
        run: npm run test

  lint: npm run lint
```

`exec` and `run` are both supported and treated as executable command actions.

## `config.fire.yaml` Validation in VS Code
The schema is available at [`schemas/fire.schema.json`](./schemas/fire.schema.json).

You can associate it in two ways:

### Option 1: Global mapping by file name
In `.vscode/settings.json`:

```json
{
  "yaml.schemas": {
    "./schemas/fire.schema.json": [
      "config.fire.yaml",
      "*.fire.yml",
      "*.fire.yaml"
    ]
  }
}
```

### Option 2: Per-file mapping with `$schema`
In the first line of your YAML file:

```yaml
# yaml-language-server: $schema=./schemas/fire.schema.json
```

With this, VS Code (YAML extension) validates structure, types, and allowed DSL fields.

Note: `cli` is a reserved command name at the top-level `commands` map, so `commands.cli` is rejected by the schema.

Expected validation error (example in VS Code): `Property cli is not allowed.`

Why: `cli` is reserved for internal CLI behavior and cannot be overridden as a user command at the root `commands` level.

## Autocomplete Without External Scripts
Fire supports two completion modes:

- Rich zsh completion (value + description)
- Bash-compatible command completion (`complete -C`)

### zsh
```zsh
source ./zsh_completations
```

### bash
```bash
complete -o nospace -C fire fire
```

For `complete -C`, the shell invokes `fire` in completion mode using `COMP_LINE` / `COMP_POINT`.

Completion output includes command name and `description` when present (`name<TAB>description`) in the `__complete` protocol, which is consumed by the zsh completion script.

Note: `complete -C` only uses completion values; descriptions are a zsh-native feature through `zsh_completations`.

## Execution Rules
- `fire <command>` executes `exec` (or `run`) of the resolved command.
- Nested command resolution is greedy: it matches the deepest valid subcommand path.
- Remaining unmatched tokens are appended as arguments to the final executable command.

Example:
- `fire run start --host 0.0.0.0` with `run.exec: "npm run"` executes:
  - `npm run start --host 0.0.0.0`

`cli` is reserved for internal CLI management and cannot be overridden by user commands.

## Run
```bash
cargo build
./target/debug/fire
```
