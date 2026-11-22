//! The viewer side of a session: puts the local terminal in raw mode and relays bytes between it
//! and the supervisor. The shell's job control only ever sees this viewer, so Ctrl+Z / `fg` /
//! `bg` detach and reattach it while the program keeps running under the supervisor.

use std::{
    fs::File,
    io::{self, IsTerminal, Read, Seek, SeekFrom, Write},
    os::{fd::AsRawFd, unix::net::UnixStream},
    process,
    sync::atomic::{AtomicI32, Ordering},
};

use super::{connect, load, proto, pty, remove_session, session_dir, OUTPUT_LOG};

static SIGNAL_PIPE: AtomicI32 = AtomicI32::new(-1);

extern "C" fn on_signal(signal: libc::c_int) {
    let fd = SIGNAL_PIPE.load(Ordering::Relaxed);
    if fd >= 0 {
        let byte = signal as u8;
        unsafe {
            libc::write(fd, &byte as *const u8 as *const libc::c_void, 1);
        }
    }
}

pub(crate) struct Options {
    /// Print nothing of fire's own, so the session looks like running the command directly.
    pub(crate) quiet: bool,
    /// Remove the session once the program exits while attached (sessions `fire <cmd>` starts).
    pub(crate) remove_on_exit: bool,
}

enum Ended {
    Exit(i32),
    Lost,
    Suspend,
    Hangup,
}

struct RawGuard(Option<libc::termios>);

impl RawGuard {
    fn restore(&mut self) {
        if let Some(original) = self.0.take() {
            pty::restore(0, &original);
        }
    }
}

impl Drop for RawGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

pub(crate) fn attach(id: &str, options: Options) -> ! {
    let signals = signal_pipe();
    let interactive = io::stdin().is_terminal();
    let mut offset = proto::TAIL;

    if !options.quiet {
        if interactive {
            eprintln!("[fire] attached to {id}: Ctrl+Z detaches (it keeps running; `fg` reattaches), Ctrl+C goes to the program");
        } else {
            eprintln!("[fire] attached to {id}");
        }
    }

    loop {
        if interactive && !pty::is_foreground(0) {
            suspend_until_foreground(signals);
        }
        let Some(mut stream) = connect(id) else {
            finish_without_socket(id, &options, offset)
        };
        drain(signals);
        let mut raw = RawGuard(if interactive {
            pty::enter_raw(0).ok()
        } else {
            None
        });
        let (rows, cols) = pty::terminal_size().unwrap_or((pty::DEFAULT_ROWS, pty::DEFAULT_COLS));
        if proto::send(
            &mut stream,
            proto::ATTACH,
            &proto::attach_payload(rows, cols, offset),
        )
        .is_err()
        {
            raw.restore();
            finish_without_socket(id, &options, offset);
        }

        let ended = relay(&mut stream, signals, interactive, &mut offset);
        raw.restore();
        drop(stream);
        match ended {
            Ended::Exit(code) => {
                if options.remove_on_exit {
                    remove_session(id);
                } else if !options.quiet {
                    eprintln!("[fire] {id} exited with code {code}");
                }
                process::exit(code);
            }
            Ended::Lost => finish_without_socket(id, &options, offset),
            Ended::Hangup => process::exit(129),
            Ended::Suspend => {
                eprintln!(
                    "\r\n[fire] detached; {id} keeps running (`fg` reattaches, or: fire cli fg {id})"
                );
                suspend_until_foreground(signals);
            }
        }
    }
}

fn relay(stream: &mut UnixStream, signals: i32, interactive: bool, offset: &mut u64) -> Ended {
    let mut inbox = Vec::new();
    let mut stdin_open = true;
    let mut stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut buffer = [0u8; 16 * 1024];

    loop {
        let mut fds = [
            poll_fd(signals),
            poll_fd(stream.as_raw_fd()),
            poll_fd(if stdin_open { 0 } else { -1 }),
        ];
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1) };
        if ready < 0 {
            continue; // EINTR
        }

        if fds[0].revents & libc::POLLIN != 0 {
            let mut received = [0u8; 64];
            let read = unsafe {
                libc::read(
                    signals,
                    received.as_mut_ptr() as *mut libc::c_void,
                    received.len(),
                )
            };
            for &signal in &received[..read.max(0) as usize] {
                match signal as libc::c_int {
                    libc::SIGWINCH => {
                        if let Some((rows, cols)) = pty::terminal_size() {
                            let _ = proto::send(
                                stream,
                                proto::RESIZE,
                                &proto::size_payload(rows, cols),
                            );
                        }
                    }
                    libc::SIGTSTP if interactive => return Ended::Suspend,
                    libc::SIGINT => {
                        let _ = proto::send(stream, proto::SIGNAL, &libc::SIGINT.to_be_bytes());
                    }
                    libc::SIGHUP | libc::SIGTERM => return Ended::Hangup,
                    _ => {}
                }
            }
        }

        if fds[1].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
            let open = proto::fill(stream, &mut inbox).unwrap_or(false);
            while let Ok(Some((kind, payload))) = proto::decode(&mut inbox) {
                match kind {
                    proto::HELLO => *offset = proto::read_u64(&payload, 0).unwrap_or(0),
                    proto::OUTPUT => {
                        let _ = stdout.write_all(&payload);
                        let _ = stdout.flush();
                        *offset = offset.saturating_add(payload.len() as u64);
                    }
                    proto::EXIT => return Ended::Exit(proto::read_i32(&payload).unwrap_or(1)),
                    _ => {}
                }
            }
            if !open {
                return Ended::Lost;
            }
        }

        if fds[2].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
            match stdin.read(&mut buffer) {
                Ok(0) | Err(_) => stdin_open = false,
                Ok(read) => {
                    if proto::send(stream, proto::INPUT, &buffer[..read]).is_err() {
                        return Ended::Lost;
                    }
                }
            }
        }
    }
}

