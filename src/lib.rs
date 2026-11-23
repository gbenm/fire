use std::{env as std_env, path::Path, process};

mod cli;
mod completion;
mod config;
mod container;
mod dependencies;
mod docs;
mod env;
mod execute;
mod flags;
mod global;
mod help;
mod mcp;
mod mcp_install;
mod registry;
mod resolve;
mod session;

use cli::handle_cli_command;
use completion::{completion_suggestions, render_values_only, render_with_descriptions};
use config::load_config;
use execute::execute_resolved_command;
use help::{print_command_help, print_root_help, print_scope_help};
use resolve::{detect_terminal_command_collision, resolve_command};

pub const FIRE_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn setup_cli() {
    session::defer_signals_when_supervised();
    let mut args: Vec<String> = std_env::args().collect();
    let bin_name = args
        .first()
        .and_then(|value| Path::new(value).file_name())
        .and_then(|value| value.to_str())
        .unwrap_or("fire")
        .to_string();
    if args.len() >= 2 && args[1] == "__complete" {
        let config = load_config();
        args.drain(0..2);
        let words = normalize_completion_words(args, &bin_name);
        let suggestions = completion_suggestions(&config, &words);
        for suggestion in render_with_descriptions(&suggestions) {
            println!("{suggestion}");
        }
        return;
    }

    if let Some(words) = completion_words_from_env(&bin_name) {
        let config = load_config();
        let suggestions = completion_suggestions(&config, &words);
        for suggestion in render_values_only(&suggestions) {
            println!("{suggestion}");
        }
        return;
    }

    let (flags, consumed) = flags::parse(&args[1..]).unwrap_or_else(|message| {
        eprintln!("[fire] {message}");
        eprintln!("{}", flags::USAGE);
        process::exit(1);
    });
    let command_args = &args[1 + consumed..];

    if consumed > 0 {
        let first = command_args.first().map(String::as_str);
        if first.is_none() || first == Some(":h") {
            eprintln!("[fire] flags need a command");
            eprintln!("{}", flags::USAGE);
            process::exit(1);
        }
        if first == Some("cli") {
            eprintln!(
                "[fire] flags before the command only apply to your commands, not to `fire cli`"
            );
            process::exit(1);
        }
        if let Err(message) = flags.validate() {
            eprintln!("[fire] {message}");
            process::exit(1);
        }
        flags.export();
    }

    if command_args.first().map(String::as_str) == Some("cli") {
        handle_cli_command(command_args);
        return;
    }

    // Loaded after `cli` on purpose: session supervisors start through `fire cli` and must not
    // pay for config discovery.
    let config = load_config();

    if command_args.is_empty() {
        print_root_help(&config);
        return;
    }

    if let Some(help_target) = extract_help_target(command_args) {
        if help_target.is_empty() {
            print_root_help(&config);
            return;
        }

        if let Some(resolved) = resolve_command(&config, help_target) {
            let command_path = &help_target[..resolved.consumed];
            print_command_help(command_path, resolved.command);
            return;
        }

        if print_scope_help(&config, help_target) {
            return;
        }

        eprintln!("[fire] Unknown command: {}", help_target[0]);
        print_root_help(&config);
        process::exit(1);
    }

    if let Some(resolved) = resolve_command(&config, command_args) {
        let command_path = &command_args[..resolved.consumed];
        if !resolved.command.is_runnable() {
            print_command_help(command_path, resolved.command);
            return;
        }
        if let Err(message) =
            detect_terminal_command_collision(&config, command_args, resolved.consumed)
        {
            eprintln!("[fire] {message}");
            process::exit(1);
        }
        if flags.agent {
            session::refuse_if_not_for_agents(command_args, &resolved);
        }
        // A dry run starts nothing, so `--bg --dry-run` just prints what would run.
        let dry_run = flags::env_bool("FIRE_DRY_RUN") == Some(true);
        if flags.bg && !dry_run {
            session::start_detached(command_args, flags.note, flags.new, flags.agent);
            return;
        }
        global::offer_registration(&config);
        let dependency_plan = if std::env::var_os("FIRE_SKIP_NEEDS").is_some() {
            dependencies::empty_plan()
        } else {
            dependencies::build_plan(&config, resolved.command).unwrap_or_else(|message| {
                eprintln!("[fire] {message}");
                process::exit(1);
            })
        };
        session::run_in_session_if_enabled(command_args, &resolved, flags.new);
        execute_resolved_command(resolved, dependency_plan);
    }

    if print_scope_help(&config, command_args) {
        return;
    }

    eprintln!("[fire] Unknown command: {}", command_args[0]);
    print_root_help(&config);
    process::exit(1);
}

