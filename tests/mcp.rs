//! MCP behavior through the real stdio server, including schemas and structured results.

mod common;

use std::{
    fs,
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, ChildStdout, Stdio},
};

use common::{eventually, Project};
use serde_json::{json, Value};

struct McpClient {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    next_id: u64,
}

#[test]
fn list_sessions_filters_by_cwd_unless_all_is_requested() {
    let mut project = Project::new(
        "mcp-session-filter",
        r#"
namespace: local
commands:
  server:
    long_running: true
    exec: sleep 30
"#,
    );
    let other = project.dir.join("other-project");
    fs::create_dir(&other).expect("create other project");
    fs::write(
        other.join("fire.yml"),
        r#"
namespace: other
commands:
  server:
    long_running: true
    exec: sleep 30
"#,
    )
    .expect("write other project config");
    for directory in [&project.dir, &other] {
        let status = std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(directory)
            .status()
            .expect("initialize git project");
        assert!(
            status.success(),
            "git init failed in {}",
            directory.display()
        );
    }
    let mut mcp = McpClient::start(&project);

    let local = mcp.call(
        "start_session",
        json!({ "command": ["server"], "note": "local server" }),
    );
    let local_id = local["structuredContent"]["session_id"]
        .as_str()
        .expect("local session id")
        .to_string();
    project.session = Some(local_id.clone());
    let nested = project.dir.join("nested");
    fs::create_dir(&nested).expect("create nested project directory");
    let nested_session = mcp.call(
        "start_session",
        json!({
            "command": ["server"],
            "note": "nested server",
            "cwd": nested.display().to_string()
        }),
    );
    let nested_id = nested_session["structuredContent"]["session_id"]
        .as_str()
        .expect("nested session id")
        .to_string();
    let elsewhere = mcp.call(
        "start_session",
        json!({
            "command": ["server"],
            "note": "other server",
            "cwd": other.display().to_string()
        }),
    );
    let other_id = elsewhere["structuredContent"]["session_id"]
        .as_str()
        .expect("other session id")
        .to_string();

    let default = text_json(&mcp.call("list_sessions", json!({})));
    assert!(default
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["id"] == local_id));
    assert!(!default
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["id"] == other_id));
    assert!(default
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["id"] == nested_id));

    let nested_project = text_json(&mcp.call(
        "list_sessions",
        json!({ "cwd": nested.display().to_string() }),
    ));
    assert!(nested_project
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["id"] == local_id));
    assert!(nested_project
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["id"] == nested_id));

    let selected = text_json(&mcp.call("list_sessions", json!({ "cwd": "other-project" })));
    assert!(!selected
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["id"] == local_id));
    assert!(selected
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["id"] == other_id));

    let all = text_json(&mcp.call("list_sessions", json!({ "cwd": "not-used", "all": true })));
    assert!(all.as_array().unwrap().iter().any(|s| s["id"] == local_id));
    assert!(all.as_array().unwrap().iter().any(|s| s["id"] == other_id));

    let stopped = mcp.call("stop_session", json!({ "id": other_id }));
    assert_eq!(stopped["structuredContent"]["ok"], true);
    let stopped = mcp.call("stop_session", json!({ "id": nested_id }));
    assert_eq!(stopped["structuredContent"]["ok"], true);
}

impl McpClient {
    fn start(project: &Project) -> Self {
        Self::start_in(project, &project.dir)
    }

    fn start_in(project: &Project, cwd: &std::path::Path) -> Self {
        let mut child = project
            .fire(&["cli", "mcp"])
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("start MCP server");
        let input = child.stdin.take().expect("MCP stdin");
        let output = BufReader::new(child.stdout.take().expect("MCP stdout"));
        let mut client = Self {
            child,
            input,
            output,
            next_id: 1,
        };
        let init = client.request(
            "initialize",
            json!({
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": { "name": "fire-test", "version": "1" }
            }),
        );
        assert_eq!(init["serverInfo"]["name"], "fire");
        assert_eq!(init["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
        client.send(json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }));
        client
    }

    fn send(&mut self, message: Value) {
        serde_json::to_writer(&mut self.input, &message).expect("write MCP request");
        self.input.write_all(b"\n").expect("terminate MCP request");
        self.input.flush().expect("flush MCP request");
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        }));

        let mut line = String::new();
        self.output.read_line(&mut line).expect("read MCP response");
        let response: Value = serde_json::from_str(&line).expect("valid MCP response");
        assert_eq!(response["id"], id, "unexpected MCP response: {response}");
        assert!(response.get("error").is_none(), "MCP error: {response}");
        response["result"].clone()
    }

    fn call(&mut self, name: &str, arguments: Value) -> Value {
        self.request(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        )
    }
}

