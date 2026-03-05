use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{BufRead, BufReader, IsTerminal, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde::Deserialize;

use crate::config::RuntimeConfig;
use crate::resolve::ResolvedCommand;

pub(crate) fn execute_resolved_command(resolved: ResolvedCommand<'_>) -> ! {
    let context = build_execution_context(&resolved);
    ensure_working_directory(&context.dir);

    let original_args = resolved.remaining_args.to_vec();
    let compute = compute_values(&resolved, &context, &original_args);
    let computed_values = &compute.values;

    if let Some(raw_evals) = resolved.command.evaluation_expressions() {
        run_evals_with_runtime(
            &resolved,
            &context,
            &raw_evals,
            &original_args,
            computed_values,
        );
    }

    let Some(raw_commands_to_run) = resolved.command.execution_commands() else {
        eprintln!("[fire] Command path has no executable action.");
        if let Some(subcommands) = resolved.command.subcommands() {
            eprintln!("Commands:");
            let width = subcommands
                .keys()
                .map(|name| name.len())
                .max()
                .unwrap_or(0)
                .max(1);
            for (name, entry) in subcommands {
                let description = entry.description().unwrap_or_default();
                if description.is_empty() {
                    eprintln!("  {name}");
                } else {
                    let short = description.lines().next().unwrap_or("").trim();
                    eprintln!("  {:width$}  {}", name, short, width = width);
                }
            }
        }
        process::exit(1);
    };

    let mut ignored_stats = RenderStats::default();
    let rendered_check = context.check.as_deref().map(|value| {
        render_runtime_string(
            value,
            &context,
            &original_args,
            computed_values,
            false,
            RenderMode::Shell,
            &mut ignored_stats,
        )
    });
    let rendered_runner = context.runner.as_deref().map(|value| {
        render_runtime_string(
            value,
            &context,
            &original_args,
            computed_values,
            false,
            RenderMode::Shell,
            &mut ignored_stats,
        )
    });
    let rendered_fallback_runner = context.fallback_runner.as_deref().map(|value| {
        render_runtime_string(
            value,
            &context,
            &original_args,
            computed_values,
            false,
            RenderMode::Shell,
            &mut ignored_stats,
        )
    });

    let selected_runner = select_runner_mode(
        &context.dir,
        rendered_check.as_deref(),
        rendered_runner.as_deref(),
        rendered_fallback_runner.as_deref(),
    );

    if should_execute_before(&selected_runner) {
        if let Some(before) = context.before.as_deref() {
            let rendered_before = render_runtime_string(
                before,
                &context,
                &original_args,
                computed_values,
                false,
                RenderMode::Shell,
                &mut ignored_stats,
            );
            let status = run_shell_command(&rendered_before, &context.dir);
            let code = status.code().unwrap_or(1);
            if code != 0 {
                process::exit(code);
            }
        }
    }

    let mut render_stats = compute.stats;
    let rendered_commands_to_run = raw_commands_to_run
        .iter()
        .map(|command| {
            render_runtime_string(
                command,
                &context,
                &original_args,
                computed_values,
                true,
                RenderMode::Shell,
                &mut render_stats,
            )
        })
        .collect::<Vec<_>>();

    let tail_args = unresolved_args_for_tail(&context, &original_args, &render_stats);
    let commands_to_run = commands_with_remaining_args(&rendered_commands_to_run, &tail_args);

    let exit_code = match selected_runner {
        RunnerMode::Runner(runner) | RunnerMode::Fallback(runner) => {
            run_with_runner(&runner, &context.dir, &commands_to_run)
        }
        RunnerMode::Direct => run_in_single_shell(&context.dir, &commands_to_run),
    };

    process::exit(exit_code);
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RuntimeSdk {
    Node,
    Deno,
    Python,
}

#[derive(Debug, Clone)]
struct RuntimeDefinition {
    sdk: RuntimeSdk,
    runner: String,
    check: Option<String>,
    fallback_runner: Option<String>,
    paths: Vec<String>,
}

struct RuntimeEngine {
    session: RuntimeSession,
    next_id: usize,
}

struct RuntimeSession {
    child: Child,
    writer: TcpStream,
    reader: BufReader<TcpStream>,
}

struct RuntimeRequest<'a> {
    op: &'a str,
    id: String,
    path: Option<&'a str>,
    code: Option<&'a str>,
}

#[derive(Debug, Clone)]
enum RuntimeEvalResult {
    Text(String),
    Commands(Vec<String>),
}

#[derive(Debug, Default, Clone)]
struct RuntimeResponse {
    output_lines: Vec<String>,
    eval_result: Option<RuntimeEvalResult>,
}

#[derive(Debug, Default, Clone)]
struct ComputeResult {
    values: BTreeMap<String, String>,
    stats: RenderStats,
}

fn run_evals_with_runtime(
    resolved: &ResolvedCommand<'_>,
    context: &ExecutionContext,
    raw_evals: &[String],
    args: &[String],
    compute_values: &BTreeMap<String, String>,
) -> ! {
    let mut render_stats = RenderStats::default();
    let mut parsed = Vec::new();

    for raw_eval in raw_evals {
        let rendered = render_runtime_string(
            raw_eval,
            context,
            args,
            compute_values,
            true,
            RenderMode::Eval,
            &mut render_stats,
        );
        let (runtime_key, code) = split_runtime_eval(&rendered);
        parsed.push((runtime_key.to_string(), code.to_string()));
    }

    enforce_unused_args_policy(context, args, &render_stats);

    let mut engines: BTreeMap<String, RuntimeEngine> = BTreeMap::new();
    let mut commands_to_run = Vec::new();
    for (runtime_key, code) in &parsed {
        if !engines.contains_key(runtime_key) {
            let engine =
                start_runtime_engine_for_key(runtime_key, resolved, context, args, compute_values);
            engines.insert(runtime_key.clone(), engine);
        }

        let engine = engines.get_mut(runtime_key).unwrap_or_else(|| {
            eprintln!("[fire] Runtime engine `{runtime_key}` not available.");
            process::exit(1);
        });
        log_runtime(&format!("{runtime_key} eval: {code}"));
        let runtime_commands = engine.eval(code);
        if !runtime_commands.is_empty() {
            log_runtime(&format!(
                "{runtime_key} emitted {} command(s)",
                runtime_commands.len()
            ));
        }
        commands_to_run.extend(runtime_commands);
    }

    if !commands_to_run.is_empty() {
        let code = run_in_single_shell(&context.dir, &commands_to_run);
        if code != 0 {
            for (_, mut engine) in engines {
                engine.close();
            }
            process::exit(code);
        }
    }

    for (_, mut engine) in engines {
        engine.close();
    }

    process::exit(0);
}

fn start_runtime_engine_for_key(
    runtime_key: &str,
    resolved: &ResolvedCommand<'_>,
    context: &ExecutionContext,
    args: &[String],
    computed_values: &BTreeMap<String, String>,
) -> RuntimeEngine {
    let runtime_config = resolved.runtimes.get(runtime_key).unwrap_or_else(|| {
        eprintln!("[fire] Runtime `{runtime_key}` is not defined in `runtimes`.");
        process::exit(1);
    });
    let runtime = resolve_runtime_definition(runtime_key, runtime_config);

    let mut ignored_stats = RenderStats::default();
    let rendered_check = runtime.check.as_deref().map(|value| {
        render_runtime_string(
            value,
            context,
            args,
            computed_values,
            false,
            RenderMode::Shell,
            &mut ignored_stats,
        )
    });
    let rendered_runner = render_runtime_string(
        &runtime.runner,
        context,
        args,
        computed_values,
        false,
        RenderMode::Shell,
        &mut ignored_stats,
    );
    let rendered_fallback = runtime.fallback_runner.as_deref().map(|value| {
        render_runtime_string(
            value,
            context,
            args,
            computed_values,
            false,
            RenderMode::Shell,
            &mut ignored_stats,
        )
    });

    let selected = select_runner_mode(
        &context.dir,
        rendered_check.as_deref(),
        Some(rendered_runner.as_str()),
        rendered_fallback.as_deref(),
    );

    let runtime_runner = match selected {
        RunnerMode::Runner(value) | RunnerMode::Fallback(value) => value,
        RunnerMode::Direct => {
            eprintln!("[fire] Runtime `{runtime_key}` has no valid runner.");
            process::exit(1);
        }
    };

    log_runtime(&format!("{runtime_key} -> {runtime_runner}"));
    let launch = format!(
        "{} {}",
        runtime_runner,
        runtime_bootstrap_invocation(&runtime.sdk)
    );
    let mut engine = RuntimeEngine::start(&launch, &context.dir);

    let library_paths =
        resolve_runtime_library_paths(resolved.runtime_paths_base_dir, &runtime.paths);
    for path in &library_paths {
        engine.load(path);
    }

    engine
}

fn compute_values(
    resolved: &ResolvedCommand<'_>,
    context: &ExecutionContext,
    original_args: &[String],
) -> ComputeResult {
    if context.compute.is_empty() {
        return ComputeResult::default();
    }

    let mut engines: BTreeMap<String, RuntimeEngine> = BTreeMap::new();
    let mut values = BTreeMap::new();
    let mut stats = RenderStats::default();

    for (token, expr) in &context.compute {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }

        match compute_expression_value(
            resolved,
            context,
            original_args,
            &values,
            &mut engines,
            expr,
            &mut stats,
        ) {
            Ok(value) => {
                values.insert(token.to_string(), value);
            }
            Err(err) => {
                close_runtime_engines(engines);
                eprintln!("[fire] {err}");
                process::exit(1);
            }
        }
    }

    close_runtime_engines(engines);
    ComputeResult { values, stats }
}

