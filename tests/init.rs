//! End-to-end tests for `fire cli init`: interactive and scriptable initialization.

#![cfg(unix)]

mod common;

use common::*;
use std::{fs, io::Write, process::Stdio};

/// A temp project with no config file yet.
fn empty_project(name: &str) -> Project {
    let project = Project::new(name, "");
    fs::remove_file(project.dir.join("fire.yml")).unwrap();
    project
}

fn init(project: &Project, args: &[&str]) -> String {
    let mut command = vec!["cli", "init"];
    command.extend_from_slice(args);
    let output = project.fire(&command).output().unwrap();
    assert!(output.status.success(), "{output:?}");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn schema_errors(yaml: &str) -> Vec<String> {
    let schema: serde_json::Value =
        serde_json::from_str(include_str!("../schemas/fire.schema.json")).unwrap();
    let doc: serde_json::Value = yaml_serde::from_str(yaml).unwrap();
    jsonschema::validator_for(&schema)
        .unwrap()
        .iter_errors(&doc)
        .map(|e| format!("{} at {}", e, e.instance_path()))
        .collect()
}

#[test]
fn writes_fire_yaml_with_document_start() {
    let project = empty_project("init-default");
    init(&project, &["--yes"]);

    assert!(!project.dir.join("fire.yml").exists());
    let yaml = fs::read_to_string(project.dir.join("fire.yaml")).expect("fire.yaml written");
    let mut lines = yaml.lines();
    assert!(lines
        .next()
        .unwrap()
        .starts_with("# yaml-language-server: $schema="));
    assert_eq!(lines.next(), Some("---"), "{yaml}");
    assert_eq!(schema_errors(&yaml), Vec::<String>::new());

    let output = project.fire(&["example"]).output().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("hello world"));
}

#[test]
fn base_name_gets_yaml_and_namespace_validates() {
    let project = empty_project("init-base");
    init(
        &project,
        &[
            "--file",
            "backend",
            "--namespace",
            "ex",
            "--namespace-description",
            "Example",
            "--group",
            "api",
            "--yes",
        ],
    );

    let yaml = fs::read_to_string(project.dir.join("backend.fire.yaml")).expect("file written");
    assert!(yaml.contains("---\nnamespace:"), "{yaml}");
    assert_eq!(schema_errors(&yaml), Vec::<String>::new());
}

#[test]
fn explicit_yml_extension_is_kept() {
    let project = empty_project("init-yml");
    init(&project, &["--file", "backend.fire.yml", "--yes"]);

    assert!(project.dir.join("backend.fire.yml").exists());
    assert!(!project.dir.join("backend.fire.yaml").exists());
}

#[test]
fn notes_an_existing_file_with_the_other_extension() {
    let project = Project::new("init-sibling", "commands:\n  old: echo old\n");
    let stdout = init(&project, &["--yes"]);

    assert!(stdout.contains("'fire.yml' also exists here"), "{stdout}");
    // both files are loaded
    let output = project.fire(&["old"]).output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let output = project.fire(&["example"]).output().unwrap();
    assert!(output.status.success(), "{output:?}");
}

#[test]
fn prompts_by_default_on_a_tty() {
    let project = empty_project("init-tty");
    let path = project.dir.join("frontend.fire.yaml");
    fs::write(&path, "sentinel\n").unwrap();
    let (mut child, mut terminal, output) = spawn_in_pty(project.fire(&["cli", "init"]));
    wait_for_output(&output, "Base file name");
    terminal.write_all(b"frontend\n").unwrap();
    wait_for_output(&output, "Namespace prefix");
    terminal.write_all(b"web\n").unwrap();
    wait_for_output(&output, "Namespace description");
    terminal.write_all(b"Web tools\n").unwrap();
    wait_for_output(&output, "Command group");
    terminal.write_all(b"ui\n").unwrap();

    assert_eq!(wait_for_exit(&mut child), 1);
    let terminal_output = output.lock().unwrap();
    assert!(!terminal_output.contains("Overwrite?"), "{terminal_output}");
    assert!(terminal_output.contains("--force"), "{terminal_output}");
    assert_eq!(fs::read_to_string(path).unwrap(), "sentinel\n");
}

