use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    thread,
    time::Duration,
};

use crate::config::RuntimeConfig;
use crate::resolve::ResolvedCommand;

pub(crate) fn execute_resolved_command(resolved: ResolvedCommand<'_>) -> ! {
    let context = build_execution_context(&resolved);
    ensure_working_directory(&context.dir);

    if let Some(raw_evals) = resolved.command.evaluation_expressions() {
        run_evals_with_runtime(&resolved, &context, &raw_evals);
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
            resolved.remaining_args,
            false,
            RenderMode::Shell,
            &mut ignored_stats,
        )
    });
    let rendered_runner = context.runner.as_deref().map(|value| {
        render_runtime_string(
            value,
            &context,
            resolved.remaining_args,
            false,
            RenderMode::Shell,
            &mut ignored_stats,
        )
    });
    let rendered_fallback_runner = context.fallback_runner.as_deref().map(|value| {
        render_runtime_string(
            value,
            &context,
            resolved.remaining_args,
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
                resolved.remaining_args,
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

    let mut render_stats = RenderStats::default();
    let rendered_commands_to_run = raw_commands_to_run
        .iter()
        .map(|command| {
            render_runtime_string(
                command,
                &context,
                resolved.remaining_args,
                true,
                RenderMode::Shell,
                &mut render_stats,
            )
        })
        .collect::<Vec<_>>();

    let tail_args = unresolved_args_for_tail(&context, resolved.remaining_args, &render_stats);
    let commands_to_run = commands_with_remaining_args(&rendered_commands_to_run, &tail_args);

    let mut exit_code = 0;
    match selected_runner {
        RunnerMode::Runner(runner) | RunnerMode::Fallback(runner) => {
            exit_code = run_with_runner(&runner, &context.dir, &commands_to_run);
        }
        RunnerMode::Direct => {
            for command in &commands_to_run {
                let status = run_shell_command(command, &context.dir);
                let code = status.code().unwrap_or(1);
                exit_code = code;
                if code != 0 {
                    break;
                }
            }
        }
    }

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
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

struct RuntimeRequest<'a> {
    op: &'a str,
    id: String,
    path: Option<&'a str>,
    code: Option<&'a str>,
}

fn run_evals_with_runtime(
    resolved: &ResolvedCommand<'_>,
    context: &ExecutionContext,
    raw_evals: &[String],
) -> ! {
    let mut render_stats = RenderStats::default();
    let mut parsed = Vec::new();

    for raw_eval in raw_evals {
        let rendered = render_runtime_string(
            raw_eval,
            context,
            resolved.remaining_args,
            true,
            RenderMode::Eval,
            &mut render_stats,
        );
        let (runtime_key, code) = split_runtime_eval(&rendered);
        parsed.push((runtime_key.to_string(), code.to_string()));
    }

    enforce_unused_args_policy(context, resolved.remaining_args, &render_stats);

    let mut engines: BTreeMap<String, RuntimeEngine> = BTreeMap::new();
    for (runtime_key, code) in &parsed {
        if !engines.contains_key(runtime_key) {
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
                    resolved.remaining_args,
                    false,
                    RenderMode::Shell,
                    &mut ignored_stats,
                )
            });
            let rendered_runner = render_runtime_string(
                &runtime.runner,
                context,
                resolved.remaining_args,
                false,
                RenderMode::Shell,
                &mut ignored_stats,
            );
            let rendered_fallback = runtime.fallback_runner.as_deref().map(|value| {
                render_runtime_string(
                    value,
                    context,
                    resolved.remaining_args,
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

            let normalized_runner = normalize_runner_for_piped_stdin(&runtime_runner);
            let launch = format!(
                "{} {}",
                normalized_runner,
                runtime_bootstrap_invocation(&runtime.sdk)
            );
            let mut engine = RuntimeEngine::start(&launch, &context.dir);

            let library_paths = resolve_runtime_library_paths(&context.dir, &runtime.paths);
            for path in &library_paths {
                engine.load(path);
            }

            engines.insert(runtime_key.clone(), engine);
        }

        let engine = engines.get_mut(runtime_key).unwrap_or_else(|| {
            eprintln!("[fire] Runtime engine `{runtime_key}` not available.");
            process::exit(1);
        });
        engine.eval(code);
    }

    for (_, mut engine) in engines {
        engine.close();
    }

    process::exit(0);
}

impl RuntimeEngine {
    fn start(command: &str, dir: &Path) -> Self {
        let mut child = Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap_or_else(|err| {
                eprintln!("[fire] Failed to start runtime runner `{command}`: {err}");
                process::exit(1);
            });

        let stdin = child.stdin.take().unwrap_or_else(|| {
            eprintln!("[fire] Runtime runner has no stdin.");
            process::exit(1);
        });
        let stdout = child.stdout.take().unwrap_or_else(|| {
            eprintln!("[fire] Runtime runner has no stdout.");
            process::exit(1);
        });

        Self {
            session: RuntimeSession {
                child,
                stdin,
                stdout: BufReader::new(stdout),
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

    fn eval(&mut self, code: &str) {
        let id = self.next_request_id();
        let request = RuntimeRequest {
            op: "eval",
            id: id.clone(),
            path: None,
            code: Some(code),
        };

        match run_runtime_request(&mut self.session, &request) {
            Ok(output_lines) => {
                for line in output_lines {
                    println!("{line}");
                }
            }
            Err(err) => {
                eprintln!("[fire] Runtime eval failed: {err}");
                self.close();
                process::exit(1);
            }
        }
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
) -> Result<Vec<String>, String> {
    let payload = format_runtime_request_json(request);
    writeln!(session.stdin, "{payload}").map_err(|err| format!("Cannot write request: {err}"))?;
    session
        .stdin
        .flush()
        .map_err(|err| format!("Cannot flush request: {err}"))?;

    let done_marker = format!("__FIRE_DONE__{}", request.id);
    let error_prefix = format!("__FIRE_ERROR__{}:", request.id);
    let mut output = Vec::new();

    loop {
        let mut line = String::new();
        let bytes = session
            .stdout
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
        output.push(line);
    }

    Ok(output)
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

    let placeholder_configured = context.placeholder.is_some();
    let policy_configured = context.on_unused_args.is_some();
    if !placeholder_configured && !policy_configured && !stats.had_placeholders {
        return;
    }

    let unused_args = remaining_args
        .iter()
        .enumerate()
        .filter_map(|(index, arg)| {
            if stats.used_arg_indexes.contains(&index) {
                None
            } else {
                Some(arg.clone())
            }
        })
        .collect::<Vec<_>>();

    let mode = context.on_unused_args.unwrap_or(UnusedArgsMode::Error);
    match mode {
        UnusedArgsMode::Ignore => {}
        UnusedArgsMode::Warn => {
            if !unused_args.is_empty() {
                eprintln!(
                    "[fire] Warning: unused args ignored: {}",
                    join_shell_args(&unused_args)
                );
            }
        }
        UnusedArgsMode::Error => {
            if !unused_args.is_empty() {
                eprintln!(
                    "[fire] Unused args are not allowed: {}",
                    join_shell_args(&unused_args)
                );
                process::exit(1);
            }
        }
    }
}

fn python_runtime_bootstrap_script() -> &'static str {
    r#"import json, sys
ctx = globals()
for raw in sys.stdin:
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
            print(f"__FIRE_DONE__{rid}", flush=True)
        elif op == "eval":
            code = msg["code"]
            try:
                value = eval(code, ctx)
            except SyntaxError:
                exec(code, ctx)
                value = None
            if value is not None:
                print(value, flush=True)
            print(f"__FIRE_DONE__{rid}", flush=True)
        elif op == "exit":
            print(f"__FIRE_DONE__{rid}", flush=True)
            break
    except Exception as err:
        print(f"__FIRE_ERROR__{rid}:{err}", flush=True)
        print(f"__FIRE_DONE__{rid}", flush=True)"#
}

fn node_runtime_bootstrap_script() -> &'static str {
    r#"const readline = require("node:readline");
const { pathToFileURL } = require("node:url");
const rl = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
const done = (id) => process.stdout.write(`__FIRE_DONE__${id}\n`);
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
      if (value !== undefined) {
        process.stdout.write(String(value) + "\n");
      }
      done(id);
      return;
    }
    if (msg.op === "exit") {
      done(id);
      process.exit(0);
    }
  } catch (err) {
    process.stdout.write(`__FIRE_ERROR__${id}:${err && err.message ? err.message : String(err)}\n`);
    done(id);
  }
});"#
}

