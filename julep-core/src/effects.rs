//! Platform effect handlers (file dialogs, clipboard, notifications).
//!
//! **File paths:** All file path strings returned by dialog handlers use
//! OS-native path separators (`/` on Unix, `\` on Windows). Cross-platform
//! consumers should normalize paths before comparing or storing them.

use serde_json::{json, Value};

use crate::protocol::EffectResponse;

/// Convert a file path to a JSON string value, logging a warning if the path
/// contains non-UTF-8 bytes and lossy conversion is required.
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

/// Returns true for effect kinds that should run asynchronously (file dialogs).
pub fn is_async_effect(kind: &str) -> bool {
    matches!(kind, "file_open" | "file_save" | "directory_select")
}

pub fn handle_effect(id: String, kind: &str, payload: &Value) -> EffectResponse {
    match kind {
        "file_open" => handle_file_open(id, payload),
        "file_save" => handle_file_save(id, payload),
        "directory_select" => handle_directory_select(id, payload),
        "clipboard_read" => handle_clipboard_read(id),
        "clipboard_write" => handle_clipboard_write(id, payload),
        "clipboard_read_primary" => handle_clipboard_read_primary(id),
        "clipboard_write_primary" => handle_clipboard_write_primary(id, payload),
        "notification" => handle_notification(id, payload),
        _ => EffectResponse::unsupported(id),
    }
}

// ---------------------------------------------------------------------------
// File dialogs -- synchronous handlers (requires "dialogs" feature / rfd crate)
//
// These sync handlers are used by `handle_effect()`, which is the fallback
// path when the async runtime isn't available (e.g. during initialization or
// in contexts where tokio isn't running). They are NOT dead code -- the async
// counterparts in `handle_async_effect()` below use `rfd::AsyncFileDialog`
// and are used by the renderer's `SpawnAsyncEffect` path (via Task::perform).
// Both paths coexist intentionally: sync for headless/blocking contexts,
// async for the normal iced daemon event loop.
// ---------------------------------------------------------------------------

/// Synchronous file open dialog. Used when the async runtime is unavailable.
///
/// **WARNING (macOS):** Sync file dialogs may deadlock if called on the main
/// thread because macOS requires native dialogs to run on the main thread.
/// Prefer `handle_async_effect` when a tokio runtime is available.
#[cfg(feature = "dialogs")]
fn handle_file_open(id: String, payload: &Value) -> EffectResponse {
    let title = payload
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("Open File");

    let mut dialog = rfd::FileDialog::new().set_title(title);

    if let Some(filters) = payload.get("filters").and_then(|v| v.as_array()) {
        for filter in filters {
            if let Some(arr) = filter.as_array() {
                if arr.len() >= 2 {
                    if let (Some(name), Some(ext)) = (arr[0].as_str(), arr[1].as_str()) {
                        let extensions: Vec<&str> = ext
                            .split(';')
                            .map(|e| e.trim().trim_start_matches("*."))
                            .collect();
                        dialog = dialog.add_filter(name, &extensions);
                    }
                }
            }
        }
    }

    if let Some(dir) = payload.get("directory").and_then(|v| v.as_str()) {
        dialog = dialog.set_directory(dir);
    }

    match dialog.pick_file() {
        Some(path) => EffectResponse::ok(id, json!({"path": path_to_json_string(&path)})),
        None => EffectResponse::error(id, "cancelled".to_string()),
    }
}

#[cfg(not(feature = "dialogs"))]
fn handle_file_open(id: String, _payload: &Value) -> EffectResponse {
    EffectResponse::unsupported(id)
}

/// Synchronous file save dialog. Used when the async runtime is unavailable.
///
/// **WARNING (macOS):** Sync file dialogs may deadlock if called on the main
/// thread because macOS requires native dialogs to run on the main thread.
/// Prefer `handle_async_effect` when a tokio runtime is available.
#[cfg(feature = "dialogs")]
fn handle_file_save(id: String, payload: &Value) -> EffectResponse {
    let title = payload
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("Save File");

    let mut dialog = rfd::FileDialog::new().set_title(title);

    if let Some(name) = payload.get("default_name").and_then(|v| v.as_str()) {
        dialog = dialog.set_file_name(name);
    }

    if let Some(filters) = payload.get("filters").and_then(|v| v.as_array()) {
        for filter in filters {
            if let Some(arr) = filter.as_array() {
                if arr.len() >= 2 {
                    if let (Some(name), Some(ext)) = (arr[0].as_str(), arr[1].as_str()) {
                        let extensions: Vec<&str> = ext
                            .split(';')
                            .map(|e| e.trim().trim_start_matches("*."))
                            .collect();
                        dialog = dialog.add_filter(name, &extensions);
                    }
                }
            }
        }
    }

    match dialog.save_file() {
        Some(path) => EffectResponse::ok(id, json!({"path": path_to_json_string(&path)})),
        None => EffectResponse::error(id, "cancelled".to_string()),
    }
}

