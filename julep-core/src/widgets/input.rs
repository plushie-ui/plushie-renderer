use super::*;

// ---------------------------------------------------------------------------
// Text Input
// ---------------------------------------------------------------------------

pub(crate) fn render_text_input<'a>(
    node: &'a TreeNode,
    caches: &'a WidgetCaches,
) -> Element<'a, Message> {
    let props = node.props.as_object();
    let value = prop_str(props, "value").unwrap_or_default();
    let placeholder = prop_str(props, "placeholder").unwrap_or_default();
    let width = prop_length(props, "width", Length::Fill);
    let size = prop_f32(props, "size").or(caches.default_text_size);
    let padding = parse_padding_value(props);
    let secure = prop_bool_default(props, "secure", false);
    let id = node.id.clone();
    let has_on_submit = props.and_then(|p| p.get("on_submit")).is_some();

    let mut ti = text_input(&placeholder, &value)
        .on_input(move |v| Message::Input(id.clone(), v))
        .width(width)
        .padding(padding)
        .secure(secure);

    if let Some(s) = size {
        ti = ti.size(s);
    }
    let font = props
        .and_then(|p| p.get("font"))
        .map(parse_font)
        .or(caches.default_font);
    if let Some(f) = font {
        ti = ti.font(f);
    }
    if let Some(lh) = parse_line_height(props) {
        ti = ti.line_height(lh);
    }
    if let Some(ax) = props
        .and_then(|p| p.get("align_x"))
        .and_then(|v| v.as_str())
        .and_then(value_to_horizontal_alignment)
    {
        ti = ti.align_x(ax);
    }

    if has_on_submit {
        let submit_id = node.id.clone();
        let submit_value = value.clone();
        ti = ti.on_submit(Message::Submit(submit_id, submit_value));
    }

    if prop_bool_default(props, "on_paste", false) {
        let paste_id = node.id.clone();
        ti = ti.on_paste(move |text| Message::Paste(paste_id.clone(), text));
    }

    if let Some(icon) = props
        .and_then(|p| p.get("icon"))
        .and_then(parse_text_input_icon)
    {
        ti = ti.icon(icon);
    }

    // Widget ID: default to node.id, allow prop override.
    let widget_id = prop_str(props, "id").unwrap_or_else(|| node.id.clone());
    ti = ti.id(widget_id);

    // Style: string name or style map object
    if let Some(style_val) = props.and_then(|p| p.get("style")) {
        if let Some(style_name) = style_val.as_str() {
            ti = match style_name {
                "default" => ti.style(text_input::default),
                _ => {
                    log::warn!(
                        "unknown style {:?} for widget type {:?}, using default",
                        style_name,
                        "text_input"
                    );
                    ti
                }
            };
        } else if let Some(obj) = style_val.as_object() {
            let ov = parse_style_overrides(obj);
            ti = ti.style(move |theme: &iced::Theme, status| {
                let mut style = text_input::default(theme, status);
                apply_text_input_fields(&mut style, &ov.base);
                match status {
                    text_input::Status::Focused { .. } => {
                        if let Some(ref f) = ov.focused {
                            apply_text_input_fields(&mut style, f);
                        }
                    }
                    text_input::Status::Hovered => {
                        if let Some(ref f) = ov.hovered {
                            apply_text_input_fields(&mut style, f);
                        } else {
                            style.background = deviate_background(style.background, 0.1);
                        }
                    }
                    text_input::Status::Disabled => {
                        if let Some(ref f) = ov.disabled {
                            apply_text_input_fields(&mut style, f);
                        } else {
                            style.background = match style.background {
                                iced::Background::Color(c) => {
                                    iced::Background::Color(alpha_color(c, 0.5))
                                }
                                iced::Background::Gradient(g) => {
                                    iced::Background::Gradient(alpha_gradient(g, 0.5))
                                }
                            };
                            style.value = alpha_color(style.value, 0.5);
                        }
                    }
                    _ => {}
                }
                style
            });
        }
    }

    ti.into()
}

// ---------------------------------------------------------------------------
// Text Editor key binding helpers
// ---------------------------------------------------------------------------

/// Parse a JSON motion string into an iced Motion.
fn parse_motion(s: &str) -> Option<text_editor::Motion> {
    use text_editor::Motion;
    match s {
        "left" => Some(Motion::Left),
        "right" => Some(Motion::Right),
        "up" => Some(Motion::Up),
        "down" => Some(Motion::Down),
        "word_left" => Some(Motion::WordLeft),
        "word_right" => Some(Motion::WordRight),
        "home" => Some(Motion::Home),
        "end" => Some(Motion::End),
        "page_up" => Some(Motion::PageUp),
        "page_down" => Some(Motion::PageDown),
        "document_start" => Some(Motion::DocumentStart),
        "document_end" => Some(Motion::DocumentEnd),
        _ => None,
    }
}

