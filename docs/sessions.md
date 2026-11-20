# Detached sessions

`fire cli bg` runs any fire command in the background under a small supervisor. The process keeps
running after the terminal (or the agent) that started it goes away, and anyone on the machine
under the same user (you, or another agent in another project) can see it, read its output, send it
input or signals, attach to it, or stop it.

This is meant for working with AI agents across several projects at once: before restarting a dev
server, an agent can check what is already running, why it was started, and what it is printing.

## Commands
```bash
fire cli bg -m "api for auth e2e tests" api dev   # start detached, prints the session id
fire cli ps                                        # list sessions (all projects)
fire cli logs api-dev-3f2a [lines]                 # last stdout/stderr lines (default 50)
fire cli send api-dev-3f2a rs                      # write a line to its stdin
fire cli kill api-dev-3f2a HUP                     # send a signal (INT, HUP, USR1, 9, ...)
fire cli kill api-dev-3f2a                         # stop it: TERM, then KILL after 5s
fire cli fg api-dev-3f2a                           # attach in your terminal
```

- `-m` / `--note` is a **very short context** about why the command was started. It shows up in
  `fire cli ps` and in the MCP `list_sessions` tool, so other agents know what is going on. Agents
  should always pass it.
- Everything after the flags is a normal fire command path plus its arguments, resolved from the
  current directory (the session keeps that directory as its `cwd`).
- Signals go to the whole process group of the command. The intermediate `fire` process doesn't
  react to them; it waits for the command and mirrors its exit code.
- Inside a session the environment variable `FIRE_SESSION` holds the session id.

## Attaching (`fg`)
`fire cli fg <id>` prints the recent output and then follows it live, forwarding your keyboard input
to the process:
- **Ctrl+Z** detaches: the same key you use to send a job to the background leaves the process
  running in its session.
- **Ctrl+C** sends `SIGINT` to the process (like a regular foreground job).
- When the process exits, `fg` exits with the same code.

A session started by an agent is the same as one you started: attach to it, type into it, or stop it.

## Output and lifecycle
- stdout and stderr are kept separate. Output goes to files, not a terminal, so programs that
  only colorize or line-buffer on a TTY may behave as if piped.
- Input is line-oriented (no TTY), so full-screen TUIs aren't supported.
- `fire cli ps` shows `running`, `exited(<code>)` or `lost` (the supervisor itself was killed).
- Exited sessions are removed automatically after 24 hours, or right away with `fire cli kill <id>`.

Session data lives in your local data directory (`~/Library/Application Support/fire/sessions` on
macOS, `~/.local/share/fire/sessions` on Linux): `session.json`, `stdout.log`, `stderr.log` and a
`stdin` FIFO per session.

## Agents
The [MCP server](./mcp.md) exposes all of this: `list_sessions`, `session_output`,
`start_session`, `send_input` and `signal_session`. Its instructions tell agents to launch
long-running processes this way and check existing sessions first. Without MCP, agents can use the
same `fire cli` commands from a shell.
