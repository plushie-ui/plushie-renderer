//! Layout widgets -- containers that arrange child elements.
//!
//! - `column` -- vertical stack with spacing, padding, alignment
//! - `row` -- horizontal stack with spacing, padding, alignment
//! - `container` -- single-child wrapper with styling, alignment, clipping
//! - `stack` -- overlapping children (z-order by tree position)
//! - `grid` -- fixed-column grid with optional fluid layout
//! - `pin` -- absolute-positioned child at (x, y)
//! - `keyed_column` -- column with stable identity keys for efficient diffing
//! - `float` -- child offset by translation and scale
//! - `responsive` -- re-renders children when available size changes
//! - `scrollable` -- scrolling container (vertical, horizontal, or both)
//! - `pane_grid` -- resizable split panes with drag handles

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use iced::widget::scrollable::Anchor;
use iced::widget::{
    Space, Stack, column, container, grid, keyed, pane_grid, pin, row, scrollable, sensor, text,
};
use iced::{Element, Fill, Length, Point, Vector, widget};

use super::caches::WidgetCaches;
use super::helpers::*;
use crate::extensions::RenderCtx;
use crate::message::{Message, ScrollViewport};
use crate::protocol::TreeNode;

// ---------------------------------------------------------------------------
// Column
// ---------------------------------------------------------------------------

