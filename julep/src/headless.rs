//! Headless mode (`--headless`): Core + wire protocol, no display.
//!
//! Reads framed messages from stdin, processes them through
//! [`Core`](julep_core::engine::Core), and writes responses to stdout.
//! No iced daemon, no windows, no GPU. Useful for CI, integration
//! testing, and headless screenshot capture via tiny-skia.
//!
//! A persistent [`iced_test::runtime::user_interface::Cache`] is
//! maintained across the session so that iced widget state (scroll
//! positions, focus, cursor positions) survives between screenshots
//! and interactions. The renderer is also created once and reused.
//! Interactions inject real iced [`Event`]s through
//! [`UserInterface::update`] so that widget-level state (focus rings,
//! scroll offsets, text cursors) changes in response to test actions.

use std::io::{self, BufRead};

use iced::advanced::renderer::Headless as HeadlessTrait;
use iced::keyboard::{self, Key, Modifiers};
use iced::mouse;
use iced::{Event, Point, Size, Theme};

use iced_test::core::SmolStr;

use julep_core::codec::Codec;
use julep_core::engine::Core;
use julep_core::extensions::{ExtensionCaches, ExtensionDispatcher, RenderCtx};
use julep_core::image_registry::ImageRegistry;
use julep_core::protocol::IncomingMessage;

use serde_json::Value;

/// Default screenshot width when not specified by the caller.
const DEFAULT_SCREENSHOT_WIDTH: u32 = 1024;
/// Default screenshot height when not specified by the caller.
const DEFAULT_SCREENSHOT_HEIGHT: u32 = 768;
/// Maximum screenshot dimension (width or height). Matches
/// `ImageRegistry::MAX_DIMENSION`. Prevents untrusted input from
/// triggering a multi-GiB RGBA allocation.
const MAX_SCREENSHOT_DIMENSION: u32 = 16384;

type UiCache = iced_test::runtime::user_interface::Cache;

/// All mutable state for a headless session.
///
/// Holds a persistent iced renderer and UI cache so that widget state
/// (scroll positions, focus, cursor positions) survives across messages.
struct Session {
    core: Core,
    theme: Theme,
    dispatcher: ExtensionDispatcher,
    ext_caches: ExtensionCaches,
    images: ImageRegistry,
    renderer: iced::Renderer,
    ui_cache: UiCache,
    viewport_size: Size,
    cursor: mouse::Cursor,
}

impl Session {
    fn new(dispatcher: ExtensionDispatcher) -> Self {
        let renderer_settings = iced::advanced::renderer::Settings {
            default_font: iced::Font::DEFAULT,
            default_text_size: iced::Pixels(16.0),
        };
        let renderer =
            iced::futures::executor::block_on(iced::Renderer::new(renderer_settings, None))
                .expect("headless renderer must be available (tiny-skia backend)");

        Self {
            core: Core::new(),
            theme: Theme::Dark,
            dispatcher,
            ext_caches: ExtensionCaches::new(),
            images: ImageRegistry::new(),
            renderer,
            ui_cache: UiCache::default(),
            viewport_size: Size::new(
                DEFAULT_SCREENSHOT_WIDTH as f32,
                DEFAULT_SCREENSHOT_HEIGHT as f32,
            ),
            cursor: mouse::Cursor::Unavailable,
        }
    }

    /// Rebuild the renderer when default font/text size changes.
    fn rebuild_renderer(&mut self) {
        let renderer_settings = iced::advanced::renderer::Settings {
            default_font: self.core.default_font.unwrap_or(iced::Font::DEFAULT),
            default_text_size: iced::Pixels(self.core.default_text_size.unwrap_or(16.0)),
        };
        if let Some(r) =
            iced::futures::executor::block_on(iced::Renderer::new(renderer_settings, None))
        {
            self.renderer = r;
            // The renderer changed, so the old cache is invalid.
            self.ui_cache = UiCache::default();
        }
    }

    /// Build a temporary UserInterface from the current tree, run a
    /// closure against it, then store the resulting cache back.
    ///
    /// Returns `None` if the tree is empty (no root node).
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

        let cache = std::mem::take(&mut self.ui_cache);
        let mut ui = iced_test::runtime::UserInterface::build(
            element,
            self.viewport_size,
            cache,
            &mut self.renderer,
        );

        let result = f(&mut ui, &mut self.renderer, self.cursor);

