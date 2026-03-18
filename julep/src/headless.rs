//! Headless mode (`--headless`): Core + wire protocol, no display.
//!
//! Reads framed messages from stdin, processes them through
//! [`Core`](julep_core::engine::Core), and writes responses to stdout.
//! No iced daemon, no windows, no GPU. Useful for CI, integration
//! testing, and headless screenshot capture via tiny-skia.

use std::io::{self, BufRead};

use iced::Theme;

use julep_core::codec::Codec;
use julep_core::engine::Core;
use julep_core::extensions::{ExtensionCaches, ExtensionDispatcher};
use julep_core::image_registry::ImageRegistry;
use julep_core::protocol::IncomingMessage;

/// Default screenshot width when not specified by the caller.
const DEFAULT_SCREENSHOT_WIDTH: u32 = 1024;
/// Default screenshot height when not specified by the caller.
const DEFAULT_SCREENSHOT_HEIGHT: u32 = 768;
/// Maximum screenshot dimension (width or height). Matches
/// `ImageRegistry::MAX_DIMENSION`. Prevents untrusted input from
/// triggering a multi-GiB RGBA allocation.
const MAX_SCREENSHOT_DIMENSION: u32 = 16384;

/// All mutable state for a headless session.
struct Session {
    core: Core,
    theme: Theme,
    dispatcher: ExtensionDispatcher,
    ext_caches: ExtensionCaches,
    images: ImageRegistry,
}

impl Session {
    fn new(dispatcher: ExtensionDispatcher) -> Self {
        Self {
            core: Core::new(),
            theme: Theme::Dark,
            dispatcher,
            ext_caches: ExtensionCaches::new(),
            images: ImageRegistry::new(),
        }
    }
}

/// Run the headless event loop.
///
/// Extensions are initialized when a Settings message arrives and
/// prepared after each tree-changing message. Extension rendering
/// goes through the same `widgets::render` path used for screenshot
/// capture, so extensions that rely on real iced widget state (focus,
/// scroll position) won't behave identically to the full daemon mode.
pub fn run(forced_codec: Option<Codec>, dispatcher: ExtensionDispatcher) {
    let mut session = Session::new(dispatcher);
    let stdin = io::stdin();
    let mut reader = io::BufReader::new(stdin.lock());

    // Determine codec: forced by CLI flag, or auto-detected from first byte.
    let codec = match forced_codec {
        Some(c) => c,
        None => {
            let buf = match reader.fill_buf() {
                Ok(b) => b,
                Err(e) => {
                    log::error!("failed to read stdin: {e}");
                    return;
                }
            };
            if buf.is_empty() {
                log::error!("stdin closed before first message");
                return;
            }
            Codec::detect_from_first_byte(buf[0])
        }
    };
    log::info!("wire codec: {codec}");
    Codec::set_global(codec);

    crate::renderer::emit_hello();

    loop {
        match codec.read_message(&mut reader) {
            Ok(None) => break,
            Ok(Some(bytes)) => match codec.decode::<IncomingMessage>(&bytes) {
                Ok(msg) => handle_message(&mut session, msg),
                Err(e) => {
                    log::error!("decode error: {e}");
                }
            },
            Err(e) => {
                log::error!("read error: {e}");
                break;
            }
        }
    }

    log::info!("stdin closed, exiting");
}

