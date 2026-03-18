//! String constants for the stdin/stdout protocol.
//!
//! Subscription keys, default values, and other protocol constants
//! used across the renderer module. Centralised here so typos are
//! caught at compile time and the full set is discoverable.

// -- Subscription keys -------------------------------------------------------

/// Catch-all event subscription (keyboard, mouse, touch, IME).
pub(super) const SUB_EVENT: &str = "on_event";

pub(super) const SUB_KEY_PRESS: &str = "on_key_press";
pub(super) const SUB_KEY_RELEASE: &str = "on_key_release";
pub(super) const SUB_MODIFIERS_CHANGED: &str = "on_modifiers_changed";

pub(super) const SUB_MOUSE_MOVE: &str = "on_mouse_move";
pub(super) const SUB_MOUSE_BUTTON: &str = "on_mouse_button";
pub(super) const SUB_MOUSE_SCROLL: &str = "on_mouse_scroll";

pub(super) const SUB_TOUCH: &str = "on_touch";

pub(super) const SUB_IME: &str = "on_ime";

/// Catch-all window lifecycle subscription.
pub(super) const SUB_WINDOW_EVENT: &str = "on_window_event";
pub(super) const SUB_WINDOW_OPEN: &str = "on_window_open";
pub(super) const SUB_WINDOW_CLOSE: &str = "on_window_close";
pub(super) const SUB_WINDOW_MOVE: &str = "on_window_move";
pub(super) const SUB_WINDOW_RESIZE: &str = "on_window_resize";
pub(super) const SUB_WINDOW_FOCUS: &str = "on_window_focus";
pub(super) const SUB_WINDOW_UNFOCUS: &str = "on_window_unfocus";

pub(super) const SUB_FILE_DROP: &str = "on_file_drop";
pub(super) const SUB_ANIMATION_FRAME: &str = "on_animation_frame";
pub(super) const SUB_THEME_CHANGE: &str = "on_theme_change";

// -- Defaults ----------------------------------------------------------------

pub(super) const DEFAULT_WINDOW_TITLE: &str = "julep";
