use iced::theme::palette;
use iced::{Color, Theme};
use serde_json::Value;

// ---------------------------------------------------------------------------
// Theme resolution
// ---------------------------------------------------------------------------

/// Result of resolving a theme from JSON.
pub struct ThemeResult {
    pub theme: Theme,
}

/// Resolve a JSON value into an iced Theme.
///
/// Accepts a string name (case-insensitive, underscored) or a JSON object
/// describing a custom palette. Unknown values fall back to Dark.
pub fn resolve_theme(value: &Value) -> ThemeResult {
    match value {
        Value::String(s) => ThemeResult {
            theme: resolve_builtin(s),
        },
        Value::Object(map) => {
            let theme = custom_theme_from_object(map);
            ThemeResult { theme }
        }
        _ => ThemeResult { theme: Theme::Dark },
    }
}

/// Resolve a theme value, returning `None` for `"system"` (follow OS preference).
pub fn resolve_theme_only(value: &Value) -> Option<Theme> {
    if let Some(s) = value.as_str()
        && s.eq_ignore_ascii_case("system")
    {
        return None;
    }
    Some(resolve_theme(value).theme)
}

// ---------------------------------------------------------------------------
// Built-in theme resolution
// ---------------------------------------------------------------------------

/// Map a string name to a built-in iced theme variant.
fn resolve_builtin(s: &str) -> Theme {
    match s.to_ascii_lowercase().as_str() {
        "light" => Theme::Light,
        "dark" => Theme::Dark,
        "dracula" => Theme::Dracula,
        "nord" => Theme::Nord,
        "solarized_light" => Theme::SolarizedLight,
        "solarized_dark" => Theme::SolarizedDark,
        "gruvbox_light" => Theme::GruvboxLight,
        "gruvbox_dark" => Theme::GruvboxDark,
        "catppuccin_latte" => Theme::CatppuccinLatte,
        "catppuccin_frappe" => Theme::CatppuccinFrappe,
        "catppuccin_macchiato" => Theme::CatppuccinMacchiato,
        "catppuccin_mocha" => Theme::CatppuccinMocha,
        "tokyo_night" => Theme::TokyoNight,
        "tokyo_night_storm" => Theme::TokyoNightStorm,
        "tokyo_night_light" => Theme::TokyoNightLight,
        "kanagawa_wave" => Theme::KanagawaWave,
        "kanagawa_dragon" => Theme::KanagawaDragon,
        "kanagawa_lotus" => Theme::KanagawaLotus,
        "moonfly" => Theme::Moonfly,
        "nightfly" => Theme::Nightfly,
        "oxocarbon" => Theme::Oxocarbon,
        "ferra" => Theme::Ferra,
        _ => Theme::Dark,
    }
}

// ---------------------------------------------------------------------------
// Custom theme from JSON object
// ---------------------------------------------------------------------------

/// Build a custom theme from a JSON object.
///
/// Supported fields (all optional):
/// - "name"       - display name for the theme (default: "Custom")
/// - "base"       - built-in theme name whose seed is used as the starting
///   point (default: dark)
/// - "background" - hex color string, e.g. "#1a1b26"
/// - "text"       - hex color string
/// - "primary"    - hex color string
/// - "success"    - hex color string
/// - "warning"    - hex color string
/// - "danger"     - hex color string
fn custom_theme_from_object(obj: &serde_json::Map<String, Value>) -> Theme {
    let base_theme = obj
        .get("base")
        .and_then(|v| v.as_str())
        .map(resolve_builtin)
        .unwrap_or(Theme::Dark);

    let mut seed = base_theme.seed();

    if let Some(color) = get_color(obj, "background") {
        seed.background = color;
    }
    if let Some(color) = get_color(obj, "text") {
        seed.text = color;
    }
    if let Some(color) = get_color(obj, "primary") {
        seed.primary = color;
    }
    if let Some(color) = get_color(obj, "success") {
        seed.success = color;
    }
    if let Some(color) = get_color(obj, "warning") {
        seed.warning = color;
    }
    if let Some(color) = get_color(obj, "danger") {
        seed.danger = color;
    }

    let name = obj
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("Custom")
        .to_owned();

    if has_shade_keys(obj) {
        let shade_obj = obj.clone();
        Theme::custom_with_fn(name, seed, move |s| {
            let mut pal = palette::Palette::generate(s);
            apply_shade_overrides(&mut pal, &shade_obj);
            pal
        })
    } else {
        Theme::custom(name, seed)
    }
}

