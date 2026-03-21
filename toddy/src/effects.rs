//! Platform effect handlers (file dialogs, clipboard, notifications).
//!
//! Effects are side-effectful operations requested by the host that
//! interact with OS resources. Each effect has an `id` for correlating
//! the response, a `kind` string for dispatch, and a JSON `payload`
//! with kind-specific parameters.
//!
//! File dialog effects run asynchronously via [`handle_async_effect`]
//! when a tokio runtime is available (the normal iced daemon path).
//! The sync [`handle_effect`] fallback exists for headless/blocking
//! contexts. Clipboard and notification effects are always synchronous.
//!
//! # Platform notes
//!
//! **File paths:** Returned paths use OS-native separators (`/` on Unix,
//! `\` on Windows). On macOS, paths may arrive in NFD (decomposed Unicode)
//! form. Hosts should normalize paths for comparison if needed.
//!
//! **File dialogs (rfd):** On Wayland, rfd uses the xdg-desktop-portal.
//! On compositors without a portal service, dialogs return None (cancelled).
//! On X11 without a portal, rfd falls back to GTK dialogs which may block
//! a tokio worker thread. Filter extensions should be simple (e.g. "png",
//! "jpg") without wildcards for best cross-platform compatibility.
//!
//! **Clipboard (arboard):** The clipboard instance is lazily initialized
//! in a static Mutex and may be created from a worker thread. On Wayland,
//! arboard spawns a background thread for clipboard serving -- dropping the
//! Clipboard would lose served data, so it persists for process lifetime.
//! On Linux, primary selection is routed via `LinuxClipboardKind::Primary`.
//!
//! **Notifications (notify-rust):** On macOS, notifications require an app
//! bundle identifier (bare binaries may fail). The `icon` field only works
//! on Linux (freedesktop icon name); on macOS it is ignored, on Windows
//! the app icon is used instead. The `urgency` field is Linux-only
//! (freedesktop notification spec).

use serde_json::{Value, json};

use toddy_core::protocol::EffectResponse;

/// Convert a file path to a JSON string value, logging a warning if the path
/// contains non-UTF-8 bytes and lossy conversion is required.
///
/// **Platform notes:** Windows UNC paths (`\\?\C:\...`) are valid UTF-8 and
/// pass through cleanly. macOS HFS+ paths may arrive in NFD (decomposed
/// Unicode) form -- this is valid UTF-8 but the host should normalize for
/// comparison. Non-UTF-8 filenames are rare on modern systems (NTFS is
/// UTF-16, HFS+ is UTF-8, ext4 allows arbitrary bytes but tooling
/// discourages it).
fn path_to_json_string(path: &std::path::Path) -> String {
    match path.to_str() {
        Some(s) => s.to_string(),
        None => {
            log::warn!(
                "file path contains non-UTF-8 bytes, using lossy conversion: {}",
                path.display()
            );
            path.to_string_lossy().into_owned()
        }
    }
}

// ---------------------------------------------------------------------------
// Dialog parameter parsing
// ---------------------------------------------------------------------------

/// Parsed file dialog parameters extracted from the JSON payload.
struct DialogParams<'a> {
    title: &'a str,
    filters: Vec<(&'a str, Vec<&'a str>)>,
    directory: Option<&'a str>,
    default_name: Option<&'a str>,
}

/// Parse common dialog parameters from a JSON payload.
fn parse_dialog_params<'a>(payload: &'a Value, default_title: &'a str) -> DialogParams<'a> {
    let title = payload
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or(default_title);

    let mut filters = Vec::new();
    if let Some(arr) = payload.get("filters").and_then(|v| v.as_array()) {
        for filter in arr {
            if let Some(pair) = filter.as_array()
                && pair.len() >= 2
                && let (Some(name), Some(ext)) = (pair[0].as_str(), pair[1].as_str())
            {
                let extensions: Vec<&str> = ext
                    .split(';')
                    .map(|e| e.trim().trim_start_matches("*."))
                    .collect();
                filters.push((name, extensions));
            }
        }
    }

    let directory = payload.get("directory").and_then(|v| v.as_str());
    let default_name = payload.get("default_name").and_then(|v| v.as_str());

    DialogParams {
        title,
        filters,
        directory,
        default_name,
    }
}

