use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use serde_json::{json, Map, Value};

const SCHEMA_COMMENT: &str = "# yaml-language-server: $schema=https://raw.githubusercontent.com/gbenm/fire/main/schemas/fire.schema.json";

#[derive(Clone, Copy)]
enum SourceKind {
    Package,
    Make,
    Just,
    Compose,
    Cargo,
}

struct Source {
    kind: SourceKind,
    display: String,
    path: PathBuf,
    target: &'static str,
    group: &'static str,
    description: &'static str,
}

struct Discovered {
    commands: Map<String, Value>,
}

pub(crate) fn has_known_sources(cwd: &Path) -> bool {
    auto_sources(cwd).into_iter().next().is_some()
}

pub(crate) fn handle(args: &[String]) {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    import(&cwd, args);
}

pub(crate) fn import(cwd: &Path, requested: &[String]) {
    let sources = if requested.is_empty() {
        auto_sources(cwd)
    } else {
        requested_sources(cwd, requested)
    };

    if sources.is_empty() && requested.is_empty() {
        println!("No package.json, Makefile, justfile, Compose file, or Cargo.toml found.");
        return;
    }

    let mut imported_any = false;
    for source in sources {
        let discovered = match discover(&source) {
            Ok(discovered) => discovered,
            Err(err) => {
                warn(&format!("could not import {}: {err}", source.display));
                continue;
            }
        };
        if discovered.commands.is_empty() {
            warn(&format!(
                "{} contains no importable commands",
                source.display
            ));
            continue;
        }

        match merge_source(cwd, &source, discovered) {
            Ok(MergeResult { added, skipped }) => {
                if added > 0 {
                    imported_any = true;
                    println!("Imported {}: {added} commands", source.display);
                }
                if !skipped.is_empty() {
                    println!("Skipped {}: {}", source.display, skipped.join(", "));
                }
            }
            Err(err) => warn(&format!("could not import {}: {err}", source.display)),
        }
    }

    if !imported_any && !requested.is_empty() {
        println!("No commands were added.");
    }
}

fn auto_sources(cwd: &Path) -> Vec<Source> {
    let mut sources = Vec::new();
    if cwd.join("package.json").is_file() {
        sources.push(source(cwd, SourceKind::Package, "package.json"));
    }
    if cwd.join("Makefile").is_file() {
        sources.push(source(cwd, SourceKind::Make, "Makefile"));
    }
    if cwd.join("justfile").is_file() {
        sources.push(source(cwd, SourceKind::Just, "justfile"));
    } else if cwd.join("Justfile").is_file() {
        sources.push(source(cwd, SourceKind::Just, "Justfile"));
    }
    if let Some(name) = [
        "compose.yaml",
        "compose.yml",
        "docker-compose.yaml",
        "docker-compose.yml",
    ]
    .into_iter()
    .find(|name| cwd.join(name).is_file())
    {
        sources.push(source(cwd, SourceKind::Compose, name));
    }
    if cwd.join("Cargo.toml").is_file() {
        sources.push(source(cwd, SourceKind::Cargo, "Cargo.toml"));
    }
    sources
}

fn requested_sources(cwd: &Path, requested: &[String]) -> Vec<Source> {
    let mut sources = Vec::new();
    for value in requested {
        let candidate = Path::new(value);
        let name = candidate
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(value);
        let kind = match name {
            "package" | "package.json" => Some(SourceKind::Package),
            "make" | "Makefile" => Some(SourceKind::Make),
            "just" | "justfile" | "Justfile" => Some(SourceKind::Just),
            "compose"
            | "compose.yaml"
            | "compose.yml"
            | "docker-compose.yaml"
            | "docker-compose.yml" => Some(SourceKind::Compose),
            "cargo" | "Cargo.toml" => Some(SourceKind::Cargo),
            _ => None,
        };
        let Some(kind) = kind else {
            warn(&format!("unknown import source `{value}`"));
            continue;
        };
        let path = if candidate.components().count() > 1 {
            cwd.join(candidate)
        } else {
            requested_path(cwd, kind, name)
        };
        if !path.is_file() {
            warn(&format!("import source `{}` was not found", path.display()));
            continue;
        }
        let display = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(value)
            .to_string();
        sources.push(source_at(kind, display, path));
    }
    sources
}

