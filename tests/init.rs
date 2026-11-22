//! End-to-end tests for `fire cli init` (#48): the file it writes, its name and how it sits
//! next to an existing config.

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

/// Runs `fire cli init` answering its prompts (base name, namespace prefix, group) with `answers`.
fn init(project: &Project, answers: &str) -> String {
    let mut child = project
        .fire(&["cli", "init"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn fire cli init");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(answers.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
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
    init(&project, "\n\n\n");

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
    init(&project, "backend\nex\nExample\napi\n");

    let yaml = fs::read_to_string(project.dir.join("backend.fire.yaml")).expect("file written");
    assert!(yaml.contains("---\nnamespace:"), "{yaml}");
    assert_eq!(schema_errors(&yaml), Vec::<String>::new());
}

#[test]
fn explicit_yml_extension_is_kept() {
    let project = empty_project("init-yml");
    init(&project, "backend.fire.yml\n\n\n");

    assert!(project.dir.join("backend.fire.yml").exists());
    assert!(!project.dir.join("backend.fire.yaml").exists());
}

#[test]
fn notes_an_existing_file_with_the_other_extension() {
    let project = Project::new("init-sibling", "commands:\n  old: echo old\n");
    let stdout = init(&project, "\n\n\n");

    assert!(stdout.contains("'fire.yml' also exists here"), "{stdout}");
    // both files are loaded
    let output = project.fire(&["old"]).output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let output = project.fire(&["example"]).output().unwrap();
    assert!(output.status.success(), "{output:?}");
}
