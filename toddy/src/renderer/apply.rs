//! Processes incoming protocol messages (snapshots, patches, settings,
//! extension commands) by delegating to Core and handling resulting effects.

use std::io;

use iced::Task;

use toddy_core::engine::CoreEffect;
use toddy_core::message::Message;
use toddy_core::protocol::IncomingMessage;

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
            IncomingMessage::ExtensionCommands { commands } => {
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
        let is_subscribe = matches!(message, IncomingMessage::Subscribe { .. });
        let is_unsubscribe = matches!(message, IncomingMessage::Unsubscribe { .. });
        let is_settings = matches!(message, IncomingMessage::Settings { .. });

        // Flush pending widget events before a snapshot replaces the
        // tree (widget IDs may change). Clear cached widget rates
        // since the tree is being replaced.
        if is_snapshot {
            let _ = self.emitter.flush();
            self.emitter.clear_widget_rates();
        }

        let effects = self.core.apply(message);

        // Sync rate configuration from Core to the EventEmitter after
        // Subscribe, Unsubscribe, or Settings messages.
        if is_subscribe || is_settings {
            self.emitter.set_default_rate(self.core.default_event_rate);
            for (kind, rate_opt) in &self.core.subscription_rates {
                if let Some(rate) = rate_opt {
                    self.emitter.set_subscription_rate(kind, *rate);
                }
            }
        }
        if is_unsubscribe {
            // Clean up emitter rates for removed subscriptions by
            // diffing against Core's subscription_rates.
            let emitter_keys: Vec<String> = self
                .emitter
                .subscription_rate_keys()
                .map(|s| s.to_string())
                .collect();
            for key in emitter_keys {
                if !self.core.subscription_rates.contains_key(&key) {
                    self.emitter.remove_subscription_rate(&key);
                    // Flush pending events for this subscription.
                    self.emitter
                        .flush_key(&super::emitter::CoalesceKey::Subscription(key));
                }
            }
        }
        for effect in effects {
            match effect {
                CoreEffect::SyncWindows => {
                    let task = self.sync_windows();
                    self.pending_tasks.push(task);
                }
                CoreEffect::EmitEvent(event) => emit_event(event)?,
                CoreEffect::HandleEffect {
                    request_id,
                    kind,
                    payload,
                } => {
                    if crate::effects::is_async_effect(&kind) {
                        let task = Task::perform(
                            async move {
                                crate::effects::handle_async_effect(request_id, &kind, &payload)
                                    .await
                            },
                            |response| {
                                if let Err(e) = emit_effect_response(response) {
                                    log::error!("write error in async effect: {e}");
                                }
                                Message::NoOp
                            },
                        );
                        self.pending_tasks.push(task);
                    } else {
                        let response = crate::effects::handle_effect(request_id, &kind, &payload);
                        emit_effect_response(response)?;
                    }
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
                    self.dispatcher.init_all(
                        &config,
                        &self.theme,
                        self.core.default_text_size,
                        self.core.default_font,
                    );
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
                    && let Some(theme) = toddy_core::theming::resolve_theme_only(theme_val)
                {
                    self.windows.set_theme(&win_id, Some(theme));
                }
            }

            if is_snapshot {
                self.dispatcher.clear_poisoned();
                // Clear stale slider tracking -- the entire tree was replaced,
                // so old node IDs are no longer valid.
                self.last_slide_values.clear();
            }
            if let Some(root) = self.core.tree.root() {
                self.dispatcher
                    .prepare_all(root, &mut self.core.caches.extension, &self.theme);
            }
        }

        Ok(())
    }
}