        self.ui_cache = ui.into_cache();
        Some(result)
    }

    /// Process a RedrawRequested event through the UI after a tree change.
    /// This lets iced widgets settle their internal state (layout, etc.).
    fn settle_ui(&mut self) {
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

    /// Inject a sequence of iced events into the persistent UI.
    fn inject_events(&mut self, events: &[Event]) {
        if events.is_empty() {
            return;
        }
        // Update cursor from any CursorMoved events before building the UI
        // so the cursor position is current when the UI processes the events.
        for event in events {
            if let Event::Mouse(mouse::Event::CursorMoved { position }) = event {
                self.cursor = mouse::Cursor::Available(*position);
            }
        }
        self.with_ui(|ui, renderer, cursor| {
            let mut messages = Vec::new();
            let _status = ui.update(events, cursor, renderer, &mut messages);
        });
    }
}

// ---------------------------------------------------------------------------
// Key string -> iced Key conversion
// ---------------------------------------------------------------------------

/// Convert a key name string (as sent by the test protocol) to an iced
/// `keyboard::Key`. Named keys use their Debug format (e.g. "Enter",
/// "Tab", "ArrowUp"); single characters become `Key::Character`.
pub(crate) fn parse_iced_key(name: &str) -> Key {
    match name {
        "Enter" | "enter" | "Return" | "return" => Key::Named(keyboard::key::Named::Enter),
        "Tab" | "tab" => Key::Named(keyboard::key::Named::Tab),
        "Space" | "space" | " " => Key::Named(keyboard::key::Named::Space),
        "Backspace" | "backspace" => Key::Named(keyboard::key::Named::Backspace),
        "Delete" | "delete" => Key::Named(keyboard::key::Named::Delete),
        "Escape" | "escape" | "Esc" | "esc" => Key::Named(keyboard::key::Named::Escape),
        "ArrowUp" | "Up" | "up" => Key::Named(keyboard::key::Named::ArrowUp),
        "ArrowDown" | "Down" | "down" => Key::Named(keyboard::key::Named::ArrowDown),
        "ArrowLeft" | "Left" | "left" => Key::Named(keyboard::key::Named::ArrowLeft),
        "ArrowRight" | "Right" | "right" => Key::Named(keyboard::key::Named::ArrowRight),
        "Home" | "home" => Key::Named(keyboard::key::Named::Home),
        "End" | "end" => Key::Named(keyboard::key::Named::End),
        "PageUp" | "pageup" => Key::Named(keyboard::key::Named::PageUp),
        "PageDown" | "pagedown" => Key::Named(keyboard::key::Named::PageDown),
        "F1" => Key::Named(keyboard::key::Named::F1),
        "F2" => Key::Named(keyboard::key::Named::F2),
        "F3" => Key::Named(keyboard::key::Named::F3),
        "F4" => Key::Named(keyboard::key::Named::F4),
        "F5" => Key::Named(keyboard::key::Named::F5),
        "F6" => Key::Named(keyboard::key::Named::F6),
        "F7" => Key::Named(keyboard::key::Named::F7),
        "F8" => Key::Named(keyboard::key::Named::F8),
        "F9" => Key::Named(keyboard::key::Named::F9),
        "F10" => Key::Named(keyboard::key::Named::F10),
        "F11" => Key::Named(keyboard::key::Named::F11),
        "F12" => Key::Named(keyboard::key::Named::F12),
        s if s.len() == 1 => Key::Character(SmolStr::new(s)),
        s => {
            // Try lowercase single char
            let lower = s.to_lowercase();
            if lower.chars().count() == 1 {
                Key::Character(SmolStr::new(&lower))
            } else {
                Key::Character(SmolStr::new(s))
            }
        }
    }
}

/// Build iced `Modifiers` from parsed test protocol modifiers JSON.
pub(crate) fn parse_iced_modifiers(mods: &Value) -> Modifiers {
    let mut m = Modifiers::empty();
    if mods.get("shift").and_then(|v| v.as_bool()).unwrap_or(false) {
        m |= Modifiers::SHIFT;
    }
    if mods.get("ctrl").and_then(|v| v.as_bool()).unwrap_or(false) {
        m |= Modifiers::CTRL;
    }
    if mods.get("alt").and_then(|v| v.as_bool()).unwrap_or(false) {
        m |= Modifiers::ALT;
    }
    if mods.get("logo").and_then(|v| v.as_bool()).unwrap_or(false) {
        m |= Modifiers::LOGO;
    }
    m
}

