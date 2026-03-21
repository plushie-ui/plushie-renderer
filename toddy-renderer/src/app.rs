//! Application struct and core utility methods.
//!
//! Defines the [`App`] struct (the iced daemon's state) and the methods
//! that the rest of the renderer uses to query window titles, themes,
//! scale factors, and emit subscription events.

use std::collections::HashMap;

use iced::{Task, Theme, window};

use toddy_core::extensions::ExtensionDispatcher;
use toddy_core::message::Message;
use toddy_core::protocol::OutgoingEvent;

use crate::constants::*;
use crate::effects::EffectHandler;
use crate::emitter::{CoalesceKey, EventEmitter};
use crate::emitters;
use crate::window_map;

/// Validate and clamp a scale factor. Returns 1.0 for invalid values
/// (zero, negative, NaN, infinity).
pub fn validate_scale_factor(sf: f32) -> f32 {
    if sf <= 0.0 || !sf.is_finite() {
        log::warn!("invalid scale_factor {sf}, using 1.0");
        1.0
    } else {
        sf
    }
}

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

/// The iced daemon application. Owns the rendering engine, window
/// state, extension dispatcher, and all runtime state needed to
/// translate between the wire protocol and iced's update/view cycle.
pub struct App {
    pub core: toddy_core::engine::Core,
    pub theme: Theme,
    /// Widget ops and effects return iced Tasks, but `apply()` doesn't
    /// return them. They accumulate here and are drained via `Task::batch`
    /// in `update()` after `apply()` returns.
    pub pending_tasks: Vec<Task<Message>>,
    /// Bidirectional toddy ID <-> iced window ID mapping with per-window state.
    pub windows: window_map::WindowMap,
    /// In-memory image handles for use by Image widgets and canvas draw.
    pub image_registry: toddy_core::image_registry::ImageRegistry,
    /// Current system theme, tracked via ThemeChanged subscription.
    pub system_theme: Theme,
    /// True when the app-level theme is "system" (follow OS preference).
    pub theme_follows_system: bool,
    /// Global scale factor multiplier (1.0 = follow OS DPI).
    pub scale_factor: f32,
    /// Last slider value per widget ID, for correct on_release events.
    pub last_slide_values: HashMap<String, f64>,
    /// Extension dispatcher for custom widget types.
    pub dispatcher: ExtensionDispatcher,
    /// Epoch for animation_frame timestamp calculation.
    pub animation_epoch: Option<iced::time::Instant>,
    /// Rate-limited event emitter with coalescing.
    pub emitter: EventEmitter,
    /// Platform-specific effect handler injected at construction.
    /// Native and WASM crates each provide their own [`EffectHandler`]
    /// implementation.
    pub effect_handler: Box<dyn EffectHandler>,
}

impl App {
    pub fn new(dispatcher: ExtensionDispatcher, effect_handler: Box<dyn EffectHandler>) -> Self {
        Self {
            core: toddy_core::engine::Core::new(),
            theme: DEFAULT_THEME,
            pending_tasks: Vec::new(),
            windows: window_map::WindowMap::new(),
            image_registry: toddy_core::image_registry::ImageRegistry::new(),
            system_theme: DEFAULT_THEME,
            theme_follows_system: false,
            scale_factor: 1.0,
            last_slide_values: HashMap::new(),
            dispatcher,
            animation_epoch: None,
            emitter: EventEmitter::new(),
            effect_handler,
        }
    }

    pub fn title_for_window(&self, window_id: window::Id) -> String {
        if let Some(toddy_id) = self.windows.get_toddy(&window_id)
            && let Some(node) = self.core.tree.find_window(toddy_id)
            && let Some(title) = node.props.get("title").and_then(|v| v.as_str())
        {
            return title.chars().filter(|c| !c.is_control()).collect();
        }
        DEFAULT_WINDOW_TITLE.to_string()
    }

    pub fn theme_for_window(&self, window_id: window::Id) -> Theme {
        self.theme_ref_for_window(window_id).clone()
    }

    pub fn theme_ref_for_window(&self, window_id: window::Id) -> &Theme {
        if let Some(toddy_id) = self.windows.get_toddy(&window_id)
            && let Some(cached) = self.windows.cached_theme(toddy_id)
        {
            return cached;
        }
        if self.theme_follows_system {
            &self.system_theme
        } else {
            &self.theme
        }
    }

    pub fn scale_factor_for_window(&self, window_id: window::Id) -> f32 {
        let sf = self
            .windows
            .get_toddy(&window_id)
            .and_then(|jid| self.core.tree.find_window(jid))
            .and_then(|node| node.props.get("scale_factor"))
            .and_then(|v| v.as_f64())
            .map(|v| v as f32)
            .unwrap_or(self.scale_factor);
        validate_scale_factor(sf)
    }

    pub fn emit_subscription(
        &self,
        key: &str,
        captured: bool,
        event_fn: impl FnOnce(String) -> OutgoingEvent,
    ) -> Task<Message> {
        let tag = self
            .core
            .active_subscriptions
            .get(key)
            .or_else(|| self.core.active_subscriptions.get(SUB_EVENT));
        if let Some(tag) = tag {
            emitters::emit_or_exit(event_fn(tag.clone()).with_captured(captured))
        } else {
            Task::none()
        }
    }

    pub fn lookup_widget_event_rate(&self, widget_id: &str) -> Option<u32> {
        let node = self.core.tree.find_by_id(widget_id)?;
        node.props
            .get("event_rate")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
    }

    pub fn coalesce_subscription(
        &mut self,
        key: &str,
        captured: bool,
        event_fn: impl FnOnce(String) -> OutgoingEvent,
    ) -> Task<Message> {
        let tag = if let Some(tag) = self.core.active_subscriptions.get(key) {
            tag.clone()
        } else if let Some(tag) = self.core.active_subscriptions.get(SUB_EVENT) {
            tag.clone()
        } else {
            return Task::none();
        };
        let event = event_fn(tag).with_captured(captured);
        self.emitter
            .coalesce(CoalesceKey::Subscription(key.to_string()), event)
    }
}
