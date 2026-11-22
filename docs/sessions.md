# Sessions

Fire runs commands inside **sessions**: the program runs on a pseudo-terminal (pty) under a small
supervisor, so it behaves exactly as in your terminal (colors, progress bars, prompts, password
input, full-screen TUIs), but it doesn't belong to your terminal. You can detach from it, close the
terminal, and pick it up again from any other terminal, project or agent.

Anyone on the machine under the same user (you, or an agent in another project) can see a session,
read its output, type into it, send it signals, attach to it, or stop it. Before restarting a dev
server, an agent can check what is already running, why it was started, and what it is printing.

## Every `fire <command>` in a terminal
When stdin and stdout are a terminal, `fire api dev` starts a session and attaches to it right
away. It looks like running the command directly: same output, Ctrl+C stops it, prompts work, and
fire exits with the command's exit code. The difference is that your shell's job control now
detaches it instead of freezing it:

- **Ctrl+Z** detaches. Your shell shows the job as suspended (`[1]+ Stopped fire api dev`), but
  only the viewer stopped: the program keeps running.
- **`fg`** reattaches: you see what it printed in the meantime, and full-screen programs redraw.
- **`bg`** does nothing useful (the viewer has nothing to do in the background) and the viewer stops
  again; the program is unaffected either way.
- **Closing the terminal** (even with the job suspended) only ends the viewer; the program survives.
- **From anywhere else:** `fire cli fg <id>` attaches to the same session (`fire cli ps` lists ids).

When the command finishes while you're attached, its session is removed right away, so
`fire cli ps` only shows what is still running or what finished while detached.

Trade-off: the program itself can't receive Ctrl+Z from the keyboard anymore (e.g. to suspend vim
inside a session). Send it explicitly with `fire cli kill <id> TSTP` and resume with `CONT`.

Sessions are skipped, and the command runs directly as before, when:
- stdin or stdout isn't a terminal (`fire x | grep`, scripts, CI, agents' shells), unless the
  command is marked `long_running` (see below);
- the command or its file sets `session: false`;
- you pass `--fg` (`fire --fg x`) or set `FIRE_SESSIONS=off`, or it is a dry run (`--dry-run`,
  `FIRE_DRY_RUN=true`);
- fire is already running inside a session (`FIRE_SESSION` is set), so commands never nest;
- the command was started in the background with `&`.

The session's note defaults to the command's description; pass `-m "note"` (`fire -m "why" x`)
or set `FIRE_SESSION_NOTE` to override it, and `--new` to start a separate session instead of
reusing an ended one.

```yaml
commands:
  repl:
    session: false          # misbehaves under the supervisor? run it directly
    exec: ./scripts/repl.sh
```

## Long-running commands without a terminal
Agents' shells usually aren't terminals. Without a marker, an agent that runs `fire web start` in
its shell blocks until it times out. Mark servers, watchers and workers with `long_running: true`:

```yaml
commands:
  web:
    long_running: true
    exec: npm run dev
```

Without a terminal, `fire web` then starts a **detached** session, prints its id on stdout plus how
to read its output, and exits 0 right away. If a running session with the same command and
directory already exists, it reports that one instead of starting a duplicate (and updates its note
when `-m` or `FIRE_SESSION_NOTE` is given). With a terminal the
flag changes nothing (sessions are already on); it also tells agents, through the MCP server's
command list, to use `start_session`. Scripts that need to block until the command exits run it
with `fire --fg <command>` (or set `FIRE_SESSIONS=off` once for the whole script or CI job).

Both `session` and `long_running` can be set at the file level, as defaults for every command in
the file, and on a command group, for everything below it. The closest setting wins.

## Managing sessions
```bash
fire --bg -m "api for auth e2e tests" api dev     # start detached, prints the session id
fire cli ps                                        # list running sessions (all projects)
fire cli ps --failed --stopped                     # ...or ended ones (-a/--all, --exited, --failed, --stopped)
fire cli logs api-dev-3f2a [lines]                 # last lines of output as text (default 50)
fire cli fg api-dev-3f2a                           # attach in your terminal
fire cli send api-dev-3f2a rs                      # type a line (followed by Enter)
fire cli send --raw api-dev-3f2a 'q'               # type exact keys, no Enter
fire cli send --raw api-dev-3f2a '\x03'            # Ctrl+C as a key
fire cli kill api-dev-3f2a HUP                     # send a signal (INT, HUP, USR1, 9, ...)
fire cli kill api-dev-3f2a                         # stop it: TERM, then KILL after 5s
```