/// Build a KeyPressed iced event.
pub(crate) fn make_key_pressed(key: Key, modifiers: Modifiers, text: Option<SmolStr>) -> Event {
    Event::Keyboard(keyboard::Event::KeyPressed {
        key: key.clone(),
        modified_key: key,
        physical_key: keyboard::key::Physical::Unidentified(
            keyboard::key::NativeCode::Unidentified,
        ),
        location: keyboard::Location::Standard,
        modifiers,
        text,
        repeat: false,
    })
}

/// Build a KeyReleased iced event.
pub(crate) fn make_key_released(key: Key, modifiers: Modifiers) -> Event {
    Event::Keyboard(keyboard::Event::KeyReleased {
        key: key.clone(),
        modified_key: key,
        physical_key: keyboard::key::Physical::Unidentified(
            keyboard::key::NativeCode::Unidentified,
        ),
        location: keyboard::Location::Standard,
        modifiers,
    })
}

// ---------------------------------------------------------------------------
// Interaction -> iced events
// ---------------------------------------------------------------------------

/// Convert a test protocol interaction into a sequence of iced events.
///
/// Returns an empty vec for action types that don't map to iced events
/// (synthetic-only actions like paste, sort, canvas_*, pane_focus_cycle).
pub(crate) fn interaction_to_iced_events(
    action: &str,
    _widget_id: Option<&str>,
    payload: &Value,
    cursor: mouse::Cursor,
) -> Vec<Event> {
    match action {
        "click" | "toggle" | "select" => {
            // Click at the current cursor position.
            // In a real scenario we'd find the widget bounds, but the cursor
            // should already be positioned (or we use a default position).
            let pos = match cursor {
                mouse::Cursor::Available(p) | mouse::Cursor::Levitating(p) => p,
                mouse::Cursor::Unavailable => Point::new(0.0, 0.0),
            };
            vec![
                Event::Mouse(mouse::Event::CursorMoved { position: pos }),
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)),
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)),
            ]
        }
        "type_text" => {
            let text = payload.get("text").and_then(|v| v.as_str()).unwrap_or("");
            text.chars()
                .flat_map(|c| {
                    let s = SmolStr::new(c.to_string());
                    let key = Key::Character(s.clone());
                    [
                        make_key_pressed(key.clone(), Modifiers::empty(), Some(s)),
                        make_key_released(key, Modifiers::empty()),
                    ]
                })
                .collect()
        }
        "type_key" => {
            let payload_map = payload.as_object();
            let (key_str, mods_json) = crate::test_protocol::parse_key_and_modifiers(payload_map);
            let key = parse_iced_key(&key_str);
            let modifiers = parse_iced_modifiers(&mods_json);
            let text = match &key {
                Key::Character(c) if modifiers.is_empty() => Some(c.clone()),
                _ => None,
            };
            vec![
                make_key_pressed(key.clone(), modifiers, text),
                make_key_released(key, modifiers),
            ]
        }
        "press" => {
            let payload_map = payload.as_object();
            let (key_str, mods_json) = crate::test_protocol::parse_key_and_modifiers(payload_map);
            let key = parse_iced_key(&key_str);
            let modifiers = parse_iced_modifiers(&mods_json);
            let text = match &key {
                Key::Character(c) if modifiers.is_empty() => Some(c.clone()),
                _ => None,
            };
            vec![make_key_pressed(key, modifiers, text)]
        }
        "release" => {
            let payload_map = payload.as_object();
            let (key_str, mods_json) = crate::test_protocol::parse_key_and_modifiers(payload_map);
            let key = parse_iced_key(&key_str);
            let modifiers = parse_iced_modifiers(&mods_json);
            vec![make_key_released(key, modifiers)]
        }
        "submit" => {
            let key = Key::Named(keyboard::key::Named::Enter);
            vec![
                make_key_pressed(key.clone(), Modifiers::empty(), None),
                make_key_released(key, Modifiers::empty()),
            ]
        }
        "scroll" => {
            let delta_x = payload
                .get("delta_x")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0) as f32;
            let delta_y = payload
                .get("delta_y")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0) as f32;
            vec![Event::Mouse(mouse::Event::WheelScrolled {
                delta: mouse::ScrollDelta::Lines {
                    x: delta_x,
                    y: delta_y,
                },
            })]
        }
        "move_to" => {
            let x = payload.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
            let y = payload.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
            vec![Event::Mouse(mouse::Event::CursorMoved {
                position: Point::new(x, y),
            })]
        }
        // Synthetic-only actions: no iced event injection needed.
        // The synthetic events emitted by handle_interact are sufficient.
        "paste" | "sort" | "canvas_press" | "canvas_release" | "canvas_move"
        | "pane_focus_cycle" | "slide" => vec![],
        _ => vec![],
    }
}

