//! Stdin I/O: initial settings reader, background reader thread, and
//! the iced subscription that bridges stdin events into the update loop.

use std::io::{self, BufRead, Read, Write};
use std::sync::Mutex;
use std::thread;

use iced::futures::SinkExt;
use iced::stream;

use julep_core::codec::Codec;
use julep_core::message::StdinEvent;
use julep_core::protocol::IncomingMessage;
use serde_json::Value;

/// Default return value for `read_initial_settings` error paths.
/// Returns empty settings, default iced config, no fonts, and the
/// reader (so the caller can still spawn the stdin thread).
fn empty_settings(
    reader: io::BufReader<io::Stdin>,
) -> (
    Value,
    iced::Settings,
    Vec<Vec<u8>>,
    io::BufReader<io::Stdin>,
) {
    (
        Value::Object(Default::default()),
        iced::Settings::default(),
        Vec::new(),
        reader,
    )
}

/// One-shot slot for the stdin receiver. The subscription takes it once on
/// first call. Uses a Mutex because `Subscription::run` requires `fn() -> Stream`
/// (a function pointer, not a closure), so we can't capture local state.
pub(crate) static STDIN_RX: Mutex<Option<tokio::sync::mpsc::Receiver<StdinEvent>>> =
    Mutex::new(None);

/// Async stream that yields StdinEvents. Bridges the background stdin reader
/// thread into iced's subscription system. Only wakes iced when data arrives
/// -- zero CPU when idle.
pub(crate) fn stdin_subscription() -> impl iced::futures::Stream<Item = StdinEvent> {
    stream::channel(32, async |mut sender| {
        let mut rx = STDIN_RX
            .lock()
            .expect("STDIN_RX lock poisoned")
            .take()
            .expect("stdin_subscription: no receiver (called more than once?)");

        while let Some(event) = rx.recv().await {
            if sender.send(event).await.is_err() {
                break;
            }
        }
    })
}

pub(crate) fn spawn_stdin_reader(
    sender: tokio::sync::mpsc::Sender<StdinEvent>,
    mut reader: io::BufReader<io::Stdin>,
) {
    thread::spawn(move || {
        let codec = Codec::get_global();

        loop {
            match codec.read_message(&mut reader) {
                Ok(None) => {
                    let _ = sender.blocking_send(StdinEvent::Closed);
                    break;
                }
                Ok(Some(bytes)) => match codec.decode::<IncomingMessage>(&bytes) {
                    Ok(msg) => {
                        if sender.blocking_send(StdinEvent::Message(msg)).is_err() {
                            return;
                        }
                    }
                    Err(e) => {
                        let warning = format!("parse error: {e}");
                        if sender.blocking_send(StdinEvent::Warning(warning)).is_err() {
                            return;
                        }
                    }
                },
                Err(e) => {
                    let _ = sender.blocking_send(StdinEvent::Warning(format!("read error: {e}")));
                    let _ = sender.blocking_send(StdinEvent::Closed);
                    break;
                }
            }
        }
    });
}

// ---------------------------------------------------------------------------
// Initial settings reader
// ---------------------------------------------------------------------------

