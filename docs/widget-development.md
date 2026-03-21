# Widget Development

Three ways to build custom widgets for toddy, each for a different
situation.

## Canvas interactive shapes

Draw shapes from JSON. The renderer handles hit testing, hover
styles, keyboard navigation, drag, and accessibility locally.
Zero Rust code required.

Use this for: charts, diagrams, custom buttons, toggles, radio
groups, toolbars, and any widget where the visual is custom but
the interaction pattern is standard (click, hover, drag).

See the [interactive canvas shapes](protocol.md#interactive-canvas-shapes)
section in the protocol docs.

## Extension widgets

A Rust crate that implements `WidgetExtension` from `toddy-core`.
Your host SDK handles compilation and binary generation. You write
the widget logic.

Use this for: application-specific widgets that need native
rendering performance, complex state management, or access to iced's
widget library beyond what canvas provides. Most custom Rust widgets
use this path.

See the [Extension Guide](extension-guide.md).

## Reusable iced widgets

An iced widget that works directly in Rust applications AND across
every toddy SDK. You build the widget once as a standard iced
widget, then add a thin WidgetExtension wrapper for toddy
compatibility.

Use this for: widgets you want to share across the ecosystem --
a chart library, a date picker, a color wheel. Rust developers
use the widget directly. Elixir, Gleam, and other SDK users get
it through toddy without any per-language widget code.

See the [Core Widget Guide](core-widget-guide.md).

## Decision framework

| Need | Approach | Rust needed? |
|------|----------|-------------|
| Custom visuals, standard interaction | Canvas interactive shapes | no |
| Custom visuals + text editing | Compose canvas + `text_input` | no |
| Custom visuals + scrolling | Compose canvas + `scrollable` | no |
| Custom visuals + dropdown | Compose canvas + `overlay` | no |
| Application-specific native widget | Extension (`WidgetExtension`) | yes (basic) |
| Reusable widget (Rust + all SDKs) | iced widget + extension wrapper | yes (intermediate) |
| Maximum rendering performance | Extension or core widget | yes |

**Start with canvas.** Most custom widgets can be built from
canvas interactive shapes composed with built-in widgets. Move to
an extension only when canvas can't do what you need.
