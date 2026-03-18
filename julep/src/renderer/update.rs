use iced::{Task, Theme, window};

use julep_core::extensions::EventResult;
use julep_core::message::{Message, StdinEvent};
use julep_core::protocol::{IncomingMessage, OutgoingEvent};

use super::App;
use super::constants::*;
use super::emitters::{self, emit_event, emit_screenshot_response, message_to_event};

impl App {
    pub(super) fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Stdin(event) => self.handle_stdin(event),
            Message::NoOp => Task::none(),
            // Simple widget events: map through message_to_event and emit.
            ref msg @ (Message::Click(_)
            | Message::Input(..)
            | Message::Submit(..)
            | Message::Toggle(..)
            | Message::Select(..)) => match message_to_event(msg) {
                Some(event) => emitters::emit_or_exit(event),
                None => Task::none(),
            },
            Message::Slide(ref id, value) => {
                self.last_slide_values.insert(id.clone(), value);
                emitters::emit_or_exit(OutgoingEvent::slide(id.clone(), value))
            }
            Message::SlideRelease(ref id) => {
                let value = self.last_slide_values.remove(id).unwrap_or(0.0);
                emitters::emit_or_exit(OutgoingEvent::slide_release(id.clone(), value))
            }
            // Generic extension-aware events: route through dispatcher first.
            //
            // Redraw contract: iced::daemon rebuilds user interfaces (calls
            // view_window for every open window) and requests a
            // window-level redraw after every update() call, regardless of
            // the Task returned here. Returning Task::none() is therefore
            // correct for all three EventResult variants -- the view IS
            // rebuilt, and any state the extension mutated in
            // ExtensionCaches will be visible to the next render() call.
            //
            // Canvas caching note: if an extension renders via
            // canvas::Cache, the cached geometry is NOT automatically
            // invalidated by a view rebuild. The extension must call
            // cache.clear() itself when its state changes -- typically by
            // using a GenerationCounter (see extensions.rs) bumped in
            // handle_event and checked in Program::draw(). There is no
            // need to emit dummy events or return special Tasks to force a
            // redraw; iced already takes care of the update-view-render
            // cycle.
            Message::Event {
                ref id,
                ref data,
                ref family,
            } => {
                let result =
                    self.dispatcher
                        .handle_event(id, family, data, &mut self.core.caches.extension);
                let emit_result = match result {
                    EventResult::PassThrough => {
                        let data_opt = if data.is_null() {
                            None
                        } else {
                            Some(data.clone())
                        };
                        emit_event(OutgoingEvent::generic(family.clone(), id.clone(), data_opt))
                    }
                    EventResult::Consumed(events) => {
                        let mut r = Ok(());
                        for ev in events {
                            r = emit_event(ev);
                            if r.is_err() {
                                break;
                            }
                        }
                        r
                    }
                    EventResult::Observed(events) => {
                        let data_opt = if data.is_null() {
                            None
                        } else {
                            Some(data.clone())
                        };
                        let mut r = emit_event(OutgoingEvent::generic(
                            family.clone(),
                            id.clone(),
                            data_opt,
                        ));
                        if r.is_ok() {
                            for ev in events {
                                r = emit_event(ev);
                                if r.is_err() {
                                    break;
                                }
                            }
                        }
                        r
                    }
                };
                if let Err(e) = emit_result {
                    log::error!("write error: {e}");
                    return iced::exit();
                }
                Task::none()
            }
            Message::TextEditorAction(id, action) => {
                let is_edit = action.is_edit();
                if let Some(content) = self.core.caches.editor_content_mut(&id) {
                    content.perform(action);
                    if is_edit {
                        let new_text = content.text();
                        return emitters::emit_or_exit(OutgoingEvent::input(id, new_text));
                    }
                }
                Task::none()
            }
            Message::MarkdownUrl(url) => {
                log::debug!("markdown link clicked: {url}");
                Task::none()
            }

