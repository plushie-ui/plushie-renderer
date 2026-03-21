//! Canvas widget -- 2D drawing surface with per-layer caching.
//!
//! Renders shapes from JSON prop data onto an iced canvas. Supports:
//!
//! - **Shapes**: rect, circle, line, arc, path (with SVG-like commands),
//!   text, image
//! - **Layers**: multiple named layers with independent content-hash
//!   invalidation for efficient re-tessellation
//! - **Fills**: solid colors, linear/radial gradients, fill rules
//! - **Strokes**: color, width, line cap/join, dash patterns
//! - **Clipping**: push_clip/pop_clip regions for masked rendering
//! - **Events**: optional press, release, move, scroll handlers with
//!   canvas-local coordinates

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::Hasher;

use iced::widget::canvas;
use iced::{
    Color, Element, Length, Pixels, Point, Radians, Rectangle, Size, Vector, alignment, keyboard,
    mouse,
};
use serde_json::Value;

use super::caches::{WidgetCaches, canvas_layer_map, hash_json_value};
use super::helpers::*;
use crate::extensions::RenderCtx;
use crate::message::Message;
use crate::protocol::TreeNode;

/// Maximum number of shapes per canvas layer. Layers exceeding this limit
/// are truncated with a warning to prevent excessive tessellation work from
/// a single oversized payload.
const MAX_SHAPES_PER_LAYER: usize = 10_000;

// ---------------------------------------------------------------------------
// Interactive shapes -- hit testing and interaction state
// ---------------------------------------------------------------------------

/// Geometric region for hit testing an interactive shape.
#[derive(Debug, Clone)]
pub(crate) enum HitRegion {
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    },
    Circle {
        cx: f32,
        cy: f32,
        r: f32,
    },
    Line {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        half_width: f32,
    },
}

/// Axis constraint for draggable shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DragAxis {
    Both,
    X,
    Y,
}

/// Bounds constraint for draggable shapes. Fields are populated during
/// shape parsing and read during drag event handling for clamping.
#[derive(Debug, Clone)]
pub(crate) struct DragBounds {
    pub min_x: f32,
    pub max_x: f32,
    pub min_y: f32,
    pub max_y: f32,
}

/// Parsed interactive configuration for a canvas shape.
///
/// Extracted from the `"interactive"` field on shape JSON during
/// `ensure_caches`. Stored in `WidgetCaches` so `update()` can hit-test
/// without re-parsing JSON every frame.
#[derive(Debug, Clone)]
pub(crate) struct InteractiveShape {
    /// Unique ID for this shape (from `interactive.id`).
    pub id: String,
    /// Which layer this shape belongs to.
    pub layer: String,
    /// Geometric bounds for hit testing.
    pub hit_region: HitRegion,
    pub on_click: bool,
    pub on_hover: bool,
    pub draggable: bool,
    pub drag_axis: DragAxis,
    pub drag_bounds: Option<DragBounds>,
    /// Cursor to show when hovering (e.g. "pointer", "grab").
    pub cursor: Option<String>,
    /// Whether this shape has hover/pressed style overrides.
    pub has_hover_style: bool,
    pub has_pressed_style: bool,
    /// Tooltip text to show on hover.
    pub tooltip: Option<String>,
    /// Accessibility overrides for this element. Parsed from the
    /// `a11y` sub-field of `interactive` using the same
    /// [`A11yOverrides`](super::a11y::A11yOverrides) struct that all
    /// other widgets use -- same fields, same parsing, same validation.
    pub a11y: Option<super::a11y::A11yOverrides>,
}

/// Active drag state tracked in `CanvasState`.
#[derive(Debug, Clone)]
struct DragState {
    shape_id: String,
    last: Point,
}

/// Test whether a point is inside a hit region.
fn hit_test(point: Point, region: &HitRegion) -> bool {
    match *region {
        HitRegion::Rect { x, y, w, h } => {
            point.x >= x && point.x <= x + w && point.y >= y && point.y <= y + h
        }
        HitRegion::Circle { cx, cy, r } => {
            let dx = point.x - cx;
            let dy = point.y - cy;
            dx * dx + dy * dy <= r * r
        }
        HitRegion::Line {
            x1,
            y1,
            x2,
            y2,
            half_width,
        } => {
            // Distance from point to line segment.
            let dx = x2 - x1;
            let dy = y2 - y1;
            let len_sq = dx * dx + dy * dy;
            if len_sq < f32::EPSILON {
                // Degenerate line (zero length) -- treat as point.
                let d = ((point.x - x1).powi(2) + (point.y - y1).powi(2)).sqrt();
                return d <= half_width;
            }
            // Project point onto line, clamped to segment.
            let t = ((point.x - x1) * dx + (point.y - y1) * dy) / len_sq;
            let t = t.clamp(0.0, 1.0);
            let proj_x = x1 + t * dx;
            let proj_y = y1 + t * dy;
            let dist_sq = (point.x - proj_x).powi(2) + (point.y - proj_y).powi(2);
            dist_sq <= half_width * half_width
        }
    }
}

/// Find the topmost interactive shape under the given point.
///
/// Shapes are tested in reverse order (last in list = topmost drawn = tested first).
fn find_hit_shape(point: Point, shapes: &[InteractiveShape]) -> Option<&InteractiveShape> {
    shapes
        .iter()
        .rev()
        .find(|s| (s.on_click || s.on_hover || s.draggable) && hit_test(point, &s.hit_region))
}

/// Parse an `InteractiveShape` from a shape's JSON `"interactive"` field.
///
/// Returns `None` if the interactive field is missing or has no `id`.
fn parse_interactive_shape(shape: &Value, layer_name: &str) -> Option<InteractiveShape> {
    let interactive = shape.get("interactive")?.as_object()?;
    let id = interactive.get("id")?.as_str()?.to_string();
    if id.is_empty() {
        return None;
    }

    // Validate known fields -- warn on typos like "on_clck".
    const KNOWN_INTERACTIVE_FIELDS: &[&str] = &[
        "id",
        "on_click",
        "on_hover",
        "cursor",
        "draggable",
        "drag_axis",
        "drag_bounds",
        "tooltip",
        "a11y",
        "hit_rect",
        "hover_style",
        "pressed_style",
    ];
    for key in interactive.keys() {
        if !KNOWN_INTERACTIVE_FIELDS.contains(&key.as_str()) {
            log::warn!(
                "canvas shape '{id}': unknown interactive field '{key}' \
                 (known: id, on_click, on_hover, cursor, draggable, \
                 drag_axis, drag_bounds, tooltip, a11y, hit_rect, \
                 hover_style, pressed_style)"
            );
        }
    }

    // Warn on common mistakes.
    let draggable = interactive
        .get("draggable")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !draggable && interactive.contains_key("drag_bounds") {
        log::warn!("canvas shape '{id}': drag_bounds set without draggable: true");
    }
    if !draggable && interactive.contains_key("drag_axis") {
        log::warn!("canvas shape '{id}': drag_axis set without draggable: true");
    }

    let hit_region = compute_hit_region(shape, interactive)?;

    let drag_axis = match interactive
        .get("drag_axis")
        .and_then(|v| v.as_str())
        .unwrap_or("both")
    {
        "x" => DragAxis::X,
        "y" => DragAxis::Y,
        _ => DragAxis::Both,
    };

    let drag_bounds = interactive.get("drag_bounds").and_then(|v| {
        let obj = v.as_object()?;
        let get = |key: &str| -> Option<f32> {
            let val = obj.get(key).and_then(|v| v.as_f64()).map(|v| v as f32);
            if val.is_none() {
                log::warn!("canvas shape '{id}': drag_bounds missing '{key}'");
            }
            val
        };
        let min_x = get("min_x")?;
        let max_x = get("max_x")?;
        let min_y = get("min_y")?;
        let max_y = get("max_y")?;
        // Ensure min <= max to avoid panic from f32::clamp in debug.
        Some(DragBounds {
            min_x: min_x.min(max_x),
            max_x: min_x.max(max_x),
            min_y: min_y.min(max_y),
            max_y: min_y.max(max_y),
        })
    });

    let cursor = interactive
        .get("cursor")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Some(InteractiveShape {
        id,
        layer: layer_name.to_string(),
        hit_region,
        on_click: interactive
            .get("on_click")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        on_hover: interactive
            .get("on_hover")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        draggable: interactive
            .get("draggable")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        drag_axis,
        drag_bounds,
        cursor,
        has_hover_style: interactive.get("hover_style").is_some(),
        has_pressed_style: interactive.get("pressed_style").is_some(),
        tooltip: interactive
            .get("tooltip")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        a11y: interactive
            .get("a11y")
            .and_then(super::a11y::A11yOverrides::from_a11y_value),
    })
}

/// Compute the bounding box of a single shape in its parent's coordinate
/// system. Returns `(min_x, min_y, max_x, max_y)` or `None` if bounds
/// can't be determined for this shape type.
fn child_bounds(child: &Value) -> Option<(f32, f32, f32, f32)> {
    let ct = child.get("type").and_then(|v| v.as_str())?;
    match ct {
        "rect" => {
            let x = json_f32(child, "x");
            let y = json_f32(child, "y");
            let w = json_f32(child, "w");
            let h = json_f32(child, "h");
            Some((x, y, x + w, y + h))
        }
        "circle" => {
            let cx = json_f32(child, "x");
            let cy = json_f32(child, "y");
            let r = json_f32(child, "r");
            Some((cx - r, cy - r, cx + r, cy + r))
        }
        "line" => {
            let x1 = json_f32(child, "x1");
            let y1 = json_f32(child, "y1");
            let x2 = json_f32(child, "x2");
            let y2 = json_f32(child, "y2");
            Some((x1.min(x2), y1.min(y2), x1.max(x2), y1.max(y2)))
        }
        "text" => {
            let x = json_f32(child, "x");
            let y = json_f32(child, "y");
            let content = child.get("content").and_then(|v| v.as_str()).unwrap_or("");
            let size = child.get("size").and_then(|v| v.as_f64()).unwrap_or(16.0) as f32;
            let est_w = content.chars().count() as f32 * size * 0.6;
            Some((x, y - size, x + est_w, y))
        }
        "image" | "svg" => {
            let x = json_f32(child, "x");
            let y = json_f32(child, "y");
            let w = json_f32(child, "w");
            let h = json_f32(child, "h");
            Some((x, y, x + w, y + h))
        }
        "group" => {
            let gx = json_f32(child, "x");
            let gy = json_f32(child, "y");
            let nested = child.get("children").and_then(|v| v.as_array())?;
            let (min_x, min_y, max_x, max_y) = children_bounds(nested)?;
            Some((gx + min_x, gy + min_y, gx + max_x, gy + max_y))
        }
        // Paths, transforms, clips, and other types can't have their
        // bounds automatically determined. Use hit_rect on the parent.
        _ => None,
    }
}

/// Compute the union bounding box of a list of child shapes.
/// Returns `(min_x, min_y, max_x, max_y)` or `None` if no children
/// have computable bounds.
fn children_bounds(children: &[Value]) -> Option<(f32, f32, f32, f32)> {
    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    let mut max_x = f32::MIN;
    let mut max_y = f32::MIN;
    let mut has_bounds = false;
    for child in children {
        if let Some((cx0, cy0, cx1, cy1)) = child_bounds(child) {
            min_x = min_x.min(cx0);
            min_y = min_y.min(cy0);
            max_x = max_x.max(cx1);
            max_y = max_y.max(cy1);
            has_bounds = true;
        }
    }
    has_bounds.then_some((min_x, min_y, max_x, max_y))
}

