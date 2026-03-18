//! Protocol helpers for scripting messages.
//!
//! Both the daemon and headless modes handle Query/Interact/Reset/TreeHash
//! messages from stdin. The logic is identical; only the surrounding event loop
//! differs. This module contains the canonical implementations so the two modes
//! stay in sync.
//!
//! Event construction functions (`parse_iced_key`, `parse_iced_modifiers`,
//! `make_key_pressed`, `make_key_released`, `interaction_to_iced_events`) also
//! live here. Both the daemon renderer and headless mode use them to translate
//! scripting protocol interactions into iced events.

use std::io::{self, Write};

use iced::keyboard::{self, Key, Modifiers};
use iced::mouse;
use iced::{Event, Point};

use iced_test::core::SmolStr;

use serde_json::Value;

use julep_core::codec::Codec;
use julep_core::engine::Core;
use julep_core::protocol::{
    InteractResponse, OutgoingEvent, QueryResponse, ResetResponse, TreeHashResponse, TreeNode,
};

/// Maximum tree search recursion depth (matches MAX_TREE_DEPTH in widgets.rs).
const MAX_SEARCH_DEPTH: usize = 256;

// ---------------------------------------------------------------------------
// Selector
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub(crate) enum Selector {
    Id(String),
    Text(String),
    Role(String),
    Label(String),
    Focused,
}

