//! Follow mode: `fire cli logs -f <id>` and `fire --bg -f <command>` stream a session's output,
//! read-only, until the program exits (ending with its exit code) or Ctrl+C, which only closes
//! the viewer: the program keeps running. Sessions from older fire versions (pipe layout) are
//! followed by tailing their log files.

mod common;

use std::{
    fs::{self, OpenOptions},
    io::Write,
    process::Stdio,
};

use common::{eventually, spawn_in_pty, wait_for_exit, wait_for_output, LegacySession, Project};

const CONFIG: &str = r#"
commands:
  echoer:
    exec: echo ready; while read line; do echo "got $line"; done
  bye:
    exec: echo ready; read line; echo "bye $line"; exit 3
  env:
    exec: echo "follow=$FIRE_FOLLOW"
"#;

/// Ids of every session listed by `fire cli ps -a`.
fn session_ids(project: &Project) -> Vec<String> {
    let (_, ps, _) = project.cli(&["ps", "-a"]);
    ps.lines()
        .skip(1)
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_string)
        .collect()
}

fn is_running(project: &Project, id: &str) -> bool {
    let (_, ps, _) = project.cli(&["ps"]);
    ps.lines().any(|line| {
        let mut columns = line.split_whitespace();
        columns.next() == Some(id) && columns.next() == Some("running")
    })
}

/// Kills every session of the project on drop, so a failing test leaves nothing running.
struct Cleanup<'a>(&'a Project);

impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        for id in session_ids(self.0) {
            let _ = self.0.cli(&["kill", &id, "KILL"]);
        }
    }
}

/// `fire --bg <command>` without a terminal: the session id.
fn start_bg(project: &Project, command: &str) -> String {
    let output = project
        .fire(&["--bg", command])
        .stdin(Stdio::null())
        .output()
        .expect("run fire --bg");
    assert!(output.status.success(), "{output:?}");
    let id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    assert!(!id.is_empty());
    eventually(
        || project.cli(&["logs", &id]).1.contains("ready"),
        || project.cli(&["logs", &id]).1,
    );
    id
}

fn send(project: &Project, id: &str, text: &str) {
    let (code, _, stderr) = project.cli(&["send", id, text]);
    assert_eq!(code, 0, "{stderr}");
}

#[test]
fn ctrl_c_stops_following_and_leaves_the_session_running() {
    let project = Project::new("follow-ctrl-c", CONFIG);
    let _cleanup = Cleanup(&project);
    let id = start_bg(&project, "echoer");

    let (mut child, mut master, terminal) = spawn_in_pty(project.fire(&["cli", "logs", "-f", &id]));
    wait_for_output(&terminal, "ready");
    wait_for_output(&terminal, &format!("following {id}"));
    // New output streams in.
    send(&project, &id, "ping");
    wait_for_output(&terminal, "got ping");

    // Ctrl+C on the viewer's terminal: the viewer ends, the program doesn't.
    master.write_all(b"\x03").unwrap();
    let code = wait_for_exit(&mut child);
    assert_eq!(code, 130, "{}", terminal.lock().unwrap());
    wait_for_output(&terminal, &format!("{id} keeps running"));
    assert!(is_running(&project, &id), "session still running");
    send(&project, &id, "again");
    eventually(
        || project.cli(&["logs", &id]).1.contains("got again"),
        || project.cli(&["logs", &id]).1,
    );
}

#[test]
fn follow_ends_with_the_exit_code_when_the_program_exits() {
    let project = Project::new("follow-exit", CONFIG);
    let _cleanup = Cleanup(&project);
    let id = start_bg(&project, "bye");

    let (mut child, _master, terminal) =
        spawn_in_pty(project.fire(&["cli", "logs", "--follow", &id, "5"]));
    wait_for_output(&terminal, &format!("following {id}"));
    send(&project, &id, "now");
    let code = wait_for_exit(&mut child);
    assert_eq!(code, 3, "{}", terminal.lock().unwrap());
    wait_for_output(&terminal, "bye now");
    wait_for_output(&terminal, &format!("{id} exited with code 3"));
}