/// Compute the hit region from a shape's geometry.
fn compute_hit_region(
    shape: &Value,
    interactive: &serde_json::Map<String, Value>,
) -> Option<HitRegion> {
    // Explicit hit_rect overrides geometric inference.
    if let Some(hr) = interactive.get("hit_rect").and_then(|v| v.as_object()) {
        let x = hr.get("x")?.as_f64()? as f32;
        let y = hr.get("y")?.as_f64()? as f32;
        let w = hr.get("w").or(hr.get("width"))?.as_f64()? as f32;
        let h = hr.get("h").or(hr.get("height"))?.as_f64()? as f32;
        return Some(HitRegion::Rect { x, y, w, h });
    }

    let shape_type = shape.get("type").and_then(|v| v.as_str())?;
    match shape_type {
        "rect" => {
            let x = json_f32(shape, "x");
            let y = json_f32(shape, "y");
            let w = json_f32(shape, "w");
            let h = json_f32(shape, "h");
            Some(HitRegion::Rect { x, y, w, h })
        }
        "circle" => {
            let cx = json_f32(shape, "x");
            let cy = json_f32(shape, "y");
            let r = json_f32(shape, "r");
            Some(HitRegion::Circle { cx, cy, r })
        }
        "line" => {
            let x1 = json_f32(shape, "x1");
            let y1 = json_f32(shape, "y1");
            let x2 = json_f32(shape, "x2");
            let y2 = json_f32(shape, "y2");
            let stroke_width = shape
                .get("stroke")
                .and_then(|s| s.get("width"))
                .and_then(|v| v.as_f64())
                .unwrap_or(1.0) as f32;
            // Legacy: "width" at top level if no stroke object
            let stroke_width = if stroke_width <= 1.0 {
                shape
                    .get("width")
                    .and_then(|v| v.as_f64())
                    .map(|v| v as f32)
                    .unwrap_or(stroke_width)
            } else {
                stroke_width
            };
            let half_width = (stroke_width / 2.0).max(2.0); // min 2px for usability
            Some(HitRegion::Line {
                x1,
                y1,
                x2,
                y2,
                half_width,
            })
        }
        "group" => {
            let group_x = json_f32(shape, "x");
            let group_y = json_f32(shape, "y");
            let children = shape.get("children").and_then(|v| v.as_array());
            if let Some(children) = children {
                if let Some((min_x, min_y, max_x, max_y)) = children_bounds(children) {
                    Some(HitRegion::Rect {
                        x: min_x + group_x,
                        y: min_y + group_y,
                        w: max_x - min_x,
                        h: max_y - min_y,
                    })
                } else {
                    None
                }
            } else {
                None
            }
        }
        // For unsupported shape types, the host can provide an explicit
        // hit_rect in the interactive field as a fallback.
        _ => {
            log::debug!(
                "canvas: no geometric hit region for shape type '{shape_type}', \
                 use interactive.hit_rect for hit testing"
            );
            None
        }
    }
}

/// Parse a cursor name string into an iced mouse interaction.
fn parse_cursor_interaction(cursor: &str) -> mouse::Interaction {
    match cursor {
        "pointer" => mouse::Interaction::Pointer,
        "grab" => mouse::Interaction::Grab,
        "grabbing" => mouse::Interaction::Grabbing,
        "crosshair" => mouse::Interaction::Crosshair,
        "move" => mouse::Interaction::Move,
        "text" => mouse::Interaction::Text,
        "not_allowed" | "not-allowed" => mouse::Interaction::NotAllowed,
        "no_drop" | "no-drop" => mouse::Interaction::NoDrop,
        "help" => mouse::Interaction::Help,
        "progress" => mouse::Interaction::Progress,
        "wait" => mouse::Interaction::Wait,
        "cell" => mouse::Interaction::Cell,
        "copy" => mouse::Interaction::Copy,
        "alias" => mouse::Interaction::Alias,
        "zoom_in" | "zoom-in" => mouse::Interaction::ZoomIn,
        "zoom_out" | "zoom-out" => mouse::Interaction::ZoomOut,
        "col_resize" | "col-resize" => mouse::Interaction::ResizingColumn,
        "row_resize" | "row-resize" => mouse::Interaction::ResizingRow,
        _ => mouse::Interaction::Pointer, // default for interactive shapes
    }
}

/// Extract sorted layer data directly from canvas props as cloned `Value`s.
///
/// This avoids the serialize-then-deserialize round trip that
/// `canvas_layer_map` + deserialization would do. `canvas_layer_map` is
/// still used in `ensure_caches` where string hashing is needed, but
/// `render_canvas` only needs the parsed shapes.
fn canvas_layers_from_props(
    props: Option<&serde_json::Map<String, Value>>,
) -> Vec<(String, Vec<Value>)> {
    fn truncate_shapes(name: &str, mut shapes: Vec<Value>) -> Vec<Value> {
        if shapes.len() > MAX_SHAPES_PER_LAYER {
            log::warn!(
                "canvas layer `{name}` has {} shapes, truncating to {MAX_SHAPES_PER_LAYER}",
                shapes.len(),
            );
            shapes.truncate(MAX_SHAPES_PER_LAYER);
        }
        shapes
    }

    if let Some(layers_obj) = props
        .and_then(|p| p.get("layers"))
        .and_then(|v| v.as_object())
    {
        let mut layers: Vec<(String, Vec<Value>)> = layers_obj
            .iter()
            .map(|(name, shapes_val)| {
                let shapes = shapes_val.as_array().cloned().unwrap_or_default();
                (name.clone(), truncate_shapes(name, shapes))
            })
            .collect();
        layers.sort_by(|a, b| a.0.cmp(&b.0));
        layers
    } else if let Some(shapes_arr) = props
        .and_then(|p| p.get("shapes"))
        .and_then(|v| v.as_array())
    {
        vec![(
            "default".to_string(),
            truncate_shapes("default", shapes_arr.clone()),
        )]
    } else {
        Vec::new()
    }
}

#[derive(Default)]
struct CanvasState {
    cursor_position: Option<Point>,
    /// ID of the interactive shape currently under the cursor.
    hovered_shape: Option<String>,
    /// ID of the shape being pressed (mouse down, not yet released).
    pressed_shape: Option<String>,
    /// Active drag state (shape being dragged).
    dragging: Option<DragState>,
    /// Index of the interactive shape that has keyboard focus.
    focused_index: Option<usize>,
}

struct CanvasProgram<'a> {
    /// Sorted layer data: (layer_name, shapes array).
    layers: Vec<(String, Vec<Value>)>,
    /// Per-layer caches from WidgetCaches.
    caches: Option<&'a HashMap<String, (u64, canvas::Cache)>>,
    background: Option<Color>,
    id: String,
    on_press: bool,
    on_release: bool,
    on_move: bool,
    on_scroll: bool,
    /// Reference to the image registry for resolving in-memory image handles.
    images: &'a crate::image_registry::ImageRegistry,
    /// Interactive shapes parsed during ensure_caches.
    interactive_shapes: &'a [InteractiveShape],
}

impl CanvasProgram<'_> {
    fn is_interactive(&self) -> bool {
        self.on_press
            || self.on_release
            || self.on_move
            || self.on_scroll
            || !self.interactive_shapes.is_empty()
    }

    /// Find the layer name that currently has an active hover or pressed
    /// shape with style overrides. Returns `None` if no interaction is
    /// active or the active shape has no style overrides.
    fn layer_with_active_interaction(&self, state: &CanvasState) -> Option<String> {
        // Pressed takes priority -- if the user is pressing a shape on
        // layer A while hovering a shape on layer B, layer A needs the
        // force-redraw for pressed_style.
        let active_id = state
            .pressed_shape
            .as_deref()
            .or(state.hovered_shape.as_deref());
        let active_id = active_id?;
        let shape = self.interactive_shapes.iter().find(|s| s.id == active_id)?;
        if shape.has_hover_style || shape.has_pressed_style {
            Some(shape.layer.clone())
        } else {
            None
        }
    }

    /// Get the tooltip text for the currently hovered shape, if any.
    fn active_tooltip(&self, state: &CanvasState) -> Option<String> {
        let hovered_id = state.hovered_shape.as_deref()?;
        let shape = self
            .interactive_shapes
            .iter()
            .find(|s| s.id == hovered_id)?;
        shape.tooltip.clone()
    }

    /// Draw shapes with hover/pressed style overrides applied to the
    /// active shape. Used when a layer needs fresh drawing due to
    /// interaction state changes.
    fn draw_shapes_with_overrides(
        &self,
        frame: &mut canvas::Frame,
        shapes: &[&Value],
        state: &CanvasState,
        images: &crate::image_registry::ImageRegistry,
    ) {
        let hovered = state.hovered_shape.as_deref();
        let pressed = state.pressed_shape.as_deref();
        let mut i = 0;
        while i < shapes.len() {
            let shape = shapes[i];
            let shape_type = shape.get("type").and_then(|v| v.as_str()).unwrap_or("");
            match shape_type {
                "push_clip" => {
                    let x = shape.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                    let y = shape.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                    let w = shape.get("w").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                    let h = shape.get("h").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                    let (end_offset, clipped) = collect_clipped_shapes(&shapes[i + 1..]);
                    let clip_rect = iced::Rectangle {
                        x,
                        y,
                        width: w,
                        height: h,
                    };
                    frame.with_clip(clip_rect, |f| {
                        // For clipped regions, fall back to normal drawing.
                        // Overrides on clipped shapes are a rare edge case.
                        draw_canvas_shapes(f, &clipped, images);
                    });
                    i = i + 1 + end_offset + 1;
                }
                "pop_clip" => {
                    i += 1;
                }
                "group" => {
                    let gx = json_f32(shape, "x");
                    let gy = json_f32(shape, "y");
                    let group_id = shape
                        .get("interactive")
                        .and_then(|v| v.get("id"))
                        .and_then(|v| v.as_str());
                    let group_active =
                        group_id.is_some_and(|sid| pressed == Some(sid) || hovered == Some(sid));

                    if let Some(children) = shape.get("children").and_then(|v| v.as_array()) {
                        frame.push_transform();
                        frame.translate(Vector::new(gx, gy));
                        if group_active {
                            let is_pressed = group_id.is_some_and(|sid| pressed == Some(sid));
                            for child in children {
                                // Each child can have its own hover_style/pressed_style
                                // at the top level of the child shape JSON.
                                let override_style = if is_pressed {
                                    child.get("pressed_style")
                                } else {
                                    None
                                }
                                .or_else(|| child.get("hover_style"));

                                if let Some(overrides) = override_style {
                                    let merged = merge_shape_style(child, overrides);
                                    draw_canvas_shape(frame, &merged, images);
                                } else {
                                    draw_canvas_shape(frame, child, images);
                                }
                            }
                        } else {
                            let child_refs: Vec<&Value> = children.iter().collect();
                            draw_canvas_shapes(frame, &child_refs, images);
                        }
                        frame.pop_transform();
                    }
                    i += 1;
                }
                _ => {
                    let shape_id = shape
                        .get("interactive")
                        .and_then(|v| v.get("id"))
                        .and_then(|v| v.as_str());

                    let needs_override =
                        shape_id.is_some_and(|sid| pressed == Some(sid) || hovered == Some(sid));

                    if needs_override {
                        let sid = shape_id.unwrap();
                        let interactive = shape.get("interactive").unwrap();
                        // Pressed style takes priority over hover style.
                        let override_style = if pressed == Some(sid) {
                            interactive.get("pressed_style")
                        } else {
                            None
                        }
                        .or_else(|| interactive.get("hover_style"));

                        if let Some(overrides) = override_style {
                            let merged = merge_shape_style(shape, overrides);
                            draw_canvas_shape(frame, &merged, images);
                        } else {
                            draw_canvas_shape(frame, shape, images);
                        }
                    } else {
                        draw_canvas_shape(frame, shape, images);
                    }
                    i += 1;
                }
            }
        }
    }
}

/// Merge style overrides into a shape's JSON. The override object can
/// contain `fill`, `stroke`, `stroke_width`, `opacity` -- these replace
/// the corresponding fields on the shape.
fn merge_shape_style(shape: &Value, overrides: &Value) -> Value {
    let mut merged = shape.clone();
    if let (Some(merged_obj), Some(override_obj)) = (merged.as_object_mut(), overrides.as_object())
    {
        for (key, val) in override_obj {
            merged_obj.insert(key.clone(), val.clone());
        }
    }
    merged
}