/// Read the first message from stdin synchronously, expecting a Settings message.
/// Determines the wire codec (from CLI flag or auto-detection) and stores it in
/// the global wire codec. Returns the settings Value, iced Settings, font bytes, and
/// the buffered reader (to be handed off to the stdin reader thread).
pub(crate) fn read_initial_settings(
    forced_codec: Option<Codec>,
) -> (
    Value,
    iced::Settings,
    Vec<Vec<u8>>,
    io::BufReader<io::Stdin>,
) {
    let mut reader = io::BufReader::with_capacity(64 * 1024, io::stdin());

    // Determine codec: forced by CLI flag, or auto-detected from first byte.
    //
    // Auto-detect reads one byte to determine the format: '{' (0x7B) = JSON,
    // anything else = MsgPack length prefix. We use read_exact (blocking) rather
    // than fill_buf (non-blocking peek) because fill_buf on a pipe can return
    // empty before the writer has sent any data, causing a false EOF.
    //
    // Since the detection byte is consumed, we read the rest of the first
    // message manually (the remaining 3 bytes of the MsgPack length prefix,
    // or the rest of the JSON line).
    let (codec, first_byte) = match forced_codec {
        Some(c) => (c, None),
        None => {
            let mut first = [0u8; 1];
            match reader.read_exact(&mut first) {
                Ok(()) => {}
                Err(e) => {
                    log::error!("stdin closed before settings received: {e}");
                    Codec::set_global(Codec::MsgPack);
                    return empty_settings(reader);
                }
            }
            (Codec::detect_from_first_byte(first[0]), Some(first[0]))
        }
    };
    log::info!("wire codec: {codec}");
    Codec::set_global(codec);

    // Read the first framed message. If we consumed a detection byte, we need
    // to account for it when reading the rest of the message.
    let payload = if let Some(byte) = first_byte {
        match codec {
            Codec::MsgPack => {
                // byte is the first of the 4-byte BE length prefix. Read 3 more.
                let mut rest = [0u8; 3];
                if let Err(e) = reader.read_exact(&mut rest) {
                    log::error!("failed to read initial settings: {e}");
                    return empty_settings(reader);
                }
                let len = u32::from_be_bytes([byte, rest[0], rest[1], rest[2]]) as usize;
                if len == 0 || len > julep_core::codec::MAX_MESSAGE_SIZE {
                    log::error!(
                        "initial settings frame size invalid ({len} bytes, \
                         limit {} bytes)",
                        julep_core::codec::MAX_MESSAGE_SIZE
                    );
                    return empty_settings(reader);
                }
                let mut payload = vec![0u8; len];
                if let Err(e) = reader.read_exact(&mut payload) {
                    log::error!("failed to read initial settings payload: {e}");
                    return empty_settings(reader);
                }
                payload
            }
            Codec::Json => {
                // byte is '{'. Read the rest of the line, prepend '{'.
                // Wrap in Take to bound allocation before the full line is read.
                let mut line = String::new();
                let limit = (julep_core::codec::MAX_MESSAGE_SIZE + 1) as u64;
                if let Err(e) = (&mut reader).take(limit).read_line(&mut line) {
                    log::error!("failed to read initial settings: {e}");
                    return empty_settings(reader);
                }
                let full = format!("{}{}", byte as char, line.trim());
                full.into_bytes()
            }
        }
    } else {
        // Forced codec -- no detection byte consumed. Use normal read_message.
        match codec.read_message(&mut reader) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => {
                log::error!("stdin closed before settings received");
                return empty_settings(reader);
            }
            Err(e) => {
                log::error!("failed to read initial settings: {e}");
                return empty_settings(reader);
            }
        }
    };

    // Decode the payload into an IncomingMessage.
    let msg: IncomingMessage = match codec.decode(&payload) {
        Ok(m) => m,
        Err(err) => {
            log::error!("failed to decode initial settings: {err}");
            if forced_codec.is_some() {
                // Emit error in the forced codec's format so the client can decode it.
                let error = serde_json::json!({"type": "error", "message": format!("decode failed: {err}")});
                let fallback =
                    format!("{{\"type\":\"error\",\"message\":\"decode failed: {err}\"}}\n");
                let bytes = codec
                    .encode(&error)
                    .unwrap_or_else(|_| fallback.into_bytes());
                let _ = io::stdout().lock().write_all(&bytes);
                let _ = io::stdout().lock().flush();
            } else {
                // No forced codec -- emit plain JSON error for diagnostics.
                let error_msg = format!(
                    "{{\"type\":\"error\",\"message\":\"failed to decode initial settings: {err}\"}}\n"
                );
                let _ = io::stdout().lock().write_all(error_msg.as_bytes());
                let _ = io::stdout().lock().flush();
            }
            std::process::exit(1);
        }
    };

    // Extract Settings variant.
    match msg {
        IncomingMessage::Settings { settings } => {
            log::info!("initial settings received");

            // Enforce protocol version. If the host declares a version and it
            // doesn't match ours, log an error and bail -- running with a
            // mismatched protocol leads to subtle, hard-to-debug failures.
            let expected = u64::from(julep_core::protocol::PROTOCOL_VERSION);
            if let Some(version) = settings.get("protocol_version").and_then(|v| v.as_u64()) {
                if version != expected {
                    log::error!(
                        "protocol version mismatch: host sent {}, renderer expects {}",
                        version,
                        expected
                    );
                    let error_msg = format!(
                        "{{\"type\":\"error\",\"message\":\"protocol version mismatch: \
                         host sent {version}, renderer expects {expected}\"}}\n"
                    );
                    let _ = io::stdout().lock().write_all(error_msg.as_bytes());
                    let _ = io::stdout().lock().flush();
                    std::process::exit(1);
                }
            } else {
                log::warn!(
                    "no protocol_version in Settings, assuming compatible (expected {})",
                    expected
                );
            }

            // Enable prop validation if requested.
            if settings
                .get("validate_props")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                julep_core::widgets::set_validate_props(true);
                log::info!("prop validation enabled via settings");
            }

            let antialiasing = settings
                .get("antialiasing")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let vsync = settings
                .get("vsync")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            let default_text_size = settings
                .get("default_text_size")
                .and_then(|v| v.as_f64())
                .map(|s| iced::Pixels(s as f32));

            let default_font = settings.get("default_font").map(|v| {
                let family = v.get("family").and_then(|f| f.as_str());
                if family == Some("monospace") {
                    iced::Font::MONOSPACE
                } else {
                    iced::Font::DEFAULT
                }
            });

            let mut iced_settings = iced::Settings {
                antialiasing,
                vsync,
                ..Default::default()
            };
            if let Some(size) = default_text_size {
                iced_settings.default_text_size = size;
            }
            if let Some(font) = default_font {
                iced_settings.default_font = font;
            }

            let mut font_bytes: Vec<Vec<u8>> = Vec::new();
            if let Some(fonts) = settings.get("fonts").and_then(|v| v.as_array()) {
                for font_val in fonts {
                    if let Some(path) = font_val.as_str() {
                        match std::fs::read(path) {
                            Ok(bytes) => {
                                log::info!("loaded font: {path}");
                                font_bytes.push(bytes);
                            }
                            Err(e) => {
                                log::error!("failed to load font {path}: {e}");
                            }
                        }
                    }
                }
            }

            (settings, iced_settings, font_bytes, reader)
        }
        other => {
            let variant = match &other {
                IncomingMessage::Snapshot { .. } => "snapshot",
                IncomingMessage::Patch { .. } => "patch",
                IncomingMessage::EffectRequest { .. } => "effect_request",
                IncomingMessage::WidgetOp { .. } => "widget_op",
                IncomingMessage::SubscriptionRegister { .. } => "subscription_register",
                IncomingMessage::SubscriptionUnregister { .. } => "subscription_unregister",
                IncomingMessage::WindowOp { .. } => "window_op",
                IncomingMessage::Settings { .. } => "settings",
                IncomingMessage::Query { .. } => "query",
                IncomingMessage::Interact { .. } => "interact",
                IncomingMessage::SnapshotCapture { .. } => "snapshot_capture",
                IncomingMessage::ScreenshotCapture { .. } => "screenshot_capture",
                IncomingMessage::Reset { .. } => "reset",
                IncomingMessage::ImageOp { .. } => "image_op",
                IncomingMessage::ExtensionCommand { .. } => "extension_command",
                IncomingMessage::ExtensionCommandBatch { .. } => "extension_command_batch",
                IncomingMessage::AdvanceFrame { .. } => "advance_frame",
            };
            log::error!("expected settings as first message, got {variant}");
            empty_settings(reader)
        }
    }
}