fn requested_path(cwd: &Path, kind: SourceKind, name: &str) -> PathBuf {
    if name.contains('.') || matches!(name, "Makefile" | "justfile" | "Justfile") {
        return cwd.join(name);
    }
    match kind {
        SourceKind::Package => cwd.join("package.json"),
        SourceKind::Make => cwd.join("Makefile"),
        SourceKind::Just => ["justfile", "Justfile"]
            .into_iter()
            .map(|name| cwd.join(name))
            .find(|path| path.is_file())
            .unwrap_or_else(|| cwd.join("justfile")),
        SourceKind::Compose => [
            "compose.yaml",
            "compose.yml",
            "docker-compose.yaml",
            "docker-compose.yml",
        ]
        .into_iter()
        .map(|name| cwd.join(name))
        .find(|path| path.is_file())
        .unwrap_or_else(|| cwd.join("compose.yaml")),
        SourceKind::Cargo => cwd.join("Cargo.toml"),
    }
}

fn source(cwd: &Path, kind: SourceKind, name: &str) -> Source {
    source_at(kind, name.to_string(), cwd.join(name))
}

fn source_at(kind: SourceKind, display: String, path: PathBuf) -> Source {
    let (target, group, description) = match kind {
        SourceKind::Package => ("package.fire.yaml", "package", "package.json scripts"),
        SourceKind::Make => ("make.fire.yaml", "make", "Makefile targets"),
        SourceKind::Just => ("just.fire.yaml", "just", "just recipes"),
        SourceKind::Compose => ("compose.fire.yaml", "compose", "Compose services"),
        SourceKind::Cargo => ("cargo.fire.yaml", "cargo", "Cargo commands"),
    };
    Source {
        kind,
        display,
        path,
        target,
        group,
        description,
    }
}

fn discover(source: &Source) -> Result<Discovered, String> {
    match source.kind {
        SourceKind::Package => discover_package(source),
        SourceKind::Make => discover_make(source),
        SourceKind::Just => discover_just(source),
        SourceKind::Compose => discover_compose(source),
        SourceKind::Cargo => discover_cargo(source),
    }
}

fn discover_package(source: &Source) -> Result<Discovered, String> {
    let contents = read(source)?;
    let document: Value = serde_json::from_str(&contents).map_err(|err| err.to_string())?;
    let scripts = document
        .get("scripts")
        .and_then(Value::as_object)
        .ok_or_else(|| "`scripts` is missing or is not an object".to_string())?;
    let cwd = source.path.parent().unwrap_or_else(|| Path::new("."));
    let manager = package_manager(cwd);
    let mut commands = Map::new();
    for (name, body) in scripts {
        let Some(body) = body.as_str() else {
            warn(&format!("skipping non-string package.json script `{name}`"));
            continue;
        };
        if !valid_command_name(name, &source.display) {
            continue;
        }
        commands.insert(
            name.clone(),
            json!({
                "description": body,
                "exec": format!("{manager} run {}", shell_quote(name)),
            }),
        );
    }
    Ok(Discovered { commands })
}

fn package_manager(cwd: &Path) -> &'static str {
    if cwd.join("bun.lock").is_file() || cwd.join("bun.lockb").is_file() {
        "bun"
    } else if cwd.join("pnpm-lock.yaml").is_file() {
        "pnpm"
    } else if cwd.join("yarn.lock").is_file() {
        "yarn"
    } else {
        "npm"
    }
}

fn discover_make(source: &Source) -> Result<Discovered, String> {
    let contents = read(source)?;
    let mut commands = Map::new();
    let mut preceding_comment: Option<String> = None;
    for line in contents.lines() {
        if line.starts_with('\t') {
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            preceding_comment = None;
            continue;
        }
        if let Some(comment) = trimmed.strip_prefix('#') {
            preceding_comment = Some(
                comment
                    .strip_prefix('#')
                    .unwrap_or(comment)
                    .trim()
                    .to_string(),
            );
            continue;
        }
        let Some((targets, remainder)) = line.split_once(':') else {
            preceding_comment = None;
            continue;
        };
        if remainder.trim_start().starts_with('=') {
            preceding_comment = None;
            continue;
        }
        let inline = remainder
            .split_once("##")
            .map(|(_, comment)| comment.trim().to_string());
        let description = inline.or_else(|| preceding_comment.take());
        for name in targets.split_whitespace() {
            if name.starts_with('.') || name.contains('%') || name.contains('$') {
                continue;
            }
            if !valid_command_name(name, &source.display) {
                continue;
            }
            commands.insert(
                name.to_string(),
                command_value(
                    format!("make {}", shell_quote(name)),
                    description.as_deref(),
                    false,
                ),
            );
        }
        preceding_comment = None;
    }
    Ok(Discovered { commands })
}

