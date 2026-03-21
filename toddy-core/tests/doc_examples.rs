//! Compilation tests for code examples in the widget development docs.
//!
//! These tests verify that the API patterns shown in docs/extension-guide.md
//! and docs/core-widget-guide.md actually compile against the real toddy-core
//! and iced APIs. If the API changes, these tests fail, signaling that the
//! docs need updating.
//!
//! The tests don't render pixels -- they exercise the type system and verify
//! that method calls, field access, and trait implementations are correct.

use serde_json::json;
use toddy_core::prelude::*;
use toddy_core::testing::*;

// The column! and row! macros need explicit imports to avoid
// ambiguity between the prelude glob and iced's re-exports.
use toddy_core::iced::widget::{column, row};

// ============================================================================
// Extension guide: Gauge example (the opening example)
// ============================================================================

struct DocGauge;

impl WidgetExtension for DocGauge {
    fn type_names(&self) -> &[&str] {
        &["doc_gauge"]
    }

    fn config_key(&self) -> &str {
        "doc_gauge"
    }

    fn render<'a>(&self, node: &'a TreeNode, _env: &WidgetEnv<'a>) -> Element<'a, Message> {
        let value = node.prop_f32("value").unwrap_or(0.0);
        let label = node.prop_str("label").unwrap_or_default();

        column![
            text(format!("{label}: {value:.0}%")),
            progress_bar(0.0..=100.0, value),
        ]
        .spacing(4)
        .into()
    }
}

#[test]
fn extension_guide_gauge_renders() {
    let ext = DocGauge;
    let node = node_with_props("g1", "doc_gauge", json!({"value": 50.0, "label": "CPU"}));
    let test = TestEnv::default();
    let ctx = test.render_ctx();
    let env = test.env(&ctx);
    let _element = ext.render(&node, &env);
}

#[test]
fn extension_guide_gauge_no_props() {
    let ext = DocGauge;
    let node = node("g1", "doc_gauge");
    let test = TestEnv::default();
    let ctx = test.render_ctx();
    let env = test.env(&ctx);
    let _element = ext.render(&node, &env);
}

// ============================================================================
// Extension guide: prop parsing patterns
// ============================================================================

#[test]
fn extension_guide_prop_parsing() {
    let props_val = json!({
        "value": 42.5,
        "label": "test",
        "color": "#3498db",
        "show_label": true,
        "width": "fill",
    });
    let props = props_val.as_object();

    // Free function style
    let _value: Option<f32> = prop_f32(props, "value");
    let _label: Option<String> = prop_str(props, "label");
    let _color: Option<Color> = prop_color(props, "color");
    let _show_label: bool = prop_bool_default(props, "show_label", true);
    let _width: Length = prop_length(props, "width", Length::Fill);

    // TreeNode shorthand style
    let node = node_with_props("n1", "test", props_val.clone());
    let _value: Option<f32> = node.prop_f32("value");
    let _label: Option<String> = node.prop_str("label");
    let _color: Option<Color> = node.prop_color("color");
}

// ============================================================================
// Extension guide: rendering with theme
// ============================================================================

#[test]
fn extension_guide_theme_access() {
    let test = TestEnv::default();
    let ctx = test.render_ctx();
    let env = test.env(&ctx);

    let theme = env.theme();
    let palette = theme.palette();
    let _primary = palette.primary.base.color;
    let _is_dark = palette.is_dark;
}

// ============================================================================
// Extension guide: rendering children
// ============================================================================

struct DocContainer;

impl WidgetExtension for DocContainer {
    fn type_names(&self) -> &[&str] {
        &["doc_container"]
    }
    fn config_key(&self) -> &str {
        "doc_container"
    }

    fn render<'a>(&self, node: &'a TreeNode, env: &WidgetEnv<'a>) -> Element<'a, Message> {
        let header = text(node.prop_str("title").unwrap_or_default());
        let children: Vec<Element<'a, Message>> = env.ctx.render_children(node);
        let mut col = column![header].spacing(8);
        for child in children {
            col = col.push(child);
        }
        col.into()
    }
}

