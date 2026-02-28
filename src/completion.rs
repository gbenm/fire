use std::collections::{BTreeMap, BTreeSet};

use crate::config::{CommandEntry, FileScope, LoadedConfig, SourceKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompletionSuggestion {
    pub(crate) value: String,
    pub(crate) description: Option<String>,
}

pub(crate) fn completion_suggestions(
    config: &LoadedConfig,
    words: &[String],
) -> Vec<CompletionSuggestion> {
    if words.is_empty() {
        return root_suggestions(config, "");
    }

    let prefix = words.last().map(String::as_str).unwrap_or("");
    let path = &words[..words.len() - 1];

    if path.is_empty() {
        let suggestions = root_suggestions(config, prefix);
        if let Some(exact) = suggestions
            .iter()
            .find(|suggestion| suggestion.value == prefix)
        {
            return children_for_root_exact(config, &exact.value);
        }
        return suggestions;
    }

    let suggestions = children_for_path(config, path, prefix);
    if let Some(exact) = suggestions
        .iter()
        .find(|suggestion| suggestion.value == prefix)
    {
        let mut exact_path = path.to_vec();
        exact_path.push(exact.value.clone());
        return children_for_exact_path(config, &exact_path);
    }
    suggestions
}

pub(crate) fn render_with_descriptions(suggestions: &[CompletionSuggestion]) -> Vec<String> {
    suggestions
        .iter()
        .map(|suggestion| match suggestion.description.as_deref() {
            Some(description) => {
                format!(
                    "{}\t{}",
                    suggestion.value,
                    first_description_line(description)
                )
            }
            None => suggestion.value.clone(),
        })
        .collect()
}

pub(crate) fn render_values_only(suggestions: &[CompletionSuggestion]) -> Vec<String> {
    suggestions
        .iter()
        .map(|suggestion| suggestion.value.clone())
        .collect()
}

fn root_suggestions(config: &LoadedConfig, prefix: &str) -> Vec<CompletionSuggestion> {
    let local_commands = local_commands(config, prefix);
    let local_namespaces = local_namespaces(config, prefix);
    let local_groups = local_groups(config, prefix);
    let namespaces = global_namespaces(config, prefix);
    let groups = global_groups(config, prefix);
    let global_commands = global_direct_commands(config, prefix);
    concat_suggestions(vec![
        local_commands,
        local_namespaces,
        local_groups,
        namespaces,
        groups,
        global_commands,
    ])
}

fn children_for_root_exact(config: &LoadedConfig, value: &str) -> Vec<CompletionSuggestion> {
    let command_children = root_command_children(config, value);
    if !command_children.is_empty() {
        return command_children;
    }

    let namespace_children = namespace_children(config, value, "");
    if !namespace_children.is_empty() {
        return namespace_children;
    }

    group_children(config, value, "")
}

fn children_for_path(
    config: &LoadedConfig,
    path: &[String],
    prefix: &str,
) -> Vec<CompletionSuggestion> {
    if path.len() == 1 {
        let head = &path[0];
        let command_children = root_command_children(config, head);
        if !command_children.is_empty() {
            return filter_prefix(prefix, command_children);
        }

        let namespace_children = namespace_children(config, head, prefix);
        if !namespace_children.is_empty() {
            return namespace_children;
        }

        return group_children(config, head, prefix);
    }

    let candidates = children_for_exact_path(config, path);
    filter_prefix(prefix, candidates)
}

fn children_for_exact_path(config: &LoadedConfig, path: &[String]) -> Vec<CompletionSuggestion> {
    if path.is_empty() {
        return root_suggestions(config, "");
    }

    if path.len() == 1 {
        return children_for_root_exact(config, &path[0]);
    }

    if let Some(suggestions) = nested_from_root_command(config, path) {
        return suggestions;
    }
    if let Some(suggestions) = nested_from_namespace_scope(config, path) {
        return suggestions;
    }
    if let Some(suggestions) = nested_from_group_scope(config, path) {
        return suggestions;
    }
    if let Some(suggestions) = nested_from_namespace_prefix_scope(config, path) {
        return suggestions;
    }

    Vec::new()
}

fn local_commands(config: &LoadedConfig, prefix: &str) -> Vec<CompletionSuggestion> {
    let mut map = BTreeMap::new();
    for file in &config.files {
        if file.source != SourceKind::Local {
            continue;
        }
        for (name, entry) in &file.commands {
            if name.starts_with(prefix) {
                map.insert(name.clone(), command_suggestion(name, entry));
            }
        }
    }
    map.into_values().collect()
}

fn global_namespaces(config: &LoadedConfig, prefix: &str) -> Vec<CompletionSuggestion> {
    let mut map = BTreeMap::new();
    for file in &config.files {
        if file.source != SourceKind::Global {
            continue;
        }
        match &file.scope {
            FileScope::Namespace {
                namespace,
                namespace_description,
            }
            | FileScope::NamespaceGroup {
                namespace,
                namespace_description,
                ..
            } => {
                if namespace.starts_with(prefix) {
                    map.insert(
                        namespace.clone(),
                        CompletionSuggestion {
                            value: namespace.clone(),
                            description: non_empty(namespace_description),
                        },
                    );
                }
            }
            FileScope::Root | FileScope::Group { .. } => {}
        }
    }
    map.into_values().collect()
}

fn global_groups(config: &LoadedConfig, prefix: &str) -> Vec<CompletionSuggestion> {
    let mut groups = BTreeSet::new();
    for file in &config.files {
        if file.source != SourceKind::Global {
            continue;
        }
        if let FileScope::Group { group } = &file.scope {
            if group.starts_with(prefix) {
                groups.insert(group.clone());
            }
        }
    }
    groups
        .into_iter()
        .map(|group| CompletionSuggestion {
            value: group,
            description: None,
        })
        .collect()
}

fn global_direct_commands(config: &LoadedConfig, prefix: &str) -> Vec<CompletionSuggestion> {
    let mut map = BTreeMap::new();
    for file in &config.files {
        if file.source != SourceKind::Global {
            continue;
        }
        if let FileScope::Root = file.scope {
            for (name, entry) in &file.commands {
                if name.starts_with(prefix) {
                    map.insert(name.clone(), command_suggestion(name, entry));
                }
            }
        }
    }
    map.into_values().collect()
}

fn local_namespaces(config: &LoadedConfig, prefix: &str) -> Vec<CompletionSuggestion> {
    let mut map = BTreeMap::new();
    for file in &config.files {
        if file.source != SourceKind::Local {
            continue;
        }
        match &file.scope {
            FileScope::Namespace {
                namespace,
                namespace_description,
            }
            | FileScope::NamespaceGroup {
                namespace,
                namespace_description,
                ..
            } => {
                if namespace.starts_with(prefix) {
                    map.insert(
                        namespace.clone(),
                        CompletionSuggestion {
                            value: namespace.clone(),
                            description: non_empty(namespace_description),
                        },
                    );
                }
            }
            FileScope::Root | FileScope::Group { .. } => {}
        }
    }
    map.into_values().collect()
}

fn local_groups(config: &LoadedConfig, prefix: &str) -> Vec<CompletionSuggestion> {
    let mut set = BTreeSet::new();
    for file in &config.files {
        if file.source != SourceKind::Local {
            continue;
        }
        if let FileScope::Group { group } = &file.scope {
            if group.starts_with(prefix) {
                set.insert(group.clone());
            }
        }
    }
    set.into_iter()
        .map(|group| CompletionSuggestion {
            value: group,
            description: None,
        })
        .collect()
}

fn namespace_children(
    config: &LoadedConfig,
    namespace: &str,
    prefix: &str,
) -> Vec<CompletionSuggestion> {
    let commands = namespace_commands(config, namespace, prefix);
    let groups = namespace_groups(config, namespace, prefix);
    concat_suggestions(vec![commands, groups])
}

fn namespace_commands(
    config: &LoadedConfig,
    namespace: &str,
    prefix: &str,
) -> Vec<CompletionSuggestion> {
    let mut map = BTreeMap::new();
    for file in &config.files {
        if let FileScope::Namespace {
            namespace: ns_alias,
            ..
        } = &file.scope
        {
            if ns_alias == namespace {
                for (name, entry) in &file.commands {
                    if name.starts_with(prefix) {
                        map.insert(name.clone(), command_suggestion(name, entry));
                    }
                }
            }
        }
    }
    map.into_values().collect()
}

fn namespace_groups(
    config: &LoadedConfig,
    namespace: &str,
    prefix: &str,
) -> Vec<CompletionSuggestion> {
    let mut groups = BTreeSet::new();
    for file in &config.files {
        if let FileScope::NamespaceGroup {
            namespace: ns_alias,
            group,
            ..
        } = &file.scope
        {
            if ns_alias == namespace && group.starts_with(prefix) {
                groups.insert(group.clone());
            }
        }
    }
    groups
        .into_iter()
        .map(|group| CompletionSuggestion {
            value: group,
            description: None,
        })
        .collect()
}

fn group_children(config: &LoadedConfig, group: &str, prefix: &str) -> Vec<CompletionSuggestion> {
    let mut map = BTreeMap::new();
    for file in &config.files {
        if let FileScope::Group { group: file_alias } = &file.scope {
            if file_alias == group {
                for (name, entry) in &file.commands {
                    if name.starts_with(prefix) {
                        map.insert(name.clone(), command_suggestion(name, entry));
                    }
                }
            }
        }
    }
    map.into_values().collect()
}

fn root_command_children(config: &LoadedConfig, command_name: &str) -> Vec<CompletionSuggestion> {
    for file in config.files.iter().rev() {
        if let Some(command) = file.commands.get(command_name) {
            return nested_subcommands(command, "");
        }
    }
    Vec::new()
}

fn nested_from_root_command(
    config: &LoadedConfig,
    path: &[String],
) -> Option<Vec<CompletionSuggestion>> {
    let root_command = &path[0];
    let mut command = None;
    for file in config.files.iter().rev() {
        if let Some(candidate) = file.commands.get(root_command) {
            command = Some(candidate);
            break;
        }
    }
    let command = command?;
    Some(nested_command_path(command, &path[1..]))
}

fn nested_from_namespace_scope(
    config: &LoadedConfig,
    path: &[String],
) -> Option<Vec<CompletionSuggestion>> {
    if path.len() < 2 {
        return None;
    }
    let namespace = &path[0];
    let command_name = &path[1];
    let mut command = None;
    for file in config.files.iter().rev() {
        if let FileScope::Namespace {
            namespace: ns_alias,
            ..
        } = &file.scope
        {
            if ns_alias == namespace {
                if let Some(candidate) = file.commands.get(command_name) {
                    command = Some(candidate);
                    break;
                }
            }
        }
    }
    let command = command?;
    Some(nested_command_path(command, &path[2..]))
}

fn nested_from_group_scope(
    config: &LoadedConfig,
    path: &[String],
) -> Option<Vec<CompletionSuggestion>> {
    if path.len() < 2 {
        return None;
    }
    let group = &path[0];
    let command_name = &path[1];
    let mut command = None;
    for file in config.files.iter().rev() {
        if let FileScope::Group { group: file_alias } = &file.scope {
            if file_alias == group {
                if let Some(candidate) = file.commands.get(command_name) {
                    command = Some(candidate);
                    break;
                }
            }
        }
    }
    let command = command?;
    Some(nested_command_path(command, &path[2..]))
}

fn nested_from_namespace_prefix_scope(
    config: &LoadedConfig,
    path: &[String],
) -> Option<Vec<CompletionSuggestion>> {
    if path.len() < 2 {
        return None;
    }
    let namespace = &path[0];
    let group = &path[1];

    if path.len() == 2 {
        let mut map = BTreeMap::new();
        for file in &config.files {
            if let FileScope::NamespaceGroup {
                namespace: ns_alias,
                group: file_alias,
                ..
            } = &file.scope
            {
                if ns_alias == namespace && file_alias == group {
                    for (name, entry) in &file.commands {
                        map.insert(name.clone(), command_suggestion(name, entry));
                    }
                }
            }
        }
        if map.is_empty() {
            return None;
        }
        return Some(map.into_values().collect());
    }

    let command_name = &path[2];
    let mut command = None;
    for file in config.files.iter().rev() {
        if let FileScope::NamespaceGroup {
            namespace: ns_alias,
            group: file_alias,
            ..
        } = &file.scope
        {
            if ns_alias == namespace && file_alias == group {
                if let Some(candidate) = file.commands.get(command_name) {
                    command = Some(candidate);
                    break;
                }
            }
        }
    }
    let command = command?;
    Some(nested_command_path(command, &path[3..]))
}

fn nested_command_path(command: &CommandEntry, path: &[String]) -> Vec<CompletionSuggestion> {
    if path.is_empty() {
        return nested_subcommands(command, "");
    }

    let mut current = command;
    for segment in path {
        let Some(subcommands) = current.subcommands() else {
            return Vec::new();
        };
        let Some(next) = subcommands.get(segment) else {
            return Vec::new();
        };
        current = next;
    }
    nested_subcommands(current, "")
}

fn nested_subcommands(command: &CommandEntry, prefix: &str) -> Vec<CompletionSuggestion> {
    let Some(subcommands) = command.subcommands() else {
        return Vec::new();
    };
    subcommands
        .iter()
        .filter_map(|(name, entry)| {
            if !name.starts_with(prefix) {
                return None;
            }
            Some(command_suggestion(name, entry))
        })
        .collect()
}

fn command_suggestion(name: &str, entry: &CommandEntry) -> CompletionSuggestion {
    CompletionSuggestion {
        value: name.to_string(),
        description: non_empty(entry.description().unwrap_or_default()),
    }
}

fn filter_prefix(
    prefix: &str,
    suggestions: Vec<CompletionSuggestion>,
) -> Vec<CompletionSuggestion> {
    suggestions
        .into_iter()
        .filter(|suggestion| suggestion.value.starts_with(prefix))
        .collect()
}

fn concat_suggestions(groups: Vec<Vec<CompletionSuggestion>>) -> Vec<CompletionSuggestion> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for group in groups {
        for suggestion in group {
            if seen.insert(suggestion.value.clone()) {
                out.push(suggestion);
            }
        }
    }
    out
}