/// Parse a JSON binding value into an iced Binding.
fn parse_binding(val: &Value, id: &str) -> Option<text_editor::Binding<Message>> {
    use text_editor::Binding;
    match val {
        Value::String(s) => match s.as_str() {
            "copy" => Some(Binding::Copy),
            "cut" => Some(Binding::Cut),
            "paste" => Some(Binding::Paste),
            "select_all" => Some(Binding::SelectAll),
            "enter" => Some(Binding::Enter),
            "backspace" => Some(Binding::Backspace),
            "delete" => Some(Binding::Delete),
            "unfocus" => Some(Binding::Unfocus),
            "select_word" => Some(Binding::SelectWord),
            "select_line" => Some(Binding::SelectLine),
            // "default" is handled at the rule-matching level, not here
            _ => None,
        },
        Value::Object(obj) => {
            if let Some(m) = obj
                .get("move")
                .and_then(|v| v.as_str())
                .and_then(parse_motion)
            {
                return Some(Binding::Move(m));
            }
            if let Some(m) = obj
                .get("select")
                .and_then(|v| v.as_str())
                .and_then(parse_motion)
            {
                return Some(Binding::Select(m));
            }
            if let Some(c) = obj
                .get("insert")
                .and_then(|v| v.as_str())
                .and_then(|s| s.chars().next())
            {
                return Some(Binding::Insert(c));
            }
            if let Some(tag) = obj.get("custom").and_then(|v| v.as_str()) {
                let event_id = id.to_string();
                return Some(Binding::Custom(Message::Event(
                    event_id,
                    serde_json::json!(tag),
                    "key_binding".to_string(),
                )));
            }
            if let Some(seq) = obj.get("sequence").and_then(|v| v.as_array()) {
                let bindings: Vec<_> = seq.iter().filter_map(|v| parse_binding(v, id)).collect();
                if !bindings.is_empty() {
                    return Some(Binding::Sequence(bindings));
                }
            }
            None
        }
        _ => None,
    }
}

/// Check if a KeyPress matches the modifiers specified in a binding rule.
///
/// Requires an exact match: all required modifiers must be pressed, and
/// no extra modifiers may be active. This prevents a rule for `["shift"]`
/// from firing on `shift+ctrl+A`.
fn match_modifiers(mods: &keyboard::Modifiers, required: &[String]) -> bool {
    let want_shift = required.iter().any(|m| m == "shift");
    let want_ctrl = required.iter().any(|m| m == "ctrl");
    let want_alt = required.iter().any(|m| m == "alt");
    let want_logo = required.iter().any(|m| m == "logo");
    let want_command = required.iter().any(|m| m == "command");
    let want_jump = required.iter().any(|m| m == "jump");

    mods.shift() == want_shift
        && mods.control() == want_ctrl
        && mods.alt() == want_alt
        && mods.logo() == want_logo
        && mods.command() == want_command
        && mods.jump() == want_jump
}

/// Match a named key string against a KeyPress key.
fn match_named_key(named_key: &str, key: &keyboard::Key) -> bool {
    use keyboard::key::Named;
    let target = match named_key {
        "Enter" => Named::Enter,
        "Backspace" => Named::Backspace,
        "Delete" => Named::Delete,
        "Escape" => Named::Escape,
        "Tab" => Named::Tab,
        "Space" => Named::Space,
        "ArrowLeft" => Named::ArrowLeft,
        "ArrowRight" => Named::ArrowRight,
        "ArrowUp" => Named::ArrowUp,
        "ArrowDown" => Named::ArrowDown,
        "Home" => Named::Home,
        "End" => Named::End,
        "PageUp" => Named::PageUp,
        "PageDown" => Named::PageDown,
        "F1" => Named::F1,
        "F2" => Named::F2,
        "F3" => Named::F3,
        "F4" => Named::F4,
        "F5" => Named::F5,
        "F6" => Named::F6,
        "F7" => Named::F7,
        "F8" => Named::F8,
        "F9" => Named::F9,
        "F10" => Named::F10,
        "F11" => Named::F11,
        "F12" => Named::F12,
        _ => return false,
    };
    matches!(key, keyboard::Key::Named(n) if *n == target)
}

/// Pre-parsed key binding rule for closure capture.
struct KeyRule {
    key: Option<String>,
    named: Option<String>,
    modifiers: Vec<String>,
    binding_val: Value,
    is_default: bool,
}

// ---------------------------------------------------------------------------
// Text Editor
// ---------------------------------------------------------------------------

