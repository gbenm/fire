use std::{
    env, fs,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    process::{self, Command},
    str::FromStr,
};

use serde::Serialize;
use serde_json::{json, Map, Value};
use toml_edit::{value as toml_value, Array as TomlArray, DocumentMut, InlineTable, Item, Table};

const HOSTS: &[Host] = &[
    Host::Claude,
    Host::ClaudeDesktop,
    Host::Cursor,
    Host::VsCode,
    Host::Codex,
    Host::Gemini,
    Host::Windsurf,
];

pub(crate) fn supported_hosts() -> impl Iterator<Item = (&'static str, &'static str)> {
    HOSTS.iter().map(|host| (host.name(), host.description()))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Host {
    Claude,
    ClaudeDesktop,
    Cursor,
    VsCode,
    Codex,
    Gemini,
    Windsurf,
}

impl Host {
    fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::ClaudeDesktop => "claude-desktop",
            Self::Cursor => "cursor",
            Self::VsCode => "vscode",
            Self::Codex => "codex",
            Self::Gemini => "gemini",
            Self::Windsurf => "windsurf",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::ClaudeDesktop => "Claude Desktop",
            Self::Cursor => "Cursor",
            Self::VsCode => "VS Code (Copilot)",
            Self::Codex => "Codex",
            Self::Gemini => "Gemini CLI",
            Self::Windsurf => "Windsurf",
        }
    }

    fn cli(self) -> Option<&'static str> {
        match self {
            Self::Claude => Some("claude"),
            Self::VsCode => Some("code"),
            Self::Codex => Some("codex"),
            _ => None,
        }
    }
}

