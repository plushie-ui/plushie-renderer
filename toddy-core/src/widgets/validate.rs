//! Debug-mode prop validation.
//!
//! When enabled, [`validate_props`] checks each node's props against a
//! schema of expected prop names and types per widget type. Unexpected
//! names or type mismatches are logged as warnings.
//!
//! Enabled unconditionally in debug builds. In release builds, the host
//! can opt in via `validate_props: true` in the Settings message.

use std::sync::OnceLock;

use crate::protocol::TreeNode;
use serde_json::Value;

/// Props accepted by all widget types. Checked before widget-specific
/// schemas so they don't appear as "unexpected" in validation warnings.
const UNIVERSAL_PROPS: &[&str] = &["a11y", "event_rate", "id"];

/// Global flag to enable prop validation in release builds.
/// Set via `set_validate_props(true)` during settings init.
/// In debug builds, validation always runs regardless of this flag.
static VALIDATE_PROPS: OnceLock<bool> = OnceLock::new();

/// Enable or disable prop validation at runtime. Called once during
/// settings initialization. Returns false if already set.
pub fn set_validate_props(enabled: bool) -> bool {
    VALIDATE_PROPS.set(enabled).is_ok()
}

/// Returns true if prop validation is enabled (debug build OR explicit opt-in).
pub fn is_validate_props_enabled() -> bool {
    cfg!(debug_assertions) || *VALIDATE_PROPS.get().unwrap_or(&false)
}

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

/// Collect prop validation warnings for a node without logging them.
///
/// Returns a list of human-readable warning strings. Useful for testing
/// and for callers that want to inspect warnings programmatically.
pub(crate) fn collect_prop_warnings(node: &TreeNode) -> Vec<String> {
    use PropType::*;

    let expected: &[(&str, PropType)] = match node.type_name.as_str() {
        "button" => &[
            ("label", Str),
            ("content", Str),
            ("style", Any),
            ("width", Length),
            ("height", Length),
            ("padding", Any),
            ("clip", Bool),
            ("disabled", Bool),
            ("enabled", Bool),
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
            ("ellipsis", Str),
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
            ("on_submit", Any),
            ("on_paste", Bool),
            ("align_x", Str),
            ("placeholder_color", Color),
            ("selection_color", Color),
            ("ime_purpose", Str),
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
            ("rail_color", Color),
            ("rail_width", Number),
            ("circular_handle", Bool),
            ("handle_radius", Number),
            ("label", Str),
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
            ("line_height", Number),
            ("wrapping", Str),
            ("shaping", Str),
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
            ("line_height", Number),
            ("wrapping", Str),
            ("shaping", Str),
        ],
        "progress_bar" => &[
            ("value", Number),
            ("range", Array),
            ("width", Length),
            ("height", Length),
            ("style", Any),
            ("vertical", Bool),
            ("label", Str),
        ],
        "image" => &[
            ("source", Any),
            ("width", Length),
            ("height", Length),
            ("content_fit", Str),
            ("filter_method", Str),
            ("rotation", Any),
            ("opacity", Number),
            ("border_radius", Number),
            ("expand", Bool),
            ("scale", Number),
            ("alt", Str),
            ("description", Str),
            ("decorative", Bool),
            ("crop", Any),
        ],
        "svg" => &[
            ("source", Str),
            ("width", Length),
            ("height", Length),
            ("content_fit", Str),
            ("rotation", Any),
            ("opacity", Number),
            ("color", Color),
            ("alt", Str),
            ("description", Str),
            ("decorative", Bool),
        ],
        "scrollable" => &[
            ("width", Length),
            ("height", Length),
            ("direction", Str),
            ("style", Any),
            ("anchor", Str),
            ("spacing", Number),
            ("scrollbar_width", Number),
            ("scrollbar_margin", Number),
            ("scroller_width", Number),
            ("scrollbar_color", Color),
            ("scroller_color", Color),
            ("auto_scroll", Bool),
            ("on_scroll", Bool),
        ],
        "grid" => &[
            ("columns", Number),
            ("spacing", Number),
            ("width", Number),
            ("height", Number),
            ("column_width", Length),
            ("row_height", Length),
            ("fluid", Number),
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
            ("line_height", Number),
            ("wrapping", Str),
            ("shaping", Str),
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
        "sensor" => &[("delay", Number), ("anticipate", Number)],
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
            ("ellipsis", Str),
            ("menu_style", Any),
            ("style", Any),
            ("on_open", Bool),
            ("on_close", Bool),
        ],
        "combo_box" => &[
            ("selected", Str),
            ("placeholder", Str),
            ("options", Array),
            ("width", Length),
            ("padding", Any),
            ("size", Number),
            ("font", Any),
            ("line_height", Number),
            ("shaping", Str),
            ("menu_height", Number),
            ("icon", Any),
            ("on_option_hovered", Bool),
            ("on_open", Bool),
            ("on_close", Bool),
            ("ellipsis", Str),
            ("menu_style", Any),
            ("style", Any),
        ],
        "text_editor" => &[
            ("content", Str),
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
            ("placeholder_color", Color),
            ("selection_color", Color),
            ("ime_purpose", Str),
        ],
        "overlay" => &[
            ("position", Str),
            ("gap", Number),
            ("offset_x", Number),
            ("offset_y", Number),
            ("flip", Bool),
            ("align", Str),
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
            ("wrapping", Str),
            ("ellipsis", Str),
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
            ("rail_color", Color),
            ("rail_width", Number),
            ("label", Str),
        ],
        "table" => &[
            ("columns", Array),
            ("rows", Array),
            ("width", Length),
            ("header", Bool),
            ("padding", Any),
            ("sort_by", Str),
            ("sort_order", Str),
            ("header_text_size", Number),
            ("row_text_size", Number),
            ("cell_spacing", Number),
            ("row_spacing", Number),
            ("separator_thickness", Number),
            ("separator_color", Color),
            ("separator", Bool),
        ],
        "pane_grid" => &[
            ("spacing", Number),
            ("width", Length),
            ("height", Length),
            ("min_size", Number),
            ("leeway", Number),
            ("divider_color", Color),
            ("divider_width", Number),
            ("split_axis", Str),
        ],
        "markdown" => &[
            ("content", Str),
            ("text_size", Number),
            ("h1_size", Number),
            ("h2_size", Number),
            ("h3_size", Number),
            ("code_size", Number),
            ("spacing", Number),
            ("width", Length),
            ("link_color", Color),
            ("code_theme", Str),
        ],
        "canvas" => &[
            ("layers", Any),
            ("shapes", Any),
            ("background", Color),
            ("width", Length),
            ("height", Length),
            ("interactive", Any),
            ("on_press", Bool),
            ("on_release", Bool),
            ("on_move", Bool),
            ("on_scroll", Bool),
            ("alt", Str),
            ("description", Str),
        ],
        "qr_code" => &[
            ("data", Str),
            ("cell_size", Number),
            ("error_correction", Str),
            ("cell_color", Color),
            ("background_color", Color),
            ("alt", Str),
            ("description", Str),
        ],
        "window" => &[
            ("padding", Any),
            ("width", Length),
            ("height", Length),
            ("scale_factor", Number),
        ],
        _ => return Vec::new(), // Unknown widget type -- skip validation
    };

    let props = match node.props.as_object() {
        Some(p) => p,
        None => return Vec::new(),
    };

    let expected_names: Vec<&str> = expected.iter().map(|(name, _)| *name).collect();
    let mut warnings = Vec::new();

    for (key, val) in props {
        // Skip props accepted by all widget types.
        if UNIVERSAL_PROPS.contains(&key.as_str()) {
            continue;
        }
        match expected.iter().find(|(name, _)| name == key) {
            Some((_, expected_type)) => {
                if !prop_type_matches(val, *expected_type) {
                    warnings.push(format!(
                        "widget '{}' ({}): prop '{}' has unexpected type {:?} (expected {:?})",
                        node.id, node.type_name, key, val, expected_type
                    ));
                }
            }
            None => {
                warnings.push(format!(
                    "widget '{}' ({}): unexpected prop '{}' (known: {:?})",
                    node.id, node.type_name, key, expected_names
                ));
            }
        }
    }

    warnings
}