#[test]
fn command_tools_resolve_and_run_local_commands_from_cwd() {
    let project = Project::new(
        "mcp-command-cwd",
        r#"
namespace: local
commands:
  where:
    placeholder: "@{n}"
    exec: printf 'cwd=%s\narg=%s\n' "$PWD" @1
"#,
    );
    let server_dir = project.dir.join("home");
    let mut mcp = McpClient::start_in(&project, &server_dir);

    let server_commands = text_json(&mcp.call("search_commands", json!({})));
    assert!(
        server_commands
            .as_array()
            .expect("command matches")
            .is_empty(),
        "commands without cwd should resolve from the MCP server directory: {server_commands}"
    );

    let cwd = project.dir.display().to_string();
    let project_commands =
        text_json(&mcp.call("search_commands", json!({ "query": "where", "cwd": cwd })));
    assert_eq!(
        project_commands.as_array().expect("command matches").len(),
        1
    );
    assert_eq!(project_commands[0]["path"], "fire local where");
    assert_eq!(project_commands[0]["source"], "local");

    let argument = "  exact argument; $HOME  ";
    let result = mcp.call(
        "run_command",
        json!({
            "path": "fire local where",
            "args": [argument],
            "cwd": project.dir.display().to_string()
        }),
    );
    let structured = &result["structuredContent"];
    assert_eq!(structured["exit_code"], 0, "result: {result}");
    assert_eq!(
        structured["stdout"],
        format!(
            "cwd={}\narg={argument}\n",
            project
                .dir
                .canonicalize()
                .expect("canonical project cwd")
                .display()
        )
    );
}

#[test]
fn init_project_creates_a_discoverable_config_and_never_overwrites() {
    let project = Project::new("mcp-init", "");
    fs::remove_file(project.dir.join("fire.yml")).unwrap();
    let server_dir = project.dir.join("home");
    let target = project.dir.join("new-project");
    fs::create_dir(&target).unwrap();
    let mut mcp = McpClient::start_in(&project, &server_dir);

    let initialized = mcp.call(
        "init_project",
        json!({
            "cwd": target.display().to_string(),
            "file": "backend.yml",
            "namespace": "exact:namespace",
            "group": "api ops"
        }),
    );
    let structured = &initialized["structuredContent"];
    assert_eq!(structured["ok"], true, "{initialized}");
    let expected = target
        .canonicalize()
        .expect("canonical target")
        .join("backend.fire.yml");
    assert_eq!(structured["path"], expected.display().to_string());
    let yaml = fs::read_to_string(&expected).expect("initialized config");
    assert!(yaml.contains("prefix: \"exact:namespace\""), "{yaml}");
    assert!(yaml.contains("group: \"api ops\""), "{yaml}");

    let found = text_json(&mcp.call(
        "search_commands",
        json!({ "cwd": target.display().to_string() }),
    ));
    assert_eq!(found[0]["path"], "fire exact:namespace api ops example");

    fs::write(&expected, "sentinel\n").unwrap();
    let refused = mcp.call(
        "init_project",
        json!({
            "cwd": target.display().to_string(),
            "file": "backend.yml"
        }),
    );
    assert_eq!(refused["structuredContent"]["ok"], false, "{refused}");
    assert_eq!(
        refused["structuredContent"]["path"],
        expected.display().to_string()
    );
    assert!(refused["structuredContent"]["error"]
        .as_str()
        .unwrap()
        .contains("already exists"));
    assert_eq!(fs::read_to_string(expected).unwrap(), "sentinel\n");
}

impl Drop for McpClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn text_json(result: &Value) -> Value {
    let text = result["content"][0]["text"]
        .as_str()
        .expect("text tool result");
    serde_json::from_str(text).expect("JSON tool text")
}