impl FromStr for Host {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        HOSTS
            .iter()
            .copied()
            .find(|host| host.name() == value)
            .ok_or(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Action {
    Install,
    Uninstall,
}

#[derive(Clone, Copy)]
enum Platform {
    MacOs,
    Linux,
}

pub(crate) fn handle(args: &[String]) {
    let (action, host, print_only) = parse_args(args).unwrap_or_else(|message| fail(&message));
    let hosts = match host {
        Some(host) => vec![host],
        None if action == Action::Install => select_detected_hosts(),
        None => fail("`uninstall` requires a host"),
    };

    let fire = fire_command().unwrap_or_else(|message| fail(&message));
    for host in hosts {
        apply(action, host, &fire, print_only).unwrap_or_else(|message| fail(&message));
    }
}

fn parse_args(args: &[String]) -> Result<(Action, Option<Host>, bool), String> {
    let Some(operation) = args.first() else {
        return Err(usage());
    };
    let action = match operation.as_str() {
        "install" => Action::Install,
        "uninstall" => Action::Uninstall,
        _ => return Err(usage()),
    };
    let mut host = None;
    let mut print_only = false;
    for argument in &args[1..] {
        if argument == "--print" {
            if print_only {
                return Err("`--print` was provided more than once".to_string());
            }
            print_only = true;
        } else if host.is_none() {
            host = Some(argument.parse().map_err(|()| {
                format!(
                    "Unknown MCP host `{argument}`. Use one of: {}.",
                    host_names()
                )
            })?);
        } else {
            return Err(usage());
        }
    }
    Ok((action, host, print_only))
}

fn usage() -> String {
    format!(
        "Usage: fire cli mcp install [<host>] [--print]\n       fire cli mcp uninstall <host> [--print]\nHosts: {}",
        host_names()
    )
}

fn host_names() -> String {
    HOSTS
        .iter()
        .map(|host| host.name())
        .collect::<Vec<_>>()
        .join(", ")
}

fn select_detected_hosts() -> Vec<Host> {
    let home = dirs::home_dir().unwrap_or_else(|| fail("Could not determine the home directory"));
    let platform = current_platform();
    let detected: Vec<Host> = HOSTS
        .iter()
        .copied()
        .filter(|host| host_is_installed(*host, &home, platform))
        .collect();

    if detected.is_empty() {
        fail(&format!(
            "No supported MCP hosts were detected. Specify one explicitly: {}.",
            host_names()
        ));
    }
    if !io::stdin().is_terminal() {
        eprintln!(
            "[fire] Detected MCP hosts: {}",
            detected
                .iter()
                .map(|host| host.name())
                .collect::<Vec<_>>()
                .join(", ")
        );
        fail("Choose a host explicitly when standard input is not a terminal");
    }

    println!("Detected MCP hosts:");
    for host in &detected {
        println!("  {}\t{}", host.name(), host.description());
    }
    print!("Hosts to install (comma-separated names, or `all`): ");
    io::stdout()
        .flush()
        .unwrap_or_else(|err| fail(&err.to_string()));
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .unwrap_or_else(|err| fail(&format!("Failed to read host selection: {err}")));
    let answer = answer.trim_end_matches(['\r', '\n']);
    if answer == "all" {
        return detected;
    }
    if answer.is_empty() {
        fail("No hosts selected");
    }
    answer
        .split(',')
        .map(|name| {
            let host: Host = name.parse().unwrap_or_else(|()| {
                fail(&format!(
                    "Unknown MCP host `{name}`. Use an exact detected name."
                ))
            });
            if !detected.contains(&host) {
                fail(&format!("MCP host `{name}` was not detected"));
            }
            host
        })
        .collect()
}

fn apply(action: Action, host: Host, fire: &Path, print_only: bool) -> Result<(), String> {
    if let Some(cli) = host.cli().filter(|cli| executable_on_path(cli).is_some()) {
        if action == Action::Install || host != Host::VsCode {
            return apply_cli(action, host, cli, fire, print_only);
        }
    }
    if host == Host::Claude {
        println!("Claude Code CLI was not found; run this command after installing it:");
        return apply_cli(action, host, "claude", fire, true);
    }

    let home =
        dirs::home_dir().ok_or_else(|| "Could not determine the home directory".to_string())?;
    let path = config_path(host, &home, current_platform());
    let changed = match host {
        Host::Codex => edit_toml(&path, action, fire, print_only)?,
        _ => edit_json(&path, config_key(host), action, fire, print_only)?,
    };
    if changed {
        if !print_only {
            println!(
                "{} MCP server for {} in {}",
                past_tense(action),
                host.description(),
                path.display()
            );
        }
    } else {
        println!(
            "{} is already {} for {}",
            host.description(),
            desired_state(action),
            host.name()
        );
    }
    Ok(())
}

fn apply_cli(
    action: Action,
    host: Host,
    cli: &str,
    fire: &Path,
    print_only: bool,
) -> Result<(), String> {
    let fire = fire.to_string_lossy().into_owned();
    let args: Vec<String> = match (host, action) {
        (Host::Claude, Action::Install) => ["mcp", "add", "--scope", "user", "fire", "--"]
            .into_iter()
            .map(str::to_string)
            .chain([fire, "cli".into(), "mcp".into()])
            .collect(),
        (Host::Claude, Action::Uninstall) => ["mcp", "remove", "--scope", "user", "fire"]
            .into_iter()
            .map(str::to_string)
            .collect(),
        (Host::VsCode, Action::Install) => vec![
            "--add-mcp".into(),
            json!({"name":"fire", "command":fire, "args":["cli", "mcp"]}).to_string(),
        ],
        (Host::Codex, Action::Install) => ["mcp", "add", "fire", "--"]
            .into_iter()
            .map(str::to_string)
            .chain([fire, "cli".into(), "mcp".into()])
            .collect(),
        (Host::Codex, Action::Uninstall) => ["mcp", "remove", "fire"]
            .into_iter()
            .map(str::to_string)
            .collect(),
        (Host::VsCode, Action::Uninstall) => unreachable!("VS Code uninstall edits its config"),
        _ => unreachable!("file-only host routed to CLI"),
    };
    if print_only {
        println!(
            "Would run {} with arguments: {}",
            cli,
            serde_json::to_string(&args).unwrap()
        );
        return Ok(());
    }
    let status = Command::new(cli)
        .args(&args)
        .status()
        .map_err(|err| format!("Failed to run `{cli}`: {err}"))?;
    if !status.success() {
        return Err(format!("`{cli}` exited with status {status}"));
    }
    println!(
        "{} MCP server for {}",
        past_tense(action),
        host.description()
    );
    Ok(())
}

fn edit_json(
    path: &Path,
    key: &str,
    action: Action,
    fire: &Path,
    print_only: bool,
) -> Result<bool, String> {
    let original = read_optional(path)?;
    let mut document: Value = match &original {
        Some(content) => serde_json::from_str(content)
            .map_err(|err| format!("Failed to parse {}: {err}", path.display()))?,
        None => Value::Object(Map::new()),
    };
    let root = document
        .as_object_mut()
        .ok_or_else(|| format!("{} must contain a JSON object", path.display()))?;
    let servers = match root.entry(key) {
        serde_json::map::Entry::Vacant(entry) if action == Action::Install => entry
            .insert(Value::Object(Map::new()))
            .as_object_mut()
            .unwrap(),
        serde_json::map::Entry::Vacant(_) => return Ok(false),
        serde_json::map::Entry::Occupied(entry) => entry
            .into_mut()
            .as_object_mut()
            .ok_or_else(|| format!("`{key}` in {} must be a JSON object", path.display()))?,
    };
    let changed = match action {
        Action::Install => match servers.entry("fire") {
            serde_json::map::Entry::Vacant(entry) => {
                entry.insert(json!({
                    "command": fire.to_string_lossy(),
                    "args": ["cli", "mcp"]
                }));
                true
            }
            serde_json::map::Entry::Occupied(mut entry) => {
                let wanted_command = Value::String(fire.to_string_lossy().into_owned());
                let wanted_args = json!(["cli", "mcp"]);
                if let Some(config) = entry.get_mut().as_object_mut() {
                    if config.get("command") == Some(&wanted_command)
                        && config.get("args") == Some(&wanted_args)
                    {
                        false
                    } else {
                        config.insert("command".to_string(), wanted_command);
                        config.insert("args".to_string(), wanted_args);
                        true
                    }
                } else {
                    entry.insert(json!({
                        "command": fire.to_string_lossy(),
                        "args": ["cli", "mcp"]
                    }));
                    true
                }
            }
        },
        Action::Uninstall => servers.remove("fire").is_some(),
    };
    if !changed {
        return Ok(false);
    }
    let rendered = render_json(&document, original.as_deref())?;
    write_change(path, original.as_deref(), &rendered, print_only)?;
    Ok(true)
}

fn render_json(document: &Value, original: Option<&str>) -> Result<String, String> {
    let trailing_newline = original.is_none_or(|content| content.ends_with('\n'));
    let compact = original.is_some_and(|content| !content.contains('\n'));
    let mut rendered = if compact {
        serde_json::to_string(document)
    } else {
        let indent = original
            .and_then(|content| content.lines().find(|line| line.starts_with([' ', '\t'])))
            .map(|line| &line.as_bytes()[..line.len() - line.trim_start().len()])
            .unwrap_or(b"  ");
        let formatter = serde_json::ser::PrettyFormatter::with_indent(indent);
        let mut bytes = Vec::new();
        let mut serializer = serde_json::Serializer::with_formatter(&mut bytes, formatter);
        document
            .serialize(&mut serializer)
            .map(|()| String::from_utf8(bytes).unwrap())
    }
    .map_err(|err| format!("Failed to serialize MCP config: {err}"))?;
    if trailing_newline {
        rendered.push('\n');
    }
    Ok(rendered)
}

fn edit_toml(path: &Path, action: Action, fire: &Path, print_only: bool) -> Result<bool, String> {
    let original = read_optional(path)?;
    let content = original.as_deref().unwrap_or("");
    let (rendered, changed) = edit_toml_text(content, action, fire)?;
    if !changed {
        return Ok(false);
    }
    write_change(path, original.as_deref(), &rendered, print_only)?;
    Ok(true)
}

fn edit_toml_text(content: &str, action: Action, fire: &Path) -> Result<(String, bool), String> {
    let mut document = content
        .parse::<DocumentMut>()
        .map_err(|err| format!("Failed to parse Codex config: {err}"))?;

    if action == Action::Uninstall {
        let Some(servers) = document.get_mut("mcp_servers") else {
            return Ok((content.to_string(), false));
        };
        let servers = servers
            .as_table_like_mut()
            .ok_or_else(|| "`mcp_servers` in Codex config must be a table".to_string())?;
        if servers.remove("fire").is_none() {
            return Ok((content.to_string(), false));
        }
        return Ok((document.to_string(), true));
    }

    if document.get("mcp_servers").is_none() {
        document.insert("mcp_servers", Item::Table(Table::new()));
    }
    let servers = document
        .get_mut("mcp_servers")
        .expect("mcp_servers was inserted");
    let servers_is_inline = servers.is_inline_table();
    let servers = servers
        .as_table_like_mut()
        .ok_or_else(|| "`mcp_servers` in Codex config must be a table".to_string())?;
    let fire_command = fire.to_string_lossy();

    if let Some(config) = servers.get_mut("fire") {
        let config = config
            .as_table_like_mut()
            .ok_or_else(|| "`mcp_servers.fire` in Codex config must be a table".to_string())?;
        if toml_fire_config_matches(config, &fire_command) {
            return Ok((content.to_string(), false));
        }
        set_toml_fire_values(config, &fire_command);
    } else if servers_is_inline {
        let mut config = InlineTable::new();
        set_toml_fire_values(&mut config, &fire_command);
        servers.insert("fire", Item::Value(config.into()));
    } else {
        let mut config = Table::new();
        set_toml_fire_values(&mut config, &fire_command);
        servers.insert("fire", Item::Table(config));
    }

    Ok((document.to_string(), true))
}

fn toml_fire_config_matches(config: &dyn toml_edit::TableLike, fire: &str) -> bool {
    let args = config
        .get("args")
        .and_then(Item::as_array)
        .map(|args| {
            args.len() == 2
                && args.get(0).and_then(toml_edit::Value::as_str) == Some("cli")
                && args.get(1).and_then(toml_edit::Value::as_str) == Some("mcp")
        })
        .unwrap_or(false);
    config.get("command").and_then(Item::as_str) == Some(fire) && args
}

fn set_toml_fire_values(config: &mut dyn toml_edit::TableLike, fire: &str) {
    let mut args = TomlArray::new();
    args.push("cli");
    args.push("mcp");
    config.insert("command", toml_value(fire));
    config.insert("args", toml_value(args));
}

fn write_change(
    path: &Path,
    original: Option<&str>,
    rendered: &str,
    print_only: bool,
) -> Result<(), String> {
    if print_only {
        println!("Would write {}:\n{}", path.display(), rendered);
        return Ok(());
    }
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|err| format!("Failed to create {}: {err}", parent.display()))?;
    if original.is_some() {
        let backup = backup_path(path);
        fs::copy(path, &backup).map_err(|err| {
            format!(
                "Failed to back up {} to {}: {err}",
                path.display(),
                backup.display()
            )
        })?;
    }
    fs::write(path, rendered).map_err(|err| format!("Failed to write {}: {err}", path.display()))
}

fn read_optional(path: &Path) -> Result<Option<String>, String> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(Some(content)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!("Failed to read {}: {err}", path.display())),
    }
}

