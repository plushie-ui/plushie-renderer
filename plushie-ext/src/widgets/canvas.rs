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
// Interactive elements -- hit testing and interaction state
// ---------------------------------------------------------------------------

/// Geometric region for hit testing an interactive element.
///
/// Currently only `Rect` is constructed (groups auto-infer bounding boxes).
/// `Circle` and `Line` are retained for `hit_test` dispatch, focus ring
/// geometry, and future `hit_rect` extensions that support non-rectangular
/// regions.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Circle/Line not yet constructed but used in match arms
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

/// 2D affine transform matrix for mapping between coordinate spaces.
///
/// Stored as a 2x3 matrix `[a, b, c, d, tx, ty]` representing:
/// ```text
/// | a  b  tx |
/// | c  d  ty |
/// | 0  0   1 |
/// ```
///
/// Transforms are composed by multiplication. Points are transformed as:
/// ```text
/// x' = a*x + b*y + tx
/// y' = c*x + d*y + ty
/// ```
///
/// Used to map cursor positions from canvas space into an element's local
/// coordinate space for hit testing. Each [`InteractiveElement`] stores
/// the accumulated matrix from all ancestor group transforms and its
/// inverse (precomputed for efficient per-frame hit testing).
#[derive(Debug, Clone, Copy)]
pub(crate) struct TransformMatrix {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub tx: f32,
    pub ty: f32,
}

impl TransformMatrix {
    /// The identity matrix (no transformation).
    pub fn identity() -> Self {
        Self {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            tx: 0.0,
            ty: 0.0,
        }
    }

    /// Append a translation to this matrix.
    pub fn translate(self, x: f32, y: f32) -> Self {
        Self {
            a: self.a,
            b: self.b,
            c: self.c,
            d: self.d,
            tx: self.a * x + self.b * y + self.tx,
            ty: self.c * x + self.d * y + self.ty,
        }
    }

    /// Append a rotation (in radians) to this matrix.
    pub fn rotate(self, angle: f32) -> Self {
        let cos = angle.cos();
        let sin = angle.sin();
        Self {
            a: self.a * cos + self.b * sin,
            b: self.b * cos - self.a * sin,
            c: self.c * cos + self.d * sin,
            d: self.d * cos - self.c * sin,
            tx: self.tx,
            ty: self.ty,
        }
    }

    /// Append a non-uniform scale to this matrix.
    pub fn scale(self, sx: f32, sy: f32) -> Self {
        Self {
            a: self.a * sx,
            b: self.b * sy,
            c: self.c * sx,
            d: self.d * sy,
            tx: self.tx,
            ty: self.ty,
        }
    }

    /// Transform a point from the source coordinate space to the
    /// destination coordinate space.
    pub fn transform_point(&self, x: f32, y: f32) -> (f32, f32) {
        (
            self.a * x + self.b * y + self.tx,
            self.c * x + self.d * y + self.ty,
        )
    }

    /// Compose this matrix with another: `self * other`.
    ///
    /// The result maps points through `other` first, then through `self`.
    /// This matches the "append" convention: `parent.compose(&local)`
    /// produces a matrix that applies the local transform in the parent's
    /// coordinate space.
    pub fn compose(&self, other: &Self) -> Self {
        Self {
            a: self.a * other.a + self.b * other.c,
            b: self.a * other.b + self.b * other.d,
            c: self.c * other.a + self.d * other.c,
            d: self.c * other.b + self.d * other.d,
            tx: self.a * other.tx + self.b * other.ty + self.tx,
            ty: self.c * other.tx + self.d * other.ty + self.ty,
        }
    }

    /// Compute the inverse matrix, or `None` if the matrix is singular
    /// (determinant is zero, meaning the transform collapses a dimension).
    pub fn inverse(&self) -> Option<Self> {
        let det = self.a * self.d - self.b * self.c;
        if det.abs() < 1e-10 {
            return None;
        }
        let inv_det = 1.0 / det;
        Some(Self {
            a: self.d * inv_det,
            b: -self.b * inv_det,
            c: -self.c * inv_det,
            d: self.a * inv_det,
            tx: (self.b * self.ty - self.d * self.tx) * inv_det,
            ty: (self.c * self.tx - self.a * self.ty) * inv_det,
        })
    }

    /// Decompose this matrix into translation, rotation, and scale.
    ///
    /// Returns `(tx, ty, angle, sx, sy)` where the matrix can be
    /// reconstructed as `translate(tx, ty) * rotate(angle) * scale(sx, sy)`.
    ///
    /// This decomposition is exact for matrices built from translate,
    /// rotate, and scale operations (no shear). For matrices with shear,
    /// the result is an approximation.
    pub fn decompose(&self) -> (f32, f32, f32, f32, f32) {
        let tx = self.tx;
        let ty = self.ty;
        let angle = self.c.atan2(self.a);
        let sx = (self.a * self.a + self.c * self.c).sqrt();
        // Use determinant / sx to get sy with correct sign (handles reflection).
        let det = self.a * self.d - self.b * self.c;
        let sy = if sx.abs() > 1e-10 { det / sx } else { 0.0 };
        (tx, ty, angle, sx, sy)
    }

    /// Apply this matrix's transform to an iced canvas [`Frame`].
    ///
    /// Decomposes the matrix into translate + rotate + scale and applies
    /// them in order. The caller must call `frame.push_transform()` before
    /// and `frame.pop_transform()` after.
    ///
    /// [`Frame`]: iced::widget::canvas::Frame
    pub fn apply_to_frame(&self, frame: &mut canvas::Frame) {
        let (tx, ty, angle, sx, sy) = self.decompose();
        frame.translate(Vector::new(tx, ty));
        if angle.abs() > 1e-6 {
            frame.rotate(Radians(angle));
        }
        if (sx - 1.0).abs() > 1e-6 || (sy - 1.0).abs() > 1e-6 {
            frame.scale_nonuniform(Vector::new(sx, sy));
        }
    }

    /// Build a matrix from a group's `"transforms"` JSON array.
    ///
    /// Applies each transform entry in order: translate, rotate, scale.
    /// Returns the composed matrix.
    pub fn from_transforms(transforms: &[Value]) -> Self {
        let mut m = Self::identity();
        for t in transforms {
            let t_type = t.get("type").and_then(|v| v.as_str()).unwrap_or("");
            match t_type {
                "translate" => {
                    let x = t.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                    let y = t.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                    m = m.translate(x, y);
                }
                "rotate" => {
                    let angle = t.get("angle").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                    m = m.rotate(angle);
                }
                "scale" => {
                    if let Some(factor) = t.get("factor").and_then(|v| v.as_f64()) {
                        let f = factor as f32;
                        m = m.scale(f, f);
                    } else {
                        let sx = t.get("x").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;
                        let sy = t.get("y").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;
                        m = m.scale(sx, sy);
                    }
                }
                _ => {}
            }
        }
        m
    }
}

/// Arrow key navigation mode for canvas interactive elements.
///
/// Controls how arrow keys behave at the boundaries of the element list.
/// Set via the `"arrow_mode"` prop on the canvas widget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ArrowMode {
    /// Arrows wrap around at boundaries (last -> first, first -> last).
    /// Always captures the event. This is the standard roving tabindex
    /// pattern for composite widgets like toolbars and radio groups.
    #[default]
    Wrap,
    /// Arrows stop at first/last element (no wrapping). Captures the event.
    Clamp,
    /// Arrows navigate but return `None` at boundaries, letting the event
    /// propagate to parent widgets. Useful for canvases inside scrollable
    /// containers where arrows should scroll at the edges.
    Linear,
    /// Arrows are not handled by the canvas at all. Elements are only
    /// navigable via Tab. Useful when arrow keys have app-specific meaning.
    None,
}

impl ArrowMode {
    fn from_str(s: &str) -> Self {
        match s {
            "wrap" => Self::Wrap,
            "clamp" => Self::Clamp,
            "linear" => Self::Linear,
            "none" => Self::None,
            _ => {
                log::warn!("canvas: unknown arrow_mode '{s}', defaulting to 'wrap'");
                Self::Wrap
            }
        }
    }
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

/// Parsed interactive configuration for a canvas element.
///
/// An "element" is a group with an `id` field. Interactive fields
/// (`on_click`, `on_hover`, `a11y`, etc.) live at the top level of
/// the group JSON, not in a nested `"interactive"` sub-object.
///
/// Extracted during `ensure_canvas_cache` and stored in `WidgetCaches`
/// so `update()` can hit-test without re-parsing JSON every frame.
#[derive(Debug, Clone)]
pub(crate) struct InteractiveElement {
    /// Unique ID for this element (from the group's `"id"` field).
    pub id: String,
    /// Which layer this element belongs to.
    pub layer: String,
    /// Geometric bounds for hit testing (in local group coordinates).
    /// Use [`inverse_transform`](Self::inverse_transform) to map cursor
    /// positions from canvas space into this local space.
    pub hit_region: HitRegion,
    /// Accumulated transform from canvas origin to this element's local
    /// coordinate space. Composed from all ancestor group transforms.
    pub transform: TransformMatrix,
    /// Precomputed inverse of [`transform`](Self::transform). Used to
    /// map cursor positions from canvas space to local space for hit
    /// testing. `None` if the transform is singular (degenerate).
    pub inverse_transform: Option<TransformMatrix>,
    /// Optional clip rectangle in canvas space. If set, the element is
    /// only hittable when the cursor is inside this rect. Derived from
    /// ancestor group `"clip"` fields, transformed to canvas space.
    pub clip_rect: Option<(f32, f32, f32, f32)>,
    pub on_click: bool,
    pub on_hover: bool,
    pub draggable: bool,
    pub drag_axis: DragAxis,
    pub drag_bounds: Option<DragBounds>,
    /// Cursor to show when hovering (e.g. "pointer", "grab").
    pub cursor: Option<String>,
    /// Whether this element has hover/pressed/focus style overrides.
    pub has_hover_style: bool,
    pub has_pressed_style: bool,
    pub has_focus_style: bool,
    /// Whether to show the default focus ring when this element is focused.
    /// Defaults to `true`. Set to `false` when using `focus_style` instead.
    pub show_focus_ring: bool,
    /// Corner radius for the focus ring. Defaults to `inflate + 1.0` (~3px).
    /// Set to `h/2 + inflate` for pill shapes, `r + inflate` for circles.
    /// Only used when `show_focus_ring` is `true` and the hit region is Rect.
    pub focus_ring_radius: Option<f32>,
    /// Whether this group acts as a Tab stop for two-level navigation.
    /// When true, Tab navigates between focusable groups, and arrows
    /// navigate between elements within the focused group.
    pub focusable: bool,
    /// ID of the parent focusable group, if this element is a child
    /// within a focusable group. `None` for top-level elements and
    /// for focusable groups themselves.
    ///
    /// Used for two-level navigation: Tab moves between elements with
    /// `parent_group == None`, arrows move within elements sharing the
    /// same `parent_group`.
    pub parent_group: Option<String>,
    /// Tooltip text to show on hover.
    pub tooltip: Option<String>,
    /// Accessibility overrides for this element. Parsed from the `a11y`
    /// field on the group using the same [`A11yOverrides`] struct that
    /// all other widgets use -- same fields, same parsing, same validation.
    ///
    /// [`A11yOverrides`]: super::a11y::A11yOverrides
    pub a11y: Option<super::a11y::A11yOverrides>,
}

/// Active drag state tracked in `CanvasState`.
#[derive(Debug, Clone)]
struct DragState {
    element_id: String,
    last: Point,
}

/// Test whether a point is inside a hit region.
///
/// Uses a small epsilon (0.5px) for boundary comparisons to handle
/// floating-point imprecision from transform matrix inversion. Without
/// this, points exactly on the boundary of a rotated element would
/// sometimes miss due to rounding errors.
fn hit_test(point: Point, region: &HitRegion) -> bool {
    /// Half-pixel tolerance for boundary comparisons after transform.
    const EPS: f32 = 0.5;

    match *region {
        HitRegion::Rect { x, y, w, h } => {
            point.x >= x - EPS
                && point.x <= x + w + EPS
                && point.y >= y - EPS
                && point.y <= y + h + EPS
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

/// Find the topmost interactive element under the given point.
///
/// Elements are tested in reverse order (last in list = topmost drawn = tested first).
/// The cursor point (in canvas space) is transformed into each element's
/// local coordinate space using the precomputed inverse transform matrix
/// before testing against the local-coordinate hit region.
///
/// Elements with a `clip_rect` are only hittable when the cursor is
/// inside the clip rectangle (tested in canvas space, before transform).
fn find_hit_element(point: Point, elements: &[InteractiveElement]) -> Option<&InteractiveElement> {
    elements.iter().rev().find(|e| {
        if !(e.on_click || e.on_hover || e.draggable) {
            return false;
        }
        // Clip test in canvas space.
        if let Some((cx, cy, cw, ch)) = e.clip_rect {
            if point.x < cx || point.x > cx + cw || point.y < cy || point.y > cy + ch {
                return false;
            }
        }
        // Transform cursor to element's local space.
        let local = match &e.inverse_transform {
            Some(inv) => {
                let (lx, ly) = inv.transform_point(point.x, point.y);
                Point::new(lx, ly)
            }
            // Singular transform -- element can't be hit.
            None => return false,
        };
        hit_test(local, &e.hit_region)
    })
}

/// Parse an [`InteractiveElement`] from a group's top-level JSON fields.
///
/// A group is interactive when it has an `"id"` field. All interactive
/// properties (`on_click`, `a11y`, `hover_style`, etc.) live at the
/// group's top level, not in a nested `"interactive"` sub-object.
///
/// Returns `None` if the group has no `"id"` or is not a group type.
fn parse_interactive_element(group: &Value, layer_name: &str) -> Option<InteractiveElement> {
    // Only groups can be interactive elements.
    let shape_type = group.get("type").and_then(|v| v.as_str())?;
    if shape_type != "group" {
        return None;
    }

    let id = group.get("id").and_then(|v| v.as_str())?.to_string();
    if id.is_empty() {
        return None;
    }

    // Validate known fields -- warn on typos like "on_clck" or "focussable".
    const KNOWN_GROUP_FIELDS: &[&str] = &[
        "type", "children", "transforms", "clip",
        // Interactive
        "id", "on_click", "on_hover", "cursor", "draggable", "drag_axis",
        "drag_bounds", "tooltip", "hit_rect", "hover_style", "pressed_style",
        "focus_style", "show_focus_ring", "focus_ring_radius", "focusable",
        // Accessibility
        "a11y",
    ];
    if let Some(obj) = group.as_object() {
        for key in obj.keys() {
            if !KNOWN_GROUP_FIELDS.contains(&key.as_str()) {
                log::warn!(
                    "canvas element '{id}': unknown field '{key}'"
                );
            }
        }
    }

    // Warn on common mistakes.
    let draggable = group
        .get("draggable")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !draggable && group.get("drag_bounds").is_some() {
        log::warn!("canvas element '{id}': drag_bounds set without draggable: true");
    }
    if !draggable && group.get("drag_axis").is_some() {
        log::warn!("canvas element '{id}': drag_axis set without draggable: true");
    }

    let hit_region = compute_hit_region(group)?;

    let drag_axis = match group
        .get("drag_axis")
        .and_then(|v| v.as_str())
        .unwrap_or("both")
    {
        "x" => DragAxis::X,
        "y" => DragAxis::Y,
        _ => DragAxis::Both,
    };

    let drag_bounds = group.get("drag_bounds").and_then(|v| {
        let obj = v.as_object()?;
        let get = |key: &str| -> Option<f32> {
            let val = obj.get(key).and_then(|v| v.as_f64()).map(|v| v as f32);
            if val.is_none() {
                log::warn!("canvas element '{id}': drag_bounds missing '{key}'");
            }
            val
        };
        let min_x = get("min_x")?;
        let max_x = get("max_x")?;
        let min_y = get("min_y")?;
        let max_y = get("max_y")?;
        Some(DragBounds {
            min_x: min_x.min(max_x),
            max_x: min_x.max(max_x),
            min_y: min_y.min(max_y),
            max_y: min_y.max(max_y),
        })
    });

    let cursor = group
        .get("cursor")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Some(InteractiveElement {
        id,
        layer: layer_name.to_string(),
        hit_region,
        // Transform fields are set by collect_interactive_elements after
        // parsing. Defaults here represent a top-level element with no
        // ancestor transforms or clips.
        transform: TransformMatrix::identity(),
        inverse_transform: Some(TransformMatrix::identity()),
        clip_rect: None,
        on_click: group
            .get("on_click")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        on_hover: group
            .get("on_hover")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        draggable,
        drag_axis,
        drag_bounds,
        cursor,
        has_hover_style: group.get("hover_style").is_some(),
        has_pressed_style: group.get("pressed_style").is_some(),
        has_focus_style: group.get("focus_style").is_some(),
        show_focus_ring: group
            .get("show_focus_ring")
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        focus_ring_radius: group
            .get("focus_ring_radius")
            .and_then(|v| v.as_f64())
            .map(|v| v as f32),
        focusable: group
            .get("focusable")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        // Set by collect_interactive_elements based on nesting context.
        parent_group: None,
        tooltip: group
            .get("tooltip")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        a11y: group
            .get("a11y")
            .and_then(super::a11y::A11yOverrides::from_a11y_value),
    })
}

/// Extract the net translation from a group's `"transforms"` array.
///
/// Scans for `{"type": "translate", "x": ..., "y": ...}` entries and
/// sums their x/y components. Non-translate transforms (rotate, scale)
/// are ignored for this purpose -- they affect hit testing via the
/// transform matrix in Phase 2.5, not via this simple offset.
fn group_translation(group: &Value) -> (f32, f32) {
    let transforms = match group.get("transforms").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return (0.0, 0.0),
    };
    let mut tx = 0.0f32;
    let mut ty = 0.0f32;
    for t in transforms {
        if t.get("type").and_then(|v| v.as_str()) == Some("translate") {
            tx += t.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
            ty += t.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
        }
    }
    (tx, ty)
}

/// Apply a group's transforms to the drawing frame.
///
/// Reads the `"transforms"` array and applies each entry in order.
/// Supported transform types: `translate`, `rotate`, `scale`.
/// The caller is responsible for calling `frame.push_transform()` before
/// and `frame.pop_transform()` after this function.
fn apply_group_transforms(frame: &mut canvas::Frame, group: &Value) {
    let transforms = match group.get("transforms").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return,
    };
    for t in transforms {
        let t_type = t.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match t_type {
            "translate" => {
                let x = t.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                let y = t.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                frame.translate(Vector::new(x, y));
            }
            "rotate" => {
                let angle = t.get("angle").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                frame.rotate(Radians(angle));
            }
            "scale" => {
                if let Some(factor) = t.get("factor").and_then(|v| v.as_f64()) {
                    frame.scale(factor as f32);
                } else {
                    let x = t.get("x").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;
                    let y = t.get("y").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;
                    frame.scale_nonuniform(Vector::new(x, y));
                }
            }
            _ => {
                log::warn!("canvas group: unknown transform type '{t_type}'");
            }
        }
    }
}

/// Apply a group's clip region to the frame, drawing children inside.
///
/// If the group has a `"clip"` field with `{x, y, w, h}`, children are
/// drawn clipped to that rectangle. Otherwise children are drawn directly.
fn draw_with_group_clip(
    frame: &mut canvas::Frame,
    group: &Value,
    images: &crate::image_registry::ImageRegistry,
    theme: &iced::Theme,
    children: &[&Value],
    draw_fn: impl FnOnce(&mut canvas::Frame, &[&Value], &crate::image_registry::ImageRegistry, &iced::Theme),
) {
    if let Some(clip) = group.get("clip").and_then(|v| v.as_object()) {
        let x = clip.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
        let y = clip.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
        let w = clip.get("w").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
        let h = clip.get("h").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
        let clip_rect = iced::Rectangle { x, y, width: w, height: h };
        frame.with_clip(clip_rect, |f| {
            draw_fn(f, children, images, theme);
        });
    } else {
        draw_fn(frame, children, images, theme);
    }
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
            let (gx, gy) = group_translation(child);
            let nested = child.get("children").and_then(|v| v.as_array())?;
            let (min_x, min_y, max_x, max_y) = children_bounds(nested)?;
            Some((gx + min_x, gy + min_y, gx + max_x, gy + max_y))
        }
        "path" => path_bounds(child),
        // Other shape types can't have their bounds automatically
        // determined. Use hit_rect on the parent group.
        _ => None,
    }
}

