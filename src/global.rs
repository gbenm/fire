//! `fire cli global ...`: directories whose commands load from anywhere, plus the first-use
//! offer to register the current one.

use std::{
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    process::{self, Command},
};

use crate::config::{
    directory_config_files, global_directory_files, local_project_root, LoadedConfig, SourceKind,
};
use crate::registry::{
    decline_directory, install_directory, load_declined_directories, load_installed_directories,
    load_merged_namespaces, save_installed_directories, set_namespace_merged, InstallResult,
};

pub(crate) const SUBCOMMANDS: &[(&str, &str)] = &[
    (
        "add",
        "Register a directory (default: current) so its commands load from anywhere",
    ),
    (
        "remove",
        "Unregister a directory, by namespace or path (files are kept)",
    ),
    (
        "list",
        "Registered directories by namespace, with their config files",
    ),
    ("prune", "Unregister directories that no longer exist"),
    (
        "edit",
        "Open a config file of a directory with $VISUAL / $EDITOR",
    ),
    (
        "merge",
        "Call a registered namespace's commands without the namespace word",
    ),
    ("unmerge", "Require the namespace word again"),
];

/// A registered directory and what it contributes.
#[derive(Debug, Clone)]
pub(crate) struct GlobalEntry {
    pub(crate) dir: PathBuf,
    pub(crate) exists: bool,
    /// One per namespace (`None`: commands without namespace), in file order.
    pub(crate) scopes: Vec<ScopeFiles>,
}

#[derive(Debug, Clone)]
pub(crate) struct ScopeFiles {
    pub(crate) namespace: Option<String>,
    pub(crate) files: Vec<String>,
}

impl GlobalEntry {
    fn has_namespace(&self, namespace: &str) -> bool {
        self.scopes
            .iter()
            .any(|scope| scope.namespace.as_deref() == Some(namespace))
    }

    /// `api, tools` / `(root)` / `(missing)` / `(no config files)`.
    pub(crate) fn summary(&self) -> String {
        if !self.exists {
            return "(missing)".to_string();
        }
        if self.scopes.is_empty() {
            return "(no config files)".to_string();
        }
        let names: Vec<&str> = self
            .scopes
            .iter()
            .map(|scope| scope.namespace.as_deref().unwrap_or("(root)"))
            .collect();
        let files: Vec<&str> = self
            .scopes
            .iter()
            .flat_map(|scope| scope.files.iter().map(String::as_str))
            .collect();
        format!("{} · {}", names.join(", "), files.join(", "))
    }
}

pub(crate) fn entries() -> Vec<GlobalEntry> {
    load_installed_directories()
        .into_iter()
        .map(|dir| entry_for(&dir))
        .collect()
}

fn entry_for(dir: &Path) -> GlobalEntry {
    let exists = dir.is_dir();
    let mut scopes: Vec<ScopeFiles> = Vec::new();
    if exists {
        for file in global_directory_files(dir) {
            let namespace = file.scope.namespace().map(str::to_string);
            let name = relative_display(&file.config_path, dir);
            match scopes.iter_mut().find(|scope| scope.namespace == namespace) {
                Some(scope) => {
                    if !scope.files.contains(&name) {
                        scope.files.push(name);
                    }
                }
                None => scopes.push(ScopeFiles {
                    namespace,
                    files: vec![name],
                }),
            }
        }
    }
    GlobalEntry {
        dir: dir.to_path_buf(),
        exists,
        scopes,
    }
}

pub(crate) fn handle(args: &[String]) {
    let result = match args {
        [] => {
            print_help();
            Ok(())
        }
        [sub, rest @ ..] => match (sub.as_str(), rest) {
            ("add", []) => add("."),
            ("add", [path]) => add(path),
            ("remove", []) => remove("."),
            ("remove", [target]) => remove(target),
            ("list", []) => {
                list();
                Ok(())
            }
            ("prune", []) => prune(),
            ("edit", []) => edit(".", None),
            ("edit", [target]) => edit(target, None),
            ("edit", [target, file]) => edit(target, Some(file)),
            ("merge", [namespace]) => merge(namespace),
            ("unmerge", [namespace]) => unmerge(namespace),
            _ => Err(usage()),
        },
    };
    if let Err(message) = result {
        eprintln!("[fire] {message}");
        process::exit(1);
    }
}

/// `fire cli install`: kept as a hidden alias of `fire cli global add`.
pub(crate) fn deprecated_install() {
    eprintln!(
        "[fire] `fire cli install` is deprecated; use `fire cli global add` (same behavior)."
    );
    handle(&["add".to_string()]);
}