pub(crate) fn render_text_editor<'a>(
    node: &'a TreeNode,
    caches: &'a WidgetCaches,
) -> Element<'a, Message> {
    let props = node.props.as_object();
    let height = prop_length(props, "height", Length::Shrink);
    let placeholder = prop_str(props, "placeholder").unwrap_or_default();
    let id = node.id.clone();

    let content = match caches.editor_contents.get(&node.id) {
        Some(c) => c,
        None => {
            log::warn!("text_editor cache miss for id={}", node.id);
            return text("(text_editor: cache miss)").into();
        }
    };

    let editor_id = id;
    let mut te = text_editor(content)
        .on_action(move |action| Message::TextEditorAction(editor_id.clone(), action))
        .height(height);

    if !placeholder.is_empty() {
        te = te.placeholder(placeholder);
    }
    let font = props
        .and_then(|p| p.get("font"))
        .map(parse_font)
        .or(caches.default_font);
    if let Some(f) = font {
        te = te.font(f);
    }
    if let Some(sz) = prop_f32(props, "size").or(caches.default_text_size) {
        te = te.size(sz);
    }
    if let Some(lh) = parse_line_height(props) {
        te = te.line_height(lh);
    }
    if let Some(p) = prop_f32(props, "padding") {
        te = te.padding(p);
    }
    if let Some(minh) = prop_f32(props, "min_height") {
        te = te.min_height(minh);
    }
    if let Some(maxh) = prop_f32(props, "max_height") {
        te = te.max_height(maxh);
    }
    if let Some(w) = parse_wrapping(props) {
        te = te.wrapping(w);
    }
    // text_editor.width() takes impl Into<Pixels>, not Length
    if let Some(w) = prop_f32(props, "width") {
        te = te.width(w);
    }

    // Key bindings -- declarative rules parsed into a closure
    if let Some(rules) = props
        .and_then(|p| p.get("key_bindings"))
        .and_then(|v| v.as_array())
    {
        let editor_id = node.id.clone();
        let parsed_rules: Vec<KeyRule> = rules
            .iter()
            .filter_map(|rule| {
                let obj = rule.as_object()?;
                let key = obj.get("key").and_then(|v| v.as_str()).map(str::to_owned);
                let named = obj.get("named").and_then(|v| v.as_str()).map(str::to_owned);
                let modifiers = obj
                    .get("modifiers")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default();
                let binding_val = obj.get("binding").cloned().unwrap_or(Value::Null);
                let is_default = binding_val.as_str() == Some("default");
                Some(KeyRule {
                    key,
                    named,
                    modifiers,
                    binding_val,
                    is_default,
                })
            })
            .collect();

        if !parsed_rules.is_empty() {
            te = te.key_binding(move |key_press: text_editor::KeyPress| {
                for rule in &parsed_rules {
                    // Check modifiers first
                    if !match_modifiers(&key_press.modifiers, &rule.modifiers) {
                        continue;
                    }

                    // Check key match
                    if let Some(ref key_char) = rule.key {
                        // Match via to_latin for layout-independent character matching
                        let latin = key_press.key.to_latin(key_press.physical_key);
                        match latin {
                            Some(c) if c.to_string() == *key_char => {}
                            _ => continue,
                        }
                    } else if let Some(ref named_key) = rule.named
                        && !match_named_key(named_key, &key_press.key)
                    {
                        continue;
                    }
                    // else: no key/named constraint -- matches any key (catch-all rule)

                    // Default binding: delegate to iced's built-in handler
                    if rule.is_default {
                        return text_editor::Binding::from_key_press(key_press);
                    }

                    // Parse the specific binding
                    return parse_binding(&rule.binding_val, &editor_id);
                }
                // No rule matched -- no binding
                None
            });
        }
    }

    // Style closure, shared between plain and highlighted paths
    #[allow(clippy::type_complexity)]
    let style_fn: Option<Box<dyn Fn(&iced::Theme, text_editor::Status) -> text_editor::Style>> =
        if let Some(style_val) = props.and_then(|p| p.get("style")) {
            if let Some(style_name) = style_val.as_str() {
                match style_name {
                    "default" => Some(Box::new(text_editor::default)),
                    _ => {
                        log::warn!(
                            "unknown style {:?} for widget type {:?}, using default",
                            style_name,
                            "text_editor"
                        );
                        None
                    }
                }
            } else if let Some(obj) = style_val.as_object() {
                let ov = parse_style_overrides(obj);
                Some(Box::new(move |theme: &iced::Theme, status| {
                    let mut style = text_editor::default(theme, status);
                    apply_text_editor_fields(&mut style, &ov.base);
                    match status {
                        text_editor::Status::Focused { .. } => {
                            if let Some(ref f) = ov.focused {
                                apply_text_editor_fields(&mut style, f);
                            }
                        }
                        text_editor::Status::Hovered => {
                            if let Some(ref f) = ov.hovered {
                                apply_text_editor_fields(&mut style, f);
                            } else {
                                style.background = deviate_background(style.background, 0.1);
                            }
                        }
                        text_editor::Status::Disabled => {
                            if let Some(ref f) = ov.disabled {
                                apply_text_editor_fields(&mut style, f);
                            } else {
                                style.background = match style.background {
                                    iced::Background::Color(c) => {
                                        iced::Background::Color(alpha_color(c, 0.5))
                                    }
                                    iced::Background::Gradient(g) => {
                                        iced::Background::Gradient(alpha_gradient(g, 0.5))
                                    }
                                };
                                style.value = alpha_color(style.value, 0.5);
                            }
                        }
                        _ => {}
                    }
                    style
                }))
            } else {
                None
            }
        } else {
            None
        };

    let wid = widget::Id::from(node.id.clone());

    // Syntax highlighting changes the generic type parameter, so we must
    // branch here and produce Element from each path separately.
    if let Some(syntax) = prop_str(props, "highlight_syntax") {
        let theme = match prop_str(props, "highlight_theme").as_deref() {
            Some("base16_mocha") => iced::highlighter::Theme::Base16Mocha,
            Some("base16_ocean") => iced::highlighter::Theme::Base16Ocean,
            Some("base16_eighties") => iced::highlighter::Theme::Base16Eighties,
            Some("inspired_github") => iced::highlighter::Theme::InspiredGitHub,
            _ => iced::highlighter::Theme::SolarizedDark,
        };
        // Set ID before highlight() -- .id() is only available on PlainText variant
        te = te.id(wid);
        let mut hl = te.highlight(&syntax, theme);
        if let Some(sf) = style_fn {
            hl = hl.style(sf);
        }
        return hl.into();
    }

    {
        if let Some(sf) = style_fn {
            te = te.style(sf);
        }
        te = te.id(wid);
        te.into()
    }
}