pub(crate) fn parse_selector(selector: &Value) -> Option<Selector> {
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
pub(crate) fn parse_key_and_modifiers(
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
// Key string -> iced Key conversion
// ---------------------------------------------------------------------------

/// Convert a key name string (as sent by the scripting protocol) to an iced
/// `keyboard::Key`. Named keys use their Debug format (e.g. "Enter",
/// "Tab", "ArrowUp"); single characters become `Key::Character`.
pub(crate) fn parse_iced_key(name: &str) -> Key {
    match name {
        "Enter" | "enter" | "Return" | "return" => Key::Named(keyboard::key::Named::Enter),
        "Tab" | "tab" => Key::Named(keyboard::key::Named::Tab),
        "Space" | "space" | " " => Key::Named(keyboard::key::Named::Space),
        "Backspace" | "backspace" => Key::Named(keyboard::key::Named::Backspace),
        "Delete" | "delete" => Key::Named(keyboard::key::Named::Delete),
        "Escape" | "escape" | "Esc" | "esc" => Key::Named(keyboard::key::Named::Escape),
        "ArrowUp" | "Up" | "up" => Key::Named(keyboard::key::Named::ArrowUp),
        "ArrowDown" | "Down" | "down" => Key::Named(keyboard::key::Named::ArrowDown),
        "ArrowLeft" | "Left" | "left" => Key::Named(keyboard::key::Named::ArrowLeft),
        "ArrowRight" | "Right" | "right" => Key::Named(keyboard::key::Named::ArrowRight),
        "Home" | "home" => Key::Named(keyboard::key::Named::Home),
        "End" | "end" => Key::Named(keyboard::key::Named::End),
        "PageUp" | "pageup" => Key::Named(keyboard::key::Named::PageUp),
        "PageDown" | "pagedown" => Key::Named(keyboard::key::Named::PageDown),
        "F1" => Key::Named(keyboard::key::Named::F1),
        "F2" => Key::Named(keyboard::key::Named::F2),
        "F3" => Key::Named(keyboard::key::Named::F3),
        "F4" => Key::Named(keyboard::key::Named::F4),
        "F5" => Key::Named(keyboard::key::Named::F5),
        "F6" => Key::Named(keyboard::key::Named::F6),
        "F7" => Key::Named(keyboard::key::Named::F7),
        "F8" => Key::Named(keyboard::key::Named::F8),
        "F9" => Key::Named(keyboard::key::Named::F9),
        "F10" => Key::Named(keyboard::key::Named::F10),
        "F11" => Key::Named(keyboard::key::Named::F11),
        "F12" => Key::Named(keyboard::key::Named::F12),
        s if s.len() == 1 => Key::Character(SmolStr::new(s)),
        s => {
            // Try lowercase single char
            let lower = s.to_lowercase();
            if lower.chars().count() == 1 {
                Key::Character(SmolStr::new(&lower))
            } else {
                Key::Character(SmolStr::new(s))
            }
        }
    }
}

/// Build iced `Modifiers` from parsed scripting protocol modifiers JSON.
pub(crate) fn parse_iced_modifiers(mods: &Value) -> Modifiers {
    let mut m = Modifiers::empty();
    if mods.get("shift").and_then(|v| v.as_bool()).unwrap_or(false) {
        m |= Modifiers::SHIFT;
    }
    if mods.get("ctrl").and_then(|v| v.as_bool()).unwrap_or(false) {
        m |= Modifiers::CTRL;
    }
    if mods.get("alt").and_then(|v| v.as_bool()).unwrap_or(false) {
        m |= Modifiers::ALT;
    }
    if mods.get("logo").and_then(|v| v.as_bool()).unwrap_or(false) {
        m |= Modifiers::LOGO;
    }
    m
}

/// Build a KeyPressed iced event.
pub(crate) fn make_key_pressed(key: Key, modifiers: Modifiers, text: Option<SmolStr>) -> Event {
    Event::Keyboard(keyboard::Event::KeyPressed {
        key: key.clone(),
        modified_key: key,
        physical_key: keyboard::key::Physical::Unidentified(
            keyboard::key::NativeCode::Unidentified,
        ),
        location: keyboard::Location::Standard,
        modifiers,
        text,
        repeat: false,
    })
}

/// Build a KeyReleased iced event.
pub(crate) fn make_key_released(key: Key, modifiers: Modifiers) -> Event {
    Event::Keyboard(keyboard::Event::KeyReleased {
        key: key.clone(),
        modified_key: key,
        physical_key: keyboard::key::Physical::Unidentified(
            keyboard::key::NativeCode::Unidentified,
        ),
        location: keyboard::Location::Standard,
        modifiers,
    })
}

// ---------------------------------------------------------------------------
// Interaction -> iced events
// ---------------------------------------------------------------------------

/// Convert a scripting protocol interaction into a sequence of iced events.
///
/// Returns an empty vec for action types that don't map to iced events
/// (synthetic-only actions like paste, sort, canvas_*, pane_focus_cycle).
pub(crate) fn interaction_to_iced_events(
    action: &str,
    _widget_id: Option<&str>,
    payload: &Value,
    cursor: mouse::Cursor,
) -> Vec<Event> {
    match action {
        "click" | "toggle" | "select" => {
            // Click at the current cursor position.
            let pos = match cursor {
                mouse::Cursor::Available(p) | mouse::Cursor::Levitating(p) => p,
                mouse::Cursor::Unavailable => Point::new(0.0, 0.0),
            };
            vec![
                Event::Mouse(mouse::Event::CursorMoved { position: pos }),
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
            ]
        }
        "type_text" => {
            let text = payload.get("text").and_then(|v| v.as_str()).unwrap_or("");
            text.chars()
                .flat_map(|c| {
                    let s = SmolStr::new(c.to_string());
                    let key = Key::Character(s.clone());
                    [
                        make_key_pressed(key.clone(), Modifiers::empty(), Some(s)),
                        make_key_released(key, Modifiers::empty()),
                    ]
                })
                .collect()
        }
        "type_key" => {
            let payload_map = payload.as_object();
            let (key_str, mods_json) = parse_key_and_modifiers(payload_map);
            let key = parse_iced_key(&key_str);
            let modifiers = parse_iced_modifiers(&mods_json);
            let text = match &key {
                Key::Character(c) if modifiers.is_empty() => Some(c.clone()),
                _ => None,
            };
            vec![
                make_key_pressed(key.clone(), modifiers, text),
                make_key_released(key, modifiers),
            ]
        }
        "press" => {
            let payload_map = payload.as_object();
            let (key_str, mods_json) = parse_key_and_modifiers(payload_map);
            let key = parse_iced_key(&key_str);
            let modifiers = parse_iced_modifiers(&mods_json);
            let text = match &key {
                Key::Character(c) if modifiers.is_empty() => Some(c.clone()),
                _ => None,
            };
            vec![make_key_pressed(key, modifiers, text)]
        }
        "release" => {
            let payload_map = payload.as_object();
            let (key_str, mods_json) = parse_key_and_modifiers(payload_map);
            let key = parse_iced_key(&key_str);
            let modifiers = parse_iced_modifiers(&mods_json);
            vec![make_key_released(key, modifiers)]
        }
        "submit" => {
            let key = Key::Named(keyboard::key::Named::Enter);
            vec![
                make_key_pressed(key.clone(), Modifiers::empty(), None),
                make_key_released(key, Modifiers::empty()),
            ]
        }
        "scroll" => {
            let delta_x = payload
                .get("delta_x")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0) as f32;
            let delta_y = payload
                .get("delta_y")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0) as f32;
            vec![Event::Mouse(mouse::Event::WheelScrolled {
                delta: mouse::ScrollDelta::Lines {
                    x: delta_x,
                    y: delta_y,
                },
            })]
        }
        "move_to" => {
            let x = payload.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
            let y = payload.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
            vec![Event::Mouse(mouse::Event::CursorMoved {
                position: Point::new(x, y),
            })]
        }
        // Synthetic-only actions: no iced event injection needed.
        "paste" | "sort" | "canvas_press" | "canvas_release" | "canvas_move"
        | "pane_focus_cycle" | "slide" => vec![],
        _ => vec![],
    }
}

