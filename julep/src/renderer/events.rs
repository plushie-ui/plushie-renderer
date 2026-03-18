use std::io;

use iced::widget::pane_grid;
use iced::{Point, Task, window};

use julep_core::message::{
    KeyEventData, Message, serialize_modifiers, serialize_mouse_button, serialize_scroll_delta,
};
use julep_core::protocol::OutgoingEvent;

use super::App;
use super::constants::*;
use super::emitters::{self, emit_event};

fn path_to_string(path: std::path::PathBuf) -> String {
    match path.to_str() {
        Some(s) => s.to_string(),
        None => {
            log::warn!(
                "file path contains non-UTF-8 bytes, using lossy conversion: {}",
                path.display()
            );
            path.to_string_lossy().into_owned()
        }
    }
}

impl App {
    pub(super) fn handle_key_pressed(&self, data: KeyEventData) -> Task<Message> {
        self.emit_subscription(SUB_KEY_PRESS, data.captured, |tag| {
            OutgoingEvent::key_press(tag, &data)
        })
    }

    pub(super) fn handle_key_released(&self, data: KeyEventData) -> Task<Message> {
        self.emit_subscription(SUB_KEY_RELEASE, data.captured, |tag| {
            OutgoingEvent::key_release(tag, &data)
        })
    }

    pub(super) fn handle_modifiers_changed(
        &self,
        mods: iced::keyboard::Modifiers,
        captured: bool,
    ) -> Task<Message> {
        self.emit_subscription(SUB_MODIFIERS_CHANGED, captured, |tag| {
            OutgoingEvent::modifiers_changed(tag, serialize_modifiers(mods))
        })
    }

    pub(super) fn handle_cursor_moved(&self, pos: Point, captured: bool) -> Task<Message> {
        self.emit_subscription(SUB_MOUSE_MOVE, captured, |tag| {
            OutgoingEvent::cursor_moved(tag, pos.x, pos.y)
        })
    }

    pub(super) fn handle_cursor_entered(&self, captured: bool) -> Task<Message> {
        self.emit_subscription(SUB_MOUSE_MOVE, captured, |tag| {
            OutgoingEvent::cursor_entered(tag)
        })
    }

    pub(super) fn handle_cursor_left(&self, captured: bool) -> Task<Message> {
        self.emit_subscription(SUB_MOUSE_MOVE, captured, |tag| {
            OutgoingEvent::cursor_left(tag)
        })
    }

    pub(super) fn handle_mouse_button_pressed(
        &self,
        button: iced::mouse::Button,
        captured: bool,
    ) -> Task<Message> {
        self.emit_subscription(SUB_MOUSE_BUTTON, captured, |tag| {
            OutgoingEvent::button_pressed(tag, serialize_mouse_button(&button))
        })
    }

    pub(super) fn handle_mouse_button_released(
        &self,
        button: iced::mouse::Button,
        captured: bool,
    ) -> Task<Message> {
        self.emit_subscription(SUB_MOUSE_BUTTON, captured, |tag| {
            OutgoingEvent::button_released(tag, serialize_mouse_button(&button))
        })
    }

    pub(super) fn handle_wheel_scrolled(
        &self,
        delta: iced::mouse::ScrollDelta,
        captured: bool,
    ) -> Task<Message> {
        self.emit_subscription(SUB_MOUSE_SCROLL, captured, |tag| {
            let (dx, dy, unit) = serialize_scroll_delta(&delta);
            OutgoingEvent::wheel_scrolled(tag, dx, dy, unit)
        })
    }

    pub(super) fn handle_finger_pressed(
        &self,
        finger: iced::touch::Finger,
        pos: Point,
        captured: bool,
    ) -> Task<Message> {
        self.emit_subscription(SUB_TOUCH, captured, |tag| {
            OutgoingEvent::finger_pressed(tag, finger.0, pos.x, pos.y)
        })
    }

    pub(super) fn handle_finger_moved(
        &self,
        finger: iced::touch::Finger,
        pos: Point,
        captured: bool,
    ) -> Task<Message> {
        self.emit_subscription(SUB_TOUCH, captured, |tag| {
            OutgoingEvent::finger_moved(tag, finger.0, pos.x, pos.y)
        })
    }

    pub(super) fn handle_finger_lifted(
        &self,
        finger: iced::touch::Finger,
        pos: Point,
        captured: bool,
    ) -> Task<Message> {
        self.emit_subscription(SUB_TOUCH, captured, |tag| {
            OutgoingEvent::finger_lifted(tag, finger.0, pos.x, pos.y)
        })
    }

