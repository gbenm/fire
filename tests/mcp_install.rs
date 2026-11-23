//! End-to-end tests for MCP host registration and removal.

#![cfg(unix)]

mod common;

use common::*;
use serde_json::Value;
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

fn project(name: &str) -> Project {
    Project::new(name, "commands: {}\n")
}

fn host_path(project: &Project, host: &str) -> PathBuf {
    let home = project.dir.join("home");
    match host {
        "claude-desktop" if cfg!(target_os = "macos") => {
            home.join("Library/Application Support/Claude/claude_desktop_config.json")
        }
        "claude-desktop" => home.join(".config/Claude/claude_desktop_config.json"),
        "cursor" => home.join(".cursor/mcp.json"),
        "vscode" if cfg!(target_os = "macos") => {
            home.join("Library/Application Support/Code/User/mcp.json")
        }
        "vscode" => home.join(".config/Code/User/mcp.json"),
        "codex" => home.join(".codex/config.toml"),
        "gemini" => home.join(".gemini/settings.json"),
        "windsurf" => home.join(".codeium/windsurf/mcp_config.json"),
        _ => panic!("unknown test host {host}"),
    }
}

fn run_without_host_clis(project: &Project, args: &[&str]) -> std::process::Output {
    let mut command = vec!["cli", "mcp"];
    command.extend_from_slice(args);
    project
        .fire(&command)
        .env("PATH", "/path/with/no/host/clis")
        .output()
        .expect("run mcp installer")
}

fn json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn json_hosts_preserve_unrelated_content_back_up_and_are_idempotent() {
    let project = project("mcp-install-json");
    for host in ["claude-desktop", "cursor", "vscode", "gemini", "windsurf"] {
        let path = host_path(&project, host);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let key = if host == "vscode" {
            "servers"
        } else {
            "mcpServers"
        };
        let fixture = format!(
            "{{\n    \"theme\": \"dark\",\n    \"{key}\": {{\n        \"other\": {{ \"command\": \"keep-me\" }}\n    }}\n}}\n"
        );
        fs::write(&path, &fixture).unwrap();

        let output = run_without_host_clis(&project, &["install", host]);
        assert!(output.status.success(), "{host}: {output:?}");
        let installed = json(&path);
        assert_eq!(installed["theme"], "dark");
        assert_eq!(installed[key]["other"]["command"], "keep-me");
        assert_eq!(
            installed[key]["fire"]["args"],
            serde_json::json!(["cli", "mcp"])
        );
        assert!(Path::new(installed[key]["fire"]["command"].as_str().unwrap()).is_absolute());
        assert_eq!(
            fs::read_to_string(format!("{}.bak", path.display())).unwrap(),
            fixture
        );
        assert!(fs::read_to_string(&path).unwrap().contains("    \"theme\""));

        let once = fs::read_to_string(&path).unwrap();
        let backup_once = fs::read_to_string(format!("{}.bak", path.display())).unwrap();
        let output = run_without_host_clis(&project, &["install", host]);
        assert!(output.status.success(), "{host}: {output:?}");
        assert_eq!(fs::read_to_string(&path).unwrap(), once);
        assert_eq!(
            fs::read_to_string(format!("{}.bak", path.display())).unwrap(),
            backup_once
        );

        let output = run_without_host_clis(&project, &["uninstall", host]);
        assert!(output.status.success(), "{host}: {output:?}");
        let removed = json(&path);
        assert!(removed[key].get("fire").is_none());
        assert_eq!(removed[key]["other"]["command"], "keep-me");
        assert_eq!(removed["theme"], "dark");
    }
}

