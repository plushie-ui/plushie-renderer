# Extension Guide

Build native Rust widgets for toddy. Your host SDK handles
compilation and binary wiring. You write a Rust crate that
implements a trait, and the widget appears in the host's widget
tree like any built-in widget.

## What you need

A Rust crate with one dependency:

```toml
[package]
name = "my_gauge"
version = "0.1.0"
edition = "2024"

[dependencies]
toddy-core = "0.3"
```

Your host SDK generates the binary that links your crate. You
never touch `main.rs`, `Cargo` workspaces, or build scripts.
Consult your SDK's extension documentation for the host-side
setup.

**Important:** Never add a direct `iced` dependency. toddy uses
a fork (`toddy-iced`), and version mismatches will fail to compile.
Use `toddy_core::iced::*` for any iced types not in the prelude.

**Note:** The `column!` and `row!` macros may need explicit imports
to resolve ambiguity with the prelude glob:

```rust
use toddy_core::iced::widget::{column, row};
```

## The trait

Import everything from the prelude:

```rust
use toddy_core::prelude::*;
```

Implement `WidgetExtension` with three required methods:

```rust
pub struct Gauge;

impl WidgetExtension for Gauge {
    fn type_names(&self) -> &[&str] {
        &["gauge"]
    }

    fn config_key(&self) -> &str {
        "gauge"
    }

    fn render<'a>(&self, node: &'a TreeNode, env: &WidgetEnv<'a>) -> Element<'a, Message> {
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
```

That's a working extension. The host sends a node with
`"type": "gauge"` and the renderer calls your `render()` method.

### type_names vs config_key

`type_names()` lists the node type strings your extension handles.
An extension can handle multiple types (e.g., `&["bar_chart", "line_chart"]`).

`config_key()` is a unique identifier for your extension, used to:
- Namespace `ExtensionCaches` entries (prevents collisions between extensions)
- Key the `extension_config` section in Settings (host sends configuration)
- Identify the extension in the `hello` message's `extensions` array

`config_key` must not contain `:` (used as the cache key separator).

### What each method does

| Method | Required | When called | Phase |
|--------|----------|-------------|-------|
| `type_names()` | yes | startup | -- |
| `config_key()` | yes | startup | -- |
| `init(ctx)` | no | after Settings | mutable |
| `prepare(node, caches, theme)` | no | before each render cycle | mutable |
| `render(node, env)` | yes | during view() | immutable |
| `handle_event(id, family, data, caches)` | no | on widget events | mutable |
| `handle_command(id, op, payload, caches)` | no | on host commands | mutable |
| `cleanup(id, caches)` | no | when node leaves tree | mutable |
| `new_instance()` | no | for concurrent sessions | -- |

The mutable/immutable split matches iced's `update()`/`view()`
pattern. You can mutate state in `prepare()`, `handle_event()`,
and `handle_command()`. In `render()`, you can only read.

### Initialization

If your extension needs startup configuration (API keys, feature
flags, default settings), implement `init()`:

```rust
fn init(&mut self, ctx: &InitCtx<'_>) {
    // ctx.config is the JSON from Settings.extension_config["my_key"]
    if let Some(precision) = ctx.config.get("precision").and_then(|v| v.as_u64()) {
        self.precision = precision as usize;
    }
}
```

`init()` is called once after the host sends the Settings message.
The `InitCtx` provides the theme, default text size, and default
font in addition to the extension-specific config.

### WidgetEnv reference

`render()` receives a `WidgetEnv` with everything you need:

| Field / Method | Returns | Use for |
|---------------|---------|---------|
| `env.caches` | `&ExtensionCaches` | read cached state |
| `env.theme()` | `&Theme` | theme colors, palette |
| `env.images()` | `&ImageRegistry` | in-memory image handles |
| `env.default_text_size()` | `Option<f32>` | app-wide text size |
| `env.default_font()` | `Option<Font>` | app-wide font |
| `env.window_id()` | `&str` | which window is rendering |
| `env.scale_factor()` | `f32` | display scale (1.0 = 100%) |
| `env.ctx.render_child(node)` | `Element` | render a child node |
| `env.ctx.render_children(node)` | `Vec<Element>` | render all children |

