//! Internal message enum and serialization helpers.
//!
//! [`Message`] is the iced `Message` type used by the renderer. Every
//! widget interaction (click, input, slide, toggle, etc.) and every
//! runtime event (keyboard, mouse, window lifecycle) maps to a variant.
//! The renderer's `update()` method dispatches on these variants to
//! emit outgoing events over the wire protocol.
//!
//! The serialization helpers convert iced types (keys, modifiers, mouse
//! buttons, scroll deltas) into the wire-format strings expected by the
//! host.

use iced::widget::markdown;
use iced::widget::text_editor;
use iced::{Point, window};
use serde_json::Value;

use crate::protocol::KeyModifiers;

// ---------------------------------------------------------------------------
// Event data structs
// ---------------------------------------------------------------------------

/// Scrollable viewport state, emitted on scroll position changes.
#[derive(Debug, Clone, Copy)]
pub struct ScrollViewport {
    /// Absolute scroll offset on the x axis (pixels from left).
    pub absolute_x: f32,
    /// Absolute scroll offset on the y axis (pixels from top).
    pub absolute_y: f32,
    /// Relative scroll position on the x axis (0.0 = start, 1.0 = end).
    pub relative_x: f32,
    /// Relative scroll position on the y axis (0.0 = top, 1.0 = bottom).
    pub relative_y: f32,
    /// Total content width (may exceed viewport).
    pub content_width: f32,
    /// Total content height (may exceed viewport).
    pub content_height: f32,
    /// Visible viewport width.
    pub viewport_width: f32,
    /// Visible viewport height.
    pub viewport_height: f32,
}

/// All fields from an iced keyboard event, packed for Message transport.
#[derive(Debug, Clone)]
pub struct KeyEventData {
    pub key: iced::keyboard::Key,
    pub modified_key: iced::keyboard::Key,
    pub physical_key: iced::keyboard::key::Physical,
    pub location: iced::keyboard::Location,
    pub modifiers: iced::keyboard::Modifiers,
    pub text: Option<String>,
    pub repeat: bool,
    /// Whether iced reported this event as `Captured` (consumed by a widget).
    pub captured: bool,
}

// ---------------------------------------------------------------------------
// Message
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum Message {
    /// A user clicked a button with the given node ID.
    Click(String),
    /// A text input value changed (id, new_value).
    Input(String, String),
    /// A text input was submitted (id, current_value).
    Submit(String, String),
    /// A checkbox or toggler was toggled (id, checked).
    Toggle(String, bool),
    /// A slider value changed (id, value).
    Slide(String, f64),
    /// A slider was released (id).
    SlideRelease(String),
    /// A pick_list/combo_box/radio selection (id, value).
    Select(String, String),
    /// A text editor action (id, action).
    TextEditorAction(String, text_editor::Action),
    /// A markdown link was clicked.
    MarkdownUrl(markdown::Uri),
    /// A message arrived from the stdin reader (or stdin closed).
    Stdin(StdinEvent),
    /// No-op: used as return value for fire-and-forget tasks (font loads, etc.)
    NoOp,
    /// A keyboard key was pressed (full event data).
    KeyPressed(KeyEventData),
    /// A keyboard key was released (full event data).
    KeyReleased(KeyEventData),
    /// Keyboard modifiers changed (modifiers, captured).
    ModifiersChanged(iced::keyboard::Modifiers, bool),
    // -- IME events --
    /// IME session opened (captured).
    ImeOpened(bool),
    /// IME preedit text updated (composing text, optional cursor range, captured).
    ImePreedit(String, Option<std::ops::Range<usize>>, bool),
    /// IME committed final text (text, captured).
    ImeCommit(String, bool),
    /// IME session closed (captured).
    ImeClosed(bool),
    /// A window close was requested by the user (WM close button).
    WindowCloseRequested(window::Id),
    /// A window was actually closed by iced.
    WindowClosed(window::Id),
    /// A new window was opened (iced_id, toddy_id).
    WindowOpened(window::Id, String),
    // -- Mouse events --
    /// Cursor moved to (x, y) in a window (position, window_id, captured).
    CursorMoved(Point, window::Id, bool),
    /// Cursor entered a window (window_id, captured).
    CursorEntered(window::Id, bool),
    /// Cursor left a window (window_id, captured).
    CursorLeft(window::Id, bool),
    /// Mouse button pressed (button, window_id, captured).
    MouseButtonPressed(iced::mouse::Button, window::Id, bool),
    /// Mouse button released (button, window_id, captured).
    MouseButtonReleased(iced::mouse::Button, window::Id, bool),
    /// Mouse wheel scrolled (delta, window_id, captured).
    WheelScrolled(iced::mouse::ScrollDelta, window::Id, bool),
    // -- Touch events --
    /// Touch finger pressed (finger, position, window_id, captured).
    FingerPressed(iced::touch::Finger, Point, window::Id, bool),
    /// Touch finger moved (finger, position, window_id, captured).
    FingerMoved(iced::touch::Finger, Point, window::Id, bool),
    /// Touch finger lifted (finger, position, window_id, captured).
    FingerLifted(iced::touch::Finger, Point, window::Id, bool),
    /// Touch finger lost (finger, position, window_id, captured).
    FingerLost(iced::touch::Finger, Point, window::Id, bool),
    // -- Window lifecycle events --
    /// A window event from iced (window_id, event).
    WindowEvent(window::Id, window::Event),
    // -- System / animation events --
    /// Animation frame with timestamp.
    AnimationFrame(iced::time::Instant),
    /// System theme mode changed.
    ThemeChanged(iced::theme::Mode),
    /// Sensor widget resize event (id, width, height).
    SensorResize(String, f32, f32),
    /// Canvas interaction event (press, release, move).
    CanvasEvent {
        id: String,
        kind: String,
        x: f32,
        y: f32,
        extra: String,
    },
    /// Canvas scroll event.
    CanvasScroll {
        id: String,
        x: f32,
        y: f32,
        delta_x: f32,
        delta_y: f32,
    },
    /// Canvas shape entered (cursor entered interactive shape bounds).
    CanvasShapeEnter {
        canvas_id: String,
        shape_id: String,
        x: f32,
        y: f32,
    },
    /// Canvas shape left (cursor exited interactive shape bounds).
    CanvasShapeLeave { canvas_id: String, shape_id: String },
    /// Canvas shape clicked (press + release on same shape).
    CanvasShapeClick {
        canvas_id: String,
        shape_id: String,
        x: f32,
        y: f32,
        button: String,
    },
    /// Canvas shape drag (continuous drag on draggable shape).
    CanvasShapeDrag {
        canvas_id: String,
        shape_id: String,
        x: f32,
        y: f32,
        delta_x: f32,
        delta_y: f32,
    },
    /// Canvas shape drag ended (release after drag).
    CanvasShapeDragEnd {
        canvas_id: String,
        shape_id: String,
        x: f32,
        y: f32,
    },
    /// Canvas shape received keyboard focus.
    CanvasShapeFocused { canvas_id: String, shape_id: String },
    /// PaneGrid pane was resized (grid_id, resize_event).
    PaneResized(String, iced::widget::pane_grid::ResizeEvent),
    /// PaneGrid pane was dragged (grid_id, drag_event).
    PaneDragged(String, iced::widget::pane_grid::DragEvent),
    /// PaneGrid pane was clicked (grid_id, pane).
    PaneClicked(String, iced::widget::pane_grid::Pane),
    /// PaneGrid focus cycle via F6 (grid_id, target_pane).
    PaneFocusCycle(String, iced::widget::pane_grid::Pane),
    /// Scrollable viewport changed.
    ScrollEvent(String, ScrollViewport),
    /// Text was pasted into a text_input (id, pasted_text).
    Paste(String, String),
    /// ComboBox option was hovered (combo_id, option_value).
    OptionHovered(String, String),
    /// MouseArea simple event (id, kind). Kind is one of: right_press,
    /// right_release, middle_release, double_click, enter, exit.
    MouseAreaEvent(String, String),
    /// MouseArea cursor move event (id, x, y).
    MouseAreaMove(String, f32, f32),
    /// MouseArea scroll event (id, delta_x, delta_y).
    MouseAreaScroll(String, f32, f32),
    /// Generic widget event. Used for on_open, on_close, sort, and
    /// other events that carry a family string and optional data.
    Event {
        id: String,
        data: Value,
        family: String,
    },
    /// Internal: flush the event coalesce buffer. Fired by a timer
    /// task scheduled by the EventEmitter when rate-limited events
    /// are pending.
    FlushCoalesce,
}

