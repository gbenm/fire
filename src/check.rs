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
    config::{self, CommandEntry, FileConfig, FileScope, ImplicitNamespaces, RunnerSetting},
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
        let mut specific = Vec::new();
        collect_specific_errors(&error, &mut specific);
        let deepest = specific
            .iter()
            .map(|error| error.instance_path().as_str().matches('/').count())
            .max()
            .unwrap_or(0);
        for error in specific
            .into_iter()
            .filter(|error| error.instance_path().as_str().matches('/').count() == deepest)
        {
            let mut unexpected = Vec::new();
            collect_unexpected(error, &mut unexpected);
            if unexpected.is_empty() {
                let pointer = error.instance_path().as_str();
                diagnostics.push(Diagnostic {
                    file: document.path.clone(),
                    line: line_for_json_pointer(&document.text, pointer, None),
                    severity: Severity::Error,
                    message: format!("Schema validation failed: {error}"),
                    hint: "Use the documented type and keys from schemas/fire.schema.json."
                        .to_string(),
                });
            } else {
                for (pointer, key) in unexpected {
                    diagnostics.push(Diagnostic {
                        file: document.path.clone(),
                        line: line_for_json_pointer(&document.text, &pointer, Some(&key)),
                        severity: Severity::Error,
                        message: format!("Unknown configuration key `{key}`."),
                        hint: "Remove the key or replace it with a documented Fire key."
                            .to_string(),
                    });
                }
            }
        }
    }
}

fn collect_specific_errors<'error, 'instance>(
    error: &'error jsonschema::ValidationError<'instance>,
    output: &mut Vec<&'error jsonschema::ValidationError<'instance>>,
) {
    match error.kind() {
        ValidationErrorKind::AnyOf { context }
        | ValidationErrorKind::OneOfNotValid { context }
        | ValidationErrorKind::OneOfMultipleValid { context } => {
            for branch in context {
                for nested in branch {
                    collect_specific_errors(nested, output);
                }
            }
        }
        _ => output.push(error),
    }
}