#[cfg(not(feature = "dialogs"))]
fn handle_file_save(id: String, _payload: &Value) -> EffectResponse {
    EffectResponse::unsupported(id)
}

/// Synchronous directory select dialog. Used when the async runtime is unavailable.
///
/// **WARNING (macOS):** Sync file dialogs may deadlock if called on the main
/// thread because macOS requires native dialogs to run on the main thread.
/// Prefer `handle_async_effect` when a tokio runtime is available.
#[cfg(feature = "dialogs")]
fn handle_directory_select(id: String, payload: &Value) -> EffectResponse {
    let title = payload
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("Select Directory");

    let dialog = rfd::FileDialog::new().set_title(title);

    match dialog.pick_folder() {
        Some(path) => EffectResponse::ok(id, json!({"path": path_to_json_string(&path)})),
        None => EffectResponse::error(id, "cancelled".to_string()),
    }
}

#[cfg(not(feature = "dialogs"))]
fn handle_directory_select(id: String, _payload: &Value) -> EffectResponse {
    EffectResponse::unsupported(id)
}

// ---------------------------------------------------------------------------
// Clipboard (requires "clipboard" feature / arboard crate)
//
// A single Clipboard instance is kept alive for the process lifetime.
// On Wayland, arboard serves clipboard data from a background thread
// tied to the Clipboard instance -- dropping it loses the data.
// ---------------------------------------------------------------------------

#[cfg(feature = "clipboard")]
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
                return EffectResponse::error(id.to_string(), format!("clipboard init failed: {e}"))
            }
        },
    };

    f(clipboard, id)
}

#[cfg(feature = "clipboard")]
fn handle_clipboard_read(id: String) -> EffectResponse {
    with_clipboard(&id, |clipboard, id| match clipboard.get_text() {
        Ok(text) => EffectResponse::ok(id.to_string(), json!({"text": text})),
        Err(e) => EffectResponse::error(id.to_string(), format!("clipboard read failed: {e}")),
    })
}

#[cfg(not(feature = "clipboard"))]
fn handle_clipboard_read(id: String) -> EffectResponse {
    EffectResponse::unsupported(id)
}

#[cfg(feature = "clipboard")]
fn handle_clipboard_write(id: String, payload: &Value) -> EffectResponse {
    let text = payload
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    with_clipboard(&id, |clipboard, id| match clipboard.set_text(text) {
        Ok(()) => EffectResponse::ok(id.to_string(), json!(null)),
        Err(e) => EffectResponse::error(id.to_string(), format!("clipboard write failed: {e}")),
    })
}

#[cfg(not(feature = "clipboard"))]
fn handle_clipboard_write(id: String, _payload: &Value) -> EffectResponse {
    EffectResponse::unsupported(id)
}

// Primary clipboard: uses the X11/Wayland primary selection on Linux.
// On other platforms, falls back to the standard clipboard.

#[cfg(all(feature = "clipboard", target_os = "linux"))]
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

