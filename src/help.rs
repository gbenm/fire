use std::collections::BTreeMap;

use crate::config::{CommandEntry, FileScope, ImplicitNamespaces, LoadedConfig};

pub(crate) fn print_root_help(config: &LoadedConfig) {
    let local_commands = local_commands(config);
    let namespaces = namespaces(config);
    let groups = groups(config);
    let merged = merged_entries(config);
    let global_commands = global_direct_commands(config);
    let builtin_commands = vec![(
        "cli".to_string(),
        Some("Manage command configuration".to_string()),
    )];

    println!("Fire CLI v{}", crate::FIRE_VERSION);
    print_section("Commands", &local_commands);
    print_section("Namespaces", &namespaces);
    print_section("Groups", &groups);
    print_section("Merged Commands", &merged);
    print_section("Global Commands", &global_commands);
    print_section("Built-in Commands", &builtin_commands);
    println!();
    print_section("Flags (before the command)", &global_flags());

    println!();
    println!("{SESSION_HINT}");
    println!("Docs: https://github.com/gbenm/fire");
}

fn global_flags() -> Vec<(String, Option<String>)> {
    crate::flags::GLOBAL_FLAGS
        .iter()
        .filter(|(name, _)| *name != "--note")
        .map(|(name, description)| {
            let name = match *name {
                "-m" => "-m, --note <text>",
                other => other,
            };
            (name.to_string(), Some(description.to_string()))
        })
        .collect()
}

/// Agents often discover commands through help output, so point them at managed sessions here.
const SESSION_HINT: &str = "Long-running (servers, watchers)? Run detached: fire --bg -m \"why\" <command> (see: fire cli ps). In a terminal, Ctrl+Z detaches any fire command.";

pub(crate) fn print_scope_help(config: &LoadedConfig, path: &[String]) -> bool {
    let printed = print_scope_help_inner(config, path);
    if printed {
        println!();
        println!("{SESSION_HINT}");
    }
    printed
}

fn print_scope_help_inner(config: &LoadedConfig, path: &[String]) -> bool {
    match path {
        [namespace] => {
            if !has_namespace(config, namespace) {
                if has_root_group(config, namespace) {
                    print_group_help(config, namespace);
                    return true;
                }
                return false;
            }
            print_namespace_help(config, namespace);
            true
        }
        [namespace, group] if has_namespace_prefix(config, namespace, group) => {
            print_namespace_prefix_help(config, namespace, group);
            true
        }
        _ => false,
    }
}