fn compute_expression_value(
    resolved: &ResolvedCommand<'_>,
    context: &ExecutionContext,
    original_args: &[String],
    computed_values: &BTreeMap<String, String>,
    engines: &mut BTreeMap<String, RuntimeEngine>,
    expr: &str,
    stats: &mut RenderStats,
) -> Result<String, String> {
    match parse_compute_expression(expr, resolved.runtimes) {
        ComputeExpression::Runtime { key, code } => {
            let rendered_code = render_runtime_string(
                code,
                context,
                original_args,
                computed_values,
                true,
                RenderMode::Eval,
                stats,
            );

            let engine = if let Some(engine) = engines.get_mut(key) {
                engine
            } else {
                let engine = start_runtime_engine_for_key(
                    key,
                    resolved,
                    context,
                    original_args,
                    computed_values,
                );
                engines.insert(key.to_string(), engine);
                engines.get_mut(key).unwrap()
            };

            engine
                .eval_capture(&rendered_code)
                .map(trim_trailing_newlines)
                .map_err(|err| format!("Runtime `{key}` compute failed: {err}"))
        }
        ComputeExpression::Shell(command) => {
            let rendered_command = render_runtime_string(
                command,
                context,
                original_args,
                computed_values,
                true,
                RenderMode::Shell,
                stats,
            );
            run_shell_command_capture(&rendered_command, &context.dir).map(trim_trailing_newlines)
        }
    }
}

enum ComputeExpression<'a> {
    Runtime { key: &'a str, code: &'a str },
    Shell(&'a str),
}

fn parse_compute_expression<'a>(
    expr: &'a str,
    runtimes: &BTreeMap<String, RuntimeConfig>,
) -> ComputeExpression<'a> {
    let trimmed = expr.trim();
    if let Some(index) = trimmed.find(':') {
        let prefix = &trimmed[..index];
        if !prefix.is_empty() && !prefix.chars().any(char::is_whitespace) {
            let code = trimmed[index + 1..].trim();
            if matches!(prefix, "py" | "python" | "ts" | "js" | "node" | "deno")
                && !runtimes.contains_key(prefix)
            {
                eprintln!("[fire] Runtime `{prefix}` is not defined in `runtimes`.");
                process::exit(1);
            }
            if runtimes.contains_key(prefix) {
                if code.is_empty() {
                    eprintln!(
                        "[fire] Invalid compute expression `{expr}`. Runtime code is required."
                    );
                    process::exit(1);
                }
                return ComputeExpression::Runtime { key: prefix, code };
            }
        }
    }

    ComputeExpression::Shell(trimmed)
}

fn close_runtime_engines(engines: BTreeMap<String, RuntimeEngine>) {
    for (_, mut engine) in engines.into_iter() {
        engine.close();
    }
}

fn trim_trailing_newlines(mut value: String) -> String {
    while value.ends_with(['\n', '\r']) {
        value.pop();
    }
    value
}

impl RuntimeEngine {
    fn start(command: &str, dir: &Path) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap_or_else(|err| {
            eprintln!("[fire] Failed to bind runtime control channel: {err}");
            process::exit(1);
        });
        let addr = listener.local_addr().unwrap_or_else(|err| {
            eprintln!("[fire] Failed to inspect runtime control channel: {err}");
            process::exit(1);
        });
        listener.set_nonblocking(true).unwrap_or_else(|err| {
            eprintln!("[fire] Failed to configure runtime control channel: {err}");
            process::exit(1);
        });

        let mut child = Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(dir)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .env("FIRE_RUNTIME_RPC_ADDR", addr.to_string())
            .spawn()
            .unwrap_or_else(|err| {
                eprintln!("[fire] Failed to start runtime runner `{command}`: {err}");
                process::exit(1);
            });

        let started = Instant::now();
        let stream = loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    stream.set_nonblocking(false).unwrap_or_else(|err| {
                        let _ = child.kill();
                        eprintln!(
                            "[fire] Failed to configure runtime control stream: {err}"
                        );
                        process::exit(1);
                    });
                    break stream;
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    if let Ok(Some(status)) = child.try_wait() {
                        eprintln!(
                            "[fire] Runtime runner exited before connecting (status: {}).",
                            status
                        );
                        process::exit(status.code().unwrap_or(1));
                    }
                    if started.elapsed() > Duration::from_secs(5) {
                        let _ = child.kill();
                        eprintln!("[fire] Runtime runner did not establish control channel.");
                        process::exit(1);
                    }
                    thread::sleep(Duration::from_millis(20));
                }
                Err(err) => {
                    let _ = child.kill();
                    eprintln!("[fire] Runtime control channel accept failed: {err}");
                    process::exit(1);
                }
            }
        };

        let writer = stream.try_clone().unwrap_or_else(|err| {
            let _ = child.kill();
            eprintln!("[fire] Runtime control channel clone failed: {err}");
            process::exit(1);
        });

        Self {
            session: RuntimeSession {
                child,
                writer,
                reader: BufReader::new(stream),
            },
            next_id: 1,
        }
    }

    fn load(&mut self, path: &Path) {
        let path_str = path.to_string_lossy().to_string();
        let id = self.next_request_id();
        let request = RuntimeRequest {
            op: "load",
            id: id.clone(),
            path: Some(path_str.as_str()),
            code: None,
        };
        if let Err(err) = run_runtime_request(&mut self.session, &request) {
            eprintln!(
                "[fire] Failed to load runtime library `{}`: {err}",
                path.display()
            );
            self.close();
            process::exit(1);
        }
    }

    fn eval(&mut self, code: &str) -> Vec<String> {
        let id = self.next_request_id();
        let request = RuntimeRequest {
            op: "eval",
            id: id.clone(),
            path: None,
            code: Some(code),
        };

        match run_runtime_request(&mut self.session, &request) {
            Ok(response) => {
                for line in response.output_lines {
                    println!("{line}");
                }

                match response.eval_result {
                    Some(RuntimeEvalResult::Text(value)) => {
                        println!("{value}");
                        Vec::new()
                    }
                    Some(RuntimeEvalResult::Commands(commands)) => commands,
                    None => Vec::new(),
                }
            }
            Err(err) => {
                eprintln!("[fire] Runtime eval failed: {err}");
                self.close();
                process::exit(1);
            }
        }
    }

    fn eval_capture(&mut self, code: &str) -> Result<String, String> {
        let id = self.next_request_id();
        let request = RuntimeRequest {
            op: "eval",
            id: id.clone(),
            path: None,
            code: Some(code),
        };

        run_runtime_request(&mut self.session, &request)
            .map(|response| {
                let mut lines = response.output_lines;
                match response.eval_result {
                    Some(RuntimeEvalResult::Text(value)) => lines.push(value),
                    Some(RuntimeEvalResult::Commands(commands)) => lines.push(commands.join("\n")),
                    None => {}
                }
                lines.join("\n")
            })
            .map(|mut output| {
                while output.ends_with(['\n', '\r']) {
                    output.pop();
                }
                output
            })
    }

    fn close(&mut self) {
        let id = self.next_request_id();
        let request = RuntimeRequest {
            op: "exit",
            id,
            path: None,
            code: None,
        };
        let _ = run_runtime_request(&mut self.session, &request);
        for _ in 0..20 {
            match self.session.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => thread::sleep(Duration::from_millis(25)),
                Err(_) => break,
            }
        }
        let _ = self.session.child.kill();
        let _ = self.session.child.wait();
    }

    fn next_request_id(&mut self) -> String {
        let current = self.next_id;
        self.next_id += 1;
        current.to_string()
    }
}

fn run_runtime_request(
    session: &mut RuntimeSession,
    request: &RuntimeRequest<'_>,
) -> Result<RuntimeResponse, String> {
    let payload = format_runtime_request_json(request);
    writeln!(session.writer, "{payload}").map_err(|err| format!("Cannot write request: {err}"))?;
    session
        .writer
        .flush()
        .map_err(|err| format!("Cannot flush request: {err}"))?;

    let done_marker = format!("__FIRE_DONE__{}", request.id);
    let error_prefix = format!("__FIRE_ERROR__{}:", request.id);
    let result_prefix = format!("__FIRE_RESULT__{}:", request.id);
    let mut output = Vec::new();
    let mut eval_result = None;

    loop {
        let mut line = String::new();
        let bytes = session
            .reader
            .read_line(&mut line)
            .map_err(|err| format!("Cannot read runtime output: {err}"))?;
        if bytes == 0 {
            return Err("Runtime process closed unexpectedly.".to_string());
        }

        let line = line.trim_end_matches(['\r', '\n']).to_string();
        if line == done_marker {
            break;
        }
        if let Some(rest) = line.strip_prefix(&error_prefix) {
            return Err(rest.to_string());
        }
        if let Some(payload) = line.strip_prefix(&result_prefix) {
            if let Some(parsed) = parse_runtime_result_payload(payload) {
                eval_result = Some(parsed);
                continue;
            }
        }
        output.push(line);
    }

    Ok(RuntimeResponse {
        output_lines: output,
        eval_result,
    })
}

