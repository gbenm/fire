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