/// Compute bounding box of a path from its commands.
/// Examines move_to, line_to, and arc endpoints. Bezier control points
/// are included conservatively (they bound the curve).
fn path_bounds(shape: &Value) -> Option<(f32, f32, f32, f32)> {
    let commands = shape.get("commands")?.as_array()?;
    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    let mut max_x = f32::MIN;
    let mut max_y = f32::MIN;
    let mut has_point = false;

    for cmd in commands {
        let points: Vec<f32> = if let Some(arr) = cmd.as_array() {
            // ["move_to", x, y] or ["line_to", x, y] etc.
            arr.iter()
                .skip(1)
                .filter_map(|v| v.as_f64().map(|f| f as f32))
                .collect()
        } else {
            continue; // "close" or other string commands
        };

        // Take all numeric values as x,y pairs
        for pair in points.chunks(2) {
            if pair.len() == 2 {
                min_x = min_x.min(pair[0]);
                min_y = min_y.min(pair[1]);
                max_x = max_x.max(pair[0]);
                max_y = max_y.max(pair[1]);
                has_point = true;
            }
        }
    }

    has_point.then_some((min_x, min_y, max_x, max_y))
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

/// Compute the hit region for a group from its children's geometry.
///
/// Hit regions are in **local** coordinates (before transforms are applied).
/// Transform matrices are composed separately during hit testing.
///
/// An explicit `hit_rect` on the group overrides automatic inference.
/// `hit_rect` is in the group's local coordinate space.
fn compute_hit_region(group: &Value) -> Option<HitRegion> {
    // Explicit hit_rect overrides geometric inference.
    // hit_rect is in local coordinates -- no offset needed.
    if let Some(hr) = group.get("hit_rect").and_then(|v| v.as_object()) {
        let x = hr.get("x")?.as_f64()? as f32;
        let y = hr.get("y")?.as_f64()? as f32;
        let w = hr.get("w").or(hr.get("width"))?.as_f64()? as f32;
        let h = hr.get("h").or(hr.get("height"))?.as_f64()? as f32;
        return Some(HitRegion::Rect { x, y, w, h });
    }

    // Infer from children's bounding box.
    let children = group.get("children").and_then(|v| v.as_array())?;
    let (min_x, min_y, max_x, max_y) = children_bounds(children)?;
    Some(HitRegion::Rect {
        x: min_x,
        y: min_y,
        w: max_x - min_x,
        h: max_y - min_y,
    })
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
        _ => mouse::Interaction::Pointer, // default for interactive elements
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
    /// ID of the interactive element currently under the cursor.
    hovered_element: Option<String>,
    /// ID of the element being pressed (mouse down, not yet released).
    pressed_element: Option<String>,
    /// Active drag state (element being dragged).
    dragging: Option<DragState>,
    /// ID of the interactive element that has keyboard focus.
    /// ID-based (not index-based) so focus survives element reordering
    /// between renders. When the focused element is removed, focus is
    /// cleared and a blur event is emitted.
    focused_id: Option<String>,
    /// ID of the focusable group that currently has group-level focus
    /// in two-level navigation. `None` when navigating at the top level
    /// or when no focusable groups exist.
    focused_group: Option<String>,
    /// Tracks the last consumed pending_focus to prevent re-firing.
    /// See pending_focus consumption in update().
    last_consumed_pending: Option<String>,
    /// Whether the canvas currently has iced-level focus. Set by
    /// `on_focus_gained`, cleared by `on_focus_lost`. Used to suppress
    /// focus visuals (focus_style, focus ring) when the canvas is
    /// unfocused but `focused_id` is preserved for re-entry.
    canvas_focused: bool,
    /// Whether the focus indicator should be visible. `true` for
    /// keyboard navigation (Tab), `false` for mouse clicks.
    /// Matches iced's "focus-visible" pattern.
    focus_visible: bool,
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
    /// Interactive elements parsed during ensure_caches.
    interactive_elements: &'a [InteractiveElement],
    /// Arrow key navigation mode.
    arrow_mode: ArrowMode,
    /// Pending programmatic focus from `focus_element` widget_op.
    /// Consumed at the top of `update()` to set `focused_id`.
    pending_focus: Option<String>,
}

impl CanvasProgram<'_> {
    fn is_interactive(&self) -> bool {
        self.on_press
            || self.on_release
            || self.on_move
            || self.on_scroll
            || !self.interactive_elements.is_empty()
    }

    /// Collect layer names that need cache bypass due to active
    /// interaction state (hover_style, pressed_style, or focus_style).
    ///
    /// Multiple layers can be active simultaneously (e.g., hover on
    /// layer A while focus is on layer B). All returned layers are
    /// redrawn fresh with style overrides applied.
    fn layers_with_active_interaction(&self, state: &CanvasState) -> Vec<String> {
        let mut layers = Vec::new();

        // Hover/pressed style.
        let active_id = state
            .pressed_element
            .as_deref()
            .or(state.hovered_element.as_deref());
        if let Some(id) = active_id {
            if let Some(shape) = self.interactive_elements.iter().find(|s| s.id == id) {
                if shape.has_hover_style || shape.has_pressed_style {
                    layers.push(shape.layer.clone());
                }
            }
        }

        // Keyboard focus with focus_style (only when canvas has iced focus).
        if state.canvas_focused && state.focus_visible {
        if let Some(ref focused_id) = state.focused_id {
            if let Some(shape) = self
                .interactive_elements
                .iter()
                .find(|s| &s.id == focused_id)
            {
                if shape.has_focus_style && !layers.contains(&shape.layer) {
                    layers.push(shape.layer.clone());
                }
            }
        }
        }

        layers
    }

    /// Get the tooltip text for the currently hovered shape, if any.
    fn active_tooltip(&self, state: &CanvasState) -> Option<String> {
        let hovered_id = state.hovered_element.as_deref()?;
        let shape = self
            .interactive_elements
            .iter()
            .find(|s| s.id == hovered_id)?;
        shape.tooltip.clone()
    }

    /// Resolve the currently focused element ID to its index in the
    /// interactive elements list. Returns `None` if no element is focused
    /// or the focused element no longer exists (removed between renders).
    fn resolve_focus_index(&self, state: &CanvasState) -> Option<usize> {
        let focused_id = state.focused_id.as_deref()?;
        self.interactive_elements
            .iter()
            .position(|e| e.id == focused_id)
    }

    /// Transition focus to a new element by index. Returns a single
    /// [`CanvasElementFocusChanged`](Message::CanvasElementFocusChanged)
    /// message that the emitter splits into separate blur + focus
    /// outgoing events (in that order).
    ///
    /// Pass `None` for `new_index` to clear focus without moving to
    /// another element (e.g., Escape or click-on-empty).
    ///
    /// Returns `None` if no state change occurred (already focused on
    /// the target, or clearing focus when nothing was focused).
    fn set_focus(
        &self,
        state: &mut CanvasState,
        new_index: Option<usize>,
    ) -> Option<Message> {
        let old_id = state.focused_id.take();

        let new_id = new_index
            .filter(|&idx| idx < self.interactive_elements.len())
            .map(|idx| self.interactive_elements[idx].id.clone());

        // No-op if focus didn't actually change.
        if old_id == new_id {
            // Restore the original focused_id since we took it.
            state.focused_id = old_id;
            return None;
        }

        state.focused_id = new_id.clone();

        // Only emit a message if something actually changed.
        if old_id.is_some() || new_id.is_some() {
            Some(Message::CanvasElementFocusChanged {
                canvas_id: self.id.clone(),
                old_element_id: old_id,
                new_element_id: new_id,
            })
        } else {
            None
        }
    }

    /// Get the indices of "top-level" entries for Tab navigation.
    ///
    /// Top-level entries are elements where `parent_group.is_none()`.
    /// This includes standalone elements and focusable groups themselves
    /// (but not children of focusable groups).
    fn top_level_indices(&self) -> Vec<usize> {
        self.interactive_elements
            .iter()
            .enumerate()
            .filter(|(_, e)| e.parent_group.is_none())
            .map(|(i, _)| i)
            .collect()
    }

    /// Get the indices of children within a focusable group.
    fn group_child_indices(&self, group_id: &str) -> Vec<usize> {
        self.interactive_elements
            .iter()
            .enumerate()
            .filter(|(_, e)| e.parent_group.as_deref() == Some(group_id))
            .map(|(i, _)| i)
            .collect()
    }

    /// Draw shapes with hover/pressed/focus style overrides applied to the
    /// active element. Used when a layer needs fresh drawing due to
    /// interaction state changes (cache is bypassed).
    ///
    /// Only groups can be interactive elements. Non-group shapes are drawn
    /// directly. When a group is the active element, its children have
    /// per-child style overrides applied. Priority: pressed > hover > focus.
    fn draw_shapes_with_overrides(
        &self,
        frame: &mut canvas::Frame,
        shapes: &[&Value],
        state: &CanvasState,
        images: &crate::image_registry::ImageRegistry,
        theme: &iced::Theme,
    ) {
        let hovered = state.hovered_element.as_deref();
        let pressed = state.pressed_element.as_deref();
        // Only apply focus_style when the canvas has iced-level focus.
        let focused = if state.canvas_focused && state.focus_visible {
            state.focused_id.as_deref()
        } else {
            None
        };

        for &shape in shapes {
            let shape_type = shape.get("type").and_then(|v| v.as_str()).unwrap_or("");

            if shape_type == "group" {
                // Interactive ID is now at the group's top level.
                let group_id = shape.get("id").and_then(|v| v.as_str());
                let is_pressed = group_id.is_some_and(|gid| pressed == Some(gid));
                let is_hovered = group_id.is_some_and(|gid| hovered == Some(gid));
                let is_focused = group_id.is_some_and(|gid| focused == Some(gid));

                if let Some(children) = shape.get("children").and_then(|v| v.as_array()) {
                    let has_transforms = shape.get("transforms")
                        .and_then(|v| v.as_array())
                        .is_some_and(|a| !a.is_empty());

                    if has_transforms {
                        frame.push_transform();
                        apply_group_transforms(frame, shape);
                    }

                    // Resolve the active style override from the GROUP (not children).
                    // Priority: pressed > hover > focus.
                    let group_override: Option<&Value> = if is_pressed {
                        shape.get("pressed_style")
                    } else {
                        None
                    }
                    .or_else(|| if is_hovered { shape.get("hover_style") } else { None })
                    .or_else(|| if is_focused { shape.get("focus_style") } else { None });

                    let draw_children = |f: &mut canvas::Frame, child_refs: &[&Value], img: &crate::image_registry::ImageRegistry, theme: &iced::Theme| {
                        if let Some(overrides) = group_override {
                            for &child in child_refs {
                                // Apply group-level style override to each child.
                                // Children can also have their own per-child overrides
                                // which take precedence (merged on top).
                                let child_override = if is_pressed {
                                    child.get("pressed_style")
                                } else {
                                    None
                                }
                                .or_else(|| if is_hovered { child.get("hover_style") } else { None })
                                .or_else(|| if is_focused { child.get("focus_style") } else { None });

                                let effective = child_override.unwrap_or(overrides);
                                let merged = merge_shape_style(child, effective);
                                draw_canvas_shape(f, &merged, img, theme);
                            }
                        } else {
                            draw_canvas_shapes(f, child_refs, img, theme);
                        }
                    };

                    let child_refs: Vec<&Value> = children.iter().collect();
                    draw_with_group_clip(frame, shape, images, theme, &child_refs, draw_children);

                    if has_transforms {
                        frame.pop_transform();
                    }
                }
            } else {
                // Non-group shapes are never interactive elements in the
                // new design. Draw them directly.
                draw_canvas_shape(frame, shape, images, theme);
            }
        }
    }

    /// Handle keyboard events for interactive element navigation.
    ///
    /// Extracted from `update()` for readability. Implements the roving
    /// tabindex pattern with two-level navigation for focusable groups.
    fn handle_keyboard(
        &self,
        state: &mut CanvasState,
        key: &keyboard::Key,
        modifiers: keyboard::Modifiers,
    ) -> Option<iced::widget::Action<Message>> {
        if state.dragging.is_some() {
            return Some(iced::widget::Action::capture());
        }

        use keyboard::key::Named;

        let current_idx = self.resolve_focus_index(state);
        if current_idx.is_none() && state.focused_id.is_some() {
            state.focused_group = None;
            if let Some(msg) = self.set_focus(state, None) {
                return Some(iced::widget::Action::publish(msg).and_capture());
            }
        }

        let focus_to = |state: &mut CanvasState, idx: Option<usize>| -> Option<iced::widget::Action<Message>> {
            match self.set_focus(state, idx) {
                Some(msg) => Some(iced::widget::Action::publish(msg).and_capture()),
                None => Some(iced::widget::Action::capture()),
            }
        };

        let has_focusable_groups = self.interactive_elements.iter().any(|e| e.focusable);
        let arrow_indices: Vec<usize> = if let Some(ref gid) = state.focused_group {
            self.group_child_indices(gid)
        } else if has_focusable_groups {
            self.top_level_indices()
        } else {
            (0..self.interactive_elements.len()).collect()
        };
        let arrow_pos = current_idx.and_then(|ci| arrow_indices.iter().position(|&i| i == ci));
        let arrow_count = arrow_indices.len();

        match key {
            keyboard::Key::Named(Named::Tab) if !modifiers.shift() => {
                let top = self.top_level_indices();
                let top_pos = current_idx.and_then(|ci| {
                    if let Some(ref gid) = state.focused_group {
                        top.iter().position(|&i| self.interactive_elements[i].id == *gid)
                    } else {
                        top.iter().position(|&i| i == ci)
                    }
                });
                match top_pos {
                    None => {
                        if let Some(&first) = top.first() {
                            state.focused_group = None;
                            let elem = &self.interactive_elements[first];
                            if elem.focusable {
                                state.focused_group = Some(elem.id.clone());
                                let children = self.group_child_indices(&elem.id);
                                if let Some(&fc) = children.first() {
                                    focus_to(state, Some(fc))
                                } else {
                                    focus_to(state, Some(first))
                                }
                            } else {
                                focus_to(state, Some(first))
                            }
                        } else {
                            None
                        }
                    }
                    Some(pos) if pos + 1 < top.len() => {
                        let next_idx = top[pos + 1];
                        let elem = &self.interactive_elements[next_idx];
                        if elem.focusable {
                            state.focused_group = Some(elem.id.clone());
                            let children = self.group_child_indices(&elem.id);
                            if let Some(&fc) = children.first() {
                                focus_to(state, Some(fc))
                            } else {
                                focus_to(state, Some(next_idx))
                            }
                        } else {
                            state.focused_group = None;
                            focus_to(state, Some(next_idx))
                        }
                    }
                    Some(_) => None,
                }
            }
            keyboard::Key::Named(Named::Tab) if modifiers.shift() => {
                let top = self.top_level_indices();
                let top_pos = current_idx.and_then(|ci| {
                    if let Some(ref gid) = state.focused_group {
                        top.iter().position(|&i| self.interactive_elements[i].id == *gid)
                    } else {
                        top.iter().position(|&i| i == ci)
                    }
                });
                match top_pos {
                    None => {
                        if let Some(&last) = top.last() {
                            let elem = &self.interactive_elements[last];
                            if elem.focusable {
                                state.focused_group = Some(elem.id.clone());
                                let children = self.group_child_indices(&elem.id);
                                if let Some(&lc) = children.last() {
                                    focus_to(state, Some(lc))
                                } else {
                                    focus_to(state, Some(last))
                                }
                            } else {
                                state.focused_group = None;
                                focus_to(state, Some(last))
                            }
                        } else {
                            None
                        }
                    }
                    Some(0) => None,
                    Some(pos) => {
                        let prev_idx = top[pos - 1];
                        let elem = &self.interactive_elements[prev_idx];
                        if elem.focusable {
                            state.focused_group = Some(elem.id.clone());
                            let children = self.group_child_indices(&elem.id);
                            if let Some(&lc) = children.last() {
                                focus_to(state, Some(lc))
                            } else {
                                focus_to(state, Some(prev_idx))
                            }
                        } else {
                            state.focused_group = None;
                            focus_to(state, Some(prev_idx))
                        }
                    }
                }
            }
            keyboard::Key::Named(Named::ArrowDown | Named::ArrowRight)
                if self.arrow_mode != ArrowMode::None && arrow_count > 0 =>
            {
                match (arrow_pos, self.arrow_mode) {
                    (None, _) => focus_to(state, Some(arrow_indices[0])),
                    (Some(pos), ArrowMode::Wrap) => {
                        focus_to(state, Some(arrow_indices[(pos + 1) % arrow_count]))
                    }
                    (Some(pos), _) if pos + 1 < arrow_count => {
                        focus_to(state, Some(arrow_indices[pos + 1]))
                    }
                    (Some(_), ArrowMode::Clamp) => Some(iced::widget::Action::capture()),
                    (Some(_), ArrowMode::Linear) => None,
                    _ => None,
                }
            }
            keyboard::Key::Named(Named::ArrowUp | Named::ArrowLeft)
                if self.arrow_mode != ArrowMode::None && arrow_count > 0 =>
            {
                match (arrow_pos, self.arrow_mode) {
                    (None, _) => focus_to(state, Some(*arrow_indices.last().unwrap())),
                    (Some(0), ArrowMode::Wrap) => {
                        focus_to(state, Some(*arrow_indices.last().unwrap()))
                    }
                    (Some(0), ArrowMode::Clamp) => Some(iced::widget::Action::capture()),
                    (Some(0), ArrowMode::Linear) => None,
                    (Some(pos), _) => focus_to(state, Some(arrow_indices[pos - 1])),
                }
            }
            keyboard::Key::Named(Named::Enter | Named::Space) => {
                if let Some(idx) = current_idx {
                    let element = &self.interactive_elements[idx];
                    if element.on_click {
                        let center = hit_region_center(&element.hit_region);
                        Some(iced::widget::Action::publish(Message::CanvasElementClick {
                            canvas_id: self.id.clone(),
                            element_id: element.id.clone(),
                            x: center.x,
                            y: center.y,
                            button: "keyboard".to_string(),
                        }).and_capture())
                    } else {
                        Some(iced::widget::Action::capture())
                    }
                } else {
                    None
                }
            }
            keyboard::Key::Named(Named::Escape) => {
                if state.focused_group.is_some() {
                    let gid = state.focused_group.take().unwrap();
                    let group_idx = self.interactive_elements.iter().position(|e| e.id == gid);
                    match self.set_focus(state, group_idx) {
                        Some(msg) => Some(iced::widget::Action::publish(msg).and_capture()),
                        None => Some(iced::widget::Action::capture()),
                    }
                } else if state.focused_id.is_some() {
                    match self.set_focus(state, None) {
                        Some(msg) => Some(iced::widget::Action::publish(msg).and_capture()),
                        None => Some(iced::widget::Action::capture()),
                    }
                } else {
                    None
                }
            }
            keyboard::Key::Named(Named::Home) if !arrow_indices.is_empty() => {
                focus_to(state, Some(arrow_indices[0]))
            }
            keyboard::Key::Named(Named::End) if !arrow_indices.is_empty() => {
                focus_to(state, Some(*arrow_indices.last().unwrap()))
            }
            keyboard::Key::Named(Named::PageDown) if !arrow_indices.is_empty() => {
                let page_size = 10.min(arrow_count);
                let pos = arrow_pos.unwrap_or(0);
                focus_to(state, Some(arrow_indices[(pos + page_size).min(arrow_count - 1)]))
            }
            keyboard::Key::Named(Named::PageUp) if !arrow_indices.is_empty() => {
                let page_size = 10.min(arrow_count);
                let pos = arrow_pos.unwrap_or(0);
                focus_to(state, Some(arrow_indices[pos.saturating_sub(page_size)]))
            }
            _ => None,
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

/// Draw a focus ring around an interactive element.
///
/// The ring shape adapts to the element's hit region geometry:
/// - **Rect**: rounded rectangle inflated by `inflate` on each side
/// - **Circle**: circle inflated by `inflate`
/// - **Line**: capsule (stadium) around the line, inflated by `inflate`
///
/// The element's accumulated transform is applied to the frame so the
/// ring matches the element's visual position, including rotation and scale.
/// Draw a focus ring around an interactive element.
///
/// The ring shape adapts to the element's hit region geometry:
/// - **Rect**: rounded rectangle inflated by `inflate` on each side
/// - **Circle**: circle inflated by `inflate`
/// - **Line**: capsule (stadium) around the line, inflated by `inflate`
///
/// The element's accumulated transform is applied to the frame so the
/// ring matches the element's visual position, including rotation and scale.
///
/// **Clipping note**: when the hit region fills the entire canvas, the
/// outset ring may be clipped. SDKs should add padding to the canvas
/// (e.g. 4px on each side) to accommodate the focus ring.
fn draw_focus_ring(
    frame: &mut canvas::Frame,
    element: &InteractiveElement,
    color: Color,
    stroke_width: f32,
    inflate: f32,
) {
    frame.push_transform();
    element.transform.apply_to_frame(frame);

    let ring_stroke = canvas::Stroke::default()
        .with_color(color)
        .with_width(stroke_width);

    match &element.hit_region {
        HitRegion::Rect { x, y, w, h } => {
            let radius = element
                .focus_ring_radius
                .unwrap_or(inflate + 1.0);
            let path = canvas::Path::rounded_rectangle(
                Point::new(x - inflate, y - inflate),
                Size::new(w + inflate * 2.0, h + inflate * 2.0),
                iced::border::Radius::from(radius),
            );
            frame.stroke(&path, ring_stroke);
        }
        HitRegion::Circle { cx, cy, r } => {
            let path = canvas::Path::circle(Point::new(*cx, *cy), r + inflate);
            frame.stroke(&path, ring_stroke);
        }
        HitRegion::Line {
            x1, y1, x2, y2, half_width,
        } => {
            // Draw a capsule (stadium shape) around the line.
            // This is a rounded rectangle oriented along the line.
            let dx = x2 - x1;
            let dy = y2 - y1;
            let len = (dx * dx + dy * dy).sqrt();
            if len < 0.01 {
                // Degenerate line -- draw a circle at the midpoint.
                let path = canvas::Path::circle(
                    Point::new((x1 + x2) / 2.0, (y1 + y2) / 2.0),
                    half_width + inflate,
                );
                frame.stroke(&path, ring_stroke);
            } else {
                // Rotate so the line is horizontal, draw a rounded rect,
                // then the existing transform handles the visual rotation.
                let angle = dy.atan2(dx);
                let total_half = half_width + inflate;
                frame.push_transform();
                frame.translate(Vector::new(*x1, *y1));
                frame.rotate(Radians(angle));
                let path = canvas::Path::rounded_rectangle(
                    Point::new(-total_half, -total_half),
                    Size::new(len + total_half * 2.0, total_half * 2.0),
                    iced::border::Radius::from(total_half),
                );
                frame.stroke(&path, ring_stroke);
                frame.pop_transform();
            }
        }
    }

    frame.pop_transform();
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
#[allow(dead_code)] // used by tests
pub(crate) fn parse_canvas_fill(value: &Value, shape: &Value) -> canvas::Fill {
    parse_canvas_fill_themed(value, shape, None)
}

/// Parse a canvas fill with optional theme-aware color resolution.
///
/// When `theme` is `Some`, color strings that match palette names
/// (`"primary"`, `"text"`, `"background"`, `"success"`, `"danger"`,
/// `"warning"`) are resolved against the theme instead of being
/// treated as hex strings.
fn parse_canvas_fill_themed(
    value: &Value,
    shape: &Value,
    theme: Option<&iced::Theme>,
) -> canvas::Fill {
    let rule = parse_fill_rule(shape.get("fill_rule"));
    match value {
        Value::String(_) => {
            let color = theme
                .and_then(|t| resolve_color(value, t))
                .or_else(|| parse_color(value))
                .unwrap_or(Color::WHITE);
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
                                .and_then(|v| {
                                    theme
                                        .and_then(|t| resolve_color(v, t))
                                        .or_else(|| parse_color(v))
                                })
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
#[allow(dead_code)] // used by tests
pub(crate) fn parse_canvas_stroke(value: &Value) -> canvas::Stroke<'static> {
    parse_canvas_stroke_themed(value, None)
}

/// Parse a canvas stroke with optional theme-aware color resolution.
fn parse_canvas_stroke_themed(
    value: &Value,
    theme: Option<&iced::Theme>,
) -> canvas::Stroke<'static> {
    let obj = match value.as_object() {
        Some(o) => o,
        None => return canvas::Stroke::default(),
    };
    let color = theme
        .and_then(|t| obj.get("color").and_then(|v| resolve_color(v, t)))
        .or_else(|| obj.get("color").and_then(parse_color))
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

/// Draw a sequence of shapes.
///
/// Clips and transforms are handled at the group level -- each group
/// carries its own `"transforms"` and `"clip"` fields, applied in
/// [`draw_canvas_shape`] when rendering the `"group"` type.
fn draw_canvas_shapes(
    frame: &mut canvas::Frame,
    shapes: &[&Value],
    images: &crate::image_registry::ImageRegistry,
    theme: &iced::Theme,
) {
    for &shape in shapes {
        let shape_type = shape.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match shape_type {
            // Standalone clip commands are no longer supported.
            "push_clip" | "pop_clip" => {
                log::warn!(
                    "canvas: standalone '{shape_type}' commands are no longer supported. \
                     Use group clip instead."
                );
            }
            _ => draw_canvas_shape(frame, shape, images, theme),
        }
    }
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
    theme: &iced::Theme,
) {
    let shape_type = shape.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match shape_type {
        // Standalone transform/clip commands are no longer supported.
        // Transforms and clips now live on groups via "transforms" and "clip" fields.
        "push_transform" | "pop_transform" | "translate" | "rotate" | "scale" => {
            log::warn!(
                "canvas: standalone '{shape_type}' commands are no longer supported. \
                 Use group transforms instead."
            );
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
                let fill = apply_opacity_to_fill(shape, parse_canvas_fill_themed(fill_val, shape, Some(theme)));
                frame.fill(&rect_path, fill);
            } else if shape.get("stroke").is_none() {
                // Legacy fallback: no fill or stroke key means solid white fill
                let color = apply_opacity_to_color(shape, Color::WHITE);
                frame.fill_rectangle(Point::new(x, y), Size::new(w, h), color);
            }
            if let Some(stroke_val) = shape.get("stroke") {
                let stroke = apply_opacity_to_stroke(shape, parse_canvas_stroke_themed(stroke_val, Some(theme)));
                frame.stroke(&rect_path, stroke);
            }
        }
        "circle" => {
            let x = json_f32(shape, "x");
            let y = json_f32(shape, "y");
            let r = json_f32(shape, "r");
            let circle_path = canvas::Path::circle(Point::new(x, y), r);
            if let Some(fill_val) = shape.get("fill") {
                let fill = apply_opacity_to_fill(shape, parse_canvas_fill_themed(fill_val, shape, Some(theme)));
                frame.fill(&circle_path, fill);
            } else if shape.get("stroke").is_none() {
                let color = apply_opacity_to_color(shape, Color::WHITE);
                frame.fill(&circle_path, color);
            }
            if let Some(stroke_val) = shape.get("stroke") {
                let stroke = apply_opacity_to_stroke(shape, parse_canvas_stroke_themed(stroke_val, Some(theme)));
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
                let stroke = apply_opacity_to_stroke(shape, parse_canvas_stroke_themed(stroke_val, Some(theme)));
                frame.stroke(&line_path, stroke);
            } else {
                // Legacy: use fill color as stroke color
                let color = apply_opacity_to_color(shape, json_color_themed(shape, "fill", theme));
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
            let fill_color = apply_opacity_to_color(shape, json_color_themed(shape, "fill", theme));
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
                let fill = apply_opacity_to_fill(shape, parse_canvas_fill_themed(fill_val, shape, Some(theme)));
                frame.fill(&path, fill);
            }
            if let Some(stroke_val) = shape.get("stroke") {
                let stroke = apply_opacity_to_stroke(shape, parse_canvas_stroke_themed(stroke_val, Some(theme)));
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
            if let Some(children) = shape.get("children").and_then(|v| v.as_array()) {
                let child_refs: Vec<&Value> = children.iter().collect();
                let has_transforms = shape.get("transforms")
                    .and_then(|v| v.as_array())
                    .is_some_and(|a| !a.is_empty());

                if has_transforms {
                    frame.push_transform();
                    apply_group_transforms(frame, shape);
                }

                // draw_with_group_clip handles the clip field (if present)
                // using frame.with_clip, which manages its own scope.
                draw_with_group_clip(frame, shape, images, theme, &child_refs, |f, c, img, theme| {
                    draw_canvas_shapes(f, c, img, theme);
                });

                if has_transforms {
                    frame.pop_transform();
                }
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
        // Keyboard events don't depend on cursor position -- handle them
        // before the cursor check so they work when the mouse is outside.
        if matches!(event, iced::Event::Keyboard(..)) {
            if !self.interactive_elements.is_empty() {
                if let iced::Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) = event {
                    return self.handle_keyboard(state, key, *modifiers);
                }
            }
            return None;
        }

        // Consume pending programmatic focus (not position-dependent).
        if let Some(ref pending) = self.pending_focus {
            if state.last_consumed_pending.as_deref() != Some(pending.as_str()) {
                state.last_consumed_pending = Some(pending.clone());
                let idx = self
                    .interactive_elements
                    .iter()
                    .position(|e| e.id == *pending);
                if let Some(idx) = idx {
                    if let Some(msg) = self.set_focus(state, Some(idx)) {
                        state.focused_group = self.interactive_elements[idx]
                            .parent_group
                            .clone();
                        return Some(iced::widget::Action::publish(msg));
                    }
                }
            }
        }

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
                    let msg = Message::CanvasElementDragEnd {
                        canvas_id: self.id.clone(),
                        element_id: drag.element_id,
                        x: pos.x,
                        y: pos.y,
                    };
                    action = Some(iced::widget::Action::publish(msg));
                }
                if let Some(hovered_id) = state.hovered_element.take() {
                    let msg = Message::CanvasElementLeave {
                        canvas_id: self.id.clone(),
                        element_id: hovered_id,
                    };
                    action = Some(pick_action(action, iced::widget::Action::publish(msg)));
                }
                state.pressed_element = None;
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
                        .interactive_elements
                        .iter()
                        .find(|s| s.id == drag.element_id);

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
                    let msg = Message::CanvasElementDrag {
                        canvas_id: self.id.clone(),
                        element_id: drag.element_id.clone(),
                        x: effective.x,
                        y: effective.y,
                        delta_x: dx,
                        delta_y: dy,
                    };
                    action = Some(iced::widget::Action::publish(msg).and_capture());
                }

                // -- Hover tracking (skip during active drag) --
                if state.dragging.is_none() {
                    let hit = find_hit_element(position, self.interactive_elements);
                    let new_hovered = hit.map(|s| s.id.clone());
                    let old_hovered = state.hovered_element.take();

                    if new_hovered != old_hovered {
                        // Enter is emitted AFTER leave so that pick_action
                        // keeps Enter when both fire (direct A -> B transition).
                        // The host can infer leave from receiving enter for a
                        // different shape. Losing Enter is worse than losing
                        // Leave -- Enter tells the host WHAT is hovered.
                        if let Some(ref old_id) = old_hovered {
                            let msg = Message::CanvasElementLeave {
                                canvas_id: self.id.clone(),
                                element_id: old_id.clone(),
                            };
                            action = Some(pick_action(action, iced::widget::Action::publish(msg)));
                        }
                        if let Some(ref new_id) = new_hovered {
                            let msg = Message::CanvasElementEnter {
                                canvas_id: self.id.clone(),
                                element_id: new_id.clone(),
                                x: position.x,
                                y: position.y,
                            };
                            // Override any previous action -- Enter takes
                            // priority over Leave and raw canvas move.
                            action = Some(iced::widget::Action::publish(msg));
                        }
                    }
                    state.hovered_element = new_hovered;
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

                // Mouse interaction clears focus-visible (focus ring only
                // shows for keyboard navigation, not mouse clicks).
                state.focus_visible = false;

                // -- Shape press: start drag or track pressed --
                // Drag and click are mutually exclusive: if a shape is
                // draggable, we start a drag (click never fires for it).
                // If it's only clickable, we track pressed state for
                // click detection on release.
                if matches!(button, mouse::Button::Left) {
                    if let Some(shape) = find_hit_element(position, self.interactive_elements) {
                        // Click-to-focus: move keyboard focus to clicked element.
                        let clicked_idx = self
                            .interactive_elements
                            .iter()
                            .position(|e| e.id == shape.id);
                        if let Some(msg) = self.set_focus(state, clicked_idx) {
                            action = Some(pick_action(action, iced::widget::Action::publish(msg)));
                        }
                        // Update focused_group context for two-level navigation.
                        state.focused_group = shape.parent_group.clone();

                        if shape.draggable {
                            state.dragging = Some(DragState {
                                element_id: shape.id.clone(),
                                last: position,
                            });
                        } else if shape.on_click {
                            state.pressed_element = Some(shape.id.clone());
                        }
                    } else if state.focused_id.is_some() {
                        // Click on empty area -- clear focus and group context.
                        state.focused_group = None;
                        if let Some(msg) = self.set_focus(state, None) {
                            action = Some(pick_action(action, iced::widget::Action::publish(msg)));
                        }
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
                        let msg = Message::CanvasElementDragEnd {
                            canvas_id: self.id.clone(),
                            element_id: drag.element_id,
                            x: position.x,
                            y: position.y,
                        };
                        action = Some(pick_action(action, iced::widget::Action::publish(msg)));
                    }

                    // -- Click detection: pressed shape == current hover --
                    if let Some(pressed_id) = state.pressed_element.take() {
                        let still_over = state
                            .hovered_element
                            .as_ref()
                            .map(|h| h == &pressed_id)
                            .unwrap_or(false);
                        if still_over {
                            let msg = Message::CanvasElementClick {
                                canvas_id: self.id.clone(),
                                element_id: pressed_id,
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

            // Keyboard events are handled before the cursor position check
            // (at the top of update) so they work when the cursor is outside.
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
        let active_layers = self.layers_with_active_interaction(state);

        // Draw each layer, using its cache when available.
        let images = self.images;
        for (layer_name, shapes) in &self.layers {
            let shape_refs: Vec<&Value> = shapes.iter().collect();
            let force_redraw = active_layers.iter().any(|l| l == layer_name);

            let geom = if !force_redraw {
                if let Some((_hash, cache)) = self.caches.and_then(|c| c.get(layer_name)) {
                    cache.draw(renderer, bounds.size(), |frame| {
                        draw_canvas_shapes(frame, &shape_refs, images, theme);
                    })
                } else {
                    let mut frame = canvas::Frame::new(renderer, bounds.size());
                    draw_canvas_shapes(&mut frame, &shape_refs, images, theme);
                    frame.into_geometry()
                }
            } else {
                // Layer has active interaction (hover/pressed/focus style) --
                // clear cache and draw fresh with style overrides applied.
                if let Some((_hash, cache)) = self.caches.and_then(|c| c.get(layer_name)) {
                    cache.clear();
                }
                let mut frame = canvas::Frame::new(renderer, bounds.size());
                self.draw_shapes_with_overrides(&mut frame, &shape_refs, state, images, theme);
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
        // Only drawn when the canvas has iced-level focus.
        if state.canvas_focused && state.focus_visible {
        if let Some(focused_id) = &state.focused_id {
            if let Some(element) = self.interactive_elements.iter().find(|e| &e.id == focused_id) {
                if element.show_focus_ring {
                    let mut frame = canvas::Frame::new(renderer, bounds.size());
                    let focus_color = theme.palette().primary.base.color;
                    draw_focus_ring(&mut frame, element, focus_color, 2.0, 2.0);
                    geometries.push(frame.into_geometry());
                }
            }
        }
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
        if let Some(ref hovered_id) = state.hovered_element
            && let Some(shape) = self.interactive_elements.iter().find(|s| &s.id == hovered_id)
        {
            if let Some(ref cursor_name) = shape.cursor {
                return parse_cursor_interaction(cursor_name);
            }
            // Default cursor for interactive elements without explicit cursor.
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
        !self.interactive_elements.is_empty()
    }

    fn on_focus_gained(
        &self,
        state: &mut CanvasState,
        focus_visible: bool,
    ) -> Vec<iced::widget::Action<Message>> {
        state.canvas_focused = true;
        state.focus_visible = focus_visible;
        let mut actions = vec![iced::widget::Action::publish(Message::CanvasFocused {
            canvas_id: self.id.clone(),
        })];
        // If returning to a canvas that had internal focus, re-announce
        // the focused element -- but only if it still exists. If it was
        // removed while the canvas was unfocused, clear the stale ID.
        if let Some(ref id) = state.focused_id {
            let still_exists = self.interactive_elements.iter().any(|e| &e.id == id);
            if still_exists {
                actions.push(iced::widget::Action::publish(
                    Message::CanvasElementFocused {
                        canvas_id: self.id.clone(),
                        element_id: id.clone(),
                    },
                ));
            } else {
                // Element was removed while canvas was unfocused.
                // Emit blur for the stale element and clear.
                actions.push(iced::widget::Action::publish(
                    Message::CanvasElementBlurred {
                        canvas_id: self.id.clone(),
                        element_id: id.clone(),
                    },
                ));
                state.focused_id = None;
                state.focused_group = None;
            }
        }
        actions
    }

    fn on_focus_lost(
        &self,
        state: &mut CanvasState,
    ) -> Vec<iced::widget::Action<Message>> {
        state.canvas_focused = false;
        let mut actions = Vec::new();
        // Emit blur for the currently focused element (but DON'T clear
        // focused_id -- preserve position so re-entry via Tab returns
        // to the same element).
        if let Some(ref id) = state.focused_id {
            actions.push(iced::widget::Action::publish(
                Message::CanvasElementBlurred {
                    canvas_id: self.id.clone(),
                    element_id: id.clone(),
                },
            ));
        }
        actions.push(iced::widget::Action::publish(Message::CanvasBlurred {
            canvas_id: self.id.clone(),
        }));
        actions
    }

    fn active_descendant_id(
        &self,
        state: &CanvasState,
    ) -> Option<iced::widget::Id> {
        state
            .focused_id
            .as_ref()
            .map(|id| iced::widget::Id::from(id.clone()))
    }

    fn operate_accessible(
        &self,
        _state: &CanvasState,
        canvas_bounds: iced::Rectangle,
        operation: &mut dyn iced::advanced::widget::Operation,
    ) {
        let mut seen_ids = std::collections::HashSet::new();

        // Emit accessible nodes for each interactive element with a11y metadata.
        // Focusable groups use traverse() to create parent-child relationships
        // in the accessibility tree.
        for element in self.interactive_elements {
            let a11y = match &element.a11y {
                Some(a) => a,
                None => continue,
            };
            if !seen_ids.insert(&element.id) {
                continue;
            }

            // Compute bounds in canvas space using the element's transform.
            let local_rect = hit_region_to_rect(&element.hit_region);
            let (tx, ty) = element.transform.transform_point(local_rect.x, local_rect.y);
            let (bx, by) = element.transform.transform_point(
                local_rect.x + local_rect.width,
                local_rect.y + local_rect.height,
            );
            let element_bounds = Rectangle {
                x: canvas_bounds.x + tx.min(bx),
                y: canvas_bounds.y + ty.min(by),
                width: (bx - tx).abs(),
                height: (by - ty).abs(),
            };

            // Pass widget ID so active_descendant references can resolve.
            let wid = iced::widget::Id::from(element.id.clone());

            if element.focusable {
                // Focusable group: emit as parent, then traverse children.
                operation.accessible(Some(&wid), element_bounds, &a11y.to_accessible());
                operation.traverse(&mut |child_op| {
                    for child in self.interactive_elements.iter() {
                        if child.parent_group.as_deref() != Some(&element.id) {
                            continue;
                        }
                        if let Some(ref child_a11y) = child.a11y {
                            if !seen_ids.insert(&child.id) {
                                continue;
                            }
                            let cr = hit_region_to_rect(&child.hit_region);
                            let (ctx, cty) = child.transform.transform_point(cr.x, cr.y);
                            let (cbx, cby) = child.transform.transform_point(
                                cr.x + cr.width,
                                cr.y + cr.height,
                            );
                            let child_bounds = Rectangle {
                                x: canvas_bounds.x + ctx.min(cbx),
                                y: canvas_bounds.y + cty.min(cby),
                                width: (cbx - ctx).abs(),
                                height: (cby - cty).abs(),
                            };
                            let child_wid = iced::widget::Id::from(child.id.clone());
                            child_op.accessible(
                                Some(&child_wid),
                                child_bounds,
                                &child_a11y.to_accessible(),
                            );
                        }
                    }
                });
            } else if element.parent_group.is_none() {
                // Top-level non-group element.
                operation.accessible(Some(&wid), element_bounds, &a11y.to_accessible());
            }
            // Elements with parent_group are emitted inside their group's traverse().
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

    let interactive_elements = ctx
        .caches
        .canvas_interactions
        .get(&node.id)
        .map(|v| v.as_slice())
        .unwrap_or(&[]);
    let has_interactive_elements = !interactive_elements.is_empty();

    let mut c = iced::widget::canvas(CanvasProgram {
        layers,
        caches: node_caches,
        background,
        id: node.id.clone(),
        on_press: on_press || interactive || has_interactive_elements,
        on_release: on_release || interactive || has_interactive_elements,
        on_move: on_move || interactive || has_interactive_elements,
        on_scroll: on_scroll || interactive,
        images: ctx.images,
        interactive_elements,
        arrow_mode: prop_str(props, "arrow_mode")
            .map(|s| ArrowMode::from_str(&s))
            .unwrap_or_default(),
        pending_focus: ctx
            .caches
            .canvas_pending_focus
            .get(&node.id)
            .cloned(),
    })
    .width(width)
    .height(height);

    // Widget ID -- enables Command.focus("canvas-id") targeting.
    c = c.id(iced::widget::Id::from(node.id.clone()));

    if let Some(alt) = prop_str(props, "alt") {
        c = c.alt(alt);
    }
    if let Some(desc) = prop_str(props, "description") {
        c = c.description(desc);
    }

    // Accessible role: explicit prop, or auto-infer from content.
    // Default: Group when interactive elements exist, Image otherwise.
    if let Some(role_str) = prop_str(props, "role") {
        if let Some(role) = super::a11y::parse_role_str(&role_str) {
            c = c.role(role);
        } else {
            log::warn!("canvas '{}': unknown role '{role_str}'", node.id);
        }
    } else if has_interactive_elements {
        c = c.role(iced::advanced::widget::operation::accessible::Role::Group);
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
#[allow(dead_code)] // used by tests
pub(crate) fn json_color(val: &Value, key: &str) -> Color {
    val.get(key).and_then(parse_color).unwrap_or(Color::WHITE)
}

/// Theme-aware version of [`json_color`]. Resolves palette names against
/// the theme before falling back to hex parsing.
fn json_color_themed(val: &Value, key: &str, theme: &iced::Theme) -> Color {
    val.get(key)
        .and_then(|v| resolve_color(v, theme))
        .unwrap_or(Color::WHITE)
}

/// Resolve a color value that may be a hex string OR a theme palette name.
///
/// Theme palette names: `"primary"`, `"text"`, `"background"`, `"success"`,
/// `"danger"`, `"warning"`. When a canvas shape uses one of these as a fill
/// or stroke color, the renderer resolves it against the current iced theme
/// at draw time instead of treating it as a literal hex string.
///
/// Returns `None` if the value is neither a valid hex color nor a recognized
/// theme palette name.
fn resolve_color(value: &Value, theme: &iced::Theme) -> Option<Color> {
    let s = value.as_str()?;

    // Try hex first (most common case).
    if s.starts_with('#') {
        return parse_color(value);
    }

    // Theme palette name resolution.
    let palette = theme.palette();
    match s {
        "primary" => Some(palette.primary.base.color),
        "text" => Some(palette.background.base.text),
        "background" => Some(palette.background.base.color),
        "success" => Some(palette.success.base.color),
        "danger" => Some(palette.danger.base.color),
        "warning" => Some(palette.warning.base.color),
        _ => {
            // Fall back to hex parsing (handles non-# prefixed hex, etc.)
            parse_color(value)
        }
    }
}

// ---------------------------------------------------------------------------
// Cache ensure function
// ---------------------------------------------------------------------------

/// Recursively collect interactive elements from a shape array, descending
/// into groups. The `parent_transform` accumulates the full 2D affine
/// transform from all ancestor groups, enabling correct hit testing for
/// rotated, scaled, and translated elements.
///
/// The optional `parent_clip` is the intersection of all ancestor clip
/// rectangles (in canvas space). Elements are only hittable when the
/// cursor falls within this clip region.
///
/// `focusable_parent` is the ID of the nearest ancestor focusable group.
///
/// **Limitation**: Nested focusable groups (focusable inside focusable)
/// are not fully supported. The inner group appears as a child of the
/// outer group in arrow navigation, but there is no keyboard mechanism
/// to "drill into" the inner group. This is an uncommon edge case --
/// most canvases have at most one level of focusable groups.
/// Children of a focusable group get `parent_group = Some(group_id)`,
/// which controls two-level keyboard navigation: Tab moves between
/// top-level entries, arrows navigate within a focused group's children.
///
/// Only groups with an `"id"` field are collected as interactive elements.
/// Non-group shapes are skipped regardless of any fields they carry.
fn collect_interactive_elements(
    shapes: &[Value],
    layer_name: &str,
    parent_transform: TransformMatrix,
    parent_clip: Option<(f32, f32, f32, f32)>,
    focusable_parent: Option<&str>,
    out: &mut Vec<InteractiveElement>,
) {
    for shape in shapes {
        let is_group = shape
            .get("type")
            .and_then(|v| v.as_str())
            .is_some_and(|t| t == "group");

        if !is_group {
            continue;
        }

        // Compose this group's transforms with the parent's accumulated matrix.
        let group_matrix = match shape.get("transforms").and_then(|v| v.as_array()) {
            Some(arr) if !arr.is_empty() => {
                let local = TransformMatrix::from_transforms(arr);
                parent_transform.compose(&local)
            }
            _ => parent_transform,
        };

        // Intersect this group's clip (if any) with parent clip.
        let group_clip = if let Some(clip) = shape.get("clip").and_then(|v| v.as_object()) {
            let cx = clip.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
            let cy = clip.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
            let cw = clip.get("w").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
            let ch = clip.get("h").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;

            let corners = [
                group_matrix.transform_point(cx, cy),
                group_matrix.transform_point(cx + cw, cy),
                group_matrix.transform_point(cx, cy + ch),
                group_matrix.transform_point(cx + cw, cy + ch),
            ];
            let min_x = corners.iter().map(|c| c.0).fold(f32::MAX, f32::min);
            let min_y = corners.iter().map(|c| c.1).fold(f32::MAX, f32::min);
            let max_x = corners.iter().map(|c| c.0).fold(f32::MIN, f32::max);
            let max_y = corners.iter().map(|c| c.1).fold(f32::MIN, f32::max);

            let clip_in_canvas = (min_x, min_y, max_x - min_x, max_y - min_y);

            match parent_clip {
                Some(pc) => Some(intersect_rects(pc, clip_in_canvas)),
                None => Some(clip_in_canvas),
            }
        } else {
            parent_clip
        };

        // Collect this group if it's interactive (has an id).
        // Determine the focusable context for children: if this group is
        // focusable, its children get parent_group = this group's ID.
        let mut child_focusable_parent = focusable_parent;
        let mut focusable_group_id: Option<String> = None;

        if let Some(mut element) = parse_interactive_element(shape, layer_name) {
            element.transform = group_matrix;
            element.inverse_transform = group_matrix.inverse();
            element.clip_rect = group_clip;
            element.parent_group = focusable_parent.map(|s| s.to_string());

            if element.focusable {
                focusable_group_id = Some(element.id.clone());
            }

            out.push(element);
        }

        // If this group is focusable, its children belong to it.
        if let Some(ref gid) = focusable_group_id {
            child_focusable_parent = Some(gid.as_str());
        }

        // Recurse into group children to find nested interactive elements.
        if let Some(children) = shape.get("children").and_then(|v| v.as_array()) {
            collect_interactive_elements(
                children,
                layer_name,
                group_matrix,
                group_clip,
                child_focusable_parent,
                out,
            );
        }
    }
}

/// Intersect two axis-aligned rectangles. Returns the intersection rect
/// as `(x, y, w, h)`. If the rectangles don't overlap, returns a
/// zero-area rect (w=0 or h=0).
fn intersect_rects(
    a: (f32, f32, f32, f32),
    b: (f32, f32, f32, f32),
) -> (f32, f32, f32, f32) {
    let x = a.0.max(b.0);
    let y = a.1.max(b.1);
    let w = ((a.0 + a.2).min(b.0 + b.2) - x).max(0.0);
    let h = ((a.1 + a.3).min(b.1 + b.3) - y).max(0.0);
    (x, y, w, h)
}

// offset_hit_region removed -- transforms are now handled by the
// TransformMatrix stored on each InteractiveElement. Hit regions
// stay in local coordinates; the inverse matrix maps cursor positions
// from canvas space to local space during hit testing.

/// Validate interactive elements and emit log warnings for common
/// accessibility issues. Called once per tree snapshot/patch.
fn validate_interactive_elements(canvas_id: &str, elements: &[InteractiveElement]) {
    for element in elements {
        // Interactive element without a11y metadata.
        if element.a11y.is_none() {
            log::warn!(
                "canvas '{}': interactive element '{}' has no a11y metadata; \
                 focusable but invisible to screen readers",
                canvas_id,
                element.id,
            );
        }

        if let Some(ref a11y) = element.a11y {
            // Switch without toggled state.
            if matches!(a11y.role, Some(iced::advanced::widget::operation::accessible::Role::Switch))
                && a11y.toggled.is_none()
            {
                log::warn!(
                    "canvas '{}': element '{}' has role 'switch' without 'toggled' state",
                    canvas_id,
                    element.id,
                );
            }
            // Radio without selected state.
            if matches!(a11y.role, Some(iced::advanced::widget::operation::accessible::Role::RadioButton))
                && a11y.selected.is_none()
            {
                log::warn!(
                    "canvas '{}': element '{}' has role 'radio' without 'selected' state",
                    canvas_id,
                    element.id,
                );
            }
            // Checkbox without toggled state.
            if matches!(a11y.role, Some(iced::advanced::widget::operation::accessible::Role::CheckBox))
                && a11y.toggled.is_none()
            {
                log::warn!(
                    "canvas '{}': element '{}' has role 'check_box' without 'toggled' state",
                    canvas_id,
                    element.id,
                );
            }
        }
    }

    // Multiple elements without position_in_set.
    let interactive_count = elements
        .iter()
        .filter(|e| e.parent_group.is_none())
        .count();
    if interactive_count > 1 {
        let missing_position = elements
            .iter()
            .filter(|e| e.parent_group.is_none())
            .filter(|e| {
                e.a11y
                    .as_ref()
                    .map(|a| a.position_in_set.is_none())
                    .unwrap_or(true)
            })
            .count();
        if missing_position == interactive_count {
            log::info!(
                "canvas '{}': {} interactive elements without position_in_set/size_of_set; \
                 consider adding set position for screen reader context",
                canvas_id,
                interactive_count,
            );
        }
    }
}

pub(crate) fn ensure_canvas_cache(node: &crate::protocol::TreeNode, caches: &mut WidgetCaches) {
    let props = node.props.as_object();
    // Build layer map: either from "layers" (object) or "shapes" (array -> single layer).
    let layer_map = canvas_layer_map(props);
    let node_caches = caches.canvas_caches.entry(node.id.clone()).or_default();

    // Parse interactive elements from all layers, recursing into groups.
    let mut interactive_elements = Vec::new();
    for (layer_name, shapes_val) in &layer_map {
        if let Some(shapes_arr) = shapes_val.as_array() {
            collect_interactive_elements(shapes_arr, layer_name, TransformMatrix::identity(), None, None, &mut interactive_elements);
        }
    }
    // A11y validation diagnostics (logged as warnings).
    validate_interactive_elements(&node.id, &interactive_elements);

    caches
        .canvas_interactions
        .insert(node.id.clone(), interactive_elements);

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

    // Standalone clip tests removed -- clips are now a group-level field.
    // See draw_with_group_clip() and the "clip" field on group JSON.

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
    fn parse_interactive_group_basic() {
        let shape = json!({
            "type": "group",
            "id": "bar-1",
            "on_click": true,
            "on_hover": true,
            "cursor": "pointer",
            "tooltip": "Bar 1: 200 units",
            "children": [
                {"type": "rect", "x": 10, "y": 20, "w": 30, "h": 40, "fill": "#ff0000"}
            ]
        });
        let result = parse_interactive_element(&shape, "default").unwrap();
        assert_eq!(result.id, "bar-1");
        assert!(result.on_click);
        assert!(result.on_hover);
        assert_eq!(result.cursor.as_deref(), Some("pointer"));
        assert_eq!(result.tooltip.as_deref(), Some("Bar 1: 200 units"));
        assert!(matches!(result.hit_region, HitRegion::Rect { .. }));
    }

    #[test]
    fn parse_interactive_group_with_drag() {
        let shape = json!({
            "type": "group",
            "id": "dot-1",
            "on_click": true,
            "draggable": true,
            "drag_axis": "x",
            "children": [
                {"type": "circle", "x": 50, "y": 50, "r": 20}
            ]
        });
        let result = parse_interactive_element(&shape, "layer1").unwrap();
        assert_eq!(result.id, "dot-1");
        assert!(result.draggable);
        assert_eq!(result.drag_axis, DragAxis::X);
        // Group hit region is a bounding box (Rect), not Circle.
        assert!(matches!(result.hit_region, HitRegion::Rect { .. }));
    }

    #[test]
    fn parse_interactive_group_with_hit_rect() {
        let shape = json!({
            "type": "group",
            "id": "path-group",
            "on_click": true,
            "hit_rect": {"x": 0, "y": 0, "w": 100, "h": 100},
            "children": [
                {"type": "path", "commands": [["move_to", 0, 0], ["line_to", 100, 100]]}
            ]
        });
        let result = parse_interactive_element(&shape, "default").unwrap();
        assert_eq!(result.id, "path-group");
        assert!(matches!(result.hit_region, HitRegion::Rect { .. }));
    }

    #[test]
    fn parse_interactive_missing_id_returns_none() {
        // Group without an id is not interactive.
        let shape = json!({
            "type": "group",
            "on_click": true,
            "children": [{"type": "rect", "x": 0, "y": 0, "w": 10, "h": 10}]
        });
        assert!(parse_interactive_element(&shape, "default").is_none());
    }

    #[test]
    fn parse_interactive_non_group_returns_none() {
        // Only groups can be interactive elements.
        let shape = json!({"type": "rect", "x": 0, "y": 0, "w": 10, "h": 10, "id": "r"});
        assert!(parse_interactive_element(&shape, "default").is_none());
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
        // Hit regions are in LOCAL coordinates (no group offset applied).
        let shape = json!({
            "type": "group",
            "id": "grp1", "on_click": true,
            "children": [
                {"type": "rect", "x": 0, "y": 0, "w": 100, "h": 40},
                {"type": "rect", "x": 10, "y": 50, "w": 80, "h": 20}
            ]
        });
        let region = compute_hit_region(&shape).unwrap();
        // Bounding box of children in local space: x=0..100, y=0..70.
        match region {
            HitRegion::Rect { x, y, w, h } => {
                assert!((x - 0.0).abs() < 0.01);
                assert!((y - 0.0).abs() < 0.01);
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
            "id": "grp2", "on_click": true,
            "children": [
                {"type": "rect", "x": 0, "y": 0, "w": 50, "h": 30},
                {"type": "circle", "x": 80, "y": 15, "r": 10}
            ]
        });
        let region = compute_hit_region(&shape).unwrap();
        // Rect: 0..50, 0..30; Circle: 70..90, 5..25
        // Union in local space: 0..90, 0..30
        match region {
            HitRegion::Rect { x, y, w, h } => {
                assert!((x - 0.0).abs() < 0.01);
                assert!((y - 0.0).abs() < 0.01);
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
            "id": "empty", "on_click": true,
            "children": []
        });
        assert!(compute_hit_region(&shape).is_none());
    }

    #[test]
    fn parse_interactive_group() {
        // New format: interactive fields at top level, no nested "interactive".
        let shape = json!({
            "type": "group",
            "id": "btn",
            "on_click": true,
            "on_hover": true,
            "cursor": "pointer",
            "a11y": {"role": "button", "label": "Save"},
            "children": [
                {"type": "rect", "x": 0, "y": 0, "w": 100, "h": 40, "fill": "#3498db"},
                {"type": "text", "x": 30, "y": 25, "content": "Save", "fill": "#ccc"}
            ]
        });
        let result = parse_interactive_element(&shape, "default").unwrap();
        assert_eq!(result.id, "btn");
        assert!(result.on_click);
        assert!(result.on_hover);
        assert_eq!(result.cursor.as_deref(), Some("pointer"));
        assert!(result.a11y.is_some());
        assert!(result.show_focus_ring); // default true
        assert!(!result.focusable); // default false
        match result.hit_region {
            HitRegion::Rect { x, y, w, h } => {
                // Bounding box in local space. Union of:
                //   rect child: (0, 0) -> (100, 40)
                //   text child: (30, 9) -> (60.4, 25) (estimated)
                // Result: (0, 0) -> (100, 40)
                assert!((x - 0.0).abs() < 0.01, "x={x}");
                assert!((y - 0.0).abs() < 0.01, "y={y}");
                assert!((w - 100.0).abs() < 0.01, "w={w}");
                assert!((h - 40.0).abs() < 0.01, "h={h}");
            }
            other => panic!("expected Rect, got {other:?}"),
        }
    }

    #[test]
    fn parse_interactive_element_skips_non_groups() {
        let shape = json!({
            "type": "rect", "x": 0, "y": 0, "w": 100, "h": 40,
            "id": "rect-btn", "on_click": true
        });
        assert!(parse_interactive_element(&shape, "default").is_none());
    }

    #[test]
    fn parse_interactive_group_with_new_fields() {
        let shape = json!({
            "type": "group",
            "id": "toggle",
            "on_click": true,
            "focus_style": {"stroke": "#3b82f6"},
            "show_focus_ring": false,
            "focusable": true,
            "children": [
                {"type": "rect", "x": 0, "y": 0, "w": 60, "h": 30}
            ]
        });
        let result = parse_interactive_element(&shape, "default").unwrap();
        assert_eq!(result.id, "toggle");
        assert!(result.has_focus_style);
        assert!(!result.show_focus_ring);
        assert!(result.focusable);
    }

    #[test]
    fn group_translation_from_transforms() {
        let group = json!({
            "type": "group",
            "transforms": [
                {"type": "translate", "x": 50.0, "y": 30.0},
                {"type": "rotate", "angle": 0.5},
                {"type": "translate", "x": 10.0, "y": 0.0}
            ],
            "children": []
        });
        let (tx, ty) = group_translation(&group);
        assert!((tx - 60.0).abs() < 0.01);
        assert!((ty - 30.0).abs() < 0.01);
    }

    #[test]
    fn group_translation_no_transforms() {
        let group = json!({"type": "group", "children": []});
        let (tx, ty) = group_translation(&group);
        assert_eq!(tx, 0.0);
        assert_eq!(ty, 0.0);
    }

    #[test]
    fn collect_interactive_elements_recurses_into_groups() {
        let shapes = vec![
            // Non-group shapes are skipped, even with an id.
            json!({
                "type": "rect", "x": 0, "y": 0, "w": 10, "h": 10,
                "id": "top-rect", "on_click": true
            }),
            // Interactive group with a nested interactive group.
            json!({
                "type": "group",
                "id": "grp", "on_click": true,
                "children": [
                    {"type": "rect", "x": 0, "y": 0, "w": 50, "h": 50},
                    {
                        "type": "group",
                        "transforms": [{"type": "translate", "x": 10, "y": 10}],
                        "id": "nested-grp", "on_click": true,
                        "children": [
                            {"type": "circle", "x": 5, "y": 5, "r": 5}
                        ]
                    }
                ]
            }),
        ];
        let mut result = Vec::new();
        collect_interactive_elements(&shapes, "default", TransformMatrix::identity(), None, None, &mut result);
        let ids: Vec<&str> = result.iter().map(|s| s.id.as_str()).collect();
        // Non-group "top-rect" is NOT collected.
        assert!(!ids.contains(&"top-rect"));
        // Both groups are collected.
        assert!(ids.contains(&"grp"));
        assert!(ids.contains(&"nested-grp"));
    }

    #[test]
    fn path_bounds_computes_from_commands() {
        let shape = json!({
            "type": "path",
            "commands": [
                ["move_to", 0.0, -12.0],
                ["line_to", 11.4, -3.7],
                ["line_to", 7.0, 9.7],
                ["line_to", -7.0, 9.7],
                ["line_to", -11.4, -3.7],
                "close"
            ]
        });
        let bounds = path_bounds(&shape).unwrap();
        assert!(bounds.0 < -11.0); // min_x
        assert!(bounds.1 < -11.0); // min_y
        assert!(bounds.2 > 11.0); // max_x
        assert!(bounds.3 > 9.0); // max_y
    }

    #[test]
    fn interactive_group_with_path_child_gets_hit_region() {
        let shape = json!({
            "type": "group",
            "id": "star", "on_click": true,
            "children": [
                {
                    "type": "path",
                    "commands": [
                        ["move_to", 0.0, -12.0],
                        ["line_to", 11.4, -3.7],
                        ["line_to", 7.0, 9.7],
                        ["line_to", -7.0, 9.7],
                        ["line_to", -11.4, -3.7],
                        "close"
                    ],
                    "fill": "#ff0000"
                }
            ]
        });
        let region = compute_hit_region(&shape);
        assert!(
            region.is_some(),
            "group with path child should have a hit region"
        );
    }

    #[test]
    fn hit_rect_on_group_is_local_coordinates() {
        // hit_rect is in local coordinates -- no transform offset applied.
        // Transform offsets are handled by the transform matrix during hit testing.
        let shape = json!({
            "type": "group",
            "id": "star", "on_click": true,
            "hit_rect": {"x": -12.0, "y": -12.0, "w": 28.0, "h": 28.0},
            "children": [
                {"type": "rect", "x": 0, "y": 0, "w": 10, "h": 10}
            ]
        });
        let region = compute_hit_region(&shape).unwrap();
        match region {
            HitRegion::Rect { x, y, w, h } => {
                // Local coordinates, no offset.
                assert!((x - (-12.0)).abs() < 0.01, "x should be -12, got {x}");
                assert!((y - (-12.0)).abs() < 0.01, "y should be -12, got {y}");
                assert!((w - 28.0).abs() < 0.01);
                assert!((h - 28.0).abs() < 0.01);
            }
            other => panic!("expected Rect, got {other:?}"),
        }
    }

    // -- TransformMatrix tests --

    #[test]
    fn transform_identity() {
        let m = TransformMatrix::identity();
        let (x, y) = m.transform_point(10.0, 20.0);
        assert!((x - 10.0).abs() < 0.001);
        assert!((y - 20.0).abs() < 0.001);
    }

    #[test]
    fn transform_translate() {
        let m = TransformMatrix::identity().translate(50.0, 30.0);
        let (x, y) = m.transform_point(10.0, 20.0);
        assert!((x - 60.0).abs() < 0.001);
        assert!((y - 50.0).abs() < 0.001);
    }

    #[test]
    fn transform_rotate_90() {
        let m = TransformMatrix::identity().rotate(std::f32::consts::FRAC_PI_2);
        // (10, 0) rotated 90 degrees CCW -> (0, 10)
        let (x, y) = m.transform_point(10.0, 0.0);
        assert!(x.abs() < 0.001, "x should be ~0, got {x}");
        assert!((y - 10.0).abs() < 0.001, "y should be ~10, got {y}");
    }

    #[test]
    fn transform_scale() {
        let m = TransformMatrix::identity().scale(2.0, 3.0);
        let (x, y) = m.transform_point(10.0, 20.0);
        assert!((x - 20.0).abs() < 0.001);
        assert!((y - 60.0).abs() < 0.001);
    }

    #[test]
    fn transform_inverse_roundtrip() {
        let m = TransformMatrix::identity()
            .translate(50.0, 30.0)
            .rotate(0.5)
            .scale(2.0, 1.5);
        let inv = m.inverse().unwrap();
        // Forward then inverse should return to original.
        let (fx, fy) = m.transform_point(10.0, 20.0);
        let (rx, ry) = inv.transform_point(fx, fy);
        assert!((rx - 10.0).abs() < 0.01, "roundtrip x: expected 10, got {rx}");
        assert!((ry - 20.0).abs() < 0.01, "roundtrip y: expected 20, got {ry}");
    }

    #[test]
    fn transform_singular_has_no_inverse() {
        // Scale to zero on one axis -> singular.
        let m = TransformMatrix::identity().scale(0.0, 1.0);
        assert!(m.inverse().is_none());
    }

    #[test]
    fn transform_from_json() {
        let transforms = vec![
            json!({"type": "translate", "x": 50.0, "y": 30.0}),
            json!({"type": "rotate", "angle": 0.0}),
            json!({"type": "scale", "factor": 2.0}),
        ];
        let m = TransformMatrix::from_transforms(&transforms);
        // translate(50,30) then scale(2) -> point (10,20) becomes (50+10*2, 30+20*2) = (70, 70)
        let (x, y) = m.transform_point(10.0, 20.0);
        assert!((x - 70.0).abs() < 0.01, "x={x}");
        assert!((y - 70.0).abs() < 0.01, "y={y}");
    }

    #[test]
    fn find_hit_element_with_transform() {
        // Group at (100, 100) with a 50x50 rect child.
        // Cursor at canvas (125, 125) should hit (local 25, 25).
        let shapes = vec![json!({
            "type": "group",
            "id": "btn",
            "on_click": true,
            "transforms": [{"type": "translate", "x": 100, "y": 100}],
            "children": [{"type": "rect", "x": 0, "y": 0, "w": 50, "h": 50}]
        })];
        let mut elements = Vec::new();
        collect_interactive_elements(
            &shapes, "default", TransformMatrix::identity(), None, None, &mut elements,
        );
        assert_eq!(elements.len(), 1);

        // Hit inside.
        let hit = find_hit_element(Point::new(125.0, 125.0), &elements);
        assert!(hit.is_some(), "should hit at (125, 125)");

        // Miss outside.
        let miss = find_hit_element(Point::new(50.0, 50.0), &elements);
        assert!(miss.is_none(), "should miss at (50, 50)");
    }

    #[test]
    fn find_hit_element_with_rotation() {
        // Group rotated 45 degrees, with a 100x20 rect at origin.
        // The rect covers a diagonal strip in canvas space.
        let shapes = vec![json!({
            "type": "group",
            "id": "rotated",
            "on_click": true,
            "transforms": [{"type": "rotate", "angle": 0.7854}],
            "children": [{"type": "rect", "x": 0, "y": 0, "w": 100, "h": 20}]
        })];
        let mut elements = Vec::new();
        collect_interactive_elements(
            &shapes, "default", TransformMatrix::identity(), None, None, &mut elements,
        );

        // Canvas point (35, 35) -> local via inverse rotate(-45deg):
        //   local_x = cos(-45)*35 + (-sin(-45))*35 = 0.707*35 + 0.707*35 ~ 49.5
        //   local_y = sin(-45)*35 + cos(-45)*35 = -0.707*35 + 0.707*35 ~ 0
        // local (49.5, 0) is inside rect (0,0,100,20). Should hit.
        let hit = find_hit_element(Point::new(35.0, 35.0), &elements);
        assert!(hit.is_some(), "should hit along the rotated diagonal");

        // Canvas point (0, 80) -> local:
        //   local_x = 0.707*0 + 0.707*80 ~ 56.6
        //   local_y = -0.707*0 + 0.707*80 ~ 56.6
        // local (56.6, 56.6) is outside rect height 20. Should miss.
        let miss = find_hit_element(Point::new(0.0, 80.0), &elements);
        assert!(miss.is_none(), "should miss far from diagonal");
    }

    #[test]
    fn find_hit_element_respects_clip() {
        // Group at (0,0) with a 100x100 rect, but clipped to a 50x50 region.
        let shapes = vec![json!({
            "type": "group",
            "id": "clipped",
            "on_click": true,
            "clip": {"x": 0, "y": 0, "w": 50, "h": 50},
            "children": [{"type": "rect", "x": 0, "y": 0, "w": 100, "h": 100}]
        })];
        let mut elements = Vec::new();
        collect_interactive_elements(
            &shapes, "default", TransformMatrix::identity(), None, None, &mut elements,
        );

        // Inside clip region.
        let hit = find_hit_element(Point::new(25.0, 25.0), &elements);
        assert!(hit.is_some(), "should hit inside clip");

        // Inside hit region but outside clip.
        let miss = find_hit_element(Point::new(75.0, 75.0), &elements);
        assert!(miss.is_none(), "should miss outside clip despite being in hit region");
    }

    #[test]
    fn intersect_rects_overlap() {
        let r = intersect_rects((0.0, 0.0, 100.0, 100.0), (50.0, 50.0, 100.0, 100.0));
        assert!((r.0 - 50.0).abs() < 0.01);
        assert!((r.1 - 50.0).abs() < 0.01);
        assert!((r.2 - 50.0).abs() < 0.01);
        assert!((r.3 - 50.0).abs() < 0.01);
    }

    #[test]
    fn intersect_rects_no_overlap() {
        let r = intersect_rects((0.0, 0.0, 10.0, 10.0), (20.0, 20.0, 10.0, 10.0));
        assert!(r.2 == 0.0 || r.3 == 0.0, "no-overlap should have zero area");
    }

    #[test]
    fn collect_nested_transform_accumulates() {
        // Outer group translated (100, 0), inner group translated (0, 50).
        // Inner element should have accumulated transform (100, 50).
        let shapes = vec![json!({
            "type": "group",
            "transforms": [{"type": "translate", "x": 100, "y": 0}],
            "children": [{
                "type": "group",
                "id": "inner",
                "on_click": true,
                "transforms": [{"type": "translate", "x": 0, "y": 50}],
                "children": [{"type": "rect", "x": 0, "y": 0, "w": 10, "h": 10}]
            }]
        })];
        let mut elements = Vec::new();
        collect_interactive_elements(
            &shapes, "default", TransformMatrix::identity(), None, None, &mut elements,
        );
        assert_eq!(elements.len(), 1);
        let e = &elements[0];
        // The transform should map (0,0) to (100, 50).
        let (tx, ty) = e.transform.transform_point(0.0, 0.0);
        assert!((tx - 100.0).abs() < 0.01, "tx={tx}");
        assert!((ty - 50.0).abs() < 0.01, "ty={ty}");
    }

    // -- TransformMatrix::compose tests --

    #[test]
    fn compose_identity_is_noop() {
        let m = TransformMatrix::identity().translate(10.0, 20.0);
        let id = TransformMatrix::identity();
        let composed = m.compose(&id);
        let (x, y) = composed.transform_point(0.0, 0.0);
        assert!((x - 10.0).abs() < 0.001);
        assert!((y - 20.0).abs() < 0.001);
    }

    #[test]
    fn compose_translate_then_scale() {
        let parent = TransformMatrix::identity().translate(100.0, 0.0);
        let local = TransformMatrix::identity().scale(2.0, 2.0);
        let composed = parent.compose(&local);
        // Point (5, 5) -> scale(2,2) -> (10, 10) -> translate(100, 0) -> (110, 10)
        let (x, y) = composed.transform_point(5.0, 5.0);
        assert!((x - 110.0).abs() < 0.01, "x={x}");
        assert!((y - 10.0).abs() < 0.01, "y={y}");
    }

    #[test]
    fn compose_matches_chained_transforms() {
        // compose(parent, local) should equal parent.translate().rotate()
        // when local = translate then rotate
        let parent = TransformMatrix::identity().translate(50.0, 0.0);
        let local = TransformMatrix::identity()
            .translate(10.0, 0.0)
            .rotate(std::f32::consts::FRAC_PI_2);
        let composed = parent.compose(&local);
        let chained = TransformMatrix::identity()
            .translate(50.0, 0.0)
            .translate(10.0, 0.0)
            .rotate(std::f32::consts::FRAC_PI_2);
        // Both should transform (1, 0) the same way.
        let (cx, cy) = composed.transform_point(1.0, 0.0);
        let (sx, sy) = chained.transform_point(1.0, 0.0);
        assert!((cx - sx).abs() < 0.01, "x: composed={cx}, chained={sx}");
        assert!((cy - sy).abs() < 0.01, "y: composed={cy}, chained={sy}");
    }

    // -- hit_test epsilon tolerance --

    #[test]
    fn hit_test_rect_boundary_with_epsilon() {
        // Point exactly on the boundary should hit (within epsilon).
        let region = HitRegion::Rect { x: 0.0, y: 0.0, w: 100.0, h: 50.0 };
        assert!(hit_test(Point::new(0.0, 0.0), &region));
        assert!(hit_test(Point::new(100.0, 50.0), &region));
        // Slightly outside but within epsilon (0.5px).
        assert!(hit_test(Point::new(-0.3, -0.3), &region));
        assert!(hit_test(Point::new(100.3, 50.3), &region));
        // Beyond epsilon.
        assert!(!hit_test(Point::new(-1.0, 0.0), &region));
        assert!(!hit_test(Point::new(0.0, -1.0), &region));
    }

    // -- Clip inheritance through nested groups --

    #[test]
    fn nested_clip_is_intersected() {
        // Outer group clips to (0,0,100,100).
        // Inner group clips to (50,50,100,100).
        // Effective clip should be intersection: (50,50,50,50).
        let shapes = vec![json!({
            "type": "group",
            "clip": {"x": 0, "y": 0, "w": 100, "h": 100},
            "children": [{
                "type": "group",
                "id": "inner",
                "on_click": true,
                "clip": {"x": 50, "y": 50, "w": 100, "h": 100},
                "children": [{"type": "rect", "x": 0, "y": 0, "w": 200, "h": 200}]
            }]
        })];
        let mut elements = Vec::new();
        collect_interactive_elements(
            &shapes, "default", TransformMatrix::identity(), None, None, &mut elements,
        );
        assert_eq!(elements.len(), 1);
        let clip = elements[0].clip_rect.unwrap();
        // Intersection of (0,0,100,100) and (50,50,100,100) = (50,50,50,50).
        assert!((clip.0 - 50.0).abs() < 0.01, "clip x={}", clip.0);
        assert!((clip.1 - 50.0).abs() < 0.01, "clip y={}", clip.1);
        assert!((clip.2 - 50.0).abs() < 0.01, "clip w={}", clip.2);
        assert!((clip.3 - 50.0).abs() < 0.01, "clip h={}", clip.3);
    }

    #[test]
    fn clip_from_parent_propagates_to_child() {
        // Parent has a clip, child has none.
        // Child should inherit the parent's clip.
        let shapes = vec![json!({
            "type": "group",
            "clip": {"x": 10, "y": 10, "w": 80, "h": 80},
            "children": [{
                "type": "group",
                "id": "child",
                "on_click": true,
                "children": [{"type": "rect", "x": 0, "y": 0, "w": 100, "h": 100}]
            }]
        })];
        let mut elements = Vec::new();
        collect_interactive_elements(
            &shapes, "default", TransformMatrix::identity(), None, None, &mut elements,
        );
        assert_eq!(elements.len(), 1);
        let clip = elements[0].clip_rect.unwrap();
        assert!((clip.0 - 10.0).abs() < 0.01);
        assert!((clip.1 - 10.0).abs() < 0.01);
        assert!((clip.2 - 80.0).abs() < 0.01);
        assert!((clip.3 - 80.0).abs() < 0.01);
    }

    #[test]
    fn no_clip_means_no_clip_rect() {
        let shapes = vec![json!({
            "type": "group",
            "id": "noclip",
            "on_click": true,
            "children": [{"type": "rect", "x": 0, "y": 0, "w": 50, "h": 50}]
        })];
        let mut elements = Vec::new();
        collect_interactive_elements(
            &shapes, "default", TransformMatrix::identity(), None, None, &mut elements,
        );
        assert!(elements[0].clip_rect.is_none());
    }

    // -- set_focus and resolve_focus_index --

    /// Helper to build a minimal CanvasProgram for state-machine tests.
    fn test_program(elements: &[InteractiveElement]) -> CanvasProgram<'_> {
        static IMAGES: std::sync::LazyLock<crate::image_registry::ImageRegistry> =
            std::sync::LazyLock::new(crate::image_registry::ImageRegistry::new);
        CanvasProgram {
            layers: vec![],
            caches: None,
            background: None,
            id: "test-canvas".to_string(),
            on_press: false,
            on_release: false,
            on_move: false,
            on_scroll: false,
            images: &IMAGES,
            interactive_elements: elements,
            arrow_mode: ArrowMode::Wrap,
            pending_focus: None,
        }
    }

    /// Helper to build a minimal InteractiveElement.
    fn test_element(id: &str) -> InteractiveElement {
        InteractiveElement {
            id: id.to_string(),
            layer: "default".to_string(),
            hit_region: HitRegion::Rect { x: 0.0, y: 0.0, w: 10.0, h: 10.0 },
            transform: TransformMatrix::identity(),
            inverse_transform: Some(TransformMatrix::identity()),
            clip_rect: None,
            on_click: true,
            on_hover: false,
            draggable: false,
            drag_axis: DragAxis::Both,
            drag_bounds: None,
            cursor: None,
            has_hover_style: false,
            has_pressed_style: false,
            has_focus_style: false,
            show_focus_ring: true,
            focus_ring_radius: None,
            focusable: false,
            parent_group: None,
            tooltip: None,
            a11y: None,
        }
    }

    #[test]
    fn resolve_focus_index_finds_element() {
        let elements = vec![test_element("a"), test_element("b"), test_element("c")];
        let program = test_program(&elements);
        let state = CanvasState {
            cursor_position: None,
            hovered_element: None,
            pressed_element: None,
            dragging: None,
            focused_id: Some("b".to_string()),
            focused_group: None,
            last_consumed_pending: None,
            canvas_focused: false,
            focus_visible: false,
        };
        assert_eq!(program.resolve_focus_index(&state), Some(1));
    }

    #[test]
    fn resolve_focus_index_returns_none_for_missing() {
        let elements = vec![test_element("a"), test_element("b")];
        let program = test_program(&elements);
        let state = CanvasState {
            cursor_position: None,
            hovered_element: None,
            pressed_element: None,
            dragging: None,
            focused_id: Some("deleted".to_string()),
            focused_group: None,
            last_consumed_pending: None,
            canvas_focused: false,
            focus_visible: false,
        };
        assert_eq!(program.resolve_focus_index(&state), None);
    }

    #[test]
    fn resolve_focus_index_returns_none_when_unfocused() {
        let elements = vec![test_element("a")];
        let program = test_program(&elements);
        let state = CanvasState {
            cursor_position: None,
            hovered_element: None,
            pressed_element: None,
            dragging: None,
            focused_id: None,
            focused_group: None,
            last_consumed_pending: None,
            canvas_focused: false,
            focus_visible: false,
        };
        assert_eq!(program.resolve_focus_index(&state), None);
    }

    #[test]
    fn set_focus_from_none_to_element() {
        let elements = vec![test_element("a"), test_element("b")];
        let program = test_program(&elements);
        let mut state = CanvasState {
            cursor_position: None,
            hovered_element: None,
            pressed_element: None,
            dragging: None,
            focused_id: None,
            focused_group: None,
            last_consumed_pending: None,
            canvas_focused: false,
            focus_visible: false,
        };
        let msg = program.set_focus(&mut state, Some(1));
        assert!(msg.is_some());
        assert_eq!(state.focused_id, Some("b".to_string()));
        match msg.unwrap() {
            Message::CanvasElementFocusChanged {
                old_element_id,
                new_element_id,
                ..
            } => {
                assert_eq!(old_element_id, None);
                assert_eq!(new_element_id, Some("b".to_string()));
            }
            other => panic!("expected FocusChanged, got {other:?}"),
        }
    }

    #[test]
    fn set_focus_between_elements() {
        let elements = vec![test_element("a"), test_element("b")];
        let program = test_program(&elements);
        let mut state = CanvasState {
            cursor_position: None,
            hovered_element: None,
            pressed_element: None,
            dragging: None,
            focused_id: Some("a".to_string()),
            focused_group: None,
            last_consumed_pending: None,
            canvas_focused: false,
            focus_visible: false,
        };
        let msg = program.set_focus(&mut state, Some(1));
        assert!(msg.is_some());
        assert_eq!(state.focused_id, Some("b".to_string()));
        match msg.unwrap() {
            Message::CanvasElementFocusChanged {
                old_element_id,
                new_element_id,
                ..
            } => {
                assert_eq!(old_element_id, Some("a".to_string()));
                assert_eq!(new_element_id, Some("b".to_string()));
            }
            other => panic!("expected FocusChanged, got {other:?}"),
        }
    }

    #[test]
    fn set_focus_to_same_is_noop() {
        let elements = vec![test_element("a"), test_element("b")];
        let program = test_program(&elements);
        let mut state = CanvasState {
            cursor_position: None,
            hovered_element: None,
            pressed_element: None,
            dragging: None,
            focused_id: Some("a".to_string()),
            focused_group: None,
            last_consumed_pending: None,
            canvas_focused: false,
            focus_visible: false,
        };
        let msg = program.set_focus(&mut state, Some(0));
        assert!(msg.is_none());
        assert_eq!(state.focused_id, Some("a".to_string()));
    }

    #[test]
    fn set_focus_clear() {
        let elements = vec![test_element("a")];
        let program = test_program(&elements);
        let mut state = CanvasState {
            cursor_position: None,
            hovered_element: None,
            pressed_element: None,
            dragging: None,
            focused_id: Some("a".to_string()),
            focused_group: None,
            last_consumed_pending: None,
            canvas_focused: false,
            focus_visible: false,
        };
        let msg = program.set_focus(&mut state, None);
        assert!(msg.is_some());
        assert_eq!(state.focused_id, None);
        match msg.unwrap() {
            Message::CanvasElementFocusChanged {
                old_element_id,
                new_element_id,
                ..
            } => {
                assert_eq!(old_element_id, Some("a".to_string()));
                assert_eq!(new_element_id, None);
            }
            other => panic!("expected FocusChanged, got {other:?}"),
        }
    }

    #[test]
    fn set_focus_clear_when_already_none() {
        let elements = vec![test_element("a")];
        let program = test_program(&elements);
        let mut state = CanvasState {
            cursor_position: None,
            hovered_element: None,
            pressed_element: None,
            dragging: None,
            focused_id: None,
            focused_group: None,
            last_consumed_pending: None,
            canvas_focused: false,
            focus_visible: false,
        };
        let msg = program.set_focus(&mut state, None);
        assert!(msg.is_none());
    }

    #[test]
    fn set_focus_out_of_bounds_clears() {
        let elements = vec![test_element("a")];
        let program = test_program(&elements);
        let mut state = CanvasState {
            cursor_position: None,
            hovered_element: None,
            pressed_element: None,
            dragging: None,
            focused_id: Some("a".to_string()),
            focused_group: None,
            last_consumed_pending: None,
            canvas_focused: false,
            focus_visible: false,
        };
        // Index 99 is out of bounds -> new_id becomes None -> blur.
        let msg = program.set_focus(&mut state, Some(99));
        assert!(msg.is_some());
        assert_eq!(state.focused_id, None);
    }

    // -- layers_with_active_interaction --

    #[test]
    fn layers_active_hover_style() {
        let mut elements = vec![test_element("btn")];
        elements[0].layer = "ui".to_string();
        elements[0].has_hover_style = true;
        let program = test_program(&elements);
        let state = CanvasState {
            cursor_position: None,
            hovered_element: Some("btn".to_string()),
            pressed_element: None,
            dragging: None,
            focused_id: None,
            focused_group: None,
            last_consumed_pending: None,
            canvas_focused: false,
            focus_visible: false,
        };
        let layers = program.layers_with_active_interaction(&state);
        assert_eq!(layers, vec!["ui"]);
    }

    #[test]
    fn layers_active_focus_style() {
        let mut elements = vec![test_element("btn")];
        elements[0].layer = "ui".to_string();
        elements[0].has_focus_style = true;
        let program = test_program(&elements);
        let state = CanvasState {
            cursor_position: None,
            hovered_element: None,
            pressed_element: None,
            dragging: None,
            focused_id: Some("btn".to_string()),
            focused_group: None,
            last_consumed_pending: None,
            canvas_focused: true,
            focus_visible: true,
        };
        let layers = program.layers_with_active_interaction(&state);
        assert_eq!(layers, vec!["ui"]);
    }

    #[test]
    fn layers_active_hover_and_focus_on_different_layers() {
        let mut hover_elem = test_element("hover-btn");
        hover_elem.layer = "layer-a".to_string();
        hover_elem.has_hover_style = true;
        let mut focus_elem = test_element("focus-btn");
        focus_elem.layer = "layer-b".to_string();
        focus_elem.has_focus_style = true;
        let elements = vec![hover_elem, focus_elem];
        let program = test_program(&elements);
        let state = CanvasState {
            cursor_position: None,
            hovered_element: Some("hover-btn".to_string()),
            pressed_element: None,
            dragging: None,
            focused_id: Some("focus-btn".to_string()),
            focused_group: None,
            last_consumed_pending: None,
            canvas_focused: true,
            focus_visible: true,
        };
        let layers = program.layers_with_active_interaction(&state);
        assert_eq!(layers.len(), 2);
        assert!(layers.contains(&"layer-a".to_string()));
        assert!(layers.contains(&"layer-b".to_string()));
    }

    #[test]
    fn layers_active_hover_and_focus_on_same_layer_no_dupe() {
        let mut hover_elem = test_element("hover-btn");
        hover_elem.layer = "ui".to_string();
        hover_elem.has_hover_style = true;
        let mut focus_elem = test_element("focus-btn");
        focus_elem.layer = "ui".to_string();
        focus_elem.has_focus_style = true;
        let elements = vec![hover_elem, focus_elem];
        let program = test_program(&elements);
        let state = CanvasState {
            cursor_position: None,
            hovered_element: Some("hover-btn".to_string()),
            pressed_element: None,
            dragging: None,
            focused_id: Some("focus-btn".to_string()),
            focused_group: None,
            last_consumed_pending: None,
            canvas_focused: true,
            focus_visible: true,
        };
        let layers = program.layers_with_active_interaction(&state);
        // Should not duplicate "ui".
        assert_eq!(layers, vec!["ui"]);
    }

    #[test]
    fn layers_active_no_style_returns_empty() {
        let elements = vec![test_element("btn")]; // no style flags
        let program = test_program(&elements);
        let state = CanvasState {
            cursor_position: None,
            hovered_element: Some("btn".to_string()),
            pressed_element: None,
            dragging: None,
            focused_id: None,
            focused_group: None,
            last_consumed_pending: None,
            canvas_focused: false,
            focus_visible: false,
        };
        let layers = program.layers_with_active_interaction(&state);
        assert!(layers.is_empty());
    }

    // -- find_hit_element edge cases --

    #[test]
    fn find_hit_element_empty_list() {
        assert!(find_hit_element(Point::new(0.0, 0.0), &[]).is_none());
    }

    #[test]
    fn find_hit_element_singular_transform_not_hittable() {
        // Element with scale(0, 1) -> singular matrix -> inverse is None.
        let shapes = vec![json!({
            "type": "group",
            "id": "collapsed",
            "on_click": true,
            "transforms": [{"type": "scale", "x": 0, "y": 1}],
            "children": [{"type": "rect", "x": 0, "y": 0, "w": 100, "h": 100}]
        })];
        let mut elements = Vec::new();
        collect_interactive_elements(
            &shapes, "default", TransformMatrix::identity(), None, None, &mut elements,
        );
        assert_eq!(elements.len(), 1);
        assert!(elements[0].inverse_transform.is_none());
        // Can't hit an element with a singular transform.
        let hit = find_hit_element(Point::new(50.0, 50.0), &elements);
        assert!(hit.is_none());
    }

    #[test]
    fn find_hit_element_topmost_wins() {
        // Two overlapping elements. Last in list = topmost = tested first.
        let mut a = test_element("bottom");
        a.hit_region = HitRegion::Rect { x: 0.0, y: 0.0, w: 100.0, h: 100.0 };
        let mut b = test_element("top");
        b.hit_region = HitRegion::Rect { x: 0.0, y: 0.0, w: 100.0, h: 100.0 };
        let elements = vec![a, b];
        let hit = find_hit_element(Point::new(50.0, 50.0), &elements).unwrap();
        assert_eq!(hit.id, "top");
    }

    #[test]
    fn find_hit_element_skips_non_interactive() {
        // Element with on_click=false, on_hover=false, draggable=false.
        let mut elem = test_element("passive");
        elem.on_click = false;
        elem.hit_region = HitRegion::Rect { x: 0.0, y: 0.0, w: 100.0, h: 100.0 };
        let elements = vec![elem];
        let hit = find_hit_element(Point::new(50.0, 50.0), &elements);
        assert!(hit.is_none());
    }

    // -- Transformed clip test --

    #[test]
    fn clip_transformed_by_group_matrix() {
        // Group at translated (100, 100) with clip (0,0,50,50).
        // In canvas space, the clip should be at (100,100,50,50).
        let shapes = vec![json!({
            "type": "group",
            "id": "shifted-clip",
            "on_click": true,
            "transforms": [{"type": "translate", "x": 100, "y": 100}],
            "clip": {"x": 0, "y": 0, "w": 50, "h": 50},
            "children": [{"type": "rect", "x": 0, "y": 0, "w": 100, "h": 100}]
        })];
        let mut elements = Vec::new();
        collect_interactive_elements(
            &shapes, "default", TransformMatrix::identity(), None, None, &mut elements,
        );
        let clip = elements[0].clip_rect.unwrap();
        assert!((clip.0 - 100.0).abs() < 0.01, "clip x={}", clip.0);
        assert!((clip.1 - 100.0).abs() < 0.01, "clip y={}", clip.1);
        assert!((clip.2 - 50.0).abs() < 0.01, "clip w={}", clip.2);
        assert!((clip.3 - 50.0).abs() < 0.01, "clip h={}", clip.3);

        // Hit inside clip (canvas 125, 125 -> local 25, 25 -> in rect).
        let hit = find_hit_element(Point::new(125.0, 125.0), &elements);
        assert!(hit.is_some());

        // Hit outside clip but inside transformed rect.
        let miss = find_hit_element(Point::new(175.0, 175.0), &elements);
        assert!(miss.is_none());
    }

    // -- TransformMatrix::decompose tests --

    #[test]
    fn decompose_identity() {
        let (tx, ty, angle, sx, sy) = TransformMatrix::identity().decompose();
        assert!((tx).abs() < 0.001);
        assert!((ty).abs() < 0.001);
        assert!((angle).abs() < 0.001);
        assert!((sx - 1.0).abs() < 0.001);
        assert!((sy - 1.0).abs() < 0.001);
    }

    #[test]
    fn decompose_translate_only() {
        let m = TransformMatrix::identity().translate(42.0, -17.0);
        let (tx, ty, angle, sx, sy) = m.decompose();
        assert!((tx - 42.0).abs() < 0.001);
        assert!((ty - (-17.0)).abs() < 0.001);
        assert!((angle).abs() < 0.001);
        assert!((sx - 1.0).abs() < 0.001);
        assert!((sy - 1.0).abs() < 0.001);
    }

    #[test]
    fn decompose_rotate_only() {
        let angle_in = 0.7;
        let m = TransformMatrix::identity().rotate(angle_in);
        let (tx, ty, angle, sx, sy) = m.decompose();
        assert!((tx).abs() < 0.001);
        assert!((ty).abs() < 0.001);
        assert!((angle - angle_in).abs() < 0.001, "angle={angle}");
        assert!((sx - 1.0).abs() < 0.001);
        assert!((sy - 1.0).abs() < 0.001);
    }

    #[test]
    fn decompose_scale_only() {
        let m = TransformMatrix::identity().scale(3.0, 0.5);
        let (tx, ty, angle, sx, sy) = m.decompose();
        assert!((tx).abs() < 0.001);
        assert!((ty).abs() < 0.001);
        assert!((angle).abs() < 0.001);
        assert!((sx - 3.0).abs() < 0.001, "sx={sx}");
        assert!((sy - 0.5).abs() < 0.001, "sy={sy}");
    }

    #[test]
    fn decompose_roundtrip() {
        // Build a complex matrix, decompose it, rebuild, and verify
        // the same point is transformed identically.
        let original = TransformMatrix::identity()
            .translate(50.0, 30.0)
            .rotate(0.5)
            .scale(2.0, 1.5);
        let (tx, ty, angle, sx, sy) = original.decompose();
        let rebuilt = TransformMatrix::identity()
            .translate(tx, ty)
            .rotate(angle)
            .scale(sx, sy);

        // Test several points.
        for &(px, py) in &[(0.0, 0.0), (10.0, 20.0), (-5.0, 15.0)] {
            let (ox, oy) = original.transform_point(px, py);
            let (rx, ry) = rebuilt.transform_point(px, py);
            assert!(
                (ox - rx).abs() < 0.1 && (oy - ry).abs() < 0.1,
                "point ({px},{py}): original=({ox},{oy}), rebuilt=({rx},{ry})"
            );
        }
    }

    // draw_focus_ring is tested visually (requires a real Renderer).
    // The decompose + apply_to_frame math is verified by the decompose
    // tests above -- the draw function is straightforward path construction
    // delegating to iced's Path::rounded_rectangle / Path::circle.

    // -- ArrowMode tests --

    #[test]
    fn arrow_mode_from_str_known_values() {
        assert_eq!(ArrowMode::from_str("wrap"), ArrowMode::Wrap);
        assert_eq!(ArrowMode::from_str("clamp"), ArrowMode::Clamp);
        assert_eq!(ArrowMode::from_str("linear"), ArrowMode::Linear);
        assert_eq!(ArrowMode::from_str("none"), ArrowMode::None);
    }

    #[test]
    fn arrow_mode_from_str_unknown_defaults_to_wrap() {
        assert_eq!(ArrowMode::from_str("invalid"), ArrowMode::Wrap);
        assert_eq!(ArrowMode::from_str(""), ArrowMode::Wrap);
    }

    #[test]
    fn arrow_mode_default_is_wrap() {
        assert_eq!(ArrowMode::default(), ArrowMode::Wrap);
    }

    // -- Focusable groups / two-level navigation --

    #[test]
    fn parent_group_set_for_children_of_focusable_group() {
        // A focusable group "toolbar" with two children "btn-a" and "btn-b".
        let shapes = vec![json!({
            "type": "group",
            "id": "toolbar",
            "focusable": true,
            "on_click": true,
            "children": [
                {
                    "type": "group",
                    "id": "btn-a",
                    "on_click": true,
                    "children": [{"type": "rect", "x": 0, "y": 0, "w": 30, "h": 30}]
                },
                {
                    "type": "group",
                    "id": "btn-b",
                    "on_click": true,
                    "children": [{"type": "rect", "x": 40, "y": 0, "w": 30, "h": 30}]
                }
            ]
        })];
        let mut elements = Vec::new();
        collect_interactive_elements(
            &shapes, "default", TransformMatrix::identity(), None, None, &mut elements,
        );
        // Should collect: toolbar, btn-a, btn-b
        assert_eq!(elements.len(), 3);
        assert_eq!(elements[0].id, "toolbar");
        assert!(elements[0].focusable);
        assert_eq!(elements[0].parent_group, None); // top-level
        assert_eq!(elements[1].id, "btn-a");
        assert_eq!(elements[1].parent_group, Some("toolbar".to_string()));
        assert_eq!(elements[2].id, "btn-b");
        assert_eq!(elements[2].parent_group, Some("toolbar".to_string()));
    }

    #[test]
    fn parent_group_none_without_focusable() {
        // Non-focusable group with children: no parent_group set.
        let shapes = vec![json!({
            "type": "group",
            "id": "container",
            "on_click": true,
            "children": [
                {
                    "type": "group",
                    "id": "child",
                    "on_click": true,
                    "children": [{"type": "rect", "x": 0, "y": 0, "w": 10, "h": 10}]
                }
            ]
        })];
        let mut elements = Vec::new();
        collect_interactive_elements(
            &shapes, "default", TransformMatrix::identity(), None, None, &mut elements,
        );
        assert_eq!(elements.len(), 2);
        assert_eq!(elements[0].parent_group, None);
        assert_eq!(elements[1].parent_group, None);
    }

    #[test]
    fn top_level_indices_excludes_group_children() {
        let mut toolbar = test_element("toolbar");
        toolbar.focusable = true;
        let mut btn_a = test_element("btn-a");
        btn_a.parent_group = Some("toolbar".to_string());
        let mut btn_b = test_element("btn-b");
        btn_b.parent_group = Some("toolbar".to_string());
        let standalone = test_element("standalone");
        let elements = vec![toolbar, btn_a, btn_b, standalone];
        let program = test_program(&elements);
        let top = program.top_level_indices();
        // Only "toolbar" (idx 0) and "standalone" (idx 3) are top-level.
        assert_eq!(top, vec![0, 3]);
    }

    #[test]
    fn group_child_indices_returns_children() {
        let mut toolbar = test_element("toolbar");
        toolbar.focusable = true;
        let mut btn_a = test_element("btn-a");
        btn_a.parent_group = Some("toolbar".to_string());
        let mut btn_b = test_element("btn-b");
        btn_b.parent_group = Some("toolbar".to_string());
        let standalone = test_element("standalone");
        let elements = vec![toolbar, btn_a, btn_b, standalone];
        let program = test_program(&elements);
        let children = program.group_child_indices("toolbar");
        assert_eq!(children, vec![1, 2]);
    }

    #[test]
    fn group_child_indices_empty_for_nonexistent_group() {
        let elements = vec![test_element("a"), test_element("b")];
        let program = test_program(&elements);
        assert!(program.group_child_indices("nonexistent").is_empty());
    }
}
