//! Headless and mock modes for the julep renderer.
//!
//! `--headless`: real rendering via tiny-skia with persistent widget
//! state. Accurate screenshots after interactions.
//!
//! `--mock`: protocol-only, no rendering. Stub screenshots. Fast
//! protocol-level testing from any language.
//!
//! Both modes read framed messages from stdin, process them through
//! [`Core`](julep_core::engine::Core), and write responses to stdout.
//! No iced daemon, no windows, no GPU. The difference is whether a
//! persistent iced renderer and UI cache are maintained for real
//! screenshot capture (`--headless`) or omitted for speed (`--mock`).

use std::io::{self, BufRead};

use iced::advanced::renderer::Headless as HeadlessTrait;
use iced::mouse;
use iced::{Event, Size, Theme};

use julep_core::codec::Codec;
use julep_core::engine::Core;
use julep_core::extensions::{ExtensionCaches, ExtensionDispatcher, RenderCtx};
use julep_core::image_registry::ImageRegistry;
use julep_core::protocol::IncomingMessage;

use crate::scripting::{interaction_to_iced_events, resolve_widget_id};

/// Default screenshot width when not specified by the caller.
const DEFAULT_SCREENSHOT_WIDTH: u32 = 1024;
/// Default screenshot height when not specified by the caller.
const DEFAULT_SCREENSHOT_HEIGHT: u32 = 768;
/// Maximum screenshot dimension (width or height). Matches
/// `ImageRegistry::MAX_DIMENSION`. Prevents untrusted input from
/// triggering a multi-GiB RGBA allocation.
const MAX_SCREENSHOT_DIMENSION: u32 = 16384;

/// Execution mode for the headless/mock event loop.
pub(crate) enum Mode {
    /// Real rendering via tiny-skia with persistent widget state.
    Headless,
    /// Protocol-only, no rendering. Stub screenshots.
    Mock,
}

type UiCache = iced_test::runtime::user_interface::Cache;

/// Persistent iced renderer and UI cache. Present in `--headless`
/// mode, absent in `--mock` mode.
struct UiState {
    renderer: iced::Renderer,
    ui_cache: UiCache,
    viewport_size: Size,
    cursor: mouse::Cursor,
}

/// All mutable state for a headless/mock session.
///
/// When `ui` is `Some`, the session maintains a persistent iced
/// renderer and UI cache for real screenshot capture and widget state
/// tracking. When `None` (mock mode), rendering is skipped entirely.
struct Session {
    core: Core,
    theme: Theme,
    dispatcher: ExtensionDispatcher,
    ext_caches: ExtensionCaches,
    images: ImageRegistry,
    /// None in --mock mode (no rendering).
    ui: Option<UiState>,
}

impl Session {
    fn new(dispatcher: ExtensionDispatcher, mode: Mode) -> Self {
        let ui = if matches!(mode, Mode::Headless) {
            let renderer_settings = iced::advanced::renderer::Settings {
                default_font: iced::Font::DEFAULT,
                default_text_size: iced::Pixels(16.0),
            };
            let renderer =
                iced::futures::executor::block_on(iced::Renderer::new(renderer_settings, None))
                    .expect("headless renderer must be available (tiny-skia backend)");

            Some(UiState {
                renderer,
                ui_cache: UiCache::default(),
                viewport_size: Size::new(
                    DEFAULT_SCREENSHOT_WIDTH as f32,
                    DEFAULT_SCREENSHOT_HEIGHT as f32,
                ),
                cursor: mouse::Cursor::Unavailable,
            })
        } else {
            None
        };

        Self {
            core: Core::new(),
            theme: Theme::Dark,
            dispatcher,
            ext_caches: ExtensionCaches::new(),
            images: ImageRegistry::new(),
            ui,
        }
    }

    /// Rebuild the renderer when default font/text size changes.
    fn rebuild_renderer(&mut self) {
        let Some(ui_state) = &mut self.ui else {
            return;
        };
        let renderer_settings = iced::advanced::renderer::Settings {
            default_font: self.core.default_font.unwrap_or(iced::Font::DEFAULT),
            default_text_size: iced::Pixels(self.core.default_text_size.unwrap_or(16.0)),
        };
        if let Some(r) =
            iced::futures::executor::block_on(iced::Renderer::new(renderer_settings, None))
        {
            ui_state.renderer = r;
            // The renderer changed, so the old cache is invalid.
            ui_state.ui_cache = UiCache::default();
        }
    }

