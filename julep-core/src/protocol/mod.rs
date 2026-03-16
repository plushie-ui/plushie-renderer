mod outgoing;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// Replace non-finite f32 with 0.0 for safe JSON serialization.
pub(crate) fn sanitize_f32(v: f32) -> f32 {
    if v.is_finite() {
        v
    } else {
        log::warn!("non-finite f32 ({v}) replaced with 0.0 in outgoing event");
        0.0
    }
}

/// Replace non-finite f64 with 0.0 for safe JSON serialization.
pub(crate) fn sanitize_f64(v: f64) -> f64 {
    if v.is_finite() {
        v
    } else {
        log::warn!("non-finite f64 ({v}) replaced with 0.0 in outgoing event");
        0.0
    }
}

/// Protocol version number. Sent in the `hello` handshake message on startup
/// and checked against the value the host embeds in Settings.
pub const PROTOCOL_VERSION: u32 = 1;

/// Messages sent from the host to the renderer over stdin.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IncomingMessage {
    Snapshot {
        tree: TreeNode,
    },
    Patch {
        ops: Vec<PatchOp>,
    },
    EffectRequest {
        id: String,
        kind: String,
        payload: Value,
    },
    WidgetOp {
        op: String,
        #[serde(default)]
        payload: Value,
    },
    SubscriptionRegister {
        kind: String,
        tag: String,
    },
    SubscriptionUnregister {
        kind: String,
    },
    WindowOp {
        op: String,
        window_id: String,
        #[serde(default)]
        settings: Value,
    },
    Settings {
        settings: Value,
    },
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
    /// Capture a structural tree snapshot (hash of JSON tree).
    #[allow(dead_code)]
    SnapshotCapture {
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
    Reset {
        id: String,
    },
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
    ExtensionCommandBatch {
        commands: Vec<ExtensionCommandItem>,
    },
}

/// A single item within an `ExtensionCommandBatch`.
#[derive(Debug, Clone, Deserialize)]
pub struct ExtensionCommandItem {
    pub node_id: String,
    pub op: String,
    #[serde(default)]
    pub payload: Value,
}