// ---------------------------------------------------------------------------
// Checkbox
// ---------------------------------------------------------------------------

pub(crate) fn render_checkbox<'a>(
    node: &'a TreeNode,
    caches: &'a WidgetCaches,
) -> Element<'a, Message> {
    let props = node.props.as_object();
    let label = prop_str(props, "label").unwrap_or_default();
    let checked = prop_bool_default(props, "checked", false);
    let spacing = prop_f32(props, "spacing");
    let width = prop_length(props, "width", Length::Shrink);
    let id = node.id.clone();

    let disabled = prop_bool_default(props, "disabled", false);

    let mut cb = checkbox(checked).label(label).width(width);

    if !disabled {
        cb = cb.on_toggle(move |v| Message::Toggle(id.clone(), v));
    }

    if let Some(s) = spacing {
        cb = cb.spacing(s);
    }
    if let Some(sz) = prop_f32(props, "size") {
        cb = cb.size(sz);
    }
    if let Some(ts) = prop_f32(props, "text_size").or(caches.default_text_size) {
        cb = cb.text_size(ts);
    }
    let font = props
        .and_then(|p| p.get("font"))
        .map(parse_font)
        .or(caches.default_font);
    if let Some(f) = font {
        cb = cb.font(f);
    }
    if let Some(lh) = parse_line_height(props) {
        cb = cb.line_height(lh);
    }
    if let Some(shaping) = parse_shaping(props) {
        cb = cb.shaping(shaping);
    }
    if let Some(w) = parse_wrapping(props) {
        cb = cb.wrapping(w);
    }
    if let Some(icon_val) = props
        .and_then(|p| p.get("icon"))
        .and_then(|v| v.as_object())
        && let Some(cp_str) = icon_val.get("code_point").and_then(|v| v.as_str())
        && let Some(code_point) = cp_str.chars().next()
    {
        let icon_font = icon_val
            .get("font")
            .map(parse_font)
            .unwrap_or(Font::DEFAULT);
        let icon_size = icon_val
            .get("size")
            .and_then(|v| v.as_f64())
            .map(|v| Pixels(v as f32));
        let icon_line_height = icon_val
            .get("line_height")
            .and_then(|v| match v {
                Value::Number(n) => n.as_f64().map(|r| LineHeight::Relative(r as f32)),
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
            })
            .unwrap_or(LineHeight::default());
        let icon_shaping = icon_val
            .get("shaping")
            .and_then(|v| v.as_str())
            .and_then(|s| match s.to_ascii_lowercase().as_str() {
                "basic" => Some(iced::widget::text::Shaping::Basic),
                "advanced" => Some(iced::widget::text::Shaping::Advanced),
                "auto" => Some(iced::widget::text::Shaping::Auto),
                _ => None,
            })
            .unwrap_or(iced::widget::text::Shaping::Auto);
        let icon_struct = checkbox::Icon {
            font: icon_font,
            code_point,
            size: icon_size,
            line_height: icon_line_height,
            shaping: icon_shaping,
        };
        cb = cb.icon(icon_struct);
    }
    // Style: string name or style map object
    if let Some(style_val) = props.and_then(|p| p.get("style")) {
        if let Some(style_name) = style_val.as_str() {
            cb = match style_name {
                "primary" => cb.style(checkbox::primary),
                "secondary" => cb.style(checkbox::secondary),
                "success" => cb.style(checkbox::success),
                "danger" => cb.style(checkbox::danger),
                _ => {
                    log::warn!(
                        "unknown style {:?} for widget type {:?}, using default",
                        style_name,
                        "checkbox"
                    );
                    cb.style(checkbox::primary)
                }
            };
        } else if let Some(obj) = style_val.as_object() {
            let ov = parse_style_overrides(obj);
            cb = cb.style(move |theme: &iced::Theme, status| {
                let mut style = checkbox::primary(theme, status);
                apply_checkbox_fields(&mut style, &ov.base);
                match status {
                    checkbox::Status::Hovered { .. } => {
                        if let Some(ref f) = ov.hovered {
                            apply_checkbox_fields(&mut style, f);
                        } else {
                            style.background = deviate_background(style.background, 0.1);
                        }
                    }
                    checkbox::Status::Disabled { .. } => {
                        if let Some(ref f) = ov.disabled {
                            apply_checkbox_fields(&mut style, f);
                        } else {
                            style.background = match style.background {
                                iced::Background::Color(c) => {
                                    iced::Background::Color(alpha_color(c, 0.5))
                                }
                                iced::Background::Gradient(g) => {
                                    iced::Background::Gradient(alpha_gradient(g, 0.5))
                                }
                            };
                            if let Some(tc) = style.text_color {
                                style.text_color = Some(alpha_color(tc, 0.5));
                            }
                        }
                    }
                    _ => {}
                }
                style
            });
        }
    }

    container(cb).id(widget::Id::from(node.id.clone())).into()
}

