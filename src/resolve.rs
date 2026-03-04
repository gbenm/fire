use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use crate::config::{local_implicit_namespace, CommandEntry, LoadedConfig, RuntimeConfig};

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
    let implicit_namespace = local_implicit_namespace(config);

    for file in &config.files {
        for (command_name, command_entry) in &file.commands {
            let Some(base_consumed) =
                scope_match_consumed(&file, command_name, args, implicit_namespace.as_deref())
            else {
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

pub(crate) fn detect_terminal_command_collision(
    config: &LoadedConfig,
    args: &[String],
    consumed: usize,
) -> Result<(), String> {
    let implicit_namespace = local_implicit_namespace(config);
    let mut paths: BTreeSet<PathBuf> = BTreeSet::new();

    for file in &config.files {
        for (command_name, command_entry) in &file.commands {
            let Some(base_consumed) =
                scope_match_consumed(file, command_name, args, implicit_namespace.as_deref())
            else {
                continue;
            };

            let mut candidate_consumed = base_consumed;
            let mut current = command_entry;
            while candidate_consumed < args.len() {
                let Some(subcommands) = current.subcommands() else {
                    break;
                };
                if let Some(next) = subcommands.get(&args[candidate_consumed]) {
                    current = next;
                    candidate_consumed += 1;
                    continue;
                }
                break;
            }

            if candidate_consumed == consumed && current.is_runnable() {
                paths.insert(file.config_path.clone());
            }
        }
    }

    if paths.len() <= 1 {
        return Ok(());
    }

    let invocation = args[..consumed].join(" ");
    let mut message = format!("Duplicate terminal command:\ninvocation: {invocation}");
    for (index, path) in paths.iter().enumerate() {
        message.push('\n');
        message.push_str(&format!("file_{}: {}", index + 1, path.display()));
    }
    Err(message)
}

fn scope_match_consumed(
    file: &crate::config::FileConfig,
    command_name: &str,
    args: &[String],
    implicit_namespace: Option<&str>,
) -> Option<usize> {
    let explicit_match = match &file.scope {
        crate::config::FileScope::Root => {
            if args.first().map(String::as_str) == Some(command_name) {
                Some(1)
            } else {
                None
            }
        }
        crate::config::FileScope::Namespace { namespace, .. } => {
            if args.first().map(String::as_str) == Some(namespace.as_str())
                && args.get(1).map(String::as_str) == Some(command_name)
            {
                Some(2)
            } else {
                None
            }
        }
        crate::config::FileScope::Group { group } => {
            if args.first().map(String::as_str) == Some(group.as_str())
                && args.get(1).map(String::as_str) == Some(command_name)
            {
                Some(2)
            } else {
                None
            }
        }
        crate::config::FileScope::NamespaceGroup {
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

    if explicit_match.is_some() {
        return explicit_match;
    }

    // Implicit namespace matching: when there is one local active namespace,
    // namespace-prefixed commands can omit the namespace token.
    if let Some(active_namespace) = implicit_namespace {
        match &file.scope {
            crate::config::FileScope::Namespace { namespace, .. } => {
                if namespace == active_namespace
                    && args.first().map(String::as_str) == Some(command_name)
                {
                    return Some(1);
                }
            }
            crate::config::FileScope::NamespaceGroup {
                namespace, group, ..
            } => {
                if namespace == active_namespace
                    && args.first().map(String::as_str) == Some(group.as_str())
                    && args.get(1).map(String::as_str) == Some(command_name)
                {
                    return Some(2);
                }
            }
            crate::config::FileScope::Root | crate::config::FileScope::Group { .. } => {}
        }
    }

    None
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

        yaml_serde::from_str::<Wrapper>(yaml)
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
                config_path: PathBuf::from("/tmp/fire-test.yml"),
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
                config_path: PathBuf::from("/tmp/fire-test.yml"),
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
        // Case 1: Local source (Implicit allowed)
        let config = LoadedConfig {
            files: vec![FileConfig {
                source: SourceKind::Local,
                project_dir: PathBuf::from("."),
                config_path: PathBuf::from("/tmp/fire-test.yml"),
                scope: FileScope::NamespaceGroup {
                    namespace: "ex".to_string(),
                    namespace_description: String::new(),
                    group: "backend".to_string(),
                },
                runtimes: BTreeMap::new(),
                commands: parse_commands(yaml),
            }],
        };

        // Local: fire backend api -> works (implicit namespace)
        let args = vec![
            "backend".to_string(),
            "api".to_string(),
            "--watch".to_string(),
        ];
        let resolved = resolve_command(&config, &args).expect("resolved implicit local");
        assert_eq!(resolved.consumed, 2);

        // Case 2: Global source (implicit allowed when local namespace is active)
        let config_global = LoadedConfig {
            files: vec![
                FileConfig {
                    source: SourceKind::Local,
                    project_dir: PathBuf::from("."),
                    config_path: PathBuf::from("/tmp/fire-test.yml"),
                    scope: FileScope::Namespace {
                        namespace: "ex".to_string(),
                        namespace_description: String::new(),
                    },
                    runtimes: BTreeMap::new(),
                    commands: BTreeMap::new(),
                },
                FileConfig {
                    source: SourceKind::Global,
                    project_dir: PathBuf::from("."),
                    config_path: PathBuf::from("/tmp/fire-test.yml"),
                    scope: FileScope::NamespaceGroup {
                        namespace: "ex".to_string(),
                        namespace_description: String::new(),
                        group: "backend".to_string(),
                    },
                    runtimes: BTreeMap::new(),
                    commands: parse_commands(yaml),
                },
            ],
        };

        // Global: fire backend api -> works because local namespace context is "ex"
        let args_global = vec![
            "backend".to_string(),
            "api".to_string(),
            "--watch".to_string(),
        ];
        let resolved_global = resolve_command(&config_global, &args_global)
            .expect("resolved implicit global by local namespace context");
        assert_eq!(resolved_global.consumed, 2);

        // Global: fire ex backend api -> works
        let args_global_explicit = vec![
            "ex".to_string(),
            "backend".to_string(),
            "api".to_string(),
            "--watch".to_string(),
        ];
        let resolved_global_explicit = resolve_command(&config_global, &args_global_explicit)
            .expect("resolved explicit global");
        assert_eq!(resolved_global_explicit.consumed, 3);
    }

    #[test]
    fn detects_terminal_collision_for_global_group_without_namespace() {
        let yaml = r#"
commands:
  example:
    exec: echo one
"#;
        let config = LoadedConfig {
            files: vec![
                FileConfig {
                    source: SourceKind::Local,
                    project_dir: PathBuf::from("."),
                    config_path: PathBuf::from("/tmp/a.fire.yml"),
                    scope: FileScope::Group {
                        group: "common".to_string(),
                    },
                    runtimes: BTreeMap::new(),
                    commands: parse_commands(yaml),
                },
                FileConfig {
                    source: SourceKind::Global,
                    project_dir: PathBuf::from("."),
                    config_path: PathBuf::from("/tmp/b.fire.yml"),
                    scope: FileScope::Group {
                        group: "common".to_string(),
                    },
                    runtimes: BTreeMap::new(),
                    commands: parse_commands(yaml),
                },
            ],
        };

        let args = vec!["common".to_string(), "example".to_string()];
        let resolved = resolve_command(&config, &args).expect("resolved");
        let error = detect_terminal_command_collision(&config, &args, resolved.consumed)
            .expect_err("must report collision");

        assert!(error.contains("Duplicate terminal command"));
        assert!(error.contains("invocation: common example"));
        assert!(error.contains("file_1: /tmp/a.fire.yml"));
        assert!(error.contains("file_2: /tmp/b.fire.yml"));
    }

    #[test]
    fn allows_same_name_when_both_are_non_terminal() {
        let yaml = r#"
commands:
  example:
    commands:
      sub:
        exec: echo one
"#;
        let config = LoadedConfig {
            files: vec![
                FileConfig {
                    source: SourceKind::Local,
                    project_dir: PathBuf::from("."),
                    config_path: PathBuf::from("/tmp/a.fire.yml"),
                    scope: FileScope::Group {
                        group: "common".to_string(),
                    },
                    runtimes: BTreeMap::new(),
                    commands: parse_commands(yaml),
                },
                FileConfig {
                    source: SourceKind::Global,
                    project_dir: PathBuf::from("."),
                    config_path: PathBuf::from("/tmp/b.fire.yml"),
                    scope: FileScope::Group {
                        group: "common".to_string(),
                    },
                    runtimes: BTreeMap::new(),
                    commands: parse_commands(yaml),
                },
            ],
        };

        let args = vec!["common".to_string(), "example".to_string()];
        let resolved = resolve_command(&config, &args).expect("resolved");
        assert!(!resolved.command.is_runnable());
        let check = detect_terminal_command_collision(&config, &args, resolved.consumed);
        assert!(check.is_ok());
    }

    #[test]
    fn detects_collision_between_implicit_namespace_group_and_global_group() {
        let yaml = r#"
commands:
  example:
    exec: echo one
"#;
        let config = LoadedConfig {
            files: vec![
                FileConfig {
                    source: SourceKind::Local,
                    project_dir: PathBuf::from("."),
                    config_path: PathBuf::from("/tmp/local-ns.fire.yml"),
                    scope: FileScope::Namespace {
                        namespace: "ex".to_string(),
                        namespace_description: String::new(),
                    },
                    runtimes: BTreeMap::new(),
                    commands: BTreeMap::new(),
                },
                FileConfig {
                    source: SourceKind::Local,
                    project_dir: PathBuf::from("."),
                    config_path: PathBuf::from("/tmp/local-group.fire.yml"),
                    scope: FileScope::NamespaceGroup {
                        namespace: "ex".to_string(),
                        namespace_description: String::new(),
                        group: "common".to_string(),
                    },
                    runtimes: BTreeMap::new(),
                    commands: parse_commands(yaml),
                },
                FileConfig {
                    source: SourceKind::Global,
                    project_dir: PathBuf::from("."),
                    config_path: PathBuf::from("/tmp/global-group.fire.yml"),
                    scope: FileScope::Group {
                        group: "common".to_string(),
                    },
                    runtimes: BTreeMap::new(),
                    commands: parse_commands(yaml),
                },
            ],
        };

        let args = vec!["common".to_string(), "example".to_string()];
        let resolved = resolve_command(&config, &args).expect("resolved");
        let error = detect_terminal_command_collision(&config, &args, resolved.consumed)
            .expect_err("must report collision");

        assert!(error.contains("invocation: common example"));
        assert!(error.contains("/tmp/local-group.fire.yml"));
        assert!(error.contains("/tmp/global-group.fire.yml"));
    }

    #[test]
    fn terminal_and_non_terminal_same_name_both_work() {
        let config = LoadedConfig {
            files: vec![
                FileConfig {
                    source: SourceKind::Local,
                    project_dir: PathBuf::from("."),
                    config_path: PathBuf::from("/tmp/one.fire.yml"),
                    scope: FileScope::Root,
                    runtimes: BTreeMap::new(),
                    commands: parse_commands(
                        r#"
commands:
  example:
    commands:
      nested: echo "hello world"
"#,
                    ),
                },
                FileConfig {
                    source: SourceKind::Local,
                    project_dir: PathBuf::from("."),
                    config_path: PathBuf::from("/tmp/two.fire.yml"),
                    scope: FileScope::Root,
                    runtimes: BTreeMap::new(),
                    commands: parse_commands(
                        r#"
commands:
  example: echo "hello world"
"#,
                    ),
                },
            ],
        };

        let root_args = vec!["example".to_string()];
        let root_resolved = resolve_command(&config, &root_args).expect("resolve example");
        assert!(root_resolved.command.is_runnable());
        assert!(
            detect_terminal_command_collision(&config, &root_args, root_resolved.consumed).is_ok()
        );

        let nested_args = vec!["example".to_string(), "nested".to_string()];
        let nested_resolved = resolve_command(&config, &nested_args).expect("resolve nested");
        assert_eq!(nested_resolved.consumed, 2);
        assert!(nested_resolved.command.is_runnable());
        assert!(
            detect_terminal_command_collision(&config, &nested_args, nested_resolved.consumed)
                .is_ok()
        );
    }
}
