//! Pure state engine, decoupled from the iced runtime.
//!
//! [`Core`] owns the UI tree, widget caches, and subscription state.
//! It processes [`IncomingMessage`]s and returns [`CoreEffect`]s that
//! the host (the iced `App` or the headless runner) must execute.
//! Core never touches iced directly -- it's pure state management.

use std::collections::HashMap;

use iced::Font;
use serde_json::Value;

use crate::protocol::{IncomingMessage, OutgoingEvent};
use crate::theming;
use crate::tree::Tree;
use crate::widgets::{self, WidgetCaches};

/// Side effects produced by [`Core::apply`] that the host must handle.
///
/// Core is zero-I/O: it never writes to stdout, opens windows, or runs
/// platform operations. Instead it returns these effects as commands for
/// the host (the iced daemon or headless runner) to execute. This keeps
/// Core testable and mode-agnostic.
///
/// Effects are returned in a `Vec` and should be processed in order.
/// Some variants (e.g. `SyncWindows`) may depend on prior tree mutations
/// from the same `apply` call.
#[derive(Debug)]
pub enum CoreEffect {
    /// The window set may have changed -- re-sync with renderer.
    ///
    /// Produced after every Snapshot and Patch that succeeds. The host
    /// should compare `tree.window_ids()` against its open window set
    /// and open/close as needed.
    SyncWindows,

    /// Emit a protocol event to the host process.
    EmitEvent(OutgoingEvent),

    /// Handle a platform effect (file dialog, clipboard, notification).
    ///
    /// Core does not execute effects -- it passes the raw request through
    /// for the host to dispatch. The host decides whether to run the
    /// effect synchronously, asynchronously (via Task::perform), or
    /// return a cancellation (e.g. in headless mode where file dialogs
    /// are unavailable).
    ///
    /// # Known effect kinds
    ///
    /// **Async (file dialogs):** `file_open`, `file_open_multiple`,
    /// `file_save`, `directory_select`, `directory_select_multiple`
    ///
    /// **Sync (clipboard):** `clipboard_read`, `clipboard_write`,
    /// `clipboard_read_html`, `clipboard_write_html`, `clipboard_clear`,
    /// `clipboard_read_primary`, `clipboard_write_primary`
    ///
    /// **Sync (notification):** `notification`
    HandleEffect {
        request_id: String,
        kind: String,
        payload: Value,
    },

    /// Execute a widget operation.
    ///
    /// # Known ops
    ///
    /// `focus`, `focus_next`, `focus_previous`, `scroll_to`, `scroll_by`,
    /// `snap_to`, `snap_to_end`, `select_all`, `select_range`,
    /// `move_cursor_to_front`, `move_cursor_to_end`,
    /// `move_cursor_to_line_start`, `move_cursor_to_line_end`,
    /// `close_window`, `announce`, `exit`, `pane_split`, `pane_close`,
    /// `pane_swap`, `tree_hash`, `list_images`, `clear_images`,
    /// `load_font`, `find_focused`, `system_theme`, `system_info`
    WidgetOp { op: String, payload: Value },

    /// Execute a window operation.
    ///
    /// # Known ops
    ///
    /// `open`, `close`, `update`, `resize`, `move_to`, `maximize`,
    /// `minimize`, `set_mode`, `toggle_maximize`, `toggle_decorations`,
    /// `gain_focus`, `set_level`, `drag`, `drag_resize`,
    /// `request_attention`, `show_system_menu`, `set_resizable`,
    /// `set_min_size`, `set_max_size`, `mouse_passthrough`,
    /// `get_size`, `get_position`, `set_icon`
    WindowOp {
        op: String,
        window_id: String,
        settings: Value,
    },

    /// The global/root theme changed to an explicit value.
    ///
    /// The host should update its cached theme and set
    /// `theme_follows_system = false`.
    ThemeChanged(iced::Theme),

    /// The root theme was set to `"system"` -- the app-level theme
    /// should follow the OS preference.
    ThemeFollowsSystem,

    /// Image operation (create/update/delete in-memory handles).
    ///
    /// # Known ops
    ///
    /// `create_from_bytes`, `create_from_rgba`, `delete`
    ImageOp {
        op: String,
        handle: String,
        data: Option<Vec<u8>>,
        pixels: Option<Vec<u8>>,
        width: Option<u32>,
        height: Option<u32>,
    },

    /// Extension configuration received from the host's Settings message.
    ///
    /// The host should call `dispatcher.init_all(&config, &theme, ...)`
    /// to pass configuration and context to registered extensions.
    ExtensionConfig(Value),
}

