//! End-to-end tests for interactive shell `compute` (#23): the real `fire` binary run inside a
//! pseudo-terminal (like a regular terminal or a `fire --bg` / MCP session) and without one.

#![cfg(unix)]

mod common;

use common::*;
use std::{
    io::{Read, Write},
    process::Stdio,
    thread,
    time::Duration,
};

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
    let project = Project::new("tty", CONFIG);
    answer_prompt_in_pty(&project, "ask");
}

#[test]
fn zsh_vared_compute_reads_from_a_terminal() {
    if !has_program("zsh") {
        eprintln!("skipping: zsh not installed");
        return;
    }
    let project = Project::new("zsh", CONFIG);
    answer_prompt_in_pty(&project, "zask");
}

#[test]
fn shell_compute_does_not_block_without_a_terminal() {
    let project = Project::new("null", CONFIG);
    let (code, stdout) = run_without_terminal(&project, Stdio::null());
    assert_eq!(code, 0);
    assert_eq!(stdout, "Hello \n");
}

#[test]
fn shell_compute_ignores_piped_stdin() {
    // stdin is only handed to compute when it is a terminal; a pipe stays closed
    let project = Project::new("pipe", CONFIG);
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

#[test]
fn shell_compute_prompt_in_a_session_is_answered_with_send() {
    let mut project = Project::new("session", CONFIG);

    let output = project
        .fire(&["--bg", "-m", "test #23", "--new", "ask"])
        .stdin(Stdio::null())
        .output()
        .expect("run fire --bg");
    let code = output.status.code().unwrap_or(-1);
    let stderr = String::from_utf8_lossy(&output.stderr).replace('\r', "");
    assert_eq!(code, 0, "fire --bg failed: {stderr}");
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
    let project = Project::new("fail", CONFIG);
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