pub(crate) fn print_command_help(command_path: &[String], command: &CommandEntry) {
    if command_path.is_empty() {
        println!("Fire CLI v{}", crate::FIRE_VERSION);
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

    if !command.allows_agents() {
        println!("Agents: not available (agents: false)");
    }
    if let Some(spec) = command.spec() {
        if !spec.needs.is_empty() {
            println!("Needs: {}", spec.needs.join(", "));
            if spec.parallel {
                println!("Dependencies: parallel");
            }
        }
        if spec.long_running == Some(true) {
            println!(
                "Long-running: starts detached when run without a terminal (long_running: true)"
            );
        }
        if spec.session == Some(false) {
            println!("Sessions: off, always runs directly (session: false)");
        }
    }

    let subcommands = command_subcommands(command);
    print_section("Commands", &subcommands);
    println!();
    println!("{SESSION_HINT}");
}

fn print_namespace_help(config: &LoadedConfig, namespace: &str) {
    let title = format!("Namespace: {namespace}");
    println!("{title}");
    if let Some(description) = namespace_description(config, namespace) {
        println!("Description:");
        for line in description.lines() {
            println!("  {}", line.trim());
        }
    }
    let commands = namespace_commands(config, namespace);
    let groups = namespace_groups(config, namespace);
    print_section("Commands", &commands);
    print_section("Groups", &groups);
}

fn print_group_help(config: &LoadedConfig, group: &str) {
    let title = format!("Group: {group}");
    println!("{title}");
    if let Some(description) = group_description(config, group) {
        println!("Description:");
        for line in description.lines() {
            println!("  {}", line.trim());
        }
    }
    let commands = group_commands(config, group);
    print_section("Commands", &commands);
}

fn print_namespace_prefix_help(config: &LoadedConfig, namespace: &str, group: &str) {
    let title = format!("Namespace Group: {namespace} {group}");
    println!("{title}");
    if let Some(description) = namespace_group_description(config, namespace, group) {
        println!("Description:");
        for line in description.lines() {
            println!("  {}", line.trim());
        }
    }
    let commands = namespace_prefix_commands(config, namespace, group);
    print_section("Commands", &commands);
}

fn print_section(title: &str, items: &[(String, Option<String>)]) {
    if items.is_empty() {
        return;
    }

    println!("{title}:");
    let width = items
        .iter()
        .map(|(name, _)| name.len())
        .max()
        .unwrap_or(0)
        .max(1);

    for (name, description) in items {
        if let Some(description) = description.as_deref() {
            let first_line = description.lines().next().unwrap_or("").trim();
            if first_line.is_empty() {
                println!("  {name}");
            } else {
                println!("  {:width$}  {}", name, first_line, width = width);
            }
        } else {
            println!("  {name}");
        }
    }
}

fn local_commands(config: &LoadedConfig) -> Vec<(String, Option<String>)> {
    let mut map = BTreeMap::new();
    let implicit = ImplicitNamespaces::of(config);
    for file in &config.files {
        match &file.scope {
            FileScope::Root if file.source == crate::config::SourceKind::Local => {
                for (name, entry) in &file.commands {
                    map.insert(name.clone(), optional_description(entry));
                }
            }
            FileScope::Namespace { namespace, .. } => {
                if implicit.is_local(namespace) {
                    for (name, entry) in &file.commands {
                        map.insert(name.clone(), optional_description(entry));
                    }
                }
            }
            FileScope::Group { .. } | FileScope::NamespaceGroup { .. } => {
                // Commands inside groups are NOT displayed at root level.
            }
            FileScope::Root => {}
        }
    }
    map.into_iter().collect()
}

fn namespaces(config: &LoadedConfig) -> Vec<(String, Option<String>)> {
    let mut map = BTreeMap::new();
    for file in &config.files {
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
                map.insert(namespace.clone(), non_empty(namespace_description));
            }
            FileScope::Root | FileScope::Group { .. } => {}
        }
    }
    map.into_iter().collect()
}

fn groups(config: &LoadedConfig) -> Vec<(String, Option<String>)> {
    let mut map = BTreeMap::new();
    let implicit = ImplicitNamespaces::of(config);
    for file in &config.files {
        match &file.scope {
            FileScope::Group {
                group,
                group_description,
            } => {
                map.insert(group.clone(), non_empty(group_description));
            }
            FileScope::NamespaceGroup {
                namespace,
                group,
                group_description,
                ..
            } if implicit.is_local(namespace) => {
                map.insert(group.clone(), non_empty(group_description));
            }
            _ => {}
        }
    }
    map.into_iter().collect()
}

/// Commands and groups of merged namespaces (`fire cli global merge`), labeled with the namespace.
fn merged_entries(config: &LoadedConfig) -> Vec<(String, Option<String>)> {
    ImplicitNamespaces::of(config)
        .merged_entries(config)
        .into_iter()
        .map(|entry| {
            let description = match entry.command {
                Some(command) => optional_description(command),
                None => non_empty(entry.group_description),
            };
            (
                entry.name.to_string(),
                Some(entry.label(description.as_deref())),
            )
        })
        .collect()
}

fn global_direct_commands(config: &LoadedConfig) -> Vec<(String, Option<String>)> {
    let mut map = BTreeMap::new();
    for file in &config.files {
        if file.source != crate::config::SourceKind::Global {
            continue;
        }
        if let FileScope::Root = file.scope {
            for (name, entry) in &file.commands {
                map.insert(name.clone(), optional_description(entry));
            }
        }
    }
    map.into_iter().collect()
}

fn command_subcommands(command: &CommandEntry) -> Vec<(String, Option<String>)> {
    let Some(subcommands) = command.subcommands() else {
        return Vec::new();
    };
    subcommands
        .iter()
        .map(|(name, entry)| (name.clone(), optional_description(entry)))
        .collect()
}