fn backup_path(path: &Path) -> PathBuf {
    let mut backup = path.as_os_str().to_os_string();
    backup.push(".bak");
    PathBuf::from(backup)
}

fn config_key(host: Host) -> &'static str {
    if host == Host::VsCode {
        "servers"
    } else {
        "mcpServers"
    }
}

fn config_path(host: Host, home: &Path, platform: Platform) -> PathBuf {
    match host {
        Host::Claude => unreachable!("Claude Code always uses its CLI"),
        Host::ClaudeDesktop => match platform {
            Platform::MacOs => {
                home.join("Library/Application Support/Claude/claude_desktop_config.json")
            }
            Platform::Linux => xdg_config_home(home).join("Claude/claude_desktop_config.json"),
        },
        Host::Cursor => home.join(".cursor/mcp.json"),
        Host::VsCode => match platform {
            Platform::MacOs => home.join("Library/Application Support/Code/User/mcp.json"),
            Platform::Linux => xdg_config_home(home).join("Code/User/mcp.json"),
        },
        Host::Codex => home.join(".codex/config.toml"),
        Host::Gemini => home.join(".gemini/settings.json"),
        Host::Windsurf => home.join(".codeium/windsurf/mcp_config.json"),
    }
}

fn xdg_config_home(home: &Path) -> PathBuf {
    env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"))
}

