//! Follow mode (`fire cli logs -f`, `fire --bg -f`): streams a session's output, read-only, until
//! the program exits or the viewer is interrupted.
//!
//! It tails the session's log files instead of attaching over `ctl.sock`: the supervisor
//! records every byte there anyway, so following never sends anything to the program (no
//! input, no resize, no signals), works with supervisors of every fire version, and follows
//! sessions in the pipe layout (`stdout.log`, `stderr.log`) the same way. Ctrl+C reaches only
//! this viewer, never the session (its supervisor runs in a session of its own).

use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom, Write},
    path::PathBuf,
    process,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::Duration,
};

use super::{fail, load, session_dir, tail_until, Session, OUTPUT_LOG};

/// How often the log files and the session record are checked.
const POLL: Duration = Duration::from_millis(100);

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_interrupt(_: libc::c_int) {
    INTERRUPTED.store(true, Ordering::Relaxed);
}

/// Where following starts.
pub(crate) enum Start {
    /// The last lines already written (as clean text, like `fire cli logs`), then new output.
    Tail(usize),
    /// Everything from the beginning: a session that was just started.
    Beginning,
}

/// A log file and how much of it was shown.
struct Source {
    path: PathBuf,
    offset: u64,
    /// The pipe layout keeps stderr apart: it goes to our stderr.
    stderr: bool,
}

impl Source {
    fn len(&self) -> u64 {
        std::fs::metadata(&self.path)
            .map(|meta| meta.len())
            .unwrap_or(0)
    }

    /// Copies what was appended since the last call (raw bytes, as the program wrote them).
    fn pump(&mut self) {
        let Ok(mut file) = File::open(&self.path) else {
            return;
        };
        let len = file.metadata().map(|meta| meta.len()).unwrap_or(0);
        if len <= self.offset || file.seek(SeekFrom::Start(self.offset)).is_err() {
            return;
        }
        let mut bytes = Vec::new();
        if file
            .take(len - self.offset)
            .read_to_end(&mut bytes)
            .is_err()
        {
            return;
        }
        self.offset += bytes.len() as u64;
        self.write(&bytes);
    }

    fn write(&self, bytes: &[u8]) {
        let written = if self.stderr {
            let mut stderr = io::stderr();
            stderr.write_all(bytes).and_then(|_| stderr.flush())
        } else {
            let mut stdout = io::stdout();
            stdout.write_all(bytes).and_then(|_| stdout.flush())
        };
        // Nobody reads us anymore (`| head`): stop quietly; the session keeps running.
        if written.is_err() {
            process::exit(0);
        }
    }
}

fn sources(session: &Session) -> Vec<Source> {
    let dir = session_dir(&session.id);
    let source = |name: &str, stderr: bool| Source {
        path: dir.join(name),
        offset: 0,
        stderr,
    };
    if session.protocol() == 0 {
        vec![source("stdout.log", false), source("stderr.log", true)]
    } else {
        vec![source(OUTPUT_LOG, false)]
    }
}

/// Follows session `id` until its program exits (then exits with its code) or Ctrl+C (then
/// exits 130 and leaves the session running). `header` prints the session line `fire cli logs`
/// starts with.
pub(crate) fn follow(id: &str, start: Start, header: bool) -> ! {
    let Some(session) = load(id) else {
        fail(&format!("No session `{id}`. See: fire cli ps"))
    };
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = on_interrupt as *const () as libc::sighandler_t;
        libc::sigaction(libc::SIGINT, &action, std::ptr::null_mut());
    }
    if header {
        eprintln!(
            "[fire] {} — fire {} ({})",
            session.id,
            session.command.join(" "),
            session.status()
        );
    }

    let mut sources = sources(&session);
    if let Start::Tail(lines) = start {
        for source in &mut sources {
            source.offset = source.len();
            let text = tail_until(&source.path, source.offset, lines);
            source.write(text.as_bytes());
        }
    }
    if session.is_running() {
        eprintln!(
            "[fire] following {id}: Ctrl+C stops following (it keeps running) | attach: fire cli fg {id} | stop: fire cli kill {id}"
        );
    }

    loop {
        // Read the record first: whatever the program wrote before it was marked as ended is
        // in the logs by then, so the pump below shows all of it.
        let current = load(id);
        for source in &mut sources {
            source.pump();
        }
        match current {
            Some(session) if session.ended_at.is_some() => {
                let code = session.exit_code.unwrap_or(1);
                eprintln!("[fire] {id} exited with code {code}");
                process::exit(code);
            }
            Some(session) if session.status() == "lost" => {
                eprintln!("[fire] lost track of {id}: its supervisor died");
                process::exit(1);
            }
            Some(_) => {}
            None => {
                eprintln!("[fire] {id} ended and its session was removed");
                process::exit(1);
            }
        }
        if INTERRUPTED.load(Ordering::Relaxed) {
            eprintln!(
                "\n[fire] stopped following; {id} keeps running (attach: fire cli fg {id} | stop: fire cli kill {id})"
            );
            process::exit(130);
        }
        thread::sleep(POLL);
    }
}
