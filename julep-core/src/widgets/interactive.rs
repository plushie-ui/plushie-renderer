//! Interactive wrapper widgets: button, mouse_area, sensor, tooltip,
//! themer, window, and overlay.

use std::time::Duration;

use iced::widget::{Space, button, container, mouse_area, sensor, text, tooltip};
use iced::{Element, Fill, Length, mouse, widget};

use super::caches::WidgetCaches;
use super::helpers::*;
use crate::extensions::RenderCtx;
use crate::message::Message;
use crate::protocol::TreeNode;

// ---------------------------------------------------------------------------
// Button
// ---------------------------------------------------------------------------

pub(crate) fn render_button<'a>(node: &'a TreeNode, ctx: RenderCtx<'a>) -> Element<'a, Message> {
    let props = node.props.as_object();
    let id = node.id.clone();

    // Button can have either a text label or child content
    let child: Element<'a, Message> = if !node.children.is_empty() {
        node.children
            .first()
            .map(|c| ctx.render_child(c))
            .unwrap_or_else(|| Space::new().into())
    } else {
        let label = prop_str(props, "label")
            .or_else(|| prop_str(props, "content"))
            .unwrap_or_default();
        text(label).into()
    };

    let padding = parse_padding_value(props);
    let width = prop_length(props, "width", Length::Shrink);
    let height = prop_length(props, "height", Length::Shrink);
    let clip = prop_bool_default(props, "clip", false);
    let disabled =
        prop_bool_default(props, "disabled", false) || !prop_bool_default(props, "enabled", true);

    let mut b = button(child)
        .padding(padding)
        .width(width)
        .height(height)
        .clip(clip);

    if !disabled {
        b = b.on_press(Message::Click(id));
    }

    // Style: string name or style map object
    if let Some(style_val) = props.and_then(|p| p.get("style")) {
        if let Some(style_name) = style_val.as_str() {
            b = match style_name {
                "primary" => b.style(button::primary),
                "secondary" => b.style(button::secondary),
                "success" => b.style(button::success),
                "warning" => b.style(button::warning),
                "danger" => b.style(button::danger),
                "text" => b.style(button::text),
                "background" => b.style(button::background),
                "subtle" => b.style(button::subtle),
                _ => {
                    log::warn!(
                        "unknown style {:?} for widget type {:?}, using default",
                        style_name,
                        "button"
                    );
                    b.style(button::primary)
                }
            };
        } else if let Some(obj) = style_val.as_object() {
            let ov = parse_style_overrides(obj);
            b = b.style(move |theme: &iced::Theme, status| {
                let mut style = match ov.preset_base.as_deref() {
                    Some("primary") => button::primary(theme, status),
                    Some("secondary") => button::secondary(theme, status),
                    Some("success") => button::success(theme, status),
                    Some("danger") => button::danger(theme, status),
                    Some("warning") => button::warning(theme, status),
                    Some("text") => button::text(theme, status),
                    Some("background") => button::background(theme, status),
                    Some("subtle") => button::subtle(theme, status),
                    _ => button::primary(theme, status),
                };
                apply_button_fields(&mut style, &ov.base);
                match status {
                    button::Status::Hovered => {
                        if let Some(ref f) = ov.hovered {
                            apply_button_fields(&mut style, f);
                        } else {
                            style.background = auto_derive_hover_bg(style.background);
                        }
                    }
                    button::Status::Pressed => {
                        if let Some(ref f) = ov.pressed {
                            apply_button_fields(&mut style, f);
                        }
                    }
                    button::Status::Disabled => {
                        if let Some(ref f) = ov.disabled {
                            apply_button_fields(&mut style, f);
                        } else {
                            style.background = auto_derive_disabled_bg(style.background);
                            style.text_color = auto_derive_disabled_text(style.text_color);
                            style.border = auto_derive_disabled_border(style.border);
                            style.shadow = auto_derive_disabled_shadow(style.shadow);
                        }
                    }
                    _ => {}
                }
                style
            });
        }
    }

    container(b).id(widget::Id::from(node.id.clone())).into()
}

// ---------------------------------------------------------------------------
// MouseArea
// ---------------------------------------------------------------------------

