//! `fire <command>` in a terminal when the same command is already running in this directory:
//! it attaches to that session (or reports one it can't attach) instead of starting a duplicate.

mod common;

use std::{
    fs,
    os::unix::process::CommandExt,
    path::PathBuf,
    process::{Child, Command, Stdio},
};

use common::{eventually, spawn_in_pty, wait_for_exit, wait_for_output, Project};

const CONFIG: &str = r#"
commands:
  server:
    exec: echo serving && sleep 30
"#;

/// Ids of every session listed by `fire cli ps -a`.
fn session_ids(project: &Project) -> Vec<String> {
    let (_, ps, _) = project.cli(&["ps", "-a"]);
    ps.lines()
        .skip(1)
        .filter_map(|line| line.split_whitespace().next())
        .filter(|id| id.starts_with("server-"))
        .map(str::to_string)
        .collect()
}

/// Kills every session of the project on drop, so a failing test leaves nothing running.
struct Cleanup<'a> {
    project: &'a Project,
    extra: Option<Child>,
}

impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        for id in session_ids(self.project) {
            let _ = self.project.cli(&["kill", &id, "KILL"]);
        }
        if let Some(child) = self.extra.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[test]
fn terminal_run_attaches_to_the_running_session() {
    let project = Project::new("reuse-attach", CONFIG);
    let _cleanup = Cleanup {
        project: &project,
        extra: None,
    };

    let output = project
        .fire(&["--bg", "server"])
        .stdin(Stdio::null())
        .output()
        .expect("run fire --bg");
    assert!(output.status.success());
    let id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    assert!(!id.is_empty());
    eventually(
        || project.cli(&["logs", &id]).1.contains("serving"),
        || project.cli(&["logs", &id]).1,
    );

    let (mut child, _master, terminal) = spawn_in_pty(project.fire(&["server"]));
    wait_for_output(&terminal, &format!("already running here as session {id}"));
    wait_for_output(&terminal, "serving");
    assert_eq!(session_ids(&project), vec![id.clone()], "no second session");

    // The viewer follows the existing session: stopping it ends the viewer.
    project.cli(&["kill", &id, "KILL"]);
    wait_for_exit(&mut child);
    assert_eq!(session_ids(&project), vec![id], "no second session");
}

fn sessions_root(project: &Project) -> PathBuf {
    let home = project.dir.join("home");
    let data = if cfg!(target_os = "macos") {
        home.join("Library/Application Support")
    } else {
        home.join(".local/share")
    };
    data.join("fire").join("sessions")
}

#[test]
fn terminal_run_reports_a_legacy_session_instead_of_duplicating_it() {
    let project = Project::new("reuse-legacy", CONFIG);
    // Stands in for the program of a session started by an older fire.
    // Its own process group, as sessions have, so stopping it never reaches the test runner.
    let program = Command::new("sleep")
        .arg("30")
        .process_group(0)
        .spawn()
        .expect("spawn sleep");
    let pid = program.id();
    let _cleanup = Cleanup {
        project: &project,
        extra: Some(program),
    };

    // The pipe layout older versions used: stdout.log, stderr.log, a stdin FIFO, no ctl.sock.
    let id = "server-legacy1";
    let dir = sessions_root(&project).join(id);
    fs::create_dir_all(&dir).unwrap();
    let cwd = fs::canonicalize(&project.dir).unwrap();
    let record = serde_json::json!({
        "id": id,
        "command": ["server"],
        "note": null,
        "cwd": cwd.display().to_string(),
        "started_at": 1,
        "supervisor_pid": pid,
        "pid": pid,
        "exit_code": null,
        "ended_at": null,
    });
    fs::write(dir.join("session.json"), record.to_string()).unwrap();
    fs::write(dir.join("stdout.log"), "legacy serving on :3000\n").unwrap();
    fs::write(dir.join("stderr.log"), "").unwrap();
    assert_eq!(session_ids(&project), vec![id.to_string()]);

    let (mut child, _master, terminal) = spawn_in_pty(project.fire(&["server"]));
    let code = wait_for_exit(&mut child);
    wait_for_output(&terminal, &format!("is running as session {id}"));
    wait_for_output(&terminal, "legacy serving on :3000");
    wait_for_output(&terminal, &format!("fire cli kill {id}"));
    assert_ne!(code, 0);
    assert_eq!(
        session_ids(&project),
        vec![id.to_string()],
        "no duplicate session"
    );
    assert!(dir.join("session.json").exists(), "legacy session is kept");
}