/// What the stdin reader thread sends back.
#[derive(Debug, Clone)]
pub enum StdinEvent {
    Message(crate::protocol::IncomingMessage),
    Closed,
    Warning(String),
}

// ---------------------------------------------------------------------------
// Key serialization helpers
// ---------------------------------------------------------------------------

pub fn serialize_key(key: &iced::keyboard::Key) -> String {
    match key {
        iced::keyboard::Key::Named(named) => format!("{named:?}"),
        iced::keyboard::Key::Character(c) => c.to_string(),
        iced::keyboard::Key::Unidentified => "Unidentified".to_string(),
    }
}

pub fn serialize_modifiers(mods: iced::keyboard::Modifiers) -> KeyModifiers {
    KeyModifiers {
        shift: mods.shift(),
        ctrl: mods.control(),
        alt: mods.alt(),
        logo: mods.logo(),
        command: mods.command(),
    }
}

pub fn serialize_physical_key(physical: &iced::keyboard::key::Physical) -> String {
    match physical {
        iced::keyboard::key::Physical::Code(code) => format!("{code:?}"),
        iced::keyboard::key::Physical::Unidentified(code) => {
            format!("Unidentified({code:?})")
        }
    }
}

pub fn serialize_location(location: &iced::keyboard::Location) -> &'static str {
    match location {
        iced::keyboard::Location::Standard => "standard",
        iced::keyboard::Location::Left => "left",
        iced::keyboard::Location::Right => "right",
        iced::keyboard::Location::Numpad => "numpad",
    }
}

// ---------------------------------------------------------------------------
// Mouse serialization helpers
// ---------------------------------------------------------------------------

pub fn serialize_mouse_button(button: &iced::mouse::Button) -> String {
    match button {
        iced::mouse::Button::Left => "left".to_string(),
        iced::mouse::Button::Right => "right".to_string(),
        iced::mouse::Button::Middle => "middle".to_string(),
        iced::mouse::Button::Back => "back".to_string(),
        iced::mouse::Button::Forward => "forward".to_string(),
        iced::mouse::Button::Other(n) => format!("other_{n}"),
    }
}

pub fn serialize_scroll_delta(delta: &iced::mouse::ScrollDelta) -> (f32, f32, &'static str) {
    match delta {
        iced::mouse::ScrollDelta::Lines { x, y } => (*x, *y, "lines"),
        iced::mouse::ScrollDelta::Pixels { x, y } => (*x, *y, "pixels"),
    }
}
