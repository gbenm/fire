//! End-to-end tests for explicit runners (#21): runners run exactly as written, `{fire.exec}`
//! gets the commands as one quoted argument, otherwise the commands go to stdin, with the same
//! behavior in a terminal and without one. Runtime runners get `FIRE_RUNTIME_RPC_PORT`, run
//! through `shell`, and their bootstraps say why they can't connect.

#![cfg(unix)]

mod common;

use common::*;
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command, Stdio},
    sync::OnceLock,
};

const CONFIG: &str = r#"runtimes:
  py:
    sdk: python
    runner: python3
  py_port:
    sdk: python
    runner: env FIRE_RUNTIME_RPC_ADDR=127.0.0.1:$FIRE_RUNTIME_RPC_PORT python3
  py_placeholder:
    sdk: python
    runner: python3 {fire.exec}
  py_missing:
    sdk: python
    runner: env -u FIRE_RUNTIME_RPC_ADDR python3
  py_refused:
    sdk: python
    runner: env FIRE_RUNTIME_RPC_ADDR=127.0.0.1:1 python3
  node_missing:
    sdk: node
    runner: env -u FIRE_RUNTIME_RPC_ADDR node
  node_refused:
    sdk: node
    runner: env FIRE_RUNTIME_RPC_ADDR=127.0.0.1:1 node
  deno_missing:
    sdk: deno
    runner: env -u FIRE_RUNTIME_RPC_ADDR deno
  deno_refused:
    sdk: deno
    runner: env FIRE_RUNTIME_RPC_ADDR=127.0.0.1:1 deno
  py_shell:
    sdk: python
    runner: python3
    check: "true"
  deno_docker:
    sdk: deno
    runner: docker run --rm -i -e FIRE_RUNTIME_RPC_ADDR=host.docker.internal:$FIRE_RUNTIME_RPC_PORT denoland/deno:latest
commands:
  as_written:
    runner: ./args.sh -it --tty
    exec: echo hi
  compose:
    runner: docker compose exec app sh
    exec: echo hi
  placeholder:
    runner: sh -c {fire.exec}
    exec:
      - echo "first"
      - echo "last"
  stdin_free:
    runner: sh -c {fire.exec}
    exec: read answer; echo "got $answer"
  piped:
    runner: sh
    # the `[fire] cmd` log line shows `stdin_$mode`, never the result
    exec: 'mode=tty; [ -t 0 ] || mode=commands; echo "stdin_$mode"'
  piped_stops:
    runner: sh
    exec:
      - "false"
      - echo "not reached"
  single_quoted:
    runner: sh -c '{fire.exec}'
    exec: echo hi
  double_quoted:
    runner: sh -c "{fire.exec}"
    exec: echo hi
  port_eval:
    eval: 'py_port:"hi " + "x"'
  port_compute:
    compute:
      "{v}": 'py_port:"hi " + "x"'
    exec: echo "{v}"
  runtime_placeholder:
    eval: 'py_placeholder:1'
  py_missing:
    eval: 'py_missing:1'
  py_refused:
    eval: 'py_refused:1'
  node_missing:
    eval: 'node_missing:1'
  node_refused:
    eval: 'node_refused:1'
  deno_missing:
    eval: 'deno_missing:1'
  deno_refused:
    eval: 'deno_refused:1'
  runtime_shell:
    shell: ./logshell.sh
    compute:
      "{v}": 'py_shell:"via " + "shell"'
    exec: echo "{v}"
  docker_eval:
    eval: 'deno_docker:"hi " + "x"'
  docker_compute:
    compute:
      "{v}": 'deno_docker:"hi " + "x"'
    exec: echo "{v}"
"#;