fn handle_message(s: &mut Session, msg: IncomingMessage) {
    let is_snapshot = matches!(msg, IncomingMessage::Snapshot { .. });
    let is_tree_change = is_snapshot || matches!(msg, IncomingMessage::Patch { .. });

    match msg {
        // Messages that go through Core::apply().
        IncomingMessage::Snapshot { .. }
        | IncomingMessage::Patch { .. }
        | IncomingMessage::EffectRequest { .. }
        | IncomingMessage::WidgetOp { .. }
        | IncomingMessage::SubscriptionRegister { .. }
        | IncomingMessage::SubscriptionUnregister { .. }
        | IncomingMessage::WindowOp { .. }
        | IncomingMessage::Settings { .. }
        | IncomingMessage::ImageOp { .. } => {
            let effects = s.core.apply(msg);

            for effect in effects {
                use julep_core::engine::CoreEffect;
                match effect {
                    CoreEffect::EmitEvent(event) => {
                        crate::test_protocol::emit_wire(&event);
                    }
                    CoreEffect::EmitEffectResponse(response) => {
                        crate::test_protocol::emit_wire(&response);
                    }
                    CoreEffect::SpawnAsyncEffect {
                        request_id,
                        effect_type,
                        ..
                    } => {
                        log::debug!(
                            "headless: async effect {effect_type} returning cancelled \
                             (no display)"
                        );
                        crate::test_protocol::emit_wire(
                            &julep_core::protocol::EffectResponse::error(
                                request_id,
                                "cancelled".to_string(),
                            ),
                        );
                    }
                    CoreEffect::ThemeChanged(t) => {
                        s.theme = t;
                    }
                    CoreEffect::ImageOp {
                        op,
                        handle,
                        data,
                        pixels,
                        width,
                        height,
                    } => {
                        if let Err(e) = s.images.apply_op(&op, &handle, data, pixels, width, height)
                        {
                            log::warn!("headless: image_op {op} failed: {e}");
                        }
                    }
                    CoreEffect::ExtensionConfig(config) => {
                        s.dispatcher.init_all(&config);
                    }
                    // No-ops in headless (no windows, no iced widget tree).
                    CoreEffect::SyncWindows => {}
                    CoreEffect::WidgetOp { .. } => {}
                    CoreEffect::WindowOp { .. } => {}
                    CoreEffect::ThemeFollowsSystem => {}
                }
            }

            // Prepare extensions after tree changes (Snapshot/Patch).
            if is_tree_change {
                if is_snapshot {
                    s.dispatcher.clear_poisoned();
                }
                if let Some(root) = s.core.tree.root() {
                    s.dispatcher.prepare_all(root, &mut s.ext_caches, &s.theme);
                }
            }
        }

        // Test protocol messages
        IncomingMessage::Query {
            id,
            target,
            selector,
        } => {
            crate::test_protocol::handle_query(&s.core, id, target, selector);
        }
        IncomingMessage::Interact {
            id,
            action,
            selector,
            payload,
        } => {
            crate::test_protocol::handle_interact(&s.core, id, action, selector, payload);
        }
        IncomingMessage::SnapshotCapture { id, name, .. } => {
            crate::test_protocol::handle_snapshot_capture(&s.core, id, name);
        }
        IncomingMessage::ScreenshotCapture {
            id,
            name,
            width,
            height,
        } => {
            let w = width
                .unwrap_or(DEFAULT_SCREENSHOT_WIDTH)
                .clamp(1, MAX_SCREENSHOT_DIMENSION);
            let h = height
                .unwrap_or(DEFAULT_SCREENSHOT_HEIGHT)
                .clamp(1, MAX_SCREENSHOT_DIMENSION);
            handle_screenshot_capture(s, id, name, w, h);
        }
        IncomingMessage::Reset { id } => {
            s.dispatcher.reset(&mut s.ext_caches);
            s.images = ImageRegistry::new();
            s.theme = Theme::Dark;
            crate::test_protocol::handle_reset(&mut s.core, id);
        }
        IncomingMessage::ExtensionCommand {
            node_id,
            op,
            payload,
        } => {
            let events = s
                .dispatcher
                .handle_command(&node_id, &op, &payload, &mut s.ext_caches);
            for event in events {
                crate::test_protocol::emit_wire(&event);
            }
        }
        IncomingMessage::ExtensionCommandBatch { commands } => {
            for cmd in commands {
                let events = s.dispatcher.handle_command(
                    &cmd.node_id,
                    &cmd.op,
                    &cmd.payload,
                    &mut s.ext_caches,
                );
                for event in events {
                    crate::test_protocol::emit_wire(&event);
                }
            }
        }
        IncomingMessage::AdvanceFrame { timestamp } => {
            if let Some(tag) = s.core.active_subscriptions.get("on_animation_frame") {
                crate::test_protocol::emit_wire(
                    &julep_core::protocol::OutgoingEvent::animation_frame(
                        tag.clone(),
                        timestamp as u128,
                    ),
                );
            }
        }
    }
}

/// Handle a ScreenshotCapture message.
///
/// Uses iced's `Headless` renderer trait (backed by tiny-skia) to produce
/// real RGBA pixel data without a display server or GPU. Builds an iced
/// `UserInterface` from the current tree, draws it, and captures pixels
/// via `renderer.screenshot()`.
fn handle_screenshot_capture(s: &mut Session, id: String, name: String, width: u32, height: u32) {
    use iced_test::core::renderer::Headless as HeadlessTrait;
    use iced_test::core::theme::Base;
    use sha2::{Digest, Sha256};

    let root = match s.core.tree.root() {
        Some(r) => r,
        None => {
            crate::renderer::emitters::emit_screenshot_response(&id, &name, "", 0, 0, &[]);
            return;
        }
    };

    // Prepare caches and build the iced Element from the tree.
    julep_core::widgets::ensure_caches(root, &mut s.core.caches);
    let ctx = julep_core::extensions::RenderCtx {
        caches: &s.core.caches,
        images: &s.images,
        theme: &s.theme,
        extensions: &s.dispatcher,
        default_text_size: s.core.default_text_size,
        default_font: s.core.default_font,
    };
    let element: iced::Element<'_, julep_core::message::Message> =
        julep_core::widgets::render(root, ctx);

    // Create a headless tiny-skia renderer using the host's defaults.
    let renderer_settings = iced::advanced::renderer::Settings {
        default_font: s.core.default_font.unwrap_or(iced::Font::DEFAULT),
        default_text_size: iced::Pixels(s.core.default_text_size.unwrap_or(16.0)),
    };
    let mut renderer =
        match iced::futures::executor::block_on(iced::Renderer::new(renderer_settings, None)) {
            Some(r) => r,
            None => {
                log::error!("failed to create headless renderer");
                crate::renderer::emitters::emit_screenshot_response(&id, &name, "", 0, 0, &[]);
                return;
            }
        };

    let size = iced::Size::new(width as f32, height as f32);
    let mut ui = iced_test::runtime::UserInterface::build(
        element,
        size,
        iced_test::runtime::user_interface::Cache::default(),
        &mut renderer,
    );

    let base = s.theme.base();
    ui.draw(
        &mut renderer,
        &s.theme,
        &iced_test::core::renderer::Style {
            text_color: base.text_color,
        },
        iced::mouse::Cursor::Unavailable,
    );

    let phys_size = iced::Size::new(width, height);
    let rgba = renderer.screenshot(phys_size, 1.0, base.background_color);

    let hash = {
        let mut hasher = Sha256::new();
        hasher.update(&rgba);
        format!("{:x}", hasher.finalize())
    };

    crate::renderer::emitters::emit_screenshot_response(&id, &name, &hash, width, height, &rgba);
}