pub(crate) fn render_mouse_area<'a>(
    node: &'a TreeNode,
    ctx: RenderCtx<'a>,
) -> Element<'a, Message> {
    let props = node.props.as_object();
    let child: Element<'a, Message> = node
        .children
        .first()
        .map(|c| ctx.render_child(c))
        .unwrap_or_else(|| Space::new().into());

    let id = node.id.clone();
    let release_id = format!("{}:release", node.id);

    let mut ma = mouse_area(child)
        .on_press(Message::Click(id))
        .on_release(Message::Click(release_id));

    // Conditional event handlers (opt-in via boolean props)
    if prop_bool_default(props, "on_middle_press", false) {
        let ev_id = node.id.clone();
        ma = ma.on_middle_press(Message::MouseAreaEvent(ev_id, "middle_press".into()));
    }
    if prop_bool_default(props, "on_right_press", false) {
        let ev_id = node.id.clone();
        ma = ma.on_right_press(Message::MouseAreaEvent(ev_id, "right_press".into()));
    }
    if prop_bool_default(props, "on_right_release", false) {
        let ev_id = node.id.clone();
        ma = ma.on_right_release(Message::MouseAreaEvent(ev_id, "right_release".into()));
    }
    if prop_bool_default(props, "on_middle_release", false) {
        let ev_id = node.id.clone();
        ma = ma.on_middle_release(Message::MouseAreaEvent(ev_id, "middle_release".into()));
    }
    if prop_bool_default(props, "on_double_click", false) {
        let ev_id = node.id.clone();
        ma = ma.on_double_click(Message::MouseAreaEvent(ev_id, "double_click".into()));
    }
    if prop_bool_default(props, "on_enter", false) {
        let ev_id = node.id.clone();
        ma = ma.on_enter(Message::MouseAreaEvent(ev_id, "enter".into()));
    }
    if prop_bool_default(props, "on_exit", false) {
        let ev_id = node.id.clone();
        ma = ma.on_exit(Message::MouseAreaEvent(ev_id, "exit".into()));
    }
    if prop_bool_default(props, "on_move", false) {
        let ev_id = node.id.clone();
        ma = ma.on_move(move |p| Message::MouseAreaMove(ev_id.clone(), p.x, p.y));
    }
    if prop_bool_default(props, "on_scroll", false) {
        let ev_id = node.id.clone();
        ma = ma.on_scroll(move |delta| {
            let (dx, dy) = match delta {
                mouse::ScrollDelta::Lines { x, y } => (x, y),
                mouse::ScrollDelta::Pixels { x, y } => (x, y),
            };
            Message::MouseAreaScroll(ev_id.clone(), dx, dy)
        });
    }

    if let Some(cursor) = prop_str(props, "cursor")
        && let Some(interaction) = parse_interaction(&cursor)
    {
        ma = ma.interaction(interaction);
    }

    ma.into()
}

// ---------------------------------------------------------------------------
// Sensor
// ---------------------------------------------------------------------------

pub(crate) fn render_sensor<'a>(node: &'a TreeNode, ctx: RenderCtx<'a>) -> Element<'a, Message> {
    let child: Element<'a, Message> = node
        .children
        .first()
        .map(|c| ctx.render_child(c))
        .unwrap_or_else(|| Space::new().into());

    // Sensor needs a key. Use the node id.
    let id = node.id.clone();
    let show_id = node.id.clone();
    let resize_id = node.id.clone();
    let hide_id = format!("{}:hide", node.id);

    let props = node.props.as_object();

    let mut s = sensor(child)
        .key(id)
        .on_show(move |size| {
            Message::SensorResize(format!("{}:show", show_id), size.width, size.height)
        })
        .on_resize(move |size| Message::SensorResize(resize_id.clone(), size.width, size.height))
        .on_hide(Message::Click(hide_id));

    if let Some(d) = prop_f64(props, "delay") {
        s = s.delay(Duration::from_millis(d as u64));
    }
    if let Some(a) = prop_f32(props, "anticipate") {
        s = s.anticipate(a);
    }

    s.into()
}

// ---------------------------------------------------------------------------
// Tooltip
// ---------------------------------------------------------------------------