/// Apply parsed parameters to an `rfd::FileDialog` or `rfd::AsyncFileDialog`.
/// Both types share identical builder methods but no common trait.
macro_rules! apply_dialog_params {
    ($dialog_type:ty, $params:expr) => {{
        let params = &$params;
        let mut d = <$dialog_type>::new().set_title(params.title);
        for (name, exts) in &params.filters {
            d = d.add_filter(*name, exts);
        }
        if let Some(dir) = params.directory {
            d = d.set_directory(dir);
        }
        if let Some(name) = params.default_name {
            d = d.set_file_name(name);
        }
        d
    }};
}

// ---------------------------------------------------------------------------
// Effect dispatch
// ---------------------------------------------------------------------------

/// Returns true for effect kinds that should run asynchronously (file dialogs).
pub(crate) fn is_async_effect(kind: &str) -> bool {
    matches!(
        kind,
        "file_open"
            | "file_open_multiple"
            | "file_save"
            | "directory_select"
            | "directory_select_multiple"
    )
}

/// Dispatch an effect synchronously and return the response.
///
/// File dialog effects use `rfd::FileDialog` (blocking). On macOS, sync
/// dialogs may deadlock if called on the main thread -- prefer
/// [`handle_async_effect`] when a tokio runtime is available.
///
/// Clipboard and notification effects are always synchronous regardless
/// of which dispatch function is used.
pub(crate) fn handle_effect(id: String, kind: &str, payload: &Value) -> EffectResponse {
    match kind {
        "file_open" => handle_file_open(id, payload),
        "file_open_multiple" => handle_file_open_multiple(id, payload),
        "file_save" => handle_file_save(id, payload),
        "directory_select" => handle_directory_select(id, payload),
        "directory_select_multiple" => handle_directory_select_multiple(id, payload),
        "clipboard_read" => handle_clipboard_read(id),
        "clipboard_write" => handle_clipboard_write(id, payload),
        "clipboard_read_html" => handle_clipboard_read_html(id),
        "clipboard_write_html" => handle_clipboard_write_html(id, payload),
        "clipboard_clear" => handle_clipboard_clear(id),
        "clipboard_read_primary" => handle_clipboard_read_primary(id),
        "clipboard_write_primary" => handle_clipboard_write_primary(id, payload),
        "notification" => handle_notification(id, payload),
        _ => EffectResponse::unsupported(id),
    }
}