            // -- Keyboard events --
            Message::KeyPressed(data) => self.handle_key_pressed(data),
            Message::KeyReleased(data) => self.handle_key_released(data),
            Message::ModifiersChanged(mods, captured) => {
                self.handle_modifiers_changed(mods, captured)
            }

            // -- Mouse events --
            Message::CursorMoved(pos, _win, captured) => self.handle_cursor_moved(pos, captured),
            Message::CursorEntered(_win, captured) => self.handle_cursor_entered(captured),
            Message::CursorLeft(_win, captured) => self.handle_cursor_left(captured),
            Message::MouseButtonPressed(button, _win, captured) => {
                self.handle_mouse_button_pressed(button, captured)
            }
            Message::MouseButtonReleased(button, _win, captured) => {
                self.handle_mouse_button_released(button, captured)
            }
            Message::WheelScrolled(delta, _win, captured) => {
                self.handle_wheel_scrolled(delta, captured)
            }

            // -- Touch events --
            Message::FingerPressed(finger, pos, _win, captured) => {
                self.handle_finger_pressed(finger, pos, captured)
            }
            Message::FingerMoved(finger, pos, _win, captured) => {
                self.handle_finger_moved(finger, pos, captured)
            }
            Message::FingerLifted(finger, pos, _win, captured) => {
                self.handle_finger_lifted(finger, pos, captured)
            }
            Message::FingerLost(finger, pos, _win, captured) => {
                self.handle_finger_lost(finger, pos, captured)
            }

            // -- IME events --
            Message::ImeOpened(captured) => self.handle_ime_opened(captured),
            Message::ImePreedit(text, cursor, captured) => {
                self.handle_ime_preedit(text, cursor, captured)
            }
            Message::ImeCommit(text, captured) => self.handle_ime_commit(text, captured),
            Message::ImeClosed(captured) => self.handle_ime_closed(captured),

            // -- Window lifecycle events --
            Message::WindowCloseRequested(window_id) => {
                // Do NOT close the window or remove from maps here. The host
                // decides whether to close by sending a close_window command
                // or removing the window from the tree. Closing immediately
                // would bypass app-level confirmation dialogs.
                if let Some(tag) = self.core.active_subscriptions.get(SUB_WINDOW_CLOSE) {
                    let julep_id = self.windows.julep_id_for(&window_id);
                    emitters::emit_or_exit(OutgoingEvent::window_close_requested(
                        tag.clone(),
                        julep_id,
                    ))
                } else {
                    Task::none()
                }
            }
            Message::WindowClosed(window_id) => {
                if let Some(julep_id) = self.windows.remove_by_iced(&window_id) {
                    if let Some(tag) = self.core.active_subscriptions.get(SUB_WINDOW_EVENT)
                        && let Err(e) =
                            emit_event(OutgoingEvent::window_closed(tag.clone(), julep_id.clone()))
                    {
                        log::error!("write error: {e}");
                        return iced::exit();
                    }
                    log::info!("window closed: {julep_id}");
                }
                // All managed windows gone -- notify the host.
                // The host can choose to exit, send a new Snapshot, or take other action.
                // We do NOT call iced::exit() here because the daemon should stay alive
                // to receive new tree snapshots (e.g. after a Reset or window re-creation).
                if self.windows.is_empty() && self.core.tree.root().is_some() {
                    log::info!("all windows closed -- notifying host");
                    return emitters::emit_or_exit(OutgoingEvent::generic(
                        "all_windows_closed".to_string(),
                        String::new(),
                        None,
                    ));
                }
                Task::none()
            }
            Message::WindowOpened(iced_id, julep_id) => {
                log::info!("window opened: {julep_id} -> {iced_id:?}");
                self.windows.insert(julep_id, iced_id);
                Task::none()
            }
            Message::WindowEvent(iced_id, evt) => self.handle_window_event(iced_id, evt),

