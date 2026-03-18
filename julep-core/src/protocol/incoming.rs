//! Incoming wire messages from the host process.

use serde::{Deserialize, Deserializer};
use serde_json::Value;

use super::types::{PatchOp, TreeNode};

/// Messages sent from the host to the renderer over stdin.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IncomingMessage {
    /// Replace the entire UI tree with a new snapshot.
    Snapshot { tree: TreeNode },
    /// Apply incremental changes to the retained UI tree.
    Patch { ops: Vec<PatchOp> },
    /// Request a platform effect (file dialog, clipboard, notification).
    EffectRequest {
        id: String,
        kind: String,
        payload: Value,
    },
    /// Perform a widget operation (focus, scroll, select, etc.).
    WidgetOp {
        op: String,
        #[serde(default)]
        payload: Value,
    },
    /// Subscribe to a runtime event source (keyboard, mouse, window, etc.).
    SubscriptionRegister { kind: String, tag: String },
    /// Unsubscribe from a runtime event source.
    SubscriptionUnregister { kind: String },
    /// Perform a window operation (resize, move, close, etc.).
    WindowOp {
        op: String,
        window_id: String,
        #[serde(default)]
        settings: Value,
    },
    /// Apply or update renderer settings.
    Settings { settings: Value },
    /// Query the current tree or find a widget.
    Query {
        id: String,
        target: String,
        #[serde(default)]
        selector: Value,
    },
    /// Interact with a widget (click, type, etc.)
    Interact {
        id: String,
        action: String,
        #[serde(default)]
        selector: Value,
        #[serde(default)]
        payload: Value,
    },
    /// Capture a structural tree hash (hash of JSON tree).
    // Used by the binary crate's headless and test modes. Appears dead
    // from julep-core's perspective because the usage is in julep/.
    #[allow(dead_code)]
    TreeHash {
        id: String,
        name: String,
        #[serde(default)]
        theme: Value,
        #[serde(default)]
        viewport: Value,
    },
    /// Capture a pixel screenshot (GPU-rendered RGBA data).
    #[allow(dead_code)]
    ScreenshotCapture {
        id: String,
        name: String,
        #[serde(default)]
        width: Option<u32>,
        #[serde(default)]
        height: Option<u32>,
    },
    /// Reset the app state.
    Reset { id: String },
    /// Image operation (create, update, delete in-memory image handles).
    ///
    /// Binary fields (`data`, `pixels`) accept either raw bytes (from msgpack)
    /// or base64-encoded strings (from JSON). The custom deserializer handles both.
    ImageOp {
        op: String,
        handle: String,
        #[serde(default, deserialize_with = "deserialize_binary_field")]
        data: Option<Vec<u8>>,
        #[serde(default, deserialize_with = "deserialize_binary_field")]
        pixels: Option<Vec<u8>>,
        #[serde(default)]
        width: Option<u32>,
        #[serde(default)]
        height: Option<u32>,
    },
    /// A single extension command pushed to a native extension widget.
    /// Bypasses the normal tree update / diff / patch cycle.
    ExtensionCommand {
        node_id: String,
        op: String,
        #[serde(default)]
        payload: Value,
    },
    /// A batch of extension commands processed in one cycle.
    ExtensionCommandBatch { commands: Vec<ExtensionCommandItem> },
    /// Advance the animation clock by one frame (headless/test mode).
    /// Emits an `animation_frame` event if `on_animation_frame` is subscribed.
    AdvanceFrame { timestamp: u64 },
}

/// A single item within an `ExtensionCommandBatch`.
#[derive(Debug, Clone, Deserialize)]
pub struct ExtensionCommandItem {
    pub node_id: String,
    pub op: String,
    #[serde(default)]
    pub payload: Value,
}

// ---------------------------------------------------------------------------
// Binary field deserialization (handles both raw bytes and base64 strings)
// ---------------------------------------------------------------------------