/// Dispatch an async effect and return the response. The response format
/// matches [`handle_effect`] exactly so the host can deserialize uniformly.
///
/// Only file dialog effects have async implementations (via
/// `rfd::AsyncFileDialog`). Other kinds are not routed here -- see
/// [`is_async_effect`].
///
/// Note: on X11-only Linux desktops without a portal (e.g. minimal WMs),
/// rfd falls back to a GTK dialog which may block a tokio worker thread.
/// This is a known rfd limitation, not specific to toddy.
pub(crate) async fn handle_async_effect(
    id: String,
    effect_type: &str,
    params: &Value,
) -> EffectResponse {
    match effect_type {
        "file_open" => {
            let p = parse_dialog_params(params, "Open File");
            let dialog = apply_dialog_params!(rfd::AsyncFileDialog, p);
            match dialog.pick_file().await {
                Some(h) => EffectResponse::ok(id, json!({"path": path_to_json_string(h.path())})),
                None => EffectResponse::cancelled(id),
            }
        }
        "file_open_multiple" => {
            let p = parse_dialog_params(params, "Open Files");
            let dialog = apply_dialog_params!(rfd::AsyncFileDialog, p);
            match dialog.pick_files().await {
                Some(handles) => {
                    let paths: Vec<String> = handles
                        .iter()
                        .map(|h| path_to_json_string(h.path()))
                        .collect();
                    EffectResponse::ok(id, json!({"paths": paths}))
                }
                None => EffectResponse::cancelled(id),
            }
        }
        "file_save" => {
            let p = parse_dialog_params(params, "Save File");
            let dialog = apply_dialog_params!(rfd::AsyncFileDialog, p);
            match dialog.save_file().await {
                Some(h) => EffectResponse::ok(id, json!({"path": path_to_json_string(h.path())})),
                None => EffectResponse::cancelled(id),
            }
        }
        "directory_select" => {
            let p = parse_dialog_params(params, "Select Directory");
            let dialog = apply_dialog_params!(rfd::AsyncFileDialog, p);
            match dialog.pick_folder().await {
                Some(h) => EffectResponse::ok(id, json!({"path": path_to_json_string(h.path())})),
                None => EffectResponse::cancelled(id),
            }
        }
        "directory_select_multiple" => {
            let p = parse_dialog_params(params, "Select Directories");
            let dialog = apply_dialog_params!(rfd::AsyncFileDialog, p);
            match dialog.pick_folders().await {
                Some(handles) => {
                    let paths: Vec<String> = handles
                        .iter()
                        .map(|h| path_to_json_string(h.path()))
                        .collect();
                    EffectResponse::ok(id, json!({"paths": paths}))
                }
                None => EffectResponse::cancelled(id),
            }
        }
        _ => EffectResponse::unsupported(id),
    }
}

// ---------------------------------------------------------------------------
// Sync file dialog handlers
//
// These use rfd::FileDialog (blocking). The async counterparts above use
// rfd::AsyncFileDialog. Both coexist: sync for headless/blocking contexts,
// async for the normal iced daemon event loop.
// ---------------------------------------------------------------------------

fn handle_file_open(id: String, payload: &Value) -> EffectResponse {
    let p = parse_dialog_params(payload, "Open File");
    let dialog = apply_dialog_params!(rfd::FileDialog, p);
    match dialog.pick_file() {
        Some(path) => EffectResponse::ok(id, json!({"path": path_to_json_string(&path)})),
        None => EffectResponse::cancelled(id),
    }
}

fn handle_file_open_multiple(id: String, payload: &Value) -> EffectResponse {
    let p = parse_dialog_params(payload, "Open Files");
    let dialog = apply_dialog_params!(rfd::FileDialog, p);
    match dialog.pick_files() {
        Some(paths) => {
            let paths: Vec<String> = paths.iter().map(|p| path_to_json_string(p)).collect();
            EffectResponse::ok(id, json!({"paths": paths}))
        }
        None => EffectResponse::cancelled(id),
    }
}

fn handle_file_save(id: String, payload: &Value) -> EffectResponse {
    let p = parse_dialog_params(payload, "Save File");
    let dialog = apply_dialog_params!(rfd::FileDialog, p);
    match dialog.save_file() {
        Some(path) => EffectResponse::ok(id, json!({"path": path_to_json_string(&path)})),
        None => EffectResponse::cancelled(id),
    }
}

fn handle_directory_select(id: String, payload: &Value) -> EffectResponse {
    let p = parse_dialog_params(payload, "Select Directory");
    let dialog = apply_dialog_params!(rfd::FileDialog, p);
    match dialog.pick_folder() {
        Some(path) => EffectResponse::ok(id, json!({"path": path_to_json_string(&path)})),
        None => EffectResponse::cancelled(id),
    }
}

