//! Headless and mock modes for the toddy renderer.
//!
//! `--headless`: real rendering via tiny-skia with persistent widget
//! state. Accurate screenshots after interactions.
//!
//! `--mock`: protocol-only, no rendering. Stub screenshots. Fast
//! protocol-level testing from any language.
//!
//! Both modes read framed messages from stdin, process them through
//! [`Core`](toddy_core::engine::Core), and write responses to stdout.
//! No iced daemon, no windows, no GPU. The difference is whether a
//! persistent iced renderer and UI cache are maintained for real
//! screenshot capture (`--headless`) or omitted for speed (`--mock`).
//!
//! # Session multiplexing
//!
//! When `max_sessions > 1`, multiple sessions run concurrently in
//! separate threads. A reader thread dispatches incoming messages by
//! the `session` field to per-session threads. A writer thread
//! collects responses from all sessions and writes them to stdout.
//! Each session is fully isolated (own Core, caches, extensions, UI).

use std::collections::HashMap;
use std::io::{self, BufRead, Write as _};
use std::sync::mpsc;
use std::thread;

use iced::advanced::renderer::Headless as HeadlessTrait;
use iced::mouse;
use iced::{Event, Size, Theme};
use serde::Serialize;

use toddy_core::codec::Codec;
use toddy_core::engine::Core;
use toddy_core::extensions::{EventResult, ExtensionDispatcher, RenderCtx};
use toddy_core::image_registry::ImageRegistry;
use toddy_core::message::Message;
use toddy_core::protocol::{IncomingMessage, OutgoingEvent, SessionMessage};

use crate::renderer::emitters::message_to_event;
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
#[derive(Clone, Copy)]
pub(crate) enum Mode {
    /// Real rendering via tiny-skia with persistent widget state.
    Headless,
    /// Protocol-only, no rendering. Stub screenshots.
    Mock,
}

// ---------------------------------------------------------------------------
// WireWriter -- abstracts output destination
// ---------------------------------------------------------------------------

/// Encodes and writes wire messages. Each session owns one.
///
/// In single-session mode, writes directly to stdout. In multiplexed
/// mode, sends encoded bytes through a channel to the writer thread.
struct WireWriter {
    inner: WriterInner,
}

enum WriterInner {
    /// Write directly to stdout (single-session mode).
    Stdout,
    /// Send encoded bytes to the writer thread (multiplexed mode).
    Channel(mpsc::Sender<Vec<u8>>),
}

impl WireWriter {
    fn stdout() -> Self {
        Self {
            inner: WriterInner::Stdout,
        }
    }

    fn channel(tx: mpsc::Sender<Vec<u8>>) -> Self {
        Self {
            inner: WriterInner::Channel(tx),
        }
    }

    /// Encode a serializable value and write it.
    fn emit<T: Serialize>(&self, value: &T) -> io::Result<()> {
        let codec = Codec::get_global();
        let bytes = codec.encode(value).map_err(io::Error::other)?;
        self.write_bytes(&bytes)
    }

    /// Encode a message with a binary field (e.g. screenshot RGBA data)
    /// and write it.
    fn emit_binary(
        &self,
        map: serde_json::Map<String, serde_json::Value>,
        binary: Option<(&str, &[u8])>,
    ) -> io::Result<()> {
        let codec = Codec::get_global();
        let bytes = codec
            .encode_binary_message(map, binary)
            .map_err(io::Error::other)?;
        self.write_bytes(&bytes)
    }

    fn write_bytes(&self, bytes: &[u8]) -> io::Result<()> {
        match &self.inner {
            WriterInner::Stdout => {
                let stdout = io::stdout();
                let mut handle = stdout.lock();
                handle.write_all(bytes)?;
                handle.flush()
            }
            WriterInner::Channel(tx) => tx
                .send(bytes.to_vec())
                .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "writer channel closed")),
        }
    }
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

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
    images: ImageRegistry,
    writer: WireWriter,
    /// Slider value tracking for SlideRelease (mirrors App.last_slide_values).
    last_slide_values: HashMap<String, f64>,
    /// None in --mock mode (no rendering).
    ui: Option<UiState>,
}

