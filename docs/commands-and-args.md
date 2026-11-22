# Writing Commands and Handling Arguments

This guide shows how to author commands, chain hooks, and work with positional arguments and placeholders.

## File-level metadata
- `group`: adds a path prefix for all commands in the file. If omitted, it's inferred from the filename (see below). Set it to `null` to keep the file ungrouped regardless of filename.
- `description`: file-level label used for the group in help/completion.
- `session` / `long_running`: defaults for every command in the file (see the command keys below and [Sessions](./sessions.md)).

```yaml
group: backend
description: Backend commands
commands:
  build:
    exec: npm run build
```

### Inferring `group` from the filename
When `group` is omitted in a `*.fire.yml` / `*.fire.yaml` file, Fire infers it from the filename (the part before `.fire.yml`/`.fire.yaml`), verbatim:
- `tracking.fire.yml` → `group: tracking`
- `instance.fire.yaml` → `group: instance`

Rules:
- An explicit `group` in the YAML always takes precedence over the inferred value.
- Set `group: null` to opt out and keep the file ungrouped (the pre-inference default behavior).
- The base `fire.yml` / `fire.yaml` files never infer a group, since they have no prefix before the extension.
- A file with no commands never creates a group, whether inferred or explicit.

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
- `shell`: interpreter for every shell snippet of the command (default: `sh`). See [Shell](#shell).
- `runner`: command that runs the `exec` commands inside another environment (a container, ssh, …). See [Hooks and runners](#hooks-and-runners).
- `fallback_runner`: alternative runner used when `check` fails.
- `check`: health probe; if it fails (exit code != 0), Fire switches to `fallback_runner` (or errors if none).
- `macros`: simple string replacements applied before placeholder expansion.
- `compute`: define computed variables (see below).
- `placeholder`: placeholder pattern. If absent, no interpolation or spreads occur.
- `on_unused_args`: behavior for unused args in **eval** (default: `ignore`).
- `agents`: set to `false` to keep the command (and its subcommands) away from agents using the [MCP server](./mcp.md#keeping-commands-away-from-agents) (default: `true`).
- `session`: set to `false` to always run the command directly instead of in a [session](./sessions.md) (default: `true`, sessions are used when running in a terminal).
- `long_running`: set to `true` for servers, watchers and workers. Without a terminal (agents, scripts) the command then starts as a detached [session](./sessions.md#long-running-commands-without-a-terminal) and returns its id instead of blocking (default: `false`).

String shorthand (`commands.foo: "npm test"`) is equivalent to a spec with `exec: "npm test"`.

## Environment

Every Fire file automatically loads a `.env` beside that file; a missing automatic `.env` is ignored. An included file also receives the including directory's `.env`, then its own `.env` (its values win on duplicates). Base-file commands never inherit included files' environments.

Use `env_file` to replace automatic loading for a file or command. It accepts a path, a list (loaded in order), or `null` to disable loading. Missing explicit files emit a warning and execution continues. Command-level `env_file` replaces the file-level setting.

```yaml
env_file: [.env, .env.prod]
environment:
  NODE_ENV: production
  DATABASE_URL: postgres://${DB_USER:-postgres}:${DB_PASS:-secret}@localhost/app
commands:
  deploy:
    environment:
      NODE_ENV: production
    exec: ./deploy.sh
```

`environment` is an inline key/value map. A command-level map replaces the file-level map. Values in `.env` files and `environment` support `${VAR}`, `${VAR:-default}`, and `${VAR:?message}`. The invoking shell has highest precedence, followed by inline `environment`, then `env_file` values. The resolved environment is available to hooks, checks, runners, shell compute, runtime eval, and command execution.

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

## Shell
Every shell snippet of a command runs with `sh -c` by default: `exec`, shell `compute`, `before` and `check`. Set `shell` to use another interpreter for all of them, per command or at file level as a default:
```yaml
shell: zsh            # file-level default

commands:
  release:
    shell: bash       # overrides the file default
    compute:
      "{branch}": git branch --show-current
    check: '[[ -n "$CI" ]]'
    exec: ./scripts/release.sh "{branch}"

  mine:
    shell: $SHELL     # the user's login shell
    exec: echo "running in $0"
```
- **The default is `sh`**, on purpose: fire files are shared, and `$SHELL` changes between users (fish isn't even POSIX), CI, agents and background sessions.
- Accepts any executable, optionally with arguments (`zsh`, `/bin/zsh`, `bash -l`), or a variable such as `$SHELL` / `${SHELL}`. Variables are read from the command's [environment](#environment) first, then from fire's own; an unset variable is an error.
- For your personal commands, put `shell: $SHELL` at the top of the fire files in the directories you registered with `fire cli global add`: it only affects those commands, never shared repos.
- Non-interactive shells don't load `.zshrc` / `.bashrc`, so aliases and functions defined there aren't available. Use `bash -l` / `zsh -l` if you need your login profile.
- Fire runs `<shell> [args] -c '<script>'`, so the shell must accept `-c` (sh, bash, zsh, dash, ksh, fish, nu, elvish, xonsh, tcsh, pwsh all do). It's meant for shells: interpreters such as `node` or `python` belong in [`runtimes`](./runtimes-and-eval.md).
- Commands in an `exec` array stop at the first failure (`set -e`) with sh, bash, zsh, dash, ash, ksh, mksh and yash. With `fish`, which has no `set -e`, they are chained with `and` instead. Other shells (nu, tcsh, …) run every line and don't stop on failure.
- `shell` takes precedence over `runner`: the runner itself is launched through it (`<shell> -c '<runner>'`). The shell *inside* the runner (a container, ssh, …) is whatever the runner starts.
- The runtime `runner` and its `check` (for `eval` and runtime `compute`) are launched through it too; the code inside the runtime is not affected.

## Hooks and runners
```yaml
commands:
  run:
    # start front service it it's not running
    before: docker compose ps -q front | grep -q . || docker compose up -d front
    exec: compose exec front npm run

  start:
    runner: docker run --rm -i -v .:/app -w /app node:lts-alpine sh
    exec:
      - npm run build
      - npm run start
```
- `before` runs once before execution on direct mode or primary runner mode; it is skipped on fallback.
- `fallback_runner` engages when `check` is defined and fails.
- Runners are launched through the command's [`shell`](#shell) and run **exactly as written**: Fire never adds or removes flags, and behaves the same with and without a terminal.

### How commands reach the runner

There are two modes, and the runner string alone decides which one:

- **Without `{fire.exec}`** (above), Fire writes the commands to the runner's stdin, after `set -e`. The runner must read stdin (`docker run -i`, `docker compose exec -T`, `ssh host sh`) and can't use `-t`, since stdin is not a terminal. Programs inside can't prompt for input.
- **With `{fire.exec}`**, Fire replaces it with the commands and stdin stays free, so interactive prompts work and `-t` is fine when there is a terminal:

```yaml
commands:
  start:
    runner: docker run --rm -it node sh -c {fire.exec}
    exec:
      - npm run build
      - npm run start
```

`fire start --watch` runs:

```sh
docker run --rm -it node sh -c 'set -e
npm run build
npm run start --watch'
```

`{fire.exec}` expands to every command in `exec`, one per line after `set -e`, with the extra args appended to the last one, **already quoted as a single argument**. Don't wrap it in quotes: `'{fire.exec}'` or `"{fire.exec}"` is an error. It works anywhere in the runner:

```yaml
runner: docker compose exec app bash -lc {fire.exec}
runner: kubectl exec -it pod -- sh -c {fire.exec}
runner: ssh -t server {fire.exec}
runner: docker exec -it db bash -c {fire.exec}   # e.g. mysql asking for a password
```

`--dry-run` and the `[fire] runner` log show the runner as written, followed by each command.

### Container runners

`runner` and `fallback_runner` (in commands and in [runtimes](./runtimes-and-eval.md#container-object)) also take a container object, with Docker Compose key names. Fire **only turns it into a string runner** that follows the rules above; that string is what runs, what the `[fire] runner` log and `--dry-run` show, and you can copy it and run it by hand.

```yaml
commands:
  test:
    runner:
      image: alpine
    exec: ./scripts/test.sh
```

runs

```sh
docker run --rm -i -v /path/to/project:/path/to/project -w /path/to/project alpine
```

with the commands on stdin.

| Key | Generates | Default |
|---|---|---|
| `image` | the image (required) | |
| `engine` | `docker` or `podman` | `docker` |
| `tty` | `-t` | `false`, never inferred from the terminal |
| `command` | what runs inside the image | the image's own command |
| `volumes` | `-v` for each entry | the project directory and the command's `dir`, at the same absolute path |
| `working_dir` | `-w` | the command's resolved `dir` |
| `environment` | `-e NAME` for each variable | the command's `environment` / `env_file` |
| `user` | `--user` | none |
| `network` / `network_mode` | `--network` | none |
| `args` | extra flags before the image | none |

- **`command`**: fire never picks a shell. Without `command`, the image's own command runs and the commands go to its stdin, which works for images whose default is a shell (`alpine`, `debian`, `ubuntu`). Images like `node` start `node` by default, so write `command: sh -c {fire.exec}` (stdin stays free) or `command: sh` (commands on stdin). Any `command` follows the [two modes](#how-commands-reach-the-runner).
- **`tty: true`** needs `{fire.exec}` in `command`: otherwise the commands go to stdin, which can't be a terminal, and fire stops with an error before running anything.
- **`volumes`** replace the inferred mounts entirely: list the project yourself if you still need it. Relative host paths (`./`, `../`) resolve from the fire file's directory, as in Compose.
- **`environment`**: the container gets the variables the fire file declares (`environment`, `env_file`) plus the object's `environment`, which wins on conflict, never the whole host environment. Fire passes them as `-e NAME` and sets the values in the runner's environment, so they never show up in the log or `--dry-run`. Values take `${VAR}` like the command's `environment`.
- **Values you write go into the string as written**, so the shell expands `$VAR`, `~` and `$(...)` in them; paths fire computes are quoted. On Linux, `user: $(id -u):$(id -g)` keeps files written to the project owned by you instead of root.
- `network` and `network_mode` can't be combined, as in Compose.
- Unknown keys are reported when the command runs (`unknown key `imagen``).

```yaml
commands:
  build:
    runner:
      image: node:lts-alpine
      command: sh -c {fire.exec}
      tty: true
      volumes:
        - ./:/app
        - ~/.npm:/root/.npm
      working_dir: /app
      environment:
        NODE_ENV: production
      user: $(id -u):$(id -g)
      args: [--cpus, "2"]
    exec: npm run build
```

## Execution logs
By default, Fire prints each shell command before executing it:
- Direct execution (`exec` / shorthand)
- Array commands (each item)
- `before`
- Runner mode commands
- Shell-based `compute`
- `check` commands used for runner selection

Logs are label-based for readability (`cmd`, `runner`, `check`, `before`, `compute`), and use subtle ANSI colors when stderr is a TTY. Set `NO_COLOR=1` to force plain output.

To disable it for one run, pass `--no-log` (`fire --no-log api test`); to disable it by default,
set `FIRE_LOG_COMMANDS=false` (see [environment variables](#environment-variables)).

## Dry run (log only)
Pass `--dry-run` to see what Fire would run without running it:

```bash
fire --dry-run api deploy
```

Behavior:
- Fire logs commands as usual.
- Fire does **not** execute shell commands (`exec`, `before`, runner/fallback shell commands, and command arrays returned by `eval`).
- `compute` still runs normally.
- Runtime `eval` still runs normally; only shell command execution is skipped.
- No session is started, even with `--bg`.

## Global flags
Flags that change how a command runs go **before** the command. Anything after the first command
word belongs to the command, so `fire api dev --bg` passes `--bg` to `api dev`.

| Flag | Effect | Environment variable |
|---|---|---|
| `--bg` | Run detached, in a [session](sessions.md#managing-sessions); prints its id | |
| `--new` | Start a new session even if this command already has one here | |
| `-m`, `--note <text>` | Short note on why the session is running | `FIRE_SESSION_NOTE` |
| `--fg` | Run attached in this process, without a session | `FIRE_SESSIONS=off` |
| `--dry-run` | Print what would run without running it | `FIRE_DRY_RUN=true` |
| `--no-log` | Don't log the commands Fire runs | `FIRE_LOG_COMMANDS=false` |

```bash
fire --bg -m "auth tests" --no-log api dev   # same as: fire --no-log --bg --note "auth tests" api dev
fire --dry-run api deploy
```

- Flags combine in any order. A repeated flag is fine; a repeated `-m` keeps the last note.
- `--bg` and `--fg` can't go together. `-m` and `--new` apply to sessions (with `--bg`, or to the
  session a command starts on its own in a terminal), so they fail with `--fg`.
- An unknown flag (`fire --foo api dev`) is an error that lists the valid ones; it is never looked
  up as a command. Flags need a command, and they don't apply to `fire cli ...`.
- `fire` (root help) lists them, and `fire -<TAB>` completes them.

### Environment variables
Each flag with a variable sets it for the command it runs, so nested `fire` calls inherit it: a
script started with `fire --dry-run deploy` that runs `fire build` is a dry run too. Set the
variables yourself for a default that lasts a whole shell, CI job or agent; a flag always wins
(`--bg` starts a session even with `FIRE_SESSIONS=off`).

Boolean variables accept `1`, `true`, `yes`, `on` and `0`, `false`, `no`, `off` (any case).
Anything else is ignored.

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

### Interactive shell compute
A shell compute's stdout is its value. Its stderr goes straight to your terminal, and when fire runs in a terminal its stdin is too, so a compute can ask for input:
```yaml
commands:
  greet:
    compute:
      "{name}": 'printf "Name: " >&2; read name; echo "$name"'
    exec: echo "Hello {name}"
```
With `shell: zsh` you can use `vared`: `vared -cp "Name: " name && echo $name`.

- Write prompts to stderr (`>&2`); anything on stdout becomes part of the value.
- Background sessions (`fire --bg`, MCP `start_session`) run in a terminal too: the prompt shows up in the session output, and you answer it with `fire cli send <id> <text>` (MCP `send_input`) or by attaching with `fire cli fg <id>`.
- Without a terminal (pipes, CI, cron, `fire x < file`) stdin is closed, so a compute that reads input gets EOF instead of waiting.
- If a compute fails, fire reports the command and its exit code; its stderr has already been shown.

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
