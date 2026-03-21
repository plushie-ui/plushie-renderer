//! WASM entry point for the toddy renderer.
//!
//! This crate provides a `wasm-bindgen` API for running toddy in the
//! browser. It uses `iced::daemon` with a canvas-based backend and
//! communicates with the host via JavaScript callbacks.

mod effects;
mod output;

use wasm_bindgen::prelude::*;

use toddy_core::codec::Codec;
use toddy_core::protocol::IncomingMessage;

use toddy_renderer::emitters::{emit_hello, init_output};
use toddy_renderer::App;

use effects::WebEffectHandler;
use output::WebOutputWriter;

/// Initialize the WASM toddy renderer.
///
/// `on_event` is a JavaScript callback that receives serialized event
/// strings whenever the renderer emits an outgoing event.
///
/// Returns a promise that resolves when the renderer exits (which
/// typically only happens if the host explicitly closes it).
#[wasm_bindgen]
pub fn start(on_event: js_sys::Function) -> Result<(), JsValue> {
    // Initialize console logging for WASM.
    console_log::init_with_level(log::Level::Warn).ok();

    // Set up the output writer with the JS callback.
    let writer = WebOutputWriter::new(on_event);
    init_output(Box::new(writer));

    // Use JSON codec for WASM (simpler JS interop than msgpack).
    Codec::set_global(Codec::Json);

    Ok(())
}

/// Send a JSON message to the renderer.
///
/// The message is parsed as an `IncomingMessage` and processed by the
/// renderer. This is the WASM equivalent of writing to stdin.
#[wasm_bindgen]
pub fn send_message(json: &str) -> Result<(), JsValue> {
    let _msg: serde_json::Value =
        serde_json::from_str(json).map_err(|e| JsValue::from_str(&e.to_string()))?;
    // Message processing will be wired through the daemon's update
    // loop in a future iteration. For now this validates parsing.
    Ok(())
}

/// Run the iced daemon with the given settings JSON and event callback.
///
/// This starts the full iced rendering loop. The function returns a
/// Future that is driven by the browser's requestAnimationFrame loop.
#[wasm_bindgen]
pub async fn run_app(
    settings_json: &str,
    on_event: js_sys::Function,
) -> Result<(), JsValue> {
    console_log::init_with_level(log::Level::Warn).ok();

    // Set up output
    let writer = WebOutputWriter::new(on_event);
    init_output(Box::new(writer));
    Codec::set_global(Codec::Json);

    // Parse initial settings
    let settings: serde_json::Value = serde_json::from_str(settings_json)
        .map_err(|e| JsValue::from_str(&format!("invalid settings JSON: {e}")))?;

    // Emit hello
    let _ = emit_hello("web", "wgpu", &[], "wasm");

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