pub(crate) fn render_tooltip<'a>(node: &'a TreeNode, ctx: RenderCtx<'a>) -> Element<'a, Message> {
    let props = node.props.as_object();
    let tip = prop_str(props, "tip").unwrap_or_default();
    let gap = prop_f32(props, "gap");
    let position = prop_str(props, "position")
        .map(|s| match s.to_ascii_lowercase().as_str() {
            "bottom" => tooltip::Position::Bottom,
            "left" => tooltip::Position::Left,
            "right" => tooltip::Position::Right,
            "follow_cursor" | "follow" => tooltip::Position::FollowCursor,
            _ => tooltip::Position::Top,
        })
        .unwrap_or(tooltip::Position::Top);

    let child: Element<'a, Message> = node
        .children
        .first()
        .map(|c| ctx.render_child(c))
        .unwrap_or_else(|| Space::new().into());

    let mut tt = tooltip(child, text(tip), position);
    if let Some(g) = gap {
        tt = tt.gap(g);
    }

    // Tooltip padding is a single f32 value (not per-side)
    if let Some(p) = prop_f32(props, "padding") {
        tt = tt.padding(p);
    }

    let snap = prop_bool_default(props, "snap_within_viewport", true);
    tt = tt.snap_within_viewport(snap);

    if let Some(d) = prop_f64(props, "delay") {
        tt = tt.delay(Duration::from_millis(d as u64));
    }

    // Style: string name or style map (tooltip uses container::Style)
    if let Some(style_val) = props.and_then(|p| p.get("style")) {
        if let Some(style_name) = style_val.as_str() {
            tt = match style_name {
                "transparent" => tt.style(container::transparent),
                "rounded_box" => tt.style(container::rounded_box),
                "bordered_box" => tt.style(container::bordered_box),
                "dark" => tt.style(container::dark),
                "primary" => tt.style(container::primary),
                "secondary" => tt.style(container::secondary),
                "success" => tt.style(container::success),
                "danger" => tt.style(container::danger),
                "warning" => tt.style(container::warning),
                _ => {
                    log::warn!(
                        "unknown style {:?} for widget type {:?}, using default",
                        style_name,
                        "tooltip"
                    );
                    tt
                }
            };
        } else if let Some(obj) = style_val.as_object() {
            let ov = parse_style_overrides(obj);
            tt = tt.style(move |_theme| container_style_from_base(&ov.base));
        }
    }

    tt.into()
}

// ---------------------------------------------------------------------------
// Themer (applies a sub-theme to child content)
// ---------------------------------------------------------------------------

pub(crate) fn render_themer<'a>(node: &'a TreeNode, ctx: RenderCtx<'a>) -> Element<'a, Message> {
    // The resolved theme lives in ctx.caches.themer_themes (populated by
    // ensure_caches) so we can borrow it with lifetime 'a for child rendering.
    let cached_theme = ctx.caches.themer_themes.get(&node.id);
    let child_theme = cached_theme.unwrap_or(ctx.theme);

    // Build a child ctx with the resolved sub-theme so children render
    // against the overridden theme.
    let child_ctx = ctx.with_theme(child_theme);

    let child: Element<'a, Message> = node
        .children
        .first()
        .map(|c| child_ctx.render_child(c))
        .unwrap_or_else(|| Space::new().into());

    // Clone the cached theme into an owned Option for the Themer wrapper.
    let themer_theme = cached_theme.cloned();
    iced::widget::Themer::new(themer_theme, child).into()
}

// ---------------------------------------------------------------------------
// Window (top-level container)
// ---------------------------------------------------------------------------

pub(crate) fn render_window<'a>(node: &'a TreeNode, ctx: RenderCtx<'a>) -> Element<'a, Message> {
    let props = node.props.as_object();
    let padding = parse_padding_value(props);
    let width = prop_length(props, "width", Fill);
    let height = prop_length(props, "height", Fill);

    let child: Element<'a, Message> = node
        .children
        .first()
        .map(|c| ctx.render_child(c))
        .unwrap_or_else(|| Space::new().into());

    container(child)
        .padding(padding)
        .width(width)
        .height(height)
        .into()
}

// ---------------------------------------------------------------------------
// Overlay
// ---------------------------------------------------------------------------

pub(crate) fn render_overlay<'a>(node: &'a TreeNode, ctx: RenderCtx<'a>) -> Element<'a, Message> {
    use crate::overlay_widget;

    let props = node.props.as_object();
    let position = prop_str(props, "position").unwrap_or_else(|| "below".to_string());
    let gap = prop_f32(props, "gap").unwrap_or(0.0);
    let offset_x = prop_f32(props, "offset_x").unwrap_or(0.0);
    let offset_y = prop_f32(props, "offset_y").unwrap_or(0.0);

    let children = &node.children;
    if children.len() < 2 {
        return text(format!("overlay requires 2 children (id={})", node.id)).into();
    }

    let anchor = ctx.render_child(&children[0]);
    let content = ctx.render_child(&children[1]);

    let pos = match position.as_str() {
        "above" => overlay_widget::Position::Above,
        "left" => overlay_widget::Position::Left,
        "right" => overlay_widget::Position::Right,
        _ => overlay_widget::Position::Below,
    };

    overlay_widget::OverlayWrapper::new(anchor, content, pos, gap, offset_x, offset_y).into()
}

// ---------------------------------------------------------------------------
// Cache ensure function
// ---------------------------------------------------------------------------

pub(crate) fn ensure_themer_cache(node: &TreeNode, caches: &mut WidgetCaches) {
    let props = node.props.as_object();
    if let Some(resolved) = props
        .and_then(|p| p.get("theme"))
        .and_then(crate::theming::resolve_theme_only)
    {
        caches.themer_themes.insert(node.id.clone(), resolved);
    } else {
        // No valid theme prop -- remove stale cache entry if present.
        caches.themer_themes.remove(&node.id);
    }
}
