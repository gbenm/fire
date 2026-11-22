//! End-to-end tests for the container object in `runner` / `fallback_runner` (#44). The object
//! only generates a string runner: these tests check that string (log and `--dry-run`), run it
//! through a fake `docker` that applies `-w` / `-e` and runs the command locally, and, when
//! Docker is available, through the real engine.

#![cfg(unix)]

mod common;

use common::*;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command, Stdio},
};

const CONFIG: &str = r#"runtimes:
  py:
    sdk: python
    paths: [lib/helpers.py]
    runner:
      image: python:3-alpine
      network_mode: host
      command: python3
  deno:
    sdk: deno
    runner:
      image: denoland/deno:latest
environment:
  DECLARED: from-fire
commands:
  minimal:
    runner:
      image: alpine
    exec: echo "dir=$(pwd) declared=$DECLARED host=${HOST_ONLY:-unset}"
  sub:
    dir: web
    runner:
      image: alpine
    exec: pwd
  placeholder:
    runner:
      image: node:lts-alpine
      command: sh -c {fire.exec}
    exec:
      - echo first
      - echo last
  secrets:
    env_file: .env
    runner:
      image: alpine
      environment:
        TOKEN: ${SECRET}-object
        DECLARED: object-wins
    exec: echo "secret=$SECRET token=$TOKEN declared=$DECLARED"
  fallback:
    check: "false"
    fallback_runner:
      image: alpine
    exec: echo "in fallback"
  tty_stdin:
    runner:
      image: alpine
      tty: true
    exec: echo hi
  networks:
    runner:
      image: alpine
      network: my-net
      network_mode: host
    exec: echo hi
  typo:
    runner:
      imagen: alpine
    exec: echo hi
  py_eval:
    eval: 'py:greet("x")'
  deno_eval:
    eval: 'deno:"hi " + "x"'
  docker_exec:
    runner:
      image: alpine
    exec: echo "in $(cat /etc/alpine-release | cut -c1) $(pwd)"
"#;

const FAKE_ENGINE: &str = r#"
# Fake `docker`: logs its args, applies -w and -e, runs the command (or `sh`) locally.
import json, os, sys
args = sys.argv[1:]
with open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "engine.log"), "a") as log:
    log.write(json.dumps(args) + "\n")
assert args[0] == "run", args
env = {"PATH": os.path.dirname(sys.executable) + ":" + os.environ["PATH"]}
cwd = None
i = 1
while i < len(args):
    a = args[i]
    if a in ("--rm", "-i", "-t") or a.startswith("--add-host="):
        i += 1
    elif a in ("-v", "--user", "--network"):
        i += 2
    elif a == "-w":
        cwd = args[i + 1]
        i += 2
    elif a == "-e":
        key, sep, value = args[i + 1].partition("=")
        if sep:
            env[key] = value
        elif key in os.environ:
            env[key] = os.environ[key]
        i += 2
    else:
        break
command = args[i + 1:] or ["sh"]
os.chdir(cwd)
os.execvpe(command[0], command, env)
"#;

fn project(name: &str) -> Project {
    let project = Project::new(&format!("container-{name}"), CONFIG);
    fs::create_dir_all(project.dir.join("web")).unwrap();
    fs::create_dir_all(project.dir.join("lib")).unwrap();
    fs::create_dir_all(project.dir.join("bin")).unwrap();
    fs::write(project.dir.join(".env"), "SECRET=s3cr3t\n").unwrap();
    fs::write(
        project.dir.join("lib/helpers.py"),
        "def greet(name):\n    return 'hello ' + name\n",
    )
    .unwrap();
    // the real interpreter: version-manager shims read their config from HOME, which the
    // tests point elsewhere
    let python = Command::new("python3")
        .args(["-c", "import sys; print(sys.executable)"])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_default();
    let engine = project.dir.join("bin/docker");
    fs::write(&engine, format!("#!{python}{FAKE_ENGINE}")).unwrap();
    fs::set_permissions(&engine, fs::Permissions::from_mode(0o755)).unwrap();
    project
}

/// `fire <args>` without a terminal and with the fake engine first in PATH.
fn run_fake(project: &Project, args: &[&str]) -> (i32, String, String) {
    let mut fire = project.fire(args);
    let path = format!(
        "{}:{}",
        project.dir.join("bin").display(),
        std::env::var("PATH").unwrap_or_default()
    );
    fire.env("PATH", path).env("HOST_ONLY", "leaked");
    output(fire)
}

fn output(mut command: Command) -> (i32, String, String) {
    let output = command.stdin(Stdio::null()).output().expect("run fire");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

fn runner_line(stderr: &str) -> &str {
    stderr
        .lines()
        .find(|line| line.starts_with("[fire] runner"))
        .unwrap_or_else(|| panic!("no runner line in {stderr}"))
}

fn real_dir(project: &Project) -> String {
    fs::canonicalize(&project.dir)
        .unwrap()
        .to_string_lossy()
        .to_string()
}

fn skip_without_python() -> bool {
    if has_program("python3") {
        return false;
    }
    eprintln!("skipping: python3 not installed");
    true
}

#[test]
fn dry_run_shows_the_generated_string() {
    let project = project("dry-run");
    let mut fire = project.fire(&["minimal"]);
    fire.env("FIRE_DRY_RUN", "true");
    let (code, stdout, stderr) = output(fire);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "");
    let line = runner_line(&stderr);
    assert!(
        line.starts_with("[fire] runner   docker run --rm -i -v "),
        "{line}"
    );
    assert!(line.ends_with(" -e DECLARED -e SECRET alpine"), "{line}");
    assert!(line.contains(" -w "), "{line}");
}

