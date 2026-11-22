//! The supervisor owns the pty master of a session: it records every byte to `output.log`,
//! streams it to attached clients over `ctl.sock`, and forwards their input, resizes and signals.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    os::{
        fd::{AsRawFd, RawFd},
        unix::{
            fs::OpenOptionsExt,
            net::{UnixListener, UnixStream},
            process::CommandExt,
        },
    },
    path::Path,
    process::{self, Command, Stdio},
    sync::atomic::{AtomicI32, Ordering},
    time::{Duration, Instant},
};

use super::{current_exe, load, now, proto, pty, save, session_dir, Session, OUTPUT_LOG, SOCKET};

/// Replay for a fresh attach: enough recent output to give context.
const REPLAY_TAIL: u64 = 16 * 1024;
/// Upper bound when replaying from an offset (reattaching after Ctrl+Z / shell `fg`).
const REPLAY_MAX: u64 = 256 * 1024;
/// A client that falls this far behind is dropped instead of slowing the program down.
const CLIENT_BACKLOG: usize = 8 * 1024 * 1024;
/// Delay between the two resizes that force full-screen programs to redraw on attach.
const REDRAW_NUDGE: Duration = Duration::from_millis(40);

struct Client {
    stream: UnixStream,
    inbox: Vec<u8>,
    outbox: Vec<u8>,
    attached: bool,
    closed: bool,
}

static CHILD_EXITED: AtomicI32 = AtomicI32::new(-1);

extern "C" fn on_child_exit(_: libc::c_int) {
    let fd = CHILD_EXITED.load(Ordering::Relaxed);
    if fd >= 0 {
        unsafe {
            libc::write(fd, b"x".as_ptr() as *const libc::c_void, 1);
        }
    }
}

/// Wakes `poll` as soon as the program exits instead of on the next timeout.
fn child_exit_pipe() -> RawFd {
    let mut fds = [-1; 2];
    unsafe {
        if libc::pipe(fds.as_mut_ptr()) != 0 {
            return -1;
        }
        for fd in fds {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            pty::set_nonblocking(fd);
        }
        CHILD_EXITED.store(fds[1], Ordering::Relaxed);
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = on_child_exit as *const () as libc::sighandler_t;
        action.sa_flags = libc::SA_RESTART | libc::SA_NOCLDSTOP;
        libc::sigaction(libc::SIGCHLD, &action, std::ptr::null_mut());
    }
    fds[0]
}

struct Supervisor {
    pid: libc::pid_t,
    child_exited: RawFd,
    master: File,
    log: File,
    log_len: u64,
    to_pty: Vec<u8>,
    pending_size: Option<(Instant, u16, u16)>,
}

pub(crate) fn supervise(id: &str, rows: u16, cols: u16) {
    let dir = session_dir(id);
    let Some(mut session) = load(id) else {
        process::exit(1)
    };
    session.supervisor_pid = process::id();
    save(&session);

    // Relative socket path: `sun_path` is ~104 bytes and data dirs can be long.
    if std::env::set_current_dir(&dir).is_err() {
        process::exit(1);
    }
    let Ok(mut log) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(OUTPUT_LOG)
    else {
        process::exit(1)
    };

    let (pty, listener) = match open_pty_and_socket() {
        Ok(opened) => opened,
        Err(err) => fail_start(&mut session, &mut log, &format!("cannot open a pty: {err}")),
    };
    pty::set_size(pty.master.as_raw_fd(), rows, cols);
    // Holding a slave fd keeps the master readable (no EOF/EIO) until the program exits; the
    // exit itself is detected with waitpid.
    let _slave = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOCTTY)
        .open(Path::new(&*pty.slave_path.to_string_lossy()));

    let mut command = Command::new(current_exe());
    command
        .args(&session.command)
        .current_dir(&session.cwd)
        .env("FIRE_SESSION", id)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if std::env::var("TERM").map_or(true, |term| term.is_empty() || term == "dumb") {
        command.env("TERM", "xterm-256color");
    }
    let child_exited = child_exit_pipe();
    let slave_path = pty.slave_path.clone();
    unsafe {
        command.pre_exec(move || pty::become_session_leader_on(&slave_path));
    }
    let child = match command.spawn() {
        Ok(child) => child,
        Err(err) => fail_start(&mut session, &mut log, &format!("failed to start: {err}")),
    };
    session.pid = Some(child.id());
    save(&session);

    let master = File::from(pty.master);
    pty::set_nonblocking(master.as_raw_fd());
    let log_len = log.metadata().map(|meta| meta.len()).unwrap_or(0);
    let mut supervisor = Supervisor {
        pid: child.id() as libc::pid_t,
        child_exited,
        master,
        log,
        log_len,
        to_pty: Vec::new(),
        pending_size: None,
    };
    let mut clients: Vec<Client> = Vec::new();

    let exit_code = loop {
        supervisor.poll_once(&listener, &mut clients);
        if let Some(code) = supervisor.try_wait() {
            break code;
        }
    };

    // Output written right before exiting may still be buffered in the pty.
    supervisor.pump_output(&mut clients);
    // Re-read: the note or `stopped` may have changed while it ran.
    let mut session = load(id).unwrap_or(session);
    session.exit_code = Some(exit_code);
    session.ended_at = Some(now());
    save(&session);

    let exit = proto::encode(proto::EXIT, &exit_code.to_be_bytes());
    for client in clients.iter_mut().filter(|client| client.attached) {
        client.outbox.extend_from_slice(&exit);
        let _ = client.stream.set_nonblocking(false);
        let _ = client
            .stream
            .set_write_timeout(Some(Duration::from_secs(1)));
        let _ = client.stream.write_all(&client.outbox);
    }
    drop(listener);
    let _ = fs::remove_file(SOCKET);
}