// ---------------------------------------------------------------------------
// Toggler
// ---------------------------------------------------------------------------

pub(crate) fn render_toggler<'a>(
    node: &'a TreeNode,
    caches: &'a WidgetCaches,
) -> Element<'a, Message> {
    let props = node.props.as_object();
    let is_toggled = prop_bool_default(props, "is_toggled", false);
    let label = prop_str(props, "label");
    let spacing = prop_f32(props, "spacing");
    let width = prop_length(props, "width", Length::Shrink);
    let id = node.id.clone();

    let disabled = prop_bool_default(props, "disabled", false);

    let mut t = toggler(is_toggled).width(width);

    if !disabled {
        t = t.on_toggle(move |v| Message::Toggle(id.clone(), v));
    }

    if let Some(l) = label {
        t = t.label(l);
    }
    if let Some(s) = spacing {
        t = t.spacing(s);
    }
    if let Some(sz) = prop_f32(props, "size") {
        t = t.size(sz);
    }
    if let Some(ts) = prop_f32(props, "text_size").or(caches.default_text_size) {
        t = t.text_size(ts);
    }
    let font = props
        .and_then(|p| p.get("font"))
        .map(parse_font)
        .or(caches.default_font);
    if let Some(f) = font {
        t = t.font(f);
    }
    if let Some(lh) = parse_line_height(props) {
        t = t.line_height(lh);
    }
    if let Some(shaping) = parse_shaping(props) {
        t = t.shaping(shaping);
    }
    if let Some(w) = parse_wrapping(props) {
        t = t.wrapping(w);
    }
    if let Some(align) = props
        .and_then(|p| p.get("text_alignment"))
        .and_then(|v| v.as_str())
        .and_then(value_to_horizontal_alignment)
    {
        t = t.alignment(align);
    }

    // Style: string name or style map object
    if let Some(style_val) = props.and_then(|p| p.get("style")) {
        if let Some(style_name) = style_val.as_str() {
            t = match style_name {
                "default" => t.style(toggler::default),
                _ => {
                    log::warn!(
                        "unknown style {:?} for widget type {:?}, using default",
                        style_name,
                        "toggler"
                    );
                    t
                }
            };
        } else if let Some(obj) = style_val.as_object() {
            let ov = parse_style_overrides(obj);
            t = t.style(move |theme: &iced::Theme, status| {
                let mut style = toggler::default(theme, status);
                apply_toggler_fields(&mut style, &ov.base);
                match status {
                    toggler::Status::Hovered { .. } => {
                        if let Some(ref f) = ov.hovered {
                            apply_toggler_fields(&mut style, f);
                        } else {
                            style.background = deviate_background(style.background, 0.1);
                        }
                    }
                    toggler::Status::Disabled { .. } => {
                        if let Some(ref f) = ov.disabled {
                            apply_toggler_fields(&mut style, f);
                        } else {
                            style.background = match style.background {
                                iced::Background::Color(c) => {
                                    iced::Background::Color(alpha_color(c, 0.5))
                                }
                                iced::Background::Gradient(g) => {
                                    iced::Background::Gradient(alpha_gradient(g, 0.5))
                                }
                            };
                            if let Some(tc) = style.text_color {
                                style.text_color = Some(alpha_color(tc, 0.5));
                            }
                        }
                    }
                    _ => {}
                }
                style
            });
        }
    }

    container(t).id(widget::Id::from(node.id.clone())).into()
}