#[test]
fn bg_follow_starts_the_session_and_follows_it() {
    let project = Project::new("follow-bg-start", CONFIG);
    let _cleanup = Cleanup(&project);

    let (mut child, _master, terminal) = spawn_in_pty(project.fire(&["--bg", "-f", "bye"]));
    wait_for_output(&terminal, "ready");
    let ids = session_ids(&project);
    let [id] = &ids[..] else {
        panic!("one session expected: {ids:?}")
    };
    assert!(is_running(&project, id));
    wait_for_output(&terminal, &format!("following {id}"));
    send(&project, id, "later");
    let code = wait_for_exit(&mut child);
    assert_eq!(code, 3, "{}", terminal.lock().unwrap());
    wait_for_output(&terminal, "bye later");
    wait_for_output(&terminal, &format!("{id} exited with code 3"));
}

#[test]
fn bg_follow_reuses_the_running_session_and_ctrl_c_leaves_it_running() {
    let project = Project::new("follow-bg-reuse", CONFIG);
    let _cleanup = Cleanup(&project);
    let id = start_bg(&project, "echoer");

    let (mut child, mut master, terminal) =
        spawn_in_pty(project.fire(&["--bg", "--follow", "echoer"]));
    wait_for_output(&terminal, &format!("already running here as session {id}"));
    wait_for_output(&terminal, "ready");
    wait_for_output(&terminal, &format!("following {id}"));
    assert_eq!(session_ids(&project), vec![id.clone()], "no second session");
    send(&project, &id, "ping");
    wait_for_output(&terminal, "got ping");

    master.write_all(b"\x03").unwrap();
    assert_eq!(wait_for_exit(&mut child), 130);
    assert!(is_running(&project, &id), "session still running");
}

#[test]
fn follow_flag_sets_fire_follow_and_the_variable_alone_follows_too() {
    let project = Project::new("follow-env", CONFIG);
    let _cleanup = Cleanup(&project);

    // The flag exports FIRE_FOLLOW=true to what the command runs; following a quick command
    // shows all of its output from the start and ends with it.
    let output = project
        .fire(&["--bg", "-f", "env"])
        .stdin(Stdio::null())
        .output()
        .expect("run fire --bg -f");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert!(stdout.contains("follow=true"), "{stdout}");
    assert!(stderr.contains("exited with code 0"), "{stderr}");

    // The variable (any boolean spelling) follows like the flag.
    let output = project
        .fire(&["--bg", "env"])
        .env("FIRE_FOLLOW", "yes")
        .stdin(Stdio::null())
        .output()
        .expect("run fire --bg with FIRE_FOLLOW");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("exited with code 0"), "{stderr}");

    // Following needs a session started with --bg.
    let output = project
        .fire(&["-f", "env"])
        .stdin(Stdio::null())
        .output()
        .expect("run fire -f");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("use it with --bg"), "{stderr}");
}

#[test]
fn legacy_session_is_followed_by_tailing_its_logs() {
    let project = Project::new("follow-legacy", CONFIG);
    let _cleanup = Cleanup(&project);
    let legacy = LegacySession::start(
        &project,
        "echoer-legacy1",
        &["echoer"],
        "legacy serving on :3000\n",
    );
    let id = legacy.id.as_str();

    let (mut child, _master, terminal) = spawn_in_pty(project.fire(&["cli", "logs", "-f", id]));
    wait_for_output(&terminal, "legacy serving on :3000");
    wait_for_output(&terminal, &format!("following {id}"));

    let append = |file: &str, text: &str| {
        OpenOptions::new()
            .append(true)
            .open(legacy.dir.join(file))
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
    };
    append("stdout.log", "legacy request handled\n");
    wait_for_output(&terminal, "legacy request handled");
    append("stderr.log", "legacy warning\n");
    wait_for_output(&terminal, "legacy warning");
    assert!(legacy.alive(), "the old program is untouched");

    // The old supervisor records the exit: the follow ends with its code.
    let record_path = legacy.dir.join("session.json");
    let mut record: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&record_path).unwrap()).unwrap();
    record["exit_code"] = 3.into();
    record["ended_at"] = 2.into();
    fs::write(&record_path, record.to_string()).unwrap();
    assert_eq!(wait_for_exit(&mut child), 3);
    wait_for_output(&terminal, &format!("{id} exited with code 3"));
}
