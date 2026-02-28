use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use crate::registry::load_installed_directories;

#[derive(Debug, Clone, Default)]
pub(crate) struct LoadedConfig {
    pub(crate) files: Vec<FileConfig>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceKind {
    Local,
    Global,
}

#[derive(Debug, Clone)]
pub(crate) struct FileConfig {
    pub(crate) source: SourceKind,
    pub(crate) scope: FileScope,
    pub(crate) commands: BTreeMap<String, CommandEntry>,
}

#[derive(Debug, Clone)]
pub(crate) enum FileScope {
    Root,
    Namespace {
        namespace: String,
        namespace_description: String,
    },
    Group {
        group: String,
    },
    NamespaceGroup {
        namespace: String,
        namespace_description: String,
        group: String,
    },
}

#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub(crate) enum CommandEntry {
    Shorthand(String),
    Spec(CommandSpec),
}

#[derive(Debug, Deserialize, Clone, Default)]
pub(crate) struct CommandSpec {
    #[serde(default)]
    pub(crate) description: String,
    #[serde(default)]
    pub(crate) exec: Option<CommandAction>,
    #[serde(default)]
    pub(crate) run: Option<CommandAction>,
    #[serde(default)]
    pub(crate) commands: BTreeMap<String, CommandEntry>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub(crate) enum CommandAction {
    Single(String),
    Multiple(Vec<String>),
}

#[derive(Debug, Deserialize, Clone, Default)]
struct FireFileRaw {
    #[serde(default)]
    group: String,
    #[serde(default)]
    namespace: Option<NamespaceRaw>,
    #[serde(default)]
    commands: BTreeMap<String, CommandEntry>,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct NamespaceRaw {
    #[serde(default)]
    prefix: String,
    #[serde(default)]
    description: String,
}

impl CommandEntry {
    pub(crate) fn description(&self) -> Option<&str> {
        match self {
            CommandEntry::Shorthand(_) => None,
            CommandEntry::Spec(spec) => Some(spec.description.as_str()),
        }
    }

    pub(crate) fn execution_commands(&self) -> Option<Vec<String>> {
        match self {
            CommandEntry::Shorthand(value) => Some(vec![value.clone()]),
            CommandEntry::Spec(spec) => spec
                .exec
                .as_ref()
                .or(spec.run.as_ref())
                .map(CommandAction::as_vec),
        }
    }