/// Draw a tooltip overlay at the cursor position.
fn draw_tooltip(
    frame: &mut canvas::Frame,
    text: &str,
    cursor: Point,
    bounds: Size,
    theme: &iced::Theme,
) {
    use iced::widget::canvas::Text;

    let palette = theme.palette();
    // Use inverse colors: dark bg on light theme, light bg on dark theme.
    let (bg_color, text_color) = if palette.is_dark {
        (
            Color::from_rgba(0.85, 0.85, 0.85, 0.95),
            Color::from_rgb(0.1, 0.1, 0.1),
        )
    } else {
        (
            Color::from_rgba(0.15, 0.15, 0.15, 0.95),
            Color::from_rgb(0.95, 0.95, 0.95),
        )
    };

    let padding = 6.0;
    let font_size = 13.0;
    // Estimate text width (rough: 0.6 * font_size per char).
    let est_width = text.chars().count() as f32 * font_size * 0.6 + padding * 2.0;
    let est_height = font_size + padding * 2.0;

    // Position tooltip near cursor, clamped to canvas bounds.
    let mut x = cursor.x + 12.0;
    let mut y = cursor.y - est_height - 4.0;
    if x + est_width > bounds.width {
        x = (cursor.x - est_width - 4.0).max(0.0);
    }
    if y < 0.0 {
        y = cursor.y + 20.0;
    }

    // Background
    let bg_rect = iced::Rectangle {
        x,
        y,
        width: est_width,
        height: est_height,
    };
    frame.fill_rectangle(
        Point::new(bg_rect.x, bg_rect.y),
        Size::new(bg_rect.width, bg_rect.height),
        bg_color,
    );

    // Text
    frame.fill_text(Text {
        content: text.to_string(),
        position: Point::new(x + padding, y + padding),
        color: text_color,
        size: Pixels(font_size),
        ..Text::default()
    });
}

/// Pick the most important `Action` when multiple events fire in one
/// `update()` call. iced's `Action` can only carry one message, so
/// when shape events (enter/leave/click) and raw canvas events
/// (move/press/release) fire simultaneously, we keep the shape event.
/// Raw canvas events use Replace coalescing, so the next frame
/// delivers the latest position anyway.
fn pick_action(
    existing: Option<iced::widget::Action<Message>>,
    new: iced::widget::Action<Message>,
) -> iced::widget::Action<Message> {
    existing.unwrap_or(new)
}

/// Parse a `fill_rule` string into a `canvas::fill::Rule`. Defaults to `NonZero`.
fn parse_fill_rule(value: Option<&Value>) -> canvas::fill::Rule {
    match value.and_then(|v| v.as_str()) {
        Some("even_odd") => canvas::fill::Rule::EvenOdd,
        _ => canvas::fill::Rule::NonZero,
    }
}

/// Parse a canvas fill value. If string, hex color. If gradient object,
/// build a gradient::Linear. Falls back to white. The `shape` parameter
/// provides the parent shape object for reading the `fill_rule` key.
pub(crate) fn parse_canvas_fill(value: &Value, shape: &Value) -> canvas::Fill {
    let rule = parse_fill_rule(shape.get("fill_rule"));
    match value {
        Value::String(s) => {
            let color = parse_hex_color(s).unwrap_or(Color::WHITE);
            canvas::Fill {
                style: canvas::Style::Solid(color),
                rule,
            }
        }
        Value::Object(obj) => match obj.get("type").and_then(|v| v.as_str()) {
            Some("linear") => {
                // Warn on unrecognized canvas gradient keys
                let valid_keys: &[&str] = &["type", "start", "end", "stops"];
                for key in obj.keys() {
                    if !valid_keys.contains(&key.as_str()) {
                        log::warn!(
                            "unrecognized canvas gradient key '{}' (valid: {:?})",
                            key,
                            valid_keys
                        );
                    }
                }

                let start = obj
                    .get("start")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        Point::new(
                            a.first().and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                            a.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                        )
                    })
                    .unwrap_or(Point::ORIGIN);
                let end = obj
                    .get("end")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        Point::new(
                            a.first().and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                            a.get(1).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                        )
                    })
                    .unwrap_or(Point::ORIGIN);
                let mut linear = canvas::gradient::Linear::new(start, end);
                if let Some(stops) = obj.get("stops").and_then(|v| v.as_array()) {
                    for stop in stops {
                        if let Some(arr) = stop.as_array() {
                            let offset = arr.first().and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                            let color = arr
                                .get(1)
                                .and_then(parse_color)
                                .unwrap_or(Color::TRANSPARENT);
                            linear = linear.add_stop(offset, color);
                        }
                    }
                }
                canvas::Fill {
                    style: canvas::Style::Gradient(canvas::Gradient::Linear(linear)),
                    rule,
                }
            }
            Some(other) => {
                log::warn!(
                    "unrecognized canvas gradient type '{}' (supported: \"linear\")",
                    other
                );
                let color = parse_color(value).unwrap_or(Color::WHITE);
                canvas::Fill {
                    style: canvas::Style::Solid(color),
                    rule,
                }
            }
            _ => {
                let color = parse_color(value).unwrap_or(Color::WHITE);
                canvas::Fill {
                    style: canvas::Style::Solid(color),
                    rule,
                }
            }
        },
        _ => canvas::Fill {
            style: canvas::Style::Solid(Color::WHITE),
            rule,
        },
    }
}

/// Parse a canvas stroke from a JSON object.
pub(crate) fn parse_canvas_stroke(value: &Value) -> canvas::Stroke<'static> {
    let obj = match value.as_object() {
        Some(o) => o,
        None => return canvas::Stroke::default(),
    };
    let color = obj
        .get("color")
        .and_then(parse_color)
        .unwrap_or(Color::WHITE);
    let width = obj
        .get("width")
        .and_then(|v| v.as_f64())
        .map(|v| v as f32)
        .unwrap_or(1.0);
    let cap = match obj.get("cap").and_then(|v| v.as_str()).unwrap_or("butt") {
        "round" => canvas::LineCap::Round,
        "square" => canvas::LineCap::Square,
        _ => canvas::LineCap::Butt,
    };
    let join = match obj.get("join").and_then(|v| v.as_str()).unwrap_or("miter") {
        "round" => canvas::LineJoin::Round,
        "bevel" => canvas::LineJoin::Bevel,
        _ => canvas::LineJoin::Miter,
    };
    let mut stroke = canvas::Stroke::default()
        .with_color(color)
        .with_width(width)
        .with_line_cap(cap)
        .with_line_join(join);
    if let Some(dash_obj) = obj.get("dash").and_then(|v| v.as_object()) {
        let segments_val = dash_obj
            .get("segments")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let segments: Vec<f32> = segments_val
            .iter()
            .filter_map(|v| v.as_f64().map(|n| n as f32))
            .collect();
        let offset = dash_obj
            .get("offset")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(0);
        // LineDash borrows segments, but we need 'static. Intern via a
        // global cache so identical patterns reuse the same allocation and
        // we only leak once per unique dash pattern (not per render).
        let segments: &'static [f32] = intern_dash_segments(segments);
        stroke.line_dash = canvas::LineDash { segments, offset };
    }
    stroke
}

/// Maximum number of unique dash patterns cached. Beyond this limit,
/// new patterns are still leaked (LineDash requires `'static` segments)
/// but not inserted into the cache, bounding the HashMap's memory.
const MAX_DASH_CACHE: usize = 1024;

/// Intern a dash segment array so that identical patterns share one
/// leaked allocation. Without this, every re-render of a dashed stroke
/// leaked a fresh `Box<[f32]>` via `Box::leak`.
///
/// When the cache reaches [`MAX_DASH_CACHE`] entries, new unique
/// patterns still get a leaked slice (LineDash requires `'static`
/// segments) but are not inserted into the cache. A one-time warning
/// is logged when this limit is hit.
fn intern_dash_segments(segments: Vec<f32>) -> &'static [f32] {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{LazyLock, Mutex};

    static CACHE: LazyLock<Mutex<HashMap<Vec<u32>, &'static [f32]>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    static WARNED: AtomicBool = AtomicBool::new(false);

    let key: Vec<u32> = segments.iter().map(|s| s.to_bits()).collect();
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());

    if let Some(existing) = cache.get(&key) {
        return existing;
    }

    let leaked: &'static [f32] = Box::leak(segments.into_boxed_slice());

    if cache.len() >= MAX_DASH_CACHE {
        if !WARNED.swap(true, Ordering::Relaxed) {
            log::warn!(
                "dash segment cache full ({MAX_DASH_CACHE} entries); \
                 new patterns will leak without caching"
            );
        }
        return leaked;
    }

    cache.insert(key, leaked);
    leaked
}

/// Build a Path from an array of path commands.
fn build_path_from_commands(commands: &[Value]) -> canvas::Path {
    canvas::Path::new(|builder| {
        for cmd in commands {
            if let Some(s) = cmd.as_str() {
                if s == "close" {
                    builder.close();
                }
                continue;
            }
            let arr = match cmd.as_array() {
                Some(a) if !a.is_empty() => a,
                _ => continue,
            };
            let cmd_name = arr[0].as_str().unwrap_or("");
            let f = |i: usize| -> f32 {
                arr.get(i)
                    .and_then(|v| v.as_f64())
                    .map(|v| v as f32)
                    .unwrap_or(0.0)
            };
            match cmd_name {
                "move_to" => builder.move_to(Point::new(f(1), f(2))),
                "line_to" => builder.line_to(Point::new(f(1), f(2))),
                "bezier_to" => builder.bezier_curve_to(
                    Point::new(f(1), f(2)),
                    Point::new(f(3), f(4)),
                    Point::new(f(5), f(6)),
                ),
                "quadratic_to" => {
                    builder.quadratic_curve_to(Point::new(f(1), f(2)), Point::new(f(3), f(4)))
                }
                "arc" => {
                    builder.arc(canvas::path::Arc {
                        center: Point::new(f(1), f(2)),
                        radius: f(3),
                        start_angle: Radians(f(4)),
                        end_angle: Radians(f(5)),
                    });
                }
                "arc_to" => {
                    builder.arc_to(Point::new(f(1), f(2)), Point::new(f(3), f(4)), f(5));
                }
                "ellipse" => {
                    builder.ellipse(canvas::path::arc::Elliptical {
                        center: Point::new(f(1), f(2)),
                        radii: Vector::new(f(3), f(4)),
                        rotation: Radians(f(5)),
                        start_angle: Radians(f(6)),
                        end_angle: Radians(f(7)),
                    });
                }
                "rounded_rect" => {
                    builder.rounded_rectangle(
                        Point::new(f(1), f(2)),
                        Size::new(f(3), f(4)),
                        iced::border::Radius::new(f(5)),
                    );
                }
                _ => {}
            }
        }
    })
}

/// Draw a sequence of shapes, handling push_clip/pop_clip nesting.
fn draw_canvas_shapes(
    frame: &mut canvas::Frame,
    shapes: &[&Value],
    images: &crate::image_registry::ImageRegistry,
) {
    let mut i = 0;
    while i < shapes.len() {
        let shape = shapes[i];
        let shape_type = shape.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match shape_type {
            "push_clip" => {
                let x = shape.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                let y = shape.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                let w = shape.get("w").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                let h = shape.get("h").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                let (end_offset, clipped) = collect_clipped_shapes(&shapes[i + 1..]);
                let clip_rect = iced::Rectangle {
                    x,
                    y,
                    width: w,
                    height: h,
                };
                frame.with_clip(clip_rect, |f| {
                    draw_canvas_shapes(f, &clipped, images);
                });
                // Skip past the matching pop_clip
                i = i + 1 + end_offset + 1;
            }
            "pop_clip" => {
                // Stray pop_clip at top level -- should not happen if properly paired.
                log::warn!("canvas: pop_clip without matching push_clip");
                i += 1;
            }
            _ => {
                draw_canvas_shape(frame, shape, images);
                i += 1;
            }
        }
    }
}

