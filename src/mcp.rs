use serde::Serialize;

use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    schemars, serve_server, tool, tool_handler, tool_router,
    transport::io::stdio,
    ServerHandler,
};

use crate::config::{local_implicit_namespace, CommandEntry, FileScope, LoadedConfig, SourceKind};

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
    /// Server, watcher or worker: run it with `start_session`, not in your own shell.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) long_running: bool,
    pub(crate) subcommands: Vec<CommandNode>,
}

#[derive(Debug, Serialize)]
pub(crate) struct CommandMatch {
    pub(crate) path: String,
    pub(crate) description: Option<String>,
    pub(crate) runnable: bool,
    pub(crate) exec: Option<Vec<String>>,
    pub(crate) eval: Option<Vec<String>>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) long_running: bool,
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
            if !entry.allows_agents() {
                continue;
            }
            let mut path_tokens = scope_prefix.clone();
            path_tokens.push(name.clone());
            commands.push(build_node(
                &path_tokens,
                name,
                entry,
                namespace.as_deref(),
                group.as_deref(),
                source,
                None,
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
            "`long_running: true` marks servers, watchers and workers: start them with `start_session`. Run from a shell without a terminal, they detach on their own and print a session id.".to_string(),
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
    inherited_long_running: Option<bool>,
) -> CommandNode {
    let long_running = entry
        .spec()
        .and_then(|spec| spec.long_running)
        .or(inherited_long_running);
    let subcommands = entry
        .subcommands()
        .map(|map| {
            map.iter()
                .filter(|(_, sub_entry)| sub_entry.allows_agents())
                .map(|(sub_name, sub_entry)| {
                    let mut sub_path = path_tokens.to_vec();
                    sub_path.push(sub_name.clone());
                    build_node(
                        &sub_path,
                        sub_name,
                        sub_entry,
                        namespace,
                        group,
                        source,
                        long_running,
                    )
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
        long_running: long_running == Some(true),
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
            long_running: node.long_running,
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

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SessionOutputRequest {
    /// Session id, as returned by `list_sessions` or `fire cli bg`.
    id: String,
    /// How many trailing lines of output to return (default 100).
    lines: Option<usize>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct StartSessionRequest {
    /// The fire command path plus its arguments, one token per item, without the leading `fire`,
    /// e.g. `["api", "dev", "--port", "3000"]` for `fire api dev --port 3000`.
    command: Vec<String>,
    /// Very short context about why you are starting it (e.g. "api for auth e2e tests"). Other
    /// agents and the user see it in `list_sessions`.
    note: String,
    /// Project directory to resolve and run the command from. Defaults to the server's cwd.
    cwd: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SendInputRequest {
    /// Session id.
    id: String,
    /// Text typed into the session's terminal. Enter is appended unless `raw` is true.
    text: String,
    /// Send `text` as keys, without Enter, interpreting escapes: `\r` (Enter), `\n`, `\t`,
    /// `\e` (Esc), `\\`, `\xHH` (any byte, e.g. `\x03` = Ctrl+C, `\e[A` = up arrow).
    raw: Option<bool>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SignalSessionRequest {
    /// Session id.
    id: String,
    /// Signal name or number (INT, TERM, HUP, USR1, USR2, QUIT, KILL, STOP, CONT, 9...). Omit to
    /// stop the session gracefully (TERM, then KILL after 5s) or to remove an exited session.
    signal: Option<String>,
}

#[derive(Debug, Serialize)]
struct CliResult {
    ok: bool,
    stdout: String,
    stderr: String,
}

/// Session management reuses the CLI (`fire cli bg|send|kill`) so both paths behave the same.
fn run_fire_cli(args: &[&str], cwd: Option<&str>) -> String {
    let exe = std::env::current_exe().unwrap_or_else(|_| "fire".into());
    let mut command = std::process::Command::new(exe);
    command
        .arg("cli")
        .args(args)
        .stdin(std::process::Stdio::null());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let result = match command.output() {
        Ok(output) => CliResult {
            ok: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        },
        Err(err) => CliResult {
            ok: false,
            stdout: String::new(),
            stderr: format!("failed to run fire: {err}"),
        },
    };
    to_json(&result)
}

#[derive(Debug, Serialize)]
struct SessionOutput {
    #[serde(flatten)]
    session: crate::session::SessionInfo,
    /// The terminal output (stdout and stderr merged) as plain text.
    output: String,
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
        description = "List the full tree of `fire` commands the user configured (builds, tests, dev servers, deploys...), with invocation paths, descriptions, and exec/eval templates. Prefer these over ad-hoc shell commands."
    )]
    fn list_commands(&self) -> String {
        let config = crate::config::load_config();
        let context = build_command_context(&config);
        to_json(&context)
    }

    #[tool(
        description = "Search the user's configured `fire` commands (case-insensitive substring over path, description, and exec/eval templates). First stop when the user names an app, service or task you don't recognize (\"start X\", \"run the Y tests\"): it is likely a fire command. Also use it before writing your own build/test/run/deploy commands."
    )]
    fn search_commands(
        &self,
        Parameters(SearchCommandsRequest { query }): Parameters<SearchCommandsRequest>,
    ) -> String {
        let config = crate::config::load_config();
        let context = build_command_context(&config);
        to_json(&search_context(&context, &query))
    }

    #[tool(
        description = "List detached `fire` sessions across all projects (started with `fire cli bg`, by any agent or the user): id, command, note (why it was started), cwd, status (`running`, `exited`, `stopped` = stopped on purpose, `lost`), pid and exit code. Check this before starting or restarting servers to avoid duplicates. If a previous session for the same command exited, read its `session_output` first: it usually shows why it failed (e.g. port in use)."
    )]
    fn list_sessions(&self) -> String {
        let sessions: Vec<_> = crate::session::list_sessions()
            .into_iter()
            .map(crate::session::Session::info)
            .collect();
        to_json(&sessions)
    }

    #[tool(
        description = "Read the last lines of a session's terminal output (stdout and stderr merged, colors and progress redraws cleaned up), plus its status."
    )]
    fn session_output(
        &self,
        Parameters(SessionOutputRequest { id, lines }): Parameters<SessionOutputRequest>,
    ) -> String {
        match crate::session::read_output(&id, lines.unwrap_or(100)) {
            Some((session, output)) => to_json(&SessionOutput {
                session: session.info(),
                output,
            }),
            None => format!("{{\"error\": \"no session `{id}`\"}}"),
        }
    }

    #[tool(
        description = "Start a configured `fire` command detached, in a managed session. ALWAYS use this for long-running processes (dev servers, watchers, workers, tunnels, log tails) instead of backgrounding them in your own shell: the process survives your shell, and you, other agents and the user can list it, read its output, send input/signals, attach, or stop it. `command` must be a configured fire command (find it with `search_commands`). One command in one cwd is one session: if it is already running, this returns its id and replaces its note instead of starting a duplicate; if it ended (crashed or stopped), it starts again under the same id with the new note. So to restart a server, stop it with `signal_session` and call this again. Returns the session id in `stdout`; then confirm it came up with `session_output`. Fails for commands the user reserved for humans (`agents: false`)."
    )]
    fn start_session(
        &self,
        Parameters(StartSessionRequest { command, note, cwd }): Parameters<StartSessionRequest>,
    ) -> String {
        let mut args = vec!["bg", "--agent", "-m", note.as_str()];
        args.extend(command.iter().map(String::as_str));
        run_fire_cli(&args, cwd.as_deref())
    }

    #[tool(
        description = "Type into a running session's terminal (it runs on a pty): a line of text followed by Enter, or with `raw: true` exact keys such as `q`, `\\x03` (Ctrl+C) or `\\e[A` (up arrow)."
    )]
    fn send_input(
        &self,
        Parameters(SendInputRequest { id, text, raw }): Parameters<SendInputRequest>,
    ) -> String {
        if raw == Some(true) {
            run_fire_cli(&["send", "--raw", &id, &text], None)
        } else {
            run_fire_cli(&["send", &id, &text], None)
        }
    }

    #[tool(
        description = "Send a signal to a session's process group (e.g. INT, HUP, USR1), or omit `signal` to stop it (TERM, then KILL after 5s). On an already-exited session, removes its record."
    )]
    fn signal_session(
        &self,
        Parameters(SignalSessionRequest { id, signal }): Parameters<SignalSessionRequest>,
    ) -> String {
        match signal {
            Some(signal) => run_fire_cli(&["kill", &id, &signal], None),
            None => run_fire_cli(&["kill", &id], None),
        }
    }
}