#[test]
fn extension_guide_container_renders() {
    let ext = DocContainer;
    let node = node_with_props_and_children(
        "c1",
        "doc_container",
        json!({"title": "Section"}),
        vec![node("child1", "text")],
    );
    let test = TestEnv::default();
    let ctx = test.render_ctx();
    let env = test.env(&ctx);
    let _element = ext.render(&node, &env);
}

// ============================================================================
// Extension guide: state management with ExtensionCaches
// ============================================================================

#[derive(Debug)]
struct SparklineState {
    data: Vec<f32>,
    min: f32,
    max: f32,
}

struct DocSparkline;

impl WidgetExtension for DocSparkline {
    fn type_names(&self) -> &[&str] {
        &["doc_sparkline"]
    }
    fn config_key(&self) -> &str {
        "doc_sparkline"
    }

    fn prepare(&mut self, node: &TreeNode, caches: &mut ExtensionCaches, _theme: &Theme) {
        let data: Vec<f32> = prop_f32_array(node.props(), "data").unwrap_or_default();
        let (min, max) = data
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
        caches.insert(
            self.config_key(),
            &node.id,
            SparklineState { data, min, max },
        );
    }

    fn render<'a>(&self, node: &'a TreeNode, env: &WidgetEnv<'a>) -> Element<'a, Message> {
        let _state: Option<&SparklineState> = env.caches.get(self.config_key(), &node.id);
        text("sparkline").into()
    }
}

#[test]
fn extension_guide_state_management() {
    let mut ext = DocSparkline;
    let node = node_with_props("s1", "doc_sparkline", json!({"data": [1.0, 2.0, 3.0]}));
    let mut caches = ExtensionCaches::new();
    ext.prepare(&node, &mut caches, &Theme::Dark);

    let state: Option<&SparklineState> = caches.get("doc_sparkline", "s1");
    let state = state.unwrap();
    assert_eq!(state.data, vec![1.0, 2.0, 3.0]);
    assert!((state.min - 1.0).abs() < f32::EPSILON);
    assert!((state.max - 3.0).abs() < f32::EPSILON);
}

// ============================================================================
// Extension guide: GenerationCounter pattern
// ============================================================================

struct ChartState {
    data: Vec<f32>,
    generation: GenerationCounter,
}

#[test]
fn extension_guide_generation_counter() {
    let mut caches = ExtensionCaches::new();
    let state = caches.get_or_insert("chart", "c1", || ChartState {
        data: vec![],
        generation: GenerationCounter::new(),
    });
    let gen_before = state.generation.get();
    state.data = vec![1.0, 2.0];
    state.generation.bump();
    assert_ne!(gen_before, state.generation.get());
}

// ============================================================================
// Extension guide: EventResult patterns
// ============================================================================

#[test]
fn extension_guide_event_result() {
    // Consumed: suppress original, emit replacement
    let result = EventResult::Consumed(vec![OutgoingEvent::extension_event(
        "item_selected",
        "w1",
        Some(json!({"index": 3})),
    )]);
    match result {
        EventResult::Consumed(events) => assert_eq!(events[0].family, "item_selected"),
        _ => panic!("expected Consumed"),
    }

    // Observed: forward original + emit additional
    let result = EventResult::Observed(vec![OutgoingEvent::extension_event(
        "scroll_stats",
        "w1",
        None,
    )]);
    assert!(matches!(result, EventResult::Observed(_)));

    // PassThrough: forward as-is
    let result = EventResult::PassThrough;
    assert!(matches!(result, EventResult::PassThrough));
}

// ============================================================================
// Extension guide: CoalesceHint
// ============================================================================

#[test]
fn extension_guide_coalesce_hint() {
    let event =
        OutgoingEvent::extension_event("cursor_pos", "w1", Some(json!({"x": 10.0, "y": 20.0})))
            .with_coalesce(CoalesceHint::Replace);
    assert!(event.coalesce.is_some());
}

// ============================================================================
// Extension guide: handle_command pattern
// ============================================================================

struct DocCommandWidget;

impl WidgetExtension for DocCommandWidget {
    fn type_names(&self) -> &[&str] {
        &["doc_cmd"]
    }
    fn config_key(&self) -> &str {
        "doc_cmd"
    }

    fn render<'a>(&self, _node: &'a TreeNode, _env: &WidgetEnv<'a>) -> Element<'a, Message> {
        text("cmd widget").into()
    }