/// Collect shapes between a push_clip and its matching pop_clip, respecting
/// nesting. Returns (index_of_pop_clip_in_slice, collected_shapes).
pub(crate) fn collect_clipped_shapes<'a>(shapes: &[&'a Value]) -> (usize, Vec<&'a Value>) {
    let mut depth: usize = 0;
    let mut result: Vec<&'a Value> = Vec::new();
    for (i, &shape) in shapes.iter().enumerate() {
        let t = shape.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match t {
            "push_clip" => {
                depth += 1;
                result.push(shape);
            }
            "pop_clip" if depth == 0 => {
                return (i, result);
            }
            "pop_clip" => {
                depth -= 1;
                result.push(shape);
            }
            _ => {
                result.push(shape);
            }
        }
    }
    // No matching pop_clip found -- draw all remaining shapes anyway.
    log::warn!("canvas: push_clip without matching pop_clip");
    (shapes.len(), result)
}

/// Apply per-shape opacity to a `canvas::Fill`. Multiplies the opacity
/// into solid color alpha. Gradient stops are left unchanged (the host
/// should bake opacity into gradient stop colors if needed).
fn apply_opacity_to_fill(shape: &Value, mut fill: canvas::Fill) -> canvas::Fill {
    if let Some(opacity) = shape.get("opacity").and_then(|v| v.as_f64()) {
        let a = opacity as f32;
        if let canvas::Style::Solid(ref mut c) = fill.style {
            c.a *= a;
        }
    }
    fill
}

/// Apply per-shape opacity to a `canvas::Stroke`.
fn apply_opacity_to_stroke(
    shape: &Value,
    mut stroke: canvas::Stroke<'static>,
) -> canvas::Stroke<'static> {
    if let Some(opacity) = shape.get("opacity").and_then(|v| v.as_f64()) {
        let a = opacity as f32;
        if let canvas::Style::Solid(ref mut c) = stroke.style {
            c.a *= a;
        }
    }
    stroke
}

/// Apply per-shape opacity to a plain color (used by text fill and
/// legacy line stroke).
fn apply_opacity_to_color(shape: &Value, mut color: Color) -> Color {
    if let Some(opacity) = shape.get("opacity").and_then(|v| v.as_f64()) {
        color.a *= opacity as f32;
    }
    color
}

/// Parse horizontal text alignment from a JSON string value.
fn parse_canvas_text_align_x(value: Option<&Value>) -> iced::widget::text::Alignment {
    match value.and_then(|v| v.as_str()) {
        Some("left") | Some("start") => iced::widget::text::Alignment::Left,
        Some("center") => iced::widget::text::Alignment::Center,
        Some("right") | Some("end") => iced::widget::text::Alignment::Right,
        _ => iced::widget::text::Alignment::Default,
    }
}

/// Parse vertical text alignment from a JSON string value.
fn parse_canvas_text_align_y(value: Option<&Value>) -> alignment::Vertical {
    match value.and_then(|v| v.as_str()) {
        Some("center") => alignment::Vertical::Center,
        Some("bottom") | Some("end") => alignment::Vertical::Bottom,
        _ => alignment::Vertical::Top,
    }
}

/// Draw a single shape (or transform command) into the frame.
fn draw_canvas_shape(
    frame: &mut canvas::Frame,
    shape: &Value,
    images: &crate::image_registry::ImageRegistry,
) {
    let shape_type = shape.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match shape_type {
        // -- Transform commands --
        "push_transform" => frame.push_transform(),
        "pop_transform" => frame.pop_transform(),
        "translate" => {
            let x = json_f32(shape, "x");
            let y = json_f32(shape, "y");
            frame.translate(Vector::new(x, y));
        }
        "rotate" => {
            let angle = json_f32(shape, "angle");
            frame.rotate(Radians(angle));
        }
        "scale" => {
            // Uniform scaling via "factor", or non-uniform via "x"/"y".
            if let Some(factor) = shape.get("factor").and_then(|v| v.as_f64()) {
                frame.scale(factor as f32);
            } else {
                let x = shape.get("x").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;
                let y = shape.get("y").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;
                frame.scale_nonuniform(Vector::new(x, y));
            }
        }
        // -- Primitive shapes --
        "rect" => {
            let x = json_f32(shape, "x");
            let y = json_f32(shape, "y");
            let w = json_f32(shape, "w");
            let h = json_f32(shape, "h");
            let rect_path = if let Some(r) = shape.get("radius").and_then(|v| v.as_f64()) {
                canvas::Path::rounded_rectangle(
                    Point::new(x, y),
                    Size::new(w, h),
                    iced::border::Radius::from(r as f32),
                )
            } else {
                canvas::Path::rectangle(Point::new(x, y), Size::new(w, h))
            };
            if let Some(fill_val) = shape.get("fill") {
                let fill = apply_opacity_to_fill(shape, parse_canvas_fill(fill_val, shape));
                frame.fill(&rect_path, fill);
            } else if shape.get("stroke").is_none() {
                // Legacy fallback: no fill or stroke key means solid white fill
                let color = apply_opacity_to_color(shape, Color::WHITE);
                frame.fill_rectangle(Point::new(x, y), Size::new(w, h), color);
            }
            if let Some(stroke_val) = shape.get("stroke") {
                let stroke = apply_opacity_to_stroke(shape, parse_canvas_stroke(stroke_val));
                frame.stroke(&rect_path, stroke);
            }
        }
        "circle" => {
            let x = json_f32(shape, "x");
            let y = json_f32(shape, "y");
            let r = json_f32(shape, "r");
            let circle_path = canvas::Path::circle(Point::new(x, y), r);
            if let Some(fill_val) = shape.get("fill") {
                let fill = apply_opacity_to_fill(shape, parse_canvas_fill(fill_val, shape));
                frame.fill(&circle_path, fill);
            } else if shape.get("stroke").is_none() {
                let color = apply_opacity_to_color(shape, Color::WHITE);
                frame.fill(&circle_path, color);
            }
            if let Some(stroke_val) = shape.get("stroke") {
                let stroke = apply_opacity_to_stroke(shape, parse_canvas_stroke(stroke_val));
                frame.stroke(&circle_path, stroke);
            }
        }
        "line" => {
            let x1 = json_f32(shape, "x1");
            let y1 = json_f32(shape, "y1");
            let x2 = json_f32(shape, "x2");
            let y2 = json_f32(shape, "y2");
            let line_path = canvas::Path::line(Point::new(x1, y1), Point::new(x2, y2));
            if let Some(stroke_val) = shape.get("stroke") {
                let stroke = apply_opacity_to_stroke(shape, parse_canvas_stroke(stroke_val));
                frame.stroke(&line_path, stroke);
            } else {
                // Legacy: use fill color as stroke color
                let color = apply_opacity_to_color(shape, json_color(shape, "fill"));
                let width = shape
                    .get("width")
                    .and_then(|v| v.as_f64())
                    .map(|v| v as f32)
                    .unwrap_or(1.0);
                frame.stroke(
                    &line_path,
                    canvas::Stroke::default()
                        .with_color(color)
                        .with_width(width),
                );
            }
        }
        "text" => {
            let x = json_f32(shape, "x");
            let y = json_f32(shape, "y");
            let content = shape.get("content").and_then(|v| v.as_str()).unwrap_or("");
            let fill_color = apply_opacity_to_color(shape, json_color(shape, "fill"));
            let size = shape.get("size").and_then(|v| v.as_f64()).map(|v| v as f32);
            let align_x = parse_canvas_text_align_x(
                shape
                    .get("align_x")
                    .or_else(|| shape.get("horizontal_alignment")),
            );
            let align_y = parse_canvas_text_align_y(
                shape
                    .get("align_y")
                    .or_else(|| shape.get("vertical_alignment")),
            );
            let mut canvas_text = canvas::Text {
                content: content.to_owned(),
                position: Point::new(x, y),
                color: fill_color,
                align_x,
                align_y,
                ..canvas::Text::default()
            };
            if let Some(s) = size {
                canvas_text.size = Pixels(s);
            }
            if let Some(f) = shape.get("font") {
                canvas_text.font = parse_font(f);
            }
            frame.fill_text(canvas_text);
        }
        "path" => {
            let commands = shape
                .get("commands")
                .and_then(|v| v.as_array())
                .map(|a| a.as_slice())
                .unwrap_or(&[]);
            let path = build_path_from_commands(commands);
            if let Some(fill_val) = shape.get("fill") {
                let fill = apply_opacity_to_fill(shape, parse_canvas_fill(fill_val, shape));
                frame.fill(&path, fill);
            }
            if let Some(stroke_val) = shape.get("stroke") {
                let stroke = apply_opacity_to_stroke(shape, parse_canvas_stroke(stroke_val));
                frame.stroke(&path, stroke);
            }
        }
        "image" => {
            let x = json_f32(shape, "x");
            let y = json_f32(shape, "y");
            let w = json_f32(shape, "w");
            let h = json_f32(shape, "h");
            let bounds = iced::Rectangle {
                x,
                y,
                width: w,
                height: h,
            };
            // Source can be a string (file path) or an object with "handle" key
            // (in-memory image from the registry), same as the Image widget.
            let source_val = shape.get("source");
            let handle = match source_val {
                Some(Value::Object(obj)) => {
                    if let Some(name) = obj.get("handle").and_then(|v| v.as_str()) {
                        match images.get(name) {
                            Some(h) => h.clone(),
                            None => {
                                log::warn!("canvas image: unknown registry handle: {name}");
                                return;
                            }
                        }
                    } else {
                        return;
                    }
                }
                _ => {
                    let path = source_val.and_then(|v| v.as_str()).unwrap_or("");
                    iced::widget::image::Handle::from_path(path)
                }
            };
            let rotation = shape
                .get("rotation")
                .and_then(|v| v.as_f64())
                .map(|r| Radians(r as f32))
                .unwrap_or(Radians(0.0));
            let opacity = shape
                .get("opacity")
                .and_then(|v| v.as_f64())
                .map(|o| o as f32)
                .unwrap_or(1.0);
            let img = iced::advanced::image::Image {
                handle,
                filter_method: iced::advanced::image::FilterMethod::default(),
                rotation,
                border_radius: Default::default(),
                opacity,
            };
            frame.draw_image(bounds, img);
        }
        "svg" => {
            let source = shape.get("source").and_then(|v| v.as_str()).unwrap_or("");
            let x = json_f32(shape, "x");
            let y = json_f32(shape, "y");
            let w = json_f32(shape, "w");
            let h = json_f32(shape, "h");
            let bounds = iced::Rectangle {
                x,
                y,
                width: w,
                height: h,
            };
            let handle = iced::widget::svg::Handle::from_path(source);
            frame.draw_svg(bounds, &handle);
        }
        "group" => {
            let x = json_f32(shape, "x");
            let y = json_f32(shape, "y");
            if let Some(children) = shape.get("children").and_then(|v| v.as_array()) {
                frame.push_transform();
                frame.translate(Vector::new(x, y));
                let child_refs: Vec<&Value> = children.iter().collect();
                draw_canvas_shapes(frame, &child_refs, images);
                frame.pop_transform();
            }
        }
        _ => {}
    }
}