/// Validate props for known widget types. Only active in debug builds.
/// Logs warnings for unexpected prop names or mismatched types.
pub(crate) fn validate_props(node: &TreeNode) {
    for warning in collect_prop_warnings(node) {
        log::warn!("{warning}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_node(type_name: &str, props: serde_json::Value) -> TreeNode {
        crate::testing::node_with_props(&format!("test-{type_name}"), type_name, props)
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
            (
                "canvas",
                json!({"width": "fill", "height": 200, "interactive": true}),
            ),
            ("qr_code", json!({"data": "hello", "cell_size": 4})),
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

    // -- collect_prop_warnings --

    #[test]
    fn warnings_for_unexpected_prop_name() {
        let node = make_node("button", json!({"label": "ok", "bogus_prop": 42}));
        let warnings = collect_prop_warnings(&node);
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("unexpected prop 'bogus_prop'"),
            "warning should name the bad prop, got: {}",
            warnings[0]
        );
    }

    #[test]
    fn warnings_for_type_mismatch() {
        // "label" expects Str, passing a number should trigger a type warning.
        let node = make_node("button", json!({"label": 42}));
        let warnings = collect_prop_warnings(&node);
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("unexpected type"),
            "warning should mention type mismatch, got: {}",
            warnings[0]
        );
    }

    #[test]
    fn no_warnings_for_valid_props() {
        let node = make_node("button", json!({"label": "ok", "disabled": false}));
        let warnings = collect_prop_warnings(&node);
        assert!(
            warnings.is_empty(),
            "expected no warnings, got: {warnings:?}"
        );
    }

    #[test]
    fn no_warnings_for_universal_props() {
        // "a11y", "id", and "event_rate" are universal props, should never trigger warnings.
        let node = make_node(
            "button",
            json!({"a11y": {"role": "button"}, "id": "btn1", "event_rate": 30}),
        );
        let warnings = collect_prop_warnings(&node);
        assert!(
            warnings.is_empty(),
            "universal props should not warn, got: {warnings:?}"
        );
    }

    #[test]
    fn no_warnings_for_unknown_widget_type() {
        let node = make_node("antimatter_widget", json!({"flux": 42}));
        let warnings = collect_prop_warnings(&node);
        assert!(
            warnings.is_empty(),
            "unknown types should produce no warnings"
        );
    }

    #[test]
    fn multiple_warnings_for_multiple_bad_props() {
        // "content" expects Str but gets bool -> type mismatch
        // "bogus" is not a known prop -> unexpected prop
        let node = make_node("text", json!({"content": true, "bogus": 1}));
        let warnings = collect_prop_warnings(&node);
        assert_eq!(warnings.len(), 2, "expected 2 warnings, got: {warnings:?}");
    }
}