// ---------------------------------------------------------------------------
// Wire I/O
// ---------------------------------------------------------------------------

/// Write a serialized response to stdout using the negotiated wire codec.
pub(crate) fn emit_wire<T: serde::Serialize>(value: &T) -> io::Result<()> {
    let codec = Codec::get_global();
    let bytes = codec.encode(value).map_err(io::Error::other)?;
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    handle.write_all(&bytes)?;
    handle.flush()
}

// ---------------------------------------------------------------------------
// Tree search helpers
// ---------------------------------------------------------------------------

/// Walk the tree depth-first, returning the first node matching the predicate.
/// `extract` converts the matching node to the desired return type.
fn search_tree<R>(
    node: &TreeNode,
    depth: usize,
    predicate: &dyn Fn(&TreeNode) -> bool,
    extract: &dyn Fn(&TreeNode) -> R,
) -> Option<R> {
    if depth > MAX_SEARCH_DEPTH {
        return None;
    }
    if predicate(node) {
        return Some(extract(node));
    }
    for child in &node.children {
        if let Some(found) = search_tree(child, depth + 1, predicate, extract) {
            return Some(found);
        }
    }
    None
}

// -- Extractors -------------------------------------------------------------

fn node_to_value(node: &TreeNode) -> Value {
    serde_json::to_value(node).unwrap_or(Value::Null)
}

fn node_id(node: &TreeNode) -> String {
    node.id.clone()
}

// -- Predicates -------------------------------------------------------------

/// Match by explicit `a11y.role`, falling back to `type_name` only when no
/// `a11y` prop is present at all.
fn matches_role(node: &TreeNode, role: &str) -> bool {
    if let Some(a11y) = node.props.get("a11y") {
        a11y.get("role").and_then(|v| v.as_str()) == Some(role)
    } else {
        node.type_name == role
    }
}

/// Match by explicit `a11y.label`, falling back to `label` and `content` props.
fn matches_label(node: &TreeNode, label: &str) -> bool {
    if let Some(a11y) = node.props.get("a11y")
        && let Some(node_label) = a11y.get("label").and_then(|v| v.as_str())
        && node_label == label
    {
        return true;
    }
    for key in &["label", "content"] {
        if let Some(val) = node.props.get(*key)
            && val.as_str() == Some(label)
        {
            return true;
        }
    }
    false
}

/// Match against text content in `content`, `label`, `value`, and `placeholder` props.
fn matches_text(node: &TreeNode, text: &str) -> bool {
    for key in &["content", "label", "value", "placeholder"] {
        if let Some(val) = node.props.get(*key)
            && val.as_str() == Some(text)
        {
            return true;
        }
    }
    false
}

/// Match nodes with `props.focused == true` or `a11y.focused == true`.
fn is_focused(node: &TreeNode) -> bool {
    if node.props.get("focused").and_then(|v| v.as_bool()) == Some(true) {
        return true;
    }
    if let Some(a11y) = node.props.get("a11y")
        && a11y.get("focused").and_then(|v| v.as_bool()) == Some(true)
    {
        return true;
    }
    false
}

// -- Public API (node as Value) ---------------------------------------------

pub(crate) fn find_node_by_id(core: &Core, widget_id: &str) -> Value {
    core.tree
        .root()
        .and_then(|root| search_tree(root, 0, &|n| n.id == widget_id, &node_to_value))
        .unwrap_or(Value::Null)
}

