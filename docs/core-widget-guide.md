# Core Widget Guide

Build an iced widget once, use it everywhere: directly in Rust
applications, and across every toddy-powered SDK (Elixir, Gleam,
and any future host language).

## The two-crate pattern

A reusable widget is two crates:

```
my-gauge/               depends on iced (via toddy-iced)
  src/lib.rs            the Widget impl -- rendering, layout, events, a11y
  Cargo.toml

my-gauge-toddy/         depends on toddy-core + my-gauge
  src/lib.rs            WidgetExtension wrapper -- prop parsing, event bridging
  Cargo.toml
```

**The widget crate** (`my-gauge`) is a pure iced widget. It knows
nothing about toddy, JSON, protocols, or host SDKs. A Rust
developer imports it and uses it like any iced widget:

```rust
use my_gauge::gauge;

fn view(&self) -> Element<Message> {
    gauge(self.battery_level)
        .width(200)
        .color(Color::from_rgb(0.2, 0.8, 0.3))
        .into()
}
```

**The extension crate** (`my-gauge-toddy`) wraps the widget for
toddy's protocol. It parses JSON props, constructs the widget, and
bridges events. Every host SDK gets the widget through this single
wrapper -- no per-language duplication:

```rust
use toddy_core::prelude::*;
use my_gauge::gauge;

pub struct GaugeExtension;

impl WidgetExtension for GaugeExtension {
    fn type_names(&self) -> &[&str] { &["gauge"] }
    fn config_key(&self) -> &str { "gauge" }

    fn render<'a>(&self, node: &'a TreeNode, env: &WidgetEnv<'a>) -> Element<'a, Message> {
        let value = node.prop_f32("value").unwrap_or(0.0);
        let width = prop_length(node.props(), "width", Length::Fixed(100.0));
        let color = node.prop_color("color")
            .unwrap_or(env.theme().palette().primary.base.color);

        gauge(value).width(width).color(color).into()
    }
}
```

An Elixir developer uses it:

```elixir
gauge(id: "battery", value: 0.75, color: "#4CAF50")
```

A Gleam developer uses it the same way. A Rust developer uses the
widget crate directly without the wrapper. One widget, every
platform.

## Why two crates?

Separation of concerns. The widget crate has zero toddy knowledge
-- it depends only on iced. This means:

- **Testable in isolation.** Test the widget with iced's test
  harness. No protocol, no JSON, no toddy runtime needed.
- **Usable outside toddy.** Any iced application can use it. The
  widget isn't locked to toddy's ecosystem.
- **Clean API.** The widget has typed Rust parameters (`f32`,
  `Color`, `Length`), not `&Value` JSON blobs. The extension
  wrapper handles the JSON-to-typed conversion.

The extension wrapper is intentionally thin. It parses props,
constructs the widget, and maybe bridges events. The real logic
lives in the widget crate.

## Part 1: The iced widget crate

### Dependencies

```toml
[package]
name = "my-gauge"
version = "0.1.0"
edition = "2024"

[dependencies]
iced = { package = "toddy-iced", version = "0.6" }
```

**Note:** Use `toddy-iced` (the fork), not upstream `iced`. toddy
and all its SDKs use this fork. Using a different iced version
causes type mismatches at compile time.

If you're building a widget that should also work with upstream
iced, you can use Cargo features to switch between the two. But
for toddy ecosystem widgets, `toddy-iced` is the standard.

### The Widget trait

Every iced widget implements the `Widget` trait:

```rust
// Simplified signatures -- see iced::advanced::widget::Widget
// for the full trait with all type parameters.
pub trait Widget<Message, Theme, Renderer> {
    fn size(&self) -> Size<Length>;           // size hint
    fn layout(&mut self, tree, renderer, limits) -> layout::Node;
    fn draw(&self, tree, renderer, theme, style, layout, cursor, viewport);
    fn update(&mut self, tree, event, layout, cursor, renderer, shell, viewport);
    fn operate(&mut self, tree, layout, renderer, operation);
    fn mouse_interaction(&self, tree, layout, cursor, viewport, renderer) -> Interaction;
    // ... plus tag(), state(), overlay()
}
```

`size()`, `layout()`, and `draw()` are required. Everything else
has defaults.

**Call order per frame:** `layout()` -> `draw()` -> `update()`
(for each event) -> `operate()` (for a11y/focus queries).

### A complete gauge widget

