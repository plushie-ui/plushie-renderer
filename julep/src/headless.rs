pub mod headless_mode {
    use std::io::{self, BufRead};

    use iced::Theme;
    use serde_json::Value;

    use julep_core::codec::Codec;
    use julep_core::engine::Core;
    use julep_core::protocol::{IncomingMessage, ScreenshotResponseEmpty};

    /// Default screenshot width when not specified by the caller.
    const DEFAULT_SCREENSHOT_WIDTH: u32 = 1024;
    /// Default screenshot height when not specified by the caller.
    const DEFAULT_SCREENSHOT_HEIGHT: u32 = 768;
    /// Maximum screenshot dimension (width or height). Matches ImageRegistry::MAX_DIMENSION.
    /// Prevents untrusted input from triggering a multi-GiB RGBA allocation.
    const MAX_SCREENSHOT_DIMENSION: u32 = 16384;

    use julep_core::extensions::{ExtensionCaches, ExtensionDispatcher};

    /// Run the headless mode event loop.
    /// No iced::daemon. Reads framed messages from stdin, processes through Core,
    /// writes responses to stdout using the negotiated wire codec.
    ///
    /// Extensions are initialized when a Settings message arrives (via
    /// `init_all`) and prepared after each tree-changing message (via
    /// `prepare_all`). Full extension interactivity (handle_event,
    /// handle_command) is supported. Note that extension rendering in
    /// headless mode goes through the same `widgets::render` path used
    /// for screenshot capture, so extensions that rely on real iced widget
    /// state (e.g. focus, scroll position) won't behave identically to the
    /// full daemon mode.
    pub fn run(forced_codec: Option<Codec>, mut dispatcher: ExtensionDispatcher) {
        let mut core = Core::new();
        let mut ext_caches = ExtensionCaches::new();
        let mut theme = Theme::Dark;
        let stdin = io::stdin();
        let mut reader = io::BufReader::new(stdin.lock());

        // Determine codec: forced by CLI flag, or auto-detected from first byte.
        let codec = match forced_codec {
            Some(c) => c,
            None => {
                let buf = reader.fill_buf().unwrap_or(&[]);
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
                    Ok(msg) => {
                        handle_message(&mut core, &mut theme, &mut dispatcher, &mut ext_caches, msg)
                    }
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

    fn handle_message(
        core: &mut Core,
        theme: &mut Theme,
        dispatcher: &mut ExtensionDispatcher,
        ext_caches: &mut ExtensionCaches,
        msg: IncomingMessage,
    ) {
        // Track whether this message might change the tree or settings so
        // we know when to call extension lifecycle hooks.
        let is_settings = matches!(msg, IncomingMessage::Settings { .. });
        let is_snapshot = matches!(msg, IncomingMessage::Snapshot { .. });
        let is_tree_change = is_snapshot || matches!(msg, IncomingMessage::Patch { .. });

        match msg {
            // Normal messages go through Core::apply()
            IncomingMessage::Snapshot { .. }
            | IncomingMessage::Patch { .. }
            | IncomingMessage::EffectRequest { .. }
            | IncomingMessage::WidgetOp { .. }
            | IncomingMessage::SubscriptionRegister { .. }
            | IncomingMessage::SubscriptionUnregister { .. }
            | IncomingMessage::WindowOp { .. }
            | IncomingMessage::Settings { .. }
            | IncomingMessage::ImageOp { .. } => {
                // Extract extension_config before apply consumes the message.
                let ext_config = if is_settings {
                    if let IncomingMessage::Settings { ref settings } = msg {
                        settings
                            .get("extension_config")
                            .cloned()
                            .unwrap_or(Value::Null)
                    } else {
                        Value::Null
                    }
                } else {
                    Value::Null
                };

                let effects = core.apply(msg);

                // Route extension config on Settings.
                if is_settings {
                    dispatcher.init_all(&ext_config);
                }

                // In headless mode, we handle effects differently:
                // - SyncWindows: no-op (no real windows)
                // - EmitEvent: write to stdout
                // - EmitEffectResponse: write to stdout
                // - WidgetOp: no-op (no real iced widgets to operate on)
                // - WindowOp: no-op (no real windows)
                // - ThemeChanged: track for screenshot rendering
                for effect in effects {
                    match effect {
                        julep_core::engine::CoreEffect::EmitEvent(event) => {
                            crate::test_protocol::emit_wire(&event);
                        }
                        julep_core::engine::CoreEffect::EmitEffectResponse(response) => {
                            crate::test_protocol::emit_wire(&response);
                        }
                        julep_core::engine::CoreEffect::SpawnAsyncEffect {
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
                        julep_core::engine::CoreEffect::ThemeChanged(t) => {
                            *theme = t;
                        }
                        _ => {} // No-op for window/widget ops in headless
                    }
                }

                // Prepare extensions after tree changes (Snapshot/Patch).
                if is_tree_change {
                    if is_snapshot {
                        dispatcher.clear_poisoned();
                    }
                    if let Some(root) = core.tree.root() {
                        dispatcher.prepare_all(root, ext_caches, theme);
                    }
                }
            }

            // Test protocol messages -- dispatched to shared module.
            IncomingMessage::Query {
                id,
                target,
                selector,
            } => {
                crate::test_protocol::handle_query(core, id, target, selector);
            }
            IncomingMessage::Interact {
                id,
                action,
                selector,
                payload,
            } => {
                crate::test_protocol::handle_interact(core, id, action, selector, payload);
            }
            IncomingMessage::SnapshotCapture { id, name, .. } => {
                crate::test_protocol::handle_snapshot_capture(core, id, name);
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
                handle_screenshot_capture(core, theme, dispatcher, id, name, w, h);
            }
            IncomingMessage::Reset { id } => {
                // Clean up extension state before wiping core.
                dispatcher.reset(ext_caches);
                *theme = Theme::Dark;
                crate::test_protocol::handle_reset(core, id);
            }
            IncomingMessage::ExtensionCommand {
                node_id,
                op,
                payload,
            } => {
                let events = dispatcher.handle_command(&node_id, &op, &payload, ext_caches);
                for event in events {
                    crate::test_protocol::emit_wire(&event);
                }
            }
            IncomingMessage::ExtensionCommandBatch { commands } => {
                for cmd in commands {
                    let events =
                        dispatcher.handle_command(&cmd.node_id, &cmd.op, &cmd.payload, ext_caches);
                    for event in events {
                        crate::test_protocol::emit_wire(&event);
                    }
                }
            }
            IncomingMessage::AdvanceFrame { timestamp } => {
                if let Some(tag) = core.active_subscriptions.get("on_animation_frame") {
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

    /// Handle a ScreenshotCapture message in headless mode.
    ///
    /// Uses iced's `Headless` renderer trait (backed by tiny-skia) to produce
    /// real RGBA pixel data without a display server or GPU. Builds an iced
    /// `UserInterface` from the current tree, draws it, and captures pixels
    /// via `renderer.screenshot()`.
    fn handle_screenshot_capture(
        core: &mut Core,
        theme: &Theme,
        dispatcher: &ExtensionDispatcher,
        id: String,
        name: String,
        width: u32,
        height: u32,
    ) {
        use iced_test::core::renderer::Headless as HeadlessTrait;
        use iced_test::core::theme::Base;
        use sha2::{Digest, Sha256};

        let root = match core.tree.root() {
            Some(r) => r,
            None => {
                crate::test_protocol::emit_wire(&ScreenshotResponseEmpty::new(id, name));
                return;
            }
        };

        // Prepare caches and build the iced Element from the tree.
        julep_core::widgets::ensure_caches(root, &mut core.caches);
        let images = julep_core::image_registry::ImageRegistry::new();
        let element: iced::Element<'_, julep_core::message::Message> =
            julep_core::widgets::render(root, &core.caches, &images, theme, dispatcher);

        // Create a headless tiny-skia renderer.
        let renderer_settings = iced::advanced::renderer::Settings {
            default_font: iced::Font::DEFAULT,
            default_text_size: iced::Pixels(16.0),
        };
        let mut renderer =
            match iced::futures::executor::block_on(iced::Renderer::new(renderer_settings, None)) {
                Some(r) => r,
                None => {
                    log::error!("failed to create headless renderer");
                    crate::test_protocol::emit_wire(&ScreenshotResponseEmpty::new(id, name));
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

        let base = theme.base();
        ui.draw(
            &mut renderer,
            theme,
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

        julep_core::protocol::emit_screenshot_response(&id, &name, &hash, width, height, &rgba);
    }
}