## Designing your prop interface

Props arrive as JSON in `node.props`. The host SDK sends them as
key-value pairs. Your widget defines which props it accepts and
what types they are.

### Parsing props

Use the prop helpers from the prelude. They handle missing keys,
wrong types, and edge cases (NaN, overflow) gracefully:

```rust
fn render<'a>(&self, node: &'a TreeNode, env: &WidgetEnv<'a>) -> Element<'a, Message> {
    let props = node.props();

    // Required prop with fallback
    let value = prop_f32(props, "value").unwrap_or(0.0);

    // Optional prop
    let label = prop_str(props, "label");

    // Color (hex string: "#rrggbb" or "#rrggbbaa")
    let color = prop_color(props, "color").unwrap_or(Color::from_rgb(0.2, 0.6, 1.0));

    // Boolean with default
    let show_label = prop_bool_default(props, "show_label", true);

    // Length (accepts numbers, "fill", "shrink", {fill_portion: N})
    let width = prop_length(props, "width", Length::Fill);

    // ...
}
```

TreeNode also has shorthand methods that skip the `props()` call:

```rust
let value = node.prop_f32("value").unwrap_or(0.0);
let label = node.prop_str("label");
let color = node.prop_color("color");
```

### Prop helpers reference

| Helper | Return type | Accepts |
|--------|------------|---------|
| `prop_str` | `Option<String>` | strings |
| `prop_f32` | `Option<f32>` | numbers, numeric strings |
| `prop_f64` | `Option<f64>` | numbers, numeric strings |
| `prop_bool` | `Option<bool>` | booleans |
| `prop_bool_default` | `bool` | booleans (with default) |
| `prop_u32` / `prop_i32` | `Option<u32>` / `Option<i32>` | numbers |
| `prop_color` | `Option<Color>` | hex strings |
| `prop_length` | `Length` | numbers, "fill", "shrink", objects |
| `prop_padding` | `Option<Padding>` | numbers, arrays, objects |
| `prop_f32_array` | `Option<Vec<f32>>` | arrays of numbers |
| `prop_str_array` | `Option<Vec<String>>` | arrays of strings |
| `prop_object` | `Option<&Map>` | objects |
| `prop_value` | `Option<&Value>` | any JSON value |

### Design principles

**Make props optional with sensible defaults.** The host should be
able to create a minimal widget node with just `type` and `id`.
Every prop that can have a default should have one.

**Use the same prop names as built-in widgets.** If your widget has
a width, call it `width`. If it has text, call it `label` or
`content`. Consistency across the widget set makes the host SDK
predictable.

**Accept the same types as built-in widgets.** Colors are hex
strings. Sizes are numbers or Length values. Padding is a number
(uniform) or array (per-side). Don't invent new conventions.

**Fail gracefully on bad props.** Never panic on unexpected JSON.
Missing props return `None`. Wrong types return `None`. The prop
helpers handle this for you.

```rust
// Bad -- panics if "value" is missing or not a number
let value = node.props.get("value").unwrap().as_f64().unwrap() as f32;

// Good -- returns 0.0 if missing or wrong type
let value = node.prop_f32("value").unwrap_or(0.0);
```

**Return a placeholder on error, don't panic.** If your extension
can't render because required data is missing, return a visible
placeholder instead of panicking:

```rust
fn render<'a>(&self, node: &'a TreeNode, env: &WidgetEnv<'a>) -> Element<'a, Message> {
    let data = match prop_f32_array(node.props(), "data") {
        Some(d) if !d.is_empty() => d,
        _ => return text("No data").into(),
    };
    // ... render with data ...
}
```

## Rendering

`render()` returns an `Element<'a, Message>` -- iced's universal
widget type. Build it by composing widgets from the prelude:

```rust
fn render<'a>(&self, node: &'a TreeNode, env: &WidgetEnv<'a>) -> Element<'a, Message> {
    let value = node.prop_f32("value").unwrap_or(0.0);
    let max = node.prop_f32("max").unwrap_or(100.0);
    let color = node.prop_color("color").unwrap_or(Color::from_rgb(0.2, 0.6, 1.0));
    let width = prop_length(node.props(), "width", Length::Fill);
    let height = prop_length(node.props(), "height", Length::Fixed(24.0));

    container(
        progress_bar(0.0..=max, value)
            .style(move |theme| {
                let mut style = progress_bar::primary(theme);
                style.bar = iced::Background::Color(color);
                style
            })
    )
    .width(width)
    .height(height)
    .into()
}
```

### Using the theme

Access the current theme through `env`:

```rust
let theme = env.theme();
let palette = theme.palette();
let primary = palette.primary.base.color;
let is_dark = palette.is_dark;
```

Use theme colors for visual consistency with the rest of the UI.
Hard-coded colors look out of place when the user switches themes.

### Rendering children

Extensions can render child nodes from the host's tree:

```rust
fn render<'a>(&self, node: &'a TreeNode, env: &WidgetEnv<'a>) -> Element<'a, Message> {
    let header = text(node.prop_str("title").unwrap_or_default());

    // Render all children through the main widget dispatch.
    // Children can be any widget type: built-in, canvas, or
    // other extensions.
    let children: Vec<Element<'a, Message>> = env.ctx.render_children(node);

    let mut col = column![header].spacing(8);
    for child in children {
        col = col.push(child);
    }
    col.into()
}
```

`render_child` and `render_children` go through toddy's full
dispatch. A child node with `"type": "button"` renders as an iced
button. A child with `"type": "my_other_extension"` renders
through that extension. Your extension doesn't need to know what
its children are.

### The immutability constraint

`render()` takes `&self` (immutable). You cannot modify the
extension struct or write to `ExtensionCaches` during render.

This is deliberate -- it matches iced's architecture where
`view()` is pure. All state changes happen in the mutable phase
(`prepare()`, `handle_event()`, `handle_command()`), and
`render()` reads the results.