#[test]
fn discovery_and_session_lifecycle_match_the_mcp_contract() {
    let mut project = Project::new(
        "mcp-contract",
        r#"
commands:
  prepare:
    exec: echo prepare
  build:
    needs: [prepare]
    placeholder: "@{n}"
    exec: echo build @1
  server:
    long_running: true
    exec: sleep 30
"#,
    );
    let mut mcp = McpClient::start(&project);

    let tools = mcp.request("tools/list", json!({}));
    let tools = tools["tools"].as_array().expect("tools array");
    assert!(tools.iter().all(|tool| tool["name"] != "list_commands"));
    let get_docs = tools
        .iter()
        .find(|tool| tool["name"] == "get_docs")
        .expect("get_docs tool");
    let docs_description = get_docs["description"].as_str().unwrap();
    assert!(docs_description.contains("`commands-and-args`"));
    assert!(docs_description.contains("`schema`"));
    let topics = get_docs["inputSchema"]["$defs"]["DocsTopic"]["enum"]
        .as_array()
        .expect("topic enum");
    assert!(topics.iter().any(|topic| topic == "overview"));
    assert!(topics.iter().any(|topic| topic == "schema"));
    let init_project = tools
        .iter()
        .find(|tool| tool["name"] == "init_project")
        .expect("init_project tool");
    assert!(init_project["inputSchema"]["required"]
        .as_array()
        .expect("required fields")
        .iter()
        .any(|field| field == "cwd"));
    assert!(init_project["outputSchema"]["properties"]
        .get("path")
        .is_some());
    let start = tools
        .iter()
        .find(|tool| tool["name"] == "start_session")
        .expect("start_session tool");
    assert!(start["outputSchema"]["properties"]
        .get("session_id")
        .is_some());
    let signal = tools
        .iter()
        .find(|tool| tool["name"] == "signal_session")
        .expect("signal_session tool");
    assert!(signal["inputSchema"]["required"]
        .as_array()
        .expect("required fields")
        .iter()
        .any(|field| field == "signal"));

    let docs_index = mcp.call("get_docs", json!({}));
    let docs_index = docs_index["content"][0]["text"].as_str().unwrap();
    assert!(docs_index.contains("overview\t"), "{docs_index}");
    assert!(docs_index.contains("schema\t"), "{docs_index}");
    let overview = mcp.call("get_docs", json!({ "topic": "overview" }));
    assert_eq!(
        overview["content"][0]["text"],
        include_str!("../docs/overview.md")
    );
    let schema = mcp.call("get_docs", json!({ "topic": "schema" }));
    assert_eq!(
        schema["content"][0]["text"],
        include_str!("../schemas/fire.schema.json")
    );

    let found = text_json(&mcp.call("search_commands", json!({})));
    assert_eq!(found.as_array().expect("command matches").len(), 3);
    let build = found
        .as_array()
        .unwrap()
        .iter()
        .find(|command| {
            command["path"]
                .as_str()
                .is_some_and(|path| path.ends_with(" build"))
        })
        .unwrap_or_else(|| panic!("build command in {found}"));
    assert_eq!(build["source"], "local");
    assert_eq!(build["placeholder"], "@{n}");
    assert_eq!(build["needs"], json!(["prepare"]));
    assert!(build.get("exec").is_none());
    assert!(build.get("eval").is_none());

    let found = text_json(&mcp.call(
        "search_commands",
        json!({ "query": "build", "include_exec": true }),
    ));
    assert_eq!(found[0]["exec"][0], "echo build @1");

    let note = "  exact agent note  ";
    let started = mcp.call(
        "start_session",
        json!({ "command": ["server"], "note": note }),
    );
    assert_eq!(started["isError"], false);
    let structured = &started["structuredContent"];
    assert_eq!(structured["ok"], true);
    let id = structured["session_id"]
        .as_str()
        .expect("structured session id")
        .to_string();
    assert_eq!(structured["stdout"], id);
    project.session = Some(id.clone());

    let sessions = text_json(&mcp.call("list_sessions", json!({})));
    let session = sessions
        .as_array()
        .unwrap()
        .iter()
        .find(|session| session["id"] == id)
        .expect("started session");
    assert_eq!(session["note"], note);

    let signalled = mcp.call("signal_session", json!({ "id": id, "signal": "CONT" }));
    assert_eq!(signalled["structuredContent"]["ok"], true);

    let stopped = mcp.call("stop_session", json!({ "id": id }));
    assert_eq!(stopped["structuredContent"]["ok"], true);
    eventually(
        || !project.cli(&["ps"]).1.contains(&id),
        || format!("session still running: {:?}", project.cli(&["ps", "-a"])),
    );

    let stopped_again = mcp.call("stop_session", json!({ "id": id }));
    assert_eq!(stopped_again["structuredContent"]["ok"], false);
    assert!(project.cli(&["ps", "-a"]).1.contains(&id));

    let forgotten = mcp.call("forget_session", json!({ "id": id }));
    assert_eq!(forgotten["structuredContent"]["ok"], true);
    assert!(!project.cli(&["ps", "-a"]).1.contains(&id));
    project.session = None;
}

