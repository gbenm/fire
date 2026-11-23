//! End-to-end tests for importing commands from existing project tools.

#![cfg(unix)]

mod common;

use common::*;
use serde_json::Value;
use std::{fs, io::Write, path::Path};

fn empty_project(name: &str) -> Project {
    let project = Project::new(name, "");
    fs::remove_file(project.dir.join("fire.yml")).unwrap();
    project
}

fn copy_fixture(project: &Project, name: &str) {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/import")
        .join(name);
    fs::copy(source, project.dir.join(name)).unwrap();
}

fn init_with(project: &Project, fixture: &str) -> String {
    copy_fixture(project, fixture);
    let output = project.fire(&["cli", "init", "--yes"]).output().unwrap();
    assert!(output.status.success(), "{output:?}");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn config(project: &Project, name: &str) -> Value {
    let yaml = fs::read_to_string(project.dir.join(name)).unwrap();
    yaml_serde::from_str(&yaml).unwrap()
}

fn assert_dry_run(project: &Project, args: &[&str], expected: &str) {
    let mut command = vec!["--dry-run"];
    command.extend_from_slice(args);
    let output = project.fire(&command).output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(expected), "{stderr}");
}

#[test]
fn init_imports_package_scripts_with_lockfile_package_manager_and_exact_descriptions() {
    let project = empty_project("import-package");
    copy_fixture(&project, "pnpm-lock.yaml");
    let stdout = init_with(&project, "package.json");

    let value = config(&project, "package.fire.yaml");
    assert_eq!(value["group"], "package");
    assert_eq!(
        value["commands"]["build"]["description"],
        "vite build --mode production"
    );
    assert_eq!(value["commands"]["build"]["exec"], "pnpm run build");
    assert_eq!(
        value["commands"]["say'hi"]["description"],
        "printf \"user's exact text: $HOME\""
    );
    assert_eq!(
        value["commands"]["say'hi"]["exec"],
        "pnpm run 'say'\"'\"'hi'"
    );
    assert!(stdout.contains("package.json: 3 commands"), "{stdout}");
    assert_dry_run(&project, &["package", "build"], "pnpm run build");
}

#[test]
fn package_manager_lockfiles_select_npm_yarn_and_bun() {
    for (project_name, lockfile, manager) in [
        ("import-npm-lock", "package-lock.json", "npm"),
        ("import-yarn-lock", "yarn.lock", "yarn"),
        ("import-bun-lock", "bun.lockb", "bun"),
    ] {
        let project = empty_project(project_name);
        fs::write(project.dir.join(lockfile), "fixture\n").unwrap();
        init_with(&project, "package.json");
        let value = config(&project, "package.fire.yaml");
        assert_eq!(
            value["commands"]["build"]["exec"],
            format!("{manager} run build")
        );
    }
}

#[test]
fn init_imports_make_targets_and_both_documented_comment_styles() {
    let project = empty_project("import-make");
    let stdout = init_with(&project, "Makefile");

    let value = config(&project, "make.fire.yaml");
    assert_eq!(
        value["commands"]["build"]["description"],
        "Compile the application exactly"
    );
    assert_eq!(
        value["commands"]["test"]["description"],
        "Run the complete test suite"
    );
    assert!(value["commands"].get(".PHONY").is_none());
    assert!(value["commands"].get("%.o").is_none());
    assert!(stdout.contains("Makefile: 2 commands"), "{stdout}");
    assert_dry_run(&project, &["make", "test"], "make test");
}

#[test]
fn init_imports_just_recipes_and_doc_comments() {
    let project = empty_project("import-just");
    let stdout = init_with(&project, "justfile");

    let value = config(&project, "just.fire.yaml");
    assert_eq!(
        value["commands"]["build"]["description"],
        "Build the release artifact"
    );
    assert_eq!(value["commands"]["test"]["description"], "Run all tests");
    assert!(stdout.contains("justfile: 2 commands"), "{stdout}");
    assert_dry_run(&project, &["just", "test"], "just test");
}