    pub(crate) fn subcommands(&self) -> Option<&BTreeMap<String, CommandEntry>> {
        match self {
            CommandEntry::Shorthand(_) => None,
            CommandEntry::Spec(spec) => Some(&spec.commands),
        }
    }
}

impl CommandAction {
    pub(crate) fn as_vec(&self) -> Vec<String> {
        match self {
            CommandAction::Single(command) => vec![command.clone()],
            CommandAction::Multiple(commands) => commands.clone(),
        }
    }
}

pub(crate) fn load_config() -> LoadedConfig {
    let mut loaded = LoadedConfig::default();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    // Local project commands.
    let local_paths = discover_config_files(&cwd);
    let local_raw = parse_raw_files(&local_paths);
    let local_default_namespace = select_local_default_namespace_prefix(&local_raw, &cwd);
    loaded.files.extend(to_file_configs(
        &local_raw,
        SourceKind::Local,
        Some(&local_default_namespace),
    ));

    // Global installed directories. No implicit namespace here:
    // files without namespace/group stay as direct global commands.
    let installed_dirs = load_installed_directories();
    for directory in installed_dirs {
        if directory == cwd {
            continue;
        }
        let paths = discover_config_files(&directory);
        let raw = parse_raw_files(&paths);
        let default_namespace = select_directory_default_namespace_prefix(&raw);
        loaded.files.extend(to_file_configs(
            &raw,
            SourceKind::Global,
            default_namespace.as_deref(),
        ));
    }

    loaded
}

fn parse_raw_files(paths: &[PathBuf]) -> Vec<FireFileRaw> {
    let mut parsed = Vec::new();
    for path in paths {
        let Ok(text) = fs::read_to_string(path) else {
            eprintln!("[fire] Could not read {}. Skipping.", path.display());
            continue;
        };
        let value: FireFileRaw = match serde_yaml::from_str(&text) {
            Ok(value) => value,
            Err(err) => {
                eprintln!(
                    "[fire] Invalid YAML in {}: {}. Skipping.",
                    path.display(),
                    err
                );
                continue;
            }
        };
        parsed.push(value);
    }
    parsed
}

fn to_file_configs(
    raw_files: &[FireFileRaw],
    source: SourceKind,
    default_namespace_prefix: Option<&str>,
) -> Vec<FileConfig> {
    raw_files
        .iter()
        .map(|raw| FileConfig {
            source,
            scope: scope_from_raw(raw, default_namespace_prefix),
            commands: raw.commands.clone(),
        })
        .collect()
}

fn scope_from_raw(raw: &FireFileRaw, default_namespace_prefix: Option<&str>) -> FileScope {
    let group = raw.group.trim();
    let namespace_prefix = raw
        .namespace
        .as_ref()
        .map(|namespace| namespace.prefix.trim())
        .filter(|value| !value.is_empty())
        .or(default_namespace_prefix)
        .unwrap_or("");
    let namespace_description = raw
        .namespace
        .as_ref()
        .map(|namespace| namespace.description.trim())
        .unwrap_or("")
        .to_string();

    match (namespace_prefix.is_empty(), group.is_empty()) {
        (true, true) => FileScope::Root,
        (false, true) => FileScope::Namespace {
            namespace: namespace_prefix.to_string(),
            namespace_description,
        },
        (true, false) => FileScope::Group {
            group: group.to_string(),
        },
        (false, false) => FileScope::NamespaceGroup {
            namespace: namespace_prefix.to_string(),
            namespace_description,
            group: group.to_string(),
        },
    }
}

fn select_directory_default_namespace_prefix(raw_files: &[FireFileRaw]) -> Option<String> {
    for raw in raw_files {
        if let Some(namespace) = &raw.namespace {
            let prefix = namespace.prefix.trim();
            if !prefix.is_empty() {
                return Some(prefix.to_string());
            }
        }
    }
    None
}

fn select_local_default_namespace_prefix(raw_files: &[FireFileRaw], cwd: &Path) -> String {
    if let Some(prefix) = select_directory_default_namespace_prefix(raw_files) {
        return prefix;
    }

    let raw = cwd
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("default");
    let normalized = raw
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    let trimmed = normalized.trim_matches('-');
    if trimmed.is_empty() {
        "default".to_string()
    } else {
        trimmed.to_string()
    }
}

fn discover_config_files(base_dir: impl AsRef<Path>) -> Vec<PathBuf> {
    let mut base_files = Vec::new();
    let mut pattern_files = Vec::new();

    let Ok(entries) = fs::read_dir(base_dir) else {
        return Vec::new();
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };

        if matches!(name, "fire.yml" | "fire.yaml") {
            base_files.push(path);
            continue;
        }

        if name.ends_with(".fire.yml") || name.ends_with(".fire.yaml") {
            pattern_files.push(path);
        }
    }

    base_files.sort();
    pattern_files.sort();
    base_files.extend(pattern_files);
    base_files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_namespace_uses_directory_explicit_prefix() {
        let raw = FireFileRaw::default();
        let scope = scope_from_raw(&raw, Some("ex"));
        match scope {
            FileScope::Namespace { namespace, .. } => assert_eq!(namespace, "ex"),
            _ => panic!("expected namespace scope"),
        }
    }

    #[test]
    fn missing_namespace_without_directory_prefix_stays_root() {
        let raw = FireFileRaw::default();
        let scope = scope_from_raw(&raw, None);
        match scope {
            FileScope::Root => {}
            _ => panic!("expected root scope"),
        }
    }

    #[test]
    fn select_directory_default_namespace_prefix_reads_first_explicit() {
        let files = vec![
            FireFileRaw {
                group: String::new(),
                namespace: Some(NamespaceRaw {
                    prefix: "ex".to_string(),
                    description: String::new(),
                }),
                commands: BTreeMap::new(),
            },
            FireFileRaw {
                group: String::new(),
                namespace: None,
                commands: BTreeMap::new(),
            },
        ];
        let selected = select_directory_default_namespace_prefix(&files);
        assert_eq!(selected, Some("ex".to_string()));
    }

    #[test]
    fn select_local_default_namespace_prefix_falls_back_to_directory_name() {
        let files = vec![FireFileRaw::default()];
        let cwd = PathBuf::from("/tmp/My Project");
        let selected = select_local_default_namespace_prefix(&files, &cwd);
        assert_eq!(selected, "my-project".to_string());
    }
}