fn collect_unexpected(error: &jsonschema::ValidationError<'_>, output: &mut Vec<(String, String)>) {
    match error.kind() {
        ValidationErrorKind::AdditionalProperties { unexpected }
        | ValidationErrorKind::UnevaluatedProperties { unexpected } => {
            let pointer = error.instance_path().as_str().to_string();
            output.extend(unexpected.iter().cloned().map(|key| (pointer.clone(), key)));
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
    fn visit(
        document: &Document,
        value: &Value,
        path: &mut Vec<String>,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        match value {
            Value::Object(object) => {
                if let Some(setting) = object.get("env_file") {
                    let paths: Vec<&str> = match setting {
                        Value::String(path) => vec![path],
                        Value::Array(paths) => paths.iter().filter_map(Value::as_str).collect(),
                        _ => Vec::new(),
                    };
                    for env_path in paths {
                        let resolved = document
                            .path
                            .parent()
                            .unwrap_or(Path::new("."))
                            .join(env_path);
                        if !resolved.is_file() {
                            diagnostics.push(Diagnostic {
                                file: document.path.clone(),
                                line: {
                                    path.push("env_file".to_string());
                                    let line = line_for_yaml_path(&document.text, path)
                                        .unwrap_or_else(|| line_for_key(&document.text, "env_file"));
                                    path.pop();
                                    line
                                },
                                severity: Severity::Warning,
                                message: format!(
                                    "Explicit env_file `{env_path}` does not exist."
                                ),
                                hint: "Create the file, correct the literal path, or use null to disable env loading."
                                    .to_string(),
                            });
                        }
                    }
                }
                for (key, child) in object {
                    path.push(key.clone());
                    visit(document, child, path, diagnostics);
                    path.pop();
                }
            }
            Value::Array(items) => {
                for (index, item) in items.iter().enumerate() {
                    path.push(index.to_string());
                    visit(document, item, path, diagnostics);
                    path.pop();
                }
            }
            _ => {}
        }
    }
    visit(document, value, &mut Vec::new(), diagnostics);
}

fn validate_effective_config(
    config: &config::LoadedConfig,
    sources: &BTreeMap<PathBuf, String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    validate_collisions(config, sources, diagnostics);
    let mut dependency_errors = BTreeSet::new();
    for file in &config.files {
        walk_command_chains(
            &file.commands,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut |path, chain| {
                let entry = chain.last().expect("command chain");
                let source = sources
                    .get(&file.config_path)
                    .map(String::as_str)
                    .unwrap_or("");
                validate_command(file, path, chain, source, diagnostics);
                if !entry.needs().is_empty() {
                    if let Err(message) = dependencies::build_plan(config, entry, false) {
                        if dependency_errors.insert((file.config_path.clone(), message.clone())) {
                            diagnostics.push(Diagnostic {
                                file: file.config_path.clone(),
                                line: line_for_command_field(source, path, "needs"),
                                severity: Severity::Error,
                                message,
                                hint: "Use existing runnable command paths in the same file scope and remove dependency cycles."
                                    .to_string(),
                            });
                        }
                    }
                }
            },
        );
    }
}

#[derive(Default)]
struct EffectiveValidation {
    placeholder: Option<(String, Vec<String>)>,
    compute: BTreeMap<String, (String, Vec<String>)>,
    macros: BTreeMap<String, String>,
    before: Option<String>,
    check: Option<String>,
    runner: Option<RunnerSetting>,
    fallback_runner: Option<RunnerSetting>,
}

fn validate_command(
    file: &FileConfig,
    path: &[String],
    chain: &[&CommandEntry],
    source: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let label = full_command_path(file, path).join(" ");
    let entry = chain.last().expect("command chain");
    let Some(spec) = entry.spec() else {
        validate_effective_command(file, path, chain, source, &label, diagnostics);
        return;
    };
    if !entry.is_runnable() && spec.commands.is_empty() {
        diagnostics.push(Diagnostic {
            file: file.config_path.clone(),
            line: line_for_command(source, path),
            severity: Severity::Error,
            message: format!("Command `{label}` has neither `exec`, `eval`, nor subcommands."),
            hint: "Add an executable action or at least one nested command.".to_string(),
        });
    }

    validate_effective_command(file, path, chain, source, &label, diagnostics);
}

fn validate_effective_command(
    file: &FileConfig,
    path: &[String],
    chain: &[&CommandEntry],
    source: &str,
    label: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !chain.last().is_some_and(|entry| entry.is_runnable()) {
        return;
    }

    let mut effective = EffectiveValidation::default();
    for (index, entry) in chain.iter().enumerate() {
        let Some(spec) = entry.spec() else {
            continue;
        };
        let declaration_path = path[..=index].to_vec();
        if !spec.placeholder.trim().is_empty() {
            effective.placeholder = Some((spec.placeholder.clone(), declaration_path.clone()));
        }
        if !spec.before.trim().is_empty() {
            effective.before = Some(spec.before.clone());
        }
        if !spec.check.trim().is_empty() {
            effective.check = Some(spec.check.clone());
        }
        if !spec.runner.is_empty() {
            effective.runner = Some(spec.runner.clone());
        }
        if !spec.fallback_runner.is_empty() {
            effective.fallback_runner = Some(spec.fallback_runner.clone());
        }
        for (key, value) in &spec.compute {
            effective
                .compute
                .insert(key.clone(), (value.clone(), declaration_path.clone()));
        }
        for (key, value) in &spec.macros {
            effective.macros.insert(key.clone(), value.clone());
        }
    }

    if let Some((placeholder, declaration_path)) = &effective.placeholder {
        if !crate::execute::placeholder_pattern_is_valid(placeholder) {
            diagnostics.push(Diagnostic {
                file: file.config_path.clone(),
                line: line_for_command_field(source, declaration_path, "placeholder"),
                severity: Severity::Warning,
                message: format!(
                    "Command `{label}` has placeholder pattern `{}` that cannot produce positional placeholders.",
                    placeholder
                ),
                hint: "Include `n` or `{n}` in a surrounding pattern, for example `@n`, `@{n}` or `{{{n}}}`."
                    .to_string(),
            });
        }
    }

    let mut strings: Vec<(Option<&str>, String)> = Vec::new();
    if let Some(entry) = chain.last() {
        strings.extend(
            entry
                .execution_commands()
                .into_iter()
                .flatten()
                .map(|value| (None, value)),
        );
        strings.extend(
            entry
                .evaluation_expressions()
                .into_iter()
                .flatten()
                .map(|value| (None, value)),
        );
    }
    strings.extend(effective.before.iter().cloned().map(|value| (None, value)));
    strings.extend(effective.check.iter().cloned().map(|value| (None, value)));
    if let Some(runner) = &effective.runner {
        runner_strings(runner, &mut strings);
    }
    if let Some(runner) = &effective.fallback_runner {
        runner_strings(runner, &mut strings);
    }
    for (key, (expression, _)) in &effective.compute {
        strings.push((Some(key), expression.clone()));
    }
    for (_, value) in &mut strings {
        *value = crate::execute::apply_macros(value, &effective.macros);
    }

    for (key, (_, declaration_path)) in &effective.compute {
        if !strings
            .iter()
            .any(|(origin, value)| *origin != Some(key.as_str()) && value.contains(key))
        {
            diagnostics.push(Diagnostic {
                file: file.config_path.clone(),
                line: line_for_command_field(source, declaration_path, "compute"),
                severity: Severity::Warning,
                message: format!("Command `{label}` compute key `{key}` is never used."),
                hint: "Use the exact literal key in the command or remove the compute entry."
                    .to_string(),
            });
        }
    }

    if let Some(entry) = chain.last() {
        for expression in entry.evaluation_expressions().into_iter().flatten() {
            match expression.split_once(':') {
                Some((prefix, _)) if !file.runtimes.contains_key(prefix.trim()) => {
                    let runtime = prefix.trim();
                    diagnostics.push(Diagnostic {
                        file: file.config_path.clone(),
                        line: line_for_command_field(source, path, "eval"),
                        severity: Severity::Error,
                        message: format!("Runtime `{runtime}` is not configured for command `{label}`."),
                        hint: format!("Add a `runtimes.{runtime}` definition or use a configured runtime prefix."),
                    });
                }
                None => diagnostics.push(Diagnostic {
                    file: file.config_path.clone(),
                    line: line_for_command_field(source, path, "eval"),
                    severity: Severity::Error,
                    message: format!("Command `{label}` eval has no runtime prefix."),
                    hint: "Use the documented `<runtime>:<code>` convention.".to_string(),
                }),
                _ => {}
            }
        }
    }
    for (expression, declaration_path) in effective.compute.values() {
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
                line: line_for_command_field(source, declaration_path, "compute"),
                severity: Severity::Error,
                message: format!("Runtime `{prefix}` is not configured for command `{label}`."),
                hint: format!(
                    "Add a `runtimes.{prefix}` definition or use a shell compute expression."
                ),
            });
        }
    }
}