#[test]
fn generated_runner_runs_in_the_command_dir_with_declared_env_only() {
    if skip_without_python() {
        return;
    }
    let project = project("minimal");
    let (code, stdout, stderr) = run_fake(&project, &["minimal"]);
    assert_eq!(code, 0, "{stderr}");
    let dir = real_dir(&project);
    assert_eq!(
        stdout.replace("/private/var/", "/var/"),
        format!("dir={dir} declared=from-fire host=unset\n").replace("/private/var/", "/var/")
    );

    let (code, stdout, stderr) = run_fake(&project, &["sub"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.trim_end().ends_with("/web"), "{stdout}");
}

#[test]
fn running_the_logged_string_by_hand_behaves_the_same() {
    if skip_without_python() {
        return;
    }
    let project = project("by-hand");
    let (code, stdout, stderr) = run_fake(&project, &["minimal"]);
    assert_eq!(code, 0, "{stderr}");
    let runner = runner_line(&stderr)
        .trim_start_matches("[fire] runner")
        .trim()
        .to_string();
    let path = format!(
        "{}:{}",
        project.dir.join("bin").display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let by_hand = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "{runner} <<'EOF'\necho \"dir=$(pwd) declared=$DECLARED host=${{HOST_ONLY:-unset}}\"\nEOF"
        ))
        .env("PATH", path)
        .env("DECLARED", "from-fire")
        .env("HOST_ONLY", "leaked")
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&by_hand.stdout), stdout);
}

#[test]
fn command_with_fire_exec_gets_the_commands() {
    if skip_without_python() {
        return;
    }
    let project = project("placeholder");
    let (code, stdout, stderr) = run_fake(&project, &["placeholder", "a b"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "first\nlast a b\n");
    assert!(
        runner_line(&stderr).ends_with("node:lts-alpine sh -c {fire.exec}"),
        "{stderr}"
    );
}

#[test]
fn env_values_reach_the_container_but_never_the_log() {
    if skip_without_python() {
        return;
    }
    let project = project("secrets");
    let (code, stdout, stderr) = run_fake(&project, &["secrets"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(
        stdout,
        "secret=s3cr3t token=s3cr3t-object declared=object-wins\n"
    );
    let line = runner_line(&stderr);
    assert!(line.contains(" -e DECLARED -e SECRET -e TOKEN "), "{line}");
    assert!(!stderr.contains("s3cr3t"), "{stderr}");
    assert!(!stderr.contains("object-wins"), "{stderr}");
}

#[test]
fn fallback_runner_accepts_the_object() {
    if skip_without_python() {
        return;
    }
    let project = project("fallback");
    let (code, stdout, stderr) = run_fake(&project, &["fallback"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "in fallback\n");
}

#[test]
fn invalid_objects_fail_with_a_clear_message() {
    let project = project("invalid");
    for (command, message) in [
        ("tty_stdin", "`tty: true` needs `{fire.exec}` in `command`"),
        (
            "networks",
            "`network` and `network_mode` can't be used together",
        ),
        ("typo", "imagen"),
    ] {
        let (code, stdout, stderr) = run_fake(&project, &[command]);
        assert_ne!(code, 0, "{command}: {stderr}");
        assert_eq!(stdout, "", "{command}");
        assert!(stderr.contains(message), "{command}: {stderr}");
    }
}

#[test]
fn runtime_object_with_network_mode_host_connects_over_loopback() {
    if skip_without_python() {
        return;
    }
    let project = project("runtime");
    let (code, stdout, stderr) = run_fake(&project, &["py_eval"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "hello x\n");
    let log = fs::read_to_string(project.dir.join("bin/engine.log")).unwrap();
    assert!(log.contains("\"--network\", \"host\""), "{log}");
    assert!(
        log.contains("FIRE_RUNTIME_RPC_ADDR=127.0.0.1:"),
        "port not expanded by the shell: {log}"
    );
}

fn docker_ready(image: &str) -> bool {
    let running = Command::new("docker")
        .arg("info")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !running {
        eprintln!("skipping: docker is not running");
        return false;
    }
    // pull up front: fire waits 5s for a runtime to connect
    let pulled = Command::new("docker")
        .args(["pull", "-q", image])
        .stdout(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    assert!(pulled, "docker pull {image} failed");
    true
}

/// Real engine: the project is mounted at the same path and `exec` runs in it.
#[test]
fn real_docker_runs_exec_in_the_project() {
    if !docker_ready("alpine") {
        return;
    }
    let project = project("docker-exec");
    let mut fire = project.fire(&["docker_exec"]);
    fire.env_remove("HOST_ONLY");
    let (code, stdout, stderr) = output(fire);
    assert_eq!(code, 0, "{stderr}");
    let dir = project.dir.to_string_lossy().to_string();
    assert!(stdout.starts_with("in 3 "), "{stdout}");
    assert!(
        Path::new(stdout.trim_end().trim_start_matches("in 3 "))
            .ends_with(Path::new(&dir).file_name().unwrap()),
        "{stdout}"
    );
}

/// Real engine on Docker Desktop: the default host alias reaches fire's loopback listener.
#[test]
#[cfg(target_os = "macos")]
fn real_docker_runtime_object_connects_with_the_default_alias() {
    if !docker_ready("denoland/deno:latest") {
        return;
    }
    let project = project("docker-runtime");
    let (code, stdout, stderr) = output(project.fire(&["deno_eval"]));
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "hi x\n");
}
