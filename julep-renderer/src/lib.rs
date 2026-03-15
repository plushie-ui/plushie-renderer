//! # julep-renderer
//!
//! Binary crate for the Julep renderer. Three execution modes:
//!
//! - **Default:** `julep-renderer` -- Full iced::daemon with real windows.
//! - **Headless:** `julep-renderer --headless` -- Core + wire protocol only,
//!   no display server required. Useful for CI and integration testing.
//!   Requires the `headless` feature.
//! - **Test mode:** `julep-renderer --test` -- Real iced::daemon windows plus
//!   test protocol messages (Query, Interact, SnapshotCapture). Requires
//!   the `test-mode` feature.
//!
//! Wire codec auto-detection: the first byte of stdin determines the format
//! (`{` = JSON, anything else = MessagePack). Override with `--json` or
//! `--msgpack`.

#![deny(warnings)]

#[cfg(feature = "headless")]
mod headless;
mod test_mode;
mod test_protocol;

mod renderer;

/// Entry point for the julep renderer.
///
/// Extension packages create a `JulepAppBuilder`, register their extensions,
/// and pass it here. The default `main.rs` simply passes an empty builder.
pub fn run(builder: julep_core::app::JulepAppBuilder) -> iced::Result {
    renderer::run(builder)
}
