use std::{process, process::Command};

use crate::resolve::ResolvedCommand;

pub(crate) fn execute_resolved_command(resolved: ResolvedCommand<'_>) -> ! {
    let Some(commands_to_run) = resolved.command.execution_commands() else {
        eprintln!("[fire] Command path has no executable action.");
        if let Some(subcommands) = resolved.command.subcommands() {
            eprintln!("[fire] Available subcommands:");
            for (name, entry) in subcommands {
                let description = entry.description().unwrap_or_default();
                if description.is_empty() {
                    eprintln!("  - {name}");
                } else {
                    eprintln!("  - {name}\t{description}");
                }
            }
        }
        process::exit(1);
    };

    let mut exit_code = 0;
    for (index, command) in commands_to_run.iter().enumerate() {
        let mut full_command = command.clone();
        if index + 1 == commands_to_run.len() && !resolved.remaining_args.is_empty() {
            full_command.push(' ');
            full_command.push_str(&join_shell_args(resolved.remaining_args));
        }

        let status = Command::new("sh")
            .arg("-c")
            .arg(&full_command)
            .status()
            .unwrap_or_else(|err| {
                eprintln!("[fire] Failed to execute `{full_command}`: {err}");
                process::exit(1);
            });

        let code = status.code().unwrap_or(1);
        exit_code = code;
        if code != 0 {
            break;
        }
    }

    process::exit(exit_code);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_single_quote_in_shell_argument() {
        assert_eq!(shell_escape("it'ok"), "'it'\"'\"'ok'");
    }
}