fn discover_just(source: &Source) -> Result<Discovered, String> {
    let cwd = source.path.parent().unwrap_or_else(|| Path::new("."));
    let dumped = Command::new("just")
        .args(["--dump", "--dump-format", "json"])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    match dumped {
        Ok(output) if output.status.success() => discover_just_dump(source, &output.stdout),
        Ok(output) => Err(format!(
            "`just --dump --dump-format json` exited with {}",
            output.status
        )),
        Err(err) if err.kind() == ErrorKind::NotFound => discover_just_text(source),
        Err(err) => Err(format!(
            "could not run `just --dump --dump-format json`: {err}"
        )),
    }
}

fn discover_just_dump(source: &Source, output: &[u8]) -> Result<Discovered, String> {
    let document: Value = serde_json::from_slice(output).map_err(|err| err.to_string())?;
    let recipes = document
        .get("recipes")
        .and_then(Value::as_object)
        .ok_or_else(|| "just dump has no `recipes` object".to_string())?;
    let mut commands = Map::new();
    for (name, recipe) in recipes {
        if !valid_command_name(name, &source.display) {
            continue;
        }
        let description = recipe.get("doc").and_then(Value::as_str);
        commands.insert(
            name.clone(),
            command_value(format!("just {}", shell_quote(name)), description, false),
        );
    }
    Ok(Discovered { commands })
}

fn discover_just_text(source: &Source) -> Result<Discovered, String> {
    let contents = read(source)?;
    let mut commands = Map::new();
    let mut preceding_comment: Option<String> = None;
    for line in contents.lines() {
        if line.chars().next().is_some_and(char::is_whitespace) {
            continue;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            preceding_comment = None;
            continue;
        }
        if let Some(comment) = trimmed.strip_prefix('#') {
            preceding_comment = Some(
                comment
                    .strip_prefix('#')
                    .unwrap_or(comment)
                    .trim()
                    .to_string(),
            );
            continue;
        }
        let Some((signature, remainder)) = trimmed.split_once(':') else {
            preceding_comment = None;
            continue;
        };
        if remainder.trim_start().starts_with('=') {
            preceding_comment = None;
            continue;
        }
        let Some(name) = signature.split_whitespace().next() else {
            continue;
        };
        if matches!(name, "alias" | "export" | "import" | "mod" | "set")
            || !valid_command_name(name, &source.display)
        {
            preceding_comment = None;
            continue;
        }
        commands.insert(
            name.to_string(),
            command_value(
                format!("just {}", shell_quote(name)),
                preceding_comment.as_deref(),
                false,
            ),
        );
        preceding_comment = None;
    }
    Ok(Discovered { commands })
}

fn discover_compose(source: &Source) -> Result<Discovered, String> {
    let contents = read(source)?;
    let document: Value = yaml_serde::from_str(&contents).map_err(|err| err.to_string())?;
    let services = document
        .get("services")
        .and_then(Value::as_object)
        .ok_or_else(|| "`services` is missing or is not an object".to_string())?;
    let mut commands = Map::new();
    commands.insert(
        "up".to_string(),
        command_value(
            "docker compose up".to_string(),
            Some("Create and start services"),
            true,
        ),
    );
    commands.insert(
        "down".to_string(),
        command_value(
            "docker compose down".to_string(),
            Some("Stop and remove services"),
            false,
        ),
    );
    commands.insert(
        "logs".to_string(),
        command_value(
            "docker compose logs".to_string(),
            Some("Show service logs"),
            false,
        ),
    );
    let mut service_commands = Map::new();
    for (name, definition) in services {
        if !valid_command_name(name, &source.display) {
            continue;
        }
        service_commands.insert(
            name.clone(),
            command_value(
                format!("docker compose exec {}", shell_quote(name)),
                Some(&format!("Open a command in the {name} service")),
                dev_like_service(name, definition),
            ),
        );
    }
    commands.insert(
        "exec".to_string(),
        json!({
            "description": "Run a command in a service",
            "commands": service_commands,
        }),
    );
    Ok(Discovered { commands })
}

