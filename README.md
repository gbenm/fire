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
group: backend
namespace:
  prefix: ex
  description: Example

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

Default namespace behavior:
- If `namespace.prefix` is omitted in a file, Fire inherits it from another file in the same directory that defines it.
- If no file in that directory defines `namespace.prefix`, the file remains outside namespace scope.

## Command Resolution Rules

Fire resolves commands by file scope:

1. No `namespace.prefix`, no `group`:
- `fire <command>` (local direct command)
- `fire <implicit-namespace> <command>` (namespace path)

2. With `namespace.prefix`, no global `group`:
- `fire <namespace> <command>`

3. With global `group`, no `namespace`:
- `fire <group> <command>`

4. With `namespace` and global `group`:
- `fire <namespace> <group> <command>`

Root completion priority (`fire <TAB>`) is:
1. Root-local commands
2. Global namespaces
3. Global groups without namespace
4. Global direct commands

## Global Installation

`fire cli` is a reserved internal command.

Install the current directory globally:

```bash
fire cli install
```

Behavior:
- Stores only the absolute directory path (no command cache, no file copy).
- Avoids duplicates if the path is already installed.
- On each run, Fire dynamically reads installed directories and loads fire files from each directory root (non-recursive).

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

## Help Suffix

Append `:h` to any resolved command path to show help without execution:

- `fire run :h`
- `fire ex backend api :h`

It prints:
- Command path
- Command `description` (if any)
- Subcommands and their descriptions (if any)

## Run
```bash
cargo build
./target/debug/fire
```
