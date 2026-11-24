//! Global flags before the command (`fire --bg -m "why" api dev`, `--fg`, `--dry-run`,
//! `--no-log`), run against the real binary without a terminal.

mod common;

use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
};

use common::{eventually, Project};

fn config() -> String {
    format!(
        r#"
commands:
  hello:
    exec: echo hello
  server:
    long_running: true
    exec: echo serving && sleep 30
  quick:
    long_running: true
    exec: echo serving
  outer:
    exec: '"{fire}" hello'
  human:
    agents: false
    exec: echo human
"#,
        fire = env!("CARGO_BIN_EXE_fire")
    )
}

fn run(command: Command) -> (i32, String, String) {
    let mut command = command;
    let output = command.stdin(Stdio::null()).output().expect("run fire");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).replace('\r', ""),
        String::from_utf8_lossy(&output.stderr).replace('\r', ""),
    )
}

fn fire(project: &Project, args: &[&str]) -> (i32, String, String) {
    run(project.fire(args))
}

fn session_record(root: &Path, id: &str) -> Option<String> {
    for entry in fs::read_dir(root).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().and_then(|name| name.to_str()) == Some(id) {
                if let Ok(record) = fs::read_to_string(path.join("session.json")) {
                    return Some(record);
                }
            }
            if let Some(record) = session_record(&path, id) {
                return Some(record);
            }
        }
    }
    None
}

fn session_row(project: &Project, id: &str) -> String {
    let (_, ps, _) = project.cli(&["ps", "-a"]);
    ps.lines()
        .find(|line| line.starts_with(id))
        .unwrap_or("")
        .to_string()
}