fn project(name: &str) -> Project {
    let project = Project::new(&format!("runners-{name}"), CONFIG);
    write_script(
        &project,
        "args.sh",
        "#!/bin/sh\necho \"args: $*\"\nexec sh\n",
    );
    write_script(
        &project,
        "logshell.sh",
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$(dirname \"$0\")/shell.log\"\nexec sh \"$@\"\n",
    );
    project
}

fn write_script(project: &Project, name: &str, body: &str) {
    let path = project.dir.join(name);
    fs::write(&path, body).expect("write script");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod script");
}

/// Runs `fire <args>` without a terminal; returns (exit code, stdout, stderr).
fn run(project: &Project, args: &[&str]) -> (i32, String, String) {
    let mut fire = project.fire(args);
    fire.env("PATH", interpreters_path());
    run_with(fire)
}

/// `PATH` with the real directories of python3, node and deno first: version-manager shims
/// (asdf, pyenv, …) read their config from HOME, which the tests point elsewhere.
fn interpreters_path() -> &'static str {
    static PATH: OnceLock<String> = OnceLock::new();
    PATH.get_or_init(|| {
        let probes = [
            ("python3", "-c", "import sys; print(sys.executable)"),
            ("node", "-e", "console.log(process.execPath)"),
            ("deno", "eval", "console.log(Deno.execPath())"),
        ];
        let mut dirs = probes
            .iter()
            .filter_map(|(program, flag, code)| {
                let output = Command::new(program).args([flag, code]).output().ok()?;
                let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let dir = Path::new(&path).parent()?.to_str()?.to_string();
                output.status.success().then_some(dir)
            })
            .collect::<Vec<_>>();
        dirs.push(std::env::var("PATH").unwrap_or_default());
        dirs.join(":")
    })
}