    /// Build a temporary UserInterface from the current tree, run a
    /// closure against it, then store the resulting cache back.
    ///
    /// Returns `None` if the tree is empty (no root node) or if
    /// rendering is disabled (mock mode).
    fn with_ui<R>(
        &mut self,
        f: impl FnOnce(
            &mut iced_test::runtime::UserInterface<
                '_,
                julep_core::message::Message,
                Theme,
                iced::Renderer,
            >,
            &mut iced::Renderer,
            mouse::Cursor,
        ) -> R,
    ) -> Option<R> {
        let ui_state = self.ui.as_mut()?;
        let root = self.core.tree.root()?;

        julep_core::widgets::ensure_caches(root, &mut self.core.caches);
        let ctx = RenderCtx {
            caches: &self.core.caches,
            images: &self.images,
            theme: &self.theme,
            extensions: &self.dispatcher,
            default_text_size: self.core.default_text_size,
            default_font: self.core.default_font,
        };
        let element = julep_core::widgets::render(root, ctx);

        let cache = std::mem::take(&mut ui_state.ui_cache);
        let mut ui = iced_test::runtime::UserInterface::build(
            element,
            ui_state.viewport_size,
            cache,
            &mut ui_state.renderer,
        );

        let result = f(&mut ui, &mut ui_state.renderer, ui_state.cursor);

        ui_state.ui_cache = ui.into_cache();
        Some(result)
    }

    /// Process a RedrawRequested event through the UI after a tree change.
    /// This lets iced widgets settle their internal state (layout, etc.).
    /// No-op in mock mode.
    fn settle_ui(&mut self) {
        if self.ui.is_some() {
            self.with_ui(|ui, renderer, cursor| {
                let mut messages = Vec::new();
                let redraw = Event::Window(iced::window::Event::RedrawRequested(
                    iced_test::core::time::Instant::now(),
                ));
                let _status = ui.update(&[redraw], cursor, renderer, &mut messages);
                // Messages are discarded -- julep manages state through the
                // wire protocol, not through iced's message loop.
            });
        }
    }

    /// Inject a sequence of iced events into the persistent UI.
    /// No-op in mock mode.
    fn inject_events(&mut self, events: &[Event]) {
        if self.ui.is_none() || events.is_empty() {
            return;
        }
        // Update cursor from any CursorMoved events before building the UI
        // so the cursor position is current when the UI processes the events.
        if let Some(ui_state) = &mut self.ui {
            for event in events {
                if let Event::Mouse(mouse::Event::CursorMoved { position }) = event {
                    ui_state.cursor = mouse::Cursor::Available(*position);
                }
            }
        }
        self.with_ui(|ui, renderer, cursor| {
            let mut messages = Vec::new();
            let _status = ui.update(events, cursor, renderer, &mut messages);
        });
    }
}

// ---------------------------------------------------------------------------
// Event loop
// ---------------------------------------------------------------------------

/// Run the headless/mock event loop.
///
/// In `Mode::Headless`, a persistent iced renderer and UI cache are
/// maintained. Interactions inject real iced events so widget state
/// (scroll positions, focus, text cursors) persists. Screenshots
/// capture the accumulated widget state.
///
/// In `Mode::Mock`, no renderer is created. Screenshots return empty
/// stubs. This is the fastest option for protocol-level testing.
pub fn run(forced_codec: Option<Codec>, dispatcher: ExtensionDispatcher, mode: Mode) {
    let mut session = Session::new(dispatcher, mode);
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
                Ok(msg) => {
                    if let Err(e) = handle_message(&mut session, msg) {
                        log::error!("write error: {e}");
                        break;
                    }
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

fn handle_message(s: &mut Session, msg: IncomingMessage) -> io::Result<()> {
    let is_snapshot = matches!(msg, IncomingMessage::Snapshot { .. });
    let is_tree_change = is_snapshot || matches!(msg, IncomingMessage::Patch { .. });
    let is_settings = matches!(msg, IncomingMessage::Settings { .. });

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
                        crate::scripting::emit_wire(&event)?;
                    }
                    CoreEffect::EmitEffectResponse(response) => {
                        crate::scripting::emit_wire(&response)?;
                    }
                    CoreEffect::SpawnAsyncEffect {
                        request_id,
                        effect_type,
                        ..
                    } => {
                        let mode = if s.ui.is_some() { "headless" } else { "mock" };
                        log::debug!(
                            "{mode}: async effect {effect_type} returning cancelled \
                             (no display)"
                        );
                        crate::scripting::emit_wire(&julep_core::protocol::EffectResponse::error(
                            request_id,
                            "cancelled".to_string(),
                        ))?;
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
                        let mode = if s.ui.is_some() { "headless" } else { "mock" };
                        if let Err(e) = s.images.apply_op(&op, &handle, data, pixels, width, height)
                        {
                            log::warn!("{mode}: image_op {op} failed: {e}");
                        }
                    }
                    CoreEffect::ExtensionConfig(config) => {
                        s.dispatcher.init_all(&config);
                    }
                    // No-ops in headless/mock (no windows, no iced widget tree).
                    CoreEffect::SyncWindows => {}
                    CoreEffect::WidgetOp { .. } => {}
                    CoreEffect::WindowOp { .. } => {}
                    CoreEffect::ThemeFollowsSystem => {}
                }
            }

            // Rebuild renderer if defaults changed (Settings message).
            if is_settings {
                s.rebuild_renderer();
            }

            // Prepare extensions after tree changes (Snapshot/Patch).
            if is_tree_change {
                if is_snapshot {
                    s.dispatcher.clear_poisoned();
                }
                if let Some(root) = s.core.tree.root() {
                    s.dispatcher.prepare_all(root, &mut s.ext_caches, &s.theme);
                }
                // Settle the UI so widget state reflects the new tree.
                s.settle_ui();
            }
        }

        // Scripting messages
        IncomingMessage::Query {
            id,
            target,
            selector,
        } => {
            crate::scripting::handle_query(&s.core, id, target, selector)?;
        }
        IncomingMessage::Interact {
            id,
            action,
            selector,
            payload,
        } => {
            // Resolve the widget ID for synthetic event emission.
            let widget_id = resolve_widget_id(&s.core, &selector);

            // Inject real iced events into the persistent UI so widget
            // state (focus, scroll, cursor) updates. No-op in mock mode
            // (inject_events returns early when ui is None).
            let cursor =
                s.ui.as_ref()
                    .map(|u| u.cursor)
                    .unwrap_or(mouse::Cursor::Unavailable);
            let iced_events =
                interaction_to_iced_events(&action, widget_id.as_deref(), &payload, cursor);
            if !iced_events.is_empty() {
                s.inject_events(&iced_events);
            }

            // Emit synthetic events back to the host.
            crate::scripting::handle_interact(&s.core, id, action, selector, payload)?;
        }
        IncomingMessage::SnapshotCapture { id, name, .. } => {
            crate::scripting::handle_snapshot_capture(&s.core, id, name)?;
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
            if let Some(ui_state) = &mut s.ui {
                ui_state.ui_cache = UiCache::default();
                ui_state.cursor = mouse::Cursor::Unavailable;
            }
            s.rebuild_renderer();
            crate::scripting::handle_reset(&mut s.core, id)?;
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
                crate::scripting::emit_wire(&event)?;
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
                    crate::scripting::emit_wire(&event)?;
                }
            }
        }
        IncomingMessage::AdvanceFrame { timestamp } => {
            if let Some(tag) = s.core.active_subscriptions.get("on_animation_frame") {
                crate::scripting::emit_wire(
                    &julep_core::protocol::OutgoingEvent::animation_frame(
                        tag.clone(),
                        timestamp as u128,
                    ),
                )?;
            }
        }
    }

    Ok(())
}