fn first_description_line(description: &str) -> &str {
    description.lines().next().unwrap_or("").trim()
}

fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{FileConfig, FileScope, SourceKind};

    fn config_with_scopes() -> LoadedConfig {
        fn commands(yaml: &str) -> BTreeMap<String, CommandEntry> {
            #[derive(serde::Deserialize)]
            struct Wrapper {
                commands: BTreeMap<String, CommandEntry>,
            }
            serde_yaml::from_str::<Wrapper>(yaml)
                .expect("valid yaml")
                .commands
        }

        LoadedConfig {
            files: vec![
                FileConfig {
                    source: SourceKind::Local,
                    scope: FileScope::Root,
                    commands: commands(
                        r#"
commands:
  run:
    description: run local
    exec: npm run
  dev:
    description: local dev
    exec: npm run dev
"#,
                    ),
                },
                FileConfig {
                    source: SourceKind::Global,
                    scope: FileScope::Namespace {
                        namespace: "ex".to_string(),
                        namespace_description: "example namespace".to_string(),
                    },
                    commands: commands(
                        r#"
commands:
  api:
    description: api command
    exec: npm run api
"#,
                    ),
                },
                FileConfig {
                    source: SourceKind::Global,
                    scope: FileScope::Group {
                        group: "backend".to_string(),
                    },
                    commands: commands(
                        r#"
commands:
  start:
    description: start service
    exec: npm run start
"#,
                    ),
                },
                FileConfig {
                    source: SourceKind::Global,
                    scope: FileScope::Root,
                    commands: commands(
                        r#"
commands:
  ping:
    description: global direct command
    exec: echo ping
"#,
                    ),
                },
                FileConfig {
                    source: SourceKind::Global,
                    scope: FileScope::NamespaceGroup {
                        namespace: "ex".to_string(),
                        namespace_description: String::new(),
                        group: "ops".to_string(),
                    },
                    commands: commands(
                        r#"
commands:
  deploy:
    description: deploy service
    exec: npm run deploy
"#,
                    ),
                },
            ],
        }
    }

    #[test]
    fn root_suggestions_respect_priority_groups() {
        let config = config_with_scopes();
        let values = completion_suggestions(&config, &[]);
        let names: Vec<String> = values.into_iter().map(|it| it.value).collect();
        assert_eq!(names, vec!["dev", "run", "ex", "backend", "ping"]);
    }

    #[test]
    fn namespace_lists_commands_and_nested_groups() {
        let config = config_with_scopes();
        let values = completion_suggestions(&config, &["ex".to_string()]);
        let names: Vec<String> = values.into_iter().map(|it| it.value).collect();
        assert_eq!(names, vec!["api", "ops"]);
    }

    #[test]
    fn namespace_prefix_lists_only_scoped_commands() {
        let config = config_with_scopes();
        let values = completion_suggestions(&config, &["ex".to_string(), "ops".to_string()]);
        let names: Vec<String> = values.into_iter().map(|it| it.value).collect();
        assert_eq!(names, vec!["deploy"]);
    }

    #[test]
    fn namespace_prefix_filters_commands_by_prefix() {
        let config = config_with_scopes();
        let values = completion_suggestions(
            &config,
            &["ex".to_string(), "ops".to_string(), "de".to_string()],
        );
        let names: Vec<String> = values.into_iter().map(|it| it.value).collect();
        assert_eq!(names, vec!["deploy"]);
    }

    #[test]
    fn local_command_from_namespace_prefix_exposes_nested_completion() {
        fn commands(yaml: &str) -> BTreeMap<String, CommandEntry> {
            #[derive(serde::Deserialize)]
            struct Wrapper {
                commands: BTreeMap<String, CommandEntry>,
            }
            serde_yaml::from_str::<Wrapper>(yaml)
                .expect("valid yaml")
                .commands
        }

        let config = LoadedConfig {
            files: vec![FileConfig {
                source: SourceKind::Local,
                scope: FileScope::NamespaceGroup {
                    namespace: "ex".to_string(),
                    namespace_description: String::new(),
                    group: "ops".to_string(),
                },
                commands: commands(
                    r#"
commands:
  run:
    exec: npm run
    commands:
      start: npm run start
      test: npm run test
"#,
                ),
            }],
        };

        let values = completion_suggestions(&config, &["run".to_string()]);
        let names: Vec<String> = values.into_iter().map(|it| it.value).collect();
        assert_eq!(names, vec!["start", "test"]);
    }

    #[test]
    fn render_with_descriptions_uses_only_first_line() {
        let values = vec![CompletionSuggestion {
            value: "run".to_string(),
            description: Some("run service\nwith host".to_string()),
        }];
        assert_eq!(
            render_with_descriptions(&values),
            vec!["run\trun service".to_string()]
        );
    }
}