fn deno_runtime_bootstrap_script() -> &'static str {
    r#"const encoder = new TextEncoder();
const decoder = new TextDecoder();
let buffer = "";
const writeLine = async (value) => {
  await Deno.stdout.write(encoder.encode(value + "\n"));
};
const done = async (id) => writeLine(`__FIRE_DONE__${id}`);
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
      if (value !== undefined) {
        await writeLine(String(value));
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
for await (const chunk of Deno.stdin.readable) {
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
    matches!(mode, RunnerMode::Runner(_))
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

#[derive(Debug, Default)]
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
    let policy_configured = context.on_unused_args.is_some();

    if !placeholder_configured && !policy_configured && !stats.had_placeholders {
        return remaining_args.to_vec();
    }

    let unused_args = remaining_args
        .iter()
        .enumerate()
        .filter_map(|(index, arg)| {
            if stats.used_arg_indexes.contains(&index) {
                None
            } else {
                Some(arg.clone())
            }
        })
        .collect::<Vec<_>>();

    let mode = context.on_unused_args.unwrap_or(UnusedArgsMode::Error);
    match mode {
        UnusedArgsMode::Ignore => Vec::new(),
        UnusedArgsMode::Warn => {
            if !unused_args.is_empty() {
                eprintln!(
                    "[fire] Warning: unused args ignored: {}",
                    join_shell_args(&unused_args)
                );
            }
            Vec::new()
        }
        UnusedArgsMode::Error => {
            if !unused_args.is_empty() {
                eprintln!(
                    "[fire] Unused args are not allowed: {}",
                    join_shell_args(&unused_args)
                );
                process::exit(1);
            }
            Vec::new()
        }
    }
}

fn render_runtime_string(
    value: &str,
    context: &ExecutionContext,
    remaining_args: &[String],
    track_usage: bool,
    mode: RenderMode,
    stats: &mut RenderStats,
) -> String {
    let with_macros = apply_macros(value, &context.macros);

    let mut output = with_macros;
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
    templates.push("{n}".to_string());
    templates.push("{{n}}".to_string());
    templates.push("$n".to_string());

    let mut seen = BTreeSet::new();
    let mut unique = templates
        .into_iter()
        .filter(|template| seen.insert(template.clone()))
        .collect::<Vec<_>>();
    unique.sort_by(|left, right| right.len().cmp(&left.len()));
    unique
}

fn replace_placeholder_template(
    input: &str,
    template: &str,
    remaining_args: &[String],
    track_usage: bool,
    mode: RenderMode,
    stats: &mut RenderStats,
) -> String {
    let Some(index_marker) = template.find('n') else {
        return input.to_string();
    };

    let prefix = &template[..index_marker];
    let suffix = &template[index_marker + 1..];

    if prefix.is_empty() {
        return input.to_string();
    }

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
    let normalized_runner = normalize_runner_for_piped_stdin(runner);
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

fn run_shell_command_silent(command: &str, dir: &Path) -> process::ExitStatus {
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
    use crate::config::{CommandEntry, CommandSpec};

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
    fn before_runs_only_for_primary_runner() {
        assert!(should_execute_before(&RunnerMode::Runner(
            "bash".to_string()
        )));
        assert!(!should_execute_before(&RunnerMode::Fallback(
            "bash".to_string()
        )));
        assert!(!should_execute_before(&RunnerMode::Direct));
    }

    #[test]
    fn placeholders_replace_indexed_args_with_shell_escape() {
        let context = ExecutionContext::default();
        let mut stats = RenderStats::default();
        let rendered = render_runtime_string(
            "echo {1} {{2}} $3",
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
        context
            .macros
            .insert("{{dynamic}}".to_string(), "docker exec {{1}}".to_string());
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
            "echo {{1}} ...{{n}}",
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
            "echo {{2}} ...{{n}}",
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
        let context = ExecutionContext::default();
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
    fn unresolved_args_respects_ignore_policy() {
        let context = ExecutionContext {
            on_unused_args: Some(UnusedArgsMode::Ignore),
            ..ExecutionContext::default()
        };
        let stats = RenderStats::default();
        let args = vec!["one".to_string()];
        assert!(unresolved_args_for_tail(&context, &args, &stats).is_empty());
    }

    #[test]
    fn unresolved_args_uses_consumed_indexes() {
        let mut stats = RenderStats::default();
        stats.had_placeholders = true;
        stats.used_arg_indexes.insert(0);
        let context = ExecutionContext {
            on_unused_args: Some(UnusedArgsMode::Ignore),
            ..ExecutionContext::default()
        };
        let args = vec!["one".to_string(), "two".to_string()];
        assert!(unresolved_args_for_tail(&context, &args, &stats).is_empty());
    }
}