fn usage() -> String {
    let mut text = String::from("Unknown `fire cli global` command\nUsage:");
    text.push_str("\n  fire cli global add [path]");
    text.push_str("\n  fire cli global remove [namespace|path]");
    text.push_str("\n  fire cli global list");
    text.push_str("\n  fire cli global prune");
    text.push_str("\n  fire cli global edit [namespace|path] [file]");
    text.push_str("\n  fire cli global merge <namespace>");
    text.push_str("\n  fire cli global unmerge <namespace>");
    text
}

pub(crate) fn print_help() {
    println!("Fire CLI Global v{}", crate::FIRE_VERSION);
    println!("Commands of registered directories load from any directory.");
    println!("Commands:");
    println!("  add [path]                    {}", SUBCOMMANDS[0].1);
    println!("  remove [namespace|path]       {}", SUBCOMMANDS[1].1);
    println!("  list                          {}", SUBCOMMANDS[2].1);
    println!("  prune                         {}", SUBCOMMANDS[3].1);
    println!("  edit [namespace|path] [file]  {}", SUBCOMMANDS[4].1);
    println!("  merge <namespace>             {}", SUBCOMMANDS[5].1);
    println!("  unmerge <namespace>           {}", SUBCOMMANDS[6].1);
    println!();
    println!(
        "An argument with `/`, `.`, `..` or a leading `~` is a path; anything else is a namespace."
    );
}

fn add(path: &str) -> Result<(), String> {
    let directory = canonical_dir(&expand_path(path))?;
    match install_directory(&directory)
        .map_err(|err| format!("Failed to register directory: {err}"))?
    {
        InstallResult::Added => {
            println!(
                "Registered {}: its commands are available from any directory.",
                display_path(&directory)
            );
        }
        InstallResult::AlreadyInstalled => {
            println!("Already registered: {}", display_path(&directory));
        }
    }
    if directory_config_files(&directory).is_empty() {
        eprintln!(
            "[fire] warning: {} has no fire config files yet (create one with `fire cli init`).",
            display_path(&directory)
        );
    }
    Ok(())
}

fn merge(namespace: &str) -> Result<(), String> {
    if !entries().iter().any(|entry| entry.has_namespace(namespace)) {
        return Err(format!(
            "No registered directory has namespace `{namespace}` (see `fire cli global list`)."
        ));
    }
    if set_namespace_merged(namespace, true)? {
        println!(
            "Merged `{namespace}`: call its commands without `{namespace}` (`fire {namespace} <command>` still works)."
        );
        println!(
            "A first word another command, group or namespace already uses keeps pointing there; for those, write `fire {namespace} ...`."
        );
    } else {
        println!("Already merged: {namespace}");
    }
    Ok(())
}

fn unmerge(namespace: &str) -> Result<(), String> {
    if set_namespace_merged(namespace, false)? {
        println!("Unmerged `{namespace}`: call its commands as `fire {namespace} <command>`.");
        Ok(())
    } else {
        Err(format!(
            "`{namespace}` is not merged (merged: {}).",
            list_or_none(&load_merged_namespaces())
        ))
    }
}

fn remove(target: &str) -> Result<(), String> {
    let installed = load_installed_directories();
    let directory = resolve_registered(target, &installed)?;
    let remaining: Vec<PathBuf> = installed
        .into_iter()
        .filter(|path| path != &directory)
        .collect();
    save_installed_directories(&remaining)
        .map_err(|err| format!("Failed to unregister directory: {err}"))?;
    // An explicit removal is an answer too: don't offer to register it again.
    let _ = decline_directory(&directory);
    println!(
        "Unregistered {} (its files were not touched).",
        display_path(&directory)
    );
    Ok(())
}