/// Pure state core, decoupled from the iced runtime.
///
/// Owns the retained UI tree, widget caches, active subscriptions, and
/// global rendering defaults. The host calls [`apply`](Self::apply) with
/// each incoming message and executes the returned [`CoreEffect`]s.
pub struct Core {
    /// The retained UI tree (snapshots replace it, patches update it).
    pub tree: Tree,
    /// Caches for stateful widgets (text_editor content, markdown items, etc.).
    pub caches: WidgetCaches,
    /// Active event subscriptions: kind -> tag.
    pub active_subscriptions: HashMap<String, String>,
    /// Per-subscription max_rate values from Subscribe messages.
    pub subscription_rates: HashMap<String, Option<u32>>,
    /// Global default event rate from Settings (events per second).
    /// None = no limit (full speed).
    pub default_event_rate: Option<u32>,
    /// Global default text size from Settings.
    pub default_text_size: Option<f32>,
    /// Global default font from Settings.
    pub default_font: Option<Font>,
    /// Cached resolved theme from the root node's `theme` prop.
    /// Only re-resolved when the raw JSON value changes.
    pub cached_theme: Option<iced::Theme>,
    /// Content hash of the last resolved theme prop, used for change
    /// detection. Replaces the previous `to_string()` approach which
    /// allocated and compared a full JSON string on every check.
    cached_theme_hash: Option<u64>,
    /// True after the first Settings message has been applied. Used to
    /// suppress warnings about startup-only fields on the initial Settings.
    settings_applied: bool,
}

impl Default for Core {
    fn default() -> Self {
        Self::new()
    }
}

impl Core {
    pub fn new() -> Self {
        Self {
            tree: Tree::new(),
            caches: WidgetCaches::new(),
            active_subscriptions: HashMap::new(),
            subscription_rates: HashMap::new(),
            default_event_rate: None,
            default_text_size: None,
            default_font: None,
            cached_theme: None,
            cached_theme_hash: None,
            settings_applied: false,
        }
    }

    /// Compute a SHA-256 hash of the current tree (serialized as JSON).
    /// Returns the hex-encoded hash string, or an empty string if no tree.
    pub fn tree_hash(&self) -> String {
        use sha2::{Digest, Sha256};
        match &self.tree.root() {
            Some(root) => {
                let json = match serde_json::to_string(root) {
                    Ok(s) => s,
                    Err(e) => {
                        log::error!("tree_hash: serialization failed: {e}");
                        return "SERIALIZATION_ERROR".to_string();
                    }
                };
                let hash = Sha256::digest(json.as_bytes());
                format!("{:x}", hash)
            }
            None => String::new(),
        }
    }

    /// Resolve and cache a theme from a JSON prop value. Only re-resolves
    /// when the serialized JSON differs from the cached version.
    fn resolve_and_cache_theme(
        &mut self,
        theme_val: &serde_json::Value,
        effects: &mut Vec<CoreEffect>,
    ) {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::Hasher;

        let mut hasher = DefaultHasher::new();
        crate::widgets::hash_json_value(theme_val, &mut hasher);
        let hash = hasher.finish();

        if self.cached_theme_hash == Some(hash) {
            // Theme prop unchanged -- skip resolution.
            return;
        }
        self.cached_theme_hash = Some(hash);
        match theming::resolve_theme_only(theme_val) {
            Some(theme) => {
                self.cached_theme = Some(theme.clone());
                effects.push(CoreEffect::ThemeChanged(theme));
            }
            None => {
                self.cached_theme = None;
                effects.push(CoreEffect::ThemeFollowsSystem);
            }
        }
    }

