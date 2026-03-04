use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
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
    pub(crate) project_dir: PathBuf,
    pub(crate) config_path: PathBuf,
    pub(crate) scope: FileScope,
    pub(crate) runtimes: BTreeMap<String, RuntimeConfig>,
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
    pub(crate) before: String,
    #[serde(default)]
    pub(crate) placeholder: String,
    #[serde(default)]
    pub(crate) on_unused_args: String,
    #[serde(default)]
    pub(crate) compute: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) macros: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) dir: String,
    #[serde(default)]
    pub(crate) check: String,
    #[serde(default)]
    pub(crate) runner: String,
    #[serde(default)]
    pub(crate) fallback_runner: String,
    #[serde(default)]
    pub(crate) exec: Option<CommandAction>,
    #[serde(default)]
    pub(crate) run: Option<CommandAction>,
    #[serde(default)]
    pub(crate) eval: Option<CommandAction>,
    #[serde(default)]
    pub(crate) commands: BTreeMap<String, CommandEntry>,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub(crate) struct RuntimeConfig {
    #[serde(default)]
    pub(crate) sdk: String,
    #[serde(default)]
    pub(crate) runner: String,
    #[serde(default)]
    pub(crate) check: String,
    #[serde(default)]
    pub(crate) fallback_runner: String,
    #[serde(default)]
    pub(crate) paths: Vec<String>,
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
    include: Vec<String>,
    #[serde(default)]
    runtimes: BTreeMap<String, RuntimeConfig>,
    #[serde(default)]
    commands: BTreeMap<String, CommandEntry>,
    #[serde(flatten)]
    _extra: BTreeMap<String, serde::de::IgnoredAny>,
}

#[derive(Debug, Deserialize, Clone, Default)]
struct NamespaceRaw {
    #[serde(default)]
    prefix: String,
    #[serde(default)]
    description: String,
}

#[derive(Debug, Clone)]
struct NamespaceScope {
    prefix: String,
    description: String,
}

#[derive(Debug, Clone)]
struct ParsedFireFile {
    path: PathBuf,
    raw: FireFileRaw,
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

    pub(crate) fn is_runnable(&self) -> bool {
        match self {
            CommandEntry::Shorthand(_) => true,
            CommandEntry::Spec(spec) => {
                spec.exec.is_some() || spec.run.is_some() || spec.eval.is_some()
            }
        }
    }

    pub(crate) fn subcommands(&self) -> Option<&BTreeMap<String, CommandEntry>> {
        match self {
            CommandEntry::Shorthand(_) => None,
            CommandEntry::Spec(spec) => Some(&spec.commands),
        }
    }

    pub(crate) fn evaluation_expressions(&self) -> Option<Vec<String>> {
        match self {
            CommandEntry::Shorthand(_) => None,
            CommandEntry::Spec(spec) => spec.eval.as_ref().map(CommandAction::as_vec),
        }
    }