fn list() {
    let entries = entries();
    if entries.is_empty() {
        println!("No registered directories. Register one with `fire cli global add`.");
        return;
    }

    let merged = load_merged_namespaces();
    // (rank, namespace column, directory column, files column)
    let mut rows: Vec<(u8, String, String, String)> = Vec::new();
    for entry in &entries {
        let dir = display_path(&entry.dir);
        if entry.scopes.is_empty() {
            let note = if entry.exists {
                "(no config files)"
            } else {
                "(missing)"
            };
            rows.push((2, "-".to_string(), dir, note.to_string()));
            continue;
        }
        for scope in &entry.scopes {
            let (rank, name) = match &scope.namespace {
                Some(name) if merged.contains(name) => (0, format!("{name} (merged)")),
                Some(name) => (0, name.clone()),
                None => (1, "(root)".to_string()),
            };
            rows.push((rank, name, dir.clone(), scope.files.join(", ")));
        }
    }
    rows.sort();

    let name_width = rows.iter().map(|row| row.1.len()).max().unwrap_or(0);
    let dir_width = rows.iter().map(|row| row.2.len()).max().unwrap_or(0);
    for (_, name, dir, files) in rows {
        println!("{name:<name_width$}  {dir:<dir_width$}  {files}");
    }
}

fn prune() -> Result<(), String> {
    let installed = load_installed_directories();
    let (keep, missing): (Vec<PathBuf>, Vec<PathBuf>) =
        installed.into_iter().partition(|path| path.is_dir());
    if missing.is_empty() {
        println!("Nothing to prune: every registered directory exists.");
        return Ok(());
    }
    save_installed_directories(&keep).map_err(|err| format!("Failed to prune: {err}"))?;
    for path in missing {
        println!("Unregistered missing directory: {}", display_path(&path));
    }
    Ok(())
}

fn edit(target: &str, file: Option<&str>) -> Result<(), String> {
    let directory = match parse_target(target) {
        Target::Path(path) => canonical_dir(&path)?,
        Target::Namespace(_) => resolve_registered(target, &load_installed_directories())?,
    };
    let files = directory_config_files(&directory);
    let names: Vec<String> = files
        .iter()
        .map(|path| relative_display(path, &directory))
        .collect();

    let chosen = match file {
        Some(file) => {
            let index = names.iter().position(|name| name == file).ok_or_else(|| {
                format!(
                    "`{file}` is not a fire config file in {}. Files: {}",
                    display_path(&directory),
                    list_or_none(&names)
                )
            })?;
            files[index].clone()
        }
        None => match files.len() {
            0 => {
                return Err(format!(
                    "No fire config files in {}. Create one with `fire cli init`.",
                    display_path(&directory)
                ))
            }
            1 => files[0].clone(),
            _ => files[pick_file(&directory, &names)?].clone(),
        },
    };

    open_in_editor(&chosen)
}

fn pick_file(directory: &Path, names: &[String]) -> Result<usize, String> {
    if !(io::stdin().is_terminal() && io::stderr().is_terminal()) {
        return Err(format!(
            "{} has several config files; name one: fire cli global edit {} <file>\nFiles: {}",
            display_path(directory),
            display_path(directory),
            names.join(", ")
        ));
    }
    for (index, name) in names.iter().enumerate() {
        eprintln!("  {}) {name}", index + 1);
    }
    eprint!("Open which file? [1-{}]: ", names.len());
    let _ = io::stderr().flush();
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .map_err(|err| format!("Cannot read answer: {err}"))?;
    match answer.trim().parse::<usize>() {
        Ok(choice) if (1..=names.len()).contains(&choice) => Ok(choice - 1),
        _ => Err("No file selected.".to_string()),
    }
}