```rust
use iced::advanced::layout::{self, Layout};
use iced::advanced::renderer;
use iced::advanced::widget::{self, Widget, tree};
use iced::{Color, Element, Length, Size, Rectangle, Theme, mouse};

/// A circular gauge that displays a value from 0.0 to 1.0.
pub struct Gauge {
    value: f32,
    color: Color,
    width: Length,
    height: Length,
}

impl Gauge {
    pub fn new(value: f32) -> Self {
        Self {
            value: value.clamp(0.0, 1.0),
            color: Color::from_rgb(0.2, 0.6, 1.0),
            width: Length::Fixed(100.0),
            height: Length::Fixed(100.0),
        }
    }

    pub fn color(mut self, color: Color) -> Self {
        self.color = color;
        self
    }

    pub fn width(mut self, width: impl Into<Length>) -> Self {
        self.width = width.into();
        self
    }

    pub fn height(mut self, height: impl Into<Length>) -> Self {
        self.height = height.into();
        self
    }
}

impl<Message, Renderer> Widget<Message, Theme, Renderer> for Gauge
where
    Renderer: iced::advanced::Renderer,
{
    fn size(&self) -> Size<Length> {
        Size { width: self.width, height: self.height }
    }

    fn layout(
        &mut self,
        _tree: &mut widget::Tree,
        _renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::atomic(limits, self.width, self.height)
    }

    fn draw(
        &self,
        _tree: &widget::Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        _style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let bg = theme.palette().background.weak.color;

        // Background track
        renderer.fill_quad(
            renderer::Quad {
                bounds,
                border: iced::Border {
                    radius: (bounds.height / 2.0).into(),
                    ..Default::default()
                },
                ..renderer::Quad::default()
            },
            iced::Background::Color(bg),
        );

        // Filled portion
        let filled_width = bounds.width * self.value;
        if filled_width > 0.0 {
            renderer.fill_quad(
                renderer::Quad {
                    bounds: Rectangle {
                        width: filled_width,
                        ..bounds
                    },
                    border: iced::Border {
                        radius: (bounds.height / 2.0).into(),
                        ..Default::default()
                    },
                    ..renderer::Quad::default()
                },
                iced::Background::Color(self.color),
            );
        }
    }

    fn operate(
        &mut self,
        _tree: &mut widget::Tree,
        layout: Layout<'_>,
        _renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        use iced::advanced::widget::operation::accessible::{Accessible, Role};

        operation.accessible(
            None,
            layout.bounds(),
            &Accessible {
                role: Role::Meter,
                label: Some("Gauge"),
                ..Accessible::default()
            },
        );
    }
}

/// Convenience constructor.
pub fn gauge(value: f32) -> Gauge {
    Gauge::new(value)
}

/// Into Element conversion.
impl<'a, Message: 'a, Renderer> From<Gauge>
    for Element<'a, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer + 'a,
{
    fn from(widget: Gauge) -> Self {
        Self::new(widget)
    }
}
```

This widget works in any iced application. No toddy dependency.

### Layout

`layout()` returns a `layout::Node` describing the widget's size.
For leaf widgets (no children), `layout::atomic(limits, width, height)`
handles the constraint resolution.

For widgets with children, compute child layouts and position them:

```rust
fn layout(&mut self, tree, renderer, limits) -> layout::Node {
    let child_limits = limits.width(Length::Fill);
    let child_layout = self.child
        .as_widget_mut()
        .layout(&mut tree.children[0], renderer, &child_limits);

    let child_size = child_layout.bounds().size();
    let padding = 10.0;
    let node_size = Size::new(
        child_size.width + padding * 2.0,
        child_size.height + padding * 2.0,
    );

    layout::Node::with_children(
        node_size,
        vec![child_layout.move_to(Point::new(padding, padding))],
    )
}
```

### Drawing

Use `renderer.fill_quad()` for rectangles -- it's batched (hundreds
of quads in one GPU draw call). For text, use `renderer.fill_text()`.
For complex paths or gradients, use `canvas::Frame`.

### Events

`update()` receives all iced events. Call `shell.capture_event()`
to stop propagation, `shell.publish(message)` to emit messages:

```rust
fn update(&mut self, _tree, event, layout, cursor, _renderer, shell, _viewport) {
    if let iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event {
        if cursor.is_over(layout.bounds()) {
            shell.publish(MyMessage::Clicked);
            shell.capture_event();
        }
    }
}
```

### Widget state

Widgets that need mutable state across frames declare it via
`tag()` and `state()`:

```rust
fn tag(&self) -> tree::Tag {
    tree::Tag::of::<MyState>()
}

fn state(&self) -> tree::State {
    tree::State::new(MyState::default())
}
```

Access in other methods: `tree.state.downcast_ref::<MyState>()`.

### Accessibility

`operate()` exposes the widget to screen readers and other AT:

```rust
fn operate(&mut self, _tree, layout, _renderer, operation) {
    operation.accessible(None, layout.bounds(), &Accessible {
        role: Role::Meter,
        label: Some("Battery level"),
        ..Accessible::default()
    });
}
```

For focusable widgets, also call `operation.focusable()` with a
state that implements the `Focusable` trait.

---

## Part 2: The toddy extension wrapper

The wrapper crate bridges your iced widget to toddy's protocol.
It's intentionally thin -- just prop parsing and event bridging.

### Dependencies

```toml
[package]
name = "my-gauge-toddy"
version = "0.1.0"
edition = "2024"

[dependencies]
toddy-core = "0.3"
my-gauge = { path = "../my-gauge" }
```

### The wrapper