pub(crate) fn render_column<'a>(node: &'a TreeNode, ctx: RenderCtx<'a>) -> Element<'a, Message> {
    let props = node.props.as_object();
    let spacing = prop_f32(props, "spacing").unwrap_or(0.0);
    let padding = parse_padding_value(props);
    let width = prop_length(props, "width", Length::Shrink);
    let height = prop_length(props, "height", Length::Shrink);
    let align_x = prop_horizontal_alignment(props, "align_x");
    let clip = prop_bool_default(props, "clip", false);

    let children = ctx.render_children(node);

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

pub(crate) fn render_row<'a>(node: &'a TreeNode, ctx: RenderCtx<'a>) -> Element<'a, Message> {
    let props = node.props.as_object();
    let spacing = prop_f32(props, "spacing").unwrap_or(0.0);
    let padding = parse_padding_value(props);
    let width = prop_length(props, "width", Length::Shrink);
    let height = prop_length(props, "height", Length::Shrink);
    let align_y = prop_vertical_alignment(props, "align_y");
    let clip = prop_bool_default(props, "clip", false);

    let children = ctx.render_children(node);

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

pub(crate) fn render_container<'a>(node: &'a TreeNode, ctx: RenderCtx<'a>) -> Element<'a, Message> {
    let props = node.props.as_object();
    let padding = parse_padding_value(props);
    let width = prop_length(props, "width", Length::Shrink);
    let height = prop_length(props, "height", Length::Shrink);
    let center = prop_bool_default(props, "center", false);
    let clip = prop_bool_default(props, "clip", false);

    let child: Element<'a, Message> = node
        .children
        .first()
        .map(|c| ctx.render_child(c))
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

pub(crate) fn render_stack<'a>(node: &'a TreeNode, ctx: RenderCtx<'a>) -> Element<'a, Message> {
    let props = node.props.as_object();
    let width = prop_length(props, "width", Length::Shrink);
    let height = prop_length(props, "height", Length::Shrink);
    let clip = prop_bool_default(props, "clip", false);

    let children = ctx.render_children(node);

    Stack::with_children(children)
        .width(width)
        .height(height)
        .clip(clip)
        .into()
}

// ---------------------------------------------------------------------------
// Grid
// ---------------------------------------------------------------------------

pub(crate) fn render_grid<'a>(node: &'a TreeNode, ctx: RenderCtx<'a>) -> Element<'a, Message> {
    let props = node.props.as_object();
    let cols = props
        .and_then(|p| p.get("columns"))
        .and_then(|v| v.as_u64())
        .unwrap_or(1) as usize;
    let spacing = prop_f32(props, "spacing").unwrap_or(0.0);

    let column_width = prop_length(props, "column_width", Length::Shrink);
    let row_height = prop_length(props, "row_height", Length::Shrink);

    let children = ctx.render_children(node);

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

    // Fluid mode: auto-wrap columns with a max cell width
    if let Some(max_w) = prop_f32(props, "fluid") {
        g = g.fluid(max_w);
    }

    g.into()
}

// ---------------------------------------------------------------------------
// Pin (absolute positioning)
// ---------------------------------------------------------------------------

pub(crate) fn render_pin<'a>(node: &'a TreeNode, ctx: RenderCtx<'a>) -> Element<'a, Message> {
    let props = node.props.as_object();
    let x = prop_f32(props, "x").unwrap_or(0.0);
    let y = prop_f32(props, "y").unwrap_or(0.0);
    let width = prop_length(props, "width", Length::Shrink);
    let height = prop_length(props, "height", Length::Shrink);

    let child: Element<'a, Message> = node
        .children
        .first()
        .map(|c| ctx.render_child(c))
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
    ctx: RenderCtx<'a>,
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
            let elem = ctx.render_child(c);
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

pub(crate) fn render_float<'a>(node: &'a TreeNode, ctx: RenderCtx<'a>) -> Element<'a, Message> {
    let props = node.props.as_object();

    let child: Element<'a, Message> = node
        .children
        .first()
        .map(|c| ctx.render_child(c))
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
    ctx: RenderCtx<'a>,
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
        .map(|c| ctx.render_child(c))
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
    ctx: RenderCtx<'a>,
) -> Element<'a, Message> {
    let props = node.props.as_object();
    let width = prop_length(props, "width", Length::Shrink);
    let height = prop_length(props, "height", Length::Shrink);
    let spacing = prop_f32(props, "spacing");

    let child: Element<'a, Message> = node
        .children
        .first()
        .map(|c| ctx.render_child(c))
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
                ScrollViewport {
                    absolute_x: abs.x,
                    absolute_y: abs.y,
                    relative_x: rel.x,
                    relative_y: rel.y,
                    viewport_width: bounds.width,
                    viewport_height: bounds.height,
                    content_width: content_bounds.width,
                    content_height: content_bounds.height,
                },
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

pub(crate) fn render_pane_grid<'a>(node: &'a TreeNode, ctx: RenderCtx<'a>) -> Element<'a, Message> {
    let props = node.props.as_object();
    let spacing = prop_f32(props, "spacing").unwrap_or(2.0);
    let width = prop_length(props, "width", Length::Fill);
    let height = prop_length(props, "height", Length::Fill);

    let state = match ctx.caches.pane_grid_states.get(&node.id) {
        Some(s) => s,
        None => return text("(pane_grid: no state)").into(),
    };

    // Pre-render children into a map keyed by julep ID. Also extract
    // title props from child nodes before the closure consumes the elements.
    let mut child_map: HashMap<String, Element<'a, Message>> = HashMap::new();
    let mut title_map: HashMap<String, String> = HashMap::new();
    for c in &node.children {
        child_map.insert(c.id.clone(), ctx.render_child(c));
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

// ---------------------------------------------------------------------------
// Cache ensure functions
// ---------------------------------------------------------------------------

pub(crate) fn ensure_pane_grid_cache(node: &TreeNode, caches: &mut WidgetCaches) {
    let props = node.props.as_object();
    let axis = match prop_str(props, "split_axis").as_deref() {
        Some("horizontal") => pane_grid::Axis::Horizontal,
        _ => pane_grid::Axis::Vertical,
    };
    let child_ids: HashSet<String> = node.children.iter().map(|c| c.id.clone()).collect();

    if let Some(state) = caches.pane_grid_states.get_mut(&node.id) {
        // Prune panes whose child nodes no longer exist.
        let stale_panes: Vec<pane_grid::Pane> = state
            .panes
            .iter()
            .filter(|(_pane, id)| !child_ids.contains(*id))
            .map(|(pane, _id)| *pane)
            .collect();
        for pane in stale_panes {
            state.close(pane);
        }
        // Add panes for new children that don't have a pane yet.
        // Collect owned IDs to avoid holding an immutable borrow on
        // state.panes while we call state.split() (mutable).
        let existing_ids: HashSet<String> = state.panes.values().cloned().collect();
        let new_child_ids: Vec<String> = node
            .children
            .iter()
            .filter(|c| !existing_ids.contains(&c.id))
            .map(|c| c.id.clone())
            .collect();
        for new_id in new_child_ids {
            if let Some((&anchor, _)) = state.panes.iter().next() {
                let _ = state.split(axis, anchor, new_id);
            }
        }
    } else {
        let child_list: Vec<String> = node.children.iter().map(|c| c.id.clone()).collect();
        let new_state = if child_list.is_empty() {
            let (state, _) = pane_grid::State::new("default".to_string());
            state
        } else if child_list.len() == 1 {
            let (state, _) = pane_grid::State::new(child_list[0].clone());
            state
        } else {
            let (mut state, first_pane) = pane_grid::State::new(child_list[0].clone());
            let mut last_pane = first_pane;
            for id in child_list.iter().skip(1) {
                if let Some((new_pane, _)) = state.split(axis, last_pane, id.clone()) {
                    last_pane = new_pane;
                }
            }
            state
        };
        caches.pane_grid_states.insert(node.id.clone(), new_state);
    }
}