pub(crate) fn find_node_by_text(core: &Core, text: &str) -> Value {
    core.tree
        .root()
        .and_then(|root| search_tree(root, 0, &|n| matches_text(n, text), &node_to_value))
        .unwrap_or(Value::Null)
}

pub(crate) fn find_node_by_role(core: &Core, role: &str) -> Value {
    core.tree
        .root()
        .and_then(|root| search_tree(root, 0, &|n| matches_role(n, role), &node_to_value))
        .unwrap_or(Value::Null)
}

pub(crate) fn find_node_by_label(core: &Core, label: &str) -> Value {
    core.tree
        .root()
        .and_then(|root| search_tree(root, 0, &|n| matches_label(n, label), &node_to_value))
        .unwrap_or(Value::Null)
}

pub(crate) fn find_focused_node(core: &Core) -> Value {
    core.tree
        .root()
        .and_then(|root| search_tree(root, 0, &is_focused, &node_to_value))
        .unwrap_or(Value::Null)
}

// -- Public API (ID only) ---------------------------------------------------

pub(crate) fn find_id_by_text(node: &TreeNode, text: &str, depth: usize) -> Option<String> {
    search_tree(node, depth, &|n| matches_text(n, text), &node_id)
}

pub(crate) fn find_id_by_role(node: &TreeNode, role: &str, depth: usize) -> Option<String> {
    search_tree(node, depth, &|n| matches_role(n, role), &node_id)
}

pub(crate) fn find_id_by_label(node: &TreeNode, label: &str, depth: usize) -> Option<String> {
    search_tree(node, depth, &|n| matches_label(n, label), &node_id)
}

pub(crate) fn find_id_focused(node: &TreeNode, depth: usize) -> Option<String> {
    search_tree(node, depth, &is_focused, &node_id)
}

// ---------------------------------------------------------------------------
// Message handlers
// ---------------------------------------------------------------------------

/// Build a QueryResponse without writing it anywhere.
pub(crate) fn build_query_response(
    core: &Core,
    id: String,
    target: String,
    selector: Value,
) -> QueryResponse {
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

    QueryResponse::new(id, target, data)
}

/// Build and emit a QueryResponse to stdout.
pub(crate) fn handle_query(
    core: &Core,
    id: String,
    target: String,
    selector: Value,
) -> io::Result<()> {
    emit_wire(&build_query_response(core, id, target, selector))
}

/// Resolve a selector to a widget ID without emitting anything.
pub(crate) fn resolve_widget_id(core: &Core, selector: &Value) -> Option<String> {
    match parse_selector(selector)? {
        Selector::Id(wid) => Some(wid),
        Selector::Text(text) => core
            .tree
            .root()
            .and_then(|root| find_id_by_text(root, &text, 0)),
        Selector::Role(role) => core
            .tree
            .root()
            .and_then(|root| find_id_by_role(root, &role, 0)),
        Selector::Label(label) => core
            .tree
            .root()
            .and_then(|root| find_id_by_label(root, &label, 0)),
        Selector::Focused => core.tree.root().and_then(|root| find_id_focused(root, 0)),
    }
}