/// Stop like any job the shell suspends; the program is unaffected. On SIGCONT, carry on only
/// once the shell gave us the terminal back (`fg`); after `bg` there is nothing to show, so stop
/// again instead of fighting the shell for the terminal. A shell that exits with stopped jobs
/// sends SIGHUP + SIGCONT: then the viewer ends (the program keeps running).
fn suspend_until_foreground(signals: i32) {
    loop {
        unsafe {
            // SIGTSTP makes the shell report a plain "suspended" job, but the kernel discards it
            // for orphaned process groups (no parent shell in our session); SIGSTOP always works.
            let parent = libc::getppid();
            if parent > 1 && libc::getsid(parent) == libc::getsid(0) {
                libc::signal(libc::SIGTSTP, libc::SIG_DFL);
                libc::raise(libc::SIGTSTP);
                let mut action: libc::sigaction = std::mem::zeroed();
                action.sa_sigaction = on_signal as *const () as libc::sighandler_t;
                action.sa_flags = libc::SA_RESTART;
                libc::sigaction(libc::SIGTSTP, &action, std::ptr::null_mut());
            } else {
                libc::raise(libc::SIGSTOP);
            }
        }
        let orphaned = unsafe { libc::getppid() == 1 || libc::tcgetpgrp(0) < 0 };
        if hung_up(signals) || orphaned {
            process::exit(129);
        }
        if pty::is_foreground(0) {
            return;
        }
    }
}

/// Drains pending signals; true if one of them asked us to go away.
fn hung_up(signals: i32) -> bool {
    let mut buffer = [0u8; 64];
    let mut hangup = false;
    loop {
        let read = unsafe {
            libc::read(
                signals,
                buffer.as_mut_ptr() as *mut libc::c_void,
                buffer.len(),
            )
        };
        if read <= 0 {
            return hangup;
        }
        hangup |= buffer[..read as usize]
            .iter()
            .any(|&signal| matches!(signal as libc::c_int, libc::SIGHUP | libc::SIGTERM));
    }
}

/// The socket is gone: the program exited (or the supervisor died) while we weren't listening.
/// Whatever we haven't shown yet is still in the log (a quick command can finish before the
/// viewer connects), so print it before reporting the exit.
fn finish_without_socket(id: &str, options: &Options, offset: u64) -> ! {
    match load(id) {
        Some(session) if session.ended_at.is_some() => {
            if let Ok(mut log) = File::open(session_dir(id).join(OUTPUT_LOG)) {
                let len = log.metadata().map(|meta| meta.len()).unwrap_or(0);
                let start = match offset {
                    proto::TAIL if options.remove_on_exit => 0,
                    proto::TAIL => len.saturating_sub(16 * 1024),
                    seen => seen.min(len),
                };
                if log.seek(SeekFrom::Start(start)).is_ok() {
                    let mut stdout = io::stdout();
                    let _ = io::copy(&mut log, &mut stdout);
                    let _ = stdout.flush();
                }
            }
            let code = session.exit_code.unwrap_or(1);
            if options.remove_on_exit {
                remove_session(id);
            } else if !options.quiet {
                eprintln!("[fire] {id} exited with code {code}");
            }
            process::exit(code);
        }
        Some(session) if session.status() == "running" => {
            eprintln!(
                "[fire] {id} has no control socket (started by an older fire?). Read it with: fire cli logs {id}"
            );
            process::exit(1);
        }
        Some(_) => {
            eprintln!("[fire] lost track of {id}: its supervisor died");
            process::exit(1);
        }
        None => process::exit(1),
    }
}

fn signal_pipe() -> i32 {
    let mut fds = [0; 2];
    unsafe {
        if libc::pipe(fds.as_mut_ptr()) != 0 {
            eprintln!(
                "[fire] cannot create a pipe: {}",
                io::Error::last_os_error()
            );
            process::exit(1);
        }
        for fd in fds {
            libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC);
            pty::set_nonblocking(fd);
        }
        SIGNAL_PIPE.store(fds[1], Ordering::Relaxed);
        for signal in [
            libc::SIGWINCH,
            libc::SIGTSTP,
            libc::SIGINT,
            libc::SIGHUP,
            libc::SIGTERM,
        ] {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = on_signal as *const () as libc::sighandler_t;
            action.sa_flags = libc::SA_RESTART;
            libc::sigaction(signal, &action, std::ptr::null_mut());
        }
    }
    fds[0]
}

fn drain(fd: i32) {
    let mut buffer = [0u8; 64];
    while unsafe { libc::read(fd, buffer.as_mut_ptr() as *mut libc::c_void, buffer.len()) } > 0 {}
}

fn poll_fd(fd: i32) -> libc::pollfd {
    libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    }
}
