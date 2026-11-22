//! Detached sessions: run a fire command on a pseudo-terminal under a small supervisor so that
//! users and agents (from any terminal or project) can list it, read its output, attach to it,
//! send it input or signals, or kill it.
//!
//! Layout per session (`<data dir>/fire/sessions/<id>/`):
//! - `session.json` — metadata (command, note, cwd, pids, exit code)
//! - `output.log` — raw pty output (stdout and stderr merged, as a terminal shows them)
//! - `ctl.sock` — control socket of the supervisor while the program runs
//!
//! Sessions from older fire versions used pipes instead: `stdout.log`, `stderr.log` and a `stdin` FIFO.
//! Those are still readable and can be signalled until they are cleaned up.

mod attach;
mod clean;
mod proto;
mod pty;
mod supervisor;

use std::{
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    io::{self, IsTerminal, Read, Seek, SeekFrom, Write},
    os::unix::{ffi::OsStrExt, fs::OpenOptionsExt, net::UnixStream},
    path::{Path, PathBuf},
    process::{self, Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::resolve::ResolvedCommand;

const EXITED_RETENTION_SECS: u64 = 24 * 60 * 60;
const OUTPUT_LOG: &str = "output.log";
const SOCKET: &str = "ctl.sock";
/// `sun_path` holds 104 (macOS) to 108 (Linux) bytes including the trailing NUL.
const MAX_SOCKET_PATH: usize = 100;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Session {
    pub(crate) id: String,
    /// Arguments passed to `fire`, e.g. `["api", "dev"]`.
    pub(crate) command: Vec<String>,
    /// Short context about why this was started.
    pub(crate) note: Option<String>,
    pub(crate) cwd: String,
    pub(crate) started_at: u64,
    #[serde(default)]
    pub(crate) supervisor_pid: u32,
    pub(crate) pid: Option<u32>,
    pub(crate) exit_code: Option<i32>,
    pub(crate) ended_at: Option<u64>,
    /// Started implicitly by `fire <command>` in a terminal: removed when the program exits
    /// while someone is attached.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) auto: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct SessionInfo {
    #[serde(flatten)]
    pub(crate) session: Session,
    /// `running`, `exited` or `lost` (supervisor died without recording an exit).
    pub(crate) status: &'static str,
}

impl Session {
    pub(crate) fn status(&self) -> &'static str {
        if self.ended_at.is_some() {
            "exited"
        } else if self.supervisor_pid == 0 || pid_alive(self.supervisor_pid) {
            "running"
        } else {
            "lost"
        }
    }

    fn is_running(&self) -> bool {
        self.status() == "running"
    }

    pub(crate) fn info(self) -> SessionInfo {
        let status = self.status();
        SessionInfo {
            session: self,
            status,
        }
    }
}

pub(crate) fn handle(args: &[String]) {
    let rest = &args[1..];
    match rest.first().map(String::as_str) {
        Some("bg") => start(&rest[1..]),
        Some("ps") => print_sessions(),
        Some("logs") => match &rest[1..] {
            [id] => print_logs(id, 50),
            [id, lines] => print_logs(
                id,
                lines
                    .parse()
                    .unwrap_or_else(|_| fail("lines must be a number")),
            ),
            _ => fail("Usage: fire cli logs <id> [lines]"),
        },
        Some("fg") => match &rest[1..] {
            [id] => attach_existing(id),
            _ => fail("Usage: fire cli fg <id>"),
        },
        Some("kill") => match &rest[1..] {
            [id] => kill(id),
            [id, signal] => send_signal(id, signal),
            _ => fail("Usage: fire cli kill <id> [SIGNAL]"),
        },
        Some("send") => match &rest[1..] {
            [flag, id, text @ ..] if flag == "--raw" && !text.is_empty() => {
                send_input(id, unescape(&text.join(" ")))
            }
            [id, text @ ..] if !text.is_empty() => {
                // Enter on a terminal is CR; the line discipline turns it into a newline for
                // programs reading lines.
                let mut bytes = text.join(" ").into_bytes();
                bytes.push(b'\r');
                send_input(id, bytes)
            }
            _ => fail("Usage: fire cli send [--raw] <id> <text...>"),
        },
        Some("__supervise") => match &rest[1..] {
            [id] => supervisor::supervise(id, pty::DEFAULT_ROWS, pty::DEFAULT_COLS),
            [id, rows, cols] => supervisor::supervise(
                id,
                rows.parse().unwrap_or(pty::DEFAULT_ROWS),
                cols.parse().unwrap_or(pty::DEFAULT_COLS),
            ),
            _ => process::exit(2),
        },
        _ => fail("Unknown session command"),
    }
}

// ---------------------------------------------------------------- start

extern "C" fn ignore_signal(_: libc::c_int) {}

/// Inside a session, signals are delivered to the whole process group. Let the real command
/// decide what to do with them; this `fire` process just waits and mirrors its exit code.
/// A no-op handler (unlike SIG_IGN) is reset on exec, so children keep default behavior.
pub(crate) fn defer_signals_when_supervised() {
    if std::env::var_os("FIRE_SESSION").is_none() {
        return;
    }
    for signal in [
        libc::SIGHUP,
        libc::SIGINT,
        libc::SIGQUIT,
        libc::SIGTERM,
        libc::SIGUSR1,
        libc::SIGUSR2,
    ] {
        unsafe {
            libc::signal(signal, ignore_signal as *const () as libc::sighandler_t);
        }
    }
}

fn start(args: &[String]) {
    let mut note = None;
    let mut from_agent = false;
    let mut index = 0;
    while let Some(flag) = args.get(index).map(String::as_str) {
        if flag == "--agent" {
            from_agent = true;
            index += 1;
            continue;
        }
        if flag != "-m" && flag != "--note" {
            break;
        }
        note = Some(
            args.get(index + 1)
                .unwrap_or_else(|| fail("-m requires a short note"))
                .trim()
                .to_string(),
        );
        index += 2;
    }
    let command = &args[index..];
    if command.is_empty() {
        fail("Usage: fire cli bg [-m \"short context\"] <command> [args...]");
    }

    let config = crate::config::load_config();
    match crate::resolve::resolve_command(&config, command) {
        Some(resolved)
            if from_agent && !resolved.command_chain.iter().all(|entry| entry.allows_agents()) =>
        {
            fail(&format!(
                "fire {} is not available to agents (the user set `agents: false`); do not run it any other way either, ask the user to run it",
                command[..resolved.consumed].join(" ")
            ))
        }
        Some(resolved) if resolved.command.is_runnable() => {
            let id = spawn_session(command, resolved.consumed, note, false)
                .unwrap_or_else(|err| fail(&err));
            print_started(&id, command);
        }
        _ => fail(&format!(
            "Unknown or non-runnable command: fire {}",
            command.join(" ")
        )),
    }
}

fn print_started(id: &str, command: &[String]) {
    println!("{id}");
    eprintln!(
        "[fire] started `fire {}` as session {id}",
        command.join(" ")
    );
    eprintln!(
        "[fire] logs: fire cli logs {id} | attach: fire cli fg {id} | stop: fire cli kill {id}"
    );
}

/// Creates the session record and starts its supervisor. `consumed` is how many leading tokens
/// of `command` are the command path (used for the id).
fn spawn_session(
    command: &[String],
    consumed: usize,
    note: Option<String>,
    auto: bool,
) -> Result<String, String> {
    let id = new_id(&command[..consumed.clamp(1, command.len())].join("-"));
    let dir = session_dir(&id);
    fs::create_dir_all(&dir).map_err(|err| {
        format!(
            "Failed to create session directory {}: {err}",
            dir.display()
        )
    })?;

    let session = Session {
        id: id.clone(),
        command: command.to_vec(),
        note: note.filter(|value| !value.is_empty()),
        cwd: std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .display()
            .to_string(),
        started_at: now(),
        supervisor_pid: 0,
        pid: None,
        exit_code: None,
        ended_at: None,
        auto,
    };
    save(&session);

    let (rows, cols) = pty::terminal_size().unwrap_or((pty::DEFAULT_ROWS, pty::DEFAULT_COLS));
    let mut supervisor = Command::new(current_exe());
    supervisor
        .args([
            "cli",
            "__supervise",
            &id,
            &rows.to_string(),
            &cols.to_string(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Detach from the terminal so closing it doesn't take the process down.
    unsafe {
        use std::os::unix::process::CommandExt;
        supervisor.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    if let Err(err) = supervisor.spawn() {
        let _ = fs::remove_dir_all(&dir);
        return Err(format!("Failed to start session: {err}"));
    }
    Ok(id)
}

// ---------------------------------------------------------------- `fire <command>` in a session

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Launch {
    /// Run in this process, as fire always did.
    Direct,
    /// Start a session and attach this terminal to it right away.
    Attach,
    /// Start a detached session, print its id and return.
    Detach,
}

#[derive(Debug, Clone, Copy, Default)]
struct LaunchContext {
    inside_session: bool,
    disabled: bool,
    session_setting: Option<bool>,
    long_running: bool,
    terminal: bool,
}

fn choose_launch(context: LaunchContext) -> Launch {
    if context.inside_session || context.disabled || context.session_setting == Some(false) {
        Launch::Direct
    } else if context.terminal {
        Launch::Attach
    } else if context.long_running {
        Launch::Detach
    } else {
        Launch::Direct
    }
}

/// With a terminal, every `fire <command>` runs in a session the user can detach (Ctrl+Z) and
/// pick up again anywhere. Without one (agents, scripts), only `long_running` commands use a
/// session, detached, so they don't block the caller. Returns when the command should run
/// directly in this process.
pub(crate) fn run_in_session_if_enabled(command_args: &[String], resolved: &ResolvedCommand<'_>) {
    let setting = |pick: fn(&crate::config::CommandSpec) -> Option<bool>| {
        resolved
            .command_chain
            .iter()
            .rev()
            .find_map(|entry| entry.spec().and_then(pick))
    };
    let terminal = io::stdin().is_terminal() && io::stdout().is_terminal();
    let context = LaunchContext {
        inside_session: std::env::var_os("FIRE_SESSION").is_some(),
        disabled: env_flag_off("FIRE_SESSIONS") || env_flag_on("FIRE_DRY_RUN"),
        session_setting: setting(|spec| spec.session),
        long_running: setting(|spec| spec.long_running) == Some(true),
        // Started with `&`: keep today's behavior rather than stopping on the terminal.
        terminal: terminal && pty::is_foreground(0),
    };

    match choose_launch(context) {
        Launch::Direct => {}
        Launch::Attach => {
            let note = std::env::var("FIRE_SESSION_NOTE").ok().or_else(|| {
                resolved
                    .command
                    .description()
                    .and_then(|text| text.lines().next())
                    .map(|line| line.trim().to_string())
            });
            // If the supervisor can't start, fall back to running directly.
            if let Ok(id) = spawn_session(command_args, resolved.consumed, note, true) {
                attach::attach(
                    &id,
                    attach::Options {
                        quiet: true,
                        remove_on_exit: true,
                    },
                );
            }
        }
        Launch::Detach => {
            let cwd = std::env::current_dir()
                .map(|path| path.display().to_string())
                .unwrap_or_default();
            if let Some(existing) = list_sessions().into_iter().find(|session| {
                session.is_running() && session.command == command_args && session.cwd == cwd
            }) {
                println!("{}", existing.id);
                eprintln!(
                    "[fire] `fire {}` is already running here as session {}",
                    command_args.join(" "),
                    existing.id
                );
                print_detached_hints(&existing.id);
                process::exit(0);
            }
            let note = std::env::var("FIRE_SESSION_NOTE").ok();
            match spawn_session(command_args, resolved.consumed, note, false) {
                Ok(id) => {
                    println!("{id}");
                    eprintln!(
                        "[fire] `fire {}` is long-running; started it detached as session {id}",
                        command_args.join(" ")
                    );
                    print_detached_hints(&id);
                    process::exit(0);
                }
                Err(err) => eprintln!("[fire] {err}; running it directly"),
            }
        }
    }
}

fn print_detached_hints(id: &str) {
    eprintln!(
        "[fire] output: fire cli logs {id} (MCP: session_output) | attach: fire cli fg {id} | stop: fire cli kill {id}"
    );
    eprintln!("[fire] to block until it exits instead, set FIRE_SESSIONS=off");
}

fn env_flag_off(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "off" | "0" | "false" | "no"
        )
    })
}

fn env_flag_on(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| value.trim() == "true")
}

// ---------------------------------------------------------------- list / logs

pub(crate) fn list_sessions() -> Vec<Session> {
    let Ok(entries) = fs::read_dir(sessions_root()) else {
        return Vec::new();
    };
    let mut sessions: Vec<Session> = entries
        .flatten()
        .filter_map(|entry| load(&entry.file_name().to_string_lossy()))
        .filter(|session| {
            let stale = !session.is_running()
                && now().saturating_sub(session.ended_at.unwrap_or(session.started_at))
                    > EXITED_RETENTION_SECS;
            if stale {
                remove_session(&session.id);
            }
            !stale
        })
        .collect();
    sessions.sort_by_key(|session| std::cmp::Reverse(session.started_at));
    sessions
}

fn print_sessions() {
    let sessions = list_sessions();
    if sessions.is_empty() {
        println!("No sessions. Start one with: fire cli bg -m \"why\" <command>");
        return;
    }

    let home = dirs::home_dir().map(|path| path.display().to_string());
    let rows: Vec<[String; 6]> = sessions
        .iter()
        .map(|session| {
            let status = match (session.status(), session.exit_code) {
                ("exited", Some(code)) => format!("exited({code})"),
                (status, _) => status.to_string(),
            };
            let cwd = match &home {
                Some(home) if session.cwd.starts_with(home.as_str()) => {
                    format!("~{}", &session.cwd[home.len()..])
                }
                _ => session.cwd.clone(),
            };
            [
                session.id.clone(),
                status,
                format_age(now().saturating_sub(session.started_at)),
                cwd,
                format!("fire {}", session.command.join(" ")),
                session.note.clone().unwrap_or_default(),
            ]
        })
        .collect();

    let header = ["ID", "STATUS", "AGE", "DIR", "COMMAND", "NOTE"].map(str::to_string);
    let mut widths = [0usize; 6];
    for row in std::iter::once(&header).chain(rows.iter()) {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    for row in std::iter::once(&header).chain(rows.iter()) {
        let line: Vec<String> = row
            .iter()
            .zip(widths)
            .map(|(cell, width)| format!("{cell:<width$}"))
            .collect();
        println!("{}", line.join("  ").trim_end());
    }
}

/// Last `lines` lines of a session's output as plain text (escape sequences stripped, progress
/// rewrites collapsed).
pub(crate) fn read_output(id: &str, lines: usize) -> Option<(Session, String)> {
    let session = load(id)?;
    let dir = session_dir(id);
    let output = if dir.join(OUTPUT_LOG).exists() {
        tail(&dir.join(OUTPUT_LOG), lines)
    } else {
        let stdout = tail(&dir.join("stdout.log"), lines);
        let stderr = tail(&dir.join("stderr.log"), lines);
        if stderr.is_empty() {
            stdout
        } else {
            format!("{stdout}--- stderr ---\n{stderr}")
        }
    };
    Some((session, output))
}

fn print_logs(id: &str, lines: usize) {
    let Some((session, output)) = read_output(id, lines) else {
        fail(&format!("No session `{id}`. See: fire cli ps"))
    };
    eprintln!(
        "[fire] {} — fire {} ({})",
        session.id,
        session.command.join(" "),
        session.status()
    );
    print!("{output}");
}

fn tail(path: &Path, lines: usize) -> String {
    const MAX_BYTES: u64 = 256 * 1024;
    let Ok(mut file) = File::open(path) else {
        return String::new();
    };
    let len = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    let _ = file.seek(SeekFrom::Start(len.saturating_sub(MAX_BYTES)));
    let mut buffer = Vec::new();
    let _ = file.read_to_end(&mut buffer);
    let text = clean::clean(&String::from_utf8_lossy(&buffer));
    let all: Vec<&str> = text.lines().collect();
    let mut out = all[all.len().saturating_sub(lines)..].join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------- fg / kill / send

fn attach_existing(id: &str) {
    let Some(session) = load(id) else {
        fail(&format!("No session `{id}`. See: fire cli ps"))
    };
    if !session.is_running() {
        print_logs(id, 20);
        return;
    }
    attach::attach(
        id,
        attach::Options {
            quiet: false,
            remove_on_exit: session.auto,
        },
    );
}

fn kill(id: &str) {
    let Some(session) = load(id) else {
        fail(&format!("No session `{id}`. See: fire cli ps"))
    };
    if !session.is_running() {
        remove_session(id);
        println!("[fire] {id} was not running; removed");
        return;
    }

    signal_group(&session, libc::SIGTERM);
    for _ in 0..50 {
        if load(id).is_none_or(|current| !current.is_running()) {
            println!("[fire] {id} stopped");
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }
    signal_group(&session, libc::SIGKILL);
    println!("[fire] {id} killed");
}

fn send_signal(id: &str, name: &str) {
    let Some(signal) = parse_signal(name) else {
        fail(&format!(
            "Unknown signal `{name}`. Use a name (INT, TERM, HUP, USR1...) or number"
        ))
    };
    let session = running(id);
    signal_group(&session, signal);
    println!("[fire] sent {name} to {id}");
}

fn send_input(id: &str, bytes: Vec<u8>) {
    running(id);
    let result = match connect(id) {
        Some(mut stream) => proto::send(&mut stream, proto::INPUT, &bytes),
        // Pre-pty session: line-based FIFO.
        None => OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(session_dir(id).join("stdin"))
            .and_then(|mut fifo| {
                let mut line = bytes;
                if line.last() == Some(&b'\r') {
                    line.pop();
                    line.push(b'\n');
                }
                fifo.write_all(&line)
            }),
    };
    if let Err(err) = result {
        fail(&format!("Failed to write to {id}: {err}"));
    }
}

/// Escapes for `send --raw`: `\r` (Enter), `\n`, `\t`, `\e` (Esc), `\\`, and `\xHH` for any
/// byte, e.g. `\x03` for Ctrl+C.
fn unescape(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte != b'\\' || index + 1 == bytes.len() {
            out.push(byte);
            index += 1;
            continue;
        }
        let escaped = match bytes[index + 1] {
            b'r' => Some(b'\r'),
            b'n' => Some(b'\n'),
            b't' => Some(b'\t'),
            b'e' => Some(0x1b),
            b'\\' => Some(b'\\'),
            b'x' => {
                let value =
                    std::str::from_utf8(bytes.get(index + 2..index + 4).unwrap_or_default())
                        .ok()
                        .and_then(|hex| u8::from_str_radix(hex, 16).ok());
                if value.is_some() {
                    index += 2;
                }
                value
            }
            _ => None,
        };
        match escaped {
            Some(value) => {
                out.push(value);
                index += 2;
            }
            None => {
                out.push(byte);
                index += 1;
            }
        }
    }
    out
}

fn running(id: &str) -> Session {
    match load(id) {
        Some(session) if session.is_running() => session,
        Some(_) => fail(&format!("Session `{id}` is not running")),
        None => fail(&format!("No session `{id}`. See: fire cli ps")),
    }
}

pub(crate) fn parse_signal(value: &str) -> Option<libc::c_int> {
    if let Ok(number) = value.parse::<libc::c_int>() {
        return (number > 0).then_some(number);
    }
    let upper = value.to_ascii_uppercase();
    let name = upper.strip_prefix("SIG").unwrap_or(&upper);
    Some(match name {
        "HUP" => libc::SIGHUP,
        "INT" => libc::SIGINT,
        "QUIT" => libc::SIGQUIT,
        "KILL" => libc::SIGKILL,
        "USR1" => libc::SIGUSR1,
        "USR2" => libc::SIGUSR2,
        "TERM" => libc::SIGTERM,
        "CONT" => libc::SIGCONT,
        "STOP" => libc::SIGSTOP,
        "TSTP" => libc::SIGTSTP,
        "WINCH" => libc::SIGWINCH,
        _ => return None,
    })
}

/// The supervisor knows the pty's foreground process group, so ask it first; fall back to the
/// program's own group (pre-pty sessions, or a supervisor that stopped answering).
fn signal_group(session: &Session, signal: libc::c_int) {
    if let Some(mut stream) = connect(&session.id) {
        if proto::send(&mut stream, proto::SIGNAL, &signal.to_be_bytes()).is_ok() {
            return;
        }
    }
    // Re-read: the pid is recorded by the supervisor after spawning.
    let pid = load(&session.id).and_then(|current| current.pid);
    if let Some(pid) = pid {
        unsafe {
            libc::kill(-(pid as libc::pid_t), signal);
        }
    }
}

// ---------------------------------------------------------------- storage helpers

fn sessions_root() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("fire")
        .join("sessions")
}

fn session_dir(id: &str) -> PathBuf {
    sessions_root().join(id)
}

fn remove_session(id: &str) {
    let _ = fs::remove_dir_all(session_dir(id));
}

/// Connects to a running session's supervisor, waiting briefly while a fresh one starts up.
fn connect(id: &str) -> Option<UnixStream> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Ok(stream) = connect_once(&session_dir(id).join(SOCKET)) {
            return Some(stream);
        }
        let starting = load(id).is_some_and(|session| {
            session.ended_at.is_none() && session.pid.is_none() && session.is_running()
        });
        if !starting || Instant::now() >= deadline {
            return None;
        }
        thread::sleep(Duration::from_millis(2));
    }
}