// ---------------------------------------------------------------------------
// Event loop
// ---------------------------------------------------------------------------

/// Run the headless event loop.
///
/// A persistent iced renderer and UI cache are maintained across the
/// session. Interactions inject real iced events so widget state (scroll
/// positions, focus, text cursors) persists. Screenshots capture the
/// accumulated widget state.
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
            // Resolve the widget ID for synthetic event emission.
            let widget_id = resolve_widget_id(&s.core, &selector);

            // Inject real iced events into the persistent UI so widget
            // state (focus, scroll, cursor) updates.
            let iced_events =
                interaction_to_iced_events(&action, widget_id.as_deref(), &payload, s.cursor);
            if !iced_events.is_empty() {
                s.inject_events(&iced_events);
            }

            // Emit synthetic events back to the host (unchanged behaviour).
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
            s.ui_cache = UiCache::default();
            s.cursor = mouse::Cursor::Unavailable;
            s.rebuild_renderer();
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

/// Resolve a widget ID from a selector, without emitting anything.
fn resolve_widget_id(core: &Core, selector: &Value) -> Option<String> {
    use crate::test_protocol::Selector;
    use crate::test_protocol::parse_selector;

    match parse_selector(selector)? {
        Selector::Id(wid) => Some(wid),
        Selector::Text(text) => core
            .tree
            .root()
            .and_then(|root| find_id_by_text(root, &text, 0)),
        Selector::Role(role) => core
            .tree
            .root()
            .and_then(|root| find_id_by_role(root, &role, 0)),
        Selector::Label(label) => core
            .tree
            .root()
            .and_then(|root| find_id_by_label(root, &label, 0)),
        Selector::Focused => core.tree.root().and_then(|root| find_id_focused(root, 0)),
    }
}

// Re-use the search helpers from test_protocol. They're pub, just
// not exported from the crate. Import them by full path.
use crate::test_protocol::{find_id_by_label, find_id_by_role, find_id_by_text, find_id_focused};

