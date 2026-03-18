//! Stdin I/O: initial settings reader, background reader thread, and
//! the iced subscription that bridges stdin events into the update loop.

use std::io::{self, BufRead, Write};
use std::sync::Mutex;
use std::thread;

use iced::futures::SinkExt;
use iced::stream;

use julep_core::codec::Codec;
use julep_core::message::StdinEvent;
use julep_core::protocol::IncomingMessage;
use serde_json::Value;

/// Emit an error message to stdout and exit the process. Used for
/// fatal startup failures (decode error, protocol version mismatch)
/// where the daemon cannot proceed.
fn startup_exit(codec: &Codec, message: &str) -> ! {
    log::error!("{message}");
    let error = serde_json::json!({"type": "error", "message": message});
    if let Ok(bytes) = codec.encode(&error) {
        let mut out = io::stdout().lock();
        let _ = out.write_all(&bytes);
        let _ = out.flush();
    }
    std::process::exit(1);
}

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
    // Auto-detect peeks at the first byte via fill_buf() (which blocks until
    // data arrives on the pipe) without consuming it, so read_message() can
    // read the full message normally including the detection byte.
    let codec = match forced_codec {
        Some(c) => c,
        None => {
            let buf = match reader.fill_buf() {
                Ok(buf) if !buf.is_empty() => buf,
                Ok(_) => {
                    log::error!("stdin closed before settings received");
                    Codec::set_global(Codec::MsgPack);
                    return empty_settings(reader);
                }
                Err(e) => {
                    log::error!("stdin closed before settings received: {e}");
                    Codec::set_global(Codec::MsgPack);
                    return empty_settings(reader);
                }
            };
            Codec::detect_from_first_byte(buf[0])
        }
    };
    log::info!("wire codec: {codec}");
    Codec::set_global(codec);

    // Read the first framed message. The detection byte (if auto-detected)
    // is still in the buffer, so read_message works normally.
    let payload = match codec.read_message(&mut reader) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => {
            log::error!("stdin closed before settings received");
            return empty_settings(reader);
        }
        Err(e) => {
            log::error!("failed to read initial settings: {e}");
            return empty_settings(reader);
        }
    };

    // Decode the payload into an IncomingMessage.
    let msg: IncomingMessage = match codec.decode(&payload) {
        Ok(m) => m,
        Err(err) => {
            startup_exit(&codec, &format!("failed to decode initial settings: {err}"));
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
                    startup_exit(
                        &codec,
                        &format!(
                            "protocol version mismatch: host sent {version}, renderer expects {expected}"
                        ),
                    );
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
                IncomingMessage::TreeHash { .. } => "tree_hash",
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
