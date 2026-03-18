//! Integration test: verify session multiplexing in mock mode.
//!
//! Spawns `julep --mock --max-sessions 4 --json` as a subprocess,
//! sends interleaved messages with different session IDs, and verifies
//! that responses come back tagged with the correct session.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

fn send(stdin: &mut impl Write, msg: &serde_json::Value) {
    let line = serde_json::to_string(msg).unwrap();
    writeln!(stdin, "{line}").unwrap();
    stdin.flush().unwrap();
}

fn recv(reader: &mut impl BufRead) -> serde_json::Value {
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    serde_json::from_str(line.trim()).unwrap()
}

fn julep_binary() -> String {
    // The integration test binary is in target/debug/deps. The julep
    // binary is in target/debug.
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // remove test binary name
    path.pop(); // remove deps/
    path.push("julep");
    path.to_string_lossy().to_string()
}

#[test]
fn hello_message_has_empty_session() {
    let mut child = Command::new(julep_binary())
        .args(["--mock", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn julep");

    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    // Send initial settings to trigger hello.
    send(
        &mut stdin,
        &serde_json::json!({"session": "s1", "type": "settings", "settings": {}}),
    );

    let hello = recv(&mut stdout);
    assert_eq!(hello["type"], "hello");
    assert_eq!(hello["session"], "");

    drop(stdin);
    child.wait().unwrap();
}

#[test]
fn single_session_echoes_session_id() {
    let mut child = Command::new(julep_binary())
        .args(["--mock", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn julep");

    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    send(
        &mut stdin,
        &serde_json::json!({"session": "test_1", "type": "settings", "settings": {}}),
    );
    let _hello = recv(&mut stdout);

    // Send a reset and verify session is echoed.
    send(
        &mut stdin,
        &serde_json::json!({"session": "test_1", "type": "reset", "id": "r1"}),
    );
    let resp = recv(&mut stdout);
    assert_eq!(resp["type"], "reset_response");
    assert_eq!(resp["session"], "test_1");
    assert_eq!(resp["id"], "r1");

    drop(stdin);
    child.wait().unwrap();
}

#[test]
fn multiplexed_sessions_are_isolated() {
    let mut child = Command::new(julep_binary())
        .args(["--mock", "--max-sessions", "4", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn julep");

    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    // Consume hello.
    send(
        &mut stdin,
        &serde_json::json!({"session": "s1", "type": "settings", "settings": {}}),
    );
    let _hello = recv(&mut stdout);

    // Send snapshots to two different sessions with different trees.
    send(
        &mut stdin,
        &serde_json::json!({
            "session": "s1",
            "type": "snapshot",
            "tree": {"id": "root", "type": "text", "props": {"content": "session one"}, "children": []}
        }),
    );
    send(
        &mut stdin,
        &serde_json::json!({
            "session": "s2",
            "type": "snapshot",
            "tree": {"id": "root", "type": "text", "props": {"content": "session two"}, "children": []}
        }),
    );

    // Query tree from each session -- they should have different content.
    send(
        &mut stdin,
        &serde_json::json!({
            "session": "s1",
            "type": "query",
            "id": "q1",
            "target": "tree",
            "selector": {}
        }),
    );
    send(
        &mut stdin,
        &serde_json::json!({
            "session": "s2",
            "type": "query",
            "id": "q2",
            "target": "tree",
            "selector": {}
        }),
    );

    // Collect both responses (order may vary due to threading).
    let r1 = recv(&mut stdout);
    let r2 = recv(&mut stdout);

    let mut responses: std::collections::HashMap<String, serde_json::Value> =
        std::collections::HashMap::new();
    responses.insert(r1["session"].as_str().unwrap().to_string(), r1);
    responses.insert(r2["session"].as_str().unwrap().to_string(), r2);

    let s1_tree = &responses["s1"];
    assert_eq!(s1_tree["type"], "query_response");
    assert_eq!(s1_tree["id"], "q1");
    assert_eq!(s1_tree["data"]["props"]["content"], "session one");

    let s2_tree = &responses["s2"];
    assert_eq!(s2_tree["type"], "query_response");
    assert_eq!(s2_tree["id"], "q2");
    assert_eq!(s2_tree["data"]["props"]["content"], "session two");

    drop(stdin);
    child.wait().unwrap();
}

#[test]
fn reset_tears_down_session() {
    let mut child = Command::new(julep_binary())
        .args(["--mock", "--max-sessions", "4", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn julep");

    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    send(
        &mut stdin,
        &serde_json::json!({"session": "s1", "type": "settings", "settings": {}}),
    );
    let _hello = recv(&mut stdout);

    // Create a session, send a tree, reset it.
    send(
        &mut stdin,
        &serde_json::json!({
            "session": "s1",
            "type": "snapshot",
            "tree": {"id": "root", "type": "text", "props": {"content": "before"}, "children": []}
        }),
    );
    send(
        &mut stdin,
        &serde_json::json!({"session": "s1", "type": "reset", "id": "r1"}),
    );

    let reset_resp = recv(&mut stdout);
    assert_eq!(reset_resp["type"], "reset_response");
    assert_eq!(reset_resp["session"], "s1");

    // Reuse the same session ID -- should get a fresh session.
    send(
        &mut stdin,
        &serde_json::json!({
            "session": "s1",
            "type": "query",
            "id": "q1",
            "target": "tree",
            "selector": {}
        }),
    );

    let tree_resp = recv(&mut stdout);
    assert_eq!(tree_resp["session"], "s1");
    // Tree should be null (fresh session, no snapshot sent).
    assert!(tree_resp["data"].is_null());

    drop(stdin);
    child.wait().unwrap();
}