// ---------------------------------------------------------------------------
// Radio
// ---------------------------------------------------------------------------

/// Render a radio button widget.
///
/// Radio buttons use a `group` prop to form logical groups. When a radio
/// in a group is selected, the event uses the `group` value as the event ID
/// (not the individual radio's node ID). This allows the host to handle
/// all radios in a group with a single event handler.
///
/// Props: `label`, `value`, `selected` (current group value), `group` (event ID).
pub(crate) fn render_radio<'a>(
    node: &'a TreeNode,
    caches: &'a WidgetCaches,
) -> Element<'a, Message> {
    let props = node.props.as_object();
    let value = prop_str(props, "value").unwrap_or_default();
    let selected_str = prop_str(props, "selected").unwrap_or_default();
    let label = prop_str(props, "label").unwrap_or_else(|| value.clone());
    // Use "group" prop as the event ID so all radios in a group emit the same ID.
    let event_id = prop_str(props, "group").unwrap_or_else(|| node.id.clone());

    let is_selected = if value == selected_str {
        Some(0u8)
    } else {
        None
    };
    let select_value = value;

    let mut r = iced::widget::Radio::new(label, 0u8, is_selected, move |_| {
        Message::Select(event_id.clone(), select_value.clone())
    });

    if let Some(s) = prop_f32(props, "spacing") {
        r = r.spacing(s);
    }
    if let Some(w) = value_to_length_opt(props.and_then(|p| p.get("width"))) {
        r = r.width(w);
    }
    if let Some(sz) = prop_f32(props, "size") {
        r = r.size(sz);
    }
    if let Some(ts) = prop_f32(props, "text_size").or(caches.default_text_size) {
        r = r.text_size(ts);
    }
    let font = props
        .and_then(|p| p.get("font"))
        .map(parse_font)
        .or(caches.default_font);
    if let Some(f) = font {
        r = r.font(f);
    }
    if let Some(lh) = parse_line_height(props) {
        r = r.line_height(lh);
    }
    if let Some(shaping) = parse_shaping(props) {
        r = r.shaping(shaping);
    }
    if let Some(w) = parse_wrapping(props) {
        r = r.wrapping(w);
    }

    // Style: string name or style map object
    if let Some(style_val) = props.and_then(|p| p.get("style")) {
        if let Some(style_name) = style_val.as_str() {
            r = match style_name {
                "default" => r.style(iced::widget::radio::default),
                _ => {
                    log::warn!(
                        "unknown style {:?} for widget type {:?}, using default",
                        style_name,
                        "radio"
                    );
                    r
                }
            };
        } else if let Some(obj) = style_val.as_object() {
            let ov = parse_style_overrides(obj);
            r = r.style(move |theme: &iced::Theme, status| {
                let mut style = iced::widget::radio::default(theme, status);
                apply_radio_fields(&mut style, &ov.base);
                if matches!(status, iced::widget::radio::Status::Hovered { .. }) {
                    if let Some(ref f) = ov.hovered {
                        apply_radio_fields(&mut style, f);
                    } else {
                        style.background = deviate_background(style.background, 0.1);
                    }
                }
                style
            });
        }
    }

    container(r).id(widget::Id::from(node.id.clone())).into()
}

// ---------------------------------------------------------------------------
// Slider
// ---------------------------------------------------------------------------

