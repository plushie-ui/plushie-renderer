//! Stdout emitters for the wire protocol.
//!
//! All renderer output (events, handshake, effect responses, query
//! responses, screenshots) flows through this module. Each emitter
//! encodes via the global [`Codec`] and writes to stdout.
//!
//! In windowed mode, a [`ChannelWriter`] decouples the iced event
//! loop from pipe writes: `write_output` fills a buffer and flushes
//! it through a bounded channel to a background writer thread that
//! owns the actual transport. This prevents GUI stalls when the pipe
//! blocks (e.g. over SSH with high latency).

use std::io::{self, Write};
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::thread;

use iced::Task;

use toddy_core::codec::Codec;
use toddy_core::message::Message;
use toddy_core::protocol::OutgoingEvent;

// ---------------------------------------------------------------------------
// configurable output writer
// ---------------------------------------------------------------------------

static OUTPUT_WRITER: OnceLock<Mutex<Box<dyn Write + Send>>> = OnceLock::new();

/// Channel capacity for the background writer thread. 256 messages
/// gives ample headroom before the event loop would block on send.
const WRITER_CHANNEL_CAPACITY: usize = 256;

/// A [`Write`] adapter that buffers bytes and sends them through a
/// bounded channel on flush. The background writer thread receives
/// the chunks and performs the actual (potentially blocking) I/O.
pub(crate) struct ChannelWriter {
    tx: mpsc::SyncSender<Vec<u8>>,
    buffer: Vec<u8>,
}

impl Write for ChannelWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }
        let bytes = std::mem::take(&mut self.buffer);
        self.tx
            .send(bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "writer thread exited"))
    }
}

/// Spawn a background writer thread and return a [`ChannelWriter`]
/// that sends encoded bytes to it. The thread owns the transport
/// writer and performs blocking I/O without stalling the caller.
pub(crate) fn spawn_writer_thread(writer: Box<dyn Write + Send>) -> ChannelWriter {
    let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(WRITER_CHANNEL_CAPACITY);

    thread::Builder::new()
        .name("toddy-writer".into())
        .spawn(move || {
            let mut writer = writer;
            for bytes in rx {
                if writer.write_all(&bytes).is_err() || writer.flush().is_err() {
                    break;
                }
            }
        })
        .expect("failed to spawn writer thread");

    ChannelWriter {
        tx,
        buffer: Vec::new(),
    }
}

/// Initialize the global output writer. Must be called once at startup.
///
/// In windowed mode, pass a [`ChannelWriter`] (from [`spawn_writer_thread`])
/// so the iced event loop never blocks on pipe writes. In headless mode,
/// pass the raw transport writer directly -- headless events are host-paced
/// and don't need background I/O.
pub(crate) fn init_output(writer: Box<dyn Write + Send>) {
    if OUTPUT_WRITER.set(Mutex::new(writer)).is_err() {
        panic!("output writer already initialized");
    }
}

/// Write bytes to the protocol output channel (stdout or transport writer).
///
/// Each call acquires the writer lock and flushes. When the global writer
/// is a [`ChannelWriter`], flush sends the bytes through a bounded channel
/// to the background writer thread -- the actual pipe write happens there.
///
/// Falls back to direct stdout if the global writer has not been
/// initialized yet (only possible during very early startup errors).
pub(crate) fn write_output(bytes: &[u8]) -> io::Result<()> {
    if let Some(writer) = OUTPUT_WRITER.get() {
        let mut guard = writer.lock().unwrap_or_else(|e| e.into_inner());
        guard.write_all(bytes)?;
        guard.flush()
    } else {
        let stdout = io::stdout();
        let mut handle = stdout.lock();
        handle.write_all(bytes)?;
        handle.flush()
    }
}

// ---------------------------------------------------------------------------
// stdout event emitter
// ---------------------------------------------------------------------------

/// Emit an event and return `Task::none()`, or log the error and return
/// `iced::exit()` if the write fails. This is the standard pattern for
/// event emission from `update()` -- a broken stdout pipe means the host
/// is gone and we should shut down.
pub(crate) fn emit_or_exit(event: OutgoingEvent) -> Task<Message> {
    if let Err(e) = emit_event(event) {
        log::error!("write error: {e}");
        return iced::exit();
    }
    Task::none()
}

/// Encode and write an [`OutgoingEvent`] to stdout. Returns the I/O
/// result so the caller can decide whether to exit or continue.
pub(crate) fn emit_event(event: OutgoingEvent) -> io::Result<()> {
    let codec = Codec::get_global();
    let bytes = codec.encode(&event).map_err(io::Error::other)?;
    write_output(&bytes)
}

// ---------------------------------------------------------------------------
// stdout hello message emitter
// ---------------------------------------------------------------------------