fn host_is_installed(host: Host, home: &Path, platform: Platform) -> bool {
    host.cli()
        .is_some_and(|cli| executable_on_path(cli).is_some())
        || detection_executable(host).is_some_and(|cli| executable_on_path(cli).is_some())
        || (matches!(platform, Platform::MacOs)
            && macos_application(host, home).is_some_and(|path| path.exists()))
        || (host != Host::Claude
            && config_path(host, home, platform)
                .parent()
                .is_some_and(Path::exists))
}

fn detection_executable(host: Host) -> Option<&'static str> {
    match host {
        Host::ClaudeDesktop => Some("claude-desktop"),
        Host::Cursor => Some("cursor"),
        Host::Gemini => Some("gemini"),
        Host::Windsurf => Some("windsurf"),
        _ => None,
    }
}

fn macos_application(host: Host, home: &Path) -> Option<PathBuf> {
    let name = match host {
        Host::ClaudeDesktop => "Claude.app",
        Host::Cursor => "Cursor.app",
        Host::VsCode => "Visual Studio Code.app",
        Host::Windsurf => "Windsurf.app",
        _ => return None,
    };
    let system = Path::new("/Applications").join(name);
    if system.exists() {
        Some(system)
    } else {
        Some(home.join("Applications").join(name))
    }
}

