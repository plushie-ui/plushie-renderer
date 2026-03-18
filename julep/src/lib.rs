//! # julep
//!
//! Native GUI renderer binary. Three execution modes:
//!
//! - **Default:** `julep` -- Full iced::daemon with real windows and
//!   GPU rendering. Production mode.
//! - **Headless:** `julep --headless` -- No display server. Real
//!   rendering via tiny-skia with persistent widget state. Accurate
//!   screenshots after interactions. For CI with visual verification.
//! - **Mock:** `julep --mock` -- No rendering. Core + wire protocol
//!   only. Stub screenshots. For fast protocol-level testing from
//!   any language.
//!
//! All modes handle scripting messages (Query, Interact, TreeHash,
//! ScreenshotCapture, Reset) for programmatic inspection and interaction.
//!
//! Wire codec auto-detection: the first byte of stdin determines the format
//! (`{` = JSON, anything else = MessagePack). Override with `--json` or
//! `--msgpack`.

#![deny(warnings)]

mod headless;
mod renderer;
mod scripting;

/// Entry point for the julep renderer.
///
/// Extension packages create a `JulepAppBuilder`, register their extensions,
/// and pass it here. The default `main.rs` simply passes an empty builder.
pub fn run(builder: julep_core::app::JulepAppBuilder) -> iced::Result {
    renderer::run(builder)
}
