//! MCP behavior through the real stdio server, including schemas and structured results.

mod common;

use std::{
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

impl McpClient {
    fn start(project: &Project) -> Self {
        let mut child = project
            .fire(&["cli", "mcp"])
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
        client.request(
            "initialize",
            json!({
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": { "name": "fire-test", "version": "1" }
            }),
        );
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
  build:
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

    let found = text_json(&mcp.call("search_commands", json!({})));
    assert_eq!(found.as_array().expect("command matches").len(), 2);
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
