# Fire CLI — Overview

Fire is a configuration-driven CLI. Commands, completion, and runtime execution are defined in YAML files you keep alongside your project. This document covers how Fire discovers config, merges scopes, and resolves command paths.

## What makes Fire different?
- **Config-first**: behavior lives in `*.fire.yaml` files; no code changes needed to add commands.
- **Dynamic completion**: tab-completion is generated from your config (with descriptions when present).
- **Runtime-aware**: evaluate JS/TS (node/deno) or Python snippets directly from YAML.
- **Containers and runners**: route commands through runners or fallback runners with health checks.

## Embedded documentation

The installed binary contains this documentation and the Fire config schema, so the material you
read always matches that version. `fire cli docs` lists every topic with a summary, and
`fire cli docs <topic>` prints the selected Markdown document (`schema` prints the JSON schema).
Topic completion includes the same summaries. When stdout is a terminal and `$PAGER` is non-empty,
Fire passes the document to that shell command; arguments and quoting in `$PAGER` follow the usual
shell-command convention. Redirected output is printed directly.

## File discovery and merge order
Fire loads YAML files from the current working directory using these patterns:
- `fire.yaml`
- `fire.yml`
- `*.fire.yaml`
- `*.fire.yml`

`.yaml` and `.yml` are equivalent; `fire cli init` writes `.yaml` unless you type a `.yml` name.

`fire cli init` prompts when run in a terminal. For scripts, pass `--yes` (or redirect stdin) to
use defaults without prompting. Detected project commands are imported in both modes:

```text
fire cli init [--file <base>] [--namespace <prefix>] [--namespace-description <text>] [--group <name>] [--yes] [--empty] [--force]
```

`--file backend` creates `backend.fire.yaml`; an explicit `.yml` suffix is kept. Existing files
are refused unless `--force` is passed, including in interactive use. `--empty` creates only the
starter config and skips imports; in a terminal, answering no to the import prompt does the same.

### Automatic imports

Initialization detects these files and writes one separate, grouped Fire file for each source so
names from different tools do not collide:

- `package.json` scripts become `package <script>` commands. Fire uses `bun`, `pnpm`, or `yarn`
  when the corresponding lockfile exists (in that priority), and `npm` otherwise. The complete
  script body is preserved as its description.
- `Makefile` targets become `make <target>`. Pattern and special targets are skipped. A target's
  inline `## comment`, or the immediately preceding comment, becomes its description.
- `justfile` recipes become `just <recipe>`. Fire uses `just --dump --dump-format json` when
  `just` is installed and otherwise reads recipe declarations and their preceding doc comments.
- `compose.yaml`, `compose.yml`, `docker-compose.yaml`, or `docker-compose.yml` creates
  `compose up`, `down`, `logs`, and `exec <service>`. `up` is long-running. An `exec` service is
  also marked long-running when it has a Compose `develop` section or its name contains an exact
  `dev`, `develop`, or `development` segment separated by `-`, `_`, or `.`.
- `Cargo.toml` creates `cargo build`, `test`, `run`, and `check`.

Generated shell commands quote source names without changing them. A source command name that is
empty, contains whitespace, or is the reserved name `cli` cannot be represented as one Fire path
word, so Fire warns and skips it instead of inventing a replacement.

Run `fire cli import` to scan all sources again, or name only the desired sources:

```text
fire cli import [package.json|Makefile|justfile|compose.yml|Cargo.toml]...
```

Imports add missing commands to `package.fire.yaml`, `make.fire.yaml`, `just.fire.yaml`,
`compose.fire.yaml`, and `cargo.fire.yaml`. Existing command names are listed as skipped and are
never replaced. Unknown, unreadable, or invalid sources produce warnings and do not make the
command fail.

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
fire cli global merge <namespace>             # call its commands without the namespace word
fire cli global unmerge <namespace>           # require the namespace word again
```

`list` shows one row per namespace, with the directory and the files that contribute to it:

```text
api     ~/code/api      fire.yaml, db.fire.yaml
tools   ~/code/tools    fire.yaml
(root)  ~/code/scripts  fire.yaml
-       ~/old/project   (missing)
```

`remove` and `edit` take a namespace or a path. An argument with `/`, `.`, `..` or a leading `~` is a path (`./api`, `~/code/api`); anything else is a namespace. A namespace found in several registered directories is an error that lists them, so pass the path instead. `edit` without a file opens the directory's only config file; with several, it asks which one in a terminal and otherwise lists them.

The first time you run a command in an unregistered directory with fire files, Fire asks once whether to register it (`[y/N]`). The answer is remembered, and `fire cli global remove` counts as a "no". It only asks in an interactive terminal: never in sessions (`fire --bg`, MCP), pipes, completion or when `CI` is set.

The registry lives in Fire's config directory (`installed-dirs.yml`, which also stores the merged namespaces, and `declined-dirs.yml` for the answered prompts). `fire cli install` still works as a deprecated alias of `fire cli global add`.

### Merged namespaces
On a machine where most commands come from one registered namespace, merge it to drop the namespace word everywhere:

```text
fire cli global merge acme
fire deploy        # → fire acme deploy
fire db migrate    # → fire acme db migrate
```

The explicit form (`fire acme deploy`) keeps working. `fire cli global list` marks merged namespaces with `(merged)`, and `fire <TAB>` and root help (`Merged Commands`) list their commands and groups with the namespace in the description.

Merging is only a shortcut: it never changes what an existing invocation runs. A merged namespace doesn't get a first word that is already in use without merging, meaning a local command, the local implicit namespace's commands and groups, a global direct command, a group without namespace, or any namespace name. For those, write the namespace (`fire acme ping`). If two merged namespaces define the same command, running it reports a collision and you need the explicit form.

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
1. Root-local commands (no namespace/group), then local namespaces and groups
2. Merged namespaces' commands and groups (only first words nothing above or below uses)
3. Global namespaces
4. Global groups without namespace
5. Global direct commands

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
