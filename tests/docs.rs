//! End-to-end coverage for version-matched embedded documentation (#33).

#![cfg(unix)]

mod common;

use common::*;
use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Stdio};

fn project(name: &str) -> Project {
    Project::new(name, "commands: {}\n")
}

fn source_topic_ids() -> Vec<String> {
    let docs = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs");
    let mut ids: Vec<String> = fs::read_dir(docs)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "md"))
        .map(|path| path.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    ids.push("schema".to_string());
    ids.sort();
    ids
}

#[test]
fn index_is_generated_from_every_source_document() {
    let project = project("docs-index");
    let (code, help, stderr) = project.cli(&[]);
    assert_eq!(code, 0, "{stderr}");
    assert!(help.contains("docs     List or read"), "{help}");

    let (code, stdout, stderr) = project.cli(&["docs"]);
    assert_eq!(code, 0, "{stderr}");

    let rows: Vec<(&str, &str)> = stdout
        .lines()
        .map(|line| line.split_once('\t').expect("topic and summary"))
        .collect();
    assert_eq!(
        rows.iter()
            .map(|(id, _)| id.to_string())
            .collect::<Vec<_>>(),
        source_topic_ids()
    );
    assert!(rows.iter().all(|(_, summary)| !summary.trim().is_empty()));
}

#[test]
fn topic_and_schema_return_the_exact_embedded_sources() {
    let project = project("docs-content");
    let (code, overview, stderr) = project.cli(&["docs", "overview"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(overview, include_str!("../docs/overview.md"));

    let (code, schema, stderr) = project.cli(&["docs", "schema"]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(schema, include_str!("../schemas/fire.schema.json"));
}

#[test]
fn unknown_topic_is_rejected_without_rewriting_it() {
    let project = project("docs-unknown");
    let topic = "  Overview?  ";
    let (code, stdout, stderr) = project.cli(&["docs", topic]);
    assert_eq!(code, 1);
    assert!(stdout.is_empty());
    assert!(stderr.contains(&format!("`{topic}`")), "{stderr}");
    assert!(stderr.contains("fire cli docs"), "{stderr}");
}

#[test]
fn completion_suggests_every_topic_with_its_summary() {
    let project = project("docs-completion");
    let output = project
        .fire(&["__complete", "fire", "cli", "docs", ""])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let rows: Vec<(&str, &str)> = stdout
        .lines()
        .map(|line| line.split_once('\t').expect("completion description"))
        .collect();
    assert_eq!(
        rows.iter()
            .map(|(id, _)| id.to_string())
            .collect::<Vec<_>>(),
        source_topic_ids()
    );
    assert!(rows.iter().all(|(_, summary)| !summary.is_empty()));
}

#[test]
fn terminal_topics_use_pager_as_a_shell_command() {
    let project = project("docs-pager");
    let pager = project.dir.join("pager.sh");
    let marker = project.dir.join("pager-used");
    fs::write(&pager, "#!/bin/sh\nprintf used > \"$1\"\ncat\n").unwrap();
    let mut permissions = fs::metadata(&pager).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&pager, permissions).unwrap();

    let mut command = project.fire(&["cli", "docs", "overview"]);
    command.env("PAGER", format!("{} {}", pager.display(), marker.display()));
    let (mut child, _master, output) = spawn_in_pty(command);
    assert_eq!(wait_for_exit(&mut child), 0);
    assert_eq!(fs::read_to_string(marker).unwrap(), "used");
    assert!(output.lock().unwrap().contains("# Fire CLI — Overview"));
}

#[test]
fn redirected_topics_do_not_start_the_pager() {
    let project = project("docs-no-pager");
    let marker = project.dir.join("pager-used");
    let output = project
        .fire(&["cli", "docs", "overview"])
        .env("PAGER", format!("touch {}", marker.display()))
        .stdout(Stdio::piped())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(!marker.exists());
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("# Fire CLI — Overview"));
}