impl canvas::Program<Message> for CanvasProgram<'_> {
    type State = CanvasState;

    fn update(
        &self,
        state: &mut CanvasState,
        event: &iced::Event,
        bounds: iced::Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<iced::widget::Action<Message>> {
        let position = match cursor.position_in(bounds) {
            Some(pos) => {
                state.cursor_position = Some(pos);
                pos
            }
            None => {
                // Cursor is outside canvas bounds. Clean up interaction
                // state so we don't have stale hover/drag.
                //
                // DragEnd is processed first (higher priority) because
                // losing a drag-end event leaves the host thinking the
                // drag is still active. ShapeLeave is less critical --
                // the host can infer leave from the drag-end.
                let mut action: Option<iced::widget::Action<Message>> = None;
                if let Some(drag) = state.dragging.take() {
                    let pos = state.cursor_position.unwrap_or(Point::ORIGIN);
                    let msg = Message::CanvasShapeDragEnd {
                        canvas_id: self.id.clone(),
                        shape_id: drag.shape_id,
                        x: pos.x,
                        y: pos.y,
                    };
                    action = Some(iced::widget::Action::publish(msg));
                }
                if let Some(hovered_id) = state.hovered_shape.take() {
                    let msg = Message::CanvasShapeLeave {
                        canvas_id: self.id.clone(),
                        shape_id: hovered_id,
                    };
                    action = Some(pick_action(action, iced::widget::Action::publish(msg)));
                }
                state.pressed_shape = None;
                state.cursor_position = None;
                return action;
            }
        };

        match event {
            iced::Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let mut action: Option<iced::widget::Action<Message>> = None;

                // -- Drag tracking --
                if let Some(ref mut drag) = state.dragging {
                    let shape = self
                        .interactive_shapes
                        .iter()
                        .find(|s| s.id == drag.shape_id);

                    // Start from raw cursor position, apply bounds
                    // clamping first, then axis constraints. This
                    // ensures axis-constrained drags still respect
                    // bounds on the constrained axis.
                    let mut effective = position;
                    if let Some(shape) = shape
                        && let Some(ref db) = shape.drag_bounds
                    {
                        effective.x = effective.x.clamp(db.min_x, db.max_x);
                        effective.y = effective.y.clamp(db.min_y, db.max_y);
                    }
                    let mut dx = effective.x - drag.last.x;
                    let mut dy = effective.y - drag.last.y;
                    if let Some(shape) = shape {
                        match shape.drag_axis {
                            DragAxis::X => dy = 0.0,
                            DragAxis::Y => dx = 0.0,
                            DragAxis::Both => {}
                        }
                    }
                    // Track the effective (clamped) position so deltas
                    // are consistent across frames.
                    drag.last = effective;
                    let msg = Message::CanvasShapeDrag {
                        canvas_id: self.id.clone(),
                        shape_id: drag.shape_id.clone(),
                        x: effective.x,
                        y: effective.y,
                        delta_x: dx,
                        delta_y: dy,
                    };
                    action = Some(iced::widget::Action::publish(msg).and_capture());
                }

                // -- Hover tracking (skip during active drag) --
                if state.dragging.is_none() {
                    let hit = find_hit_shape(position, self.interactive_shapes);
                    let new_hovered = hit.map(|s| s.id.clone());
                    let old_hovered = state.hovered_shape.take();

                    if new_hovered != old_hovered {
                        // Enter is emitted AFTER leave so that pick_action
                        // keeps Enter when both fire (direct A -> B transition).
                        // The host can infer leave from receiving enter for a
                        // different shape. Losing Enter is worse than losing
                        // Leave -- Enter tells the host WHAT is hovered.
                        if let Some(ref old_id) = old_hovered {
                            let msg = Message::CanvasShapeLeave {
                                canvas_id: self.id.clone(),
                                shape_id: old_id.clone(),
                            };
                            action = Some(pick_action(action, iced::widget::Action::publish(msg)));
                        }
                        if let Some(ref new_id) = new_hovered {
                            let msg = Message::CanvasShapeEnter {
                                canvas_id: self.id.clone(),
                                shape_id: new_id.clone(),
                                x: position.x,
                                y: position.y,
                            };
                            // Override any previous action -- Enter takes
                            // priority over Leave and raw canvas move.
                            action = Some(iced::widget::Action::publish(msg));
                        }
                    }
                    state.hovered_shape = new_hovered;
                }

                // -- Raw canvas move event --
                if self.on_move {
                    let msg = Message::CanvasEvent {
                        id: self.id.clone(),
                        kind: "move".to_string(),
                        x: position.x,
                        y: position.y,
                        extra: String::new(),
                    };
                    action = Some(pick_action(action, iced::widget::Action::publish(msg)));
                }

                action
            }

            iced::Event::Mouse(mouse::Event::ButtonPressed(button)) => {
                let btn_str = serialize_mouse_button_for_canvas(button);
                let mut action: Option<iced::widget::Action<Message>> = None;

                // -- Shape press: start drag or track pressed --
                // Drag and click are mutually exclusive: if a shape is
                // draggable, we start a drag (click never fires for it).
                // If it's only clickable, we track pressed state for
                // click detection on release.
                if matches!(button, mouse::Button::Left)
                    && let Some(shape) = find_hit_shape(position, self.interactive_shapes)
                {
                    if shape.draggable {
                        state.dragging = Some(DragState {
                            shape_id: shape.id.clone(),
                            last: position,
                        });
                    } else if shape.on_click {
                        state.pressed_shape = Some(shape.id.clone());
                    }
                }

                // -- Raw canvas press event --
                if self.on_press {
                    let msg = Message::CanvasEvent {
                        id: self.id.clone(),
                        kind: "press".to_string(),
                        x: position.x,
                        y: position.y,
                        extra: btn_str,
                    };
                    action = Some(pick_action(action, iced::widget::Action::publish(msg)));
                }

                action
            }

            iced::Event::Mouse(mouse::Event::ButtonReleased(button)) => {
                let btn_str = serialize_mouse_button_for_canvas(button);
                let mut action: Option<iced::widget::Action<Message>> = None;

                if matches!(button, mouse::Button::Left) {
                    // -- Drag end --
                    if let Some(drag) = state.dragging.take() {
                        let msg = Message::CanvasShapeDragEnd {
                            canvas_id: self.id.clone(),
                            shape_id: drag.shape_id,
                            x: position.x,
                            y: position.y,
                        };
                        action = Some(pick_action(action, iced::widget::Action::publish(msg)));
                    }

                    // -- Click detection: pressed shape == current hover --
                    if let Some(pressed_id) = state.pressed_shape.take() {
                        let still_over = state
                            .hovered_shape
                            .as_ref()
                            .map(|h| h == &pressed_id)
                            .unwrap_or(false);
                        if still_over {
                            let msg = Message::CanvasShapeClick {
                                canvas_id: self.id.clone(),
                                shape_id: pressed_id,
                                x: position.x,
                                y: position.y,
                                button: btn_str.clone(),
                            };
                            action = Some(pick_action(action, iced::widget::Action::publish(msg)));
                        }
                    }
                }

                // -- Raw canvas release event --
                if self.on_release {
                    let msg = Message::CanvasEvent {
                        id: self.id.clone(),
                        kind: "release".to_string(),
                        x: position.x,
                        y: position.y,
                        extra: btn_str,
                    };
                    action = Some(pick_action(action, iced::widget::Action::publish(msg)));
                }

                action
            }

            iced::Event::Mouse(mouse::Event::WheelScrolled { delta }) if self.on_scroll => {
                let (dx, dy) = match delta {
                    mouse::ScrollDelta::Lines { x, y } => (*x, *y),
                    mouse::ScrollDelta::Pixels { x, y } => (*x, *y),
                };
                Some(iced::widget::Action::publish(Message::CanvasScroll {
                    id: self.id.clone(),
                    x: position.x,
                    y: position.y,
                    delta_x: dx,
                    delta_y: dy,
                }))
            }

            iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. })
                if !self.interactive_shapes.is_empty() =>
            {
                use keyboard::key::Named;
                let count = self.interactive_shapes.len();
                // Validate focused_index against current shape count
                // (shapes may have changed between renders).
                if state.focused_index.is_some_and(|idx| idx >= count) {
                    state.focused_index = None;
                }
                match key {
                    keyboard::Key::Named(Named::Tab) if !modifiers.shift() => {
                        match state.focused_index {
                            None => {
                                state.focused_index = Some(0);
                                let shape_id = self.interactive_shapes[0].id.clone();
                                Some(
                                    iced::widget::Action::publish(Message::CanvasShapeFocused {
                                        canvas_id: self.id.clone(),
                                        shape_id,
                                    })
                                    .and_capture(),
                                )
                            }
                            Some(idx) if idx + 1 < count => {
                                let next = idx + 1;
                                state.focused_index = Some(next);
                                let shape_id = self.interactive_shapes[next].id.clone();
                                Some(
                                    iced::widget::Action::publish(Message::CanvasShapeFocused {
                                        canvas_id: self.id.clone(),
                                        shape_id,
                                    })
                                    .and_capture(),
                                )
                            }
                            Some(_) => {
                                // At last element -- clear focus, let Tab propagate.
                                state.focused_index = None;
                                None
                            }
                        }
                    }
                    keyboard::Key::Named(Named::Tab) if modifiers.shift() => {
                        match state.focused_index {
                            None => {
                                // Focus last element.
                                let last = count - 1;
                                state.focused_index = Some(last);
                                let shape_id = self.interactive_shapes[last].id.clone();
                                Some(
                                    iced::widget::Action::publish(Message::CanvasShapeFocused {
                                        canvas_id: self.id.clone(),
                                        shape_id,
                                    })
                                    .and_capture(),
                                )
                            }
                            Some(0) => {
                                // At first element -- clear focus, let Shift+Tab propagate.
                                state.focused_index = None;
                                None
                            }
                            Some(idx) => {
                                let prev = idx - 1;
                                state.focused_index = Some(prev);
                                let shape_id = self.interactive_shapes[prev].id.clone();
                                Some(
                                    iced::widget::Action::publish(Message::CanvasShapeFocused {
                                        canvas_id: self.id.clone(),
                                        shape_id,
                                    })
                                    .and_capture(),
                                )
                            }
                        }
                    }
                    keyboard::Key::Named(Named::ArrowDown | Named::ArrowRight) => {
                        let next = match state.focused_index {
                            None => 0,
                            Some(idx) => (idx + 1) % count,
                        };
                        state.focused_index = Some(next);
                        let shape_id = self.interactive_shapes[next].id.clone();
                        Some(
                            iced::widget::Action::publish(Message::CanvasShapeFocused {
                                canvas_id: self.id.clone(),
                                shape_id,
                            })
                            .and_capture(),
                        )
                    }
                    keyboard::Key::Named(Named::ArrowUp | Named::ArrowLeft) => {
                        let prev = match state.focused_index {
                            None => count - 1,
                            Some(0) => count - 1,
                            Some(idx) => idx - 1,
                        };
                        state.focused_index = Some(prev);
                        let shape_id = self.interactive_shapes[prev].id.clone();
                        Some(
                            iced::widget::Action::publish(Message::CanvasShapeFocused {
                                canvas_id: self.id.clone(),
                                shape_id,
                            })
                            .and_capture(),
                        )
                    }
                    keyboard::Key::Named(Named::Enter | Named::Space) => {
                        if let Some(idx) = state.focused_index {
                            let shape = &self.interactive_shapes[idx];
                            if shape.on_click {
                                let center = hit_region_center(&shape.hit_region);
                                let msg = Message::CanvasShapeClick {
                                    canvas_id: self.id.clone(),
                                    shape_id: shape.id.clone(),
                                    x: center.x,
                                    y: center.y,
                                    button: "keyboard".to_string(),
                                };
                                Some(iced::widget::Action::publish(msg).and_capture())
                            } else {
                                // Shape is focusable but not clickable
                                // (e.g., draggable-only). Consume the key
                                // but don't emit an event.
                                Some(iced::widget::Action::capture())
                            }
                        } else {
                            None
                        }
                    }
                    keyboard::Key::Named(Named::Escape) => {
                        if state.focused_index.is_some() {
                            state.focused_index = None;
                            // Capture so Escape doesn't propagate to parent
                            // widgets. Return None would let iced unfocus
                            // the canvas, which is correct when nothing
                            // internal was focused. But when we HAD internal
                            // focus, Escape should only clear internal focus.
                            Some(iced::widget::Action::capture())
                        } else {
                            // Nothing internally focused -- let Escape
                            // propagate (iced will unfocus the canvas).
                            None
                        }
                    }
                    keyboard::Key::Named(Named::Home) => {
                        state.focused_index = Some(0);
                        let shape_id = self.interactive_shapes[0].id.clone();
                        Some(
                            iced::widget::Action::publish(Message::CanvasShapeFocused {
                                canvas_id: self.id.clone(),
                                shape_id,
                            })
                            .and_capture(),
                        )
                    }
                    keyboard::Key::Named(Named::End) => {
                        let last = count - 1;
                        state.focused_index = Some(last);
                        let shape_id = self.interactive_shapes[last].id.clone();
                        Some(
                            iced::widget::Action::publish(Message::CanvasShapeFocused {
                                canvas_id: self.id.clone(),
                                shape_id,
                            })
                            .and_capture(),
                        )
                    }
                    keyboard::Key::Named(Named::PageDown) => {
                        let page_size = 10.min(count);
                        let idx = state.focused_index.unwrap_or(0);
                        let new_idx = (idx + page_size).min(count - 1);
                        state.focused_index = Some(new_idx);
                        let shape_id = self.interactive_shapes[new_idx].id.clone();
                        Some(
                            iced::widget::Action::publish(Message::CanvasShapeFocused {
                                canvas_id: self.id.clone(),
                                shape_id,
                            })
                            .and_capture(),
                        )
                    }
                    keyboard::Key::Named(Named::PageUp) => {
                        let page_size = 10.min(count);
                        let idx = state.focused_index.unwrap_or(0);
                        let new_idx = idx.saturating_sub(page_size);
                        state.focused_index = Some(new_idx);
                        let shape_id = self.interactive_shapes[new_idx].id.clone();
                        Some(
                            iced::widget::Action::publish(Message::CanvasShapeFocused {
                                canvas_id: self.id.clone(),
                                shape_id,
                            })
                            .and_capture(),
                        )
                    }
                    _ => None,
                }
            }

            _ => None,
        }
    }

    fn draw(
        &self,
        state: &CanvasState,
        renderer: &iced::Renderer,
        theme: &iced::Theme,
        bounds: iced::Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut geometries = Vec::new();

        // Background fill -- cheap single rect, not cached.
        if let Some(bg) = self.background {
            let mut frame = canvas::Frame::new(renderer, bounds.size());
            frame.fill_rectangle(Point::ORIGIN, bounds.size(), bg);
            geometries.push(frame.into_geometry());
        }

        // Determine which layers need fresh drawing due to active interaction.
        let active_layer = self.layer_with_active_interaction(state);

        // Draw each layer, using its cache when available.
        let images = self.images;
        for (layer_name, shapes) in &self.layers {
            let shape_refs: Vec<&Value> = shapes.iter().collect();
            let force_redraw = active_layer.as_deref() == Some(layer_name.as_str());

            let geom = if !force_redraw {
                if let Some((_hash, cache)) = self.caches.and_then(|c| c.get(layer_name)) {
                    cache.draw(renderer, bounds.size(), |frame| {
                        draw_canvas_shapes(frame, &shape_refs, images);
                    })
                } else {
                    let mut frame = canvas::Frame::new(renderer, bounds.size());
                    draw_canvas_shapes(&mut frame, &shape_refs, images);
                    frame.into_geometry()
                }
            } else {
                // Layer has active hover/pressed interaction -- clear cache
                // and draw fresh with style overrides applied.
                if let Some((_hash, cache)) = self.caches.and_then(|c| c.get(layer_name)) {
                    cache.clear();
                }
                let mut frame = canvas::Frame::new(renderer, bounds.size());
                self.draw_shapes_with_overrides(&mut frame, &shape_refs, state, images);
                frame.into_geometry()
            };
            geometries.push(geom);
        }

        // Tooltip overlay (uncached, drawn on top of all layers).
        if let Some(ref tooltip) = self.active_tooltip(state)
            && let Some(pos) = state.cursor_position
        {
            let mut frame = canvas::Frame::new(renderer, bounds.size());
            draw_tooltip(&mut frame, tooltip, pos, bounds.size(), theme);
            geometries.push(frame.into_geometry());
        }

        // Focus ring overlay (uncached, drawn on top of everything).
        if let Some(idx) = state.focused_index
            && idx < self.interactive_shapes.len()
        {
            let shape = &self.interactive_shapes[idx];
            let rect = hit_region_to_rect(&shape.hit_region);
            let mut frame = canvas::Frame::new(renderer, bounds.size());
            let ring_path = canvas::Path::rounded_rectangle(
                Point::new(rect.x - 2.0, rect.y - 2.0),
                Size::new(rect.width + 4.0, rect.height + 4.0),
                iced::border::Radius::from(3.0),
            );
            let focus_color = theme.palette().primary.base.color;
            frame.stroke(
                &ring_path,
                canvas::Stroke::default()
                    .with_color(focus_color)
                    .with_width(2.0),
            );
            geometries.push(frame.into_geometry());
        }

        geometries
    }

    fn mouse_interaction(
        &self,
        state: &CanvasState,
        _bounds: iced::Rectangle,
        _cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        // Dragging overrides everything.
        if state.dragging.is_some() {
            return mouse::Interaction::Grabbing;
        }
        // Per-shape cursor.
        if let Some(ref hovered_id) = state.hovered_shape
            && let Some(shape) = self.interactive_shapes.iter().find(|s| &s.id == hovered_id)
        {
            if let Some(ref cursor_name) = shape.cursor {
                return parse_cursor_interaction(cursor_name);
            }
            // Default cursor for interactive shapes without explicit cursor.
            return mouse::Interaction::Pointer;
        }
        // Fallback to canvas-level cursor.
        if self.is_interactive() {
            mouse::Interaction::Crosshair
        } else {
            mouse::Interaction::default()
        }
    }

    fn is_focusable(&self, _state: &CanvasState) -> bool {
        !self.interactive_shapes.is_empty()
    }

    fn operate_accessible(
        &self,
        _state: &CanvasState,
        canvas_bounds: iced::Rectangle,
        operation: &mut dyn iced::advanced::widget::Operation,
    ) {
        let mut seen_ids = std::collections::HashSet::new();
        for shape in self.interactive_shapes {
            let a11y = match &shape.a11y {
                Some(a) => a,
                None => continue,
            };
            // Deduplicate by ID (composites may share IDs).
            if !seen_ids.insert(&shape.id) {
                continue;
            }
            let shape_rect = hit_region_to_rect(&shape.hit_region);
            let shape_bounds = Rectangle {
                x: canvas_bounds.x + shape_rect.x,
                y: canvas_bounds.y + shape_rect.y,
                width: shape_rect.width,
                height: shape_rect.height,
            };
            operation.accessible(None, shape_bounds, &a11y.to_accessible());
        }
    }
}

