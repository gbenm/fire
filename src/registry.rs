//! Directories registered with `fire cli global add` and the namespaces merged with
//! `fire cli global merge` (`installed-dirs.yml`), and the directories whose first-use
//! registration prompt was declined (`declined-dirs.yml`), both in fire's config dir.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Default)]
struct RegistryFile {
    #[serde(default)]
    installed_dirs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    merged_namespaces: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct DeclinedFile {
    #[serde(default)]
    declined_dirs: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstallResult {
    Added,
    AlreadyInstalled,
}

fn load_registry() -> RegistryFile {
    read_yaml::<RegistryFile>(&registry_path()).unwrap_or_default()
}

pub(crate) fn load_installed_directories() -> Vec<PathBuf> {
    absolute_paths(load_registry().installed_dirs)
}

/// Namespaces whose commands can be called without the namespace word.
pub(crate) fn load_merged_namespaces() -> Vec<String> {
    load_registry().merged_namespaces
}

/// Adds or removes `namespace` from the merged set; `false` when it was already so.
pub(crate) fn set_namespace_merged(namespace: &str, merged: bool) -> Result<bool, String> {
    let mut registry = load_registry();
    let present = registry
        .merged_namespaces
        .iter()
        .any(|name| name == namespace);
    if present == merged {
        return Ok(false);
    }
    if merged {
        registry.merged_namespaces.push(namespace.to_string());
        registry.merged_namespaces.sort();
    } else {
        registry.merged_namespaces.retain(|name| name != namespace);
    }
    write_yaml(&registry_path(), &registry)?;
    Ok(true)
}

/// Registers `directory` (already canonical) and clears any declined prompt for it.
pub(crate) fn install_directory(directory: &Path) -> Result<InstallResult, String> {
    let mut installed = load_installed_directories();
    if installed.iter().any(|path| path == directory) {
        return Ok(InstallResult::AlreadyInstalled);
    }

    installed.push(directory.to_path_buf());
    installed.sort();
    installed.dedup();
    save_installed_directories(&installed)?;

    let mut declined = load_declined_directories();
    if declined.iter().any(|path| path == directory) {
        declined.retain(|path| path != directory);
        save_declined_directories(&declined)?;
    }
    Ok(InstallResult::Added)
}

pub(crate) fn save_installed_directories(installed: &[PathBuf]) -> Result<(), String> {
    let mut registry = load_registry();
    registry.installed_dirs = path_strings(installed);
    write_yaml(&registry_path(), &registry)
}

pub(crate) fn load_declined_directories() -> Vec<PathBuf> {
    let Some(parsed) = read_yaml::<DeclinedFile>(&declined_path()) else {
        return Vec::new();
    };
    absolute_paths(parsed.declined_dirs)
}

/// Remembers that fire must not offer to register `directory` again.
pub(crate) fn decline_directory(directory: &Path) -> Result<(), String> {
    let mut declined = load_declined_directories();
    if declined.iter().any(|path| path == directory) {
        return Ok(());
    }
    declined.push(directory.to_path_buf());
    declined.sort();
    save_declined_directories(&declined)
}

fn save_declined_directories(declined: &[PathBuf]) -> Result<(), String> {
    let data = DeclinedFile {
        declined_dirs: path_strings(declined),
    };
    write_yaml(&declined_path(), &data)
}

fn read_yaml<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let text = fs::read_to_string(path).ok()?;
    yaml_serde::from_str(&text).ok()
}

fn write_yaml<T: Serialize>(path: &Path, data: &T) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Invalid registry path".to_string())?;
    fs::create_dir_all(parent).map_err(|err| format!("Cannot create config directory: {err}"))?;
    let content =
        yaml_serde::to_string(data).map_err(|err| format!("Cannot serialize registry: {err}"))?;
    fs::write(path, content).map_err(|err| format!("Cannot write {}: {err}", path.display()))
}

fn absolute_paths(values: Vec<String>) -> Vec<PathBuf> {
    values
        .into_iter()
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .collect()
}

fn path_strings(paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect()
}

fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("fire")
}

fn registry_path() -> PathBuf {
    config_dir().join("installed-dirs.yml")
}

fn declined_path() -> PathBuf {
    config_dir().join("declined-dirs.yml")
}