fn executable_on_path(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    env::split_paths(&path)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file() && is_executable(candidate))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .is_ok_and(|metadata| metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

fn fire_command() -> Result<PathBuf, String> {
    if executable_on_path("fire").is_some() {
        return Ok(PathBuf::from("fire"));
    }
    env::current_exe().map_err(|err| format!("Could not determine the running fire binary: {err}"))
}

fn current_platform() -> Platform {
    if cfg!(target_os = "macos") {
        Platform::MacOs
    } else {
        Platform::Linux
    }
}

fn past_tense(action: Action) -> &'static str {
    match action {
        Action::Install => "Installed",
        Action::Uninstall => "Uninstalled",
    }
}

fn desired_state(action: Action) -> &'static str {
    match action {
        Action::Install => "installed",
        Action::Uninstall => "uninstalled",
    }
}

fn fail(message: &str) -> ! {
    eprintln!("[fire] {message}");
    process::exit(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hosts_without_normalizing_input() {
        assert_eq!("claude-desktop".parse(), Ok(Host::ClaudeDesktop));
        assert!("Claude-Desktop".parse::<Host>().is_err());
    }

    #[test]
    fn platform_paths_cover_macos_and_linux() {
        let home = Path::new("/home/example");
        assert_eq!(
            config_path(Host::ClaudeDesktop, home, Platform::Linux),
            Path::new("/home/example/.config/Claude/claude_desktop_config.json")
        );
        assert_eq!(
            config_path(Host::ClaudeDesktop, home, Platform::MacOs),
            Path::new(
                "/home/example/Library/Application Support/Claude/claude_desktop_config.json"
            )
        );
        assert_eq!(
            config_path(Host::VsCode, home, Platform::Linux),
            Path::new("/home/example/.config/Code/User/mcp.json")
        );
        assert_eq!(
            config_path(Host::VsCode, home, Platform::MacOs),
            Path::new("/home/example/Library/Application Support/Code/User/mcp.json")
        );
    }
}