fn validate_collisions(
    config: &config::LoadedConfig,
    sources: &BTreeMap<PathBuf, String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    validate_scope_collisions(config, sources, diagnostics);
    let mut terminal_paths: BTreeMap<Vec<String>, Vec<(PathBuf, Vec<String>)>> = BTreeMap::new();
    let implicit = ImplicitNamespaces::of(config);
    for file in &config.files {
        walk_commands(&file.commands, &mut Vec::new(), &mut |path, entry| {
            if entry.is_runnable() {
                terminal_paths
                    .entry(callable_command_path(file, path, &implicit))
                    .or_default()
                    .push((file.config_path.clone(), path.to_vec()));
            }
        });
    }
    for (path, mut entries) in terminal_paths {
        entries.sort();
        entries.dedup();
        let files: BTreeSet<&PathBuf> = entries.iter().map(|(file, _)| file).collect();
        if files.len() < 2 {
            continue;
        }
        let locations = files
            .iter()
            .map(|file| file.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        for (file, local_path) in entries {
            let source = sources.get(&file).map(String::as_str).unwrap_or("");
            diagnostics.push(Diagnostic {
                file: file.clone(),
                line: line_for_command(source, &local_path),
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

fn walk_command_chains<'a>(
    commands: &'a BTreeMap<String, CommandEntry>,
    path: &mut Vec<String>,
    chain: &mut Vec<&'a CommandEntry>,
    visit: &mut impl FnMut(&[String], &[&'a CommandEntry]),
) {
    for (name, entry) in commands {
        path.push(name.clone());
        chain.push(entry);
        visit(path, chain);
        if let Some(children) = entry.subcommands() {
            walk_command_chains(children, path, chain, visit);
        }
        chain.pop();
        path.pop();
    }
}

fn runner_strings(runner: &RunnerSetting, output: &mut Vec<(Option<&str>, String)>) {
    match runner {
        RunnerSetting::Text(value) => output.push((None, value.clone())),
        RunnerSetting::Container(container) => {
            output.push((None, container.image.clone()));
            if let Some(value) = &container.command {
                output.push((None, value.clone()));
            }
            if let Some(value) = &container.working_dir {
                output.push((None, value.clone()));
            }
            if let Some(value) = &container.user {
                output.push((None, value.clone()));
            }
            if let Some(value) = &container.network {
                output.push((None, value.clone()));
            }
            if let Some(value) = &container.network_mode {
                output.push((None, value.clone()));
            }
            output.extend(
                container
                    .volumes
                    .iter()
                    .flatten()
                    .cloned()
                    .map(|value| (None, value)),
            );
            output.extend(
                container
                    .environment
                    .values()
                    .cloned()
                    .map(|value| (None, value)),
            );
            output.extend(container.args.iter().cloned().map(|value| (None, value)));
        }
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

fn line_for_command(text: &str, path: &[String]) -> usize {
    let mut yaml_path = vec!["commands".to_string()];
    for (index, name) in path.iter().enumerate() {
        yaml_path.push(name.clone());
        if index + 1 < path.len() {
            yaml_path.push("commands".to_string());
        }
    }
    line_for_yaml_path(text, &yaml_path).unwrap_or_else(|| {
        path.last()
            .map_or(1, |name| line_for_key(text, name.as_str()))
    })
}

fn line_for_command_field(text: &str, path: &[String], field: &str) -> usize {
    let mut yaml_path = vec!["commands".to_string()];
    for name in path {
        yaml_path.push(name.clone());
        yaml_path.push("commands".to_string());
    }
    yaml_path.pop();
    yaml_path.push(field.to_string());
    line_for_yaml_path(text, &yaml_path).unwrap_or_else(|| line_for_key(text, field))
}

fn line_for_json_pointer(text: &str, pointer: &str, extra_key: Option<&str>) -> usize {
    let mut path: Vec<String> = pointer
        .split('/')
        .skip(1)
        .map(|part| part.replace("~1", "/").replace("~0", "~"))
        .collect();
    if let Some(key) = extra_key {
        path.push(key.to_string());
    }
    line_for_yaml_path(text, &path).unwrap_or_else(|| {
        extra_key
            .or_else(|| {
                path.iter()
                    .rev()
                    .find(|part| !part.chars().all(|character| character.is_ascii_digit()))
                    .map(String::as_str)
            })
            .map_or(1, |key| line_for_key(text, key))
    })
}

fn line_for_yaml_path(text: &str, path: &[String]) -> Option<usize> {
    if path.is_empty() {
        return Some(1);
    }
    let lines: Vec<&str> = text.lines().collect();
    let mut start = 0;
    let mut end = lines.len();
    let mut parent_indent: Option<usize> = None;

    for (path_index, key) in path.iter().enumerate() {
        let mut found: Option<(usize, usize)> = None;
        for (index, line) in lines.iter().enumerate().take(end).skip(start) {
            let trimmed = line.trim_start();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let indent = line.len() - trimmed.len();
            if parent_indent.is_some_and(|parent| indent <= parent) {
                continue;
            }
            if yaml_key(trimmed) == Some(key.as_str())
                && found.is_none_or(|(_, found_indent)| indent < found_indent)
            {
                found = Some((index, indent));
            }
        }
        let (index, indent) = found?;
        start = index + 1;
        end = lines
            .iter()
            .enumerate()
            .take(end)
            .skip(start)
            .find_map(|(next, line)| {
                let trimmed = line.trim_start();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    return None;
                }
                let next_indent = line.len() - trimmed.len();
                (next_indent <= indent).then_some(next)
            })
            .unwrap_or(end);
        parent_indent = Some(indent);

        if path_index + 1 == path.len() {
            return Some(index + 1);
        }
    }
    None
}

fn yaml_key(line: &str) -> Option<&str> {
    let line = line.strip_prefix("- ").unwrap_or(line);
    if let Some(quote) = line
        .chars()
        .next()
        .filter(|quote| matches!(quote, '\'' | '"'))
    {
        let rest = &line[quote.len_utf8()..];
        if let Some(end) = rest.find(quote) {
            let after = rest[end + quote.len_utf8()..].trim_start();
            if after.starts_with(':') {
                return Some(&rest[..end]);
            }
        }
    }
    let (candidate, _) = line.split_once(':')?;
    Some(candidate.trim().trim_matches(['\'', '"']))
}