If you need mutable state during rendering, you're looking for the
`prepare()` / `ExtensionCaches` pattern. See [State
management](#state-management).

## State management

### Tier A: stateless

Many extensions need no state. They render directly from props:

```rust
fn render<'a>(&self, node: &'a TreeNode, env: &WidgetEnv<'a>) -> Element<'a, Message> {
    let value = node.prop_f32("value").unwrap_or(0.0);
    text(format!("{value:.1}")).into()
}
```

Props change -> host sends a tree update -> render() is called
again with new props. No caching, no lifecycle methods.

### Tier B: stateful with ExtensionCaches

When you need state that persists across renders (parsed data,
computed layouts, iced widget state), use `ExtensionCaches`.

Write in `prepare()`, read in `render()`:

```rust
fn prepare(&mut self, node: &TreeNode, caches: &mut ExtensionCaches, _theme: &Theme) {
    let data: Vec<f32> = prop_f32_array(node.props(), "data").unwrap_or_default();

    // Compute derived state once, not on every render.
    let (min, max) = data.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &v| {
        (lo.min(v), hi.max(v))
    });

    caches.insert(self.config_key(), &node.id, SparklineState { data, min, max });
}

fn render<'a>(&self, node: &'a TreeNode, env: &WidgetEnv<'a>) -> Element<'a, Message> {
    let state: Option<&SparklineState> = env.caches.get(self.config_key(), &node.id);
    // ... render using precomputed state ...
}
```

`ExtensionCaches` is type-erased (`HashMap<String, Box<dyn Any + Send + Sync>>`).
The namespace is your `config_key()`, the key is typically the
node ID.

**Gotcha: type mismatches.** If you `insert::<TypeA>` and then
`get::<TypeB>`, you get `None` and a logged warning. This usually
means your state struct changed shape between versions. Use
`get_or_insert` for defensive initialization:

```rust
let state = caches.get_or_insert(self.config_key(), &node.id, || {
    MyState::default()
});
```

### Canvas cache invalidation

If your extension uses `iced::widget::canvas` with a `Cache`, the
cache needs explicit invalidation when data changes.
`canvas::Cache` is `!Send + !Sync`, so it can't be stored in
`ExtensionCaches`. Use `GenerationCounter` instead:

```rust
fn prepare(&mut self, node: &TreeNode, caches: &mut ExtensionCaches, _theme: &Theme) {
    let new_data = prop_f32_array(node.props(), "data").unwrap_or_default();
    let state = caches.get_or_insert(self.config_key(), &node.id, || {
        ChartState { data: vec![], generation: GenerationCounter::new() }
    });

    if state.data != new_data {
        state.data = new_data;
        state.generation.bump(); // signals the canvas to redraw
    }
}
```

In your `canvas::Program::update()`, compare the generation
counter and clear the cache when it changes.

## Events

There are two ways to emit events from your extension:

1. **From `render()`:** Publish `Message::Event` via iced's
   `on_press`, `on_submit`, etc. on the widgets you compose. This
   is how the Rating example works -- each star button publishes a
   Message when clicked.

2. **From `handle_event()`:** Intercept events that iced already
   generated, and transform, suppress, or augment them with
   `OutgoingEvent`. This is for when you need to process or
   reshape events before the host sees them.

Most extensions use approach 1. Use approach 2 when you need to
aggregate events, add computed data, or suppress events the host
doesn't need.

### Tier B: handling events

When the host interacts with your widget, toddy routes events
through your extension before sending them to the host. You choose
what to do:

```rust
fn handle_event(
    &mut self,
    node_id: &str,
    family: &str,
    data: &Value,
    caches: &mut ExtensionCaches,
) -> EventResult {
    match family {
        // Transform the event before the host sees it.
        "click" => {
            let state: Option<&MyState> = caches.get(self.config_key(), node_id);
            let selected_item = state.and_then(|s| s.item_at_click(data));

            EventResult::Consumed(vec![
                OutgoingEvent::extension_event("item_selected", node_id, selected_item)
            ])
        }

        // Let the host handle it, but also emit a side effect.
        "scroll" => {
            let extra = OutgoingEvent::extension_event("scroll_stats", node_id, None);
            EventResult::Observed(vec![extra])
        }

        // Don't care about this event. Forward to host as-is.
        _ => EventResult::PassThrough,
    }
}
```

### EventResult

| Variant | Original event | Your events | When to use |
|---------|---------------|-------------|-------------|
| `PassThrough` | forwarded | none | you don't care about this event |
| `Consumed(events)` | suppressed | emitted | you're replacing the event with something better |
| `Observed(events)` | forwarded | also emitted | you want side effects without blocking the original |

### CoalesceHint for continuous events

If your extension emits high-frequency events (position tracking,
value scrubbing), set a `CoalesceHint` so the event throttling
system can rate-limit them:

```rust
let event = OutgoingEvent::extension_event("cursor_pos", node_id,
    Some(serde_json::json!({"x": pos.x, "y": pos.y})),
).with_coalesce(CoalesceHint::Replace);
```

`Replace` keeps only the latest event. `Accumulate(fields)` sums
the named fields across coalesced events (for deltas like scroll
distance). Events without a hint are always delivered immediately.

The host controls the rate via the `event_rate` prop on the widget
node or `default_event_rate` in Settings.

## Commands

### Tier C: receiving commands from the host

The host can send imperative commands to your extension:

```rust
fn handle_command(
    &mut self,
    node_id: &str,
    op: &str,
    payload: &Value,
    caches: &mut ExtensionCaches,
) -> Vec<OutgoingEvent> {
    match op {
        "reset" => {
            if let Some(state) = caches.get_mut::<MyState>(self.config_key(), node_id) {
                state.reset();
            }
            vec![] // no response events needed
        }
        "export" => {
            let format = payload.get("format").and_then(|v| v.as_str()).unwrap_or("png");
            // ... generate export ...
            vec![OutgoingEvent::extension_event("exported", node_id,
                Some(serde_json::json!({"format": format, "size": 1024}))
            )]
        }
        _ => vec![],
    }
}
```

Commands arrive as `extension_command` messages in the protocol.
The host SDK wraps them into typed function calls.

## Cleanup

When a node is removed from the tree (the host stops rendering your
widget), `cleanup()` is called:

```rust
fn cleanup(&mut self, node_id: &str, caches: &mut ExtensionCaches) {
    caches.remove(self.config_key(), node_id);
}
```

**Important:** If you don't implement `cleanup()`, cache entries
for removed nodes accumulate permanently. The default `cleanup()`
does nothing. Always remove your cache entries when a node leaves
the tree.

## Accessibility

Your extension gets accessibility support automatically. toddy's
`A11yOverride` wrapper intercepts `operate()` on your widget's
output and applies any `a11y` props the host sets on the node.

What this means in practice: if your extension renders an iced
`button`, that button is already accessible. The host can add
`"a11y": {"label": "Submit form"}` to the node and it flows
through to the screen reader.

**What you can do to help:**

- Use semantic iced widgets (button, text_input, checkbox) instead
  of bare containers with click handlers. Screen readers understand
  buttons; they don't understand clickable containers.
- Expose props that map to a11y fields. If your widget has a value,
  expose it so the host can set `a11y.value`. If it has a label,
  expose a `label` prop.
- If your widget composes multiple interactive elements, give each
  one a meaningful role. The host can set `a11y.position_in_set`
  and `a11y.size_of_set` for items in a group.

## Testing

The `toddy_core::testing` module provides helpers for writing unit
tests without a running renderer:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use toddy_core::testing::*;
    use serde_json::json;

    #[test]
    fn renders_with_default_props() {
        let ext = Gauge;
        let node = node_with_props("g1", "gauge", json!({"value": 50.0}));

        let test = TestEnv::default();
        let ctx = test.render_ctx();
        let env = test.env(&ctx);

        // This calls render() and verifies it doesn't panic.
        let _element = ext.render(&node, &env);
    }

    #[test]
    fn handles_missing_props_gracefully() {
        let ext = Gauge;
        let node = node("g1", "gauge"); // no props at all

        let test = TestEnv::default();
        let ctx = test.render_ctx();
        let env = test.env(&ctx);

        let _element = ext.render(&node, &env); // should not panic
    }

    #[test]
    fn event_transforms_click() {
        let mut ext = Gauge;
        let mut caches = ExtensionCaches::new();

        let result = ext.handle_event(
            "g1",
            "click",
            &json!({}),
            &mut caches,
        );

        match result {
            EventResult::Consumed(events) => {
                assert_eq!(events[0].family, "item_selected");
            }
            _ => panic!("expected Consumed"),
        }
    }
}
```

### TestEnv

`TestEnv` provides a minimal rendering environment. All fields are
public -- customize them before calling `env()`:

```rust
let mut test = TestEnv::default();
test.theme = Theme::Light; // test with light theme
let ctx = test.render_ctx();
let env = test.env(&ctx);
```

**Note:** `render_ctx()` and `env()` are separate calls because of
Rust's borrow rules. The `ctx` must outlive the `env`.

## Panic safety

All mutable extension methods are wrapped in `catch_unwind`. If
your extension panics:

1. The panic is logged with the node ID and extension name.
2. The panic is counted per extension.
3. After 3 consecutive `render()` panics, the extension is
   "poisoned" on the next tree update -- a red error placeholder
   is rendered instead of calling your code.
4. Poison clears on the next tree Snapshot, giving your extension
   a fresh start.
5. Other extensions and the rest of the renderer are unaffected.

**For debugging:** Set `TODDY_NO_CATCH_UNWIND=1` to let panics
propagate normally. This gives you a full backtrace instead of
the caught-and-logged message.

**Important:** Your crate must not set `panic = "abort"` in
Cargo.toml. toddy requires stack unwinding for panic isolation.
A compile-time error fires if abort is detected.

## Multi-session support

When the renderer runs with `--max-sessions > 1` (concurrent
client sessions), each session gets its own extension instance.
Implement `new_instance()` to create a fresh instance:

```rust
fn new_instance(&self) -> Box<dyn WidgetExtension> {
    Box::new(Gauge)
}
```

If your extension holds no state in `self` (all state is in
`ExtensionCaches`), this is trivial. If your extension has
constructor parameters, capture them:

```rust
pub struct Gauge { precision: usize }

