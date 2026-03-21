//! Outgoing wire messages: events and response types.
//!
//! [`OutgoingEvent`] is the main event struct emitted by the renderer.
//! Response types ([`EffectResponse`], [`QueryResponse`], etc.) are
//! serialized in reply to incoming messages.

use serde::Serialize;
use serde_json::Value;

/// Hint for the renderer's event coalescing system.
///
/// Set by event constructors or extension authors via
/// [`OutgoingEvent::with_coalesce`]. The renderer uses this to decide
/// whether and how to buffer events during rate-limited delivery. Not
/// serialized to the wire -- renderer-internal metadata.
///
/// # For extension authors
///
/// Set on events returned from `handle_event()`:
///
/// ```ignore
/// let event = OutgoingEvent::extension_event("cursor_pos", node_id, data)
///     .with_coalesce(CoalesceHint::Replace);
/// ```
///
/// Events without a hint are always delivered immediately (never
/// rate-limited), regardless of `event_rate` or `default_event_rate`.
#[derive(Debug, Clone, PartialEq)]
pub enum CoalesceHint {
    /// Keep the latest event, discard intermediates.
    /// Use for: position reports, state snapshots, progress values --
    /// anything where only the most recent value matters.
    Replace,
    /// Sum the named `data` fields across coalesced events.
    /// Other fields keep the latest event's values.
    /// Use for: scroll deltas, velocity changes, counters -- anything
    /// where intermediate values carry magnitude that would be lost
    /// if only the latest were kept.
    Accumulate(Vec<String>),
}

/// An event written to stdout by the renderer.
///
/// All events share a flat struct with optional fields. There are two
/// constructor patterns:
///
/// - **Widget events** (click, input, toggle, etc.) use `id` to identify
///   the source widget. Built via the internal `bare()` constructor.
/// - **Subscription events** (key_press, cursor_moved, window_opened,
///   etc.) use `tag` to identify the subscription that requested them.
///   Built via the internal `tagged()` constructor. The `id` field is empty.
///
/// Extension authors emit custom events via
/// [`extension_event`](Self::extension_event).
#[derive(Debug, Serialize)]
pub struct OutgoingEvent {
    /// Always `"event"`.
    #[serde(rename = "type")]
    pub message_type: &'static str,
    /// Session that produced this event.
    pub session: String,
    /// Event type (e.g. `"click"`, `"key_press"`, `"window_opened"`).
    pub family: String,
    /// Source widget node ID (widget events) or empty (subscription events).
    pub id: String,
    /// Primary value payload (e.g. input text, slider value, selected option).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    /// Subscription tag identifying which subscription requested this event.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// Keyboard modifier state at the time of the event.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modifiers: Option<KeyModifiers>,
    /// Flexible extra data for events that carry additional fields beyond
    /// the standard id/value/tag/modifiers shape.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    /// Whether the event was captured (consumed) by an iced widget before
    /// reaching the subscription listener. Present on keyboard, mouse,
    /// touch, and IME events; absent on widget-level events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub captured: Option<bool>,
    /// Coalescing hint for rate-limited delivery.
    /// Not serialized to the wire -- renderer-internal metadata.
    #[serde(skip)]
    pub coalesce: Option<CoalesceHint>,
}

impl OutgoingEvent {
    /// Mark the event with its capture status.
    pub fn with_captured(mut self, captured: bool) -> Self {
        self.captured = Some(captured);
        self
    }

    /// Set the session ID for this event.
    pub fn with_session(mut self, session: impl Into<String>) -> Self {
        self.session = session.into();
        self
    }

    /// Declare that this event can be coalesced during rate-limited
    /// delivery. Without this hint, the event is always delivered
    /// immediately regardless of rate settings.
    pub fn with_coalesce(mut self, hint: CoalesceHint) -> Self {
        self.coalesce = Some(hint);
        self
    }

    /// Set the primary `value` field on this event.
    ///
    /// For built-in widget events, `value` carries the widget's primary
    /// datum (input text, slider position, selected option). Extension
    /// authors wrapping built-in widgets can use this to emit events
    /// compatible with the built-in shape:
    ///
    /// ```ignore
    /// OutgoingEvent::extension_event("input", id, data)
    ///     .with_value(serde_json::Value::String(text))
    /// ```
    pub fn with_value(mut self, value: Value) -> Self {
        self.value = Some(value);
        self
    }
}

/// Serializable representation of keyboard modifiers.
///
/// All fields default to `false`, so partial JSON like `{"shift": true}`
/// deserializes correctly with unset modifiers left as `false`.
#[derive(Debug, Clone, Default, Serialize, serde::Deserialize)]
pub struct KeyModifiers {
    #[serde(default)]
    pub shift: bool,
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub alt: bool,
    #[serde(default)]
    pub logo: bool,
    #[serde(default)]
    pub command: bool,
}

// ---------------------------------------------------------------------------
// Widget events (click, input, toggle, slide, select, submit)
// ---------------------------------------------------------------------------

