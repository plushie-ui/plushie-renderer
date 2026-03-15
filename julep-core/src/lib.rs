//! # julep-core
//!
//! Core library for the Julep desktop GUI framework. This crate provides the
//! rendering engine, wire protocol handling, and widget infrastructure that
//! powers the `julep-renderer` binary. The host process drives state and logic;
//! this crate turns UI tree descriptions into native iced widgets.
//!
//! ## Feature flags
//!
//! Feature flags live on this crate and are re-exported by `julep-renderer`.
//!
//! **Widget features** (all enabled by `builtin-all`, which is on by default):
//! - `widget-image` -- raster image widget (`iced/image`)
//! - `widget-svg` -- SVG widget (`iced/svg`)
//! - `widget-canvas` -- 2D canvas drawing surface (`iced/canvas`)
//! - `widget-markdown` -- markdown rendering (`iced/markdown`)
//! - `widget-highlighter` -- syntax highlighting for text_editor (`iced/highlighter`)
//! - `widget-sysinfo` -- system info queries (`iced/sysinfo`)
//! - `widget-qr-code` -- QR code generation (`iced/canvas` + `qrcode`)
//!
//! **Platform effect features:**
//! - `dialogs` -- native file dialogs via `rfd`
//! - `clipboard` -- clipboard read/write via `arboard`
//! - `notifications` -- OS notifications via `notify-rust`
//!
//! Note: `headless` and `test-mode` features are defined in `julep-renderer` only,
//! as they affect the binary entrypoint (iced_test Simulator vs real windows).
//!
//! **Feature flag interactions:**
//! - `builtin-all` enables all `widget-*` features.
//! - `widget-qr-code` requires `widget-canvas` (both use `iced/canvas`).
//! - `widget-sysinfo` enables `iced/system` which pulls in additional
//!   system information dependencies.
//! - The `a11y` feature is independent and can be disabled without affecting
//!   widget features: `--no-default-features --features builtin-all,dialogs,...`
//!
//! ## Module guide
//!
//! - [`engine`] -- `Core` struct: pure state management decoupled from iced runtime
//! - [`widgets`] -- tree node to iced widget rendering (all widget types)
//! - [`protocol`] -- wire message parsing and event serialization
//! - [`message`] -- `Message` enum, keyboard/mouse serialization helpers
//! - [`tree`] -- tree data structure, patch application, window discovery
//! - [`codec`] -- wire codec: JSON + MessagePack encode/decode/framing
//! - [`theming`] -- theme resolution, custom palette parsing, hex colors
//! - [`effects`] -- platform effect handlers (file dialogs, clipboard, notifications)
//! - [`image_registry`] -- in-memory image handle storage
//! - [`overlay_widget`] -- custom `Widget` + `Overlay` impl for positioned overlays
//! - [`extensions`] -- `WidgetExtension` trait, `ExtensionDispatcher`, `ExtensionCaches`
//! - [`app`] -- `JulepAppBuilder` for registering extensions
//! - [`prop_helpers`] -- public prop extraction helpers for extension authors
//! - [`prelude`] -- common re-exports for extension authors
//! - [`testing`] -- test factory helpers for extension authors

#![deny(warnings)]

// Ensure catch_unwind works: extension panic isolation requires unwinding.
// If this fails, remove `panic = "abort"` from your Cargo profile.
#[cfg(all(not(test), panic = "abort"))]
compile_error!(
    "julep-core requires panic=\"unwind\" (the default). \
     Extension panic isolation via catch_unwind is a no-op with panic=\"abort\"."
);

pub mod app;
pub mod codec;
pub mod effects;
pub mod engine;
pub mod extensions;
pub mod image_registry;
pub mod message;
pub mod overlay_widget;
pub mod prelude;
pub mod prop_helpers;
pub mod protocol;
pub mod testing;
pub mod theming;
pub mod tree;
pub mod widgets;

#[cfg(feature = "a11y")]
pub(crate) mod a11y_widget;

// Re-export iced so extension crates can use `julep_core::iced::*` without
// adding a direct iced dependency. This avoids version conflicts when
// julep-core bumps its iced version -- extensions that use only
// `julep_core::prelude::*` and `julep_core::iced::*` get the upgrade
// automatically.
pub use iced;