fn run_with(mut command: Command) -> (i32, String, String) {
    let output = command.stdin(Stdio::null()).output().expect("run fire");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

/// Runs `fire <args>` in a pseudo-terminal; returns (exit code, everything on the terminal).
fn run_in_pty(project: &Project, args: &[&str]) -> (i32, String) {
    let (mut child, _master, output) = spawn_in_pty(project.fire(args));
    let code = wait_for_exit(&mut child);
    // the reader thread may still be draining the last bytes
    std::thread::sleep(std::time::Duration::from_millis(100));
    let output = output.lock().unwrap().replace('\r', "");
    (code, output)
}

#[test]
fn runner_flags_are_passed_as_written() {
    let project = project("as-written");
    let (code, stdout, stderr) = run(&project, &["as_written"]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(stdout, "args: -it --tty\nhi\n");
    assert!(
        stderr.contains("[fire] runner   ./args.sh -it --tty\n"),
        "{stderr}"
    );
}

#[test]
fn dry_run_shows_the_runner_as_written() {
    let project = project("dry-run");
    for (command, runner) in [
        ("as_written", "./args.sh -it --tty"),
        ("compose", "docker compose exec app sh"),
        ("placeholder", "sh -c {fire.exec}"),
    ] {
        let mut fire = project.fire(&[command]);
        fire.env("FIRE_DRY_RUN", "true");
        let (code, stdout, stderr) = run_with(fire);
        assert_eq!(code, 0, "stderr: {stderr}");
        assert_eq!(stdout, "");
        assert!(
            stderr.contains(&format!("[fire] runner   {runner}\n")),
            "{command}: {stderr}"
        );
    }
}

#[test]
fn fire_exec_gets_all_commands_and_args_quoted() {
    let project = project("placeholder");
    let (code, stdout, stderr) = run(&project, &["placeholder", "a b", "it's"]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(stdout, "first\nlast a b it's\n");
}

#[test]
fn fire_exec_behaves_the_same_in_a_terminal() {
    let project = project("placeholder-tty");
    let (code, output) = run_in_pty(&project, &["placeholder", "a b"]);
    assert_eq!(code, 0, "terminal: {output}");
    assert!(output.contains("first\nlast a b\n"), "terminal: {output}");
}

#[test]
fn fire_exec_leaves_stdin_to_the_user() {
    let project = project("stdin-free");
    let (mut child, mut master, output) = spawn_in_pty(project.fire(&["stdin_free"]));
    wait_for_output(&output, "[fire] cmd");
    master.write_all(b"Ben\r").expect("answer");
    assert_eq!(wait_for_exit(&mut child), 0);
    wait_for_output(&output, "got Ben");
}

#[test]
fn without_fire_exec_commands_go_to_stdin_even_in_a_terminal() {
    let project = project("piped");
    let (code, output) = run_in_pty(&project, &["piped"]);
    assert_eq!(code, 0, "terminal: {output}");
    assert!(output.contains("stdin_commands\n"), "terminal: {output}");

    let (code, stdout, stderr) = run(&project, &["piped"]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(stdout, "stdin_commands\n");
}

#[test]
fn piped_runner_stops_at_the_first_failure() {
    let project = project("piped-stops");
    let (code, stdout, _) = run(&project, &["piped_stops"]);
    assert_eq!(code, 1);
    assert_eq!(stdout, "");
}

#[test]
fn quoted_fire_exec_is_an_error() {
    let project = project("quoted");
    for (command, runner) in [
        ("single_quoted", "sh -c '{fire.exec}'"),
        ("double_quoted", "sh -c \"{fire.exec}\""),
    ] {
        let (code, stdout, stderr) = run(&project, &[command]);
        assert_eq!(code, 1, "{command}: {stderr}");
        assert_eq!(stdout, "");
        assert!(
            stderr.contains(&format!(
                "`{{fire.exec}}` in runner `{runner}` is already quoted: remove the surrounding quotes"
            )),
            "{command}: {stderr}"
        );
    }
}

fn skip_without(program: &str) -> bool {
    if has_program(program) {
        return false;
    }
    eprintln!("skipping: {program} not installed");
    true
}

#[test]
fn runtime_runner_can_build_its_address_from_the_port() {
    if skip_without("python3") {
        return;
    }
    let project = project("port");
    for command in ["port_eval", "port_compute"] {
        let (code, stdout, stderr) = run(&project, &[command]);
        assert_eq!(code, 0, "{command}: {stderr}");
        assert_eq!(stdout, "hi x\n", "{command}: {stderr}");
    }
}

#[test]
fn fire_exec_in_a_runtime_runner_is_an_error() {
    let project = project("runtime-placeholder");
    let (code, _, stderr) = run(&project, &["runtime_placeholder"]);
    assert_eq!(code, 1);
    assert!(
        stderr.contains("Runtime `py_placeholder`: `{fire.exec}` only works in command runners"),
        "{stderr}"
    );
}

fn assert_bootstrap_error(project: &Project, command: &str, message: &str) {
    let (code, _, stderr) = run(project, &[command]);
    assert_eq!(code, 1, "{command}: {stderr}");
    let reason = stderr
        .find(message)
        .unwrap_or_else(|| panic!("{command}: no {message:?} in {stderr}"));
    let exited = stderr
        .find("[fire] Runtime runner exited before connecting")
        .unwrap_or_else(|| panic!("{command}: {stderr}"));
    assert!(reason < exited, "{command}: {stderr}");
}

#[test]
fn runtime_bootstraps_say_why_they_cannot_connect() {
    let project = project("bootstrap");
    for (program, prefix) in [("python3", "py"), ("node", "node"), ("deno", "deno")] {
        if skip_without(program) {
            continue;
        }
        assert_bootstrap_error(
            &project,
            &format!("{prefix}_missing"),
            "Missing FIRE_RUNTIME_RPC_ADDR",
        );
        assert_bootstrap_error(
            &project,
            &format!("{prefix}_refused"),
            "Cannot connect to fire at 127.0.0.1:1",
        );
    }
}

#[test]
fn runtime_runner_and_check_use_the_configured_shell() {
    if skip_without("python3") {
        return;
    }
    let project = project("runtime-shell");
    let (code, stdout, stderr) = run(&project, &["runtime_shell"]);
    assert_eq!(code, 0, "stderr: {stderr}");
    assert_eq!(stdout, "via shell\n");
    let log = fs::read_to_string(project.dir.join("shell.log")).unwrap_or_default();
    assert!(
        log.contains("-c true\n"),
        "check not run through shell: {log}"
    );
    assert!(
        log.contains("-c python3 -u -c "),
        "runner not run through shell: {log}"
    );
}

/// Hand-written container runner on Docker Desktop, where `host.docker.internal` reaches fire's
/// listener on the host's 127.0.0.1. Linux without Docker Desktop is tracked in #44.
#[test]
#[cfg(target_os = "macos")]
fn docker_runtime_runner_connects_with_the_port() {
    let docker_ready = Command::new("docker")
        .arg("info")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !docker_ready {
        eprintln!("skipping: docker is not running");
        return;
    }
    // pull up front: fire waits 5s for the runtime to connect
    let pulled = Command::new("docker")
        .args(["pull", "-q", "denoland/deno:latest"])
        .stdout(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    assert!(pulled, "docker pull denoland/deno:latest failed");

    let project = project("docker");
    for command in ["docker_eval", "docker_compute"] {
        let (code, stdout, stderr) = run(&project, &[command]);
        assert_eq!(code, 0, "{command}: {stderr}");
        assert_eq!(stdout, "hi x\n", "{command}: {stderr}");
    }
}
