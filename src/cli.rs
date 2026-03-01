use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process,
};

use crate::registry::{install_directory, InstallResult};

pub(crate) fn handle_cli_command(command_args: &[String]) {
    match command_args {
        [cli, install] if cli == "cli" && install == "install" => run_install(),
        [cli, init] if cli == "cli" && init == "init" => run_init(),
        [cli, completion] if cli == "cli" && completion == "completion" => {
            print_completion_help();
        }
        [cli, completion, install]
            if cli == "cli" && completion == "completion" && install == "install" =>
        {
            run_completion_install(CompletionTarget::All)
        }
        [cli, completion, install, shell]
            if cli == "cli" && completion == "completion" && install == "install" =>
        {
            let target = match parse_completion_target(shell) {
                Some(target) => target,
                None => {
                    eprintln!("[fire] Invalid shell `{shell}`. Use one of: bash, zsh, all.");
                    process::exit(1);
                }
            };
            run_completion_install(target);
        }
        [cli] if cli == "cli" => print_cli_help(),
        _ => {
            eprintln!("[fire] Unknown cli command");
            eprintln!("Usage:");
            eprintln!("  fire cli install");
            eprintln!("  fire cli init");
            eprintln!("  fire cli completion install [bash|zsh|all]");
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
    println!("  completion  Manage shell completion scripts");
}

fn print_completion_help() {
    println!("Fire CLI Completion");
    println!("Commands:");
    println!("  install [bash|zsh|all]  Install completion scripts (default: all)");
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CompletionTarget {
    Bash,
    Zsh,
    All,
}

fn parse_completion_target(value: &str) -> Option<CompletionTarget> {
    match value {
        "bash" => Some(CompletionTarget::Bash),
        "zsh" => Some(CompletionTarget::Zsh),
        "all" => Some(CompletionTarget::All),
        _ => None,
    }
}

fn run_completion_install(target: CompletionTarget) {
    let home_dir = dirs::home_dir().unwrap_or_else(|| {
        eprintln!("[fire] Could not resolve HOME directory.");
        process::exit(1);
    });

    println!("Fire CLI Completion Install");
    println!("----------------------------------------");

    let mut installed_paths: Vec<PathBuf> = Vec::new();
    match target {
        CompletionTarget::Bash => {
            let path = install_bash_completion(&home_dir).unwrap_or_else(|err| {
                eprintln!("[fire] {err}");
                process::exit(1);
            });
            installed_paths.push(path);
        }
        CompletionTarget::Zsh => {
            let path = install_zsh_completion(&home_dir).unwrap_or_else(|err| {
                eprintln!("[fire] {err}");
                process::exit(1);
            });
            installed_paths.push(path);
        }
        CompletionTarget::All => {
            let zsh_path = install_zsh_completion(&home_dir).unwrap_or_else(|err| {
                eprintln!("[fire] {err}");
                process::exit(1);
            });
            installed_paths.push(zsh_path);

            let bash_path = install_bash_completion(&home_dir).unwrap_or_else(|err| {
                eprintln!("[fire] {err}");
                process::exit(1);
            });
            installed_paths.push(bash_path);
        }
    }

    println!("Installed completion files:");
    for path in &installed_paths {
        println!("  - {}", path.display());
    }
    println!();
    println!("Open a new shell session (or source your shell rc file) to apply changes.");
}

fn install_zsh_completion(home_dir: &Path) -> Result<PathBuf, String> {
    let completion_dir = home_dir.join(".zsh").join("completions");
    fs::create_dir_all(&completion_dir)
        .map_err(|err| format!("Failed to create zsh completion directory: {err}"))?;

    let completion_path = completion_dir.join("_fire");
    fs::write(&completion_path, zsh_completion_script())
        .map_err(|err| format!("Failed to write zsh completion script: {err}"))?;

    let zshrc_path = home_dir.join(".zshrc");
    upsert_managed_block_file(
        &zshrc_path,
        zsh_block_start_marker(),
        zsh_block_end_marker(),
        &zsh_completion_rc_block(),
    )?;

    Ok(completion_path)
}

fn install_bash_completion(home_dir: &Path) -> Result<PathBuf, String> {
    let completion_dir = home_dir
        .join(".local")
        .join("share")
        .join("bash-completion")
        .join("completions");
    fs::create_dir_all(&completion_dir)
        .map_err(|err| format!("Failed to create bash completion directory: {err}"))?;

    let completion_path = completion_dir.join("fire");
    fs::write(&completion_path, bash_completion_script())
        .map_err(|err| format!("Failed to write bash completion script: {err}"))?;

    let bashrc_path = home_dir.join(".bashrc");
    upsert_managed_block_file(
        &bashrc_path,
        bash_block_start_marker(),
        bash_block_end_marker(),
        &bash_completion_rc_block(),
    )?;

    Ok(completion_path)
}

fn upsert_managed_block_file(
    file_path: &Path,
    start_marker: &str,
    end_marker: &str,
    block: &str,
) -> Result<(), String> {
    let current = fs::read_to_string(file_path).unwrap_or_default();
    let updated = upsert_managed_block(&current, start_marker, end_marker, block);
    fs::write(file_path, updated)
        .map_err(|err| format!("Failed to update {}: {err}", file_path.display()))
}

fn upsert_managed_block(
    current: &str,
    start_marker: &str,
    end_marker: &str,
    block: &str,
) -> String {
    if let Some(start) = current.find(start_marker) {
        if let Some(end_relative) = current[start..].find(end_marker) {
            let end = start + end_relative + end_marker.len();
            let mut output = String::new();
            output.push_str(&current[..start]);
            if !output.ends_with('\n') && !output.is_empty() {
                output.push('\n');
            }
            output.push_str(block);
            output.push('\n');
            output.push_str(current[end..].trim_start_matches('\n'));
            return output;
        }
    }

    if current.trim().is_empty() {
        return format!("{block}\n");
    }

    format!("{}\n\n{block}\n", current.trim_end())
}

fn zsh_completion_script() -> &'static str {
    r#"#compdef fire

_fire_cli() {
  local -a lines
  local -a entries
  local line value note

  lines=("${(@f)$(fire __complete -- "${words[@]}")}")
  (( ${#lines[@]} == 0 )) && return 1

  for line in "${lines[@]}"; do
    if [[ "$line" == *$'\t'* ]]; then
      value="${line%%$'\t'*}"
      note="${line#*$'\t'}"
      entries+=("$value:$note")
    else
      entries+=("$line:")
    fi
  done

  _describe 'fire commands' entries
}

compdef _fire_cli fire
"#
}

fn bash_completion_script() -> &'static str {
    r#"# shellcheck shell=bash
if type complete >/dev/null 2>&1; then
  complete -o nospace -C fire fire
fi
"#
}

fn zsh_completion_rc_block() -> String {
    format!(
        "{}\nif [ -d \"$HOME/.zsh/completions\" ]; then\n  fpath=(\"$HOME/.zsh/completions\" $fpath)\nfi\nautoload -Uz compinit\ncompinit\n{}",
        zsh_block_start_marker(),
        zsh_block_end_marker()
    )
}

fn bash_completion_rc_block() -> String {
    format!(
        "{}\nif [ -f \"$HOME/.local/share/bash-completion/completions/fire\" ]; then\n  source \"$HOME/.local/share/bash-completion/completions/fire\"\nfi\n{}",
        bash_block_start_marker(),
        bash_block_end_marker()
    )
}

fn zsh_block_start_marker() -> &'static str {
    "# >>> fire completion (zsh) >>>"
}

fn zsh_block_end_marker() -> &'static str {
    "# <<< fire completion (zsh) <<<"
}

fn bash_block_start_marker() -> &'static str {
    "# >>> fire completion (bash) >>>"
}

fn bash_block_end_marker() -> &'static str {
    "# <<< fire completion (bash) <<<"
}