    /// Process an incoming message, mutate state, return effects.
    pub fn apply(&mut self, message: IncomingMessage) -> Vec<CoreEffect> {
        let mut effects = Vec::new();

        match message {
            IncomingMessage::Snapshot { tree } => {
                log::debug!("snapshot received (root id={})", tree.id);
                if let Some(theme_val) = tree.props.get("theme") {
                    self.resolve_and_cache_theme(theme_val, &mut effects);
                }
                if let Err(duplicates) = self.tree.snapshot(tree) {
                    let dup_list = duplicates.join(", ");
                    log::error!("snapshot contains duplicate node IDs: {dup_list}");
                    effects.push(CoreEffect::EmitEvent(OutgoingEvent::generic(
                        "error".to_string(),
                        "duplicate_node_ids".to_string(),
                        Some(serde_json::json!({
                            "error": "snapshot contains duplicate node IDs",
                            "duplicates": duplicates,
                        })),
                    )));
                }
                // Clear built-in caches but NOT extension caches. Extension
                // cleanup callbacks run later via prepare_all() in the host,
                // which needs the old cache entries to still be accessible.
                self.caches.clear_builtin();
                if let Some(root) = self.tree.root() {
                    widgets::ensure_caches(root, &mut self.caches);
                }
                effects.push(CoreEffect::SyncWindows);
            }
            IncomingMessage::Patch { ops } => {
                log::debug!("patch received ({} ops)", ops.len());
                self.tree.apply_patch(ops);
                // Re-check root theme prop in case a patch changed it.
                if let Some(root) = self.tree.root()
                    && let Some(theme_val) = root.props.get("theme")
                {
                    let theme_val = theme_val.clone();
                    self.resolve_and_cache_theme(&theme_val, &mut effects);
                }
                if let Some(root) = self.tree.root() {
                    widgets::ensure_caches(root, &mut self.caches);
                }
                effects.push(CoreEffect::SyncWindows);
            }
            IncomingMessage::Effect { id, kind, payload } => {
                log::debug!("effect request: {kind} ({id})");
                effects.push(CoreEffect::HandleEffect {
                    request_id: id,
                    kind,
                    payload,
                });
            }
            IncomingMessage::WidgetOp { op, payload } => {
                log::debug!("widget_op: {op}");
                effects.push(CoreEffect::WidgetOp { op, payload });
            }
            IncomingMessage::Subscribe {
                kind,
                tag,
                max_rate,
            } => {
                log::debug!("subscription register: {kind} -> {tag}");
                if let Some(old_tag) = self.active_subscriptions.insert(kind.clone(), tag.clone())
                    && old_tag != tag
                {
                    log::warn!(
                        "subscription `{kind}` re-registered with tag `{tag}` \
                         (was `{old_tag}`); previous handler replaced"
                    );
                }
                match max_rate {
                    Some(rate) => {
                        log::debug!("subscription `{kind}` max_rate: {rate}");
                        self.subscription_rates.insert(kind, Some(rate));
                    }
                    None => {
                        // Re-subscribing without max_rate clears any
                        // previously-set rate for this subscription.
                        self.subscription_rates.remove(&kind);
                    }
                }
            }
            IncomingMessage::Unsubscribe { kind } => {
                log::debug!("subscription unregister: {kind}");
                self.active_subscriptions.remove(&kind);
                self.subscription_rates.remove(&kind);
            }
            IncomingMessage::WindowOp {
                op,
                window_id,
                settings,
            } => {
                log::debug!("window_op: {op} ({window_id})");
                effects.push(CoreEffect::WindowOp {
                    op,
                    window_id,
                    settings,
                });
            }
            IncomingMessage::Settings { settings } => {
                log::debug!("settings received");

                // Protocol version check
                if let Some(v) = settings.get("protocol_version").and_then(|v| v.as_u64()) {
                    if v != u64::from(crate::protocol::PROTOCOL_VERSION) {
                        log::error!(
                            "protocol version mismatch: expected {}, got {}",
                            crate::protocol::PROTOCOL_VERSION,
                            v
                        );
                    }
                } else {
                    log::error!("no protocol_version in Settings, assuming compatible");
                }

                // Startup-only fields are extracted by run.rs before the
                // daemon starts. Subsequent Settings messages can't change
                // them -- warn so hosts notice the no-op.
                if self.settings_applied {
                    for field in &["antialiasing", "vsync", "fonts", "scale_factor"] {
                        if settings.get(*field).is_some() {
                            log::warn!(
                                "Settings field `{field}` is startup-only; \
                                 ignored after the daemon has started"
                            );
                        }
                    }
                }
                self.settings_applied = true;

                self.default_event_rate = settings
                    .get("default_event_rate")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u32);
                self.default_text_size = settings
                    .get("default_text_size")
                    .and_then(|v| v.as_f64())
                    .map(crate::prop_helpers::f64_to_f32);
                self.default_font = settings.get("default_font").map(|v| {
                    let family = v.get("family").and_then(|f| f.as_str());
                    match family {
                        Some("monospace") => Font::MONOSPACE,
                        Some(other) => {
                            log::warn!(
                                "unsupported default_font family `{other}`, \
                                 using system default"
                            );
                            Font::DEFAULT
                        }
                        None => Font::DEFAULT,
                    }
                });
                if let Some(ext_config) = settings.get("extension_config") {
                    effects.push(CoreEffect::ExtensionConfig(ext_config.clone()));
                }
            }
            IncomingMessage::ImageOp {
                op,
                handle,
                data,
                pixels,
                width,
                height,
            } => {
                log::debug!("image_op: {op} ({handle})");
                effects.push(CoreEffect::ImageOp {
                    op,
                    handle,
                    data,
                    pixels,
                    width,
                    height,
                });
            }
            // Scripting messages handled by the renderer binary (daemon /
            // headless), not by Core. Listed explicitly so adding a new
            // IncomingMessage variant produces a compile error here instead
            // of silently falling through a catch-all `_` arm.
            IncomingMessage::Query { .. } => {
                log::debug!("Query message ignored by Core (handled by scripting layer)");
            }
            IncomingMessage::Interact { .. } => {
                log::debug!("Interact message ignored by Core (handled by scripting layer)");
            }
            IncomingMessage::TreeHash { .. } => {
                log::debug!("TreeHash message ignored by Core (handled by scripting layer)");
            }
            IncomingMessage::Screenshot { .. } => {
                log::debug!("Screenshot message ignored by Core (handled by scripting layer)");
            }
            IncomingMessage::Reset { .. } => {
                log::debug!("Reset message ignored by Core (handled by scripting layer)");
            }
            IncomingMessage::ExtensionCommand { .. } => {
                log::debug!("ExtensionCommand message ignored by Core (handled by renderer App)");
            }
            IncomingMessage::AdvanceFrame { .. } => {
                log::warn!(
                    "AdvanceFrame is only supported in headless/test mode; ignored in daemon mode"
                );
            }
            IncomingMessage::ExtensionCommands { .. } => {
                log::debug!("ExtensionCommands message ignored by Core (handled by renderer App)");
            }
        }