fn open_pty_and_socket() -> io::Result<(pty::Pty, UnixListener)> {
    let pty = pty::open()?;
    let _ = fs::remove_file(SOCKET);
    let listener = UnixListener::bind(SOCKET)?;
    listener.set_nonblocking(true)?;
    Ok((pty, listener))
}

fn fail_start(session: &mut Session, log: &mut File, message: &str) -> ! {
    let _ = writeln!(log, "[fire] {message}");
    session.exit_code = Some(127);
    session.ended_at = Some(now());
    save(session);
    let _ = fs::remove_file(SOCKET);
    process::exit(1);
}

impl Supervisor {
    fn poll_once(&mut self, listener: &UnixListener, clients: &mut Vec<Client>) {
        let master_fd = self.master.as_raw_fd();
        let mut fds = vec![
            poll_fd(
                master_fd,
                libc::POLLIN
                    | if self.to_pty.is_empty() {
                        0
                    } else {
                        libc::POLLOUT
                    },
            ),
            poll_fd(listener.as_raw_fd(), libc::POLLIN),
            poll_fd(self.child_exited, libc::POLLIN),
        ];
        for client in clients.iter() {
            let events = libc::POLLIN
                | if client.outbox.is_empty() {
                    0
                } else {
                    libc::POLLOUT
                };
            fds.push(poll_fd(client.stream.as_raw_fd(), events));
        }
        let timeout = self
            .pending_size
            .map(|(at, ..)| at.saturating_duration_since(Instant::now()).as_millis() as i32)
            .unwrap_or(200)
            .clamp(0, 200);
        unsafe {
            libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout);
        }

