//! Static validation for `fire cli check`.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
    process,
};

use jsonschema::error::ValidationErrorKind;
use serde::Serialize;
use serde_json::Value;

use crate::{
    config::{
        self, CommandAction, CommandEntry, CommandSpec, EnvFileSetting, FileConfig, FileScope,
        ImplicitNamespaces, RunnerSetting,
    },
    dependencies,
    registry::load_installed_directories,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
enum Severity {
    Error,
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct Diagnostic {
    file: PathBuf,
    line: usize,
    severity: Severity,
    message: String,
    hint: String,
}

#[derive(Default)]
struct Args {
    global: bool,
    json: bool,
    strict: bool,
}

struct Document {
    path: PathBuf,
    text: String,
    value: Option<Value>,
}

pub(crate) fn handle(values: &[String]) {
    let args = parse_args(values).unwrap_or_else(|message| {
        eprintln!("[fire] {message}");
        eprintln!("Usage: fire cli check [--global] [--format json] [--strict]");
        process::exit(1);
    });
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let (mut diagnostics, sources) = validate_documents(&cwd, args.global);
    let config = config::load_config_for_check(&cwd, args.global);
    validate_effective_config(&config, &sources, &mut diagnostics);

    if args.strict {
        for diagnostic in &mut diagnostics {
            if diagnostic.severity == Severity::Warning {
                diagnostic.severity = Severity::Error;
            }
        }
    }
    diagnostics.sort_by(|left, right| {
        (&left.file, left.line, left.severity, &left.message).cmp(&(
            &right.file,
            right.line,
            right.severity,
            &right.message,
        ))
    });
    diagnostics.dedup();

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&diagnostics).expect("serialize check diagnostics")
        );
    } else {
        for diagnostic in &diagnostics {
            println!(
                "{}:{}: {}: {}",
                diagnostic.file.display(),
                diagnostic.line,
                match diagnostic.severity {
                    Severity::Error => "error",
                    Severity::Warning => "warning",
                },
                diagnostic.message
            );
            println!("  Hint: {}", diagnostic.hint);
        }
    }

    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error)
    {
        process::exit(1);
    }
}

fn parse_args(values: &[String]) -> Result<Args, String> {
    let mut args = Args::default();
    let mut index = 0;
    while index < values.len() {
        match values[index].as_str() {
            "--global" if !args.global => args.global = true,
            "--strict" if !args.strict => args.strict = true,
            "--format" => {
                index += 1;
                match values.get(index).map(String::as_str) {
                    Some("json") if !args.json => args.json = true,
                    Some(value) => {
                        return Err(format!("Unknown check format `{value}`; use `json`."))
                    }
                    None => return Err("--format requires `json`".to_string()),
                }
            }
            option => return Err(format!("Unknown check option `{option}`")),
        }
        index += 1;
    }
    Ok(args)
}

fn validate_documents(
    cwd: &Path,
    include_global: bool,
) -> (Vec<Diagnostic>, BTreeMap<PathBuf, String>) {
    let local = config::local_project_root_for_check(cwd);
    let mut directories = vec![local.clone()];
    if include_global {
        directories.extend(
            load_installed_directories()
                .into_iter()
                .filter(|directory| directory != &local),
        );
    }

    let mut root_paths = BTreeSet::new();
    for directory in &directories {
        root_paths.extend(config::directory_config_files(directory));
    }

    let mut diagnostics = Vec::new();
    let mut documents = BTreeMap::new();
    for path in root_paths {
        let document = read_document(path, &mut diagnostics);
        documents.insert(document.path.clone(), document);
    }

    let roots: Vec<PathBuf> = documents.keys().cloned().collect();
    let mut included_paths = BTreeSet::new();
    for path in roots {
        let Some(document) = documents.get(&path) else {
            continue;
        };
        let Some(value) = &document.value else {
            continue;
        };
        let base = directories
            .iter()
            .find(|directory| path.parent() == Some(directory.as_path()))
            .map(PathBuf::as_path)
            .unwrap_or_else(|| path.parent().unwrap_or(Path::new(".")));
        check_includes(document, value, base, &mut diagnostics, &mut included_paths);
    }
    for path in included_paths {
        documents
            .entry(path.clone())
            .or_insert_with(|| read_document(path, &mut diagnostics));
    }

    let schema: Value =
        serde_json::from_str(include_str!("../schemas/fire.schema.json")).expect("valid schema");
    let validator = jsonschema::validator_for(&schema).expect("compile fire schema");
    for document in documents.values() {
        if let Some(value) = &document.value {
            validate_schema(document, value, &validator, &mut diagnostics);
            check_explicit_env_files(document, value, &mut diagnostics);
        }
    }

    let sources = documents
        .into_iter()
        .map(|(path, document)| (path, document.text))
        .collect();
    (diagnostics, sources)
}