fn connect_once(path: &Path) -> io::Result<UnixStream> {
    if path.as_os_str().len() < MAX_SOCKET_PATH {
        return UnixStream::connect(path);
    }
    // Too long for `sun_path`: connect through a relative path. Callers are single-threaded at
    // this point (CLI commands), so changing the cwd briefly is safe.
    let previous = std::env::current_dir()?;
    std::env::set_current_dir(path.parent().unwrap_or(Path::new(".")))?;
    let result = UnixStream::connect(path.file_name().unwrap_or(OsStr::from_bytes(b"")));
    let _ = std::env::set_current_dir(previous);
    result
}

fn load(id: &str) -> Option<Session> {
    if id.is_empty() || id.contains('/') || id.starts_with('.') {
        return None;
    }
    let text = fs::read_to_string(session_dir(id).join("session.json")).ok()?;
    serde_json::from_str(&text).ok()
}

fn save(session: &Session) {
    let dir = session_dir(&session.id);
    let tmp = dir.join(format!("session.json.{}", process::id()));
    let Ok(text) = serde_json::to_string_pretty(session) else {
        return;
    };
    if fs::write(&tmp, text).is_ok() {
        let _ = fs::rename(&tmp, dir.join("session.json"));
    }
}

fn new_id(name: &str) -> String {
    let slug: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .take(32)
        .collect();
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos())
        .unwrap_or(0)
        ^ process::id().rotate_left(16);
    format!("{}-{:04x}", slug.trim_matches('-'), seed & 0xffff)
}

