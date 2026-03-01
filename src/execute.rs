use std::{
    io::Write,
    path::{Path, PathBuf},
    process,
    process::{Command, Stdio},
};

use crate::resolve::ResolvedCommand;

pub(crate) fn execute_resolved_command(resolved: ResolvedCommand<'_>) -> ! {
    let Some(commands_to_run) = resolved.command.execution_commands() else {
        eprintln!("[fire] Command path has no executable action.");
        if let Some(subcommands) = resolved.command.subcommands() {
            eprintln!("Commands:");
            let width = subcommands
                .keys()
                .map(|name| name.len())
                .max()
                .unwrap_or(0)
                .max(1);
            for (name, entry) in subcommands {
                let description = entry.description().unwrap_or_default();
                if description.is_empty() {
                    eprintln!("  {name}");
                } else {
                    let short = description.lines().next().unwrap_or("").trim();
                    eprintln!("  {:width$}  {}", name, short, width = width);
                }
            }
        }
        process::exit(1);
    };

    let context = build_execution_context(&resolved);
    ensure_working_directory(&context.dir);

    let selected_runner = select_runner(&context);
    let mut exit_code = 0;
    let commands_to_run = commands_with_remaining_args(&commands_to_run, resolved.remaining_args);

    match selected_runner {
        Some(runner) => {
            exit_code = run_with_runner(&runner, &context.dir, &commands_to_run);
        }
        None => {
            for command in &commands_to_run {
                let status = run_shell_command(command, &context.dir);
                let code = status.code().unwrap_or(1);
                exit_code = code;
                if code != 0 {
                    break;
                }
            }
        }
    }

    process::exit(exit_code);
}

#[derive(Debug, Default)]
struct ExecutionContext {
    dir: PathBuf,
    runner: Option<String>,
    fallback_runner: Option<String>,
    check: Option<String>,
}

fn build_execution_context(resolved: &ResolvedCommand<'_>) -> ExecutionContext {
    let mut context = ExecutionContext {
        dir: resolved.project_dir.to_path_buf(),
        ..ExecutionContext::default()
    };

    for entry in &resolved.command_chain {
        let Some(spec) = entry.spec() else {
            continue;
        };

        if let Some(next_dir) = non_empty(&spec.dir) {
            context.dir = resolve_next_dir(&context.dir, next_dir);
        }
        if let Some(check) = non_empty(&spec.check) {
            context.check = Some(check.to_string());
        }
        if let Some(runner) = non_empty(&spec.runner) {
            context.runner = Some(runner.to_string());
        }
        if let Some(fallback_runner) = non_empty(&spec.fallback_runner) {
            context.fallback_runner = Some(fallback_runner.to_string());
        }
    }

    context
}

fn resolve_next_dir(current: &Path, next: &str) -> PathBuf {
    let next_path = Path::new(next);
    if next_path.is_absolute() {
        next_path.to_path_buf()
    } else {
        current.join(next_path)
    }
}

fn ensure_working_directory(dir: &Path) {
    if !dir.exists() {
        eprintln!("[fire] Working directory does not exist: {}", dir.display());
        process::exit(1);
    }
    if !dir.is_dir() {
        eprintln!(
            "[fire] Working directory is not a directory: {}",
            dir.display()
        );
        process::exit(1);
    }
}

fn select_runner(context: &ExecutionContext) -> Option<String> {
    let check_passed = context
        .check
        .as_deref()
        .map(|check| run_shell_command(check, &context.dir).success())
        .unwrap_or(true);

    if check_passed {
        return context.runner.clone();
    }

    if let Some(fallback) = context.fallback_runner.clone() {
        return Some(fallback);
    }

    if context.check.is_some() {
        eprintln!("[fire] Check command failed and no fallback_runner is configured.");
        process::exit(1);
    }

    context.runner.clone()
}

fn run_with_runner(runner: &str, dir: &Path, commands: &[String]) -> i32 {
    let normalized_runner = normalize_runner_for_piped_stdin(runner);
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(&normalized_runner)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap_or_else(|err| {
            eprintln!("[fire] Failed to start runner `{normalized_runner}`: {err}");
            process::exit(1);
        });

    {
        let Some(stdin) = child.stdin.as_mut() else {
            eprintln!("[fire] Runner `{normalized_runner}` has no writable stdin.");
            let _ = child.kill();
            process::exit(1);
        };

        if writeln!(stdin, "set -e").is_err() {
            eprintln!("[fire] Failed to initialize runner shell.");
            let _ = child.kill();
            process::exit(1);
        }

        for command in commands {
            if writeln!(stdin, "{command}").is_err() {
                eprintln!("[fire] Failed to send command to runner: `{command}`");
                let _ = child.kill();
                process::exit(1);
            }
        }

        let _ = writeln!(stdin, "exit");
    }

    let status = child.wait().unwrap_or_else(|err| {
        eprintln!("[fire] Failed while waiting for runner `{normalized_runner}`: {err}");
        let _ = child.kill();
        process::exit(1);
    });

    if !status.success() {
        return status.code().unwrap_or(1);
    }

    status.code().unwrap_or(0)
}

fn commands_with_remaining_args(commands: &[String], remaining_args: &[String]) -> Vec<String> {
    let mut out = commands.to_vec();
    if out.is_empty() || remaining_args.is_empty() {
        return out;
    }

    if let Some(last) = out.last_mut() {
        last.push(' ');
        last.push_str(&join_shell_args(remaining_args));
    }

    out
}

