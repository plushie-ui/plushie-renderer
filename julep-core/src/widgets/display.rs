use super::*;

use iced::widget::canvas;

// ---------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------

pub(crate) fn render_text<'a>(
    node: &'a TreeNode,
    caches: &'a WidgetCaches,
) -> Element<'a, Message> {
    let props = node.props.as_object();
    let content = prop_str(props, "content").unwrap_or_default();
    let size = prop_f32(props, "size").or(caches.default_text_size);

    let mut t = text(content);
    if let Some(s) = size {
        t = t.size(s);
    }
    let font = props
        .and_then(|p| p.get("font"))
        .map(parse_font)
        .or(caches.default_font);
    if let Some(f) = font {
        t = t.font(f);
    }
    if let Some(c) = props.and_then(|p| p.get("color")).and_then(parse_color) {
        t = t.color(c);
    }
    if let Some(w) = value_to_length_opt(props.and_then(|p| p.get("width"))) {
        t = t.width(w);
    }
    if let Some(h) = value_to_length_opt(props.and_then(|p| p.get("height"))) {
        t = t.height(h);
    }
    if let Some(lh) = parse_line_height(props) {
        t = t.line_height(lh);
    }
    if let Some(ax) = props
        .and_then(|p| p.get("align_x"))
        .and_then(|v| v.as_str())
        .and_then(value_to_horizontal_alignment)
    {
        t = t.align_x(ax);
    }
    if let Some(ay) = props
        .and_then(|p| p.get("align_y"))
        .and_then(|v| v.as_str())
        .and_then(value_to_vertical_alignment)
    {
        t = t.align_y(ay);
    }
    if let Some(w) = parse_wrapping(props) {
        t = t.wrapping(w);
    }
    if let Some(shaping) = parse_shaping(props) {
        t = t.shaping(shaping);
    }

    // Named style
    if let Some(style_name) = prop_str(props, "style") {
        t = match style_name.as_str() {
            "primary" => t.style(text::primary),
            "secondary" => t.style(text::secondary),
            "success" => t.style(text::success),
            "danger" => t.style(text::danger),
            "warning" => t.style(text::warning),
            _ => {
                log::warn!(
                    "unknown style {:?} for widget type {:?}, using default",
                    style_name,
                    "text"
                );
                t.style(text::default)
            }
        };
    }

    t.into()
}

// ---------------------------------------------------------------------------
// Rich Text
// ---------------------------------------------------------------------------

pub(crate) fn render_rich_text<'a>(
    node: &'a TreeNode,
    caches: &'a WidgetCaches,
) -> Element<'a, Message> {
    let props = node.props.as_object();
    let width = prop_length(props, "width", Length::Shrink);
    let height = prop_length(props, "height", Length::Shrink);

    // spans is an array of objects: {text, size, color, font, link}
    let spans_value = props
        .and_then(|p| p.get("spans"))
        .and_then(|v| v.as_array());

    let span_list: Vec<iced::widget::text::Span<'a, String, Font>> = spans_value
        .map(|arr| {
            arr.iter()
                .map(|sv| {
                    let content = sv
                        .get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_owned();
                    let mut s = span(content);
                    if let Some(sz) = sv.get("size").and_then(|v| v.as_f64()) {
                        s = s.size(Pixels(sz as f32));
                    }
                    if let Some(c) = sv.get("color").and_then(parse_color) {
                        s = s.color(c);
                    }
                    if let Some(f) = sv.get("font") {
                        s = s.font(parse_font(f));
                    }
                    if let Some(link) = sv.get("link").and_then(|v| v.as_str()) {
                        s = s.link(link.to_owned());
                    }
                    s
                })
                .collect()
        })
        .unwrap_or_default();

    let id = node.id.clone();
    let mut rt = rich_text(span_list).width(width).height(height);

    if let Some(sz) = prop_f32(props, "size").or(caches.default_text_size) {
        rt = rt.size(sz);
    }
    let font = props
        .and_then(|p| p.get("font"))
        .map(parse_font)
        .or(caches.default_font);
    if let Some(f) = font {
        rt = rt.font(f);
    }
    if let Some(c) = props.and_then(|p| p.get("color")).and_then(parse_color) {
        rt = rt.color(c);
    }
    if let Some(lh) = parse_line_height(props) {
        rt = rt.line_height(lh);
    }

    rt = rt.on_link_click(move |link| Message::Click(format!("{}:{}", id, link)));

    rt.into()
}