#[test]
fn run_command_returns_structured_output_and_rejects_ineligible_commands() {
    let project = Project::new(
        "mcp-run-command",
        r#"
namespace: firetest
commands:
  check:
    placeholder: "{{n}}"
    exec: printf 'stdout:%s\n' {1}; printf 'stderr:%s\n' {2} >&2; exit 7
  server:
    long_running: true
    exec: sleep 30
  deploy:
    agents: false
    exec: echo deployed
  group:
    commands:
      child: echo child
  large:
    exec: awk 'BEGIN { for (i = 0; i < 70000; i++) printf "x" }'
"#,
    );
    let mut mcp = McpClient::start(&project);

    let found = text_json(&mcp.call("search_commands", json!({})));
    assert_eq!(
        found.as_array().expect("commands").len(),
        5,
        "found: {found}"
    );

    let tools = mcp.request("tools/list", json!({}));
    let run = tools["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .find(|tool| tool["name"] == "run_command")
        .expect("run_command tool");
    assert!(run["inputSchema"]["required"]
        .as_array()
        .expect("required fields")
        .iter()
        .any(|field| field == "path"));
    for field in [
        "exit_code",
        "stdout",
        "stderr",
        "timed_out",
        "stdout_truncated",
        "stderr_truncated",
    ] {
        assert!(
            run["outputSchema"]["properties"].get(field).is_some(),
            "missing output field `{field}`"
        );
    }

    let result = mcp.call(
        "run_command",
        json!({
            "path": "fire firetest check",
            "args": ["hello world", "error text"]
        }),
    );
    assert_eq!(result["isError"], false);
    let structured = &result["structuredContent"];
    assert_eq!(structured["exit_code"], 7, "result: {result}");
    assert_eq!(structured["timed_out"], false);
    assert_eq!(structured["stdout_truncated"], false);
    assert_eq!(structured["stderr_truncated"], false);
    assert_eq!(structured["stdout"], "stdout:hello world\n");
    assert!(structured["stderr"]
        .as_str()
        .expect("stderr string")
        .contains("stderr:error text\n"));

    let long_running = mcp.call("run_command", json!({ "path": "fire firetest server" }));
    assert_eq!(long_running["structuredContent"]["exit_code"], Value::Null);
    assert!(long_running["structuredContent"]["stderr"]
        .as_str()
        .expect("stderr string")
        .contains("start_session"));

    let restricted = mcp.call("run_command", json!({ "path": "fire firetest deploy" }));
    assert_eq!(restricted["structuredContent"]["exit_code"], Value::Null);
    assert!(restricted["structuredContent"]["stderr"]
        .as_str()
        .expect("stderr string")
        .contains("agents: false"));

    let non_runnable = mcp.call("run_command", json!({ "path": "fire firetest group" }));
    assert_eq!(non_runnable["structuredContent"]["exit_code"], Value::Null);
    assert!(non_runnable["structuredContent"]["stderr"]
        .as_str()
        .expect("stderr string")
        .contains("no executable action"));

    let unknown = mcp.call("run_command", json!({ "path": "fire firetest missing" }));
    assert_eq!(unknown["structuredContent"]["exit_code"], Value::Null);
    assert!(unknown["structuredContent"]["stderr"]
        .as_str()
        .expect("stderr string")
        .contains("unknown configured command path"));

    let large = mcp.call("run_command", json!({ "path": "fire firetest large" }));
    assert_eq!(large["structuredContent"]["exit_code"], 0);
    assert_eq!(
        large["structuredContent"]["stdout"]
            .as_str()
            .expect("stdout string")
            .len(),
        64 * 1024
    );
    assert_eq!(large["structuredContent"]["stdout_truncated"], true);
}

#[test]
fn run_command_uses_the_cli_resolution_when_config_paths_overlap() {
    let project = Project::new(
        "mcp-run-command-resolution",
        r#"
namespace: overlap
commands:
  task:
    commands:
      child: echo child
"#,
    );
    fs::write(
        project.dir.join("z.fire.yml"),
        r#"
namespace: overlap
group: null
commands:
  task: printf resolved
"#,
    )
    .expect("write overlapping config");
    let mut mcp = McpClient::start(&project);

    let result = mcp.call("run_command", json!({ "path": "fire overlap task" }));

    assert_eq!(result["structuredContent"]["exit_code"], 0, "{result}");
    assert_eq!(result["structuredContent"]["stdout"], "resolved");
}

#[test]
fn global_tools_manage_the_registry_like_the_cli() {
    let project = Project::new("mcp-global", "commands: {}\n");
    let api = project.dir.join("api");
    fs::create_dir_all(&api).unwrap();
    fs::write(
        api.join("fire.yaml"),
        "namespace: api\ncommands:\n  build: echo built\n",
    )
    .unwrap();
    let api = api.canonicalize().unwrap();
    let old = project.dir.join("old");
    fs::create_dir_all(&old).unwrap();
    let old = old.canonicalize().unwrap();
    let api_str = api.to_str().unwrap();
    let old_str = old.to_str().unwrap();

    let mut mcp = McpClient::start(&project);
    let tools = mcp.request("tools/list", json!({}));
    let tools = tools["tools"].as_array().expect("tools array");
    for name in [
        "add_global",
        "remove_global",
        "prune_global",
        "merge_global",
        "unmerge_global",
    ] {
        let tool = tools
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap_or_else(|| panic!("{name} tool"));
        assert!(
            tool["description"]
                .as_str()
                .unwrap()
                .contains("only when the user asks"),
            "{name}"
        );
    }

    let relative = mcp.call("add_global", json!({ "path": "api" }));
    assert_eq!(relative["structuredContent"]["ok"], false);
    assert!(relative["structuredContent"]["error"]
        .as_str()
        .unwrap()
        .contains("not an absolute path"));

    let added = mcp.call("add_global", json!({ "path": api_str }));
    let added = &added["structuredContent"];
    assert_eq!(added["ok"], true, "{added}");
    assert_eq!(added["registered"], true);
    assert_eq!(added["already_registered"], false);
    assert_eq!(added["has_config"], true);
    assert_eq!(added["path"], api_str);
    let again = mcp.call("add_global", json!({ "path": api_str }));
    assert_eq!(again["structuredContent"]["already_registered"], true);
    let empty = mcp.call("add_global", json!({ "path": old_str }));
    assert_eq!(empty["structuredContent"]["has_config"], false);

    let merged = mcp.call("merge_global", json!({ "namespace": "api" }));
    assert_eq!(merged["structuredContent"]["merged"], true);
    assert_eq!(merged["structuredContent"]["already_merged"], false);
    let merged_again = mcp.call("merge_global", json!({ "namespace": "api" }));
    assert_eq!(merged_again["structuredContent"]["already_merged"], true);
    let unknown = mcp.call("merge_global", json!({ "namespace": "nope" }));
    assert_eq!(unknown["structuredContent"]["ok"], false);

    let listing = mcp.call("list_global", json!({}));
    let listing = &listing["structuredContent"];
    assert_eq!(listing["merged"], json!(["api"]));
    let dirs = listing["directories"].as_array().unwrap();
    let api_entry = dirs.iter().find(|dir| dir["path"] == api_str).unwrap();
    assert_eq!(api_entry["exists"], true);
    assert_eq!(api_entry["namespaces"][0]["namespace"], "api");
    assert_eq!(api_entry["namespaces"][0]["merged"], true);
    assert_eq!(
        api_entry["namespaces"][0]["files"],
        json!([api.join("fire.yaml").to_str().unwrap()])
    );

    // The CLI sees the same registry.
    let (code, cli_list, stderr) = project.cli(&["global", "list"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(cli_list.contains("api (merged)"), "{cli_list}");

    let unmerged = mcp.call("unmerge_global", json!({ "namespace": "api" }));
    assert_eq!(unmerged["structuredContent"]["merged"], false);
    let not_merged = mcp.call("unmerge_global", json!({ "namespace": "api" }));
    assert_eq!(not_merged["structuredContent"]["ok"], false);

    let both = mcp.call(
        "remove_global",
        json!({ "namespace": "api", "path": api_str }),
    );
    assert_eq!(both["structuredContent"]["ok"], false);
    let removed = mcp.call("remove_global", json!({ "namespace": "api" }));
    assert_eq!(removed["structuredContent"]["removed"], true);
    assert_eq!(removed["structuredContent"]["path"], api_str);
    assert!(api.join("fire.yaml").exists(), "files are never deleted");

    fs::remove_dir_all(&old).unwrap();
    let pruned = mcp.call("prune_global", json!({}));
    assert_eq!(pruned["structuredContent"]["pruned"], json!([old_str]));
    let listing = mcp.call("list_global", json!({}));
    assert_eq!(listing["structuredContent"]["directories"], json!([]));
}
