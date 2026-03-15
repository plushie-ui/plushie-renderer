use iced::widget::pane_grid;
use iced::{window, Task};

use julep_core::message::Message;

use super::App;

// ---------------------------------------------------------------------------
// Widget operations (impl App)
// ---------------------------------------------------------------------------

impl App {
    pub(super) fn handle_widget_op(
        &mut self,
        op: &str,
        payload: &serde_json::Value,
    ) -> Task<Message> {
        let get_target = || {
            payload
                .get("target")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string()
        };

        match op {
            "focus" => {
                iced::widget::operation::focus::<Message>(iced::widget::Id::from(get_target()))
            }
            "focus_next" => iced::widget::operation::focus_next(),
            "focus_previous" => iced::widget::operation::focus_previous(),
            "scroll_to" => {
                let target = get_target();
                let offset_x = payload
                    .get("offset_x")
                    .and_then(|v| v.as_f64())
                    .map(|v| v as f32);
                let offset_y = payload
                    .get("offset")
                    .or_else(|| payload.get("offset_y"))
                    .and_then(|v| v.as_f64())
                    .map(|v| v as f32);
                iced::widget::operation::scroll_to(
                    iced::widget::Id::from(target),
                    iced::widget::operation::AbsoluteOffset {
                        x: offset_x.unwrap_or(0.0),
                        y: offset_y.unwrap_or(0.0),
                    },
                )
            }
            "scroll_by" => {
                let target = get_target();
                let offset_x = payload
                    .get("offset_x")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0) as f32;
                let offset_y = payload
                    .get("offset_y")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0) as f32;
                iced::widget::operation::scroll_by(
                    iced::widget::Id::from(target),
                    iced::widget::operation::AbsoluteOffset {
                        x: offset_x,
                        y: offset_y,
                    },
                )
            }
            "snap_to" => {
                let target = get_target();
                let x = payload.get("x").and_then(|v| v.as_f64()).map(|v| v as f32);
                let y = payload.get("y").and_then(|v| v.as_f64()).map(|v| v as f32);
                iced::widget::operation::snap_to(
                    iced::widget::Id::from(target),
                    iced::widget::operation::RelativeOffset { x, y },
                )
            }
            "snap_to_end" => {
                let target = get_target();
                iced::widget::operation::snap_to_end(iced::widget::Id::from(target))
            }
            "select_all" => {
                iced::widget::operation::select_all(iced::widget::Id::from(get_target()))
            }
            "select_range" => {
                let target = get_target();
                let start = payload.get("start").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let end = payload.get("end").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                iced::widget::operation::select_range(iced::widget::Id::from(target), start, end)
            }
            "move_cursor_to_front" => {
                iced::widget::operation::move_cursor_to_front(iced::widget::Id::from(get_target()))
            }
            "move_cursor_to_end" => {
                iced::widget::operation::move_cursor_to_end(iced::widget::Id::from(get_target()))
            }
            "move_cursor_to" => {
                let target = get_target();
                let position = payload
                    .get("position")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize;
                iced::widget::operation::move_cursor_to(iced::widget::Id::from(target), position)
            }
            "close_window" => {
                // Look up the julep window_id from the payload and close the
                // correct iced window. Falls back to oldest window only if no
                // window_id is provided (backwards compat).
                let win_id = payload
                    .get("window_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                if !win_id.is_empty() {
                    if let Some(iced_id) = self.window_map.remove(win_id) {
                        self.reverse_window_map.remove(&iced_id);
                        window::close(iced_id)
                    } else {
                        log::warn!("close_window: unknown window_id: {win_id}");
                        Task::none()
                    }
                } else {
                    window::oldest().and_then(window::close)
                }
            }
            "exit" => iced::exit(),
            // -- PaneGrid operations --
            // The host sends: target (grid id), pane, axis, new_pane_id, a, b
            "pane_split" => {
                let target = get_target();
                let pane_id = payload
                    .get("pane")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let new_pane_id = payload
                    .get("new_pane_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let axis = match payload
                    .get("axis")
                    .and_then(|v| v.as_str())
                    .unwrap_or("vertical")
                {
                    "horizontal" => pane_grid::Axis::Horizontal,
                    _ => pane_grid::Axis::Vertical,
                };

                if let Some(state) = self.core.caches.pane_grid_state_mut(&target) {
                    if let Some(pane) = find_pane_by_julep_id(state, &pane_id) {
                        let _ = state.split(axis, pane, new_pane_id);
                    }
                }
                Task::none()
            }
            "pane_close" => {
                let target = get_target();
                let pane_id = payload
                    .get("pane")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();

                if let Some(state) = self.core.caches.pane_grid_state_mut(&target) {
                    if let Some(pane) = find_pane_by_julep_id(state, &pane_id) {
                        let _ = state.close(pane);
                    }
                }
                Task::none()
            }
            "pane_swap" => {
                let target = get_target();
                let a_id = payload
                    .get("a")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let b_id = payload
                    .get("b")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();

                if let Some(state) = self.core.caches.pane_grid_state_mut(&target) {
                    if let (Some(a), Some(b)) = (
                        find_pane_by_julep_id(state, &a_id),
                        find_pane_by_julep_id(state, &b_id),
                    ) {
                        state.swap(a, b);
                    }
                }
                Task::none()
            }
            "pane_maximize" => {
                let target = get_target();
                let pane_id = payload
                    .get("pane")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();

                if let Some(state) = self.core.caches.pane_grid_state_mut(&target) {
                    if let Some(pane) = find_pane_by_julep_id(state, &pane_id) {
                        state.maximize(pane);
                    }
                }
                Task::none()
            }
            "pane_restore" => {
                let target = get_target();

                if let Some(state) = self.core.caches.pane_grid_state_mut(&target) {
                    state.restore();
                }
                Task::none()
            }
            other => {
                log::warn!("unknown widget_op: {other}");
                Task::none()
            }
        }
    }

    // -----------------------------------------------------------------------
    // Image operations
    // -----------------------------------------------------------------------

    pub(super) fn handle_image_op(
        &mut self,
        op: &str,
        handle: &str,
        data: Option<&[u8]>,
        pixels: Option<&[u8]>,
        width: Option<u32>,
        height: Option<u32>,
    ) {
        match op {
            "create_image" | "update_image" => {
                if let Some(pixel_bytes) = pixels {
                    // RGBA pixel data (raw bytes, no base64 decode needed)
                    let w = width.unwrap_or(0);
                    let h = height.unwrap_or(0);
                    self.image_registry.create_from_rgba(
                        handle.to_string(),
                        w,
                        h,
                        pixel_bytes.to_vec(),
                    );
                } else if let Some(image_bytes) = data {
                    // Encoded image bytes (PNG, JPEG, etc. -- raw bytes)
                    self.image_registry
                        .create_from_bytes(handle.to_string(), image_bytes.to_vec());
                } else {
                    log::warn!("image_op {op}: missing data or pixels field");
                }
            }
            "delete_image" => {
                self.image_registry.delete(handle);
            }
            other => {
                log::warn!("unknown image_op: {other}");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// PaneGrid helpers
// ---------------------------------------------------------------------------

/// Find a pane_grid::Pane by its julep ID string.
pub(super) fn find_pane_by_julep_id(
    state: &pane_grid::State<String>,
    julep_id: &str,
) -> Option<pane_grid::Pane> {
    state
        .panes
        .iter()
        .find(|(_, id)| id.as_str() == julep_id)
        .map(|(pane, _)| *pane)
}
