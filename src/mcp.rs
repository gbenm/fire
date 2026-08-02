use serde::Serialize;

use rmcp::{
    ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    schemars, serve_server, tool, tool_handler, tool_router,
    transport::io::stdio,
};

use crate::config::{
    local_implicit_namespace, CommandEntry, FileScope, LoadedConfig, SourceKind,
};

#[derive(Debug, Serialize)]
pub(crate) struct CommandContext {
    pub(crate) fire_version: String,
    pub(crate) implicit_local_namespace: Option<String>,
    pub(crate) notes: Vec<String>,
    pub(crate) commands: Vec<CommandNode>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct CommandNode {
    pub(crate) path: String,
    pub(crate) name: String,
    pub(crate) namespace: Option<String>,
    pub(crate) group: Option<String>,
    pub(crate) source: &'static str,
    pub(crate) description: Option<String>,
    pub(crate) runnable: bool,
    pub(crate) exec: Option<Vec<String>>,
    pub(crate) eval: Option<Vec<String>>,
    pub(crate) placeholder: Option<String>,
    pub(crate) subcommands: Vec<CommandNode>,
}

#[derive(Debug, Serialize)]
pub(crate) struct CommandMatch {
    pub(crate) path: String,
    pub(crate) description: Option<String>,
    pub(crate) runnable: bool,
    pub(crate) exec: Option<Vec<String>>,
    pub(crate) eval: Option<Vec<String>>,
}

pub(crate) fn build_command_context(config: &LoadedConfig) -> CommandContext {
    let implicit_local_namespace = local_implicit_namespace(config);
    let mut commands = Vec::new();

    for file in &config.files {
        let source = match file.source {
            SourceKind::Local => "local",
            SourceKind::Global => "global",
        };
        let (namespace, group) = scope_labels(&file.scope);
        let scope_prefix: Vec<String> = namespace.iter().chain(group.iter()).cloned().collect();

        for (name, entry) in &file.commands {
            let mut path_tokens = scope_prefix.clone();
            path_tokens.push(name.clone());
            commands.push(build_node(
                &path_tokens,
                name,
                entry,
                namespace.as_deref(),
                group.as_deref(),
                source,
            ));
        }
    }

    CommandContext {
        fire_version: crate::FIRE_VERSION.to_string(),
        implicit_local_namespace,
        notes: vec![
            "`path` is the canonical invocation, e.g. `fire backend logs`. Run it directly in a shell.".to_string(),
            "When a single local namespace is active (see `implicit_local_namespace`), its prefix token can be omitted.".to_string(),
            "Nodes with `runnable: false` are command groups; invoke one of their `subcommands` instead.".to_string(),
            "`exec`/`eval` show the underlying template(s) a command runs, for context only — invoke via `fire <path>`, not by copying these templates.".to_string(),
        ],
        commands,
    }
}

fn scope_labels(scope: &FileScope) -> (Option<String>, Option<String>) {
    match scope {
        FileScope::Root => (None, None),
        FileScope::Namespace { namespace, .. } => (Some(namespace.clone()), None),
        FileScope::Group { group, .. } => (None, Some(group.clone())),
        FileScope::NamespaceGroup {
            namespace, group, ..
        } => (Some(namespace.clone()), Some(group.clone())),
    }
}

fn build_node(
    path_tokens: &[String],
    name: &str,
    entry: &CommandEntry,
    namespace: Option<&str>,
    group: Option<&str>,
    source: &'static str,
) -> CommandNode {
    let subcommands = entry
        .subcommands()
        .map(|map| {
            map.iter()
                .map(|(sub_name, sub_entry)| {
                    let mut sub_path = path_tokens.to_vec();
                    sub_path.push(sub_name.clone());
                    build_node(&sub_path, sub_name, sub_entry, namespace, group, source)
                })
                .collect()
        })
        .unwrap_or_default();

    CommandNode {
        path: format!("fire {}", path_tokens.join(" ")),
        name: name.to_string(),
        namespace: namespace.map(str::to_string),
        group: group.map(str::to_string),
        source,
        description: non_empty(entry.description().unwrap_or_default()),
        runnable: entry.is_runnable(),
        exec: entry.execution_commands(),
        eval: entry.evaluation_expressions(),
        placeholder: entry.spec().and_then(|spec| non_empty(&spec.placeholder)),
        subcommands,
    }
}

fn non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn search_context(context: &CommandContext, query: &str) -> Vec<CommandMatch> {
    let mut out = Vec::new();
    let needle = query.to_lowercase();
    for node in &context.commands {
        collect_matches(node, &needle, &mut out);
    }
    out
}

fn collect_matches(node: &CommandNode, needle: &str, out: &mut Vec<CommandMatch>) {
    if node_matches(node, needle) {
        out.push(CommandMatch {
            path: node.path.clone(),
            description: node.description.clone(),
            runnable: node.runnable,
            exec: node.exec.clone(),
            eval: node.eval.clone(),
        });
    }
    for child in &node.subcommands {
        collect_matches(child, needle, out);
    }
}

fn node_matches(node: &CommandNode, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    if node.path.to_lowercase().contains(needle) {
        return true;
    }
    if let Some(description) = &node.description {
        if description.to_lowercase().contains(needle) {
            return true;
        }
    }
    if let Some(exec) = &node.exec {
        if exec.iter().any(|line| line.to_lowercase().contains(needle)) {
            return true;
        }
    }
    if let Some(eval) = &node.eval {
        if eval.iter().any(|line| line.to_lowercase().contains(needle)) {
            return true;
        }
    }
    false
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SearchCommandsRequest {
    /// Case-insensitive substring to match against command paths, descriptions, and exec/eval templates.
    query: String,
}

#[derive(Debug, Clone)]
pub(crate) struct FireMcpServer {
    tool_router: ToolRouter<Self>,
}

impl FireMcpServer {
    pub(crate) fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl FireMcpServer {
    #[tool(
        description = "List the full tree of `fire` commands available, including their invocation paths, descriptions, and exec/eval templates."
    )]
    fn list_commands(&self) -> String {
        let config = crate::config::load_config();
        let context = build_command_context(&config);
        to_json(&context)
    }

    #[tool(
        description = "Search `fire` commands by a case-insensitive substring match against path, description, and exec/eval templates."
    )]
    fn search_commands(
        &self,
        Parameters(SearchCommandsRequest { query }): Parameters<SearchCommandsRequest>,
    ) -> String {
        let config = crate::config::load_config();
        let context = build_command_context(&config);
        to_json(&search_context(&context, &query))
    }
}

