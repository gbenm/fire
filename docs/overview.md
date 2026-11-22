# Fire CLI — Overview

Fire is a configuration-driven CLI. Commands, completion, and runtime execution are defined in YAML files you keep alongside your project. This document covers how Fire discovers config, merges scopes, and resolves command paths.

## What makes Fire different?
- **Config-first**: behavior lives in `*.fire.yaml` files; no code changes needed to add commands.
- **Dynamic completion**: tab-completion is generated from your config (with descriptions when present).
- **Runtime-aware**: evaluate JS/TS (node/deno) or Python snippets directly from YAML.
- **Containers and runners**: route commands through runners or fallback runners with health checks.

## File discovery and merge order
Fire loads YAML files from the current working directory using these patterns:
- `fire.yaml`
- `fire.yml`
- `*.fire.yaml`
- `*.fire.yml`

`.yaml` and `.yml` are equivalent; `fire cli init` writes `.yaml` unless you type a `.yml` name.

Merge rules:
1) `fire.yaml` / `fire.yml` (base)  
2) `*.fire.yaml` / `*.fire.yml` in lexicographic order  
Commands from all files are merged. If two files in the same scope define the same command path, running it fails with a `Duplicate terminal command` error naming both files.

Both extensions of the same name (for example `fire.yml` and `fire.yaml`) are loaded as two separate files, so keep only one. `fire cli init` warns when it creates a file whose other-extension twin already exists.

### Includes
Root files may declare `include` to load additional directories (non-recursive):
```yaml
include:
  - samples/
  - tools/
```
Paths are relative to the current directory. Included files follow the same scope rules described below. For a [registered directory](#global-directories), includes are resolved relative to that directory.

### Environment files

Each Fire file loads its adjacent `.env` automatically. For included files, Fire merges the including directory's `.env` with the included directory's `.env`, with the included file taking precedence. Configure `env_file` (string, list, or `null`) and inline `environment` values at file or command scope when a command needs a different environment. See [Writing Commands](./commands-and-args.md#environment) for interpolation and precedence details.

## Global directories
Register a directory to load its commands from anywhere. Its files are discovered the same way as local ones; files without namespace or group become direct global commands.

```text
fire cli global add [path]                    # register a directory (default: current)
fire cli global remove [namespace|path]       # unregister it (default: current); never deletes files
fire cli global list                          # registered directories, by namespace
fire cli global prune                         # unregister directories that no longer exist
fire cli global edit [namespace|path] [file]  # open a config file with $VISUAL / $EDITOR
```

`list` shows one row per namespace, with the directory and the files that contribute to it:

```text
api     ~/code/api      fire.yaml, db.fire.yaml
tools   ~/code/tools    fire.yaml
(root)  ~/code/scripts  fire.yaml
-       ~/old/project   (missing)
```

`remove` and `edit` take a namespace or a path. An argument with `/`, `.`, `..` or a leading `~` is a path (`./api`, `~/code/api`); anything else is a namespace. A namespace found in several registered directories is an error that lists them, so pass the path instead. `edit` without a file opens the directory's only config file; with several, it asks which one in a terminal and otherwise lists them.

The first time you run a command in an unregistered directory with fire files, Fire asks once whether to register it (`[y/N]`). The answer is remembered, and `fire cli global remove` counts as a "no". It only asks in an interactive terminal: never in sessions (`fire cli bg`, MCP), pipes, completion or when `CI` is set.

The registry lives in Fire's config directory (`installed-dirs.yml`, and `declined-dirs.yml` for the answered prompts). `fire cli install` still works as a deprecated alias of `fire cli global add`.

## Scopes: namespace and group
- `namespace.prefix` defines a logical namespace; `namespace.description` is optional and labels it in listings and completion. With only a prefix, `namespace: tools` is shorthand for `namespace: { prefix: tools }`.
- `group` provides an additional path segment, often for team/area partitioning.
- `description` at file root describes that `group` in help/completion.
- Files in the same directory inherit `namespace.prefix` from a peer file if they do not set one.
- If `group` is omitted, it's inferred from the filename (`tracking.fire.yml` → `group: tracking`). An explicit `group` always wins; set `group: null` to keep the file ungrouped. See [Writing Commands](./commands-and-args.md#inferring-group-from-the-filename) for details.

Example:
```yaml
group: backend
description: Backend services
commands:
  deploy:
    exec: ./deploy.sh
```

### Command path shapes
Depending on scope, users call commands as:
- `fire <command>` (no namespace, no group)
- `fire <namespace> <command>` (namespace only)
- `fire <group> <command>` (group only)
- `fire <namespace> <group> <command>` (namespace + group)

### Resolution priority at root completion (`fire <TAB>`)
1. Root-local commands (no namespace/group)
2. Global namespaces
3. Global groups without namespace
4. Global direct commands

`cli` is reserved by Fire and cannot be defined under `commands`.

## Command entry types
Every `commands.<name>` entry is either:
- **String shorthand**: executed as a shell command (e.g., `"npm test"`).
- **Full spec object**: supports `exec`, `eval`, `compute`, nested `commands`, and more.

## Help suffix
Append `:h` to any resolvable path to print help instead of executing:
```
fire run :h
fire ex backend api :h
```
The help view shows the command path, description, and any direct subcommands.