fn dev_like_service(name: &str, definition: &Value) -> bool {
    definition.get("develop").is_some()
        || name
            .split(['-', '_', '.'])
            .any(|part| matches!(part, "dev" | "develop" | "development"))
}

fn discover_cargo(source: &Source) -> Result<Discovered, String> {
    let contents = read(source)?;
    contents
        .parse::<toml_edit::DocumentMut>()
        .map_err(|err| err.to_string())?;
    let commands = [
        ("build", "Build the package"),
        ("test", "Run the tests"),
        ("run", "Run a binary or example"),
        ("check", "Check the package for errors"),
    ]
    .into_iter()
    .map(|(name, description)| {
        (
            name.to_string(),
            command_value(format!("cargo {name}"), Some(description), false),
        )
    })
    .collect::<Map<_, _>>();
    Ok(Discovered { commands })
}

struct MergeResult {
    added: usize,
    skipped: Vec<String>,
}

fn merge_source(
    cwd: &Path,
    source: &Source,
    discovered: Discovered,
) -> Result<MergeResult, String> {
    let path = cwd.join(source.target);
    let mut document = if path.exists() {
        let contents = fs::read_to_string(&path)
            .map_err(|err| format!("could not read {}: {err}", path.display()))?;
        yaml_serde::from_str::<Value>(&contents)
            .map_err(|err| format!("{} is not valid YAML: {err}", path.display()))?
    } else {
        json!({
            "group": source.group,
            "description": source.description,
            "commands": {},
        })
    };
    let root = document
        .as_object_mut()
        .ok_or_else(|| format!("{} does not contain a YAML object", path.display()))?;
    let commands = root
        .entry("commands")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| format!("{} has a non-object `commands` value", path.display()))?;

    let mut added = 0;
    let mut skipped = Vec::new();
    for (name, command) in discovered.commands {
        if commands.contains_key(&name) {
            skipped.push(name);
        } else {
            added += leaf_count(&command);
            commands.insert(name, command);
        }
    }
    if added > 0 {
        let yaml = yaml_serde::to_string(&document)
            .map_err(|err| format!("could not serialize {}: {err}", path.display()))?;
        let content = format!("{SCHEMA_COMMENT}\n---\n{yaml}");
        fs::write(&path, content)
            .map_err(|err| format!("could not write {}: {err}", path.display()))?;
    }
    Ok(MergeResult { added, skipped })
}

fn leaf_count(command: &Value) -> usize {
    command
        .get("commands")
        .and_then(Value::as_object)
        .map(|commands| commands.values().map(leaf_count).sum())
        .unwrap_or(1)
}

fn command_value(exec: String, description: Option<&str>, long_running: bool) -> Value {
    let mut command = Map::new();
    if let Some(description) = description {
        command.insert(
            "description".to_string(),
            Value::String(description.to_string()),
        );
    }
    if long_running {
        command.insert("long_running".to_string(), Value::Bool(true));
    }
    command.insert("exec".to_string(), Value::String(exec));
    Value::Object(command)
}

fn valid_command_name(name: &str, source: &str) -> bool {
    if name.is_empty() || name == "cli" || name.chars().any(char::is_whitespace) {
        warn(&format!(
            "skipping unsupported command name `{name}` from {source}; names must be one non-reserved word"
        ));
        false
    } else {
        true
    }
}

fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&byte))
    {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\"'\"'"))
    }
}

fn read(source: &Source) -> Result<String, String> {
    fs::read_to_string(&source.path)
        .map_err(|err| format!("could not read {}: {err}", source.path.display()))
}

fn warn(message: &str) {
    eprintln!("[fire] warning: {message}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn just_json_dump_preserves_recipe_names_and_docs() {
        let source = source_at(
            SourceKind::Just,
            "justfile".to_string(),
            PathBuf::from("justfile"),
        );
        let discovered = discover_just_dump(
            &source,
            br#"{
                "recipes": {
                    "say'hi": {
                        "doc": "Print user's exact $HOME text"
                    }
                }
            }"#,
        )
        .unwrap();

        assert_eq!(
            discovered.commands["say'hi"]["description"],
            "Print user's exact $HOME text"
        );
        assert_eq!(discovered.commands["say'hi"]["exec"], "just 'say'\"'\"'hi'");
    }
}
