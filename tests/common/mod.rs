//! Helpers shared by the end-to-end tests: a temp project and the real `fire` binary run
//! inside a pseudo-terminal or without one.

#![allow(dead_code)]

use std::{
    ffi::CStr,
    fs,
    io::Read,
    os::{
        fd::{FromRawFd, OwnedFd},
        unix::process::CommandExt,
    },
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

pub const TIMEOUT: Duration = Duration::from_secs(10);

/// A temp project with a `fire.yml`; HOME points inside it so the user's global config
/// and sessions stay out of the test.
pub struct Project {
    pub dir: PathBuf,
    /// Session started by the test, killed on drop so a failing test leaves nothing running.
    pub session: Option<String>,
}

impl Project {
    pub fn new(name: &str, config: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("fire-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("home")).expect("create project dir");
        fs::write(dir.join("fire.yml"), config).expect("write fire.yml");
        Project { dir, session: None }
    }

    pub fn fire(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fire"));
        command
            .args(args)
            .current_dir(&self.dir)
            .env("HOME", self.dir.join("home"))
            .env("XDG_CONFIG_HOME", self.dir.join("home/.config"))
            .env("XDG_DATA_HOME", self.dir.join("home/.local/share"))
            .env("NO_COLOR", "1")
            // Keeps the first-use "register this directory?" prompt out of terminal tests;
            // tests/global.rs removes it to exercise the prompt.
            .env("CI", "1")
            .env_remove("FIRE_DRY_RUN")
            .env_remove("FIRE_LOG_COMMANDS")
            .env_remove("FIRE_SESSION")
            .env_remove("FIRE_SESSIONS")
            .env_remove("FIRE_SESSION_NOTE");
        command.env_remove("FIRE_SKIP_NEEDS");
        command
    }

    /// Runs a `fire cli ...` command to completion and returns (exit code, stdout, stderr).
    pub fn cli(&self, args: &[&str]) -> (i32, String, String) {
        let mut full = vec!["cli"];
        full.extend_from_slice(args);
        let output = self
            .fire(&full)
            .stdin(Stdio::null())
            .output()
            .expect("run fire cli");
        (
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout).replace('\r', ""),
            String::from_utf8_lossy(&output.stderr).replace('\r', ""),
        )
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        if let Some(id) = self.session.take() {
            let _ = self.cli(&["kill", &id, "KILL"]);
        }
        let _ = fs::remove_dir_all(&self.dir);
    }
}

pub struct Pty {
    pub master: fs::File,
    pub slave: OwnedFd,
}

pub fn open_pty() -> Pty {
    unsafe {
        let fd = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY);
        assert!(fd >= 0, "posix_openpt failed");
        assert_eq!(libc::grantpt(fd), 0, "grantpt failed");
        assert_eq!(libc::unlockpt(fd), 0, "unlockpt failed");
        let name = libc::ptsname(fd);
        assert!(!name.is_null(), "ptsname failed");
        let slave = libc::open(CStr::from_ptr(name).as_ptr(), libc::O_RDWR | libc::O_NOCTTY);
        assert!(slave >= 0, "open slave failed");
        Pty {
            master: fs::File::from_raw_fd(fd),
            slave: OwnedFd::from_raw_fd(slave),
        }
    }
}

/// Spawns `command` with the pty slave as its controlling terminal and stdio, and collects
/// everything written to the terminal in the background.
pub fn spawn_in_pty(mut command: Command) -> (Child, fs::File, Arc<Mutex<String>>) {
    let pty = open_pty();
    let slave = pty.slave;
    command
        .stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::from(slave.try_clone().unwrap()))
        .stderr(Stdio::from(slave));
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command.spawn().expect("spawn fire in pty");
    drop(command); // closes our copies of the slave, so reads end when fire exits

    let output = Arc::new(Mutex::new(String::new()));
    let mut reader = pty.master.try_clone().expect("clone master");
    let sink = Arc::clone(&output);
    thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
            sink.lock()
                .unwrap()
                .push_str(&String::from_utf8_lossy(&buf[..n]));
        }
    });
    (child, pty.master, output)
}

