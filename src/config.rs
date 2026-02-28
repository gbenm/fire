use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

#[derive(Debug, Deserialize, Clone, Default)]
pub(crate) struct FireConfig {
    #[serde(default)]
    pub(crate) commands: BTreeMap<String, CommandEntry>,
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

pub(crate) fn load_config() -> FireConfig {
    let files = discover_config_files(".");
    if files.is_empty() {
        return FireConfig::default();
    }

    let mut merged = FireConfig::default();
    for file in files {
        let Ok(text) = fs::read_to_string(&file) else {
            eprintln!("[fire] Could not read {}. Skipping.", file.display());
            continue;
        };

        let parsed: FireConfig = match serde_yaml::from_str(&text) {
            Ok(value) => value,
            Err(err) => {
                eprintln!(
                    "[fire] Invalid YAML in {}: {}. Skipping.",
                    file.display(),
                    err
                );
                continue;
            }
        };

        merged.commands.extend(parsed.commands);
    }

    merged
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
