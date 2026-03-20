//! String constants for the stdin/stdout protocol.
//!
//! Subscription keys, default values, and other protocol constants
//! used across the renderer module. Centralised here so typos are
//! caught at compile time and the full set is discoverable.

// -- Subscription keys -------------------------------------------------------

/// Catch-all event subscription (keyboard, mouse, touch, IME).
pub(crate) const SUB_EVENT: &str = "on_event";

pub(crate) const SUB_KEY_PRESS: &str = "on_key_press";
pub(crate) const SUB_KEY_RELEASE: &str = "on_key_release";
pub(crate) const SUB_MODIFIERS_CHANGED: &str = "on_modifiers_changed";

pub(crate) const SUB_MOUSE_MOVE: &str = "on_mouse_move";
pub(crate) const SUB_MOUSE_BUTTON: &str = "on_mouse_button";
pub(crate) const SUB_MOUSE_SCROLL: &str = "on_mouse_scroll";

pub(crate) const SUB_TOUCH: &str = "on_touch";

pub(crate) const SUB_IME: &str = "on_ime";

/// Catch-all window lifecycle subscription.
pub(crate) const SUB_WINDOW_EVENT: &str = "on_window_event";
pub(crate) const SUB_WINDOW_OPEN: &str = "on_window_open";
pub(crate) const SUB_WINDOW_CLOSE: &str = "on_window_close";
pub(crate) const SUB_WINDOW_MOVE: &str = "on_window_move";
pub(crate) const SUB_WINDOW_RESIZE: &str = "on_window_resize";
pub(crate) const SUB_WINDOW_FOCUS: &str = "on_window_focus";
pub(crate) const SUB_WINDOW_UNFOCUS: &str = "on_window_unfocus";

pub(crate) const SUB_FILE_DROP: &str = "on_file_drop";
pub(crate) const SUB_ANIMATION_FRAME: &str = "on_animation_frame";
pub(crate) const SUB_THEME_CHANGE: &str = "on_theme_change";

// -- Defaults ----------------------------------------------------------------

pub(crate) const DEFAULT_WINDOW_TITLE: &str = "toddy";

/// Default theme when no theme is specified in Settings or after a Reset.
pub(crate) const DEFAULT_THEME: iced::Theme = iced::Theme::Dark;

/// Maximum decoded font data size for runtime `load_font` operations.
/// Font files are typically under 1 MB; large CJK fonts top out around
/// 15-17 MB. Anything beyond this limit is rejected as likely not a font.
pub(crate) const MAX_FONT_BYTES: usize = 16 * 1024 * 1024;