pub fn wait_for_output(output: &Mutex<String>, needle: &str) {
    let start = Instant::now();
    while !output.lock().unwrap().contains(needle) {
        assert!(
            start.elapsed() < TIMEOUT,
            "timed out waiting for {needle:?}; terminal so far: {:?}",
            output.lock().unwrap()
        );
        thread::sleep(Duration::from_millis(20));
    }
}

pub fn wait_for_exit(child: &mut Child) -> i32 {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("wait") {
            return status.code().unwrap_or(-1);
        }
        if start.elapsed() > TIMEOUT {
            let _ = child.kill();
            panic!("fire did not exit within {TIMEOUT:?}");
        }
        thread::sleep(Duration::from_millis(20));
    }
}

pub fn has_program(program: &str) -> bool {
    Command::new("sh")
        .args(["-c", &format!("command -v {program}")])
        .stdout(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Polls `check` until it returns true, failing with `describe()` after the timeout.
pub fn eventually(mut check: impl FnMut() -> bool, describe: impl Fn() -> String) {
    let start = Instant::now();
    while !check() {
        assert!(start.elapsed() < TIMEOUT, "timed out: {}", describe());
        thread::sleep(Duration::from_millis(50));
    }
}

/// Where `fire` keeps the sessions of `project` (its HOME is inside the project).
pub fn sessions_root(project: &Project) -> PathBuf {
    let home = project.dir.join("home");
    let data = if cfg!(target_os = "macos") {
        home.join("Library/Application Support")
    } else {
        home.join(".local/share")
    };
    data.join("fire").join("sessions")
}

/// A running session as fire versions before session versioning left it: the pipe layout
/// (`stdout.log`, `stderr.log`, a `stdin` FIFO), no `ctl.sock`, no `protocol` or `fire_version`
/// in `session.json`. Its program is a `sleep` in a process group of its own (never the test
/// runner's) and not a child of the test, so it doesn't linger as a zombie once killed. Killed
/// on drop.
pub struct LegacySession {
    pub id: String,
    pub dir: PathBuf,
    /// The `sleep`, standing in for the old supervisor (its liveness is the session's).
    pub pid: u32,
    /// Its process group, which `fire cli kill` signals.
    pub group: u32,
}

impl LegacySession {
    pub fn start(project: &Project, id: &str, command: &[&str], stdout: &str) -> Self {
        // `sh` leads a new process group, leaves `sleep` running in it and exits.
        let output = Command::new("sh")
            .args(["-c", "sleep 30 >/dev/null 2>&1 & echo $$ $!"])
            .process_group(0)
            .stdin(Stdio::null())
            .output()
            .expect("spawn sleep");
        let pids: Vec<u32> = String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .map(|pid| pid.parse().expect("pid"))
            .collect();
        let [group, pid] = pids[..] else {
            panic!("unexpected output: {output:?}")
        };

        let dir = sessions_root(project).join(id);
        fs::create_dir_all(&dir).unwrap();
        let cwd = fs::canonicalize(&project.dir).unwrap();
        let record = serde_json::json!({
            "id": id,
            "command": command,
            "note": null,
            "cwd": cwd.display().to_string(),
            "started_at": 1,
            "supervisor_pid": pid,
            "pid": group,
            "exit_code": null,
            "ended_at": null,
        });
        fs::write(dir.join("session.json"), record.to_string()).unwrap();
        fs::write(dir.join("stdout.log"), stdout).unwrap();
        fs::write(dir.join("stderr.log"), "").unwrap();
        let fifo = Command::new("mkfifo")
            .arg(dir.join("stdin"))
            .status()
            .expect("mkfifo");
        assert!(fifo.success());
        LegacySession {
            id: id.to_string(),
            dir,
            pid,
            group,
        }
    }

    pub fn alive(&self) -> bool {
        Command::new("kill")
            .args(["-0", &self.pid.to_string()])
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }
}

impl Drop for LegacySession {
    fn drop(&mut self) {
        let _ = Command::new("kill")
            .args(["-KILL", "--", &format!("-{}", self.group)])
            .stderr(Stdio::null())
            .status();
    }
}