#[derive(Debug, Deserialize)]
struct RuntimeResultPayload {
    kind: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    commands: Vec<String>,
}

fn parse_runtime_result_payload(payload: &str) -> Option<RuntimeEvalResult> {
    let parsed: RuntimeResultPayload = yaml_serde::from_str(payload).ok()?;
    match parsed.kind.as_str() {
        "text" => Some(RuntimeEvalResult::Text(parsed.text)),
        "commands" => Some(RuntimeEvalResult::Commands(parsed.commands)),
        _ => None,
    }
}

fn format_runtime_request_json(request: &RuntimeRequest<'_>) -> String {
    let mut parts = Vec::new();
    parts.push(format!("\"op\":{}", json_quote(request.op)));
    parts.push(format!("\"id\":{}", json_quote(&request.id)));
    if let Some(path) = request.path {
        parts.push(format!("\"path\":{}", json_quote(path)));
    }
    if let Some(code) = request.code {
        parts.push(format!("\"code\":{}", json_quote(code)));
    }
    format!("{{{}}}", parts.join(","))
}

fn resolve_runtime_definition(key: &str, config: &RuntimeConfig) -> RuntimeDefinition {
    let sdk = parse_runtime_sdk(&config.sdk);
    let runner = non_empty(&config.runner)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| default_runtime_runner(&sdk).to_string());
    let check = non_empty(&config.check).map(ToOwned::to_owned);
    let fallback_runner = non_empty(&config.fallback_runner).map(ToOwned::to_owned);

    if runner.trim().is_empty() {
        eprintln!("[fire] Runtime `{key}` has an invalid runner.");
        process::exit(1);
    }

    RuntimeDefinition {
        sdk,
        runner,
        check,
        fallback_runner,
        paths: config.paths.clone(),
    }
}

fn parse_runtime_sdk(value: &str) -> RuntimeSdk {
    match value.trim() {
        "node" => RuntimeSdk::Node,
        "deno" => RuntimeSdk::Deno,
        "python" => RuntimeSdk::Python,
        other => {
            eprintln!(
                "[fire] Unsupported runtime sdk `{other}`. Supported values: node, deno, python."
            );
            process::exit(1);
        }
    }
}

fn default_runtime_runner(sdk: &RuntimeSdk) -> &'static str {
    match sdk {
        RuntimeSdk::Node => "node",
        RuntimeSdk::Deno => "deno",
        RuntimeSdk::Python => "python",
    }
}

fn split_runtime_eval(value: &str) -> (&str, &str) {
    let Some(index) = value.find(':') else {
        eprintln!("[fire] Invalid eval expression `{value}`. Expected format `<runtime>:<code>`.");
        process::exit(1);
    };

    let runtime = value[..index].trim();
    let code = value[index + 1..].trim();
    if runtime.is_empty() || code.is_empty() {
        eprintln!("[fire] Invalid eval expression `{value}`. Runtime key and code are required.");
        process::exit(1);
    }
    (runtime, code)
}

fn runtime_bootstrap_invocation(sdk: &RuntimeSdk) -> String {
    match sdk {
        RuntimeSdk::Python => {
            format!("-u -c {}", shell_escape(python_runtime_bootstrap_script()))
        }
        RuntimeSdk::Node => format!("-e {}", shell_escape(node_runtime_bootstrap_script())),
        RuntimeSdk::Deno => format!("eval {}", shell_escape(deno_runtime_bootstrap_script())),
    }
}

fn resolve_runtime_library_paths(base_dir: &Path, patterns: &[String]) -> Vec<PathBuf> {
    let mut files = BTreeSet::new();
    for pattern in patterns {
        let pattern = pattern.trim();
        if pattern.is_empty() {
            continue;
        }

        for path in expand_runtime_pattern(base_dir, pattern) {
            if path.is_file() {
                files.insert(path);
            }
        }
    }

    files.into_iter().collect()
}

fn expand_runtime_pattern(base_dir: &Path, pattern: &str) -> Vec<PathBuf> {
    let full = if Path::new(pattern).is_absolute() {
        PathBuf::from(pattern)
    } else {
        base_dir.join(pattern)
    };

    let pattern_text = full.to_string_lossy();
    if !pattern_text.contains('*') {
        return if full.exists() {
            vec![full]
        } else {
            Vec::new()
        };
    }

    let parent = full.parent().unwrap_or_else(|| Path::new("."));
    let file_pattern = full
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("");

    if parent.to_string_lossy().contains('*') {
        eprintln!(
            "[fire] Unsupported runtime path pattern `{pattern}`. Wildcards are only supported in the file name."
        );
        process::exit(1);
    }

    let entries = fs::read_dir(parent).unwrap_or_else(|err| {
        eprintln!(
            "[fire] Failed to read runtime path directory `{}`: {err}",
            parent.display()
        );
        process::exit(1);
    });

    let mut matches = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if wildcard_match(name, file_pattern) {
            matches.push(path);
        }
    }
    matches.sort();
    matches
}

fn wildcard_match(value: &str, pattern: &str) -> bool {
    if pattern == "*" {
        return true;
    }

    let mut cursor = 0usize;
    let mut first = true;
    let parts = pattern.split('*').collect::<Vec<_>>();
    for (index, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }

        if first && !pattern.starts_with('*') {
            if !value[cursor..].starts_with(part) {
                return false;
            }
            cursor += part.len();
            first = false;
            continue;
        }

        if index + 1 == parts.len() && !pattern.ends_with('*') {
            return value[cursor..].ends_with(part);
        }

        let Some(found) = value[cursor..].find(part) else {
            return false;
        };
        cursor += found + part.len();
        first = false;
    }

    if !pattern.ends_with('*') {
        if let Some(last) = parts.last() {
            return value.ends_with(last);
        }
    }

    true
}

fn enforce_unused_args_policy(
    context: &ExecutionContext,
    remaining_args: &[String],
    stats: &RenderStats,
) {
    if remaining_args.is_empty() {
        return;
    }

    let mode = context.on_unused_args.unwrap_or(UnusedArgsMode::Ignore);
    if matches!(mode, UnusedArgsMode::Ignore) {
        return;
    }

    let unused_indexes = remaining_args
        .iter()
        .enumerate()
        .filter_map(|(index, _)| {
            if stats.used_arg_indexes.contains(&index) {
                None
            } else {
                Some(index + 1)
            }
        })
        .collect::<Vec<_>>();

    if unused_indexes.is_empty() {
        return;
    }

    match mode {
        UnusedArgsMode::Ignore => {}
        UnusedArgsMode::Warn => {
            eprintln!(
                "[fire] Warning: unused arguments detected: {:?}",
                unused_indexes
            );
        }
        UnusedArgsMode::Error => {
            eprintln!(
                "[fire] Error: unused arguments detected: {:?}",
                unused_indexes
            );
            process::exit(1);
        }
    }
}

fn python_runtime_bootstrap_script() -> &'static str {
    r#"import json, os, socket, sys
ctx = globals()
addr = os.environ.get("FIRE_RUNTIME_RPC_ADDR", "")
if ":" not in addr:
    sys.stderr.write("Missing FIRE_RUNTIME_RPC_ADDR\n")
    sys.stderr.flush()
    sys.exit(1)
host, port = addr.rsplit(":", 1)
sock = socket.create_connection((host, int(port)))
reader = sock.makefile("r", encoding="utf-8", newline="\n")
writer = sock.makefile("w", encoding="utf-8", newline="\n")
def send(line):
    writer.write(line + "\n")
    writer.flush()
for raw in reader:
    raw = raw.strip()
    if not raw:
        continue
    msg = json.loads(raw)
    rid = str(msg.get("id", "0"))
    try:
        op = msg.get("op")
        if op == "load":
            with open(msg["path"], "r", encoding="utf-8") as handle:
                exec(handle.read(), ctx)
            send(f"__FIRE_DONE__{rid}")
        elif op == "eval":
            code = msg["code"]
            try:
                value = eval(code, ctx)
            except SyntaxError:
                exec(code, ctx)
                value = None
            if isinstance(value, list) and all(isinstance(item, str) for item in value):
                payload = json.dumps({"kind": "commands", "commands": value}, ensure_ascii=False)
                send(f"__FIRE_RESULT__{rid}:{payload}")
            elif isinstance(value, str):
                payload = json.dumps({"kind": "text", "text": value}, ensure_ascii=False)
                send(f"__FIRE_RESULT__{rid}:{payload}")
            elif value is not None:
                payload = json.dumps({"kind": "text", "text": str(value)}, ensure_ascii=False)
                send(f"__FIRE_RESULT__{rid}:{payload}")
            send(f"__FIRE_DONE__{rid}")
        elif op == "exit":
            send(f"__FIRE_DONE__{rid}")
            break
    except Exception as err:
        send(f"__FIRE_ERROR__{rid}:{err}")
        send(f"__FIRE_DONE__{rid}")"#
}

fn node_runtime_bootstrap_script() -> &'static str {
    r#"const net = require("node:net");
