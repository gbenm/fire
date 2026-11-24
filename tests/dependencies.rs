//! Command dependency behavior through the real binary.

mod common;

use std::{fs, process::Stdio, time::Instant};

use common::{spawn_in_pty, wait_for_exit, Project};

fn run(project: &Project, args: &[&str]) -> (i32, String, String) {
    let output = project
        .fire(args)
        .stdin(Stdio::null())
        .output()
        .expect("run fire");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).replace('\r', ""),
        String::from_utf8_lossy(&output.stderr).replace('\r', ""),
    )
}

#[test]
fn needs_are_scoped_ordered_deduplicated_and_receive_no_user_args() {
    let project = Project::new(
        "needs-ordered",
        r#"
group: tools
commands:
  shared:
    exec: printf 'shared:%s\n' "$*" >> events
  first:
    needs: [shared]
    exec: printf 'first:%s\n' "$*" >> events
  docs:
    commands:
      build:
        needs: [shared]
        exec: printf 'docs:%s\n' "$*" >> events
  release:
    needs: [first, "docs build"]
    placeholder: "{{n}}"
    before: printf 'before\n' >> events
    exec: printf 'release:%s %s\n' '{1}' '{2}' >> events
"#,
    );

    let (code, help, stderr) = run(&project, &["tools", "release", ":h"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(help.contains("Needs: first, docs build"), "{help}");

    let (code, _, stderr) = run(&project, &["tools", "release", "one", "two"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(
        fs::read_to_string(project.dir.join("events")).unwrap(),
        "shared:\nfirst:\ndocs:\nbefore\nrelease:one two\n"
    );
}

#[test]
fn sequential_needs_wait_for_each_preceding_dependency_graph() {
    let project = Project::new(
        "needs-nested-order",
        r#"
commands:
  a-setup:
    exec: printf 'a-setup-start\n' >> events; sleep 0.1; printf 'a-setup-end\n' >> events
  a:
    needs: [a-setup]
    exec: printf 'a\n' >> events
  b-setup:
    exec: printf 'b-setup\n' >> events
  b:
    needs: [b-setup]
    exec: printf 'b\n' >> events
  all:
    needs: [a, b]
    exec: printf 'all\n' >> events
"#,
    );

    let (code, _, stderr) = run(&project, &["all"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(
        fs::read_to_string(project.dir.join("events")).unwrap(),
        "a-setup-start\na-setup-end\na\nb-setup\nb\nall\n"
    );
}

#[test]
fn a_repeated_need_is_a_no_op_instead_of_a_dependency_cycle() {
    let project = Project::new(
        "needs-repeated",
        r#"
commands:
  prepare:
    exec: printf 'prepare\n' >> events
  all:
    needs: [prepare, prepare]
    exec: printf 'all\n' >> events
"#,
    );

    let (mut child, _, output) = spawn_in_pty(project.fire(&["all"]));
    assert_eq!(wait_for_exit(&mut child), 0, "{}", output.lock().unwrap());
    assert_eq!(
        fs::read_to_string(project.dir.join("events")).unwrap(),
        "prepare\nall\n"
    );
}

#[test]
fn agent_runs_reject_restricted_dependencies_before_any_work_starts() {
    let project = Project::new(
        "needs-agent-restricted",
        r#"
commands:
  allowed:
    exec: touch allowed-ran
  restricted:
    agents: false
    exec: touch restricted-ran
  all:
    needs: [allowed, restricted]
    exec: touch all-ran
"#,
    );

    let (code, _, stderr) = run(&project, &["--agent", "all"]);
    assert_eq!(code, 1, "{stderr}");
    assert!(
        stderr.contains("cannot need `restricted`") && stderr.contains("agents: false"),
        "{stderr}"
    );
    assert!(!project.dir.join("allowed-ran").exists());
    assert!(!project.dir.join("restricted-ran").exists());
    assert!(!project.dir.join("all-ran").exists());
}

#[test]
fn dependency_failure_stops_the_chain_and_names_the_failed_command() {
    let project = Project::new(
        "needs-failure",
        r#"
commands:
  prepare:
    exec: printf 'prepare\n' >> events; exit 7
  skipped:
    exec: printf 'skipped\n' >> events
  deploy:
    needs: [prepare, skipped]
    exec: printf 'deploy\n' >> events
"#,
    );

    let (code, _, stderr) = run(&project, &["deploy"]);
    assert_eq!(code, 7, "{stderr}");
    assert!(
        stderr.contains("Dependency `prepare` failed with exit code 7"),
        "{stderr}"
    );
    assert_eq!(
        fs::read_to_string(project.dir.join("events")).unwrap(),
        "prepare\n"
    );
}

#[test]
fn cycles_missing_commands_and_long_running_dependencies_are_rejected_before_execution() {
    let cases = [
        (
            "cycle",
            "commands:\n  a:\n    needs: [b]\n    exec: touch ran\n  b:\n    needs: [a]\n    exec: touch ran\n",
            "a",
            "Dependency cycle: a -> b -> a",
        ),
        (
            "missing",
            "commands:\n  a:\n    needs: [missing]\n    exec: touch ran\n",
            "a",
            "needs `missing`, but that command does not exist in the same scope",
        ),
        (
            "long-running",
            "commands:\n  server:\n    long_running: true\n    exec: sleep 30\n  a:\n    needs: [server]\n    exec: touch ran\n",
            "a",
            "cannot need `server` because it is `long_running: true`",
        ),
    ];

    for (name, config, command, message) in cases {
        let project = Project::new(&format!("needs-{name}"), config);
        let (code, _, stderr) = run(&project, &[command]);
        assert_eq!(code, 1, "{name}: {stderr}");
        assert!(stderr.contains(message), "{name}: {stderr}");
        assert!(!project.dir.join("ran").exists(), "{name} executed");
    }
}

#[test]
fn parallel_needs_prefix_output_and_fail_fast() {
    let project = Project::new(
        "needs-parallel",
        r#"
commands:
  slow:
    exec: echo started; sleep 5; echo finished
  bad:
    exec: sleep 0.1; echo broken >&2; exit 9
  check:
    needs: [slow, bad]
    parallel: true
    exec: echo should-not-run
"#,
    );

    let start = Instant::now();
    let (code, stdout, stderr) = run(&project, &["check"]);
    assert_eq!(code, 9, "stdout={stdout}\nstderr={stderr}");
    assert!(
        start.elapsed().as_secs_f32() < 3.0,
        "parallel dependency was not cancelled"
    );
    assert!(stdout.contains("[slow] started"), "{stdout}");
    assert!(stderr.contains("[bad] broken"), "{stderr}");
    assert!(!stdout.contains("should-not-run"), "{stdout}");
}

#[test]
fn parallel_needs_start_together_and_dry_run_shows_the_entire_plan() {
    let project = Project::new(
        "needs-parallel-dry",
        r#"
commands:
  one:
    exec: printf 'one-start\n' >> events; sleep 0.1; printf 'one-end\n' >> events
  two:
    exec: printf 'two\n' >> events
  all:
    needs: [one, two]
    parallel: true
    exec: printf 'all\n' >> events
"#,
    );

    let (code, stdout, stderr) = run(&project, &["all"]);
    assert_eq!(code, 0, "stdout={stdout}\nstderr={stderr}");
    let events = fs::read_to_string(project.dir.join("events")).unwrap();
    // No concurrency limit: `two` runs while `one` is still sleeping.
    assert!(
        events.find("two").unwrap() < events.find("one-end").unwrap(),
        "{events}"
    );
    assert!(events.ends_with("all\n"), "{events}");

    fs::remove_file(project.dir.join("events")).unwrap();
    let (code, stdout, stderr) = run(&project, &["--dry-run", "all"]);
    assert_eq!(code, 0, "stdout={stdout}\nstderr={stderr}");
    let plan = format!("{stdout}{stderr}");
    assert!(plan.contains("one-start"), "{plan}");
    assert!(plan.contains("printf 'two"), "{plan}");
    assert!(plan.contains("printf 'all"), "{plan}");
    assert!(!project.dir.join("events").exists());
}

#[test]
fn dependency_chain_stays_inside_the_requested_commands_session() {
    let project = Project::new(
        "needs-session",
        r#"
commands:
  prepare:
    exec: printf '%s\n' "$FIRE_SESSION" >> sessions
  run:
    needs: [prepare]
    exec: printf '%s\n' "$FIRE_SESSION" >> sessions
"#,
    );

    let (mut child, _, _) = spawn_in_pty(project.fire(&["run"]));
    assert_eq!(wait_for_exit(&mut child), 0);
    let sessions = fs::read_to_string(project.dir.join("sessions")).unwrap();
    let ids: Vec<&str> = sessions.lines().collect();
    assert_eq!(ids.len(), 2, "{sessions:?}");
    assert!(!ids[0].is_empty(), "{sessions:?}");
    assert_eq!(ids[0], ids[1], "{sessions:?}");
}
