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

## Runtime runner exited before connecting
- The runtime prints the reason above Fire's message: `Missing FIRE_RUNTIME_RPC_ADDR` or `Cannot connect to fire at …`.
- In a container, pass the address: `-e FIRE_RUNTIME_RPC_ADDR=host.docker.internal:$FIRE_RUNTIME_RPC_PORT` (see [Runtime runners](./runtimes-and-eval.md#runtime-runners)).
- The runner must end in the sdk's executable (or an image whose entrypoint is it): Fire appends the sdk's arguments.

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
