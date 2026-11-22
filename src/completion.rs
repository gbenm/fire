use std::collections::{BTreeMap, BTreeSet};

use crate::config::{CommandEntry, FileScope, ImplicitNamespaces, LoadedConfig, SourceKind};
use crate::flags::{completion_position, CompletionPosition, BG_FLAGS, GLOBAL_FLAGS};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CompletionSuggestion {
    pub(crate) value: String,
    pub(crate) description: Option<String>,
}

pub(crate) fn completion_suggestions(
    config: &LoadedConfig,
    words: &[String],
) -> Vec<CompletionSuggestion> {
    let all: Vec<&'static str> = GLOBAL_FLAGS.iter().map(|(name, _)| *name).collect();
    match completion_position(words, &all) {
        CompletionPosition::Flags(offered) => flag_suggestions(&offered),
        CompletionPosition::NoteValue => Vec::new(),
        CompletionPosition::Command(0) => command_suggestions(config, words),
        CompletionPosition::Command(consumed) => after_flags(config, &words[consumed..]),
    }
}

/// `fire cli bg ...` (deprecated): its flags, then any command, like `fire --bg ...`.
fn cli_bg_suggestions(config: &LoadedConfig, words: &[String]) -> Vec<CompletionSuggestion> {
    match completion_position(words, &BG_FLAGS) {
        CompletionPosition::Flags(offered) => flag_suggestions(&offered),
        CompletionPosition::NoteValue => Vec::new(),
        CompletionPosition::Command(consumed) => after_flags(config, &words[consumed..]),
    }
}

/// Completion after the flags: the root completion, without `cli` (flags only apply to user
/// commands).
fn after_flags(config: &LoadedConfig, words: &[String]) -> Vec<CompletionSuggestion> {
    if words.first().map(String::as_str) == Some("cli") {
        return Vec::new();
    }
    command_suggestions(config, words)
        .into_iter()
        .filter(|suggestion| !(words.len() == 1 && suggestion.value == "cli"))
        .collect()
}

fn flag_suggestions(offered: &[&str]) -> Vec<CompletionSuggestion> {
    GLOBAL_FLAGS
        .iter()
        .filter(|(name, _)| offered.contains(name))
        .map(|(name, description)| CompletionSuggestion {
            value: name.to_string(),
            description: Some(description.to_string()),
        })
        .collect()
}

fn command_suggestions(config: &LoadedConfig, words: &[String]) -> Vec<CompletionSuggestion> {
    if words.len() >= 3 && words[0] == "cli" && words[1] == "bg" {
        return cli_bg_suggestions(config, &words[2..]);
    }

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
            let children = children_for_root_exact(config, &exact.value);
            if !children.is_empty() {
                return children;
            }
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
        let children = children_for_exact_path(config, &exact_path);
        if !children.is_empty() {
            return children;
        }
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
    let core = core_root_commands(prefix);
    let local_commands = local_commands(config, prefix);
    let local_namespaces = local_namespaces(config, prefix);
    let local_groups = local_groups(config, prefix);
    let merged = merged_entries(config, prefix);
    let namespaces = global_namespaces(config, prefix);
    let groups = global_groups(config, prefix);
    let global_commands = global_direct_commands(config, prefix);
    concat_suggestions(vec![
        core,
        local_commands,
        local_namespaces,
        local_groups,
        merged,
        namespaces,
        groups,
        global_commands,
    ])
}