/// Emit a `hello` handshake message to stdout immediately after codec
/// negotiation. This tells the host which protocol version and
/// renderer build it is talking to.
///
/// `backend` identifies the rendering backend: `"wgpu"` for windowed,
/// `"tiny-skia"` for headless, `"none"` for mock.
/// `extensions` lists the config keys of registered extensions.
pub(crate) fn emit_hello(
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
// stdout effect response emitter
// ---------------------------------------------------------------------------

/// Encode and write an [`EffectResponse`](toddy_core::protocol::EffectResponse)
/// to stdout (file dialog results, clipboard data, etc.).
pub(crate) fn emit_effect_response(
    response: toddy_core::protocol::EffectResponse,
) -> io::Result<()> {
    let codec = Codec::get_global();
    let bytes = codec.encode(&response).map_err(io::Error::other)?;
    write_output(&bytes)
}

/// Emit a query_response message to stdout. Used for system-level queries
/// (system_info, system_theme) that are not window-specific.
pub(crate) fn emit_query_response(
    kind: &str,
    tag: &str,
    data: serde_json::Value,
) -> io::Result<()> {
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
// stdout screenshot response emitter
// ---------------------------------------------------------------------------

/// Emit a screenshot_response to stdout. Uses `Codec::encode_binary_message`
/// so that RGBA pixel data is encoded as native msgpack binary (avoiding
/// ~33% base64 overhead) while remaining valid base64 over JSON.
pub(crate) fn emit_screenshot_response(
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
///
/// Covers all message variants that emit directly without special
/// handling (no state mutation, no extension routing, no subscription
/// lookup). Messages that need special handling (Slide, Event with
/// extension dispatcher, TextEditorAction, keyboard/mouse/touch/IME
/// subscription events, window lifecycle) return `None` and are
/// handled by dedicated arms in `update()`.
pub(crate) fn message_to_event(msg: &Message) -> Option<OutgoingEvent> {
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
            cursor_x,
            cursor_y,
            delta_x,
            delta_y,
        } => Some(OutgoingEvent::canvas_scroll(
            id.clone(),
            *cursor_x,
            *cursor_y,
            *delta_x,
            *delta_y,
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_writer_sends_on_flush() {
        let (tx, rx) = mpsc::sync_channel(16);
        let mut w = ChannelWriter {
            tx,
            buffer: Vec::new(),
        };
        w.write_all(b"hello").unwrap();
        assert!(rx.try_recv().is_err(), "nothing sent before flush");
        w.flush().unwrap();
        let received = rx.recv().unwrap();
        assert_eq!(received, b"hello");
    }

    #[test]
    fn channel_writer_empty_flush_is_noop() {
        let (tx, rx) = mpsc::sync_channel(16);
        let mut w = ChannelWriter {
            tx,
            buffer: Vec::new(),
        };
        w.flush().unwrap();
        assert!(rx.try_recv().is_err(), "empty flush should send nothing");
    }

    #[test]
    fn channel_writer_broken_pipe_on_closed_receiver() {
        let (tx, rx) = mpsc::sync_channel(16);
        drop(rx);
        let mut w = ChannelWriter {
            tx,
            buffer: Vec::new(),
        };
        w.write_all(b"data").unwrap();
        let result = w.flush();
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::BrokenPipe);
    }

    #[test]
    fn spawn_writer_thread_delivers_bytes() {
        let (inner_tx, inner_rx) = mpsc::sync_channel::<Vec<u8>>(16);
        // Use a simple writer that forwards to a channel so we can
        // observe what the background thread wrote.
        struct TestWriter(mpsc::SyncSender<Vec<u8>>);
        impl Write for TestWriter {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.0
                    .send(buf.to_vec())
                    .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "closed"))?;
                Ok(buf.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut cw = spawn_writer_thread(Box::new(TestWriter(inner_tx)));
        cw.write_all(b"test data").unwrap();
        cw.flush().unwrap();
        let received = inner_rx.recv().unwrap();
        assert_eq!(received, b"test data");
    }

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
        // Unknown canvas event kind
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
            cursor_x: 10.0,
            cursor_y: 20.0,
            delta_x: 1.0,
            delta_y: -1.0,
        };
        let event = message_to_event(&msg).unwrap();
        assert_eq!(event.family, "canvas_scroll");
    }

    #[test]
    fn message_to_event_extension_event_returns_none() {
        // Message::Event goes through the extension dispatcher in
        // update(), not through message_to_event.
        let msg = Message::Event {
            id: "node1".into(),
            data: serde_json::json!({"key": "value"}),
            family: "custom_family".into(),
        };
        assert!(message_to_event(&msg).is_none());
    }
}
