use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Default)]
struct RegistryFile {
    #[serde(default)]
    installed_dirs: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstallResult {
    Added,
    AlreadyInstalled,
}

pub(crate) fn load_installed_directories() -> Vec<PathBuf> {
    let path = registry_path();
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let parsed: RegistryFile = match yaml_serde::from_str(&text) {
        Ok(value) => value,
        Err(_) => return Vec::new(),
    };

    parsed
        .installed_dirs
        .into_iter()
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .collect()
}

pub(crate) fn install_directory(directory: &Path) -> Result<InstallResult, String> {
    let absolute = directory
        .canonicalize()
        .map_err(|err| format!("Cannot resolve directory path: {err}"))?;

    let mut installed = load_installed_directories();
    if installed.iter().any(|path| path == &absolute) {
        return Ok(InstallResult::AlreadyInstalled);
    }

    installed.push(absolute);
    installed.sort();
    installed.dedup();

    write_registry(&installed)
}

fn write_registry(installed: &[PathBuf]) -> Result<InstallResult, String> {
    let parent = registry_path()
        .parent()
        .ok_or_else(|| "Invalid registry path".to_string())?
        .to_path_buf();
    fs::create_dir_all(&parent).map_err(|err| format!("Cannot create config directory: {err}"))?;

    let data = RegistryFile {
        installed_dirs: installed
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect(),
    };
    let content =
        yaml_serde::to_string(&data).map_err(|err| format!("Cannot serialize registry: {err}"))?;
    fs::write(registry_path(), content)
        .map_err(|err| format!("Cannot write registry file: {err}"))?;
    Ok(InstallResult::Added)
}

fn registry_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("fire")
        .join("installed-dirs.yml")
}
