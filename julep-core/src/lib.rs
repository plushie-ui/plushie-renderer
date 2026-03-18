//! # julep-core
//!
//! The public SDK for julep. Extension authors depend on this crate to
//! implement the [`WidgetExtension`](extensions::WidgetExtension) trait
//! and build custom native widgets. The [`prelude`] module re-exports
//! everything an extension needs; [`iced`] is re-exported so extensions
//! don't need a direct iced dependency.
//!
//! This crate also provides the rendering engine, wire protocol, and
//! widget infrastructure used internally by the `julep` binary.
//!
//! ## Module guide
//!
//! **Core engine:**
//! - [`engine`] -- `Core` struct: pure state management decoupled from iced runtime
//! - [`tree`] -- tree data structure, patch application, window discovery
//! - [`message`] -- `Message` enum, keyboard/mouse serialization helpers
//!
//! **Widgets:**
//! - [`widgets`] -- tree node to iced widget rendering (all widget types)
//! - [`widgets::overlay`] -- custom `Widget` + `Overlay` impl for positioned overlays
//!
//! **Protocol:**
//! - [`protocol`] -- wire message parsing and event serialization
//! - [`codec`] -- wire codec: JSON + MessagePack encode/decode/framing
//!
//! **Platform:**
//! - [`theming`] -- theme resolution, custom palette parsing, hex colors
//! - [`effects`] -- platform effect handlers (file dialogs, clipboard, notifications)
//! - [`image_registry`] -- in-memory image handle storage
//!
//! **Extension SDK:**
//! - [`extensions`] -- `WidgetExtension` trait, `ExtensionDispatcher`, `ExtensionCaches`
//! - [`app`] -- `JulepAppBuilder` for registering extensions
//! - [`prelude`] -- common re-exports for extension authors
//! - [`prop_helpers`] -- public prop extraction helpers for extension authors
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
pub mod prelude;
pub mod prop_helpers;
pub mod protocol;
pub mod testing;
pub mod theming;
pub mod tree;
pub mod widgets;

// Re-export iced so extension crates can use `julep_core::iced::*` without
// adding a direct iced dependency. This avoids version conflicts when
// julep-core bumps its iced version -- extensions that use only
// `julep_core::prelude::*` and `julep_core::iced::*` get the upgrade
// automatically.
pub use iced;
