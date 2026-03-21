//! Output emitters for the wire protocol.
//!
//! All renderer output (events, handshake, effect responses, query
//! responses, screenshots) flows through this module. Each emitter
//! encodes via the global [`Codec`] and writes to the output writer.
//!
//! The output writer is initialized at startup via [`init_output`].
//! Native mode uses a ChannelWriter backed by a background thread
//! (provided by the binary crate). WASM mode uses a JS callback wrapper.

use std::io::{self, Write};
use std::sync::{Mutex, OnceLock};

use iced::Task;

use toddy_core::codec::Codec;
use toddy_core::message::Message;
use toddy_core::protocol::OutgoingEvent;

// ---------------------------------------------------------------------------
// configurable output writer
// ---------------------------------------------------------------------------

static OUTPUT_WRITER: OnceLock<Mutex<Box<dyn Write + Send>>> = OnceLock::new();

/// Initialize the global output writer. Must be called once at startup.
///
/// The caller provides a boxed [`Write`] implementation. On native,
/// this is typically a ChannelWriter backed by a background thread.
/// On WASM, this wraps a JS callback.
pub fn init_output(writer: Box<dyn Write + Send>) {
    if OUTPUT_WRITER.set(Mutex::new(writer)).is_err() {
        panic!("output writer already initialized");
    }
}