/// Handle a ScreenshotCapture message.
///
/// In headless mode, uses the persistent renderer and UI cache to
/// produce real RGBA pixel data via tiny-skia. In mock mode, returns
/// an empty stub.
fn handle_screenshot_capture(s: &mut Session, id: String, name: String, width: u32, height: u32) {
    if s.ui.is_none() {
        // Mock mode: stub screenshot.
        crate::renderer::emitters::emit_screenshot_response(&id, &name, "", 0, 0, &[]);
        return;
    }

    use iced_test::core::theme::Base;
    use sha2::{Digest, Sha256};

    let ui_state = s.ui.as_mut().unwrap();

    // Update viewport size for this screenshot.
    ui_state.viewport_size = Size::new(width as f32, height as f32);

    let root = match s.core.tree.root() {
        Some(r) => r,
        None => {
            crate::renderer::emitters::emit_screenshot_response(&id, &name, "", 0, 0, &[]);
            return;
        }
    };

    // Prepare caches and build the iced Element from the tree.
    julep_core::widgets::ensure_caches(root, &mut s.core.caches);
    let ctx = RenderCtx {
        caches: &s.core.caches,
        images: &s.images,
        theme: &s.theme,
        extensions: &s.dispatcher,
        default_text_size: s.core.default_text_size,
        default_font: s.core.default_font,
    };
    let element: iced::Element<'_, julep_core::message::Message> =
        julep_core::widgets::render(root, ctx);

    // Build UI with the persistent cache.
    let cache = std::mem::take(&mut ui_state.ui_cache);
    let mut ui = iced_test::runtime::UserInterface::build(
        element,
        ui_state.viewport_size,
        cache,
        &mut ui_state.renderer,
    );

    // Process a RedrawRequested so widgets can update their visual state.
    {
        let cursor = ui_state.cursor;
        let mut messages = Vec::new();
        let redraw = Event::Window(iced::window::Event::RedrawRequested(
            iced_test::core::time::Instant::now(),
        ));
        let _status = ui.update(&[redraw], cursor, &mut ui_state.renderer, &mut messages);
    }

    let base = s.theme.base();
    ui.draw(
        &mut ui_state.renderer,
        &s.theme,
        &iced_test::core::renderer::Style {
            text_color: base.text_color,
        },
        ui_state.cursor,
    );

    // Store cache before taking the screenshot (screenshot doesn't
    // need the UI, just the renderer).
    ui_state.ui_cache = ui.into_cache();

    let phys_size = iced::Size::new(width, height);
    let rgba = ui_state
        .renderer
        .screenshot(phys_size, 1.0, base.background_color);

    let hash = {
        let mut hasher = Sha256::new();
        hasher.update(&rgba);
        format!("{:x}", hasher.finalize())
    };

    crate::renderer::emitters::emit_screenshot_response(&id, &name, &hash, width, height, &rgba);
}