pub(crate) fn render_slider<'a>(node: &'a TreeNode) -> Element<'a, Message> {
    let props = node.props.as_object();
    let range = prop_range_f64(props);
    let value = prop_f64(props, "value").unwrap_or(*range.start());
    let step = prop_f64(props, "step");
    let width = prop_length(props, "width", Length::Fill);
    let id = node.id.clone();
    let release_id = node.id.clone();

    let mut s = slider(range, value, move |v| Message::Slide(id.clone(), v))
        .on_release(Message::SlideRelease(release_id))
        .width(width);

    if let Some(st) = step {
        // Clamp step to a small positive minimum to prevent division by
        // zero or infinite loops in iced's slider internals.
        s = s.step(st.max(f64::EPSILON));
    }
    if let Some(d) = prop_f64(props, "default") {
        s = s.default(d);
    }
    if let Some(h) = prop_f32(props, "height") {
        s = s.height(h);
    }
    if let Some(ss) = prop_f64(props, "shift_step") {
        s = s.shift_step(ss);
    }

    // Style with optional circular handle
    let circular = prop_bool_default(props, "circular_handle", false);
    if circular {
        let radius = prop_f32(props, "handle_radius").unwrap_or(8.0);
        s = s.style(move |theme, status| {
            slider::default(theme, status).with_circular_handle(radius)
        });
    } else if let Some(style_val) = props.and_then(|p| p.get("style")) {
        if let Some(style_name) = style_val.as_str() {
            s = match style_name {
                "default" => s.style(slider::default),
                _ => {
                    log::warn!(
                        "unknown style {:?} for widget type {:?}, using default",
                        style_name,
                        "slider"
                    );
                    s
                }
            };
        } else if let Some(obj) = style_val.as_object() {
            let ov = parse_style_overrides(obj);
            s = s.style(move |theme: &iced::Theme, status| {
                let mut style = slider::default(theme, status);
                apply_slider_handle_fields(&mut style.handle, &ov.base);
                if matches!(status, slider::Status::Hovered) {
                    if let Some(ref f) = ov.hovered {
                        apply_slider_handle_fields(&mut style.handle, f);
                    } else {
                        style.handle.background = deviate_background(style.handle.background, 0.1);
                    }
                }
                style
            });
        }
    }

    container(s).id(widget::Id::from(node.id.clone())).into()
}

// ---------------------------------------------------------------------------
// Vertical Slider
// ---------------------------------------------------------------------------

pub(crate) fn render_vertical_slider<'a>(node: &'a TreeNode) -> Element<'a, Message> {
    let props = node.props.as_object();
    let range = prop_range_f64(props);
    let value = prop_f64(props, "value").unwrap_or(*range.start());
    let step = prop_f64(props, "step");
    let width = prop_f32(props, "width");
    let height = prop_length(props, "height", Length::Fill);
    let id = node.id.clone();
    let release_id = node.id.clone();

    let mut s = vertical_slider(range, value, move |v| Message::Slide(id.clone(), v))
        .on_release(Message::SlideRelease(release_id))
        .height(height);

    if let Some(w) = width {
        s = s.width(w);
    }

    if let Some(st) = step {
        s = s.step(st.max(f64::EPSILON));
    }
    if let Some(d) = prop_f64(props, "default") {
        s = s.default(d);
    }
    if let Some(ss) = prop_f64(props, "shift_step") {
        s = s.shift_step(ss);
    }

    // Style: string name or style map object
    if let Some(style_val) = props.and_then(|p| p.get("style")) {
        if let Some(style_name) = style_val.as_str() {
            s = match style_name {
                "default" => s.style(vertical_slider::default),
                _ => {
                    log::warn!(
                        "unknown style {:?} for widget type {:?}, using default",
                        style_name,
                        "vertical_slider"
                    );
                    s
                }
            };
        } else if let Some(obj) = style_val.as_object() {
            let ov = parse_style_overrides(obj);
            s = s.style(move |theme: &iced::Theme, status| {
                let mut style = vertical_slider::default(theme, status);
                apply_slider_handle_fields(&mut style.handle, &ov.base);
                if matches!(status, vertical_slider::Status::Hovered) {
                    if let Some(ref f) = ov.hovered {
                        apply_slider_handle_fields(&mut style.handle, f);
                    } else {
                        style.handle.background = deviate_background(style.handle.background, 0.1);
                    }
                }
                style
            });
        }
    }

    container(s).id(widget::Id::from(node.id.clone())).into()
}

// ---------------------------------------------------------------------------
// Pick List
// ---------------------------------------------------------------------------