fn pid_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

fn current_exe() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("fire"))
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn format_age(seconds: u64) -> String {
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3600 => format!("{}m", seconds / 60),
        3600..86400 => format!("{}h", seconds / 3600),
        _ => format!("{}d", seconds / 86400),
    }
}

fn fail(message: &str) -> ! {
    eprintln!("[fire] {message}");
    process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_is_slugged_with_suffix() {
        let id = new_id("web-start");
        assert!(id.starts_with("web-start-"), "{id}");
        let id = new_id("API_Dev");
        assert!(id.starts_with("api-dev-"), "{id}");
        assert_eq!(id.len(), "api-dev-".len() + 4);
        assert!(new_id(&"x".repeat(80)).len() <= 32 + 5);
    }

    #[test]
    fn tail_returns_last_clean_lines() {
        let dir = std::env::temp_dir().join(format!("fire-tail-{}", process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("out.log");
        fs::write(&path, "a\r\n\x1b[31mb\x1b[0m\r\n10%\r100%\r\n").unwrap();
        assert_eq!(tail(&path, 2), "b\n100%\n");
        assert_eq!(tail(&dir.join("missing"), 2), "");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn parses_signal_names_and_numbers() {
        assert_eq!(parse_signal("INT"), Some(libc::SIGINT));
        assert_eq!(parse_signal("sigterm"), Some(libc::SIGTERM));
        assert_eq!(parse_signal("9"), Some(9));
        assert_eq!(parse_signal("NOPE"), None);
        assert_eq!(parse_signal("0"), None);
    }

    #[test]
    fn unescapes_raw_keys() {
        assert_eq!(unescape(r"q"), b"q");
        assert_eq!(unescape(r"\x03"), vec![3]);
        assert_eq!(unescape(r"yes\r"), b"yes\r");
        assert_eq!(unescape(r"\e[A\t\\"), b"\x1b[A\t\\");
        assert_eq!(unescape(r"\xZZ\q\"), br"\xZZ\q\");
    }

    #[test]
    fn launch_follows_terminal_and_settings() {
        let tty = LaunchContext {
            terminal: true,
            ..LaunchContext::default()
        };
        assert_eq!(choose_launch(tty), Launch::Attach);
        assert_eq!(
            choose_launch(LaunchContext {
                session_setting: Some(false),
                ..tty
            }),
            Launch::Direct
        );
        assert_eq!(
            choose_launch(LaunchContext {
                inside_session: true,
                ..tty
            }),
            Launch::Direct
        );
        assert_eq!(
            choose_launch(LaunchContext {
                disabled: true,
                ..tty
            }),
            Launch::Direct
        );

        let piped = LaunchContext::default();
        assert_eq!(choose_launch(piped), Launch::Direct);
        assert_eq!(
            choose_launch(LaunchContext {
                long_running: true,
                ..piped
            }),
            Launch::Detach
        );
        assert_eq!(
            choose_launch(LaunchContext {
                long_running: true,
                session_setting: Some(false),
                ..piped
            }),
            Launch::Direct
        );
    }

    #[test]
    fn exited_session_reports_exited() {
        let session = Session {
            id: "x-0000".into(),
            command: vec!["x".into()],
            note: None,
            cwd: ".".into(),
            started_at: 0,
            supervisor_pid: 1,
            pid: Some(2),
            exit_code: Some(0),
            ended_at: Some(1),
            auto: false,
        };
        assert_eq!(session.status(), "exited");
    }
}
