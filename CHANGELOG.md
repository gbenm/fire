# Changelog

User-facing changes to Fire. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and versions follow [Semantic Versioning](https://semver.org/). Entries for 2.10.1 and earlier were
reconstructed from the git history.

## [Unreleased]

### Breaking
- Runners run exactly as written. Fire no longer strips `-t`/`--tty`, no longer adds `-T` to
  `docker compose exec`, and no longer switches shell runners (`docker exec -it db bash`) to an
  "attached" mode when there is a terminal. Runners that relied on attached mode need
  `-c {fire.exec}`; runners that get the commands on stdin and use `-t` need it removed. See
  [troubleshooting](https://github.com/gbenm/fire/blob/main/docs/troubleshooting.md#the-input-device-is-not-a-tty). (#21)

### Added
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
- Docs: comparison with other task runners, MCP setup per host, schema `$id`.

### Changed
- The runtime `runner` and its `check` are launched through the command's `shell`. (#21)
- Documented rule: a runtime `runner` is the sdk's executable, or a command that ends in it. (#21)

### Fixed
- node and deno runtimes print why they exit (`Missing FIRE_RUNTIME_RPC_ADDR`,
  `Cannot connect to fire at …`), like python; python also reports refused connections. (#21)
- A `*.fire.yml` file with no commands no longer creates an empty inferred group. (#31)
- Runtime `fallback_runner` example in the docs works as written. (#21)

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

[Unreleased]: https://github.com/gbenm/fire/compare/v2.10.1...HEAD
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