    pub(crate) fn spec(&self) -> Option<&CommandSpec> {
        match self {
            CommandEntry::Shorthand(_) => None,
            CommandEntry::Spec(spec) => Some(spec),
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

    // Attempt git root detection FIRST
    let project_root = detect_git_root(&cwd).unwrap_or(cwd.clone());

    // Local project commands.
    loaded
        .files
        .extend(load_local_file_configs(&project_root, SourceKind::Local));

    // Global installed directories. No implicit namespace here:
    // files without namespace/group stay as direct global commands.
    let installed_dirs = load_installed_directories();
    for directory in installed_dirs {
        if directory == project_root {
            continue;
        }
        loaded
            .files
            .extend(load_global_file_configs(&directory, SourceKind::Global));
    }

    loaded
}

pub(crate) fn local_implicit_namespace(config: &LoadedConfig) -> Option<String> {
    let namespaces: BTreeSet<String> = config
        .files
        .iter()
        .filter(|file| file.source == SourceKind::Local)
        .filter_map(|file| match &file.scope {
            FileScope::Namespace { namespace, .. } => Some(namespace.clone()),
            FileScope::NamespaceGroup { namespace, .. } => Some(namespace.clone()),
            FileScope::Root | FileScope::Group { .. } => None,
        })
        .collect();

    if namespaces.len() == 1 {
        namespaces.into_iter().next()
    } else {
        None
    }
}

fn detect_git_root(cwd: &Path) -> Option<PathBuf> {
    if !cwd.exists() {
        return None;
    }

    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(cwd)
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let path_str = String::from_utf8(output.stdout).ok()?;
    let path = PathBuf::from(path_str.trim());

    // Check if git root has ANY fire config.
    let config_files = discover_config_files(&path);
    if config_files.is_empty() {
        return None;
    }

    // Check if any of the root config files defines a namespace.
    let raw = parse_raw_files(&config_files);
    let has_explicit_namespace = select_directory_explicit_namespace(&raw).is_some();

    if has_explicit_namespace {
        return Some(path);
    }

    None
}

fn load_local_file_configs(cwd: &Path, source: SourceKind) -> Vec<FileConfig> {
    load_directory_file_configs(cwd, source, DefaultNamespaceMode::DirectoryFallback)
}

fn load_global_file_configs(directory: &Path, source: SourceKind) -> Vec<FileConfig> {
    load_directory_file_configs(directory, source, DefaultNamespaceMode::ExplicitOnly)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DefaultNamespaceMode {
    DirectoryFallback,
    ExplicitOnly,
}

fn load_directory_file_configs(
    base_dir: &Path,
    source: SourceKind,
    namespace_mode: DefaultNamespaceMode,
) -> Vec<FileConfig> {
    let root_paths = discover_config_files(base_dir);
    let root_raw = parse_raw_files(&root_paths);
    let explicit_namespace = select_directory_explicit_namespace(&root_raw);

    let default_namespace = match namespace_mode {
        DefaultNamespaceMode::DirectoryFallback => {
            Some(select_local_default_namespace_prefix(&root_raw, base_dir))
        }
        DefaultNamespaceMode::ExplicitOnly => select_directory_default_namespace_prefix(&root_raw),
    };

    let include_dirs = resolve_include_directories(base_dir, &root_raw);
    let include_paths = discover_config_files_from_dirs(&include_dirs);
    let include_raw = parse_raw_files(&include_paths);

    let mut files = to_file_configs(
        &root_raw,
        source,
        base_dir,
        default_namespace.as_deref(),
        None,
    );
    files.extend(to_file_configs(
        &include_raw,
        source,
        base_dir,
        default_namespace.as_deref(),
        explicit_namespace.as_ref(),
    ));
    files
}

fn parse_raw_files(paths: &[PathBuf]) -> Vec<ParsedFireFile> {
    let mut parsed = Vec::new();
    for path in paths {
        let Ok(text) = fs::read_to_string(path) else {
            eprintln!("[fire] Could not read {}. Skipping.", path.display());
            continue;
        };

        let mut value: yaml_serde::Value = match yaml_serde::from_str(&text) {
            Ok(v) => v,
            Err(err) => {
                eprintln!(
                    "[fire] Invalid YAML in {}: {}. Skipping.",
                    path.display(),
                    err
                );
                continue;
            }
        };

        if let Err(e) = value.apply_merge() {
            eprintln!(
                "[fire] Merge failed in {}: {}. Skipping.",
                path.display(),
                e
            );
            continue;
        }

        let value: FireFileRaw = match yaml_serde::from_value(value) {
            Ok(value) => value,
            Err(err) => {
                eprintln!(
                    "[fire] Invalid config structure in {}: {}. Skipping.",
                    path.display(),
                    err
                );
                continue;
            }
        };
        parsed.push(ParsedFireFile {
            path: path.clone(),
            raw: value,
        });
    }
    parsed
}

fn to_file_configs(
    raw_files: &[ParsedFireFile],
    source: SourceKind,
    project_dir: &Path,
    default_namespace_prefix: Option<&str>,
    forced_namespace: Option<&NamespaceScope>,
) -> Vec<FileConfig> {
    raw_files
        .iter()
        .map(|parsed| FileConfig {
            source,
            project_dir: project_dir.to_path_buf(),
            config_path: parsed.path.clone(),
            scope: scope_from_raw(&parsed.raw, default_namespace_prefix, forced_namespace),
            runtimes: parsed.raw.runtimes.clone(),
            commands: parsed.raw.commands.clone(),
        })
        .collect()
}

fn scope_from_raw(
    raw: &FireFileRaw,
    default_namespace_prefix: Option<&str>,
    forced_namespace: Option<&NamespaceScope>,
) -> FileScope {
    let group = raw.group.trim();
    let (namespace_prefix, namespace_description) = if let Some(namespace) = forced_namespace {
        (namespace.prefix.as_str(), namespace.description.clone())
    } else {
        let prefix = raw
            .namespace
            .as_ref()
            .map(|namespace| namespace.prefix.trim())
            .filter(|value| !value.is_empty())
            .or(default_namespace_prefix)
            .unwrap_or("");
        let description = raw
            .namespace
            .as_ref()
            .map(|namespace| namespace.description.trim())
            .unwrap_or("")
            .to_string();
        (prefix, description)
    };

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

fn select_directory_default_namespace_prefix(raw_files: &[ParsedFireFile]) -> Option<String> {
    select_directory_explicit_namespace(raw_files).map(|namespace| namespace.prefix)
}

fn select_directory_explicit_namespace(raw_files: &[ParsedFireFile]) -> Option<NamespaceScope> {
    for parsed in raw_files {
        let raw = &parsed.raw;
        if let Some(namespace) = &raw.namespace {
            let prefix = namespace.prefix.trim();
            if !prefix.is_empty() {
                return Some(NamespaceScope {
                    prefix: prefix.to_string(),
                    description: namespace.description.trim().to_string(),
                });
            }
        }
    }
    None
}

fn select_local_default_namespace_prefix(raw_files: &[ParsedFireFile], cwd: &Path) -> String {
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

fn discover_config_files_from_dirs(directories: &[PathBuf]) -> Vec<PathBuf> {
    let mut files = BTreeSet::new();
    for directory in directories {
        for path in discover_config_files(directory) {
            files.insert(path);
        }
    }
    files.into_iter().collect()
}

fn resolve_include_directories(base_dir: &Path, raw_files: &[ParsedFireFile]) -> Vec<PathBuf> {
    let mut directories = BTreeSet::new();

    for parsed in raw_files {
        let raw = &parsed.raw;
        for include in &raw.include {
            let trimmed = include.trim();
            if trimmed.is_empty() {
                continue;
            }

            let Some(relative) = normalize_relative_include_path(trimmed) else {
                eprintln!("[fire] Invalid include path '{trimmed}'. Skipping.");
                continue;
            };

            let path = base_dir.join(relative);
            if !path.is_dir() {
                eprintln!(
                    "[fire] Include directory '{}' does not exist. Skipping.",
                    path.display()
                );
                continue;
            }

            directories.insert(path);
        }
    }

    directories.into_iter().collect()
}

fn normalize_relative_include_path(path: &str) -> Option<PathBuf> {
    let raw = Path::new(path);
    if raw.is_absolute() {
        return None;
    }

    let mut normalized = PathBuf::new();
    for component in raw.components() {
        match component {
            Component::Normal(segment) => normalized.push(segment),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }

    if normalized.as_os_str().is_empty() {
        return None;
    }

    Some(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("fire-{prefix}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn missing_namespace_uses_directory_explicit_prefix() {
        let raw = FireFileRaw::default();
        let scope = scope_from_raw(&raw, Some("ex"), None);
        match scope {
            FileScope::Namespace { namespace, .. } => assert_eq!(namespace, "ex"),
            _ => panic!("expected namespace scope"),
        }
    }

    #[test]
    fn missing_namespace_without_directory_prefix_stays_root() {
        let raw = FireFileRaw::default();
        let scope = scope_from_raw(&raw, None, None);
        match scope {
            FileScope::Root => {}
            _ => panic!("expected root scope"),
        }
    }

    #[test]
    fn select_directory_default_namespace_prefix_reads_first_explicit() {
        let files = vec![
            ParsedFireFile {
                path: PathBuf::from("/tmp/one.fire.yml"),
                raw: FireFileRaw {
                    group: String::new(),
                    namespace: Some(NamespaceRaw {
                        prefix: "ex".to_string(),
                        description: String::new(),
                    }),
                    include: Vec::new(),
                    runtimes: BTreeMap::new(),
                    commands: BTreeMap::new(),
                    _extra: BTreeMap::new(),
                },
            },
            ParsedFireFile {
                path: PathBuf::from("/tmp/two.fire.yml"),
                raw: FireFileRaw {
                    group: String::new(),
                    namespace: None,
                    include: Vec::new(),
                    runtimes: BTreeMap::new(),
                    commands: BTreeMap::new(),
                    _extra: BTreeMap::new(),
                },
            },
        ];
        let selected = select_directory_default_namespace_prefix(&files);
        assert_eq!(selected, Some("ex".to_string()));
    }

    #[test]
    fn select_local_default_namespace_prefix_falls_back_to_directory_name() {
        let files = vec![ParsedFireFile {
            path: PathBuf::from("/tmp/one.fire.yml"),
            raw: FireFileRaw::default(),
        }];
        let cwd = PathBuf::from("/tmp/My Project");
        let selected = select_local_default_namespace_prefix(&files, &cwd);
        assert_eq!(selected, "my-project".to_string());
    }

    #[test]
    fn included_file_uses_forced_root_namespace() {
        let raw = FireFileRaw {
            group: "backend".to_string(),
            namespace: Some(NamespaceRaw {
                prefix: "custom".to_string(),
                description: "Custom".to_string(),
            }),
            include: Vec::new(),
            runtimes: BTreeMap::new(),
            commands: BTreeMap::new(),
            _extra: BTreeMap::new(),
        };
        let forced = NamespaceScope {
            prefix: "ex".to_string(),
            description: "Example".to_string(),
        };

        let scope = scope_from_raw(&raw, None, Some(&forced));
        match scope {
            FileScope::NamespaceGroup {
                namespace,
                namespace_description,
                group,
            } => {
                assert_eq!(namespace, "ex");
                assert_eq!(namespace_description, "Example");
                assert_eq!(group, "backend");
            }
            _ => panic!("expected namespace group scope"),
        }
    }

    #[test]
    fn include_paths_must_be_relative_and_non_parent() {
        assert!(normalize_relative_include_path("samples/").is_some());
        assert!(normalize_relative_include_path("./samples").is_some());
        assert!(normalize_relative_include_path("../samples").is_none());
        assert!(normalize_relative_include_path("/abs").is_none());
        assert!(normalize_relative_include_path("").is_none());
    }

    #[test]
    fn local_load_includes_directories_without_recursion_and_inherits_namespace() {
        let root = unique_temp_dir("local-include");
        let samples_dir = root.join("samples");
        let nested_dir = samples_dir.join("nested");
        fs::create_dir_all(&nested_dir).expect("create include dirs");

        fs::write(
            root.join("fire.yml"),
            r#"
namespace:
  prefix: ex
  description: Example
include:
  - samples/
commands:
  run:
    exec: npm run
"#,
        )
        .expect("write root file");

        fs::write(
            samples_dir.join("deploy.fire.yml"),
            r#"
group: backend
namespace:
  prefix: ignored
  description: Ignored
commands:
  build:
    exec: npm run build
"#,
        )
        .expect("write included file");

        fs::write(
            nested_dir.join("ignored.fire.yml"),
            r#"
commands:
  deep:
    exec: echo deep
"#,
        )
        .expect("write nested file");

        let files = load_local_file_configs(&root, SourceKind::Local);
        let has_build = files.iter().any(|file| file.commands.contains_key("build"));
        let has_deep = files.iter().any(|file| file.commands.contains_key("deep"));
        assert!(has_build);
        assert!(!has_deep);

        let backend_scope = files
            .iter()
            .find(|file| file.commands.contains_key("build"))
            .map(|file| file.scope.clone())
            .expect("backend scope");

        match backend_scope {
            FileScope::NamespaceGroup {
                namespace,
                namespace_description,
                group,
            } => {
                assert_eq!(namespace, "ex");
                assert_eq!(namespace_description, "Example");
                assert_eq!(group, "backend");
            }
            _ => panic!("expected namespace group"),
        }

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn global_load_includes_directories_without_extra_install_step() {
        let root = unique_temp_dir("global-include");
        let samples_dir = root.join("samples");
        fs::create_dir_all(&samples_dir).expect("create include dirs");

        fs::write(
            root.join("fire.yml"),
            r#"
include:
  - samples/
commands:
  root:
    exec: echo root
"#,
        )
        .expect("write root file");

        fs::write(
            samples_dir.join("test.fire.yml"),
            r#"
commands:
  test:
    exec: echo test
"#,
        )
        .expect("write included file");

        let files = load_global_file_configs(&root, SourceKind::Global);
        let has_root = files.iter().any(|file| file.commands.contains_key("root"));
        let has_test = files.iter().any(|file| file.commands.contains_key("test"));

        assert!(has_root);
        assert!(has_test);

        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn eval_only_command_is_runnable() {
        let command = CommandEntry::Spec(CommandSpec {
            eval: Some(CommandAction::Single("py:sayHello()".to_string())),
            ..CommandSpec::default()
        });
        assert!(command.is_runnable());
    }

    #[test]
    fn command_without_exec_run_or_eval_is_not_runnable() {
        let command = CommandEntry::Spec(CommandSpec::default());
        assert!(!command.is_runnable());
    }

    #[test]
    fn yaml_anchors_merge_keys_work() {
        let yaml = r#"
x-common: &common
  description: Shared description

commands:
  param:
    <<: *common
    exec: echo hi
"#;
        let mut value: yaml_serde::Value = yaml_serde::from_str(yaml).expect("parse yaml");
        // Apply merge keys manually as we do in production code
        value.apply_merge().expect("merge");
        let raw: FireFileRaw = yaml_serde::from_value(value).expect("deserialize");

        let command = raw.commands.get("param").expect("param command");
        match command {
            CommandEntry::Spec(spec) => {
                assert_eq!(spec.description, "Shared description");
            }
            _ => panic!("expected spec"),
        }
    }

    #[test]
    fn local_implicit_namespace_requires_single_value() {
        let config_single = LoadedConfig {
            files: vec![FileConfig {
                source: SourceKind::Local,
                project_dir: PathBuf::from("."),
                config_path: PathBuf::from("/tmp/fire-test.yml"),
                scope: FileScope::Namespace {
                    namespace: "ex".to_string(),
                    namespace_description: String::new(),
                },
                runtimes: BTreeMap::new(),
                commands: BTreeMap::new(),
            }],
        };
        assert_eq!(
            local_implicit_namespace(&config_single),
            Some("ex".to_string())
        );

        let config_multiple = LoadedConfig {
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
                    source: SourceKind::Local,
                    project_dir: PathBuf::from("."),
                    config_path: PathBuf::from("/tmp/fire-test.yml"),
                    scope: FileScope::Namespace {
                        namespace: "qa".to_string(),
                        namespace_description: String::new(),
                    },
                    runtimes: BTreeMap::new(),
                    commands: BTreeMap::new(),
                },
            ],
        };
        assert_eq!(local_implicit_namespace(&config_multiple), None);
    }
}
