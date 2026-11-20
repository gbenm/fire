//! Detached sessions: run a fire command in the background under a small supervisor so that
//! users and agents (from any terminal or project) can list it, read its output, attach to it,
//! or kill it.
//!
//! Layout per session (`<data dir>/fire/sessions/<id>/`):
//! - `session.json` — metadata (command, note, cwd, pids, exit code)
//! - `stdout.log` / `stderr.log` — captured output
//! - `stdin` — FIFO forwarded to the process's stdin while it runs

use std::{
    ffi::CString,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    os::unix::{
        fs::OpenOptionsExt,
        process::{CommandExt, ExitStatusExt},
    },
    path::{Path, PathBuf},
    process::{self, Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

const EXITED_RETENTION_SECS: u64 = 24 * 60 * 60;

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
            [id] => attach(id),
            _ => fail("Usage: fire cli fg <id>"),
        },
        Some("kill") => match &rest[1..] {
            [id] => kill(id),
            [id, signal] => send_signal(id, signal),
            _ => fail("Usage: fire cli kill <id> [SIGNAL]"),
        },
        Some("send") => match &rest[1..] {
            [id, text @ ..] if !text.is_empty() => send_input(id, &text.join(" ")),
            _ => fail("Usage: fire cli send <id> <text...>"),
        },
        Some("__supervise") => match &rest[1..] {
            [id] => supervise(id),
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
    let mut index = 0;
    while let Some(flag) = args.get(index).map(String::as_str) {
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
    let name = match crate::resolve::resolve_command(&config, command) {
        Some(resolved) if resolved.command.is_runnable() => command[resolved.consumed - 1].clone(),
        _ => fail(&format!(
            "Unknown or non-runnable command: fire {}",
            command.join(" ")
        )),
    };

    let id = new_id(&name);
    let dir = session_dir(&id);
    if let Err(err) = fs::create_dir_all(&dir).and_then(|_| mkfifo(&dir.join("stdin"))) {
        fail(&format!(
            "Failed to create session directory {}: {err}",
            dir.display()
        ));
    }

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
    };
    save(&session);

    let mut supervisor = Command::new(current_exe());
    supervisor
        .args(["cli", "__supervise", &id])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Detach from the terminal so closing it doesn't take the process down.
    unsafe {
        supervisor.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    if let Err(err) = supervisor.spawn() {
        let _ = fs::remove_dir_all(&dir);
        fail(&format!("Failed to start session: {err}"));
    }

    println!("{id}");
    eprintln!(
        "[fire] started `fire {}` as session {id}",
        command.join(" ")
    );
    eprintln!(
        "[fire] logs: fire cli logs {id} | attach: fire cli fg {id} | stop: fire cli kill {id}"
    );
}

fn supervise(id: &str) {
    let dir = session_dir(id);
    let Some(mut session) = load(id) else {
        process::exit(1)
    };
    session.supervisor_pid = process::id();
    save(&session);

    let open_log = |name: &str| File::create(dir.join(name)).unwrap_or_else(|_| process::exit(1));
    let stdout = open_log("stdout.log");
    let mut stderr = open_log("stderr.log");
    // Read+write keeps a writer open, so the process never sees EOF between attaches.
    let stdin = OpenOptions::new()
        .read(true)
        .write(true)
        .open(dir.join("stdin"));

    let mut command = Command::new(current_exe());
    command
        .args(&session.command)
        .current_dir(&session.cwd)
        .env("FIRE_SESSION", id)
        .stdout(stdout)
        .stderr(stderr.try_clone().unwrap_or_else(|_| process::exit(1)))
        .process_group(0);
    match stdin {
        Ok(file) => command.stdin(file),
        Err(_) => command.stdin(Stdio::null()),
    };

    let exit_code = match command.spawn() {
        Ok(mut child) => {
            session.pid = Some(child.id());
            save(&session);
            match child.wait() {
                Ok(status) => status
                    .code()
                    .unwrap_or_else(|| 128 + status.signal().unwrap_or(0)),
                Err(_) => 1,
            }
        }
        Err(err) => {
            let _ = writeln!(stderr, "[fire] failed to start: {err}");
            127
        }
    };

    session.exit_code = Some(exit_code);
    session.ended_at = Some(now());
    save(&session);
    let _ = fs::remove_file(dir.join("stdin"));
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
                let _ = fs::remove_dir_all(session_dir(&session.id));
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

/// Last `lines` lines of a session's stdout/stderr.
pub(crate) fn read_output(id: &str, lines: usize) -> Option<(Session, String, String)> {
    let session = load(id)?;
    let dir = session_dir(id);
    Some((
        session,
        tail(&dir.join("stdout.log"), lines),
        tail(&dir.join("stderr.log"), lines),
    ))
}

fn print_logs(id: &str, lines: usize) {
    let Some((session, stdout, stderr)) = read_output(id, lines) else {
        fail(&format!("No session `{id}`. See: fire cli ps"))
    };
    eprintln!(
        "[fire] {} — fire {} ({})",
        session.id,
        session.command.join(" "),
        session.status()
    );
    if !stdout.is_empty() {
        eprintln!("== stdout ==");
        print!("{stdout}");
    }
    if !stderr.is_empty() {
        eprintln!("== stderr ==");
        eprint!("{stderr}");
    }
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
    let text = String::from_utf8_lossy(&buffer);
    let all: Vec<&str> = text.lines().collect();
    let mut out = all[all.len().saturating_sub(lines)..].join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------- fg / kill

static INTERRUPT: AtomicBool = AtomicBool::new(false);
static DETACH: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(signal: libc::c_int) {
    if signal == libc::SIGTSTP {
        DETACH.store(true, Ordering::SeqCst);
    } else {
        INTERRUPT.store(true, Ordering::SeqCst);
    }
}

fn attach(id: &str) {
    let Some(session) = load(id) else {
        fail(&format!("No session `{id}`. See: fire cli ps"))
    };
    if !session.is_running() {
        print_logs(id, 20);
        return;
    }

    let dir = session_dir(id);
    print_logs(id, 20);
    eprintln!("[fire] attached — Ctrl+Z detaches (keeps running), Ctrl+C interrupts");

    unsafe {
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTSTP, on_signal as *const () as libc::sighandler_t);
    }

    let fifo = dir.join("stdin");
    thread::spawn(move || {
        let Ok(mut writer) = OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&fifo)
        else {
            return;
        };
        // Non-blocking was only needed for open(); writes should block normally.
        unsafe {
            use std::os::fd::AsRawFd;
            let fd = writer.as_raw_fd();
            libc::fcntl(
                fd,
                libc::F_SETFL,
                libc::fcntl(fd, libc::F_GETFL) & !libc::O_NONBLOCK,
            );
        }
        let mut buffer = [0u8; 4096];
        let mut stdin = io::stdin();
        while let Ok(read) = stdin.read(&mut buffer) {
            if read == 0 || writer.write_all(&buffer[..read]).is_err() {
                return;
            }
        }
    });

    let mut streams = [
        Follow::new(dir.join("stdout.log"), false),
        Follow::new(dir.join("stderr.log"), true),
    ];
    loop {
        if DETACH.load(Ordering::SeqCst) {
            eprintln!("\n[fire] detached from {id}; still running");
            process::exit(0);
        }
        if INTERRUPT.swap(false, Ordering::SeqCst) {
            signal_group(&session, libc::SIGINT);
        }
        let current = load(id);
        for stream in &mut streams {
            stream.pump();
        }
        match current {
            Some(current) if current.is_running() => {}
            Some(current) => {
                let code = current.exit_code.unwrap_or(1);
                eprintln!("[fire] {id} exited with code {code}");
                process::exit(code);
            }
            None => process::exit(1),
        }
        thread::sleep(Duration::from_millis(100));
    }
}

struct Follow {
    path: PathBuf,
    offset: u64,
    stderr: bool,
}

impl Follow {
    fn new(path: PathBuf, stderr: bool) -> Self {
        let offset = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        Self {
            path,
            offset,
            stderr,
        }
    }

    fn pump(&mut self) {
        let Ok(mut file) = File::open(&self.path) else {
            return;
        };
        if file.seek(SeekFrom::Start(self.offset)).is_err() {
            return;
        }
        let mut buffer = Vec::new();
        if file.read_to_end(&mut buffer).is_err() || buffer.is_empty() {
            return;
        }
        self.offset += buffer.len() as u64;
        if self.stderr {
            let _ = io::stderr().write_all(&buffer);
        } else {
            let mut out = io::stdout();
            let _ = out.write_all(&buffer);
            let _ = out.flush();
        }
    }
}

fn kill(id: &str) {
    let Some(session) = load(id) else {
        fail(&format!("No session `{id}`. See: fire cli ps"))
    };
    if !session.is_running() {
        let _ = fs::remove_dir_all(session_dir(id));
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

fn send_input(id: &str, text: &str) {
    running(id);
    let result = OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(session_dir(id).join("stdin"))
        .and_then(|mut fifo| fifo.write_all(format!("{text}\n").as_bytes()));
    if let Err(err) = result {
        fail(&format!("Failed to write to {id}: {err}"));
    }
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

fn signal_group(session: &Session, signal: libc::c_int) {
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
        .collect();
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos())
        .unwrap_or(0)
        ^ process::id().rotate_left(16);
    format!("{}-{:04x}", slug.trim_matches('-'), seed & 0xffff)
}

fn mkfifo(path: &Path) -> io::Result<()> {
    let path = CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
    if unsafe { libc::mkfifo(path.as_ptr(), 0o600) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
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
        let id = new_id("API_Dev");
        assert!(id.starts_with("api-dev-"), "{id}");
        assert_eq!(id.len(), "api-dev-".len() + 4);
    }

    #[test]
    fn tail_returns_last_lines() {
        let dir = std::env::temp_dir().join(format!("fire-tail-{}", process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("out.log");
        fs::write(&path, "a\nb\nc\n").unwrap();
        assert_eq!(tail(&path, 2), "b\nc\n");
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
        };
        assert_eq!(session.status(), "exited");
    }
}
