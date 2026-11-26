//! Completion of `fire cli fg/send/kill/logs` arguments against real session records.

mod common;

use std::process::Stdio;

use common::{eventually, Project};

const CONFIG: &str = r#"
commands:
  server:
    long_running: true
    exec: echo serving && sleep 30
  quick:
    long_running: true
    exec: echo done
"#;

fn complete(project: &Project, words: &[&str]) -> Vec<String> {
    let mut args = vec!["__complete", "fire", "cli"];
    args.extend_from_slice(words);
    let output = project
        .fire(&args)
        .stdin(Stdio::null())
        .output()
        .expect("run fire __complete");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

fn start(project: &Project, args: &[&str]) -> String {
    let output = project
        .fire(args)
        .stdin(Stdio::null())
        .output()
        .expect("start session");
    let id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    assert!(
        !id.is_empty(),
        "no session id: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    id
}

#[test]
fn session_commands_complete_ids_with_descriptions() {
    let mut project = Project::new("session-completion", CONFIG);
    // No sessions dir yet: no suggestions, no error.
    assert!(complete(&project, &["fg", ""]).is_empty());

    let done = start(&project, &["--bg", "quick"]);
    eventually(
        || project.cli(&["ps", "-a"]).1.contains("exited"),
        || project.cli(&["ps", "-a"]).1,
    );
    let server = start(&project, &["--bg", "-m", "for completion", "server"]);
    project.session = Some(server.clone());

    let fg = complete(&project, &["fg", ""]);
    assert_eq!(fg.len(), 1, "{fg:?}");
    assert!(
        fg[0].starts_with(&format!(
            "{server}\trunning · fire server · \"for completion\" · "
        )),
        "{fg:?}"
    );

    let logs = complete(&project, &["logs", ""]);
    assert!(logs[0].starts_with("-f\t"), "{logs:?}");
    assert!(logs[1].starts_with("--follow\t"), "{logs:?}");
    let logs = logs[2..].to_vec();
    assert_eq!(complete(&project, &["logs", "-f", ""]), logs);
    assert_eq!(logs.len(), 2, "{logs:?}");
    assert!(
        logs[0].starts_with(&format!("{server}\t")),
        "running first: {logs:?}"
    );
    assert!(
        logs[1].starts_with(&format!("{done}\texited 0 · fire quick · ")),
        "{logs:?}"
    );
    assert_eq!(complete(&project, &["kill", ""]), logs);

    let send = complete(&project, &["send", ""]);
    assert!(send[0].starts_with("--raw\t"), "{send:?}");
    assert_eq!(&send[1..], &fg[..]);
    assert_eq!(complete(&project, &["send", "--raw", ""]), fg);
    assert!(complete(&project, &["send", &server, ""]).is_empty());

    let signals = complete(&project, &["kill", &server, ""]);
    assert!(
        signals.iter().any(|line| line.starts_with("INT\t")),
        "{signals:?}"
    );
    assert!(
        signals.iter().any(|line| line.starts_with("KILL\t")),
        "{signals:?}"
    );
    assert!(complete(&project, &["kill", &done, ""]).is_empty());
    assert!(complete(&project, &["logs", &server, ""]).is_empty());

    // Typed prefix narrows the ids.
    assert_eq!(complete(&project, &["logs", "qu"]).len(), 1);
}