fn read_document(path: PathBuf, diagnostics: &mut Vec<Diagnostic>) -> Document {
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => {
            diagnostics.push(Diagnostic {
                file: path.clone(),
                line: 1,
                severity: Severity::Error,
                message: format!("Could not read fire file: {error}"),
                hint: "Make the file readable or remove it from the configuration directory."
                    .to_string(),
            });
            return Document {
                path,
                text: String::new(),
                value: None,
            };
        }
    };

    let mut yaml: yaml_serde::Value = match yaml_serde::from_str(&text) {
        Ok(value) => value,
        Err(error) => {
            diagnostics.push(Diagnostic {
                file: path.clone(),
                line: error.location().map_or(1, |location| location.line()),
                severity: Severity::Error,
                message: format!("Invalid YAML: {error}"),
                hint: "Fix the YAML syntax at this location.".to_string(),
            });
            return Document {
                path,
                text,
                value: None,
            };
        }
    };
    if let Err(error) = yaml.apply_merge() {
        diagnostics.push(Diagnostic {
            file: path.clone(),
            line: 1,
            severity: Severity::Error,
            message: format!("YAML merge failed: {error}"),
            hint: "Fix the YAML anchor or merge key.".to_string(),
        });
        return Document {
            path,
            text,
            value: None,
        };
    }
    match yaml_serde::from_value::<Value>(yaml) {
        Ok(value) => Document {
            path,
            text,
            value: Some(value),
        },
        Err(error) => {
            diagnostics.push(Diagnostic {
                file: path.clone(),
                line: error.location().map_or(1, |location| location.line()),
                severity: Severity::Error,
                message: format!("Invalid YAML value: {error}"),
                hint: "Use string keys and values supported by the Fire schema.".to_string(),
            });
            Document {
                path,
                text,
                value: None,
            }
        }
    }
}

fn check_includes(
    document: &Document,
    value: &Value,
    base: &Path,
    diagnostics: &mut Vec<Diagnostic>,
    included_paths: &mut BTreeSet<PathBuf>,
) {
    let Some(includes) = value.get("include").and_then(Value::as_array) else {
        return;
    };
    for include in includes.iter().filter_map(Value::as_str) {
        let Some(relative) = normalize_include(include) else {
            diagnostics.push(Diagnostic {
                file: document.path.clone(),
                line: line_for_key(&document.text, "include"),
                severity: Severity::Error,
                message: format!("Invalid include path `{include}`."),
                hint: "Use a non-empty relative directory without `..`.".to_string(),
            });
            continue;
        };
        let directory = base.join(relative);
        if !directory.is_dir() {
            diagnostics.push(Diagnostic {
                file: document.path.clone(),
                line: line_for_key(&document.text, "include"),
                severity: Severity::Error,
                message: format!("Include directory `{include}` does not exist."),
                hint: "Create the directory or remove this include entry.".to_string(),
            });
            continue;
        }
        included_paths.extend(config::directory_config_files(&directory));
    }
}

