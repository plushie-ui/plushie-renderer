//! Renderer entry point. Parses CLI flags, reads the initial Settings
//! message, spawns the stdin reader, and starts the iced daemon.

use std::sync::Mutex;

use iced::{Subscription, Task};

use toddy_core::codec::Codec;
use toddy_core::message::{Message, StdinEvent};
use toddy_core::protocol::IncomingMessage;

use toddy_renderer::App;
use toddy_renderer::emitters::emit_hello;

use super::stdin::{STDIN_RX, read_initial_settings, spawn_stdin_reader};

pub(crate) fn run(builder: toddy_core::app::ToddyAppBuilder) -> iced::Result {
    let args: Vec<String> = std::env::args().collect();

    // Levelled logging via RUST_LOG. Default: warn (quiet). Use
    // RUST_LOG=toddy=debug (or =info, =trace) for more output.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    // Parse codec flags early so all modes (headless, test, normal) can use them.
    let has_flag = |flag: &str| args.iter().any(|a| a == flag);
    let forced_codec = if has_flag("--msgpack") {
        Some(Codec::MsgPack)
    } else if has_flag("--json") {
        Some(Codec::Json)
    } else {
        None
    };

    // Parse --max-sessions N for concurrent session support.
    let max_sessions = args
        .windows(2)
        .find(|w| w[0] == "--max-sessions")
        .and_then(|w| w[1].parse::<usize>().ok())
        .unwrap_or(1)
        .max(1);

    // Parse --exec <command> for transport selection.
    let exec_command = args
        .windows(2)
        .find(|w| w[0] == "--exec")
        .map(|w| w[1].clone());

    // Create transport: exec if --exec is present, otherwise stdio.
    let transport = if let Some(cmd) = &exec_command {
        match crate::transport::Transport::exec(cmd) {
            Ok(t) => t,
            Err(e) => {
                log::error!("failed to start exec transport: {e}");
                return Ok(());
            }
        }
    } else {
        // Windows binary mode only needed for stdio transport.
        #[cfg(windows)]
        set_binary_mode();
        crate::transport::Transport::stdio()
    };

    let transport_name = transport.name();
    let (reader, writer, _transport_guard) = transport.into_parts();

    // Initialize the global output writer before any protocol I/O.
    let is_headless = has_flag("--headless") || has_flag("--mock");
    if is_headless {
        toddy_renderer::emitters::init_output(writer);
    } else {
        let channel_writer = crate::output::spawn_writer_thread(writer);
        toddy_renderer::emitters::init_output(Box::new(channel_writer));
    }

    // Collect extension keys before building the dispatcher so the hello
    // message can include them in all modes.
    let ext_keys = builder
        .extension_keys()
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();

    if has_flag("--mock") {
        crate::headless::run(
            forced_codec,
            builder.build_dispatcher(),
            crate::headless::Mode::Mock,
            max_sessions,
            &ext_keys,
            transport_name,
            reader,
        );
        return Ok(());
    }
    if has_flag("--headless") {
        crate::headless::run(
            forced_codec,
            builder.build_dispatcher(),
            crate::headless::Mode::Headless,
            max_sessions,
            &ext_keys,
            transport_name,
            reader,
        );
        return Ok(());
    }

    // Read the first message synchronously to get iced settings and font
    // data before the daemon starts.
    let (initial_settings, iced_settings, font_bytes, reader) =
        read_initial_settings(forced_codec, reader);

    // Send the hello handshake before any other output.
    let ext_key_refs: Vec<&str> = ext_keys.iter().map(|s| s.as_str()).collect();
    if let Err(e) = emit_hello("windowed", "wgpu", &ext_key_refs, transport_name) {
        log::error!("failed to emit hello: {e}");
        return Ok(());
    }

    // Spawn stdin reader thread with tokio channel.
    let (tx, rx) = tokio::sync::mpsc::channel::<StdinEvent>(64);
    spawn_stdin_reader(tx, reader);
    *STDIN_RX.lock().expect("STDIN_RX lock poisoned") = Some(rx);

    let settings_slot: Mutex<Option<(serde_json::Value, Vec<Vec<u8>>)>> =
        Mutex::new(Some((initial_settings, font_bytes)));
    let builder_slot: Mutex<Option<toddy_core::app::ToddyAppBuilder>> = Mutex::new(Some(builder));

    iced::daemon(
        move || {
            let (settings, fonts) = settings_slot
                .lock()
                .expect("settings_slot lock poisoned")
                .take()
                .unwrap_or_default();

            let dispatcher = builder_slot
                .lock()
                .expect("builder_slot lock poisoned")
                .take()
                .expect("daemon init closure called more than once")
                .build_dispatcher();

            let effect_handler = Box::new(crate::effects::NativeEffectHandler);
            let mut app = App::new(dispatcher, effect_handler);

            // Extract scale_factor before applying settings to Core
            app.scale_factor = toddy_renderer::app::validate_scale_factor(
                settings
                    .get("scale_factor")
                    .and_then(|v| v.as_f64())
                    .map(toddy_core::prop_helpers::f64_to_f32)
                    .unwrap_or(1.0),
            );

            // Apply initial settings to Core.
            let effects = app.core.apply(IncomingMessage::Settings { settings });
            for effect in effects {
                match effect {
                    toddy_core::engine::CoreEffect::ExtensionConfig(config) => {
                        app.dispatcher.init_all(
                            &config,
                            &app.theme,
                            app.core.default_text_size,
                            app.core.default_font,
                        );
                    }
                    other => {
                        log::warn!("unexpected effect from initial Settings: {other:?}");
                    }
                }
            }

            // Build font load tasks
            let font_tasks: Vec<Task<Message>> = fonts
                .into_iter()
                .map(|bytes| {
                    iced::font::load(bytes).map(|result| {
                        if let Err(e) = result {
                            log::error!("font load error: {e:?}");
                        }
                        Message::NoOp
                    })
                })
                .collect();

            let task = if font_tasks.is_empty() {
                Task::none()
            } else {
                Task::batch(font_tasks)
            };

            (app, task)
        },
        App::update,
        App::view_window,
    )
    .title(App::title_for_window)
    .subscription(|app: &App| {
        Subscription::batch([
            app.renderer_subscriptions(),
            Subscription::run(super::stdin::stdin_subscription).map(Message::Stdin),
        ])
    })
    .theme(App::theme_for_window)
    .scale_factor(App::scale_factor_for_window)
    .settings(iced_settings)
    .run()
}

/// Switch stdin and stdout to binary mode on Windows.
#[cfg(windows)]
#[allow(unsafe_code)]
fn set_binary_mode() {
    extern "C" {
        fn _setmode(fd: i32, mode: i32) -> i32;
    }
    const O_BINARY: i32 = 0x8000;

    unsafe {
        _setmode(0, O_BINARY);
        _setmode(1, O_BINARY);
    }
}