/// Convert a HitRegion to a bounding Rectangle for accessibility.
fn hit_region_to_rect(region: &HitRegion) -> Rectangle {
    match *region {
        HitRegion::Rect { x, y, w, h } => Rectangle {
            x,
            y,
            width: w,
            height: h,
        },
        HitRegion::Circle { cx, cy, r } => Rectangle {
            x: cx - r,
            y: cy - r,
            width: r * 2.0,
            height: r * 2.0,
        },
        HitRegion::Line {
            x1,
            y1,
            x2,
            y2,
            half_width,
        } => {
            let min_x = x1.min(x2) - half_width;
            let min_y = y1.min(y2) - half_width;
            let max_x = x1.max(x2) + half_width;
            let max_y = y1.max(y2) + half_width;
            Rectangle {
                x: min_x,
                y: min_y,
                width: max_x - min_x,
                height: max_y - min_y,
            }
        }
    }
}

/// Compute the center point of a hit region.
fn hit_region_center(region: &HitRegion) -> Point {
    match *region {
        HitRegion::Rect { x, y, w, h } => Point::new(x + w / 2.0, y + h / 2.0),
        HitRegion::Circle { cx, cy, .. } => Point::new(cx, cy),
        HitRegion::Line { x1, y1, x2, y2, .. } => Point::new((x1 + x2) / 2.0, (y1 + y2) / 2.0),
    }
}

/// Serialize a mouse button for canvas events.
fn serialize_mouse_button_for_canvas(button: &mouse::Button) -> String {
    match button {
        mouse::Button::Left => "left".to_string(),
        mouse::Button::Right => "right".to_string(),
        mouse::Button::Middle => "middle".to_string(),
        mouse::Button::Back => "back".to_string(),
        mouse::Button::Forward => "forward".to_string(),
        mouse::Button::Other(n) => format!("other_{n}"),
    }
}

pub(crate) fn render_canvas<'a>(node: &'a TreeNode, ctx: RenderCtx<'a>) -> Element<'a, Message> {
    let props = node.props.as_object();
    let width = prop_length(props, "width", Length::Fill);
    let height = prop_length(props, "height", Length::Fixed(200.0));

    // Build sorted layer data directly from props, avoiding the
    // serialize-then-deserialize round trip that canvas_layer_map would do.
    let layers: Vec<(String, Vec<Value>)> = canvas_layers_from_props(props);

    let node_caches = ctx.caches.canvas_caches.get(&node.id);

    let background = props
        .and_then(|p| p.get("background"))
        .and_then(parse_color);

    let on_press = prop_bool_default(props, "on_press", false);
    let on_release = prop_bool_default(props, "on_release", false);
    let on_move = prop_bool_default(props, "on_move", false);
    let on_scroll = prop_bool_default(props, "on_scroll", false);
    // "interactive" is a convenience flag that enables all event handlers.
    let interactive = prop_bool_default(props, "interactive", false);

    let interactive_shapes = ctx
        .caches
        .canvas_interactions
        .get(&node.id)
        .map(|v| v.as_slice())
        .unwrap_or(&[]);
    let has_interactive_shapes = !interactive_shapes.is_empty();

    let mut c = iced::widget::canvas(CanvasProgram {
        layers,
        caches: node_caches,
        background,
        id: node.id.clone(),
        on_press: on_press || interactive || has_interactive_shapes,
        on_release: on_release || interactive || has_interactive_shapes,
        on_move: on_move || interactive || has_interactive_shapes,
        on_scroll: on_scroll || interactive,
        images: ctx.images,
        interactive_shapes,
    })
    .width(width)
    .height(height);

    if let Some(alt) = prop_str(props, "alt") {
        c = c.alt(alt);
    }
    if let Some(desc) = prop_str(props, "description") {
        c = c.description(desc);
    }

    c.into()
}

/// Parse an f32 from a JSON value by key, defaulting to 0.
pub(crate) fn json_f32(val: &Value, key: &str) -> f32 {
    val.get(key)
        .and_then(|v| v.as_f64())
        .map(|v| v as f32)
        .unwrap_or(0.0)
}

/// Parse a Color from a JSON "fill" field. Accepts "#rrggbb" hex strings;
/// defaults to white if missing or unparseable.
pub(crate) fn json_color(val: &Value, key: &str) -> Color {
    val.get(key).and_then(parse_color).unwrap_or(Color::WHITE)
}

// ---------------------------------------------------------------------------
// Cache ensure function
// ---------------------------------------------------------------------------

/// Recursively collect interactive shapes from a shape array, descending
/// into groups. `offset_x`/`offset_y` accumulate group x/y translations
/// so nested shapes' hit regions are in canvas-space coordinates.
fn collect_interactive_shapes(
    shapes: &[Value],
    layer_name: &str,
    offset_x: f32,
    offset_y: f32,
    out: &mut Vec<InteractiveShape>,
) {
    for shape in shapes {
        if let Some(mut ishape) = parse_interactive_shape(shape, layer_name) {
            // Apply accumulated group offset to the hit region.
            if offset_x != 0.0 || offset_y != 0.0 {
                ishape.hit_region = offset_hit_region(&ishape.hit_region, offset_x, offset_y);
            }
            out.push(ishape);
        }
        // Recurse into group children to find nested interactive shapes.
        let is_group = shape
            .get("type")
            .and_then(|v| v.as_str())
            .is_some_and(|t| t == "group");
        if is_group && let Some(children) = shape.get("children").and_then(|v| v.as_array()) {
            let gx = json_f32(shape, "x");
            let gy = json_f32(shape, "y");
            collect_interactive_shapes(children, layer_name, offset_x + gx, offset_y + gy, out);
        }
    }
}