        effects
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{IncomingMessage, TreeNode};
    use crate::testing::{node as make_node, node_with_props as make_node_with_props};

    // -- Core::new() --

    #[test]
    fn new_returns_empty_tree() {
        let core = Core::new();
        assert!(core.tree.root().is_none());
    }

    #[test]
    fn new_has_empty_active_subscriptions() {
        let core = Core::new();
        assert!(core.active_subscriptions.is_empty());
    }

    #[test]
    fn new_has_no_default_text_size() {
        let core = Core::new();
        assert!(core.default_text_size.is_none());
    }

    #[test]
    fn new_has_no_default_font() {
        let core = Core::new();
        assert!(core.default_font.is_none());
    }

    // -- Snapshot --

    #[test]
    fn snapshot_sets_tree_and_returns_sync_windows() {
        let mut core = Core::new();
        let msg = IncomingMessage::Snapshot {
            tree: make_node("root", "column"),
        };
        let effects = core.apply(msg);
        // Tree should be populated
        assert!(core.tree.root().is_some());
        assert_eq!(core.tree.root().unwrap().id, "root");
        // Must include SyncWindows
        let has_sync = effects.iter().any(|e| matches!(e, CoreEffect::SyncWindows));
        assert!(has_sync);
    }

    #[test]
    fn snapshot_with_theme_prop_returns_theme_changed() {
        let mut core = Core::new();
        let msg = IncomingMessage::Snapshot {
            tree: make_node_with_props("root", "column", serde_json::json!({"theme": "dark"})),
        };
        let effects = core.apply(msg);
        let has_theme = effects
            .iter()
            .any(|e| matches!(e, CoreEffect::ThemeChanged(_)));
        assert!(has_theme);
    }

    #[test]
    fn snapshot_without_theme_prop_has_no_theme_changed() {
        let mut core = Core::new();
        let msg = IncomingMessage::Snapshot {
            tree: make_node("root", "column"),
        };
        let effects = core.apply(msg);
        let has_theme = effects
            .iter()
            .any(|e| matches!(e, CoreEffect::ThemeChanged(_)));
        assert!(!has_theme);
    }

    // -- Patch --

    #[test]
    fn patch_with_no_ops_returns_sync_windows() {
        let mut core = Core::new();
        // First put a tree in place so patch has something to work with
        let snapshot_msg = IncomingMessage::Snapshot {
            tree: make_node("root", "column"),
        };
        core.apply(snapshot_msg);

        let patch_msg = IncomingMessage::Patch { ops: vec![] };
        let effects = core.apply(patch_msg);
        let has_sync = effects.iter().any(|e| matches!(e, CoreEffect::SyncWindows));
        assert!(has_sync);
    }

    // -- Settings --

    #[test]
    fn settings_sets_default_text_size() {
        let mut core = Core::new();
        let msg = IncomingMessage::Settings {
            settings: serde_json::json!({"default_text_size": 18.0}),
        };
        core.apply(msg);
        assert_eq!(core.default_text_size, Some(18.0_f32));
    }

