//! Window operations: open, close, resize, move, maximize, fullscreen,
//! decorations, icon, queries (size, position, mode, scale factor), and
//! window sync. Dispatched from [`CoreEffect::WindowOp`] via the `op`
//! string, `window_id`, and JSON `settings`.
//!
//! ## Platform notes
//!
//! Several operations are no-ops on Wayland because the compositor owns
//! window positioning, focus, and icon management. When the renderer
//! detects Wayland (via `WAYLAND_DISPLAY`), it logs a debug warning for
//! these operations so SDK users can understand why their requests have
//! no visible effect.

use std::collections::HashSet;
use std::sync::OnceLock;

use base64::Engine as _;
use iced::{Point, Size, Task, window};

use toddy_core::message::Message;

use crate::App;
use crate::emitters::{emit_effect_response, emit_query_response};

/// Returns true if the current display server is Wayland.
///
/// Detected via the `WAYLAND_DISPLAY` environment variable, which is
/// set by Wayland compositors. Cached in a `OnceLock` so the env
/// lookup happens at most once per process.
/// On WASM, always returns false.
fn is_wayland() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        static IS_WAYLAND: OnceLock<bool> = OnceLock::new();
        *IS_WAYLAND.get_or_init(|| std::env::var("WAYLAND_DISPLAY").is_ok())
    }
    #[cfg(target_arch = "wasm32")]
    {
        false
    }
}

/// Log a debug warning when a window operation is a known no-op on Wayland.
fn warn_wayland_noop(op: &str) {
    if is_wayland() {
        log::debug!("{op}: no-op on Wayland (compositor-controlled)");
    }
}

// ---------------------------------------------------------------------------
// Window operations (impl App)
// ---------------------------------------------------------------------------

