use std::io::{self, Write};

use julep_core::codec::Codec;
use julep_core::message::Message;
use julep_core::protocol::OutgoingEvent;

// ---------------------------------------------------------------------------
// stdout write helper
// ---------------------------------------------------------------------------

/// Write pre-encoded bytes to stdout. Exits the process on broken pipe
/// (the host process has gone away and there's nothing useful to do).
///
/// Each call acquires the stdout lock and flushes. This is correct for
/// the common case (one event per update cycle) but suboptimal when
/// apply() produces multiple effects. A future optimization could batch
/// writes within apply() using a single lock/flush cycle.
pub(crate) fn write_stdout(bytes: &[u8]) {
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    if let Err(e) = handle.write_all(bytes) {
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

// ---------------------------------------------------------------------------
// stdout event emitter
// ---------------------------------------------------------------------------

pub(crate) fn emit_event(event: OutgoingEvent) {
    let codec = Codec::get_global();
    match codec.encode(&event) {
        Ok(bytes) => write_stdout(&bytes),
        Err(e) => log::error!("failed to serialize event: {e}"),
    }
}

// ---------------------------------------------------------------------------
// stdout hello message emitter
// ---------------------------------------------------------------------------

/// Emit a `hello` handshake message to stdout immediately after codec
/// negotiation. This tells the host which protocol version and
/// renderer build it is talking to.
pub(crate) fn emit_hello() {
    let msg = serde_json::json!({
        "type": "hello",
        "protocol": julep_core::protocol::PROTOCOL_VERSION,
        "version": env!("CARGO_PKG_VERSION"),
        "name": "julep-renderer",
    });
    let codec = Codec::get_global();
    match codec.encode(&msg) {
        Ok(bytes) => write_stdout(&bytes),
        Err(e) => log::error!("failed to serialize hello: {e}"),
    }
}

// ---------------------------------------------------------------------------
// stdout effect response emitter
// ---------------------------------------------------------------------------

pub(crate) fn emit_effect_response(response: julep_core::protocol::EffectResponse) {
    let codec = Codec::get_global();
    match codec.encode(&response) {
        Ok(bytes) => write_stdout(&bytes),
        Err(e) => log::error!("failed to serialize effect response: {e}"),
    }
}

/// Emit a query_response message to stdout. Used for system-level queries
/// (system_info, system_theme) that are not window-specific.
pub(crate) fn emit_query_response(kind: &str, tag: &str, data: serde_json::Value) {
    let msg = serde_json::json!({
        "type": "query_response",
        "kind": kind,
        "tag": tag,
        "data": data,
    });
    let codec = Codec::get_global();
    match codec.encode(&msg) {
        Ok(bytes) => write_stdout(&bytes),
        Err(e) => log::error!("failed to serialize query response: {e}"),
    }
}

// ---------------------------------------------------------------------------
// stdout screenshot response emitter -- delegates to shared protocol function
// ---------------------------------------------------------------------------

/// Emit a screenshot_response to stdout (thin wrapper around shared function).
#[cfg(feature = "test-mode")]
pub(crate) fn emit_screenshot_response(
    id: &str,
    name: &str,
    hash: &str,
    width: u32,
    height: u32,
    rgba_bytes: &[u8],
) {
    julep_core::protocol::emit_screenshot_response(id, name, hash, width, height, rgba_bytes);
}

// ---------------------------------------------------------------------------
// Message -> OutgoingEvent mapping
// ---------------------------------------------------------------------------

/// Convert a widget Message to an OutgoingEvent, if applicable.
/// Returns None for messages that don't map to user-initiated widget events
/// (Stdin, NoOp, keyboard, mouse, window lifecycle, etc.)
pub(crate) fn message_to_event(msg: &Message) -> Option<OutgoingEvent> {
    match msg {
        Message::Click(id) => Some(OutgoingEvent::click(id.clone())),
        Message::Input(id, value) => Some(OutgoingEvent::input(id.clone(), value.clone())),
        Message::Submit(id, value) => Some(OutgoingEvent::submit(id.clone(), value.clone())),
        Message::Toggle(id, value) => Some(OutgoingEvent::toggle(id.clone(), *value)),
        Message::Slide(..) | Message::SlideRelease(..) => None,
        Message::Select(id, value) => Some(OutgoingEvent::select(id.clone(), value.clone())),
        Message::SensorResize(id, w, h) => Some(OutgoingEvent::sensor_resize(id.clone(), *w, *h)),
        Message::MouseAreaEvent(id, kind) => match kind.as_str() {
            "right_press" => Some(OutgoingEvent::mouse_right_press(id.clone())),
            "right_release" => Some(OutgoingEvent::mouse_right_release(id.clone())),
            "middle_press" => Some(OutgoingEvent::mouse_middle_press(id.clone())),
            "middle_release" => Some(OutgoingEvent::mouse_middle_release(id.clone())),
            "double_click" => Some(OutgoingEvent::mouse_double_click(id.clone())),
            "enter" => Some(OutgoingEvent::mouse_enter(id.clone())),
            "exit" => Some(OutgoingEvent::mouse_exit(id.clone())),
            _ => None,
        },
        Message::MouseAreaMove(id, x, y) => {
            Some(OutgoingEvent::mouse_area_move(id.clone(), *x, *y))
        }
        Message::MouseAreaScroll(id, dx, dy) => {
            Some(OutgoingEvent::mouse_area_scroll(id.clone(), *dx, *dy))
        }
        Message::ScrollEvent(id, abs_x, abs_y, rel_x, rel_y, bw, bh, cw, ch) => {
            Some(OutgoingEvent::scroll(
                id.clone(),
                *abs_x,
                *abs_y,
                *rel_x,
                *rel_y,
                *bw,
                *bh,
                *cw,
                *ch,
            ))
        }
        Message::Event(id, data, family) => {
            let data_opt = if data.is_null() {
                None
            } else {
                Some(data.clone())
            };
            Some(OutgoingEvent::generic(family.clone(), id.clone(), data_opt))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_to_event_click() {
        let msg = Message::Click("btn1".into());
        let event = message_to_event(&msg).unwrap();
        assert_eq!(event.family, "click");
        assert_eq!(event.id, "btn1");
    }

    #[test]
    fn message_to_event_input() {
        let msg = Message::Input("field1".into(), "hello".into());
        let event = message_to_event(&msg).unwrap();
        assert_eq!(event.family, "input");
        assert_eq!(event.id, "field1");
    }

    #[test]
    fn message_to_event_submit() {
        let msg = Message::Submit("form1".into(), "data".into());
        let event = message_to_event(&msg).unwrap();
        assert_eq!(event.family, "submit");
    }

    #[test]
    fn message_to_event_toggle() {
        let msg = Message::Toggle("cb1".into(), true);
        let event = message_to_event(&msg).unwrap();
        assert_eq!(event.family, "toggle");
    }

    #[test]
    fn message_to_event_select() {
        let msg = Message::Select("pick1".into(), "option_a".into());
        let event = message_to_event(&msg).unwrap();
        assert_eq!(event.family, "select");
    }

    #[test]
    fn message_to_event_slide_returns_none() {
        let msg = Message::Slide("sl1".into(), 0.5);
        assert!(message_to_event(&msg).is_none());
    }

    #[test]
    fn message_to_event_slide_release_returns_none() {
        let msg = Message::SlideRelease("sl1".into());
        assert!(message_to_event(&msg).is_none());
    }

    #[test]
    fn message_to_event_noop_returns_none() {
        let msg = Message::NoOp;
        assert!(message_to_event(&msg).is_none());
    }

    #[test]
    fn message_to_event_mouse_area_events() {
        for kind in &[
            "right_press",
            "right_release",
            "middle_press",
            "middle_release",
            "double_click",
            "enter",
            "exit",
        ] {
            let msg = Message::MouseAreaEvent("ma1".into(), kind.to_string());
            assert!(
                message_to_event(&msg).is_some(),
                "mouse area event `{kind}` should map"
            );
        }
        // Unknown mouse area event kind
        let msg = Message::MouseAreaEvent("ma1".into(), "unknown".into());
        assert!(message_to_event(&msg).is_none());
    }

    #[test]
    fn message_to_event_sensor_resize() {
        let msg = Message::SensorResize("s1".into(), 100.0, 200.0);
        let event = message_to_event(&msg).unwrap();
        assert_eq!(event.family, "sensor_resize");
    }

    #[test]
    fn message_to_event_generic_event() {
        let msg = Message::Event(
            "node1".into(),
            serde_json::json!({"key": "value"}),
            "custom_family".into(),
        );
        let event = message_to_event(&msg).unwrap();
        assert_eq!(event.family, "custom_family");
        assert_eq!(event.id, "node1");
        assert!(event.data.is_some());
    }

    #[test]
    fn message_to_event_generic_null_data() {
        let msg = Message::Event("node1".into(), serde_json::Value::Null, "fam".into());
        let event = message_to_event(&msg).unwrap();
        assert!(event.data.is_none());
    }
}
