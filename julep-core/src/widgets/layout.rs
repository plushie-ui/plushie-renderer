use super::*;

// ---------------------------------------------------------------------------
// Column
// ---------------------------------------------------------------------------

pub(crate) fn render_column<'a>(
    node: &'a TreeNode,
    caches: &'a WidgetCaches,
    images: &'a crate::image_registry::ImageRegistry,
    theme: &'a iced::Theme,
    dispatcher: &'a ExtensionDispatcher,
) -> Element<'a, Message> {
    let props = node.props.as_object();
    let spacing = prop_f32(props, "spacing").unwrap_or(0.0);
    let padding = parse_padding_value(props);
    let width = prop_length(props, "width", Length::Shrink);
    let height = prop_length(props, "height", Length::Shrink);
    let align_x = prop_horizontal_alignment(props, "align_x");
    let clip = prop_bool_default(props, "clip", false);

    let children = super::render_children(node, caches, images, theme, dispatcher);

    let mut col = column(children)
        .spacing(spacing)
        .padding(padding)
        .width(width)
        .height(height)
        .align_x(align_x)
        .clip(clip);

    if let Some(mw) = prop_f32(props, "max_width") {
        col = col.max_width(mw);
    }

    let elem: Element<'a, Message> = if prop_bool_default(props, "wrap", false) {
        col.wrap().into()
    } else {
        col.into()
    };

    container(elem).id(widget::Id::from(node.id.clone())).into()
}

// ---------------------------------------------------------------------------
// Row
// ---------------------------------------------------------------------------

pub(crate) fn render_row<'a>(
    node: &'a TreeNode,
    caches: &'a WidgetCaches,
    images: &'a crate::image_registry::ImageRegistry,
    theme: &'a iced::Theme,
    dispatcher: &'a ExtensionDispatcher,
) -> Element<'a, Message> {
    let props = node.props.as_object();
    let spacing = prop_f32(props, "spacing").unwrap_or(0.0);
    let padding = parse_padding_value(props);
    let width = prop_length(props, "width", Length::Shrink);
    let height = prop_length(props, "height", Length::Shrink);
    let align_y = prop_vertical_alignment(props, "align_y");
    let clip = prop_bool_default(props, "clip", false);

    let children = super::render_children(node, caches, images, theme, dispatcher);

    let r = row(children)
        .spacing(spacing)
        .padding(padding)
        .width(width)
        .height(height)
        .align_y(align_y)
        .clip(clip);

    let max_width = prop_f32(props, "max_width");

    let elem: Element<'a, Message> = if prop_bool_default(props, "wrap", false) {
        r.wrap().into()
    } else {
        r.into()
    };

    // Row doesn't have max_width natively; wrap in a container to constrain it.
    let row_elem = if let Some(mw) = max_width {
        container(elem).max_width(mw).into()
    } else {
        elem
    };

    container(row_elem)
        .id(widget::Id::from(node.id.clone()))
        .into()
}

// ---------------------------------------------------------------------------
// Container
// ---------------------------------------------------------------------------