fn namespace_commands(config: &LoadedConfig, namespace: &str) -> Vec<(String, Option<String>)> {
    let mut map = BTreeMap::new();
    for file in &config.files {
        match &file.scope {
            FileScope::Namespace {
                namespace: ns_alias,
                ..
            } => {
                if ns_alias == namespace {
                    for (name, entry) in &file.commands {
                        map.insert(name.clone(), optional_description(entry));
                    }
                }
            }
            FileScope::NamespaceGroup { .. } => {}
            FileScope::Root | FileScope::Group { .. } => {}
        }
    }
    map.into_iter().collect()
}

fn namespace_groups(config: &LoadedConfig, namespace: &str) -> Vec<(String, Option<String>)> {
    let mut map = BTreeMap::new();
    for file in &config.files {
        if let FileScope::NamespaceGroup {
            namespace: ns_alias,
            group,
            group_description,
            ..
        } = &file.scope
        {
            if ns_alias == namespace {
                map.insert(group.clone(), non_empty(group_description));
            }
        }
    }
    map.into_iter().collect()
}

fn group_commands(config: &LoadedConfig, group: &str) -> Vec<(String, Option<String>)> {
    let mut map = BTreeMap::new();
    let implicit = ImplicitNamespaces::of(config);
    for file in &config.files {
        let is_match = match &file.scope {
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

        if is_match {
            for (name, entry) in &file.commands {
                map.insert(name.clone(), optional_description(entry));
            }
        }
    }
    map.into_iter().collect()
}

fn namespace_prefix_commands(
    config: &LoadedConfig,
    namespace: &str,
    group: &str,
) -> Vec<(String, Option<String>)> {
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
                    map.insert(name.clone(), optional_description(entry));
                }
            }
        }
    }
    map.into_iter().collect()
}

fn has_namespace(config: &LoadedConfig, namespace: &str) -> bool {
    config.files.iter().any(|file| {
        matches!(
            &file.scope,
            FileScope::Namespace {
                namespace: ns_alias,
                ..
            } if ns_alias == namespace
        ) || matches!(
            &file.scope,
            FileScope::NamespaceGroup {
                namespace: ns_alias,
                ..
            } if ns_alias == namespace
        )
    })
}

fn has_root_group(config: &LoadedConfig, group: &str) -> bool {
    let implicit = ImplicitNamespaces::of(config);
    config.files.iter().any(|file| match &file.scope {
        FileScope::Group {
            group: file_alias, ..
        } => file_alias == group,
        FileScope::NamespaceGroup {
            namespace,
            group: file_alias,
            ..
        } => file_alias == group && implicit.allows(namespace, group),
        _ => false,
    })
}

fn has_namespace_prefix(config: &LoadedConfig, namespace: &str, group: &str) -> bool {
    config.files.iter().any(|file| {
        matches!(
            &file.scope,
            FileScope::NamespaceGroup {
                namespace: ns_alias,
                group: file_alias,
                ..
            } if ns_alias == namespace && file_alias == group
        )
    })
}

fn namespace_description(config: &LoadedConfig, namespace: &str) -> Option<String> {
    for file in &config.files {
        match &file.scope {
            FileScope::Namespace {
                namespace: ns_alias,
                namespace_description,
            } if ns_alias == namespace => {
                let value = non_empty(namespace_description);
                if value.is_some() {
                    return value;
                }
            }
            FileScope::NamespaceGroup {
                namespace: ns_alias,
                namespace_description,
                ..
            } if ns_alias == namespace => {
                let value = non_empty(namespace_description);
                if value.is_some() {
                    return value;
                }
            }
            _ => {}
        }
    }
    None
}

fn group_description(config: &LoadedConfig, group: &str) -> Option<String> {
    let implicit = ImplicitNamespaces::of(config);
    for file in &config.files {
        match &file.scope {
            FileScope::Group {
                group: file_alias,
                group_description,
            } if file_alias == group => {
                let value = non_empty(group_description);
                if value.is_some() {
                    return value;
                }
            }
            FileScope::NamespaceGroup {
                namespace,
                group: file_alias,
                group_description,
                ..
            } if file_alias == group && implicit.allows(namespace, group) => {
                let value = non_empty(group_description);
                if value.is_some() {
                    return value;
                }
            }
            _ => {}
        }
    }
    None
}

