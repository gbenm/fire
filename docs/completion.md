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

## Updating after config changes
Re-run `fire cli completion install` if you move the binary or want to refresh shell init files. Otherwise, completion picks up config changes automatically on the next invocation.
