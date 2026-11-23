//! End-to-end coverage for external CLI drivers: repository installation, cached builds,
//! exact argument forwarding, project working directories, and delegated completion.

mod common;

use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};

use common::Project;

fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).expect("write executable");
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}

fn driver_repository(project: &Project) -> std::path::PathBuf {
    let repository = project.dir.join("driver-repository");
    fs::create_dir_all(repository.join("src")).unwrap();
    fs::write(
        repository.join("driver.yaml"),
        r#"name: fake-tool
build:
  - mkdir -p bin
  - cp src/complete bin/complete
  - chmod +x bin/complete
handlers:
  completion:
    exec: bin/complete
"#,
    )
    .unwrap();
    write_executable(
        &repository.join("src/complete"),
        r#"#!/bin/sh
printf '%s\n' "$PWD" > completion-cwd.txt
printf '%s\n' "$@" > completion-args.txt
printf '%s\n' '[{"value":"serve","description":"Start server","children":[{"value":"api","description":"API service"}]},{"value":"status","description":"Show status"}]'
"#,
    );
    let status = Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(&repository)
        .status()
        .unwrap();
    assert!(status.success());
    Command::new("git")
        .args(["add", "."])
        .current_dir(&repository)
        .status()
        .unwrap();
    let status = Command::new("git")
        .args([
            "-c",
            "user.name=Fire Test",
            "-c",
            "user.email=fire@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--quiet",
            "-m",
            "test driver",
        ])
        .current_dir(&repository)
        .status()
        .unwrap();
    assert!(status.success());
    repository
}

#[test]
fn installs_executes_and_completes_external_driver() {
    let project = Project::new(
        "drivers",
        r#"commands:
  tool:
    description: Fake external tool
    dir: workspace
    driver: fake-tool
"#,
    );
    fs::create_dir(project.dir.join("workspace")).unwrap();
    let repository = driver_repository(&project);

    let (code, stdout, stderr) = project.cli(&[
        "driver",
        "install",
        repository.to_str().expect("utf-8 repository path"),
    ]);
    assert_eq!(code, 0, "stdout={stdout}\nstderr={stderr}");
    assert!(stdout.contains("Installed driver `fake-tool`"), "{stdout}");

    let bin = project.dir.join("bin");
    fs::create_dir(&bin).unwrap();
    write_executable(
        &bin.join("fake-tool"),
        r#"#!/bin/sh
printf '%s\n' "$PWD"
printf '<%s>\n' "$@"
"#,
    );
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let output = project
        .fire(&["tool", "two words", "--literal=$HOME"])
        .env("PATH", &path)
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout,
        format!(
            "{}\n<two words>\n<--literal=$HOME>\n",
            project
                .dir
                .join("workspace")
                .canonicalize()
                .unwrap()
                .display()
        )
    );

    let output = project
        .fire(&["--dry-run", "tool", "two words", "--literal=$HOME"])
        .env("PATH", &path)
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output);
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[fire] driver   fake-tool 'two words' '--literal=$HOME'"),
        "{stderr}"
    );

    let output = project
        .fire(&["__complete", "fire", "tool", ""])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "serve\tStart server\nstatus\tShow status\n"
    );
    assert_eq!(
        fs::read_to_string(project.dir.join("workspace/completion-cwd.txt")).unwrap(),
        format!(
            "{}\n",
            project
                .dir
                .join("workspace")
                .canonicalize()
                .unwrap()
                .display()
        )
    );
    assert_eq!(
        fs::read_to_string(project.dir.join("workspace/completion-args.txt")).unwrap(),
        "\n"
    );

    let output = project
        .fire(&["__complete", "fire", "tool", "serve", ""])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "api\tAPI service\n"
    );
    assert_eq!(
        fs::read_to_string(project.dir.join("workspace/completion-args.txt")).unwrap(),
        "serve\n"
    );

    let (code, _, stderr) = project.cli(&[
        "driver",
        "install",
        repository.to_str().expect("utf-8 repository path"),
    ]);
    assert_ne!(code, 0);
    assert!(stderr.contains("already installed"), "{stderr}");
}
