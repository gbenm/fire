# Troubleshooting

Common issues and quick fixes.

## Completion not working
- Re-run `fire cli completion install` to refresh shell init.
- Open a new shell or `source ~/.zshrc` / `~/.bashrc`.
- Ensure `fire` is on your `PATH` (e.g., Homebrew path is exported).

## Command not found / wrong path
- Check namespace/group: try `fire <namespace> <command>` or `fire <namespace> <group> <command>`.
- Remember `cli` is reserved and cannot be overridden.
- Use `:h` suffix to verify resolution: `fire run :h`.

## Fallback runner always triggers
- Inspect the `check` command; if it exits non-zero, Fire will use `fallback_runner` every time.
- Remove or relax `check` if you want the primary runner to be used more often.

## `the input device is not a TTY`
The runner has `-t` but Fire writes the commands to its stdin (no `{fire.exec}` in the runner), so stdin is not a terminal. Either:
- remove `-t` (keep `-i`), or
- use `{fire.exec}` so stdin stays free: `docker run --rm -it node sh -c {fire.exec}`. Outside a terminal (CI, agents, `fire cli bg` without a pty) `-t` still fails, so
- add `-t` only when there is a terminal: `docker run --rm -i $([ -t 0 ] && echo -t) node sh -c {fire.exec}`.

Fire runs runners exactly as written: it no longer strips `-t` or picks an "attached" mode based on the terminal. Runners that relied on that (e.g. `docker exec -it db bash` to type a password) need `-c {fire.exec}`.

## Runner does nothing, with no error
Without `{fire.exec}`, the commands go to the runner's stdin, so the runner must read it:
- `docker run` / `docker exec` need `-i`; without it docker ignores stdin and nothing runs.
- `docker compose exec` needs `-T` (it allocates a TTY by default).

## `{fire.exec}` ... is already quoted
Fire quotes the commands it puts in place of `{fire.exec}`. Remove the quotes around it:
`sh -c {fire.exec}`, not `sh -c '{fire.exec}'` or `sh -c "{fire.exec}"`.

## `{fire.exec}` only works in command runners
A runtime `runner` doesn't take `{fire.exec}`: it's the sdk's executable, or a command that ends
in it, and Fire appends the sdk's arguments (`runner: docker run --rm -i my-image deno`). See
[Runtime runners](./runtimes-and-eval.md#runtime-runners).

## Runtime runner exited before connecting
- The runtime prints the reason above Fire's message: `Missing FIRE_RUNTIME_RPC_ADDR` or `Cannot connect to fire at …`.
- In a container, pass the address: `-e FIRE_RUNTIME_RPC_ADDR=host.docker.internal:$FIRE_RUNTIME_RPC_PORT` (see [Runtime runners](./runtimes-and-eval.md#runtime-runners)).
- `Cannot connect to fire at host.docker.internal:…` with native Docker on Linux: the alias points at the bridge, not at Fire's loopback listener. Use `network_mode: host` in the [container object](./runtimes-and-eval.md#container-object), or `--network host -e FIRE_RUNTIME_RPC_ADDR=127.0.0.1:$FIRE_RUNTIME_RPC_PORT` in a string runner.
- The runner must end in the sdk's executable (or an image whose entrypoint is it): Fire appends the sdk's arguments.

## Container runner errors
- `tty: true needs {fire.exec} in command`: without `{fire.exec}` the commands go to the container's stdin, which can't be a terminal. Add `command: sh -c {fire.exec}` or drop `tty`.
- The container runs `node` (or another program) instead of your commands: without `command`, the image's own command runs. Add `command: sh -c {fire.exec}` or `command: sh`.
- `unknown key ...`: the object only takes `image`, `engine`, `tty`, `command`, `volumes`, `working_dir`, `environment`, `user`, `network`, `network_mode` and `args`.
- Files missing inside the container after setting `volumes`: explicit `volumes` replace the inferred mounts, so add the project yourself (`./:/app`).
- Files created as root on Linux: set `user: $(id -u):$(id -g)`.

## Interactive `compute` gets an empty value
- Without a terminal (pipes, CI, cron, `fire x < file`) a shell `compute` gets a closed stdin, so
  `read` returns empty instead of waiting. Pass the value as an argument or through the
  environment there.
- In a background session the prompt waits in the session output: answer with
  `fire cli send <id> <text>` or attach with `fire cli fg <id>`.
- The prompt ended up in the value: write prompts to stderr (`printf "Name: " >&2`); everything
  on stdout is the value.

## Aliases or functions not found with `shell:`
- Non-interactive shells don't load `.zshrc` / `.bashrc`. Use `bash -l` / `zsh -l` for your login
  profile, or call the program directly.
- `shell: $SHELL` with an unset variable is an error: set it in `environment` or use a program name.
- With `fish`, `exec` arrays are chained with `and`; with nu, tcsh and other shells without
  `set -e`, every line runs even after a failure.

## Eval fails or exits early
- Missing runtime key: confirm it exists under `runtimes`.
- `on_unused_args: error` aborts when extra args are not consumed by placeholders in `eval`.
- For Python/Node/Deno version mismatches, adjust `runner` or `fallback_runner` to the right binary/image.

## Windows
- Native Windows isn't supported: sessions run commands on Unix ptys. Use
  [WSL](https://learn.microsoft.com/windows/wsl/install) and install the Linux build inside it
  (`curl -fsSL https://raw.githubusercontent.com/gbenm/fire/main/install.sh | sh`).
- Run `fire` and `fire cli completion install` from the WSL shell, and keep your project in the
  Linux filesystem (`~/...`) rather than `/mnt/c/...` for much faster file access.
- To let a Windows-side MCP host reach Fire in WSL, use command `wsl` with args
  `["bash", "-lc", "fire cli mcp"]` (the login shell loads the `PATH` where Fire was installed).