    #[test]
    fn settings_sets_default_font_monospace() {
        let mut core = Core::new();
        let msg = IncomingMessage::Settings {
            settings: serde_json::json!({"default_font": {"family": "monospace"}}),
        };
        core.apply(msg);
        assert_eq!(core.default_font, Some(iced::Font::MONOSPACE));
    }

    #[test]
    fn settings_sets_default_font_default_for_unknown_family() {
        let mut core = Core::new();
        let msg = IncomingMessage::Settings {
            settings: serde_json::json!({"default_font": {"family": "sans-serif"}}),
        };
        core.apply(msg);
        assert_eq!(core.default_font, Some(iced::Font::DEFAULT));
    }

    #[test]
    fn settings_sets_default_event_rate() {
        let mut core = Core::new();
        let msg = IncomingMessage::Settings {
            settings: serde_json::json!({"default_event_rate": 60}),
        };
        core.apply(msg);
        assert_eq!(core.default_event_rate, Some(60));
    }

    #[test]
    fn settings_without_default_event_rate_leaves_none() {
        let mut core = Core::new();
        let msg = IncomingMessage::Settings {
            settings: serde_json::json!({"default_text_size": 14.0}),
        };
        core.apply(msg);
        assert_eq!(core.default_event_rate, None);
    }

    #[test]
    fn subscribe_with_max_rate_stores_rate() {
        let mut core = Core::new();
        let msg = IncomingMessage::Subscribe {
            kind: "on_mouse_move".to_string(),
            tag: "mouse".to_string(),
            max_rate: Some(30),
        };
        core.apply(msg);
        assert_eq!(
            core.subscription_rates.get("on_mouse_move"),
            Some(&Some(30))
        );
    }

    #[test]
    fn subscribe_without_max_rate_does_not_store_rate() {
        let mut core = Core::new();
        let msg = IncomingMessage::Subscribe {
            kind: "on_key_press".to_string(),
            tag: "keys".to_string(),
            max_rate: None,
        };
        core.apply(msg);
        assert!(!core.subscription_rates.contains_key("on_key_press"));
    }

    #[test]
    fn unsubscribe_removes_subscription_rate() {
        let mut core = Core::new();
        core.apply(IncomingMessage::Subscribe {
            kind: "on_mouse_move".to_string(),
            tag: "mouse".to_string(),
            max_rate: Some(30),
        });
        core.apply(IncomingMessage::Unsubscribe {
            kind: "on_mouse_move".to_string(),
        });
        assert!(!core.subscription_rates.contains_key("on_mouse_move"));
    }

    #[test]
    fn settings_without_extension_config_returns_no_effects() {
        let mut core = Core::new();
        let msg = IncomingMessage::Settings {
            settings: serde_json::json!({"default_text_size": 14.0}),
        };
        let effects = core.apply(msg);
        assert!(effects.is_empty());
    }

    #[test]
    fn settings_with_extension_config_emits_effect() {
        let mut core = Core::new();
        let msg = IncomingMessage::Settings {
            settings: serde_json::json!({
                "default_text_size": 14.0,
                "extension_config": {
                    "terminal": {"shell": "/bin/bash"}
                }
            }),
        };
        let effects = core.apply(msg);
        let has_ext_config = effects
            .iter()
            .any(|e| matches!(e, CoreEffect::ExtensionConfig(_)));
        assert!(has_ext_config);
    }

    #[test]
    fn settings_with_extension_config_contains_correct_value() {
        let mut core = Core::new();
        let config_val = serde_json::json!({"terminal": {"shell": "/bin/zsh"}});
        let msg = IncomingMessage::Settings {
            settings: serde_json::json!({
                "extension_config": config_val,
            }),
        };
        let effects = core.apply(msg);
        let ext_config = effects.iter().find_map(|e| match e {
            CoreEffect::ExtensionConfig(v) => Some(v),
            _ => None,
        });
        assert_eq!(
            ext_config.unwrap(),
            &serde_json::json!({"terminal": {"shell": "/bin/zsh"}})
        );
    }

    // -- Subscribe / Unsubscribe --

    #[test]
    fn subscription_register_adds_to_active_subscriptions() {
        let mut core = Core::new();
        let msg = IncomingMessage::Subscribe {
            kind: "time".to_string(),
            tag: "tick".to_string(),
            max_rate: None,
        };
        core.apply(msg);
        assert_eq!(
            core.active_subscriptions.get("time").map(|s| s.as_str()),
            Some("tick")
        );
    }

    #[test]
    fn subscription_register_returns_no_effects() {
        let mut core = Core::new();
        let msg = IncomingMessage::Subscribe {
            kind: "keyboard".to_string(),
            tag: "key".to_string(),
            max_rate: None,
        };
        let effects = core.apply(msg);
        assert!(effects.is_empty());
    }