fn open_in_editor(file: &Path) -> Result<(), String> {
    let editor = ["VISUAL", "EDITOR"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            format!(
                "Set $VISUAL or $EDITOR to open files (file: {}).",
                file.display()
            )
        })?;
    // Like git: the editor value may carry arguments (`code -w`), so the shell splits it.
    let status = Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$@\""))
        .arg(&editor)
        .arg(file)
        .status()
        .map_err(|err| format!("Cannot start editor `{editor}`: {err}"))?;
    if !status.success() {
        process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

/// Before running a command: offer once to register the local project directory. Only in an
/// interactive terminal, never in CI, sessions (and so MCP), pipes or completion.
pub(crate) fn offer_registration(config: &LoadedConfig) {
    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
    let in_ci = std::env::var_os("CI").is_some_and(|value| !value.is_empty());
    if !interactive || in_ci || std::env::var_os("FIRE_SESSION").is_some() {
        return;
    }
    if !config
        .files
        .iter()
        .any(|file| file.source == SourceKind::Local)
    {
        return;
    }
    let Ok(directory) = local_project_root().canonicalize() else {
        return;
    };
    if load_installed_directories().contains(&directory)
        || load_declined_directories().contains(&directory)
    {
        return;
    }

    eprint!(
        "[fire] Register this directory ({}) so its commands are available globally? [y/N] ",
        display_path(&directory)
    );
    let _ = io::stderr().flush();
    let mut answer = String::new();
    let _ = io::stdin().read_line(&mut answer);

    if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        match install_directory(&directory) {
            Ok(_) => eprintln!("[fire] Registered {}.", display_path(&directory)),
            Err(err) => eprintln!("[fire] Failed to register directory: {err}"),
        }
    } else {
        if let Err(err) = decline_directory(&directory) {
            eprintln!("[fire] Failed to remember the answer: {err}");
        }
        eprintln!("[fire] Not registered. Do it anytime with `fire cli global add`.");
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Target {
    Path(PathBuf),
    Namespace(String),
}

/// Documented rule: `/`, `.`, `..` or a leading `~` make it a path; anything else is a namespace.
fn parse_target(value: &str) -> Target {
    if value == "." || value == ".." || value.contains('/') || value.starts_with('~') {
        Target::Path(expand_path(value))
    } else {
        Target::Namespace(value.to_string())
    }
}

/// The registered directory `target` names, as stored in the registry.
fn resolve_registered(target: &str, installed: &[PathBuf]) -> Result<PathBuf, String> {
    match parse_target(target) {
        Target::Path(path) => {
            let path = path.canonicalize().unwrap_or(path);
            if installed.contains(&path) {
                Ok(path)
            } else {
                Err(format!(
                    "{} is not registered (see `fire cli global list`).",
                    display_path(&path)
                ))
            }
        }
        Target::Namespace(namespace) => {
            let matches: Vec<PathBuf> = installed
                .iter()
                .map(|dir| entry_for(dir))
                .filter(|entry| entry.has_namespace(&namespace))
                .map(|entry| entry.dir)
                .collect();
            match matches.as_slice() {
                [] => Err(format!(
                    "No registered directory has namespace `{namespace}` (see `fire cli global list`). For a directory named `{namespace}`, write `./{namespace}`."
                )),
                [one] => Ok(one.clone()),
                many => Err(format!(
                    "Namespace `{namespace}` is in several registered directories; pass the path instead: {}",
                    many.iter()
                        .map(|path| display_path(path))
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            }
        }
    }
}

fn canonical_dir(path: &Path) -> Result<PathBuf, String> {
    let directory = path
        .canonicalize()
        .map_err(|err| format!("Cannot resolve {}: {err}", path.display()))?;
    if !directory.is_dir() {
        return Err(format!("{} is not a directory", directory.display()));
    }
    Ok(directory)
}

fn expand_path(value: &str) -> PathBuf {
    let expanded = match (value.strip_prefix('~'), dirs::home_dir()) {
        (Some(rest), Some(home)) if rest.is_empty() || rest.starts_with('/') => {
            home.join(rest.trim_start_matches('/'))
        }
        _ => PathBuf::from(value),
    };
    if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(expanded)
    }
}

/// `~/code/api` for paths under the home directory.
pub(crate) fn display_path(path: &Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(rest) = path.strip_prefix(&home) {
            if rest.as_os_str().is_empty() {
                return "~".to_string();
            }
            return format!("~/{}", rest.display());
        }
    }
    path.display().to_string()
}

fn relative_display(path: &Path, base: &Path) -> String {
    match path.strip_prefix(base) {
        Ok(rest) => rest.display().to_string(),
        Err(_) => display_path(path),
    }
}

fn list_or_none(names: &[String]) -> String {
    if names.is_empty() {
        "(none)".to_string()
    } else {
        names.join(", ")
    }
}

/// Completion values for `fire cli global remove|edit <TAB>`: namespaces and directories.
pub(crate) fn target_suggestions(entries: &[GlobalEntry]) -> Vec<(String, String)> {
    let mut namespaces: Vec<(String, Vec<String>)> = Vec::new();
    for entry in entries {
        for scope in &entry.scopes {
            let Some(name) = &scope.namespace else {
                continue;
            };
            let detail = format!("{} · {}", display_path(&entry.dir), scope.files.join(", "));
            match namespaces.iter_mut().find(|(known, _)| known == name) {
                Some((_, details)) => details.push(detail),
                None => namespaces.push((name.clone(), vec![detail])),
            }
        }
    }
    namespaces.sort();

    let mut suggestions: Vec<(String, String)> = namespaces
        .into_iter()
        .map(|(name, details)| (name, details.join("; ")))
        .collect();
    for entry in entries {
        suggestions.push((entry.dir.display().to_string(), entry.summary()));
    }
    suggestions
}

/// Completion values for `fire cli global merge <TAB>`: registered namespaces not merged yet.
pub(crate) fn merge_suggestions(entries: &[GlobalEntry]) -> Vec<(String, String)> {
    let merged = load_merged_namespaces();
    target_suggestions(entries)
        .into_iter()
        .filter(|(value, _)| {
            matches!(parse_target(value), Target::Namespace(_)) && !merged.contains(value)
        })
        .collect()
}

/// Completion values for `fire cli global unmerge <TAB>`: merged namespaces.
pub(crate) fn unmerge_suggestions() -> Vec<(String, String)> {
    load_merged_namespaces()
        .into_iter()
        .map(|name| (name, "merged".to_string()))
        .collect()
}

/// Completion values for `fire cli global edit <target> <TAB>`: that directory's files.
pub(crate) fn file_suggestions(entries: &[GlobalEntry], target: &str) -> Vec<(String, String)> {
    let directory = match parse_target(target) {
        Target::Path(path) => path.canonicalize().unwrap_or(path),
        Target::Namespace(namespace) => {
            let matches: Vec<&GlobalEntry> = entries
                .iter()
                .filter(|entry| entry.has_namespace(&namespace))
                .collect();
            match matches.as_slice() {
                [one] => one.dir.clone(),
                _ => return Vec::new(),
            }
        }
    };
    let known = entries.iter().find(|entry| entry.dir == directory);
    directory_config_files(&directory)
        .iter()
        .map(|path| {
            let name = relative_display(path, &directory);
            let namespace = known
                .and_then(|entry| {
                    entry
                        .scopes
                        .iter()
                        .find(|scope| scope.files.contains(&name))
                })
                .map(|scope| {
                    scope
                        .namespace
                        .clone()
                        .unwrap_or_else(|| "(root)".to_string())
                });
            let description = match namespace {
                Some(namespace) => format!("{namespace} · {}", display_path(&directory)),
                None => display_path(&directory),
            };
            (name, description)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(dir: &str, scopes: &[(Option<&str>, &[&str])]) -> GlobalEntry {
        GlobalEntry {
            dir: PathBuf::from(dir),
            exists: true,
            scopes: scopes
                .iter()
                .map(|(namespace, files)| ScopeFiles {
                    namespace: namespace.map(str::to_string),
                    files: files.iter().map(|file| file.to_string()).collect(),
                })
                .collect(),
        }
    }

    #[test]
    fn targets_with_slash_dot_or_tilde_are_paths() {
        assert_eq!(parse_target("api"), Target::Namespace("api".to_string()));
        assert!(matches!(parse_target("."), Target::Path(_)));
        assert!(matches!(parse_target(".."), Target::Path(_)));
        assert!(matches!(parse_target("./api"), Target::Path(_)));
        assert!(matches!(parse_target("/abs/api"), Target::Path(_)));
        assert!(matches!(parse_target("~/code/api"), Target::Path(_)));
        assert!(matches!(parse_target("code/api"), Target::Path(_)));
    }

    #[test]
    fn tilde_expands_to_home() {
        let home = dirs::home_dir().expect("home");
        assert_eq!(expand_path("~/code"), home.join("code"));
        assert_eq!(expand_path("~"), home);
        assert_eq!(display_path(&home.join("code/api")), "~/code/api");
    }

    #[test]
    fn target_suggestions_list_namespaces_then_directories() {
        let entries = vec![
            entry("/w/tools", &[(Some("tools"), &["fire.yaml"])]),
            entry(
                "/w/api",
                &[
                    (Some("api"), &["fire.yaml", "db.fire.yaml"]),
                    (None, &["x.fire.yaml"]),
                ],
            ),
        ];
        let suggestions = target_suggestions(&entries);
        let values: Vec<&str> = suggestions
            .iter()
            .map(|(value, _)| value.as_str())
            .collect();
        assert_eq!(values, vec!["api", "tools", "/w/tools", "/w/api"]);
        assert_eq!(suggestions[0].1, "/w/api · fire.yaml, db.fire.yaml");
        assert_eq!(
            suggestions[3].1,
            "api, (root) · fire.yaml, db.fire.yaml, x.fire.yaml"
        );
    }

    #[test]
    fn missing_directory_summary() {
        let mut missing = entry("/gone", &[]);
        missing.exists = false;
        assert_eq!(missing.summary(), "(missing)");
        assert_eq!(entry("/empty", &[]).summary(), "(no config files)");
    }
}
