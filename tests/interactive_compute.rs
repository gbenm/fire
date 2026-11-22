//! End-to-end tests for interactive shell `compute` (#23): the real `fire` binary run inside a
//! pseudo-terminal (like a regular terminal or a `fire cli bg` / MCP session) and without one.

#![cfg(unix)]

use std::{
    ffi::CStr,
    fs,
    io::{Read, Write},
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

const TIMEOUT: Duration = Duration::from_secs(10);

const CONFIG: &str = r#"commands:
  ask:
    compute:
      "{name}": 'printf "Name: " >&2; read tmp; echo "$tmp"'
    exec: echo "Hello {name}"
  zask:
    shell: zsh
    compute:
      "{name}": 'vared -cp "Name: " tmp && echo $tmp'
    exec: echo "Hello {name}"
  fail:
    compute:
      "{x}": 'echo boom >&2; exit 3'
    exec: echo "{x}"
"#;

/// A temp project with the test config; HOME points inside it so the user's global config
/// and sessions stay out of the test.
struct Project {
    dir: PathBuf,
    /// Session started by the test, killed on drop so a failing test leaves nothing running.
    session: Option<String>,
}

impl Project {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("fire-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("home")).expect("create project dir");
        fs::write(dir.join("fire.yml"), CONFIG).expect("write fire.yml");
        Project { dir, session: None }
    }

    fn fire(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_fire"));
        command
            .args(args)
            .current_dir(&self.dir)
            .env("HOME", self.dir.join("home"))
            .env("XDG_CONFIG_HOME", self.dir.join("home/.config"))
            .env("XDG_DATA_HOME", self.dir.join("home/.local/share"))
            .env("NO_COLOR", "1")
            .env_remove("FIRE_DRY_RUN")
            .env_remove("FIRE_LOG_COMMANDS")
            .env_remove("FIRE_SESSION")
            .env_remove("FIRE_SESSIONS")
            .env_remove("FIRE_SESSION_NOTE");
        command
    }

    /// Runs a `fire cli ...` command to completion and returns (exit code, stdout, stderr).
    fn cli(&self, args: &[&str]) -> (i32, String, String) {
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

struct Pty {
    master: fs::File,
    slave: OwnedFd,
}

fn open_pty() -> Pty {
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
fn spawn_in_pty(mut command: Command) -> (Child, fs::File, Arc<Mutex<String>>) {
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

fn wait_for_output(output: &Mutex<String>, needle: &str) {
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

fn wait_for_exit(child: &mut Child) -> i32 {
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

fn answer_prompt_in_pty(project: &Project, command: &str) {
    let (mut child, mut master, output) = spawn_in_pty(project.fire(&[command]));

    wait_for_output(&output, "Name: ");
    // still waiting for the answer, not finished with an empty value
    thread::sleep(Duration::from_millis(200));
    assert!(
        child.try_wait().expect("wait").is_none(),
        "fire exited before the prompt was answered; terminal: {:?}",
        output.lock().unwrap()
    );

    master.write_all(b"Ben\r").expect("answer prompt");
    assert_eq!(wait_for_exit(&mut child), 0);
    wait_for_output(&output, "Hello Ben");
}

fn has_program(program: &str) -> bool {
    Command::new("sh")
        .args(["-c", &format!("command -v {program}")])
        .stdout(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn run_without_terminal(project: &Project, stdin: Stdio) -> (i32, String) {
    let mut child = project
        .fire(&["ask"])
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn fire");
    let code = wait_for_exit(&mut child);
    let mut stdout = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    (code, stdout)
}

#[test]
fn shell_compute_prompts_and_reads_from_a_terminal() {
    let project = Project::new("tty");
    answer_prompt_in_pty(&project, "ask");
}

#[test]
fn zsh_vared_compute_reads_from_a_terminal() {
    if !has_program("zsh") {
        eprintln!("skipping: zsh not installed");
        return;
    }
    let project = Project::new("zsh");
    answer_prompt_in_pty(&project, "zask");
}

#[test]
fn shell_compute_does_not_block_without_a_terminal() {
    let project = Project::new("null");
    let (code, stdout) = run_without_terminal(&project, Stdio::null());
    assert_eq!(code, 0);
    assert_eq!(stdout, "Hello \n");
}

#[test]
fn shell_compute_ignores_piped_stdin() {
    // stdin is only handed to compute when it is a terminal; a pipe stays closed
    let project = Project::new("pipe");
    let mut child = project
        .fire(&["ask"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn fire");
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(b"Ben\n").unwrap();
    // keep the pipe open: a compute reading from it would hang until the timeout
    let code = wait_for_exit(&mut child);
    drop(stdin);
    let mut stdout = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut stdout)
        .unwrap();
    assert_eq!(code, 0);
    assert_eq!(stdout, "Hello \n");
}

/// Polls `check` until it returns true, failing with `describe()` after the timeout.
fn eventually(mut check: impl FnMut() -> bool, describe: impl Fn() -> String) {
    let start = Instant::now();
    while !check() {
        assert!(start.elapsed() < TIMEOUT, "timed out: {}", describe());
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn shell_compute_prompt_in_a_session_is_answered_with_send() {
    let mut project = Project::new("session");

    let (code, _, stderr) = project.cli(&["bg", "-m", "test #23", "--new", "ask"]);
    assert_eq!(code, 0, "fire cli bg failed: {stderr}");
    let id = stderr
        .split("as session ")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("no session id in: {stderr}"))
        .to_string();
    project.session = Some(id.clone());

    let logs = |project: &Project| project.cli(&["logs", &id]).1;
    eventually(
        || logs(&project).contains("Name: "),
        || format!("prompt in session output: {:?}", logs(&project)),
    );
    let (_, ps, _) = project.cli(&["ps", "-a"]);
    let row = ps.lines().find(|line| line.starts_with(&id)).unwrap_or("");
    assert!(
        row.contains("running"),
        "session should wait for input: {ps}"
    );

    let (code, _, stderr) = project.cli(&["send", &id, "Ben"]);
    assert_eq!(code, 0, "fire cli send failed: {stderr}");
    eventually(
        || logs(&project).contains("Hello Ben"),
        || format!("answer in session output: {:?}", logs(&project)),
    );
    eventually(
        || {
            let (_, ps, _) = project.cli(&["ps", "-a"]);
            ps.lines()
                .any(|line| line.starts_with(&id) && line.contains("exited(0)"))
        },
        || format!("session exited(0): {:?}", project.cli(&["ps", "-a"]).1),
    );
    project.session = None;
}

#[test]
fn failed_shell_compute_shows_its_stderr_then_the_error() {
    let project = Project::new("fail");
    let output = project
        .fire(&["fail"])
        .stdin(Stdio::null())
        .output()
        .expect("run fire");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "stderr: {stderr}");
    let error = "Compute command `echo boom >&2; exit 3` failed with exit code 3";
    let boom = stderr.find("boom\n").expect("compute stderr shown");
    let reported = stderr.find(error).expect("error reported");
    assert!(
        boom < reported,
        "stderr should come before the error: {stderr}"
    );
    assert!(
        !stderr.contains("exit code 3: boom"),
        "stderr repeated in the error: {stderr}"
    );
}
