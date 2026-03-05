# Writing Commands and Handling Arguments

This guide shows how to author commands, chain hooks, and work with positional arguments and placeholders.

## File-level metadata
- `group`: adds a path prefix for all commands in the file.
- `description`: file-level label used for the group in help/completion.

```yaml
group: backend
description: Backend commands
commands:
  build:
    exec: npm run build
```

## Command shapes
A command spec supports:
- `description`: short help text shown in listings/completion.
- `exec`: shell command(s) to run. If you pass an array, commands run in order; the last one receives user args.
  - Array commands execute in the same shell session, so stateful steps like `cd`, `export`, etc. carry over.
- `eval`: runtime expression(s) (see `runtimes` guide).
  - Return handling: `void` = no output, `string` = printed, `string[]` = each string executed as a shell command.
- `commands`: nested subcommands. Resolution is greedy—the deepest valid path wins.
- `before`: shell command that runs before execution unless `fallback_runner` is selected.
- `dir`: working directory for this command (overrides file-level `dir`).
- `runner`: prefix to execute commands inside another environment (e.g., container shell).
- `fallback_runner`: alternative runner used when `check` fails.
- `check`: health probe; if it fails (exit code != 0), Fire switches to `fallback_runner` (or errors if none).
- `macros`: simple string replacements applied before placeholder expansion.
- `compute`: define computed variables (see below).
- `placeholder`: placeholder pattern. If absent, no interpolation or spreads occur.
- `on_unused_args`: behavior for unused args in **eval** (default: `ignore`).

String shorthand (`commands.foo: "npm test"`) is equivalent to a spec with `exec: "npm test"`.

### Reusing arg settings
Use `x-arg-config` at the file root to share `placeholder` and `on_unused_args`, then merge with `<<`:
```yaml
x-arg-config: &arg-config
  placeholder: "{{n}}"
  on_unused_args: warn

commands:
  computed:
    <<: *arg-config
    eval: py:sayHello("{1}", ...{{n}})
```

## Hooks and runners
```yaml
commands:
  run:
    # start front service it it's not running
    before: docker compose ps -q front | grep -q . || docker compose up -d front
    exec: compose exec front npm run

  start:
    runner: docker run --rm -it docker run --rm -it node:lts-alpine sh
    exec:
      - npm run build
      - npm run start
```
- `before` runs once before execution on direct mode or primary runner mode; it is skipped on fallback.
- `runner` pipes commands through another process.
- `fallback_runner` engages when `check` is defined and fails.
- For shell-based runners (for example `docker exec -it <container> bash` or plain `bash`), Fire uses attached shell mode when a TTY is available so interactive prompts (passwords) work correctly.

## Execution logs
By default, Fire prints each shell command before executing it:
- Direct execution (`exec` / shorthand)
- Array commands (each item)
- `before`
- Runner mode commands
- Shell-based `compute`
- `check` commands used for runner selection

Logs are label-based for readability (`cmd`, `runner`, `check`, `before`, `compute`), and use subtle ANSI colors when stderr is a TTY. Set `NO_COLOR=1` to force plain output.

To disable this and keep output behavior like before:
```bash
export FIRE_LOG_COMMANDS=false
```

Only the literal value `false` disables execution logs.

## Dry run (log only)
Set this variable to avoid executing shell commands while still showing what Fire would run:

```bash
export FIRE_DRY_RUN=true
```

Behavior:
- Fire logs commands as usual.
- Fire does **not** execute shell commands (`exec`, `before`, runner/fallback shell commands, and command arrays returned by `eval`).
- `compute` still runs normally.
- Runtime `eval` still runs normally; only shell command execution is skipped.

## Positional placeholders
Placeholders are **opt-in**: nothing is substituted unless you set `placeholder` on the command (or via an anchor like `x-arg-config`).

How the pattern works:

- The pattern must contain `{n}` (or `n`) to mark the index position.
- The **same** pattern drives all forms; there are no fallbacks.
- Individual placeholders are the pattern with `{n}` replaced by a 1-based index. Examples:
  - `placeholder: "@{n}"` → use `@1`, `@2`, ...
  - `placeholder: "{{n}}"` → use `{1}`, `{2}`, ...
- Spread placeholders reuse the literal pattern:
  - `...<placeholder>` spreads **unused** args individually (shell-escaped for exec; string literals for eval).
  - `[<placeholder>]` spreads unused args as an array literal (eval only).

Only the configured pattern is recognized. If you set `placeholder: "@{n}"`, tokens like `{1}` or `$1` will **not** be replaced.

## Macros
Macros are literal string replacements, applied before placeholders:
```yaml
front:
  macros:
    "{{front}}": docker compose exec front
    DYNAMIC: docker compose exec {{1}}
  exec: "{{front}} npm run"
  commands:
    hello: "DYNAMIC echo Hello"
```

- `fire front` -> `docker compose exec front npm run`
- `fire front hello alpine` -> `docker compose exec alpine echo Hello`

## Unused arguments policy (eval only)
`on_unused_args` controls what happens when users pass extra args that are not consumed by placeholders in `eval` expressions:
- `ignore` (default): do nothing.
- `warn`: print a warning with the 1-based unused indexes, continue.
- `error`: print an error and stop before eval.

Only `eval` uses this policy; shell execution keeps trailing args intact.

## Compute: computed variables
Use `compute` to define literal tokens that are replaced with computed values before placeholders are processed:
```yaml
compute:
  "{hash}": ts:makeHash("{1}", "sha256")
  "{service}": printf %s "{2}"
exec: echo "{service} => {hash}"
```
- Keys are literal replacement tokens (for example `"{hash}"`).
- Values with a known runtime prefix (`py:`, `ts:`, `js:`, `node:`, `deno:`) execute in that runtime (see [Runtimes](./runtimes-and-eval.md)); others run as shell commands.
- Compute sees the original user args and can use placeholders in the compute expression (for example `{1}`, `...{{n}}` based on your configured `placeholder`).
- The computed token can be reused in `exec`, `eval`, `before`, `check`, `runner`, and `fallback_runner`.

## Nested commands and greedy resolution
Fire resolves the deepest matching path, then passes remaining tokens to the target command:
```yaml
run:
  exec: npm run
  commands:
    build: npm run clean && npm run build
    start: fire build && npm run start
```
- `fire run` → `npm run`
- `fire run build` → `npm run clean && npm run build`
- `fire run other` → `npm run other` (because `other` isn’t a subcommand)

## Help suffix
Users can append `:h` to any resolved path to show description and subcommands instead of executing:
```
fire run :h
fire ex backend api :h
```