        if fds[0].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
            self.pump_output(clients);
        }
        if fds[0].revents & libc::POLLOUT != 0 {
            self.flush_input();
        }
        if fds[1].revents & libc::POLLIN != 0 {
            while let Ok((stream, _)) = listener.accept() {
                if stream.set_nonblocking(true).is_ok() {
                    clients.push(Client {
                        stream,
                        inbox: Vec::new(),
                        outbox: Vec::new(),
                        attached: false,
                        closed: false,
                    });
                }
            }
        }
        if fds[2].revents & libc::POLLIN != 0 {
            let mut drained = [0u8; 64];
            while unsafe {
                libc::read(
                    self.child_exited,
                    drained.as_mut_ptr() as *mut libc::c_void,
                    drained.len(),
                )
            } > 0
            {}
        }
        for (client, fd) in clients.iter_mut().zip(&fds[3..]) {
            if fd.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
                self.read_client(client);
            }
            if fd.revents & libc::POLLOUT != 0 {
                flush_client(client);
            }
        }
        clients.retain(|client| !client.closed);

        if let Some((at, rows, cols)) = self.pending_size {
            if Instant::now() >= at {
                pty::set_size(master_fd, rows, cols);
                self.pending_size = None;
            }
        }
    }

    fn try_wait(&self) -> Option<i32> {
        let mut status = 0;
        let reaped = unsafe { libc::waitpid(self.pid, &mut status, libc::WNOHANG) };
        if reaped == self.pid {
            Some(if libc::WIFEXITED(status) {
                libc::WEXITSTATUS(status)
            } else if libc::WIFSIGNALED(status) {
                128 + libc::WTERMSIG(status)
            } else {
                1
            })
        } else if reaped < 0 {
            Some(1)
        } else {
            None
        }
    }

    fn pump_output(&mut self, clients: &mut [Client]) {
        let mut buffer = [0u8; 64 * 1024];
        loop {
            match self.master.read(&mut buffer) {
                Ok(0) => return,
                Ok(read) => {
                    let chunk = &buffer[..read];
                    let _ = self.log.write_all(chunk);
                    self.log_len += read as u64;
                    let frame = proto::encode(proto::OUTPUT, chunk);
                    for client in clients.iter_mut().filter(|client| client.attached) {
                        client.outbox.extend_from_slice(&frame);
                        flush_client(client);
                    }
                }
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => return,
            }
        }
    }

    fn flush_input(&mut self) {
        while !self.to_pty.is_empty() {
            match self.master.write(&self.to_pty) {
                Ok(written) => {
                    self.to_pty.drain(..written);
                }
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => return,
            }
        }
    }

    fn read_client(&mut self, client: &mut Client) {
        match proto::fill(&mut client.stream, &mut client.inbox) {
            Ok(true) => {}
            Ok(false) => client.closed = true,
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {}
            Err(_) => client.closed = true,
        }
        // Frames that arrived before a hangup still count (e.g. `fire cli send` writes and quits).
        loop {
            match proto::decode(&mut client.inbox) {
                Ok(Some((kind, payload))) => self.handle(client, kind, &payload),
                Ok(None) => break,
                Err(()) => {
                    client.closed = true;
                    break;
                }
            }
        }
    }

    fn handle(&mut self, client: &mut Client, kind: u8, payload: &[u8]) {
        let master_fd = self.master.as_raw_fd();
        match kind {
            proto::INPUT => {
                self.to_pty.extend_from_slice(payload);
                self.flush_input();
            }
            proto::RESIZE => {
                if let (Some(rows), Some(cols)) =
                    (proto::read_u16(payload, 0), proto::read_u16(payload, 2))
                {
                    self.pending_size = None;
                    pty::set_size(master_fd, rows, cols);
                }
            }
            proto::SIGNAL => {
                if let Some(signal) = proto::read_i32(payload) {
                    signal_program(master_fd, self.pid, signal);
                }
            }
            proto::ATTACH => {
                let since = proto::read_u64(payload, 4).unwrap_or(proto::TAIL);
                let (start, replay) = self.replay(since);
                client
                    .outbox
                    .extend(proto::encode(proto::HELLO, &start.to_be_bytes()));
                if !replay.is_empty() {
                    client.outbox.extend(proto::encode(proto::OUTPUT, &replay));
                }
                client.attached = true;
                flush_client(client);

                if let (Some(rows), Some(cols)) =
                    (proto::read_u16(payload, 0), proto::read_u16(payload, 2))
                {
                    if self.log_len > 0 && rows > 1 {
                        // Same trick as tmux/abduco: a size change makes full-screen
                        // programs repaint the whole screen for the new viewer.
                        pty::set_size(master_fd, rows - 1, cols);
                        self.pending_size = Some((Instant::now() + REDRAW_NUDGE, rows, cols));
                    } else {
                        pty::set_size(master_fd, rows, cols);
                    }
                }
            }
            _ => {}
        }
    }

    /// Bytes of the log to replay for a new viewer, and the offset they start at.
    fn replay(&self, since: u64) -> (u64, Vec<u8>) {
        let tail = since == proto::TAIL || since > self.log_len;
        let mut start = if tail {
            self.log_len.saturating_sub(REPLAY_TAIL)
        } else {
            since.max(self.log_len.saturating_sub(REPLAY_MAX))
        };
        let mut bytes = Vec::new();
        if let Ok(mut file) = File::open(OUTPUT_LOG) {
            let _ = file.seek(SeekFrom::Start(start));
            let _ = file
                .take(self.log_len.saturating_sub(start))
                .read_to_end(&mut bytes);
        }
        if tail && start > 0 {
            // Start on a line boundary rather than in the middle of a line or escape sequence.
            if let Some(newline) = bytes.iter().position(|&byte| byte == b'\n') {
                bytes.drain(..=newline);
                start += newline as u64 + 1;
            }
        }
        (start, bytes)
    }
}

/// Signals go to the pty's foreground process group, as a keyboard signal would. Stopping
/// signals also reach the program's own group in case it moved a job to the foreground.
fn signal_program(master_fd: RawFd, pid: libc::pid_t, signal: libc::c_int) {
    let foreground = unsafe { libc::tcgetpgrp(master_fd) };
    unsafe {
        if foreground > 0 {
            libc::kill(-foreground, signal);
        }
        if foreground <= 0
            || (foreground != pid && matches!(signal, libc::SIGTERM | libc::SIGKILL | libc::SIGHUP))
        {
            libc::kill(-pid, signal);
        }
    }
}

fn flush_client(client: &mut Client) {
    while !client.outbox.is_empty() {
        match client.stream.write(&client.outbox) {
            Ok(written) => {
                client.outbox.drain(..written);
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => break,
            Err(_) => {
                client.closed = true;
                return;
            }
        }
    }
    if client.outbox.len() > CLIENT_BACKLOG {
        client.closed = true;
    }
}

fn poll_fd(fd: RawFd, events: libc::c_short) -> libc::pollfd {
    libc::pollfd {
        fd,
        events,
        revents: 0,
    }
}
