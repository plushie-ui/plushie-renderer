use crate::protocol::TreeNode;
use serde_json::Value;

/// Prop type expectations for validation.
#[derive(Debug, Clone, Copy)]
enum PropType {
    Str,
    Number,
    Bool,
    Length,
    Color,
    Array,
    Any,
}

fn prop_type_matches(val: &Value, expected: PropType) -> bool {
    match expected {
        PropType::Str => val.is_string(),
        PropType::Number => val.is_number() || val.is_string(), // numeric strings accepted
        PropType::Bool => val.is_boolean(),
        PropType::Length => val.is_number() || val.is_string() || val.is_object(),
        PropType::Color => val.is_string(),
        PropType::Array => val.is_array(),
        PropType::Any => true,
    }
}

/// Validate props for known widget types. Only active in debug builds.
/// Logs warnings for unexpected prop names or mismatched types.
pub(crate) fn validate_props(node: &TreeNode) {
    use PropType::*;

    let expected: &[(&str, PropType)] = match node.type_name.as_str() {
        "button" => &[
            ("label", Str),
            ("style", Any),
            ("width", Length),
            ("height", Length),
            ("padding", Any),
            ("clip", Bool),
            ("disabled", Bool),
        ],
        "text" => &[
            ("content", Str),
            ("size", Number),
            ("color", Color),
            ("font", Any),
            ("width", Length),
            ("height", Length),
            ("align_x", Str),
            ("align_y", Str),
            ("line_height", Number),
            ("shaping", Str),
            ("wrapping", Str),
            ("style", Str),
        ],
        "column" => &[
            ("spacing", Number),
            ("padding", Any),
            ("width", Length),
            ("height", Length),
            ("max_width", Number),
            ("align_x", Str),
            ("clip", Bool),
            ("wrap", Bool),
        ],
        "row" => &[
            ("spacing", Number),
            ("padding", Any),
            ("width", Length),
            ("height", Length),
            ("max_width", Number),
            ("align_y", Str),
            ("clip", Bool),
            ("wrap", Bool),
        ],
        "container" => &[
            ("padding", Any),
            ("width", Length),
            ("height", Length),
            ("max_width", Number),
            ("max_height", Number),
            ("center", Bool),
            ("align_x", Str),
            ("align_y", Str),
            ("clip", Bool),
            ("style", Any),
            ("background", Any),
            ("color", Color),
            ("border", Any),
            ("shadow", Any),
        ],
        "text_input" => &[
            ("value", Str),
            ("placeholder", Str),
            ("font", Any),
            ("width", Length),
            ("padding", Any),
            ("size", Number),
            ("line_height", Number),
            ("secure", Bool),
            ("style", Any),
            ("icon", Any),
            ("disabled", Bool),
            ("id", Str),
            ("on_submit", Any),
            ("on_paste", Bool),
            ("align_x", Str),
        ],
        "slider" => &[
            ("value", Number),
            ("range", Array),
            ("step", Number),
            ("width", Length),
            ("height", Number),
            ("style", Any),
            ("shift_step", Number),
            ("default", Number),
        ],
        "checkbox" => &[
            ("label", Str),
            ("checked", Bool),
            ("size", Number),
            ("font", Any),
            ("text_size", Number),
            ("spacing", Number),
            ("width", Length),
            ("style", Any),
            ("icon", Any),
            ("disabled", Bool),
        ],
        "toggler" => &[
            ("label", Str),
            ("is_toggled", Bool),
            ("size", Number),
            ("font", Any),
            ("text_size", Number),
            ("spacing", Number),
            ("width", Length),
            ("style", Any),
            ("disabled", Bool),
        ],
        "progress_bar" => &[
            ("value", Number),
            ("range", Array),
            ("width", Length),
            ("height", Length),
            ("style", Any),
        ],
        "image" => &[
            ("source", Any),
            ("width", Length),
            ("height", Length),
            ("content_fit", Str),
            ("filter_method", Str),
            ("rotation", Any),
            ("opacity", Number),
            ("border_radius", Any),
        ],
        "svg" => &[
            ("source", Str),
            ("width", Length),
            ("height", Length),
            ("content_fit", Str),
            ("rotation", Any),
            ("opacity", Number),
            ("color", Color),
        ],
        "scrollable" => &[
            ("width", Length),
            ("height", Length),
            ("direction", Any),
            ("style", Any),
            ("anchor", Str),
            ("spacing", Number),
        ],
        "grid" => &[
            ("columns", Number),
            ("spacing", Number),
            ("width", Number),
            ("height", Number),
            ("column_width", Length),
            ("row_height", Length),
        ],
        "radio" => &[
            ("label", Str),
            ("value", Str),
            ("selected", Any),
            ("size", Number),
            ("font", Any),
            ("text_size", Number),
            ("spacing", Number),
            ("width", Length),
            ("style", Any),
            ("group", Str),
        ],
        "tooltip" => &[
            ("tip", Str),
            ("position", Str),
            ("gap", Number),
            ("padding", Number),
            ("snap_within_viewport", Bool),
            ("delay", Number),
            ("style", Any),
        ],
        "mouse_area" => &[
            ("on_middle_press", Bool),
            ("on_right_press", Bool),
            ("on_right_release", Bool),
            ("on_middle_release", Bool),
            ("on_double_click", Bool),
            ("on_enter", Bool),
            ("on_exit", Bool),
            ("on_move", Bool),
            ("on_scroll", Bool),
            ("cursor", Str),
        ],
        "sensor" => &[("delay", Number)],
        "space" => &[("width", Length), ("height", Length)],
        "rule" => &[
            ("direction", Str),
            ("width", Number),
            ("height", Number),
            ("thickness", Number),
            ("style", Any),
        ],
        "pick_list" => &[
            ("options", Array),
            ("selected", Str),
            ("placeholder", Str),
            ("width", Length),
            ("padding", Any),
            ("text_size", Number),
            ("font", Any),
            ("menu_height", Number),
            ("line_height", Number),
            ("shaping", Str),
            ("handle", Any),
            ("style", Any),
            ("on_open", Bool),
            ("on_close", Bool),
        ],
        "combo_box" => &[
            ("selected", Str),
            ("placeholder", Str),
            ("width", Length),
            ("padding", Any),
            ("size", Number),
            ("font", Any),
            ("line_height", Number),
            ("menu_height", Number),
            ("icon", Any),
            ("on_option_hovered", Bool),
            ("on_open", Bool),
            ("on_close", Bool),
        ],
        "text_editor" => &[
            ("placeholder", Str),
            ("height", Length),
            ("width", Number),
            ("size", Number),
            ("font", Any),
            ("line_height", Number),
            ("padding", Number),
            ("min_height", Number),
            ("max_height", Number),
            ("wrapping", Str),
            ("key_bindings", Array),
            ("style", Any),
            ("highlight_syntax", Str),
            ("highlight_theme", Str),
        ],
        "overlay" => &[
            ("position", Str),
            ("gap", Number),
            ("offset_x", Number),
            ("offset_y", Number),
        ],
        "themer" => &[("theme", Any)],
        "stack" => &[("width", Length), ("height", Length), ("clip", Bool)],
        "pin" => &[
            ("x", Number),
            ("y", Number),
            ("width", Length),
            ("height", Length),
        ],
        "keyed_column" => &[
            ("spacing", Number),
            ("padding", Any),
            ("width", Length),
            ("height", Length),
            ("max_width", Number),
        ],
        "float" => &[
            ("translate_x", Number),
            ("translate_y", Number),
            ("scale", Number),
        ],
        "responsive" => &[("width", Length), ("height", Length)],
        "rich_text" => &[
            ("spans", Array),
            ("size", Number),
            ("font", Any),
            ("color", Color),
            ("width", Length),
            ("height", Length),
            ("line_height", Number),
        ],
        "vertical_slider" => &[
            ("value", Number),
            ("range", Array),
            ("step", Number),
            ("width", Number),
            ("height", Length),
            ("style", Any),
            ("shift_step", Number),
            ("default", Number),
        ],
        "table" => &[
            ("columns", Array),
            ("rows", Array),
            ("width", Length),
            ("header", Bool),
            ("padding", Any),
            ("sort_by", Str),
            ("sort_order", Str),
        ],
        "pane_grid" => &[("spacing", Number), ("width", Length), ("height", Length)],
        "markdown" => &[
            ("content", Str),
            ("text_size", Number),
            ("h1_size", Number),
            ("h2_size", Number),
            ("h3_size", Number),
            ("code_size", Number),
            ("spacing", Number),
            ("width", Length),
        ],
        "window" => &[("padding", Any), ("width", Length), ("height", Length)],
        _ => return, // Unknown widget type -- skip validation
    };

    let props = match node.props.as_object() {
        Some(p) => p,
        None => return,
    };

    let expected_names: Vec<&str> = expected.iter().map(|(name, _)| *name).collect();

    for (key, val) in props {
        match expected.iter().find(|(name, _)| name == key) {
            Some((_, expected_type)) => {
                if !prop_type_matches(val, *expected_type) {
                    log::warn!(
                        "widget '{}' ({}): prop '{}' has unexpected type {:?} (expected {:?})",
                        node.id,
                        node.type_name,
                        key,
                        val,
                        expected_type
                    );
                }
            }
            None => {
                log::warn!(
                    "widget '{}' ({}): unexpected prop '{}' (known: {:?})",
                    node.id,
                    node.type_name,
                    key,
                    expected_names
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_node(type_name: &str, props: serde_json::Value) -> TreeNode {
        TreeNode {
            id: format!("test-{type_name}"),
            type_name: type_name.to_string(),
            props,
            children: vec![],
        }
    }

    /// Verify validate_props doesn't panic for every supported widget type,
    /// including with an empty props object and with representative props.
    #[test]
    fn validate_all_supported_types_no_panic() {
        let types_with_sample_props: Vec<(&str, serde_json::Value)> = vec![
            ("button", json!({"label": "ok", "disabled": false})),
            ("text", json!({"content": "hello", "size": 14})),
            ("column", json!({"spacing": 8})),
            ("row", json!({"spacing": 4, "wrap": true})),
            (
                "container",
                json!({"padding": 10, "width": "fill", "clip": false}),
            ),
            ("text_input", json!({"value": "", "placeholder": "type..."})),
            ("slider", json!({"value": 50, "range": [0, 100]})),
            ("checkbox", json!({"label": "agree", "checked": true})),
            (
                "toggler",
                json!({"label": "dark mode", "is_toggled": false}),
            ),
            ("progress_bar", json!({"value": 75, "range": [0, 100]})),
            ("image", json!({"source": "test.png"})),
            ("svg", json!({"source": "icon.svg"})),
            ("scrollable", json!({"direction": "vertical"})),
            ("grid", json!({"columns": 3, "spacing": 4})),
            (
                "radio",
                json!({"label": "opt", "value": "a", "group": "g1"}),
            ),
            ("tooltip", json!({"tip": "help", "position": "top"})),
            (
                "mouse_area",
                json!({"on_enter": true, "on_exit": true, "cursor": "pointer"}),
            ),
            ("sensor", json!({"delay": 100})),
            ("space", json!({"width": 10, "height": 10})),
            ("rule", json!({"direction": "horizontal", "thickness": 2})),
            ("pick_list", json!({"options": ["a", "b"], "selected": "a"})),
            (
                "combo_box",
                json!({"placeholder": "search...", "width": "fill"}),
            ),
            (
                "text_editor",
                json!({"placeholder": "code here", "height": 200}),
            ),
            ("overlay", json!({"position": "below", "gap": 4})),
            ("themer", json!({"theme": {"background": "#000"}})),
            ("stack", json!({"width": "fill", "clip": false})),
            ("pin", json!({"x": 10, "y": 20})),
            ("keyed_column", json!({"spacing": 8, "max_width": 400})),
            ("float", json!({"translate_x": 5, "translate_y": 10})),
            ("responsive", json!({"width": "fill", "height": "fill"})),
            ("rich_text", json!({"spans": [{"text": "hi"}], "size": 16})),
            (
                "vertical_slider",
                json!({"value": 50, "range": [0, 100], "height": "fill"}),
            ),
            (
                "table",
                json!({"columns": [{"key": "name", "label": "Name"}], "rows": []}),
            ),
            ("pane_grid", json!({"spacing": 2})),
            ("markdown", json!({"content": "# Hello", "text_size": 16})),
            ("window", json!({"padding": 8})),
        ];

        for (type_name, props) in &types_with_sample_props {
            let node = make_node(type_name, props.clone());
            validate_props(&node); // must not panic

            // Also test with empty props
            let empty_node = make_node(type_name, json!({}));
            validate_props(&empty_node);
        }
    }

    /// Unknown widget types are silently skipped (no panic).
    #[test]
    fn unknown_type_skipped() {
        let node = make_node("antimatter_widget", json!({"flux": 42}));
        validate_props(&node);
    }

    /// Null props are handled gracefully.
    #[test]
    fn null_props_no_panic() {
        let node = make_node("button", json!(null));
        validate_props(&node);
    }

    /// prop_type_matches covers all variants correctly.
    #[test]
    fn prop_type_matching() {
        use PropType::*;

        assert!(prop_type_matches(&json!("hello"), Str));
        assert!(!prop_type_matches(&json!(42), Str));

        assert!(prop_type_matches(&json!(42), Number));
        assert!(prop_type_matches(&json!("42"), Number)); // numeric strings OK
        assert!(!prop_type_matches(&json!(true), Number));

        assert!(prop_type_matches(&json!(true), Bool));
        assert!(!prop_type_matches(&json!("true"), Bool));

        assert!(prop_type_matches(&json!(100), Length));
        assert!(prop_type_matches(&json!("fill"), Length));
        assert!(prop_type_matches(&json!({"portion": 2}), Length));
        assert!(!prop_type_matches(&json!(true), Length));

        assert!(prop_type_matches(&json!("#ff0000"), Color));
        assert!(!prop_type_matches(&json!(42), Color));

        assert!(prop_type_matches(&json!([1, 2, 3]), Array));
        assert!(!prop_type_matches(&json!("nope"), Array));

        // Any matches everything
        assert!(prop_type_matches(&json!(null), Any));
        assert!(prop_type_matches(&json!(42), Any));
        assert!(prop_type_matches(&json!("x"), Any));
    }
}