#[test]
fn interactive_values_are_preserved_without_trimming() {
    let project = empty_project("init-tty-exact-values");
    let command = project.fire(&["cli", "init"]);
    let (mut child, mut terminal, output) = spawn_in_pty(command);
    wait_for_output(&output, "Base file name");
    terminal.write_all(b"\n").unwrap();
    wait_for_output(&output, "Namespace prefix");
    terminal.write_all(b"  team  \n").unwrap();
    wait_for_output(&output, "Namespace description");
    terminal.write_all(b"  Team tools  \n").unwrap();
    wait_for_output(&output, "Command group");
    terminal.write_all(b"  api  \n").unwrap();

    assert_eq!(wait_for_exit(&mut child), 0, "{}", output.lock().unwrap());
    let yaml = fs::read_to_string(project.dir.join("fire.yaml")).unwrap();
    let parsed: serde_json::Value = yaml_serde::from_str(&yaml).unwrap();
    assert_eq!(parsed["namespace"]["prefix"], "  team  ");
    assert_eq!(parsed["namespace"]["description"], "  Team tools  ");
    assert_eq!(parsed["group"], "  api  ");
}

#[test]
fn yes_uses_defaults_without_reading_stdin() {
    let project = empty_project("init-yes");
    let output = project
        .fire(&["cli", "init", "--yes"])
        .stdin(Stdio::null())
        .output()
        .expect("run non-interactive init");

    assert!(output.status.success(), "{output:?}");
    assert!(project.dir.join("fire.yaml").exists());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("Base file name"), "{stdout}");
}

#[test]
fn scripted_namespace_without_description_omits_the_description_key() {
    let project = empty_project("init-namespace-only");
    init(&project, &["--namespace", "exact namespace", "--yes"]);

    let yaml = fs::read_to_string(project.dir.join("fire.yaml")).unwrap();
    let parsed: serde_json::Value = yaml_serde::from_str(&yaml).unwrap();
    assert_eq!(parsed["namespace"]["prefix"], "exact namespace");
    assert!(parsed["namespace"].get("description").is_none(), "{yaml}");
}

#[test]
fn non_tty_uses_defaults_without_yes() {
    let project = empty_project("init-no-tty");
    let output = project
        .fire(&["cli", "init"])
        .stdin(Stdio::null())
        .output()
        .expect("run init without a tty");

    assert!(output.status.success(), "{output:?}");
    assert!(project.dir.join("fire.yaml").exists());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("Base file name"));
}

#[test]
fn flags_preserve_values_and_force_is_required_to_overwrite() {
    let project = empty_project("init-flags");
    let args = [
        "cli",
        "init",
        "--file",
        "backend",
        "--namespace",
        "team:\none",
        "--namespace-description",
        "  Team's tools  ",
        "--group",
        "api\tops",
        "--yes",
    ];
    let created = project.fire(&args).output().expect("create config");
    assert!(created.status.success(), "{created:?}");
    let path = project.dir.join("backend.fire.yaml");
    let yaml = fs::read_to_string(&path).expect("scripted config");
    let parsed: serde_json::Value = yaml_serde::from_str(&yaml).unwrap();
    assert_eq!(parsed["namespace"]["prefix"], "team:\none");
    assert_eq!(parsed["namespace"]["description"], "  Team's tools  ");
    assert_eq!(parsed["group"], "api\tops");

    fs::write(&path, "sentinel\n").unwrap();
    let refused = project.fire(&args).output().expect("refuse overwrite");
    assert!(!refused.status.success(), "{refused:?}");
    assert_eq!(fs::read_to_string(&path).unwrap(), "sentinel\n");
    assert!(String::from_utf8_lossy(&refused.stderr).contains("--force"));

    let forced = project
        .fire(&[
            "cli",
            "init",
            "--file",
            "backend",
            "--group",
            "replacement",
            "--yes",
            "--force",
        ])
        .output()
        .expect("force overwrite");
    assert!(forced.status.success(), "{forced:?}");
    assert!(fs::read_to_string(path)
        .unwrap()
        .contains("group: \"replacement\""));
}