    #[test]
    fn subscription_unregister_removes_from_active_subscriptions() {
        let mut core = Core::new();
        core.active_subscriptions
            .insert("time".to_string(), "tick".to_string());
        let msg = IncomingMessage::Unsubscribe {
            kind: "time".to_string(),
        };
        core.apply(msg);
        assert!(!core.active_subscriptions.contains_key("time"));
    }

    #[test]
    fn subscription_unregister_returns_no_effects() {
        let mut core = Core::new();
        let msg = IncomingMessage::Unsubscribe {
            kind: "time".to_string(),
        };
        let effects = core.apply(msg);
        assert!(effects.is_empty());
    }

    // -- Unhandled message types --

    #[test]
    fn unhandled_message_returns_empty_effects() {
        let mut core = Core::new();
        // Query is handled by the scripting layer, not Core -- hits the catch-all
        let msg = IncomingMessage::Query {
            id: "q1".to_string(),
            target: "tree".to_string(),
            selector: Value::Null,
        };
        let effects = core.apply(msg);
        assert!(effects.is_empty());
    }

    // -- Snapshot preserves extension caches for prepare_all --

    #[test]
    fn snapshot_preserves_extension_caches() {
        let mut core = Core::new();

        // Simulate extension storing data in extension caches.
        core.caches.extension.insert("ext", "node-1", 42u32);

        // Snapshot replaces the tree.
        let msg = IncomingMessage::Snapshot {
            tree: make_node("root", "column"),
        };
        core.apply(msg);

        // Extension caches must survive -- clear_builtin() must NOT
        // wipe them. The host calls prepare_all() after apply() to
        // handle extension cleanup properly.
        assert_eq!(core.caches.extension.get::<u32>("ext", "node-1"), Some(&42));
    }

    #[test]
    fn snapshot_clears_builtin_caches() {
        let mut core = Core::new();

        // Populate a built-in cache by applying a snapshot with a text_editor.
        let editor_node = make_node_with_props(
            "ed1",
            "text_editor",
            serde_json::json!({"content": "hello"}),
        );
        let mut root = make_node("root", "column");
        root.children.push(editor_node);
        core.apply(IncomingMessage::Snapshot { tree: root });
        assert!(core.caches.editor_contents.contains_key("ed1"));

        // Second snapshot without the editor -- built-in caches should
        // be cleared and repopulated (without the editor).
        core.apply(IncomingMessage::Snapshot {
            tree: make_node("root2", "column"),
        });
        assert!(!core.caches.editor_contents.contains_key("ed1"));
    }

    // -- Multi-window sequence -----------------------------------------------

    fn make_window_node(id: &str) -> TreeNode {
        TreeNode {
            id: id.to_string(),
            type_name: "window".to_string(),
            props: serde_json::json!({}),
            children: vec![],
        }
    }

