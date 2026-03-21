//! Shared renderer logic for toddy.
//!
//! This crate contains the platform-independent rendering engine that
//! processes incoming messages, dispatches iced updates, and emits
//! outgoing events. It compiles to both native and wasm32 targets.
//!
//! Platform-specific behavior (I/O, effects, sleep) is injected via
//! traits and cfg-gated dependencies. The `toddy` binary crate and
//! `toddy-web` WASM crate each provide their own implementations.

pub mod app;
pub mod apply;
pub mod constants;
pub mod emitter;
pub mod emitters;
pub mod events;
pub mod message_processing;
pub mod scripting;
pub mod subscriptions;
pub mod update;
pub mod view;
pub mod widget_ops;
pub mod window_map;
pub mod window_ops;

pub mod effects;

pub use app::{App, validate_scale_factor};
pub use effects::EffectHandler;