fn run_shell_command(command: &str, dir: &Path) -> process::ExitStatus {
    Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(dir)
        .status()
        .unwrap_or_else(|err| {
            eprintln!("[fire] Failed to execute `{command}`: {err}");
            process::exit(1);
        })
}

fn join_shell_args(args: &[String]) -> String {
    args.iter()
        .map(String::as_str)
        .map(shell_escape)
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_escape(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }

    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | ':' | '='))
    {
        return value.to_string();
    }

    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn non_empty(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn normalize_runner_for_piped_stdin(runner: &str) -> String {
    // Commands are sent through stdin. In that mode, explicit TTY flags break
    // tools like docker with "the input device is not a TTY".
    let mut out = Vec::new();
    for token in runner.split_whitespace() {
        if token == "-t" || token == "--tty" {
            continue;
        }
        if token == "-it" || token == "-ti" {
            out.push("-i".to_string());
            continue;
        }
        if token.starts_with('-') && !token.starts_with("--") && token.len() > 2 {
            let mut chars = token.chars();
            let dash = chars.next().unwrap_or('-');
            let flags: String = chars.filter(|ch| *ch != 't').collect();
            if flags.is_empty() {
                continue;
            }
            out.push(format!("{dash}{flags}"));
            continue;
        }
        out.push(token.to_string());
    }

    ensure_non_tty_for_docker_compose_exec(&mut out);

    if out.is_empty() {
        runner.to_string()
    } else {
        out.join(" ")
    }
}

fn ensure_non_tty_for_docker_compose_exec(tokens: &mut Vec<String>) {
    if tokens.is_empty() {
        return;
    }

    let exec_index = if tokens.first().map(String::as_str) == Some("docker-compose") {
        tokens.iter().position(|token| token == "exec")
    } else if tokens.len() >= 2
        && tokens.first().map(String::as_str) == Some("docker")
        && tokens.get(1).map(String::as_str) == Some("compose")
    {
        tokens.iter().position(|token| token == "exec")
    } else {
        None
    };

    let Some(exec_index) = exec_index else {
        return;
    };

    if tokens
        .iter()
        .any(|token| token == "-T" || token == "--no-tty")
    {
        return;
    }

    tokens.insert(exec_index + 1, "-T".to_string());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CommandEntry, CommandSpec};

    #[test]
    fn escape_single_quote_in_shell_argument() {
        assert_eq!(shell_escape("it'ok"), "'it'\"'\"'ok'");
    }

    #[test]
    fn nested_relative_dirs_are_resolved_from_parent() {
        let root = PathBuf::from("/tmp/project");
        let child = resolve_next_dir(&root, "services");
        let nested = resolve_next_dir(&child, "api");
        assert_eq!(nested, PathBuf::from("/tmp/project/services/api"));
    }

    #[test]
    fn absolute_dir_overrides_parent_dir() {
        let root = PathBuf::from("/tmp/project");
        let nested = resolve_next_dir(&root, "/opt/workspace");
        assert_eq!(nested, PathBuf::from("/opt/workspace"));
    }

    #[test]
    fn remaining_args_only_append_to_last_command() {
        let commands = vec!["npm run build".to_string(), "npm run start".to_string()];
        let result =
            commands_with_remaining_args(&commands, &["--host".to_string(), "0.0.0.0".to_string()]);
        assert_eq!(
            result,
            vec![
                "npm run build".to_string(),
                "npm run start --host 0.0.0.0".to_string()
            ]
        );
    }

    #[test]
    fn select_runner_uses_fallback_when_check_fails() {
        let context = ExecutionContext {
            dir: std::env::current_dir().expect("cwd"),
            runner: Some("bash".to_string()),
            fallback_runner: Some("sh".to_string()),
            check: Some("false".to_string()),
        };
        let selected = select_runner(&context);
        assert_eq!(selected, Some("sh".to_string()));
    }

    #[test]
    fn select_runner_uses_primary_when_check_passes() {
        let context = ExecutionContext {
            dir: std::env::current_dir().expect("cwd"),
            runner: Some("bash".to_string()),
            fallback_runner: Some("sh".to_string()),
            check: Some("true".to_string()),
        };
        let selected = select_runner(&context);
        assert_eq!(selected, Some("bash".to_string()));
    }

    #[test]
    fn command_entry_spec_is_available_for_spec_variant() {
        let entry = CommandEntry::Spec(CommandSpec {
            dir: "api".to_string(),
            ..CommandSpec::default()
        });
        assert!(entry.spec().is_some());
    }

    #[test]
    fn normalizes_tty_flags_for_piped_runner() {
        let runner = "docker run --rm -it node:lts-alpine /bin/bash";
        let normalized = normalize_runner_for_piped_stdin(runner);
        assert_eq!(normalized, "docker run --rm -i node:lts-alpine /bin/bash");
    }

    #[test]
    fn keeps_non_tty_flags_untouched() {
        let runner = "docker exec -i my-container /bin/sh";
        let normalized = normalize_runner_for_piped_stdin(runner);
        assert_eq!(normalized, runner);
    }

    #[test]
    fn docker_compose_exec_adds_no_tty_flag() {
        let runner = "docker compose exec linux sh";
        let normalized = normalize_runner_for_piped_stdin(runner);
        assert_eq!(normalized, "docker compose exec -T linux sh");
    }

    #[test]
    fn docker_compose_exec_keeps_existing_no_tty_flag() {
        let runner = "docker compose exec -T linux sh";
        let normalized = normalize_runner_for_piped_stdin(runner);
        assert_eq!(normalized, runner);
    }
}