// ---------------------------------------------------------------------------
// Image
// ---------------------------------------------------------------------------

pub(crate) fn render_image<'a>(
    node: &'a TreeNode,
    images: &'a crate::image_registry::ImageRegistry,
) -> Element<'a, Message> {
    use iced::widget::Image;
    use iced::widget::image::FilterMethod;

    let props = node.props.as_object();
    let width = prop_length(props, "width", Length::Shrink);
    let height = prop_length(props, "height", Length::Shrink);
    let content_fit = prop_content_fit(props);

    // source can be a string (file path) or an object with a "handle" field
    // (in-memory image from the registry).
    let source_val = props.and_then(|p| p.get("source"));
    if source_val.is_none() {
        log::warn!("[id={}] image: no 'source' prop specified", node.id);
    }
    let handle: iced::widget::image::Handle = match source_val {
        Some(Value::Object(obj)) => {
            if let Some(name) = obj.get("handle").and_then(|v| v.as_str()) {
                match images.get(name) {
                    Some(h) => h.clone(),
                    None => {
                        log::warn!("[id={}] image: unknown registry handle: {name}", node.id);
                        iced::widget::image::Handle::from_bytes(vec![])
                    }
                }
            } else {
                iced::widget::image::Handle::from_bytes(vec![])
            }
        }
        _ => {
            let path = prop_str(props, "source").unwrap_or_default();
            iced::widget::image::Handle::from_path(path)
        }
    };

    let mut img = Image::new(handle).width(width).height(height);
    if let Some(cf) = content_fit {
        img = img.content_fit(cf);
    }
    if let Some(r) = prop_f32(props, "rotation") {
        img = img.rotation(Rotation::from(Radians(r.to_radians())));
    }
    if let Some(o) = prop_f32(props, "opacity") {
        img = img.opacity(o);
    }
    if let Some(br) = prop_f32(props, "border_radius") {
        img = img.border_radius(br);
    }
    if let Some(fm_str) = prop_str(props, "filter_method") {
        let fm = match fm_str.to_ascii_lowercase().as_str() {
            "nearest" => FilterMethod::Nearest,
            _ => FilterMethod::Linear,
        };
        img = img.filter_method(fm);
    }
    if let Some(expand) = prop_bool(props, "expand") {
        img = img.expand(expand);
    }
    if let Some(scale) = prop_f32(props, "scale") {
        img = img.scale(scale);
    }
    if let Some(alt) = prop_str(props, "alt") {
        img = img.alt(alt);
    }
    if let Some(desc) = prop_str(props, "description") {
        img = img.description(desc);
    }
    // crop: {"x": u32, "y": u32, "width": u32, "height": u32}
    if let Some(crop_obj) = props
        .and_then(|p| p.get("crop"))
        .and_then(|v| v.as_object())
    {
        let cx = crop_obj.get("x").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let cy = crop_obj.get("y").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let cw = crop_obj.get("width").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let ch = crop_obj.get("height").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        img = img.crop(iced::Rectangle {
            x: cx,
            y: cy,
            width: cw,
            height: ch,
        });
    }

    img.into()
}

// ---------------------------------------------------------------------------
// SVG
// ---------------------------------------------------------------------------

