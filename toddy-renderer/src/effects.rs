//! Platform abstraction for side effects.
//!
//! The renderer needs to perform platform-specific operations (file
//! dialogs, clipboard, notifications) that differ between native and
//! WASM targets. The [`EffectHandler`] trait abstracts these so
//! toddy-renderer can compile to both targets.

use iced::Task;
use serde_json::Value;

use toddy_core::message::Message;
use toddy_core::protocol::EffectResponse;

/// Handler for platform-specific side effects.
///
/// Native implementations use rfd (file dialogs), arboard (clipboard),
/// and notify-rust (notifications). WASM implementations stub or use
/// web platform APIs.
///
/// The `Send + 'static` bound is required because iced's daemon holds
/// the App across async boundaries and may move it between executor
/// contexts on native (tokio). On wasm32, `Send` is trivially satisfied.
pub trait EffectHandler: Send + 'static {
    /// Handle a synchronous effect. Returns `Some(response)` for effects
    /// that complete immediately (clipboard, notifications).
    ///
    /// Returns `None` only if the effect kind is completely unrecognized.
    /// The caller (`apply.rs`) silently ignores `None` -- the host
    /// receives no response for unrecognized effects. Implementations
    /// should prefer returning `EffectResponse::unsupported(id)` over
    /// `None` to ensure the host always gets a response.
    fn handle_sync(&self, id: &str, kind: &str, payload: &Value) -> Option<EffectResponse>;

    /// Spawn an async effect as an iced Task. Used for operations that
    /// must not block the event loop (file dialogs on native).
    fn spawn_async(&self, id: String, kind: String, payload: Value) -> Task<Message>;

    /// Returns true if the given effect kind should be handled async.
    ///
    /// Must be consistent with `handle_sync`/`spawn_async` -- if this
    /// returns true for a kind, `spawn_async` is called; otherwise
    /// `handle_sync`. Implementations must handle all kinds they claim
    /// are async via `spawn_async`.
    fn is_async(&self, kind: &str) -> bool;
}
