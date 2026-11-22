use std::{
    io::{ErrorKind, Read},
    os::{fd::AsRawFd, unix::process::CommandExt},
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde::Serialize;

use rmcp::{
    handler::server::{
        router::tool::ToolRouter,
        wrapper::{Json, Parameters},
    },
    model::{ServerCapabilities, ServerInfo},
    schemars, serve_server, tool, tool_handler, tool_router,
    transport::io::stdio,
    ServerHandler,
};

use crate::config::{CommandEntry, FileScope, LoadedConfig, SourceKind};

#[derive(Debug, Serialize)]
pub(crate) struct CommandContext {
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
    #[serde(skip)]
    command: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct CommandMatch {
    /// The canonical invocation to run in a shell.
    pub(crate) path: String,
    pub(crate) description: Option<String>,
    pub(crate) runnable: bool,
    /// Whether the command came from the current project or a registered global directory.
    pub(crate) source: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) placeholder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) exec: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) eval: Option<Vec<String>>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) long_running: bool,
}

pub(crate) fn build_command_context(config: &LoadedConfig) -> CommandContext {
    build_command_context_with_restricted(config, false)
}

fn build_command_context_with_restricted(
    config: &LoadedConfig,
    include_restricted: bool,
) -> CommandContext {
    let mut commands = Vec::new();

    for file in &config.files {
        let source = match file.source {
            SourceKind::Local => "local",
            SourceKind::Global => "global",
        };
        let (namespace, group) = scope_labels(&file.scope);
        let scope_prefix: Vec<String> = namespace.iter().chain(group.iter()).cloned().collect();

        for (name, entry) in &file.commands {
            if !include_restricted && !entry.allows_agents() {
                continue;
            }
            let mut path_tokens = scope_prefix.clone();
            path_tokens.push(name.clone());
            commands.push(build_node(
                &path_tokens,
                entry,
                namespace.as_deref(),
                group.as_deref(),
                source,
                None,
                include_restricted,
            ));
        }
    }

    CommandContext { commands }
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
    entry: &CommandEntry,
    namespace: Option<&str>,
    group: Option<&str>,
    source: &'static str,
    inherited_long_running: Option<bool>,
    include_restricted: bool,
) -> CommandNode {
    let long_running = entry
        .spec()
        .and_then(|spec| spec.long_running)
        .or(inherited_long_running);
    let subcommands = entry
        .subcommands()
        .map(|map| {
            map.iter()
                .filter(|(_, sub_entry)| include_restricted || sub_entry.allows_agents())
                .map(|(sub_name, sub_entry)| {
                    let mut sub_path = path_tokens.to_vec();
                    sub_path.push(sub_name.clone());
                    build_node(
                        &sub_path,
                        sub_entry,
                        namespace,
                        group,
                        source,
                        long_running,
                        include_restricted,
                    )
                })
                .collect()
        })
        .unwrap_or_default();

    CommandNode {
        path: format!("fire {}", path_tokens.join(" ")),
        name: path_tokens.last().cloned().unwrap_or_default(),
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
        command: path_tokens.to_vec(),
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

fn search_context(context: &CommandContext, query: &str, include_exec: bool) -> Vec<CommandMatch> {
    let mut out = Vec::new();
    let needle = query.to_lowercase();
    for node in &context.commands {
        collect_matches(node, &needle, include_exec, &mut out);
    }
    out
}

fn collect_matches(
    node: &CommandNode,
    needle: &str,
    include_exec: bool,
    out: &mut Vec<CommandMatch>,
) {
    if node_matches(node, needle) {
        out.push(CommandMatch {
            path: node.path.clone(),
            description: node.description.clone(),
            runnable: node.runnable,
            source: node.source,
            placeholder: node.placeholder.clone(),
            exec: include_exec.then(|| node.exec.clone()).flatten(),
            eval: include_exec.then(|| node.eval.clone()).flatten(),
            long_running: node.long_running,
        });
    }
    for child in &node.subcommands {
        collect_matches(child, needle, include_exec, out);
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
    /// Optional case-insensitive substring to match against command paths, descriptions, and
    /// exec/eval templates. Omit or pass an empty string to list every command.
    query: Option<String>,
    /// Include underlying exec/eval templates for context. Never run these directly; run `path`.
    include_exec: Option<bool>,
    /// Project directory whose local commands should be discovered. Defaults to the server's cwd.
    cwd: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct GetDocsRequest {
    /// Documentation topic to read. Omit it to return the full topic index.
    topic: Option<crate::docs::DocsTopic>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct RunCommandRequest {
    /// Exact command path returned by `search_commands`, including the leading `fire`.
    path: String,
    /// Arguments appended as separate, unchanged tokens.
    args: Option<Vec<String>>,
    /// Project directory to resolve and run the command from. Defaults to the server's cwd.
    cwd: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct ListSessionsRequest {
    /// Project directory to select. Relative paths resolve from the MCP server's directory.
    cwd: Option<String>,
    /// Return sessions from every project. When true, `cwd` is ignored.
    all: Option<bool>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SessionOutputRequest {
    /// Session id, as returned by `list_sessions` or `fire --bg`.
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
struct SessionIdRequest {
    /// Session id.
    id: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct SignalSessionRequest {
    /// Session id.
    id: String,
    /// Signal name or number (INT, TERM, HUP, USR1, USR2, QUIT, KILL, STOP, CONT, 9...).
    signal: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct GlobalPathRequest {
    /// Absolute path of the directory (a leading `~` means the home directory).
    path: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct RemoveGlobalRequest {
    /// Namespace of the registered directory to remove (as shown by `list_global`).
    namespace: Option<String>,
    /// Absolute path of the registered directory to remove. Pass exactly one of `namespace`/`path`.
    path: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct NamespaceRequest {
    /// Namespace of a registered directory (as shown by `list_global`).
    namespace: String,
}

#[derive(Debug, Default, Serialize, schemars::JsonSchema)]
struct GlobalResult {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    /// Affected directory (absolute).
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    registered: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    already_registered: Option<bool>,
    /// `false` when the registered directory has no fire config files yet.
    #[serde(skip_serializing_if = "Option::is_none")]
    has_config: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    removed: Option<bool>,
    /// Directories `prune_global` unregistered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pruned: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    merged: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    already_merged: Option<bool>,
}

impl GlobalResult {
    fn err(error: String) -> Self {
        Self {
            error: Some(error),
            ..Self::default()
        }
    }
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct GlobalListing {
    directories: Vec<GlobalDirectory>,
    /// Namespaces whose commands can be called without the namespace word.
    merged: Vec<String>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct GlobalDirectory {
    path: String,
    exists: bool,
    namespaces: Vec<GlobalNamespace>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct GlobalNamespace {
    /// `null` for commands without a namespace (callable at the root).
    namespace: Option<String>,
    merged: bool,
    /// Absolute paths of the config files that define it; edit these to change its commands.
    files: Vec<String>,
}

/// MCP takes explicit absolute paths: relative ones would resolve against the server's own
/// directory, which the agent doesn't control.
fn absolute_path(path: &str) -> Result<PathBuf, String> {
    let expanded = match (path.strip_prefix('~'), dirs::home_dir()) {
        (Some(rest), Some(home)) if rest.is_empty() || rest.starts_with('/') => {
            home.join(rest.trim_start_matches('/'))
        }
        _ => PathBuf::from(path),
    };
    if expanded.is_absolute() {
        Ok(expanded)
    } else {
        Err(format!("`{path}` is not an absolute path"))
    }
}

fn global_listing() -> GlobalListing {
    let merged = crate::registry::load_merged_namespaces();
    let directories = crate::global::entries()
        .into_iter()
        .map(|entry| GlobalDirectory {
            path: entry.dir.display().to_string(),
            exists: entry.exists,
            namespaces: entry
                .scopes
                .iter()
                .map(|scope| GlobalNamespace {
                    merged: scope
                        .namespace
                        .as_ref()
                        .is_some_and(|name| merged.contains(name)),
                    namespace: scope.namespace.clone(),
                    files: scope
                        .files
                        .iter()
                        .map(|file| entry.dir.join(file).display().to_string())
                        .collect(),
                })
                .collect(),
        })
        .collect();
    GlobalListing {
        directories,
        merged,
    }
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct CliResult {
    ok: bool,
    stdout: String,
    stderr: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct StartSessionResult {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<String>,
    stdout: String,
    stderr: String,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct RunCommandResult {
    /// Process exit code, or null if the command was refused, could not start, or timed out.
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
    timed_out: bool,
    stdout_truncated: bool,
    stderr_truncated: bool,
}

const RUN_COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
const RUN_COMMAND_MAX_OUTPUT: usize = 64 * 1024;

impl RunCommandResult {
    fn rejected(message: impl Into<String>) -> Self {
        Self {
            exit_code: None,
            stdout: String::new(),
            stderr: message.into(),
            timed_out: false,
            stdout_truncated: false,
            stderr_truncated: false,
        }
    }
}

impl From<CliResult> for StartSessionResult {
    fn from(result: CliResult) -> Self {
        let session_id = result
            .ok
            .then(|| result.stdout.clone())
            .filter(|id| !id.is_empty());
        Self {
            ok: result.ok,
            session_id,
            stdout: result.stdout,
            stderr: result.stderr,
        }
    }
}

/// Input reuses the CLI so its terminal escape handling stays identical.
fn run_fire_cli(args: &[&str], cwd: Option<&str>) -> CliResult {
    let mut full = vec!["cli"];
    full.extend_from_slice(args);
    run_fire_result(&full, cwd)
}

fn run_fire_result(args: &[&str], cwd: Option<&str>) -> CliResult {
    let exe = std::env::current_exe().unwrap_or_else(|_| "fire".into());
    let mut command = std::process::Command::new(exe);
    command.args(args).stdin(std::process::Stdio::null());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    match command.output() {
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
    }
}

fn find_command_node<'a>(context: &'a CommandContext, path: &str) -> Option<&'a CommandNode> {
    fn find<'a>(nodes: &'a [CommandNode], path: &str) -> Option<&'a CommandNode> {
        for node in nodes {
            if node.path == path {
                return Some(node);
            }
            if let Some(found) = find(&node.subcommands, path) {
                return Some(found);
            }
        }
        None
    }

    find(&context.commands, path)
}

fn run_command_process(
    command_args: &[String],
    cwd: Option<&std::path::Path>,
    timeout: Duration,
) -> RunCommandResult {
    let exe = std::env::current_exe().unwrap_or_else(|_| "fire".into());
    let mut command = Command::new(exe);
    // `--fg` also exports sessions=off to nested fire calls, so a supposedly finite command
    // cannot leak a detached session if its config changes after the policy check.
    command.args(["--agent", "--fg"]).args(command_args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    capture_process(command, timeout)
}

fn requested_cwd(cwd: Option<&str>) -> std::path::PathBuf {
    let cwd = cwd
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| ".".into()));
    let cwd = if cwd.is_absolute() {
        cwd
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| ".".into())
            .join(cwd)
    };
    cwd.canonicalize().unwrap_or(cwd)
}

fn capture_process(mut command: Command, timeout: Duration) -> RunCommandResult {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Fire commands commonly launch a shell. A separate process group lets a timeout stop the
    // whole command tree and close its output pipes, rather than orphaning the shell's children.
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => return RunCommandResult::rejected(format!("failed to run fire: {err}")),
    };
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    if let Err(err) = set_nonblocking(&stdout).and_then(|_| set_nonblocking(&stderr)) {
        stop_process_group(&mut child);
        return RunCommandResult::rejected(format!("failed to capture fire output: {err}"));
    }
    let mut captured_stdout = BoundedOutput::new(RUN_COMMAND_MAX_OUTPUT);
    let mut captured_stderr = BoundedOutput::new(RUN_COMMAND_MAX_OUTPUT);
    let mut stdout_open = true;
    let mut stderr_open = true;

    let started = Instant::now();
    let mut status = None;
    let mut timed_out = false;
    loop {
        if stdout_open {
            stdout_open = captured_stdout.read_available(&mut stdout);
        }
        if stderr_open {
            stderr_open = captured_stderr.read_available(&mut stderr);
        }

        if status.is_none() {
            match child.try_wait() {
                Ok(next) => status = next,
                Err(err) => {
                    stop_process_group(&mut child);
                    captured_stderr.push(format!("failed to wait for fire: {err}").as_bytes());
                    break;
                }
            }
        }
        if status.is_some() && !stdout_open && !stderr_open {
            break;
        }
        if started.elapsed() >= timeout {
            timed_out = true;
            stop_process_group(&mut child);
            captured_stderr.push(
                format!(
                    "[fire] command timed out after {} seconds",
                    timeout.as_secs()
                )
                .as_bytes(),
            );
            // A descendant may have detached into another process group while retaining an output
            // pipe. Drain bytes already available, but never let that escaped process extend this
            // tool's timeout.
            if stdout_open {
                captured_stdout.read_available(&mut stdout);
            }
            if stderr_open {
                captured_stderr.read_available(&mut stderr);
            }
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }

    let (stdout, stdout_truncated) = captured_stdout.finish();
    let (stderr, stderr_truncated) = captured_stderr.finish();
    RunCommandResult {
        exit_code: (!timed_out)
            .then(|| status.and_then(|status| status.code()))
            .flatten(),
        stdout,
        stderr,
        timed_out,
        stdout_truncated,
        stderr_truncated,
    }
}

fn set_nonblocking(pipe: &impl AsRawFd) -> std::io::Result<()> {
    let fd = pipe.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn stop_process_group(child: &mut std::process::Child) {
    let process_group = -(child.id() as i32);
    if unsafe { libc::kill(process_group, libc::SIGKILL) } == -1 {
        let _ = child.kill();
    }
    let _ = child.wait();
}

struct BoundedOutput {
    bytes: Vec<u8>,
    limit: usize,
    truncated: bool,
}

impl BoundedOutput {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
            truncated: false,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        let available = self.limit.saturating_sub(self.bytes.len());
        self.bytes
            .extend_from_slice(&bytes[..bytes.len().min(available)]);
        self.truncated |= bytes.len() > available;
    }

    /// Drains a bounded amount so a process that writes continuously cannot hide the deadline.
    /// Returns whether the pipe is still open.
    fn read_available(&mut self, reader: &mut impl Read) -> bool {
        let mut chunk = [0_u8; 8192];
        for _ in 0..8 {
            match reader.read(&mut chunk) {
                Ok(0) => return false,
                Ok(read) => self.push(&chunk[..read]),
                Err(err) if err.kind() == ErrorKind::WouldBlock => return true,
                Err(_) => return false,
            }
        }
        true
    }

    fn finish(self) -> (String, bool) {
        let mut text = String::from_utf8_lossy(&self.bytes).into_owned();
        let truncated = self.truncated || text.len() > self.limit;
        if text.len() > self.limit {
            let mut end = self.limit;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
        }
        (text, truncated)
    }
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
        let mut tool_router = Self::tool_router();
        tool_router
            .map
            .get_mut("get_docs")
            .expect("get_docs tool route")
            .attr
            .description = Some(crate::docs::MCP_TOOL_DESCRIPTION.into());
        Self { tool_router }
    }
}

#[tool_router]
impl FireMcpServer {
    #[tool(description = "Read Fire's version-matched embedded documentation.")]
    fn get_docs(&self, Parameters(GetDocsRequest { topic }): Parameters<GetDocsRequest>) -> String {
        match topic {
            Some(topic) => crate::docs::find(topic.id())
                .map(|doc| doc.content.to_string())
                .unwrap_or_else(|| {
                    "Documentation topic is not embedded in this build.".to_string()
                }),
            None => crate::docs::index(),
        }
    }

    #[tool(
        description = "Discover the user's configured `fire` commands. Pass your working directory as `cwd` to see that project's local commands; omit it to use the server's directory. `path` is the exact command to run. Omit `query` or pass an empty string to list everything; otherwise it is a case-insensitive substring over paths, descriptions, and exec/eval templates. Underlying templates are hidden by default; request `include_exec` only for context, and never copy or run them directly. Use this first when the user names an app, service or task you don't recognize, and before writing your own build/test/run/deploy commands."
    )]
    fn search_commands(
        &self,
        Parameters(SearchCommandsRequest {
            query,
            include_exec,
            cwd,
        }): Parameters<SearchCommandsRequest>,
    ) -> String {
        let config = crate::config::load_config_from(&requested_cwd(cwd.as_deref()));
        let context = build_command_context(&config);
        to_json(&search_context(
            &context,
            query.as_deref().unwrap_or_default(),
            include_exec.unwrap_or(false),
        ))
    }

    #[tool(
        description = "Run a finite configured command to completion. Pass the exact `path` from `search_commands`, the same `cwd` used for discovery, and optional `args` as separate unchanged tokens. `cwd` defaults to the server's directory. Returns its exit code and separate stdout/stderr, capped at 64 KiB each. Commands have a fixed 60-second timeout. Long-running commands are refused with a hint to use `start_session`; commands marked `agents: false` are also refused."
    )]
    fn run_command(
        &self,
        Parameters(RunCommandRequest { path, args, cwd }): Parameters<RunCommandRequest>,
    ) -> Json<RunCommandResult> {
        let cwd = requested_cwd(cwd.as_deref());
        let config = crate::config::load_config_from(&cwd);
        let context = build_command_context_with_restricted(&config, true);
        let Some(node) = find_command_node(&context, &path) else {
            return Json(RunCommandResult::rejected(format!(
                "unknown configured command path `{path}`; use search_commands and pass its exact path"
            )));
        };
        let Some(resolved) = crate::resolve::resolve_command(&config, &node.command) else {
            return Json(RunCommandResult::rejected(format!(
                "unknown configured command path `{path}`; use search_commands and pass its exact path"
            )));
        };
        if !resolved
            .command_chain
            .iter()
            .all(|entry| entry.allows_agents())
        {
            return Json(RunCommandResult::rejected(format!(
                "{path} is not available to agents (the user set `agents: false`); do not run it any other way either, ask the user to run it"
            )));
        }
        if !resolved.command.is_runnable() {
            return Json(RunCommandResult::rejected(format!(
                "{path} has no executable action; choose a runnable subcommand from search_commands"
            )));
        }
        let long_running = resolved
            .command_chain
            .iter()
            .rev()
            .find_map(|entry| entry.spec().and_then(|spec| spec.long_running))
            == Some(true);
        if long_running {
            return Json(RunCommandResult::rejected(format!(
                "{path} is marked `long_running: true`; use start_session instead"
            )));
        }

        let mut command = node.command.clone();
        command.extend(args.unwrap_or_default());
        Json(run_command_process(
            &command,
            Some(&cwd),
            RUN_COMMAND_TIMEOUT,
        ))
    }

    #[tool(
        description = "List detached `fire` sessions for the current project by default, including other directories that resolve to it (started with `fire --bg` or `start_session`, by any agent or the user): id, command, note (why it was started), cwd, status (`running`, `exited`, `stopped` = stopped on purpose, `lost`), pid and exit code. Pass `cwd` to select another project directory, or `all: true` to list every project (`cwd` is then ignored). Check this before starting or restarting servers to avoid duplicates. If a previous session for the same command exited, read its `session_output` first: it usually shows why it failed (e.g. port in use)."
    )]
    fn list_sessions(
        &self,
        Parameters(ListSessionsRequest { cwd, all }): Parameters<ListSessionsRequest>,
    ) -> String {
        let project_root = if all == Some(true) {
            None
        } else {
            Some(crate::config::local_project_root_from(&requested_cwd(
                cwd.as_deref(),
            )))
        };
        let sessions: Vec<_> = crate::session::list_sessions()
            .into_iter()
            .filter(|session| {
                project_root.as_ref().is_none_or(|project_root| {
                    crate::config::local_project_root_from(std::path::Path::new(&session.cwd))
                        == *project_root
                })
            })
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
        description = "Start a configured `fire` command detached, in a managed session. ALWAYS use this for long-running processes (dev servers, watchers, workers, tunnels, log tails) instead of backgrounding them in your own shell: the process survives your shell, and you, other agents and the user can list it, read its output, send input/signals, or stop it. `command` must be a configured fire command (find it with `search_commands`). One command in one cwd is one session: if it is already running, this returns its id and replaces its note instead of starting a duplicate; if it ended (crashed or stopped), it starts again under the same id with the new note. So to restart a server, stop it with `stop_session` and call this again. Returns the session id as `session_id`; then confirm it came up with `session_output`. Fails for commands the user reserved for humans (`agents: false`)."
    )]
    fn start_session(
        &self,
        Parameters(StartSessionRequest { command, note, cwd }): Parameters<StartSessionRequest>,
    ) -> Json<StartSessionResult> {
        let mut args = vec!["--bg", "--agent", "-m", note.as_str()];
        args.extend(command.iter().map(String::as_str));
        Json(StartSessionResult::from(run_fire_result(
            &args,
            cwd.as_deref(),
        )))
    }

    #[tool(
        description = "Type into a running session's terminal (it runs on a pty): a line of text followed by Enter, or with `raw: true` exact keys such as `q`, `\\x03` (Ctrl+C) or `\\e[A` (up arrow)."
    )]
    fn send_input(
        &self,
        Parameters(SendInputRequest { id, text, raw }): Parameters<SendInputRequest>,
    ) -> Json<CliResult> {
        Json(if raw == Some(true) {
            run_fire_cli(&["send", "--raw", &id, &text], None)
        } else {
            run_fire_cli(&["send", &id, &text], None)
        })
    }

    #[tool(
        description = "Stop a running session gracefully with TERM, then KILL after 5 seconds if needed."
    )]
    fn stop_session(
        &self,
        Parameters(SessionIdRequest { id }): Parameters<SessionIdRequest>,
    ) -> Json<CliResult> {
        Json(session_action_result(crate::session::stop_session(&id)))
    }

    #[tool(
        description = "Send a specific signal to a running session's process group (e.g. INT, HUP, USR1)."
    )]
    fn signal_session(
        &self,
        Parameters(SignalSessionRequest { id, signal }): Parameters<SignalSessionRequest>,
    ) -> Json<CliResult> {
        Json(session_action_result(crate::session::signal_session(
            &id, &signal,
        )))
    }

    #[tool(
        description = "List the directories registered globally (their commands load from anywhere), grouped by namespace, with the absolute paths of the config files that define each namespace (open those to edit a command) and which namespaces are merged. Read-only."
    )]
    fn list_global(&self) -> Json<GlobalListing> {
        Json(global_listing())
    }

    #[tool(
        description = "Register a directory globally so its commands load from anywhere (same as `fire cli global add`). Changes the user's global setup: use only when the user asks for it, never on your own initiative."
    )]
    fn add_global(
        &self,
        Parameters(GlobalPathRequest { path }): Parameters<GlobalPathRequest>,
    ) -> Json<GlobalResult> {
        Json(
            match absolute_path(&path).and_then(|path| crate::global::add_directory(&path)) {
                Ok(added) => GlobalResult {
                    ok: true,
                    path: Some(added.dir.display().to_string()),
                    registered: Some(added.newly),
                    already_registered: Some(!added.newly),
                    has_config: Some(added.has_config),
                    ..GlobalResult::default()
                },
                Err(err) => GlobalResult::err(err),
            },
        )
    }

    #[tool(
        description = "Unregister a global directory by `namespace` or absolute `path` (same as `fire cli global remove`); its files are never touched. Changes the user's global setup: use only when the user asks for it, never on your own initiative."
    )]
    fn remove_global(
        &self,
        Parameters(RemoveGlobalRequest { namespace, path }): Parameters<RemoveGlobalRequest>,
    ) -> Json<GlobalResult> {
        let target = match (namespace, path) {
            (Some(namespace), None) => Ok(crate::global::Target::Namespace(namespace)),
            (None, Some(path)) => absolute_path(&path).map(crate::global::Target::Path),
            _ => Err("pass exactly one of `namespace` or `path`".to_string()),
        };
        Json(match target.and_then(crate::global::remove_directory) {
            Ok(dir) => GlobalResult {
                ok: true,
                path: Some(dir.display().to_string()),
                removed: Some(true),
                ..GlobalResult::default()
            },
            Err(err) => GlobalResult::err(err),
        })
    }

    #[tool(
        description = "Unregister global directories that no longer exist (same as `fire cli global prune`). Changes the user's global setup: use only when the user asks for it, never on your own initiative."
    )]
    fn prune_global(&self) -> Json<GlobalResult> {
        Json(match crate::global::prune_directories() {
            Ok(pruned) => GlobalResult {
                ok: true,
                pruned: Some(
                    pruned
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect(),
                ),
                ..GlobalResult::default()
            },
            Err(err) => GlobalResult::err(err),
        })
    }

    #[tool(
        description = "Merge a registered namespace so its commands can be called without the namespace word (same as `fire cli global merge`); explicit paths of other commands still win. Changes the user's global setup: use only when the user asks for it, never on your own initiative."
    )]
    fn merge_global(
        &self,
        Parameters(NamespaceRequest { namespace }): Parameters<NamespaceRequest>,
    ) -> Json<GlobalResult> {
        Json(match crate::global::merge_namespace(&namespace) {
            Ok(newly) => GlobalResult {
                ok: true,
                merged: Some(true),
                already_merged: Some(!newly),
                ..GlobalResult::default()
            },
            Err(err) => GlobalResult::err(err),
        })
    }

    #[tool(
        description = "Unmerge a namespace so its commands need the namespace word again (same as `fire cli global unmerge`). Changes the user's global setup: use only when the user asks for it, never on your own initiative."
    )]
    fn unmerge_global(
        &self,
        Parameters(NamespaceRequest { namespace }): Parameters<NamespaceRequest>,
    ) -> Json<GlobalResult> {
        Json(match crate::global::unmerge_namespace(&namespace) {
            Ok(()) => GlobalResult {
                ok: true,
                merged: Some(false),
                ..GlobalResult::default()
            },
            Err(err) => GlobalResult::err(err),
        })
    }

    #[tool(description = "Remove the saved record of an exited, stopped, or lost session.")]
    fn forget_session(
        &self,
        Parameters(SessionIdRequest { id }): Parameters<SessionIdRequest>,
    ) -> Json<CliResult> {
        Json(session_action_result(crate::session::forget_session(&id)))
    }
}