    #[test]
    fn multi_window_snapshot_two_windows_produces_sync_windows() {
        let mut core = Core::new();
        let mut root = make_node("root", "column");
        root.children.push(make_window_node("win-a"));
        root.children.push(make_window_node("win-b"));

        let effects = core.apply(IncomingMessage::Snapshot { tree: root });

        let has_sync = effects.iter().any(|e| matches!(e, CoreEffect::SyncWindows));
        assert!(has_sync, "Snapshot with windows should produce SyncWindows");

        // Verify the tree has both windows.
        let ids = core.tree.window_ids();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"win-a".to_string()));
        assert!(ids.contains(&"win-b".to_string()));
    }

    #[test]
    fn multi_window_second_snapshot_removes_window() {
        let mut core = Core::new();

        // First snapshot: two windows.
        let mut root1 = make_node("root", "column");
        root1.children.push(make_window_node("win-a"));
        root1.children.push(make_window_node("win-b"));
        core.apply(IncomingMessage::Snapshot { tree: root1 });
        assert_eq!(core.tree.window_ids().len(), 2);

        // Second snapshot: only one window.
        let mut root2 = make_node("root", "column");
        root2.children.push(make_window_node("win-a"));
        let effects = core.apply(IncomingMessage::Snapshot { tree: root2 });

        let has_sync = effects.iter().any(|e| matches!(e, CoreEffect::SyncWindows));
        assert!(has_sync, "Second Snapshot should produce SyncWindows");

        let ids = core.tree.window_ids();
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0], "win-a");
    }

    #[test]
    fn multi_window_snapshot_then_add_window_via_second_snapshot() {
        let mut core = Core::new();

        // First: one window.
        let mut root1 = make_node("root", "column");
        root1.children.push(make_window_node("win-a"));
        core.apply(IncomingMessage::Snapshot { tree: root1 });
        assert_eq!(core.tree.window_ids().len(), 1);

        // Second: three windows.
        let mut root2 = make_node("root", "column");
        root2.children.push(make_window_node("win-a"));
        root2.children.push(make_window_node("win-b"));
        root2.children.push(make_window_node("win-c"));
        let effects = core.apply(IncomingMessage::Snapshot { tree: root2 });

        let has_sync = effects.iter().any(|e| matches!(e, CoreEffect::SyncWindows));
        assert!(has_sync);

        let ids = core.tree.window_ids();
        assert_eq!(ids.len(), 3);
    }

    // -- Duplicate node ID detection --

    #[test]
    fn snapshot_with_duplicate_ids_emits_error_event() {
        let mut core = Core::new();
        let mut root = make_node("root", "column");
        root.children.push(make_node("dupe", "text"));
        root.children.push(make_node("dupe", "button"));

        let effects = core.apply(IncomingMessage::Snapshot { tree: root });
        let has_error = effects.iter().any(|e| match e {
            CoreEffect::EmitEvent(ev) => ev.family == "error",
            _ => false,
        });
        assert!(has_error, "duplicate IDs should produce an error event");
        // Tree should still be accepted
        assert!(core.tree.root().is_some());
    }

    #[test]
    fn snapshot_without_duplicates_has_no_error_event() {
        let mut core = Core::new();
        let mut root = make_node("root", "column");
        root.children.push(make_node("a", "text"));
        root.children.push(make_node("b", "button"));

        let effects = core.apply(IncomingMessage::Snapshot { tree: root });
        let has_error = effects.iter().any(|e| match e {
            CoreEffect::EmitEvent(ev) => ev.family == "error",
            _ => false,
        });
        assert!(!has_error, "unique IDs should not produce an error event");
    }
}

