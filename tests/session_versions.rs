//! Sessions outlive the fire that started them (an upgrade replaces the binary, not the running
//! supervisors): a newer fire keeps listing, reading, signalling and stopping them, and never
//! starts a duplicate next to one. `fire cli upgrade` never stops them and lists the ones still
//! on the old version.

mod common;

use std::{fs, process::Stdio};

use common::{eventually, sessions_root, LegacySession, Project};

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
        .map(str::to_string)
        .collect()
}

fn running_ids(project: &Project) -> Vec<String> {
    let (_, ps, _) = project.cli(&["ps"]);
    ps.lines()
        .skip(1)
        .filter(|line| line.split_whitespace().nth(1) == Some("running"))
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_string)
        .collect()
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

/// `fire --bg <command>`: (exit code, stdout, stderr).
fn start_bg(project: &Project, command: &str) -> (i32, String, String) {
    let output = project
        .fire(&["--bg", command])
        .stdin(Stdio::null())
        .output()
        .expect("run fire --bg");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).trim().to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

#[test]
fn legacy_session_is_listed_readable_signalable_and_not_duplicated() {
    let project = Project::new("versions-legacy", CONFIG);
    let _cleanup = Cleanup(&project);
    let legacy = LegacySession::start(
        &project,
        "server-legacy1",
        &["server"],
        "legacy serving on :3000\n",
    );
    let id = legacy.id.as_str();

    assert_eq!(running_ids(&project), vec![id.to_string()], "listed");
    let (code, logs, _) = project.cli(&["logs", id]);
    assert_eq!(code, 0);
    assert!(logs.contains("legacy serving on :3000"), "{logs}");

    // Recognized as already running: no duplicate, here or from a terminal.
    let (code, stdout, stderr) = start_bg(&project, "server");
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(stdout, id, "{stderr}");
    assert!(stderr.contains("already running"), "{stderr}");
    assert_eq!(session_ids(&project), vec![id.to_string()], "no duplicate");

    // Attaching needs the control protocol, which it predates: reported, not attempted.
    let (code, _, stderr) = project.cli(&["fg", id]);
    assert_eq!(code, 1);
    assert!(
        stderr.contains("started by an older fire (session format 0, attach needs 1)"),
        "{stderr}"
    );
    assert!(stderr.contains(&format!("fire cli kill {id}")), "{stderr}");

    let (code, stdout, stderr) = project.cli(&["kill", id, "CONT"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains(&format!("sent CONT to {id}")), "{stdout}");
    assert!(legacy.alive(), "a harmless signal leaves it running");

    let (code, _, stderr) = project.cli(&["kill", id]);
    assert_eq!(code, 0, "{stderr}");
    eventually(|| !legacy.alive(), || "legacy program still alive".into());
    assert!(running_ids(&project).is_empty());
}

#[test]
fn versioned_session_records_its_version_and_is_managed() {
    let project = Project::new("versions-current", CONFIG);
    let _cleanup = Cleanup(&project);

    let (code, id, stderr) = start_bg(&project, "server");
    assert_eq!(code, 0, "{stderr}");
    let record: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(sessions_root(&project).join(&id).join("session.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(record["protocol"], 1, "{record}");
    assert_eq!(
        record["fire_version"],
        env!("CARGO_PKG_VERSION"),
        "{record}"
    );

    assert_eq!(running_ids(&project), vec![id.clone()], "listed");
    eventually(
        || project.cli(&["logs", &id]).1.contains("serving"),
        || project.cli(&["logs", &id]).1,
    );
    let (_, again, _) = start_bg(&project, "server");
    assert_eq!(again, id, "no duplicate");
    assert_eq!(session_ids(&project), vec![id.clone()], "no duplicate");

    let (code, stdout, stderr) = project.cli(&["kill", &id, "CONT"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains(&format!("sent CONT to {id}")), "{stdout}");
    assert_eq!(running_ids(&project), vec![id.clone()]);

    let (code, _, stderr) = project.cli(&["kill", &id]);
    assert_eq!(code, 0, "{stderr}");
    assert!(running_ids(&project).is_empty());
}

/// `fire cli upgrade` against a stand-in `brew` that "installs" `installed`.
fn upgrade(project: &Project, installed: &str) -> (i32, String, String) {
    let bin = project.dir.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let brew = bin.join("brew");
    fs::write(
        &brew,
        format!(
            "#!/bin/sh\necho \"$@\" >> \"{}\"\ncase \"$1\" in list) echo \"fire {installed}\";; esac\nexit 0\n",
            project.dir.join("brew.log").display()
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&brew).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    fs::set_permissions(&brew, permissions).unwrap();

    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = project
        .fire(&["cli", "upgrade"])
        .env("PATH", path)
        .stdin(Stdio::null())
        .output()
        .expect("run fire cli upgrade");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

#[test]
fn upgrade_keeps_sessions_running_and_lists_those_on_the_old_version() {
    let project = Project::new("versions-upgrade", CONFIG);
    let _cleanup = Cleanup(&project);
    let legacy = LegacySession::start(&project, "old-legacy1", &["old"], "");
    let (code, id, stderr) = start_bg(&project, "server");
    assert_eq!(code, 0, "{stderr}");

    let (code, _, stderr) = upgrade(&project, "99.0.0");
    assert_eq!(code, 0, "{stderr}");
    let brew_log = fs::read_to_string(project.dir.join("brew.log")).unwrap();
    assert!(brew_log.contains("upgrade fire"), "{brew_log}");
    assert!(
        stderr.contains("keep running on the old version until restarted"),
        "{stderr}"
    );
    let line = |id: &str| {
        stderr
            .lines()
            .find(|line| line.trim_start().starts_with(id))
            .unwrap_or_else(|| panic!("{id} not listed: {stderr}"))
            .to_string()
    };
    assert!(
        line(&id).contains(&format!("fire {}", env!("CARGO_PKG_VERSION"))),
        "{stderr}"
    );
    assert!(line(&legacy.id).contains("an older fire"), "{stderr}");
    assert!(
        stderr.contains("to move one to fire 99.0.0: fire cli kill <id>, then start it again"),
        "{stderr}"
    );
    // Never stopped by the upgrade.
    let mut running = running_ids(&project);
    running.sort();
    let mut expected = vec![id.clone(), legacy.id.clone()];
    expected.sort();
    assert_eq!(running, expected);
    assert!(legacy.alive());

    // Nothing new installed: sessions of this version are not on an old one.
    let (code, _, stderr) = upgrade(&project, env!("CARGO_PKG_VERSION"));
    assert_eq!(code, 0, "{stderr}");
    assert!(!stderr.contains(&id), "{stderr}");
    assert!(stderr.contains(&legacy.id), "{stderr}");
}