pub(crate) fn render_container<'a>(
    node: &'a TreeNode,
    caches: &'a WidgetCaches,
    images: &'a crate::image_registry::ImageRegistry,
    theme: &'a iced::Theme,
    dispatcher: &'a ExtensionDispatcher,
) -> Element<'a, Message> {
    let props = node.props.as_object();
    let padding = parse_padding_value(props);
    let width = prop_length(props, "width", Length::Shrink);
    let height = prop_length(props, "height", Length::Shrink);
    let center = prop_bool_default(props, "center", false);
    let clip = prop_bool_default(props, "clip", false);

    let child: Element<'a, Message> = node
        .children
        .first()
        .map(|c| super::render(c, caches, images, theme, dispatcher))
        .unwrap_or_else(|| Space::new().into());

    let mut c = container(child)
        .padding(padding)
        .width(width)
        .height(height)
        .clip(clip);

    if let Some(mw) = prop_f32(props, "max_width") {
        c = c.max_width(mw);
    }
    if let Some(mh) = prop_f32(props, "max_height") {
        c = c.max_height(mh);
    }

    if center {
        c = c.center(Fill);
    }

    if let Some(ax) = props
        .and_then(|p| p.get("align_x"))
        .and_then(|v| v.as_str())
        .and_then(value_to_horizontal_alignment)
    {
        c = c.align_x(ax);
    }
    if let Some(ay) = props
        .and_then(|p| p.get("align_y"))
        .and_then(|v| v.as_str())
        .and_then(value_to_vertical_alignment)
    {
        c = c.align_y(ay);
    }

    // Inline styling via custom style closure
    let bg = props
        .and_then(|p| p.get("background"))
        .and_then(parse_background);
    let text_color = props.and_then(|p| p.get("color")).and_then(parse_color);
    let border_val = props.and_then(|p| p.get("border")).map(parse_border);
    let shadow_val = props.and_then(|p| p.get("shadow")).map(parse_shadow);
    let has_inline_style =
        bg.is_some() || text_color.is_some() || border_val.is_some() || shadow_val.is_some();

    if has_inline_style {
        c = c.style(move |_theme| {
            let mut style = container::Style {
                background: bg,
                text_color,
                ..Default::default()
            };
            if let Some(b) = border_val {
                style.border = b;
            }
            if let Some(s) = shadow_val {
                style.shadow = s;
            }
            style
        });
    }

    // Named style or style map (overrides inline if both present)
    if let Some(style_val) = props.and_then(|p| p.get("style")) {
        if let Some(style_name) = style_val.as_str() {
            c = match style_name {
                "transparent" => c.style(container::transparent),
                "rounded_box" => c.style(container::rounded_box),
                "bordered_box" => c.style(container::bordered_box),
                "dark" => c.style(container::dark),
                "primary" => c.style(container::primary),
                "secondary" => c.style(container::secondary),
                "success" => c.style(container::success),
                "danger" => c.style(container::danger),
                "warning" => c.style(container::warning),
                _ => {
                    log::warn!(
                        "unknown style {:?} for widget type {:?}, using default",
                        style_name,
                        "container"
                    );
                    c
                }
            };
        } else if let Some(obj) = style_val.as_object() {
            let ov = parse_style_overrides(obj);
            c = c.style(move |theme| {
                let mut style = match ov.preset_base.as_deref() {
                    Some("transparent") => container::transparent(theme),
                    Some("rounded_box") => container::rounded_box(theme),
                    Some("bordered_box") => container::bordered_box(theme),
                    Some("dark") => container::dark(theme),
                    Some("primary") => container::primary(theme),
                    Some("secondary") => container::secondary(theme),
                    Some("success") => container::success(theme),
                    Some("danger") => container::danger(theme),
                    Some("warning") => container::warning(theme),
                    _ => container::Style::default(),
                };
                if let Some(bg) = ov.base.background {
                    style.background = Some(bg);
                }
                if let Some(tc) = ov.base.text_color {
                    style.text_color = Some(tc);
                }
                if let Some(brd) = ov.base.border {
                    style.border = brd;
                }
                if let Some(shd) = ov.base.shadow {
                    style.shadow = shd;
                }
                style
            });
        }
    }

    // Widget ID for operations targeting
    c = c.id(widget::Id::from(node.id.clone()));

    c.into()
}

// ---------------------------------------------------------------------------
// Stack
// ---------------------------------------------------------------------------

pub(crate) fn render_stack<'a>(
    node: &'a TreeNode,
    caches: &'a WidgetCaches,
    images: &'a crate::image_registry::ImageRegistry,
    theme: &'a iced::Theme,
    dispatcher: &'a ExtensionDispatcher,
) -> Element<'a, Message> {
    let props = node.props.as_object();
    let width = prop_length(props, "width", Length::Shrink);
    let height = prop_length(props, "height", Length::Shrink);
    let clip = prop_bool_default(props, "clip", false);

    let children = super::render_children(node, caches, images, theme, dispatcher);

    Stack::with_children(children)
        .width(width)
        .height(height)
        .clip(clip)
        .into()
}

// ---------------------------------------------------------------------------
// Grid
// ---------------------------------------------------------------------------

