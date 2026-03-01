use std::{collections::BTreeMap, path::Path};

use crate::config::{CommandEntry, FileScope, LoadedConfig, RuntimeConfig};

pub(crate) struct ResolvedCommand<'a> {
    pub(crate) project_dir: &'a Path,
    pub(crate) runtimes: &'a BTreeMap<String, RuntimeConfig>,
    pub(crate) command: &'a CommandEntry,
    pub(crate) command_chain: Vec<&'a CommandEntry>,
    pub(crate) consumed: usize,
    pub(crate) remaining_args: &'a [String],
}

pub(crate) fn resolve_command<'a>(
    config: &'a LoadedConfig,
    args: &'a [String],
) -> Option<ResolvedCommand<'a>> {
    let mut best: Option<ResolvedCommand<'a>> = None;

    for file in &config.files {
        for (command_name, command_entry) in &file.commands {
            let Some(base_consumed) = scope_match_consumed(&file.scope, command_name, args) else {
                continue;
            };

            let mut consumed = base_consumed;
            let mut current = command_entry;
            let mut chain = vec![command_entry];

            while consumed < args.len() {
                let Some(subcommands) = current.subcommands() else {
                    break;
                };
                if let Some(next) = subcommands.get(&args[consumed]) {
                    current = next;
                    chain.push(current);
                    consumed += 1;
                    continue;
                }
                break;
            }

            let candidate = ResolvedCommand {
                project_dir: &file.project_dir,
                runtimes: &file.runtimes,
                command: current,
                command_chain: chain,
                consumed,
                remaining_args: &args[consumed..],
            };

            if better_than(file.source, &candidate, best.as_ref()) {
                best = Some(candidate);
            }
        }
    }

    best
}

fn scope_match_consumed(scope: &FileScope, command_name: &str, args: &[String]) -> Option<usize> {
    let local_match = if args.first().map(String::as_str) == Some(command_name) {
        Some(1)
    } else {
        None
    };

    let scoped_match = match scope {
        FileScope::Root => {
            if args.first().map(String::as_str) == Some(command_name) {
                Some(1)
            } else {
                None
            }
        }
        FileScope::Namespace { namespace, .. } => {
            if args.first().map(String::as_str) == Some(namespace.as_str())
                && args.get(1).map(String::as_str) == Some(command_name)
            {
                Some(2)
            } else {
                None
            }
        }
        FileScope::Group { group } => {
            if args.first().map(String::as_str) == Some(group.as_str())
                && args.get(1).map(String::as_str) == Some(command_name)
            {
                Some(2)
            } else {
                None
            }
        }
        FileScope::NamespaceGroup {
            namespace, group, ..
        } => {
            if args.first().map(String::as_str) == Some(namespace.as_str())
                && args.get(1).map(String::as_str) == Some(group.as_str())
                && args.get(2).map(String::as_str) == Some(command_name)
            {
                Some(3)
            } else {
                None
            }
        }
    };

    scoped_match.or(local_match)
}

fn better_than(
    source: crate::config::SourceKind,
    candidate: &ResolvedCommand<'_>,
    current: Option<&ResolvedCommand<'_>>,
) -> bool {
    match current {
        None => true,
        Some(existing) => {
            if candidate.consumed != existing.consumed {
                candidate.consumed > existing.consumed
            } else {
                source == crate::config::SourceKind::Local
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, path::PathBuf};

    use crate::config::{CommandEntry, FileConfig, FileScope, LoadedConfig, SourceKind};

    use super::*;

    fn parse_commands(yaml: &str) -> BTreeMap<String, CommandEntry> {
        #[derive(serde::Deserialize)]
        struct Wrapper {
            commands: BTreeMap<String, CommandEntry>,
        }

        serde_yaml::from_str::<Wrapper>(yaml)
            .expect("valid yaml")
            .commands
    }

    #[test]
    fn resolves_root_command_without_scope() {
        let yaml = r#"
commands:
  run:
    description: run npm script
    exec: npm run
"#;
        let config = LoadedConfig {
            files: vec![FileConfig {
                source: SourceKind::Local,
                project_dir: PathBuf::from("."),
                scope: FileScope::Root,
                runtimes: BTreeMap::new(),
                commands: parse_commands(yaml),
            }],
        };
        let args = vec!["run".to_string(), "start".to_string()];
        let resolved = resolve_command(&config, &args).expect("resolved");

        let commands = resolved.command.execution_commands().expect("exec");
        assert_eq!(commands, vec!["npm run".to_string()]);
        assert_eq!(resolved.remaining_args, &["start".to_string()]);
    }

    #[test]
    fn resolves_namespace_prefix_command() {
        let yaml = r#"
commands:
  api:
    exec: npm run api
"#;
        let config = LoadedConfig {
            files: vec![FileConfig {
                source: SourceKind::Global,
                project_dir: PathBuf::from("."),
                scope: FileScope::NamespaceGroup {
                    namespace: "ex".to_string(),
                    namespace_description: String::new(),
                    group: "backend".to_string(),
                },
                runtimes: BTreeMap::new(),
                commands: parse_commands(yaml),
            }],
        };

        let args = vec![
            "ex".to_string(),
            "backend".to_string(),
            "api".to_string(),
            "--watch".to_string(),
        ];
        let resolved = resolve_command(&config, &args).expect("resolved");

        assert_eq!(resolved.consumed, 3);
        assert_eq!(resolved.remaining_args, &["--watch".to_string()]);
    }

    #[test]
    fn resolves_namespace_prefix_command_from_local_root_without_prefix() {
        let yaml = r#"
commands:
  api:
    exec: npm run api
"#;
        let config = LoadedConfig {
            files: vec![FileConfig {
                source: SourceKind::Global,
                project_dir: PathBuf::from("."),
                scope: FileScope::NamespaceGroup {
                    namespace: "ex".to_string(),
                    namespace_description: String::new(),
                    group: "backend".to_string(),
                },
                runtimes: BTreeMap::new(),
                commands: parse_commands(yaml),
            }],
        };

        let args = vec!["api".to_string(), "--watch".to_string()];
        let resolved = resolve_command(&config, &args).expect("resolved");

        assert_eq!(resolved.consumed, 1);
        assert_eq!(resolved.remaining_args, &["--watch".to_string()]);
    }
}