pub(crate) fn render_svg<'a>(node: &'a TreeNode) -> Element<'a, Message> {
    use iced::widget::Svg;

    let props = node.props.as_object();
    let source = prop_str(props, "source").unwrap_or_default();
    if source.is_empty() {
        log::warn!("[id={}] svg: no 'source' prop specified", node.id);
    }
    let width = prop_length(props, "width", Length::Shrink);
    let height = prop_length(props, "height", Length::Shrink);
    let content_fit = prop_content_fit(props);

    let mut s = Svg::from_path(source).width(width).height(height);
    if let Some(cf) = content_fit {
        s = s.content_fit(cf);
    }
    if let Some(r) = prop_f32(props, "rotation") {
        s = s.rotation(Rotation::from(Radians(r.to_radians())));
    }
    if let Some(o) = prop_f32(props, "opacity") {
        s = s.opacity(o);
    }
    if let Some(alt) = prop_str(props, "alt") {
        s = s.alt(alt);
    }
    if let Some(desc) = prop_str(props, "description") {
        s = s.description(desc);
    }
    if let Some(color_str) = prop_str(props, "color")
        && let Some(c) = crate::theming::parse_hex_color(&color_str)
    {
        s = s.style(move |_theme, _status| iced::widget::svg::Style { color: Some(c) });
    }

    s.into()
}

// ---------------------------------------------------------------------------
// Markdown
// ---------------------------------------------------------------------------

pub(crate) fn render_markdown<'a>(
    node: &'a TreeNode,
    caches: &'a WidgetCaches,
    theme: &'a iced::Theme,
) -> Element<'a, Message> {
    use iced::widget::markdown;

    let props = node.props.as_object();
    let items = match caches.markdown_items.get(&node.id) {
        Some((_hash, items)) => items.as_slice(),
        None => {
            log::warn!("markdown cache miss for id={}", node.id);
            return text("(markdown: cache miss)").into();
        }
    };

    // Build markdown Settings from props, falling back to theme defaults.
    let link_color = prop_color(props, "link_color");
    let settings =
        if let Some(text_size) = prop_f32(props, "text_size").or(caches.default_text_size) {
            let mut s = markdown::Settings::with_text_size(text_size, markdown::Style::from(theme));
            if let Some(v) = prop_f32(props, "h1_size") {
                s.h1_size = Pixels(v);
            }
            if let Some(v) = prop_f32(props, "h2_size") {
                s.h2_size = Pixels(v);
            }
            if let Some(v) = prop_f32(props, "h3_size") {
                s.h3_size = Pixels(v);
            }
            if let Some(v) = prop_f32(props, "code_size") {
                s.code_size = Pixels(v);
            }
            if let Some(v) = prop_f32(props, "spacing") {
                s.spacing = Pixels(v);
            }
            if let Some(lc) = link_color {
                s.style.link_color = lc;
            }
            s
        } else {
            let mut s = markdown::Settings::from(theme);
            if let Some(v) = prop_f32(props, "h1_size") {
                s.h1_size = Pixels(v);
            }
            if let Some(v) = prop_f32(props, "h2_size") {
                s.h2_size = Pixels(v);
            }
            if let Some(v) = prop_f32(props, "h3_size") {
                s.h3_size = Pixels(v);
            }
            if let Some(v) = prop_f32(props, "code_size") {
                s.code_size = Pixels(v);
            }
            if let Some(v) = prop_f32(props, "spacing") {
                s.spacing = Pixels(v);
            }
            if let Some(lc) = link_color {
                s.style.link_color = lc;
            }
            s
        };

    let mut md: Element<'a, Message> = markdown::view(items, settings).map(Message::MarkdownUrl);

    // Wrap in container if width is specified
    if let Some(w) = value_to_length_opt(props.and_then(|p| p.get("width"))) {
        md = container(md).width(w).into();
    }

    md
}

// ---------------------------------------------------------------------------
// Progress Bar
// ---------------------------------------------------------------------------