pub(crate) fn render_grid<'a>(
    node: &'a TreeNode,
    caches: &'a WidgetCaches,
    images: &'a crate::image_registry::ImageRegistry,
    theme: &'a iced::Theme,
    dispatcher: &'a ExtensionDispatcher,
) -> Element<'a, Message> {
    let props = node.props.as_object();
    let cols = props
        .and_then(|p| p.get("columns"))
        .and_then(|v| v.as_u64())
        .unwrap_or(1) as usize;
    let spacing = prop_f32(props, "spacing").unwrap_or(0.0);

    let column_width = prop_length(props, "column_width", Length::Shrink);
    let row_height = prop_length(props, "row_height", Length::Shrink);

    let children = super::render_children(node, caches, images, theme, dispatcher);

    let mut g = grid(children).columns(cols).spacing(spacing);

    // Legacy pixel-only width/height props
    if let Some(w) = prop_f32(props, "width") {
        g = g.width(w);
    }
    if let Some(h) = prop_f32(props, "height") {
        g = g.height(h);
    }

    // Length-typed column_width: only Fixed maps to Pixels for iced's Grid::width
    if props.and_then(|p| p.get("column_width")).is_some()
        && let Length::Fixed(px) = column_width
    {
        g = g.width(px);
    }

    // Length-typed row_height: maps to Grid::height via Sizing::EvenlyDistribute
    if props.and_then(|p| p.get("row_height")).is_some() {
        g = g.height(row_height);
    }

    g.into()
}

// ---------------------------------------------------------------------------
// Pin (absolute positioning)
// ---------------------------------------------------------------------------

pub(crate) fn render_pin<'a>(
    node: &'a TreeNode,
    caches: &'a WidgetCaches,
    images: &'a crate::image_registry::ImageRegistry,
    theme: &'a iced::Theme,
    dispatcher: &'a ExtensionDispatcher,
) -> Element<'a, Message> {
    let props = node.props.as_object();
    let x = prop_f32(props, "x").unwrap_or(0.0);
    let y = prop_f32(props, "y").unwrap_or(0.0);
    let width = prop_length(props, "width", Length::Shrink);
    let height = prop_length(props, "height", Length::Shrink);

    let child: Element<'a, Message> = node
        .children
        .first()
        .map(|c| super::render(c, caches, images, theme, dispatcher))
        .unwrap_or_else(|| Space::new().into());

    pin(child)
        .position(Point::new(x, y))
        .width(width)
        .height(height)
        .into()
}

// ---------------------------------------------------------------------------
// Keyed Column
// ---------------------------------------------------------------------------

pub(crate) fn render_keyed_column<'a>(
    node: &'a TreeNode,
    caches: &'a WidgetCaches,
    images: &'a crate::image_registry::ImageRegistry,
    theme: &'a iced::Theme,
    dispatcher: &'a ExtensionDispatcher,
) -> Element<'a, Message> {
    let props = node.props.as_object();
    let spacing = prop_f32(props, "spacing").unwrap_or(0.0);
    let padding = parse_padding_value(props);
    let width = prop_length(props, "width", Length::Shrink);
    let height = prop_length(props, "height", Length::Shrink);

    let keyed_children: Vec<(u64, Element<'a, Message>)> = node
        .children
        .iter()
        .map(|c| {
            let mut hasher = DefaultHasher::new();
            c.id.hash(&mut hasher);
            let key = hasher.finish();
            let elem = super::render(c, caches, images, theme, dispatcher);
            (key, elem)
        })
        .collect();

    let mut kc = keyed::Column::with_children(keyed_children);
    kc = kc
        .spacing(spacing)
        .padding(padding)
        .width(width)
        .height(height);

    if let Some(mw) = prop_f32(props, "max_width") {
        kc = kc.max_width(mw);
    }

    kc.into()
}

// ---------------------------------------------------------------------------
// Float (floating overlay with scale/translate)
// ---------------------------------------------------------------------------

pub(crate) fn render_float<'a>(
    node: &'a TreeNode,
    caches: &'a WidgetCaches,
    images: &'a crate::image_registry::ImageRegistry,
    theme: &'a iced::Theme,
    dispatcher: &'a ExtensionDispatcher,
) -> Element<'a, Message> {
    let props = node.props.as_object();

    let child: Element<'a, Message> = node
        .children
        .first()
        .map(|c| super::render(c, caches, images, theme, dispatcher))
        .unwrap_or_else(|| Space::new().into());

    let tx = prop_f32(props, "translate_x").unwrap_or(0.0);
    let ty = prop_f32(props, "translate_y").unwrap_or(0.0);

    let mut f =
        iced::widget::float(child).translate(move |_content, _viewport| Vector::new(tx, ty));

    if let Some(s) = prop_f32(props, "scale") {
        f = f.scale(s);
    }

    f.into()
}