const readline = require("node:readline");
const { pathToFileURL } = require("node:url");
const addr = process.env.FIRE_RUNTIME_RPC_ADDR || "";
const [host, portRaw] = addr.split(":");
if (!host || !portRaw) process.exit(1);
const socket = net.createConnection({ host, port: Number(portRaw) });
const rl = readline.createInterface({ input: socket, crlfDelay: Infinity });
const done = (id) => socket.write(`__FIRE_DONE__${id}\n`);
const result = (id, payload) => socket.write(`__FIRE_RESULT__${id}:${JSON.stringify(payload)}\n`);
rl.on("line", async (line) => {
  const trimmed = line.trim();
  if (!trimmed) return;
  let msg;
  try {
    msg = JSON.parse(trimmed);
  } catch (err) {
    return;
  }
  const id = String(msg.id ?? "0");
  try {
    if (msg.op === "load") {
      const mod = await import(pathToFileURL(msg.path).href);
      Object.assign(globalThis, mod);
      done(id);
      return;
    }
    if (msg.op === "eval") {
      const value = await eval(msg.code);
      if (Array.isArray(value) && value.every((item) => typeof item === "string")) {
        result(id, { kind: "commands", commands: value });
      } else if (typeof value === "string") {
        result(id, { kind: "text", text: value });
      } else if (value !== undefined) {
        result(id, { kind: "text", text: String(value) });
      }
      done(id);
      return;
    }
    if (msg.op === "exit") {
      done(id);
      process.exit(0);
    }
  } catch (err) {
    socket.write(`__FIRE_ERROR__${id}:${err && err.message ? err.message : String(err)}\n`);
    done(id);
  }
});"#
}

fn deno_runtime_bootstrap_script() -> &'static str {
    r#"const encoder = new TextEncoder();
const decoder = new TextDecoder();
const addr = Deno.env.get("FIRE_RUNTIME_RPC_ADDR") ?? "";
const [host, portRaw] = addr.split(":");
if (!host || !portRaw) Deno.exit(1);
const conn = await Deno.connect({ hostname: host, port: Number(portRaw) });
let buffer = "";
const writeLine = async (value) => {
  await conn.write(encoder.encode(value + "\n"));
};
const done = async (id) => writeLine(`__FIRE_DONE__${id}`);
const result = async (id, payload) => writeLine(`__FIRE_RESULT__${id}:${JSON.stringify(payload)}`);
const toFileUrl = (path) => new URL(`file://${path.startsWith("/") ? path : "/" + path}`).href;
const handle = async (line) => {
  const trimmed = line.trim();
  if (!trimmed) return;
  let msg;
  try {
    msg = JSON.parse(trimmed);
  } catch (_) {
    return;
  }
  const id = String(msg.id ?? "0");
  try {
    if (msg.op === "load") {
      const mod = await import(toFileUrl(msg.path));
      Object.assign(globalThis, mod);
      await done(id);
      return;
    }
    if (msg.op === "eval") {
      const value = await eval(msg.code);
      if (Array.isArray(value) && value.every((item) => typeof item === "string")) {
        await result(id, { kind: "commands", commands: value });
      } else if (typeof value === "string") {
        await result(id, { kind: "text", text: value });
      } else if (value !== undefined) {
        await result(id, { kind: "text", text: String(value) });
      }
      await done(id);
      return;
    }
    if (msg.op === "exit") {
      await done(id);
      Deno.exit(0);
    }
  } catch (err) {
    await writeLine(`__FIRE_ERROR__${id}:${err && err.message ? err.message : String(err)}`);
    await done(id);
  }
};
for await (const chunk of conn.readable) {
  buffer += decoder.decode(chunk, { stream: true });
  let idx = buffer.indexOf("\n");
  while (idx >= 0) {
    const line = buffer.slice(0, idx);
    buffer = buffer.slice(idx + 1);
    await handle(line);
    idx = buffer.indexOf("\n");
  }
}"#
}

fn should_execute_before(mode: &RunnerMode) -> bool {
    !matches!(mode, RunnerMode::Fallback(_))
}