// ---------------------------------------------------------------------------
// Palette shade overrides
// ---------------------------------------------------------------------------

/// Shade keys that can appear in a custom theme object.
const SHADE_KEYS: &[&str] = &[
    "primary_base",
    "primary_weak",
    "primary_strong",
    "secondary_base",
    "secondary_weak",
    "secondary_strong",
    "success_base",
    "success_weak",
    "success_strong",
    "warning_base",
    "warning_weak",
    "warning_strong",
    "danger_base",
    "danger_weak",
    "danger_strong",
    "background_base",
    "background_weakest",
    "background_weaker",
    "background_weak",
    "background_neutral",
    "background_strong",
    "background_stronger",
    "background_strongest",
];

/// Returns true if the object contains any shade or shade text override keys.
fn has_shade_keys(obj: &serde_json::Map<String, Value>) -> bool {
    SHADE_KEYS
        .iter()
        .any(|k| obj.contains_key(*k) || obj.contains_key(&format!("{}_text", k)))
}

/// Apply shade overrides from the JSON object onto the generated palette.
fn apply_shade_overrides(pal: &mut palette::Palette, obj: &serde_json::Map<String, Value>) {
    // Primary / secondary / success / warning / danger families
    override_pair(&mut pal.primary.base, obj, "primary_base");
    override_pair(&mut pal.primary.weak, obj, "primary_weak");
    override_pair(&mut pal.primary.strong, obj, "primary_strong");

    override_pair(&mut pal.secondary.base, obj, "secondary_base");
    override_pair(&mut pal.secondary.weak, obj, "secondary_weak");
    override_pair(&mut pal.secondary.strong, obj, "secondary_strong");

    override_pair(&mut pal.success.base, obj, "success_base");
    override_pair(&mut pal.success.weak, obj, "success_weak");
    override_pair(&mut pal.success.strong, obj, "success_strong");

    override_pair(&mut pal.warning.base, obj, "warning_base");
    override_pair(&mut pal.warning.weak, obj, "warning_weak");
    override_pair(&mut pal.warning.strong, obj, "warning_strong");

    override_pair(&mut pal.danger.base, obj, "danger_base");
    override_pair(&mut pal.danger.weak, obj, "danger_weak");
    override_pair(&mut pal.danger.strong, obj, "danger_strong");

    // Background family (8 levels)
    override_pair(&mut pal.background.base, obj, "background_base");
    override_pair(&mut pal.background.weakest, obj, "background_weakest");
    override_pair(&mut pal.background.weaker, obj, "background_weaker");
    override_pair(&mut pal.background.weak, obj, "background_weak");
    override_pair(&mut pal.background.neutral, obj, "background_neutral");
    override_pair(&mut pal.background.strong, obj, "background_strong");
    override_pair(&mut pal.background.stronger, obj, "background_stronger");
    override_pair(&mut pal.background.strongest, obj, "background_strongest");
}

/// Override a single Pair's color and/or text from the JSON object.
fn override_pair(pair: &mut palette::Pair, obj: &serde_json::Map<String, Value>, key: &str) {
    if let Some(hex) = obj.get(key).and_then(|v| v.as_str())
        && let Some(color) = parse_hex_color(hex)
    {
        pair.color = color;
    }
    let text_key = format!("{}_text", key);
    if let Some(hex) = obj.get(&text_key).and_then(|v| v.as_str())
        && let Some(color) = parse_hex_color(hex)
    {
        pair.text = color;
    }
}

// ---------------------------------------------------------------------------
// Color parsing
// ---------------------------------------------------------------------------

/// Extract a hex color value from a JSON object field.
fn get_color(obj: &serde_json::Map<String, Value>, key: &str) -> Option<Color> {
    obj.get(key)
        .and_then(|v| v.as_str())
        .and_then(parse_hex_color)
}