```rust
use toddy_core::prelude::*;
use my_gauge::gauge;

pub struct GaugeExtension;

impl WidgetExtension for GaugeExtension {
    fn type_names(&self) -> &[&str] { &["gauge"] }
    fn config_key(&self) -> &str { "gauge" }

    fn render<'a>(&self, node: &'a TreeNode, env: &WidgetEnv<'a>) -> Element<'a, Message> {
        let value = node.prop_f32("value").unwrap_or(0.0);
        let color = node.prop_color("color")
            .unwrap_or(env.theme().palette().primary.base.color);
        let width = prop_length(node.props(), "width", Length::Fixed(100.0));
        let height = prop_length(node.props(), "height", Length::Fixed(100.0));

        gauge(value)
            .color(color)
            .width(width)
            .height(height)
            .into()
    }

    fn new_instance(&self) -> Box<dyn WidgetExtension> {
        Box::new(GaugeExtension)
    }
}
```

That's the entire wrapper. The widget logic, layout, drawing,
and accessibility are all in the widget crate. The wrapper just
translates JSON props to typed parameters.

### What the wrapper handles

| Concern | Where |
|---------|-------|
| Layout, drawing, events, a11y | Widget crate (`my-gauge`) |
| Prop parsing (JSON -> types) | Wrapper crate (`my-gauge-toddy`) |
| Event bridging (toddy Message -> host) | Wrapper crate |
| State management (ExtensionCaches) | Wrapper crate (if needed) |
| Compilation, binary generation | Host SDK (automatic) |

### Events from your widget

If your iced widget emits messages via `shell.publish()`, the
wrapper catches them in `handle_event()` and translates to
`OutgoingEvent`:

```rust
fn handle_event(
    &mut self,
    node_id: &str,
    family: &str,
    data: &Value,
    _caches: &mut ExtensionCaches,
) -> EventResult {
    match family {
        "click" => EventResult::Consumed(vec![
            OutgoingEvent::extension_event("gauge_clicked", node_id, None)
        ]),
        _ => EventResult::PassThrough,
    }
}
```

For high-frequency events (continuous value changes), set a
`CoalesceHint`:

```rust
OutgoingEvent::extension_event("value_changed", node_id, data)
    .with_coalesce(CoalesceHint::Replace)
```

### Testing

Test the widget crate and wrapper crate independently:

**Widget crate:** Standard iced widget testing. Construct the
widget, verify it doesn't panic with various inputs.

**Wrapper crate:** Use toddy's test helpers:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use toddy_core::testing::*;
    use serde_json::json;

    #[test]
    fn renders_with_props() {
        let ext = GaugeExtension;
        let node = node_with_props("g1", "gauge", json!({
            "value": 0.75,
            "color": "#4CAF50"
        }));

        let test = TestEnv::default();
        let ctx = test.render_ctx();
        let env = test.env(&ctx);

        let _element = ext.render(&node, &env);
    }

    #[test]
    fn renders_with_no_props() {
        let ext = GaugeExtension;
        let node = node("g1", "gauge");

        let test = TestEnv::default();
        let ctx = test.render_ctx();
        let env = test.env(&ctx);

        let _element = ext.render(&node, &env); // should use defaults
    }
}
```

### Publishing

Publish both crates. The widget crate is useful to Rust/iced
developers directly. The toddy wrapper crate is used by host SDKs:

```
crates.io:
  my-gauge         -- the iced widget (Rust developers use this)
  my-gauge-toddy   -- the toddy wrapper (SDKs reference this)
```

Host SDK authors add the toddy wrapper to their extension list.
The SDK's build system compiles it into the renderer binary
automatically.

---

## Adding a widget to toddy's standard set

If your widget is general-purpose enough to ship with every toddy
installation (like text_input, slider, or canvas), it can be added
to toddy-core instead of distributed as a separate crate.

This is a contribution to the toddy project, not the normal
distribution path:

| What | Where |
|------|-------|
| The iced widget (if new to iced) | `toddy-iced` fork |
| The render function | `toddy-core/src/widgets/` |
| The validate schema | `toddy-core/src/widgets/validate.rs` |
| Message variants (if new) | `toddy-core/src/message.rs` |
| OutgoingEvent constructors | `toddy-core/src/protocol/outgoing.rs` |
| Message wiring | `toddy/src/renderer/emitters.rs` |
| Dispatch table entry | `toddy-core/src/widgets/render.rs` |

The toddy-iced fork stays close to upstream iced. Only add to the
fork for: new accessible roles, Widget trait extensions, or bug
fixes not yet upstream. toddy-specific code (prop parsing, event
emission, validation) belongs in toddy-core.

## Further reading

- [Extension Guide](extension-guide.md) for building
  application-specific widgets (simpler, no iced Widget trait)
- [Widget Development](widget-development.md) for the decision
  framework
- iced widget examples in the
  [iced repository](https://github.com/iced-rs/iced)
- toddy-core rustdocs (`cargo doc --open` in the toddy workspace)