#[derive(Debug, Default)]
struct ExecutionContext {
    before: Option<String>,
    dir: PathBuf,
    runner: Option<String>,
    fallback_runner: Option<String>,
    check: Option<String>,
    placeholder: Option<String>,
    on_unused_args: Option<UnusedArgsMode>,
    macros: BTreeMap<String, String>,
    compute: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RunnerMode {
    Direct,
    Runner(String),
    Fallback(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnusedArgsMode {
    Ignore,
    Warn,
    Error,
}

#[derive(Debug, Default, Clone)]
struct RenderStats {
    used_arg_indexes: BTreeSet<usize>,
    had_placeholders: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RenderMode {
    Shell,
    Eval,
}

fn build_execution_context(resolved: &ResolvedCommand<'_>) -> ExecutionContext {
    let mut context = ExecutionContext {
        dir: resolved.project_dir.to_path_buf(),
        ..ExecutionContext::default()
    };

    for entry in &resolved.command_chain {
        let Some(spec) = entry.spec() else {
            continue;
        };

        if let Some(next_dir) = non_empty(&spec.dir) {
            context.dir = resolve_next_dir(&context.dir, next_dir);
        }
        if let Some(before) = non_empty(&spec.before) {
            context.before = Some(before.to_string());
        }
        if let Some(check) = non_empty(&spec.check) {
            context.check = Some(check.to_string());
        }
        if let Some(runner) = non_empty(&spec.runner) {
            context.runner = Some(runner.to_string());
        }
        if let Some(fallback_runner) = non_empty(&spec.fallback_runner) {
            context.fallback_runner = Some(fallback_runner.to_string());
        }
        if let Some(placeholder) = non_empty(&spec.placeholder) {
            context.placeholder = Some(placeholder.to_string());
        }
        if let Some(on_unused_args) = non_empty(&spec.on_unused_args) {
            context.on_unused_args = Some(parse_on_unused_args_mode(on_unused_args));
        }
        if !spec.compute.is_empty() {
            for (key, value) in &spec.compute {
                context.compute.insert(key.clone(), value.clone());
            }
        }
        for (macro_key, macro_value) in &spec.macros {
            context
                .macros
                .insert(macro_key.clone(), macro_value.clone());
        }
    }

    context
}

fn parse_on_unused_args_mode(value: &str) -> UnusedArgsMode {
    match value {
        "ignore" => UnusedArgsMode::Ignore,
        "warn" => UnusedArgsMode::Warn,
        "error" => UnusedArgsMode::Error,
        _ => {
            eprintln!(
                "[fire] Invalid on_unused_args value `{value}`. Use one of: ignore, warn, error."
            );
            process::exit(1);
        }
    }
}

fn unresolved_args_for_tail(
    context: &ExecutionContext,
    remaining_args: &[String],
    stats: &RenderStats,
) -> Vec<String> {
    if remaining_args.is_empty() {
        return Vec::new();
    }

    let placeholder_configured = context.placeholder.is_some();

    if !placeholder_configured && !stats.had_placeholders {
        return remaining_args.to_vec();
    }

    remaining_args
        .iter()
        .enumerate()
        .filter_map(|(index, arg)| {
            if stats.used_arg_indexes.contains(&index) {
                None
            } else {
                Some(arg.clone())
            }
        })
        .collect::<Vec<_>>()
}

fn render_runtime_string(
    value: &str,
    context: &ExecutionContext,
    remaining_args: &[String],
    computed_values: &BTreeMap<String, String>,
    track_usage: bool,
    mode: RenderMode,
    stats: &mut RenderStats,
) -> String {
    let with_macros = apply_macros(value, &context.macros);
    let with_compute = apply_macros(&with_macros, computed_values);

    let mut output = with_compute;
    let templates = placeholder_templates(context.placeholder.as_deref());
    for template in &templates {
        output = replace_placeholder_template(
            &output,
            template,
            remaining_args,
            track_usage,
            mode,
            stats,
        );
    }

    for template in &templates {
        output = replace_array_placeholder_literal_forms(
            &output,
            template,
            remaining_args,
            track_usage,
            mode,
            stats,
        );
    }

    output
}

fn split_placeholder_pattern(template: &str) -> Option<(&str, &str)> {
    let trimmed = template.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Some(idx) = trimmed.find("{n}") {
        let prefix = &trimmed[..idx];
        let suffix = &trimmed[idx + 3..];
        if prefix.is_empty() && suffix.is_empty() {
            return None;
        }
        return Some((prefix, suffix));
    }

    if let Some(idx) = trimmed.find('n') {
        let prefix = &trimmed[..idx];
        let suffix = &trimmed[idx + 1..];
        if prefix.is_empty() && suffix.is_empty() {
            return None;
        }
        return Some((prefix, suffix));
    }

    None
}

fn apply_macros(value: &str, macros_map: &BTreeMap<String, String>) -> String {
    if macros_map.is_empty() {
        return value.to_string();
    }

    let mut ordered_macros = macros_map
        .iter()
        .filter(|(key, _)| !key.is_empty())
        .collect::<Vec<_>>();
    ordered_macros.sort_by(|(left, _), (right, _)| right.len().cmp(&left.len()));

    let mut output = value.to_string();
    for _ in 0..8 {
        let mut changed = false;
        for (key, replacement) in &ordered_macros {
            if output.contains(*key) {
                output = output.replace(*key, replacement);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    output
}

fn placeholder_templates(custom: Option<&str>) -> Vec<String> {
    let mut templates = Vec::new();
    if let Some(custom) = custom {
        let custom = custom.trim();
        if !custom.is_empty() {
            templates.push(custom.to_string());
        }
    }
    templates
}

fn replace_placeholder_template(
    input: &str,
    template: &str,
    remaining_args: &[String],
    track_usage: bool,
    mode: RenderMode,
    stats: &mut RenderStats,
) -> String {
    let Some((prefix, suffix)) = split_placeholder_pattern(template) else {
        return input.to_string();
    };

    let mut output = String::new();
    let mut cursor = 0;

    while cursor < input.len() {
        let Some(relative_prefix_start) = input[cursor..].find(prefix) else {
            output.push_str(&input[cursor..]);
            break;
        };

        let prefix_start = cursor + relative_prefix_start;
        output.push_str(&input[cursor..prefix_start]);

        let digit_start = prefix_start + prefix.len();
        let mut digit_end = digit_start;

        while digit_end < input.len() {
            let Some(ch) = input[digit_end..].chars().next() else {
                break;
            };
            if ch.is_ascii_digit() {
                digit_end += ch.len_utf8();
            } else {
                break;
            }
        }

        if digit_start == digit_end {
            output.push_str(prefix);
            cursor = prefix_start + prefix.len();
            continue;
        }

        if !suffix.is_empty() {
            let suffix_end = digit_end + suffix.len();
            if suffix_end > input.len() || &input[digit_end..suffix_end] != suffix {
                output.push_str(prefix);
                cursor = prefix_start + prefix.len();
                continue;
            }

            let index_raw = &input[digit_start..digit_end];
            let index = index_raw
                .parse::<usize>()
                .ok()
                .and_then(|value| value.checked_sub(1));

            if track_usage {
                stats.had_placeholders = true;
            }

            if let Some(index) = index {
                if let Some(value) = remaining_args.get(index) {
                    if track_usage {
                        stats.used_arg_indexes.insert(index);
                    }
                    output.push_str(&format_placeholder_value(value, mode));
                }
            }

            cursor = suffix_end;
            continue;
        }

        let index_raw = &input[digit_start..digit_end];
        let index = index_raw
            .parse::<usize>()
            .ok()
            .and_then(|value| value.checked_sub(1));

        if track_usage {
            stats.had_placeholders = true;
        }

        if let Some(index) = index {
            if let Some(value) = remaining_args.get(index) {
                if track_usage {
                    stats.used_arg_indexes.insert(index);
                }
                output.push_str(&format_placeholder_value(value, mode));
            }
        }

        cursor = digit_end;
    }

    output
}

fn replace_array_placeholder_literal_forms(
    input: &str,
    template: &str,
    remaining_args: &[String],
    track_usage: bool,
    mode: RenderMode,
    stats: &mut RenderStats,
) -> String {
    let mut output = input.to_string();
    output = replace_array_literal_token(
        &output,
        &format!("...{template}"),
        remaining_args,
        track_usage,
        mode,
        ArrayLiteralKind::Spread,
        stats,
    );
    output = replace_array_literal_token(
        &output,
        &format!("[{template}]"),
        remaining_args,
        track_usage,
        mode,
        ArrayLiteralKind::Bracket,
        stats,
    );
    output
}

fn replace_array_literal_token(
    input: &str,
    token: &str,
    remaining_args: &[String],
    track_usage: bool,
    mode: RenderMode,
    kind: ArrayLiteralKind,
    stats: &mut RenderStats,
) -> String {
    if token.is_empty() || !input.contains(token) {
        return input.to_string();
    }

    let unused = remaining_args
        .iter()
        .enumerate()
        .filter_map(|(index, value)| {
            if stats.used_arg_indexes.contains(&index) {
                None
            } else {
                Some((index, value))
            }
        })
        .collect::<Vec<_>>();

    let replacement = if unused.is_empty() {
        String::new()
    } else {
        let args = unused
            .iter()
            .map(|(_, value)| (*value).clone())
            .collect::<Vec<_>>();
        if track_usage {
            stats.had_placeholders = true;
            for (index, _) in &unused {
                stats.used_arg_indexes.insert(*index);
            }
        }
        format_array_literal(&args, mode, kind)
    };

    input.replace(token, &replacement)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArrayLiteralKind {
    Spread,
    Bracket,
}

fn format_placeholder_value(value: &str, mode: RenderMode) -> String {
    match mode {
        RenderMode::Shell => shell_escape(value),
        RenderMode::Eval => value.to_string(),
    }
}

fn format_array_literal(args: &[String], mode: RenderMode, kind: ArrayLiteralKind) -> String {
    match mode {
        RenderMode::Shell => join_shell_args(args),
        RenderMode::Eval => {
            let values = args
                .iter()
                .map(|value| json_quote(value))
                .collect::<Vec<_>>()
                .join(", ");
            match kind {
                ArrayLiteralKind::Spread => values,
                ArrayLiteralKind::Bracket => format!("[{values}]"),
            }
        }
    }
}

fn json_quote(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let code = c as u32;
                out.push_str(&format!("\\u{:04x}", code));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn resolve_next_dir(current: &Path, next: &str) -> PathBuf {
    let next_path = Path::new(next);
    if next_path.is_absolute() {
        next_path.to_path_buf()
    } else {
        current.join(next_path)
    }
}

fn ensure_working_directory(dir: &Path) {
    if !dir.exists() {
        eprintln!("[fire] Working directory does not exist: {}", dir.display());
        process::exit(1);
    }
    if !dir.is_dir() {
        eprintln!(
            "[fire] Working directory is not a directory: {}",
            dir.display()
        );
        process::exit(1);
    }
}

fn select_runner_mode(
    dir: &Path,
    check: Option<&str>,
    runner: Option<&str>,
    fallback_runner: Option<&str>,
) -> RunnerMode {
    let check_passed = check
        .map(|command| run_shell_command_silent(command, dir).success())
        .unwrap_or(true);

    if check_passed {
        if let Some(runner) = runner {
            return RunnerMode::Runner(runner.to_string());
        }
        return RunnerMode::Direct;
    }

    if let Some(fallback_runner) = fallback_runner {
        return RunnerMode::Fallback(fallback_runner.to_string());
    }

    if check.is_some() {
        eprintln!("[fire] Check command failed and no fallback_runner is configured.");
        process::exit(1);
    }

    if let Some(runner) = runner {
        return RunnerMode::Runner(runner.to_string());
    }

    RunnerMode::Direct
}

fn run_with_runner(runner: &str, dir: &Path, commands: &[String]) -> i32 {
    if commands.is_empty() {
        return 0;
    }

    if execution_dry_run_enabled() {
        let normalized_runner = normalize_runner_for_piped_stdin(runner);
        log_runner_start(&normalized_runner);
        for command in commands {
            log_command(command);
        }
        return 0;
    }

    if can_use_attached_runner_mode() {
        if let Some(invocation) = build_attached_shell_runner_invocation(runner, commands) {
            log_runner_start(runner);
            for command in commands {
                log_command(command);
            }
            return run_shell_command_passthrough(&invocation, dir);
        }
    }

    let normalized_runner = normalize_runner_for_piped_stdin(runner);
    log_runner_start(&normalized_runner);
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(&normalized_runner)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap_or_else(|err| {
            eprintln!("[fire] Failed to start runner `{normalized_runner}`: {err}");
            process::exit(1);
        });

    {
        let Some(stdin) = child.stdin.as_mut() else {
            eprintln!("[fire] Runner `{normalized_runner}` has no writable stdin.");
            let _ = child.kill();
            process::exit(1);
        };

        if writeln!(stdin, "set -e").is_err() {
            eprintln!("[fire] Failed to initialize runner shell.");
            let _ = child.kill();
            process::exit(1);
        }

        for command in commands {
            log_command(command);
            if writeln!(stdin, "{command}").is_err() {
                eprintln!("[fire] Failed to send command to runner: `{command}`");
                let _ = child.kill();
                process::exit(1);
            }
        }
        let _ = writeln!(stdin, "exit");
    }

    let status = child.wait().unwrap_or_else(|err| {
        eprintln!("[fire] Failed while waiting for runner `{normalized_runner}`: {err}");
        let _ = child.kill();
        process::exit(1);
    });

    if !status.success() {
        return status.code().unwrap_or(1);
    }

    status.code().unwrap_or(0)
}

fn run_in_single_shell(dir: &Path, commands: &[String]) -> i32 {
    if commands.is_empty() {
        return 0;
    }
    let mut script = String::from("set -e\n");
    for command in commands {
        log_command(command);
        script.push_str(command);
        script.push('\n');
    }

    if execution_dry_run_enabled() {
        return 0;
    }

    let status = Command::new("sh")
        .arg("-c")
        .arg(&script)
        .current_dir(dir)
        .status()
        .unwrap_or_else(|err| {
            eprintln!("[fire] Failed to execute command script: {err}");
            process::exit(1);
        });

    status.code().unwrap_or(1)
}

fn run_shell_command_passthrough(command: &str, dir: &Path) -> i32 {
    let status = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(dir)
        .status()
        .unwrap_or_else(|err| {
            eprintln!("[fire] Failed to execute command script: {err}");
            process::exit(1);
        });

    status.code().unwrap_or(1)
}

fn can_use_attached_runner_mode() -> bool {
    std::io::stdin().is_terminal()
}

fn commands_with_remaining_args(commands: &[String], remaining_args: &[String]) -> Vec<String> {
    let mut out = commands.to_vec();
    if out.is_empty() || remaining_args.is_empty() {
        return out;
    }

    if let Some(last) = out.last_mut() {
        last.push(' ');
        last.push_str(&join_shell_args(remaining_args));
    }

    out
}

fn run_shell_command(command: &str, dir: &Path) -> process::ExitStatus {
    log_before(command);
    if execution_dry_run_enabled() {
        return success_exit_status();
    }
    Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(dir)
        .status()
        .unwrap_or_else(|err| {
            eprintln!("[fire] Failed to execute `{command}`: {err}");
            process::exit(1);
        })
}

fn run_shell_command_capture(command: &str, dir: &Path) -> Result<String, String> {
    log_compute(command);
    let output = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(dir)
        .output()
        .map_err(|err| format!("Failed to execute `{command}`: {err}"))?;

    if !output.status.success() {
        let code = output.status.code().unwrap_or(1);
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.trim().is_empty() {
            return Err(format!(
                "Compute command `{command}` failed with exit code {code}"
            ));
        }
        return Err(format!(
            "Compute command `{command}` failed with exit code {code}: {}",
            stderr.trim()
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn run_shell_command_silent(command: &str, dir: &Path) -> process::ExitStatus {
    log_check(command);
    if execution_dry_run_enabled() {
        return success_exit_status();
    }
    Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap_or_else(|err| {
            eprintln!("[fire] Failed to execute `{command}`: {err}");
            process::exit(1);
        })
}

fn join_shell_args(args: &[String]) -> String {
    args.iter()
        .map(String::as_str)
        .map(shell_escape)
        .collect::<Vec<_>>()
        .join(" ")
}

fn log_command(command: impl AsRef<str>) {
    log_step("cmd", command.as_ref());
}

fn log_runner_start(runner: &str) {
    log_step("runner", runner);
}

fn log_check(command: &str) {
    log_step("check", command);
}

fn log_before(command: &str) {
    log_step("before", command);
}

fn log_compute(command: &str) {
    log_step("compute", command);
}

fn log_runtime(message: &str) {
    log_step("runtime", message);
}

fn log_step(label: &str, command: &str) {
    if !execution_logging_enabled() {
        return;
    }

    if log_color_enabled() {
        eprintln!(
            "\x1b[2m[fire]\x1b[0m \x1b[36m{:<8}\x1b[0m \x1b[2m{}\x1b[0m",
            label, command
        );
        return;
    }

    eprintln!("[fire] {:<8} {}", label, command);
}

fn log_color_enabled() -> bool {
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    std::io::stderr().is_terminal()
}

fn execution_logging_enabled() -> bool {
    match std::env::var("FIRE_LOG_COMMANDS") {
        Ok(raw) => !is_disabled_flag(raw.trim()),
        Err(_) => true,
    }
}

fn is_disabled_flag(value: &str) -> bool {
    value == "false"
}

fn execution_dry_run_enabled() -> bool {
    match std::env::var("FIRE_DRY_RUN") {
        Ok(raw) => is_enabled_flag(raw.trim()),
        Err(_) => false,
    }
}

fn is_enabled_flag(value: &str) -> bool {
    value == "true"
}

fn success_exit_status() -> process::ExitStatus {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        process::ExitStatus::from_raw(0)
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::ExitStatusExt;
        process::ExitStatus::from_raw(0)
    }
}

fn shell_escape(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }

    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | ':' | '='))
    {
        return value.to_string();
    }

    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn non_empty(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn build_attached_shell_runner_invocation(runner: &str, commands: &[String]) -> Option<String> {
    if commands.is_empty() {
        return None;
    }

    let tokens = runner.split_whitespace().collect::<Vec<_>>();
    if tokens.is_empty() {
        return None;
    }

    let shell_token = tokens.iter().rev().find(|token| !token.starts_with('-'))?;
    let shell_name = Path::new(shell_token.trim_matches(['"', '\'']))
        .file_name()
        .and_then(|value| value.to_str())?;
    if !is_supported_interactive_shell(shell_name) {
        return None;
    }

    let script = build_runner_script(commands);
    Some(format!("{runner} -c {}", shell_escape(&script)))
}

fn is_supported_interactive_shell(value: &str) -> bool {
    matches!(
        value,
        "sh" | "bash" | "zsh" | "ash" | "dash" | "ksh" | "mksh" | "fish"
    )
}

fn build_runner_script(commands: &[String]) -> String {
    let mut script = String::from("set -e\n");
    for command in commands {
        script.push_str(command);
        script.push('\n');
    }
    script
}

fn normalize_runner_for_piped_stdin(runner: &str) -> String {
    // Commands are sent through stdin. In that mode, explicit TTY flags break
    // tools like docker with "the input device is not a TTY".
    let mut out = Vec::new();
    for token in runner.split_whitespace() {
        if token == "-t" || token == "--tty" {
            continue;
        }
        if token == "-it" || token == "-ti" {
            out.push("-i".to_string());
            continue;
        }
        if token.starts_with('-') && !token.starts_with("--") && token.len() > 2 {
            let mut chars = token.chars();
            let dash = chars.next().unwrap_or('-');
            let flags: String = chars.filter(|ch| *ch != 't').collect();
            if flags.is_empty() {
                continue;
            }
            out.push(format!("{dash}{flags}"));
            continue;
        }
        out.push(token.to_string());
    }

    ensure_non_tty_for_docker_compose_exec(&mut out);

    if out.is_empty() {
        runner.to_string()
    } else {
        out.join(" ")
    }
}

fn ensure_non_tty_for_docker_compose_exec(tokens: &mut Vec<String>) {
    if tokens.is_empty() {
        return;
    }

    let exec_index = if tokens.first().map(String::as_str) == Some("docker-compose") {
        tokens.iter().position(|token| token == "exec")
    } else if tokens.len() >= 2
        && tokens.first().map(String::as_str) == Some("docker")
        && tokens.get(1).map(String::as_str) == Some("compose")
    {
        tokens.iter().position(|token| token == "exec")
    } else {
        None
    };

    let Some(exec_index) = exec_index else {
        return;
    };

    if tokens
        .iter()
        .any(|token| token == "-T" || token == "--no-tty")
    {
        return;
    }

    tokens.insert(exec_index + 1, "-T".to_string());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CommandAction, CommandEntry, CommandSpec};
    use std::{collections::BTreeMap, path::Path};

    fn render_runtime_string(
        value: &str,
        context: &ExecutionContext,
        remaining_args: &[String],
        track_usage: bool,
        mode: RenderMode,
        stats: &mut RenderStats,
    ) -> String {
        super::render_runtime_string(
            value,
            context,
            remaining_args,
            &BTreeMap::new(),
            track_usage,
            mode,
            stats,
        )
    }

    #[test]
    fn escape_single_quote_in_shell_argument() {
        assert_eq!(shell_escape("it'ok"), "'it'\"'\"'ok'");
    }

    #[test]
    fn nested_relative_dirs_are_resolved_from_parent() {
        let root = PathBuf::from("/tmp/project");
        let child = resolve_next_dir(&root, "services");
        let nested = resolve_next_dir(&child, "api");
        assert_eq!(nested, PathBuf::from("/tmp/project/services/api"));
    }

    #[test]
    fn absolute_dir_overrides_parent_dir() {
        let root = PathBuf::from("/tmp/project");
        let nested = resolve_next_dir(&root, "/opt/workspace");
        assert_eq!(nested, PathBuf::from("/opt/workspace"));
    }

    #[test]
    fn remaining_args_only_append_to_last_command() {
        let commands = vec!["npm run build".to_string(), "npm run start".to_string()];
        let result =
            commands_with_remaining_args(&commands, &["--host".to_string(), "0.0.0.0".to_string()]);
        assert_eq!(
            result,
            vec![
                "npm run build".to_string(),
                "npm run start --host 0.0.0.0".to_string()
            ]
        );
    }

    #[test]
    fn select_runner_uses_fallback_when_check_fails() {
        let dir = std::env::current_dir().expect("cwd");
        let selected = select_runner_mode(&dir, Some("false"), Some("bash"), Some("sh"));
        assert_eq!(selected, RunnerMode::Fallback("sh".to_string()));
    }

    #[test]
    fn select_runner_uses_primary_when_check_passes() {
        let dir = std::env::current_dir().expect("cwd");
        let selected = select_runner_mode(&dir, Some("true"), Some("bash"), Some("sh"));
        assert_eq!(selected, RunnerMode::Runner("bash".to_string()));
    }

    #[test]
    fn select_runner_returns_direct_when_no_runner() {
        let dir = std::env::current_dir().expect("cwd");
        let selected = select_runner_mode(&dir, None, None, None);
        assert_eq!(selected, RunnerMode::Direct);
    }

    #[test]
    fn command_entry_spec_is_available_for_spec_variant() {
        let entry = CommandEntry::Spec(CommandSpec {
            dir: "api".to_string(),
            ..CommandSpec::default()
        });
        assert!(entry.spec().is_some());
    }

    #[test]
    fn direct_shell_array_runs_in_same_shell_session() {
        let dir = std::env::temp_dir().join(format!("fire-shell-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("schemas")).expect("create schemas");

        let status = run_in_single_shell(
            &dir,
            &[
                "cd schemas".to_string(),
                "pwd > ../pwd.txt".to_string(),
                "ls > ../ls.txt".to_string(),
            ],
        );
        assert_eq!(status, 0);

        let pwd = fs::read_to_string(dir.join("pwd.txt")).expect("pwd output");
        assert!(
            pwd.trim_end().ends_with("/schemas"),
            "pwd should stay inside schemas, got: {pwd}"
        );

        let ls = fs::read_to_string(dir.join("ls.txt")).expect("ls output");
        assert!(ls.trim().is_empty(), "schemas dir should be empty");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn normalizes_tty_flags_for_piped_runner() {
        let runner = "docker run --rm -it node:lts-alpine /bin/bash";
        let normalized = normalize_runner_for_piped_stdin(runner);
        assert_eq!(normalized, "docker run --rm -i node:lts-alpine /bin/bash");
    }

    #[test]
    fn keeps_non_tty_flags_untouched() {
        let runner = "docker exec -i my-container /bin/sh";
        let normalized = normalize_runner_for_piped_stdin(runner);
        assert_eq!(normalized, runner);
    }

    #[test]
    fn docker_compose_exec_adds_no_tty_flag() {
        let runner = "docker compose exec linux sh";
        let normalized = normalize_runner_for_piped_stdin(runner);
        assert_eq!(normalized, "docker compose exec -T linux sh");
    }

    #[test]
    fn docker_compose_exec_keeps_existing_no_tty_flag() {
        let runner = "docker compose exec -T linux sh";
        let normalized = normalize_runner_for_piped_stdin(runner);
        assert_eq!(normalized, runner);
    }

    #[test]
    fn builds_attached_runner_invocation_for_shell_runner() {
        let runner = "docker exec -it db bash";
        let invocation = build_attached_shell_runner_invocation(
            runner,
            &["mysql -u laravel -p".to_string()],
        );
        assert_eq!(
            invocation,
            Some("docker exec -it db bash -c 'set -e\nmysql -u laravel -p\n'".to_string())
        );
    }

    #[test]
    fn builds_attached_runner_invocation_for_plain_shell_runner() {
        let runner = "bash";
        let invocation =
            build_attached_shell_runner_invocation(runner, &["echo hi".to_string()]);
        assert_eq!(invocation, Some("bash -c 'set -e\necho hi\n'".to_string()));
    }

    #[test]
    fn does_not_build_attached_invocation_without_shell_runner() {
        let runner = "docker compose exec linux";
        let invocation =
            build_attached_shell_runner_invocation(runner, &["echo hi".to_string()]);
        assert_eq!(invocation, None);
    }

    #[test]
    fn before_runs_for_direct_and_primary_runner_but_not_fallback() {
        assert!(should_execute_before(&RunnerMode::Runner(
            "bash".to_string()
        )));
        assert!(!should_execute_before(&RunnerMode::Fallback(
            "bash".to_string()
        )));
        assert!(should_execute_before(&RunnerMode::Direct));
    }

    #[test]
    fn disabled_flag_accepts_only_false() {
        assert!(is_disabled_flag("false"));
    }

    #[test]
    fn disabled_flag_rejects_non_false_values() {
        assert!(!is_disabled_flag(""));
        assert!(!is_disabled_flag("FALSE"));
        assert!(!is_disabled_flag("1"));
        assert!(!is_disabled_flag("true"));
        assert!(!is_disabled_flag("off"));
        assert!(!is_disabled_flag("no"));
    }

    #[test]
    fn enabled_flag_accepts_only_true() {
        assert!(is_enabled_flag("true"));
        assert!(!is_enabled_flag("TRUE"));
        assert!(!is_enabled_flag("1"));
        assert!(!is_enabled_flag("false"));
    }

    #[test]
    fn placeholders_replace_indexed_args_with_shell_escape() {
        let mut context = ExecutionContext::default();
        context.placeholder = Some("{{n}}".to_string());
        let mut stats = RenderStats::default();
        let rendered = render_runtime_string(
            "echo {1} {2} {3}",
            &context,
            &[
                "hello".to_string(),
                "sp ace".to_string(),
                "quo'te".to_string(),
            ],
            true,
            RenderMode::Shell,
            &mut stats,
        );
        assert_eq!(rendered, "echo hello 'sp ace' 'quo'\"'\"'te'");
        assert!(stats.had_placeholders);
        assert_eq!(stats.used_arg_indexes.len(), 3);
    }

    #[test]
    fn custom_placeholder_template_is_supported() {
        let mut context = ExecutionContext::default();
        context.placeholder = Some("[[n]]".to_string());
        let mut stats = RenderStats::default();
        let rendered = render_runtime_string(
            "echo [[1]]",
            &context,
            &["hey".to_string()],
            true,
            RenderMode::Shell,
            &mut stats,
        );
        assert_eq!(rendered, "echo hey");
    }

    #[test]
    fn macros_expand_before_placeholder_replacement() {
        let mut context = ExecutionContext::default();
        context.placeholder = Some("{{n}}".to_string());
        context
            .macros
            .insert("{{dynamic}}".to_string(), "docker exec {1}".to_string());
        let mut stats = RenderStats::default();
        let rendered = render_runtime_string(
            "{{dynamic}} echo hi",
            &context,
            &["front".to_string()],
            true,
            RenderMode::Shell,
            &mut stats,
        );
        assert_eq!(rendered, "docker exec front echo hi");
    }

    #[test]
    fn spread_placeholder_expands_to_remaining_args() {
        let mut context = ExecutionContext::default();
        context.placeholder = Some("{{n}}".to_string());
        let mut stats = RenderStats::default();
        let rendered = render_runtime_string(
            "echo {1} ...{{n}}",
            &context,
            &[
                "first".to_string(),
                "second arg".to_string(),
                "third".to_string(),
            ],
            true,
            RenderMode::Shell,
            &mut stats,
        );
        assert_eq!(rendered, "echo first 'second arg' third");
        assert_eq!(stats.used_arg_indexes.len(), 3);
    }

    #[test]
    fn bracket_placeholder_expands_to_remaining_args() {
        let mut context = ExecutionContext::default();
        context.placeholder = Some("{{n}}".to_string());
        let mut stats = RenderStats::default();
        let rendered = render_runtime_string(
            "echo [{{n}}]",
            &context,
            &["one".to_string(), "two".to_string()],
            true,
            RenderMode::Shell,
            &mut stats,
        );
        assert_eq!(rendered, "echo one two");
        assert_eq!(stats.used_arg_indexes.len(), 2);
    }

    #[test]
    fn spread_placeholder_uses_only_unused_args() {
        let mut context = ExecutionContext::default();
        context.placeholder = Some("{{n}}".to_string());
        let mut stats = RenderStats::default();
        let rendered = render_runtime_string(
            "echo {2} ...{{n}}",
            &context,
            &[
                "first".to_string(),
                "second arg".to_string(),
                "third".to_string(),
            ],
            true,
            RenderMode::Shell,
            &mut stats,
        );
        assert_eq!(rendered, "echo 'second arg' first third");
        assert_eq!(stats.used_arg_indexes.len(), 3);
    }

    #[test]
    fn eval_array_placeholder_uses_only_unused_args() {
        let mut context = ExecutionContext::default();
        context.placeholder = Some("{{n}}".to_string());
        let mut stats = RenderStats::default();
        let rendered = render_runtime_string(
            "fn({2}, [{{n}}])",
            &context,
            &["a".to_string(), "b".to_string(), "c".to_string()],
            true,
            RenderMode::Eval,
            &mut stats,
        );
        assert_eq!(rendered, "fn(b, [\"a\", \"c\"])");
        assert_eq!(stats.used_arg_indexes.len(), 3);
    }

    #[test]
    fn eval_placeholder_replaces_without_shell_escaping() {
        let mut context = ExecutionContext::default();
        context.placeholder = Some("{{n}}".to_string());
        let mut stats = RenderStats::default();
        let rendered = render_runtime_string(
            "sayHello(\"{1}\", {2})",
            &context,
            &["hello world".to_string(), "1 + 1".to_string()],
            true,
            RenderMode::Eval,
            &mut stats,
        );
        assert_eq!(rendered, "sayHello(\"hello world\", 1 + 1)");
    }

    #[test]
    fn eval_spread_placeholder_expands_as_string_arguments() {
        let mut context = ExecutionContext::default();
        context.placeholder = Some("{{n}}".to_string());
        let mut stats = RenderStats::default();
        let rendered = render_runtime_string(
            "fn(...{{n}})",
            &context,
            &["a".to_string(), "b c".to_string()],
            true,
            RenderMode::Eval,
            &mut stats,
        );
        assert_eq!(rendered, "fn(\"a\", \"b c\")");
    }

    #[test]
    fn eval_array_placeholder_expands_as_string_array() {
        let mut context = ExecutionContext::default();
        context.placeholder = Some("{{n}}".to_string());
        let mut stats = RenderStats::default();
        let rendered = render_runtime_string(
            "fn([{{n}}])",
            &context,
            &["a".to_string(), "b".to_string()],
            true,
            RenderMode::Eval,
            &mut stats,
        );
        assert_eq!(rendered, "fn([\"a\", \"b\"])");
    }

    #[test]
    fn split_runtime_eval_parses_runtime_and_code() {
        let (runtime, code) = split_runtime_eval("py:sayHello()");
        assert_eq!(runtime, "py");
        assert_eq!(code, "sayHello()");
    }

    #[test]
    fn runtime_result_payload_parses_text_value() {
        let parsed = parse_runtime_result_payload(r#"{"kind":"text","text":"hello"}"#)
            .expect("parse text payload");
        match parsed {
            RuntimeEvalResult::Text(value) => assert_eq!(value, "hello"),
            RuntimeEvalResult::Commands(_) => panic!("expected text result"),
        }
    }

    #[test]
    fn runtime_result_payload_parses_commands_array() {
        let parsed = parse_runtime_result_payload(
            r#"{"kind":"commands","commands":["echo one","echo two"]}"#,
        )
        .expect("parse commands payload");
        match parsed {
            RuntimeEvalResult::Commands(values) => {
                assert_eq!(values, vec!["echo one".to_string(), "echo two".to_string()])
            }
            RuntimeEvalResult::Text(_) => panic!("expected commands result"),
        }
    }

    #[test]
    fn wildcard_match_supports_basic_star_patterns() {
        assert!(wildcard_match("test.ts", "*.ts"));
        assert!(wildcard_match("helpers.test.ts", "*.test.ts"));
        assert!(!wildcard_match("test.js", "*.ts"));
    }

    #[test]
    fn unresolved_args_defaults_to_passthrough_without_placeholder_or_policy() {
        let context = ExecutionContext::default();
        let stats = RenderStats::default();
        let args = vec!["one".to_string(), "two".to_string()];
        assert_eq!(unresolved_args_for_tail(&context, &args, &stats), args);
    }

    #[test]
    fn unresolved_args_respects_consumed_indexes_without_policy_effect() {
        let mut stats = RenderStats::default();
        stats.had_placeholders = true;
        stats.used_arg_indexes.insert(0);
        let context = ExecutionContext {
            on_unused_args: Some(UnusedArgsMode::Error),
            ..ExecutionContext::default()
        };
        let args = vec!["one".to_string(), "two".to_string()];
        assert_eq!(
            unresolved_args_for_tail(&context, &args, &stats),
            vec!["two".to_string()]
        );
    }

    #[test]
    fn unused_args_policy_defaults_to_ignore_for_eval() {
        let context = ExecutionContext::default();
        let stats = RenderStats::default();
        let args = vec!["one".to_string(), "two".to_string()];
        enforce_unused_args_policy(&context, &args, &stats);
    }

    #[test]
    fn unused_args_policy_warns_without_stopping_eval() {
        let context = ExecutionContext {
            on_unused_args: Some(UnusedArgsMode::Warn),
            ..ExecutionContext::default()
        };
        let mut stats = RenderStats::default();
        stats.had_placeholders = true;
        stats.used_arg_indexes.insert(0);
        let args = vec!["one".to_string(), "two".to_string()];
        enforce_unused_args_policy(&context, &args, &stats);
    }

    #[test]
    fn unused_args_detection_counts_dynamic_spread_in_eval() {
        let mut context = ExecutionContext::default();
        context.placeholder = Some("{{n}}".to_string());
        let mut stats = RenderStats::default();
        let args = vec![
            "first".to_string(),
            "second".to_string(),
            "third".to_string(),
        ];
        render_runtime_string(
            "py:print({1}, ...{{n}})",
            &context,
            &args,
            true,
            RenderMode::Eval,
            &mut stats,
        );

        let unused = args
            .iter()
            .enumerate()
            .filter_map(|(index, value)| {
                if stats.used_arg_indexes.contains(&index) {
                    None
                } else {
                    Some(value.clone())
                }
            })
            .collect::<Vec<_>>();

        assert!(unused.is_empty());
        assert!(stats.had_placeholders);
        assert_eq!(stats.used_arg_indexes.len(), 3);
    }

    #[test]
    fn compute_resolves_literal_variable_values() {
        let mut context = ExecutionContext::default();
        context.dir = std::env::current_dir().expect("cwd");
        context.placeholder = Some("{{n}}".to_string());
        context
            .compute
            .insert("{left}".to_string(), "printf %s {2}".to_string());
        context
            .compute
            .insert("{right}".to_string(), "printf %s {1}".to_string());

        let runtimes = BTreeMap::new();
        let command = CommandEntry::Spec(CommandSpec::default());
        let original = vec!["first".to_string(), "second value".to_string()];
        let resolved = ResolvedCommand {
            project_dir: Path::new("."),
            runtime_paths_base_dir: Path::new("."),
            runtimes: &runtimes,
            command: &command,
            command_chain: vec![&command],
            consumed: 0,
            remaining_args: &original,
        };

        let computed = compute_values(&resolved, &context, &original);
        assert_eq!(computed.values.get("{left}"), Some(&"second value".to_string()));
        assert_eq!(computed.values.get("{right}"), Some(&"first".to_string()));
    }

    #[test]
    fn compute_values_are_reusable_as_exact_tokens() {
        let mut context = ExecutionContext::default();
        context.dir = std::env::current_dir().expect("cwd");
        context.placeholder = Some("{{n}}".to_string());
        context
            .compute
            .insert("{sum}".to_string(), "printf %s 42".to_string());

        let runtimes = BTreeMap::new();
        let command = CommandEntry::Spec(CommandSpec::default());
        let original = vec!["one".to_string()];
        let resolved = ResolvedCommand {
            project_dir: Path::new("."),
            runtime_paths_base_dir: Path::new("."),
            runtimes: &runtimes,
            command: &command,
            command_chain: vec![&command],
            consumed: 0,
            remaining_args: &original,
        };

        let computed = compute_values(&resolved, &context, &original);
        let mut stats = RenderStats::default();
        let rendered = super::render_runtime_string(
            "echo {sum} {1}",
            &context,
            &original,
            &computed.values,
            true,
            RenderMode::Shell,
            &mut stats,
        );
        assert_eq!(rendered, "echo 42 one");
    }

    #[test]
    fn compute_trims_trailing_newlines() {
        let mut context = ExecutionContext::default();
        context.dir = std::env::current_dir().expect("cwd");
        context.placeholder = Some("{{n}}".to_string());
        context
            .compute
            .insert("{value}".to_string(), "echo value".to_string());

        let runtimes = BTreeMap::new();
        let command = CommandEntry::Spec(CommandSpec::default());
        let original = vec!["placeholder".to_string()];
        let resolved = ResolvedCommand {
            project_dir: Path::new("."),
            runtime_paths_base_dir: Path::new("."),
            runtimes: &runtimes,
            command: &command,
            command_chain: vec![&command],
            consumed: 0,
            remaining_args: &original,
        };

        let computed = compute_values(&resolved, &context, &original);
        assert_eq!(computed.values.get("{value}"), Some(&"value".to_string()));
    }

    #[test]
    fn compute_placeholder_consumes_argument_for_exec_tail() {
        let mut context = ExecutionContext::default();
        context.dir = std::env::current_dir().expect("cwd");
        context.placeholder = Some("{{n}}".to_string());
        context
            .compute
            .insert("$name".to_string(), "echo {1}".to_string());

        let runtimes = BTreeMap::new();
        let command = CommandEntry::Spec(CommandSpec::default());
        let original = vec!["Josue".to_string()];
        let resolved = ResolvedCommand {
            project_dir: Path::new("."),
            runtime_paths_base_dir: Path::new("."),
            runtimes: &runtimes,
            command: &command,
            command_chain: vec![&command],
            consumed: 0,
            remaining_args: &original,
        };

        let computed = compute_values(&resolved, &context, &original);
        let mut stats = computed.stats.clone();
        let _ = super::render_runtime_string(
            "echo hello $name",
            &context,
            &original,
            &computed.values,
            true,
            RenderMode::Shell,
            &mut stats,
        );
        let tail = unresolved_args_for_tail(&context, &original, &stats);
        assert!(tail.is_empty());
    }

    #[test]
    fn nested_dirs_are_resolved_relative_to_parent_command() {
        let nested2 = CommandEntry::Spec(CommandSpec {
            dir: "sub".to_string(),
            exec: Some(CommandAction::Single("echo nested2".to_string())),
            ..CommandSpec::default()
        });
        let nested = CommandEntry::Spec(CommandSpec {
            dir: "sub".to_string(),
            commands: BTreeMap::from([("nested2".to_string(), nested2.clone())]),
            ..CommandSpec::default()
        });
        let root = CommandEntry::Spec(CommandSpec {
            dir: "schemas".to_string(),
            commands: BTreeMap::from([("nested".to_string(), nested.clone())]),
            ..CommandSpec::default()
        });

        let runtimes = BTreeMap::new();
        let args = Vec::<String>::new();
        let resolved = ResolvedCommand {
            project_dir: Path::new("/tmp/project"),
            runtime_paths_base_dir: Path::new("/tmp/project"),
            runtimes: &runtimes,
            command: &nested2,
            command_chain: vec![&root, &nested, &nested2],
            consumed: 3,
            remaining_args: &args,
        };

        let context = build_execution_context(&resolved);
        assert_eq!(context.dir, PathBuf::from("/tmp/project/schemas/sub/sub"));
    }
}