fn to_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string_pretty(value)
        .unwrap_or_else(|err| format!("{{\"error\": \"failed to serialize response: {err}\"}}"))
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for FireMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "Provides context about the `fire` CLI's available commands \
             (https://github.com/gbenm/fire). Call `list_commands` for the full command tree, or \
             `search_commands` to filter it. Each command's `path` field is the exact shell \
             invocation, e.g. `fire backend logs`. This server only provides context; to actually \
             run a command, invoke `fire <path>` directly in a shell.",
        )
    }
}

pub(crate) async fn serve_stdio() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let server = FireMcpServer::new();
    let running = serve_server(server, stdio()).await?;
    running.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, path::PathBuf};

    use super::*;
    use crate::config::{FileConfig, FileScope, SourceKind};

    fn parse_commands(yaml: &str) -> BTreeMap<String, CommandEntry> {
        #[derive(serde::Deserialize)]
        struct Wrapper {
            commands: BTreeMap<String, CommandEntry>,
        }
        yaml_serde::from_str::<Wrapper>(yaml)
            .expect("valid yaml")
            .commands
    }

    #[test]
    fn root_scope_produces_bare_path() {
        let config = LoadedConfig {
            files: vec![FileConfig {
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
            }],
        };

        let context = build_command_context(&config);
        let run = context
            .commands
            .iter()
            .find(|node| node.name == "run")
            .expect("run command");
        assert_eq!(run.path, "fire run");
        assert_eq!(run.description.as_deref(), Some("Run local"));
        assert_eq!(run.exec, Some(vec!["npm run".to_string()]));
        assert!(run.runnable);
    }

    #[test]
    fn namespace_group_scope_prefixes_path() {
        let config = LoadedConfig {
            files: vec![FileConfig {
                source: SourceKind::Global,
                project_dir: PathBuf::from("."),
                config_path: PathBuf::from("/tmp/fire-test.yml"),
                scope: FileScope::NamespaceGroup {
                    namespace: "ex".to_string(),
                    namespace_description: String::new(),
                    group: "backend".to_string(),
                    group_description: String::new(),
                },
                runtimes: BTreeMap::new(),
                commands: parse_commands(
                    r#"
commands:
  logs:
    exec: docker compose logs
"#,
                ),
            }],
        };

        let context = build_command_context(&config);
        let logs = context
            .commands
            .iter()
            .find(|node| node.name == "logs")
            .expect("logs command");
        assert_eq!(logs.path, "fire ex backend logs");
        assert_eq!(logs.namespace.as_deref(), Some("ex"));
        assert_eq!(logs.group.as_deref(), Some("backend"));
    }

    #[test]
    fn nested_subcommands_build_full_path() {
        let config = LoadedConfig {
            files: vec![FileConfig {
                source: SourceKind::Local,
                project_dir: PathBuf::from("."),
                config_path: PathBuf::from("/tmp/fire-test.yml"),
                scope: FileScope::Root,
                runtimes: BTreeMap::new(),
                commands: parse_commands(
                    r#"
commands:
  double:
    commands:
      hello:
        description: "say twice"
        exec:
          - echo hello world
          - echo again {1}
"#,
                ),
            }],
        };

        let context = build_command_context(&config);
        let double = context
            .commands
            .iter()
            .find(|node| node.name == "double")
            .expect("double command");
        assert!(!double.runnable);
        let hello = double
            .subcommands
            .iter()
            .find(|node| node.name == "hello")
            .expect("hello subcommand");
        assert_eq!(hello.path, "fire double hello");
        assert_eq!(
            hello.exec,
            Some(vec![
                "echo hello world".to_string(),
                "echo again {1}".to_string()
            ])
        );
    }

    #[test]
    fn eval_only_command_captures_eval_expressions() {
        let config = LoadedConfig {
            files: vec![FileConfig {
                source: SourceKind::Local,
                project_dir: PathBuf::from("."),
                config_path: PathBuf::from("/tmp/fire-test.yml"),
                scope: FileScope::Root,
                runtimes: BTreeMap::new(),
                commands: parse_commands(
                    r#"
commands:
  computed:
    eval: py:sayHello("{1}")
"#,
                ),
            }],
        };

        let context = build_command_context(&config);
        let computed = context
            .commands
            .iter()
            .find(|node| node.name == "computed")
            .expect("computed command");
        assert_eq!(computed.eval, Some(vec!["py:sayHello(\"{1}\")".to_string()]));
        assert_eq!(computed.exec, None);
    }

    #[test]
    fn search_matches_on_description_and_exec() {
        let config = LoadedConfig {
            files: vec![FileConfig {
                source: SourceKind::Local,
                project_dir: PathBuf::from("."),
                config_path: PathBuf::from("/tmp/fire-test.yml"),
                scope: FileScope::Root,
                runtimes: BTreeMap::new(),
                commands: parse_commands(
                    r#"
commands:
  run:
    description: Run the backend service
    exec: npm run start
  other:
    exec: echo unrelated
"#,
                ),
            }],
        };

        let context = build_command_context(&config);
        let matches = search_context(&context, "backend");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].path, "fire run");

        let matches = search_context(&context, "npm");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].path, "fire run");
    }
}
