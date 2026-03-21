//! Shared settings parsing logic for toddy renderer startup.
//!
//! Both native and WASM entry points need to extract iced settings,
//! font data, and validation flags from the host's Settings JSON message.
//! This module centralizes that parsing so each platform only handles
//! platform-specific concerns (file I/O, environment variables, etc.).

use serde_json::Value;

/// Parse iced-level settings from the host's Settings JSON.
///
/// Extracts `antialiasing`, `vsync`, `default_text_size`, and
/// `default_font` from the settings object and returns a configured
/// `iced::Settings`. Fields that are absent or have invalid types
/// fall back to iced defaults (antialiasing off, vsync on).
///
/// Called early in the startup flow, before the iced daemon launches.
pub fn parse_iced_settings(settings: &Value) -> iced::Settings {
    let antialiasing = settings
        .get("antialiasing")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let vsync = settings
        .get("vsync")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let default_text_size = settings
        .get("default_text_size")
        .and_then(|v| v.as_f64())
        .map(|s| iced::Pixels(s as f32));
    let default_font = settings.get("default_font").map(|v| {
        let family = v.get("family").and_then(|f| f.as_str());
        if family == Some("monospace") {
            iced::Font::MONOSPACE
        } else {
            iced::Font::DEFAULT
        }
    });

    let mut iced_settings = iced::Settings {
        antialiasing,
        vsync,
        ..Default::default()
    };
    if let Some(size) = default_text_size {
        iced_settings.default_text_size = size;
    }
    if let Some(font) = default_font {
        iced_settings.default_font = font;
    }
    iced_settings
}

/// Enable prop validation if the host requested it.
///
/// Checks for `validate_props: true` in the settings JSON and, if
/// present, enables debug-mode prop validation globally via
/// `toddy_core::widgets::set_validate_props`. The flag is backed by
/// a `OnceLock` and can only be set once per process lifetime.
///
/// Called during startup, after the Settings message is parsed.
pub fn apply_validate_props(settings: &Value) {
    if settings
        .get("validate_props")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        toddy_core::widgets::set_validate_props(true);
        log::info!("prop validation enabled via settings");
    }
}

/// Decode font data from a JSON value.
///
/// Supports two wire formats:
///
/// - **String**: base64-encoded binary data. This is the standard JSON
///   wire format used when the host sends font bytes as a string field.
/// - **Array of numbers**: raw byte values (0-255). This is the format
///   that results when MessagePack binary data passes through the
///   codec's `rmpv -> serde_json` conversion path.
///
/// Returns `None` if the value is neither a string nor an array, if
/// base64 decoding fails, or if an array element is not a valid u8.
pub fn decode_font_data(value: &Value) -> Option<Vec<u8>> {
    match value {
        Value::String(s) => {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD.decode(s).ok()
        }
        Value::Array(arr) => {
            let bytes: Vec<u8> = arr
                .iter()
                .filter_map(|v| v.as_u64().map(|n| n as u8))
                .collect();
            if bytes.len() == arr.len() {
                Some(bytes)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Parse inline font data from the `fonts` array in settings.
///
/// Each element in the `fonts` array can be:
///
/// - A plain string representing a file path. These are skipped here
///   and handled by platform-specific code (native reads from disk,
///   WASM fetches over HTTP, etc.).
/// - An object with a `data` field containing inline font bytes,
///   decoded via [`decode_font_data`].
///
/// Returns a `Vec` of decoded font byte buffers, ready to be passed
/// to `iced::font::load`. Called during startup after the Settings
/// message is parsed, before the iced daemon launches.
pub fn parse_inline_fonts(settings: &Value) -> Vec<Vec<u8>> {
    let Some(fonts) = settings.get("fonts").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let mut result = Vec::new();
    for font_val in fonts {
        if let Some(obj) = font_val.as_object()
            && let Some(data_val) = obj.get("data")
        {
            match decode_font_data(data_val) {
                Some(bytes) if bytes.is_empty() => {
                    log::warn!("fonts: empty inline font data, skipping");
                }
                Some(bytes) => {
                    log::info!("loaded inline font ({} bytes)", bytes.len());
                    result.push(bytes);
                }
                None => {
                    log::warn!("fonts: failed to decode inline font data");
                }
            }
        }
        // Plain strings are file paths, handled by platform-specific code
    }
    result
}
