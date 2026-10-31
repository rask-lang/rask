// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! `rask mcp` driven over stdio by a scripted client, no model: every tool
//! answers with what its CLI command prints.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{json, Value};

fn rask_binary() -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }
    path.push("rask");
    path
}

fn runtime_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("runtime")
}

/// Send `messages` to one `rask mcp` session in `dir`; its replies, in order.
fn session(dir: &Path, messages: &[Value]) -> Vec<Value> {
    let mut child = Command::new(rask_binary())
        .arg("mcp")
        .current_dir(dir)
        .env("RASK_RUNTIME_DIR", runtime_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let stdin = child.stdin.as_mut().unwrap();
        for m in messages {
            writeln!(stdin, "{m}").unwrap();
        }
    }
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).lines().map(|l| serde_json::from_str(l).unwrap()).collect()
}

/// What `rask <args>` prints in `dir`, stdout then stderr, as the server joins them.
fn cli(dir: &Path, args: &[&str]) -> String {
    let out = Command::new(rask_binary())
        .args(args)
        .current_dir(dir)
        .env("RASK_RUNTIME_DIR", runtime_dir())
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !stderr.trim().is_empty() {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&stderr);
    }
    text
}

fn call(id: i64, name: &str, arguments: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": name, "arguments": arguments } })
}

fn text(reply: &Value) -> &str {
    reply["result"]["content"][0]["text"].as_str().unwrap()
}

/// A copy of a claims fixture, so the build it runs writes nowhere in the repo.
fn fixture_copy(name: &str) -> PathBuf {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claims").join(name);
    let dir = std::env::temp_dir().join(format!("rask_mcp_{}", std::process::id())).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for entry in std::fs::read_dir(src).unwrap().flatten() {
        if entry.path().is_file() {
            std::fs::copy(entry.path(), dir.join(entry.file_name())).unwrap();
        }
    }
    dir
}

#[test]
fn every_tool_answers_with_what_its_command_prints() {
    let dir = fixture_copy("strict");
    // Beside the package, not in it, so the package itself still builds.
    std::fs::write(dir.parent().unwrap().join("broken.rk"), "func main() {\n    let x: i64 = \"s\"\n}\n").unwrap();

    let replies = session(
        &dir,
        &[
            json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "test", "version": "0" } } }),
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
            call(3, "check", json!({ "path": "../broken.rk" })),
            call(4, "build_claims", json!({ "path": "." })),
            call(5, "explain", json!({ "code": "E0950" })),
            call(6, "api", json!({ "path": "lib.rk" })),
            call(7, "briefing", json!({})),
        ],
    );
    // The notification got no reply.
    assert_eq!(replies.len(), 7, "{replies:?}");
    assert_eq!(replies[0]["result"]["serverInfo"]["name"], "rask");
    let names: Vec<&str> = replies[1]["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["check", "build_claims", "explain", "api", "briefing"]);

    // A file with a type error: the call worked, the check didn't.
    assert_eq!(replies[2]["result"]["isError"], false);
    assert_eq!(replies[2]["result"]["structuredContent"]["exit_code"], 1);
    assert!(text(&replies[2]).contains("\"code\": \"E0308\""), "{}", text(&replies[2]));
    assert_eq!(text(&replies[2]), cli(&dir, &["check", "--json", "../broken.rk"]));
    assert!(text(&replies[3]).contains("\"name\":\"small_pass_through\""), "{}", text(&replies[3]));
    assert_eq!(text(&replies[4]), cli(&dir, &["explain", "E0950"]));
    assert_eq!(text(&replies[5]), cli(&dir, &["api", "--json", "lib.rk"]));
    assert_eq!(text(&replies[6]), cli(&dir, &["help", "agents"]));
}

#[test]
fn init_writes_agents_md_only_when_asked() {
    let base = std::env::temp_dir().join(format!("rask_init_agents_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();

    let plain = Command::new(rask_binary()).args(["init", "plain"]).current_dir(&base).output().unwrap();
    assert!(plain.status.success(), "{}", String::from_utf8_lossy(&plain.stderr));
    assert!(!base.join("plain/AGENTS.md").exists());

    let with = Command::new(rask_binary()).args(["init", "with", "--agents"]).current_dir(&base).output().unwrap();
    assert!(with.status.success(), "{}", String::from_utf8_lossy(&with.stderr));
    let agents = std::fs::read_to_string(base.join("with/AGENTS.md")).unwrap();
    assert!(agents.contains("rask help agents"), "{agents}");
    assert_eq!(agents, cli(&base, &["help", "agents", "--snippet"]));
}
