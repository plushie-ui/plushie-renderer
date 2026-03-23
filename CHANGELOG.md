# Changelog

All notable changes to plushie will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## [Unreleased]

### Breaking changes

- **Canvas group redesign.** Interactive elements are now groups with
  top-level fields (`id`, `on_click`, `a11y`, etc.) instead of a nested
  `"interactive"` sub-object. Only groups can be interactive -- leaf
  shapes (rect, circle, etc.) are no longer interactive on their own.
- **Standalone transform/clip commands removed.** `push_transform`,
  `pop_transform`, `translate`, `rotate`, `scale`, `push_clip`,
  `pop_clip` are no longer supported as standalone shape types. Use the
  `transforms` array and `clip` field on groups instead.
- **Group `x`/`y` fields removed.** Use `transforms: [{"type":
  "translate", "x": ..., "y": ...}]` instead.
- **Event families renamed.** `canvas_shape_*` -> `canvas_element_*`,
  `shape_id` -> `element_id` in event data.

### Added

- **Transforms on groups.** Groups carry an ordered `transforms` array
  (translate, rotate, scale) and an optional `clip` field, replacing
  standalone push/pop commands.
- **Transform-aware hit testing.** Full 2D affine matrix tracks
  translate, rotate, and scale through nested groups. Cursor positions
  are mapped to local space via the inverse matrix. Clip regions from
  ancestor groups are intersected and tested.
- **Focus lifecycle events.** New event families: `canvas_element_blurred`,
  `canvas_focused`, `canvas_blurred`, `canvas_group_focused`,
  `canvas_group_blurred`.
- **Click-to-focus.** Clicking an interactive element grants the canvas
  keyboard focus and sets internal focus to the clicked element.
- **ID-based focus tracking.** Focus survives element reordering between
  renders. Stale elements are detected and blurred automatically.
- **Focus style.** New `focus_style` field on groups for visual feedback
  when keyboard-focused. Priority: pressed > hover > focus.
- **Suppressible focus ring.** `show_focus_ring: false` disables the
  default ring (use `focus_style` for custom indicators).
- **Geometry-aware focus rings.** Ring shape adapts to hit region: rounded
  rectangle for rect, circle for circle, capsule for line. Full transform
  support via matrix decomposition.
- **Arrow mode.** New `arrow_mode` canvas prop: `wrap` (default), `clamp`,
  `linear`, `none`.
- **Focusable groups.** Groups with `focusable: true` become Tab stops
  for two-level navigation. Tab moves between top-level entries, arrows
  navigate within the focused group.
- **Canvas accessible role.** New `role` canvas prop (defaults to `group`
  when interactive, `image` otherwise). `active_descendant` set
  dynamically from focused element.
- **Accessibility tree structure.** Focusable groups create parent-child
  relationships via `traverse()`. Child accessible nodes have widget IDs
  for `active_descendant` resolution.
- **Validation diagnostics.** Warnings for: interactive elements without
  `a11y`, stateful roles missing state props (switch/toggled,
  radio/selected), elements without set position.
- **`focus_element` widget op.** Programmatically focus a canvas and set
  internal focus to a specific element.
- **`click_element` / `focus_element` test interact actions.**
- **`CanvasElementFocusChanged` message.** Single message for blur+focus
  transitions, split into separate outgoing events by the emitter.
- **Theme-aware canvas colors.** Color strings in fill, stroke, and text
  that match iced palette names (`"primary"`, `"text"`, `"background"`,
  `"success"`, `"danger"`, `"warning"`) are resolved against the current
  theme at draw time. Canvas shapes now participate in the theme system.

### Fixed

- WASM build: replaced `wasm-opt --all-features` with explicit feature
  flags for compatibility with older wasm-opt versions.
- Canvas style overrides (hover_style, pressed_style, focus_style) now
  correctly read from the group, not from children.
- Canvas keyboard events now work when the mouse cursor is outside the
  canvas bounds.
- Canvas focus visuals (focus ring, focus_style) clear when the canvas
  loses iced-level focus.
- `text_editor` cursor no longer resets on every keystroke. Content hash
  is updated after TextEditorAction to prevent stale prop sync.
- `text_editor` cursor movement, selection, and click-to-position now
  work. All actions (not just edits) are performed on the Content.

## [0.4.1] - 2026-03-22

### Fixed

- Windows release build failure: `extern` block updated to `unsafe extern`
  as required by Rust 2024 edition.

