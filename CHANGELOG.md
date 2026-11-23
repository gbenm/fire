# Changelog

User-facing changes to Fire. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and versions follow [Semantic Versioning](https://semver.org/). Entries for 2.10.1 and earlier were
reconstructed from the git history.

## [Unreleased]

### Added
- Rich completion and idempotent installers for fish and Nushell. (#52)
- Automatic command imports from package.json, Makefile, justfile, Compose, and Cargo during
  `fire cli init`, plus merge-only `fire cli import` and the `--empty` opt-out. (#51)
- External CLI drivers with repository installation, cached builds, exact argument forwarding,
  and delegated rich completion through `driver.yaml`. (#24)
- A link-checked mdBook documentation site, deployed to GitHub Pages with a task-runner comparison. (#57)
- `fire cli check` (alias `validate`) validates fire files for CI and pre-commit hooks, with
  file/line diagnostics, `--format json`, `--strict`, and optional registered-directory checks. (#56)
- Command dependencies with scoped `needs`, deduplication, cycle and long-running checks,
  fail-fast execution, parallel output prefixes, and `-j`/`FIRE_JOBS` concurrency limits. (#55)
- `fire cli mcp install [<host>]` and `fire cli mcp uninstall <host>` register and remove the MCP
  server for Claude Code, Claude Desktop, Cursor, VS Code, Codex, Gemini CLI, and Windsurf. (#53)
- Scriptable `fire cli init` flags and MCP `init_project`, with explicit `--force` required for
  CLI overwrites and no overwrites through MCP. (#47)

## [3.0.1] - 2026-11-22

### Fixed
- The MCP server reports itself as `fire` with Fire's version instead of the rmcp library's.

## [3.0.0] - 2026-11-22

### Breaking
- Runners run exactly as written. Fire no longer strips `-t`/`--tty`, no longer adds `-T` to
  `docker compose exec`, and no longer switches shell runners (`docker exec -it db bash`) to an
  "attached" mode when there is a terminal. Runners that relied on attached mode need
  `-c {fire.exec}`; runners that get the commands on stdin and use `-t` need it removed. See
  [troubleshooting](https://github.com/gbenm/fire/blob/main/docs/troubleshooting.md#the-input-device-is-not-a-tty). (#21)

### Added
- Version-matched embedded docs through `fire cli docs [topic]`, topic completion, and the MCP
  `get_docs` tool, including the Fire config schema. (#33)
- MCP `list_global`, `add_global`, `remove_global`, `prune_global`, `merge_global` and
  `unmerge_global`: the `fire cli global` registry with absolute config file paths and structured
  results, for use only when the user asks. (#49)
- MCP `run_command` runs finite configured commands with structured exit code and output, a
  timeout, and refusals for `long_running` or `agents: false` commands. (#38)
- `{fire.exec}` in a command `runner`: replaced by the commands as one quoted argument, leaving
  stdin free for interactive programs (`docker run --rm -it node sh -c {fire.exec}`). Without it,
  the commands go to the runner's stdin, with and without a terminal. (#21)
- `FIRE_RUNTIME_RPC_PORT` for runtime runners, so a container can connect back with
  `-e FIRE_RUNTIME_RPC_ADDR=host.docker.internal:$FIRE_RUNTIME_RPC_PORT`. (#21)
- `shell:` setting (file or command level, inherited by subcommands) for `exec`, shell
  `compute`, `before`, `check` and launching the `runner`. Defaults to `sh`; accepts `$SHELL`. (#22)
- Interactive shell `compute`: stderr goes to the terminal and stdin is inherited from a terminal
  (including session ptys, answered with `fire cli send`), so a compute can prompt. Without a
  terminal stdin is closed and the compute never blocks. (#23)
- `fire cli global add|remove|list|prune|edit` to manage the directories whose commands load
  from anywhere: `list` groups them by namespace with their config files, `remove`/`edit` take a
  namespace or a path, and completion suggests registered namespaces and directories. Fire also
  offers once, in an interactive terminal, to register a directory the first time you run one of
  its commands. See [global directories](https://github.com/gbenm/fire/blob/main/docs/overview.md#global-directories). (#43)
- `fire cli global merge|unmerge <namespace>`: call a registered namespace's commands and groups
  without the namespace word, from any directory (`fire deploy` for `fire acme deploy`). Merging
  never takes a first word that another command, group or namespace already uses, and two merged
  namespaces with the same command report a collision. See
  [merged namespaces](https://github.com/gbenm/fire/blob/main/docs/overview.md#merged-namespaces). (#20)
- Global flags before the command: `--bg`, `--new`, `-m/--note`, `--fg`, `--dry-run` and
  `--no-log` (`fire --bg -m "auth tests" api dev`, `fire --dry-run api deploy`). They combine in
  any order, set their environment variable so nested `fire` calls inherit them, show up in root
  help and complete with `fire -<TAB>`. See
  [global flags](https://github.com/gbenm/fire/blob/main/docs/commands-and-args.md#global-flags). (#50)
- Completion for `fire cli fg|send|kill|logs`: session ids described with status, command, note
  and age (running sessions first, only running ones for `fg`/`send`), `--raw` for `send` and
  signal names for `kill <id>`. The zsh script now keeps fire's order instead of sorting; re-run
  `fire cli completion install zsh` to get it. See
  [completion](https://github.com/gbenm/fire/blob/main/docs/completion.md#how-completion-works). (#45)
- `namespace: tools` as shorthand for `namespace: { prefix: tools }`. (#32)
- Container object for `runner` / `fallback_runner` (commands and runtimes), with Compose key
  names (`image`, `engine`, `tty`, `command`, `volumes`, `working_dir`, `environment`, `user`,
  `network`, `network_mode`, `args`). Fire only turns it into a string runner, shown in the log
  and `--dry-run`: it mounts the project at the same path, forwards the declared environment as
  `-e NAME` (values never printed) and, for runtimes, passes the RPC address (`network_mode: host`
  for native Docker on Linux). (#44)
- Docs: the runtime runner form for native Docker on Linux (`--network host`). (#44)
- Docs: comparison with other task runners, MCP setup per host, schema `$id`.

### Changed
- MCP server instructions are now a short decision guide (find the command, reuse a running
  session, `start_session` for long-running, `run_command` for finite), with a single rule for
  the shell; hints for the human user moved to `docs/mcp.md`. (#37)
- MCP `search_commands` and `run_command` accept `cwd` to discover and run another project's
  local commands when the server was started elsewhere. (#46)
- MCP `list_sessions` defaults to the current Fire project, including sessions started from
  another directory that resolves to it; pass `cwd` to select a project or `all: true` to list
  sessions across projects. (#42)
- MCP command discovery is consolidated in `search_commands`: omit its query to list every
  command, use each result's prominent `path` to run it, retain its source and custom placeholder,
  and opt in to `exec`/`eval` templates with `include_exec`. (#39)
- The MCP session lifecycle is split into single-purpose `stop_session`, `signal_session`, and
  `forget_session` tools. (#41)
- MCP session-management results use structured content, and `start_session` exposes the session
  id as a `session_id` field. (#40)
- The runtime `runner` and its `check` are launched through the command's `shell`. (#21)
- Boolean environment variables (`FIRE_SESSIONS`, `FIRE_DRY_RUN`, `FIRE_LOG_COMMANDS`) all accept
  `1/true/yes/on` and `0/false/no/off`, any case; before, `FIRE_DRY_RUN` only took `true` and
  `FIRE_LOG_COMMANDS` only `false`. (#50)
- The MCP `start_session` tool runs `fire --bg` instead of `fire cli bg`. (#50)
- Documented rule: a runtime `runner` is the sdk's executable, or a command that ends in it. (#21)
- `fire cli init` writes `fire.yaml` / `<base>.fire.yaml` (a typed `.yml` name is kept) and adds
  `---` after the schema comment, so linters like yamllint's `document-start` rule pass. It warns
  when the same file with the other extension already exists, since both get loaded. Existing
  `.yml` files keep working. (#48)

### Deprecated
- `fire cli install`: use `fire cli global add` (same behavior). The old name still works and
  prints a hint. (#43)

### Fixed
- Session notes preserve the user's exact text instead of trimming leading or trailing whitespace.
- node and deno runtimes print why they exit (`Missing FIRE_RUNTIME_RPC_ADDR`,
  `Cannot connect to fire at …`), like python; python also reports refused connections. (#21)
- A `*.fire.yml` file with no commands no longer creates an empty inferred group. (#31)
- Runtime `fallback_runner` example in the docs works as written. (#21)
- The schema no longer requires `namespace.description`, so editors stop flagging a namespace
  with only a `prefix`. (#32)

## [2.10.1] - 2026-11-22

### Added
- Install script, linux arm64 builds, crates.io publishing, and CI on Linux and macOS.
- Releases update the Homebrew tap formula and publish to the MCP Registry automatically.
- Docs: README intro and demo GIF, WSL setup.

## [2.10.0] - 2026-11-22

### Changed
- `fire cli ps` shows live sessions by default.
- One session is reused per command and directory.

## [2.9.0] - 2026-11-22

### Added
- Any fire command can run detachable: sessions run on a pty (Ctrl+Z detaches).

## [2.8.1] - 2026-11-20

Version bump only.

## [2.8.0] - 2026-11-20

### Added
- `agents: false` keeps a command away from agents.

### Changed
- MCP instructions steer agents to sessions and fire commands; session ids use the full path.

## [2.7.1] - 2026-11-20

### Changed
- MCP instructions fit on one line, with the critical rule first.

## [2.7.0] - 2026-11-20

### Added
- Detached sessions.

## [2.6.0] - 2026-08-12

### Added
- Environment loader: `env_file` and `environment`.

## [2.5.0] - 2026-08-01

### Added
- MCP server.

## [2.4.0] - 2026-08-01

### Added
- Group inferred from the fire file's config.

## [2.3.0] - 2026-06-24

### Added
- `dir: null` runs in the current working directory.

## [2.2.0] - 2026-03-05

### Added
- Dry run (`FIRE_DRY_RUN`).

### Fixed
- Arguments used by `compute` were counted twice.

## [2.1.0] - 2026-03-05

### Added
- Command logs.
- Group descriptions.

### Changed
- Commands always run in a tty process.

### Fixed
- Partial and exact match completion.
- `before` command.

## [2.0.2] - 2026-03-04

### Changed
- Commands run in the same terminal; tty shell in `eval`.

## [2.0.1] - 2026-03-04

### Fixed
- Release pipeline.

## [2.0.0] - 2026-03-04

### Added
- `eval` can return commands to execute.

### Changed
- Improved computed values.
- Runtime script `paths` are relative to the file that defines them.

### Fixed
- Config merge strategies.

## [1.1.2] - 2026-03-02

### Added
- Fire files without commands are allowed.

### Changed
- Improved initial help.

## [1.1.1] - 2026-03-02

### Fixed
- YAML anchor problems.

## [1.1.0] - 2026-03-02

### Added
- Git root detection.

### Changed
- Improved command listing.

## [1.0.0] - 2026-03-01

### Added
- Runtimes and `eval`, `compute`, `on_unused_args`.

### Removed
- `delegate` and the `env_file` schema.

### Fixed
- Placeholder support.

## [0.2.0] - 2026-03-01

### Changed
- Global namespaces; the namespace id was removed.

## [0.1.1] - 2026-03-01

Version bump only.

## [0.1.0] - 2026-03-01

### Added
- First release: commands from YAML, namespaces and groups, global install, `include`, `init`,
  `runner` / `fallback_runner` / `check`, `before`, placeholders and macros, completion.

[Unreleased]: https://github.com/gbenm/fire/compare/v3.0.1...HEAD
[3.0.1]: https://github.com/gbenm/fire/compare/v3.0.0...v3.0.1
[3.0.0]: https://github.com/gbenm/fire/compare/v2.10.1...v3.0.0
[2.10.1]: https://github.com/gbenm/fire/compare/v2.10.0...v2.10.1
[2.10.0]: https://github.com/gbenm/fire/compare/v2.9.0...v2.10.0
[2.9.0]: https://github.com/gbenm/fire/compare/v2.8.1...v2.9.0
[2.8.1]: https://github.com/gbenm/fire/compare/v2.8.0...v2.8.1
[2.8.0]: https://github.com/gbenm/fire/compare/v2.7.1...v2.8.0
[2.7.1]: https://github.com/gbenm/fire/compare/v2.7.0...v2.7.1
[2.7.0]: https://github.com/gbenm/fire/compare/v2.6.0...v2.7.0
[2.6.0]: https://github.com/gbenm/fire/compare/v2.5.0...v2.6.0
[2.5.0]: https://github.com/gbenm/fire/compare/v2.4.0...v2.5.0
[2.4.0]: https://github.com/gbenm/fire/compare/v2.3.0...v2.4.0
[2.3.0]: https://github.com/gbenm/fire/compare/v2.2.0...v2.3.0
[2.2.0]: https://github.com/gbenm/fire/compare/v2.1.0...v2.2.0
[2.1.0]: https://github.com/gbenm/fire/compare/v2.0.2...v2.1.0
[2.0.2]: https://github.com/gbenm/fire/compare/v2.0.1...v2.0.2
[2.0.1]: https://github.com/gbenm/fire/compare/v2.0.0...v2.0.1
[2.0.0]: https://github.com/gbenm/fire/compare/v1.1.2...v2.0.0
[1.1.2]: https://github.com/gbenm/fire/compare/v1.1.1...v1.1.2
[1.1.1]: https://github.com/gbenm/fire/compare/v1.1.0...v1.1.1
[1.1.0]: https://github.com/gbenm/fire/compare/v1.0.0...v1.1.0
[1.0.0]: https://github.com/gbenm/fire/compare/v0.2.0...v1.0.0
[0.2.0]: https://github.com/gbenm/fire/compare/v0.1.1...v0.2.0
[0.1.1]: https://github.com/gbenm/fire/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/gbenm/fire/releases/tag/v0.1.0
