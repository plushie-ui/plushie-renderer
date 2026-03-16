// test_protocol.rs -- Shared protocol helpers for headless and test modes.
//
// Both --headless and --test modes handle Query/Interact/Reset/SnapshotCapture
// messages from stdin. The logic is identical; only the surrounding event loop
// differs. This module contains the canonical implementations so the two modes
// stay in sync.

use std::io::{self, Write};

use serde_json::Value;

use julep_core::codec::Codec;
use julep_core::engine::Core;
use julep_core::protocol::{InteractResponse, QueryResponse, ResetResponse, TreeNode};

/// Maximum tree search recursion depth (matches MAX_TREE_DEPTH in widgets.rs).
const MAX_SEARCH_DEPTH: usize = 256;

// ---------------------------------------------------------------------------
// Selector
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum Selector {
    Id(String),
    Text(String),
    Role(String),
    Label(String),
    Focused,
}

pub fn parse_selector(selector: &Value) -> Option<Selector> {
    let by = selector.get("by")?.as_str()?;
    match by {
        "focused" => Some(Selector::Focused),
        _ => {
            let value = selector.get("value")?.as_str()?.to_string();
            match by {
                "id" => Some(Selector::Id(value)),
                "text" => Some(Selector::Text(value)),
                "role" => Some(Selector::Role(value)),
                "label" => Some(Selector::Label(value)),
                _ => None,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Key / modifier parsing
// ---------------------------------------------------------------------------

/// Parse key and modifiers from an interact payload.
///
/// Supports two formats:
/// 1. Explicit modifiers map: `{"key": "s", "modifiers": {"ctrl": true, ...}}`
/// 2. Combined key string: `{"key": "ctrl+s"}` -- splits on `+` and extracts
///    modifier prefixes (ctrl/command, shift, alt, logo/super/meta).
pub fn parse_key_and_modifiers(
    payload: Option<&serde_json::Map<String, Value>>,
) -> (String, Value) {
    let empty_map = serde_json::Map::new();
    let map = payload.unwrap_or(&empty_map);

    let raw_key = map
        .get("key")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Explicit modifiers map takes priority
    if let Some(mods) = map.get("modifiers").and_then(|v| v.as_object()) {
        let modifiers = serde_json::json!({
            "shift": mods.get("shift").and_then(|v| v.as_bool()).unwrap_or(false),
            "ctrl": mods.get("ctrl").and_then(|v| v.as_bool()).unwrap_or(false),
            "alt": mods.get("alt").and_then(|v| v.as_bool()).unwrap_or(false),
            "logo": mods.get("logo").and_then(|v| v.as_bool()).unwrap_or(false),
        });
        return (raw_key, modifiers);
    }

    // Parse "ctrl+s" style combined key strings
    let parts: Vec<&str> = raw_key.split('+').collect();
    if parts.len() > 1 {
        let key = parts.last().unwrap().to_string();
        let mut shift = false;
        let mut ctrl = false;
        let mut alt = false;
        let mut logo = false;
        for &part in &parts[..parts.len() - 1] {
            match part {
                "ctrl" | "command" => ctrl = true,
                "shift" => shift = true,
                "alt" => alt = true,
                "logo" | "super" | "meta" => logo = true,
                _ => {}
            }
        }
        let modifiers = serde_json::json!({
            "shift": shift, "ctrl": ctrl, "alt": alt, "logo": logo,
        });
        (key, modifiers)
    } else {
        let modifiers = serde_json::json!({
            "shift": false, "ctrl": false, "alt": false, "logo": false,
        });
        (raw_key, modifiers)
    }
}

// ---------------------------------------------------------------------------
// Wire I/O
// ---------------------------------------------------------------------------

/// Write a serialized response to stdout using the negotiated wire codec.
pub fn emit_wire<T: serde::Serialize>(value: &T) {
    let codec = Codec::get_global();
    match codec.encode(value) {
        Ok(bytes) => {
            let stdout = io::stdout();
            let mut handle = stdout.lock();
            if let Err(e) = handle.write_all(&bytes) {
                if e.kind() == io::ErrorKind::BrokenPipe {
                    log::error!("stdout broken pipe -- shutting down");
                    std::process::exit(0);
                }
                log::error!("stdout write error: {e}");
                return;
            }
            if let Err(e) = handle.flush() {
                if e.kind() == io::ErrorKind::BrokenPipe {
                    log::error!("stdout broken pipe on flush -- shutting down");
                    std::process::exit(0);
                }
                log::error!("stdout flush error: {e}");
            }
        }
        Err(e) => log::error!("encode error: {e}"),
    }
}

// ---------------------------------------------------------------------------
// Tree search helpers
// ---------------------------------------------------------------------------

pub fn find_node_by_id(core: &Core, widget_id: &str) -> Value {
    match core.tree.root() {
        Some(root) => search_by_id(root, widget_id, 0).unwrap_or(Value::Null),
        None => Value::Null,
    }
}

pub fn find_node_by_text(core: &Core, text: &str) -> Value {
    match core.tree.root() {
        Some(root) => search_by_text(root, text, 0).unwrap_or(Value::Null),
        None => Value::Null,
    }
}

fn search_by_id(node: &TreeNode, id: &str, depth: usize) -> Option<Value> {
    if depth > MAX_SEARCH_DEPTH {
        return None;
    }
    if node.id == id {
        return serde_json::to_value(node).ok();
    }
    for child in &node.children {
        if let Some(found) = search_by_id(child, id, depth + 1) {
            return Some(found);
        }
    }
    None
}

pub fn find_node_by_role(core: &Core, role: &str) -> Value {
    match core.tree.root() {
        Some(root) => search_by_role(root, role, 0).unwrap_or(Value::Null),
        None => Value::Null,
    }
}

pub fn find_node_by_label(core: &Core, label: &str) -> Value {
    match core.tree.root() {
        Some(root) => search_by_label(root, label, 0).unwrap_or(Value::Null),
        None => Value::Null,
    }
}

pub fn find_focused_node(core: &Core) -> Value {
    match core.tree.root() {
        Some(root) => search_focused(root, 0).unwrap_or(Value::Null),
        None => Value::Null,
    }
}

fn search_by_role(node: &TreeNode, role: &str, depth: usize) -> Option<Value> {
    if depth > MAX_SEARCH_DEPTH {
        return None;
    }
    // Check explicit a11y.role prop first. When present it takes priority
    // over type_name -- the host explicitly overrode the semantic role.
    if let Some(a11y) = node.props.get("a11y") {
        if let Some(node_role) = a11y.get("role").and_then(|v| v.as_str())
            && node_role == role
        {
            return serde_json::to_value(node).ok();
        }
        // Explicit a11y present -- don't fall through to type_name.
    } else if node.type_name == role {
        // No explicit a11y role -- fall back to widget type_name.
        return serde_json::to_value(node).ok();
    }
    for child in &node.children {
        if let Some(found) = search_by_role(child, role, depth + 1) {
            return Some(found);
        }
    }
    None
}

fn search_by_label(node: &TreeNode, label: &str, depth: usize) -> Option<Value> {
    if depth > MAX_SEARCH_DEPTH {
        return None;
    }
    // Check explicit a11y.label prop
    if let Some(a11y) = node.props.get("a11y")
        && let Some(node_label) = a11y.get("label").and_then(|v| v.as_str())
        && node_label == label
    {
        return serde_json::to_value(node).ok();
    }
    // Also check common text props that serve as implicit labels
    for key in &["label", "content"] {
        if let Some(val) = node.props.get(*key)
            && val.as_str() == Some(label)
        {
            return serde_json::to_value(node).ok();
        }
    }
    for child in &node.children {
        if let Some(found) = search_by_label(child, label, depth + 1) {
            return Some(found);
        }
    }
    None
}

fn search_focused(node: &TreeNode, depth: usize) -> Option<Value> {
    if depth > MAX_SEARCH_DEPTH {
        return None;
    }
    // Check if node has a focused prop or a11y.focused
    if node.props.get("focused").and_then(|v| v.as_bool()) == Some(true) {
        return serde_json::to_value(node).ok();
    }
    if let Some(a11y) = node.props.get("a11y")
        && a11y.get("focused").and_then(|v| v.as_bool()) == Some(true)
    {
        return serde_json::to_value(node).ok();
    }
    for child in &node.children {
        if let Some(found) = search_focused(child, depth + 1) {
            return Some(found);
        }
    }
    None
}

fn search_by_text(node: &TreeNode, text: &str, depth: usize) -> Option<Value> {
    if depth > MAX_SEARCH_DEPTH {
        return None;
    }
    for key in &["content", "label", "value", "placeholder"] {
        if let Some(val) = node.props.get(*key)
            && val.as_str() == Some(text)
        {
            return serde_json::to_value(node).ok();
        }
    }
    for child in &node.children {
        if let Some(found) = search_by_text(child, text, depth + 1) {
            return Some(found);
        }
    }
    None
}

fn find_id_by_role(node: &TreeNode, role: &str, depth: usize) -> Option<String> {
    if depth > MAX_SEARCH_DEPTH {
        return None;
    }
    if let Some(a11y) = node.props.get("a11y") {
        if let Some(node_role) = a11y.get("role").and_then(|v| v.as_str())
            && node_role == role
        {
            return Some(node.id.clone());
        }
        // Explicit a11y present -- skip type_name fallback.
    } else if node.type_name == role {
        return Some(node.id.clone());
    }
    for child in &node.children {
        if let Some(found) = find_id_by_role(child, role, depth + 1) {
            return Some(found);
        }
    }
    None
}

fn find_id_by_label(node: &TreeNode, label: &str, depth: usize) -> Option<String> {
    if depth > MAX_SEARCH_DEPTH {
        return None;
    }
    if let Some(a11y) = node.props.get("a11y")
        && let Some(node_label) = a11y.get("label").and_then(|v| v.as_str())
        && node_label == label
    {
        return Some(node.id.clone());
    }
    for key in &["label", "content"] {
        if let Some(val) = node.props.get(*key)
            && val.as_str() == Some(label)
        {
            return Some(node.id.clone());
        }
    }
    for child in &node.children {
        if let Some(found) = find_id_by_label(child, label, depth + 1) {
            return Some(found);
        }
    }
    None
}

fn find_id_focused(node: &TreeNode, depth: usize) -> Option<String> {
    if depth > MAX_SEARCH_DEPTH {
        return None;
    }
    if node.props.get("focused").and_then(|v| v.as_bool()) == Some(true) {
        return Some(node.id.clone());
    }
    if let Some(a11y) = node.props.get("a11y")
        && a11y.get("focused").and_then(|v| v.as_bool()) == Some(true)
    {
        return Some(node.id.clone());
    }
    for child in &node.children {
        if let Some(found) = find_id_focused(child, depth + 1) {
            return Some(found);
        }
    }
    None
}

fn find_id_by_text(node: &TreeNode, text: &str, depth: usize) -> Option<String> {
    if depth > MAX_SEARCH_DEPTH {
        return None;
    }
    for key in &["content", "label", "value", "placeholder"] {
        if let Some(val) = node.props.get(*key)
            && val.as_str() == Some(text)
        {
            return Some(node.id.clone());
        }
    }
    for child in &node.children {
        if let Some(found) = find_id_by_text(child, text, depth + 1) {
            return Some(found);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Message handlers
// ---------------------------------------------------------------------------

/// Handle a Query message: serialize tree or find a widget by selector.
pub fn handle_query(core: &Core, id: String, target: String, selector: Value) {
    let data = match target.as_str() {
        "tree" => match core.tree.root() {
            Some(root) => serde_json::to_value(root).unwrap_or(Value::Null),
            None => Value::Null,
        },
        "find" => match parse_selector(&selector) {
            Some(Selector::Id(widget_id)) => find_node_by_id(core, &widget_id),
            Some(Selector::Text(text)) => find_node_by_text(core, &text),
            Some(Selector::Role(role)) => find_node_by_role(core, &role),
            Some(Selector::Label(label)) => find_node_by_label(core, &label),
            Some(Selector::Focused) => find_focused_node(core),
            None => Value::Null,
        },
        _ => {
            log::warn!("unknown query target: {target}");
            Value::Null
        }
    };

    emit_wire(&QueryResponse::new(id, target, data));
}

/// Handle an Interact message: resolve widget ID from selector, build
/// synthetic events for the requested action.
pub fn handle_interact(core: &Core, id: String, action: String, selector: Value, payload: Value) {
    let widget_id = match parse_selector(&selector) {
        Some(Selector::Id(wid)) => Some(wid),
        Some(Selector::Text(text)) => core
            .tree
            .root()
            .and_then(|root| find_id_by_text(root, &text, 0)),
        Some(Selector::Role(role)) => core
            .tree
            .root()
            .and_then(|root| find_id_by_role(root, &role, 0)),
        Some(Selector::Label(label)) => core
            .tree
            .root()
            .and_then(|root| find_id_by_label(root, &label, 0)),
        Some(Selector::Focused) => core.tree.root().and_then(|root| find_id_focused(root, 0)),
        None => None,
    };

    let events = match (action.as_str(), widget_id) {
        ("click", Some(wid)) => {
            vec![serde_json::json!({"type": "event", "event": "click", "id": wid})]
        }
        ("type_text", Some(wid)) => {
            let text = payload.get("text").and_then(|v| v.as_str()).unwrap_or("");
            vec![serde_json::json!({"type": "event", "event": "input", "id": wid, "value": text})]
        }
        ("submit", Some(wid)) => {
            let value = payload.get("value").and_then(|v| v.as_str()).unwrap_or("");
            vec![serde_json::json!({"type": "event", "event": "submit", "id": wid, "value": value})]
        }
        ("toggle", Some(wid)) => {
            let value = payload
                .get("value")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            vec![serde_json::json!({"type": "event", "event": "toggle", "id": wid, "value": value})]
        }
        ("select", Some(wid)) => {
            let value = payload.get("value").and_then(|v| v.as_str()).unwrap_or("");
            vec![serde_json::json!({"type": "event", "event": "select", "id": wid, "value": value})]
        }
        ("slide", Some(wid)) => {
            let value = payload.get("value").and_then(|v| v.as_f64()).unwrap_or(0.0);
            vec![serde_json::json!({"type": "event", "event": "slide", "id": wid, "value": value})]
        }
        ("press", _) => {
            let payload_map = payload.as_object();
            let (key, modifiers) = parse_key_and_modifiers(payload_map);
            vec![serde_json::json!({
                "type": "event", "event": "key_press", "id": "", "key": key, "modifiers": modifiers
            })]
        }
        ("release", _) => {
            let payload_map = payload.as_object();
            let (key, modifiers) = parse_key_and_modifiers(payload_map);
            vec![serde_json::json!({
                "type": "event", "event": "key_release", "id": "", "key": key, "modifiers": modifiers
            })]
        }
        ("move_to", _) => {
            let x = payload.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let y = payload.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
            vec![serde_json::json!({
                "type": "event", "event": "cursor_moved", "id": "", "x": x, "y": y
            })]
        }
        ("type_key", _) => {
            let payload_map = payload.as_object();
            let (key, modifiers) = parse_key_and_modifiers(payload_map);
            vec![
                serde_json::json!({
                    "type": "event", "event": "key_press", "id": "", "key": key, "modifiers": modifiers
                }),
                serde_json::json!({
                    "type": "event", "event": "key_release", "id": "", "key": key, "modifiers": modifiers
                }),
            ]
        }
        _ => {
            log::warn!("unknown action '{action}' or widget not found");
            vec![]
        }
    };

    emit_wire(&InteractResponse::new(id, events));
}

/// Reset core to a blank state and emit the response.
pub fn handle_reset(core: &mut Core, id: String) {
    *core = Core::new();
    emit_wire(&ResetResponse::ok(id));
}

/// Hash the current tree and emit a SnapshotCaptureResponse.
pub fn handle_snapshot_capture(core: &Core, id: String, name: String) {
    use julep_core::protocol::SnapshotCaptureResponse;
    use sha2::{Digest, Sha256};

    let tree_json = match core.tree.root() {
        Some(root) => serde_json::to_string(root).unwrap_or_default(),
        None => "null".to_string(),
    };

    let mut hasher = Sha256::new();
    hasher.update(tree_json.as_bytes());
    let hash = format!("{:x}", hasher.finalize());

    emit_wire(&SnapshotCaptureResponse::new(id, name, hash, 0, 0));
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use julep_core::protocol::TreeNode;
    use serde_json::json;

    fn make_node(id: &str, type_name: &str) -> TreeNode {
        TreeNode {
            id: id.to_string(),
            type_name: type_name.to_string(),
            props: json!({}),
            children: vec![],
        }
    }

    fn make_text_node(id: &str, content: &str) -> TreeNode {
        TreeNode {
            id: id.to_string(),
            type_name: "text".to_string(),
            props: json!({"content": content}),
            children: vec![],
        }
    }

    // -- parse_selector --

    #[test]
    fn parse_selector_by_id() {
        let sel = json!({"by": "id", "value": "btn-1"});
        match parse_selector(&sel) {
            Some(Selector::Id(id)) => assert_eq!(id, "btn-1"),
            other => panic!("expected Id, got {other:?}"),
        }
    }

    #[test]
    fn parse_selector_by_text() {
        let sel = json!({"by": "text", "value": "Click me"});
        match parse_selector(&sel) {
            Some(Selector::Text(t)) => assert_eq!(t, "Click me"),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn parse_selector_unknown_by() {
        let sel = json!({"by": "css", "value": ".foo"});
        assert!(parse_selector(&sel).is_none());
    }

    #[test]
    fn parse_selector_missing_fields() {
        assert!(parse_selector(&json!({})).is_none());
        assert!(parse_selector(&json!({"by": "id"})).is_none());
        assert!(parse_selector(&Value::Null).is_none());
    }

    // -- parse_key_and_modifiers --

    #[test]
    fn parse_key_plain() {
        let (key, mods) = parse_key_and_modifiers(None);
        assert_eq!(key, "");
        assert_eq!(mods["ctrl"], false);
    }

    #[test]
    fn parse_key_combined_string() {
        let map: serde_json::Map<String, Value> =
            serde_json::from_value(json!({"key": "ctrl+shift+s"})).unwrap();
        let (key, mods) = parse_key_and_modifiers(Some(&map));
        assert_eq!(key, "s");
        assert_eq!(mods["ctrl"], true);
        assert_eq!(mods["shift"], true);
        assert_eq!(mods["alt"], false);
    }

    #[test]
    fn parse_key_explicit_modifiers() {
        let map: serde_json::Map<String, Value> =
            serde_json::from_value(json!({"key": "a", "modifiers": {"alt": true}})).unwrap();
        let (key, mods) = parse_key_and_modifiers(Some(&map));
        assert_eq!(key, "a");
        assert_eq!(mods["alt"], true);
        assert_eq!(mods["ctrl"], false);
    }

    #[test]
    fn parse_key_logo_aliases() {
        for alias in &["logo", "super", "meta"] {
            let combo = format!("{alias}+x");
            let map: serde_json::Map<String, Value> =
                serde_json::from_value(json!({"key": combo})).unwrap();
            let (key, mods) = parse_key_and_modifiers(Some(&map));
            assert_eq!(key, "x");
            assert_eq!(mods["logo"], true, "alias '{alias}' should set logo=true");
        }
    }

    // -- tree search --

    #[test]
    fn search_by_id_finds_root() {
        let root = make_node("root", "column");
        let result = search_by_id(&root, "root", 0);
        assert!(result.is_some());
        assert_eq!(result.unwrap()["id"], "root");
    }

    #[test]
    fn search_by_id_finds_child() {
        let mut root = make_node("root", "column");
        root.children.push(make_node("child", "button"));
        let result = search_by_id(&root, "child", 0);
        assert!(result.is_some());
        assert_eq!(result.unwrap()["id"], "child");
    }

    #[test]
    fn search_by_id_not_found() {
        let root = make_node("root", "column");
        assert!(search_by_id(&root, "missing", 0).is_none());
    }

    #[test]
    fn search_by_text_finds_node() {
        let mut root = make_node("root", "column");
        root.children.push(make_text_node("lbl", "Hello World"));
        let result = search_by_text(&root, "Hello World", 0);
        assert!(result.is_some());
        assert_eq!(result.unwrap()["id"], "lbl");
    }

    #[test]
    fn search_by_text_not_found() {
        let root = make_text_node("lbl", "Hello");
        assert!(search_by_text(&root, "Goodbye", 0).is_none());
    }

    #[test]
    fn find_id_by_text_returns_id() {
        let mut root = make_node("root", "column");
        root.children.push(make_text_node("btn", "Submit"));
        assert_eq!(find_id_by_text(&root, "Submit", 0), Some("btn".to_string()));
    }

    #[test]
    fn find_id_by_text_not_found() {
        let root = make_node("root", "column");
        assert_eq!(find_id_by_text(&root, "nope", 0), None);
    }

    // -- find_node_by_* with Core --

    #[test]
    fn find_node_by_id_empty_tree() {
        let core = Core::new();
        assert_eq!(find_node_by_id(&core, "anything"), Value::Null);
    }

    #[test]
    fn find_node_by_text_empty_tree() {
        let core = Core::new();
        assert_eq!(find_node_by_text(&core, "anything"), Value::Null);
    }

    // -- handle_interact smoke tests --
    //
    // These verify handle_interact doesn't panic for various actions.
    // The output goes to stdout via emit_wire which we can't capture in
    // a unit test, but the test proves the code path doesn't crash.
    // We build a Core with a tree so selector resolution has something
    // to work with.

    fn core_with_tree() -> Core {
        let mut core = Core::new();
        let mut root = make_node("root", "column");
        root.children.push(make_text_node("btn1", "Click me"));
        root.children.push({
            let mut n = make_node("input1", "text_input");
            n.props = json!({"placeholder": "Type here", "value": ""});
            n
        });
        root.children.push({
            let mut n = make_node("toggle1", "toggler");
            n.props = json!({"is_toggled": false});
            n
        });
        root.children.push({
            let mut n = make_node("slider1", "slider");
            n.props = json!({"min": 0.0, "max": 100.0, "value": 50.0});
            n
        });
        core.apply(julep_core::protocol::IncomingMessage::Snapshot { tree: root });
        core
    }

    #[test]
    fn handle_interact_click_does_not_panic() {
        let core = core_with_tree();
        handle_interact(
            &core,
            "i1".to_string(),
            "click".to_string(),
            json!({"by": "id", "value": "btn1"}),
            json!({}),
        );
    }

    #[test]
    fn handle_interact_type_text_does_not_panic() {
        let core = core_with_tree();
        handle_interact(
            &core,
            "i2".to_string(),
            "type_text".to_string(),
            json!({"by": "id", "value": "input1"}),
            json!({"text": "hello"}),
        );
    }

    #[test]
    fn handle_interact_submit_does_not_panic() {
        let core = core_with_tree();
        handle_interact(
            &core,
            "i3".to_string(),
            "submit".to_string(),
            json!({"by": "id", "value": "input1"}),
            json!({"value": "submitted"}),
        );
    }

    #[test]
    fn handle_interact_toggle_does_not_panic() {
        let core = core_with_tree();
        handle_interact(
            &core,
            "i4".to_string(),
            "toggle".to_string(),
            json!({"by": "id", "value": "toggle1"}),
            json!({"value": true}),
        );
    }

    #[test]
    fn handle_interact_select_does_not_panic() {
        let core = core_with_tree();
        handle_interact(
            &core,
            "i5".to_string(),
            "select".to_string(),
            json!({"by": "id", "value": "btn1"}),
            json!({"value": "option_a"}),
        );
    }

    #[test]
    fn handle_interact_slide_does_not_panic() {
        let core = core_with_tree();
        handle_interact(
            &core,
            "i6".to_string(),
            "slide".to_string(),
            json!({"by": "id", "value": "slider1"}),
            json!({"value": 75.0}),
        );
    }

    #[test]
    fn handle_interact_press_does_not_panic() {
        let core = core_with_tree();
        handle_interact(
            &core,
            "i7".to_string(),
            "press".to_string(),
            json!({}),
            json!({"key": "ctrl+s"}),
        );
    }

    #[test]
    fn handle_interact_release_does_not_panic() {
        let core = core_with_tree();
        handle_interact(
            &core,
            "i8".to_string(),
            "release".to_string(),
            json!({}),
            json!({"key": "a"}),
        );
    }

    #[test]
    fn handle_interact_move_to_does_not_panic() {
        let core = core_with_tree();
        handle_interact(
            &core,
            "i9".to_string(),
            "move_to".to_string(),
            json!({}),
            json!({"x": 100.0, "y": 200.0}),
        );
    }

    #[test]
    fn handle_interact_type_key_does_not_panic() {
        let core = core_with_tree();
        handle_interact(
            &core,
            "i10".to_string(),
            "type_key".to_string(),
            json!({}),
            json!({"key": "enter"}),
        );
    }

    #[test]
    fn handle_interact_unknown_action_does_not_panic() {
        let core = core_with_tree();
        handle_interact(
            &core,
            "i11".to_string(),
            "nonexistent_action".to_string(),
            json!({"by": "id", "value": "btn1"}),
            json!({}),
        );
    }

    #[test]
    fn handle_interact_selector_not_found_does_not_panic() {
        let core = core_with_tree();
        handle_interact(
            &core,
            "i12".to_string(),
            "click".to_string(),
            json!({"by": "id", "value": "no_such_widget"}),
            json!({}),
        );
    }

    #[test]
    fn handle_interact_by_text_selector() {
        let core = core_with_tree();
        handle_interact(
            &core,
            "i13".to_string(),
            "click".to_string(),
            json!({"by": "text", "value": "Click me"}),
            json!({}),
        );
    }

    // -- parse_selector: new variants --

    #[test]
    fn parse_selector_by_role() {
        let sel = json!({"by": "role", "value": "button"});
        match parse_selector(&sel) {
            Some(Selector::Role(r)) => assert_eq!(r, "button"),
            other => panic!("expected Role, got {other:?}"),
        }
    }

    #[test]
    fn parse_selector_by_label() {
        let sel = json!({"by": "label", "value": "Submit"});
        match parse_selector(&sel) {
            Some(Selector::Label(l)) => assert_eq!(l, "Submit"),
            other => panic!("expected Label, got {other:?}"),
        }
    }

    #[test]
    fn parse_selector_focused() {
        let sel = json!({"by": "focused"});
        match parse_selector(&sel) {
            Some(Selector::Focused) => {}
            other => panic!("expected Focused, got {other:?}"),
        }
    }

    #[test]
    fn parse_selector_focused_ignores_value() {
        // "focused" should work even if a value field is present
        let sel = json!({"by": "focused", "value": "ignored"});
        match parse_selector(&sel) {
            Some(Selector::Focused) => {}
            other => panic!("expected Focused, got {other:?}"),
        }
    }

    // -- search_by_role --

    fn make_a11y_node(id: &str, type_name: &str, a11y: Value) -> TreeNode {
        TreeNode {
            id: id.to_string(),
            type_name: type_name.to_string(),
            props: json!({"a11y": a11y}),
            children: vec![],
        }
    }

    #[test]
    fn search_by_role_matches_a11y_prop() {
        let node = make_a11y_node("btn", "container", json!({"role": "button"}));
        let result = search_by_role(&node, "button", 0);
        assert!(result.is_some());
        assert_eq!(result.unwrap()["id"], "btn");
    }

    #[test]
    fn search_by_role_matches_type_name() {
        let node = make_node("btn", "button");
        let result = search_by_role(&node, "button", 0);
        assert!(result.is_some());
        assert_eq!(result.unwrap()["id"], "btn");
    }

    #[test]
    fn search_by_role_prefers_a11y_over_type() {
        // a11y role "heading" on a "container" type -- should match "heading", not "container"
        let node = make_a11y_node("h1", "container", json!({"role": "heading"}));
        assert!(search_by_role(&node, "heading", 0).is_some());
        assert!(search_by_role(&node, "container", 0).is_none());
    }

    #[test]
    fn search_by_role_finds_in_children() {
        let mut root = make_node("root", "column");
        root.children.push(make_a11y_node(
            "slider",
            "slider",
            json!({"role": "slider"}),
        ));
        let result = search_by_role(&root, "slider", 0);
        assert!(result.is_some());
        assert_eq!(result.unwrap()["id"], "slider");
    }

    #[test]
    fn search_by_role_not_found() {
        let node = make_node("root", "column");
        assert!(search_by_role(&node, "button", 0).is_none());
    }

    // -- search_by_label --

    #[test]
    fn search_by_label_matches_a11y_label() {
        let node = make_a11y_node("btn", "button", json!({"label": "Submit"}));
        let result = search_by_label(&node, "Submit", 0);
        assert!(result.is_some());
        assert_eq!(result.unwrap()["id"], "btn");
    }

    #[test]
    fn search_by_label_matches_label_prop() {
        let mut node = make_node("chk", "checkbox");
        node.props = json!({"label": "Accept terms"});
        let result = search_by_label(&node, "Accept terms", 0);
        assert!(result.is_some());
        assert_eq!(result.unwrap()["id"], "chk");
    }

    #[test]
    fn search_by_label_matches_content_prop() {
        let node = make_text_node("txt", "Hello World");
        let result = search_by_label(&node, "Hello World", 0);
        assert!(result.is_some());
        assert_eq!(result.unwrap()["id"], "txt");
    }

    #[test]
    fn search_by_label_not_found() {
        let node = make_node("root", "column");
        assert!(search_by_label(&node, "Missing", 0).is_none());
    }

    // -- search_focused --

    #[test]
    fn search_focused_matches_focused_prop() {
        let mut node = make_node("inp", "text_input");
        node.props = json!({"focused": true});
        let result = search_focused(&node, 0);
        assert!(result.is_some());
        assert_eq!(result.unwrap()["id"], "inp");
    }

    #[test]
    fn search_focused_matches_a11y_focused() {
        let node = make_a11y_node("inp", "text_input", json!({"focused": true}));
        let result = search_focused(&node, 0);
        assert!(result.is_some());
        assert_eq!(result.unwrap()["id"], "inp");
    }

    #[test]
    fn search_focused_skips_unfocused() {
        let mut node = make_node("inp", "text_input");
        node.props = json!({"focused": false});
        assert!(search_focused(&node, 0).is_none());
    }

    #[test]
    fn search_focused_finds_in_children() {
        let mut root = make_node("root", "column");
        let mut child = make_node("inp", "text_input");
        child.props = json!({"focused": true});
        root.children.push(child);
        let result = search_focused(&root, 0);
        assert!(result.is_some());
        assert_eq!(result.unwrap()["id"], "inp");
    }

    #[test]
    fn search_focused_not_found() {
        let root = make_node("root", "column");
        assert!(search_focused(&root, 0).is_none());
    }

    // -- find_id_by_role / find_id_by_label / find_id_focused --

    #[test]
    fn find_id_by_role_returns_id() {
        let mut root = make_node("root", "column");
        root.children.push(make_node("btn", "button"));
        assert_eq!(find_id_by_role(&root, "button", 0), Some("btn".to_string()));
    }

    #[test]
    fn find_id_by_label_returns_id() {
        let mut root = make_node("root", "column");
        root.children
            .push(make_a11y_node("btn", "button", json!({"label": "Submit"})));
        assert_eq!(
            find_id_by_label(&root, "Submit", 0),
            Some("btn".to_string())
        );
    }

    #[test]
    fn find_id_focused_returns_id() {
        let mut root = make_node("root", "column");
        let mut child = make_node("inp", "text_input");
        child.props = json!({"focused": true});
        root.children.push(child);
        assert_eq!(find_id_focused(&root, 0), Some("inp".to_string()));
    }

    // -- handle_interact with new selectors --

    #[test]
    fn handle_interact_by_role_does_not_panic() {
        let core = core_with_tree();
        handle_interact(
            &core,
            "i14".to_string(),
            "click".to_string(),
            json!({"by": "role", "value": "text_input"}),
            json!({}),
        );
    }

    #[test]
    fn handle_interact_by_label_does_not_panic() {
        let core = core_with_tree();
        handle_interact(
            &core,
            "i15".to_string(),
            "click".to_string(),
            json!({"by": "label", "value": "Click me"}),
            json!({}),
        );
    }

    #[test]
    fn handle_interact_focused_does_not_panic() {
        let core = core_with_tree();
        handle_interact(
            &core,
            "i16".to_string(),
            "click".to_string(),
            json!({"by": "focused"}),
            json!({}),
        );
    }
}