/// Response to an effect request, written to stdout as JSONL.
#[derive(Debug, Serialize)]
pub struct EffectResponse {
    #[serde(rename = "type")]
    pub message_type: &'static str,
    pub id: String,
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl EffectResponse {
    pub fn ok(id: String, result: Value) -> Self {
        Self {
            message_type: "effect_response",
            id,
            status: "ok",
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: String, reason: String) -> Self {
        Self {
            message_type: "effect_response",
            id,
            status: "error",
            result: None,
            error: Some(reason),
        }
    }

    pub fn unsupported(id: String) -> Self {
        Self::error(id, "unsupported".to_string())
    }
}

/// Response to a Query message.
#[derive(Debug, Serialize)]
pub struct QueryResponse {
    #[serde(rename = "type")]
    pub message_type: &'static str,
    pub id: String,
    pub target: String,
    pub data: Value,
}

impl QueryResponse {
    pub fn new(id: String, target: String, data: Value) -> Self {
        Self {
            message_type: "query_response",
            id,
            target,
            data,
        }
    }
}

/// Response to an Interact message.
#[derive(Debug, Serialize)]
pub struct InteractResponse {
    #[serde(rename = "type")]
    pub message_type: &'static str,
    pub id: String,
    pub events: Vec<Value>,
}

impl InteractResponse {
    pub fn new(id: String, events: Vec<Value>) -> Self {
        Self {
            message_type: "interact_response",
            id,
            events,
        }
    }
}

/// Response to a SnapshotCapture message.
///
/// Snapshots capture structural tree data (hash of JSON tree). No pixel data.
/// For pixel data, see the `screenshot_response` message type.
#[derive(Debug, Serialize)]
#[allow(dead_code)]
pub struct SnapshotCaptureResponse {
    #[serde(rename = "type")]
    pub message_type: &'static str,
    pub id: String,
    pub name: String,
    pub hash: String,
    pub width: u32,
    pub height: u32,
}

#[allow(dead_code)]
impl SnapshotCaptureResponse {
    pub fn new(id: String, name: String, hash: String, width: u32, height: u32) -> Self {
        Self {
            message_type: "snapshot_response",
            id,
            name,
            hash,
            width,
            height,
        }
    }
}

/// Empty screenshot response for backends that cannot capture pixels.
///
/// Used by headless mode. The full backend uses a format-aware emit function
/// instead (native binary for msgpack, base64 for JSON).
#[derive(Debug, Serialize)]
#[allow(dead_code)]
pub struct ScreenshotResponseEmpty {
    #[serde(rename = "type")]
    pub message_type: &'static str,
    pub id: String,
    pub name: String,
    pub hash: String,
    pub width: u32,
    pub height: u32,
}

#[allow(dead_code)]
impl ScreenshotResponseEmpty {
    pub fn new(id: String, name: String) -> Self {
        Self {
            message_type: "screenshot_response",
            id,
            name,
            hash: String::new(),
            width: 0,
            height: 0,
        }
    }
}

/// Emit a screenshot response with RGBA pixel data to stdout.
///
/// Uses native msgpack binary (`rmpv::Value::Binary`) for pixel data when the
/// wire codec is MsgPack, and base64-encoded string when JSON. This avoids the
/// ~33% size overhead of base64 on the hot path.
///
/// Shared between test-mode (main.rs) and headless (headless.rs).
#[allow(dead_code)]
pub fn emit_screenshot_response(
    id: &str,
    name: &str,
    hash: &str,
    width: u32,
    height: u32,
    rgba_bytes: &[u8],
) {
    use std::io::Write;

    let codec = crate::codec::Codec::get_global();
    let bytes = match codec {
        crate::codec::Codec::MsgPack => {
            use rmpv::Value as RmpvValue;

            let mut entries = vec![
                (
                    RmpvValue::String("type".into()),
                    RmpvValue::String("screenshot_response".into()),
                ),
                (RmpvValue::String("id".into()), RmpvValue::String(id.into())),
                (
                    RmpvValue::String("name".into()),
                    RmpvValue::String(name.into()),
                ),
                (
                    RmpvValue::String("hash".into()),
                    RmpvValue::String(hash.into()),
                ),
                (
                    RmpvValue::String("width".into()),
                    RmpvValue::Integer((width as i64).into()),
                ),
                (
                    RmpvValue::String("height".into()),
                    RmpvValue::Integer((height as i64).into()),
                ),
            ];
            if !rgba_bytes.is_empty() {
                entries.push((
                    RmpvValue::String("rgba".into()),
                    RmpvValue::Binary(rgba_bytes.to_vec()),
                ));
            }
            let map = RmpvValue::Map(entries);

            let mut payload = Vec::new();
            if let Err(e) = rmpv::encode::write_value(&mut payload, &map) {
                log::error!("msgpack encode screenshot: {e}");
                return;
            }
            let len = match u32::try_from(payload.len()) {
                Ok(n) => n,
                Err(_) => {
                    log::error!(
                        "screenshot payload exceeds u32::MAX ({} bytes)",
                        payload.len()
                    );
                    return;
                }
            };
            let mut bytes = Vec::with_capacity(4 + payload.len());
            bytes.extend_from_slice(&len.to_be_bytes());
            bytes.extend_from_slice(&payload);
            bytes
        }
        crate::codec::Codec::Json => {
            use base64::Engine;

            let mut map = serde_json::json!({
                "type": "screenshot_response",
                "id": id,
                "name": name,
                "hash": hash,
                "width": width,
                "height": height,
            });
            if !rgba_bytes.is_empty() {
                let b64 = base64::engine::general_purpose::STANDARD.encode(rgba_bytes);
                map["rgba"] = serde_json::Value::String(b64);
            }
            match serde_json::to_vec(&map) {
                Ok(mut bytes) => {
                    bytes.push(b'\n');
                    bytes
                }
                Err(e) => {
                    log::error!("json encode screenshot: {e}");
                    return;
                }
            }
        }
    };

    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    if let Err(e) = handle.write_all(&bytes) {
        if e.kind() == std::io::ErrorKind::BrokenPipe {
            log::error!("stdout broken pipe -- shutting down");
            std::process::exit(0);
        }
        log::error!("stdout write error: {e}");
        return;
    }
    if let Err(e) = handle.flush() {
        if e.kind() == std::io::ErrorKind::BrokenPipe {
            log::error!("stdout broken pipe on flush -- shutting down");
            std::process::exit(0);
        }
        log::error!("stdout flush error: {e}");
    }
}

/// Response to a Reset message.
#[derive(Debug, Serialize)]
pub struct ResetResponse {
    #[serde(rename = "type")]
    pub message_type: &'static str,
    pub id: String,
    pub status: &'static str,
}

impl ResetResponse {
    pub fn ok(id: String) -> Self {
        Self {
            message_type: "reset_response",
            id,
            status: "ok",
        }
    }
}

/// A single node in the UI tree.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TreeNode {
    pub id: String,
    #[serde(rename = "type")]
    pub type_name: String,
    #[serde(default)]
    pub props: Value,
    #[serde(default)]
    pub children: Vec<TreeNode>,
}

/// A single patch operation applied incrementally to the retained tree.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PatchOp {
    pub op: String,
    pub path: Vec<usize>,
    #[serde(flatten)]
    pub rest: Value,
}

/// Events written to stdout by the renderer.
#[derive(Debug, Serialize)]
pub struct OutgoingEvent {
    #[serde(rename = "type")]
    pub message_type: &'static str,
    pub family: String,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modifiers: Option<KeyModifiers>,
    /// Flexible extra data for events that carry additional fields beyond
    /// the standard id/value/tag/modifiers shape.  Serialized as a nested
    /// `"data"` object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    /// Whether the event was captured (consumed) by an iced widget before
    /// reaching the subscription listener.  Present on keyboard, mouse,
    /// touch, and IME events; absent on widget-level events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub captured: Option<bool>,
}