impl Session {
    fn new(dispatcher: ExtensionDispatcher, mode: Mode, writer: WireWriter) -> Self {
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
            images: ImageRegistry::new(),
            writer,
            last_slide_values: HashMap::new(),
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
            ui_state.ui_cache = UiCache::default();
        }
    }

    /// Build a temporary UserInterface from the current tree, run a
    /// closure against it, then store the resulting cache back.
    fn with_ui<R>(
        &mut self,
        f: impl FnOnce(
            &mut iced_test::runtime::UserInterface<
                '_,
                toddy_core::message::Message,
                Theme,
                iced::Renderer,
            >,
            &mut iced::Renderer,
            mouse::Cursor,
        ) -> R,
    ) -> Option<R> {
        let ui_state = self.ui.as_mut()?;
        let root = self.core.tree.root()?;

        toddy_core::widgets::ensure_caches(root, &mut self.core.caches);
        let ctx = RenderCtx {
            caches: &self.core.caches,
            images: &self.images,
            theme: &self.theme,
            extensions: &self.dispatcher,
            default_text_size: self.core.default_text_size,
            default_font: self.core.default_font,
        };
        let element = toddy_core::widgets::render(root, ctx);

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
    fn settle_ui(&mut self) {
        if self.ui.is_some() {
            self.with_ui(|ui, renderer, cursor| {
                let mut messages = Vec::new();
                let redraw = Event::Window(iced::window::Event::RedrawRequested(
                    iced_test::core::time::Instant::now(),
                ));
                let _status = ui.update(&[redraw], cursor, renderer, &mut messages);
            });
        }
    }

    /// Inject iced events one at a time, capturing the Messages that
    /// widgets produce. Settles the UI between each event so widget
    /// state updates are visible to subsequent events.
    ///
    /// For each iced event that produces widget Messages:
    /// 1. The event is injected and Messages are captured
    /// 2. Messages are converted to OutgoingEvents (with session set)
    /// 3. An `interact_step` is emitted with those events
    /// 4. The `read_next` callback is called, which blocks until the
    ///    host sends back a tree update (Snapshot or Patch)
    /// 5. The tree update is applied, caches/extensions prepared, UI settled
    ///
    /// This matches the production flow where each iced event triggers
    /// a full host round-trip before the next event is processed.
    ///
    /// For events that produce no widget Messages (e.g. CursorMoved),
    /// no step is emitted and the loop continues immediately.
    ///
    /// Returns `true` if any events were captured and emitted via
    /// interact_step, `false` if no widget Messages were produced.
    /// Only available in headless mode (returns false in mock mode).
    fn inject_and_capture(
        &mut self,
        session_id: &str,
        interact_id: &str,
        events: &[Event],
        read_next: &mut dyn FnMut() -> Option<IncomingMessage>,
    ) -> bool {
        if self.ui.is_none() || events.is_empty() {
            return false;
        }

        let mut emitted_steps = false;

        for event in events {
            // Update cursor from CursorMoved events before injection.
            if let Event::Mouse(mouse::Event::CursorMoved { position }) = event
                && let Some(ui_state) = &mut self.ui
            {
                ui_state.cursor = mouse::Cursor::Available(*position);
            }

            // Inject ONE event and capture the Messages iced produces.
            let messages = self
                .with_ui(|ui, renderer, cursor| {
                    let mut messages = Vec::new();
                    let _status =
                        ui.update(std::slice::from_ref(event), cursor, renderer, &mut messages);
                    messages
                })
                .unwrap_or_default();

            // Convert captured Messages to OutgoingEvents.
            let step_events: Vec<OutgoingEvent> = self
                .process_captured_messages(messages)
                .into_iter()
                .map(|e| e.with_session(session_id))
                .collect();

            if !step_events.is_empty() {
                emitted_steps = true;

                // Emit an interact_step so the host can process
                // these events and send back an updated tree.
                let step = toddy_core::protocol::InteractResponse {
                    message_type: "interact_step",
                    session: session_id.to_string(),
                    id: interact_id.to_string(),
                    events: step_events,
                };
                if self.writer.emit(&step).is_err() {
                    break;
                }

                // Read the next message from the host. In the normal
                // flow this is a Snapshot or Patch with the updated
                // tree. We apply whatever arrives through the normal
                // path so tree changes, settings updates, etc. all work.
                if let Some(msg) = read_next() {
                    let is_tree_change = matches!(
                        msg,
                        IncomingMessage::Snapshot { .. } | IncomingMessage::Patch { .. }
                    );
                    if !is_tree_change {
                        let msg_type = match &msg {
                            IncomingMessage::Snapshot { .. } => "snapshot",
                            IncomingMessage::Patch { .. } => "patch",
                            IncomingMessage::Query { .. } => "query",
                            IncomingMessage::Interact { .. } => "interact",
                            IncomingMessage::Reset { .. } => "reset",
                            IncomingMessage::Settings { .. } => "settings",
                            IncomingMessage::Effect { .. } => "effect",
                            IncomingMessage::WidgetOp { .. } => "widget_op",
                            IncomingMessage::WindowOp { .. } => "window_op",
                            IncomingMessage::ImageOp { .. } => "image_op",
                            IncomingMessage::Subscribe { .. } => "subscribe",
                            IncomingMessage::Unsubscribe { .. } => "unsubscribe",
                            IncomingMessage::TreeHash { .. } => "tree_hash",
                            IncomingMessage::Screenshot { .. } => "screenshot",
                            IncomingMessage::ExtensionCommand { .. } => "extension_command",
                            IncomingMessage::ExtensionCommands { .. } => "extension_commands",
                            IncomingMessage::AdvanceFrame { .. } => "advance_frame",
                        };
                        log::warn!(
                            "interact_step: expected snapshot or patch from host, \
                             got {msg_type}; tree state may be stale"
                        );
                    }
                    let effects = self.core.apply(msg);
                    for effect in effects {
                        use toddy_core::engine::CoreEffect;
                        match effect {
                            CoreEffect::ThemeChanged(t) => self.theme = t,
                            CoreEffect::ExtensionConfig(config) => {
                                self.dispatcher.init_all(&config);
                            }
                            _ => {}
                        }
                    }
                    if is_tree_change && let Some(root) = self.core.tree.root() {
                        self.dispatcher.prepare_all(
                            root,
                            &mut self.core.caches.extension,
                            &self.theme,
                        );
                    }
                }
            }

            // Settle the UI so widget state updates before the
            // next event is processed.
            self.settle_ui();
        }

        emitted_steps
    }

    /// Convert iced Messages to OutgoingEvents using the same
    /// conversion logic as the daemon's `update()` method.
    ///
    /// Handles all Message variants that produce user-visible events:
    /// - Simple widget events via `message_to_event()` (click, input, etc.)
    /// - Slider with value tracking (SlideRelease needs the last value)
    /// - TextEditorAction with editor content mutation
    /// - Extension events via dispatcher routing
    /// - Pane grid events
    fn process_captured_messages(&mut self, messages: Vec<Message>) -> Vec<OutgoingEvent> {
        let mut events = Vec::new();

        for msg in messages {
            match msg {
                // Simple widget events -- stateless conversion.
                ref m @ (Message::Click(_)
                | Message::Input(..)
                | Message::Submit(..)
                | Message::Toggle(..)
                | Message::Select(..)
                | Message::Paste(..)
                | Message::OptionHovered(..)
                | Message::SensorResize(..)
                | Message::ScrollEvent(..)
                | Message::MouseAreaEvent(..)
                | Message::MouseAreaMove(..)
                | Message::MouseAreaScroll(..)
                | Message::CanvasEvent { .. }
                | Message::CanvasScroll { .. }) => {
                    if let Some(event) = message_to_event(m) {
                        events.push(event);
                    }
                }

                // Slider -- needs value tracking for SlideRelease.
                Message::Slide(ref id, value) => {
                    self.last_slide_values.insert(id.clone(), value);
                    events.push(OutgoingEvent::slide(id.clone(), value));
                }
                Message::SlideRelease(ref id) => {
                    let value = self.last_slide_values.remove(id).unwrap_or(0.0);
                    events.push(OutgoingEvent::slide_release(id.clone(), value));
                }

                // Text editor -- apply action to editor content, emit new text.
                Message::TextEditorAction(ref id, ref action) => {
                    if action.is_edit()
                        && let Some(content) = self.core.caches.editor_content_mut(id)
                    {
                        content.perform(action.clone());
                        let new_text = content.text();
                        events.push(OutgoingEvent::input(id.clone(), new_text));
                    }
                }

                // Extension events -- route through dispatcher.
                Message::Event {
                    ref id,
                    ref data,
                    ref family,
                } => {
                    let result = self.dispatcher.handle_event(
                        id,
                        family,
                        data,
                        &mut self.core.caches.extension,
                    );
                    match result {
                        EventResult::PassThrough => {
                            let data_opt = if data.is_null() {
                                None
                            } else {
                                Some(data.clone())
                            };
                            events.push(OutgoingEvent::generic(
                                family.clone(),
                                id.clone(),
                                data_opt,
                            ));
                        }
                        EventResult::Consumed(ext_events) => {
                            events.extend(ext_events);
                        }
                        EventResult::Observed(ext_events) => {
                            let data_opt = if data.is_null() {
                                None
                            } else {
                                Some(data.clone())
                            };
                            events.push(OutgoingEvent::generic(
                                family.clone(),
                                id.clone(),
                                data_opt,
                            ));
                            events.extend(ext_events);
                        }
                    }
                }

                // Pane grid events -- need pane state lookup.
                Message::PaneFocusCycle(ref grid_id, pane) => {
                    if let Some(state) = self.core.caches.pane_grid_state(grid_id) {
                        let pane_id = state.get(pane).cloned().unwrap_or_default();
                        events.push(OutgoingEvent::pane_focus_cycle(grid_id.clone(), pane_id));
                    }
                }
                Message::PaneResized(ref grid_id, ref evt) => {
                    if let Some(state) = self.core.caches.pane_grid_state_mut(grid_id) {
                        state.resize(evt.split, evt.ratio);
                    }
                    events.push(OutgoingEvent::pane_resized(
                        grid_id.clone(),
                        format!("{:?}", evt.split),
                        evt.ratio,
                    ));
                }
                Message::PaneDragged(ref grid_id, ref evt) => {
                    use iced::widget::pane_grid;
                    match evt {
                        pane_grid::DragEvent::Picked { pane } => {
                            if let Some(state) = self.core.caches.pane_grid_state(grid_id) {
                                let pane_id = state.get(*pane).cloned().unwrap_or_default();
                                events.push(OutgoingEvent::pane_dragged(
                                    grid_id.clone(),
                                    "picked",
                                    pane_id,
                                    None,
                                    None,
                                    None,
                                ));
                            }
                        }
                        pane_grid::DragEvent::Dropped { pane, target } => {
                            if let Some(state) = self.core.caches.pane_grid_state_mut(grid_id) {
                                let pane_id = state.get(*pane).cloned().unwrap_or_default();
                                let (target_pane, region, edge) = match target {
                                    pane_grid::Target::Edge(e) => {
                                        let edge_str = match e {
                                            pane_grid::Edge::Top => "top",
                                            pane_grid::Edge::Bottom => "bottom",
                                            pane_grid::Edge::Left => "left",
                                            pane_grid::Edge::Right => "right",
                                        };
                                        (None, None, Some(edge_str))
                                    }
                                    pane_grid::Target::Pane(p, region) => {
                                        let target_id = state.get(*p).cloned().unwrap_or_default();
                                        let region_str = match region {
                                            pane_grid::Region::Center => "center",
                                            pane_grid::Region::Edge(pane_grid::Edge::Top) => "top",
                                            pane_grid::Region::Edge(pane_grid::Edge::Bottom) => {
                                                "bottom"
                                            }
                                            pane_grid::Region::Edge(pane_grid::Edge::Left) => {
                                                "left"
                                            }
                                            pane_grid::Region::Edge(pane_grid::Edge::Right) => {
                                                "right"
                                            }
                                        };
                                        (Some(target_id), Some(region_str), None)
                                    }
                                };
                                state.drop(*pane, *target);
                                events.push(OutgoingEvent::pane_dragged(
                                    grid_id.clone(),
                                    "dropped",
                                    pane_id,
                                    target_pane,
                                    region,
                                    edge,
                                ));
                            }
                        }
                        pane_grid::DragEvent::Canceled { pane } => {
                            if let Some(state) = self.core.caches.pane_grid_state(grid_id) {
                                let pane_id = state.get(*pane).cloned().unwrap_or_default();
                                events.push(OutgoingEvent::pane_dragged(
                                    grid_id.clone(),
                                    "canceled",
                                    pane_id,
                                    None,
                                    None,
                                    None,
                                ));
                            }
                        }
                    }
                }
                Message::PaneClicked(ref grid_id, pane) => {
                    if let Some(state) = self.core.caches.pane_grid_state(grid_id) {
                        let pane_id = state.get(pane).cloned().unwrap_or_default();
                        events.push(OutgoingEvent::pane_clicked(grid_id.clone(), pane_id));
                    }
                }

                // Internal messages and subscription events -- skip.
                // Subscription events (KeyPressed, CursorMoved, etc.) don't
                // appear from ui.update() in the interact path.
                _ => {}
            }
        }

        events
    }
}