#[cfg(all(feature = "clipboard", target_os = "linux"))]
fn handle_clipboard_write_primary(id: String, payload: &Value) -> EffectResponse {
    use arboard::{LinuxClipboardKind, SetExtLinux};
    let text = payload
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

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
#[cfg(all(feature = "clipboard", not(target_os = "linux")))]
fn handle_clipboard_read_primary(id: String) -> EffectResponse {
    handle_clipboard_read(id)
}

#[cfg(all(feature = "clipboard", not(target_os = "linux")))]
fn handle_clipboard_write_primary(id: String, payload: &Value) -> EffectResponse {
    handle_clipboard_write(id, payload)
}

#[cfg(not(feature = "clipboard"))]
fn handle_clipboard_read_primary(id: String) -> EffectResponse {
    EffectResponse::unsupported(id)
}

#[cfg(not(feature = "clipboard"))]
fn handle_clipboard_write_primary(id: String, _payload: &Value) -> EffectResponse {
    EffectResponse::unsupported(id)
}

// ---------------------------------------------------------------------------
// Notifications (requires "notifications" feature / notify-rust crate)
// ---------------------------------------------------------------------------

/// Send an OS notification.
///
/// **Platform quirks:**
/// - **macOS:** Requires the app to be signed or have an Info.plist for
///   notifications to appear. Notifications go to macOS Notification Center.
/// - **Linux:** Depends on the desktop environment's notification daemon
///   (e.g. dunst, mako, GNOME notifications). Behavior varies by DE.
/// - **Windows:** Uses the Windows toast notification system.
#[cfg(feature = "notifications")]
fn handle_notification(id: String, payload: &Value) -> EffectResponse {
    let title = payload
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("Julep");

    let body = payload.get("body").and_then(|v| v.as_str()).unwrap_or("");

    match notify_rust::Notification::new()
        .summary(title)
        .body(body)
        .show()
    {
        Ok(_) => EffectResponse::ok(id, json!(null)),
        Err(e) => EffectResponse::error(id, format!("notification failed: {e}")),
    }
}

#[cfg(not(feature = "notifications"))]
fn handle_notification(id: String, _payload: &Value) -> EffectResponse {
    EffectResponse::unsupported(id)
}

// ---------------------------------------------------------------------------
// Async effect handlers (file dialogs via rfd::AsyncFileDialog)
// ---------------------------------------------------------------------------

/// Handle an async effect and return an EffectResponse. The response format
/// matches the sync handlers exactly so the host can deserialize uniformly.
///
// Note: on X11-only Linux desktops without a portal (e.g. minimal WMs),
// rfd falls back to a GTK dialog which may block a tokio worker thread.
// This is a known rfd limitation, not specific to julep.
#[cfg(feature = "dialogs")]
pub async fn handle_async_effect(id: String, effect_type: &str, params: &Value) -> EffectResponse {
    match effect_type {
        "file_open" => {
            let title = params
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("Open File");

            let mut dialog = rfd::AsyncFileDialog::new().set_title(title);

            if let Some(filters) = params.get("filters").and_then(|v| v.as_array()) {
                for filter in filters {
                    if let Some(arr) = filter.as_array() {
                        if arr.len() >= 2 {
                            if let (Some(name), Some(ext)) = (arr[0].as_str(), arr[1].as_str()) {
                                let extensions: Vec<&str> = ext
                                    .split(';')
                                    .map(|e| e.trim().trim_start_matches("*."))
                                    .collect();
                                dialog = dialog.add_filter(name, &extensions);
                            }
                        }
                    }
                }
            }

            if let Some(dir) = params.get("directory").and_then(|v| v.as_str()) {
                dialog = dialog.set_directory(dir);
            }

            match dialog.pick_file().await {
                Some(handle) => {
                    EffectResponse::ok(id, json!({"path": path_to_json_string(handle.path())}))
                }
                None => EffectResponse::error(id, "cancelled".to_string()),
            }
        }
        "file_save" => {
            let title = params
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("Save File");

            let mut dialog = rfd::AsyncFileDialog::new().set_title(title);

            if let Some(name) = params.get("default_name").and_then(|v| v.as_str()) {
                dialog = dialog.set_file_name(name);
            }

            if let Some(filters) = params.get("filters").and_then(|v| v.as_array()) {
                for filter in filters {
                    if let Some(arr) = filter.as_array() {
                        if arr.len() >= 2 {
                            if let (Some(name), Some(ext)) = (arr[0].as_str(), arr[1].as_str()) {
                                let extensions: Vec<&str> = ext
                                    .split(';')
                                    .map(|e| e.trim().trim_start_matches("*."))
                                    .collect();
                                dialog = dialog.add_filter(name, &extensions);
                            }
                        }
                    }
                }
            }

            match dialog.save_file().await {
                Some(handle) => {
                    EffectResponse::ok(id, json!({"path": path_to_json_string(handle.path())}))
                }
                None => EffectResponse::error(id, "cancelled".to_string()),
            }
        }
        "directory_select" => {
            let title = params
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("Select Directory");

            let dialog = rfd::AsyncFileDialog::new().set_title(title);

            match dialog.pick_folder().await {
                Some(handle) => {
                    EffectResponse::ok(id, json!({"path": path_to_json_string(handle.path())}))
                }
                None => EffectResponse::error(id, "cancelled".to_string()),
            }
        }
        _ => EffectResponse::unsupported(id),
    }
}

#[cfg(not(feature = "dialogs"))]
pub async fn handle_async_effect(
    id: String,
    _effect_type: &str,
    _params: &Value,
) -> EffectResponse {
    EffectResponse::unsupported(id)
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
    /// none of them panic. The handlers may return "unsupported" (when the
    /// corresponding feature is not compiled in) or "error" (when the OS
    /// resource -- clipboard, display server, notification daemon -- is
    /// unavailable in the test environment). That's fine: we're testing
    /// that the routing reaches the right handler and returns cleanly.
    #[test]
    fn dispatch_routes_all_known_kinds_without_panic() {
        let kinds_with_payloads: Vec<(&str, Value)> = vec![
            ("file_open", json!({"title": "Pick a file"})),
            (
                "file_save",
                json!({"title": "Save", "default_name": "out.txt"}),
            ),
            ("directory_select", json!({"title": "Choose dir"})),
            ("clipboard_read", json!({})),
            ("clipboard_write", json!({"text": "hello"})),
            ("clipboard_read_primary", json!({})),
            ("clipboard_write_primary", json!({"text": "primary"})),
            ("notification", json!({"title": "Test", "body": "body"})),
        ];

        for (kind, payload) in &kinds_with_payloads {
            let id = format!("test-{kind}");
            let resp = handle_effect(id.clone(), kind, payload);

            // Must get a well-formed response with matching id.
            assert_eq!(resp.id, id, "id mismatch for kind {kind}");
            assert_eq!(resp.message_type, "effect_response");
            assert!(
                resp.status == "ok" || resp.status == "error",
                "unexpected status '{}' for kind {kind}",
                resp.status
            );
        }
    }

    /// Verify that minimal/empty payloads don't cause panics -- handlers
    /// should defensively unwrap_or on missing fields, not panic.
    #[test]
    fn handlers_tolerate_empty_payloads() {
        let kinds: &[&str] = &[
            "file_open",
            "file_save",
            "directory_select",
            "clipboard_read",
            "clipboard_write",
            "clipboard_read_primary",
            "clipboard_write_primary",
            "notification",
        ];

        for kind in kinds {
            let resp = handle_effect(format!("empty-{kind}"), kind, &json!({}));
            assert_eq!(resp.message_type, "effect_response");
        }
    }

    /// Multiple unknown kinds all return unsupported with distinct ids.
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

    // NOTE: The feature-gated handler implementations (file dialogs via rfd,
    // clipboard via arboard, notifications via notify-rust) interact with real
    // OS resources -- display server, clipboard daemon, notification service.
    // They can't be meaningfully unit-tested without those services running.
    // Integration-level testing of those paths belongs in a CI environment
    // with Xvfb / a clipboard provider available, not in pure unit tests.

    // -----------------------------------------------------------------------
    // is_async_effect
    // -----------------------------------------------------------------------

    #[test]
    fn async_effects_recognized() {
        assert!(is_async_effect("file_open"));
        assert!(is_async_effect("file_save"));
        assert!(is_async_effect("directory_select"));
    }

    #[test]
    fn sync_effects_not_async() {
        assert!(!is_async_effect("clipboard_read"));
        assert!(!is_async_effect("clipboard_write"));
        assert!(!is_async_effect("clipboard_read_primary"));
        assert!(!is_async_effect("clipboard_write_primary"));
        assert!(!is_async_effect("notification"));
    }

    #[test]
    fn unknown_effect_not_async() {
        assert!(!is_async_effect("teleport_sandwich"));
        assert!(!is_async_effect(""));
        assert!(!is_async_effect("FILE_OPEN")); // case-sensitive
    }

    // -----------------------------------------------------------------------
    // path_to_json_string
    // -----------------------------------------------------------------------

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