impl OutgoingEvent {
    /// Mark the event with its capture status.
    pub fn with_captured(mut self, captured: bool) -> Self {
        self.captured = Some(captured);
        self
    }
}

/// Serializable representation of keyboard modifiers.
#[derive(Debug, Serialize)]
pub struct KeyModifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub logo: bool,
    pub command: bool,
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
        let json = r#"{"type":"snapshot","tree":{"id":"root","type":"column","props":{"spacing":10},"children":[{"id":"c1","type":"text","props":{"content":"hello"},"children":[]}]}}"#;
        let msg: IncomingMessage = serde_json::from_str(json).unwrap();
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
        let json = r#"{"type":"patch","ops":[{"op":"replace_node","path":[0],"node":{"id":"x","type":"text","props":{},"children":[]}}]}"#;
        let msg: IncomingMessage = serde_json::from_str(json).unwrap();
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
        let json = r#"{"type":"patch","ops":[{"op":"update_props","path":[0],"props":{"color":"red"}},{"op":"remove_child","path":[],"index":2}]}"#;
        let msg: IncomingMessage = serde_json::from_str(json).unwrap();
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
        let json = r#"{"type":"effect_request","id":"e2","kind":"clipboard_write","payload":{"text":"copied"}}"#;
        let msg: IncomingMessage = serde_json::from_str(json).unwrap();
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
        let json = r#"{"type":"window_op","op":"resize","window_id":"main","settings":{"width":800,"height":600}}"#;
        let msg: IncomingMessage = serde_json::from_str(json).unwrap();
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
    // TreeNode deserialization
    // -----------------------------------------------------------------------

    #[test]
    fn tree_node_full() {
        let json = r#"{"id":"root","type":"column","props":{"spacing":10},"children":[{"id":"c1","type":"text","props":{"content":"hi"},"children":[]}]}"#;
        let node: TreeNode = serde_json::from_str(json).unwrap();
        assert_eq!(node.id, "root");
        assert_eq!(node.type_name, "column");
        assert_eq!(node.children.len(), 1);
        assert_eq!(node.children[0].id, "c1");
        assert_eq!(node.props["spacing"], 10);
    }

    #[test]
    fn tree_node_defaults_props_and_children() {
        let json = r#"{"id":"x","type":"text"}"#;
        let node: TreeNode = serde_json::from_str(json).unwrap();
        assert_eq!(node.id, "x");
        assert_eq!(node.type_name, "text");
        assert!(node.children.is_empty());
    }

    #[test]
    fn tree_node_deeply_nested() {
        let json = r#"{"id":"a","type":"column","children":[{"id":"b","type":"row","children":[{"id":"c","type":"text"}]}]}"#;
        let node: TreeNode = serde_json::from_str(json).unwrap();
        assert_eq!(node.children[0].children[0].id, "c");
    }

    // -----------------------------------------------------------------------
    // PatchOp deserialization
    // -----------------------------------------------------------------------

    #[test]
    fn patch_op_replace_node() {
        let json = r#"{"op":"replace_node","path":[1,2],"node":{"id":"n","type":"text"}}"#;
        let op: PatchOp = serde_json::from_str(json).unwrap();
        assert_eq!(op.op, "replace_node");
        assert_eq!(op.path, vec![1, 2]);
        assert!(op.rest.get("node").is_some());
    }

    #[test]
    fn patch_op_update_props() {
        let json = r#"{"op":"update_props","path":[0],"props":{"color":"red"}}"#;
        let op: PatchOp = serde_json::from_str(json).unwrap();
        assert_eq!(op.op, "update_props");
        assert_eq!(op.rest["props"]["color"], "red");
    }

    #[test]
    fn patch_op_insert_child() {
        let json =
            r#"{"op":"insert_child","path":[],"index":0,"node":{"id":"new","type":"button"}}"#;
        let op: PatchOp = serde_json::from_str(json).unwrap();
        assert_eq!(op.op, "insert_child");
        assert!(op.path.is_empty());
        assert_eq!(op.rest["index"], 0);
    }

    #[test]
    fn patch_op_remove_child() {
        let json = r#"{"op":"remove_child","path":[0],"index":1}"#;
        let op: PatchOp = serde_json::from_str(json).unwrap();
        assert_eq!(op.op, "remove_child");
        assert_eq!(op.rest["index"], 1);
    }

    // -----------------------------------------------------------------------
    // OutgoingEvent serialization -- widget events
    // -----------------------------------------------------------------------

    #[test]
    fn serialize_click_event() {
        let evt = OutgoingEvent::click("btn1".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["type"], "event");
        assert_eq!(json["family"], "click");
        assert_eq!(json["id"], "btn1");
        assert!(json.get("value").is_none());
        assert!(json.get("tag").is_none());
        assert!(json.get("modifiers").is_none());
    }

    #[test]
    fn serialize_input_event() {
        let evt = OutgoingEvent::input("inp1".to_string(), "hello".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "input");
        assert_eq!(json["id"], "inp1");
        assert_eq!(json["value"], "hello");
    }

    #[test]
    fn serialize_submit_event() {
        let evt = OutgoingEvent::submit("form1".to_string(), "data".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "submit");
        assert_eq!(json["value"], "data");
    }

    #[test]
    fn serialize_toggle_event_true() {
        let evt = OutgoingEvent::toggle("chk1".to_string(), true);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "toggle");
        assert_eq!(json["value"], true);
    }

    #[test]
    fn serialize_toggle_event_false() {
        let evt = OutgoingEvent::toggle("chk1".to_string(), false);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["value"], false);
    }

    #[test]
    fn serialize_slide_event() {
        let evt = OutgoingEvent::slide("slider1".to_string(), 0.75);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "slide");
        assert_eq!(json["value"], 0.75);
    }

    #[test]
    fn serialize_slide_release_event() {
        let evt = OutgoingEvent::slide_release("slider1".to_string(), 0.5);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "slide_release");
        assert_eq!(json["value"], 0.5);
    }

    #[test]
    fn serialize_select_event() {
        let evt = OutgoingEvent::select("picker1".to_string(), "option_b".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "select");
        assert_eq!(json["value"], "option_b");
    }

    // -----------------------------------------------------------------------
    // OutgoingEvent serialization -- keyboard events
    // -----------------------------------------------------------------------

    fn make_key_event_data(key_str: &str, shift: bool, alt: bool) -> crate::message::KeyEventData {
        use iced::keyboard;
        crate::message::KeyEventData {
            key: if key_str.len() == 1 {
                keyboard::Key::Character(key_str.into())
            } else {
                keyboard::Key::Named(keyboard::key::Named::Escape)
            },
            modified_key: if key_str.len() == 1 {
                keyboard::Key::Character(key_str.to_uppercase().into())
            } else {
                keyboard::Key::Named(keyboard::key::Named::Escape)
            },
            physical_key: keyboard::key::Physical::Code(keyboard::key::Code::KeyA),
            location: keyboard::Location::Standard,
            modifiers: {
                let mut m = keyboard::Modifiers::empty();
                if shift {
                    m |= keyboard::Modifiers::SHIFT;
                }
                if alt {
                    m |= keyboard::Modifiers::ALT;
                }
                m
            },
            text: if key_str.len() == 1 {
                Some(key_str.to_string())
            } else {
                None
            },
            repeat: false,
            captured: false,
        }
    }

    #[test]
    fn serialize_key_press_with_modifiers() {
        let data = make_key_event_data("a", true, true);
        let evt = OutgoingEvent::key_press("keys".to_string(), &data);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "key_press");
        assert_eq!(json["tag"], "keys");
        assert_eq!(json["value"], "a");
        assert!(json["id"].as_str().unwrap().is_empty());
        assert_eq!(json["modifiers"]["shift"], true);
        assert_eq!(json["modifiers"]["ctrl"], false);
        assert_eq!(json["modifiers"]["alt"], true);
        assert_eq!(json["modifiers"]["logo"], false);
        assert_eq!(json["modifiers"]["command"], false);
        // New fields (nested under "data")
        assert_eq!(json["data"]["modified_key"], "A");
        assert_eq!(json["data"]["physical_key"], "KeyA");
        assert_eq!(json["data"]["location"], "standard");
        assert_eq!(json["data"]["text"], "a");
        assert_eq!(json["data"]["repeat"], false);
    }

    #[test]
    fn serialize_key_release() {
        let data = make_key_event_data("Escape", false, false);
        let evt = OutgoingEvent::key_release("keys".to_string(), &data);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "key_release");
        assert_eq!(json["value"], "Escape");
        // key_release should not have text or repeat
        assert!(json.get("text").is_none() || json["text"].is_null());
    }

    #[test]
    fn serialize_modifiers_changed() {
        let mods = KeyModifiers {
            shift: true,
            ctrl: true,
            alt: false,
            logo: false,
            command: false,
        };
        let evt = OutgoingEvent::modifiers_changed("mods".to_string(), mods);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "modifiers_changed");
        assert!(json.get("value").is_none());
        assert_eq!(json["modifiers"]["shift"], true);
        assert_eq!(json["modifiers"]["ctrl"], true);
    }

    // -----------------------------------------------------------------------
    // OutgoingEvent serialization -- mouse events
    // -----------------------------------------------------------------------

    #[test]
    fn serialize_cursor_moved() {
        let evt = OutgoingEvent::cursor_moved("mouse".to_string(), 100.0, 200.0);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "cursor_moved");
        assert_eq!(json["data"]["x"], 100.0);
        assert_eq!(json["data"]["y"], 200.0);
    }

    #[test]
    fn serialize_cursor_entered() {
        let evt = OutgoingEvent::cursor_entered("mouse".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "cursor_entered");
        assert_eq!(json["tag"], "mouse");
    }

    #[test]
    fn serialize_cursor_left() {
        let evt = OutgoingEvent::cursor_left("mouse".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "cursor_left");
    }

    #[test]
    fn serialize_button_pressed() {
        let evt = OutgoingEvent::button_pressed("mouse".to_string(), "Left".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "button_pressed");
        assert_eq!(json["value"], "Left");
    }

    #[test]
    fn serialize_button_released() {
        let evt = OutgoingEvent::button_released("mouse".to_string(), "Right".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "button_released");
        assert_eq!(json["value"], "Right");
    }

    #[test]
    fn serialize_wheel_scrolled() {
        let evt = OutgoingEvent::wheel_scrolled("mouse".to_string(), 0.0, -3.0, "line");
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "wheel_scrolled");
        assert_eq!(json["data"]["delta_x"], 0.0);
        assert_eq!(json["data"]["delta_y"], -3.0);
        assert_eq!(json["data"]["unit"], "line");
    }

    // -----------------------------------------------------------------------
    // OutgoingEvent serialization -- touch events
    // -----------------------------------------------------------------------

    #[test]
    fn serialize_finger_pressed() {
        let evt = OutgoingEvent::finger_pressed("touch".to_string(), 1, 50.0, 75.0);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "finger_pressed");
        assert_eq!(json["data"]["finger_id"], 1);
        assert_eq!(json["data"]["x"], 50.0);
        assert_eq!(json["data"]["y"], 75.0);
    }

    #[test]
    fn serialize_finger_moved() {
        let evt = OutgoingEvent::finger_moved("touch".to_string(), 2, 60.0, 80.0);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "finger_moved");
        assert_eq!(json["data"]["finger_id"], 2);
    }

    #[test]
    fn serialize_finger_lifted() {
        let evt = OutgoingEvent::finger_lifted("touch".to_string(), 1, 55.0, 78.0);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "finger_lifted");
    }

    #[test]
    fn serialize_finger_lost() {
        let evt = OutgoingEvent::finger_lost("touch".to_string(), 3, 0.0, 0.0);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "finger_lost");
        assert_eq!(json["data"]["finger_id"], 3);
    }

    // -----------------------------------------------------------------------
    // OutgoingEvent serialization -- window lifecycle events
    // -----------------------------------------------------------------------

    #[test]
    fn serialize_window_opened_with_position() {
        let evt = OutgoingEvent::window_opened(
            "win_events".to_string(),
            "main".to_string(),
            Some((10.0, 20.0)),
            800.0,
            600.0,
        );
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "window_opened");
        assert_eq!(json["data"]["window_id"], "main");
        assert_eq!(json["data"]["width"], 800.0);
        assert_eq!(json["data"]["height"], 600.0);
        assert_eq!(json["data"]["position"]["x"], 10.0);
        assert_eq!(json["data"]["position"]["y"], 20.0);
    }

    #[test]
    fn serialize_window_opened_without_position() {
        let evt = OutgoingEvent::window_opened(
            "win_events".to_string(),
            "main".to_string(),
            None,
            1024.0,
            768.0,
        );
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "window_opened");
        assert!(json["data"]["position"].is_null());
    }

    #[test]
    fn serialize_window_closed() {
        let evt = OutgoingEvent::window_closed("win_events".to_string(), "popup".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "window_closed");
        assert_eq!(json["data"]["window_id"], "popup");
    }

    #[test]
    fn serialize_window_close_requested() {
        let evt =
            OutgoingEvent::window_close_requested("win_events".to_string(), "main".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "window_close_requested");
    }

    #[test]
    fn serialize_window_moved() {
        let evt =
            OutgoingEvent::window_moved("win_events".to_string(), "main".to_string(), 50.0, 100.0);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "window_moved");
        assert_eq!(json["data"]["x"], 50.0);
        assert_eq!(json["data"]["y"], 100.0);
    }

    #[test]
    fn serialize_window_resized() {
        let evt = OutgoingEvent::window_resized(
            "win_events".to_string(),
            "main".to_string(),
            1920.0,
            1080.0,
        );
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "window_resized");
        assert_eq!(json["data"]["width"], 1920.0);
    }

    #[test]
    fn serialize_window_focused() {
        let evt = OutgoingEvent::window_focused("win_events".to_string(), "main".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "window_focused");
    }

    #[test]
    fn serialize_window_unfocused() {
        let evt = OutgoingEvent::window_unfocused("win_events".to_string(), "main".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "window_unfocused");
    }

    #[test]
    fn serialize_window_rescaled() {
        let evt = OutgoingEvent::window_rescaled("win_events".to_string(), "main".to_string(), 2.0);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "window_rescaled");
        assert_eq!(json["data"]["scale_factor"], 2.0);
    }

    #[test]
    fn serialize_file_hovered() {
        let evt = OutgoingEvent::file_hovered(
            "win_events".to_string(),
            "main".to_string(),
            "/tmp/a.txt".to_string(),
        );
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "file_hovered");
        assert_eq!(json["data"]["path"], "/tmp/a.txt");
    }

    #[test]
    fn serialize_file_dropped() {
        let evt = OutgoingEvent::file_dropped(
            "win_events".to_string(),
            "main".to_string(),
            "/tmp/b.txt".to_string(),
        );
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "file_dropped");
        assert_eq!(json["data"]["path"], "/tmp/b.txt");
    }

    #[test]
    fn serialize_files_hovered_left() {
        let evt = OutgoingEvent::files_hovered_left("win_events".to_string(), "main".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "files_hovered_left");
    }

    // -----------------------------------------------------------------------
    // OutgoingEvent serialization -- sensor events
    // -----------------------------------------------------------------------

    #[test]
    fn serialize_sensor_resize() {
        let evt = OutgoingEvent::sensor_resize("s1".to_string(), 100.0, 200.0);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "sensor_resize");
        assert_eq!(json["id"], "s1");
        assert_eq!(json["data"]["width"], 100.0);
        assert_eq!(json["data"]["height"], 200.0);
    }

    // -----------------------------------------------------------------------
    // OutgoingEvent serialization -- canvas events
    // -----------------------------------------------------------------------

    #[test]
    fn serialize_canvas_press() {
        let evt = OutgoingEvent::canvas_press("c1".to_string(), 10.0, 20.0, "Left".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "canvas_press");
        assert_eq!(json["data"]["x"], 10.0);
        assert_eq!(json["data"]["button"], "Left");
    }

    #[test]
    fn serialize_canvas_release() {
        let evt = OutgoingEvent::canvas_release("c1".to_string(), 10.0, 20.0, "Left".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "canvas_release");
    }

    #[test]
    fn serialize_canvas_move() {
        let evt = OutgoingEvent::canvas_move("c1".to_string(), 30.0, 40.0);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "canvas_move");
        assert_eq!(json["data"]["x"], 30.0);
        assert_eq!(json["data"]["y"], 40.0);
    }

    #[test]
    fn serialize_canvas_scroll() {
        let evt = OutgoingEvent::canvas_scroll("c1".to_string(), 5.0, 5.0, 0.0, -1.0);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "canvas_scroll");
        assert_eq!(json["data"]["delta_y"], -1.0);
    }

    // -----------------------------------------------------------------------
    // OutgoingEvent serialization -- mouse area events
    // -----------------------------------------------------------------------

    #[test]
    fn serialize_mouse_right_press() {
        let evt = OutgoingEvent::mouse_right_press("zone".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "mouse_right_press");
        assert_eq!(json["id"], "zone");
    }

    #[test]
    fn serialize_mouse_right_release() {
        let evt = OutgoingEvent::mouse_right_release("zone".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "mouse_right_release");
    }

    #[test]
    fn serialize_mouse_middle_press() {
        let evt = OutgoingEvent::mouse_middle_press("zone".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "mouse_middle_press");
        assert_eq!(json["id"], "zone");
    }

    #[test]
    fn serialize_mouse_middle_release() {
        let evt = OutgoingEvent::mouse_middle_release("zone".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "mouse_middle_release");
    }

    #[test]
    fn serialize_mouse_double_click() {
        let evt = OutgoingEvent::mouse_double_click("zone".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "mouse_double_click");
    }

    #[test]
    fn serialize_mouse_enter() {
        let evt = OutgoingEvent::mouse_enter("zone".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "mouse_enter");
    }

    #[test]
    fn serialize_mouse_exit() {
        let evt = OutgoingEvent::mouse_exit("zone".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "mouse_exit");
    }

    #[test]
    fn serialize_mouse_area_move() {
        let evt = OutgoingEvent::mouse_area_move("zone".to_string(), 10.5, 20.3);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "mouse_move");
        assert_eq!(json["id"], "zone");
        let data = &json["data"];
        assert!((data["x"].as_f64().unwrap() - 10.5).abs() < 0.01);
        assert!((data["y"].as_f64().unwrap() - 20.3).abs() < 0.01);
    }

    #[test]
    fn serialize_mouse_area_scroll() {
        let evt = OutgoingEvent::mouse_area_scroll("zone".to_string(), 0.0, -3.0);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "mouse_scroll");
        assert_eq!(json["id"], "zone");
        assert_eq!(json["data"]["delta_x"], 0.0);
        assert_eq!(json["data"]["delta_y"], -3.0);
    }

    // -----------------------------------------------------------------------
    // OutgoingEvent serialization -- pane grid events
    // -----------------------------------------------------------------------

    #[test]
    fn serialize_pane_resized() {
        let evt = OutgoingEvent::pane_resized("pg1".to_string(), "split_0".to_string(), 0.5);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "pane_resized");
        assert_eq!(json["data"]["split"], "split_0");
        assert_eq!(json["data"]["ratio"], json!(0.5));
    }

    #[test]
    fn serialize_pane_dragged_dropped() {
        let evt = OutgoingEvent::pane_dragged(
            "pg1".to_string(),
            "dropped",
            "pane_a".to_string(),
            Some("pane_b".to_string()),
            Some("center"),
            None,
        );
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "pane_dragged");
        assert_eq!(json["data"]["kind"], "dropped");
        assert_eq!(json["data"]["pane"], "pane_a");
        assert_eq!(json["data"]["target"], "pane_b");
        assert_eq!(json["data"]["region"], "center");
    }

    #[test]
    fn serialize_pane_dragged_picked() {
        let evt = OutgoingEvent::pane_dragged(
            "pg1".to_string(),
            "picked",
            "pane_a".to_string(),
            None,
            None,
            None,
        );
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["data"]["kind"], "picked");
        assert_eq!(json["data"]["pane"], "pane_a");
        assert!(json["data"].get("target").is_none());
    }

    #[test]
    fn serialize_pane_dragged_canceled() {
        let evt = OutgoingEvent::pane_dragged(
            "pg1".to_string(),
            "canceled",
            "pane_a".to_string(),
            None,
            None,
            None,
        );
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["data"]["kind"], "canceled");
    }

    #[test]
    fn serialize_pane_focus_cycle() {
        let evt = OutgoingEvent::pane_focus_cycle("pg1".to_string(), "pane_a".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "pane_focus_cycle");
        assert_eq!(json["data"]["pane"], "pane_a");
    }

    #[test]
    fn serialize_pane_clicked() {
        let evt = OutgoingEvent::pane_clicked("pg1".to_string(), "pane_x".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "pane_clicked");
        assert_eq!(json["data"]["pane"], "pane_x");
    }

    // -----------------------------------------------------------------------
    // OutgoingEvent serialization -- animation/theme events
    // -----------------------------------------------------------------------

    #[test]
    fn serialize_animation_frame() {
        let evt = OutgoingEvent::animation_frame("anim".to_string(), 16000);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "animation_frame");
        assert_eq!(json["data"]["timestamp"], 16000);
    }

    #[test]
    fn serialize_theme_changed() {
        let evt = OutgoingEvent::theme_changed("theme".to_string(), "dark".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "theme_changed");
        assert_eq!(json["value"], "dark");
    }

    // -----------------------------------------------------------------------
    // EffectResponse serialization
    // -----------------------------------------------------------------------

    #[test]
    fn effect_response_ok() {
        let resp = EffectResponse::ok("e1".to_string(), json!("clipboard content"));
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["type"], "effect_response");
        assert_eq!(json["id"], "e1");
        assert_eq!(json["status"], "ok");
        assert_eq!(json["result"], "clipboard content");
        assert!(json.get("error").is_none());
    }

    #[test]
    fn effect_response_error() {
        let resp = EffectResponse::error("e2".to_string(), "not found".to_string());
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["type"], "effect_response");
        assert_eq!(json["id"], "e2");
        assert_eq!(json["status"], "error");
        assert_eq!(json["error"], "not found");
        assert!(json.get("result").is_none());
    }

    #[test]
    fn effect_response_unsupported() {
        let resp = EffectResponse::unsupported("e3".to_string());
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["status"], "error");
        assert_eq!(json["error"], "unsupported");
    }

    #[test]
    fn effect_response_ok_with_object_result() {
        let resp = EffectResponse::ok("e4".to_string(), json!({"files": ["/a.txt", "/b.txt"]}));
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["result"]["files"][0], "/a.txt");
        assert_eq!(json["result"]["files"][1], "/b.txt");
    }

    // -----------------------------------------------------------------------
    // Round-trip: serialize then deserialize OutgoingEvent as generic Value
    // -----------------------------------------------------------------------

    #[test]
    fn outgoing_event_roundtrip_all_fields_present() {
        let data = make_key_event_data("a", true, false);
        let evt = OutgoingEvent::key_press("kb".to_string(), &data);
        let serialized = serde_json::to_string(&evt).unwrap();
        let parsed: Value = serde_json::from_str(&serialized).unwrap();
        assert_eq!(parsed["type"], "event");
        assert_eq!(parsed["family"], "key_press");
        assert_eq!(parsed["value"], "a");
        assert_eq!(parsed["tag"], "kb");
        assert_eq!(parsed["modifiers"]["shift"], true);
        // Extra fields from KeyEventData (nested under "data")
        assert!(parsed["data"].get("modified_key").is_some());
        assert!(parsed["data"].get("physical_key").is_some());
        assert!(parsed["data"].get("location").is_some());
    }

    #[test]
    fn ime_opened_event() {
        let evt = OutgoingEvent::ime_opened("ime_tag".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["type"], "event");
        assert_eq!(json["family"], "ime");
        assert_eq!(json["tag"], "ime_tag");
        assert_eq!(json["data"]["kind"], "opened");
    }

    #[test]
    fn ime_preedit_event_with_cursor() {
        let evt =
            OutgoingEvent::ime_preedit("ime_tag".to_string(), "hello".to_string(), Some(2..5));
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "ime");
        assert_eq!(json["data"]["kind"], "preedit");
        assert_eq!(json["data"]["text"], "hello");
        assert_eq!(json["data"]["cursor"]["start"], 2);
        assert_eq!(json["data"]["cursor"]["end"], 5);
    }

    #[test]
    fn ime_preedit_event_without_cursor() {
        let evt = OutgoingEvent::ime_preedit("ime_tag".to_string(), "hi".to_string(), None);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["data"]["kind"], "preedit");
        assert_eq!(json["data"]["text"], "hi");
        assert!(json["data"]["cursor"].is_null());
    }

    #[test]
    fn ime_commit_event() {
        let evt = OutgoingEvent::ime_commit("ime_tag".to_string(), "final".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "ime");
        assert_eq!(json["data"]["kind"], "commit");
        assert_eq!(json["data"]["text"], "final");
    }

    #[test]
    fn ime_closed_event() {
        let evt = OutgoingEvent::ime_closed("ime_tag".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "ime");
        assert_eq!(json["data"]["kind"], "closed");
    }

    #[test]
    fn outgoing_event_bare_omits_optional_fields() {
        let evt = OutgoingEvent::click("b".to_string());
        let serialized = serde_json::to_string(&evt).unwrap();
        // value, tag, modifiers, captured should all be absent from the JSON string
        assert!(!serialized.contains("\"value\""));
        assert!(!serialized.contains("\"tag\""));
        assert!(!serialized.contains("\"modifiers\""));
        assert!(!serialized.contains("\"captured\""));
    }

    #[test]
    fn outgoing_event_with_captured_true() {
        let evt = OutgoingEvent::cursor_moved("m".to_string(), 1.0, 2.0).with_captured(true);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["captured"], true);
    }

    #[test]
    fn outgoing_event_with_captured_false() {
        let evt =
            OutgoingEvent::key_press("kb".to_string(), &make_key_event_data("a", false, false))
                .with_captured(false);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["captured"], false);
    }

    #[test]
    fn outgoing_event_without_captured_omits_field() {
        let evt = OutgoingEvent::click("btn".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert!(json.get("captured").is_none());
    }

    // -----------------------------------------------------------------------
    // ExtensionCommand deserialization
    // -----------------------------------------------------------------------

    #[test]
    fn extension_command_deserializes() {
        let json = r#"{"type":"extension_command","node_id":"term-1","op":"write","payload":{"data":"hello"}}"#;
        let msg: IncomingMessage = serde_json::from_str(json).unwrap();
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
        let json = r#"{"type":"extension_command_batch","commands":[{"node_id":"term-1","op":"write","payload":{"data":"a"}},{"node_id":"log-1","op":"append","payload":{"line":"x"}}]}"#;
        let msg: IncomingMessage = serde_json::from_str(json).unwrap();
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

    // -- float sanitization --

    #[test]
    fn sanitize_f32_passes_finite() {
        assert_eq!(super::sanitize_f32(1.5), 1.5);
        assert_eq!(super::sanitize_f32(-0.0), -0.0);
        assert_eq!(super::sanitize_f32(0.0), 0.0);
    }

    #[test]
    fn sanitize_f32_replaces_nan() {
        assert_eq!(super::sanitize_f32(f32::NAN), 0.0);
    }

    #[test]
    fn sanitize_f32_replaces_infinity() {
        assert_eq!(super::sanitize_f32(f32::INFINITY), 0.0);
        assert_eq!(super::sanitize_f32(f32::NEG_INFINITY), 0.0);
    }

    #[test]
    fn sanitize_f64_passes_finite() {
        assert_eq!(super::sanitize_f64(42.0), 42.0);
    }

    #[test]
    fn sanitize_f64_replaces_nan() {
        assert_eq!(super::sanitize_f64(f64::NAN), 0.0);
    }

    #[test]
    fn sanitize_f64_replaces_infinity() {
        assert_eq!(super::sanitize_f64(f64::INFINITY), 0.0);
        assert_eq!(super::sanitize_f64(f64::NEG_INFINITY), 0.0);
    }

    #[test]
    fn outgoing_slide_with_nan_produces_zero() {
        let event = OutgoingEvent::slide("s1".to_string(), f64::NAN);
        // The value should be 0.0, not NaN
        let val = event.value.unwrap();
        assert_eq!(val.as_f64(), Some(0.0));
    }

    #[test]
    fn outgoing_cursor_moved_with_infinity_produces_zero() {
        let event =
            OutgoingEvent::cursor_moved("tag".to_string(), f32::INFINITY, f32::NEG_INFINITY);
        let data = event.data.unwrap();
        assert_eq!(data["x"].as_f64(), Some(0.0));
        assert_eq!(data["y"].as_f64(), Some(0.0));
    }
}