fn handle_directory_select_multiple(id: String, payload: &Value) -> EffectResponse {
    let p = parse_dialog_params(payload, "Select Directories");
    let dialog = apply_dialog_params!(rfd::FileDialog, p);
    match dialog.pick_folders() {
        Some(paths) => {
            let paths: Vec<String> = paths.iter().map(|p| path_to_json_string(p)).collect();
            EffectResponse::ok(id, json!({"paths": paths}))
        }
        None => EffectResponse::cancelled(id),
    }
}

// ---------------------------------------------------------------------------
// Clipboard (arboard crate)
//
// A single Clipboard instance is kept alive for the process lifetime.
// On Wayland, arboard serves clipboard data from a background thread
// tied to the Clipboard instance -- dropping it loses the data.
// ---------------------------------------------------------------------------

fn with_clipboard(
    id: &str,
    f: impl FnOnce(&mut arboard::Clipboard, &str) -> EffectResponse,
) -> EffectResponse {
    use std::sync::Mutex;

    static CLIPBOARD: Mutex<Option<arboard::Clipboard>> = Mutex::new(None);

    let mut guard = CLIPBOARD.lock().unwrap_or_else(|poisoned| {
        log::warn!("clipboard mutex was poisoned, recovering");
        poisoned.into_inner()
    });

    let clipboard = match guard.as_mut() {
        Some(c) => c,
        None => match arboard::Clipboard::new() {
            Ok(c) => {
                *guard = Some(c);
                guard.as_mut().unwrap()
            }
            Err(e) => {
                return EffectResponse::error(
                    id.to_string(),
                    format!("clipboard init failed: {e}"),
                );
            }
        },
    };

    f(clipboard, id)
}

fn handle_clipboard_read(id: String) -> EffectResponse {
    with_clipboard(&id, |clipboard, id| match clipboard.get_text() {
        Ok(text) => EffectResponse::ok(id.to_string(), json!({"text": text})),
        Err(e) => EffectResponse::error(id.to_string(), format!("clipboard read failed: {e}")),
    })
}

fn handle_clipboard_write(id: String, payload: &Value) -> EffectResponse {
    let Some(text) = payload.get("text").and_then(|v| v.as_str()) else {
        return EffectResponse::error(id, "missing required field: text".to_string());
    };
    let text = text.to_string();

    with_clipboard(&id, |clipboard, id| match clipboard.set_text(text) {
        Ok(()) => EffectResponse::ok(id.to_string(), json!(null)),
        Err(e) => EffectResponse::error(id.to_string(), format!("clipboard write failed: {e}")),
    })
}

fn handle_clipboard_read_html(id: String) -> EffectResponse {
    with_clipboard(&id, |clipboard, id| match clipboard.get().html() {
        Ok(html) => EffectResponse::ok(id.to_string(), json!({"html": html})),
        Err(e) => EffectResponse::error(id.to_string(), format!("clipboard read html failed: {e}")),
    })
}

fn handle_clipboard_write_html(id: String, payload: &Value) -> EffectResponse {
    let Some(html) = payload.get("html").and_then(|v| v.as_str()) else {
        return EffectResponse::error(id, "missing required field: html".to_string());
    };
    let html = html.to_string();

    let alt_text = payload
        .get("alt_text")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    with_clipboard(&id, |clipboard, id| {
        match clipboard.set_html(&html, alt_text.as_ref()) {
            Ok(()) => EffectResponse::ok(id.to_string(), json!(null)),
            Err(e) => {
                EffectResponse::error(id.to_string(), format!("clipboard write html failed: {e}"))
            }
        }
    })
}

fn handle_clipboard_clear(id: String) -> EffectResponse {
    with_clipboard(&id, |clipboard, id| match clipboard.clear() {
        Ok(()) => EffectResponse::ok(id.to_string(), json!(null)),
        Err(e) => EffectResponse::error(id.to_string(), format!("clipboard clear failed: {e}")),
    })
}

// Primary clipboard: uses the X11/Wayland primary selection on Linux.
// On other platforms, falls back to the standard clipboard.

