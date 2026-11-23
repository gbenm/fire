# Shell Completion

Fire ships rich tab-completion for zsh, fish, and Nushell (with inline descriptions), plus a
`complete -C` compatible mode for bash.

## Install completion scripts
Install completion for bash and zsh, plus fish and Nushell when their executables are on `PATH`:
```bash
fire cli completion install
```
Install a single shell:
```bash
fire cli completion install zsh
fire cli completion install bash
fire cli completion install fish
fire cli completion install nushell
```
The installer writes scripts to standard user locations. It appends managed blocks to `~/.zshrc`
and `~/.bashrc`, writes fish's autoloaded `fish/completions/fire.fish`, and adds a managed external
completer to Nushell's `config.nu`. `XDG_CONFIG_HOME` is respected; fish otherwise uses
`~/.config/fish`, while Nushell uses its platform config directory. Re-running any installer
replaces its generated script or managed block without duplicating it.

## How completion works
- On tab, the shell invokes `fire` in completion mode.
- Fire reads the merged config, resolves scopes (namespace/group), and emits candidates.
- When a command has a `description`, zsh, fish, and Nushell display it alongside the value.
- Bash uses the `complete -C` protocol, which only consumes the values.
- A command with `driver` delegates its argument candidates to the installed driver's completion
  handler; see [External CLI Drivers](./drivers.md#completion-protocol).
- `fire -<TAB>` lists the [global flags](commands-and-args.md#global-flags) not typed yet. After
  them, completion is the same as for `fire <TAB>` (without `cli`); the note after `-m` is free text,
  so nothing is offered there.
- Session commands complete their arguments, each with a description
  (`running · fire api dev · "auth tests" · 12m`):
  - `fire cli fg <TAB>` and `fire cli send <TAB>`: running sessions (`send` also offers `--raw`).
  - `fire cli kill <TAB>` and `fire cli logs <TAB>`: every session, running ones first, then
    ended ones by most recent. Sessions started in the current directory lead each part.
  - `fire cli kill <id> <TAB>`: signal names (`INT`, `TERM`, `HUP`, `KILL`...), only when the
    session is running.
- zsh and fish keep fire's order instead of sorting. Bash always sorts the values. Nushell applies
  the user's configured completion sorting.

## Updating after config changes
Re-run `fire cli completion install` if you move the binary, want to refresh shell configuration,
or installed the zsh script before it kept fire's order. Otherwise, completion picks up config
changes automatically on the next invocation.