// ---------------------------------------------------------------------------
// Responsive (container that reports its size)
// ---------------------------------------------------------------------------

pub(crate) fn render_responsive<'a>(
    node: &'a TreeNode,
    caches: &'a WidgetCaches,
    images: &'a crate::image_registry::ImageRegistry,
    theme: &'a iced::Theme,
    dispatcher: &'a ExtensionDispatcher,
) -> Element<'a, Message> {
    // iced's Responsive widget takes a closure that receives Size and returns
    // an Element. Since we can't call back to the host within a single frame,
    // we render the children as-is and wrap in a sensor so the host receives
    // resize events with the actual measured size.
    let props = node.props.as_object();
    let width = prop_length(props, "width", Length::Fill);
    let height = prop_length(props, "height", Length::Fill);

    let child: Element<'a, Message> = node
        .children
        .first()
        .map(|c| super::render(c, caches, images, theme, dispatcher))
        .unwrap_or_else(|| Space::new().into());

    let resize_id = node.id.clone();

    sensor(container(child).width(width).height(height))
        .key(node.id.clone())
        .on_resize(move |size| Message::SensorResize(resize_id.clone(), size.width, size.height))
        .into()
}

// ---------------------------------------------------------------------------
// Scrollable
// ---------------------------------------------------------------------------

pub(crate) fn render_scrollable<'a>(
    node: &'a TreeNode,
    caches: &'a WidgetCaches,
    images: &'a crate::image_registry::ImageRegistry,
    theme: &'a iced::Theme,
    dispatcher: &'a ExtensionDispatcher,
) -> Element<'a, Message> {
    let props = node.props.as_object();
    let width = prop_length(props, "width", Length::Shrink);
    let height = prop_length(props, "height", Length::Shrink);
    let spacing = prop_f32(props, "spacing");

    let child: Element<'a, Message> = node
        .children
        .first()
        .map(|c| super::render(c, caches, images, theme, dispatcher))
        .unwrap_or_else(|| Space::new().into());

    let direction = prop_str(props, "direction").unwrap_or_default();

    // Build scrollbar configuration from props
    let build_scrollbar = |props: Props<'_>| -> scrollable::Scrollbar {
        let mut sb = scrollable::Scrollbar::default();
        if let Some(w) = prop_f32(props, "scrollbar_width") {
            sb = sb.width(w);
        }
        if let Some(m) = prop_f32(props, "scrollbar_margin") {
            sb = sb.margin(m);
        }
        if let Some(sw) = prop_f32(props, "scroller_width") {
            sb = sb.scroller_width(sw);
        }
        sb
    };

    let sb = build_scrollbar(props);
    let mut s = match direction.as_str() {
        "horizontal" => scrollable(child).direction(scrollable::Direction::Horizontal(sb)),
        "both" => scrollable(child).direction(scrollable::Direction::Both {
            vertical: sb,
            horizontal: build_scrollbar(props),
        }),
        _ => scrollable(child).direction(scrollable::Direction::Vertical(sb)),
    };

    s = s.width(width).height(height);

    // Widget ID -- always set from node.id like other widgets
    s = s.id(widget::Id::from(node.id.clone()));

    if let Some(sp) = spacing {
        s = s.spacing(sp);
    }

    // Anchor
    if let Some(anchor_str) = prop_str(props, "anchor") {
        match anchor_str.to_ascii_lowercase().as_str() {
            "end" | "bottom" | "right" => {
                s = s.anchor_y(Anchor::End);
            }
            _ => {}
        }
    }

    // on_scroll: emit viewport data when scroll position changes
    if prop_bool_default(props, "on_scroll", false) {
        let scroll_id = node.id.clone();
        s = s.on_scroll(move |viewport| {
            let abs = viewport.absolute_offset();
            let rel = viewport.relative_offset();
            let bounds = viewport.bounds();
            let content_bounds = viewport.content_bounds();
            Message::ScrollEvent(
                scroll_id.clone(),
                abs.x,
                abs.y,
                rel.x,
                rel.y,
                bounds.width,
                bounds.height,
                content_bounds.width,
                content_bounds.height,
            )
        });
    }

    // auto_scroll: automatically scroll to show new content
    if prop_bool_default(props, "auto_scroll", false) {
        s = s.auto_scroll(true);
    }

    // Scrollbar color styling
    let scrollbar_color = prop_color(props, "scrollbar_color");
    let scroller_color = prop_color(props, "scroller_color");
    if scrollbar_color.is_some() || scroller_color.is_some() {
        s = s.style(move |theme: &iced::Theme, status| {
            let mut style = scrollable::default(theme, status);
            if let Some(sc) = scrollbar_color {
                style.vertical_rail.background = Some(iced::Background::Color(sc));
                style.horizontal_rail.background = Some(iced::Background::Color(sc));
            }
            if let Some(sc) = scroller_color {
                style.vertical_rail.scroller.background = iced::Background::Color(sc);
                style.horizontal_rail.scroller.background = iced::Background::Color(sc);
            }
            style
        });
    }

    s.into()
}

