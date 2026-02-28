use crate::config::CommandEntry;

pub(crate) fn print_command_help(command_path: &[String], command: &CommandEntry) {
    if command_path.is_empty() {
        println!("Fire CLI help");
    } else {
        println!("Command: {}", command_path.join(" "));
    }

    let description = command.description().unwrap_or_default().trim();
    if !description.is_empty() {
        println!("Description:");
        for line in description.lines() {
            println!("  {}", line.trim());
        }
    }

    if let Some(subcommands) = command.subcommands() {
        if !subcommands.is_empty() {
            println!("Subcommands:");
            for (name, entry) in subcommands {
                let description = first_description_line(entry.description().unwrap_or_default());
                if description.is_empty() {
                    println!("  - {name}");
                } else {
                    println!("  - {name}\t{description}");
                }
            }
        }
    }
}

fn first_description_line(description: &str) -> &str {
    description.lines().next().unwrap_or("").trim()
}

#[cfg(test)]
mod tests {
    use crate::config::FireConfig;

    use super::*;

    #[test]
    fn first_description_line_uses_first_line_only() {
        assert_eq!(first_description_line("line one\nline two"), "line one");
    }

    #[test]
    fn print_help_is_stable_with_command_spec() {
        let yaml = r#"
commands:
  run:
    description: Run scripts
    exec: npm run
    commands:
      start:
        description: Start app
        exec: npm run start
"#;
        let config: FireConfig = serde_yaml::from_str(yaml).expect("valid config");
        let command = config.commands.get("run").expect("run command");
        print_command_help(&["run".to_string()], command);
    }
}