#[cfg(target_os = "linux")]
fn handle_clipboard_read_primary(id: String) -> EffectResponse {
    use arboard::{GetExtLinux, LinuxClipboardKind};

    with_clipboard(&id, |clipboard, id| {
        match clipboard
            .get()
            .clipboard(LinuxClipboardKind::Primary)
            .text()
        {
            Ok(text) => EffectResponse::ok(id.to_string(), json!({"text": text})),
            Err(e) => EffectResponse::error(
                id.to_string(),
                format!("primary clipboard read failed: {e}"),
            ),
        }
    })
}

#[cfg(target_os = "linux")]
fn handle_clipboard_write_primary(id: String, payload: &Value) -> EffectResponse {
    use arboard::{LinuxClipboardKind, SetExtLinux};
    let Some(text) = payload.get("text").and_then(|v| v.as_str()) else {
        return EffectResponse::error(id, "missing required field: text".to_string());
    };
    let text = text.to_string();

    with_clipboard(&id, |clipboard, id| {
        match clipboard
            .set()
            .clipboard(LinuxClipboardKind::Primary)
            .text(text)
        {
            Ok(()) => EffectResponse::ok(id.to_string(), json!(null)),
            Err(e) => EffectResponse::error(
                id.to_string(),
                format!("primary clipboard write failed: {e}"),
            ),
        }
    })
}

// On non-Linux platforms, primary clipboard falls back to the standard clipboard.
#[cfg(not(target_os = "linux"))]
fn handle_clipboard_read_primary(id: String) -> EffectResponse {
    handle_clipboard_read(id)
}

#[cfg(not(target_os = "linux"))]
fn handle_clipboard_write_primary(id: String, payload: &Value) -> EffectResponse {
    handle_clipboard_write(id, payload)
}

// ---------------------------------------------------------------------------
// Notifications (notify-rust crate)
// ---------------------------------------------------------------------------

/// Send an OS notification.
///
/// **Platform quirks:**
/// - **macOS:** Requires the app to be signed or have an Info.plist for
///   notifications to appear. The `icon` field is ignored (macOS uses the
///   app icon). Notifications go to macOS Notification Center.
/// - **Linux:** Depends on the desktop environment's notification daemon
///   (e.g. dunst, mako, GNOME notifications). The `icon` field is a
///   freedesktop icon name (e.g. "dialog-information"). `urgency` is
///   Linux-only.
/// - **Windows:** Uses the Windows toast notification system. The `icon`
///   field is ignored (Windows uses the app icon).
fn handle_notification(id: String, payload: &Value) -> EffectResponse {
    let title = payload
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("toddy");

    let body = payload.get("body").and_then(|v| v.as_str()).unwrap_or("");

    let mut notification = notify_rust::Notification::new();
    notification.summary(title).body(body);

    if let Some(icon) = payload.get("icon").and_then(|v| v.as_str()) {
        notification.icon(icon);
    }

    if let Some(timeout_ms) = payload.get("timeout").and_then(|v| v.as_u64()) {
        let clamped = timeout_ms.min(u32::MAX as u64) as u32;
        notification.timeout(notify_rust::Timeout::Milliseconds(clamped));
    }

    #[cfg(target_os = "linux")]
    if let Some(urgency) = payload.get("urgency").and_then(|v| v.as_str()) {
        let u = match urgency {
            "low" => notify_rust::Urgency::Low,
            "critical" => notify_rust::Urgency::Critical,
            _ => notify_rust::Urgency::Normal,
        };
        notification.urgency(u);
    }

    if let Some(sound) = payload.get("sound").and_then(|v| v.as_str()) {
        notification.sound_name(sound);
    }

    match notification.show() {
        Ok(_) => EffectResponse::ok(id, json!(null)),
        Err(e) => EffectResponse::error(id, format!("notification failed: {e}")),
    }
}