    fn handle_command(
        &mut self,
        node_id: &str,
        op: &str,
        payload: &Value,
        _caches: &mut ExtensionCaches,
    ) -> Vec<OutgoingEvent> {
        match op {
            "export" => {
                let format = payload
                    .get("format")
                    .and_then(|v| v.as_str())
                    .unwrap_or("png");
                vec![OutgoingEvent::extension_event(
                    "exported",
                    node_id,
                    Some(json!({"format": format, "size": 1024})),
                )]
            }
            _ => vec![],
        }
    }
}

#[test]
fn extension_guide_handle_command() {
    let mut ext = DocCommandWidget;
    let mut caches = ExtensionCaches::new();
    let events = ext.handle_command("w1", "export", &json!({"format": "svg"}), &mut caches);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].family, "exported");
}

// ============================================================================
// Extension guide: cleanup pattern
// ============================================================================

struct DocCleanupWidget;

impl WidgetExtension for DocCleanupWidget {
    fn type_names(&self) -> &[&str] {
        &["doc_cleanup"]
    }
    fn config_key(&self) -> &str {
        "doc_cleanup"
    }

    fn render<'a>(&self, _node: &'a TreeNode, _env: &WidgetEnv<'a>) -> Element<'a, Message> {
        text("cleanup widget").into()
    }

    fn cleanup(&mut self, node_id: &str, caches: &mut ExtensionCaches) {
        caches.remove(self.config_key(), node_id);
    }
}

#[test]
fn extension_guide_cleanup_removes_cache() {
    let mut ext = DocCleanupWidget;
    let mut caches = ExtensionCaches::new();
    caches.insert("doc_cleanup", "w1", 42u32);
    assert!(caches.contains("doc_cleanup", "w1"));
    ext.cleanup("w1", &mut caches);
    assert!(!caches.contains("doc_cleanup", "w1"));
}

// ============================================================================
// Extension guide: Rating widget (complete example)
// ============================================================================

struct DocRating;

impl WidgetExtension for DocRating {
    fn type_names(&self) -> &[&str] {
        &["doc_rating"]
    }
    fn config_key(&self) -> &str {
        "doc_rating"
    }

    fn render<'a>(&self, node: &'a TreeNode, env: &WidgetEnv<'a>) -> Element<'a, Message> {
        let value = node.prop_f32("value").unwrap_or(0.0) as usize;
        let max = prop_u32(node.props(), "max").unwrap_or(5) as usize;
        let size = node.prop_f32("size").unwrap_or(24.0);
        let color = node
            .prop_color("color")
            .unwrap_or(env.theme().palette().primary.base.color);
        let disabled_color = Color {
            a: color.a * 0.3,
            ..color
        };

        let id = node.id.clone();
        let mut stars = row![].spacing(2);

        for i in 1..=max {
            let filled = i <= value;
            let star_color = if filled { color } else { disabled_color };
            let label = if filled { "\u{2605}" } else { "\u{2606}" };

            let star_text = text(label).size(size).color(star_color);

            let star_id = id.clone();
            let star_button = button(star_text)
                .on_press(Message::Event {
                    id: star_id,
                    family: "select".to_string(),
                    data: json!({"value": i}),
                })
                .padding(0)
                .style(button::text);

            stars = stars.push(star_button);
        }

        stars.into()
    }

    fn new_instance(&self) -> Box<dyn WidgetExtension> {
        Box::new(DocRating)
    }
}

#[test]
fn extension_guide_rating_renders() {
    let ext = DocRating;
    let node = node_with_props(
        "r1",
        "doc_rating",
        json!({"value": 3, "max": 5, "size": 32}),
    );
    let test = TestEnv::default();
    let ctx = test.render_ctx();
    let env = test.env(&ctx);
    let _element = ext.render(&node, &env);
}

#[test]
fn extension_guide_rating_no_props() {
    let ext = DocRating;
    let node = node("r1", "doc_rating");
    let test = TestEnv::default();
    let ctx = test.render_ctx();
    let env = test.env(&ctx);
    let _element = ext.render(&node, &env);
}

// ============================================================================
// Extension guide: new_instance
// ============================================================================

#[test]
fn extension_guide_new_instance() {
    let ext = DocRating;
    let _instance: Box<dyn WidgetExtension> = ext.new_instance();
}
