# Fire CLI — Overview

Fire is a configuration-driven CLI. Commands, completion, and runtime execution are defined in YAML files you keep alongside your project. This document covers how Fire discovers config, merges scopes, and resolves command paths.

## What makes Fire different?
- **Config-first**: behavior lives in `*.fire.yml` files; no code changes needed to add commands.
- **Dynamic completion**: tab-completion is generated from your config (with descriptions when present).
- **Runtime-aware**: evaluate JS/TS (node/deno) or Python snippets directly from YAML.
- **Containers and runners**: route commands through runners or fallback runners with health checks.

## File discovery and merge order
Fire loads YAML files from the current working directory using these patterns:
- `fire.yaml`
- `fire.yml`
- `*.fire.yaml`
- `*.fire.yml`

Merge rules:
1) `fire.yaml` / `fire.yml` (base)  
2) `*.fire.yaml` / `*.fire.yml` in lexicographic order  
If the same command path is defined multiple times, the **last loaded** definition wins.

### Includes
Root files may declare `include` to load additional directories (non-recursive):
```yaml
include:
  - samples/
  - tools/
```
Paths are relative to the current directory. Included files follow the same scope rules described below. When running from an installed path, includes are resolved relative to that installed directory.

## Scopes: namespace and group
- `namespace.prefix` and `namespace.description` define a logical namespace.
- `group` provides an additional path segment, often for team/area partitioning.
- Files in the same directory inherit `namespace.prefix` from a peer file if they do not set one.

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