#[test]
fn bg_starts_a_session_reuses_it_and_new_starts_another() {
    let mut project = Project::new("flags-bg", &config());

    let (code, id, stderr) = fire(&project, &["--bg", "-m", "flag note", "hello", "--bg"]);
    assert_eq!(code, 0, "{stderr}");
    let id = id.trim().to_string();
    assert!(!id.is_empty(), "no session id: {stderr}");
    project.session = Some(id.clone());
    // `--bg` after the command word is the command's own argument
    eventually(
        || project.cli(&["logs", &id]).1.contains("hello --bg"),
        || format!("output: {:?}", project.cli(&["logs", &id]).1),
    );
    assert!(session_row(&project, &id).contains("flag note"));

    let (code, server, stderr) = fire(&project, &["--note", "srv", "--no-log", "--bg", "server"]);
    assert_eq!(code, 0, "{stderr}");
    let server = server.trim().to_string();
    let (code, again, stderr) = fire(&project, &["--bg", "server"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(again.trim(), server, "running session is reused: {stderr}");
    let (code, other, stderr) = fire(&project, &["--bg", "--new", "server"]);
    assert_eq!(code, 0, "{stderr}");
    let other = other.trim().to_string();
    assert_ne!(other, server, "--new starts another session");
    for id in [&server, &other] {
        project.cli(&["kill", id, "KILL"]);
    }
}

#[test]
fn bg_wins_over_sessions_off_and_fg_runs_attached() {
    let mut project = Project::new("flags-fg", &config());

    // without a terminal a long-running command starts detached...
    let (code, _, stderr) = fire(&project, &["quick"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stderr.contains("started it detached"), "{stderr}");

    // ...`--fg` runs it here instead (FIRE_SESSIONS=off), and `--no-log` beats the env
    let mut command = project.fire(&["--fg", "--no-log", "quick"]);
    command.env("FIRE_LOG_COMMANDS", "true");
    let (code, stdout, stderr) = run(command);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "serving\n");
    assert_eq!(stderr, "");

    // `--bg` starts a session even with FIRE_SESSIONS=off
    let mut command = project.fire(&["--bg", "hello"]);
    command.env("FIRE_SESSIONS", "off");
    let (code, id, stderr) = run(command);
    assert_eq!(code, 0, "{stderr}");
    project.session = Some(id.trim().to_string());
    assert!(stderr.contains("as session"), "{stderr}");
}

#[test]
fn dry_run_and_no_log_reach_nested_fire_calls() {
    let project = Project::new("flags-nested", &config());

    let (code, stdout, stderr) = fire(&project, &["--dry-run", "hello"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "");
    assert!(stderr.contains("echo hello"), "{stderr}");

    // the outer command really runs; the nested `fire hello` inherits --no-log
    let (code, stdout, stderr) = fire(&project, &["--no-log", "outer"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "hello\n");
    assert_eq!(stderr, "");

    // --bg --dry-run prints what would run and starts nothing
    let (code, stdout, stderr) = fire(&project, &["--bg", "--dry-run", "server"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, "");
    assert!(stderr.contains("sleep 30"), "{stderr}");
    let (_, ps, _) = project.cli(&["ps", "-a"]);
    assert!(!ps.contains("server"), "no session started: {ps}");
}

#[test]
fn boolean_env_values_are_unified() {
    let project = Project::new("flags-env", &config());
    for value in ["1", "yes", "ON"] {
        let mut command = project.fire(&["hello"]);
        command.env("FIRE_DRY_RUN", value);
        let (code, stdout, stderr) = run(command);
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(stdout, "", "FIRE_DRY_RUN={value} is a dry run");
    }
    for value in ["0", "no", "OFF"] {
        let mut command = project.fire(&["hello"]);
        command.env("FIRE_LOG_COMMANDS", value);
        let (code, stdout, stderr) = run(command);
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(stdout, "hello\n");
        assert_eq!(stderr, "", "FIRE_LOG_COMMANDS={value} turns logging off");
    }
}

#[test]
fn bad_flag_combinations_fail_without_running_anything() {
    let project = Project::new("flags-errors", &config());
    for (args, needle) in [
        (&["--bg", "--fg", "hello"][..], "--bg and --fg"),
        (&["--fg", "-m", "x", "hello"][..], "needs a session"),
        (&["--fg", "--new", "hello"][..], "needs a session"),
        (&["--foo", "hello"][..], "Unknown flag `--foo`"),
        (&["--bg"][..], "flags need a command"),
        (&["--bg", "cli", "ps"][..], "not to `fire cli`"),
        (&["--bg", "-m"][..], "-m needs a note"),
    ] {
        let (code, stdout, stderr) = fire(&project, args);
        assert_eq!(code, 1, "{args:?}: {stderr}");
        assert_eq!(stdout, "", "{args:?}");
        assert!(stderr.contains(needle), "{args:?}: {stderr}");
    }
}

#[test]
fn agent_flag_refuses_commands_reserved_for_humans() {
    let project = Project::new("flags-agent", &config());
    let (code, _, stderr) = fire(&project, &["--bg", "--agent", "-m", "x", "human"]);
    assert_eq!(code, 1);
    assert!(stderr.contains("not available to agents"), "{stderr}");
}

#[test]
fn deprecated_cli_bg_still_works_with_a_hint() {
    let mut project = Project::new("flags-cli-bg", &config());
    let note = "  old note  ";
    let (code, id, stderr) = project.cli(&["bg", "-m", note, "hello"]);
    assert_eq!(code, 0, "{stderr}");
    let id = id.trim().to_string();
    let record = session_record(&project.dir.join("home"), &id).expect("session record");
    let record: serde_json::Value = serde_json::from_str(&record).expect("session JSON");
    assert_eq!(record["note"], note);
    project.session = Some(id);
    assert!(stderr.contains("`fire cli bg` is deprecated"), "{stderr}");
    assert!(stderr.contains("fire --bg"), "{stderr}");
}

#[test]
fn root_help_and_completion_list_the_flags() {
    let project = Project::new("flags-help", &config());
    let (_, help, _) = fire(&project, &[]);
    assert!(help.contains("Flags (before the command):"), "{help}");
    assert!(help.contains("--dry-run"), "{help}");
    assert!(!help.contains("--jobs"), "{help}");
    assert!(!help.contains("--agent"), "{help}");

    let (_, flags, _) = fire(&project, &["__complete", "fire", "--"]);
    assert!(flags.contains("--bg\t"), "{flags}");
    assert!(flags.contains("--no-log\t"), "{flags}");
    assert!(!flags.contains("--jobs"), "{flags}");
    let (_, after, _) = fire(&project, &["__complete", "fire", "--bg", "-m", "why", ""]);
    let (_, root, _) = fire(&project, &["__complete", "fire", ""]);
    let root: Vec<&str> = root
        .lines()
        .filter(|line| !line.starts_with("cli"))
        .collect();
    assert_eq!(after.lines().collect::<Vec<_>>(), root);
}
