use std::collections::BTreeMap;

use crate::config::CommandEntry;

pub(crate) struct ResolvedCommand<'a> {
    pub(crate) command: &'a CommandEntry,
    pub(crate) consumed: usize,
    pub(crate) remaining_args: &'a [String],
}

pub(crate) fn resolve_command<'a>(
    commands: &'a BTreeMap<String, CommandEntry>,
    args: &'a [String],
) -> Option<ResolvedCommand<'a>> {
    let mut consumed = 1;
    let mut current = commands.get(args.first()?)?;

    while consumed < args.len() {
        let Some(subcommands) = current.subcommands() else {
            break;
        };
        if let Some(next) = subcommands.get(&args[consumed]) {
            current = next;
            consumed += 1;
            continue;
        }
        break;
    }

    Some(ResolvedCommand {
        command: current,
        consumed,
        remaining_args: &args[consumed..],
    })
}

#[cfg(test)]
mod tests {
    use crate::config::FireConfig;

    use super::*;

    fn sample_config() -> FireConfig {
        let yaml = r#"
commands:
  run:
    description: run npm script
    exec: npm run
    commands:
      build: npm run build
      test:
        description: run tests
        run: npm run test
"#;
        serde_yaml::from_str(yaml).expect("valid config")
    }

    #[test]
    fn resolves_deepest_subcommand_and_passes_remaining_args() {
        let config = sample_config();
        let args = vec![
            "run".to_string(),
            "build".to_string(),
            "--host".to_string(),
            "0.0.0.0".to_string(),
        ];
        let resolved = resolve_command(&config.commands, &args).expect("resolved");

        let commands = resolved.command.execution_commands().expect("exec");
        assert_eq!(commands, vec!["npm run build".to_string()]);
        assert_eq!(
            resolved.remaining_args,
            &["--host".to_string(), "0.0.0.0".to_string()]
        );
    }

    #[test]
    fn falls_back_to_parent_exec_when_subcommand_does_not_match() {
        let config = sample_config();
        let args = vec![
            "run".to_string(),
            "start".to_string(),
            "--watch".to_string(),
        ];
        let resolved = resolve_command(&config.commands, &args).expect("resolved");

        let commands = resolved.command.execution_commands().expect("exec");
        assert_eq!(commands, vec!["npm run".to_string()]);
        assert_eq!(
            resolved.remaining_args,
            &["start".to_string(), "--watch".to_string()]
        );
    }
}
