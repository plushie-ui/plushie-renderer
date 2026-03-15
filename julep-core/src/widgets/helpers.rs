use iced::widget::text::{LineHeight, Wrapping};
use iced::widget::{
    button, checkbox, container, pick_list, progress_bar, rule, slider, text_editor, text_input,
    toggler,
};
use iced::{
    alignment, font, mouse, Border, Color, ContentFit, Fill, Font, Length, Padding, Pixels,
    Radians, Shadow, Vector,
};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Prop helpers
// ---------------------------------------------------------------------------

pub(crate) type Props<'a> = Option<&'a serde_json::Map<String, Value>>;

pub(crate) fn prop_str<'a>(props: Props<'a>, key: &str) -> Option<String> {
    props?.get(key)?.as_str().map(str::to_owned)
}

pub(crate) fn prop_f32(props: Props<'_>, key: &str) -> Option<f32> {
    let val = props?.get(key)?;
    match val {
        Value::Number(n) => n.as_f64().map(|v| v as f32),
        Value::String(s) => s.trim().parse::<f32>().ok(),
        _ => None,
    }
}

pub(crate) fn prop_f64(props: Props<'_>, key: &str) -> Option<f64> {
    let val = props?.get(key)?;
    match val {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

pub(crate) fn prop_bool(props: Props<'_>, key: &str) -> Option<bool> {
    props?.get(key)?.as_bool()
}

/// Read a boolean prop with a default value.
pub(crate) fn prop_bool_default(props: Props<'_>, key: &str, default: bool) -> bool {
    prop_bool(props, key).unwrap_or(default)
}

pub(crate) fn prop_length(props: Props<'_>, key: &str, fallback: Length) -> Length {
    props
        .and_then(|p| p.get(key))
        .and_then(value_to_length)
        .unwrap_or(fallback)
}

pub(crate) fn value_to_length(val: &Value) -> Option<Length> {
    match val {
        Value::Number(n) => n
            .as_f64()
            .map(|v| v as f32)
            .filter(|v| *v >= 0.0)
            .map(Length::Fixed),
        Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "fill" | "full" | "expand" | "stretch" => Some(Fill),
            "shrink" | "auto" | "fit" => Some(Length::Shrink),
            other => other
                .parse::<f32>()
                .ok()
                .filter(|v| *v >= 0.0)
                .map(Length::Fixed),
        },
        Value::Object(obj) => {
            // Handle {"fill_portion": N}
            if let Some(n) = obj.get("fill_portion").and_then(|v| v.as_u64()) {
                Some(Length::FillPortion(n as u16))
            } else {
                Some(Length::Shrink)
            }
        }
        _ => None,
    }
}

/// Try to parse a length from an optional Value. Returns None if the value
/// is absent or unparseable (unlike prop_length which returns a fallback).
pub(crate) fn value_to_length_opt(val: Option<&Value>) -> Option<Length> {
    val.and_then(value_to_length)
}

// ---------------------------------------------------------------------------
// Padding parsing -- handles both number and object formats
// ---------------------------------------------------------------------------

/// Parse a padding value from props. Handles:
/// - `"padding": 10` -- uniform padding
/// - `"padding": {"top": 10, "right": 5, "bottom": 10, "left": 5}` -- per-side
/// - Individual `"padding_top"` etc. keys (legacy)
pub(crate) fn parse_padding_value(props: Props<'_>) -> Padding {
    let padding_val = props.and_then(|p| p.get("padding"));

    match padding_val {
        Some(Value::Object(obj)) => {
            let top = obj
                .get("top")
                .and_then(|v| v.as_f64())
                .map(|v| v as f32)
                .unwrap_or(0.0)
                .max(0.0);
            let right = obj
                .get("right")
                .and_then(|v| v.as_f64())
                .map(|v| v as f32)
                .unwrap_or(0.0)
                .max(0.0);
            let bottom = obj
                .get("bottom")
                .and_then(|v| v.as_f64())
                .map(|v| v as f32)
                .unwrap_or(0.0)
                .max(0.0);
            let left = obj
                .get("left")
                .and_then(|v| v.as_f64())
                .map(|v| v as f32)
                .unwrap_or(0.0)
                .max(0.0);
            Padding {
                top,
                right,
                bottom,
                left,
            }
        }
        Some(Value::Number(n)) => {
            let base = n.as_f64().map(|v| v as f32).unwrap_or(0.0).max(0.0);
            // Check for per-side overrides (legacy format)
            let top = prop_f32(props, "padding_top").unwrap_or(base);
            let right = prop_f32(props, "padding_right").unwrap_or(base);
            let bottom = prop_f32(props, "padding_bottom").unwrap_or(base);
            let left = prop_f32(props, "padding_left").unwrap_or(base);
            Padding {
                top,
                right,
                bottom,
                left,
            }
        }
        _ => {
            // No padding prop -- check legacy individual keys
            let top = prop_f32(props, "padding_top").unwrap_or(0.0);
            let right = prop_f32(props, "padding_right").unwrap_or(0.0);
            let bottom = prop_f32(props, "padding_bottom").unwrap_or(0.0);
            let left = prop_f32(props, "padding_left").unwrap_or(0.0);
            Padding {
                top,
                right,
                bottom,
                left,
            }
        }
    }
}

pub(crate) fn prop_horizontal_alignment(props: Props<'_>, key: &str) -> alignment::Horizontal {
    props
        .and_then(|p| p.get(key))
        .and_then(|v| v.as_str())
        .and_then(value_to_horizontal_alignment)
        .unwrap_or(alignment::Horizontal::Left)
}

pub(crate) fn prop_vertical_alignment(props: Props<'_>, key: &str) -> alignment::Vertical {
    props
        .and_then(|p| p.get(key))
        .and_then(|v| v.as_str())
        .and_then(value_to_vertical_alignment)
        .unwrap_or(alignment::Vertical::Top)
}

pub(crate) fn value_to_horizontal_alignment(s: &str) -> Option<alignment::Horizontal> {
    match s.trim().to_ascii_lowercase().as_str() {
        "left" | "start" => Some(alignment::Horizontal::Left),
        "center" => Some(alignment::Horizontal::Center),
        "right" | "end" => Some(alignment::Horizontal::Right),
        _ => None,
    }
}

pub(crate) fn value_to_vertical_alignment(s: &str) -> Option<alignment::Vertical> {
    match s.trim().to_ascii_lowercase().as_str() {
        "top" | "start" => Some(alignment::Vertical::Top),
        "center" => Some(alignment::Vertical::Center),
        "bottom" | "end" => Some(alignment::Vertical::Bottom),
        _ => None,
    }
}

/// Parse a "range" prop as [min, max] into an inclusive range of f32.
pub(crate) fn prop_range_f32(props: Props<'_>) -> std::ops::RangeInclusive<f32> {
    props
        .and_then(|p| p.get("range"))
        .and_then(|v| v.as_array())
        .and_then(|arr| {
            let min = arr.first()?.as_f64()? as f32;
            let max = arr.get(1)?.as_f64()? as f32;
            Some(min..=max)
        })
        .unwrap_or(0.0..=100.0)
}

/// Parse a "range" prop as [min, max] into an inclusive range of f64.
pub(crate) fn prop_range_f64(props: Props<'_>) -> std::ops::RangeInclusive<f64> {
    props
        .and_then(|p| p.get("range"))
        .and_then(|v| v.as_array())
        .and_then(|arr| {
            let min = arr.first()?.as_f64()?;
            let max = arr.get(1)?.as_f64()?;
            Some(min..=max)
        })
        .unwrap_or(0.0..=100.0)
}

#[cfg(any(feature = "widget-image", feature = "widget-svg"))]
pub(crate) fn prop_content_fit(props: Props<'_>) -> Option<ContentFit> {
    let s = prop_str(props, "content_fit")?;
    match s.to_ascii_lowercase().as_str() {
        "contain" => Some(ContentFit::Contain),
        "cover" => Some(ContentFit::Cover),
        "fill" => Some(ContentFit::Fill),
        "none" => Some(ContentFit::None),
        "scale_down" => Some(ContentFit::ScaleDown),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Mouse interaction (cursor) parsing
// ---------------------------------------------------------------------------

pub(crate) fn parse_interaction(s: &str) -> Option<mouse::Interaction> {
    Some(match s {
        "pointer" => mouse::Interaction::Pointer,
        "grab" => mouse::Interaction::Grab,
        "grabbing" => mouse::Interaction::Grabbing,
        "crosshair" => mouse::Interaction::Crosshair,
        "text" => mouse::Interaction::Text,
        "move" => mouse::Interaction::Move,
        "not_allowed" => mouse::Interaction::NotAllowed,
        "progress" => mouse::Interaction::Progress,
        "wait" => mouse::Interaction::Wait,
        "help" => mouse::Interaction::Help,
        "cell" => mouse::Interaction::Cell,
        "copy" => mouse::Interaction::Copy,
        "alias" => mouse::Interaction::Alias,
        "no_drop" => mouse::Interaction::NoDrop,
        "all_scroll" => mouse::Interaction::AllScroll,
        "zoom_in" => mouse::Interaction::ZoomIn,
        "zoom_out" => mouse::Interaction::ZoomOut,
        "context_menu" => mouse::Interaction::ContextMenu,
        "resizing_horizontally" => mouse::Interaction::ResizingHorizontally,
        "resizing_vertically" => mouse::Interaction::ResizingVertically,
        "resizing_diagonally_up" => mouse::Interaction::ResizingDiagonallyUp,
        "resizing_diagonally_down" => mouse::Interaction::ResizingDiagonallyDown,
        "resizing_column" => mouse::Interaction::ResizingColumn,
        "resizing_row" => mouse::Interaction::ResizingRow,
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Color parsing -- hex string "#rrggbb" / "#rrggbbaa" or {r,g,b,a} object
// ---------------------------------------------------------------------------

pub(crate) fn parse_hex_color(s: &str) -> Option<Color> {
    let s = s.strip_prefix('#').unwrap_or(s);
    if s.len() == 6 {
        let r = u8::from_str_radix(&s[0..2], 16).ok()?;
        let g = u8::from_str_radix(&s[2..4], 16).ok()?;
        let b = u8::from_str_radix(&s[4..6], 16).ok()?;
        Some(Color::from_rgb8(r, g, b))
    } else if s.len() == 8 {
        let r = u8::from_str_radix(&s[0..2], 16).ok()?;
        let g = u8::from_str_radix(&s[2..4], 16).ok()?;
        let b = u8::from_str_radix(&s[4..6], 16).ok()?;
        let a = u8::from_str_radix(&s[6..8], 16).ok()?;
        Some(Color::from_rgba8(r, g, b, a as f32 / 255.0))
    } else {
        None
    }
}

/// Parse a color from a JSON value. Accepts:
/// - A hex string: "#rrggbb" or "#rrggbbaa"
/// - An object: {"r": 0.5, "g": 0.5, "b": 0.5, "a": 1.0} (0-1 floats)
pub(crate) fn parse_color(value: &Value) -> Option<Color> {
    match value {
        Value::String(s) => parse_hex_color(s),
        Value::Object(obj) => {
            let r = obj.get("r").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
            let g = obj.get("g").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
            let b = obj.get("b").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
            let a = obj.get("a").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;
            Some(Color::from_rgba(r, g, b, a))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Background parsing (color or gradient)
// ---------------------------------------------------------------------------

/// Parse a background from a JSON value. Accepts:
/// - A color string ("#rrggbb") or object ({r,g,b,a}) -> Background::Color
/// - A gradient object: {"type": "linear", "angle": 45, "stops": [{"offset": 0.0, "color": "#ff0000"}, ...]}
pub(crate) fn parse_background(value: &Value) -> Option<iced::Background> {
    match value {
        Value::String(_) => parse_color(value).map(iced::Background::Color),
        Value::Object(obj) => {
            match obj.get("type").and_then(|v| v.as_str()) {
                Some("linear") => {
                    // Warn on unrecognized gradient keys
                    let valid_keys: &[&str] = &["type", "angle", "stops"];
                    for key in obj.keys() {
                        if !valid_keys.contains(&key.as_str()) {
                            log::warn!(
                                "unrecognized background gradient key '{}' (valid: {:?})",
                                key,
                                valid_keys
                            );
                        }
                    }

                    let angle_deg = obj.get("angle").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                    let angle = Radians(angle_deg.to_radians());
                    let mut linear = iced::gradient::Linear::new(angle);

                    if let Some(stops) = obj.get("stops").and_then(|v| v.as_array()) {
                        let valid_stop_keys: &[&str] = &["offset", "color"];
                        for stop in stops {
                            if let Some(stop_obj) = stop.as_object() {
                                for key in stop_obj.keys() {
                                    if !valid_stop_keys.contains(&key.as_str()) {
                                        log::warn!(
                                            "unrecognized gradient stop key '{}' (valid: {:?})",
                                            key,
                                            valid_stop_keys
                                        );
                                    }
                                }
                            }
                            let offset =
                                stop.get("offset").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                            let color = stop
                                .get("color")
                                .and_then(parse_color)
                                .unwrap_or(Color::TRANSPARENT);
                            linear = linear.add_stop(offset, color);
                        }
                    }

                    Some(iced::Background::Gradient(iced::Gradient::Linear(linear)))
                }
                Some(other) => {
                    log::warn!(
                        "unrecognized gradient type '{}' (supported: \"linear\")",
                        other
                    );
                    parse_color(value).map(iced::Background::Color)
                }
                _ => {
                    // Fall back to color object parsing ({r, g, b, a})
                    parse_color(value).map(iced::Background::Color)
                }
            }
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Font parsing
// ---------------------------------------------------------------------------

/// Parse a font from a JSON value. Accepts:
/// - "default" -> Font::DEFAULT
/// - "monospace" -> Font::MONOSPACE
/// - An object with optional family, weight, style fields
pub(crate) fn parse_font(value: &Value) -> Font {
    match value {
        Value::String(s) => match s.to_ascii_lowercase().as_str() {
            "monospace" => Font::MONOSPACE,
            _ => Font::DEFAULT,
        },
        Value::Object(obj) => {
            let mut f = Font::DEFAULT;

            if let Some(family) = obj.get("family").and_then(|v| v.as_str()) {
                match family.to_ascii_lowercase().as_str() {
                    "monospace" | "mono" => {
                        f.family = font::Family::Monospace;
                    }
                    "serif" => {
                        f.family = font::Family::Serif;
                    }
                    "cursive" => {
                        f.family = font::Family::Cursive;
                    }
                    "fantasy" => {
                        f.family = font::Family::Fantasy;
                    }
                    // Default is SansSerif; unrecognized names are passed
                    // through as custom font families (user-loaded fonts).
                    "default" | "sans_serif" | "sans-serif" | "sansserif" | "" => {}
                    other => {
                        // Leak the string to get a 'static lifetime. Font
                        // family names are a small, finite set that lives for
                        // the process lifetime, so this is acceptable.
                        let leaked: &'static str = Box::leak(other.to_owned().into_boxed_str());
                        f.family = font::Family::Name(leaked);
                    }
                }
            }

            if let Some(weight_val) = obj.get("weight") {
                if let Some(weight_num) = weight_val.as_u64() {
                    f.weight = match weight_num {
                        100 => font::Weight::Thin,
                        200 => font::Weight::ExtraLight,
                        300 => font::Weight::Light,
                        400 => font::Weight::Normal,
                        500 => font::Weight::Medium,
                        600 => font::Weight::Semibold,
                        700 => font::Weight::Bold,
                        800 => font::Weight::ExtraBold,
                        900 => font::Weight::Black,
                        _ => font::Weight::Normal,
                    };
                } else if let Some(weight) = weight_val.as_str() {
                    f.weight = match weight.to_ascii_lowercase().as_str() {
                        "thin" => font::Weight::Thin,
                        "extralight" | "extra_light" => font::Weight::ExtraLight,
                        "light" => font::Weight::Light,
                        "normal" | "regular" => font::Weight::Normal,
                        "medium" => font::Weight::Medium,
                        "semibold" | "semi_bold" => font::Weight::Semibold,
                        "bold" => font::Weight::Bold,
                        "extrabold" | "extra_bold" => font::Weight::ExtraBold,
                        "black" => font::Weight::Black,
                        _ => font::Weight::Normal,
                    };
                }
            }

            if let Some(style) = obj.get("style").and_then(|v| v.as_str()) {
                f.style = match style.to_ascii_lowercase().as_str() {
                    "italic" => font::Style::Italic,
                    "oblique" => font::Style::Oblique,
                    _ => font::Style::Normal,
                };
            }

            if let Some(stretch_val) = obj.get("stretch").and_then(|v| v.as_str()) {
                f.stretch = match stretch_val.to_ascii_lowercase().as_str() {
                    "ultra_condensed" | "ultracondensed" => font::Stretch::UltraCondensed,
                    "extra_condensed" | "extracondensed" => font::Stretch::ExtraCondensed,
                    "condensed" => font::Stretch::Condensed,
                    "semi_condensed" | "semicondensed" => font::Stretch::SemiCondensed,
                    "normal" => font::Stretch::Normal,
                    "semi_expanded" | "semiexpanded" => font::Stretch::SemiExpanded,
                    "expanded" => font::Stretch::Expanded,
                    "extra_expanded" | "extraexpanded" => font::Stretch::ExtraExpanded,
                    "ultra_expanded" | "ultraexpanded" => font::Stretch::UltraExpanded,
                    _ => font::Stretch::Normal,
                };
            }

            f
        }
        _ => Font::DEFAULT,
    }
}

// ---------------------------------------------------------------------------
// Border and Shadow parsing
// ---------------------------------------------------------------------------

/// Parse a border from a JSON value.
/// Accepts: {"color": "#rrggbb", "width": 1.0, "radius": 4.0}
/// radius can be a number or [tl, tr, br, bl]
pub(crate) fn parse_border(value: &Value) -> Border {
    let obj = match value.as_object() {
        Some(o) => o,
        None => return Border::default(),
    };

    let color = obj
        .get("color")
        .and_then(parse_color)
        .unwrap_or(Color::TRANSPARENT);
    let width = obj
        .get("width")
        .and_then(|v| v.as_f64())
        .map(|v| v as f32)
        .unwrap_or(0.0);
    let radius = match obj.get("radius") {
        Some(Value::Number(n)) => {
            let r = n.as_f64().unwrap_or(0.0) as f32;
            r.into()
        }
        Some(Value::Array(arr)) if !arr.is_empty() => {
            // Per-corner: [top_left, top_right, bottom_right, bottom_left]
            let tl = arr.first().and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
            let tr = arr.get(1).and_then(|v| v.as_f64()).unwrap_or(tl as f64) as f32;
            let br = arr.get(2).and_then(|v| v.as_f64()).unwrap_or(tl as f64) as f32;
            let bl = arr.get(3).and_then(|v| v.as_f64()).unwrap_or(tl as f64) as f32;
            iced::border::Radius {
                top_left: tl,
                top_right: tr,
                bottom_right: br,
                bottom_left: bl,
            }
        }
        Some(Value::Object(radius_obj)) => {
            // Per-corner object: {"top_left": N, "top_right": N, "bottom_right": N, "bottom_left": N}
            let tl = radius_obj
                .get("top_left")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0) as f32;
            let tr = radius_obj
                .get("top_right")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0) as f32;
            let br = radius_obj
                .get("bottom_right")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0) as f32;
            let bl = radius_obj
                .get("bottom_left")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0) as f32;
            iced::border::Radius {
                top_left: tl,
                top_right: tr,
                bottom_right: br,
                bottom_left: bl,
            }
        }
        _ => (0.0_f32).into(),
    };

    Border {
        color,
        width,
        radius,
    }
}

/// Parse a shadow from a JSON value.
/// Accepts: {"color": "#rrggbb", "offset": [x, y], "blur_radius": 5.0}
pub(crate) fn parse_shadow(value: &Value) -> Shadow {
    let obj = match value.as_object() {
        Some(o) => o,
        None => return Shadow::default(),
    };

    let color = obj
        .get("color")
        .and_then(parse_color)
        .unwrap_or(Color::BLACK);
    let offset = match obj.get("offset").and_then(|v| v.as_array()) {
        Some(arr) if arr.len() >= 2 => Vector::new(
            arr[0].as_f64().unwrap_or(0.0) as f32,
            arr[1].as_f64().unwrap_or(0.0) as f32,
        ),
        _ => Vector::new(0.0, 0.0),
    };
    let blur_radius = obj
        .get("blur_radius")
        .and_then(|v| v.as_f64())
        .map(|v| v as f32)
        .unwrap_or(0.0);

    Shadow {
        color,
        offset,
        blur_radius,
    }
}

// ---------------------------------------------------------------------------
// Style map parsing
// ---------------------------------------------------------------------------

/// Parsed fields from a style map JSON object. All fields are optional;
/// only those present in the JSON get populated.
#[derive(Clone, Default)]
pub(crate) struct StyleMapFields {
    pub(crate) background: Option<iced::Background>,
    pub(crate) text_color: Option<Color>,
    pub(crate) border: Option<Border>,
    pub(crate) shadow: Option<Shadow>,
}

pub(crate) fn parse_style_map_fields(obj: &serde_json::Map<String, Value>) -> StyleMapFields {
    StyleMapFields {
        background: obj.get("background").and_then(parse_background),
        text_color: obj.get("text_color").and_then(parse_color),
        border: obj.get("border").map(parse_border),
        shadow: obj.get("shadow").map(parse_shadow),
    }
}

/// Parsed style overrides for all status variants. The base fields are always
/// present; status-specific overrides are optional.
#[derive(Clone)]
pub(crate) struct StyleOverrides {
    pub(crate) base: StyleMapFields,
    pub(crate) hovered: Option<StyleMapFields>,
    pub(crate) pressed: Option<StyleMapFields>,
    pub(crate) disabled: Option<StyleMapFields>,
    pub(crate) focused: Option<StyleMapFields>,
}

pub(crate) fn parse_style_overrides(obj: &serde_json::Map<String, Value>) -> StyleOverrides {
    StyleOverrides {
        base: parse_style_map_fields(obj),
        hovered: obj
            .get("hovered")
            .and_then(|v| v.as_object())
            .map(parse_style_map_fields),
        pressed: obj
            .get("pressed")
            .and_then(|v| v.as_object())
            .map(parse_style_map_fields),
        disabled: obj
            .get("disabled")
            .and_then(|v| v.as_object())
            .map(parse_style_map_fields),
        focused: obj
            .get("focused")
            .and_then(|v| v.as_object())
            .map(parse_style_map_fields),
    }
}

/// Auto-derive hover background by darkening the base background to 90%.
pub(crate) fn auto_derive_hover_bg(bg: Option<iced::Background>) -> Option<iced::Background> {
    bg.map(|b| darken_background(b, 0.9))
}

/// Auto-derive disabled background by reducing alpha to 50%.
pub(crate) fn auto_derive_disabled_bg(bg: Option<iced::Background>) -> Option<iced::Background> {
    bg.map(|b| alpha_background(b, 0.5))
}

/// Auto-derive disabled text color by reducing alpha to 50%.
pub(crate) fn auto_derive_disabled_text(color: Color) -> Color {
    alpha_color(color, 0.5)
}

/// Apply style map fields to a button style. Background wraps in `Some`,
/// text_color, border, and shadow map directly.
pub(crate) fn apply_button_fields(style: &mut button::Style, fields: &StyleMapFields) {
    if let Some(bg) = fields.background {
        style.background = Some(bg);
    }
    if let Some(tc) = fields.text_color {
        style.text_color = tc;
    }
    if let Some(brd) = fields.border {
        style.border = brd;
    }
    if let Some(shd) = fields.shadow {
        style.shadow = shd;
    }
}

/// Apply style map fields to a progress_bar style. Background maps as
/// `Background::Color`, text_color maps to the bar fill, border directly.
pub(crate) fn apply_progress_bar_fields(style: &mut progress_bar::Style, fields: &StyleMapFields) {
    if let Some(iced::Background::Color(c)) = fields.background {
        style.background = iced::Background::Color(c);
    }
    if let Some(tc) = fields.text_color {
        style.bar = iced::Background::Color(tc);
    }
    if let Some(brd) = fields.border {
        style.border = brd;
    }
}

/// Apply style map fields to a text_input or text_editor style. Both widgets
/// map background as `Background::Color`, border directly, and text_color to
/// the `value` field (the typed text color).
pub(crate) fn apply_text_input_fields(style: &mut text_input::Style, fields: &StyleMapFields) {
    if let Some(iced::Background::Color(c)) = fields.background {
        style.background = iced::Background::Color(c);
    }
    if let Some(brd) = fields.border {
        style.border = brd;
    }
    if let Some(tc) = fields.text_color {
        style.value = tc;
    }
}

/// Apply style map fields to a text_editor style. Same field mapping as
/// text_input (background, border, text_color -> value).
pub(crate) fn apply_text_editor_fields(style: &mut text_editor::Style, fields: &StyleMapFields) {
    if let Some(iced::Background::Color(c)) = fields.background {
        style.background = iced::Background::Color(c);
    }
    if let Some(brd) = fields.border {
        style.border = brd;
    }
    if let Some(tc) = fields.text_color {
        style.value = tc;
    }
}

/// Apply style map fields to a pick_list style. Background is
/// `Background::Color`, text_color and border map directly.
pub(crate) fn apply_pick_list_fields(style: &mut pick_list::Style, fields: &StyleMapFields) {
    if let Some(tc) = fields.text_color {
        style.text_color = tc;
    }
    if let Some(iced::Background::Color(c)) = fields.background {
        style.background = iced::Background::Color(c);
    }
    if let Some(brd) = fields.border {
        style.border = brd;
    }
}

/// Apply style map fields to a slider handle. Background maps to
/// handle.background as `Background::Color`, border maps to
/// handle.border_width/border_color. Shared by slider and vertical_slider.
pub(crate) fn apply_slider_handle_fields(handle: &mut slider::Handle, fields: &StyleMapFields) {
    if let Some(iced::Background::Color(c)) = fields.background {
        handle.background = iced::Background::Color(c);
    }
    if let Some(brd) = fields.border {
        handle.border_width = brd.width;
        handle.border_color = brd.color;
    }
}

/// Apply style map fields to a radio style. Background is `Background::Color`,
/// text_color wraps in `Some`, border maps to border_width/border_color.
pub(crate) fn apply_radio_fields(style: &mut iced::widget::radio::Style, fields: &StyleMapFields) {
    if let Some(iced::Background::Color(c)) = fields.background {
        style.background = iced::Background::Color(c);
    }
    if let Some(tc) = fields.text_color {
        style.text_color = Some(tc);
    }
    if let Some(brd) = fields.border {
        style.border_width = brd.width;
        style.border_color = brd.color;
    }
}

/// Apply style map fields to a toggler style. Background maps directly,
/// text_color wraps in `Some`, border maps to border_width/border_color.
pub(crate) fn apply_toggler_fields(style: &mut toggler::Style, fields: &StyleMapFields) {
    if let Some(bg) = fields.background {
        style.background = bg;
    }
    if let Some(tc) = fields.text_color {
        style.text_color = Some(tc);
    }
    if let Some(brd) = fields.border {
        style.background_border_width = brd.width;
        style.background_border_color = brd.color;
    }
}

/// Apply style map fields to a rule style. Maps background -> color,
/// border -> radius.
pub(crate) fn apply_rule_style(style: &mut rule::Style, fields: &StyleMapFields) -> rule::Style {
    if let Some(iced::Background::Color(c)) = fields.background {
        style.color = c;
    }
    if let Some(brd) = fields.border {
        style.radius = brd.radius;
    }
    *style
}

/// Apply style map fields to a checkbox style. Background is `Background::Color`,
/// border directly, text_color wrapped in `Some`.
pub(crate) fn apply_checkbox_fields(style: &mut checkbox::Style, fields: &StyleMapFields) {
    if let Some(iced::Background::Color(c)) = fields.background {
        style.background = iced::Background::Color(c);
    }
    if let Some(brd) = fields.border {
        style.border = brd;
    }
    if let Some(tc) = fields.text_color {
        style.text_color = Some(tc);
    }
}

/// Build a `container::Style` from base style map fields. Used by both
/// container and tooltip widgets which share the same style type.
pub(crate) fn container_style_from_base(base: &StyleMapFields) -> container::Style {
    let mut style = container::Style {
        background: base.background,
        text_color: base.text_color,
        ..Default::default()
    };
    if let Some(brd) = base.border {
        style.border = brd;
    }
    if let Some(shd) = base.shadow {
        style.shadow = shd;
    }
    style
}

pub(crate) fn darken_color(color: Color, factor: f32) -> Color {
    Color {
        r: color.r * factor,
        g: color.g * factor,
        b: color.b * factor,
        a: color.a,
    }
}

pub(crate) fn alpha_color(color: Color, alpha: f32) -> Color {
    Color {
        r: color.r,
        g: color.g,
        b: color.b,
        a: color.a * alpha,
    }
}

pub(crate) fn darken_background(bg: iced::Background, factor: f32) -> iced::Background {
    match bg {
        iced::Background::Color(c) => iced::Background::Color(darken_color(c, factor)),
        other => other,
    }
}

pub(crate) fn alpha_background(bg: iced::Background, alpha: f32) -> iced::Background {
    match bg {
        iced::Background::Color(c) => iced::Background::Color(alpha_color(c, alpha)),
        other => other,
    }
}

// ---------------------------------------------------------------------------
// Line height and wrapping parsing
// ---------------------------------------------------------------------------

/// Parse line_height prop. Accepts:
/// - A number (interpreted as relative multiplier)
/// - An object {"relative": 1.5} or {"absolute": 20}
pub(crate) fn parse_line_height(props: Props<'_>) -> Option<LineHeight> {
    let val = props?.get("line_height")?;
    match val {
        Value::Number(n) => {
            let v = n.as_f64()? as f32;
            Some(LineHeight::Relative(v))
        }
        Value::Object(obj) => {
            if let Some(r) = obj.get("relative").and_then(|v| v.as_f64()) {
                Some(LineHeight::Relative(r as f32))
            } else {
                obj.get("absolute")
                    .and_then(|v| v.as_f64())
                    .map(|a| LineHeight::Absolute(Pixels(a as f32)))
            }
        }
        _ => None,
    }
}

/// Parse text_shaping prop from a string.
pub(crate) fn parse_shaping(props: Props<'_>) -> Option<iced::widget::text::Shaping> {
    use iced::widget::text::Shaping;
    let s = prop_str(props, "text_shaping")?;
    match s.to_ascii_lowercase().as_str() {
        "basic" => Some(Shaping::Basic),
        "advanced" => Some(Shaping::Advanced),
        "auto" => Some(Shaping::Auto),
        _ => None,
    }
}

/// Parse wrapping prop from a string.
pub(crate) fn parse_wrapping(props: Props<'_>) -> Option<Wrapping> {
    let s = prop_str(props, "wrapping")?;
    match s.to_ascii_lowercase().as_str() {
        "none" => Some(Wrapping::None),
        "word" => Some(Wrapping::Word),
        "glyph" => Some(Wrapping::Glyph),
        "word_or_glyph" => Some(Wrapping::WordOrGlyph),
        _ => None,
    }
}

/// Parse a text_input::Icon from a JSON value.
pub(crate) fn parse_text_input_icon(value: &Value) -> Option<text_input::Icon<Font>> {
    let obj = value.as_object()?;

    let code_point = obj
        .get("code_point")
        .and_then(|v| v.as_str())
        .and_then(|s| s.chars().next())?;

    let font = obj.get("font").map(parse_font).unwrap_or(Font::DEFAULT);

    let size = obj
        .get("size")
        .and_then(|v| v.as_f64())
        .map(|v| Pixels(v as f32));

    let spacing = obj
        .get("spacing")
        .and_then(|v| v.as_f64())
        .map(|v| v as f32)
        .unwrap_or(4.0);

    let side = match obj.get("side").and_then(|v| v.as_str()).unwrap_or("left") {
        "right" | "trailing" => text_input::Side::Right,
        _ => text_input::Side::Left,
    };

    Some(text_input::Icon {
        font,
        code_point,
        size,
        spacing,
        side,
    })
}

/// Parse a pick_list::Icon from a JSON value.
pub(crate) fn parse_pick_list_icon(value: &Value) -> Option<pick_list::Icon<Font>> {
    let obj = value.as_object()?;

    let code_point = obj
        .get("code_point")
        .and_then(|v| v.as_str())
        .and_then(|s| s.chars().next())?;

    let font = obj.get("font").map(parse_font).unwrap_or(Font::DEFAULT);

    let size = obj
        .get("size")
        .and_then(|v| v.as_f64())
        .map(|v| Pixels(v as f32));

    let line_height = parse_line_height(Some(obj)).unwrap_or(LineHeight::Relative(1.2));

    let shaping = parse_shaping(Some(obj)).unwrap_or(iced::widget::text::Shaping::Basic);

    Some(pick_list::Icon {
        font,
        code_point,
        size,
        line_height,
        shaping,
    })
}

/// Parse a PickList Handle from props.
pub(crate) fn parse_pick_list_handle(props: Props<'_>) -> Option<pick_list::Handle<Font>> {
    let handle_obj = props?.get("handle")?.as_object()?;
    let handle_type = handle_obj.get("type")?.as_str()?;

    match handle_type {
        "arrow" => {
            let size = handle_obj
                .get("size")
                .and_then(|v| v.as_f64())
                .map(|v| Pixels(v as f32));
            Some(pick_list::Handle::Arrow { size })
        }
        "static" => {
            let icon = parse_pick_list_icon(handle_obj.get("icon")?)?;
            Some(pick_list::Handle::Static(icon))
        }
        "dynamic" => {
            let closed = parse_pick_list_icon(handle_obj.get("closed")?)?;
            let open = parse_pick_list_icon(handle_obj.get("open")?)?;
            Some(pick_list::Handle::Dynamic { closed, open })
        }
        "none" => Some(pick_list::Handle::None),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Helper: build a Props from a json! value. The value must be an object.
    fn make_props(v: &Value) -> Props<'_> {
        v.as_object()
    }

    // -- prop_f32 --

    #[test]
    fn prop_f32_returns_number() {
        let v = json!({"size": 16.0});
        assert_eq!(prop_f32(make_props(&v), "size"), Some(16.0));
    }

    #[test]
    fn prop_f32_parses_string() {
        let v = json!({"size": "24.5"});
        assert_eq!(prop_f32(make_props(&v), "size"), Some(24.5));
    }

    #[test]
    fn prop_f32_returns_none_for_missing_key() {
        let v = json!({"other": 10});
        assert_eq!(prop_f32(make_props(&v), "size"), None);
    }

    #[test]
    fn prop_f32_returns_none_for_bool() {
        let v = json!({"size": true});
        assert_eq!(prop_f32(make_props(&v), "size"), None);
    }

    // -- prop_bool --

    #[test]
    fn prop_bool_returns_true() {
        let v = json!({"visible": true});
        assert_eq!(prop_bool(make_props(&v), "visible"), Some(true));
    }

    #[test]
    fn prop_bool_returns_false() {
        let v = json!({"visible": false});
        assert_eq!(prop_bool(make_props(&v), "visible"), Some(false));
    }

    #[test]
    fn prop_bool_returns_none_for_missing() {
        let v = json!({"other": 1});
        assert_eq!(prop_bool(make_props(&v), "visible"), None);
    }

    #[test]
    fn prop_bool_default_uses_fallback() {
        let v = json!({});
        assert!(prop_bool_default(make_props(&v), "clip", true));
        assert!(!prop_bool_default(make_props(&v), "clip", false));
    }

    // -- prop_str --

    #[test]
    fn prop_str_returns_string() {
        let v = json!({"label": "hello"});
        assert_eq!(prop_str(make_props(&v), "label"), Some("hello".to_string()));
    }

    // -- prop_length --

    #[test]
    fn prop_length_fill_string() {
        let v = json!({"width": "fill"});
        assert_eq!(prop_length(make_props(&v), "width", Length::Shrink), Fill);
    }

    #[test]
    fn prop_length_shrink_string() {
        let v = json!({"width": "shrink"});
        assert_eq!(prop_length(make_props(&v), "width", Fill), Length::Shrink);
    }

    #[test]
    fn prop_length_fixed_number() {
        let v = json!({"width": 200.0});
        assert_eq!(
            prop_length(make_props(&v), "width", Length::Shrink),
            Length::Fixed(200.0)
        );
    }

    #[test]
    fn prop_length_fill_portion_object() {
        let v = json!({"width": {"fill_portion": 3}});
        assert_eq!(
            prop_length(make_props(&v), "width", Length::Shrink),
            Length::FillPortion(3)
        );
    }

    #[test]
    fn prop_length_returns_fallback_for_missing() {
        let v = json!({});
        assert_eq!(prop_length(make_props(&v), "width", Fill), Fill);
    }

    #[test]
    fn prop_length_numeric_string() {
        let v = json!({"width": "150"});
        assert_eq!(
            prop_length(make_props(&v), "width", Length::Shrink),
            Length::Fixed(150.0)
        );
    }

    // -- parse_color --

    #[test]
    fn parse_color_hex_rrggbb() {
        let v = json!("#ff0000");
        let c = parse_color(&v).unwrap();
        assert_eq!(c, Color::from_rgb8(255, 0, 0));
    }

    #[test]
    fn parse_color_hex_rrggbbaa() {
        let v = json!("#00ff0080");
        let c = parse_color(&v).unwrap();
        assert_eq!(c, Color::from_rgba8(0, 255, 0, 128.0 / 255.0));
    }

    #[test]
    fn parse_color_object_rgba() {
        let v = json!({"r": 0.5, "g": 0.25, "b": 0.75, "a": 0.8});
        let c = parse_color(&v).unwrap();
        assert_eq!(c, Color::from_rgba(0.5, 0.25, 0.75, 0.8));
    }

    #[test]
    fn parse_color_object_defaults_alpha_to_one() {
        let v = json!({"r": 1.0, "g": 0.0, "b": 0.0});
        let c = parse_color(&v).unwrap();
        assert_eq!(c, Color::from_rgba(1.0, 0.0, 0.0, 1.0));
    }

    #[test]
    fn parse_color_returns_none_for_bad_hex() {
        let v = json!("#xyz");
        assert!(parse_color(&v).is_none());
    }

    #[test]
    fn parse_color_returns_none_for_number() {
        let v = json!(42);
        assert!(parse_color(&v).is_none());
    }

    // -- parse_font --

    #[test]
    fn parse_font_monospace_string() {
        let v = json!("monospace");
        let f = parse_font(&v);
        assert_eq!(f, Font::MONOSPACE);
    }

    #[test]
    fn parse_font_default_string() {
        let v = json!("default");
        let f = parse_font(&v);
        assert_eq!(f, Font::DEFAULT);
    }

    #[test]
    fn parse_font_object_with_weight_and_style() {
        let v = json!({"weight": "bold", "style": "italic"});
        let f = parse_font(&v);
        assert_eq!(f.weight, font::Weight::Bold);
        assert_eq!(f.style, font::Style::Italic);
    }

    #[test]
    fn parse_font_object_serif_family() {
        let v = json!({"family": "serif"});
        let f = parse_font(&v);
        assert_eq!(f.family, font::Family::Serif);
    }

    #[test]
    fn parse_font_monospace_preserves_weight_and_style() {
        let v = json!({"family": "monospace", "weight": "bold", "style": "italic"});
        let f = parse_font(&v);
        assert_eq!(f.family, font::Family::Monospace);
        assert_eq!(f.weight, font::Weight::Bold);
        assert_eq!(f.style, font::Style::Italic);
    }

    // -- parse_padding_value --

    #[test]
    fn parse_padding_uniform_number() {
        let v = json!({"padding": 10});
        let p = parse_padding_value(make_props(&v));
        assert_eq!(p.top, 10.0);
        assert_eq!(p.right, 10.0);
        assert_eq!(p.bottom, 10.0);
        assert_eq!(p.left, 10.0);
    }

    #[test]
    fn parse_padding_per_side_object() {
        let v = json!({"padding": {"top": 1, "right": 2, "bottom": 3, "left": 4}});
        let p = parse_padding_value(make_props(&v));
        assert_eq!(p.top, 1.0);
        assert_eq!(p.right, 2.0);
        assert_eq!(p.bottom, 3.0);
        assert_eq!(p.left, 4.0);
    }

    #[test]
    fn parse_padding_defaults_to_zero() {
        let v = json!({});
        let p = parse_padding_value(make_props(&v));
        assert_eq!(p.top, 0.0);
        assert_eq!(p.right, 0.0);
        assert_eq!(p.bottom, 0.0);
        assert_eq!(p.left, 0.0);
    }

    // -- parse_border --

    #[test]
    fn parse_border_with_all_fields() {
        let v = json!({"color": "#ff0000", "width": 2.0, "radius": 8.0});
        let b = parse_border(&v);
        assert_eq!(b.color, Color::from_rgb8(255, 0, 0));
        assert_eq!(b.width, 2.0);
    }

    #[test]
    fn parse_border_defaults_for_non_object() {
        let v = json!("not an object");
        let b = parse_border(&v);
        assert_eq!(b, Border::default());
    }

    // -- parse_shadow --

    #[test]
    fn parse_shadow_with_all_fields() {
        let v = json!({"color": "#000000", "offset": [3.0, 4.0], "blur_radius": 5.0});
        let s = parse_shadow(&v);
        assert_eq!(s.color, Color::from_rgb8(0, 0, 0));
        assert_eq!(s.offset, Vector::new(3.0, 4.0));
        assert_eq!(s.blur_radius, 5.0);
    }

    #[test]
    fn parse_shadow_defaults_for_non_object() {
        let v = json!(42);
        let s = parse_shadow(&v);
        assert_eq!(s, Shadow::default());
    }

    // -- Style map tests --

    #[test]
    fn style_map_parse_overrides_basic() {
        let obj = json!({
            "background": "#ff0000",
            "text_color": "#00ff00",
            "border": {"color": "#0000ff", "width": 2.0, "radius": 4.0},
            "hovered": {
                "background": "#880000",
                "text_color": "#008800"
            },
            "pressed": {
                "background": "#440000"
            },
            "disabled": {
                "text_color": "#999999"
            },
            "focused": {
                "border": {"color": "#ffffff", "width": 3.0, "radius": 0.0}
            }
        });
        let map = obj.as_object().unwrap();
        let overrides = parse_style_overrides(map);

        // Base fields
        assert!(overrides.base.background.is_some());
        assert!(overrides.base.text_color.is_some());
        assert!(overrides.base.border.is_some());
        assert_eq!(
            overrides.base.text_color.unwrap(),
            Color::from_rgb8(0, 255, 0)
        );

        // Hovered override present with both fields
        let hovered = overrides.hovered.unwrap();
        assert!(hovered.background.is_some());
        assert!(hovered.text_color.is_some());

        // Pressed override present with background only
        let pressed = overrides.pressed.unwrap();
        assert!(pressed.background.is_some());
        assert!(pressed.text_color.is_none());

        // Disabled override present with text_color only
        let disabled = overrides.disabled.unwrap();
        assert!(disabled.background.is_none());
        assert!(disabled.text_color.is_some());

        // Focused override present with border only
        let focused = overrides.focused.unwrap();
        assert!(focused.border.is_some());
        assert!(focused.background.is_none());
    }

    #[test]
    fn style_map_parse_overrides_missing() {
        // Only base fields, no status overrides at all.
        let obj = json!({"background": "#aabbcc"});
        let map = obj.as_object().unwrap();
        let overrides = parse_style_overrides(map);

        assert!(overrides.base.background.is_some());
        assert!(overrides.hovered.is_none());
        assert!(overrides.pressed.is_none());
        assert!(overrides.disabled.is_none());
        assert!(overrides.focused.is_none());
    }

    #[test]
    fn style_map_auto_derive_hover() {
        // darken_background multiplies RGB by factor (0.9), alpha unchanged.
        let bg = Some(iced::Background::Color(Color::from_rgba(
            1.0, 0.5, 0.0, 1.0,
        )));
        let result = auto_derive_hover_bg(bg);
        match result {
            Some(iced::Background::Color(c)) => {
                // 1.0 * 0.9 = 0.9
                assert!((c.r - 0.9).abs() < 0.001);
                // 0.5 * 0.9 = 0.45
                assert!((c.g - 0.45).abs() < 0.001);
                // 0.0 * 0.9 = 0.0
                assert!((c.b - 0.0).abs() < 0.001);
                // alpha unchanged
                assert!((c.a - 1.0).abs() < 0.001);
            }
            other => panic!("expected Background::Color, got {other:?}"),
        }
    }

    #[test]
    fn style_map_auto_derive_disabled_bg() {
        // alpha_background multiplies alpha by 0.5, RGB unchanged.
        let bg = Some(iced::Background::Color(Color::from_rgba(
            0.8, 0.6, 0.4, 1.0,
        )));
        let result = auto_derive_disabled_bg(bg);
        match result {
            Some(iced::Background::Color(c)) => {
                assert!((c.r - 0.8).abs() < 0.001);
                assert!((c.g - 0.6).abs() < 0.001);
                assert!((c.b - 0.4).abs() < 0.001);
                // 1.0 * 0.5 = 0.5
                assert!((c.a - 0.5).abs() < 0.001);
            }
            other => panic!("expected Background::Color, got {other:?}"),
        }
    }

    #[test]
    fn style_map_auto_derive_disabled_text() {
        let color = Color::from_rgba(1.0, 1.0, 1.0, 0.8);
        let result = auto_derive_disabled_text(color);
        // RGB unchanged, alpha halved: 0.8 * 0.5 = 0.4
        assert!((result.r - 1.0).abs() < 0.001);
        assert!((result.g - 1.0).abs() < 0.001);
        assert!((result.b - 1.0).abs() < 0.001);
        assert!((result.a - 0.4).abs() < 0.001);
    }
}
