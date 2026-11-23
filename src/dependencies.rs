//! Validation and execution of command dependency graphs (`needs`).

use std::{
    collections::{BTreeMap, BTreeSet},
    io::{BufRead, BufReader},
    os::unix::process::CommandExt,
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use crate::config::{CommandEntry, FileConfig, FileScope, LoadedConfig};

#[derive(Debug, Clone)]
struct PlanNode {
    label: String,
    invocation: Vec<String>,
    dependencies: BTreeSet<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct DependencyPlan {
    root: String,
    nodes: BTreeMap<String, PlanNode>,
    prefixed_output: bool,
}

pub(crate) fn empty_plan() -> DependencyPlan {
    DependencyPlan::default()
}

pub(crate) fn build_plan(
    config: &LoadedConfig,
    root: &CommandEntry,
) -> Result<DependencyPlan, String> {
    let Some((file_index, root_path)) = locate_entry(config, root) else {
        return Err("Could not locate the command in its Fire file.".to_string());
    };
    let mut builder = PlanBuilder {
        config,
        nodes: BTreeMap::new(),
        visiting: Vec::new(),
        prefixed_output: false,
    };
    let root = builder.visit(file_index, &root_path)?;
    Ok(DependencyPlan {
        root,
        nodes: builder.nodes,
        prefixed_output: builder.prefixed_output,
    })
}

struct PlanBuilder<'a> {
    config: &'a LoadedConfig,
    nodes: BTreeMap<String, PlanNode>,
    visiting: Vec<String>,
    prefixed_output: bool,
}

impl PlanBuilder<'_> {
    fn visit(&mut self, file_index: usize, path: &[String]) -> Result<String, String> {
        let key = node_key(file_index, path);
        if let Some(start) = self.visiting.iter().position(|item| item == &key) {
            let mut chain: Vec<String> = self.visiting[start..]
                .iter()
                .filter_map(|item| self.nodes.get(item).map(|node| node.label.clone()))
                .collect();
            chain.push(display_path(path));
            return Err(format!("Dependency cycle: {}", chain.join(" -> ")));
        }
        if self.nodes.contains_key(&key) {
            return Ok(key);
        }

        let file = &self.config.files[file_index];
        let (entry, _) = entry_at_path(file, path)
            .ok_or_else(|| format!("Dependency command `{}` does not exist.", path.join(" ")))?;
        let label = display_path(path);
        self.nodes.insert(
            key.clone(),
            PlanNode {
                label: label.clone(),
                invocation: invocation(file, path),
                dependencies: BTreeSet::new(),
            },
        );
        self.visiting.push(key.clone());

        if !entry.needs().is_empty() && entry.parallel_needs() {
            self.prefixed_output = true;
        }

        let mut previous: Option<String> = None;
        for raw_need in entry.needs() {
            let need_path = parse_need(raw_need)?;
            let (dependency, dependency_chain) = entry_at_path(file, &need_path).ok_or_else(|| {
                format!(
                    "Command `{label}` needs `{raw_need}`, but that command does not exist in the same scope."
                )
            })?;
            if !dependency.is_runnable() {
                return Err(format!(
                    "Command `{label}` needs `{raw_need}`, but that command has no executable action."
                ));
            }
            if inherited_long_running(&dependency_chain) {
                return Err(format!(
                    "Command `{label}` cannot need `{raw_need}` because it is `long_running: true`; use `before` with a check, or a session instead."
                ));
            }

            let dependency_key = self.visit(file_index, &need_path)?;
            self.nodes
                .get_mut(&key)
                .expect("current plan node")
                .dependencies
                .insert(dependency_key.clone());
            if !entry.parallel_needs() {
                if let Some(previous_key) = previous {
                    // If the earlier listed command itself needs this command, it must already
                    // run first; deduplication then makes the later list entry a no-op.
                    if !depends_on(&self.nodes, &previous_key, &dependency_key) {
                        self.nodes
                            .get_mut(&dependency_key)
                            .expect("dependency plan node")
                            .dependencies
                            .insert(previous_key);
                    }
                }
                previous = Some(dependency_key);
            }
        }

        self.visiting.pop();
        Ok(key)
    }
}