fn children_for_root_exact(config: &LoadedConfig, value: &str) -> Vec<CompletionSuggestion> {
    if value == "cli" {
        return core_cli_children("");
    }

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
    if let Some(core_children) = core_children_for_path(path, prefix) {
        return core_children;
    }

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

fn core_root_commands(prefix: &str) -> Vec<CompletionSuggestion> {
    let command = CompletionSuggestion {
        value: "cli".to_string(),
        description: Some("Fire CLI management commands".to_string()),
    };

    if command.value.starts_with(prefix) {
        vec![command]
    } else {
        Vec::new()
    }
}

fn core_children_for_path(path: &[String], prefix: &str) -> Option<Vec<CompletionSuggestion>> {
    if path.is_empty() || path[0] != "cli" {
        return None;
    }

    if path.len() >= 2 && path[1] == "ps" {
        return Some(core_cli_ps_flags(&path[2..], prefix));
    }

    if path.len() >= 2 && path[1] == "global" {
        return Some(core_cli_global_children(&path[2..], prefix));
    }

    match path.len() {
        1 => Some(core_cli_children(prefix)),
        2 => {
            if path[1] == "completion" {
                Some(core_cli_completion_children(prefix))
            } else {
                Some(Vec::new())
            }
        }
        3 => {
            if path[1] == "completion" && path[2] == "install" {
                Some(core_cli_completion_install_children(prefix))
            } else {
                Some(Vec::new())
            }
        }
        _ => Some(Vec::new()),
    }
}

fn core_cli_children(prefix: &str) -> Vec<CompletionSuggestion> {
    let suggestions = vec![
        CompletionSuggestion {
            value: "global".to_string(),
            description: Some("Register directories so their commands load anywhere".to_string()),
        },
        CompletionSuggestion {
            value: "init".to_string(),
            description: Some("Create a minimal fire config file".to_string()),
        },
        CompletionSuggestion {
            value: "completion".to_string(),
            description: Some("Install shell completion scripts".to_string()),
        },
        CompletionSuggestion {
            value: "upgrade".to_string(),
            description: Some("Upgrade fire via Homebrew (brew installs only)".to_string()),
        },
        CompletionSuggestion {
            value: "mcp".to_string(),
            description: Some("Start an MCP server exposing fire's command tree".to_string()),
        },
        CompletionSuggestion {
            value: "bg".to_string(),
            description: Some("Run a fire command detached".to_string()),
        },
        CompletionSuggestion {
            value: "ps".to_string(),
            description: Some("List detached sessions".to_string()),
        },
        CompletionSuggestion {
            value: "logs".to_string(),
            description: Some("Show a session's output".to_string()),
        },
        CompletionSuggestion {
            value: "fg".to_string(),
            description: Some("Attach to a session".to_string()),
        },
        CompletionSuggestion {
            value: "send".to_string(),
            description: Some("Type into a session".to_string()),
        },
        CompletionSuggestion {
            value: "kill".to_string(),
            description: Some("Stop or signal a session".to_string()),
        },
    ];

    suggestions
        .into_iter()
        .filter(|suggestion| suggestion.value.starts_with(prefix))
        .collect()
}

/// `fire cli global ...`: subcommands, then registered namespaces/directories, then files.
fn core_cli_global_children(typed: &[String], prefix: &str) -> Vec<CompletionSuggestion> {
    let pairs: Vec<(String, String)> = match typed {
        [] => crate::global::SUBCOMMANDS
            .iter()
            .map(|(name, description)| (name.to_string(), description.to_string()))
            .collect(),
        [sub] if sub == "remove" || sub == "edit" => {
            crate::global::target_suggestions(&crate::global::entries())
        }
        [sub] if sub == "merge" => crate::global::merge_suggestions(&crate::global::entries()),
        [sub] if sub == "unmerge" => crate::global::unmerge_suggestions(),
        [sub, target] if sub == "edit" => {
            crate::global::file_suggestions(&crate::global::entries(), target)
        }
        _ => Vec::new(),
    };
    pairs
        .into_iter()
        .filter(|(value, _)| value.starts_with(prefix))
        .map(|(value, description)| CompletionSuggestion {
            value,
            description: Some(description),
        })
        .collect()
}

/// `fire cli ps` filter flags not typed yet.
fn core_cli_ps_flags(typed: &[String], prefix: &str) -> Vec<CompletionSuggestion> {
    crate::session::PS_FLAGS
        .iter()
        .filter(|(flag, _)| flag.starts_with(prefix) && !typed.iter().any(|arg| arg == flag))
        .map(|(flag, description)| CompletionSuggestion {
            value: flag.to_string(),
            description: Some(description.to_string()),
        })
        .collect()
}

fn core_cli_completion_children(prefix: &str) -> Vec<CompletionSuggestion> {
    let suggestions = vec![CompletionSuggestion {
        value: "install".to_string(),
        description: Some("Install completion for bash/zsh".to_string()),
    }];

    suggestions
        .into_iter()
        .filter(|suggestion| suggestion.value.starts_with(prefix))
        .collect()
}

fn core_cli_completion_install_children(prefix: &str) -> Vec<CompletionSuggestion> {
    let suggestions = vec![
        CompletionSuggestion {
            value: "bash".to_string(),
            description: Some("Install bash completion".to_string()),
        },
        CompletionSuggestion {
            value: "zsh".to_string(),
            description: Some("Install zsh completion".to_string()),
        },
        CompletionSuggestion {
            value: "all".to_string(),
            description: Some("Install both shells".to_string()),
        },
    ];

    suggestions
        .into_iter()
        .filter(|suggestion| suggestion.value.starts_with(prefix))
        .collect()
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
    let implicit = ImplicitNamespaces::of(config);
    for file in &config.files {
        match &file.scope {
            FileScope::Root if file.source == SourceKind::Local => {
                for (name, entry) in &file.commands {
                    if name.starts_with(prefix) {
                        map.insert(name.clone(), command_suggestion(name, entry));
                    }
                }
            }
            FileScope::Namespace { namespace, .. } => {
                if implicit.is_local(namespace) {
                    for (name, entry) in &file.commands {
                        if name.starts_with(prefix) {
                            map.insert(name.clone(), command_suggestion(name, entry));
                        }
                    }
                }
            }
            FileScope::Group { .. } | FileScope::NamespaceGroup { .. } => {
                // Hidden from root commands
            }
            FileScope::Root => {}
        }
    }
    map.into_values().collect()
}

/// Commands and groups of merged namespaces as direct entries, labeled with the namespace.
fn merged_entries(config: &LoadedConfig, prefix: &str) -> Vec<CompletionSuggestion> {
    ImplicitNamespaces::of(config)
        .merged_entries(config)
        .into_iter()
        .filter(|entry| entry.name.starts_with(prefix))
        .map(|entry| {
            let description = match entry.command {
                Some(command) => command.description(),
                None => Some(entry.group_description),
            };
            CompletionSuggestion {
                value: entry.name.to_string(),
                description: Some(entry.label(description)),
            }
        })
        .collect()
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
    let mut groups = BTreeMap::new();
    for file in &config.files {
        if file.source != SourceKind::Global {
            continue;
        }
        if let FileScope::Group {
            group,
            group_description,
        } = &file.scope
        {
            if group.starts_with(prefix) {
                groups.insert(group.clone(), non_empty(group_description));
            }
        }
    }
    groups
        .into_iter()
        .map(|(group, description)| CompletionSuggestion {
            value: group,
            description,
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
    let mut map = BTreeMap::new();
    let implicit = ImplicitNamespaces::of(config);
    for file in &config.files {
        match &file.scope {
            FileScope::Group {
                group,
                group_description,
            } => {
                if file.source == SourceKind::Local && group.starts_with(prefix) {
                    map.insert(group.clone(), non_empty(group_description));
                }
            }
            FileScope::NamespaceGroup {
                namespace,
                group,
                group_description,
                ..
            } if implicit.is_local(namespace) && group.starts_with(prefix) => {
                map.insert(group.clone(), non_empty(group_description));
            }
            _ => {}
        }
    }
    map.into_iter()
        .map(|(group, description)| CompletionSuggestion {
            value: group,
            description,
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
    let mut groups = BTreeMap::new();
    for file in &config.files {
        if let FileScope::NamespaceGroup {
            namespace: ns_alias,
            group,
            group_description,
            ..
        } = &file.scope
        {
            if ns_alias == namespace && group.starts_with(prefix) {
                groups.insert(group.clone(), non_empty(group_description));
            }
        }
    }
    groups
        .into_iter()
        .map(|(group, description)| CompletionSuggestion {
            value: group,
            description,
        })
        .collect()
}

fn group_children(config: &LoadedConfig, group: &str, prefix: &str) -> Vec<CompletionSuggestion> {
    let mut map = BTreeMap::new();
    let implicit = ImplicitNamespaces::of(config);
    for file in &config.files {
        match &file.scope {
            FileScope::Group {
                group: file_alias, ..
            } => {
                if file_alias == group {
                    for (name, entry) in &file.commands {
                        if name.starts_with(prefix) {
                            map.insert(name.clone(), command_suggestion(name, entry));
                        }
                    }
                }
            }
            FileScope::NamespaceGroup {
                namespace,
                group: file_alias,
                ..
            } if file_alias == group && implicit.allows(namespace, group) => {
                for (name, entry) in &file.commands {
                    if name.starts_with(prefix) {
                        map.insert(name.clone(), command_suggestion(name, entry));
                    }
                }
            }
            _ => {}
        }
    }
    map.into_values().collect()
}

fn root_command_children(config: &LoadedConfig, command_name: &str) -> Vec<CompletionSuggestion> {
    let implicit = ImplicitNamespaces::of(config);
    let mut map = BTreeMap::new();
    for file in &config.files {
        let root_invocable = match &file.scope {
            FileScope::Root => true,
            FileScope::Namespace { namespace, .. } => implicit.allows(namespace, command_name),
            FileScope::Group { .. } | FileScope::NamespaceGroup { .. } => false,
        };
        if !root_invocable {
            continue;
        }
        if let Some(command) = file.commands.get(command_name) {
            for suggestion in nested_subcommands(command, "") {
                map.entry(suggestion.value.clone()).or_insert(suggestion);
            }
        }
    }
    map.into_values().collect()
}

fn nested_from_root_command(
    config: &LoadedConfig,
    path: &[String],
) -> Option<Vec<CompletionSuggestion>> {
    let root_command = &path[0];
    let implicit = ImplicitNamespaces::of(config);
    let entries = config.files.iter().filter_map(|file| {
        let root_invocable = match &file.scope {
            FileScope::Root => true,
            FileScope::Namespace { namespace, .. } => implicit.allows(namespace, root_command),
            FileScope::Group { .. } | FileScope::NamespaceGroup { .. } => false,
        };
        if !root_invocable {
            return None;
        }
        file.commands.get(root_command)
    });
    merged_nested_path(entries, &path[1..])
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
    let entries = config.files.iter().filter_map(|file| {
        if let FileScope::Namespace {
            namespace: ns_alias,
            ..
        } = &file.scope
        {
            if ns_alias == namespace {
                return file.commands.get(command_name);
            }
        }
        None
    });
    merged_nested_path(entries, &path[2..])
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

    let implicit = ImplicitNamespaces::of(config);
    let entries = config.files.iter().filter_map(|file| {
        let matches = match &file.scope {
            FileScope::Group {
                group: file_alias, ..
            } => file_alias == group,
            FileScope::NamespaceGroup {
                namespace,
                group: file_alias,
                ..
            } => file_alias == group && implicit.allows(namespace, group),
            _ => false,
        };
        if matches {
            return file.commands.get(command_name);
        }
        None
    });
    merged_nested_path(entries, &path[2..])
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
    let entries = config.files.iter().filter_map(|file| {
        if let FileScope::NamespaceGroup {
            namespace: ns_alias,
            group: file_alias,
            ..
        } = &file.scope
        {
            if ns_alias == namespace && file_alias == group {
                return file.commands.get(command_name);
            }
        }
        None
    });
    merged_nested_path(entries, &path[3..])
}

fn merged_nested_path<'a, I>(entries: I, path: &[String]) -> Option<Vec<CompletionSuggestion>>
where
    I: Iterator<Item = &'a CommandEntry>,
{
    let mut matched = false;
    let mut out = BTreeMap::new();

    for entry in entries {
        let mut current = entry;
        let mut valid = true;
        for segment in path {
            let Some(subcommands) = current.subcommands() else {
                valid = false;
                break;
            };
            let Some(next) = subcommands.get(segment) else {
                valid = false;
                break;
            };
            current = next;
        }

        if !valid {
            continue;
        }
        matched = true;
        for suggestion in nested_subcommands(current, "") {
            out.entry(suggestion.value.clone()).or_insert(suggestion);
        }
    }

    if matched {
        Some(out.into_values().collect())
    } else {
        None
    }
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
    use std::path::PathBuf;

    use super::*;
    use crate::config::{FileConfig, FileScope, SourceKind};

    fn config_with_scopes() -> LoadedConfig {
        fn commands(yaml: &str) -> BTreeMap<String, CommandEntry> {
            #[derive(serde::Deserialize)]
            struct Wrapper {
                commands: BTreeMap<String, CommandEntry>,
            }
            yaml_serde::from_str::<Wrapper>(yaml)
                .expect("valid yaml")
                .commands
        }

        LoadedConfig {
            merged_namespaces: Default::default(),
            files: vec![
                FileConfig {
                    source: SourceKind::Local,
                    project_dir: PathBuf::from("."),
                    config_path: PathBuf::from("/tmp/fire-test.yml"),
                    scope: FileScope::Root,
                    runtimes: BTreeMap::new(),
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
                    project_dir: PathBuf::from("."),
                    config_path: PathBuf::from("/tmp/fire-test.yml"),
                    scope: FileScope::Namespace {
                        namespace: "ex".to_string(),
                        namespace_description: "example namespace".to_string(),
                    },
                    runtimes: BTreeMap::new(),
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
                    project_dir: PathBuf::from("."),
                    config_path: PathBuf::from("/tmp/fire-test.yml"),
                    scope: FileScope::Group {
                        group: "backend".to_string(),
                        group_description: "Backend group".to_string(),
                    },
                    runtimes: BTreeMap::new(),
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
                    project_dir: PathBuf::from("."),
                    config_path: PathBuf::from("/tmp/fire-test.yml"),
                    scope: FileScope::Root,
                    runtimes: BTreeMap::new(),
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
                    project_dir: PathBuf::from("."),
                    config_path: PathBuf::from("/tmp/fire-test.yml"),
                    scope: FileScope::NamespaceGroup {
                        namespace: "ex".to_string(),
                        namespace_description: String::new(),
                        group: "ops".to_string(),
                        group_description: String::new(),
                    },
                    runtimes: BTreeMap::new(),
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
        assert_eq!(names, vec!["cli", "dev", "run", "ex", "backend", "ping"]);
    }

    fn names(config: &LoadedConfig, words: &[&str]) -> Vec<String> {
        let words: Vec<String> = words.iter().map(|word| word.to_string()).collect();
        completion_suggestions(config, &words)
            .into_iter()
            .map(|it| it.value)
            .collect()
    }

    #[test]
    fn dash_lists_global_flags_with_descriptions() {
        let config = config_with_scopes();
        let words = vec!["-".to_string()];
        let values = completion_suggestions(&config, &words);
        let flags: Vec<&str> = values.iter().map(|it| it.value.as_str()).collect();
        assert_eq!(
            flags,
            vec![
                "--bg",
                "--new",
                "-m",
                "--note",
                "--fg",
                "--dry-run",
                "--no-log"
            ]
        );
        assert!(values.iter().all(|it| it.description.is_some()));
    }

    #[test]
    fn completion_after_flags_equals_root_completion_without_cli() {
        let config = config_with_scopes();
        let mut root = names(&config, &[""]);
        root.retain(|name| name != "cli");
        assert_eq!(names(&config, &["--bg", ""]), root);
        assert_eq!(names(&config, &["--bg", "-m", "why", "--no-log", ""]), root);
        assert_eq!(
            names(&config, &["--dry-run", "ex", ""]),
            names(&config, &["ex", ""])
        );
        assert_eq!(names(&config, &["--bg", "ex", "ops", "de"]), vec!["deploy"]);
        assert!(names(&config, &["--bg", "cli"]).is_empty());
        assert!(names(&config, &["--bg", "cli", ""]).is_empty());
    }

    #[test]
    fn completion_skips_note_values_and_typed_flags() {
        let config = config_with_scopes();
        assert!(names(&config, &["--bg", "-m", ""]).is_empty());
        assert!(names(&config, &["--note", "d"]).is_empty());
        assert_eq!(
            names(&config, &["--bg", "--"]),
            vec!["--new", "--note", "--dry-run", "--no-log"]
        );
        assert_eq!(names(&config, &["--fg", "--n"]), vec!["--no-log"]);
    }

    #[test]
    fn deprecated_cli_bg_completes_its_flags_then_commands() {
        let config = config_with_scopes();
        assert_eq!(
            names(&config, &["cli", "bg", "-"]),
            vec!["--new", "-m", "--note"]
        );
        let mut root = names(&config, &[""]);
        root.retain(|name| name != "cli");
        assert_eq!(names(&config, &["cli", "bg", "-m", "why", ""]), root);
        assert_eq!(names(&config, &["cli", "bg", "ex", ""]), vec!["api", "ops"]);
    }

    #[test]
    fn root_group_suggestion_includes_description() {
        let config = config_with_scopes();
        let values = completion_suggestions(&config, &[]);
        let backend = values
            .into_iter()
            .find(|it| it.value == "backend")
            .expect("backend group exists");
        assert_eq!(backend.description, Some("Backend group".to_string()));
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
    fn exact_prefix_keeps_siblings_when_exact_has_no_children() {
        fn commands(yaml: &str) -> BTreeMap<String, CommandEntry> {
            #[derive(serde::Deserialize)]
            struct Wrapper {
                commands: BTreeMap<String, CommandEntry>,
            }
            yaml_serde::from_str::<Wrapper>(yaml)
                .expect("valid yaml")
                .commands
        }

        let config = LoadedConfig {
            merged_namespaces: Default::default(),
            files: vec![FileConfig {
                source: SourceKind::Local,
                project_dir: PathBuf::from("."),
                config_path: PathBuf::from("/tmp/fire-test.yml"),
                scope: FileScope::Group {
                    group: "backend".to_string(),
                    group_description: String::new(),
                },
                runtimes: BTreeMap::new(),
                commands: commands(
                    r#"
commands:
  echo: echo "hello"
  echo-eval:
    eval: ts:getEcho()
"#,
                ),
            }],
        };

        let values = completion_suggestions(&config, &["backend".to_string(), "echo".to_string()]);
        let names: Vec<String> = values.into_iter().map(|it| it.value).collect();
        assert_eq!(names, vec!["echo", "echo-eval"]);
    }

    #[test]
    fn local_command_from_namespace_prefix_exposes_nested_completion() {
        fn commands(yaml: &str) -> BTreeMap<String, CommandEntry> {
            #[derive(serde::Deserialize)]
            struct Wrapper {
                commands: BTreeMap<String, CommandEntry>,
            }
            yaml_serde::from_str::<Wrapper>(yaml)
                .expect("valid yaml")
                .commands
        }

        let config = LoadedConfig {
            merged_namespaces: Default::default(),
            files: vec![FileConfig {
                source: SourceKind::Local,
                project_dir: PathBuf::from("."),
                config_path: PathBuf::from("/tmp/fire-test.yml"),
                scope: FileScope::NamespaceGroup {
                    namespace: "ex".to_string(),
                    namespace_description: String::new(),
                    group: "ops".to_string(),
                    group_description: String::new(),
                },
                runtimes: BTreeMap::new(),
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

        // Updated loop to respect mandatory group enforcement.
        // Command "run" in group "ops" must be accessed via "ops run".
        let values = completion_suggestions(&config, &["ops".to_string(), "run".to_string()]);
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

    #[test]
    fn core_cli_global_lists_subcommands() {
        let config = config_with_scopes();
        let values =
            completion_suggestions(&config, &["cli".into(), "global".into(), String::new()]);
        let names: Vec<String> = values.iter().map(|it| it.value.clone()).collect();
        assert_eq!(
            names,
            vec!["add", "remove", "list", "prune", "edit", "merge", "unmerge"]
        );
        assert!(values.iter().all(|it| it.description.is_some()));
    }

    #[test]
    fn core_cli_lists_subcommands() {
        let config = config_with_scopes();
        let values = completion_suggestions(&config, &["cli".to_string()]);
        let names: Vec<String> = values.into_iter().map(|it| it.value).collect();
        assert_eq!(
            names,
            vec![
                "global",
                "init",
                "completion",
                "upgrade",
                "mcp",
                "bg",
                "ps",
                "logs",
                "fg",
                "send",
                "kill"
            ]
        );
    }

    #[test]
    fn core_cli_ps_completes_filter_flags() {
        let config = config_with_scopes();
        let values = completion_suggestions(
            &config,
            &[
                "cli".to_string(),
                "ps".to_string(),
                "--failed".to_string(),
                "--".to_string(),
            ],
        );
        let names: Vec<String> = values.into_iter().map(|it| it.value).collect();
        assert_eq!(names, vec!["--all", "--exited", "--stopped"]);
    }

    #[test]
    fn core_cli_completion_lists_install() {
        let config = config_with_scopes();
        let values = completion_suggestions(
            &config,
            &["cli".to_string(), "completion".to_string(), "".to_string()],
        );
        let names: Vec<String> = values.into_iter().map(|it| it.value).collect();
        assert_eq!(names, vec!["install"]);
    }

    #[test]
    fn core_cli_completion_install_lists_shell_targets() {
        let config = config_with_scopes();
        let values = completion_suggestions(
            &config,
            &[
                "cli".to_string(),
                "completion".to_string(),
                "install".to_string(),
                "".to_string(),
            ],
        );
        let names: Vec<String> = values.into_iter().map(|it| it.value).collect();
        assert_eq!(names, vec!["bash", "zsh", "all"]);
    }

    #[test]
    fn root_completion_includes_namespace_commands_from_global_when_local_namespace_is_active() {
        fn commands(yaml: &str) -> BTreeMap<String, CommandEntry> {
            #[derive(serde::Deserialize)]
            struct Wrapper {
                commands: BTreeMap<String, CommandEntry>,
            }
            yaml_serde::from_str::<Wrapper>(yaml)
                .expect("valid yaml")
                .commands
        }

        let config = LoadedConfig {
            merged_namespaces: Default::default(),
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
                    scope: FileScope::Namespace {
                        namespace: "ex".to_string(),
                        namespace_description: String::new(),
                    },
                    runtimes: BTreeMap::new(),
                    commands: commands(
                        r#"
commands:
  start:
    exec: npm run start
"#,
                    ),
                },
                FileConfig {
                    source: SourceKind::Global,
                    project_dir: PathBuf::from("."),
                    config_path: PathBuf::from("/tmp/fire-test.yml"),
                    scope: FileScope::NamespaceGroup {
                        namespace: "ex".to_string(),
                        namespace_description: String::new(),
                        group: "backend".to_string(),
                        group_description: String::new(),
                    },
                    runtimes: BTreeMap::new(),
                    commands: commands(
                        r#"
commands:
  deploy:
    exec: npm run deploy
"#,
                    ),
                },
            ],
        };

        let root = completion_suggestions(&config, &[]);
        let root_names: Vec<String> = root.into_iter().map(|it| it.value).collect();
        assert!(root_names.contains(&"start".to_string()));
        assert!(root_names.contains(&"backend".to_string()));

        let group = completion_suggestions(&config, &["backend".to_string()]);
        let group_names: Vec<String> = group.into_iter().map(|it| it.value).collect();
        assert_eq!(group_names, vec!["deploy"]);
    }

    #[test]
    fn completion_merges_nested_from_non_terminal_with_terminal_same_name() {
        fn commands(yaml: &str) -> BTreeMap<String, CommandEntry> {
            #[derive(serde::Deserialize)]
            struct Wrapper {
                commands: BTreeMap<String, CommandEntry>,
            }
            yaml_serde::from_str::<Wrapper>(yaml)
                .expect("valid yaml")
                .commands
        }

        let config = LoadedConfig {
            merged_namespaces: Default::default(),
            files: vec![
                FileConfig {
                    source: SourceKind::Local,
                    project_dir: PathBuf::from("."),
                    config_path: PathBuf::from("/tmp/one.fire.yml"),
                    scope: FileScope::Root,
                    runtimes: BTreeMap::new(),
                    commands: commands(
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
                    commands: commands(
                        r#"
commands:
  example: echo "hello world"
"#,
                    ),
                },
            ],
        };

        let children = completion_suggestions(&config, &["example".to_string()]);
        let names: Vec<String> = children.into_iter().map(|it| it.value).collect();
        assert_eq!(names, vec!["nested"]);
    }
    #[test]
    fn merged_namespace_entries_complete_at_root_with_namespace_label() {
        let mut config = config_with_scopes();
        config.merged_namespaces.insert("ex".to_string());

        let values = completion_suggestions(&config, &[]);
        let names: Vec<&str> = values.iter().map(|it| it.value.as_str()).collect();
        assert_eq!(
            names,
            vec!["cli", "dev", "run", "api", "ops", "ex", "backend", "ping"]
        );
        let api = values.iter().find(|it| it.value == "api").unwrap();
        assert_eq!(api.description.as_deref(), Some("ex · api command"));
        let ops = values.iter().find(|it| it.value == "ops").unwrap();
        assert_eq!(ops.description.as_deref(), Some("ex"));

        let nested = completion_suggestions(&config, &["ops".to_string(), String::new()]);
        let nested: Vec<&str> = nested.iter().map(|it| it.value.as_str()).collect();
        assert_eq!(nested, vec!["deploy"]);
    }

    #[test]
    fn merged_namespace_does_not_complete_taken_words() {
        let mut config = config_with_scopes();
        config.merged_namespaces.insert("ex".to_string());
        // `ping` is a global direct command: a merged `ping` must not appear twice
        if let Some(file) = config
            .files
            .iter_mut()
            .find(|file| matches!(&file.scope, FileScope::Namespace { .. }))
        {
            file.commands.insert(
                "ping".to_string(),
                CommandEntry::Shorthand("echo merged".to_string()),
            );
        }
        let values = completion_suggestions(&config, &["pi".to_string()]);
        assert_eq!(values.len(), 1);
        assert_eq!(
            values[0].description.as_deref(),
            Some("global direct command")
        );
    }
}