pub(crate) fn render_pick_list<'a>(
    node: &'a TreeNode,
    caches: &'a WidgetCaches,
) -> Element<'a, Message> {
    let props = node.props.as_object();
    let options: Vec<String> = props
        .and_then(|p| p.get("options"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let selected = prop_str(props, "selected");
    let placeholder = prop_str(props, "placeholder");
    let width = prop_length(props, "width", Length::Shrink);
    let padding = parse_padding_value(props);
    let id = node.id.clone();

    let mut pl = pick_list(selected, options, |v: &String| v.clone())
        .on_select(move |v: String| Message::Select(id.clone(), v))
        .width(width)
        .padding(padding);

    if let Some(p) = placeholder {
        pl = pl.placeholder(p);
    }
    if let Some(ts) = prop_f32(props, "text_size").or(caches.default_text_size) {
        pl = pl.text_size(ts);
    }
    let font = props
        .and_then(|p| p.get("font"))
        .map(parse_font)
        .or(caches.default_font);
    if let Some(f) = font {
        pl = pl.font(f);
    }
    if let Some(mh) = prop_f32(props, "menu_height") {
        pl = pl.menu_height(mh);
    }
    if let Some(lh) = parse_line_height(props) {
        pl = pl.line_height(lh);
    }
    if let Some(shaping) = parse_shaping(props) {
        pl = pl.shaping(shaping);
    }

    if let Some(handle) = parse_pick_list_handle(props) {
        pl = pl.handle(handle);
    }

    // Style: string name or style map object
    if let Some(style_val) = props.and_then(|p| p.get("style")) {
        if let Some(style_name) = style_val.as_str() {
            pl = match style_name {
                "default" => pl.style(pick_list::default),
                _ => {
                    log::warn!(
                        "unknown style {:?} for widget type {:?}, using default",
                        style_name,
                        "pick_list"
                    );
                    pl
                }
            };
        } else if let Some(obj) = style_val.as_object() {
            let ov = parse_style_overrides(obj);
            pl = pl.style(move |theme: &iced::Theme, status| {
                let mut style = pick_list::default(theme, status);
                apply_pick_list_fields(&mut style, &ov.base);
                if matches!(status, pick_list::Status::Hovered) {
                    if let Some(ref f) = ov.hovered {
                        apply_pick_list_fields(&mut style, f);
                    } else {
                        style.background = deviate_background(style.background, 0.1);
                    }
                }
                style
            });
        }
    }

    if prop_bool_default(props, "on_open", false) {
        let open_id = node.id.clone();
        pl = pl.on_open(Message::Event(open_id, Value::Null, "open".into()));
    }
    if prop_bool_default(props, "on_close", false) {
        let close_id = node.id.clone();
        pl = pl.on_close(Message::Event(close_id, Value::Null, "close".into()));
    }

    container(pl).id(widget::Id::from(node.id.clone())).into()
}

// ---------------------------------------------------------------------------
// Combo Box
// ---------------------------------------------------------------------------

pub(crate) fn render_combo_box<'a>(
    node: &'a TreeNode,
    caches: &'a WidgetCaches,
) -> Element<'a, Message> {
    let state = match caches.combo_states.get(&node.id) {
        Some(s) => s,
        None => {
            log::warn!("combo_box cache miss for id={}", node.id);
            return text("(combo_box: cache miss)").into();
        }
    };

    let props = node.props.as_object();
    let selected: Option<String> = prop_str(props, "selected");
    let placeholder = prop_str(props, "placeholder").unwrap_or_default();
    let width = prop_length(props, "width", Length::Fill);
    let padding_val = parse_padding_value(props);
    let id = node.id.clone();
    let input_id = node.id.clone();

    let mut cb = combo_box(state, &placeholder, selected.as_ref(), move |selected| {
        Message::Select(id.clone(), selected)
    })
    .width(width)
    .padding(padding_val);

    // on_input: emit Input events so the host can filter
    cb = cb.on_input(move |v| Message::Input(input_id.clone(), v));

    if let Some(sz) = prop_f32(props, "size").or(caches.default_text_size) {
        cb = cb.size(sz);
    }
    let font = props
        .and_then(|p| p.get("font"))
        .map(parse_font)
        .or(caches.default_font);
    if let Some(f) = font {
        cb = cb.font(f);
    }
    if let Some(lh) = parse_line_height(props) {
        cb = cb.line_height(lh);
    }
    if let Some(mh) = prop_f32(props, "menu_height") {
        cb = cb.menu_height(mh);
    }
    if let Some(icon) = props
        .and_then(|p| p.get("icon"))
        .and_then(parse_text_input_icon)
    {
        cb = cb.icon(icon);
    }
    if prop_bool_default(props, "on_option_hovered", false) {
        let hover_id = node.id.clone();
        cb = cb.on_option_hovered(move |val| Message::OptionHovered(hover_id.clone(), val));
    }
    if prop_bool_default(props, "on_open", false) {
        let open_id = node.id.clone();
        cb = cb.on_open(Message::Event(open_id, Value::Null, "open".into()));
    }
    if prop_bool_default(props, "on_close", false) {
        let close_id = node.id.clone();
        cb = cb.on_close(Message::Event(close_id, Value::Null, "close".into()));
    }

    container(cb).id(widget::Id::from(node.id.clone())).into()
}