fn depends_on(nodes: &BTreeMap<String, PlanNode>, from: &str, target: &str) -> bool {
    let Some(node) = nodes.get(from) else {
        return false;
    };
    node.dependencies
        .iter()
        .any(|dependency| dependency == target || depends_on(nodes, dependency, target))
}

fn parse_need(raw: &str) -> Result<Vec<String>, String> {
    let words: Vec<String> = raw
        .split_ascii_whitespace()
        .map(ToString::to_string)
        .collect();
    if words.is_empty() {
        Err(
            "A `needs` entry cannot be empty; write a whitespace-separated command path."
                .to_string(),
        )
    } else {
        Ok(words)
    }
}

fn locate_entry(config: &LoadedConfig, target: &CommandEntry) -> Option<(usize, Vec<String>)> {
    for (file_index, file) in config.files.iter().enumerate() {
        let mut path = Vec::new();
        if find_entry(&file.commands, target, &mut path) {
            return Some((file_index, path));
        }
    }
    None
}

fn find_entry(
    commands: &BTreeMap<String, CommandEntry>,
    target: &CommandEntry,
    path: &mut Vec<String>,
) -> bool {
    for (name, entry) in commands {
        path.push(name.clone());
        if std::ptr::eq(entry, target) {
            return true;
        }
        if let Some(children) = entry.subcommands() {
            if find_entry(children, target, path) {
                return true;
            }
        }
        path.pop();
    }
    false
}

fn entry_at_path<'a>(
    file: &'a FileConfig,
    path: &[String],
) -> Option<(&'a CommandEntry, Vec<&'a CommandEntry>)> {
    let mut entry = file.commands.get(path.first()?)?;
    let mut chain = vec![entry];
    for word in &path[1..] {
        entry = entry.subcommands()?.get(word)?;
        chain.push(entry);
    }
    Some((entry, chain))
}

fn inherited_long_running(chain: &[&CommandEntry]) -> bool {
    chain
        .iter()
        .rev()
        .find_map(|entry| entry.spec().and_then(|spec| spec.long_running))
        == Some(true)
}

fn scope_prefix(scope: &FileScope) -> Vec<String> {
    match scope {
        FileScope::Root => vec![],
        FileScope::Namespace { namespace, .. } => vec![namespace.clone()],
        FileScope::Group { group, .. } => vec![group.clone()],
        FileScope::NamespaceGroup {
            namespace, group, ..
        } => vec![namespace.clone(), group.clone()],
    }
}

fn invocation(file: &FileConfig, path: &[String]) -> Vec<String> {
    let mut words = scope_prefix(&file.scope);
    words.extend_from_slice(path);
    words
}

fn display_path(path: &[String]) -> String {
    path.join(" ")
}

fn node_key(file_index: usize, path: &[String]) -> String {
    format!("{file_index}:{}", path.join("\0"))
}

pub(crate) fn execute(plan: &DependencyPlan) -> Result<(), i32> {
    if plan.nodes.len() <= 1 {
        return Ok(());
    }
    let jobs = jobs().map_err(|message| {
        eprintln!("[fire] {message}");
        1
    })?;
    let executable = std::env::current_exe().map_err(|error| {
        eprintln!("[fire] Cannot locate the fire executable: {error}");
        1
    })?;
    let mut completed = BTreeSet::new();
    let mut running: BTreeMap<String, Running> = BTreeMap::new();
    let (output_tx, output_rx) = mpsc::channel();

    loop {
        let ready: Vec<String> = plan
            .nodes
            .iter()
            .filter(|(key, node)| {
                *key != &plan.root
                    && !completed.contains(*key)
                    && !running.contains_key(*key)
                    && node.dependencies.iter().all(|dep| completed.contains(dep))
            })
            .map(|(key, _)| key.clone())
            .take(jobs.saturating_sub(running.len()))
            .collect();

        for key in ready {
            let node = &plan.nodes[&key];
            match spawn_dependency(&executable, node, plan.prefixed_output, output_tx.clone()) {
                Ok(child) => {
                    running.insert(key, child);
                }
                Err(error) => {
                    eprintln!("[fire] Cannot start dependency `{}`: {error}", node.label);
                    cancel_all(&mut running);
                    return Err(1);
                }
            }
        }

        drain_output(&output_rx);
        let mut finished = Vec::new();
        for (key, running_child) in &mut running {
            if let Ok(Some(status)) = running_child.child.try_wait() {
                finished.push((key.clone(), status.code().unwrap_or(1)));
            }
        }
        for (key, code) in finished {
            if let Some(finished_child) = running.remove(&key) {
                for reader in finished_child.readers {
                    let _ = reader.join();
                }
            }
            drain_output(&output_rx);
            if code != 0 {
                eprintln!(
                    "[fire] Dependency `{}` failed with exit code {code}.",
                    plan.nodes[&key].label
                );
                cancel_all(&mut running);
                drain_output(&output_rx);
                return Err(code);
            }
            completed.insert(key);
        }

        if plan.nodes[&plan.root]
            .dependencies
            .iter()
            .all(|key| completed.contains(key))
            && running.is_empty()
        {
            drain_output(&output_rx);
            return Ok(());
        }
        // Completion can make another sequential dependency ready on the next iteration.
        thread::sleep(Duration::from_millis(10));
    }
}