/// Write bytes to the protocol output channel.
///
/// Each call acquires the writer lock and flushes. Falls back to
/// direct stdout if the global writer has not been initialized yet
/// (only possible during very early startup errors on native).
pub fn write_output(bytes: &[u8]) -> io::Result<()> {
    if let Some(writer) = OUTPUT_WRITER.get() {
        let mut guard = writer.lock().unwrap_or_else(|e| e.into_inner());
        guard.write_all(bytes)?;
        guard.flush()
    } else {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let stdout = io::stdout();
            let mut handle = stdout.lock();
            handle.write_all(bytes)?;
            handle.flush()
        }
        #[cfg(target_arch = "wasm32")]
        {
            Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "output writer not initialized",
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// event emitters
// ---------------------------------------------------------------------------

/// Emit an event and return `Task::none()`, or log the error and return
/// `iced::exit()` if the write fails.
pub fn emit_or_exit(event: OutgoingEvent) -> Task<Message> {
    if let Err(e) = emit_event(event) {
        log::error!("write error: {e}");
        return iced::exit();
    }
    Task::none()
}

/// Encode and write an [`OutgoingEvent`] to the output writer.
pub fn emit_event(event: OutgoingEvent) -> io::Result<()> {
    let codec = Codec::get_global();
    let bytes = codec.encode(&event).map_err(io::Error::other)?;
    write_output(&bytes)
}

// ---------------------------------------------------------------------------
// hello message emitter
// ---------------------------------------------------------------------------

/// Emit a `hello` handshake message immediately after codec negotiation.
pub fn emit_hello(
    mode: &str,
    backend: &str,
    extensions: &[&str],
    transport: &str,
) -> io::Result<()> {
    let msg = serde_json::json!({
        "type": "hello",
        "session": "",
        "protocol": toddy_core::protocol::PROTOCOL_VERSION,
        "version": env!("CARGO_PKG_VERSION"),
        "name": "toddy",
        "mode": mode,
        "backend": backend,
        "transport": transport,
        "extensions": extensions,
    });
    let codec = Codec::get_global();
    let bytes = codec.encode(&msg).map_err(io::Error::other)?;
    write_output(&bytes)
}

// ---------------------------------------------------------------------------
// effect response emitter
// ---------------------------------------------------------------------------

/// Encode and write an [`EffectResponse`](toddy_core::protocol::EffectResponse).
pub fn emit_effect_response(response: toddy_core::protocol::EffectResponse) -> io::Result<()> {
    let codec = Codec::get_global();
    let bytes = codec.encode(&response).map_err(io::Error::other)?;
    write_output(&bytes)
}

/// Emit a query_response message.
pub fn emit_query_response(kind: &str, tag: &str, data: serde_json::Value) -> io::Result<()> {
    let msg = serde_json::json!({
        "type": "op_query_response",
        "session": "",
        "kind": kind,
        "tag": tag,
        "data": data,
    });
    let codec = Codec::get_global();
    let bytes = codec.encode(&msg).map_err(io::Error::other)?;
    write_output(&bytes)
}

// ---------------------------------------------------------------------------
// screenshot response emitter
// ---------------------------------------------------------------------------

/// Emit a screenshot_response. Uses `Codec::encode_binary_message`
/// so that RGBA pixel data is encoded as native msgpack binary.
pub fn emit_screenshot_response(
    id: &str,
    name: &str,
    hash: &str,
    width: u32,
    height: u32,
    rgba_bytes: &[u8],
) -> io::Result<()> {
    use serde_json::json;

    let mut map = serde_json::Map::new();
    map.insert("type".to_string(), json!("screenshot_response"));
    map.insert("session".to_string(), json!(""));
    map.insert("id".to_string(), json!(id));
    map.insert("name".to_string(), json!(name));
    map.insert("hash".to_string(), json!(hash));
    map.insert("width".to_string(), json!(width));
    map.insert("height".to_string(), json!(height));

    let binary = if rgba_bytes.is_empty() {
        None
    } else {
        Some(("rgba", rgba_bytes))
    };
    let codec = Codec::get_global();
    let bytes = codec
        .encode_binary_message(map, binary)
        .map_err(io::Error::other)?;
    write_output(&bytes)
}

// ---------------------------------------------------------------------------
// Message -> OutgoingEvent mapping
// ---------------------------------------------------------------------------

/// Convert a widget [`Message`] to an [`OutgoingEvent`], if applicable.
pub fn message_to_event(msg: &Message) -> Option<OutgoingEvent> {
    match msg {
        Message::Click(id) => Some(OutgoingEvent::click(id.clone())),
        Message::Input(id, value) => Some(OutgoingEvent::input(id.clone(), value.clone())),
        Message::Submit(id, value) => Some(OutgoingEvent::submit(id.clone(), value.clone())),
        Message::Toggle(id, value) => Some(OutgoingEvent::toggle(id.clone(), *value)),
        Message::Select(id, value) => Some(OutgoingEvent::select(id.clone(), value.clone())),
        Message::Paste(id, text) => Some(OutgoingEvent::paste(id.clone(), text.clone())),
        Message::OptionHovered(id, value) => {
            Some(OutgoingEvent::option_hovered(id.clone(), value.clone()))
        }
        Message::SensorResize(id, w, h) => Some(OutgoingEvent::sensor_resize(id.clone(), *w, *h)),
        Message::ScrollEvent(id, viewport) => Some(OutgoingEvent::scroll(
            id.clone(),
            viewport.absolute_x,
            viewport.absolute_y,
            viewport.relative_x,
            viewport.relative_y,
            viewport.viewport_width,
            viewport.viewport_height,
            viewport.content_width,
            viewport.content_height,
        )),
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
        Message::CanvasEvent {
            id,
            kind,
            x,
            y,
            extra,
        } => match kind.as_str() {
            "press" => Some(OutgoingEvent::canvas_press(
                id.clone(),
                *x,
                *y,
                extra.clone(),
            )),
            "release" => Some(OutgoingEvent::canvas_release(
                id.clone(),
                *x,
                *y,
                extra.clone(),
            )),
            "move" => Some(OutgoingEvent::canvas_move(id.clone(), *x, *y)),
            _ => None,
        },
        Message::CanvasScroll {
            id,
            x,
            y,
            delta_x,
            delta_y,
        } => Some(OutgoingEvent::canvas_scroll(
            id.clone(),
            *x,
            *y,
            *delta_x,
            *delta_y,
        )),
        Message::CanvasShapeEnter {
            canvas_id,
            shape_id,
            x,
            y,
        } => Some(OutgoingEvent::canvas_shape_enter(
            canvas_id.clone(),
            shape_id.clone(),
            *x,
            *y,
        )),
        Message::CanvasShapeLeave {
            canvas_id,
            shape_id,
        } => Some(OutgoingEvent::canvas_shape_leave(
            canvas_id.clone(),
            shape_id.clone(),
        )),
        Message::CanvasShapeClick {
            canvas_id,
            shape_id,
            x,
            y,
            button,
        } => Some(OutgoingEvent::canvas_shape_click(
            canvas_id.clone(),
            shape_id.clone(),
            *x,
            *y,
            button.clone(),
        )),
        Message::CanvasShapeDrag {
            canvas_id,
            shape_id,
            x,
            y,
            delta_x,
            delta_y,
        } => Some(OutgoingEvent::canvas_shape_drag(
            canvas_id.clone(),
            shape_id.clone(),
            *x,
            *y,
            *delta_x,
            *delta_y,
        )),
        Message::CanvasShapeDragEnd {
            canvas_id,
            shape_id,
            x,
            y,
        } => Some(OutgoingEvent::canvas_shape_drag_end(
            canvas_id.clone(),
            shape_id.clone(),
            *x,
            *y,
        )),
        Message::CanvasShapeFocused {
            canvas_id,
            shape_id,
        } => Some(OutgoingEvent::canvas_shape_focused(
            canvas_id.clone(),
            shape_id.clone(),
        )),
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
    fn message_to_event_paste() {
        let msg = Message::Paste("f1".into(), "pasted text".into());
        let event = message_to_event(&msg).unwrap();
        assert_eq!(event.family, "paste");
    }

    #[test]
    fn message_to_event_option_hovered() {
        let msg = Message::OptionHovered("pick1".into(), "opt_a".into());
        let event = message_to_event(&msg).unwrap();
        assert_eq!(event.family, "option_hovered");
    }

    #[test]
    fn message_to_event_canvas_events() {
        for kind in &["press", "release", "move"] {
            let msg = Message::CanvasEvent {
                id: "c1".into(),
                kind: kind.to_string(),
                x: 10.0,
                y: 20.0,
                extra: String::new(),
            };
            assert!(
                message_to_event(&msg).is_some(),
                "canvas event `{kind}` should map"
            );
        }
        let msg = Message::CanvasEvent {
            id: "c1".into(),
            kind: "unknown".into(),
            x: 0.0,
            y: 0.0,
            extra: String::new(),
        };
        assert!(message_to_event(&msg).is_none());
    }

    #[test]
    fn message_to_event_canvas_scroll() {
        let msg = Message::CanvasScroll {
            id: "c1".into(),
            x: 10.0,
            y: 20.0,
            delta_x: 1.0,
            delta_y: -1.0,
        };
        let event = message_to_event(&msg).unwrap();
        assert_eq!(event.family, "canvas_scroll");
    }

    #[test]
    fn message_to_event_extension_event_returns_none() {
        let msg = Message::Event {
            id: "node1".into(),
            data: serde_json::json!({"key": "value"}),
            family: "custom_family".into(),
        };
        assert!(message_to_event(&msg).is_none());
    }
}
