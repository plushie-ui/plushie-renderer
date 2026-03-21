//! Web effect handler.
//!
//! Stubs all effects as unsupported. Web implementations for clipboard
//! (Clipboard API) and file access (File System Access API) can be
//! added in a future iteration.

use iced::Task;
use serde_json::Value;

use toddy_core::message::Message;
use toddy_core::protocol::EffectResponse;
use toddy_renderer::EffectHandler;

pub struct WebEffectHandler;

impl EffectHandler for WebEffectHandler {
    fn handle_sync(&self, id: &str, _kind: &str, _payload: &Value) -> Option<EffectResponse> {
        Some(EffectResponse::unsupported(id.to_string()))
    }

    fn spawn_async(
        &self,
        _id: String,
        _kind: String,
        _payload: Value,
    ) -> Task<Message> {
        Task::none()
    }

    fn is_async(&self, _kind: &str) -> bool {
        false
    }
}