struct Running {
    child: Child,
    readers: Vec<thread::JoinHandle<()>>,
}

enum OutputLine {
    Stdout(String, String),
    Stderr(String, String),
}

fn spawn_dependency(
    executable: &std::path::Path,
    node: &PlanNode,
    prefixed: bool,
    output_tx: mpsc::Sender<OutputLine>,
) -> std::io::Result<Running> {
    let mut command = Command::new(executable);
    command
        .args(&node.invocation)
        .env("FIRE_SKIP_NEEDS", "1")
        .env("FIRE_SESSIONS", "off");
    if prefixed {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
    }
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn()?;
    let mut readers = Vec::new();
    if prefixed {
        if let Some(stdout) = child.stdout.take() {
            readers.push(pipe_lines(
                stdout,
                node.label.clone(),
                true,
                output_tx.clone(),
            ));
        }
        if let Some(stderr) = child.stderr.take() {
            readers.push(pipe_lines(stderr, node.label.clone(), false, output_tx));
        }
    }
    Ok(Running { child, readers })
}

fn pipe_lines<R: std::io::Read + Send + 'static>(
    reader: R,
    label: String,
    stdout: bool,
    tx: mpsc::Sender<OutputLine>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            let event = if stdout {
                OutputLine::Stdout(label.clone(), line)
            } else {
                OutputLine::Stderr(label.clone(), line)
            };
            let _ = tx.send(event);
        }
    })
}

fn drain_output(rx: &mpsc::Receiver<OutputLine>) {
    while let Ok(line) = rx.try_recv() {
        match line {
            OutputLine::Stdout(label, line) => println!("[{label}] {line}"),
            OutputLine::Stderr(label, line) => eprintln!("[{label}] {line}"),
        }
    }
}

fn cancel_all(running: &mut BTreeMap<String, Running>) {
    for child in running.values_mut() {
        unsafe {
            libc::killpg(child.child.id() as i32, libc::SIGTERM);
        }
    }
    let deadline = Instant::now() + Duration::from_millis(250);
    while Instant::now() < deadline
        && running
            .values_mut()
            .any(|child| child.child.try_wait().ok().flatten().is_none())
    {
        thread::sleep(Duration::from_millis(10));
    }
    for child in running.values_mut() {
        if child.child.try_wait().ok().flatten().is_none() {
            unsafe {
                libc::killpg(child.child.id() as i32, libc::SIGKILL);
            }
        }
        let _ = child.child.wait();
    }
    let children = std::mem::take(running);
    for (_, child) in children {
        for reader in child.readers {
            let _ = reader.join();
        }
    }
}

fn jobs() -> Result<usize, String> {
    match std::env::var("FIRE_JOBS") {
        Ok(raw) => raw
            .parse::<usize>()
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| format!("FIRE_JOBS must be a positive integer, got `{raw}`.")),
        Err(_) => Ok(thread::available_parallelism().map_or(1, usize::from)),
    }
}