    pub(super) fn handle_finger_lost(
        &self,
        finger: iced::touch::Finger,
        pos: Point,
        captured: bool,
    ) -> Task<Message> {
        self.emit_subscription(SUB_TOUCH, captured, |tag| {
            OutgoingEvent::finger_lost(tag, finger.0, pos.x, pos.y)
        })
    }

    pub(super) fn handle_ime_opened(&self, captured: bool) -> Task<Message> {
        self.emit_subscription(SUB_IME, captured, OutgoingEvent::ime_opened)
    }

    pub(super) fn handle_ime_preedit(
        &self,
        text: String,
        cursor: Option<std::ops::Range<usize>>,
        captured: bool,
    ) -> Task<Message> {
        self.emit_subscription(SUB_IME, captured, |tag| {
            OutgoingEvent::ime_preedit(tag, text, cursor)
        })
    }

    pub(super) fn handle_ime_commit(&self, text: String, captured: bool) -> Task<Message> {
        self.emit_subscription(SUB_IME, captured, |tag| {
            OutgoingEvent::ime_commit(tag, text)
        })
    }

    pub(super) fn handle_ime_closed(&self, captured: bool) -> Task<Message> {
        self.emit_subscription(SUB_IME, captured, OutgoingEvent::ime_closed)
    }

    /// Emit a window event to both the catch-all window subscription and
    /// the event-specific subscription (if registered).
    fn emit_window_event(
        &self,
        specific_key: Option<&str>,
        event_fn: impl Fn(String, String) -> OutgoingEvent,
        julep_id: String,
    ) -> io::Result<()> {
        if let Some(tag) = self.core.active_subscriptions.get(SUB_WINDOW_EVENT) {
            emit_event(event_fn(tag.clone(), julep_id.clone()))?;
        }
        if let Some(key) = specific_key
            && let Some(tag) = self.core.active_subscriptions.get(key)
        {
            emit_event(event_fn(tag.clone(), julep_id))?;
        }
        Ok(())
    }