pub(crate) fn render_progress_bar<'a>(node: &'a TreeNode) -> Element<'a, Message> {
    let props = node.props.as_object();
    let range = prop_range_f32(props);
    let value = prop_f32(props, "value")
        .unwrap_or(0.0)
        .clamp(*range.start(), *range.end());
    let width = prop_length(props, "width", Length::Fill);
    let height = prop_length(props, "height", Length::Shrink);

    let mut pb = progress_bar(range, value).length(width).girth(height);

    if prop_bool_default(props, "vertical", false) {
        pb = pb.vertical();
    }

    // Style: string name or style map object
    if let Some(style_val) = props.and_then(|p| p.get("style")) {
        if let Some(style_name) = style_val.as_str() {
            pb = match style_name {
                "primary" => pb.style(progress_bar::primary),
                "secondary" => pb.style(progress_bar::secondary),
                "success" => pb.style(progress_bar::success),
                "danger" => pb.style(progress_bar::danger),
                "warning" => pb.style(progress_bar::warning),
                _ => {
                    log::warn!(
                        "unknown style {:?} for widget type {:?}, using default",
                        style_name,
                        "progress_bar"
                    );
                    pb.style(progress_bar::primary)
                }
            };
        } else if let Some(obj) = style_val.as_object() {
            let ov = parse_style_overrides(obj);
            pb = pb.style(move |theme: &iced::Theme| {
                let mut style = match ov.preset_base.as_deref() {
                    Some("primary") => progress_bar::primary(theme),
                    Some("secondary") => progress_bar::secondary(theme),
                    Some("success") => progress_bar::success(theme),
                    Some("danger") => progress_bar::danger(theme),
                    Some("warning") => progress_bar::warning(theme),
                    _ => progress_bar::primary(theme),
                };
                apply_progress_bar_fields(&mut style, &ov.base);
                style
            });
        }
    }

    pb.into()
}

// ---------------------------------------------------------------------------
// Rule (horizontal/vertical divider)
// ---------------------------------------------------------------------------

pub(crate) fn render_rule<'a>(node: &'a TreeNode) -> Element<'a, Message> {
    let props = node.props.as_object();
    let direction = prop_str(props, "direction").unwrap_or_default();

    // Thickness is the cross-axis dimension:
    // horizontal rule -> height, vertical rule -> width.
    // "thickness" is a universal alias for either.
    let thickness = if direction == "vertical" {
        prop_f32(props, "width")
    } else {
        prop_f32(props, "height")
    }
    .or_else(|| prop_f32(props, "thickness"))
    .unwrap_or(1.0);

    if direction == "vertical" {
        let mut r = rule::vertical(thickness);
        if let Some(style_val) = props.and_then(|p| p.get("style")) {
            if let Some(style_name) = style_val.as_str() {
                r = match style_name {
                    "default" => r.style(rule::default),
                    "weak" => r.style(rule::weak),
                    _ => {
                        log::warn!(
                            "unknown style {:?} for widget type {:?}, using default",
                            style_name,
                            "rule"
                        );
                        r
                    }
                };
            } else if let Some(obj) = style_val.as_object() {
                let ov = parse_style_overrides(obj);
                r = r.style(move |theme: &iced::Theme| {
                    let base_fn: fn(&iced::Theme) -> rule::Style = match ov.preset_base.as_deref() {
                        Some("default") => rule::default,
                        Some("weak") => rule::weak,
                        _ => rule::default,
                    };
                    apply_rule_style(&mut base_fn(theme), &ov.base)
                });
            }
        }
        r.into()
    } else {
        let mut r = rule::horizontal(thickness);
        if let Some(style_val) = props.and_then(|p| p.get("style")) {
            if let Some(style_name) = style_val.as_str() {
                r = match style_name {
                    "default" => r.style(rule::default),
                    "weak" => r.style(rule::weak),
                    _ => {
                        log::warn!(
                            "unknown style {:?} for widget type {:?}, using default",
                            style_name,
                            "rule"
                        );
                        r
                    }
                };
            } else if let Some(obj) = style_val.as_object() {
                let ov = parse_style_overrides(obj);
                r = r.style(move |theme: &iced::Theme| {
                    let base_fn: fn(&iced::Theme) -> rule::Style = match ov.preset_base.as_deref() {
                        Some("default") => rule::default,
                        Some("weak") => rule::weak,
                        _ => rule::default,
                    };
                    apply_rule_style(&mut base_fn(theme), &ov.base)
                });
            }
        }
        r.into()
    }
}