// ---------------------------------------------------------------------------
// PaneGrid
// ---------------------------------------------------------------------------

pub(crate) fn render_pane_grid<'a>(
    node: &'a TreeNode,
    caches: &'a WidgetCaches,
    images: &'a crate::image_registry::ImageRegistry,
    theme: &'a iced::Theme,
    dispatcher: &'a ExtensionDispatcher,
) -> Element<'a, Message> {
    let props = node.props.as_object();
    let spacing = prop_f32(props, "spacing").unwrap_or(2.0);
    let width = prop_length(props, "width", Length::Fill);
    let height = prop_length(props, "height", Length::Fill);

    let state = match caches.pane_grid_states.get(&node.id) {
        Some(s) => s,
        None => return text("(pane_grid: no state)").into(),
    };

    // Pre-render children into a map keyed by julep ID. Also extract
    // title props from child nodes before the closure consumes the elements.
    let mut child_map: HashMap<String, Element<'a, Message>> = HashMap::new();
    let mut title_map: HashMap<String, String> = HashMap::new();
    for c in &node.children {
        child_map.insert(
            c.id.clone(),
            super::render(c, caches, images, theme, dispatcher),
        );
        if let Some(title) = prop_str(c.props.as_object(), "title") {
            title_map.insert(c.id.clone(), title);
        }
    }

    // We need to move child_map into the closure but PaneGrid::new
    // requires FnMut, so use a RefCell to allow mutation.
    let child_map = std::cell::RefCell::new(child_map);

    let node_id = node.id.clone();
    let node_id2 = node.id.clone();
    let node_id3 = node.id.clone();
    let node_id4 = node.id.clone();

    let mut pg = pane_grid::PaneGrid::new(state, |_pane, pane_id, _is_maximized| {
        let child_element: Element<'a, Message> = child_map
            .borrow_mut()
            .remove(pane_id)
            .unwrap_or_else(|| text(format!("(pane: {})", pane_id)).into());
        let content = pane_grid::Content::new(child_element);
        if let Some(title_text) = title_map.get(pane_id) {
            let title_bar = pane_grid::TitleBar::new(text(title_text.clone()).size(14.0));
            content.title_bar(title_bar)
        } else {
            content
        }
    })
    .width(width)
    .height(height)
    .spacing(spacing);

    let min_size = prop_f32(props, "min_size").unwrap_or(10.0);
    let leeway = prop_f32(props, "leeway").unwrap_or(min_size);

    pg = pg.on_click(move |pane| Message::PaneClicked(node_id3.clone(), pane));
    pg = pg.on_resize(leeway, move |evt| {
        Message::PaneResized(node_id.clone(), evt)
    });
    pg = pg.on_drag(move |evt| Message::PaneDragged(node_id2.clone(), evt));
    pg = pg.on_focus_cycle(move |pane| Message::PaneFocusCycle(node_id4.clone(), pane));

    // Divider styling
    let divider_color = prop_color(props, "divider_color");
    let divider_width = prop_f32(props, "divider_width");
    if divider_color.is_some() || divider_width.is_some() {
        pg = pg.style(move |theme: &iced::Theme| {
            let mut style = pane_grid::default(theme);
            if let Some(dc) = divider_color {
                style.hovered_split.color = dc;
                style.picked_split.color = dc;
            }
            if let Some(dw) = divider_width {
                style.hovered_split.width = dw;
                style.picked_split.width = dw;
            }
            style
        });
    }

    pg.into()
}