#[test]
fn init_imports_compose_lifecycle_and_exact_service_names() {
    let project = empty_project("import-compose");
    let stdout = init_with(&project, "compose.yml");

    let value = config(&project, "compose.fire.yaml");
    assert_eq!(value["commands"]["up"]["exec"], "docker compose up");
    assert_eq!(value["commands"]["up"]["long_running"], true);
    assert_eq!(
        value["commands"]["exec"]["commands"]["web"]["exec"],
        "docker compose exec web"
    );
    assert_eq!(
        value["commands"]["exec"]["commands"]["dev"]["long_running"],
        true
    );
    assert!(stdout.contains("compose.yml: 5 commands"), "{stdout}");
    assert_dry_run(
        &project,
        &["compose", "exec", "web"],
        "docker compose exec web",
    );
}

#[test]
fn init_imports_standard_cargo_commands() {
    let project = empty_project("import-cargo");
    let stdout = init_with(&project, "Cargo.toml");

    let value = config(&project, "cargo.fire.yaml");
    assert_eq!(value["commands"]["build"]["exec"], "cargo build");
    assert_eq!(value["commands"]["test"]["exec"], "cargo test");
    assert_eq!(value["commands"]["run"]["exec"], "cargo run");
    assert_eq!(value["commands"]["check"]["exec"], "cargo check");
    assert!(stdout.contains("Cargo.toml: 4 commands"), "{stdout}");
    assert_dry_run(&project, &["cargo", "check"], "cargo check");
}

#[test]
fn empty_keeps_minimal_init_behavior() {
    let project = empty_project("import-empty");
    copy_fixture(&project, "package.json");
    let output = project
        .fire(&["cli", "init", "--yes", "--empty"])
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert!(project.dir.join("fire.yaml").exists());
    assert!(!project.dir.join("package.fire.yaml").exists());
}

#[test]
fn import_adds_new_commands_and_preserves_existing_ones() {
    let project = empty_project("import-existing");
    copy_fixture(&project, "package.json");
    fs::write(
        project.dir.join("package.fire.yaml"),
        "group: package\ncommands:\n  build:\n    description: Keep me exactly\n    exec: custom build\n",
    )
    .unwrap();

    let output = project
        .fire(&["cli", "import", "package.json"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Skipped package.json: build"), "{stdout}");

    let value = config(&project, "package.fire.yaml");
    assert_eq!(value["commands"]["build"]["description"], "Keep me exactly");
    assert_eq!(value["commands"]["build"]["exec"], "custom build");
    assert_eq!(value["commands"]["dev"]["exec"], "npm run dev");
}

#[test]
fn malformed_and_unknown_sources_warn_without_failing() {
    let project = empty_project("import-warnings");
    fs::write(project.dir.join("package.json"), "not json\n").unwrap();

    let output = project
        .fire(&["cli", "import", "package.json", "mystery.tasks"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("warning"), "{stderr}");
    assert!(stderr.contains("package.json"), "{stderr}");
    assert!(stderr.contains("mystery.tasks"), "{stderr}");
    assert!(!project.dir.join("package.fire.yaml").exists());
}

#[test]
fn interactive_init_can_decline_detected_imports() {
    let project = empty_project("import-decline");
    copy_fixture(&project, "package.json");
    let (mut child, mut terminal, output) = spawn_in_pty(project.fire(&["cli", "init"]));
    wait_for_output(&output, "Base file name");
    terminal.write_all(b"\n").unwrap();
    wait_for_output(&output, "Namespace prefix");
    terminal.write_all(b"\n").unwrap();
    wait_for_output(&output, "Command group");
    terminal.write_all(b"\n").unwrap();
    wait_for_output(&output, "Import commands from detected project files");
    terminal.write_all(b"n\n").unwrap();

    assert_eq!(wait_for_exit(&mut child), 0, "{}", output.lock().unwrap());
    assert!(project.dir.join("fire.yaml").exists());
    assert!(!project.dir.join("package.fire.yaml").exists());
}