/// Deserializes a binary field that may arrive as:
/// - Raw bytes (msgpack binary type, via rmpv path)
/// - Base64-encoded string (JSON path)
/// - null / absent (returns None)
///
/// When the codec's rmpv-based decode extracts binary fields and injects them
/// as `serde_json::Value::Array` of u8 values, serde picks them up as Vec<u8>.
/// When the field arrives as a base64 string (JSON mode), we decode it here.
fn deserialize_binary_field<'de, D>(deserializer: D) -> Result<Option<Vec<u8>>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::Error;

    let val: Option<Value> = Option::deserialize(deserializer)?;
    match val {
        None => Ok(None),
        Some(Value::Null) => Ok(None),
        // Base64 string (JSON mode)
        Some(Value::String(s)) => {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD
                .decode(&s)
                .map(Some)
                .map_err(|e| D::Error::custom(format!("base64 decode: {e}")))
        }
        // Array of u8 values (injected by rmpv binary extraction)
        Some(Value::Array(arr)) => {
            let bytes: Result<Vec<u8>, _> = arr
                .into_iter()
                .map(|v| {
                    v.as_u64()
                        .and_then(|n| u8::try_from(n).ok())
                        .ok_or_else(|| D::Error::custom("expected u8 in binary array"))
                })
                .collect();
            bytes.map(Some)
        }
        Some(other) => Err(D::Error::custom(format!(
            "expected string, array, or null for binary field, got {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -----------------------------------------------------------------------
    // IncomingMessage deserialization
    // -----------------------------------------------------------------------

    #[test]
    fn deserialize_snapshot() {
        let json =
            r#"{"type":"snapshot","tree":{"id":"root","type":"column","props":{},"children":[]}}"#;
        let msg: IncomingMessage = serde_json::from_str(json).unwrap();
        match msg {
            IncomingMessage::Snapshot { tree } => {
                assert_eq!(tree.id, "root");
                assert_eq!(tree.type_name, "column");
            }
            _ => panic!("expected Snapshot"),
        }
    }

    #[test]
    fn deserialize_snapshot_nested_tree() {
        let msg: IncomingMessage = serde_json::from_value(json!({
            "type": "snapshot",
            "tree": {
                "id": "root",
                "type": "column",
                "props": { "spacing": 10 },
                "children": [{
                    "id": "c1",
                    "type": "text",
                    "props": { "content": "hello" },
                    "children": []
                }]
            }
        }))
        .unwrap();
        match msg {
            IncomingMessage::Snapshot { tree } => {
                assert_eq!(tree.children.len(), 1);
                assert_eq!(tree.children[0].id, "c1");
                assert_eq!(tree.children[0].type_name, "text");
                assert_eq!(tree.props["spacing"], 10);
            }
            _ => panic!("expected Snapshot"),
        }
    }

    #[test]
    fn deserialize_patch_replace_node() {
        let msg: IncomingMessage = serde_json::from_value(json!({
            "type": "patch",
            "ops": [{
                "op": "replace_node",
                "path": [0],
                "node": {
                    "id": "x",
                    "type": "text",
                    "props": {},
                    "children": []
                }
            }]
        }))
        .unwrap();
        match msg {
            IncomingMessage::Patch { ops } => {
                assert_eq!(ops.len(), 1);
                assert_eq!(ops[0].op, "replace_node");
                assert_eq!(ops[0].path, vec![0]);
                assert!(ops[0].rest.get("node").is_some());
            }
            _ => panic!("expected Patch"),
        }
    }

    #[test]
    fn deserialize_patch_multiple_ops() {
        let msg: IncomingMessage = serde_json::from_value(json!({
            "type": "patch",
            "ops": [
                { "op": "update_props", "path": [0], "props": { "color": "red" } },
                { "op": "remove_child", "path": [], "index": 2 }
            ]
        }))
        .unwrap();
        match msg {
            IncomingMessage::Patch { ops } => {
                assert_eq!(ops.len(), 2);
                assert_eq!(ops[0].op, "update_props");
                assert_eq!(ops[1].op, "remove_child");
            }
            _ => panic!("expected Patch"),
        }
    }

    #[test]
    fn deserialize_effect_request() {
        let json = r#"{"type":"effect_request","id":"e1","kind":"clipboard_read","payload":{}}"#;
        let msg: IncomingMessage = serde_json::from_str(json).unwrap();
        match msg {
            IncomingMessage::EffectRequest { id, kind, payload } => {
                assert_eq!(id, "e1");
                assert_eq!(kind, "clipboard_read");
                assert!(payload.is_object());
            }
            _ => panic!("expected EffectRequest"),
        }
    }

    #[test]
    fn deserialize_effect_request_with_payload() {
        let msg: IncomingMessage = serde_json::from_value(json!({
            "type": "effect_request",
            "id": "e2",
            "kind": "clipboard_write",
            "payload": { "text": "copied" }
        }))
        .unwrap();
        match msg {
            IncomingMessage::EffectRequest { id, kind, payload } => {
                assert_eq!(id, "e2");
                assert_eq!(kind, "clipboard_write");
                assert_eq!(payload["text"], "copied");
            }
            _ => panic!("expected EffectRequest"),
        }
    }

    #[test]
    fn deserialize_widget_op() {
        let json = r#"{"type":"widget_op","op":"focus","payload":{"target":"input1"}}"#;
        let msg: IncomingMessage = serde_json::from_str(json).unwrap();
        match msg {
            IncomingMessage::WidgetOp { op, payload } => {
                assert_eq!(op, "focus");
                assert_eq!(payload["target"], "input1");
            }
            _ => panic!("expected WidgetOp"),
        }
    }

    #[test]
    fn deserialize_widget_op_no_payload() {
        let json = r#"{"type":"widget_op","op":"blur"}"#;
        let msg: IncomingMessage = serde_json::from_str(json).unwrap();
        match msg {
            IncomingMessage::WidgetOp { op, payload } => {
                assert_eq!(op, "blur");
                assert!(payload.is_null());
            }
            _ => panic!("expected WidgetOp"),
        }
    }

    #[test]
    fn deserialize_subscription_register() {
        let json = r#"{"type":"subscription_register","kind":"on_key_press","tag":"keys"}"#;
        let msg: IncomingMessage = serde_json::from_str(json).unwrap();
        match msg {
            IncomingMessage::SubscriptionRegister { kind, tag } => {
                assert_eq!(kind, "on_key_press");
                assert_eq!(tag, "keys");
            }
            _ => panic!("expected SubscriptionRegister"),
        }
    }

    #[test]
    fn deserialize_subscription_unregister() {
        let json = r#"{"type":"subscription_unregister","kind":"on_key_press"}"#;
        let msg: IncomingMessage = serde_json::from_str(json).unwrap();
        match msg {
            IncomingMessage::SubscriptionUnregister { kind } => {
                assert_eq!(kind, "on_key_press");
            }
            _ => panic!("expected SubscriptionUnregister"),
        }
    }

    #[test]
    fn deserialize_settings() {
        let json = r#"{"type":"settings","settings":{"default_text_size":18}}"#;
        let msg: IncomingMessage = serde_json::from_str(json).unwrap();
        match msg {
            IncomingMessage::Settings { settings } => {
                assert_eq!(settings["default_text_size"], 18);
            }
            _ => panic!("expected Settings"),
        }
    }

    #[test]
    fn deserialize_window_op() {
        let msg: IncomingMessage = serde_json::from_value(json!({
            "type": "window_op",
            "op": "resize",
            "window_id": "main",
            "settings": { "width": 800, "height": 600 }
        }))
        .unwrap();
        match msg {
            IncomingMessage::WindowOp {
                op,
                window_id,
                settings,
            } => {
                assert_eq!(op, "resize");
                assert_eq!(window_id, "main");
                assert_eq!(settings["width"], 800);
                assert_eq!(settings["height"], 600);
            }
            _ => panic!("expected WindowOp"),
        }
    }

    #[test]
    fn deserialize_window_op_no_settings() {
        let json = r#"{"type":"window_op","op":"close","window_id":"popup"}"#;
        let msg: IncomingMessage = serde_json::from_str(json).unwrap();
        match msg {
            IncomingMessage::WindowOp {
                op,
                window_id,
                settings,
            } => {
                assert_eq!(op, "close");
                assert_eq!(window_id, "popup");
                assert!(settings.is_null());
            }
            _ => panic!("expected WindowOp"),
        }
    }

    #[test]
    fn deserialize_malformed_json_missing_field() {
        let json = r#"{"type":"snapshot"}"#;
        let result = serde_json::from_str::<IncomingMessage>(json);
        assert!(result.is_err());
    }

    #[test]
    fn deserialize_unknown_type_tag() {
        let json = r#"{"type":"bogus_message","data":42}"#;
        let result = serde_json::from_str::<IncomingMessage>(json);
        assert!(result.is_err());
    }

    #[test]
    fn deserialize_invalid_json_syntax() {
        let json = r#"{"type":"snapshot",,,}"#;
        let result = serde_json::from_str::<IncomingMessage>(json);
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // ExtensionCommand deserialization
    // -----------------------------------------------------------------------

    #[test]
    fn extension_command_deserializes() {
        let msg: IncomingMessage = serde_json::from_value(json!({
            "type": "extension_command",
            "node_id": "term-1",
            "op": "write",
            "payload": { "data": "hello" }
        }))
        .unwrap();
        match msg {
            IncomingMessage::ExtensionCommand {
                node_id,
                op,
                payload,
            } => {
                assert_eq!(node_id, "term-1");
                assert_eq!(op, "write");
                assert_eq!(payload["data"], "hello");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn extension_command_batch_deserializes() {
        let msg: IncomingMessage = serde_json::from_value(json!({
            "type": "extension_command_batch",
            "commands": [
                { "node_id": "term-1", "op": "write", "payload": { "data": "a" } },
                { "node_id": "log-1", "op": "append", "payload": { "line": "x" } }
            ]
        }))
        .unwrap();
        match msg {
            IncomingMessage::ExtensionCommandBatch { commands } => {
                assert_eq!(commands.len(), 2);
                assert_eq!(commands[0].node_id, "term-1");
                assert_eq!(commands[1].op, "append");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn extension_command_with_default_payload() {
        let json = r#"{"type":"extension_command","node_id":"ext-1","op":"reset"}"#;
        let msg: IncomingMessage = serde_json::from_str(json).unwrap();
        match msg {
            IncomingMessage::ExtensionCommand { payload, .. } => {
                assert!(payload.is_null());
            }
            _ => panic!("wrong variant"),
        }
    }
}
