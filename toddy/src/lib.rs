//! # toddy
//!
//! Native GUI renderer binary. Three execution modes:
//!
//! - **Windowed (default):** `toddy` -- Full iced rendering with real
//!   windows and GPU. Production mode. Reports `"mode": "windowed"`.
//! - **Headless:** `toddy --headless` -- No display server. Real
//!   rendering via tiny-skia with persistent widget state. Accurate
//!   screenshots after interactions. For CI with visual verification.
//! - **Mock:** `toddy --mock` -- No rendering. Core + wire protocol
//!   only. Stub screenshots. For fast protocol-level testing from
//!   any language.
//!
//! All modes handle scripting messages (Query, Interact, TreeHash,
//! Screenshot, Reset) for programmatic inspection and interaction.
//!
//! Wire codec auto-detection: the first byte of stdin determines the format
//! (`{` = JSON, anything else = MessagePack). Override with `--json` or
//! `--msgpack`.

mod effects;
mod headless;
mod message_processing;
mod renderer;
mod scripting;
pub(crate) mod transport;

/// Entry point for the toddy renderer.
///
/// Extension packages create a `ToddyAppBuilder`, register their extensions,
/// and pass it here. The default `main.rs` simply passes an empty builder.
pub fn run(builder: toddy_core::app::ToddyAppBuilder) -> iced::Result {
    renderer::run(builder)
}
