//! Pseudo-terminal and termios helpers, on plain `libc` (`posix_openpt`, no `libutil`).

use std::{
    ffi::{CStr, CString},
    io,
    os::fd::{FromRawFd, OwnedFd, RawFd},
};

#[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
const VDISABLE: libc::cc_t = 0xff;
#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "freebsd")))]
const VDISABLE: libc::cc_t = 0;

pub(crate) const DEFAULT_ROWS: u16 = 40;
pub(crate) const DEFAULT_COLS: u16 = 120;

pub(crate) struct Pty {
    pub(crate) master: OwnedFd,
    pub(crate) slave_path: CString,
}

pub(crate) fn open() -> io::Result<Pty> {
    unsafe {
        let fd = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let master = OwnedFd::from_raw_fd(fd);
        if libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) < 0
            || libc::grantpt(fd) != 0
            || libc::unlockpt(fd) != 0
        {
            return Err(io::Error::last_os_error());
        }
        let name = libc::ptsname(fd);
        if name.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(Pty {
            master,
            slave_path: CStr::from_ptr(name).to_owned(),
        })
    }
}

/// Runs in the forked child before exec: become a session leader whose controlling terminal is
/// the pty slave, with the slave on stdin/stdout/stderr. Only async-signal-safe calls.
pub(crate) unsafe fn become_session_leader_on(slave_path: &CStr) -> io::Result<()> {
    if libc::setsid() < 0 {
        return Err(io::Error::last_os_error());
    }
    let fd = libc::open(slave_path.as_ptr(), libc::O_RDWR);
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // Linux already made it the controlling tty on open; BSD/macOS need the ioctl.
    libc::ioctl(fd, libc::TIOCSCTTY as _, 0);
    for target in 0..3 {
        if libc::dup2(fd, target) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    if fd > 2 {
        libc::close(fd);
    }
    Ok(())
}

pub(crate) fn set_nonblocking(fd: RawFd) {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
    }
}

pub(crate) fn set_size(fd: RawFd, rows: u16, cols: u16) {
    let size = libc::winsize {
        ws_row: rows.max(1),
        ws_col: cols.max(1),
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    unsafe {
        libc::ioctl(fd, libc::TIOCSWINSZ as _, &size);
    }
}

pub(crate) fn get_size(fd: RawFd) -> Option<(u16, u16)> {
    let mut size: libc::winsize = unsafe { std::mem::zeroed() };
    let ok = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ as _, &mut size) } == 0;
    (ok && size.ws_row > 0 && size.ws_col > 0).then_some((size.ws_row, size.ws_col))
}

/// Size of the terminal this process runs in, if any.
pub(crate) fn terminal_size() -> Option<(u16, u16)> {
    [1, 2, 0].into_iter().find_map(get_size)
}

/// Whether this process's group owns the terminal on `fd` (shell `fg` vs `bg`).
pub(crate) fn is_foreground(fd: RawFd) -> bool {
    unsafe { libc::tcgetpgrp(fd) == libc::getpgrp() }
}

/// Raw mode for the attaching terminal, except that Ctrl+Z still raises SIGTSTP locally (so the
/// shell's job control detaches the viewer). Ctrl+C and Ctrl+\ arrive as bytes and go to the pty,
/// whose line discipline turns them into signals for the program.
pub(crate) fn enter_raw(fd: RawFd) -> io::Result<libc::termios> {
    unsafe {
        let mut original: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(fd, &mut original) != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut raw = original;
        libc::cfmakeraw(&mut raw);
        raw.c_lflag |= libc::ISIG;
        raw.c_cc[libc::VINTR] = VDISABLE;
        raw.c_cc[libc::VQUIT] = VDISABLE;
        #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
        {
            raw.c_cc[libc::VDSUSP] = VDISABLE;
        }
        raw.c_cc[libc::VMIN] = 1;
        raw.c_cc[libc::VTIME] = 0;
        if libc::tcsetattr(fd, libc::TCSANOW, &raw) != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(original)
    }
}

pub(crate) fn restore(fd: RawFd, original: &libc::termios) {
    unsafe {
        libc::tcsetattr(fd, libc::TCSANOW, original);
    }
}