// ---------------------------------------------------------------------------
// Extension dispatch + caches integration tests
//
// These verify that the EventResult::Consumed path correctly preserves
// extension cache mutations -- the underlying mechanism that makes
// Task::none() safe in the renderer's Message::Event handler.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod extension_event_tests {
    use iced::{Element, Theme};
    use serde_json::{Value, json};

    use crate::extensions::{
        EventResult, ExtensionCaches, ExtensionDispatcher, GenerationCounter, WidgetEnv,
        WidgetExtension,
    };
    use crate::message::Message;
    use crate::protocol::{OutgoingEvent, TreeNode};

    /// A test extension that bumps a GenerationCounter and mutates a
    /// cache entry on every Consumed event.
    struct CountingExtension;

    impl WidgetExtension for CountingExtension {
        fn type_names(&self) -> &[&str] {
            &["counter_widget"]
        }

        fn config_key(&self) -> &str {
            "counting"
        }

        fn prepare(&mut self, node: &TreeNode, caches: &mut ExtensionCaches, _theme: &Theme) {
            // Seed initial state if absent.
            caches.get_or_insert(self.config_key(), &node.id, GenerationCounter::new);
        }

        fn render<'a>(&self, _node: &'a TreeNode, _env: &WidgetEnv<'a>) -> Element<'a, Message> {
            iced::widget::text("test").into()
        }

        fn handle_event(
            &mut self,
            node_id: &str,
            _family: &str,
            _data: &Value,
            caches: &mut ExtensionCaches,
        ) -> EventResult {
            // Mutate caches and return Consumed with no events -- the
            // scenario that was suspected of suppressing redraws.
            if let Some(counter) = caches.get_mut::<GenerationCounter>(self.config_key(), node_id) {
                counter.bump();
            }
            EventResult::Consumed(vec![])
        }
    }

    /// Another test extension that returns Observed with synthetic events.
    struct ObservingExtension;

    impl WidgetExtension for ObservingExtension {
        fn type_names(&self) -> &[&str] {
            &["observer_widget"]
        }

        fn config_key(&self) -> &str {
            "observing"
        }

        fn prepare(&mut self, node: &TreeNode, caches: &mut ExtensionCaches, _theme: &Theme) {
            caches.get_or_insert(self.config_key(), &node.id, GenerationCounter::new);
        }

        fn render<'a>(&self, _node: &'a TreeNode, _env: &WidgetEnv<'a>) -> Element<'a, Message> {
            iced::widget::text("test").into()
        }

        fn handle_event(
            &mut self,
            node_id: &str,
            _family: &str,
            _data: &Value,
            caches: &mut ExtensionCaches,
        ) -> EventResult {
            if let Some(counter) = caches.get_mut::<GenerationCounter>(self.config_key(), node_id) {
                counter.bump();
            }
            EventResult::Observed(vec![OutgoingEvent::generic(
                "viewport".to_string(),
                node_id.to_string(),
                Some(json!({"zoom": 1.5})),
            )])
        }
    }

    fn make_tree(id: &str, type_name: &str) -> TreeNode {
        TreeNode {
            id: id.to_string(),
            type_name: type_name.to_string(),
            props: json!({}),
            children: vec![],
        }
    }

    // -- Consumed with empty events mutates caches --------------------------

    #[test]
    fn consumed_empty_events_still_mutates_caches() {
        let ext: Box<dyn WidgetExtension> = Box::new(CountingExtension);
        let mut dispatcher = ExtensionDispatcher::new(vec![ext]);
        let mut caches = ExtensionCaches::new();
        let root = make_tree("cw-1", "counter_widget");

        // prepare registers the node and seeds the cache
        dispatcher.prepare_all(&root, &mut caches, &Theme::Dark);
        assert_eq!(
            caches
                .get::<GenerationCounter>("counting", "cw-1")
                .unwrap()
                .get(),
            0
        );

        // handle_event with Consumed(vec![]) modifies caches
        let result = dispatcher.handle_event("cw-1", "click", &Value::Null, &mut caches);
        assert!(matches!(result, EventResult::Consumed(ref v) if v.is_empty()));

        // Cache mutation is visible -- generation was bumped
        assert_eq!(
            caches
                .get::<GenerationCounter>("counting", "cw-1")
                .unwrap()
                .get(),
            1
        );
    }

    #[test]
    fn consumed_caches_accumulate_across_multiple_events() {
        let ext: Box<dyn WidgetExtension> = Box::new(CountingExtension);
        let mut dispatcher = ExtensionDispatcher::new(vec![ext]);
        let mut caches = ExtensionCaches::new();
        let root = make_tree("cw-1", "counter_widget");

        dispatcher.prepare_all(&root, &mut caches, &Theme::Dark);

        for _ in 0..5 {
            let _ = dispatcher.handle_event("cw-1", "click", &Value::Null, &mut caches);
        }

        assert_eq!(
            caches
                .get::<GenerationCounter>("counting", "cw-1")
                .unwrap()
                .get(),
            5
        );
    }

    // -- Observed returns events AND mutates caches -------------------------

    #[test]
    fn observed_mutates_caches_and_returns_events() {
        let ext: Box<dyn WidgetExtension> = Box::new(ObservingExtension);
        let mut dispatcher = ExtensionDispatcher::new(vec![ext]);
        let mut caches = ExtensionCaches::new();
        let root = make_tree("ow-1", "observer_widget");

        dispatcher.prepare_all(&root, &mut caches, &Theme::Dark);

        let result = dispatcher.handle_event("ow-1", "pan", &Value::Null, &mut caches);
        match result {
            EventResult::Observed(events) => {
                assert_eq!(events.len(), 1);
            }
            other => panic!("expected Observed, got {:?}", variant_name(&other)),
        }

        assert_eq!(
            caches
                .get::<GenerationCounter>("observing", "ow-1")
                .unwrap()
                .get(),
            1
        );
    }

    // -- PassThrough for unknown nodes --------------------------------------

    #[test]
    fn unknown_node_returns_passthrough() {
        let ext: Box<dyn WidgetExtension> = Box::new(CountingExtension);
        let mut dispatcher = ExtensionDispatcher::new(vec![ext]);
        let mut caches = ExtensionCaches::new();

        // Don't call prepare_all -- no node registered
        let result = dispatcher.handle_event("nonexistent", "click", &Value::Null, &mut caches);
        assert!(matches!(result, EventResult::PassThrough));
    }

    // -- GenerationCounter as redraw signal ---------------------------------

    #[test]
    fn generation_counter_detects_stale_state() {
        let mut counter = GenerationCounter::new();
        let saved = counter.get();
        assert_eq!(saved, 0);

        counter.bump();
        assert_ne!(counter.get(), saved, "generation should differ after bump");

        // Simulates the pattern in canvas::Program::draw -- compare saved
        // value to current, clear cache if they differ.
        let needs_redraw = counter.get() != saved;
        assert!(needs_redraw);
    }

    fn variant_name(result: &EventResult) -> &'static str {
        match result {
            EventResult::PassThrough => "PassThrough",
            EventResult::Consumed(_) => "Consumed",
            EventResult::Observed(_) => "Observed",
        }
    }
}