- `-m` / `--note` is a **very short context** about why the command was started. It shows up in
  `fire cli ps` and in the MCP `list_sessions` tool, so other agents know what is going on. Agents
  should always pass it.
- `--bg`, `-m` and `--new` are [global flags](commands-and-args.md#global-flags): they go before
  the command (`fire api dev --bg` passes `--bg` to the command) and combine with `--dry-run` and
  `--no-log`. `--bg --dry-run` prints what would run and starts nothing.
- Everything after the flags is a normal fire command path plus its arguments, resolved from the
  current directory (the session keeps that directory as its `cwd`).
- **One command in one directory is one session.** If the same command (same arguments) is already
  running from that directory, `--bg` prints its id and replaces its note instead of starting a
  duplicate. If it ended (crashed, finished or was stopped), it starts again under the **same id**,
  with the new note, and the previous run's output moves to `output.prev.log`. Restarting a server
  is `fire cli kill <id>` and the same `fire --bg` again. Running it in a terminal or detached by
  `long_running` reuses an ended session the same way. Pass `--new` to force a separate session.
- `fire --bg` starts the pty with your terminal's size, or 120x40 without one. Whoever attaches
  or resizes last sets the size.
- `fire cli bg [-m "note"] [--new] <command>` still works as a deprecated alias of `fire --bg` and
  prints a hint; it will be removed in a future release.
- `send --raw` understands `\r` (Enter), `\n`, `\t`, `\e` (Esc), `\\` and `\xHH` for any byte, e.g.
  `\e[A` for the up arrow.
- Signals go to the pty's foreground process group, like a keyboard signal would. `TERM`, `KILL`
  and `HUP` also reach the command's own process group. The intermediate `fire` process doesn't
  react to them; it waits for the command and mirrors its exit code.
- Inside a session the environment variable `FIRE_SESSION` holds the session id, and `TERM`
  defaults to `xterm-256color` when the starting environment had none.

### Attaching (`fg`)
`fire cli fg <id>` replays the recent output and then follows it live, in raw mode: every key goes
to the program, including Ctrl+C and Ctrl+\\. Ctrl+Z, `fg` and `bg` work as described above. When
the process exits, `fg` exits with the same code. Several terminals can be attached to the same
session at once.

A session started by an agent is the same as one you started: attach to it, type into it, or stop it.

## Output and lifecycle
- On a pty, stdout and stderr are one stream, as a terminal shows them. The raw bytes are kept for
  replay on attach; `fire cli logs` and the MCP `session_output` tool return clean text: colors and
  other escape sequences removed, and `\r` progress redraws collapsed to their final state.
- `fire cli ps` shows only `running` sessions, and says how many ended ones are hidden. Show those
  with flags, which combine: `-a`/`--all` (everything), `--exited` (every ended session),
  `--failed` (ended with a non-zero code on its own, or `lost`: the supervisor itself was killed)
  and `--stopped` (stopped on purpose with `fire cli kill` / MCP `signal_session`, so it isn't
  counted as a failure even though the exit code is non-zero). The flags complete in your shell.
- Statuses: `running`, `exited(<code>)`, `stopped` or `lost`.
- Exited sessions are removed automatically after 24 hours, or right away with `fire cli kill <id>`.

Session data lives in your local data directory (`~/Library/Application Support/fire/sessions` on
macOS, `~/.local/share/fire/sessions` on Linux): `session.json`, `output.log` and, while the
program runs, the supervisor's control socket `ctl.sock`. Sessions started by older fire versions
(`stdout.log`, `stderr.log` and a `stdin` FIFO) can still be listed, read, written to and stopped;
attaching to them isn't supported.

Sessions are Unix-only (macOS and Linux).

## Agents
The [MCP server](./mcp.md) exposes all of this: `list_sessions`, `session_output`,
`start_session`, `send_input` and `signal_session`. Its instructions tell agents to launch
long-running processes this way and check existing sessions first. Without MCP, agents can use the
same `fire cli` commands from a shell, and commands marked `long_running` detach by themselves.