/// Parse a hex color string into an iced Color.
///
/// Accepts 3-char (#rgb), 4-char (#rgba), 6-char (#rrggbb), and 8-char
/// (#rrggbbaa) hex strings with or without leading `#`.
pub fn parse_hex_color(hex: &str) -> Option<Color> {
    let hex = hex.trim_start_matches('#');
    match hex.len() {
        3 => {
            let mut expanded = String::with_capacity(6);
            for c in hex.chars() {
                expanded.push(c);
                expanded.push(c);
            }
            let r = u8::from_str_radix(&expanded[0..2], 16).ok()?;
            let g = u8::from_str_radix(&expanded[2..4], 16).ok()?;
            let b = u8::from_str_radix(&expanded[4..6], 16).ok()?;
            Some(Color::from_rgb8(r, g, b))
        }
        4 => {
            let mut expanded = String::with_capacity(8);
            for c in hex.chars() {
                expanded.push(c);
                expanded.push(c);
            }
            let r = u8::from_str_radix(&expanded[0..2], 16).ok()?;
            let g = u8::from_str_radix(&expanded[2..4], 16).ok()?;
            let b = u8::from_str_radix(&expanded[4..6], 16).ok()?;
            let a = u8::from_str_radix(&expanded[6..8], 16).ok()?;
            Some(Color::from_rgba8(r, g, b, a as f32 / 255.0))
        }
        6 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            Some(Color::from_rgb8(r, g, b))
        }
        8 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            let a = u8::from_str_radix(&hex[6..8], 16).ok()?;
            Some(Color::from_rgba8(r, g, b, a as f32 / 255.0))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn resolve_builtin_themes() {
        assert!(matches!(resolve_theme(&json!("Dark")).theme, Theme::Dark));
        assert!(matches!(resolve_theme(&json!("nord")).theme, Theme::Nord));
        assert!(matches!(
            resolve_theme(&json!("CATPPUCCIN_MOCHA")).theme,
            Theme::CatppuccinMocha
        ));
    }

    #[test]
    fn system_theme_returns_none() {
        assert!(resolve_theme_only(&json!("system")).is_none());
        assert!(resolve_theme_only(&json!("System")).is_none());
    }

    #[test]
    fn non_system_returns_some() {
        assert!(resolve_theme_only(&json!("Dark")).is_some());
        assert!(resolve_theme_only(&json!({"primary": "#ff0000"})).is_some());
    }

    #[test]
    fn unknown_string_falls_back_to_dark() {
        assert!(matches!(
            resolve_theme(&json!("neon_pink")).theme,
            Theme::Dark
        ));
    }

    #[test]
    fn custom_theme_minimal() {
        let val = json!({"name": "Mine"});
        let result = resolve_theme(&val);
        assert_eq!(format!("{}", result.theme), "Mine");
    }

    #[test]
    fn custom_theme_with_colors() {
        let val = json!({
            "name": "Tokyo Remix",
            "background": "#1a1b26",
            "text": "#c0caf5",
            "primary": "#7aa2f7",
            "success": "#9ece6a",
            "danger": "#f7768e"
        });
        let result = resolve_theme(&val);
        let seed = result.theme.seed();
        assert_eq!(seed.background, Color::from_rgb8(0x1a, 0x1b, 0x26));
        assert_eq!(seed.text, Color::from_rgb8(0xc0, 0xca, 0xf5));
        assert_eq!(seed.primary, Color::from_rgb8(0x7a, 0xa2, 0xf7));
        assert_eq!(seed.success, Color::from_rgb8(0x9e, 0xce, 0x6a));
        assert_eq!(seed.danger, Color::from_rgb8(0xf7, 0x76, 0x8e));
    }

    #[test]
    fn custom_theme_with_warning_color() {
        let val = json!({"warning": "#f9e2af"});
        let result = resolve_theme(&val);
        let seed = result.theme.seed();
        assert_eq!(seed.warning, Color::from_rgb8(0xf9, 0xe2, 0xaf));
    }

    #[test]
    fn custom_theme_with_base() {
        let val = json!({"base": "Nord", "primary": "#88c0d0"});
        let result = resolve_theme(&val);
        let seed = result.theme.seed();
        // Primary should be overridden.
        assert_eq!(seed.primary, Color::from_rgb8(0x88, 0xc0, 0xd0));
        // Background should come from Nord's seed.
        let nord_bg = Theme::Nord.seed().background;
        assert_eq!(seed.background, nord_bg);
    }

    #[test]
    fn custom_theme_defaults_name_to_custom() {
        let val = json!({"primary": "#ff0000"});
        let result = resolve_theme(&val);
        assert_eq!(format!("{}", result.theme), "Custom");
    }

    #[test]
    fn parse_hex_color_valid() {
        let c = parse_hex_color("#ff8800").unwrap();
        assert_eq!(c, Color::from_rgb8(0xff, 0x88, 0x00));
    }

    #[test]
    fn parse_hex_color_without_hash() {
        let c = parse_hex_color("aabbcc").unwrap();
        assert_eq!(c, Color::from_rgb8(0xaa, 0xbb, 0xcc));
    }

    #[test]
    fn parse_hex_color_with_alpha() {
        let c = parse_hex_color("#ff880080").unwrap();
        assert_eq!(c, Color::from_rgba8(0xff, 0x88, 0x00, 128.0 / 255.0));
    }

    #[test]
    fn parse_hex_color_3char() {
        let c = parse_hex_color("#f80").unwrap();
        assert_eq!(c, Color::from_rgb8(0xff, 0x88, 0x00));
    }

    #[test]
    fn parse_hex_color_4char() {
        let c = parse_hex_color("#f808").unwrap();
        assert_eq!(c, Color::from_rgba8(0xff, 0x88, 0x00, 0x88 as f32 / 255.0));
    }

    #[test]
    fn parse_hex_color_invalid_length() {
        assert!(parse_hex_color("#ff").is_none());
        assert!(parse_hex_color("").is_none());
        assert!(parse_hex_color("#fffff").is_none());
    }

    #[test]
    fn parse_hex_color_invalid_chars() {
        assert!(parse_hex_color("#zzzzzz").is_none());
    }

    #[test]
    fn bad_color_field_is_ignored() {
        let val = json!({"background": "not-a-color", "text": "#ffffff"});
        let result = resolve_theme(&val);
        let seed = result.theme.seed();
        // text should be set, background should remain the dark default.
        assert_eq!(seed.text, Color::from_rgb8(0xff, 0xff, 0xff));
        assert_eq!(seed.background, palette::Seed::DARK.background);
    }

    #[test]
    fn custom_theme_with_shade_override() {
        let val = json!({
            "primary": "#5865f2",
            "primary_strong": "#1a5276"
        });
        let result = resolve_theme(&val);
        let pal = result.theme.palette();
        assert_eq!(pal.primary.strong.color, Color::from_rgb8(0x1a, 0x52, 0x76));
    }

    #[test]
    fn custom_theme_with_text_override() {
        let val = json!({
            "primary": "#5865f2",
            "primary_strong_text": "#ffffff"
        });
        let result = resolve_theme(&val);
        let pal = result.theme.palette();
        assert_eq!(pal.primary.strong.text, Color::from_rgb8(0xff, 0xff, 0xff));
    }

    #[test]
    fn custom_theme_without_shades_uses_standard() {
        // No shade keys -- should use Theme::custom (standard generation).
        let val = json!({"primary": "#ff0000"});
        let result = resolve_theme(&val);
        let pal = result.theme.palette();
        // The generated palette should match what Palette::generate
        // produces for the same seed.
        let expected = palette::Palette::generate(result.theme.seed());
        assert_eq!(pal.primary.strong.color, expected.primary.strong.color);
        assert_eq!(pal.primary.weak.color, expected.primary.weak.color);
    }

    #[test]
    fn custom_theme_background_shade_override() {
        let val = json!({
            "background": "#1a1a2e",
            "background_weakest": "#0d0d1a",
            "background_weakest_text": "#aaaaaa"
        });
        let result = resolve_theme(&val);
        let pal = result.theme.palette();
        assert_eq!(
            pal.background.weakest.color,
            Color::from_rgb8(0x0d, 0x0d, 0x1a)
        );
        assert_eq!(
            pal.background.weakest.text,
            Color::from_rgb8(0xaa, 0xaa, 0xaa)
        );
    }
}
