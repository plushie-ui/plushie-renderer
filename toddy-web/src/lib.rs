//! WASM entry point for the toddy renderer.
//!
//! Provides a `wasm-bindgen` API for running toddy in the browser.
//! Uses `iced::daemon` with a canvas-based backend and communicates
//! with the host via JavaScript callbacks.
//!
//! # Entry point
//!
//! [`run_app(settings_json, on_event)`](run_app) is the single entry
//! point. It parses the settings JSON, validates the protocol version,
//! emits the hello handshake, and starts the iced daemon.
//!
//! # Limitations
//!
//! - Platform effects (file dialogs, clipboard, notifications) are
//!   stubbed as unsupported.
//! - Message ingestion (sending Snapshots/Patches after startup) is
//!   not yet implemented. The initial settings are applied at startup;
//!   runtime tree updates require an async channel that hasn't been
//!   wired yet.

mod effects;
mod output;

use wasm_bindgen::prelude::*;

use toddy_core::codec::Codec;
use toddy_core::protocol::IncomingMessage;

use toddy_renderer::App;
use toddy_renderer::emitters::{emit_hello, init_output};

use effects::WebEffectHandler;
use output::WebOutputWriter;

/// Run the iced daemon with the given settings JSON and event callback.
///
/// This is the single entry point for the WASM renderer. It validates
/// the protocol version, emits the hello handshake, and starts the
/// full iced rendering loop. The returned Future is driven by the
/// browser's requestAnimationFrame loop.
#[wasm_bindgen]
pub async fn run_app(settings_json: &str, on_event: js_sys::Function) -> Result<(), JsValue> {
    console_log::init_with_level(log::Level::Warn).ok();

    // Set up output
    let writer = WebOutputWriter::new(on_event);
    init_output(Box::new(writer));
    Codec::set_global(Codec::Json);

    // Parse initial settings
    let settings: serde_json::Value = serde_json::from_str(settings_json)
        .map_err(|e| JsValue::from_str(&format!("invalid settings JSON: {e}")))?;

    // Validate protocol version
    let expected = u64::from(toddy_core::protocol::PROTOCOL_VERSION);
    if let Some(version) = settings.get("protocol_version").and_then(|v| v.as_u64())
        && version != expected
    {
        return Err(JsValue::from_str(&format!(
            "protocol version mismatch: expected {expected}, got {version}"
        )));
    }

    // Emit hello
    emit_hello("web", "wgpu", &[], "wasm")
        .map_err(|e| JsValue::from_str(&format!("failed to emit hello: {e}")))?;

    // Build the app inside a Mutex so the Fn closure can move it out once.
    let app_slot: std::sync::Mutex<Option<(serde_json::Value, toddy_core::app::ToddyAppBuilder)>> =
        std::sync::Mutex::new(Some((settings, toddy_core::app::ToddyAppBuilder::new())));

    // Start the iced daemon
    iced::daemon(
        move || {
            let (settings, builder) = app_slot
                .lock()
                .expect("app_slot lock poisoned")
                .take()
                .expect("daemon init closure called more than once");

            let dispatcher = builder.build_dispatcher();
            let effect_handler = Box::new(WebEffectHandler);
            let mut app = App::new(dispatcher, effect_handler);

            // Extract scale_factor before applying settings to Core.
            app.scale_factor = toddy_renderer::validate_scale_factor(
                settings
                    .get("scale_factor")
                    .and_then(|v| v.as_f64())
                    .map(toddy_core::prop_helpers::f64_to_f32)
                    .unwrap_or(1.0),
            );

            let effects = app.core.apply(IncomingMessage::Settings { settings });
            for effect in effects {
                if let toddy_core::engine::CoreEffect::ExtensionConfig(config) = effect {
                    app.dispatcher.init_all(
                        &config,
                        &app.theme,
                        app.core.default_text_size,
                        app.core.default_font,
                    );
                }
            }

            (app, iced::Task::none())
        },
        App::update,
        App::view_window,
    )
    .title(App::title_for_window)
    .subscription(App::renderer_subscriptions)
    .theme(App::theme_for_window)
    .scale_factor(App::scale_factor_for_window)
    .run()
    .map_err(|e| JsValue::from_str(&format!("iced error: {e}")))
}