fn normalize_include(value: &str) -> Option<PathBuf> {
    let path = Path::new(value);
    if path.is_absolute() {
        return None;
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (!normalized.as_os_str().is_empty()).then_some(normalized)
}

fn validate_schema(
    document: &Document,
    value: &Value,
    validator: &jsonschema::Validator,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for error in validator.iter_errors(value) {
        let mut unexpected = Vec::new();
        collect_unexpected(&error, &mut unexpected);
        if unexpected.is_empty() {
            let pointer = error.instance_path().as_str();
            let key = pointer.rsplit('/').next().unwrap_or_default();
            diagnostics.push(Diagnostic {
                file: document.path.clone(),
                line: line_for_key(&document.text, key),
                severity: Severity::Error,
                message: format!("Schema validation failed: {error}"),
                hint: "Use the documented type and keys from schemas/fire.schema.json.".to_string(),
            });
        } else {
            for key in unexpected {
                diagnostics.push(Diagnostic {
                    file: document.path.clone(),
                    line: line_for_key(&document.text, &key),
                    severity: Severity::Error,
                    message: format!("Unknown configuration key `{key}`."),
                    hint: "Remove the key or replace it with a documented Fire key.".to_string(),
                });
            }
        }
    }
}

fn collect_unexpected(error: &jsonschema::ValidationError<'_>, output: &mut Vec<String>) {
    match error.kind() {
        ValidationErrorKind::AdditionalProperties { unexpected }
        | ValidationErrorKind::UnevaluatedProperties { unexpected } => {
            output.extend(unexpected.iter().cloned());
        }
        ValidationErrorKind::AnyOf { context }
        | ValidationErrorKind::OneOfNotValid { context }
        | ValidationErrorKind::OneOfMultipleValid { context } => {
            for branch in context {
                for nested in branch {
                    collect_unexpected(nested, output);
                }
            }
        }
        _ => {}
    }
}

fn check_explicit_env_files(document: &Document, value: &Value, diagnostics: &mut Vec<Diagnostic>) {
    fn visit(document: &Document, value: &Value, diagnostics: &mut Vec<Diagnostic>) {
        match value {
            Value::Object(object) => {
                if let Some(setting) = object.get("env_file") {
                    let paths: Vec<&str> = match setting {
                        Value::String(path) => vec![path],
                        Value::Array(paths) => paths.iter().filter_map(Value::as_str).collect(),
                        _ => Vec::new(),
                    };
                    for path in paths {
                        let resolved = document.path.parent().unwrap_or(Path::new(".")).join(path);
                        if !resolved.is_file() {
                            diagnostics.push(Diagnostic {
                                file: document.path.clone(),
                                line: line_for_key(&document.text, "env_file"),
                                severity: Severity::Warning,
                                message: format!("Explicit env_file `{path}` does not exist."),
                                hint: "Create the file, correct the literal path, or use null to disable env loading."
                                    .to_string(),
                            });
                        }
                    }
                }
                for child in object.values() {
                    visit(document, child, diagnostics);
                }
            }
            Value::Array(items) => {
                for item in items {
                    visit(document, item, diagnostics);
                }
            }
            _ => {}
        }
    }
    visit(document, value, diagnostics);
}

fn validate_effective_config(
    config: &config::LoadedConfig,
    sources: &BTreeMap<PathBuf, String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    validate_collisions(config, sources, diagnostics);
    let mut dependency_errors = BTreeSet::new();
    for file in &config.files {
        walk_commands(&file.commands, &mut Vec::new(), &mut |path, entry| {
            let source = sources
                .get(&file.config_path)
                .map(String::as_str)
                .unwrap_or("");
            validate_command(file, path, entry, source, diagnostics);
            if !entry.needs().is_empty() {
                if let Err(message) = dependencies::build_plan(config, entry) {
                    if dependency_errors.insert((file.config_path.clone(), message.clone())) {
                        diagnostics.push(Diagnostic {
                            file: file.config_path.clone(),
                            line: line_for_key(source, "needs"),
                            severity: Severity::Error,
                            message,
                            hint: "Use existing runnable command paths in the same file scope and remove dependency cycles."
                                .to_string(),
                        });
                    }
                }
            }
        });
    }
}

fn validate_command(
    file: &FileConfig,
    path: &[String],
    entry: &CommandEntry,
    source: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let label = full_command_path(file, path).join(" ");
    let Some(spec) = entry.spec() else {
        return;
    };
    if !entry.is_runnable() && spec.commands.is_empty() {
        diagnostics.push(Diagnostic {
            file: file.config_path.clone(),
            line: line_for_key(
                source,
                path.last().map(String::as_str).unwrap_or("commands"),
            ),
            severity: Severity::Error,
            message: format!("Command `{label}` has neither `exec`, `eval`, nor subcommands."),
            hint: "Add an executable action or at least one nested command.".to_string(),
        });
    }

    if !spec.placeholder.trim().is_empty() && !spec.placeholder.contains("{n}") {
        diagnostics.push(Diagnostic {
            file: file.config_path.clone(),
            line: line_for_key(source, "placeholder"),
            severity: Severity::Warning,
            message: format!(
                "Command `{label}` has placeholder pattern `{}` that cannot produce positional placeholders.",
                spec.placeholder
            ),
            hint: "Include the literal `{n}` convention in the pattern, for example `@{n}` or `{{{n}}}`."
                .to_string(),
        });
    }

    let strings = command_strings(spec);
    for key in spec.compute.keys() {
        if !strings.iter().any(|value| value.contains(key)) {
            diagnostics.push(Diagnostic {
                file: file.config_path.clone(),
                line: line_for_key(source, "compute"),
                severity: Severity::Warning,
                message: format!("Command `{label}` compute key `{key}` is never used."),
                hint: "Use the exact literal key in the command or remove the compute entry."
                    .to_string(),
            });
        }
    }

    if let Some(eval) = &spec.eval {
        for expression in action_values(eval) {
            match expression.split_once(':') {
                Some((prefix, _)) if !file.runtimes.contains_key(prefix.trim()) => {
                    let runtime = prefix.trim();
                    diagnostics.push(Diagnostic {
                        file: file.config_path.clone(),
                        line: line_for_key(source, "eval"),
                        severity: Severity::Error,
                        message: format!("Runtime `{runtime}` is not configured for command `{label}`."),
                        hint: format!("Add a `runtimes.{runtime}` definition or use a configured runtime prefix."),
                    });
                }
                None => diagnostics.push(Diagnostic {
                    file: file.config_path.clone(),
                    line: line_for_key(source, "eval"),
                    severity: Severity::Error,
                    message: format!("Command `{label}` eval has no runtime prefix."),
                    hint: "Use the documented `<runtime>:<code>` convention.".to_string(),
                }),
                _ => {}
            }
        }
    }
    for expression in spec.compute.values() {
        let trimmed = expression.trim();
        let Some((prefix, _)) = trimmed.split_once(':') else {
            continue;
        };
        let prefix = prefix.trim();
        if !prefix.is_empty()
            && !prefix.chars().any(char::is_whitespace)
            && matches!(prefix, "py" | "python" | "ts" | "js" | "node" | "deno")
            && !file.runtimes.contains_key(prefix)
        {
            diagnostics.push(Diagnostic {
                file: file.config_path.clone(),
                line: line_for_key(source, "compute"),
                severity: Severity::Error,
                message: format!("Runtime `{prefix}` is not configured for command `{label}`."),
                hint: format!(
                    "Add a `runtimes.{prefix}` definition or use a shell compute expression."
                ),
            });
        }
    }

    if let EnvFileSetting::Paths(paths) = &spec.env_file {
        // Raw document validation reports these once at their actual declaration. This branch
        // intentionally has no diagnostic; it documents that inherited settings are accounted
        // for by the effective command model without duplicating them per command.
        let _ = paths;
    }
}

fn validate_collisions(
    config: &config::LoadedConfig,
    sources: &BTreeMap<PathBuf, String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    validate_scope_collisions(config, sources, diagnostics);
    let mut terminal_paths: BTreeMap<Vec<String>, Vec<PathBuf>> = BTreeMap::new();
    let implicit = ImplicitNamespaces::of(config);
    for file in &config.files {
        walk_commands(&file.commands, &mut Vec::new(), &mut |path, entry| {
            if entry.is_runnable() {
                terminal_paths
                    .entry(callable_command_path(file, path, &implicit))
                    .or_default()
                    .push(file.config_path.clone());
            }
        });
    }
    for (path, mut files) in terminal_paths {
        files.sort();
        files.dedup();
        if files.len() < 2 {
            continue;
        }
        let locations = files
            .iter()
            .map(|file| file.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        for file in files {
            let source = sources.get(&file).map(String::as_str).unwrap_or("");
            diagnostics.push(Diagnostic {
                file: file.clone(),
                line: line_for_key(
                    source,
                    path.last().map(String::as_str).unwrap_or("commands"),
                ),
                severity: Severity::Error,
                message: format!(
                    "Command collision for `{}` across: {locations}.",
                    path.join(" ")
                ),
                hint:
                    "Rename a command, group, or namespace so each terminal command path is unique."
                        .to_string(),
            });
        }
    }
}

fn validate_scope_collisions(
    config: &config::LoadedConfig,
    sources: &BTreeMap<PathBuf, String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut names: BTreeMap<Vec<String>, Vec<(&str, &PathBuf, String)>> = BTreeMap::new();
    let implicit = ImplicitNamespaces::of(config);
    for file in &config.files {
        match &file.scope {
            FileScope::Namespace { namespace, .. } => {
                names.entry(vec![namespace.clone()]).or_default().push((
                    "namespace",
                    &file.config_path,
                    "namespace".to_string(),
                ));
            }
            FileScope::Group { group, .. } => {
                names.entry(vec![group.clone()]).or_default().push((
                    "group",
                    &file.config_path,
                    "group".to_string(),
                ));
            }
            FileScope::NamespaceGroup {
                namespace, group, ..
            } => {
                names.entry(vec![namespace.clone()]).or_default().push((
                    "namespace",
                    &file.config_path,
                    "namespace".to_string(),
                ));
                names
                    .entry(vec![namespace.clone(), group.clone()])
                    .or_default()
                    .push(("group", &file.config_path, "group".to_string()));
                if implicit.allows(namespace, group) {
                    names.entry(vec![group.clone()]).or_default().push((
                        "group",
                        &file.config_path,
                        "group".to_string(),
                    ));
                }
            }
            FileScope::Root => {}
        }
        for name in file.commands.keys() {
            let mut path = match &file.scope {
                FileScope::Root => Vec::new(),
                FileScope::Namespace { namespace, .. } => vec![namespace.clone()],
                FileScope::Group { group, .. } => vec![group.clone()],
                FileScope::NamespaceGroup {
                    namespace, group, ..
                } => vec![namespace.clone(), group.clone()],
            };
            path.push(name.clone());
            names.entry(path.clone()).or_default().push((
                "command",
                &file.config_path,
                name.clone(),
            ));
            let callable = callable_command_path(file, std::slice::from_ref(name), &implicit);
            if callable != path {
                names.entry(callable).or_default().push((
                    "command",
                    &file.config_path,
                    name.clone(),
                ));
            }
        }
    }

    for (path, entries) in names {
        let unique_files: BTreeSet<&PathBuf> = entries.iter().map(|(_, file, _)| *file).collect();
        let kinds: BTreeSet<&str> = entries.iter().map(|(kind, _, _)| *kind).collect();
        let duplicate_group = entries
            .iter()
            .filter(|(kind, _, _)| *kind == "group")
            .count()
            > 1;
        if unique_files.len() < 2 || (!duplicate_group && kinds.len() < 2) {
            continue;
        }
        let kinds = kinds.into_iter().collect::<Vec<_>>().join("/");
        let locations = unique_files
            .iter()
            .map(|file| file.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        for (_, file, source_key) in entries {
            let source = sources.get(file).map(String::as_str).unwrap_or("");
            diagnostics.push(Diagnostic {
                file: file.clone(),
                line: line_for_key(source, &source_key),
                severity: Severity::Error,
                message: format!(
                    "Name collision for {} `{}` across: {locations}.",
                    kinds,
                    path.join(" ")
                ),
                hint: "Give each command, group, and namespace path a distinct name.".to_string(),
            });
        }
    }
}

fn callable_command_path(
    file: &FileConfig,
    path: &[String],
    implicit: &ImplicitNamespaces,
) -> Vec<String> {
    match &file.scope {
        FileScope::Namespace { namespace, .. }
            if path
                .first()
                .is_some_and(|first| implicit.allows(namespace, first)) =>
        {
            path.to_vec()
        }
        FileScope::NamespaceGroup {
            namespace, group, ..
        } if implicit.allows(namespace, group) => {
            let mut full = vec![group.clone()];
            full.extend_from_slice(path);
            full
        }
        _ => full_command_path(file, path),
    }
}

fn full_command_path(file: &FileConfig, path: &[String]) -> Vec<String> {
    let mut full = match &file.scope {
        FileScope::Root => Vec::new(),
        FileScope::Namespace { namespace, .. } => vec![namespace.clone()],
        FileScope::Group { group, .. } => vec![group.clone()],
        FileScope::NamespaceGroup {
            namespace, group, ..
        } => vec![namespace.clone(), group.clone()],
    };
    full.extend_from_slice(path);
    full
}

fn walk_commands(
    commands: &BTreeMap<String, CommandEntry>,
    path: &mut Vec<String>,
    visit: &mut impl FnMut(&[String], &CommandEntry),
) {
    for (name, entry) in commands {
        path.push(name.clone());
        visit(path, entry);
        if let Some(children) = entry.subcommands() {
            walk_commands(children, path, visit);
        }
        path.pop();
    }
}

fn command_strings(spec: &CommandSpec) -> Vec<&str> {
    let mut strings = Vec::new();
    for action in [&spec.exec, &spec.run, &spec.eval].into_iter().flatten() {
        strings.extend(action_values(action));
    }
    strings.extend([
        spec.before.as_str(),
        spec.check.as_str(),
        spec.shell.as_str(),
        spec.placeholder.as_str(),
    ]);
    strings.extend(spec.compute.values().map(String::as_str));
    strings.extend(spec.macros.keys().map(String::as_str));
    strings.extend(spec.macros.values().map(String::as_str));
    runner_strings(&spec.runner, &mut strings);
    runner_strings(&spec.fallback_runner, &mut strings);
    strings
}

fn runner_strings<'a>(runner: &'a RunnerSetting, output: &mut Vec<&'a str>) {
    match runner {
        RunnerSetting::Text(value) => output.push(value),
        RunnerSetting::Container(container) => {
            output.push(&container.image);
            if let Some(value) = &container.command {
                output.push(value);
            }
            if let Some(value) = &container.working_dir {
                output.push(value);
            }
            if let Some(value) = &container.user {
                output.push(value);
            }
            if let Some(value) = &container.network {
                output.push(value);
            }
            if let Some(value) = &container.network_mode {
                output.push(value);
            }
            output.extend(container.volumes.iter().flatten().map(String::as_str));
            output.extend(container.environment.values().map(String::as_str));
            output.extend(container.args.iter().map(String::as_str));
        }
    }
}

fn action_values(action: &CommandAction) -> Vec<&str> {
    match action {
        CommandAction::Single(value) => vec![value],
        CommandAction::Multiple(values) => values.iter().map(String::as_str).collect(),
    }
}

fn line_for_key(text: &str, key: &str) -> usize {
    if key.is_empty() {
        return 1;
    }
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim_start().trim_start_matches("- ");
        let Some((candidate, _)) = trimmed.split_once(':') else {
            continue;
        };
        let candidate = candidate.trim().trim_matches(['\'', '"']);
        if candidate == key {
            return index + 1;
        }
    }
    1
}