impl OutgoingEvent {
    /// Helper to build a bare event with only the common fields.
    fn bare(family: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            message_type: "event",
            session: String::new(),
            family: family.into(),
            id: id.into(),
            value: None,
            tag: None,
            modifiers: None,
            data: None,
            captured: None,
            coalesce: None,
        }
    }

    /// Helper to build a subscription-tagged event with no widget id.
    fn tagged(family: impl Into<String>, tag: String) -> Self {
        Self {
            message_type: "event",
            session: String::new(),
            family: family.into(),
            id: String::new(),
            value: None,
            tag: Some(tag),
            modifiers: None,
            data: None,
            captured: None,
            coalesce: None,
        }
    }

    /// Generic widget event with a family string and optional data payload.
    /// Used for on_open, on_close, sort, and other events.
    pub fn generic(family: impl Into<String>, id: impl Into<String>, data: Option<Value>) -> Self {
        Self {
            data,
            ..Self::bare(family, id)
        }
    }

    /// Convenience constructor for extension-emitted events.
    ///
    /// Identical to [`generic`](Self::generic) -- exists for discoverability
    /// so extension authors searching docs for "extension" find it.
    pub fn extension_event(
        family: impl Into<String>,
        id: impl Into<String>,
        data: Option<Value>,
    ) -> Self {
        Self::generic(family, id, data)
    }

    pub fn click(id: String) -> Self {
        Self::bare("click", id)
    }

    pub fn input(id: String, value: String) -> Self {
        Self {
            value: Some(Value::String(value)),
            ..Self::bare("input", id)
        }
    }

    pub fn submit(id: String, value: String) -> Self {
        Self {
            value: Some(Value::String(value)),
            ..Self::bare("submit", id)
        }
    }

    pub fn toggle(id: String, checked: bool) -> Self {
        Self {
            value: Some(Value::Bool(checked)),
            ..Self::bare("toggle", id)
        }
    }

    pub fn slide(id: String, value: f64) -> Self {
        Self {
            value: Some(serde_json::json!(sanitize_f64(value))),
            coalesce: Some(CoalesceHint::Replace),
            ..Self::bare("slide", id)
        }
    }

    pub fn slide_release(id: String, value: f64) -> Self {
        Self {
            value: Some(serde_json::json!(sanitize_f64(value))),
            ..Self::bare("slide_release", id)
        }
    }

    pub fn select(id: String, value: String) -> Self {
        Self {
            value: Some(Value::String(value)),
            ..Self::bare("select", id)
        }
    }

    // -----------------------------------------------------------------------
    // Keyboard events
    // -----------------------------------------------------------------------

    pub fn key_press(tag: String, data: &crate::message::KeyEventData) -> Self {
        Self {
            modifiers: Some(crate::message::serialize_modifiers(data.modifiers)),
            data: Some(serde_json::json!({
                "key": crate::message::serialize_key(&data.key),
                "modified_key": crate::message::serialize_key(&data.modified_key),
                "physical_key": crate::message::serialize_physical_key(&data.physical_key),
                "location": crate::message::serialize_location(&data.location),
                "text": data.text.as_deref(),
                "repeat": data.repeat,
            })),
            ..Self::tagged("key_press", tag)
        }
    }

    pub fn key_release(tag: String, data: &crate::message::KeyEventData) -> Self {
        Self {
            modifiers: Some(crate::message::serialize_modifiers(data.modifiers)),
            data: Some(serde_json::json!({
                "key": crate::message::serialize_key(&data.key),
                "modified_key": crate::message::serialize_key(&data.modified_key),
                "physical_key": crate::message::serialize_physical_key(&data.physical_key),
                "location": crate::message::serialize_location(&data.location),
            })),
            ..Self::tagged("key_release", tag)
        }
    }

    pub fn modifiers_changed(tag: String, modifiers: KeyModifiers) -> Self {
        Self {
            modifiers: Some(modifiers),
            coalesce: Some(CoalesceHint::Replace),
            ..Self::tagged("modifiers_changed", tag)
        }
    }

    // -----------------------------------------------------------------------
    // Mouse events
    // -----------------------------------------------------------------------

    pub fn cursor_moved(tag: String, x: f32, y: f32) -> Self {
        Self {
            data: Some(serde_json::json!({"x": sanitize_f32(x), "y": sanitize_f32(y)})),
            coalesce: Some(CoalesceHint::Replace),
            ..Self::tagged("cursor_moved", tag)
        }
    }

    pub fn cursor_entered(tag: String) -> Self {
        Self::tagged("cursor_entered", tag)
    }

    pub fn cursor_left(tag: String) -> Self {
        Self::tagged("cursor_left", tag)
    }

    pub fn button_pressed(tag: String, button: String) -> Self {
        Self {
            value: Some(Value::String(button)),
            ..Self::tagged("button_pressed", tag)
        }
    }

    pub fn button_released(tag: String, button: String) -> Self {
        Self {
            value: Some(Value::String(button)),
            ..Self::tagged("button_released", tag)
        }
    }

    pub fn wheel_scrolled(tag: String, delta_x: f32, delta_y: f32, unit: &str) -> Self {
        Self {
            data: Some(serde_json::json!({
                "delta_x": sanitize_f32(delta_x),
                "delta_y": sanitize_f32(delta_y),
                "unit": unit,
            })),
            coalesce: Some(CoalesceHint::Accumulate(vec![
                "delta_x".into(),
                "delta_y".into(),
            ])),
            ..Self::tagged("wheel_scrolled", tag)
        }
    }

    // -----------------------------------------------------------------------
    // Touch events
    // -----------------------------------------------------------------------

    fn touch_event(family: &str, tag: String, finger_id: u64, x: f32, y: f32) -> Self {
        Self {
            data: Some(serde_json::json!({
                "id": finger_id,
                "x": sanitize_f32(x),
                "y": sanitize_f32(y),
            })),
            ..Self::tagged(family, tag)
        }
    }

    pub fn finger_pressed(tag: String, finger_id: u64, x: f32, y: f32) -> Self {
        Self::touch_event("finger_pressed", tag, finger_id, x, y)
    }

    pub fn finger_moved(tag: String, finger_id: u64, x: f32, y: f32) -> Self {
        Self {
            coalesce: Some(CoalesceHint::Replace),
            ..Self::touch_event("finger_moved", tag, finger_id, x, y)
        }
    }

    pub fn finger_lifted(tag: String, finger_id: u64, x: f32, y: f32) -> Self {
        Self::touch_event("finger_lifted", tag, finger_id, x, y)
    }

    pub fn finger_lost(tag: String, finger_id: u64, x: f32, y: f32) -> Self {
        Self::touch_event("finger_lost", tag, finger_id, x, y)
    }

    // -----------------------------------------------------------------------
    // IME events
    // -----------------------------------------------------------------------

    pub fn ime_opened(tag: String) -> Self {
        Self::tagged("ime_opened", tag)
    }

    pub fn ime_preedit(tag: String, text: String, cursor: Option<std::ops::Range<usize>>) -> Self {
        let cursor_val = cursor
            .map(|r| serde_json::json!({"start": r.start, "end": r.end}))
            .unwrap_or(serde_json::Value::Null);
        Self {
            data: Some(serde_json::json!({"text": text, "cursor": cursor_val})),
            ..Self::tagged("ime_preedit", tag)
        }
    }

    pub fn ime_commit(tag: String, text: String) -> Self {
        Self {
            data: Some(serde_json::json!({"text": text})),
            ..Self::tagged("ime_commit", tag)
        }
    }

    pub fn ime_closed(tag: String) -> Self {
        Self::tagged("ime_closed", tag)
    }

    // -----------------------------------------------------------------------
    // Window lifecycle events
    // -----------------------------------------------------------------------

    pub fn window_opened(
        tag: String,
        window_id: String,
        position: Option<(f32, f32)>,
        width: f32,
        height: f32,
        scale_factor: f32,
    ) -> Self {
        let pos =
            position.map(|(x, y)| serde_json::json!({"x": sanitize_f32(x), "y": sanitize_f32(y)}));
        Self {
            data: Some(serde_json::json!({
                "window_id": window_id,
                "position": pos,
                "width": sanitize_f32(width),
                "height": sanitize_f32(height),
                "scale_factor": sanitize_f32(scale_factor),
            })),
            ..Self::tagged("window_opened", tag)
        }
    }

    /// Window event carrying only a window_id in its data payload.
    fn window_event(family: &str, tag: String, window_id: String) -> Self {
        Self {
            data: Some(serde_json::json!({"window_id": window_id})),
            ..Self::tagged(family, tag)
        }
    }

    pub fn window_closed(tag: String, window_id: String) -> Self {
        Self::window_event("window_closed", tag, window_id)
    }

    pub fn window_close_requested(tag: String, window_id: String) -> Self {
        Self::window_event("window_close_requested", tag, window_id)
    }

    pub fn window_moved(tag: String, window_id: String, x: f32, y: f32) -> Self {
        Self {
            data: Some(serde_json::json!({
                "window_id": window_id,
                "x": sanitize_f32(x),
                "y": sanitize_f32(y),
            })),
            ..Self::tagged("window_moved", tag)
        }
    }

    pub fn window_resized(tag: String, window_id: String, width: f32, height: f32) -> Self {
        Self {
            data: Some(serde_json::json!({
                "window_id": window_id,
                "width": sanitize_f32(width),
                "height": sanitize_f32(height),
            })),
            ..Self::tagged("window_resized", tag)
        }
    }

    pub fn window_focused(tag: String, window_id: String) -> Self {
        Self::window_event("window_focused", tag, window_id)
    }

    pub fn window_unfocused(tag: String, window_id: String) -> Self {
        Self::window_event("window_unfocused", tag, window_id)
    }

    pub fn window_rescaled(tag: String, window_id: String, scale_factor: f32) -> Self {
        Self {
            data: Some(serde_json::json!({
                "window_id": window_id,
                "scale_factor": sanitize_f32(scale_factor),
            })),
            ..Self::tagged("window_rescaled", tag)
        }
    }

    pub fn file_hovered(tag: String, window_id: String, path: String) -> Self {
        Self {
            data: Some(serde_json::json!({
                "window_id": window_id,
                "path": path,
            })),
            ..Self::tagged("file_hovered", tag)
        }
    }

    pub fn file_dropped(tag: String, window_id: String, path: String) -> Self {
        Self {
            data: Some(serde_json::json!({
                "window_id": window_id,
                "path": path,
            })),
            ..Self::tagged("file_dropped", tag)
        }
    }

    pub fn files_hovered_left(tag: String, window_id: String) -> Self {
        Self::window_event("files_hovered_left", tag, window_id)
    }

    // -----------------------------------------------------------------------
    // Animation / theme / system events
    // -----------------------------------------------------------------------

    pub fn animation_frame(tag: String, timestamp_millis: u128) -> Self {
        Self {
            data: Some(serde_json::json!({"timestamp": timestamp_millis})),
            coalesce: Some(CoalesceHint::Replace),
            ..Self::tagged("animation_frame", tag)
        }
    }

    pub fn theme_changed(tag: String, mode: String) -> Self {
        Self {
            value: Some(Value::String(mode)),
            coalesce: Some(CoalesceHint::Replace),
            ..Self::tagged("theme_changed", tag)
        }
    }

    // -----------------------------------------------------------------------
    // Sensor events
    // -----------------------------------------------------------------------

    pub fn sensor_resize(id: String, width: f32, height: f32) -> Self {
        Self {
            data: Some(
                serde_json::json!({"width": sanitize_f32(width), "height": sanitize_f32(height)}),
            ),
            coalesce: Some(CoalesceHint::Replace),
            ..Self::bare("sensor_resize", id)
        }
    }

    // -----------------------------------------------------------------------
    // Canvas events
    // -----------------------------------------------------------------------

    pub fn canvas_press(id: String, x: f32, y: f32, button: String) -> Self {
        Self {
            data: Some(
                serde_json::json!({"x": sanitize_f32(x), "y": sanitize_f32(y), "button": button}),
            ),
            ..Self::bare("canvas_press", id)
        }
    }

    pub fn canvas_release(id: String, x: f32, y: f32, button: String) -> Self {
        Self {
            data: Some(
                serde_json::json!({"x": sanitize_f32(x), "y": sanitize_f32(y), "button": button}),
            ),
            ..Self::bare("canvas_release", id)
        }
    }

    pub fn canvas_move(id: String, x: f32, y: f32) -> Self {
        Self {
            data: Some(serde_json::json!({"x": sanitize_f32(x), "y": sanitize_f32(y)})),
            coalesce: Some(CoalesceHint::Replace),
            ..Self::bare("canvas_move", id)
        }
    }

    pub fn canvas_scroll(id: String, x: f32, y: f32, delta_x: f32, delta_y: f32) -> Self {
        Self {
            data: Some(serde_json::json!({
                "x": sanitize_f32(x),
                "y": sanitize_f32(y),
                "delta_x": sanitize_f32(delta_x),
                "delta_y": sanitize_f32(delta_y),
            })),
            coalesce: Some(CoalesceHint::Accumulate(vec![
                "delta_x".into(),
                "delta_y".into(),
            ])),
            ..Self::bare("canvas_scroll", id)
        }
    }

    // -----------------------------------------------------------------------
    // Canvas shape events (interactive shapes)
    // -----------------------------------------------------------------------

    pub fn canvas_shape_enter(canvas_id: String, shape_id: String, x: f32, y: f32) -> Self {
        Self {
            data: Some(serde_json::json!({
                "shape_id": shape_id,
                "x": sanitize_f32(x),
                "y": sanitize_f32(y),
            })),
            ..Self::bare("canvas_shape_enter", canvas_id)
        }
    }

    pub fn canvas_shape_leave(canvas_id: String, shape_id: String) -> Self {
        Self {
            data: Some(serde_json::json!({"shape_id": shape_id})),
            ..Self::bare("canvas_shape_leave", canvas_id)
        }
    }

    pub fn canvas_shape_click(
        canvas_id: String,
        shape_id: String,
        x: f32,
        y: f32,
        button: String,
    ) -> Self {
        Self {
            data: Some(serde_json::json!({
                "shape_id": shape_id,
                "x": sanitize_f32(x),
                "y": sanitize_f32(y),
                "button": button,
            })),
            ..Self::bare("canvas_shape_click", canvas_id)
        }
    }

    pub fn canvas_shape_drag(
        canvas_id: String,
        shape_id: String,
        x: f32,
        y: f32,
        dx: f32,
        dy: f32,
    ) -> Self {
        Self {
            data: Some(serde_json::json!({
                "shape_id": shape_id,
                "x": sanitize_f32(x),
                "y": sanitize_f32(y),
                "delta_x": sanitize_f32(dx),
                "delta_y": sanitize_f32(dy),
            })),
            coalesce: Some(CoalesceHint::Replace),
            ..Self::bare("canvas_shape_drag", canvas_id)
        }
    }

    pub fn canvas_shape_drag_end(canvas_id: String, shape_id: String, x: f32, y: f32) -> Self {
        Self {
            data: Some(serde_json::json!({
                "shape_id": shape_id,
                "x": sanitize_f32(x),
                "y": sanitize_f32(y),
            })),
            ..Self::bare("canvas_shape_drag_end", canvas_id)
        }
    }

    pub fn canvas_shape_focused(canvas_id: String, shape_id: String) -> Self {
        Self {
            data: Some(serde_json::json!({"shape_id": shape_id})),
            ..Self::bare("canvas_shape_focused", canvas_id)
        }
    }

    // -----------------------------------------------------------------------
    // MouseArea events
    // -----------------------------------------------------------------------

    pub fn mouse_right_press(id: String) -> Self {
        Self::bare("mouse_right_press", id)
    }

    pub fn mouse_right_release(id: String) -> Self {
        Self::bare("mouse_right_release", id)
    }

    pub fn mouse_middle_press(id: String) -> Self {
        Self::bare("mouse_middle_press", id)
    }

    pub fn mouse_middle_release(id: String) -> Self {
        Self::bare("mouse_middle_release", id)
    }

    pub fn mouse_double_click(id: String) -> Self {
        Self::bare("mouse_double_click", id)
    }

    pub fn mouse_enter(id: String) -> Self {
        Self::bare("mouse_enter", id)
    }

    pub fn mouse_exit(id: String) -> Self {
        Self::bare("mouse_exit", id)
    }

    pub fn mouse_area_move(id: String, x: f32, y: f32) -> Self {
        Self {
            data: Some(serde_json::json!({"x": sanitize_f32(x), "y": sanitize_f32(y)})),
            coalesce: Some(CoalesceHint::Replace),
            ..Self::bare("mouse_move", id)
        }
    }

    pub fn mouse_area_scroll(id: String, delta_x: f32, delta_y: f32) -> Self {
        Self {
            data: Some(
                serde_json::json!({"delta_x": sanitize_f32(delta_x), "delta_y": sanitize_f32(delta_y)}),
            ),
            coalesce: Some(CoalesceHint::Accumulate(vec![
                "delta_x".into(),
                "delta_y".into(),
            ])),
            ..Self::bare("mouse_scroll", id)
        }
    }

    // -----------------------------------------------------------------------
    // PaneGrid events
    // -----------------------------------------------------------------------

    pub fn pane_resized(id: String, split: String, ratio: f32) -> Self {
        Self {
            data: Some(serde_json::json!({"split": split, "ratio": sanitize_f32(ratio)})),
            coalesce: Some(CoalesceHint::Replace),
            ..Self::bare("pane_resized", id)
        }
    }

    pub fn pane_dragged(
        id: String,
        kind: &str,
        pane: String,
        target: Option<String>,
        region: Option<&str>,
        edge: Option<&str>,
    ) -> Self {
        let mut data = serde_json::json!({"action": kind, "pane": pane});
        if let Some(t) = target {
            data["target"] = serde_json::json!(t);
        }
        if let Some(r) = region {
            data["region"] = serde_json::json!(r);
        }
        if let Some(e) = edge {
            data["edge"] = serde_json::json!(e);
        }
        Self {
            data: Some(data),
            ..Self::bare("pane_dragged", id)
        }
    }

    pub fn pane_clicked(id: String, pane: String) -> Self {
        Self {
            data: Some(serde_json::json!({"pane": pane})),
            ..Self::bare("pane_clicked", id)
        }
    }

    pub fn pane_focus_cycle(id: String, pane: String) -> Self {
        Self {
            data: Some(serde_json::json!({"pane": pane})),
            ..Self::bare("pane_focus_cycle", id)
        }
    }

    // -----------------------------------------------------------------------
    // TextInput paste event
    // -----------------------------------------------------------------------

    pub fn paste(id: String, text: String) -> Self {
        Self {
            value: Some(Value::String(text)),
            ..Self::bare("paste", id)
        }
    }

    // -----------------------------------------------------------------------
    // Scripting key events (no full KeyEventData available)
    // -----------------------------------------------------------------------

    /// Key press event from scripting (no full KeyEventData).
    ///
    /// Produces the same event shape as real key_press events: `key` in
    /// `data.key`, modifiers in the top-level `modifiers` field. Missing
    /// modifier fields default to `false`.
    pub fn scripting_key_press(key: String, modifiers_json: Value) -> Self {
        let mods: KeyModifiers =
            serde_json::from_value(modifiers_json).unwrap_or(KeyModifiers::default());
        Self {
            modifiers: Some(mods),
            data: Some(serde_json::json!({"key": key})),
            ..Self::bare("key_press", String::new())
        }
    }

    /// Key release event from scripting (no full KeyEventData).
    ///
    /// Produces the same event shape as real key_release events: `key` in
    /// `data.key`, modifiers in the top-level `modifiers` field. Missing
    /// modifier fields default to `false`.
    pub fn scripting_key_release(key: String, modifiers_json: Value) -> Self {
        let mods: KeyModifiers =
            serde_json::from_value(modifiers_json).unwrap_or(KeyModifiers::default());
        Self {
            modifiers: Some(mods),
            data: Some(serde_json::json!({"key": key})),
            ..Self::bare("key_release", String::new())
        }
    }

    /// Cursor moved event from scripting.
    pub fn scripting_cursor_moved(x: f64, y: f64) -> Self {
        Self {
            data: Some(serde_json::json!({"x": x, "y": y})),
            ..Self::bare("cursor_moved", String::new())
        }
    }

    /// Scroll event from scripting.
    pub fn scripting_scroll(delta_x: f64, delta_y: f64) -> Self {
        Self {
            data: Some(
                serde_json::json!({"delta_x": delta_x, "delta_y": delta_y, "unit": "pixel"}),
            ),
            ..Self::bare("wheel_scrolled", String::new())
        }
    }

    // -----------------------------------------------------------------------
    // ComboBox option hovered event
    // -----------------------------------------------------------------------

    pub fn option_hovered(id: String, value: String) -> Self {
        Self {
            value: Some(Value::String(value)),
            ..Self::bare("option_hovered", id)
        }
    }

    // -----------------------------------------------------------------------
    // Scrollable events
    // -----------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    pub fn scroll(
        id: String,
        abs_x: f32,
        abs_y: f32,
        rel_x: f32,
        rel_y: f32,
        bounds_w: f32,
        bounds_h: f32,
        content_w: f32,
        content_h: f32,
    ) -> Self {
        Self {
            data: Some(serde_json::json!({
                "absolute_x": sanitize_f32(abs_x), "absolute_y": sanitize_f32(abs_y),
                "relative_x": sanitize_f32(rel_x), "relative_y": sanitize_f32(rel_y),
                "bounds_width": sanitize_f32(bounds_w), "bounds_height": sanitize_f32(bounds_h),
                "content_width": sanitize_f32(content_w), "content_height": sanitize_f32(content_h),
            })),
            coalesce: Some(CoalesceHint::Replace),
            ..Self::bare("scroll", id)
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Replace non-finite f32 with 0.0 for safe JSON serialization.
fn sanitize_f32(v: f32) -> f32 {
    if v.is_finite() {
        v
    } else {
        log::warn!("non-finite f32 ({v}) replaced with 0.0 in outgoing event");
        0.0
    }
}

/// Replace non-finite f64 with 0.0 for safe JSON serialization.
fn sanitize_f64(v: f64) -> f64 {
    if v.is_finite() {
        v
    } else {
        log::warn!("non-finite f64 ({v}) replaced with 0.0 in outgoing event");
        0.0
    }
}

// ---------------------------------------------------------------------------
// Response types (serialized to stdout in reply to incoming messages)
// ---------------------------------------------------------------------------

/// Response to an effect request, written to stdout as JSONL.
#[derive(Debug, Serialize)]
pub struct EffectResponse {
    #[serde(rename = "type")]
    pub message_type: &'static str,
    pub session: String,
    pub id: String,
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl EffectResponse {
    /// The effect completed successfully with the given result.
    pub fn ok(id: String, result: Value) -> Self {
        Self {
            message_type: "effect_response",
            session: String::new(),
            id,
            status: "ok",
            result: Some(result),
            error: None,
        }
    }

    /// The effect failed with the given reason.
    pub fn error(id: String, reason: String) -> Self {
        Self {
            message_type: "effect_response",
            session: String::new(),
            id,
            status: "error",
            result: None,
            error: Some(reason),
        }
    }

    /// The requested effect kind is not supported.
    pub fn unsupported(id: String) -> Self {
        Self::error(id, "unsupported".to_string())
    }

    /// The user cancelled the operation (e.g. closed a file dialog).
    /// Distinct from `error` -- cancellation is a normal user action,
    /// not a failure.
    pub fn cancelled(id: String) -> Self {
        Self {
            message_type: "effect_response",
            session: String::new(),
            id,
            status: "cancelled",
            result: None,
            error: None,
        }
    }

    /// Set the session ID for this response.
    pub fn with_session(mut self, session: impl Into<String>) -> Self {
        self.session = session.into();
        self
    }
}

/// Response to a Query message.
#[derive(Debug, Serialize)]
pub struct QueryResponse {
    #[serde(rename = "type")]
    pub message_type: &'static str,
    pub session: String,
    pub id: String,
    pub target: String,
    pub data: Value,
}

impl QueryResponse {
    pub fn new(id: String, target: String, data: Value) -> Self {
        Self {
            message_type: "query_response",
            session: String::new(),
            id,
            target,
            data,
        }
    }

    /// Set the session ID for this response.
    pub fn with_session(mut self, session: impl Into<String>) -> Self {
        self.session = session.into();
        self
    }
}

/// Response to an Interact message.
#[derive(Debug, Serialize)]
pub struct InteractResponse {
    #[serde(rename = "type")]
    pub message_type: &'static str,
    pub session: String,
    pub id: String,
    pub events: Vec<OutgoingEvent>,
}

impl InteractResponse {
    pub fn new(id: String, events: Vec<OutgoingEvent>) -> Self {
        Self {
            message_type: "interact_response",
            session: String::new(),
            id,
            events,
        }
    }

    /// Set the session ID for this response and all contained events.
    pub fn with_session(mut self, session: impl Into<String>) -> Self {
        let session = session.into();
        for event in &mut self.events {
            event.session.clone_from(&session);
        }
        self.session = session;
        self
    }
}

/// Response to a TreeHash message.
///
/// Tree hashes capture structural tree data (hash of JSON tree). No pixel data.
/// For pixel data, see the `screenshot_response` message type.
#[derive(Debug, Serialize)]
#[allow(dead_code)]
pub struct TreeHashResponse {
    #[serde(rename = "type")]
    pub message_type: &'static str,
    pub session: String,
    pub id: String,
    pub name: String,
    pub hash: String,
}

#[allow(dead_code)]
impl TreeHashResponse {
    pub fn new(id: String, name: String, hash: String) -> Self {
        Self {
            message_type: "tree_hash_response",
            session: String::new(),
            id,
            name,
            hash,
        }
    }

    /// Set the session ID for this response.
    pub fn with_session(mut self, session: impl Into<String>) -> Self {
        self.session = session.into();
        self
    }
}

/// Response to a Reset message.
#[derive(Debug, Serialize)]
pub struct ResetResponse {
    #[serde(rename = "type")]
    pub message_type: &'static str,
    pub session: String,
    pub id: String,
    pub status: &'static str,
}

impl ResetResponse {
    pub fn ok(id: String) -> Self {
        Self {
            message_type: "reset_response",
            session: String::new(),
            id,
            status: "ok",
        }
    }

    /// Set the session ID for this response.
    pub fn with_session(mut self, session: impl Into<String>) -> Self {
        self.session = session.into();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // -----------------------------------------------------------------------
    // OutgoingEvent serialization -- widget events
    // -----------------------------------------------------------------------

    #[test]
    fn serialize_click_event() {
        let evt = OutgoingEvent::click("btn1".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["type"], "event");
        assert_eq!(json["family"], "click");
        assert_eq!(json["id"], "btn1");
        assert!(json.get("value").is_none());
        assert!(json.get("tag").is_none());
        assert!(json.get("modifiers").is_none());
    }

    #[test]
    fn serialize_input_event() {
        let evt = OutgoingEvent::input("inp1".to_string(), "hello".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "input");
        assert_eq!(json["id"], "inp1");
        assert_eq!(json["value"], "hello");
    }

    #[test]
    fn serialize_submit_event() {
        let evt = OutgoingEvent::submit("form1".to_string(), "data".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "submit");
        assert_eq!(json["value"], "data");
    }

    #[test]
    fn serialize_toggle_event_true() {
        let evt = OutgoingEvent::toggle("chk1".to_string(), true);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "toggle");
        assert_eq!(json["value"], true);
    }

    #[test]
    fn serialize_toggle_event_false() {
        let evt = OutgoingEvent::toggle("chk1".to_string(), false);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["value"], false);
    }

    #[test]
    fn serialize_slide_event() {
        let evt = OutgoingEvent::slide("slider1".to_string(), 0.75);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "slide");
        assert_eq!(json["value"], 0.75);
    }

    #[test]
    fn serialize_slide_release_event() {
        let evt = OutgoingEvent::slide_release("slider1".to_string(), 0.5);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "slide_release");
        assert_eq!(json["value"], 0.5);
    }

    #[test]
    fn serialize_select_event() {
        let evt = OutgoingEvent::select("picker1".to_string(), "option_b".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "select");
        assert_eq!(json["value"], "option_b");
    }

    // -----------------------------------------------------------------------
    // OutgoingEvent serialization -- keyboard events
    // -----------------------------------------------------------------------

    fn make_key_event_data(key_str: &str, shift: bool, alt: bool) -> crate::message::KeyEventData {
        use iced::keyboard;
        crate::message::KeyEventData {
            key: if key_str.len() == 1 {
                keyboard::Key::Character(key_str.into())
            } else {
                keyboard::Key::Named(keyboard::key::Named::Escape)
            },
            modified_key: if key_str.len() == 1 {
                keyboard::Key::Character(key_str.to_uppercase().into())
            } else {
                keyboard::Key::Named(keyboard::key::Named::Escape)
            },
            physical_key: keyboard::key::Physical::Code(keyboard::key::Code::KeyA),
            location: keyboard::Location::Standard,
            modifiers: {
                let mut m = keyboard::Modifiers::empty();
                if shift {
                    m |= keyboard::Modifiers::SHIFT;
                }
                if alt {
                    m |= keyboard::Modifiers::ALT;
                }
                m
            },
            text: if key_str.len() == 1 {
                Some(key_str.to_string())
            } else {
                None
            },
            repeat: false,
            captured: false,
        }
    }

    #[test]
    fn serialize_key_press_with_modifiers() {
        let data = make_key_event_data("a", true, true);
        let evt = OutgoingEvent::key_press("keys".to_string(), &data);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "key_press");
        assert_eq!(json["tag"], "keys");
        assert!(json.get("value").is_none());
        assert!(json["id"].as_str().unwrap().is_empty());
        assert_eq!(json["modifiers"]["shift"], true);
        assert_eq!(json["modifiers"]["ctrl"], false);
        assert_eq!(json["modifiers"]["alt"], true);
        assert_eq!(json["modifiers"]["logo"], false);
        assert_eq!(json["modifiers"]["command"], false);
        assert_eq!(json["data"]["key"], "a");
        assert_eq!(json["data"]["modified_key"], "A");
        assert_eq!(json["data"]["physical_key"], "KeyA");
        assert_eq!(json["data"]["location"], "standard");
        assert_eq!(json["data"]["text"], "a");
        assert_eq!(json["data"]["repeat"], false);
    }

    #[test]
    fn serialize_key_release() {
        let data = make_key_event_data("Escape", false, false);
        let evt = OutgoingEvent::key_release("keys".to_string(), &data);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "key_release");
        assert_eq!(json["data"]["key"], "Escape");
        // key_release should not have text or repeat
        assert!(json["data"].get("text").is_none());
    }

    #[test]
    fn serialize_modifiers_changed() {
        let mods = KeyModifiers {
            shift: true,
            ctrl: true,
            alt: false,
            logo: false,
            command: false,
        };
        let evt = OutgoingEvent::modifiers_changed("mods".to_string(), mods);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "modifiers_changed");
        assert!(json.get("value").is_none());
        assert_eq!(json["modifiers"]["shift"], true);
        assert_eq!(json["modifiers"]["ctrl"], true);
    }

    // -----------------------------------------------------------------------
    // OutgoingEvent serialization -- mouse events
    // -----------------------------------------------------------------------

    #[test]
    fn serialize_cursor_moved() {
        let evt = OutgoingEvent::cursor_moved("mouse".to_string(), 100.0, 200.0);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "cursor_moved");
        assert_eq!(json["data"]["x"], 100.0);
        assert_eq!(json["data"]["y"], 200.0);
    }

    #[test]
    fn serialize_cursor_entered() {
        let evt = OutgoingEvent::cursor_entered("mouse".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "cursor_entered");
        assert_eq!(json["tag"], "mouse");
    }

    #[test]
    fn serialize_cursor_left() {
        let evt = OutgoingEvent::cursor_left("mouse".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "cursor_left");
    }

    #[test]
    fn serialize_button_pressed() {
        let evt = OutgoingEvent::button_pressed("mouse".to_string(), "Left".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "button_pressed");
        assert_eq!(json["value"], "Left");
    }

    #[test]
    fn serialize_button_released() {
        let evt = OutgoingEvent::button_released("mouse".to_string(), "Right".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "button_released");
        assert_eq!(json["value"], "Right");
    }

    #[test]
    fn serialize_wheel_scrolled() {
        let evt = OutgoingEvent::wheel_scrolled("mouse".to_string(), 0.0, -3.0, "line");
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "wheel_scrolled");
        assert_eq!(json["data"]["delta_x"], 0.0);
        assert_eq!(json["data"]["delta_y"], -3.0);
        assert_eq!(json["data"]["unit"], "line");
    }

    // -----------------------------------------------------------------------
    // OutgoingEvent serialization -- touch events
    // -----------------------------------------------------------------------

    #[test]
    fn serialize_finger_pressed() {
        let evt = OutgoingEvent::finger_pressed("touch".to_string(), 1, 50.0, 75.0);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "finger_pressed");
        assert_eq!(json["data"]["id"], 1);
        assert_eq!(json["data"]["x"], 50.0);
        assert_eq!(json["data"]["y"], 75.0);
    }

    #[test]
    fn serialize_finger_moved() {
        let evt = OutgoingEvent::finger_moved("touch".to_string(), 2, 60.0, 80.0);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "finger_moved");
        assert_eq!(json["data"]["id"], 2);
    }

    #[test]
    fn serialize_finger_lifted() {
        let evt = OutgoingEvent::finger_lifted("touch".to_string(), 1, 55.0, 78.0);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "finger_lifted");
    }

    #[test]
    fn serialize_finger_lost() {
        let evt = OutgoingEvent::finger_lost("touch".to_string(), 3, 0.0, 0.0);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "finger_lost");
        assert_eq!(json["data"]["id"], 3);
    }

    // -----------------------------------------------------------------------
    // OutgoingEvent serialization -- window lifecycle events
    // -----------------------------------------------------------------------

    #[test]
    fn serialize_window_opened_with_position() {
        let evt = OutgoingEvent::window_opened(
            "win_events".to_string(),
            "main".to_string(),
            Some((10.0, 20.0)),
            800.0,
            600.0,
            2.0,
        );
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "window_opened");
        assert_eq!(json["data"]["window_id"], "main");
        assert_eq!(json["data"]["width"], 800.0);
        assert_eq!(json["data"]["height"], 600.0);
        assert_eq!(json["data"]["position"]["x"], 10.0);
        assert_eq!(json["data"]["position"]["y"], 20.0);
        assert_eq!(json["data"]["scale_factor"], 2.0);
    }

    #[test]
    fn serialize_window_opened_without_position() {
        let evt = OutgoingEvent::window_opened(
            "win_events".to_string(),
            "main".to_string(),
            None,
            1024.0,
            768.0,
            1.0,
        );
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "window_opened");
        assert!(json["data"]["position"].is_null());
        assert_eq!(json["data"]["scale_factor"], 1.0);
    }

    #[test]
    fn serialize_window_closed() {
        let evt = OutgoingEvent::window_closed("win_events".to_string(), "popup".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "window_closed");
        assert_eq!(json["data"]["window_id"], "popup");
    }

    #[test]
    fn serialize_window_close_requested() {
        let evt =
            OutgoingEvent::window_close_requested("win_events".to_string(), "main".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "window_close_requested");
    }

    #[test]
    fn serialize_window_moved() {
        let evt =
            OutgoingEvent::window_moved("win_events".to_string(), "main".to_string(), 50.0, 100.0);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "window_moved");
        assert_eq!(json["data"]["x"], 50.0);
        assert_eq!(json["data"]["y"], 100.0);
    }

    #[test]
    fn serialize_window_resized() {
        let evt = OutgoingEvent::window_resized(
            "win_events".to_string(),
            "main".to_string(),
            1920.0,
            1080.0,
        );
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "window_resized");
        assert_eq!(json["data"]["width"], 1920.0);
    }

    #[test]
    fn serialize_window_focused() {
        let evt = OutgoingEvent::window_focused("win_events".to_string(), "main".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "window_focused");
    }

    #[test]
    fn serialize_window_unfocused() {
        let evt = OutgoingEvent::window_unfocused("win_events".to_string(), "main".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "window_unfocused");
    }

    #[test]
    fn serialize_window_rescaled() {
        let evt = OutgoingEvent::window_rescaled("win_events".to_string(), "main".to_string(), 2.0);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "window_rescaled");
        assert_eq!(json["data"]["scale_factor"], 2.0);
    }

    #[test]
    fn serialize_file_hovered() {
        let evt = OutgoingEvent::file_hovered(
            "win_events".to_string(),
            "main".to_string(),
            "/tmp/a.txt".to_string(),
        );
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "file_hovered");
        assert_eq!(json["data"]["path"], "/tmp/a.txt");
    }

    #[test]
    fn serialize_file_dropped() {
        let evt = OutgoingEvent::file_dropped(
            "win_events".to_string(),
            "main".to_string(),
            "/tmp/b.txt".to_string(),
        );
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "file_dropped");
        assert_eq!(json["data"]["path"], "/tmp/b.txt");
    }

    #[test]
    fn serialize_files_hovered_left() {
        let evt = OutgoingEvent::files_hovered_left("win_events".to_string(), "main".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "files_hovered_left");
    }

    // -----------------------------------------------------------------------
    // OutgoingEvent serialization -- sensor events
    // -----------------------------------------------------------------------

    #[test]
    fn serialize_sensor_resize() {
        let evt = OutgoingEvent::sensor_resize("s1".to_string(), 100.0, 200.0);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "sensor_resize");
        assert_eq!(json["id"], "s1");
        assert_eq!(json["data"]["width"], 100.0);
        assert_eq!(json["data"]["height"], 200.0);
    }

    // -----------------------------------------------------------------------
    // OutgoingEvent serialization -- canvas events
    // -----------------------------------------------------------------------

    #[test]
    fn serialize_canvas_press() {
        let evt = OutgoingEvent::canvas_press("c1".to_string(), 10.0, 20.0, "Left".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "canvas_press");
        assert_eq!(json["data"]["x"], 10.0);
        assert_eq!(json["data"]["button"], "Left");
    }

    #[test]
    fn serialize_canvas_release() {
        let evt = OutgoingEvent::canvas_release("c1".to_string(), 10.0, 20.0, "Left".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "canvas_release");
    }

    #[test]
    fn serialize_canvas_move() {
        let evt = OutgoingEvent::canvas_move("c1".to_string(), 30.0, 40.0);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "canvas_move");
        assert_eq!(json["data"]["x"], 30.0);
        assert_eq!(json["data"]["y"], 40.0);
    }

    #[test]
    fn serialize_canvas_scroll() {
        let evt = OutgoingEvent::canvas_scroll("c1".to_string(), 5.0, 5.0, 0.0, -1.0);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "canvas_scroll");
        assert_eq!(json["data"]["delta_y"], -1.0);
    }

    // -----------------------------------------------------------------------
    // OutgoingEvent serialization -- mouse area events
    // -----------------------------------------------------------------------

    #[test]
    fn serialize_mouse_right_press() {
        let evt = OutgoingEvent::mouse_right_press("zone".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "mouse_right_press");
        assert_eq!(json["id"], "zone");
    }

    #[test]
    fn serialize_mouse_right_release() {
        let evt = OutgoingEvent::mouse_right_release("zone".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "mouse_right_release");
    }

    #[test]
    fn serialize_mouse_middle_press() {
        let evt = OutgoingEvent::mouse_middle_press("zone".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "mouse_middle_press");
        assert_eq!(json["id"], "zone");
    }

    #[test]
    fn serialize_mouse_middle_release() {
        let evt = OutgoingEvent::mouse_middle_release("zone".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "mouse_middle_release");
    }

    #[test]
    fn serialize_mouse_double_click() {
        let evt = OutgoingEvent::mouse_double_click("zone".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "mouse_double_click");
    }

    #[test]
    fn serialize_mouse_enter() {
        let evt = OutgoingEvent::mouse_enter("zone".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "mouse_enter");
    }

    #[test]
    fn serialize_mouse_exit() {
        let evt = OutgoingEvent::mouse_exit("zone".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "mouse_exit");
    }

    #[test]
    fn serialize_mouse_area_move() {
        let evt = OutgoingEvent::mouse_area_move("zone".to_string(), 10.5, 20.3);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "mouse_move");
        assert_eq!(json["id"], "zone");
        let data = &json["data"];
        assert!((data["x"].as_f64().unwrap() - 10.5).abs() < 0.01);
        assert!((data["y"].as_f64().unwrap() - 20.3).abs() < 0.01);
    }

    #[test]
    fn serialize_mouse_area_scroll() {
        let evt = OutgoingEvent::mouse_area_scroll("zone".to_string(), 0.0, -3.0);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "mouse_scroll");
        assert_eq!(json["id"], "zone");
        assert_eq!(json["data"]["delta_x"], 0.0);
        assert_eq!(json["data"]["delta_y"], -3.0);
    }

    // -----------------------------------------------------------------------
    // OutgoingEvent serialization -- pane grid events
    // -----------------------------------------------------------------------

    #[test]
    fn serialize_pane_resized() {
        let evt = OutgoingEvent::pane_resized("pg1".to_string(), "split_0".to_string(), 0.5);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "pane_resized");
        assert_eq!(json["data"]["split"], "split_0");
        assert_eq!(json["data"]["ratio"], json!(0.5));
    }

    #[test]
    fn serialize_pane_dragged_dropped() {
        let evt = OutgoingEvent::pane_dragged(
            "pg1".to_string(),
            "dropped",
            "pane_a".to_string(),
            Some("pane_b".to_string()),
            Some("center"),
            None,
        );
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "pane_dragged");
        assert_eq!(json["data"]["action"], "dropped");
        assert_eq!(json["data"]["pane"], "pane_a");
        assert_eq!(json["data"]["target"], "pane_b");
        assert_eq!(json["data"]["region"], "center");
    }

    #[test]
    fn serialize_pane_dragged_picked() {
        let evt = OutgoingEvent::pane_dragged(
            "pg1".to_string(),
            "picked",
            "pane_a".to_string(),
            None,
            None,
            None,
        );
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["data"]["action"], "picked");
        assert_eq!(json["data"]["pane"], "pane_a");
        assert!(json["data"].get("target").is_none());
    }

    #[test]
    fn serialize_pane_dragged_canceled() {
        let evt = OutgoingEvent::pane_dragged(
            "pg1".to_string(),
            "canceled",
            "pane_a".to_string(),
            None,
            None,
            None,
        );
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["data"]["action"], "canceled");
    }

    #[test]
    fn serialize_pane_focus_cycle() {
        let evt = OutgoingEvent::pane_focus_cycle("pg1".to_string(), "pane_a".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "pane_focus_cycle");
        assert_eq!(json["data"]["pane"], "pane_a");
    }

    #[test]
    fn serialize_pane_clicked() {
        let evt = OutgoingEvent::pane_clicked("pg1".to_string(), "pane_x".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "pane_clicked");
        assert_eq!(json["data"]["pane"], "pane_x");
    }

    // -----------------------------------------------------------------------
    // OutgoingEvent serialization -- animation/theme events
    // -----------------------------------------------------------------------

    #[test]
    fn serialize_animation_frame() {
        let evt = OutgoingEvent::animation_frame("anim".to_string(), 16000);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "animation_frame");
        assert_eq!(json["data"]["timestamp"], 16000);
    }

    #[test]
    fn serialize_theme_changed() {
        let evt = OutgoingEvent::theme_changed("theme".to_string(), "dark".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "theme_changed");
        assert_eq!(json["value"], "dark");
    }

    // -----------------------------------------------------------------------
    // Round-trip: serialize then deserialize OutgoingEvent as generic Value
    // -----------------------------------------------------------------------

    #[test]
    fn outgoing_event_roundtrip_all_fields_present() {
        let data = make_key_event_data("a", true, false);
        let evt = OutgoingEvent::key_press("kb".to_string(), &data);
        let serialized = serde_json::to_string(&evt).unwrap();
        let parsed: Value = serde_json::from_str(&serialized).unwrap();
        assert_eq!(parsed["type"], "event");
        assert_eq!(parsed["family"], "key_press");
        assert_eq!(parsed["tag"], "kb");
        assert_eq!(parsed["modifiers"]["shift"], true);
        assert_eq!(parsed["data"]["key"], "a");
        assert!(parsed["data"].get("modified_key").is_some());
        assert!(parsed["data"].get("physical_key").is_some());
        assert!(parsed["data"].get("location").is_some());
    }

    #[test]
    fn ime_opened_event() {
        let evt = OutgoingEvent::ime_opened("ime_tag".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["type"], "event");
        assert_eq!(json["family"], "ime_opened");
        assert_eq!(json["tag"], "ime_tag");
        assert!(json.get("data").is_none());
    }

    #[test]
    fn ime_preedit_event_with_cursor() {
        let evt =
            OutgoingEvent::ime_preedit("ime_tag".to_string(), "hello".to_string(), Some(2..5));
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "ime_preedit");
        assert_eq!(json["data"]["text"], "hello");
        assert_eq!(json["data"]["cursor"]["start"], 2);
        assert_eq!(json["data"]["cursor"]["end"], 5);
    }

    #[test]
    fn ime_preedit_event_without_cursor() {
        let evt = OutgoingEvent::ime_preedit("ime_tag".to_string(), "hi".to_string(), None);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "ime_preedit");
        assert_eq!(json["data"]["text"], "hi");
        assert!(json["data"]["cursor"].is_null());
    }

    #[test]
    fn ime_commit_event() {
        let evt = OutgoingEvent::ime_commit("ime_tag".to_string(), "final".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "ime_commit");
        assert_eq!(json["data"]["text"], "final");
    }

    #[test]
    fn ime_closed_event() {
        let evt = OutgoingEvent::ime_closed("ime_tag".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["family"], "ime_closed");
        assert!(json.get("data").is_none());
    }

    #[test]
    fn outgoing_event_bare_omits_optional_fields() {
        let evt = OutgoingEvent::click("b".to_string());
        let serialized = serde_json::to_string(&evt).unwrap();
        // value, tag, modifiers, captured should all be absent from the JSON string
        assert!(!serialized.contains("\"value\""));
        assert!(!serialized.contains("\"tag\""));
        assert!(!serialized.contains("\"modifiers\""));
        assert!(!serialized.contains("\"captured\""));
    }

    #[test]
    fn outgoing_event_with_captured_true() {
        let evt = OutgoingEvent::cursor_moved("m".to_string(), 1.0, 2.0).with_captured(true);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["captured"], true);
    }

    #[test]
    fn outgoing_event_with_captured_false() {
        let evt =
            OutgoingEvent::key_press("kb".to_string(), &make_key_event_data("a", false, false))
                .with_captured(false);
        let json = serde_json::to_value(&evt).unwrap();
        assert_eq!(json["captured"], false);
    }

    #[test]
    fn outgoing_event_without_captured_omits_field() {
        let evt = OutgoingEvent::click("btn".to_string());
        let json = serde_json::to_value(&evt).unwrap();
        assert!(json.get("captured").is_none());
    }

    // -- float sanitization (outgoing context) --

    #[test]
    fn outgoing_slide_with_nan_produces_zero() {
        let event = OutgoingEvent::slide("s1".to_string(), f64::NAN);
        // The value should be 0.0, not NaN
        let val = event.value.unwrap();
        assert_eq!(val.as_f64(), Some(0.0));
    }

    #[test]
    fn outgoing_cursor_moved_with_infinity_produces_zero() {
        let event =
            OutgoingEvent::cursor_moved("tag".to_string(), f32::INFINITY, f32::NEG_INFINITY);
        let data = event.data.unwrap();
        assert_eq!(data["x"].as_f64(), Some(0.0));
        assert_eq!(data["y"].as_f64(), Some(0.0));
    }

    // -- float sanitization --

    #[test]
    fn sanitize_f32_passes_finite() {
        assert_eq!(sanitize_f32(1.5), 1.5);
        assert_eq!(sanitize_f32(-0.0), -0.0);
        assert_eq!(sanitize_f32(0.0), 0.0);
    }

    #[test]
    fn sanitize_f32_replaces_nan() {
        assert_eq!(sanitize_f32(f32::NAN), 0.0);
    }

    #[test]
    fn sanitize_f32_replaces_infinity() {
        assert_eq!(sanitize_f32(f32::INFINITY), 0.0);
        assert_eq!(sanitize_f32(f32::NEG_INFINITY), 0.0);
    }

    #[test]
    fn sanitize_f64_passes_finite() {
        assert_eq!(sanitize_f64(42.0), 42.0);
    }

    #[test]
    fn sanitize_f64_replaces_nan() {
        assert_eq!(sanitize_f64(f64::NAN), 0.0);
    }

    #[test]
    fn sanitize_f64_replaces_infinity() {
        assert_eq!(sanitize_f64(f64::INFINITY), 0.0);
        assert_eq!(sanitize_f64(f64::NEG_INFINITY), 0.0);
    }

    // -----------------------------------------------------------------------
    // EffectResponse serialization
    // -----------------------------------------------------------------------

    #[test]
    fn effect_response_ok() {
        let resp = EffectResponse::ok("e1".to_string(), json!("clipboard content"));
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["type"], "effect_response");
        assert_eq!(json["id"], "e1");
        assert_eq!(json["status"], "ok");
        assert_eq!(json["result"], "clipboard content");
        assert!(json.get("error").is_none());
    }

    #[test]
    fn effect_response_error() {
        let resp = EffectResponse::error("e2".to_string(), "not found".to_string());
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["type"], "effect_response");
        assert_eq!(json["id"], "e2");
        assert_eq!(json["status"], "error");
        assert_eq!(json["error"], "not found");
        assert!(json.get("result").is_none());
    }

    #[test]
    fn effect_response_unsupported() {
        let resp = EffectResponse::unsupported("e3".to_string());
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["status"], "error");
        assert_eq!(json["error"], "unsupported");
    }

    #[test]
    fn effect_response_ok_with_object_result() {
        let resp = EffectResponse::ok("e4".to_string(), json!({"files": ["/a.txt", "/b.txt"]}));
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["result"]["files"][0], "/a.txt");
        assert_eq!(json["result"]["files"][1], "/b.txt");
    }

    #[test]
    fn effect_response_cancelled() {
        let resp = EffectResponse::cancelled("e5".to_string());
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["type"], "effect_response");
        assert_eq!(json["id"], "e5");
        assert_eq!(json["status"], "cancelled");
        assert!(json.get("result").is_none());
        assert!(json.get("error").is_none());
    }
}
