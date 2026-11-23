mod common;

use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Stdio};

use common::Project;

fn run(project: &Project, shell: &str) -> (i32, String, String) {
    project.cli(&["completion", "install", shell])
}

fn complete_targets(project: &Project) -> Vec<String> {
    let output = project
        .fire(&["__complete", "fire", "cli", "completion", "install", ""])
        .stdin(Stdio::null())
        .output()
        .expect("complete install targets");
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn completion_targets_include_fish_and_nushell_with_descriptions() {
    let project = Project::new("completion-targets", "commands: {}\n");
    let targets = complete_targets(&project);

    assert!(targets
        .iter()
        .any(|line| line == "fish\tInstall fish completion"));
    assert!(targets
        .iter()
        .any(|line| line == "nushell\tInstall Nushell completion"));
    assert!(targets
        .iter()
        .any(|line| line == "all\tInstall completion for detected shells"));
}

#[test]
fn fish_install_is_rich_and_idempotent() {
    let project = Project::new("completion-fish", "commands: {}\n");
    let (code, stdout, stderr) = run(&project, "fish");
    assert_eq!(code, 0, "{stderr}");

    let path = project.dir.join("home/.config/fish/completions/fire.fish");
    assert!(stdout.contains(&path.display().to_string()), "{stdout}");
    let first = fs::read_to_string(&path).expect("fish completion script");
    assert!(first.contains("commandline -opc"), "{first}");
    assert!(first.contains("fire __complete --"), "{first}");
    assert!(first.contains("complete -c fire"), "{first}");

    let (code, _, stderr) = run(&project, "fish");
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(fs::read_to_string(path).unwrap(), first);
}

#[test]
fn nushell_install_manages_a_rich_external_completer() {
    let project = Project::new("completion-nushell", "commands: {}\n");
    let config = project.dir.join("home/.config/nushell/config.nu");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(&config, "$env.config.show_banner = false\n").unwrap();

    let (code, stdout, stderr) = run(&project, "nushell");
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains(&config.display().to_string()), "{stdout}");
    let first = fs::read_to_string(&config).expect("Nushell config");
    assert!(first.contains("$env.config.show_banner = false"), "{first}");
    assert!(
        first.contains("# >>> fire completion (nushell) >>>"),
        "{first}"
    );
    assert!(first.contains("$place.command"), "{first}");
    assert!(first.contains("fire __complete --"), "{first}");
    assert!(first.contains("value:"), "{first}");
    assert!(first.contains("description:"), "{first}");

    let (code, _, stderr) = run(&project, "nushell");
    assert_eq!(code, 0, "{stderr}");
    let second = fs::read_to_string(config).unwrap();
    assert_eq!(second, first);
    assert_eq!(
        second
            .matches("# >>> fire completion (nushell) >>>")
            .count(),
        1
    );
}

#[test]
fn all_installs_fish_and_nushell_when_the_shells_are_available() {
    let project = Project::new("completion-all", "commands: {}\n");
    let bin = project.dir.join("bin");
    fs::create_dir_all(&bin).unwrap();
    fake_executable(&bin.join("fish"));
    fake_executable(&bin.join("nu"));

    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let output = project
        .fire(&["cli", "completion", "install", "all"])
        .env("PATH", path)
        .stdin(Stdio::null())
        .output()
        .expect("install all completion scripts");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(project
        .dir
        .join("home/.config/fish/completions/fire.fish")
        .is_file());
    assert!(project.dir.join("home/.config/nushell/config.nu").is_file());
}

#[test]
fn invalid_target_lists_every_supported_shell() {
    let project = Project::new("completion-invalid", "commands: {}\n");
    let (code, _, stderr) = run(&project, "powershell");
    assert_eq!(code, 1);
    assert!(stderr.contains("bash, zsh, fish, nushell, all"), "{stderr}");
}

fn fake_executable(path: &Path) {
    fs::write(path, "#!/bin/sh\nexit 0\n").unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).unwrap();
}
