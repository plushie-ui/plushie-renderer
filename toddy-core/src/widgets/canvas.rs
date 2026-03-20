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
use iced::{Color, Element, Length, Pixels, Point, Radians, Size, Vector, alignment, mouse};
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
}

impl CanvasProgram<'_> {
    fn is_interactive(&self) -> bool {
        self.on_press || self.on_release || self.on_move || self.on_scroll
    }
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
        let position = cursor.position_in(bounds)?;
        state.cursor_position = Some(position);

        match event {
            iced::Event::Mouse(mouse::Event::ButtonPressed(button)) if self.on_press => {
                let btn_str = serialize_mouse_button_for_canvas(button);
                Some(iced::widget::Action::publish(Message::CanvasEvent {
                    id: self.id.clone(),
                    kind: "press".to_string(),
                    x: position.x,
                    y: position.y,
                    extra: btn_str,
                }))
            }
            iced::Event::Mouse(mouse::Event::ButtonReleased(button)) if self.on_release => {
                let btn_str = serialize_mouse_button_for_canvas(button);
                Some(iced::widget::Action::publish(Message::CanvasEvent {
                    id: self.id.clone(),
                    kind: "release".to_string(),
                    x: position.x,
                    y: position.y,
                    extra: btn_str,
                }))
            }
            iced::Event::Mouse(mouse::Event::CursorMoved { .. }) if self.on_move => {
                Some(iced::widget::Action::publish(Message::CanvasEvent {
                    id: self.id.clone(),
                    kind: "move".to_string(),
                    x: position.x,
                    y: position.y,
                    extra: String::new(),
                }))
            }
            iced::Event::Mouse(mouse::Event::WheelScrolled { delta }) if self.on_scroll => {
                let (dx, dy) = match delta {
                    mouse::ScrollDelta::Lines { x, y } => (*x, *y),
                    mouse::ScrollDelta::Pixels { x, y } => (*x, *y),
                };
                Some(iced::widget::Action::publish(Message::CanvasScroll {
                    id: self.id.clone(),
                    cursor_x: position.x,
                    cursor_y: position.y,
                    delta_x: dx,
                    delta_y: dy,
                }))
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        _state: &CanvasState,
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
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

        // Draw each layer, using its cache when available.
        let images = self.images;
        for (layer_name, shapes) in &self.layers {
            let shape_refs: Vec<&Value> = shapes.iter().collect();
            let geom = if let Some((_hash, cache)) = self.caches.and_then(|c| c.get(layer_name)) {
                cache.draw(renderer, bounds.size(), |frame| {
                    draw_canvas_shapes(frame, &shape_refs, images);
                })
            } else {
                // No cache available (shouldn't happen after ensure_caches, but
                // handle gracefully by drawing uncached).
                let mut frame = canvas::Frame::new(renderer, bounds.size());
                draw_canvas_shapes(&mut frame, &shape_refs, images);
                frame.into_geometry()
            };
            geometries.push(geom);
        }

        geometries
    }

    fn mouse_interaction(
        &self,
        _state: &CanvasState,
        _bounds: iced::Rectangle,
        _cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if self.is_interactive() {
            mouse::Interaction::Crosshair
        } else {
            mouse::Interaction::default()
        }
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

    iced::widget::canvas(CanvasProgram {
        layers,
        caches: node_caches,
        background,
        id: node.id.clone(),
        on_press: on_press || interactive,
        on_release: on_release || interactive,
        on_move: on_move || interactive,
        on_scroll: on_scroll || interactive,
        images: ctx.images,
    })
    .width(width)
    .height(height)
    .into()
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

pub(crate) fn ensure_canvas_cache(node: &crate::protocol::TreeNode, caches: &mut WidgetCaches) {
    let props = node.props.as_object();
    // Build layer map: either from "layers" (object) or "shapes" (array -> single layer).
    let layer_map = canvas_layer_map(props);
    let node_caches = caches.canvas_caches.entry(node.id.clone()).or_default();

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
}
