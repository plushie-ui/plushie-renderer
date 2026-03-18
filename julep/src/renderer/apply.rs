//! Processes incoming protocol messages (snapshots, patches, settings,
//! extension commands) by delegating to Core and handling resulting effects.

use std::io;

use iced::Task;

use julep_core::engine::CoreEffect;
use julep_core::message::Message;
use julep_core::protocol::IncomingMessage;

use super::App;
use super::emitters::{emit_effect_response, emit_event};

impl App {
    pub(super) fn apply(&mut self, message: IncomingMessage) -> io::Result<()> {
        // Extension commands bypass the normal tree update / diff / patch cycle.
        match &message {
            IncomingMessage::ExtensionCommand {
                node_id,
                op,
                payload,
            } => {
                let events = self.dispatcher.handle_command(
                    node_id,
                    op,
                    payload,
                    &mut self.core.caches.extension,
                );
                for ev in events {
                    emit_event(ev)?;
                }
                return Ok(());
            }
            IncomingMessage::ExtensionCommandBatch { commands } => {
                for cmd in commands {
                    let events = self.dispatcher.handle_command(
                        &cmd.node_id,
                        &cmd.op,
                        &cmd.payload,
                        &mut self.core.caches.extension,
                    );
                    for ev in events {
                        emit_event(ev)?;
                    }
                }
                return Ok(());
            }
            _ => {}
        }

        let is_snapshot = matches!(message, IncomingMessage::Snapshot { .. });
        let is_tree_change = matches!(
            message,
            IncomingMessage::Snapshot { .. } | IncomingMessage::Patch { .. }
        );

        let effects = self.core.apply(message);
        for effect in effects {
            match effect {
                CoreEffect::SyncWindows => {
                    let task = self.sync_windows();
                    self.pending_tasks.push(task);
                }
                CoreEffect::EmitEvent(event) => emit_event(event)?,
                CoreEffect::EmitEffectResponse(response) => {
                    emit_effect_response(response)?;
                }
                CoreEffect::WidgetOp { op, payload } => {
                    let task = self.handle_widget_op(&op, &payload);
                    self.pending_tasks.push(task);
                }
                CoreEffect::WindowOp {
                    op,
                    window_id,
                    settings,
                } => {
                    let task = self.handle_window_op(&op, &window_id, &settings);
                    self.pending_tasks.push(task);
                }
                CoreEffect::ThemeChanged(theme) => {
                    self.theme = theme;
                    self.theme_follows_system = false;
                }
                CoreEffect::ThemeFollowsSystem => {
                    self.theme_follows_system = true;
                }
                CoreEffect::ImageOp {
                    op,
                    handle,
                    data,
                    pixels,
                    width,
                    height,
                } => {
                    self.handle_image_op(&op, &handle, data, pixels, width, height);
                }
                CoreEffect::ExtensionConfig(config) => {
                    self.dispatcher.init_all(&config);
                }
                CoreEffect::SpawnAsyncEffect {
                    request_id,
                    effect_type,
                    params,
                } => {
                    let task = Task::perform(
                        async move {
                            julep_core::effects::handle_async_effect(
                                request_id,
                                &effect_type,
                                &params,
                            )
                            .await
                        },
                        |response| {
                            // Inside an async Task callback -- log and
                            // continue; the next synchronous write will
                            // detect the broken pipe and exit cleanly.
                            if let Err(e) = emit_effect_response(response) {
                                log::error!("write error in async effect: {e}");
                            }
                            Message::NoOp
                        },
                    );
                    self.pending_tasks.push(task);
                }
            }
        }

        // After tree changes, update per-window theme cache and notify extensions.
        if is_tree_change {
            // Rebuild per-window theme cache from current tree.
            self.windows.clear_theme_cache();
            for win_id in self.core.tree.window_ids() {
                // When resolve_theme_only returns None it means "system" --
                // no cache entry, falls through to the system_theme path
                // in theme_for_window().
                if let Some(node) = self.core.tree.find_window(&win_id)
                    && let Some(theme_val) = node.props.get("theme")
                    && let Some(theme) = julep_core::theming::resolve_theme_only(theme_val)
                {
                    self.windows.set_theme(&win_id, Some(theme));
                }
            }

            if is_snapshot {
                self.dispatcher.clear_poisoned();
            }
            if let Some(root) = self.core.tree.root() {
                self.dispatcher
                    .prepare_all(root, &mut self.core.caches.extension, &self.theme);
            }
        }

        Ok(())
    }
}
