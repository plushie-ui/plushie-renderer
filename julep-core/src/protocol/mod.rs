//! Wire protocol types for host-renderer communication.
//!
//! [`IncomingMessage`] is deserialized from the host. [`OutgoingEvent`]
//! and response types are serialized back. The transport (stdin/stdout,
//! socket, test harness) is handled by the binary crate, not here.
//!
//! Every wire message carries a `session` field identifying the logical
//! session it belongs to. [`SessionMessage`] pairs a session ID with a
//! deserialized [`IncomingMessage`]. All outgoing types include a
//! `session` field that echoes the originating session ID back.

mod incoming;
mod outgoing;
mod types;

/// Protocol version number. Sent in the `hello` handshake message on startup
/// and checked against the value the host embeds in Settings.
pub const PROTOCOL_VERSION: u32 = 1;

pub use incoming::{ExtensionCommandItem, IncomingMessage};
pub use outgoing::{
    EffectResponse, InteractResponse, KeyModifiers, OutgoingEvent, QueryResponse, ResetResponse,
    TreeHashResponse,
};
pub use types::{PatchOp, TreeNode};

/// An incoming message paired with its session ID.
///
/// The `session` field is extracted from the raw wire object before
/// deserializing the rest as [`IncomingMessage`]. This keeps
/// `IncomingMessage` free of session concerns -- the session is
/// routing metadata, not message content.
#[derive(Debug)]
pub struct SessionMessage {
    pub session: String,
    pub message: IncomingMessage,
}

impl SessionMessage {
    /// Extract `session` from a JSON value and deserialize the rest as
    /// [`IncomingMessage`].
    ///
    /// If the `session` key is absent, defaults to an empty string
    /// (single-session backwards compatibility).
    pub fn from_value(mut value: serde_json::Value) -> Result<Self, serde_json::Error> {
        let session = value
            .as_object_mut()
            .and_then(|obj| obj.remove("session"))
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();

        let message = serde_json::from_value(value)?;
        Ok(Self { session, message })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn session_message_extracts_session() {
        let val = json!({
            "session": "test_1",
            "type": "snapshot",
            "tree": {"id": "r", "type": "column", "props": {}, "children": []}
        });
        let sm = SessionMessage::from_value(val).unwrap();
        assert_eq!(sm.session, "test_1");
        assert!(matches!(sm.message, IncomingMessage::Snapshot { .. }));
    }

    #[test]
    fn session_message_defaults_to_empty() {
        let val = json!({
            "type": "reset",
            "id": "r1"
        });
        let sm = SessionMessage::from_value(val).unwrap();
        assert_eq!(sm.session, "");
        assert!(matches!(sm.message, IncomingMessage::Reset { .. }));
    }

    #[test]
    fn session_message_preserves_all_fields() {
        let val = json!({
            "session": "s42",
            "type": "query",
            "id": "q1",
            "target": "find",
            "selector": {"id": "btn"}
        });
        let sm = SessionMessage::from_value(val).unwrap();
        assert_eq!(sm.session, "s42");
        match sm.message {
            IncomingMessage::Query { id, target, .. } => {
                assert_eq!(id, "q1");
                assert_eq!(target, "find");
            }
            _ => panic!("expected Query"),
        }
    }

    #[test]
    fn outgoing_event_includes_session() {
        let evt = OutgoingEvent::click("btn".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["session"], "");
    }

    #[test]
    fn outgoing_event_with_session() {
        let evt = OutgoingEvent::click("btn".to_string()).with_session("s1".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["session"], "s1");
    }

    #[test]
    fn effect_response_includes_session() {
        let resp =
            EffectResponse::ok("e1".to_string(), json!("data")).with_session("s2".to_string());
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["session"], "s2");
    }

    #[test]
    fn reset_response_includes_session() {
        let resp = ResetResponse::ok("r1".to_string()).with_session("s3");
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["session"], "s3");
    }

    #[test]
    fn interact_response_propagates_session_to_events() {
        let events = vec![
            OutgoingEvent::click("btn".to_string()),
            OutgoingEvent::input("inp".to_string(), "text".to_string()),
        ];
        let resp = InteractResponse::new("i1".to_string(), events).with_session("s4");
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["session"], "s4");
        assert_eq!(json["events"][0]["session"], "s4");
        assert_eq!(json["events"][1]["session"], "s4");
    }

    #[test]
    fn session_message_rejects_non_object() {
        let val = json!([1, 2, 3]);
        let result = SessionMessage::from_value(val);
        assert!(result.is_err());
    }

    #[test]
    fn session_message_ignores_non_string_session() {
        let val = json!({
            "session": 42,
            "type": "reset",
            "id": "r1"
        });
        let sm = SessionMessage::from_value(val).unwrap();
        assert_eq!(sm.session, "");
    }
}
