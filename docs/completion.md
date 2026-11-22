# Shell Completion

Fire ships rich tab-completion for zsh (with inline descriptions) and a `complete -C` compatible mode for bash.

## Install completion scripts
Install both shells:
```bash
fire cli completion install
```
Install a single shell:
```bash
fire cli completion install zsh
fire cli completion install bash
```
The installer writes scripts to standard user locations and appends managed blocks to `~/.zshrc` / `~/.bashrc`.

## How completion works
- On tab, the shell invokes `fire` in completion mode.
- Fire reads the merged config, resolves scopes (namespace/group), and emits candidates.
- When a command has a `description`, zsh displays it alongside the value.
- Bash uses the `complete -C` protocol, which only consumes the values.
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
- zsh keeps fire's order instead of sorting. Bash always sorts the values.

## Updating after config changes
Re-run `fire cli completion install` if you move the binary, want to refresh shell init files, or
installed the zsh script before it kept fire's order. Otherwise, completion picks up config changes automatically on the next invocation.