impl WidgetExtension for Gauge {
    fn new_instance(&self) -> Box<dyn WidgetExtension> {
        Box::new(Gauge { precision: self.precision })
    }
    // ...
}
```

The default `new_instance()` implementation panics. If you don't
implement it, your extension works fine in single-session mode but
crashes on `--max-sessions > 1`.

## Complete example: a rating widget

A star rating widget that displays 1-5 stars and emits click events
when a star is selected.

```rust
use toddy_core::prelude::*;
use serde_json::json;

pub struct Rating;

impl WidgetExtension for Rating {
    fn type_names(&self) -> &[&str] { &["rating"] }
    fn config_key(&self) -> &str { "rating" }

    fn render<'a>(&self, node: &'a TreeNode, env: &WidgetEnv<'a>) -> Element<'a, Message> {
        let value = node.prop_f32("value").unwrap_or(0.0) as usize;
        let max = prop_u32(node.props(), "max").unwrap_or(5) as usize;
        let size = node.prop_f32("size").unwrap_or(24.0);
        let color = node.prop_color("color")
            .unwrap_or(env.theme().palette().primary.base.color);
        let disabled_color = Color { a: color.a * 0.3, ..color };

        let id = node.id.clone();
        let mut stars = row![].spacing(2);

        for i in 1..=max {
            let filled = i <= value;
            let star_color = if filled { color } else { disabled_color };
            let label = if filled { "\u{2605}" } else { "\u{2606}" }; // filled/empty star

            let star_text = text(label)
                .size(size)
                .color(star_color);

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
        Box::new(Rating)
    }
}
```

The host sends:

```json
{"id": "movie-rating", "type": "rating", "props": {"value": 3, "max": 5, "size": 32}}
```

When a star is clicked, the host receives:

```json
{"type": "event", "family": "select", "id": "movie-rating", "data": {"value": 4}}
```

## Troubleshooting

**Red "Extension error" placeholder.** Your extension panicked 3
times in `render()`. Check the logs (`RUST_LOG=toddy_core=debug`)
for the panic messages. Set `TODDY_NO_CATCH_UNWIND=1` for a full
backtrace.

**Widget doesn't appear.** The node's `"type"` doesn't match any
string in your `type_names()`. Check for typos. Type names are
case-sensitive.

**Props are always None.** Make sure you're reading the correct
key name. Use `RUST_LOG=toddy_core=trace` to see prop parsing
trace logs.

**ExtensionCaches returns None.** Either the key doesn't match
(check `config_key()` + node ID), or there's a type mismatch
(you inserted `TypeA` but are reading `TypeB`). Type mismatches
are logged as warnings.

**Compilation fails with iced version conflict.** You have a
direct `iced` dependency in your Cargo.toml. Remove it and use
`toddy_core::iced::*` instead.

**Panic: "does not support multiplexed sessions".** Your extension
is running with `--max-sessions > 1` but doesn't implement
`new_instance()`. Add it or run in single-session mode.

## Further reading

- `WidgetExtension` trait documentation in `toddy-core/src/extensions.rs`
- Prop helpers API in `toddy-core/src/prop_helpers.rs`
- Testing helpers in `toddy-core/src/testing.rs`
- [Core Widget Guide](core-widget-guide.md) for building reusable
  iced widgets
- [Widget Development](widget-development.md) for the decision
  framework (canvas vs extension vs core widget)
