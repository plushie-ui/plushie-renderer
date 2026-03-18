//! iced::daemon application: the main rendering loop.
//!
//! `App` owns the [`Core`](julep_core::engine::Core) state, reads
//! messages from stdin, renders the UI tree, and emits events to
//! stdout. Submodules handle stdin I/O, window operations, widget
//! operations, and event emission.

mod app;
mod apply;
mod constants;
mod events;
mod run;
mod subscriptions;
mod update;
mod view;
mod window_map;

pub(crate) mod emitters;
mod stdin;
mod widget_ops;
mod window_ops;

pub(crate) use emitters::emit_hello;
pub(crate) use run::run;

use app::App;
