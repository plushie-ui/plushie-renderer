use serde_json::Value;

use super::{sanitize_f32, sanitize_f64, KeyModifiers, OutgoingEvent};

// ---------------------------------------------------------------------------
// Widget events (click, input, toggle, slide, select, submit)
// ---------------------------------------------------------------------------

impl OutgoingEvent {
    /// Helper to build a bare event with only the common fields.
    fn bare(family: impl Into<String>, id: String) -> Self {
        Self {
            message_type: "event",
            family: family.into(),
            id,
            value: None,
            tag: None,
            modifiers: None,
            data: None,
        }
    }

    /// Helper to build a subscription-tagged event with no widget id.
    fn tagged(family: impl Into<String>, tag: String) -> Self {
        Self {
            message_type: "event",
            family: family.into(),
            id: String::new(),
            value: None,
            tag: Some(tag),
            modifiers: None,
            data: None,
        }
    }

    /// Generic widget event with a family string and optional data payload.
    /// Used for on_open, on_close, sort, and other events.
    pub fn generic(family: impl Into<String>, id: String, data: Option<Value>) -> Self {
        Self {
            data,
            ..Self::bare(family, id)
        }
    }

    /// Convenience constructor for extension-emitted events.
    pub fn extension_event(family: String, id: String, data: Option<Value>) -> Self {
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
            value: Some(Value::String(crate::message::serialize_key(&data.key))),
            data: Some(serde_json::json!({
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
            value: Some(Value::String(crate::message::serialize_key(&data.key))),
            data: Some(serde_json::json!({
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
            ..Self::tagged("modifiers_changed", tag)
        }
    }

    // -----------------------------------------------------------------------
    // Mouse events
    // -----------------------------------------------------------------------

    pub fn cursor_moved(tag: String, x: f32, y: f32) -> Self {
        Self {
            data: Some(serde_json::json!({"x": sanitize_f32(x), "y": sanitize_f32(y)})),
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
            ..Self::tagged("wheel_scrolled", tag)
        }
    }

    // -----------------------------------------------------------------------
    // Touch events
    // -----------------------------------------------------------------------

    pub fn finger_pressed(tag: String, finger_id: u64, x: f32, y: f32) -> Self {
        Self {
            data: Some(serde_json::json!({
                "finger_id": finger_id,
                "x": sanitize_f32(x),
                "y": sanitize_f32(y),
            })),
            ..Self::tagged("finger_pressed", tag)
        }
    }

    pub fn finger_moved(tag: String, finger_id: u64, x: f32, y: f32) -> Self {
        Self {
            data: Some(serde_json::json!({
                "finger_id": finger_id,
                "x": sanitize_f32(x),
                "y": sanitize_f32(y),
            })),
            ..Self::tagged("finger_moved", tag)
        }
    }

    pub fn finger_lifted(tag: String, finger_id: u64, x: f32, y: f32) -> Self {
        Self {
            data: Some(serde_json::json!({
                "finger_id": finger_id,
                "x": sanitize_f32(x),
                "y": sanitize_f32(y),
            })),
            ..Self::tagged("finger_lifted", tag)
        }
    }

    pub fn finger_lost(tag: String, finger_id: u64, x: f32, y: f32) -> Self {
        Self {
            data: Some(serde_json::json!({
                "finger_id": finger_id,
                "x": sanitize_f32(x),
                "y": sanitize_f32(y),
            })),
            ..Self::tagged("finger_lost", tag)
        }
    }

    // -----------------------------------------------------------------------
    // IME events
    // -----------------------------------------------------------------------

    pub fn ime_opened(tag: String) -> Self {
        Self {
            data: Some(serde_json::json!({"kind": "opened"})),
            ..Self::tagged("ime", tag)
        }
    }

    pub fn ime_preedit(tag: String, text: String, cursor: Option<std::ops::Range<usize>>) -> Self {
        let cursor_val = cursor
            .map(|r| serde_json::json!({"start": r.start, "end": r.end}))
            .unwrap_or(serde_json::Value::Null);
        Self {
            data: Some(serde_json::json!({"kind": "preedit", "text": text, "cursor": cursor_val})),
            ..Self::tagged("ime", tag)
        }
    }

    pub fn ime_commit(tag: String, text: String) -> Self {
        Self {
            data: Some(serde_json::json!({"kind": "commit", "text": text})),
            ..Self::tagged("ime", tag)
        }
    }

    pub fn ime_closed(tag: String) -> Self {
        Self {
            data: Some(serde_json::json!({"kind": "closed"})),
            ..Self::tagged("ime", tag)
        }
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
    ) -> Self {
        let pos =
            position.map(|(x, y)| serde_json::json!({"x": sanitize_f32(x), "y": sanitize_f32(y)}));
        Self {
            data: Some(serde_json::json!({
                "window_id": window_id,
                "position": pos,
                "width": sanitize_f32(width),
                "height": sanitize_f32(height),
            })),
            ..Self::tagged("window_opened", tag)
        }
    }

    pub fn window_closed(tag: String, window_id: String) -> Self {
        Self {
            data: Some(serde_json::json!({"window_id": window_id})),
            ..Self::tagged("window_closed", tag)
        }
    }

    pub fn window_close_requested(tag: String, window_id: String) -> Self {
        Self {
            data: Some(serde_json::json!({"window_id": window_id})),
            ..Self::tagged("window_close_requested", tag)
        }
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
        Self {
            data: Some(serde_json::json!({"window_id": window_id})),
            ..Self::tagged("window_focused", tag)
        }
    }

    pub fn window_unfocused(tag: String, window_id: String) -> Self {
        Self {
            data: Some(serde_json::json!({"window_id": window_id})),
            ..Self::tagged("window_unfocused", tag)
        }
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
        Self {
            data: Some(serde_json::json!({"window_id": window_id})),
            ..Self::tagged("files_hovered_left", tag)
        }
    }

    // -----------------------------------------------------------------------
    // Animation / theme / system events
    // -----------------------------------------------------------------------

    pub fn animation_frame(tag: String, timestamp_millis: u128) -> Self {
        Self {
            data: Some(serde_json::json!({"timestamp": timestamp_millis})),
            ..Self::tagged("animation_frame", tag)
        }
    }

    pub fn theme_changed(tag: String, mode: String) -> Self {
        Self {
            value: Some(Value::String(mode)),
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
            ..Self::bare("sensor_resize", id)
        }
    }

    // -----------------------------------------------------------------------
    // Canvas events
    // -----------------------------------------------------------------------

    #[cfg(feature = "widget-canvas")]
    pub fn canvas_press(id: String, x: f32, y: f32, button: String) -> Self {
        Self {
            data: Some(
                serde_json::json!({"x": sanitize_f32(x), "y": sanitize_f32(y), "button": button}),
            ),
            ..Self::bare("canvas_press", id)
        }
    }

    #[cfg(feature = "widget-canvas")]
    pub fn canvas_release(id: String, x: f32, y: f32, button: String) -> Self {
        Self {
            data: Some(
                serde_json::json!({"x": sanitize_f32(x), "y": sanitize_f32(y), "button": button}),
            ),
            ..Self::bare("canvas_release", id)
        }
    }

    #[cfg(feature = "widget-canvas")]
    pub fn canvas_move(id: String, x: f32, y: f32) -> Self {
        Self {
            data: Some(serde_json::json!({"x": sanitize_f32(x), "y": sanitize_f32(y)})),
            ..Self::bare("canvas_move", id)
        }
    }

    #[cfg(feature = "widget-canvas")]
    pub fn canvas_scroll(id: String, x: f32, y: f32, delta_x: f32, delta_y: f32) -> Self {
        Self {
            data: Some(
                serde_json::json!({"x": sanitize_f32(x), "y": sanitize_f32(y), "delta_x": sanitize_f32(delta_x), "delta_y": sanitize_f32(delta_y)}),
            ),
            ..Self::bare("canvas_scroll", id)
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
            ..Self::bare("mouse_move", id)
        }
    }

    pub fn mouse_area_scroll(id: String, delta_x: f32, delta_y: f32) -> Self {
        Self {
            data: Some(
                serde_json::json!({"delta_x": sanitize_f32(delta_x), "delta_y": sanitize_f32(delta_y)}),
            ),
            ..Self::bare("mouse_scroll", id)
        }
    }

    // -----------------------------------------------------------------------
    // PaneGrid events
    // -----------------------------------------------------------------------

    pub fn pane_resized(id: String, split: String, ratio: f32) -> Self {
        Self {
            data: Some(serde_json::json!({"split": split, "ratio": sanitize_f32(ratio)})),
            ..Self::bare("pane_resized", id)
        }
    }

    pub fn pane_dragged(id: String, pane: String, target: String) -> Self {
        Self {
            data: Some(serde_json::json!({"pane": pane, "target": target})),
            ..Self::bare("pane_dragged", id)
        }
    }

    pub fn pane_clicked(id: String, pane: String) -> Self {
        Self {
            data: Some(serde_json::json!({"pane": pane})),
            ..Self::bare("pane_clicked", id)
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
            ..Self::bare("scroll", id)
        }
    }
}