    pub(super) fn handle_window_event(
        &self,
        iced_id: window::Id,
        evt: window::Event,
    ) -> Task<Message> {
        let julep_id = self.windows.julep_id_for(&iced_id);
        if julep_id.is_empty() {
            log::warn!(
                "received window event for unknown iced window {:?}, skipping emission",
                iced_id
            );
            return Task::none();
        }
        // Helper closure: emit and propagate errors uniformly.
        let result: io::Result<()> = (|| {
            match evt {
                window::Event::Opened {
                    position,
                    size,
                    scale_factor,
                } => {
                    if let Some(tag) = self.core.active_subscriptions.get(SUB_WINDOW_EVENT) {
                        let pos = position.map(|p| (p.x, p.y));
                        emit_event(OutgoingEvent::window_opened(
                            tag.clone(),
                            julep_id.clone(),
                            pos,
                            size.width,
                            size.height,
                            scale_factor,
                        ))?;
                    }
                    if let Some(tag) = self.core.active_subscriptions.get(SUB_WINDOW_OPEN) {
                        let pos = position.map(|p| (p.x, p.y));
                        emit_event(OutgoingEvent::window_opened(
                            tag.clone(),
                            julep_id,
                            pos,
                            size.width,
                            size.height,
                            scale_factor,
                        ))?;
                    }
                }
                window::Event::Closed => {
                    if let Some(tag) = self.core.active_subscriptions.get(SUB_WINDOW_EVENT) {
                        emit_event(OutgoingEvent::window_closed(tag.clone(), julep_id))?;
                    }
                }
                window::Event::Moved(point) => {
                    self.emit_window_event(
                        Some(SUB_WINDOW_MOVE),
                        |tag, jid| OutgoingEvent::window_moved(tag, jid, point.x, point.y),
                        julep_id,
                    )?;
                }
                window::Event::Resized(size) => {
                    self.emit_window_event(
                        Some(SUB_WINDOW_RESIZE),
                        |tag, jid| OutgoingEvent::window_resized(tag, jid, size.width, size.height),
                        julep_id,
                    )?;
                }
                window::Event::Rescaled(factor) => {
                    if let Some(tag) = self.core.active_subscriptions.get(SUB_WINDOW_EVENT) {
                        emit_event(OutgoingEvent::window_rescaled(
                            tag.clone(),
                            julep_id,
                            factor,
                        ))?;
                    }
                }
                window::Event::Focused => {
                    self.emit_window_event(
                        Some(SUB_WINDOW_FOCUS),
                        OutgoingEvent::window_focused,
                        julep_id,
                    )?;
                }
                window::Event::Unfocused => {
                    self.emit_window_event(
                        Some(SUB_WINDOW_UNFOCUS),
                        OutgoingEvent::window_unfocused,
                        julep_id,
                    )?;
                }
                window::Event::FileHovered(path) => {
                    if let Some(tag) = self.core.active_subscriptions.get(SUB_FILE_DROP) {
                        let path_str = path_to_string(path);
                        emit_event(OutgoingEvent::file_hovered(tag.clone(), julep_id, path_str))?;
                    }
                }
                window::Event::FileDropped(path) => {
                    if let Some(tag) = self.core.active_subscriptions.get(SUB_FILE_DROP) {
                        let path_str = path_to_string(path);
                        emit_event(OutgoingEvent::file_dropped(tag.clone(), julep_id, path_str))?;
                    }
                }
                window::Event::FilesHoveredLeft => {
                    if let Some(tag) = self.core.active_subscriptions.get(SUB_FILE_DROP) {
                        emit_event(OutgoingEvent::files_hovered_left(tag.clone(), julep_id))?;
                    }
                }
                window::Event::CloseRequested => {
                    // Handled via close_requests() subscription separately.
                }
                window::Event::RedrawRequested(_) => {
                    // Handled via animation_frame subscription separately.
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            log::error!("write error: {e}");
            return iced::exit();
        }
        Task::none()
    }

    pub(super) fn handle_pane_resized(
        &mut self,
        grid_id: String,
        evt: pane_grid::ResizeEvent,
    ) -> Task<Message> {
        if let Some(state) = self.core.caches.pane_grid_state_mut(&grid_id) {
            state.resize(evt.split, evt.ratio);
        }
        emitters::emit_or_exit(OutgoingEvent::pane_resized(
            grid_id,
            format!("{:?}", evt.split),
            evt.ratio,
        ))
    }

    pub(super) fn handle_pane_dragged(
        &mut self,
        grid_id: String,
        evt: pane_grid::DragEvent,
    ) -> Task<Message> {
        let result: io::Result<()> = (|| {
            match evt {
                pane_grid::DragEvent::Picked { pane } => {
                    if let Some(state) = self.core.caches.pane_grid_state(&grid_id) {
                        let pane_id = state.get(pane).cloned().unwrap_or_default();
                        emit_event(OutgoingEvent::pane_dragged(
                            grid_id, "picked", pane_id, None, None, None,
                        ))?;
                    }
                }
                pane_grid::DragEvent::Dropped { pane, target } => {
                    if let Some(state) = self.core.caches.pane_grid_state_mut(&grid_id) {
                        let pane_id = state.get(pane).cloned().unwrap_or_default();
                        let (target_pane, region, edge) = match target {
                            pane_grid::Target::Edge(e) => {
                                let edge_str = match e {
                                    pane_grid::Edge::Top => "top",
                                    pane_grid::Edge::Bottom => "bottom",
                                    pane_grid::Edge::Left => "left",
                                    pane_grid::Edge::Right => "right",
                                };
                                (None, None, Some(edge_str))
                            }
                            pane_grid::Target::Pane(p, region) => {
                                let target_id = state.get(p).cloned().unwrap_or_default();
                                let region_str = match region {
                                    pane_grid::Region::Center => "center",
                                    pane_grid::Region::Edge(pane_grid::Edge::Top) => "top",
                                    pane_grid::Region::Edge(pane_grid::Edge::Bottom) => "bottom",
                                    pane_grid::Region::Edge(pane_grid::Edge::Left) => "left",
                                    pane_grid::Region::Edge(pane_grid::Edge::Right) => "right",
                                };
                                (Some(target_id), Some(region_str), None)
                            }
                        };
                        state.drop(pane, target);
                        emit_event(OutgoingEvent::pane_dragged(
                            grid_id,
                            "dropped",
                            pane_id,
                            target_pane,
                            region,
                            edge,
                        ))?;
                    }
                }
                pane_grid::DragEvent::Canceled { pane } => {
                    if let Some(state) = self.core.caches.pane_grid_state(&grid_id) {
                        let pane_id = state.get(pane).cloned().unwrap_or_default();
                        emit_event(OutgoingEvent::pane_dragged(
                            grid_id, "canceled", pane_id, None, None, None,
                        ))?;
                    }
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            log::error!("write error: {e}");
            return iced::exit();
        }
        Task::none()
    }

    pub(super) fn handle_pane_clicked(
        &self,
        grid_id: String,
        pane: pane_grid::Pane,
    ) -> Task<Message> {
        if let Some(state) = self.core.caches.pane_grid_state(&grid_id) {
            let pane_id = state.get(pane).cloned().unwrap_or_default();
            return emitters::emit_or_exit(OutgoingEvent::pane_clicked(grid_id, pane_id));
        }
        Task::none()
    }
}