/// Translate a hit region by the given offset.
fn offset_hit_region(region: &HitRegion, dx: f32, dy: f32) -> HitRegion {
    match *region {
        HitRegion::Rect { x, y, w, h } => HitRegion::Rect {
            x: x + dx,
            y: y + dy,
            w,
            h,
        },
        HitRegion::Circle { cx, cy, r } => HitRegion::Circle {
            cx: cx + dx,
            cy: cy + dy,
            r,
        },
        HitRegion::Line {
            x1,
            y1,
            x2,
            y2,
            half_width,
        } => HitRegion::Line {
            x1: x1 + dx,
            y1: y1 + dy,
            x2: x2 + dx,
            y2: y2 + dy,
            half_width,
        },
    }
}

pub(crate) fn ensure_canvas_cache(node: &crate::protocol::TreeNode, caches: &mut WidgetCaches) {
    let props = node.props.as_object();
    // Build layer map: either from "layers" (object) or "shapes" (array -> single layer).
    let layer_map = canvas_layer_map(props);
    let node_caches = caches.canvas_caches.entry(node.id.clone()).or_default();

    // Parse interactive shapes from all layers, recursing into groups.
    let mut interactive_shapes = Vec::new();
    for (layer_name, shapes_val) in &layer_map {
        if let Some(shapes_arr) = shapes_val.as_array() {
            collect_interactive_shapes(shapes_arr, layer_name, 0.0, 0.0, &mut interactive_shapes);
        }
    }
    caches
        .canvas_interactions
        .insert(node.id.clone(), interactive_shapes);

    // Update or create caches for each layer.
    for (layer_name, shapes_val) in &layer_map {
        let hash = {
            let mut hasher = DefaultHasher::new();
            hash_json_value(shapes_val, &mut hasher);
            hasher.finish()
        };
        match node_caches.get_mut(layer_name) {
            Some((existing_hash, cache)) => {
                if *existing_hash != hash {
                    cache.clear();
                    // Update just the hash, keep the same cache object.
                    *existing_hash = hash;
                }
            }
            None => {
                node_caches.insert(layer_name.clone(), (hash, canvas::Cache::new()));
            }
        }
    }

    // Remove stale layers that are no longer in the tree.
    node_caches.retain(|name, _| layer_map.contains_key(name));
}

#[cfg(test)]
mod tests {
    use super::super::caches::{canvas_layer_map, hash_str};
    use super::*;
    use serde_json::json;

