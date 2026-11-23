//! End-to-end fixtures for `fire cli check` / `validate` (#56).

mod common;

use std::fs;

use common::Project;

fn check(project: &Project, args: &[&str]) -> (i32, String, String) {
    project.cli(&[&["check"], args].concat())
}

#[test]
fn clean_config_is_silent_and_validate_is_an_alias() {
    let project = Project::new("check-clean", "commands:\n  hello: echo hello\n");
    assert_eq!(check(&project, &[]), (0, String::new(), String::new()));
    assert_eq!(
        project.cli(&["validate"]),
        (0, String::new(), String::new())
    );
}

#[test]
fn schema_errors_report_the_source_line_and_a_hint() {
    let project = Project::new(
        "check-schema",
        "commands:\n  hello:\n    exac: echo hello\n",
    );
    let (code, stdout, stderr) = check(&project, &[]);
    assert_eq!(code, 1, "{stdout}{stderr}");
    assert!(stdout.contains("fire.yml:3: error:"), "{stdout}");
    assert!(
        stdout.contains("exac") && stdout.contains("Hint:"),
        "{stdout}"
    );
}

#[test]
fn command_collisions_between_files_are_errors() {
    let project = Project::new("check-collision", "commands:\n  test: cargo test\n");
    fs::write(
        project.dir.join("other.fire.yml"),
        "group: null\ncommands:\n  test: npm test\n",
    )
    .unwrap();
    let (code, stdout, _) = check(&project, &[]);
    assert_eq!(code, 1, "{stdout}");
    assert!(
        stdout.contains("collision") && stdout.contains("`test`"),
        "{stdout}"
    );
    assert!(
        stdout.contains("fire.yml") && stdout.contains("other.fire.yml"),
        "{stdout}"
    );
}

#[test]
fn duplicate_groups_and_group_command_name_collisions_are_errors() {
    let project = Project::new(
        "check-scope-collision",
        "commands:\n  tools:\n    commands:\n      local: echo local\n",
    );
    fs::write(
        project.dir.join("one.fire.yml"),
        "group: tools\ncommands:\n  one: echo one\n",
    )
    .unwrap();
    fs::write(
        project.dir.join("two.fire.yml"),
        "group: tools\ncommands:\n  two: echo two\n",
    )
    .unwrap();
    let (code, stdout, _) = check(&project, &[]);
    assert_eq!(code, 1, "{stdout}");
    assert!(
        stdout.contains("Name collision") && stdout.contains("`tools`"),
        "{stdout}"
    );
    assert!(stdout.contains("command/group"), "{stdout}");
}

#[test]
fn missing_include_is_an_error_and_missing_explicit_env_file_is_strict_warning() {
    let project = Project::new(
        "check-paths",
        "include: [missing-commands]\nenv_file: missing.env\ncommands:\n  hello: echo hello\n",
    );
    let (code, stdout, _) = check(&project, &[]);
    assert_eq!(code, 1, "{stdout}");
    assert!(
        stdout.contains("include") && stdout.contains("missing-commands"),
        "{stdout}"
    );
    assert!(
        stdout.contains("warning:") && stdout.contains("missing.env"),
        "{stdout}"
    );

    fs::create_dir(project.dir.join("missing-commands")).unwrap();
    let (code, stdout, _) = check(&project, &[]);
    assert_eq!(code, 0, "{stdout}");
    let (code, stdout, _) = check(&project, &["--strict"]);
    assert_eq!(code, 1, "{stdout}");
    assert!(
        stdout.contains("error:") && stdout.contains("missing.env"),
        "{stdout}"
    );
}

#[test]
fn impossible_placeholders_and_unused_compute_keys_warn() {
    let project = Project::new(
        "check-placeholders",
        "commands:\n  hello:\n    placeholder: ARG\n    compute:\n      \"{unused}\": echo value\n    exec: echo ARG\n",
    );
    let (code, stdout, _) = check(&project, &[]);
    assert_eq!(code, 0, "{stdout}");
    assert!(
        stdout.contains("placeholder") && stdout.contains("`{n}`"),
        "{stdout}"
    );
    assert!(
        stdout.contains("compute key `{unused}` is never used"),
        "{stdout}"
    );
}

#[test]
fn missing_runtime_prefixes_are_errors_for_eval_and_compute() {
    let project = Project::new(
        "check-runtimes",
        "commands:\n  evaluated:\n    eval: py:print('hi')\n  computed:\n    compute:\n      \"{value}\": node:process.version\n    exec: echo \"{value}\"\n",
    );
    let (code, stdout, _) = check(&project, &[]);
    assert_eq!(code, 1, "{stdout}");
    assert!(
        stdout.contains("Runtime `py` is not configured"),
        "{stdout}"
    );
    assert!(
        stdout.contains("Runtime `node` is not configured"),
        "{stdout}"
    );
}

#[test]
fn dependency_cycles_and_unknown_targets_are_errors() {
    let project = Project::new(
        "check-needs",
        "commands:\n  a:\n    needs: [b]\n    exec: echo a\n  b:\n    needs: [a]\n    exec: echo b\n  c:\n    needs: [missing]\n    exec: echo c\n",
    );
    let (code, stdout, _) = check(&project, &[]);
    assert_eq!(code, 1, "{stdout}");
    assert!(stdout.contains("Dependency cycle"), "{stdout}");
    assert!(
        stdout.contains("needs `missing`") && stdout.contains("does not exist"),
        "{stdout}"
    );
}

#[test]
fn commands_without_an_action_or_subcommands_are_errors() {
    let project = Project::new(
        "check-empty",
        "commands:\n  empty:\n    description: Does nothing\n    commands: {}\n",
    );
    let (code, stdout, _) = check(&project, &[]);
    assert_eq!(code, 1, "{stdout}");
    assert!(
        stdout.contains("neither `exec`, `eval`, nor subcommands"),
        "{stdout}"
    );
}

#[test]
fn json_format_is_machine_readable() {
    let project = Project::new(
        "check-json",
        "commands:\n  hello:\n    placeholder: never\n    exec: echo hello\n",
    );
    let (code, stdout, stderr) = check(&project, &["--format", "json"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(stderr.is_empty(), "{stderr}");
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(value[0]["severity"], "warning");
    assert_eq!(value[0]["line"], 3);
    assert!(value[0]["file"].as_str().unwrap().ends_with("fire.yml"));
}

#[test]
fn global_configs_are_checked_only_with_global_flag() {
    let project = Project::new("check-global", "commands:\n  hello: echo hello\n");
    let global = project.dir.join("global");
    fs::create_dir(&global).unwrap();
    fs::write(
        global.join("fire.yml"),
        "commands:\n  broken:\n    eval: py:print('hi')\n",
    )
    .unwrap();
    let path = global.to_string_lossy().to_string();
    assert_eq!(project.cli(&["global", "add", &path]).0, 0);
    assert_eq!(check(&project, &[]).0, 0);
    let (code, stdout, _) = check(&project, &["--global"]);
    assert_eq!(code, 1, "{stdout}");
    assert!(
        stdout.contains("global/fire.yml") && stdout.contains("Runtime `py`"),
        "{stdout}"
    );
}