fn session_action_result(result: Result<String, String>) -> CliResult {
    match result {
        Ok(stdout) => CliResult {
            ok: true,
            stdout,
            stderr: String::new(),
        },
        Err(stderr) => CliResult {
            ok: false,
            stdout: String::new(),
            stderr,
        },
    }
}

fn to_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string_pretty(value)
        .unwrap_or_else(|err| format!("{{\"error\": \"failed to serialize response: {err}\"}}"))
}

/// Single line on purpose: some MCP clients only show the first line, or a truncated prefix,
/// of a server's instructions. The critical rule therefore comes first.
const INSTRUCTIONS: &str = "Long-running fire commands (dev servers, `start`/`dev`/`watch`, workers, \
tunnels) go through `start_session`, never your own shell. Decide in this order: \
1) find the command with `search_commands`, passing your working directory as `cwd` (an app or \
service name you don't recognize is probably one), before writing your own; \
2) check `list_sessions` (this project; `all: true` for every project) and reuse what is already running; \
3) long-running (`long_running: true`, or it doesn't exit on its own): `start_session` with a short \
`note` saying why, then confirm it came up with `session_output`; finite (build, test, lint, one-off \
scripts): `run_command`, with arguments as separate tokens. \
Sessions are shared with the user and other agents: read output with `session_output`, type with \
`send_input`, signal with `signal_session`, stop (or stop then start to restart) with `stop_session`, \
remove an exited record with `forget_session`. \
Never run commands the user marked `agents: false`, not even through your own shell. \
To write or edit fire configs, use `get_docs`. Change global directories (`*_global` tools) only \
when the user asks. https://github.com/gbenm/fire";

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
        assert!(INSTRUCTIONS.contains("fire configs, use `get_docs`"));
    }

    #[test]
    fn instructions_are_a_decision_guide_without_user_hints() {
        let position = |needle: &str| {
            INSTRUCTIONS
                .find(needle)
                .unwrap_or_else(|| panic!("instructions mention {needle}"))
        };
        // find → already running? → long-running or finite.
        assert!(position("1) find") < position("2) check `list_sessions`"));
        assert!(position("2) check") < position("3) long-running"));
        assert!(position("3) long-running") < position("`run_command`"));
        position("`stop_session`");
        // One rule for the shell, and no hints meant for the human user.
        assert!(!INSTRUCTIONS.contains("ad hoc"));
        assert!(!INSTRUCTIONS.contains("in your shell as"));
        assert!(!INSTRUCTIONS.contains("fire cli"));
    }

    #[test]
    fn finite_command_capture_times_out_and_truncates_output() {
        let mut command = Command::new("sh");
        command.args(["-c", "printf 123456; sleep 5 &"]);
        let started = Instant::now();
        let result = capture_process(command, Duration::from_millis(30));

        assert!(result.timed_out);
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(result.exit_code, None);
        assert_eq!(result.stdout, "123456");
        assert!(result.stderr.contains("timed out"));

        let mut captured = BoundedOutput::new(4);
        captured.push(b"abcdef");
        let (output, truncated) = captured.finish();
        assert_eq!(output, "abcd");
        assert!(truncated);
    }

    #[test]
    fn start_session_result_exposes_session_id() {
        let result = StartSessionResult::from(CliResult {
            ok: true,
            stdout: "api-dev-a1b2c3".to_string(),
            stderr: String::new(),
        });

        assert_eq!(result.session_id.as_deref(), Some("api-dev-a1b2c3"));
        assert_eq!(result.stdout, "api-dev-a1b2c3");
        assert_eq!(
            serde_json::to_value(&result).expect("serializable result")["session_id"],
            "api-dev-a1b2c3"
        );

        let router = FireMcpServer::tool_router();
        let tool = router.get("start_session").expect("start tool");
        let output = tool
            .output_schema
            .as_ref()
            .expect("structured output schema");
        assert!(output["properties"].get("session_id").is_some());
    }

    #[test]
    fn failed_start_session_has_no_session_id() {
        let result = StartSessionResult::from(CliResult {
            ok: false,
            stdout: String::new(),
            stderr: "unknown command".to_string(),
        });

        assert_eq!(result.session_id, None);
        assert_eq!(result.stderr, "unknown command");
    }

    #[test]
    fn session_lifecycle_tools_have_single_purpose_contracts() {
        let router = FireMcpServer::tool_router();
        for name in ["stop_session", "signal_session", "forget_session"] {
            assert!(router.has_route(name), "missing `{name}` tool");
        }

        let signal = router.get("signal_session").expect("signal tool");
        let required = signal
            .input_schema
            .get("required")
            .and_then(|value| value.as_array())
            .expect("required fields");
        assert!(required.iter().any(|field| field == "signal"));

        let stop_description = router
            .get("stop_session")
            .and_then(|tool| tool.description.as_deref())
            .expect("stop description");
        assert!(stop_description.contains("running session"));
        assert!(!stop_description.contains("signal"));
        assert!(!stop_description.contains("remove"));

        let signal_description = signal.description.as_deref().expect("signal description");
        assert!(signal_description.contains("specific signal"));
        assert!(!signal_description.contains("stop it"));
        assert!(!signal_description.contains("remove"));

        let forget_description = router
            .get("forget_session")
            .and_then(|tool| tool.description.as_deref())
            .expect("forget description");
        assert!(forget_description.contains("exited"));
        assert!(forget_description.contains("stopped"));
        assert!(!forget_description.contains("running session"));
        assert!(!forget_description.contains("signal"));
    }

    #[test]
    fn stop_session_does_not_forget_an_exited_or_missing_session() {
        let result = FireMcpServer::new().stop_session(Parameters(SessionIdRequest {
            id: "definitely-not-a-session".to_string(),
        }));
        let result = serde_json::to_value(result.0).expect("JSON result");

        assert_eq!(result["ok"], false);
        assert_eq!(
            result["stderr"],
            "No session `definitely-not-a-session`. See: fire cli ps"
        );
    }

    #[test]
    fn agents_false_hides_command_and_its_subcommands() {
        let config = LoadedConfig {
            merged_namespaces: Default::default(),
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
        assert!(search_context(&context, "deploy", false).is_empty());
        assert!(search_context(&context, "drop", false).is_empty());
    }

    #[test]
    fn root_scope_produces_bare_path() {
        let config = LoadedConfig {
            merged_namespaces: Default::default(),
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
            merged_namespaces: Default::default(),
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
            merged_namespaces: Default::default(),
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
            merged_namespaces: Default::default(),
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
            merged_namespaces: Default::default(),
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
        let matches = search_context(&context, "backend", false);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].path, "fire run");

        let matches = search_context(&context, "npm", false);
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].path, "fire run");
    }

    #[test]
    fn search_is_the_only_command_discovery_tool() {
        let router = FireMcpServer::tool_router();
        assert!(router.has_route("search_commands"));
        assert!(!router.has_route("list_commands"));

        let search = router.get("search_commands").expect("search tool");
        let required = search
            .input_schema
            .get("required")
            .and_then(|value| value.as_array());
        assert!(required.is_none_or(Vec::is_empty), "query must be optional");
        assert!(search.input_schema["properties"]
            .get("include_exec")
            .is_some());
        let description = search.description.as_deref().expect("search description");
        assert!(description.contains("`path` is the exact command to run"));
    }

    #[test]
    fn search_lists_everything_and_hides_templates_by_default() {
        let config = LoadedConfig {
            merged_namespaces: Default::default(),
            files: vec![FileConfig {
                source: SourceKind::Local,
                project_dir: PathBuf::from("."),
                config_path: PathBuf::from("/tmp/fire-test.yml"),
                scope: FileScope::Root,
                runtimes: BTreeMap::new(),
                commands: parse_commands(
                    r#"
commands:
  build:
    description: Build everything
    placeholder: "@{n}"
    exec: cargo build
  generated:
    eval: py:commands()
"#,
                ),
            }],
        };
        let context = build_command_context(&config);

        let matches = search_context(&context, "", false);
        assert_eq!(matches.len(), 2);
        let json = serde_json::to_value(&matches).expect("serializable matches");
        assert_eq!(json[0]["source"], "local");
        assert_eq!(json[0]["placeholder"], "@{n}");
        for command in json.as_array().expect("command array") {
            assert!(command.get("exec").is_none());
            assert!(command.get("eval").is_none());
        }

        let matches = search_context(&context, "", true);
        let json = serde_json::to_value(&matches).expect("serializable matches");
        assert_eq!(json[0]["exec"][0], "cargo build");
        assert_eq!(json[1]["eval"][0], "py:commands()");
    }
}
