use std::{
    fs,
    path::{Component, Path, PathBuf},
    process::{self, Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Deserialize;

use crate::{
    completion::CompletionSuggestion, execute::resolved_working_directory, resolve::ResolvedCommand,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DriverManifest {
    name: String,
    #[serde(default)]
    build: Vec<String>,
    #[serde(default)]
    handlers: DriverHandlers,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct DriverHandlers {
    completion: Option<DriverHandler>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DriverHandler {
    exec: String,
}

#[derive(Debug, Deserialize)]
struct DriverCandidate {
    value: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    children: Vec<DriverCandidate>,
}

pub(crate) fn handle(args: &[String]) {
    match args {
        [command, source] if command == "install" => install(source),
        [] => print_help(),
        _ => {
            eprintln!("[fire] Usage: fire cli driver install <github-repo-url>");
            process::exit(1);
        }
    }
}

fn print_help() {
    println!("Fire CLI Drivers v{}", crate::FIRE_VERSION);
    println!("Commands:");
    println!("  install <github-repo-url>  Clone, build, and cache a driver repository");
}

fn install(source: &str) {
    let root = drivers_dir();
    fs::create_dir_all(&root).unwrap_or_else(|err| {
        fail(&format!(
            "Failed to create driver directory {}: {err}",
            root.display()
        ))
    });
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let checkout = root.join(format!(".install-{}-{nonce}", process::id()));

    let cloned = Command::new("git")
        .args(["clone", "--depth", "1", "--", source])
        .arg(&checkout)
        .status()
        .unwrap_or_else(|err| fail(&format!("Failed to run git: {err}")));
    if !cloned.success() {
        cleanup(&checkout);
        fail(&format!("Failed to clone driver repository `{source}`"));
    }

    let manifest = match load_manifest(&checkout) {
        Ok(manifest) => manifest,
        Err(err) => {
            cleanup(&checkout);
            fail(&err)
        }
    };
    if let Err(err) = validate_name(&manifest.name) {
        cleanup(&checkout);
        fail(&err);
    }
    let destination = root.join(&manifest.name);
    if destination.exists() {
        cleanup(&checkout);
        fail(&format!(
            "Driver `{}` is already installed at {}",
            manifest.name,
            destination.display()
        ));
    }

    for step in &manifest.build {
        let status = Command::new("sh")
            .args(["-c", step])
            .current_dir(&checkout)
            .status()
            .unwrap_or_else(|err| {
                cleanup(&checkout);
                fail(&format!("Failed to run driver build step `{step}`: {err}"))
            });
        if !status.success() {
            cleanup(&checkout);
            fail(&format!("Driver build step failed: `{step}`"));
        }
    }
    if let Some(handler) = &manifest.handlers.completion {
        if let Err(err) = handler_path(&checkout, &handler.exec) {
            cleanup(&checkout);
            fail(&err);
        }
    }

    fs::rename(&checkout, &destination).unwrap_or_else(|err| {
        cleanup(&checkout);
        fail(&format!(
            "Failed to cache driver at {}: {err}",
            destination.display()
        ))
    });
    println!(
        "Installed driver `{}` at {}",
        manifest.name,
        destination.display()
    );
}

pub(crate) fn execute(resolved: &ResolvedCommand<'_>, name: &str) -> ! {
    let directory = resolved_working_directory(resolved);
    if !directory.is_dir() {
        fail(&format!(
            "Working directory does not exist or is not a directory: {}",
            directory.display()
        ));
    }
    crate::execute::log_driver(name, resolved.remaining_args);
    if crate::flags::env_bool("FIRE_DRY_RUN") == Some(true) {
        process::exit(0);
    }
    let status = Command::new(name)
        .args(resolved.remaining_args)
        .current_dir(&directory)
        .status()
        .unwrap_or_else(|err| fail(&format!("Failed to run driver `{name}`: {err}")));
    process::exit(status.code().unwrap_or(1));
}

pub(crate) fn completion(
    resolved: &ResolvedCommand<'_>,
    name: &str,
    typed_args: &[String],
    prefix: &str,
) -> Vec<CompletionSuggestion> {
    if validate_name(name).is_err() {
        return Vec::new();
    }
    let repository = drivers_dir().join(name);
    let Ok(manifest) = load_manifest(&repository) else {
        return Vec::new();
    };
    if manifest.name != name {
        return Vec::new();
    }
    let Some(handler) = manifest.handlers.completion else {
        return Vec::new();
    };
    let Ok(executable) = handler_path(&repository, &handler.exec) else {
        return Vec::new();
    };
    let output = match Command::new(executable)
        .args(typed_args)
        .current_dir(resolved_working_directory(resolved))
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    {
        Ok(output) if output.status.success() => output,
        _ => return Vec::new(),
    };
    let Ok(candidates) = serde_json::from_slice::<Vec<DriverCandidate>>(&output.stdout) else {
        return Vec::new();
    };
    completion_candidates(candidates, typed_args, prefix)
}

fn completion_candidates(
    mut candidates: Vec<DriverCandidate>,
    typed_args: &[String],
    prefix: &str,
) -> Vec<CompletionSuggestion> {
    let mut traversed = false;
    for argument in typed_args {
        let Some(position) = candidates.iter().position(|item| item.value == *argument) else {
            // A handler may use the arguments to return candidates for the current context
            // directly instead of returning a nested tree from the root.
            if traversed {
                return Vec::new();
            }
            break;
        };
        // A context-sensitive handler returns candidates for the received arguments directly.
        // Do not mistake an equal-valued leaf in that response for a static tree node.
        if candidates[position].children.is_empty() && !traversed {
            break;
        }
        candidates = candidates.swap_remove(position).children;
        traversed = true;
    }
    candidates
        .into_iter()
        .filter(|candidate| candidate.value.starts_with(prefix))
        .map(|candidate| CompletionSuggestion {
            value: candidate.value,
            description: candidate.description,
        })
        .collect()
}

fn load_manifest(repository: &Path) -> Result<DriverManifest, String> {
    let path = repository.join("driver.yaml");
    let contents = fs::read_to_string(&path)
        .map_err(|err| format!("Failed to read {}: {err}", path.display()))?;
    yaml_serde::from_str(&contents)
        .map_err(|err| format!("Invalid driver manifest {}: {err}", path.display()))
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name == "fire"
        || matches!(name, "." | "..")
        || !name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_.".contains(character))
    {
        return Err(format!(
            "Invalid driver name `{name}`; use ASCII letters, numbers, '-', '_' or '.', and not `fire`"
        ));
    }
    Ok(())
}

fn handler_path(repository: &Path, value: &str) -> Result<PathBuf, String> {
    let relative = Path::new(value);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return Err(format!(
            "Driver completion handler `{value}` must be a relative path inside the repository"
        ));
    }
    let path = repository.join(relative);
    if !path.is_file() {
        return Err(format!(
            "Driver completion handler does not exist: {}",
            path.display()
        ));
    }
    Ok(path)
}

fn drivers_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("fire")
        .join("drivers")
}

fn cleanup(path: &Path) {
    let _ = fs::remove_dir_all(path);
}

fn fail(message: &str) -> ! {
    eprintln!("[fire] {message}");
    process::exit(1)
}

#[cfg(test)]
mod tests {
    use super::{completion_candidates, validate_name, DriverCandidate};

    fn candidate(value: &str, children: Vec<DriverCandidate>) -> DriverCandidate {
        DriverCandidate {
            value: value.to_string(),
            description: None,
            children,
        }
    }

    #[test]
    fn context_candidates_are_not_mistaken_for_a_static_tree() {
        let candidates = vec![candidate("serve", vec![]), candidate("api", vec![])];
        let suggestions = completion_candidates(candidates, &["serve".to_string()], "");

        assert_eq!(
            suggestions
                .into_iter()
                .map(|suggestion| suggestion.value)
                .collect::<Vec<_>>(),
            ["serve", "api"]
        );
    }

    #[test]
    fn static_candidate_trees_are_traversed() {
        let candidates = vec![candidate("serve", vec![candidate("api", vec![])])];
        let suggestions = completion_candidates(candidates, &["serve".to_string()], "");

        assert_eq!(suggestions[0].value, "api");
    }

    #[test]
    fn driver_names_are_single_safe_path_components() {
        assert!(validate_name("docker-compose").is_ok());
        assert!(validate_name(".").is_err());
        assert!(validate_name("..").is_err());
        assert!(validate_name("fire").is_err());
    }
}