## [0.4.0] - 2026-03-21

### Breaking changes

- **Project renamed from toddy to plushie.** All crate names, binary name,
  module paths, and import paths have changed: `toddy` -> `plushie`,
  `toddy-core` -> `plushie-core`, `toddy_core` -> `plushie_core`. The
  binary is now `plushie` (was `toddy`). The iced fork is now
  `plushie-iced` (was `toddy-iced`).
- **Crate split.** The single `toddy` crate is now three:
  `plushie-core` (SDK library), `plushie-renderer` (shared renderer
  logic, compilable to native and wasm32), and `plushie` (native binary).
  A fourth crate `plushie-wasm` provides the WASM entry point.
  Extension authors now depend on `plushie-core` instead of `toddy-core`.
- **Wire protocol field renames.** `canvas_scroll` position fields
  changed from `cursor_x`/`cursor_y` to `x`/`y`. `canvas_shape_drag`
  delta fields changed from `dx`/`dy` to `delta_x`/`delta_y`.
- **`scroll_to` field change.** The legacy `offset` key is removed;
  use `offset_y` only.
- **Scripting scroll event family renamed** from `scroll` to
  `wheel_scrolled` (the old name collided with the scrollable widget
  family).
- **Accessibility role name aliases removed.** Concatenated forms like
  `columnheader` are gone; use underscore form only (`column_header`).
- **Color format restricted.** Only hex notation (`#RRGGBB` /
  `#RRGGBBAA`) is accepted; other color notations are rejected.
- **Shaping prop renamed.** `text_shaping` is now `shaping`.
- **`OutgoingEvent` constructor signatures changed.** Parameter types
  standardized across the SDK; callers constructing events manually
  will need updating.
- **`CoalesceHint` added to `OutgoingEvent`.** The hardcoded coalescing
  table is removed. Extensions and host code that relied on implicit
  coalescing behavior must set `CoalesceHint` explicitly.
- **Core is zero-I/O.** Platform effects (file dialogs, clipboard,
  notifications) moved out of `plushie-core` into the binary crate.
  `Core` now returns `CoreEffect` variants instead of performing I/O.
  Extension authors using core directly will see a different effect API.
- **IME event family names changed** to avoid collisions with other
  event families.
- **Key event shapes unified.** Scripting and real key events now use
  the same field layout.

### Added

- Event throttling and coalescing system: `EventEmitter` with per-event
  `max_rate` and session-wide `default_event_rate` for rate-limited
  delivery. `CoalesceHint` on `OutgoingEvent` replaces hardcoded
  coalescing tables so extensions get equal footing with built-in events.
- Transport abstraction with `--exec` flag for SSH and remote rendering
  scenarios. A background writer thread handles non-blocking I/O in
  windowed mode.
- Canvas interactive shapes: hit testing, hover/pressed styles, drag
  events, tooltips, and semantic click/press/release events on
  individual shapes.
- Canvas shape groups for composing multi-shape elements into a single
  interactive unit.
- Canvas keyboard navigation: Tab/Shift-Tab between shapes, arrow keys,
  Home/End, PgUp/PgDown, Enter/Space activation, Escape to exit.
- Canvas shape accessibility via `A11yOverride` wrappers, using the same
  system as all other widgets. Focused event emitted on keyboard focus
  transitions.
- Canvas interactive field validation with warnings for unknown keys.
- Overlay `flip` prop for auto-flipping when popup content overflows the
  viewport edge.
- Overlay `align` prop for cross-axis alignment (start, center, end).
- Accessibility overrides: `disabled`, `position_in_set`, `size_of_set`,
  `has_popup` exposed to host SDKs.
- Table semantic roles (Table, Row, Cell, ColumnHeader) for screen
  reader navigation.
- Widget `label`, `alt`, `description`, and `decorative` props passed
  through to iced's accessibility layer.
- Headless mode: announce events and `find_focused` query responses.
- Session lifecycle events (`session_error`, `session_closed`) and
  error response when `max_sessions` is exceeded.
- Duplicate node ID detection and error reporting on snapshot.
- `Debug` impls on all public SDK types.
- Extension `InitCtx` and enriched `RenderCtx` with `window_id` and
  `scale_factor`.
- `TreeNode` convenience methods (`prop_str`, `prop_f32`, `prop_bool`,
  etc.) and `testing` module helpers for extension authors.
