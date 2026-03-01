use std::{
    fs,
    io::{self, Write},
    path::PathBuf,
    process,
};

use crate::registry::{install_directory, InstallResult};

pub(crate) fn handle_cli_command(command_args: &[String]) {
    match command_args {
        [cli, install] if cli == "cli" && install == "install" => run_install(),
        [cli, init] if cli == "cli" && init == "init" => run_init(),
        [cli] if cli == "cli" => print_cli_help(),
        _ => {
            eprintln!("[fire] Unknown cli command");
            eprintln!("Usage:");
            eprintln!("  fire cli install");
            eprintln!("  fire cli init");
            process::exit(1);
        }
    }
}

fn run_install() {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    match install_directory(&cwd) {
        Ok(InstallResult::Added) => {
            println!("Installed directory: {}", cwd.display());
        }
        Ok(InstallResult::AlreadyInstalled) => {
            println!("Directory already installed: {}", cwd.display());
        }
        Err(err) => {
            eprintln!("[fire] Failed to install directory: {err}");
            process::exit(1);
        }
    }
}

fn run_init() {
    println!("Fire CLI Init");
    println!("----------------------------------------");
    println!("Create a minimal fire config file for this directory.");
    println!();

    let base_name = prompt_base_file_name();
    let file_name = file_name_from_base(&base_name);

    println!();
    println!("Namespace");
    println!("A namespace can separate companies or a full product/service.");
    let namespace_prefix = prompt_line("Namespace prefix (optional):");

    let namespace_description = if namespace_prefix.is_empty() {
        String::new()
    } else {
        prompt_non_empty_line("Namespace description:")
    };

    println!();
    println!("Group");
    println!("A group can separate areas like backend, frontend, or data.");
    let group = prompt_line("Command group (optional):");

    let output_path = PathBuf::from(&file_name);
    if output_path.exists() && !confirm_overwrite(&file_name) {
        println!("Init cancelled. File not modified.");
        return;
    }

    let content = build_init_yaml(
        non_empty(&namespace_prefix),
        non_empty(&namespace_description),
        non_empty(&group),
    );

    if let Err(err) = fs::write(&output_path, content) {
        eprintln!("[fire] Failed to write {}: {err}", output_path.display());
        process::exit(1);
    }

    println!();
    println!("Created {}", output_path.display());
    println!("Try it with:");
    println!("  fire example");
}

fn print_cli_help() {
    println!("Fire CLI Management");
    println!("Commands:");
    println!("  install  Register the current directory for global command loading");
    println!("  init     Create a minimal fire config file with guided prompts");
}

fn prompt_base_file_name() -> String {
    loop {
        let value = prompt_line("Base file name (backend/common/database). Empty -> fire.yml:");
        if value.is_empty() {
            return value;
        }

        let normalized = strip_known_yaml_suffixes(&value);
        if is_valid_file_base(normalized) {
            return value;
        }

        println!("Invalid base name. Use letters, numbers, '-' or '_' only.");
    }
}

fn prompt_non_empty_line(label: &str) -> String {
    loop {
        let value = prompt_line(label);
        if !value.is_empty() {
            return value;
        }
        println!("This field is required.");
    }
}

fn prompt_line(label: &str) -> String {
    print!("{label} ");
    let _ = io::stdout().flush();

    let mut value = String::new();
    if io::stdin().read_line(&mut value).is_err() {
        return String::new();
    }
    value.trim().to_string()
}

fn confirm_overwrite(file_name: &str) -> bool {
    let answer = prompt_line(&format!(
        "File '{file_name}' already exists. Overwrite? [y/N]:"
    ));
    matches!(answer.to_ascii_lowercase().as_str(), "y" | "yes")
}

fn strip_known_yaml_suffixes(value: &str) -> &str {
    value
        .strip_suffix(".fire.yml")
        .or_else(|| value.strip_suffix(".fire.yaml"))
        .or_else(|| value.strip_suffix(".yml"))
        .or_else(|| value.strip_suffix(".yaml"))
        .unwrap_or(value)
}

fn is_valid_file_base(value: &str) -> bool {
    !value.is_empty()
        && !value.contains('/')
        && !value.contains('\\')
        && !value.contains("..")
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

fn file_name_from_base(base: &str) -> String {
    let trimmed = base.trim();
    if trimmed.is_empty() {
        return "fire.yml".to_string();
    }
    let normalized = strip_known_yaml_suffixes(trimmed);
    format!("{normalized}.fire.yml")
}

fn build_init_yaml(
    namespace_prefix: Option<&str>,
    namespace_description: Option<&str>,
    group: Option<&str>,
) -> String {
    let mut lines = Vec::new();
    lines.push(
        "# yaml-language-server: $schema=https://raw.githubusercontent.com/gbenm/fire/main/schemas/fire.schema.json"
            .to_string(),
    );

    if let Some(prefix) = namespace_prefix {
        lines.push(String::new());
        lines.push("namespace:".to_string());
        lines.push(format!("  prefix: {}", yaml_quote(prefix)));
        lines.push(format!(
            "  description: {}",
            yaml_quote(namespace_description.unwrap_or_default())
        ));
    }

    if let Some(group) = group {
        lines.push(String::new());
        lines.push(format!("group: {}", yaml_quote(group)));
    }

    lines.push(String::new());
    lines.push("commands:".to_string());
    lines.push("  example: echo \"hello world\"".to_string());
    lines.push(String::new());

    lines.join("\n")
}

fn yaml_quote(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

fn non_empty(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

#[cfg(test)]
mod tests {
    use super::{build_init_yaml, file_name_from_base, is_valid_file_base};

    #[test]
    fn empty_base_name_uses_fire_yml() {
        assert_eq!(file_name_from_base(""), "fire.yml");
    }

    #[test]
    fn base_name_generates_pattern_file_name() {
        assert_eq!(file_name_from_base("backend"), "backend.fire.yml");
        assert_eq!(file_name_from_base("backend.fire.yml"), "backend.fire.yml");
    }

    #[test]
    fn validates_base_name() {
        assert!(is_valid_file_base("backend_api"));
        assert!(!is_valid_file_base("backend/api"));
        assert!(!is_valid_file_base(".."));
    }

    #[test]
    fn minimal_yaml_contains_example_command() {
        let yaml = build_init_yaml(None, None, None);
        assert!(yaml.contains("commands:"));
        assert!(yaml.contains("example: echo \"hello world\""));
    }

    #[test]
    fn yaml_contains_namespace_and_group_when_provided() {
        let yaml = build_init_yaml(Some("ex"), Some("Example"), Some("backend"));
        assert!(yaml.contains("namespace:"));
        assert!(yaml.contains("prefix: \"ex\""));
        assert!(yaml.contains("description: \"Example\""));
        assert!(yaml.contains("group: \"backend\""));
    }
}