#[cfg(test)]
mod tests {
    use super::{
        bash_block_end_marker, bash_block_start_marker, build_init_yaml, file_name_from_base,
        is_valid_file_base, parse_completion_target, upsert_managed_block, zsh_completion_script,
        CompletionTarget,
    };

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

    #[test]
    fn parses_completion_target() {
        assert_eq!(
            parse_completion_target("bash"),
            Some(CompletionTarget::Bash)
        );
        assert_eq!(parse_completion_target("zsh"), Some(CompletionTarget::Zsh));
        assert_eq!(parse_completion_target("all"), Some(CompletionTarget::All));
        assert_eq!(parse_completion_target("fish"), None);
    }

    #[test]
    fn upsert_managed_block_appends_when_missing() {
        let block = format!(
            "{}\nsource ~/.bash_completion\n{}",
            bash_block_start_marker(),
            bash_block_end_marker()
        );
        let updated = upsert_managed_block(
            "export PATH=\"$PATH:/bin\"\n",
            bash_block_start_marker(),
            bash_block_end_marker(),
            &block,
        );
        assert!(updated.contains(bash_block_start_marker()));
        assert!(updated.contains("source ~/.bash_completion"));
    }

    #[test]
    fn upsert_managed_block_replaces_existing() {
        let old = format!(
            "{}\nold\n{}\n",
            bash_block_start_marker(),
            bash_block_end_marker()
        );
        let block = format!(
            "{}\nnew\n{}",
            bash_block_start_marker(),
            bash_block_end_marker()
        );
        let updated = upsert_managed_block(
            &old,
            bash_block_start_marker(),
            bash_block_end_marker(),
            &block,
        );
        assert!(!updated.contains("\nold\n"));
        assert!(updated.contains("\nnew\n"));
    }

    #[test]
    fn embedded_zsh_completion_script_contains_compdef() {
        let script = zsh_completion_script();
        assert!(script.contains("#compdef fire"));
        assert!(script.contains("compdef _fire_cli fire"));
    }
}