/// Build an InteractResponse without writing it anywhere.
pub(crate) fn build_interact_response(
    core: &Core,
    id: String,
    action: String,
    selector: Value,
    payload: Value,
) -> InteractResponse {
    let widget_id = resolve_widget_id(core, &selector);

    let events: Vec<OutgoingEvent> = match (action.as_str(), widget_id) {
        ("click", Some(wid)) => {
            vec![OutgoingEvent::click(wid)]
        }
        ("type_text", Some(wid)) => {
            let text = payload.get("text").and_then(|v| v.as_str()).unwrap_or("");
            vec![OutgoingEvent::input(wid, text.to_string())]
        }
        ("submit", Some(wid)) => {
            let value = payload.get("value").and_then(|v| v.as_str()).unwrap_or("");
            vec![OutgoingEvent::submit(wid, value.to_string())]
        }
        ("toggle", Some(wid)) => {
            let value = payload
                .get("value")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            vec![OutgoingEvent::toggle(wid, value)]
        }
        ("select", Some(wid)) => {
            let value = payload.get("value").and_then(|v| v.as_str()).unwrap_or("");
            vec![OutgoingEvent::select(wid, value.to_string())]
        }
        ("slide", Some(wid)) => {
            let value = payload.get("value").and_then(|v| v.as_f64()).unwrap_or(0.0);
            vec![OutgoingEvent::slide(wid, value)]
        }
        ("press", _) => {
            let payload_map = payload.as_object();
            let (key, modifiers) = parse_key_and_modifiers(payload_map);
            vec![OutgoingEvent::scripting_key_press(key, modifiers)]
        }
        ("release", _) => {
            let payload_map = payload.as_object();
            let (key, modifiers) = parse_key_and_modifiers(payload_map);
            vec![OutgoingEvent::scripting_key_release(key, modifiers)]
        }
        ("move_to", _) => {
            let x = payload.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let y = payload.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
            vec![OutgoingEvent::scripting_cursor_moved(x, y)]
        }
        ("type_key", _) => {
            let payload_map = payload.as_object();
            let (key, modifiers) = parse_key_and_modifiers(payload_map);
            vec![
                OutgoingEvent::scripting_key_press(key.clone(), modifiers.clone()),
                OutgoingEvent::scripting_key_release(key, modifiers),
            ]
        }
        ("paste", Some(wid)) => {
            let text = payload.get("text").and_then(|v| v.as_str()).unwrap_or("");
            vec![OutgoingEvent::paste(wid, text.to_string())]
        }
        ("scroll", _) => {
            let delta_x = payload
                .get("delta_x")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let delta_y = payload
                .get("delta_y")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            vec![OutgoingEvent::scripting_scroll(delta_x, delta_y)]
        }
        ("sort", Some(wid)) => {
            let column = payload.get("column").and_then(|v| v.as_str()).unwrap_or("");
            vec![OutgoingEvent::generic(
                "sort",
                wid,
                Some(serde_json::json!({"column": column})),
            )]
        }
        ("pane_focus_cycle", Some(wid)) => {
            vec![OutgoingEvent::generic("pane_focus_cycle", wid, None)]
        }
        ("canvas_press", Some(wid)) => {
            let x = payload.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let y = payload.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
            vec![OutgoingEvent::generic(
                "canvas_press",
                wid,
                Some(serde_json::json!({"x": x, "y": y})),
            )]
        }
        ("canvas_release", Some(wid)) => {
            let x = payload.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let y = payload.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
            vec![OutgoingEvent::generic(
                "canvas_release",
                wid,
                Some(serde_json::json!({"x": x, "y": y})),
            )]
        }
        ("canvas_move", Some(wid)) => {
            let x = payload.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let y = payload.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
            vec![OutgoingEvent::generic(
                "canvas_move",
                wid,
                Some(serde_json::json!({"x": x, "y": y})),
            )]
        }
        _ => {
            log::warn!("unknown action '{action}' or widget not found");
            vec![]
        }
    };

    InteractResponse::new(id, events)
}

/// Build and emit an InteractResponse to stdout.
pub(crate) fn handle_interact(
    core: &Core,
    id: String,
    action: String,
    selector: Value,
    payload: Value,
) -> io::Result<()> {
    emit_wire(&build_interact_response(
        core, id, action, selector, payload,
    ))
}

/// Reset core to a blank state and return the response.
pub(crate) fn build_reset_response(core: &mut Core, id: String) -> ResetResponse {
    *core = Core::new();
    ResetResponse::ok(id)
}

/// Reset core and emit the response to stdout.
pub(crate) fn handle_reset(core: &mut Core, id: String) -> io::Result<()> {
    emit_wire(&build_reset_response(core, id))
}

/// Build a TreeHashResponse without writing it anywhere.
pub(crate) fn build_tree_hash_response(core: &Core, id: String, name: String) -> TreeHashResponse {
    use sha2::{Digest, Sha256};

    let tree_json = match core.tree.root() {
        Some(root) => serde_json::to_string(root).unwrap_or_default(),
        None => "null".to_string(),
    };

    let mut hasher = Sha256::new();
    hasher.update(tree_json.as_bytes());
    let hash = format!("{:x}", hasher.finalize());

    TreeHashResponse::new(id, name, hash, 0, 0)
}