fn to_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string_pretty(value)
        .unwrap_or_else(|err| format!("{{\"error\": \"failed to serialize response: {err}\"}}"))
}

/// Single line on purpose: some MCP clients only show the first line, or a truncated prefix,
/// of a server's instructions. The critical rule therefore comes first.
const INSTRUCTIONS: &str = "Run long-running fire commands (servers, `start`/`dev`/`watch` commands, \
workers, tunnels) with the `start_session` tool, never in your own shell, and call `list_sessions` \
first to reuse what is already running; find the user's configured commands with \
`search_commands` (an app or service name you don't recognize is probably one) before writing your own. Details: these MCP tools beat running fire ad hoc. \
Commands the user marked `agents: false` are not listed and are off-limits: never run them, not \
even through your own shell. \
Sessions are shared with the user and other agents across projects, so give `start_session` a \
short `note` saying why; read output with `session_output`, write stdin with `send_input`, \
signal or stop with `signal_session`; the user can attach with `fire cli fg <id>`. \
`search_commands`/`list_commands` return each command's exact `path` (e.g. `fire backend test`); \
run short ones in your shell as `fire <path> [args]`. `exec`/`eval` are context only. \
https://github.com/gbenm/fire";

#[tool_handler(router = self.tool_router)]
impl ServerHandler for FireMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(INSTRUCTIONS)
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
    fn instructions_are_one_line_with_critical_rule_first() {
        assert!(!INSTRUCTIONS.contains('\n'));
        assert!(INSTRUCTIONS[..120].contains("start_session"));
    }

    #[test]
    fn agents_false_hides_command_and_its_subcommands() {
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
  build: cargo build
  deploy:
    agents: false
    commands:
      prod: ./deploy prod
  db:
    commands:
      migrate: ./migrate
      drop:
        agents: false
        exec: ./drop
"#,
                ),
            }],
        };

        let context = build_command_context(&config);
        let names: Vec<_> = context
            .commands
            .iter()
            .map(|node| node.name.as_str())
            .collect();
        assert_eq!(names, vec!["build", "db"]);
        let db = &context.commands[1];
        let subs: Vec<_> = db
            .subcommands
            .iter()
            .map(|node| node.name.as_str())
            .collect();
        assert_eq!(subs, vec!["migrate"]);
        assert!(search_context(&context, "deploy").is_empty());
        assert!(search_context(&context, "drop").is_empty());
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
        assert_eq!(
            computed.eval,
            Some(vec!["py:sayHello(\"{1}\")".to_string()])
        );
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