- Property-based tests (proptest) for codec and prop helpers.
- Headless mode: custom font loading from Settings and `load_font` ops.

### Changed

- `CoalesceHint` on `OutgoingEvent` drives coalescing decisions; the
  hardcoded coalescing table in the emitter is removed.
- Accessibility role names standardized to underscore form only
  (concatenated aliases like `columnheader` removed).
- Color values standardized to hex-only format (`#RRGGBB` /
  `#RRGGBBAA`); other notations are rejected.
- `parse_shaping` reads the `shaping` prop (was `text_shaping`).
- Core is zero-I/O: platform effects moved out of `plushie-core` into the
  binary crate. Core now returns `CoreEffect` variants instead of
  performing I/O directly.
- Shared message processing logic between daemon and headless modes
  (extracted into reusable helpers).
- Extension caches unified: `core.caches.extension` used everywhere
  instead of separate per-mode storage.
- `canvas_scroll` position fields renamed from `cursor_x`/`cursor_y` to
  `x`/`y`.
- `canvas_shape_drag` delta fields use `delta_x`/`delta_y` (not
  `dx`/`dy`).
- Scripting scroll uses `wheel_scrolled` event family (was `scroll`,
  which collided with the scrollable widget family).
- `scroll_to` uses `offset_y` only (removed legacy `offset` key).
- Workspace-level lints replace per-crate `#![deny(warnings)]`.
- `OutgoingEvent` constructor parameter types standardized across the
  SDK.
- Scripting and real key event shapes unified.
- IME events use distinct family names to avoid collisions.
- Event field names aligned with protocol spec.

### Fixed

- Overlay `operate()` forwards to both anchor and content children,
  fixing accessibility and focus traversal for overlaid widgets.
- Subscription rate not cleared when re-subscribing with `max_rate`
  removed; coalesce key collision between similarly-named events.
- `prop_f32`/`prop_f64` reject NaN and Infinity from string parsing.
- Input clamping across widget props (padding, color channels, range
  bounds, spacing, opacity, etc.).
- Content size limits: markdown capped at 1 MB, text_editor at 10 MB.
- Resource limits: images capped at 4096 handles / 1 GiB total, fonts
  at 16 MiB per file / 256 runtime loads, font family name length
  bounded, dash segment intern cache bounded.
- Tree depth limit (256) on recursive functions (`find_window`,
  `collect_window_ids`).
- Window size and position clamped to reasonable bounds.
- Animation epoch resets on `Reset` message for clean hot-reload.
- Bounded channels for multiplexed headless sessions, preventing
  unbounded memory growth.
- Session thread `catch_unwind` with error events; extension
  `catch_unwind` on `clone_for_session` and `handle_event`.
- Validate schemas added for checkbox, toggler, and radio (`line_height`,
  `wrapping`, `shaping`) and pane_grid (`split_axis`).
- Image `border_radius` validate type corrected (Number, was Any).
- `f64`-to-`f32` conversions clamped via `f64_to_f32` helper to avoid
  silent overflow.
- `tree_hash` returns sentinel on serialization failure instead of
  panicking.
- Headless mode: canvas interact actions (`canvas_press`, `canvas_release`,
  `canvas_move`) now inject real iced mouse events, producing shape-level
  events (enter/leave/click/drag) just like windowed mode. Previously
  they were synthetic-only and could not trigger canvas shape interaction.
- Headless mode: break event injection loop on EOF mid-interact; use
  `cancelled` status for unavailable async effects; emit
  `theme_changed` subscription events.
- Decode errors include debug context; `set_global` panic documented.
- Binary mode set on stdin/stdout for Windows compatibility.
- `list_images` query returns correct response kind.
- Wayland: no-op warnings for unsupported window position ops;
  fullscreen behavior documented.
- `last_slide_values` cleared on snapshot to avoid stale slider state.
- `ExtensionCaches` logs warning on type mismatch in `get`/`get_mut`
  instead of silently returning `None`.
- Font log accuracy and panic logging improvements.

## [0.3.1] - 2026-03-19

### Fixed

- Preserve iced widget defaults when props are unset. Padding,
  spacing, text size, and other optional props now use `Option`
  return types from parsers. When absent from the wire message,
  the widget setter is skipped and iced uses its built-in default.
  Affected widgets: button, container, window, column, row, grid,
  keyed_column, text_input, pick_list, combo_box, table.

## [0.3.0] - 2026-03-19

Initial public release.