            // -- System / animation --
            Message::AnimationFrame(instant) => {
                if let Some(tag) = self.core.active_subscriptions.get(SUB_ANIMATION_FRAME) {
                    use std::sync::OnceLock;
                    static EPOCH: OnceLock<iced::time::Instant> = OnceLock::new();
                    let epoch = *EPOCH.get_or_init(|| instant);
                    let millis = instant.duration_since(epoch).as_millis();
                    emitters::emit_or_exit(OutgoingEvent::animation_frame(tag.clone(), millis))
                } else {
                    Task::none()
                }
            }
            Message::ThemeChanged(mode) => {
                // Track system theme so "system" theme value follows OS preference
                self.system_theme = match mode {
                    iced::theme::Mode::Light => Theme::Light,
                    iced::theme::Mode::Dark => Theme::Dark,
                    _ => Theme::Dark,
                };
                if let Some(tag) = self.core.active_subscriptions.get(SUB_THEME_CHANGE) {
                    let mode_str = match mode {
                        iced::theme::Mode::Light => "light",
                        iced::theme::Mode::Dark => "dark",
                        _ => "system",
                    };
                    emitters::emit_or_exit(OutgoingEvent::theme_changed(
                        tag.clone(),
                        mode_str.to_string(),
                    ))
                } else {
                    Task::none()
                }
            }
            Message::SensorResize(id, width, height) => {
                emitters::emit_or_exit(OutgoingEvent::sensor_resize(id, width, height))
            }
            Message::CanvasEvent {
                id,
                kind,
                x,
                y,
                extra,
            } => match kind.as_str() {
                "press" => emitters::emit_or_exit(OutgoingEvent::canvas_press(id, x, y, extra)),
                "release" => emitters::emit_or_exit(OutgoingEvent::canvas_release(id, x, y, extra)),
                "move" => emitters::emit_or_exit(OutgoingEvent::canvas_move(id, x, y)),
                _ => Task::none(),
            },
            Message::CanvasScroll {
                id,
                cursor_x,
                cursor_y,
                delta_x,
                delta_y,
            } => emitters::emit_or_exit(OutgoingEvent::canvas_scroll(
                id, cursor_x, cursor_y, delta_x, delta_y,
            )),
            Message::PaneResized(grid_id, evt) => self.handle_pane_resized(grid_id, evt),
            Message::PaneDragged(grid_id, evt) => self.handle_pane_dragged(grid_id, evt),
            Message::PaneClicked(grid_id, pane) => self.handle_pane_clicked(grid_id, pane),
            Message::PaneFocusCycle(grid_id, pane) => {
                if let Some(state) = self.core.caches.pane_grid_state(&grid_id) {
                    let pane_id = state.get(pane).cloned().unwrap_or_default();
                    return emitters::emit_or_exit(OutgoingEvent::pane_focus_cycle(
                        grid_id, pane_id,
                    ));
                }
                Task::none()
            }
            Message::ScrollEvent(id, viewport) => emitters::emit_or_exit(OutgoingEvent::scroll(
                id,
                viewport.absolute_x,
                viewport.absolute_y,
                viewport.relative_x,
                viewport.relative_y,
                viewport.viewport_width,
                viewport.viewport_height,
                viewport.content_width,
                viewport.content_height,
            )),
            Message::Paste(id, text) => emitters::emit_or_exit(OutgoingEvent::paste(id, text)),
            Message::OptionHovered(id, value) => {
                emitters::emit_or_exit(OutgoingEvent::option_hovered(id, value))
            }
            Message::MouseAreaEvent(id, kind) => {
                let event = match kind.as_str() {
                    "right_press" => OutgoingEvent::mouse_right_press(id),
                    "right_release" => OutgoingEvent::mouse_right_release(id),
                    "middle_press" => OutgoingEvent::mouse_middle_press(id),
                    "middle_release" => OutgoingEvent::mouse_middle_release(id),
                    "double_click" => OutgoingEvent::mouse_double_click(id),
                    "enter" => OutgoingEvent::mouse_enter(id),
                    "exit" => OutgoingEvent::mouse_exit(id),
                    _ => return Task::none(),
                };
                emitters::emit_or_exit(event)
            }
            Message::MouseAreaMove(id, x, y) => {
                emitters::emit_or_exit(OutgoingEvent::mouse_area_move(id, x, y))
            }
            Message::MouseAreaScroll(id, dx, dy) => {
                emitters::emit_or_exit(OutgoingEvent::mouse_area_scroll(id, dx, dy))
            }
        }
    }

    pub(super) fn handle_stdin(&mut self, event: StdinEvent) -> Task<Message> {
        match event {
            StdinEvent::Message(incoming) => {
                // Handle scripting messages directly instead of passing
                // them to Core::apply. All other messages fall through.
                match incoming {
                    IncomingMessage::Query {
                        id,
                        target,
                        selector,
                    } => {
                        if let Err(e) =
                            crate::scripting::handle_query(&self.core, id, target, selector)
                        {
                            log::error!("write error: {e}");
                            return iced::exit();
                        }
                        Task::none()
                    }
                    IncomingMessage::Interact {
                        id,
                        action,
                        selector,
                        payload,
                    } => {
                        if let Err(e) = crate::scripting::handle_interact(
                            &self.core, id, action, selector, payload,
                        ) {
                            log::error!("write error: {e}");
                            return iced::exit();
                        }
                        Task::none()
                    }
                    IncomingMessage::Reset { id } => {
                        // Clean up extension state before wiping core.
                        self.dispatcher.reset(&mut self.core.caches.extension);

                        // Reset core and emit the response.
                        if let Err(e) = crate::scripting::handle_reset(&mut self.core, id) {
                            log::error!("write error: {e}");
                            return iced::exit();
                        }

                        // Close all open windows and clear maps.
                        let close_tasks: Vec<Task<Message>> = self
                            .windows
                            .iced_ids()
                            .map(|&iced_id| window::close(iced_id))
                            .collect();
                        self.windows.clear();

                        // Reset remaining App-level state.
                        self.image_registry = julep_core::image_registry::ImageRegistry::new();
                        self.theme = Theme::Dark;
                        self.theme_follows_system = false;
                        self.scale_factor = 1.0;
                        self.last_slide_values.clear();
                        self.pending_tasks.clear();

                        Task::batch(close_tasks)
                    }
                    IncomingMessage::SnapshotCapture { id, name, .. } => {
                        if let Err(e) =
                            crate::scripting::handle_snapshot_capture(&self.core, id, name)
                        {
                            log::error!("write error: {e}");
                            return iced::exit();
                        }
                        Task::none()
                    }
                    IncomingMessage::ScreenshotCapture { id, name, .. } => {
                        // Capture real GPU-rendered pixels via iced
                        if let Some((_, &iced_id)) = self.windows.iter().next() {
                            window::screenshot(iced_id).map(move |shot| {
                                use sha2::{Digest, Sha256};
                                let rgba: &[u8] = &shot.rgba;
                                let mut hasher = Sha256::new();
                                hasher.update(rgba);
                                let hash = format!("{:x}", hasher.finalize());
                                let w = shot.size.width;
                                let h = shot.size.height;
                                // Inside a Task callback -- log and continue;
                                // the next synchronous write will exit cleanly.
                                if let Err(e) =
                                    emit_screenshot_response(&id, &name, &hash, w, h, rgba)
                                {
                                    log::error!("write error in screenshot: {e}");
                                }
                                Message::NoOp
                            })
                        } else {
                            // No windows open -- return empty screenshot
                            if let Err(e) = emit_screenshot_response(&id, &name, "", 0, 0, &[]) {
                                log::error!("write error: {e}");
                                return iced::exit();
                            }
                            Task::none()
                        }
                    }
                    other => {
                        if let Err(e) = self.apply(other) {
                            log::error!("write error: {e}");
                            return iced::exit();
                        }
                        let tasks: Vec<Task<Message>> = self.pending_tasks.drain(..).collect();
                        Task::batch(tasks)
                    }
                }
            }
            StdinEvent::Warning(msg) => {
                log::warn!("stdin warning: {msg}");
                Task::none()
            }
            StdinEvent::Closed => {
                log::info!("stdin closed -- exiting");
                iced::exit()
            }
        }
    }
}