/// Handle a ScreenshotCapture message.
///
/// Uses the persistent renderer and UI cache to produce real RGBA pixel
/// data via tiny-skia. The screenshot reflects accumulated widget state
/// (scroll positions, focus, etc.) from prior interactions.
fn handle_screenshot_capture(s: &mut Session, id: String, name: String, width: u32, height: u32) {
    use iced_test::core::theme::Base;
    use sha2::{Digest, Sha256};

    // Update viewport size for this screenshot.
    s.viewport_size = Size::new(width as f32, height as f32);

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
    let cache = std::mem::take(&mut s.ui_cache);
    let mut ui =
        iced_test::runtime::UserInterface::build(element, s.viewport_size, cache, &mut s.renderer);

    // Process a RedrawRequested so widgets can update their visual state.
    {
        let mut messages = Vec::new();
        let redraw = Event::Window(iced::window::Event::RedrawRequested(
            iced_test::core::time::Instant::now(),
        ));
        let _status = ui.update(&[redraw], s.cursor, &mut s.renderer, &mut messages);
    }

    let base = s.theme.base();
    ui.draw(
        &mut s.renderer,
        &s.theme,
        &iced_test::core::renderer::Style {
            text_color: base.text_color,
        },
        s.cursor,
    );

    // Store cache before taking the screenshot (screenshot doesn't
    // need the UI, just the renderer).
    s.ui_cache = ui.into_cache();

    let phys_size = iced::Size::new(width, height);
    let rgba = s.renderer.screenshot(phys_size, 1.0, base.background_color);

    let hash = {
        let mut hasher = Sha256::new();
        hasher.update(&rgba);
        format!("{:x}", hasher.finalize())
    };

    crate::renderer::emitters::emit_screenshot_response(&id, &name, &hash, width, height, &rgba);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_iced_key_named_enter() {
        assert_eq!(
            parse_iced_key("Enter"),
            Key::Named(keyboard::key::Named::Enter)
        );
        assert_eq!(
            parse_iced_key("enter"),
            Key::Named(keyboard::key::Named::Enter)
        );
    }

    #[test]
    fn parse_iced_key_named_tab() {
        assert_eq!(parse_iced_key("Tab"), Key::Named(keyboard::key::Named::Tab));
    }

    #[test]
    fn parse_iced_key_named_arrows() {
        assert_eq!(
            parse_iced_key("ArrowUp"),
            Key::Named(keyboard::key::Named::ArrowUp)
        );
        assert_eq!(
            parse_iced_key("Up"),
            Key::Named(keyboard::key::Named::ArrowUp)
        );
        assert_eq!(
            parse_iced_key("ArrowDown"),
            Key::Named(keyboard::key::Named::ArrowDown)
        );
    }

    #[test]
    fn parse_iced_key_single_char() {
        assert_eq!(parse_iced_key("a"), Key::Character(SmolStr::new("a")));
        assert_eq!(parse_iced_key("Z"), Key::Character(SmolStr::new("Z")));
    }

    #[test]
    fn parse_iced_key_function_keys() {
        assert_eq!(parse_iced_key("F1"), Key::Named(keyboard::key::Named::F1));
        assert_eq!(parse_iced_key("F12"), Key::Named(keyboard::key::Named::F12));
    }

    #[test]
    fn parse_iced_modifiers_from_json() {
        let mods = json!({"shift": true, "ctrl": true, "alt": false, "logo": false});
        let result = parse_iced_modifiers(&mods);
        assert!(result.shift());
        assert!(result.control());
        assert!(!result.alt());
        assert!(!result.logo());
    }

    #[test]
    fn parse_iced_modifiers_empty() {
        let mods = json!({});
        let result = parse_iced_modifiers(&mods);
        assert!(result.is_empty());
    }

    #[test]
    fn interaction_to_iced_events_click() {
        let events = interaction_to_iced_events(
            "click",
            Some("btn1"),
            &json!({}),
            mouse::Cursor::Available(Point::new(100.0, 50.0)),
        );
        assert_eq!(events.len(), 3); // CursorMoved + ButtonPressed + ButtonReleased
    }

    #[test]
    fn interaction_to_iced_events_type_text() {
        let events = interaction_to_iced_events(
            "type_text",
            Some("inp1"),
            &json!({"text": "hi"}),
            mouse::Cursor::Unavailable,
        );
        // 2 chars * 2 events each (press + release)
        assert_eq!(events.len(), 4);
    }

    #[test]
    fn interaction_to_iced_events_scroll() {
        let events = interaction_to_iced_events(
            "scroll",
            None,
            &json!({"delta_x": 0.0, "delta_y": -10.0}),
            mouse::Cursor::Unavailable,
        );
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                assert_eq!(*delta, mouse::ScrollDelta::Lines { x: 0.0, y: -10.0 });
            }
            _ => panic!("expected WheelScrolled"),
        }
    }

    #[test]
    fn interaction_to_iced_events_move_to() {
        let events = interaction_to_iced_events(
            "move_to",
            None,
            &json!({"x": 42.0, "y": 84.0}),
            mouse::Cursor::Unavailable,
        );
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::Mouse(mouse::Event::CursorMoved { position }) => {
                assert_eq!(*position, Point::new(42.0, 84.0));
            }
            _ => panic!("expected CursorMoved"),
        }
    }

    #[test]
    fn interaction_to_iced_events_synthetic_only() {
        // These actions should produce no iced events.
        for action in &[
            "paste",
            "sort",
            "canvas_press",
            "canvas_release",
            "canvas_move",
            "pane_focus_cycle",
            "slide",
        ] {
            let events = interaction_to_iced_events(
                action,
                Some("w1"),
                &json!({}),
                mouse::Cursor::Unavailable,
            );
            assert!(
                events.is_empty(),
                "action '{action}' should produce no iced events"
            );
        }
    }

    #[test]
    fn interaction_to_iced_events_submit() {
        let events = interaction_to_iced_events(
            "submit",
            Some("inp1"),
            &json!({}),
            mouse::Cursor::Unavailable,
        );
        assert_eq!(events.len(), 2); // KeyPressed(Enter) + KeyReleased(Enter)
    }

    #[test]
    fn interaction_to_iced_events_type_key() {
        let events = interaction_to_iced_events(
            "type_key",
            None,
            &json!({"key": "ctrl+s"}),
            mouse::Cursor::Unavailable,
        );
        assert_eq!(events.len(), 2); // KeyPressed + KeyReleased
    }

    #[test]
    fn interaction_to_iced_events_press_release() {
        let press = interaction_to_iced_events(
            "press",
            None,
            &json!({"key": "a"}),
            mouse::Cursor::Unavailable,
        );
        assert_eq!(press.len(), 1);

        let release = interaction_to_iced_events(
            "release",
            None,
            &json!({"key": "a"}),
            mouse::Cursor::Unavailable,
        );
        assert_eq!(release.len(), 1);
    }
}