/// Build and emit a TreeHashResponse to stdout.
pub(crate) fn handle_tree_hash(core: &Core, id: String, name: String) -> io::Result<()> {
    emit_wire(&build_tree_hash_response(core, id, name))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(unused_must_use)]
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
        let result = search_tree(&root, 0, &|n| n.id == "root", &node_to_value);
        assert!(result.is_some());
        assert_eq!(result.unwrap()["id"], "root");
    }

    #[test]
    fn search_by_id_finds_child() {
        let mut root = make_node("root", "column");
        root.children.push(make_node("child", "button"));
        let result = search_tree(&root, 0, &|n| n.id == "child", &node_to_value);
        assert!(result.is_some());
        assert_eq!(result.unwrap()["id"], "child");
    }

    #[test]
    fn search_by_id_not_found() {
        let root = make_node("root", "column");
        assert!(search_tree(&root, 0, &|n| n.id == "missing", &node_to_value).is_none());
    }

    #[test]
    fn search_by_text_finds_node() {
        let mut root = make_node("root", "column");
        root.children.push(make_text_node("lbl", "Hello World"));
        let result = search_tree(
            &root,
            0,
            &|n| matches_text(n, "Hello World"),
            &node_to_value,
        );
        assert!(result.is_some());
        assert_eq!(result.unwrap()["id"], "lbl");
    }

    #[test]
    fn search_by_text_not_found() {
        let root = make_text_node("lbl", "Hello");
        assert!(search_tree(&root, 0, &|n| matches_text(n, "Goodbye"), &node_to_value).is_none());
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
        let result = search_tree(&node, 0, &|n| matches_role(n, "button"), &node_to_value);
        assert!(result.is_some());
        assert_eq!(result.unwrap()["id"], "btn");
    }

    #[test]
    fn search_by_role_matches_type_name() {
        let node = make_node("btn", "button");
        let result = search_tree(&node, 0, &|n| matches_role(n, "button"), &node_to_value);
        assert!(result.is_some());
        assert_eq!(result.unwrap()["id"], "btn");
    }

    #[test]
    fn search_by_role_prefers_a11y_over_type() {
        // a11y role "heading" on a "container" type -- should match "heading", not "container"
        let node = make_a11y_node("h1", "container", json!({"role": "heading"}));
        assert!(search_tree(&node, 0, &|n| matches_role(n, "heading"), &node_to_value).is_some());
        assert!(search_tree(&node, 0, &|n| matches_role(n, "container"), &node_to_value).is_none());
    }

    #[test]
    fn search_by_role_finds_in_children() {
        let mut root = make_node("root", "column");
        root.children.push(make_a11y_node(
            "slider",
            "slider",
            json!({"role": "slider"}),
        ));
        let result = search_tree(&root, 0, &|n| matches_role(n, "slider"), &node_to_value);
        assert!(result.is_some());
        assert_eq!(result.unwrap()["id"], "slider");
    }

    #[test]
    fn search_by_role_not_found() {
        let node = make_node("root", "column");
        assert!(search_tree(&node, 0, &|n| matches_role(n, "button"), &node_to_value).is_none());
    }

    // -- search_by_label --

    #[test]
    fn search_by_label_matches_a11y_label() {
        let node = make_a11y_node("btn", "button", json!({"label": "Submit"}));
        let result = search_tree(&node, 0, &|n| matches_label(n, "Submit"), &node_to_value);
        assert!(result.is_some());
        assert_eq!(result.unwrap()["id"], "btn");
    }

    #[test]
    fn search_by_label_matches_label_prop() {
        let mut node = make_node("chk", "checkbox");
        node.props = json!({"label": "Accept terms"});
        let result = search_tree(
            &node,
            0,
            &|n| matches_label(n, "Accept terms"),
            &node_to_value,
        );
        assert!(result.is_some());
        assert_eq!(result.unwrap()["id"], "chk");
    }

    #[test]
    fn search_by_label_matches_content_prop() {
        let node = make_text_node("txt", "Hello World");
        let result = search_tree(
            &node,
            0,
            &|n| matches_label(n, "Hello World"),
            &node_to_value,
        );
        assert!(result.is_some());
        assert_eq!(result.unwrap()["id"], "txt");
    }

    #[test]
    fn search_by_label_not_found() {
        let node = make_node("root", "column");
        assert!(search_tree(&node, 0, &|n| matches_label(n, "Missing"), &node_to_value).is_none());
    }

    // -- search_focused --

    #[test]
    fn search_focused_matches_focused_prop() {
        let mut node = make_node("inp", "text_input");
        node.props = json!({"focused": true});
        let result = search_tree(&node, 0, &is_focused, &node_to_value);
        assert!(result.is_some());
        assert_eq!(result.unwrap()["id"], "inp");
    }

    #[test]
    fn search_focused_matches_a11y_focused() {
        let node = make_a11y_node("inp", "text_input", json!({"focused": true}));
        let result = search_tree(&node, 0, &is_focused, &node_to_value);
        assert!(result.is_some());
        assert_eq!(result.unwrap()["id"], "inp");
    }

    #[test]
    fn search_focused_skips_unfocused() {
        let mut node = make_node("inp", "text_input");
        node.props = json!({"focused": false});
        assert!(search_tree(&node, 0, &is_focused, &node_to_value).is_none());
    }

    #[test]
    fn search_focused_finds_in_children() {
        let mut root = make_node("root", "column");
        let mut child = make_node("inp", "text_input");
        child.props = json!({"focused": true});
        root.children.push(child);
        let result = search_tree(&root, 0, &is_focused, &node_to_value);
        assert!(result.is_some());
        assert_eq!(result.unwrap()["id"], "inp");
    }

    #[test]
    fn search_focused_not_found() {
        let root = make_node("root", "column");
        assert!(search_tree(&root, 0, &is_focused, &node_to_value).is_none());
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
    fn handle_interact_paste_does_not_panic() {
        let core = core_with_tree();
        handle_interact(
            &core,
            "i17".to_string(),
            "paste".to_string(),
            json!({"by": "id", "value": "input1"}),
            json!({"text": "pasted text"}),
        );
    }

    #[test]
    fn handle_interact_scroll_does_not_panic() {
        let core = core_with_tree();
        handle_interact(
            &core,
            "i18".to_string(),
            "scroll".to_string(),
            json!({}),
            json!({"delta_x": 0.0, "delta_y": -10.0}),
        );
    }

    #[test]
    fn handle_interact_sort_does_not_panic() {
        let core = core_with_tree();
        handle_interact(
            &core,
            "i19".to_string(),
            "sort".to_string(),
            json!({"by": "id", "value": "btn1"}),
            json!({"column": "name"}),
        );
    }

    #[test]
    fn handle_interact_pane_focus_cycle_does_not_panic() {
        let core = core_with_tree();
        handle_interact(
            &core,
            "i20".to_string(),
            "pane_focus_cycle".to_string(),
            json!({"by": "id", "value": "btn1"}),
            json!({}),
        );
    }

    #[test]
    fn handle_interact_canvas_press_does_not_panic() {
        let core = core_with_tree();
        handle_interact(
            &core,
            "i21".to_string(),
            "canvas_press".to_string(),
            json!({"by": "id", "value": "btn1"}),
            json!({"x": 50.0, "y": 75.0}),
        );
    }

    #[test]
    fn handle_interact_canvas_release_does_not_panic() {
        let core = core_with_tree();
        handle_interact(
            &core,
            "i22".to_string(),
            "canvas_release".to_string(),
            json!({"by": "id", "value": "btn1"}),
            json!({"x": 50.0, "y": 75.0}),
        );
    }

    #[test]
    fn handle_interact_canvas_move_does_not_panic() {
        let core = core_with_tree();
        handle_interact(
            &core,
            "i23".to_string(),
            "canvas_move".to_string(),
            json!({"by": "id", "value": "btn1"}),
            json!({"x": 60.0, "y": 80.0}),
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

    // -- parse_iced_key / parse_iced_modifiers / interaction_to_iced_events --

    #[test]
    fn parse_iced_key_named_enter() {
        assert_eq!(
            parse_iced_key("Enter"),
            Key::Named(keyboard::key::Named::Enter)
        );
        assert_eq!(
            parse_iced_key("enter"),
            Key::Named(keyboard::key::Named::Enter)
        );
    }

    #[test]
    fn parse_iced_key_named_tab() {
        assert_eq!(parse_iced_key("Tab"), Key::Named(keyboard::key::Named::Tab));
    }

    #[test]
    fn parse_iced_key_named_arrows() {
        assert_eq!(
            parse_iced_key("ArrowUp"),
            Key::Named(keyboard::key::Named::ArrowUp)
        );
        assert_eq!(
            parse_iced_key("Up"),
            Key::Named(keyboard::key::Named::ArrowUp)
        );
        assert_eq!(
            parse_iced_key("ArrowDown"),
            Key::Named(keyboard::key::Named::ArrowDown)
        );
    }

    #[test]
    fn parse_iced_key_single_char() {
        assert_eq!(parse_iced_key("a"), Key::Character(SmolStr::new("a")));
        assert_eq!(parse_iced_key("Z"), Key::Character(SmolStr::new("Z")));
    }

    #[test]
    fn parse_iced_key_function_keys() {
        assert_eq!(parse_iced_key("F1"), Key::Named(keyboard::key::Named::F1));
        assert_eq!(parse_iced_key("F12"), Key::Named(keyboard::key::Named::F12));
    }

    #[test]
    fn parse_iced_modifiers_from_json() {
        let mods = json!({"shift": true, "ctrl": true, "alt": false, "logo": false});
        let result = parse_iced_modifiers(&mods);
        assert!(result.shift());
        assert!(result.control());
        assert!(!result.alt());
        assert!(!result.logo());
    }

    #[test]
    fn parse_iced_modifiers_empty() {
        let mods = json!({});
        let result = parse_iced_modifiers(&mods);
        assert!(result.is_empty());
    }

    #[test]
    fn interaction_to_iced_events_click() {
        let events = interaction_to_iced_events(
            "click",
            Some("btn1"),
            &json!({}),
            mouse::Cursor::Available(Point::new(100.0, 50.0)),
        );
        assert_eq!(events.len(), 3); // CursorMoved + ButtonPressed + ButtonReleased
    }

    #[test]
    fn interaction_to_iced_events_type_text() {
        let events = interaction_to_iced_events(
            "type_text",
            Some("inp1"),
            &json!({"text": "hi"}),
            mouse::Cursor::Unavailable,
        );
        // 2 chars * 2 events each (press + release)
        assert_eq!(events.len(), 4);
    }

    #[test]
    fn interaction_to_iced_events_scroll() {
        let events = interaction_to_iced_events(
            "scroll",
            None,
            &json!({"delta_x": 0.0, "delta_y": -10.0}),
            mouse::Cursor::Unavailable,
        );
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                assert_eq!(*delta, mouse::ScrollDelta::Lines { x: 0.0, y: -10.0 });
            }
            _ => panic!("expected WheelScrolled"),
        }
    }

    #[test]
    fn interaction_to_iced_events_move_to() {
        let events = interaction_to_iced_events(
            "move_to",
            None,
            &json!({"x": 42.0, "y": 84.0}),
            mouse::Cursor::Unavailable,
        );
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                assert_eq!(*position, Point::new(42.0, 84.0));
            }
            _ => panic!("expected CursorMoved"),
        }
    }

    #[test]
    fn interaction_to_iced_events_synthetic_only() {
        // These actions should produce no iced events.
        for action in &[
            "paste",
            "sort",
            "canvas_press",
            "canvas_release",
            "canvas_move",
            "pane_focus_cycle",
            "slide",
        ] {
            let events = interaction_to_iced_events(
                action,
                Some("w1"),
                &json!({}),
                mouse::Cursor::Unavailable,
            );
            assert!(
                events.is_empty(),
                "action '{action}' should produce no iced events"
            );
        }
    }

    #[test]
    fn interaction_to_iced_events_submit() {
        let events = interaction_to_iced_events(
            "submit",
            Some("inp1"),
            &json!({}),
            mouse::Cursor::Unavailable,
        );
        assert_eq!(events.len(), 2); // KeyPressed(Enter) + KeyReleased(Enter)
    }

    #[test]
    fn interaction_to_iced_events_type_key() {
        let events = interaction_to_iced_events(
            "type_key",
            None,
            &json!({"key": "ctrl+s"}),
            mouse::Cursor::Unavailable,
        );
        assert_eq!(events.len(), 2); // KeyPressed + KeyReleased
    }

    #[test]
    fn interaction_to_iced_events_press_release() {
        let press = interaction_to_iced_events(
            "press",
            None,
            &json!({"key": "a"}),
            mouse::Cursor::Unavailable,
        );
        assert_eq!(press.len(), 1);

        let release = interaction_to_iced_events(
            "release",
            None,
            &json!({"key": "a"}),
            mouse::Cursor::Unavailable,
        );
        assert_eq!(release.len(), 1);
    }
}