impl App {
    pub fn handle_window_op(
        &mut self,
        op: &str,
        window_id: &str,
        settings: &serde_json::Value,
    ) -> Task<Message> {
        match op {
            "open" => {
                if self.windows.contains_toddy(window_id) {
                    log::warn!("window_op open: {window_id} already open, skipping");
                    return Task::none();
                }

                let win_settings = parse_window_settings(settings);
                let initial_decorations = win_settings.decorations;
                let (iced_id, open_task) = window::open(win_settings);

                self.windows.insert(window_id.to_string(), iced_id);
                self.windows.set_decorated(window_id, initial_decorations);

                let toddy_id = window_id.to_string();
                open_task.map(move |id| Message::WindowOpened(id, toddy_id.clone()))
            }
            "close" => {
                if let Some(iced_id) = self.windows.remove_by_toddy(window_id) {
                    window::close(iced_id)
                } else {
                    log::warn!("window_op close: unknown window_id: {window_id}");
                    Task::none()
                }
            }
            "update" => {
                // Apply changed window props to an already-open window.
                // The host sends this when a surviving window's props change
                // between renders.
                if let Some(&iced_id) = self.windows.get_iced(window_id) {
                    let mut tasks: Vec<Task<Message>> = Vec::new();

                    if let Some(obj) = settings.as_object() {
                        if let Some(title) = obj.get("title").and_then(|v| v.as_str()) {
                            log::debug!("update window {window_id}: title={title}");
                            // Title is read from the tree in title(), no task needed.
                            let _ = title;
                        }
                        if obj.contains_key("width") || obj.contains_key("height") {
                            let w =
                                obj.get("width").and_then(|v| v.as_f64()).unwrap_or(800.0) as f32;
                            let h =
                                obj.get("height").and_then(|v| v.as_f64()).unwrap_or(600.0) as f32;
                            tasks.push(window::resize(iced_id, Size::new(w, h)));
                        }
                        if let Some(maximized) = obj.get("maximized").and_then(|v| v.as_bool()) {
                            tasks.push(window::maximize(iced_id, maximized));
                        }
                        if let Some(resizable) = obj.get("resizable").and_then(|v| v.as_bool()) {
                            tasks.push(window::set_resizable(iced_id, resizable));
                        }
                        // Note: visible and fullscreen both call set_mode. If both are
                        // present, the last one wins. Hosts should not set both.
                        if let Some(visible) = obj.get("visible").and_then(|v| v.as_bool()) {
                            let mode = if visible {
                                window::Mode::Windowed
                            } else {
                                window::Mode::Hidden
                            };
                            tasks.push(window::set_mode(iced_id, mode));
                        }
                        if let Some(fullscreen) = obj.get("fullscreen").and_then(|v| v.as_bool()) {
                            let mode = if fullscreen {
                                window::Mode::Fullscreen
                            } else {
                                window::Mode::Windowed
                            };
                            tasks.push(window::set_mode(iced_id, mode));
                        }
                        if obj.contains_key("min_size") {
                            let sz = parse_optional_size(
                                obj.get("min_size").unwrap_or(&serde_json::Value::Null),
                            );
                            tasks.push(window::set_min_size(iced_id, sz));
                        }
                        if obj.contains_key("max_size") {
                            let sz = parse_optional_size(
                                obj.get("max_size").unwrap_or(&serde_json::Value::Null),
                            );
                            tasks.push(window::set_max_size(iced_id, sz));
                        }
                        if obj.contains_key("level") {
                            let level = parse_window_level(
                                obj.get("level")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("normal"),
                            );
                            tasks.push(window::set_level(iced_id, level));
                        }
                        if let Some(desired) = obj.get("decorations").and_then(|v| v.as_bool()) {
                            let current = self.windows.is_decorated(window_id);
                            if desired != current {
                                self.windows.set_decorated(window_id, desired);
                                tasks.push(window::toggle_decorations(iced_id));
                            }
                        }
                    }

                    Task::batch(tasks)
                } else {
                    log::warn!("window_op update: unknown window_id: {window_id}");
                    Task::none()
                }
            }
            "resize" => {
                if let Some(&iced_id) = self.windows.get_iced(window_id) {
                    let w = settings
                        .get("width")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(800.0) as f32;
                    let h = settings
                        .get("height")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(600.0) as f32;
                    window::resize(iced_id, Size::new(w, h))
                } else {
                    Task::none()
                }
            }
            "move" => {
                warn_wayland_noop("move");
                if let Some(&iced_id) = self.windows.get_iced(window_id) {
                    let x = settings.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                    let y = settings.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                    window::move_to(iced_id, Point::new(x, y))
                } else {
                    Task::none()
                }
            }
            "maximize" => {
                if let Some(&iced_id) = self.windows.get_iced(window_id) {
                    let maximized = settings
                        .get("maximized")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true);
                    window::maximize(iced_id, maximized)
                } else {
                    Task::none()
                }
            }
            "minimize" => {
                if let Some(&iced_id) = self.windows.get_iced(window_id) {
                    let minimized = settings
                        .get("minimized")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true);
                    window::minimize(iced_id, minimized)
                } else {
                    Task::none()
                }
            }
            "set_mode" => {
                if let Some(&iced_id) = self.windows.get_iced(window_id) {
                    let mode = parse_window_mode(settings);
                    window::set_mode(iced_id, mode)
                } else {
                    Task::none()
                }
            }
            "toggle_maximize" => {
                if let Some(&iced_id) = self.windows.get_iced(window_id) {
                    window::toggle_maximize(iced_id)
                } else {
                    Task::none()
                }
            }
            "toggle_decorations" => {
                if let Some(&iced_id) = self.windows.get_iced(window_id) {
                    let current = self.windows.is_decorated(window_id);
                    self.windows.set_decorated(window_id, !current);
                    window::toggle_decorations(iced_id)
                } else {
                    Task::none()
                }
            }
            "gain_focus" => {
                warn_wayland_noop("gain_focus");
                if let Some(&iced_id) = self.windows.get_iced(window_id) {
                    window::gain_focus(iced_id)
                } else {
                    Task::none()
                }
            }
            "set_level" => {
                if let Some(&iced_id) = self.windows.get_iced(window_id) {
                    let level = parse_window_level(
                        settings
                            .get("level")
                            .and_then(|v| v.as_str())
                            .unwrap_or("normal"),
                    );
                    window::set_level(iced_id, level)
                } else {
                    Task::none()
                }
            }
            "drag" => {
                if let Some(&iced_id) = self.windows.get_iced(window_id) {
                    window::drag(iced_id)
                } else {
                    Task::none()
                }
            }
            "drag_resize" => {
                #[cfg(target_os = "macos")]
                log::warn!("drag_resize is not supported on macOS");
                if let Some(&iced_id) = self.windows.get_iced(window_id) {
                    let direction = parse_direction(
                        settings
                            .get("direction")
                            .and_then(|v| v.as_str())
                            .unwrap_or("south_east"),
                    );
                    window::drag_resize(iced_id, direction)
                } else {
                    Task::none()
                }
            }
            "request_attention" => {
                if let Some(&iced_id) = self.windows.get_iced(window_id) {
                    let urgency =
                        settings
                            .get("urgency")
                            .and_then(|v| v.as_str())
                            .map(|s| match s {
                                "critical" => window::UserAttention::Critical,
                                _ => window::UserAttention::Informational,
                            });
                    window::request_user_attention(iced_id, urgency)
                } else {
                    Task::none()
                }
            }
            "show_system_menu" => {
                #[cfg(not(target_os = "windows"))]
                log::warn!("show_system_menu is only supported on Windows");
                if let Some(&iced_id) = self.windows.get_iced(window_id) {
                    window::show_system_menu(iced_id)
                } else {
                    Task::none()
                }
            }
            "set_resizable" => {
                if let Some(&iced_id) = self.windows.get_iced(window_id) {
                    let resizable = settings
                        .get("resizable")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true);
                    window::set_resizable(iced_id, resizable)
                } else {
                    Task::none()
                }
            }
            "set_min_size" => {
                if let Some(&iced_id) = self.windows.get_iced(window_id) {
                    let size = parse_optional_size(settings);
                    window::set_min_size(iced_id, size)
                } else {
                    Task::none()
                }
            }
            "set_max_size" => {
                if let Some(&iced_id) = self.windows.get_iced(window_id) {
                    let size = parse_optional_size(settings);
                    window::set_max_size(iced_id, size)
                } else {
                    Task::none()
                }
            }
            // Platform: X11 uses SHAPE extension, Windows uses
            // WS_EX_TRANSPARENT, macOS uses ignoresMouseEvents. On Wayland,
            // support depends on the compositor -- may silently no-op.
            "mouse_passthrough" => {
                if let Some(&iced_id) = self.windows.get_iced(window_id) {
                    let enabled = settings
                        .get("enabled")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true);
                    if enabled {
                        window::enable_mouse_passthrough(iced_id)
                    } else {
                        window::disable_mouse_passthrough(iced_id)
                    }
                } else {
                    Task::none()
                }
            }
            // -- Query operations: return results as effect_response --
            "get_size" => {
                if let Some(&iced_id) = self.windows.get_iced(window_id) {
                    let wid = window_id.to_string();
                    let req_id = settings.get("request_id").cloned();
                    window::size(iced_id).map(move |size| {
                        let mut data = serde_json::json!({
                            "width": size.width,
                            "height": size.height,
                            "op": "get_size",
                        });
                        if let Some(rid) = &req_id {
                            data["request_id"] = rid.clone();
                        }
                        let resp = toddy_core::protocol::EffectResponse::ok(wid.clone(), data);
                        if let Err(e) = emit_effect_response(resp) {
                            log::error!("write error: {e}");
                        }
                        Message::NoOp
                    })
                } else {
                    Task::none()
                }
            }
            "get_position" => {
                if let Some(&iced_id) = self.windows.get_iced(window_id) {
                    let wid = window_id.to_string();
                    let req_id = settings.get("request_id").cloned();
                    window::position(iced_id).map(move |pos| {
                        let mut data = match pos {
                            Some(p) => {
                                serde_json::json!({"x": p.x, "y": p.y, "op": "get_position"})
                            }
                            None => serde_json::json!({"op": "get_position"}),
                        };
                        if let Some(rid) = &req_id {
                            data["request_id"] = rid.clone();
                        }
                        let resp = toddy_core::protocol::EffectResponse::ok(wid.clone(), data);
                        if let Err(e) = emit_effect_response(resp) {
                            log::error!("write error: {e}");
                        }
                        Message::NoOp
                    })
                } else {
                    Task::none()
                }
            }
            "get_mode" => {
                if let Some(&iced_id) = self.windows.get_iced(window_id) {
                    let wid = window_id.to_string();
                    let req_id = settings.get("request_id").cloned();
                    window::mode(iced_id).map(move |mode| {
                        let mode_str = match mode {
                            window::Mode::Windowed => "windowed",
                            window::Mode::Fullscreen => "fullscreen",
                            window::Mode::Hidden => "hidden",
                        };
                        let mut data = serde_json::json!({
                            "mode": mode_str,
                            "op": "get_mode",
                        });
                        if let Some(rid) = &req_id {
                            data["request_id"] = rid.clone();
                        }
                        let resp = toddy_core::protocol::EffectResponse::ok(wid.clone(), data);
                        if let Err(e) = emit_effect_response(resp) {
                            log::error!("write error: {e}");
                        }
                        Message::NoOp
                    })
                } else {
                    Task::none()
                }
            }
            "get_scale_factor" => {
                if let Some(&iced_id) = self.windows.get_iced(window_id) {
                    let wid = window_id.to_string();
                    let req_id = settings.get("request_id").cloned();
                    window::scale_factor(iced_id).map(move |factor| {
                        let mut data = serde_json::json!({
                            "scale_factor": factor,
                            "op": "get_scale_factor",
                        });
                        if let Some(rid) = &req_id {
                            data["request_id"] = rid.clone();
                        }
                        let resp = toddy_core::protocol::EffectResponse::ok(wid.clone(), data);
                        if let Err(e) = emit_effect_response(resp) {
                            log::error!("write error: {e}");
                        }
                        Message::NoOp
                    })
                } else {
                    Task::none()
                }
            }
            "is_maximized" => {
                if let Some(&iced_id) = self.windows.get_iced(window_id) {
                    let wid = window_id.to_string();
                    let req_id = settings.get("request_id").cloned();
                    window::is_maximized(iced_id).map(move |val| {
                        let mut data = serde_json::json!({
                            "maximized": val,
                            "op": "is_maximized",
                        });
                        if let Some(rid) = &req_id {
                            data["request_id"] = rid.clone();
                        }
                        let resp = toddy_core::protocol::EffectResponse::ok(wid.clone(), data);
                        if let Err(e) = emit_effect_response(resp) {
                            log::error!("write error: {e}");
                        }
                        Message::NoOp
                    })
                } else {
                    Task::none()
                }
            }
            "is_minimized" => {
                if let Some(&iced_id) = self.windows.get_iced(window_id) {
                    let wid = window_id.to_string();
                    let req_id = settings.get("request_id").cloned();
                    window::is_minimized(iced_id).map(move |val| {
                        let mut data = serde_json::json!({
                            "minimized": val,
                            "op": "is_minimized",
                        });
                        if let Some(rid) = &req_id {
                            data["request_id"] = rid.clone();
                        }
                        let resp = toddy_core::protocol::EffectResponse::ok(wid.clone(), data);
                        if let Err(e) = emit_effect_response(resp) {
                            log::error!("write error: {e}");
                        }
                        Message::NoOp
                    })
                } else {
                    Task::none()
                }
            }
            "screenshot" => {
                if let Some(&iced_id) = self.windows.get_iced(window_id) {
                    let wid = window_id.to_string();
                    let req_id = settings.get("request_id").cloned();
                    window::screenshot(iced_id).map(move |screenshot| {
                        let rgba_b64 = {
                            Some(base64::engine::general_purpose::STANDARD.encode(&screenshot.rgba))
                        };

                        let mut data = serde_json::json!({
                            "width": screenshot.size.width,
                            "height": screenshot.size.height,
                            "bytes_len": screenshot.rgba.len(),
                            "op": "screenshot",
                        });
                        if let Some(b64) = rgba_b64 {
                            data["rgba"] = serde_json::json!(b64);
                        }
                        if let Some(rid) = &req_id {
                            data["request_id"] = rid.clone();
                        }
                        let resp = toddy_core::protocol::EffectResponse::ok(wid.clone(), data);
                        if let Err(e) = emit_effect_response(resp) {
                            log::error!("write error: {e}");
                        }
                        Message::NoOp
                    })
                } else {
                    Task::none()
                }
            }
            // Icon size recommendations:
            // - Windows: 32x32 or 64x64 RGBA (smaller icons are upscaled)
            // - macOS: 512x512 or 1024x1024 (macOS uses larger icons)
            // - Linux: 32x32 or 48x48 (depends on WM/DE)
            // All platforms: square, power-of-two dimensions recommended.
            "set_icon" => {
                warn_wayland_noop("set_icon");
                if let Some(&iced_id) = self.windows.get_iced(window_id) {
                    let icon_data_b64 = settings
                        .get("icon_data")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let width = settings.get("width").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
                    let height =
                        settings.get("height").and_then(|v| v.as_u64()).unwrap_or(0) as u32;

                    const MAX_ICON_DIMENSION: u32 = 1024;

                    if width == 0 || height == 0 {
                        log::error!("set_icon: zero dimension ({}x{})", width, height);
                        return Task::none();
                    }
                    if width > MAX_ICON_DIMENSION || height > MAX_ICON_DIMENSION {
                        log::error!(
                            "set_icon: dimensions {}x{} exceed maximum {}",
                            width,
                            height,
                            MAX_ICON_DIMENSION
                        );
                        return Task::none();
                    }
                    if width != height {
                        log::warn!(
                            "set_icon: non-square icon ({}x{}); some platforms may render poorly",
                            width,
                            height
                        );
                    }

                    match base64::engine::general_purpose::STANDARD.decode(icon_data_b64) {
                        Ok(rgba) => {
                            let expected_len = match (width as usize)
                                .checked_mul(height as usize)
                                .and_then(|v| v.checked_mul(4))
                            {
                                Some(len) => len,
                                None => {
                                    log::error!(
                                        "set_icon: dimensions {}x{} would overflow",
                                        width,
                                        height
                                    );
                                    return Task::none();
                                }
                            };
                            if rgba.len() != expected_len {
                                log::error!(
                                    "set_icon: expected {} bytes ({}x{}x4), got {}",
                                    expected_len,
                                    width,
                                    height,
                                    rgba.len()
                                );
                                return Task::none();
                            }
                            match window::icon::from_rgba(rgba, width, height) {
                                Ok(icon) => window::set_icon(iced_id, icon),
                                Err(e) => {
                                    log::error!("set_icon: icon creation failed: {e}");
                                    Task::none()
                                }
                            }
                        }
                        Err(e) => {
                            log::error!("set_icon: base64 decode failed: {e}");
                            Task::none()
                        }
                    }
                } else {
                    Task::none()
                }
            }
            "raw_id" => {
                if let Some(&iced_id) = self.windows.get_iced(window_id) {
                    let wid = window_id.to_string();
                    let req_id = settings.get("request_id").cloned();
                    window::raw_id::<Message>(iced_id).map(move |raw| {
                        let mut data = serde_json::json!({
                            "raw_id": raw,
                            "op": "raw_id",
                            "platform": std::env::consts::OS,
                        });
                        if let Some(rid) = &req_id {
                            data["request_id"] = rid.clone();
                        }
                        let resp = toddy_core::protocol::EffectResponse::ok(wid.clone(), data);
                        if let Err(e) = emit_effect_response(resp) {
                            log::error!("write error: {e}");
                        }
                        Message::NoOp
                    })
                } else {
                    Task::none()
                }
            }
            // Returns logical dimensions (physical pixels / scale_factor).
            // On HiDPI displays, these are smaller than the actual pixel count.
            "monitor_size" => {
                if let Some(&iced_id) = self.windows.get_iced(window_id) {
                    let wid = window_id.to_string();
                    let req_id = settings.get("request_id").cloned();
                    window::monitor_size(iced_id).map(move |size_opt| {
                        let mut data = match size_opt {
                            Some(size) => serde_json::json!({
                                "width": size.width,
                                "height": size.height,
                                "op": "monitor_size",
                            }),
                            None => serde_json::json!({"op": "monitor_size"}),
                        };
                        if let Some(rid) = &req_id {
                            data["request_id"] = rid.clone();
                        }
                        let resp = toddy_core::protocol::EffectResponse::ok(wid.clone(), data);
                        if let Err(e) = emit_effect_response(resp) {
                            log::error!("write error: {e}");
                        }
                        Message::NoOp
                    })
                } else {
                    Task::none()
                }
            }
            // -- System queries: not window-specific, use tag from settings --
            "get_system_theme" => {
                let tag = settings
                    .get("tag")
                    .and_then(|v| v.as_str())
                    .unwrap_or("system_theme")
                    .to_string();
                iced::system::theme().map(move |mode| {
                    let mode_str = match mode {
                        iced::theme::Mode::Light => "light",
                        iced::theme::Mode::Dark => "dark",
                        iced::theme::Mode::None => "none",
                    };
                    if let Err(e) =
                        emit_query_response("system_theme", &tag, serde_json::json!(mode_str))
                    {
                        log::error!("write error: {e}");
                    }
                    Message::NoOp
                })
            }
            #[cfg(not(target_arch = "wasm32"))]
            "get_system_info" => {
                let tag = settings
                    .get("tag")
                    .and_then(|v| v.as_str())
                    .unwrap_or("system_info")
                    .to_string();
                iced::system::information().map(move |info| {
                    let data = serde_json::json!({
                        "system_name": info.system_name,
                        "system_kernel": info.system_kernel,
                        "system_version": info.system_version,
                        "system_short_version": info.system_short_version,
                        "cpu_brand": info.cpu_brand,
                        "cpu_cores": info.cpu_cores,
                        "memory_total": info.memory_total,
                        "memory_used": info.memory_used,
                        "graphics_backend": info.graphics_backend,
                        "graphics_adapter": info.graphics_adapter,
                    });
                    if let Err(e) = emit_query_response("system_info", &tag, data) {
                        log::error!("write error: {e}");
                    }
                    Message::NoOp
                })
            }
            "set_resize_increments" => {
                if let Some(&iced_id) = self.windows.get_iced(window_id) {
                    let w = settings
                        .get("width")
                        .and_then(|v| v.as_f64())
                        .map(|v| v as f32);
                    let h = settings
                        .get("height")
                        .and_then(|v| v.as_f64())
                        .map(|v| v as f32);
                    let increments = match (w, h) {
                        (Some(w), Some(h)) => Some(Size::new(w, h)),
                        _ => None,
                    };
                    window::set_resize_increments(iced_id, increments)
                } else {
                    Task::none()
                }
            }
            "allow_automatic_tabbing" => {
                let enabled = settings
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(true);
                window::allow_automatic_tabbing(enabled)
            }
            other => {
                log::warn!("unknown window_op: {other}");
                Task::none()
            }
        }
    }

    /// Compare the set of window nodes in the tree against the currently open
    /// windows and open/close as needed.
    pub fn sync_windows(&mut self) -> Task<Message> {
        let tree_windows: HashSet<String> = self.core.tree.window_ids().into_iter().collect();
        let open_windows: HashSet<String> = self.windows.toddy_ids().cloned().collect();

        let mut tasks = Vec::new();

        // Open windows that exist in the tree but are not yet open.
        for win_id in &tree_windows {
            if !open_windows.contains(win_id) {
                let settings = self.window_settings_for(win_id);
                let initial_decorations = settings.decorations;
                let (iced_id, open_task) = window::open(settings);
                self.windows.insert(win_id.clone(), iced_id);
                self.windows.set_decorated(win_id, initial_decorations);

                let toddy_id = win_id.clone();
                tasks.push(open_task.map(move |id| Message::WindowOpened(id, toddy_id.clone())));
            }
        }

        // Close windows that are open but no longer in the tree.
        for win_id in &open_windows {
            if !tree_windows.contains(win_id)
                && let Some(iced_id) = self.windows.remove_by_toddy(win_id)
            {
                tasks.push(window::close(iced_id));
            }
        }

        Task::batch(tasks)
    }

    /// Build window::Settings from a window node's props.
    pub fn window_settings_for(&self, toddy_id: &str) -> window::Settings {
        if let Some(node) = self.core.tree.find_window(toddy_id) {
            parse_window_settings(&node.props)
        } else {
            window::Settings {
                size: Size::new(800.0, 600.0),
                ..window::Settings::default()
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Settings / enum parsing helpers
// ---------------------------------------------------------------------------

/// Maximum window dimension in logical pixels.
const MAX_WINDOW_DIM: f32 = 16384.0;

/// Parse a full `window::Settings` from a JSON value (node props or op settings).
pub fn parse_window_settings(v: &serde_json::Value) -> window::Settings {
    let mut width = v.get("width").and_then(|v| v.as_f64()).unwrap_or(800.0) as f32;
    let mut height = v.get("height").and_then(|v| v.as_f64()).unwrap_or(600.0) as f32;
    if !(1.0..=MAX_WINDOW_DIM).contains(&width) {
        log::warn!("window width {width} out of range, clamping to 1.0..={MAX_WINDOW_DIM}");
        width = width.clamp(1.0, MAX_WINDOW_DIM);
    }
    if !(1.0..=MAX_WINDOW_DIM).contains(&height) {
        log::warn!("window height {height} out of range, clamping to 1.0..={MAX_WINDOW_DIM}");
        height = height.clamp(1.0, MAX_WINDOW_DIM);
    }

    let maximized = v
        .get("maximized")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let fullscreen = v
        .get("fullscreen")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let visible = v.get("visible").and_then(|v| v.as_bool()).unwrap_or(true);
    let resizable = v.get("resizable").and_then(|v| v.as_bool()).unwrap_or(true);
    let closeable = v.get("closeable").and_then(|v| v.as_bool()).unwrap_or(true);
    let minimizable = v
        .get("minimizable")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let decorations = v
        .get("decorations")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let transparent = v
        .get("transparent")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let blur = v.get("blur").and_then(|v| v.as_bool()).unwrap_or(false);
    let exit_on_close_request = v
        .get("exit_on_close_request")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    let position = match v.get("position") {
        Some(serde_json::Value::String(s)) if s == "centered" => window::Position::Centered,
        Some(obj) if obj.is_object() => {
            const MAX_POS: f32 = 32768.0;
            let mut x = obj.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
            let mut y = obj.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
            if !(-MAX_POS..=MAX_POS).contains(&x) {
                log::warn!(
                    "window position x={x} out of range, clamping to -{MAX_POS}..={MAX_POS}"
                );
                x = x.clamp(-MAX_POS, MAX_POS);
            }
            if !(-MAX_POS..=MAX_POS).contains(&y) {
                log::warn!(
                    "window position y={y} out of range, clamping to -{MAX_POS}..={MAX_POS}"
                );
                y = y.clamp(-MAX_POS, MAX_POS);
            }
            warn_wayland_noop("position");
            window::Position::Specific(Point::new(x, y))
        }
        _ => window::Position::default(),
    };

    let min_size = parse_optional_size(v.get("min_size").unwrap_or(&serde_json::Value::Null));
    let max_size = parse_optional_size(v.get("max_size").unwrap_or(&serde_json::Value::Null));

    let level = parse_window_level(v.get("level").and_then(|v| v.as_str()).unwrap_or("normal"));

    window::Settings {
        size: Size::new(width, height),
        maximized,
        fullscreen,
        position,
        min_size,
        max_size,
        visible,
        resizable,
        closeable,
        minimizable,
        decorations,
        transparent,
        blur,
        level,
        exit_on_close_request,
        ..window::Settings::default()
    }
}

fn parse_optional_size(v: &serde_json::Value) -> Option<Size> {
    let w = v.get("width").and_then(|v| v.as_f64())? as f32;
    let h = v.get("height").and_then(|v| v.as_f64())? as f32;
    Some(Size::new(w, h))
}

fn parse_window_level(s: &str) -> window::Level {
    match s {
        "always_on_top" => window::Level::AlwaysOnTop,
        "always_on_bottom" => window::Level::AlwaysOnBottom,
        _ => window::Level::Normal,
    }
}

/// Parse a window mode from a JSON value.
///
/// Supported modes: `"windowed"` (default), `"fullscreen"`, `"hidden"`.
///
/// Note: `"fullscreen"` maps to borderless windowed fullscreen (not
/// exclusive fullscreen). This is the iced default and provides better
/// multi-monitor behavior and faster alt-tab than exclusive mode.
fn parse_window_mode(v: &serde_json::Value) -> window::Mode {
    match v.get("mode").and_then(|v| v.as_str()).unwrap_or("windowed") {
        "fullscreen" => window::Mode::Fullscreen,
        "hidden" => window::Mode::Hidden,
        _ => window::Mode::Windowed,
    }
}

fn parse_direction(s: &str) -> window::Direction {
    match s {
        "north" => window::Direction::North,
        "south" => window::Direction::South,
        "east" => window::Direction::East,
        "west" => window::Direction::West,
        "north_east" => window::Direction::NorthEast,
        "north_west" => window::Direction::NorthWest,
        "south_east" => window::Direction::SouthEast,
        "south_west" => window::Direction::SouthWest,
        _ => window::Direction::SouthEast,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::{Size, window};
    use serde_json::json;

    #[test]
    fn parse_window_settings_defaults() {
        let settings = parse_window_settings(&json!({}));
        assert_eq!(settings.size, Size::new(800.0, 600.0));
        assert!(settings.visible);
        assert!(settings.resizable);
        assert!(settings.decorations);
        assert!(!settings.maximized);
        assert!(!settings.fullscreen);
        assert!(!settings.transparent);
    }

    #[test]
    fn parse_window_settings_custom_size() {
        let settings = parse_window_settings(&json!({"width": 1024, "height": 768}));
        assert_eq!(settings.size, Size::new(1024.0, 768.0));
    }

    #[test]
    fn parse_window_settings_centered_position() {
        let settings = parse_window_settings(&json!({"position": "centered"}));
        assert!(matches!(settings.position, window::Position::Centered));
    }

    #[test]
    fn parse_window_settings_specific_position() {
        let settings = parse_window_settings(&json!({"position": {"x": 100, "y": 200}}));
        match settings.position {
            window::Position::Specific(p) => {
                assert_eq!(p.x, 100.0);
                assert_eq!(p.y, 200.0);
            }
            _ => panic!("expected Specific position"),
        }
    }

    #[test]
    fn parse_window_settings_boolean_flags() {
        let settings = parse_window_settings(&json!({
            "maximized": true,
            "transparent": true,
            "decorations": false,
            "resizable": false,
        }));
        assert!(settings.maximized);
        assert!(settings.transparent);
        assert!(!settings.decorations);
        assert!(!settings.resizable);
    }

    #[test]
    fn parse_optional_size_from_object() {
        let sz = parse_optional_size(&json!({"width": 100, "height": 200}));
        assert_eq!(sz, Some(Size::new(100.0, 200.0)));
    }

    #[test]
    fn parse_optional_size_null() {
        let sz = parse_optional_size(&json!(null));
        assert_eq!(sz, None);
    }

    #[test]
    fn parse_window_level_variants() {
        assert!(matches!(
            parse_window_level("always_on_top"),
            window::Level::AlwaysOnTop
        ));
        assert!(matches!(
            parse_window_level("always_on_bottom"),
            window::Level::AlwaysOnBottom
        ));
        assert!(matches!(
            parse_window_level("normal"),
            window::Level::Normal
        ));
        assert!(matches!(
            parse_window_level("unknown"),
            window::Level::Normal
        ));
    }

    #[test]
    fn parse_window_mode_variants() {
        assert!(matches!(
            parse_window_mode(&json!({"mode": "fullscreen"})),
            window::Mode::Fullscreen
        ));
        assert!(matches!(
            parse_window_mode(&json!({"mode": "hidden"})),
            window::Mode::Hidden
        ));
        assert!(matches!(
            parse_window_mode(&json!({"mode": "windowed"})),
            window::Mode::Windowed
        ));
        assert!(matches!(
            parse_window_mode(&json!({})),
            window::Mode::Windowed
        ));
    }

    #[test]
    fn parse_direction_variants() {
        assert!(matches!(parse_direction("north"), window::Direction::North));
        assert!(matches!(
            parse_direction("south_west"),
            window::Direction::SouthWest
        ));
        assert!(matches!(
            parse_direction("invalid"),
            window::Direction::SouthEast
        ));
    }
}