// ---------------------------------------------------------------------------
// NativeEffectHandler -- EffectHandler trait impl for the native binary
// ---------------------------------------------------------------------------

/// Native effect handler wrapping the platform-specific implementations
/// in this module (rfd file dialogs, arboard clipboard, notify-rust).
pub(crate) struct NativeEffectHandler;

impl toddy_renderer::EffectHandler for NativeEffectHandler {
    fn handle_sync(
        &self,
        id: &str,
        kind: &str,
        payload: &serde_json::Value,
    ) -> Option<toddy_core::protocol::EffectResponse> {
        Some(handle_effect(id.to_string(), kind, payload))
    }

    fn spawn_async(
        &self,
        id: String,
        kind: String,
        payload: serde_json::Value,
    ) -> iced::Task<toddy_core::message::Message> {
        iced::Task::perform(
            async move { handle_async_effect(id, &kind, &payload).await },
            |response| {
                if let Err(e) = toddy_renderer::emitters::emit_effect_response(response) {
                    log::error!("write error in async effect: {e}");
                }
                toddy_core::message::Message::NoOp
            },
        )
    }

    fn is_async(&self, kind: &str) -> bool {
        is_async_effect(kind)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unknown_effect_returns_unsupported() {
        let resp = handle_effect("eff-1".to_string(), "teleport_sandwich", &json!({}));
        assert_eq!(resp.status, "error");
        assert_eq!(resp.error.as_deref(), Some("unsupported"));
        assert_eq!(resp.id, "eff-1");
    }

    /// Dispatch every known effect kind with a minimal payload and verify
    /// none of them panic. The handlers may return "error" when the OS
    /// resource (clipboard, display server, notification daemon) is
    /// unavailable in the test environment. That's fine: we're testing
    /// that the routing reaches the right handler and returns cleanly.
    #[test]
    fn dispatch_routes_all_known_kinds_without_panic() {
        let kinds_with_payloads: Vec<(&str, Value)> = vec![
            ("file_open", json!({"title": "Pick a file"})),
            ("file_open_multiple", json!({"title": "Pick files"})),
            (
                "file_save",
                json!({"title": "Save", "default_name": "out.txt"}),
            ),
            ("directory_select", json!({"title": "Choose dir"})),
            ("directory_select_multiple", json!({"title": "Choose dirs"})),
            ("clipboard_read", json!({})),
            ("clipboard_write", json!({"text": "hello"})),
            ("clipboard_read_html", json!({})),
            (
                "clipboard_write_html",
                json!({"html": "<b>hi</b>", "alt_text": "hi"}),
            ),
            ("clipboard_clear", json!({})),
            ("clipboard_read_primary", json!({})),
            ("clipboard_write_primary", json!({"text": "primary"})),
            (
                "notification",
                json!({"title": "Test", "body": "body", "icon": "dialog-information", "timeout": 3000, "urgency": "low", "sound": "message-new-instant"}),
            ),
        ];

        for (kind, payload) in &kinds_with_payloads {
            let id = format!("test-{kind}");
            let resp = handle_effect(id.clone(), kind, payload);

            assert_eq!(resp.id, id, "id mismatch for kind {kind}");
            assert_eq!(resp.message_type, "effect_response");
            assert!(
                resp.status == "ok" || resp.status == "error" || resp.status == "cancelled",
                "unexpected status '{}' for kind {kind}",
                resp.status
            );
        }
    }

    /// Verify that empty payloads don't cause panics -- handlers should
    /// defensively unwrap_or on missing fields.
    #[test]
    fn handlers_tolerate_empty_payloads() {
        let kinds: &[&str] = &[
            "file_open",
            "file_open_multiple",
            "file_save",
            "directory_select",
            "directory_select_multiple",
            "clipboard_read",
            "clipboard_write",
            "clipboard_read_html",
            "clipboard_write_html",
            "clipboard_clear",
            "clipboard_read_primary",
            "clipboard_write_primary",
            "notification",
        ];

        for kind in kinds {
            let resp = handle_effect(format!("empty-{kind}"), kind, &json!({}));
            assert_eq!(resp.message_type, "effect_response");
        }
    }

    #[test]
    fn unknown_kinds_preserve_id() {
        for i in 0..5 {
            let id = format!("unk-{i}");
            let resp = handle_effect(id.clone(), &format!("bogus_{i}"), &json!(null));
            assert_eq!(resp.id, id);
            assert_eq!(resp.status, "error");
            assert_eq!(resp.error.as_deref(), Some("unsupported"));
        }
    }

    // -- is_async_effect ------------------------------------------------------

    #[test]
    fn async_effects_recognized() {
        assert!(is_async_effect("file_open"));
        assert!(is_async_effect("file_open_multiple"));
        assert!(is_async_effect("file_save"));
        assert!(is_async_effect("directory_select"));
        assert!(is_async_effect("directory_select_multiple"));
    }

    #[test]
    fn sync_effects_not_async() {
        assert!(!is_async_effect("clipboard_read"));
        assert!(!is_async_effect("clipboard_write"));
        assert!(!is_async_effect("notification"));
    }

    #[test]
    fn unknown_effect_not_async() {
        assert!(!is_async_effect("teleport_sandwich"));
        assert!(!is_async_effect(""));
        assert!(!is_async_effect("FILE_OPEN")); // case-sensitive
    }

    // -- parse_dialog_params --------------------------------------------------

    #[test]
    fn parse_params_defaults() {
        let payload = json!({});
        let p = parse_dialog_params(&payload, "Default Title");
        assert_eq!(p.title, "Default Title");
        assert!(p.filters.is_empty());
        assert!(p.directory.is_none());
        assert!(p.default_name.is_none());
    }

    #[test]
    fn parse_params_with_all_fields() {
        let payload = json!({
            "title": "Custom Title",
            "filters": [["Images", "*.png;*.jpg"], ["All", "*.*"]],
            "directory": "/home/user",
            "default_name": "output.txt"
        });
        let p = parse_dialog_params(&payload, "Ignored");
        assert_eq!(p.title, "Custom Title");
        assert_eq!(p.filters.len(), 2);
        assert_eq!(p.filters[0].0, "Images");
        assert_eq!(p.filters[0].1, vec!["png", "jpg"]);
        assert_eq!(p.filters[1].0, "All");
        assert_eq!(p.directory, Some("/home/user"));
        assert_eq!(p.default_name, Some("output.txt"));
    }

    #[test]
    fn parse_params_malformed_filters_ignored() {
        let payload = json!({
            "filters": [
                "not an array",
                [],
                ["only one element"],
                ["Name", "*.txt"]
            ]
        });
        let p = parse_dialog_params(&payload, "T");
        // Only the last filter is valid
        assert_eq!(p.filters.len(), 1);
        assert_eq!(p.filters[0].0, "Name");
    }

    // -- path_to_json_string --------------------------------------------------

    #[test]
    fn path_normal() {
        use std::path::Path;
        assert_eq!(
            path_to_json_string(Path::new("/home/user/file.txt")),
            "/home/user/file.txt"
        );
    }

    #[test]
    fn path_empty() {
        use std::path::Path;
        assert_eq!(path_to_json_string(Path::new("")), "");
    }

    #[test]
    fn path_with_spaces() {
        use std::path::Path;
        assert_eq!(
            path_to_json_string(Path::new("/home/user/my documents/file.txt")),
            "/home/user/my documents/file.txt"
        );
    }

    #[test]
    fn path_with_special_chars() {
        use std::path::Path;
        assert_eq!(
            path_to_json_string(Path::new("/tmp/test-file_v2 (1).tar.gz")),
            "/tmp/test-file_v2 (1).tar.gz"
        );
    }
}
