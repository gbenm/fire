//! End-to-end tests for `fire cli global` (#43): registering directories, listing them by
//! namespace, removing, pruning, editing, the deprecated `fire cli install` alias, the
//! first-use registration prompt and completion.

#![cfg(unix)]

mod common;

use common::*;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Stdio,
    thread,
    time::Duration,
};

const LOCAL: &str = "commands:\n  hi: echo local-hi\n";

fn write(path: &Path, content: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

/// A directory under the project with the given config files.
fn dir_with(project: &Project, name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = project.dir.join(name);
    fs::create_dir_all(&dir).unwrap();
    for (file, content) in files {
        write(&dir.join(file), content);
    }
    dir.canonicalize().unwrap()
}

fn api_dir(project: &Project) -> PathBuf {
    dir_with(
        project,
        "api",
        &[
            (
                "fire.yaml",
                "namespace: api\ncommands:\n  ping: echo api-pong\n",
            ),
            ("db.fire.yaml", "commands:\n  migrate: echo migrated\n"),
        ],
    )
}

fn tools_dir(project: &Project) -> PathBuf {
    dir_with(
        project,
        "tools",
        &[(
            "fire.yaml",
            "namespace: tools\ncommands:\n  lint: echo linted\n",
        )],
    )
}

fn ok(result: (i32, String, String)) -> String {
    assert_eq!(result.0, 0, "stdout: {}\nstderr: {}", result.1, result.2);
    result.1
}

fn fails(result: (i32, String, String)) -> String {
    assert_ne!(result.0, 0, "expected failure; stdout: {}", result.1);
    result.2
}

fn path_str(path: &Path) -> &str {
    path.to_str().unwrap()
}

/// Runs a fire command from `cwd` without a terminal.
fn run_in(project: &Project, cwd: &Path, args: &[&str]) -> (i32, String, String) {
    let output = project
        .fire(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .output()
        .expect("run fire");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn add_list_remove_and_prune() {
    let project = Project::new("global-manage", LOCAL);
    let api = api_dir(&project);
    let tools = tools_dir(&project);
    let scripts = dir_with(
        &project,
        "scripts",
        &[("fire.yaml", "commands:\n  backup: echo backed-up\n")],
    );
    let old = dir_with(&project, "old", &[("fire.yaml", "commands: {}\n")]);

    for dir in [&api, &tools, &scripts, &old] {
        let out = ok(project.cli(&["global", "add", path_str(dir)]));
        assert!(out.contains("Registered"), "{out}");
    }
    let again = ok(project.cli(&["global", "add", path_str(&api)]));
    assert!(again.contains("Already registered"), "{again}");

    // registered commands load from any directory
    let elsewhere = dir_with(&project, "elsewhere", &[]);
    assert!(ok(run_in(&project, &elsewhere, &["api", "ping"])).contains("api-pong"));
    assert!(ok(run_in(&project, &elsewhere, &["backup"])).contains("backed-up"));

    fs::remove_dir_all(&old).unwrap();
    let list = ok(project.cli(&["global", "list"]));
    let lines: Vec<&str> = list.lines().collect();
    assert_eq!(lines.len(), 4, "{list}");
    assert!(lines[0].starts_with("api ") && lines[0].contains(path_str(&api)));
    assert!(lines[0].ends_with("fire.yaml, db.fire.yaml"), "{list}");
    assert!(lines[1].starts_with("tools ") && lines[1].ends_with("fire.yaml"));
    assert!(lines[2].starts_with("(root)") && lines[2].contains(path_str(&scripts)));
    assert!(
        lines[3].starts_with("- ") && lines[3].ends_with("(missing)"),
        "{list}"
    );

    // namespace or path
    let out = ok(project.cli(&["global", "remove", "api"]));
    assert!(
        out.contains("Unregistered") && out.contains(path_str(&api)),
        "{out}"
    );
    assert!(
        api.join("fire.yaml").exists(),
        "remove must never delete files"
    );
    let err = fails(project.cli(&["global", "remove", "api"]));
    assert!(
        err.contains("No registered directory has namespace `api`"),
        "{err}"
    );
    ok(project.cli(&["global", "remove", path_str(&tools)]));
    let err = fails(project.cli(&["global", "remove", path_str(&tools)]));
    assert!(err.contains("is not registered"), "{err}");

    let out = ok(project.cli(&["global", "prune"]));
    assert!(
        out.contains("missing directory") && out.contains("old"),
        "{out}"
    );
    let out = ok(project.cli(&["global", "prune"]));
    assert!(out.contains("Nothing to prune"), "{out}");

    let list = ok(project.cli(&["global", "list"]));
    assert_eq!(list.lines().count(), 1, "{list}");
    assert!(list.contains(path_str(&scripts)));
}

#[test]
fn add_and_remove_default_to_current_directory() {
    let project = Project::new("global-cwd", LOCAL);
    let cwd = project.dir.canonicalize().unwrap();
    ok(project.cli(&["global", "add"]));
    assert!(ok(project.cli(&["global", "list"])).contains(path_str(&cwd)));
    ok(project.cli(&["global", "remove"]));
    assert!(ok(project.cli(&["global", "list"])).contains("No registered directories"));
}

#[test]
fn namespace_in_several_directories_asks_for_a_path() {
    let project = Project::new("global-ambiguous", LOCAL);
    let one = dir_with(
        &project,
        "one",
        &[("fire.yaml", "namespace: dup\ncommands: {}\n")],
    );
    let two = dir_with(
        &project,
        "two",
        &[("fire.yaml", "namespace: dup\ncommands: {}\n")],
    );
    ok(project.cli(&["global", "add", path_str(&one)]));
    ok(project.cli(&["global", "add", path_str(&two)]));
    let err = fails(project.cli(&["global", "remove", "dup"]));
    assert!(err.contains("several registered directories"), "{err}");
    assert!(
        err.contains(path_str(&one)) && err.contains(path_str(&two)),
        "{err}"
    );
}

#[test]
fn deprecated_install_registers_with_a_hint() {
    let project = Project::new("global-install", LOCAL);
    let (code, stdout, stderr) = project.cli(&["install"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stderr.contains("deprecated") && stderr.contains("fire cli global add"));
    assert!(stdout.contains("Registered"));
    let list = ok(project.cli(&["global", "list"]));
    assert!(list.contains(path_str(&project.dir.canonicalize().unwrap())));

    let help = ok(project.cli(&[]));
    assert!(
        help.contains("global") && !help.contains("install "),
        "{help}"
    );
}

#[test]
fn edit_opens_the_config_with_the_editor() {
    let project = Project::new("global-edit", LOCAL);
    let api = api_dir(&project);
    let tools = tools_dir(&project);
    ok(project.cli(&["global", "add", path_str(&api)]));
    ok(project.cli(&["global", "add", path_str(&tools)]));
    let opened = project.dir.join("opened");
    let editor = format!("printf '%s\\n' >>{}", opened.display());
    let edit = |args: &[&str]| {
        let mut full = vec!["cli", "global", "edit"];
        full.extend_from_slice(args);
        let output = project
            .fire(&full)
            .env_remove("VISUAL")
            .env("EDITOR", &editor)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        (
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    };

    ok(edit(&[]));
    ok(edit(&["tools"]));
    ok(edit(&["api", "db.fire.yaml"]));
    let err = fails(edit(&["api"]));
    assert!(
        err.contains("several config files") && err.contains("db.fire.yaml"),
        "{err}"
    );
    let err = fails(edit(&["api", "nope.yaml"]));
    assert!(err.contains("not a fire config file"), "{err}");

    let lines: Vec<PathBuf> = fs::read_to_string(&opened)
        .unwrap()
        .lines()
        .map(PathBuf::from)
        .collect();
    assert_eq!(
        lines,
        vec![
            project.dir.canonicalize().unwrap().join("fire.yml"),
            tools.join("fire.yaml"),
            api.join("db.fire.yaml"),
        ]
    );

    let output = project
        .fire(&["cli", "global", "edit"])
        .env_remove("VISUAL")
        .env_remove("EDITOR")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Set $VISUAL or $EDITOR"));
}

#[test]
fn completion_suggests_targets_with_descriptions() {
    let project = Project::new("global-complete", LOCAL);
    let api = api_dir(&project);
    ok(project.cli(&["global", "add", path_str(&api)]));
    let complete = |words: &[&str]| {
        let mut args = vec!["__complete", "fire", "cli", "global"];
        args.extend_from_slice(words);
        let output = project.fire(&args).stdin(Stdio::null()).output().unwrap();
        String::from_utf8_lossy(&output.stdout).into_owned()
    };

    let subs = complete(&[""]);
    for sub in ["add\t", "remove\t", "list\t", "prune\t", "edit\t"] {
        assert!(subs.contains(sub), "{subs}");
    }
    let targets = complete(&["remove", ""]);
    assert!(targets.contains("api\t"), "{targets}");
    assert!(
        targets.contains(&format!("{}\tapi · fire.yaml, db.fire.yaml", api.display())),
        "{targets}"
    );
    let files = complete(&["edit", "api", ""]);
    assert!(files.contains("fire.yaml\tapi"), "{files}");
    assert!(files.contains("db.fire.yaml\tapi"), "{files}");
}

/// Runs `fire hi` in a terminal without CI set, answering the registration prompt.
fn run_with_prompt(project: &Project, answer: Option<&str>) -> String {
    let mut command = project.fire(&["hi"]);
    command.env_remove("CI");
    let (mut child, mut master, output) = spawn_in_pty(command);
    if let Some(answer) = answer {
        wait_for_output(&output, "so its commands are available globally? [y/N]");
        thread::sleep(Duration::from_millis(100));
        assert!(
            child.try_wait().unwrap().is_none(),
            "exited before the answer"
        );
        master.write_all(answer.as_bytes()).unwrap();
        master.write_all(b"\r").unwrap();
    }
    assert_eq!(wait_for_exit(&mut child), 0, "{:?}", output.lock().unwrap());
    wait_for_output(&output, "local-hi");
    let text = output.lock().unwrap().clone();
    text
}

#[test]
fn first_use_prompt_yes_registers_once() {
    let project = Project::new("global-prompt-yes", LOCAL);
    let out = run_with_prompt(&project, Some("y"));
    assert!(out.contains("Registered"), "{out}");
    let list = ok(project.cli(&["global", "list"]));
    assert!(
        list.contains(path_str(&project.dir.canonicalize().unwrap())),
        "{list}"
    );

    let out = run_with_prompt(&project, None);
    assert!(!out.contains("[y/N]"), "asked twice: {out}");
}

#[test]
fn first_use_prompt_no_is_remembered() {
    let project = Project::new("global-prompt-no", LOCAL);
    let out = run_with_prompt(&project, Some(""));
    assert!(out.contains("fire cli global add"), "{out}");
    assert!(ok(project.cli(&["global", "list"])).contains("No registered directories"));

    let out = run_with_prompt(&project, None);
    assert!(!out.contains("[y/N]"), "asked twice: {out}");
}

#[test]
fn no_prompt_without_a_terminal_or_in_ci() {
    let project = Project::new("global-prompt-quiet", LOCAL);
    let output = project
        .fire(&["hi"])
        .env_remove("CI")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("[y/N]"));

    // CI set: the terminal run doesn't ask either, and nothing was remembered by the runs above
    let (mut child, _master, out) = spawn_in_pty(project.fire(&["hi"]));
    assert_eq!(wait_for_exit(&mut child), 0);
    wait_for_output(&out, "local-hi");
    assert!(!out.lock().unwrap().contains("[y/N]"));

    let out = run_with_prompt(&project, Some("n"));
    assert!(out.contains("Not registered"), "{out}");
}
