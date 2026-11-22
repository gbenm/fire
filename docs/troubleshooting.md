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