// ---------------------------------------------------------------------------
// Message handling
// ---------------------------------------------------------------------------

/// Process one incoming message through a session.
///
/// All output goes through `session.writer`. The `session_id` is
/// echoed on every outgoing message to identify which session
/// produced it.
///
/// The `read_next` callback is used during iterative interact
/// processing to read snapshot messages from the host between
/// event injections. In single-session mode, it reads from stdin.
/// In multiplexed mode, it reads from the session's mpsc channel.
/// Returns `None` if the source is closed.
fn handle_message(
    s: &mut Session,
    session_id: &str,
    msg: IncomingMessage,
    read_next: &mut dyn FnMut() -> Option<IncomingMessage>,
) -> io::Result<()> {
    let is_snapshot = matches!(msg, IncomingMessage::Snapshot { .. });
    let is_tree_change = is_snapshot || matches!(msg, IncomingMessage::Patch { .. });
    let is_settings = matches!(msg, IncomingMessage::Settings { .. });

    match msg {
        IncomingMessage::Snapshot { .. }
        | IncomingMessage::Patch { .. }
        | IncomingMessage::Effect { .. }
        | IncomingMessage::WidgetOp { .. }
        | IncomingMessage::Subscribe { .. }
        | IncomingMessage::Unsubscribe { .. }
        | IncomingMessage::WindowOp { .. }
        | IncomingMessage::Settings { .. }
        | IncomingMessage::ImageOp { .. } => {
            let effects = s.core.apply(msg);

            for effect in effects {
                use toddy_core::engine::CoreEffect;
                match effect {
                    CoreEffect::EmitEvent(event) => {
                        s.writer.emit(&event.with_session(session_id))?;
                    }
                    CoreEffect::HandleEffect {
                        request_id,
                        kind,
                        payload,
                    } => {
                        if crate::effects::is_async_effect(&kind) {
                            let mode = if s.ui.is_some() { "headless" } else { "mock" };
                            log::debug!(
                                "{mode}: async effect {kind} returning cancelled \
                                 (no display)"
                            );
                            s.writer.emit(
                                &toddy_core::protocol::EffectResponse::cancelled(request_id)
                                    .with_session(session_id),
                            )?;
                        } else {
                            let response =
                                crate::effects::handle_effect(request_id, &kind, &payload);
                            s.writer.emit(&response.with_session(session_id))?;
                        }
                    }
                    CoreEffect::ThemeChanged(t) => {
                        let mode_str = if t == iced::Theme::Light {
                            "light"
                        } else {
                            "dark"
                        };
                        // Emit theme_changed subscription event if active,
                        // matching windowed mode behavior.
                        if let Some(tag) = s
                            .core
                            .active_subscriptions
                            .get(crate::renderer::constants::SUB_THEME_CHANGE)
                        {
                            let _ = s.writer.emit(
                                &toddy_core::protocol::OutgoingEvent::theme_changed(
                                    tag.clone(),
                                    mode_str.to_string(),
                                )
                                .with_session(session_id),
                            );
                        }
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
                    CoreEffect::SyncWindows => {}
                    CoreEffect::WidgetOp { .. } => {}
                    CoreEffect::WindowOp { .. } => {}
                    CoreEffect::ThemeFollowsSystem => {}
                }
            }

            if is_settings {
                s.rebuild_renderer();
            }
            if is_tree_change {
                if is_snapshot {
                    s.dispatcher.clear_poisoned();
                }
                if let Some(root) = s.core.tree.root() {
                    s.dispatcher
                        .prepare_all(root, &mut s.core.caches.extension, &s.theme);
                }
                s.settle_ui();
            }
        }

        IncomingMessage::Query {
            id,
            target,
            selector,
        } => {
            let resp = crate::scripting::build_query_response(&s.core, id, target, selector)
                .with_session(session_id);
            s.writer.emit(&resp)?;
        }
        IncomingMessage::Interact {
            id,
            action,
            selector,
            payload,
        } => {
            let widget_id = resolve_widget_id(&s.core, &selector);
            let cursor =
                s.ui.as_ref()
                    .map(|u| u.cursor)
                    .unwrap_or(mouse::Cursor::Unavailable);
            let iced_events =
                interaction_to_iced_events(&action, widget_id.as_deref(), &payload, cursor);

            let events = if s.ui.is_some() && !iced_events.is_empty() {
                // Headless mode: inject real iced events one at a time
                // with host round-trips between events that produce
                // widget Messages. Events are delivered to the host
                // via interact_step messages during injection.
                let had_steps = s.inject_and_capture(session_id, &id, &iced_events, read_next);

                if had_steps {
                    // Events were already delivered via interact_step.
                    // Final response is a completion signal with no
                    // events to avoid double-dispatch.
                    vec![]
                } else {
                    // No events captured -- action has no iced
                    // equivalent (paste, sort, canvas, slide, etc.).
                    // Fall back to synthetic events in the final
                    // response (no steps were emitted).
                    crate::scripting::build_interact_response(
                        &s.core,
                        id.clone(),
                        action,
                        selector,
                        payload,
                    )
                    .events
                }
            } else {
                // Mock mode (no UI) or action with no iced events:
                // use synthetic event construction.
                crate::scripting::build_interact_response(
                    &s.core,
                    id.clone(),
                    action,
                    selector,
                    payload,
                )
                .events
            };

            let resp =
                toddy_core::protocol::InteractResponse::new(id, events).with_session(session_id);
            s.writer.emit(&resp)?;
        }
        IncomingMessage::TreeHash { id, name, .. } => {
            let resp = crate::scripting::build_tree_hash_response(&s.core, id, name)
                .with_session(session_id);
            s.writer.emit(&resp)?;
        }
        IncomingMessage::Screenshot {
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
            handle_screenshot(s, session_id, id, name, w, h)?;
        }
        IncomingMessage::Reset { id } => {
            s.dispatcher.reset(&mut s.core.caches.extension);
            s.images = ImageRegistry::new();
            s.theme = Theme::Dark;
            s.last_slide_values.clear();
            if let Some(ui_state) = &mut s.ui {
                ui_state.ui_cache = UiCache::default();
                ui_state.cursor = mouse::Cursor::Unavailable;
            }
            s.rebuild_renderer();
            let resp =
                crate::scripting::build_reset_response(&mut s.core, id).with_session(session_id);
            s.writer.emit(&resp)?;
        }
        IncomingMessage::ExtensionCommand {
            node_id,
            op,
            payload,
        } => {
            let events =
                s.dispatcher
                    .handle_command(&node_id, &op, &payload, &mut s.core.caches.extension);
            for event in events {
                s.writer.emit(&event.with_session(session_id))?;
            }
        }
        IncomingMessage::ExtensionCommands { commands } => {
            for cmd in commands {
                let events = s.dispatcher.handle_command(
                    &cmd.node_id,
                    &cmd.op,
                    &cmd.payload,
                    &mut s.core.caches.extension,
                );
                for event in events {
                    s.writer.emit(&event.with_session(session_id))?;
                }
            }
        }
        IncomingMessage::AdvanceFrame { timestamp } => {
            if let Some(tag) = s
                .core
                .active_subscriptions
                .get(crate::renderer::constants::SUB_ANIMATION_FRAME)
            {
                s.writer.emit(
                    &toddy_core::protocol::OutgoingEvent::animation_frame(
                        tag.clone(),
                        timestamp as u128,
                    )
                    .with_session(session_id),
                )?;
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Screenshot capture
// ---------------------------------------------------------------------------

fn handle_screenshot(
    s: &mut Session,
    session_id: &str,
    id: String,
    name: String,
    width: u32,
    height: u32,
) -> io::Result<()> {
    let emit_stub = |s: &Session| {
        let map = screenshot_map(session_id, &id, &name, "", 0, 0);
        s.writer.emit_binary(map, None)
    };

    if s.ui.is_none() {
        return emit_stub(s);
    }

    use iced_test::core::theme::Base;
    use sha2::{Digest, Sha256};

    let ui_state = s.ui.as_mut().unwrap();
    ui_state.viewport_size = Size::new(width as f32, height as f32);

    let root = match s.core.tree.root() {
        Some(r) => r,
        None => return emit_stub(s),
    };

    toddy_core::widgets::ensure_caches(root, &mut s.core.caches);
    let ctx = RenderCtx {
        caches: &s.core.caches,
        images: &s.images,
        theme: &s.theme,
        extensions: &s.dispatcher,
        default_text_size: s.core.default_text_size,
        default_font: s.core.default_font,
    };
    let element: iced::Element<'_, toddy_core::message::Message> =
        toddy_core::widgets::render(root, ctx);

    let cache = std::mem::take(&mut ui_state.ui_cache);
    let mut ui = iced_test::runtime::UserInterface::build(
        element,
        ui_state.viewport_size,
        cache,
        &mut ui_state.renderer,
    );

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

    let map = screenshot_map(session_id, &id, &name, &hash, width, height);
    let binary = if rgba.is_empty() {
        None
    } else {
        Some(("rgba", rgba.as_slice()))
    };
    s.writer.emit_binary(map, binary)
}

/// Build the JSON map for a screenshot_response message.
fn screenshot_map(
    session: &str,
    id: &str,
    name: &str,
    hash: &str,
    width: u32,
    height: u32,
) -> serde_json::Map<String, serde_json::Value> {
    use serde_json::json;
    let mut map = serde_json::Map::new();
    map.insert("type".to_string(), json!("screenshot_response"));
    map.insert("session".to_string(), json!(session));
    map.insert("id".to_string(), json!(id));
    map.insert("name".to_string(), json!(name));
    map.insert("hash".to_string(), json!(hash));
    map.insert("width".to_string(), json!(width));
    map.insert("height".to_string(), json!(height));
    map
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

/// Run the headless/mock event loop.
///
/// When `max_sessions` is 1, runs a single session on the current
/// thread (same as the original design). When > 1, spawns reader,
/// writer, and per-session threads for concurrent multiplexing.
pub(crate) fn run(
    forced_codec: Option<Codec>,
    dispatcher: ExtensionDispatcher,
    mode: Mode,
    max_sessions: usize,
) {
    let stdin = io::stdin();
    let mut reader = io::BufReader::new(stdin.lock());

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

    let mode_str = match mode {
        Mode::Headless => "headless",
        Mode::Mock => "mock",
    };
    if let Err(e) = crate::renderer::emit_hello(mode_str) {
        log::error!("failed to emit hello: {e}");
        return;
    }

    if max_sessions <= 1 {
        run_single(codec, dispatcher, mode, &mut reader);
    } else {
        run_multiplexed(codec, dispatcher, mode, max_sessions, &mut reader);
    }

    log::info!("stdin closed, exiting");
}

/// Read and decode the next message from a BufRead source.
fn read_message(codec: Codec, reader: &mut impl BufRead) -> Option<SessionMessage> {
    loop {
        match codec.read_message(reader) {
            Ok(None) => return None,
            Ok(Some(bytes)) => {
                let value: serde_json::Value = match codec.decode(&bytes) {
                    Ok(v) => v,
                    Err(e) => {
                        log::error!("decode error: {e}");
                        continue;
                    }
                };
                match SessionMessage::from_value(value) {
                    Ok(sm) => return Some(sm),
                    Err(e) => {
                        log::error!("decode error: {e}");
                        continue;
                    }
                }
            }
            Err(e) => {
                log::error!("read error: {e}");
                return None;
            }
        }
    }
}

/// Single-session event loop (max_sessions=1). Behaves like the
/// original design: one session, direct stdout writes.
fn run_single(
    codec: Codec,
    dispatcher: ExtensionDispatcher,
    mode: Mode,
    reader: &mut impl BufRead,
) {
    let mut session = Session::new(dispatcher, mode, WireWriter::stdout());

    while let Some(sm) = read_message(codec, reader) {
        // Provide a callback that reads the next message from stdin.
        // Used by inject_and_capture during iterative interact to
        // wait for the host's snapshot between events.
        let mut read_next = || read_message(codec, reader).map(|sm| sm.message);

        if let Err(e) = handle_message(&mut session, &sm.session, sm.message, &mut read_next) {
            log::error!("write error: {e}");
            break;
        }
    }
}

/// Multiplexed event loop (max_sessions > 1). Reader thread dispatches
/// to per-session threads. Writer thread serializes output to stdout.
fn run_multiplexed(
    codec: Codec,
    template: ExtensionDispatcher,
    mode: Mode,
    max_sessions: usize,
    reader: &mut impl BufRead,
) {
    use std::collections::HashMap;

    // Writer thread: drains the channel and writes to stdout.
    let (writer_tx, writer_rx) = mpsc::channel::<Vec<u8>>();
    let writer_handle = thread::spawn(move || {
        let stdout = io::stdout();
        let mut handle = stdout.lock();
        for bytes in writer_rx {
            if handle.write_all(&bytes).is_err() || handle.flush().is_err() {
                break;
            }
        }
    });

    // Session dispatch table: session_id -> sender to that session's thread.
    let mut sessions: HashMap<String, mpsc::Sender<IncomingMessage>> = HashMap::new();
    let mut session_handles: Vec<thread::JoinHandle<()>> = Vec::new();

    loop {
        match codec.read_message(reader) {
            Ok(None) => break,
            Ok(Some(bytes)) => {
                let value: serde_json::Value = match codec.decode(&bytes) {
                    Ok(v) => v,
                    Err(e) => {
                        log::error!("decode error: {e}");
                        continue;
                    }
                };
                let sm = match SessionMessage::from_value(value) {
                    Ok(sm) => sm,
                    Err(e) => {
                        log::error!("decode error: {e}");
                        continue;
                    }
                };

                let session_id = sm.session.clone();

                // Check if this is a Reset -- if so, tear down the session.
                let is_reset = matches!(sm.message, IncomingMessage::Reset { .. });

                // Get or create the session thread.
                let tx = if let Some(tx) = sessions.get(&session_id) {
                    tx.clone()
                } else {
                    if sessions.len() >= max_sessions {
                        log::error!(
                            "max sessions ({max_sessions}) reached; \
                             dropping message for session '{session_id}'"
                        );
                        continue;
                    }

                    let (tx, rx) = mpsc::channel::<IncomingMessage>();
                    let dispatcher = template.clone_for_session();
                    let writer = WireWriter::channel(writer_tx.clone());
                    let sid = session_id.clone();

                    let handle = thread::spawn(move || {
                        let mut session = Session::new(dispatcher, mode, writer);
                        for msg in &rx {
                            let mut read_next = || rx.recv().ok();
                            if let Err(e) = handle_message(&mut session, &sid, msg, &mut read_next)
                            {
                                log::error!("session '{sid}': write error: {e}");
                                break;
                            }
                        }
                        log::debug!("session '{sid}' thread exiting");
                    });

                    sessions.insert(session_id.clone(), tx.clone());
                    session_handles.push(handle);
                    tx
                };

                // Send the message to the session thread.
                if tx.send(sm.message).is_err() {
                    log::error!("session '{session_id}' channel closed unexpectedly");
                    sessions.remove(&session_id);
                    continue;
                }

                // If this was a Reset, tear down the session after it processes.
                if is_reset {
                    // Drop the sender so the session thread exits after
                    // processing the Reset message.
                    sessions.remove(&session_id);
                }
            }
            Err(e) => {
                log::error!("read error: {e}");
                break;
            }
        }
    }

    // Drop all session senders so threads exit.
    sessions.clear();
    // Drop the writer sender so the writer thread exits.
    drop(writer_tx);

    for handle in session_handles {
        let _ = handle.join();
    }
    let _ = writer_handle.join();
}