fn namespace_group_description(
    config: &LoadedConfig,
    namespace: &str,
    group: &str,
) -> Option<String> {
    for file in &config.files {
        if let FileScope::NamespaceGroup {
            namespace: ns_alias,
            group: file_alias,
            group_description,
            ..
        } = &file.scope
        {
            if ns_alias == namespace && file_alias == group {
                let value = non_empty(group_description);
                if value.is_some() {
                    return value;
                }
            }
        }
    }
    None
}

fn optional_description(entry: &CommandEntry) -> Option<String> {
    let description = non_empty(entry.description().unwrap_or_default());
    if entry.allows_agents() {
        return description;
    }
    Some(match description {
        Some(description) => format!("{NO_AGENTS_MARK} {description}"),
        None => NO_AGENTS_MARK.to_string(),
    })
}

const NO_AGENTS_MARK: &str = "[no agents]";

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

    fn sample_config() -> LoadedConfig {
        LoadedConfig {
            merged_namespaces: Default::default(),
            files: vec![
                FileConfig {
                    source: SourceKind::Local,
                    project_dir: PathBuf::from("."),
                    config_path: PathBuf::from("/tmp/fire-test.yml"),
                    scope: FileScope::Root,
                    runtimes: BTreeMap::new(),
                    commands: parse_commands(
                        r#"
commands:
  run:
    description: Run local
    exec: npm run
"#,
                    ),
                },
                FileConfig {
                    source: SourceKind::Global,
                    project_dir: PathBuf::from("."),
                    config_path: PathBuf::from("/tmp/fire-test.yml"),
                    scope: FileScope::Namespace {
                        namespace: "ex".to_string(),
                        namespace_description: "Example".to_string(),
                    },
                    runtimes: BTreeMap::new(),
                    commands: parse_commands(
                        r#"
commands:
  api:
    description: API command
    exec: npm run api
"#,
                    ),
                },
                FileConfig {
                    source: SourceKind::Global,
                    project_dir: PathBuf::from("."),
                    config_path: PathBuf::from("/tmp/fire-test.yml"),
                    scope: FileScope::NamespaceGroup {
                        namespace: "ex".to_string(),
                        namespace_description: "Example".to_string(),
                        group: "backend".to_string(),
                        group_description: "Backend commands".to_string(),
                    },
                    runtimes: BTreeMap::new(),
                    commands: parse_commands(
                        r#"
commands:
  deploy:
    description: Deploy command
    exec: npm run deploy
"#,
                    ),
                },
            ],
        }
    }

    #[test]
    fn scope_help_detects_namespace() {
        let config = sample_config();
        assert!(print_scope_help(&config, &["ex".to_string()]));
    }

    #[test]
    fn scope_help_detects_namespace_prefix() {
        let config = sample_config();
        assert!(print_scope_help(
            &config,
            &["ex".to_string(), "backend".to_string()]
        ));
    }

    #[test]
    fn namespace_commands_exclude_namespace_group_commands() {
        let config = sample_config();
        let commands = namespace_commands(&config, "ex");
        let names: Vec<String> = commands.into_iter().map(|(name, _)| name).collect();
        assert_eq!(names, vec!["api".to_string()]);
    }

    #[test]
    fn namespace_groups_include_group_description() {
        let config = sample_config();
        let groups = namespace_groups(&config, "ex");
        assert_eq!(
            groups,
            vec![("backend".to_string(), Some("Backend commands".to_string()))]
        );
    }
    #[test]
    fn merged_namespace_shows_in_root_help_and_group_help() {
        let mut config = sample_config();
        config.merged_namespaces.insert("ex".to_string());
        let merged = merged_entries(&config);
        assert!(!merged.is_empty());
        assert!(merged
            .iter()
            .all(|(_, label)| label.as_deref().is_some_and(|text| text.starts_with("ex"))));
        for (name, _) in &merged {
            assert!(
                print_scope_help(&config, std::slice::from_ref(name))
                    || resolve_target_exists(&config, name)
            );
        }
    }

    fn resolve_target_exists(config: &LoadedConfig, name: &str) -> bool {
        crate::resolve::resolve_command(config, &[name.to_string()]).is_some()
    }
}
