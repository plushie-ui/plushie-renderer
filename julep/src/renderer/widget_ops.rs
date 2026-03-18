use iced::widget::pane_grid;
use iced::{Task, window};

use julep_core::message::Message;
use julep_core::protocol::OutgoingEvent;

use super::App;
use super::emitters::emit_event;

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
            "announce" => {
                let text = payload
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                iced::announce(text)
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

                if let Some(state) = self.core.caches.pane_grid_state_mut(&target)
                    && let Some(pane) = find_pane_by_julep_id(state, &pane_id)
                {
                    let _ = state.split(axis, pane, new_pane_id);
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

                if let Some(state) = self.core.caches.pane_grid_state_mut(&target)
                    && let Some(pane) = find_pane_by_julep_id(state, &pane_id)
                {
                    let _ = state.close(pane);
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

                if let Some(state) = self.core.caches.pane_grid_state_mut(&target)
                    && let (Some(a), Some(b)) = (
                        find_pane_by_julep_id(state, &a_id),
                        find_pane_by_julep_id(state, &b_id),
                    )
                {
                    state.swap(a, b);
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

                if let Some(state) = self.core.caches.pane_grid_state_mut(&target)
                    && let Some(pane) = find_pane_by_julep_id(state, &pane_id)
                {
                    state.maximize(pane);
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
            "find_focused" => {
                // find_focused is not available in this iced version.
                // Emit an empty response so the host doesn't hang.
                let tag = payload
                    .get("tag")
                    .and_then(|v| v.as_str())
                    .unwrap_or("find_focused")
                    .to_string();
                log::warn!("find_focused: not supported in this iced version");
                super::emitters::emit_query_response(
                    "find_focused",
                    &tag,
                    serde_json::json!({"focused": null, "error": "not supported"}),
                );
                Task::none()
            }
            "load_font" => {
                let data = payload
                    .get("data")
                    .and_then(|v| v.as_str())
                    .and_then(|s| {
                        use base64::Engine;
                        base64::engine::general_purpose::STANDARD.decode(s).ok()
                    })
                    .unwrap_or_default();
                if data.is_empty() {
                    log::warn!("load_font: no font data provided");
                    Task::none()
                } else {
                    iced::font::load(data).map(|result| {
                        match result {
                            Ok(()) => log::info!("font loaded successfully"),
                            Err(e) => log::error!("font load failed: {e:?}"),
                        }
                        Message::NoOp
                    })
                }
            }
            "tree_hash" => {
                let tag = payload
                    .get("tag")
                    .and_then(|v| v.as_str())
                    .unwrap_or("tree_hash")
                    .to_string();
                let hash = self.core.tree_hash();
                super::emitters::emit_query_response(
                    "tree_hash",
                    &tag,
                    serde_json::json!({"hash": hash}),
                );
                Task::none()
            }
            "list_images" => {
                let tag = payload
                    .get("tag")
                    .and_then(|v| v.as_str())
                    .unwrap_or("list_images")
                    .to_string();
                let handles: Vec<String> = self.image_registry.handle_names();
                super::emitters::emit_query_response(
                    "image_list",
                    &tag,
                    serde_json::json!({"handles": handles}),
                );
                Task::none()
            }
            "clear_images" => {
                self.image_registry.clear();
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
        data: Option<Vec<u8>>,
        pixels: Option<Vec<u8>>,
        width: Option<u32>,
        height: Option<u32>,
    ) {
        if let Err(error) = self
            .image_registry
            .apply_op(op, handle, data, pixels, width, height)
        {
            emit_event(OutgoingEvent::generic(
                "image_error".to_string(),
                handle.to_string(),
                Some(serde_json::json!({ "error": error })),
            ));
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