// ---------------------------------------------------------------------------
// Space
// ---------------------------------------------------------------------------

pub(crate) fn render_space<'a>(node: &'a TreeNode) -> Element<'a, Message> {
    let props = node.props.as_object();
    let width = prop_length(props, "width", Length::Shrink);
    let height = prop_length(props, "height", Length::Shrink);
    Space::new().width(width).height(height).into()
}

// ---------------------------------------------------------------------------
// QR Code
// ---------------------------------------------------------------------------

struct QrCodeProgram<'a> {
    modules: Vec<Vec<bool>>,
    cell_size: f32,
    cell_color: Color,
    background_color: Color,
    cache: Option<&'a (u64, canvas::Cache)>,
}

impl canvas::Program<Message> for QrCodeProgram<'_> {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: iced::Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let draw_fn = |frame: &mut canvas::Frame| {
            // Fill background
            frame.fill_rectangle(Point::ORIGIN, bounds.size(), self.background_color);
            // Draw each dark module as a filled square
            for (row_idx, row) in self.modules.iter().enumerate() {
                for (col_idx, &dark) in row.iter().enumerate() {
                    if dark {
                        let x = col_idx as f32 * self.cell_size;
                        let y = row_idx as f32 * self.cell_size;
                        frame.fill_rectangle(
                            Point::new(x, y),
                            Size::new(self.cell_size, self.cell_size),
                            self.cell_color,
                        );
                    }
                }
            }
        };

        if let Some((_hash, cache)) = self.cache {
            vec![cache.draw(renderer, bounds.size(), draw_fn)]
        } else {
            let mut frame = canvas::Frame::new(renderer, bounds.size());
            draw_fn(&mut frame);
            vec![frame.into_geometry()]
        }
    }
}

pub(crate) fn render_qr_code<'a>(
    node: &'a TreeNode,
    caches: &'a WidgetCaches,
) -> Element<'a, Message> {
    let props = node.props.as_object();
    let data = prop_str(props, "data").unwrap_or_default();
    let cell_size = prop_f32(props, "cell_size").unwrap_or(4.0);
    let ec_str = prop_str(props, "error_correction").unwrap_or_default();
    let cell_color = prop_str(props, "cell_color")
        .and_then(|s| parse_hex_color(&s))
        .unwrap_or(Color::BLACK);
    let background_color = prop_str(props, "background_color")
        .and_then(|s| parse_hex_color(&s))
        .unwrap_or(Color::WHITE);

    let ec_level = match ec_str.as_str() {
        "low" => qrcode::EcLevel::L,
        "quartile" => qrcode::EcLevel::Q,
        "high" => qrcode::EcLevel::H,
        _ => qrcode::EcLevel::M,
    };

    let qr = match qrcode::QrCode::with_error_correction_level(data.as_bytes(), ec_level) {
        Ok(qr) => qr,
        Err(e) => {
            log::warn!("[id={}] qr_code: failed to encode data: {e}", node.id);
            return text(format!("QR code error: {e}")).into();
        }
    };

    let width = qr.width();
    let modules: Vec<Vec<bool>> = (0..width)
        .map(|y| {
            (0..width)
                .map(|x| qr[(x, y)] == qrcode::types::Color::Dark)
                .collect()
        })
        .collect();

    let pixel_size = width as f32 * cell_size;

    let cache_entry = caches.qr_code_caches.get(&node.id);

    canvas(QrCodeProgram {
        modules,
        cell_size,
        cell_color,
        background_color,
        cache: cache_entry,
    })
    .width(Length::Fixed(pixel_size))
    .height(Length::Fixed(pixel_size))
    .into()
}