#[test]
fn codex_toml_preserves_comments_and_unrelated_servers() {
    let project = project("mcp-install-codex");
    let path = host_path(&project, "codex");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let fixture = "# user preference\nmodel = \"gpt-example\"\n\n[mcp_servers.other]\ncommand = \"keep-me\"\nargs = [\"serve\"]\n";
    fs::write(&path, fixture).unwrap();

    let output = run_without_host_clis(&project, &["install", "codex"]);
    assert!(output.status.success(), "{output:?}");
    let installed = fs::read_to_string(&path).unwrap();
    assert!(installed.starts_with(fixture));
    assert!(installed.contains("[mcp_servers.fire]\ncommand = \"/"));
    assert!(installed.contains("args = [\"cli\", \"mcp\"]"));
    assert_eq!(
        fs::read_to_string(format!("{}.bak", path.display())).unwrap(),
        fixture
    );

    let output = run_without_host_clis(&project, &["install", "codex"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(fs::read_to_string(&path).unwrap(), installed);

    let output = run_without_host_clis(&project, &["uninstall", "codex"]);
    assert!(output.status.success(), "{output:?}");
    let removed = fs::read_to_string(&path).unwrap();
    assert!(!removed.contains("mcp_servers.fire"));
    assert!(removed.contains("[mcp_servers.other]\ncommand = \"keep-me\""));
    assert!(removed.contains("# user preference"));
}

#[test]
fn codex_toml_recognizes_quoted_keys_and_semantic_noops() {
    let project = project("mcp-install-codex-quoted");
    let path = host_path(&project, "codex");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let fire = env!("CARGO_BIN_EXE_fire");
    let fixture = format!(
        "# keep every byte on a semantic no-op\n[mcp_servers.\"fire\"]\ncommand = '{fire}' # literal string\nargs = ['cli', 'mcp'] # literal strings\nenabled = true\n"
    );
    fs::write(&path, &fixture).unwrap();

    let output = run_without_host_clis(&project, &["install", "codex"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(fs::read_to_string(&path).unwrap(), fixture);
    assert!(!PathBuf::from(format!("{}.bak", path.display())).exists());
    assert!(String::from_utf8_lossy(&output.stdout).contains("already installed"));
}

#[test]
fn codex_toml_edits_inline_tables_and_rejects_invalid_toml() {
    let project = project("mcp-install-codex-inline");
    let path = host_path(&project, "codex");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        "mcp_servers = { other = { command = \"keep-me\" }, fire = { command = \"old\", args = [\"serve\"], enabled = true } }\n",
    )
    .unwrap();

    let output = run_without_host_clis(&project, &["install", "codex"]);
    assert!(output.status.success(), "{output:?}");
    let installed = fs::read_to_string(&path).unwrap();
    assert!(installed.contains("command = \"keep-me\""), "{installed}");
    assert!(installed.contains("enabled = true"), "{installed}");
    assert!(
        installed.contains("args = [\"cli\", \"mcp\"]"),
        "{installed}"
    );

    let output = run_without_host_clis(&project, &["uninstall", "codex"]);
    assert!(output.status.success(), "{output:?}");
    let removed = fs::read_to_string(&path).unwrap();
    assert!(removed.contains("command = \"keep-me\""), "{removed}");
    assert!(!removed.contains("enabled = true"), "{removed}");

    fs::write(&path, "model = [\n").unwrap();
    let output = run_without_host_clis(&project, &["install", "codex"]);
    assert!(!output.status.success(), "{output:?}");
    assert_eq!(fs::read_to_string(&path).unwrap(), "model = [\n");
    assert!(String::from_utf8_lossy(&output.stderr).contains("Failed to parse Codex config"));
}

#[test]
fn json_install_preserves_extra_fire_settings_and_semantic_noops() {
    let project = project("mcp-install-json-fire-settings");
    let path = host_path(&project, "cursor");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let fire = env!("CARGO_BIN_EXE_fire");
    let fixture = format!(
        "{{\n  \"mcpServers\": {{\n    \"fire\": {{\n      \"command\": {fire:?},\n      \"args\": [\"cli\", \"mcp\"],\n      \"env\": {{\"KEEP\": \"yes\"}}\n    }}\n  }}\n}}\n"
    );
    fs::write(&path, &fixture).unwrap();

    let output = run_without_host_clis(&project, &["install", "cursor"]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(fs::read_to_string(&path).unwrap(), fixture);
    assert!(!PathBuf::from(format!("{}.bak", path.display())).exists());

    let mut changed: Value = serde_json::from_str(&fixture).unwrap();
    changed["mcpServers"]["fire"]["command"] = Value::String("old".to_string());
    fs::write(
        &path,
        serde_json::to_string_pretty(&changed).unwrap() + "\n",
    )
    .unwrap();
    let output = run_without_host_clis(&project, &["install", "cursor"]);
    assert!(output.status.success(), "{output:?}");
    let installed = json(&path);
    assert_eq!(installed["mcpServers"]["fire"]["env"]["KEEP"], "yes");
    assert_eq!(installed["mcpServers"]["fire"]["command"], fire);
}

#[test]
fn print_shows_file_change_without_writing_it() {
    let project = project("mcp-install-print");
    let path = host_path(&project, "cursor");
    let output = run_without_host_clis(&project, &["install", "cursor", "--print"]);
    assert!(output.status.success(), "{output:?}");
    assert!(!path.exists());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Would write"), "{stdout}");
    assert!(stdout.contains("\"fire\""), "{stdout}");
}

#[test]
fn installed_host_clis_are_preferred_and_receive_exact_arguments() {
    let project = project("mcp-install-clis");
    let bin = project.dir.join("host-bin");
    fs::create_dir_all(&bin).unwrap();
    for host in ["claude", "code", "codex"] {
        let record = project.dir.join(format!("{host}.args"));
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n",
            record.display()
        );
        let executable = bin.join(host);
        fs::write(&executable, script).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
    }

    for (requested, executable) in [("claude", "claude"), ("vscode", "code"), ("codex", "codex")] {
        let output = project
            .fire(&["cli", "mcp", "install", requested])
            .env("PATH", &bin)
            .output()
            .unwrap();
        assert!(output.status.success(), "{requested}: {output:?}");
        let args = fs::read_to_string(project.dir.join(format!("{executable}.args"))).unwrap();
        if requested == "vscode" {
            assert!(args.contains("\"name\":\"fire\""), "{args}");
            assert!(args.contains("\"args\":[\"cli\",\"mcp\"]"), "{args}");
            assert!(!host_path(&project, "vscode").exists());
        } else {
            assert!(args.contains("fire\n"), "{requested}: {args}");
            assert!(args.contains("cli\nmcp\n"), "{requested}: {args}");
            if requested == "codex" {
                assert!(!host_path(&project, "codex").exists());
            }
        }
    }

    for (requested, executable, expected) in [
        ("claude", "claude", "mcp\nremove\n--scope\nuser\nfire\n"),
        ("codex", "codex", "mcp\nremove\nfire\n"),
    ] {
        let output = project
            .fire(&["cli", "mcp", "uninstall", requested])
            .env("PATH", &bin)
            .output()
            .unwrap();
        assert!(output.status.success(), "{requested}: {output:?}");
        assert_eq!(
            fs::read_to_string(project.dir.join(format!("{executable}.args"))).unwrap(),
            expected
        );
    }

    let codex_record = project.dir.join("codex.args");
    fs::remove_file(&codex_record).unwrap();
    let output = project
        .fire(&["cli", "mcp", "install", "codex", "--print"])
        .env("PATH", &bin)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(!codex_record.exists());
    assert!(String::from_utf8_lossy(&output.stdout).contains("Would run codex with arguments:"));
}

#[test]
fn config_uses_literal_fire_only_when_it_is_on_path() {
    let project = project("mcp-install-fire-path");
    let bin = project.dir.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let fire = bin.join("fire");
    fs::write(&fire, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&fire, fs::Permissions::from_mode(0o755)).unwrap();

    let output = project
        .fire(&["cli", "mcp", "install", "cursor"])
        .env("PATH", &bin)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        json(&host_path(&project, "cursor"))["mcpServers"]["fire"]["command"],
        "fire"
    );
}

#[test]
fn claude_without_its_cli_prints_the_exact_manual_command() {
    let project = project("mcp-install-claude-manual");
    let output = run_without_host_clis(&project, &["install", "claude"]);
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Claude Code CLI was not found"), "{stdout}");
    assert!(
        stdout.contains("Would run claude with arguments:"),
        "{stdout}"
    );
    assert!(
        stdout.contains("\"--scope\",\"user\",\"fire\",\"--\""),
        "{stdout}"
    );
}

#[test]
fn no_host_without_a_tty_lists_detection_and_exits_nonzero() {
    let project = project("mcp-install-detect-nontty");
    fs::create_dir_all(host_path(&project, "cursor").parent().unwrap()).unwrap();
    let output = run_without_host_clis(&project, &["install"]);
    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Detected MCP hosts:") && stderr.contains("cursor"),
        "{stderr}"
    );
    assert!(stderr.contains("Choose a host explicitly"), "{stderr}");
}

#[test]
fn no_host_with_a_tty_installs_exact_selected_names() {
    let project = project("mcp-install-detect-tty");
    fs::create_dir_all(host_path(&project, "cursor").parent().unwrap()).unwrap();
    fs::create_dir_all(host_path(&project, "gemini").parent().unwrap()).unwrap();
    let mut command = project.fire(&["cli", "mcp", "install"]);
    command.env("PATH", "/path/with/no/host/clis");
    let (mut child, mut terminal, output) = spawn_in_pty(command);
    wait_for_output(&output, "Hosts to install");
    terminal.write_all(b"cursor,gemini\n").unwrap();
    assert_eq!(wait_for_exit(&mut child), 0, "{}", output.lock().unwrap());
    assert!(host_path(&project, "cursor").exists());
    assert!(host_path(&project, "gemini").exists());
}

#[test]
fn detected_host_selection_does_not_trim_or_guess_names() {
    let project = project("mcp-install-detect-exact-input");
    fs::create_dir_all(host_path(&project, "cursor").parent().unwrap()).unwrap();
    let mut command = project.fire(&["cli", "mcp", "install"]);
    command.env("PATH", "/path/with/no/host/clis");
    let (mut child, mut terminal, output) = spawn_in_pty(command);
    wait_for_output(&output, "Hosts to install");
    terminal.write_all(b" cursor\n").unwrap();

    assert_eq!(wait_for_exit(&mut child), 1, "{}", output.lock().unwrap());
    let terminal_output = output.lock().unwrap();
    assert!(
        terminal_output.contains("Unknown MCP host ` cursor`"),
        "{terminal_output}"
    );
    assert!(!host_path(&project, "cursor").exists());
}