    /// Helper: build a Props from a json! value. The value must be an object.
    fn make_props(v: &Value) -> Props<'_> {
        v.as_object()
    }

    #[test]
    fn canvas_layer_map_from_layers() {
        let v = json!({
            "layers": {
                "background": [{"type": "rect", "width": 100}],
                "foreground": [{"type": "circle", "radius": 50}]
            }
        });
        let props = make_props(&v);
        let result = canvas_layer_map(props);
        assert_eq!(result.len(), 2);
        assert!(result.contains_key("background"));
        assert!(result.contains_key("foreground"));
        // Values are references to each layer's shapes array.
        let bg = result.get("background").unwrap();
        assert!(bg.is_array());
        assert_eq!(bg.as_array().unwrap().len(), 1);
    }

    #[test]
    fn canvas_layer_map_from_shapes() {
        // Legacy "shapes" key wraps in a "default" layer.
        let v = json!({
            "shapes": [{"type": "line", "x1": 0, "y1": 0, "x2": 100, "y2": 100}]
        });
        let props = make_props(&v);
        let result = canvas_layer_map(props);
        assert_eq!(result.len(), 1);
        assert!(result.contains_key("default"));
    }

    #[test]
    fn canvas_hash_changes() {
        let hash_a = hash_str("[{\"type\":\"rect\"}]");
        let hash_b = hash_str("[{\"type\":\"circle\"}]");
        let hash_a2 = hash_str("[{\"type\":\"rect\"}]");

        // Same input produces same hash.
        assert_eq!(hash_a, hash_a2);
        // Different input produces different hash.
        assert_ne!(hash_a, hash_b);
    }

    #[test]
    fn canvas_layer_sort_order() {
        let v = json!({
            "layers": {
                "charlie": [{"type": "rect"}],
                "alpha": [{"type": "circle"}],
                "bravo": [{"type": "line"}]
            }
        });
        let props = make_props(&v);
        let result = canvas_layer_map(props);
        let keys: Vec<&String> = result.keys().collect();
        assert_eq!(keys, vec!["alpha", "bravo", "charlie"]);
    }

    #[test]
    fn canvas_path_commands_basic() {
        let shape = json!({
            "type": "path",
            "commands": [
                ["move_to", 10, 20],
                ["line_to", 30, 40],
                "close"
            ]
        });
        assert_eq!(shape.get("type").and_then(|v| v.as_str()), Some("path"));
        let commands = shape.get("commands").and_then(|v| v.as_array()).unwrap();
        assert_eq!(commands.len(), 3);
        // First command is an array starting with "move_to".
        let move_cmd = commands[0].as_array().unwrap();
        assert_eq!(move_cmd[0].as_str(), Some("move_to"));
        assert_eq!(move_cmd[1].as_f64(), Some(10.0));
        assert_eq!(move_cmd[2].as_f64(), Some(20.0));
        // Second command is an array starting with "line_to".
        let line_cmd = commands[1].as_array().unwrap();
        assert_eq!(line_cmd[0].as_str(), Some("line_to"));
        assert_eq!(line_cmd[1].as_f64(), Some(30.0));
        assert_eq!(line_cmd[2].as_f64(), Some(40.0));
        // Third command is the bare string "close".
        assert_eq!(commands[2].as_str(), Some("close"));
    }

    #[test]
    fn canvas_stroke_parse() {
        let stroke_val = json!({
            "color": "#ff0000",
            "width": 3.0,
            "cap": "round",
            "join": "bevel"
        });
        let stroke = parse_canvas_stroke(&stroke_val);
        assert_eq!(
            stroke.style,
            canvas::Style::Solid(Color::from_rgb8(255, 0, 0))
        );
        assert_eq!(stroke.width, 3.0);
        // LineCap and LineJoin don't impl PartialEq, so use Debug format.
        assert_eq!(format!("{:?}", stroke.line_cap), "Round");
        assert_eq!(format!("{:?}", stroke.line_join), "Bevel");
    }

    #[test]
    fn canvas_gradient_parse() {
        let fill_val = json!({
            "type": "linear",
            "start": [0.0, 0.0],
            "end": [100.0, 0.0],
            "stops": [
                [0.0, "#ff0000"],
                [1.0, "#0000ff"]
            ]
        });
        let shape = json!({"fill": fill_val.clone()});
        let fill = parse_canvas_fill(&fill_val, &shape);
        // The fill rule should be NonZero for gradient fills.
        assert_eq!(fill.rule, canvas::fill::Rule::NonZero);
        // The style should be a gradient, not a solid color.
        match &fill.style {
            canvas::Style::Gradient(canvas::Gradient::Linear(_)) => {}
            other => panic!("expected Gradient::Linear, got {other:?}"),
        }
    }

    #[test]
    fn canvas_fill_rule_defaults_to_non_zero() {
        let fill_val = json!("#ff0000");
        let shape = json!({"fill": "#ff0000"});
        let fill = parse_canvas_fill(&fill_val, &shape);
        assert_eq!(fill.rule, canvas::fill::Rule::NonZero);
    }

    #[test]
    fn canvas_fill_rule_even_odd() {
        let fill_val = json!("#00ff00");
        let shape = json!({"fill": "#00ff00", "fill_rule": "even_odd"});
        let fill = parse_canvas_fill(&fill_val, &shape);
        assert_eq!(fill.rule, canvas::fill::Rule::EvenOdd);
    }

    #[test]
    fn canvas_fill_rule_explicit_non_zero() {
        let fill_val = json!("#0000ff");
        let shape = json!({"fill": "#0000ff", "fill_rule": "non_zero"});
        let fill = parse_canvas_fill(&fill_val, &shape);
        assert_eq!(fill.rule, canvas::fill::Rule::NonZero);
    }

    #[test]
    fn collect_clipped_shapes_simple() {
        let shapes = [
            json!({"type": "rect", "x": 0, "y": 0, "w": 50, "h": 50}),
            json!({"type": "pop_clip"}),
        ];
        let refs: Vec<&Value> = shapes.iter().collect();
        let (end_idx, collected) = collect_clipped_shapes(&refs);
        assert_eq!(end_idx, 1); // pop_clip is at index 1
        assert_eq!(collected.len(), 1); // just the rect
        assert_eq!(
            collected[0].get("type").and_then(|v| v.as_str()),
            Some("rect")
        );
    }

    #[test]
    fn collect_clipped_shapes_nested() {
        let shapes = [
            json!({"type": "push_clip", "x": 10, "y": 10, "w": 50, "h": 50}),
            json!({"type": "rect", "x": 0, "y": 0, "w": 20, "h": 20}),
            json!({"type": "pop_clip"}),
            json!({"type": "circle", "x": 25, "y": 25, "r": 10}),
            json!({"type": "pop_clip"}),
        ];
        let refs: Vec<&Value> = shapes.iter().collect();
        let (end_idx, collected) = collect_clipped_shapes(&refs);
        // The outer pop_clip is at index 4
        assert_eq!(end_idx, 4);
        // Collected: push_clip, rect, pop_clip (inner), circle
        assert_eq!(collected.len(), 4);
    }

    #[test]
    fn collect_clipped_shapes_no_pop() {
        let shapes = [json!({"type": "rect", "x": 0, "y": 0, "w": 50, "h": 50})];
        let refs: Vec<&Value> = shapes.iter().collect();
        let (end_idx, collected) = collect_clipped_shapes(&refs);
        // No pop_clip found -- returns all shapes
        assert_eq!(end_idx, shapes.len());
        assert_eq!(collected.len(), 1);
    }

    // -- Text alignment tests --

    #[test]
    fn text_align_x_parses_left() {
        let v = json!("left");
        assert_eq!(format!("{:?}", parse_canvas_text_align_x(Some(&v))), "Left");
    }

    #[test]
    fn text_align_x_parses_center() {
        let v = json!("center");
        assert_eq!(
            format!("{:?}", parse_canvas_text_align_x(Some(&v))),
            "Center"
        );
    }

    #[test]
    fn text_align_x_parses_right() {
        let v = json!("right");
        assert_eq!(
            format!("{:?}", parse_canvas_text_align_x(Some(&v))),
            "Right"
        );
    }

    #[test]
    fn text_align_x_defaults_to_default() {
        assert_eq!(format!("{:?}", parse_canvas_text_align_x(None)), "Default");
    }

    #[test]
    fn text_align_y_parses_center() {
        let v = json!("center");
        assert_eq!(
            parse_canvas_text_align_y(Some(&v)),
            alignment::Vertical::Center
        );
    }

    #[test]
    fn text_align_y_parses_bottom() {
        let v = json!("bottom");
        assert_eq!(
            parse_canvas_text_align_y(Some(&v)),
            alignment::Vertical::Bottom
        );
    }

    #[test]
    fn text_align_y_defaults_to_top() {
        assert_eq!(parse_canvas_text_align_y(None), alignment::Vertical::Top);
    }

    // -- Opacity tests --

    #[test]
    fn opacity_applied_to_fill() {
        let shape = json!({"type": "rect", "fill": "#ff0000", "opacity": 0.5});
        let fill = apply_opacity_to_fill(&shape, parse_canvas_fill(&json!("#ff0000"), &shape));
        match fill.style {
            canvas::Style::Solid(c) => {
                assert!(
                    (c.a - 0.5).abs() < 0.001,
                    "expected alpha ~0.5, got {}",
                    c.a
                );
            }
            _ => panic!("expected solid fill"),
        }
    }

    #[test]
    fn opacity_applied_to_stroke() {
        let shape = json!({"type": "rect", "opacity": 0.25});
        let stroke_val = json!({"color": "#00ff00", "width": 2.0});
        let stroke = apply_opacity_to_stroke(&shape, parse_canvas_stroke(&stroke_val));
        match stroke.style {
            canvas::Style::Solid(c) => {
                assert!(
                    (c.a - 0.25).abs() < 0.001,
                    "expected alpha ~0.25, got {}",
                    c.a
                );
            }
            _ => panic!("expected solid stroke"),
        }
    }

    #[test]
    fn opacity_applied_to_color() {
        let shape = json!({"opacity": 0.75});
        let color = apply_opacity_to_color(&shape, Color::WHITE);
        assert!(
            (color.a - 0.75).abs() < 0.001,
            "expected alpha ~0.75, got {}",
            color.a
        );
    }

    #[test]
    fn no_opacity_leaves_alpha_unchanged() {
        let shape = json!({"type": "rect", "fill": "#ff0000"});
        let fill = apply_opacity_to_fill(&shape, parse_canvas_fill(&json!("#ff0000"), &shape));
        match fill.style {
            canvas::Style::Solid(c) => {
                assert!(
                    (c.a - 1.0).abs() < 0.001,
                    "expected alpha ~1.0, got {}",
                    c.a
                );
            }
            _ => panic!("expected solid fill"),
        }
    }

    // -- Hit testing --

    #[test]
    fn hit_test_rect_inside() {
        let region = HitRegion::Rect {
            x: 10.0,
            y: 20.0,
            w: 30.0,
            h: 40.0,
        };
        assert!(hit_test(Point::new(25.0, 40.0), &region));
    }

    #[test]
    fn hit_test_rect_outside() {
        let region = HitRegion::Rect {
            x: 10.0,
            y: 20.0,
            w: 30.0,
            h: 40.0,
        };
        assert!(!hit_test(Point::new(5.0, 40.0), &region));
    }

    #[test]
    fn hit_test_circle_inside() {
        let region = HitRegion::Circle {
            cx: 50.0,
            cy: 50.0,
            r: 20.0,
        };
        assert!(hit_test(Point::new(50.0, 50.0), &region));
        assert!(hit_test(Point::new(60.0, 50.0), &region));
    }

    #[test]
    fn hit_test_circle_outside() {
        let region = HitRegion::Circle {
            cx: 50.0,
            cy: 50.0,
            r: 20.0,
        };
        assert!(!hit_test(Point::new(80.0, 50.0), &region));
    }

    #[test]
    fn hit_test_line_near() {
        let region = HitRegion::Line {
            x1: 0.0,
            y1: 0.0,
            x2: 100.0,
            y2: 0.0,
            half_width: 5.0,
        };
        assert!(hit_test(Point::new(50.0, 3.0), &region));
        assert!(!hit_test(Point::new(50.0, 10.0), &region));
    }

    #[test]
    fn hit_test_line_endpoint() {
        let region = HitRegion::Line {
            x1: 10.0,
            y1: 10.0,
            x2: 10.0,
            y2: 10.0,
            half_width: 5.0,
        };
        // Degenerate line (zero length) -- treated as point.
        assert!(hit_test(Point::new(12.0, 10.0), &region));
        assert!(!hit_test(Point::new(20.0, 10.0), &region));
    }

    // -- Interactive shape parsing --

    #[test]
    fn parse_interactive_rect() {
        let shape = json!({
            "type": "rect",
            "x": 10, "y": 20, "w": 30, "h": 40,
            "fill": "#ff0000",
            "interactive": {
                "id": "bar-1",
                "on_click": true,
                "on_hover": true,
                "cursor": "pointer",
                "tooltip": "Bar 1: 200 units"
            }
        });
        let result = parse_interactive_shape(&shape, "default").unwrap();
        assert_eq!(result.id, "bar-1");
        assert!(result.on_click);
        assert!(result.on_hover);
        assert_eq!(result.cursor.as_deref(), Some("pointer"));
        assert_eq!(result.tooltip.as_deref(), Some("Bar 1: 200 units"));
        assert!(matches!(
            result.hit_region,
            HitRegion::Rect {
                x: _,
                y: _,
                w: _,
                h: _
            }
        ));
    }

    #[test]
    fn parse_interactive_circle() {
        let shape = json!({
            "type": "circle",
            "x": 50, "y": 50, "r": 20,
            "interactive": {
                "id": "dot-1",
                "on_click": true,
                "draggable": true,
                "drag_axis": "x"
            }
        });
        let result = parse_interactive_shape(&shape, "layer1").unwrap();
        assert_eq!(result.id, "dot-1");
        assert!(result.draggable);
        assert_eq!(result.drag_axis, DragAxis::X);
        assert!(matches!(result.hit_region, HitRegion::Circle { .. }));
    }

    #[test]
    fn parse_interactive_with_hit_rect() {
        let shape = json!({
            "type": "path",
            "commands": [["move_to", 0, 0], ["line_to", 100, 100]],
            "interactive": {
                "id": "path-1",
                "on_click": true,
                "hit_rect": {"x": 0, "y": 0, "w": 100, "h": 100}
            }
        });
        let result = parse_interactive_shape(&shape, "default").unwrap();
        assert_eq!(result.id, "path-1");
        assert!(matches!(result.hit_region, HitRegion::Rect { .. }));
    }

    #[test]
    fn parse_interactive_missing_id_returns_none() {
        let shape = json!({
            "type": "rect", "x": 0, "y": 0, "w": 10, "h": 10,
            "interactive": {"on_click": true}
        });
        assert!(parse_interactive_shape(&shape, "default").is_none());
    }

    #[test]
    fn parse_interactive_no_field_returns_none() {
        let shape = json!({"type": "rect", "x": 0, "y": 0, "w": 10, "h": 10});
        assert!(parse_interactive_shape(&shape, "default").is_none());
    }

    // -- Hit region to rect --

    #[test]
    fn hit_region_to_rect_circle() {
        let rect = hit_region_to_rect(&HitRegion::Circle {
            cx: 50.0,
            cy: 50.0,
            r: 20.0,
        });
        assert!((rect.x - 30.0).abs() < 0.01);
        assert!((rect.y - 30.0).abs() < 0.01);
        assert!((rect.width - 40.0).abs() < 0.01);
        assert!((rect.height - 40.0).abs() < 0.01);
    }

    // -- Style merging --

    #[test]
    fn merge_shape_style_overrides_fill() {
        let shape = json!({"type": "rect", "fill": "#ff0000", "stroke": {"color": "#000"}});
        let overrides = json!({"fill": "#00ff00"});
        let merged = merge_shape_style(&shape, &overrides);
        assert_eq!(merged["fill"], "#00ff00");
        // Non-overridden fields preserved.
        assert_eq!(merged["stroke"]["color"], "#000");
    }

    // -- Group shape tests --

    #[test]
    fn compute_hit_region_group_with_rect_children() {
        let shape = json!({
            "type": "group",
            "x": 50.0, "y": 100.0,
            "interactive": {"id": "grp1", "on_click": true},
            "children": [
                {"type": "rect", "x": 0, "y": 0, "w": 100, "h": 40},
                {"type": "rect", "x": 10, "y": 50, "w": 80, "h": 20}
            ]
        });
        let interactive = shape.get("interactive").unwrap().as_object().unwrap();
        let region = compute_hit_region(&shape, interactive).unwrap();
        // Bounding box of children: x=0..100, y=0..70, offset by group's x/y.
        match region {
            HitRegion::Rect { x, y, w, h } => {
                assert!((x - 50.0).abs() < 0.01);
                assert!((y - 100.0).abs() < 0.01);
                assert!((w - 100.0).abs() < 0.01);
                assert!((h - 70.0).abs() < 0.01);
            }
            other => panic!("expected Rect, got {other:?}"),
        }
    }

    #[test]
    fn compute_hit_region_group_with_mixed_children() {
        let shape = json!({
            "type": "group",
            "x": 10.0, "y": 20.0,
            "interactive": {"id": "grp2", "on_click": true},
            "children": [
                {"type": "rect", "x": 0, "y": 0, "w": 50, "h": 30},
                {"type": "circle", "x": 80, "y": 15, "r": 10}
            ]
        });
        let interactive = shape.get("interactive").unwrap().as_object().unwrap();
        let region = compute_hit_region(&shape, interactive).unwrap();
        // Rect: 0..50, 0..30; Circle: 70..90, 5..25
        // Union: 0..90, 0..30 (rect extends lower), offset by 10, 20
        match region {
            HitRegion::Rect { x, y, w, h } => {
                assert!((x - 10.0).abs() < 0.01);
                assert!((y - 20.0).abs() < 0.01);
                assert!((w - 90.0).abs() < 0.01);
                assert!((h - 30.0).abs() < 0.01);
            }
            other => panic!("expected Rect, got {other:?}"),
        }
    }

    #[test]
    fn compute_hit_region_group_no_children() {
        let shape = json!({
            "type": "group",
            "x": 0.0, "y": 0.0,
            "interactive": {"id": "empty", "on_click": true},
            "children": []
        });
        let interactive = shape.get("interactive").unwrap().as_object().unwrap();
        // No computable bounds from empty children.
        assert!(compute_hit_region(&shape, interactive).is_none());
    }

    #[test]
    fn parse_interactive_group() {
        let shape = json!({
            "type": "group",
            "x": 50, "y": 100,
            "interactive": {
                "id": "btn",
                "on_click": true,
                "on_hover": true,
                "cursor": "pointer",
                "a11y": {"role": "button", "label": "Save"}
            },
            "children": [
                {"type": "rect", "x": 0, "y": 0, "w": 100, "h": 40, "fill": "#3498db"},
                {"type": "text", "x": 30, "y": 25, "content": "Save", "fill": "#ccc"}
            ]
        });
        let result = parse_interactive_shape(&shape, "default").unwrap();
        assert_eq!(result.id, "btn");
        assert!(result.on_click);
        assert!(result.on_hover);
        assert_eq!(result.cursor.as_deref(), Some("pointer"));
        assert!(result.a11y.is_some());
        match result.hit_region {
            HitRegion::Rect { x, y, w, h } => {
                // Group offset (50, 100) + bounding box of children.
                // Rect child: (0,0,100,40). Text child: (30,9,68.4,25).
                // Union: (0, 0, 100, 40). With offset: (50, 100, 100, 40).
                assert!((x - 50.0).abs() < 0.01);
                assert!((y - 100.0).abs() < 0.01);
                assert!((w - 100.0).abs() < 0.01);
                assert!((h - 40.0).abs() < 0.01);
            }
            other => panic!("expected Rect, got {other:?}"),
        }
    }

    #[test]
    fn collect_interactive_shapes_recurses_into_groups() {
        let shapes = vec![
            json!({
                "type": "rect", "x": 0, "y": 0, "w": 10, "h": 10,
                "interactive": {"id": "top-rect", "on_click": true}
            }),
            json!({
                "type": "group", "x": 0, "y": 0,
                "interactive": {"id": "grp", "on_click": true},
                "children": [
                    {"type": "rect", "x": 0, "y": 0, "w": 50, "h": 50},
                    {
                        "type": "group", "x": 10, "y": 10,
                        "interactive": {"id": "nested-grp", "on_click": true},
                        "children": [
                            {"type": "circle", "x": 5, "y": 5, "r": 5}
                        ]
                    }
                ]
            }),
        ];
        let mut result = Vec::new();
        collect_interactive_shapes(&shapes, "default", 0.0, 0.0, &mut result);
        let ids: Vec<&str> = result.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.contains(&"top-rect"));
        assert!(ids.contains(&"grp"));
        assert!(ids.contains(&"nested-grp"));
    }
}
