// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! `rask mcp`: the compiler's existing commands, served to an agent harness
//! over the Model Context Protocol (JSON-RPC, one message per line on stdin
//! and stdout).
//!
//! Opt-in: it does nothing until a harness is configured to start it. Each
//! tool runs one `rask` command in a fresh process and answers with what
//! that command printed, so the server can't drift from the CLI and keeps no
//! state between calls. It writes no files of its own: applying a suggested
//! fix is the harness's job, where the user approves edits.

use std::io::{BufRead, Write};
use std::process::Command;

use serde_json::{json, Value};

/// The protocol version answered when the client asks for one this server
/// doesn't know.
const PROTOCOL_VERSION: &str = "2025-06-18";
const KNOWN_VERSIONS: &[&str] = &["2024-11-05", "2025-03-26", "2025-06-18"];

struct Tool {
    name: &'static str,
    description: &'static str,
    /// The one string argument it takes, and what it means.
    arg: Option<(&'static str, &'static str)>,
    /// The `rask` command line, the argument (if any) appended.
    command: &'static [&'static str],
}

const TOOLS: &[Tool] = &[
    Tool {
        name: "check",
        description: "Type-check a Rask file or package without building it. Returns the compiler's \
                      diagnostics as JSON: error code, location, message, and a suggested \
                      replacement when the fix is mechanical.",
        arg: Some(("path", "A .rk file or a package directory")),
        command: &["check", "--json"],
    },
    Tool {
        name: "build_claims",
        description: "Build a package's claims and run them, as `rask build` does. Returns each \
                      claim's verdict as JSON; a false claim comes with the smallest \
                      counterexample found and the assertion that failed. Claims files \
                      (claims.rk and the sources under [claims] in rask.toml) are written by \
                      people: a false claim means the code is wrong, so fix the code, never \
                      the claim.",
        arg: Some(("path", "A package directory")),
        command: &["claims", "--json"],
    },
    Tool {
        name: "explain",
        description: "Explain a Rask error code (`E0308`, `E0950`, ...): what it means, why the \
                      rule exists, and an example with its fix.",
        arg: Some(("code", "An error code, like E0308")),
        command: &["explain"],
    },
    Tool {
        name: "api",
        description: "List the public API of a Rask module or package as JSON: its functions, \
                      structs, enums and their signatures.",
        arg: Some(("path", "A .rk file or a directory")),
        command: &["api", "--json"],
    },
    Tool {
        name: "briefing",
        description: "The Rask language card and how to work in a Rask project: building, \
                      testing, reading diagnostics, and what claims are. Read it before writing \
                      Rask.",
        arg: None,
        command: &["help", "agents"],
    },
];

pub fn cmd_mcp() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = handle(&line) {
            if writeln!(stdout, "{reply}").and_then(|_| stdout.flush()).is_err() {
                break;
            }
        }
    }
}

/// The reply to one message, or `None` for a notification.
pub fn handle(line: &str) -> Option<Value> {
    let msg: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return Some(error(Value::Null, -32700, &format!("not JSON: {e}"))),
    };
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or_default();
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    // A notification has no id and gets no reply.
    let id = id?;
    let result = match method {
        "initialize" => initialize(&params),
        "ping" => json!({}),
        "tools/list" => tools_list(),
        "tools/call" => match tools_call(&params) {
            Ok(r) => r,
            Err(msg) => return Some(error(id, -32602, &msg)),
        },
        other => return Some(error(id, -32601, &format!("no method `{other}`"))),
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn initialize(params: &Value) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str);
    let version = asked.filter(|v| KNOWN_VERSIONS.contains(v)).unwrap_or(PROTOCOL_VERSION);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "rask", "version": env!("CARGO_PKG_VERSION") },
        "instructions": "Rask's compiler. Call `briefing` before writing Rask; `check` after \
                         editing; `build_claims` before calling a change done.",
    })
}

fn tools_list() -> Value {
    let tools: Vec<Value> = TOOLS
        .iter()
        .map(|t| {
            let (properties, required) = match t.arg {
                Some((name, about)) => (
                    json!({ name: { "type": "string", "description": about } }),
                    json!([name]),
                ),
                None => (json!({}), json!([])),
            };
            json!({
                "name": t.name,
                "description": t.description,
                "inputSchema": { "type": "object", "properties": properties, "required": required },
            })
        })
        .collect();
    json!({ "tools": tools })
}

fn tools_call(params: &Value) -> Result<Value, String> {
    let name = params.get("name").and_then(Value::as_str).ok_or("tools/call needs a `name`")?;
    let tool = TOOLS.iter().find(|t| t.name == name).ok_or_else(|| format!("no tool `{name}`"))?;
    let mut args: Vec<String> = tool.command.iter().map(|s| s.to_string()).collect();
    if let Some((arg, _)) = tool.arg {
        let value = params
            .get("arguments")
            .and_then(|a| a.get(arg))
            .and_then(Value::as_str)
            .ok_or_else(|| format!("`{name}` needs a string `{arg}`"))?;
        // An argument is a path or a code, never a flag for the command.
        if value.starts_with('-') {
            return Err(format!("`{arg}` can't start with `-`"));
        }
        args.push(value.to_string());
    }
    Ok(run(&args))
}

/// Run `rask <args>` and answer with what it printed. A command that ran and
/// reported errors is a result, not a failed call; only a command that
/// couldn't run at all is an error.
fn run(args: &[String]) -> Value {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => return tool_error(&format!("can't find the rask binary: {e}")),
    };
    let out = match Command::new(exe).args(args).env("NO_COLOR", "1").output() {
        Ok(out) => out,
        Err(e) => return tool_error(&format!("running rask: {e}")),
    };
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !stderr.trim().is_empty() {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&stderr);
    }
    let code = out.status.code().unwrap_or(-1);
    json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": { "exit_code": code },
        "isError": false,
    })
}

fn tool_error(message: &str) -> Value {
    json!({ "content": [{ "type": "text", "text": message }], "isError": true })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notification_gets_no_reply() {
        assert!(handle(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
    }

    #[test]
    fn initialize_answers_a_version_it_knows() {
        let r = handle(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2099-01-01"}}"#).unwrap();
        assert_eq!(r["result"]["protocolVersion"], PROTOCOL_VERSION);
        let r = handle(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}"#).unwrap();
        assert_eq!(r["result"]["protocolVersion"], "2024-11-05");
    }

    #[test]
    fn every_tool_is_listed_with_its_argument() {
        let r = handle(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#).unwrap();
        let tools = r["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), TOOLS.len());
        let check = tools.iter().find(|t| t["name"] == "check").unwrap();
        assert_eq!(check["inputSchema"]["required"][0], "path");
    }

    #[test]
    fn a_flag_where_a_path_belongs_is_refused() {
        let r = handle(r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"check","arguments":{"path":"--release"}}}"#).unwrap();
        assert_eq!(r["error"]["code"], -32602);
    }

    #[test]
    fn an_unknown_method_is_an_error() {
        let r = handle(r#"{"jsonrpc":"2.0","id":4,"method":"resources/list"}"#).unwrap();
        assert_eq!(r["error"]["code"], -32601);
    }
}