fn normalize_completion_words(mut words: Vec<String>, bin_name: &str) -> Vec<String> {
    if words.first().map(String::as_str) == Some("--") {
        words.remove(0);
    }

    if matches!(words.first().map(String::as_str), Some(word) if word == bin_name || word == "fire")
    {
        words.remove(0);
    }

    words
}

fn extract_help_target(command_args: &[String]) -> Option<&[String]> {
    let last = command_args.last()?;
    if last == ":h" {
        return Some(&command_args[..command_args.len().saturating_sub(1)]);
    }
    None
}

fn completion_words_from_env(bin_name: &str) -> Option<Vec<String>> {
    let line = std_env::var("COMP_LINE").ok()?;
    let point = std_env::var("COMP_POINT")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(line.len());

    completion_words_from_line(&line, point, bin_name)
}

fn completion_words_from_line(line: &str, point: usize, bin_name: &str) -> Option<Vec<String>> {
    let prefix = utf8_prefix_at_byte(line, point)?;
    let mut words = split_shell_words(prefix);

    if prefix.chars().last().is_some_and(char::is_whitespace) {
        words.push(String::new());
    }

    Some(normalize_completion_words(words, bin_name))
}

fn utf8_prefix_at_byte(value: &str, index: usize) -> Option<&str> {
    let mut end = index.min(value.len());
    while !value.is_char_boundary(end) {
        if end == 0 {
            return None;
        }
        end -= 1;
    }
    Some(&value[..end])
}

fn split_shell_words(input: &str) -> Vec<String> {
    #[derive(Clone, Copy)]
    enum Quote {
        None,
        Single,
        Double,
    }

    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote = Quote::None;
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        match quote {
            Quote::None => match ch {
                '\'' => quote = Quote::Single,
                '"' => quote = Quote::Double,
                '\\' => {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    } else {
                        current.push(ch);
                    }
                }
                c if c.is_whitespace() => {
                    if !current.is_empty() {
                        words.push(std::mem::take(&mut current));
                    }
                }
                _ => current.push(ch),
            },
            Quote::Single => {
                if ch == '\'' {
                    quote = Quote::None;
                } else {
                    current.push(ch);
                }
            }
            Quote::Double => match ch {
                '"' => quote = Quote::None,
                '\\' => {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    } else {
                        current.push(ch);
                    }
                }
                _ => current.push(ch),
            },
        }
    }

    if !current.is_empty() {
        words.push(current);
    }

    words
}

#[cfg(test)]
mod tests {
    use super::{
        completion_words_from_line, extract_help_target, normalize_completion_words,
        split_shell_words, utf8_prefix_at_byte,
    };

    #[test]
    fn normalize_completion_words_removes_separator_and_binary_name() {
        let words = vec![
            "--".to_string(),
            "fire".to_string(),
            "vars".to_string(),
            "".to_string(),
        ];

        let normalized = normalize_completion_words(words, "fire");
        assert_eq!(normalized, vec!["vars".to_string(), "".to_string()]);
    }

    #[test]
    fn split_shell_words_handles_quotes() {
        let words = split_shell_words("fire run \"start api\" --host 0.0.0.0");
        assert_eq!(
            words,
            vec![
                "fire".to_string(),
                "run".to_string(),
                "start api".to_string(),
                "--host".to_string(),
                "0.0.0.0".to_string()
            ]
        );
    }

    #[test]
    fn utf8_prefix_handles_non_char_boundary() {
        let value = "fire vars";
        let prefix = utf8_prefix_at_byte(value, 100).expect("prefix");
        assert_eq!(prefix, "fire vars");
    }

    #[test]
    fn completion_words_from_line_parses_current_line() {
        let words = completion_words_from_line("fire vars ", 10, "fire").expect("completion words");
        assert_eq!(words, vec!["vars".to_string(), "".to_string()]);
    }

    #[test]
    fn extract_help_target_detects_help_suffix() {
        let args = vec!["run".to_string(), ":h".to_string()];
        let target = extract_help_target(&args).expect("help target");
        assert_eq!(target, &["run".to_string()]);
    }
}
