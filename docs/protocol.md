# Wire Protocol

The renderer communicates with the host process over stdin (incoming
messages) and stdout (outgoing events). All log output goes to stderr.

Protocol version: **1**

## Encoding

Two wire formats are supported. Both carry the same message structures.

### JSON

One JSON object per line (JSONL). No length prefix.

    {"session":"","type":"settings","settings":{"default_text_size":14}}\n
    {"session":"","type":"snapshot","tree":{"id":"root","type":"column",...}}\n

### MessagePack

Each message is a 4-byte big-endian length prefix followed by a
MessagePack payload.

    [4 bytes: payload length as u32 BE][msgpack payload bytes]

### Choosing a format

JSON works for most use cases. MessagePack is better when sending
binary data (images, pixel buffers) or when serialization overhead
matters at high update rates.

### Format detection

The renderer auto-detects the format from the first byte of stdin:

- `0x7B` (`{`) -- JSON
- Anything else -- MessagePack

Override with `--json` or `--msgpack` CLI flags.

### Limits

Maximum message size: **64 MiB**. Messages exceeding this are rejected.

---

## Sessions

Every wire message carries a `session` field (string) identifying the
logical session it belongs to. In single-session mode (the default),
all messages use the same session value. In multiplexed mode
(`--max-sessions N` with N > 1), multiple sessions run concurrently
in separate threads, each with fully isolated state.

The renderer echoes the `session` value from each incoming message
back on the corresponding outgoing message(s). This is routing
metadata, not message content -- the renderer does not interpret the
session value beyond using it for dispatch.

```json
{"session": "test_42", "type": "snapshot", "tree": {...}}
{"session": "test_42", "type": "query", "id": "q1", ...}
```

**Session lifecycle in multiplexed mode:**

- A session is created implicitly when the first message with a new
  session value arrives.
- A `Reset` message tears down the session (thread exits, all state
  freed). The session value can be reused -- a new session is created
  on the next message.
- The `--max-sessions` flag limits concurrent sessions. Messages for
  new sessions beyond the limit are dropped with a log error.

**Single-session mode:** When `--max-sessions` is 1 (or omitted),
the renderer runs one session on the main thread with no threading
overhead. The session field is still present on all messages.

### Session lifecycle events

In multiplexed mode, the renderer emits lifecycle events to inform the
host when sessions fail or complete teardown.

**session_error**: Emitted when a session thread panics or when
`max_sessions` is exceeded.

```json
{"type": "event", "session": "s1", "family": "session_error", "id": "", "data": {"error": "session thread panicked: ..."}}
```

- Emitted to stdout so the host knows a session is no longer functional.
- The `session` field identifies which session failed.

**session_closed**: Emitted after a Reset completes and the session
thread exits.

```json
{"type": "event", "session": "s1", "family": "session_closed", "id": "", "data": {"reason": "reset"}}
```

- Confirms the old session has fully torn down.
- The host should wait for this before recycling the session ID.

---

## Startup sequence

1. Host spawns the renderer and writes a **Settings** message to stdin.
2. Renderer detects the wire format from the first byte.
3. Renderer reads and applies the Settings.
4. Renderer writes a **hello** message to stdout.
5. Normal message exchange begins.

The hello message confirms the renderer is ready and reports its
protocol version:

```json
{
  "type": "hello",
  "session": "",
  "protocol": 1,
  "version": "0.3.0",
  "name": "plushie-renderer",
  "mode": "headless",
  "backend": "tiny-skia",
  "transport": "stdio",
  "extensions": ["charts", "editor"]
}
```

| Field | Type | Description |
|-------|------|-------------|
| `protocol` | number | Protocol version (currently 1) |
| `version` | string | Renderer build version |
| `name` | string | Renderer name (always `"plushie-renderer"`) |
| `mode` | string | Execution mode: `"windowed"`, `"headless"`, or `"mock"` |
| `backend` | string | Rendering backend: `"wgpu"` (windowed), `"tiny-skia"` (headless), `"none"` (mock) |
| `transport` | string | Transport backend: `"stdio"` (default) or `"exec"` (future: `"connect"`, `"listen"`) |
| `extensions` | array | Config keys of registered widget extensions (empty array if none) |

The host should check that `protocol` matches the version it expects.
The `mode` field tells the SDK what capabilities are available (e.g.
headless mode supports `interact_step` round-trips and real
screenshots; mock mode returns stubs). The `session` field on
`hello` is always empty -- it is a process-level message, not
scoped to any session.

---

## Common value types

**Colors** are canonical hex strings: `"#rrggbb"` (6-char) or
`"#rrggbbaa"` (8-char with alpha). Short forms (`#rgb`, `#rgba`)
are not accepted -- the host must normalize before sending.

**Lengths** are numbers (pixels), `"fill"`, `"shrink"`, or
`{"fill_portion": n}`.

**Padding** is a number (uniform), `[vertical, horizontal]`, or
`[top, right, bottom, left]`.

---

## Tree nodes

A UI tree is a nested structure of nodes. Every node has four fields:

```json
{
  "id": "unique-string",
  "type": "widget-type",
  "props": {},
  "children": []
}
```

| Field      | Type     | Description |
|------------|----------|-------------|
| `id`       | string   | Unique identifier for this node |
| `type`     | string   | Widget type (e.g. `"text"`, `"button"`, `"column"`) |
| `props`    | object   | Widget-specific properties |
| `children` | array    | Child TreeNode objects |

Window nodes (`"type": "window"`) are special -- they map to native
windows. Place them at the top level of the tree (root or direct
children of root).

---

## Incoming messages (host -> renderer)

All messages are JSON objects with a `"type"` field that determines
the message kind and a `"session"` field identifying the session.
Field names use `snake_case`.

### Settings

Sent as the first message. Configures the renderer.

```json
{
  "type": "settings",
  "session": "s1",
  "settings": {
    "protocol_version": 1,
    "default_text_size": 14.0,
    "default_font": { "family": "monospace" },
    "antialiasing": false,
    "vsync": true,
    "fonts": ["/path/to/font.ttf"],
    "scale_factor": 1.0,
    "validate_props": false,
    "extension_config": {}
  }
}
```

All fields inside `settings` are optional.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `protocol_version` | number | 1 | Expected protocol version |
| `default_text_size` | number | 16.0 | Default text size for all text widgets |
| `default_font` | object | system | Default font descriptor (`{"family": "..."}`) |
| `antialiasing` | bool | false | Enable anti-aliasing (startup only) |
| `vsync` | bool | true | Enable VSync (startup only) |
| `fonts` | array | [] | Paths to font files to load (startup only) |
| `scale_factor` | number | 1.0 | Global scale factor (startup only) |
| `validate_props` | bool | false | Enable prop validation warnings in release builds |
| `extension_config` | object | {} | Configuration passed to widget extensions |
| `default_event_rate` | number | -- | Default max events per second for all coalescable events. Omit for unlimited. |

**Startup-only fields** (ignored if sent after the first Settings):
`antialiasing`, `vsync`, `fonts`, `scale_factor`, `validate_props`.

**Runtime fields** (can be updated by sending Settings again):
`default_text_size`, `default_font`, `extension_config`,
`default_event_rate`.

**Log level.** Renderer log verbosity is controlled via the `RUST_LOG`
environment variable on the renderer process, not via a Settings field.
The renderer's built-in default level is `warn`. Examples:
`RUST_LOG=plushie_renderer=debug`, `RUST_LOG=plushie_ext::widgets=trace`.

### Snapshot

Replace the entire tree. The simplest way to update the UI -- no
diffing required on the host side.

```json
{
  "type": "snapshot",
  "session": "s1",
  "tree": {
    "id": "root",
    "type": "window",
    "props": { "title": "My App" },
    "children": [...]
  }
}
```

The renderer replaces the current tree, reconciles windows (opens new
ones, closes removed ones), and re-renders.

**Duplicate node IDs.** When a snapshot contains duplicate node IDs,
the renderer accepts the tree but emits an error event:

```json
{"type": "event", "session": "", "family": "error", "id": "duplicate_node_ids", "data": {"error": "snapshot contains duplicate node IDs", "duplicates": ["btn1 (button)", "btn1 (text)"]}}
```

The host should treat this as a bug in the tree construction. Duplicate
IDs cause undefined behavior in widget caching and event routing.

### Patch

Incrementally update the existing tree. More efficient than Snapshot
for large trees with small changes.

```json
{
  "type": "patch",
  "session": "s1",
  "ops": [...]
}
```

Each operation in `ops` is an object with an `op` field and a `path`
field. The path is an array of child indices from the root to the
target node.

#### replace_node

Replace the node at the given path.

```json
{
  "op": "replace_node",
  "path": [0, 2],
  "node": { "id": "new", "type": "text", "props": {}, "children": [] }
}
```

An empty path replaces the root.

#### update_props

Merge properties into the node at the given path. Setting a value to
`null` removes that key.

```json
{
  "op": "update_props",
  "path": [0],
  "props": { "label": "updated", "old_key": null }
}
```

#### insert_child

Insert a child node at the given index under the parent at path.

```json
{
  "op": "insert_child",
  "path": [0],
  "index": 2,
  "node": { "id": "new-child", "type": "text", "props": {}, "children": [] }
}
```

If `index` exceeds the number of children, the node is appended.

#### remove_child

Remove the child at the given index under the parent at path.

```json
{
  "op": "remove_child",
  "path": [0],
  "index": 2
}
```

#### Error handling

Operations are applied sequentially. If one fails (missing fields,
out-of-bounds path), it is skipped with a warning and subsequent
operations still apply.

#### Ordering guarantee

The host emits patch operations in a safe application order:
removals (descending child index), then updates (with indices adjusted
for prior removals), then inserts (ascending child index). The
renderer MUST apply operations in the order they appear in the `ops`
array. Reordering operations will produce incorrect results because
update paths reference the tree state after removals but before inserts.

### Subscribe

Subscribe to a category of events. The `tag` is included in events of
this kind so the host can route them.

```json
{
  "type": "subscribe",
  "session": "s1",
  "kind": "on_key_press",
  "tag": "my_key_handler"
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `kind` | string | Yes | Event category to subscribe to (see table below) |
| `tag` | string | Yes | Tag included in events of this kind for routing |
| `max_rate` | integer | No | Maximum events per second. Omit for unlimited. Zero means "subscribe but never emit." |

When `max_rate` is set, the renderer delivers at most that many events
per second for this subscription kind. Between deliveries, it coalesces
buffered events (latest value wins, or delta accumulation for scroll
events). Re-subscribing with a different `max_rate` updates the limit
in place.

**Available subscription kinds:**

| Kind | Events delivered |
|------|-----------------|
| `on_key_press` | Key press with key, modifiers, text |
| `on_key_release` | Key release |
| `on_modifiers_changed` | Modifier key state change |
| `on_mouse_move` | Cursor moved, entered, left |
| `on_mouse_button` | Mouse button pressed/released |
| `on_mouse_scroll` | Scroll wheel |
| `on_touch` | Finger press/move/lift/lost |
| `on_ime` | Input method events (open, preedit, commit, close) |
| `on_window_event` | All window lifecycle events |
| `on_window_open` | Window opened |
| `on_window_close` | Window close requested |
| `on_window_resize` | Window resized |
| `on_window_move` | Window moved |
| `on_window_focus` | Window gained focus |
| `on_window_unfocus` | Window lost focus |
| `on_file_drop` | File hovered/dropped on window |
| `on_animation_frame` | Per-frame timestamp (for animations) |
| `on_theme_change` | System theme changed (light/dark) |
| `on_event` | Catch-all: all keyboard, mouse, touch, and IME events |

`on_event` is a convenience that subscribes to everything at once. If
both `on_event` and a specific subscription (e.g. `on_key_press`) are
registered, events are delivered once, not twice.

### Unsubscribe

Remove a subscription.

```json
{
  "type": "unsubscribe",
  "session": "s1",
  "kind": "on_key_press"
}
```

### WidgetOp

Perform an operation on a widget (focus, scroll, etc.).

```json
{
  "type": "widget_op",
  "session": "s1",
  "op": "focus",
  "payload": { "target": "input-1" }
}
```

**Operations:**

| Op | Payload | Description |
|----|---------|-------------|
| `focus` | `target` | Focus a widget by ID |
| `focus_next` | -- | Focus next focusable widget |
| `focus_previous` | -- | Focus previous focusable widget |
| `focus_element` | `target` (canvas ID), `element_id` | Focus a canvas and set internal focus to a specific element |
| `scroll_to` | `target`, `offset_x`, `offset`/`offset_y` | Scroll to absolute offset |
| `scroll_by` | `target`, `offset_x`, `offset_y` | Scroll by relative amount |
| `snap_to` | `target`, `x`, `y` | Snap scrollable to relative position (0.0-1.0) |
| `snap_to_end` | `target` | Snap scrollable to end |
| `select_all` | `target` | Select all text |
| `select_range` | `target`, `start`, `end` | Select text range |
| `move_cursor_to` | `target`, `position` | Move cursor to position |
| `move_cursor_to_front` | `target` | Move cursor to start |
| `move_cursor_to_end` | `target` | Move cursor to end |
| `close_window` | `window_id` | Close a window |
| `announce` | `text` | Screen reader announcement (no visible widget needed) |
| `exit` | -- | Exit the renderer |
| `pane_split` | `target`, `pane`, `axis`, `new_pane_id` | Split a pane |
| `pane_close` | `target`, `pane` | Close a pane |
| `pane_swap` | `target`, `a`, `b` | Swap two panes |
| `pane_maximize` | `target`, `pane` | Maximize a pane |
| `pane_restore` | `target` | Restore maximized pane |
| `tree_hash` | `tag` (optional) | Compute SHA-256 hash of current tree; response via `op_query_response` |
| `find_focused` | `tag` (optional) | Find the currently focused widget; response via `op_query_response` |
| `load_font` | `data` (base64 TTF/OTF) | Load a font at runtime |
| `list_images` | `tag` (optional) | List all image handle names; response via `op_query_response` |
| `clear_images` | -- | Remove all in-memory image handles |

Widget op query responses (`tree_hash`, `find_focused`, `list_images`)
use the `op_query_response` outgoing message type.

### WindowOp

Manage windows directly (outside of tree-driven sync).

```json
{
  "type": "window_op",
  "session": "s1",
  "op": "open",
  "window_id": "win-1",
  "settings": { "width": 800, "height": 600, "title": "New Window" }
}
```

**Operations:**

| Op | Description |
|----|-------------|
| `open` | Open a new window |
| `close` | Close a window |
| `update` | Update window properties |
| `resize` | Resize (width, height) |
| `move` | Move (x, y) |
| `maximize` | Maximize (maximized: bool) |
| `minimize` | Minimize (minimized: bool) |
| `set_mode` | Set mode (windowed, fullscreen, hidden) |
| `toggle_maximize` | Toggle maximized state |
| `toggle_decorations` | Toggle window decorations |
| `gain_focus` | Bring window to front |
| `set_level` | Set window level (normal, always_on_top, always_on_bottom) |
| `drag` | Begin window drag |
| `drag_resize` | Begin window resize drag (direction) |
| `request_attention` | Flash taskbar (urgency: informational, critical) |
| `show_system_menu` | Show system menu (Windows only) |
| `set_resizable` | Set resizable (bool) |
| `set_min_size` | Set minimum size (width, height) |
| `set_max_size` | Set maximum size (width, height) |
| `mouse_passthrough` | Enable/disable click-through (enabled: bool) |
| `set_resize_increments` | Set resize step size (width, height) |
| `allow_automatic_tabbing` | macOS automatic tab grouping (enabled: bool) |

**Query operations** (response sent as `effect_response`):

| Op | Response fields |
|----|-----------------|
| `get_size` | width, height |
| `get_position` | x, y |
| `get_mode` | mode |
| `get_scale_factor` | scale_factor |
| `is_maximized` | maximized |
| `is_minimized` | minimized |
| `screenshot` | width, height, bytes_len, rgba |
| `raw_id` | raw_id, platform |
| `monitor_size` | width, height (logical pixels) |
| `set_icon` | icon_data (base64 RGBA), width, height |

These accept an optional `request_id` field in settings, echoed
back in the response for correlation.

**System query operations** (response sent as `op_query_response`):

| Op | Response kind | Response data |
|----|---------------|---------------|
| `get_system_theme` | `system_theme` | `"light"` or `"dark"` |
| `get_system_info` | `system_info` | CPU, memory, GPU info object |

System queries use a `tag` field in the payload (like widget op
queries) and produce `op_query_response` rather than
`effect_response`.

### Effect

Request a platform effect (file dialog, clipboard, notification).

```json
{
  "type": "effect",
  "session": "s1",
  "id": "req-1",
  "kind": "file_open",
  "payload": {
    "title": "Open File",
    "directory": "/home/user",
    "filters": [["Text (*.txt)", "*.txt"], ["All Files", "*"]]
  }
}
```

**Effect kinds:**

| Kind | Payload | Response |
|------|---------|----------|
| `file_open` | title, directory, filters | path |
| `file_open_multiple` | title, directory, filters | paths (array) |
| `file_save` | title, directory, filters, default_name | path |
| `directory_select` | title, directory | path |
| `directory_select_multiple` | title, directory | paths (array) |
| `clipboard_read` | -- | text |
| `clipboard_write` | text | -- |
| `clipboard_read_html` | -- | html |
| `clipboard_write_html` | html, alt_text (optional) | -- |
| `clipboard_clear` | -- | -- |
| `clipboard_read_primary` | -- | text (Linux only) |
| `clipboard_write_primary` | text | text (Linux only) |
| `notification` | title, body, icon, timeout, urgency, sound | -- |

**Notification options.** The `notification` effect accepts optional fields
beyond `title` and `body`:

| Field | Type | Description |
|-------|------|-------------|
| `icon` | string | Icon name (freedesktop icon spec, e.g. `"dialog-information"`) |
| `timeout` | number | Timeout in milliseconds |
| `urgency` | string | `"low"`, `"normal"` (default), or `"critical"` |
| `sound` | string | Sound theme name (e.g. `"message-new-instant"`) |

### ImageOp

Manage in-memory image handles for use by image widgets.

```json
{
  "type": "image_op",
  "session": "s1",
  "op": "create_image",
  "handle": "sprite-1",
  "data": "<base64-encoded PNG/JPEG bytes>"
}
```

Or with raw RGBA pixels:

```json
{
  "type": "image_op",
  "session": "s1",
  "op": "create_image",
  "handle": "sprite-1",
  "pixels": "<base64-encoded RGBA bytes>",
  "width": 64,
  "height": 64
}
```

| Op | Description |
|----|-------------|
| `create_image` | Create or replace an image handle |
| `update_image` | Same as create_image |
| `delete_image` | Remove an image handle |

In MessagePack mode, `data` and `pixels` can be sent as raw binary
(no base64 encoding needed).

### ExtensionCommand

Send a command directly to a native widget extension, bypassing the
tree update cycle. Used for high-frequency data (e.g. pushing plot
data to a chart extension).

```json
{
  "type": "extension_command",
  "session": "s1",
  "node_id": "chart-1",
  "op": "append_data",
  "payload": { "values": [1.0, 2.5, 3.7] }
}
```

### ExtensionCommands

Send multiple extension commands in a single message.

```json
{
  "type": "extension_commands",
  "session": "s1",
  "commands": [
    { "node_id": "chart-1", "op": "append_data", "payload": {...} },
    { "node_id": "chart-2", "op": "clear", "payload": {} }
  ]
}
```

### Query

Inspect the tree or find widgets by selector.

```json
{
  "type": "query",
  "session": "s1",
  "id": "q1",
  "target": "find",
  "selector": {"by": "id", "value": "btn1"}
}
```

| Field | Type | Description |
|-------|------|-------------|
| `id` | string | Request ID for response correlation |
| `target` | string | `"find"` (find widget) or `"tree"` (full tree) |
| `selector` | object | Selector for find queries (see below) |

**Selector format:**

| by | Description |
|----|-------------|
| `id` | Find by node ID |
| `text` | Find by text content |
| `role` | Find by a11y role |
| `label` | Find by a11y label |
| `focused` | Find the focused widget (no value field needed) |

**Selector search semantics:**

Selectors search the tree depth-first (max depth 256). The first
matching node is returned.

| by | Matches against |
|----|----------------|
| `id` | Exact match on node `id` field |
| `text` | Node `props.content`, `props.label`, `props.value`, or `props.placeholder` |
| `role` | Node `props.a11y.role`, or falls back to `type` field (e.g. type `"button"` matches role `"button"`) |
| `label` | Node `props.a11y.label`, or falls back to `props.label` or `props.content` |
| `focused` | Node with `props.focused == true` or `props.a11y.focused == true` |

**When not found:** Query returns `data: null`. Interact returns
empty events list.

Response: `query_response` with `id`, `target`, `data`.

**Example: full tree query**

```json
{
  "type": "query",
  "session": "s1",
  "id": "q2",
  "target": "tree",
  "selector": {}
}
```

Returns the entire tree as `data`, or `null` if no tree has been
sent via Snapshot.

### Interact

Simulate user interactions (click, type, etc.). Available in both
all modes (gui, headless, mock) for programmatic inspection and
interaction.

```json
{
  "type": "interact",
  "session": "s1",
  "id": "i1",
  "action": "click",
  "selector": {"by": "id", "value": "submit_btn"},
  "payload": {}
}
```

| Field | Type | Description |
|-------|------|-------------|
| `id` | string | Request ID for response correlation |
| `action` | string | Interaction type (see action table below) |
| `selector` | object | Target widget selector (see Query for format). Required for widget-specific actions, optional for global actions like `press`/`release`/`move_to`/`scroll`. |
| `payload` | object | Action-specific parameters (see payload table below) |

**Actions and their iced event mappings:**

| Action | Iced events injected | Typical widget response |
|--------|---------------------|------------------------|
| `click` | CursorMoved, ButtonPressed, ButtonReleased | Click |
| `toggle` | CursorMoved, ButtonPressed, ButtonReleased | Toggle |
| `select` | CursorMoved, ButtonPressed, ButtonReleased | Select |
| `type_text` | KeyPressed + KeyReleased per character | Input per char |
| `type_key` | KeyPressed, KeyReleased | Depends on widget |
| `press` | KeyPressed | Depends on widget |
| `release` | KeyReleased | Depends on widget |
| `submit` | KeyPressed(Enter), KeyReleased(Enter) | Submit |
| `scroll` | WheelScrolled | Scroll |
| `move_to` | CursorMoved | -- |
| `slide` | synthetic only | Slide |
| `paste` | synthetic only | Paste |
| `sort` | synthetic only | Sort |
| `canvas_press` | CursorMoved, ButtonPressed | Canvas press (also triggers element events) |
| `canvas_release` | CursorMoved, ButtonReleased | Canvas release (also triggers element events) |
| `canvas_move` | CursorMoved | Canvas move (also triggers element enter/leave) |
| `click_element` | CursorMoved, ButtonPressed, ButtonReleased | Click at coordinates within canvas |
| `focus_element` | KeyPressed (Tab) | Tab into canvas |
| `pane_focus_cycle` | synthetic only | Pane focus cycle |

Actions marked **synthetic only** have no iced event equivalent
(e.g. slider requires a precise mouse drag, paste has no iced
input event). The renderer produces synthetic OutgoingEvents
directly without widget processing.

**Action payloads:**

| Action | Payload fields | Description |
|--------|---------------|-------------|
| `click` | (none) | |
| `toggle` | `value` (bool) | Toggle value. Defaults to `false` if omitted. SDKs should compute the inverse of the current widget state (e.g. read `is_checked` from tree props, invert it). In headless mode, the real widget value is captured from iced regardless of this field. |
| `select` | `value` (string) | Value to select from a pick_list, combo_box, or radio group. |
| `type_text` | `text` (string, required) | Text to type into the widget |
| `type_key` | `key` (string, required) | Key to press and release (see Key format below) |
| `press` | `key` (string, required) | Key to press (key down only) |
| `release` | `key` (string, required) | Key to release (key up only) |
| `submit` | `value` (string) | Submit value. Defaults to `""` if omitted. The renderer does not read from the tree -- SDKs should read the widget's current `props.value` and provide it. |
| `scroll` | `delta_x` (number), `delta_y` (number) | Scroll deltas in lines |
| `move_to` | `x` (number), `y` (number) | Cursor position in logical pixels |
| `slide` | `value` (number, required) | Slider value |
| `paste` | `text` (string, required) | Text to paste |
| `sort` | `column` (string, required) | Column key to sort by |
| `canvas_press` | `x` (number), `y` (number) | Canvas coordinates |
| `canvas_release` | `x` (number), `y` (number) | Canvas coordinates |
| `canvas_move` | `x` (number), `y` (number) | Canvas coordinates |
| `click_element` | `x` (number), `y` (number) | Element center in canvas coordinates |
| `focus_element` | (none) | |
| `pane_focus_cycle` | (none) | |

**Key format:**

Keys for `press`, `release`, and `type_key` actions can be specified
in two formats:

Combined format (modifiers joined with `+`):
```json
{"key": "ctrl+shift+s"}
```

Explicit modifiers:
```json
{"key": "a", "modifiers": {"ctrl": true, "shift": false, "alt": false, "logo": false}}
```

Modifier aliases: `command` maps to `ctrl`, `super` and `meta` map
to `logo`.

Named keys (case-insensitive, aliases separated by `/`):

`Enter`/`Return`, `Tab`, `Space`, `Escape`/`Esc`,
`Backspace`, `Delete`/`Del`, `ArrowUp`/`Up`,
`ArrowDown`/`Down`, `ArrowLeft`/`Left`, `ArrowRight`/`Right`,
`Home`, `End`, `PageUp`/`Page_Up`, `PageDown`/`Page_Down`,
`F1` through `F12`.

Single characters (e.g. `"a"`, `"1"`, `"/"`) are sent as character
key events. Multi-character strings that don't match a named key
are sent as-is (the renderer does not reject them).

In **windowed mode**, all actions produce synthetic events regardless
-- the interact protocol is a scripting convenience, not a
substitute for real user input via iced subscriptions.

#### Headless mode: iterative interact with round-trips

In `--headless` mode, the renderer injects iced events one at a
time. When an event produces widget Messages, the renderer emits
an `interact_step` and waits for the host to process the events
and send back a Snapshot with the updated tree before continuing.
This matches production behaviour where each event triggers a full
host round-trip.

```
Host -> Renderer:  interact(type_key, ...)
Renderer -> Host:  interact_step(events: [key_press])
Host -> Renderer:  snapshot(updated_tree)
Renderer -> Host:  interact_step(events: [key_release])
Host -> Renderer:  snapshot(updated_tree)
Renderer -> Host:  interact_response(events: [])
```

The final `interact_response` carries an empty events list when
steps were used (events were already delivered via steps). For
actions with no iced events (synthetic-only), no steps are emitted
and all events are in the final response.

#### Mock mode: synthetic events

In `--mock` mode, there is no iced renderer. All events are
synthetic -- constructed from the action name and selector without
widget processing. No `interact_step` messages are emitted. All
events are in the final `interact_response`.

### TreeHash

Compute a SHA-256 hash of the renderer's current tree (serialized
as JSON). Used for structural regression testing.

```json
{
  "type": "tree_hash",
  "session": "s1",
  "id": "th1",
  "name": "after_click"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `id` | string | Request ID for response correlation |
| `name` | string | Label for this hash capture |

Response: `tree_hash_response`.

**Scope.** The hash covers tree structure and props only (the
serialized JSON representation of the node tree). Mutable widget
state -- editor content, scroll position, slider value, canvas
cache state, etc. -- is not included. Two trees with identical
structure and props will produce the same hash even if their
runtime widget state differs.

### Screenshot

Capture rendered pixels. In headless mode, renders the tree via
tiny-skia and returns RGBA pixel data. In mock mode, returns an
empty stub.

```json
{
  "type": "screenshot",
  "session": "s1",
  "id": "sc1",
  "name": "homepage",
  "width": 1024,
  "height": 768
}
```

| Field | Type | Description |
|-------|------|-------------|
| `id` | string | Request ID |
| `name` | string | Label for this screenshot |
| `width` | number | Viewport width in pixels (optional, default 1024) |
| `height` | number | Viewport height in pixels (optional, default 768) |

Response: `screenshot_response`.

**Cross-platform determinism.** Screenshot hashes may differ across
platforms due to font rendering and hinting. For deterministic
screenshot comparison in CI, pin fonts via the `fonts` array in
Settings and avoid relying on system fonts. The headless backend
(tiny-skia) produces consistent output across Linux environments
when the same fonts are loaded.

### Reset

Reset all session state: tree, caches, images, theme, extensions.
In multiplexed mode, the session thread is torn down and the session
ID can be reused.

```json
{
  "type": "reset",
  "session": "s1",
  "id": "r1"
}
```

Response: `reset_response`.

**Multiplexed mode ordering constraint.** In multiplexed mode, the host
must wait for the `reset_response` before sending new messages to a
recycled session ID. Without this, the host may receive stale responses
from the old session interleaved with responses from the new one, because
the old session thread may still be draining its channel when the reader
thread creates a replacement.

### AdvanceFrame

Advance the animation clock by one frame. If `on_animation_frame`
is subscribed, emits an `animation_frame` event with the given
timestamp. Used for deterministic animation testing in headless/mock
mode.

```json
{
  "type": "advance_frame",
  "session": "s1",
  "timestamp": 16000
}
```

| Field | Type | Description |
|-------|------|-------------|
| `timestamp` | number | Frame timestamp, passed through to the `animation_frame` event as-is. By convention, milliseconds (matching the windowed mode's `Instant::as_millis()` output). |

---

## Outgoing messages (renderer -> host)

### Request-response reference

Every request message produces exactly one response. The `id` and
`session` fields are echoed back for correlation.

| Request | Response type | Notes |
|---------|--------------|-------|
| Query | `query_response` | |
| Interact | `interact_response` | May be preceded by `interact_step` messages in headless mode |
| TreeHash | `tree_hash_response` | |
| Screenshot | `screenshot_response` | |
| Reset | `reset_response` | |
| Effect | `effect_response` | |
| WidgetOp (query ops) | `op_query_response` | tree_hash, find_focused, list_images |
| WindowOp (query ops) | `effect_response` | get_size, get_position, get_mode, etc. |
| WindowOp (system queries) | `op_query_response` | get_system_theme, get_system_info |

Messages without responses: Settings, Snapshot, Patch,
Subscribe, Unsubscribe, WidgetOp
(non-query), WindowOp (non-query), ImageOp, ExtensionCommand,
ExtensionCommands, AdvanceFrame.

### event

User interaction or subscription event.

```json
{
  "type": "event",
  "session": "s1",
  "family": "click",
  "id": "btn-1"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `type` | string | Always `"event"` |
| `session` | string | Session that produced this event |
| `family` | string | Event kind (see tables below) |
| `id` | string | Node ID that produced the event |
| `value` | any | Event value (optional) |
| `tag` | string | Subscription tag (optional, for subscription events) |
| `modifiers` | object | Keyboard modifiers (optional) |
| `data` | object | Additional event data (optional) |
| `captured` | bool | Whether a widget consumed this event (optional, subscription events only) |

Fields that are null or absent are omitted from the serialized output.

**Event capture status.** All keyboard, mouse, touch, and IME subscription
events include an optional `captured` boolean. When `true`, an iced widget
already consumed the event (e.g. a TextEditor captured a Tab key press).
When `false` or absent, no widget handled the event. Widget-level events
(click, input, submit, etc.) never carry this field.

#### Widget events

Produced by widget interactions. The `id` field is the node ID.

| Family | Fields | Description |
|--------|--------|-------------|
| `click` | id | Button or clickable clicked |
| `input` | id, value (string) | Text input changed |
| `submit` | id, value (string) | Text input submitted (Enter) |
| `toggle` | id, value (bool) | Checkbox or toggler changed |
| `slide` | id, value (f64) | Slider moved |
| `slide_release` | id, value (f64) | Slider released |
| `select` | id, value (string) | Pick list or radio selected |
| `paste` | id, value (string) | Text pasted into input |
| `option_hovered` | id, value (string) | Combo box option hovered |
| `sensor_resize` | id, data: {width, height} | Sensor widget resized |
| `scroll` | id, data: {absolute_x, absolute_y, relative_x, relative_y, bounds_width, bounds_height, content_width, content_height} | Scrollable scrolled |
| `sort` | id, data: {column} | Table column sort clicked |
| `key_binding` | id, data | TextEditor key binding rule matched |
| `open` | id | PickList or ComboBox menu opened |
| `close` | id | PickList or ComboBox menu closed |

Mouse area events (from `mouse_area` widget):

| Family | Description |
|--------|-------------|
| `mouse_right_press` | Right button pressed |
| `mouse_right_release` | Right button released |
| `mouse_middle_press` | Middle button pressed |
| `mouse_middle_release` | Middle button released |
| `mouse_double_click` | Double click |
| `mouse_enter` | Cursor entered area |
| `mouse_exit` | Cursor left area |
| `mouse_move` | Cursor moved (data: {x, y}) |
| `mouse_scroll` | Scroll within area (data: {delta_x, delta_y}) |

Canvas events:

| Family | Fields | Description |
|--------|--------|-------------|
| `canvas_press` | id, data: {x, y, button} | Mouse pressed on canvas |
| `canvas_release` | id, data: {x, y, button} | Mouse released on canvas |
| `canvas_move` | id, data: {x, y} | Mouse moved on canvas |
| `canvas_scroll` | id, data: {x, y, delta_x, delta_y} | Scroll on canvas |

Pane grid events:

| Family | Fields | Description |
|--------|--------|-------------|
| `pane_resized` | id, data: {split, ratio} | Pane divider moved |
| `pane_dragged` | id, data: {action, pane, target, region, edge} | Pane dragged (action: picked/dropped/canceled) |
| `pane_clicked` | id, data: {pane} | Pane clicked |
| `pane_focus_cycle` | id, data: {pane} | Pane focus cycled (F6/Shift+F6) |

#### Subscription events

Produced by registered subscriptions. The `tag` field contains the
tag from the subscription registration.

**Keyboard:**

| Family | Fields |
|--------|--------|
| `key_press` | tag, data: {key, modified_key, physical_key, location, text, repeat}, modifiers |
| `key_release` | tag, data: {key, modified_key, physical_key, location}, modifiers |
| `modifiers_changed` | tag, modifiers: {shift, ctrl, alt, logo, command} |

**Mouse:**

| Family | Fields |
|--------|--------|
| `cursor_moved` | tag, data: {x, y} |
| `cursor_entered` | tag |
| `cursor_left` | tag |
| `button_pressed` | tag, value (button name) |
| `button_released` | tag, value (button name) |
| `wheel_scrolled` | tag, data: {delta_x, delta_y, unit} |

**Touch:**

| Family | Fields |
|--------|--------|
| `finger_pressed` | tag, data: {id, x, y} |
| `finger_moved` | tag, data: {id, x, y} |
| `finger_lifted` | tag, data: {id, x, y} |
| `finger_lost` | tag, data: {id, x, y} |

**IME (input method):**

| Family | Fields |
|--------|--------|
| `ime_opened` | tag |
| `ime_preedit` | tag, data: {text, cursor} |
| `ime_commit` | tag, data: {text} |
| `ime_closed` | tag |

**Window lifecycle:**

| Family | Fields |
|--------|--------|
| `window_opened` | tag, data: {window_id, position: {x, y}, width, height, scale_factor} |
| `window_closed` | tag, data: {window_id} |
| `window_close_requested` | tag, data: {window_id} |
| `window_moved` | tag, data: {window_id, x, y} |
| `window_resized` | tag, data: {window_id, width, height} |
| `window_focused` | tag, data: {window_id} |
| `window_unfocused` | tag, data: {window_id} |
| `window_rescaled` | tag, data: {window_id, scale_factor} |
| `file_hovered` | tag, data: {window_id, path} |
| `file_dropped` | tag, data: {window_id, path} |
| `files_hovered_left` | tag, data: {window_id} |

**Other:**

| Family | Fields |
|--------|--------|
| `animation_frame` | tag, data: {timestamp} |
| `theme_changed` | tag, value (light/dark) |
| `all_windows_closed` | -- (emitted when last window closes) |

**`animation_frame` timing.** In windowed mode, timestamps are
monotonic milliseconds since the first animation frame after startup
(or since the last Reset -- the epoch resets so animations restart
from zero). In headless/mock mode, `AdvanceFrame` passes timestamps
through directly and the renderer does not apply any epoch offset.

### effect_response

Response to an Effect.

```json
{
  "type": "effect_response",
  "session": "s1",
  "id": "req-1",
  "status": "ok",
  "result": { "path": "/home/user/file.txt" }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `session` | string | Session that produced this response |
| `id` | string | Matches the request id |
| `status` | string | `"ok"`, `"cancelled"`, or `"error"` |
| `result` | any | Result data (when status is ok) |
| `error` | string | Error message (when status is error) |

The `"cancelled"` status is returned when the user dismisses a dialog
without selecting (e.g. clicks Cancel on a file picker). It carries no
`result` or `error` field. Clients should treat it as a normal outcome,
not as a failure.

Window query operations (get_size, get_position, etc.) also use this
format, with the `id` set to the window_id.

### query_response

Response to a Query message.

```json
{
  "type": "query_response",
  "session": "s1",
  "id": "q1",
  "target": "find",
  "data": {"id": "btn1", "type": "button", "props": {}, "children": []}
}
```

| Field | Type | Description |
|-------|------|-------------|
| `session` | string | Session |
| `id` | string | Matches query request id |
| `target` | string | Echoes the query target |
| `data` | any | Query result (node object for find, full tree for tree, null if not found) |

### op_query_response

Response to widget op queries (`tree_hash`, `find_focused`,
`list_images`, `system_theme`, `system_info`).

```json
{
  "type": "op_query_response",
  "session": "s1",
  "kind": "find_focused",
  "tag": "focus_check",
  "data": {"focused": "input1"}
}
```

| Field | Type | Description |
|-------|------|-------------|
| `session` | string | Session |
| `kind` | string | Query kind (tree_hash, find_focused, list_images, system_theme, system_info) |
| `tag` | string | Tag from the widget op request |
| `data` | object | Query-specific result |

**Data shapes by query kind:**

| kind | data | Description |
|------|------|-------------|
| `tree_hash` | `{"hash": "sha256hex"}` | SHA-256 hash of the tree |
| `find_focused` | `{"focused": "widget_id"}` | ID of focused widget, or null |
| `list_images` | `{"handles": ["name1", ...]}` | All registered image handle names |
| `system_theme` | `"light"` or `"dark"` | Current OS theme preference |
| `system_info` | `{"cpu_brand": "...", "cpu_cores": N, "memory_total": N, "memory_used": N, "graphics_backend": "...", "graphics_adapter": "...", ...}` | System hardware info |

### interact_step

Emitted during headless iterative interact when an injected iced
event produces widget Messages.

```json
{
  "type": "interact_step",
  "session": "s1",
  "id": "i1",
  "events": [{"type": "event", "session": "s1", "family": "click", "id": "btn1"}]
}
```

| Field | Type | Description |
|-------|------|-------------|
| `type` | string | Always `"interact_step"` |
| `session` | string | Session that produced this step |
| `id` | string | Matches the interact request id |
| `events` | array | OutgoingEvent objects captured from this iced event |

The host must process the events (update model, re-render tree)
and send a Snapshot or Patch back to the renderer before the next
event is injected.

### interact_response

Final response to an Interact message, signalling the interaction
is complete.

```json
{
  "type": "interact_response",
  "session": "s1",
  "id": "i1",
  "events": []
}
```

| Field | Type | Description |
|-------|------|-------------|
| `type` | string | Always `"interact_response"` |
| `session` | string | Session that produced this response |
| `id` | string | Matches the interact request id |
| `events` | array | Empty when steps were used; contains all events for synthetic/mock actions |

In headless mode with steps, the events list is empty (all events
were delivered via prior `interact_step` messages). In mock mode
or for synthetic-only actions, no steps are emitted and all events
are in this final response.

### tree_hash_response

Response to a TreeHash message.

```json
{
  "type": "tree_hash_response",
  "session": "s1",
  "id": "th1",
  "name": "after_click",
  "hash": "a1b2c3..."
}
```

| Field | Type | Description |
|-------|------|-------------|
| `session` | string | Session |
| `id` | string | Matches request id |
| `name` | string | Echoes the capture name |
| `hash` | string | SHA-256 hex hash of the tree serialized as JSON |

### screenshot_response

Response to a Screenshot message.

```json
{
  "type": "screenshot_response",
  "session": "s1",
  "id": "sc1",
  "name": "homepage",
  "hash": "d4e5f6...",
  "width": 1024,
  "height": 768,
  "rgba": "<binary pixel data>"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `session` | string | Session |
| `id` | string | Matches request id |
| `name` | string | Echoes the capture name |
| `hash` | string | SHA-256 hex hash of RGBA data (empty string in mock mode) |
| `width` | number | Rendered width in pixels (0 in mock mode) |
| `height` | number | Rendered height in pixels (0 in mock mode) |
| `rgba` | binary | RGBA pixel data (base64 in JSON, native binary in msgpack). Absent in mock mode. |

Maximum screenshot dimension: 16384 pixels (width and height are
clamped to this limit).

### reset_response

Response to a Reset message.

```json
{
  "type": "reset_response",
  "session": "s1",
  "id": "r1",
  "status": "ok"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `session` | string | Session |
| `id` | string | Matches request id |
| `status` | string | Always `"ok"` |

---

## Execution modes

The renderer runs in one of three modes, selected by CLI flags.
Behaviour differences that affect SDK implementations:

### Windowed mode (default, `"mode": "windowed"`)

Full iced rendering with real windows. Production mode.

- All messages work as documented.
- Interact always produces synthetic events (not captured from iced).
- Subscriptions emit real events from the window system.
- Effects (file dialogs, clipboard) execute natively.

### Headless mode (`--headless`, `"mode": "headless"`)

Real rendering via tiny-skia. No display server required.

- Interact injects real iced events and captures widget output.
  May emit `interact_step` messages requiring snapshot round-trips.
- Screenshot returns real RGBA pixel data.
- Effects always return `"cancelled"` status (no platform dialogs).
- Subscriptions work (events emitted on registration match).
- Window operations are no-ops (no real windows).

**Announce events.** In headless and mock modes, `announce` widget ops
emit a synthetic event instead of dispatching to the platform
accessibility layer (which does not exist without a display server):

```json
{"type": "event", "session": "", "family": "announce", "id": "", "data": {"text": "Item saved successfully"}}
```

This allows host test suites to verify that announcements are triggered
correctly. In windowed mode, announces go directly to the platform
screen reader API and do NOT produce a wire event.

### Mock mode (`--mock`, `"mode": "mock"`)

No rendering. Protocol-only. Fastest mode for testing.

- Interact always produces synthetic events. No `interact_step`.
- Screenshot returns an empty stub (hash `""`, no rgba).
- Effects always return `"cancelled"` status.
- Subscriptions register/unregister but no events are emitted.
- Window operations and widget operations (focus, scroll) are no-ops.

---

## Transport modes

The `transport` field in the hello message reports how the renderer
is connected to the host.

### stdio (default)

The renderer reads from stdin and writes to stdout. The host spawns
plushie as a subprocess and communicates over the pipe.

### exec (`--exec <command>`)

The renderer spawns a command via the system shell (`sh -c` on Unix,
`cmd /c` on Windows) and uses its stdin/stdout as the protocol
channel. The child's stderr is forwarded to plushie's stderr with a
`[remote]` prefix.

This enables remote rendering scenarios (e.g. `--exec "ssh host plushie"`)
where the host process runs on a different machine. All modes
(windowed, headless, mock) work with `--exec`.

---

## Binary data

Fields that carry binary data (`pixels`, `data` in ImageOp;
`rgba` in screenshot responses) are encoded differently depending on
the wire format:

- **JSON**: Base64-encoded string (standard alphabet, no padding required)
- **MessagePack**: Native binary type (no encoding needed)

The renderer accepts both formats transparently via a custom
deserializer.

---

## Float handling

All floating-point values in outgoing events are sanitized before
serialization. NaN and infinity are replaced with `0.0`. This prevents
JSON serialization errors and ensures all values are valid numbers.

---

## Limits

| Limit | Value | Applies to |
|-------|-------|-----------|
| MAX_MESSAGE_SIZE | 64 MiB | Wire message size (both codecs) |
| MAX_RMPV_DEPTH | 128 | MessagePack nesting depth |
| MAX_TREE_DEPTH | 256 | Widget tree recursion depth for rendering, caching, and window search |
| MAX_FONT_BYTES | 16 MiB | Decoded font data from `load_font` widget op |
| MAX_LOADED_FONTS | 256 | Runtime font loads per process lifetime |
| MAX_IMAGES | 4096 | Image handles in the registry |
| MAX_TOTAL_IMAGE_BYTES | 1 GiB | Aggregate image data in the registry |
| MAX_DIMENSION | 16384 | Single image dimension (width or height) |
| MAX_PIXEL_BYTES | 256 MiB | Single RGBA image buffer |
| MAX_SCREENSHOT_DIMENSION | 16384 | Screenshot width or height |
| MAX_MARKDOWN_CONTENT | 1 MiB | Markdown widget content (truncated with warning) |
| MAX_TEXT_EDITOR_CONTENT | 10 MiB | Text editor initial content (truncated with warning) |
| MAX_FONT_FAMILY_LEN | 256 chars | Font family name length |
| MAX_FONT_FAMILY_CACHE | 1024 entries | Interned font family name cache |
| MAX_DASH_CACHE | 1024 entries | Interned canvas dash pattern cache |
| MAX_WINDOW_DIM | 16384 | Window width or height |

Limits protect against OOM and excessive computation from untrusted or
buggy input. Most limits return errors to the host; content limits
(markdown, text editor) truncate with warnings logged to stderr.

Image and font limits are per-process -- resetting a session does not
reset them. This prevents a misbehaving session from exhausting
process-wide resources that would affect other sessions in multiplexed
mode.

---

## Error handling

The renderer is resilient to malformed input. Errors are logged to
stderr but do not crash the process.

- **Decode errors** (malformed JSON, invalid msgpack): Message is
  skipped. No response is sent. The renderer continues reading.
- **Unknown message type**: Deserialization fails. Message skipped.
- **Missing required fields**: Deserialization fails. Message skipped.
- **Unknown widget op or window op**: Logged as warning. No response.
- **Unknown interact action**: Empty events in response.
- **Selector finds nothing**: Query returns `data: null`. Interact
  returns empty events.
- **Broken stdout pipe**: Renderer exits cleanly.
- **Protocol version mismatch**: Renderer exits on startup.

---

## Message pipelining

The host can send multiple messages without waiting for responses.
The renderer processes messages sequentially within each session,
so responses arrive in the order requests were sent.

Fire-and-forget messages (Settings, Snapshot, Patch, Subscribe,
Unsubscribe, WidgetOp, WindowOp, ImageOp, ExtensionCommand,
ExtensionCommands, AdvanceFrame) can be sent freely at any time.

Request messages (Query, Interact, TreeHash, Screenshot, Reset,
Effect) can also be pipelined -- the renderer queues them and
responds in order.

**Exception: interact steps.** During an Interact in headless mode,
the renderer may emit `interact_step` messages. When this happens,
the host **must** send a Snapshot or Patch back before the renderer
will continue to the next iced event. Do not send other messages
to the same session between an `interact_step` and the
corresponding Snapshot -- the renderer is blocked waiting for the
tree update.

---

## Accessibility props

Any tree node can carry an `a11y` object in its `props` to control
accessibility behaviour. All fields are optional.

```json
{
  "a11y": {
    "role": "button",
    "label": "Submit form",
    "description": "Sends the form to the server",
    "hidden": false,
    "expanded": true,
    "required": false,
    "level": 2,
    "live": "polite",
    "busy": false,
    "invalid": false,
    "modal": false,
    "read_only": false,
    "mnemonic": "S",
    "toggled": null,
    "selected": null,
    "value": "current value",
    "orientation": "horizontal",
    "labelled_by": "label-node-id",
    "described_by": "desc-node-id",
    "error_message": "error-node-id",
    "disabled": false,
    "position_in_set": 3,
    "size_of_set": 10,
    "has_popup": "menu"
  }
}
```

| Field | Type | Description |
|-------|------|-------------|
| `role` | string | Accessible role (e.g. `"button"`, `"text_input"`, `"image"`) |
| `label` | string | Primary accessible label |
| `description` | string | Extended description |
| `hidden` | bool | Hide from assistive technology |
| `expanded` | bool | Expanded/collapsed state |
| `required` | bool | Required field indicator |
| `level` | number | Heading level (1-6) |
| `live` | string | Live region: `"off"`, `"polite"`, `"assertive"` |
| `busy` | bool | Content is loading/updating |
| `invalid` | bool | Validation failed |
| `modal` | bool | Modal container |
| `read_only` | bool | Read-only field |
| `mnemonic` | string | Keyboard mnemonic (single character) |
| `toggled` | bool | Toggle state |
| `selected` | bool | Selection state |
| `value` | string | Text value announced by AT |
| `orientation` | string | `"horizontal"` or `"vertical"` |
| `labelled_by` | string | Node ID of the labelling element |
| `described_by` | string | Node ID of the describing element |
| `error_message` | string | Node ID of the error message element |
| `disabled` | bool | Widget is disabled (not interactive) |
| `position_in_set` | number | Position in set (1-based, for list items, radios, tabs) |
| `size_of_set` | number | Total items in set (paired with `position_in_set`) |
| `has_popup` | string | Popup type: `"listbox"`, `"menu"`, `"dialog"`, `"tree"`, `"grid"` |

**Auto-inference:** Image and SVG widgets with an `alt` prop auto-populate
`label` from the alt text. Text input and text editor widgets auto-populate
`description` from their `placeholder` prop. Explicit `a11y` values always
take priority.

### Widget-specific accessibility props

Some widgets accept top-level props (outside the `a11y` object) that feed
into accessibility output.

| Widget | Prop | Type | Description |
|--------|------|------|-------------|
| `image` | `alt` | string | Accessible label (auto-populates `a11y.label`) |
| `image` | `description` | string | Extended accessible description |
| `image` | `decorative` | bool | When true, hides the image from assistive technology. Use for purely visual images that don't convey information. |
| `svg` | `alt` | string | Accessible label (auto-populates `a11y.label`) |
| `svg` | `description` | string | Extended accessible description |
| `svg` | `decorative` | bool | When true, hides the SVG from assistive technology. Use for purely visual images that don't convey information. |
| `slider` | `label` | string | Accessible label (e.g. `"Volume"`). Without this, screen readers announce the value without context. |
| `vertical_slider` | `label` | string | Accessible label (e.g. `"Volume"`). Without this, screen readers announce the value without context. |
| `progress_bar` | `label` | string | Accessible label (e.g. `"Upload progress"`). |
| `qr_code` | `alt` | string | Accessible label for the QR code |
| `qr_code` | `description` | string | Extended description |
| `canvas` | `alt` | string | Accessible label for the canvas |
| `canvas` | `description` | string | Extended description |

---

## Extended styling props

Beyond the standard `style` prop (which accepts a preset name or a
StyleMap object), several widgets support additional colour and sizing
props.

| Widget | Prop | Type | Description |
|--------|------|------|-------------|
| `text` | `ellipsis` | string | Text overflow: `"none"`, `"start"`, `"middle"`, `"end"` |
| `rich_text` | `wrapping` | string | Text wrapping mode |
| `rich_text` | `ellipsis` | string | Text overflow: `"none"`, `"start"`, `"middle"`, `"end"` |
| `text_input` | `placeholder_color` | hex color | Placeholder text colour |
| `text_input` | `selection_color` | hex color | Text selection highlight |
| `text_input` | `ime_purpose` | string | IME hint: `"normal"`, `"secure"`, `"terminal"` |
| `text_editor` | `placeholder_color` | hex color | Placeholder text colour |
| `text_editor` | `selection_color` | hex color | Text selection highlight |
| `text_editor` | `ime_purpose` | string | IME hint: `"normal"`, `"secure"`, `"terminal"` |
| `slider` | `rail_color` | hex color | Track rail colour |
| `slider` | `rail_width` | number | Track rail thickness |
| `vertical_slider` | `rail_color` | hex color | Track rail colour |
| `vertical_slider` | `rail_width` | number | Track rail thickness |
| `scrollable` | `scrollbar_color` | hex color | Scrollbar track colour |
| `scrollable` | `scroller_color` | hex color | Scroller handle colour |
| `pick_list` | `ellipsis` | string | Text overflow for selected value |
| `pick_list` | `menu_style` | object | StyleMap overrides for the dropdown menu |
| `combo_box` | `ellipsis` | string | Text overflow for selected value |
| `combo_box` | `menu_style` | object | StyleMap overrides for the dropdown menu |
| `combo_box` | `shaping` | string | Text shaping (`"basic"` or `"advanced"`) |
| `grid` | `fluid` | number | Max cell width for fluid auto-wrapping columns |
| `table` | `header_text_size` | number | Header row text size |
| `table` | `row_text_size` | number | Body row text size |
| `pane_grid` | `divider_color` | hex color | Pane divider colour |
| `pane_grid` | `divider_width` | number | Pane divider thickness |
| `markdown` | `link_color` | hex color | Hyperlink colour |
| `markdown` | `code_theme` | string | Syntax highlighting theme for code blocks |

**StyleMap `base` field.** A StyleMap object can include a `"base"` field
naming a preset to extend. The style starts from the preset's defaults,
then remaining fields override individual properties:

```json
{
  "style": {
    "base": "secondary",
    "background": "#ff0000"
  }
}
```

---

## Debugging

### PLUSHIE_NO_CATCH_UNWIND

Set `PLUSHIE_NO_CATCH_UNWIND=1` on the renderer process to disable
panic isolation for widget extensions. When set, extension panics
propagate normally instead of being caught by `catch_unwind`, which
preserves full stack traces and allows debuggers (gdb, lldb) to
break at the panic site.

**Use this only during development.** In production, `catch_unwind`
prevents one misbehaving extension from crashing the entire renderer.
With the env var set, any extension panic takes down the process.

---

## Event throttling

The renderer supports host-controlled rate limiting of high-frequency
events. Without configuration, all events are delivered at full speed
(backward compatible). The host opts in by setting rates at one or
more levels.

### Rate hierarchy

For a given event, the effective rate is determined by (highest
priority first):

1. Per-widget `event_rate` prop
2. Per-subscription `max_rate` field on Subscribe
3. Global `default_event_rate` from Settings
4. No limit (full speed) if none of the above are set

### Coalescable events

Only high-frequency events are eligible for rate limiting. The
renderer classifies events into two coalescing strategies:

**Replace (latest value wins):** `cursor_moved`, `finger_moved`,
`modifiers_changed`, `animation_frame`, `theme_changed`, `slide`,
`mouse_area_move`, `canvas_move`, `sensor_resize`, `pane_resized`.

**Accumulate (deltas sum):** `wheel_scrolled`, `mouse_area_scroll`,
`canvas_scroll`.

**Never coalesced:** `click`, `input`, `submit`, `toggle`, `select`,
`paste`, `key_press`, `key_release`, `button_pressed`,
`button_released`, `cursor_entered`, `cursor_left`, `slide_release`,
and all window lifecycle events. These are always delivered
immediately regardless of rate settings.

### Widget `event_rate` prop

Any widget node can include an `event_rate` prop (integer, events per
second) to rate-limit its coalescable events:

```json
{
  "id": "volume",
  "type": "slider",
  "props": { "range": [0, 100], "value": 50, "event_rate": 30 }
}
```

Different widgets of the same type can have different rates. The prop
is accepted on all widget types (it is a universal prop like `a11y`).

### Ordering guarantees

Non-coalescable events flush the coalesce buffer before emitting.
This preserves ordering: the host always sees the latest coalesced
state (e.g. mouse position) before the discrete event (e.g. click)
that follows.

Incoming stdin messages also flush the buffer, providing adaptive
throughput matching -- when the host sends a message, it gets all
pending coalesced events immediately.

### Extension events

Extension widget events participate in rate limiting and coalescing
when they carry a `CoalesceHint`. Extension authors set hints on
outgoing events via `.with_coalesce(CoalesceHint::Replace)` or
`.with_coalesce(CoalesceHint::Accumulate(vec!["field_x".into(), "field_y".into()]))`.
`Replace` uses the standard latest-value-wins strategy.
`Accumulate(fields)` sums the named data fields across coalesced
events; other fields keep the latest values. Events returned without
a hint are never coalesced -- they are delivered immediately
regardless of any `event_rate` setting.

### Headless and mock modes

Rate limiting configuration is accepted and stored but not applied
in headless or mock modes. Events in those modes are driven by the
scripting protocol at the host's pace, so rate limits would make
test timing unpredictable.

---

## Performance considerations

The renderer is designed for moderate UI complexity (hundreds to low
thousands of nodes). Practical considerations for large trees:

- **Snapshot vs Patch.** For trees with many nodes, prefer Patch for
  incremental updates rather than replacing the entire tree with
  Snapshot on every change. Snapshot triggers a full tree walk for
  extension prepare, cache invalidation, and window reconciliation.
- **Tree depth.** Rendering and tree search are bounded to 256 levels
  of nesting. Deeply nested trees also increase layout cost.
- **Canvas caching.** Canvas widgets cache per-layer tessellation.
  Avoid unnecessary shape changes -- only layers whose content hash
  changes are re-tessellated.
- **Screenshot size.** Large screenshots allocate proportional RGBA
  buffers (width * height * 4 bytes). The maximum dimension is
  16384 px per axis.

---

## Overlay widget

The `overlay` widget positions a popup element relative to an anchor
sibling. It takes exactly two children: the first is the anchor
(rendered normally in the layout), the second is the overlay content
(rendered as an iced overlay above all other content).

```json
{
  "id": "dropdown-1",
  "type": "overlay",
  "props": {
    "position": "below",
    "gap": 4,
    "offset_x": 0,
    "offset_y": 0,
    "flip": true,
    "align": "start"
  },
  "children": [
    {"id": "anchor", "type": "button", "props": {"label": "Open"}, "children": []},
    {"id": "menu", "type": "column", "props": {}, "children": [...]}
  ]
}
```

| Prop | Type | Default | Description |
|------|------|---------|-------------|
| `position` | string | `"below"` | Overlay placement: `"below"`, `"above"`, `"left"`, `"right"` |
| `gap` | number | 0 | Pixel gap between anchor and overlay content |
| `offset_x` | number | 0 | Horizontal pixel offset (applied after positioning) |
| `offset_y` | number | 0 | Vertical pixel offset (applied after positioning) |
| `flip` | bool | false | Auto-flip position when content overflows the viewport in the primary direction. Flips Below<->Above or Left<->Right. The flip only occurs if the opposite side has enough space; otherwise the original position is kept and viewport clamping applies. |
| `align` | string | `"center"` | Cross-axis alignment: `"start"`, `"center"`, `"end"`. For Below/Above, controls horizontal alignment (start = left-aligned with anchor edge, center = centered on anchor, end = right-aligned with anchor edge). For Left/Right, controls vertical alignment. |

**Focus and accessibility.** Both children participate in focus cycling
(Tab/Shift+Tab) and the accessibility tree. Setting `a11y.modal = true`
on the overlay node signals a modal popup, but focus trapping is the
host SDK's responsibility -- plushie does not intercept focus navigation
at the iced level.

---

## Interactive canvas elements

Groups within canvas layers can be made interactive by adding an `id`
field. An interactive group (called an "element" in the event
vocabulary) responds to pointer events, keyboard navigation, drag
gestures, tooltips, and participates in the accessibility tree.

Only groups can be interactive. Leaf shapes (rect, circle, line, path,
text, image, svg) are never interactive on their own -- wrap them in a
group to add interaction.

### Terminology

- **Shape**: a leaf drawing primitive (rect, circle, line, path, text,
  image, svg). Pure visual, no interactivity.
- **Group**: the only container type. Carries transforms, clips, and
  optionally interactivity (when it has an `id` field).
- **Element**: an interactive group (one with an `id`). The term used
  in event names (`canvas_element_click`), commands (`focus_element`),
  and test actions.

### Group wire format

A group with an `id` field is an interactive element. All interactive
properties live at the group's top level (no nested `interactive`
sub-object).

```json
{
  "type": "group",
  "children": [...],

  "transforms": [
    {"type": "translate", "x": 50, "y": 30},
    {"type": "rotate", "angle": 0.785},
    {"type": "scale", "x": 2.0, "y": 2.0}
  ],
  "clip": {"x": 0, "y": 0, "w": 200, "h": 200},

  "id": "star-0",
  "on_click": true,
  "on_hover": true,
  "cursor": "pointer",
  "tooltip": "1 star",
  "hit_rect": {"x": 0, "y": 0, "w": 40, "h": 40},

  "hover_style": {"fill": "#ddd"},
  "pressed_style": {"fill": "#bbb"},
  "focus_style": {"stroke": "#3b82f6", "stroke_width": 2},
  "show_focus_ring": true,

  "draggable": false,
  "drag_axis": "both",
  "drag_bounds": {"min_x": 0, "max_x": 100, "min_y": 0, "max_y": 100},

  "a11y": {"role": "radio", "label": "1 star", "selected": true,
           "position_in_set": 1, "size_of_set": 5},

  "focusable": false
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `type` | `"group"` | Yes | Must be `"group"` |
| `children` | array | Yes | Child shapes drawn in local coordinates |
| `transforms` | array | No | Ordered list of transforms: `translate`, `rotate`, `scale` |
| `clip` | object | No | Clip rectangle `{x, y, w, h}` in local coordinates |
| `id` | string | No* | Unique ID. Presence makes the group interactive. |
| `on_click` | bool | No | Emit `canvas_element_click` events |
| `on_hover` | bool | No | Emit `canvas_element_enter` / `canvas_element_leave` events |
| `cursor` | string | No | Cursor on hover (`pointer`, `grab`, `crosshair`, `move`, `text`) |
| `tooltip` | string | No | Tooltip text on hover |
| `hit_rect` | object | No | Explicit hit region `{x, y, w, h}` in local coords |
| `hover_style` | object | No | Style overrides while hovered |
| `pressed_style` | object | No | Style overrides while pressed |
| `focus_style` | object | No | Style overrides while keyboard-focused |
| `show_focus_ring` | bool | No | Show default focus ring (default: `true`) |
| `draggable` | bool | No | Enable drag interaction |
| `drag_axis` | string | No | `"both"` (default), `"x"`, `"y"` |
| `drag_bounds` | object | No | `{min_x, max_x, min_y, max_y}` |
| `a11y` | object | No | Accessibility overrides (see Accessibility below) |
| `focusable` | bool | No | Make this group a Tab stop for two-level navigation |

\* Groups without `id` are non-interactive (pure structural containers
for transforms, clips, and visual grouping).

**Transforms** are applied in array order. Each entry has a `type` and
type-specific fields:

| Transform | Fields | Description |
|-----------|--------|-------------|
| `translate` | `x`, `y` | Translate by (x, y) |
| `rotate` | `angle` | Rotate by angle in radians |
| `scale` | `x`, `y` or `factor` | Non-uniform or uniform scale |

Groups without transforms or clips are pure nesting containers with
no rendering overhead.

### Canvas widget props

| Prop | Type | Default | Description |
|------|------|---------|-------------|
| `role` | string | `"group"` if interactive elements exist, `"image"` otherwise | Accessible role for the canvas widget |
| `arrow_mode` | string | `"wrap"` | Arrow key behavior: `"wrap"`, `"clamp"`, `"linear"`, `"none"` |
| `alt` | string | - | Accessible label |
| `description` | string | - | Extended accessible description |

### Hit testing

Hit regions are computed in the group's **local** coordinate space.
The renderer accumulates a 2D affine transform matrix from all
ancestor groups and uses the inverse matrix to transform cursor
positions from canvas space to local space for testing.

| Children | Hit test method |
|----------|----------------|
| `rect` | Point-in-rect |
| `circle` | Distance from center <= radius |
| `line` | Distance to line segment (min 2px half-width) |
| `path` | Bounding box of command coordinates |
| `text` | Estimated bounds from position + content |
| `group` | Union bounding box of children |

Groups are tested in reverse draw order (topmost first). Explicit
`hit_rect` overrides automatic inference. `hit_rect` is in local
coordinates.

Clip regions from ancestor groups are intersected and tested in
canvas space before the hit region test. Clicks outside the
accumulated clip are ignored.

A 0.5px epsilon is applied to boundary comparisons for floating-point
precision at transformed element boundaries.

### Events emitted

Interactive elements emit the following event families. The top-level
`id` field is the canvas widget ID; element identity is in `data`.

| Family | Data | Coalescable | Description |
|--------|------|-------------|-------------|
| `canvas_element_enter` | `element_id`, `x`, `y` | No | Cursor entered hit region |
| `canvas_element_leave` | `element_id` | No | Cursor left hit region |
| `canvas_element_click` | `element_id`, `x`, `y`, `button` | No | Activated (click or keyboard). `button`: `"left"`, `"right"`, `"keyboard"` |
| `canvas_element_drag` | `element_id`, `x`, `y`, `delta_x`, `delta_y` | Replace | Drag movement |
| `canvas_element_drag_end` | `element_id`, `x`, `y` | No | Drag released |
| `canvas_element_focused` | `element_id` | No | Element gained keyboard focus |
| `canvas_element_blurred` | `element_id` | No | Element lost keyboard focus |
| `canvas_focused` | - | No | Canvas widget gained iced-level focus |
| `canvas_blurred` | - | No | Canvas widget lost iced-level focus |
| `canvas_group_focused` | `group_id` | No | Focusable group entered |
| `canvas_group_blurred` | `group_id` | No | Focusable group exited |
| `diagnostic` | `level`, `element_id`, `code`, `message` | Deduplicate | Validation warning |

**Event ordering guarantees:**

- Click on element: `canvas_focused` (if new) -> `canvas_element_blurred` (old) -> `canvas_element_focused` (new) -> `canvas_element_click`
- Tab to next: `canvas_element_blurred` (old) -> `canvas_element_focused` (new)
- Tab out: `canvas_element_blurred` -> `canvas_blurred`
- Tab in: `canvas_focused` -> `canvas_element_focused`

Raw canvas events (`canvas_press`, `canvas_release`, `canvas_move`,
`canvas_scroll`) continue to fire alongside element events.

### Keyboard navigation

A canvas with interactive elements is a single Tab stop. Once focused,
internal keyboard navigation uses the roving tabindex pattern.

| Key | Action |
|-----|--------|
| Tab | Enter canvas (first element) / advance to next top-level entry / exit canvas |
| Shift+Tab | Enter canvas (last element) / move to previous top-level entry / exit canvas |
| Arrow Down/Right | Next element within current scope (respects `arrow_mode`) |
| Arrow Up/Left | Previous element within current scope |
| Home | First element in scope |
| End | Last element in scope |
| Page Down/Up | Jump by 10 within scope |
| Enter / Space | Activate focused element (`canvas_element_click` with `button: "keyboard"`) |
| Escape | Exit focusable group / clear focus / unfocus canvas (three levels) |

**Arrow mode** (`arrow_mode` canvas prop):

| Mode | Boundary behavior |
|------|-------------------|
| `wrap` (default) | Wraps last->first and first->last. Always captures. |
| `clamp` | Stops at first/last. Captures. |
| `linear` | Propagates at boundaries (e.g. scrollable parent handles it). |
| `none` | Arrows not handled. Tab-only navigation. |

**Focusable groups** (`focusable: true` on a group): Tab moves between
top-level entries (standalone elements + focusable groups). When Tab
lands on a focusable group, it auto-enters and focuses the first (or
last for Shift+Tab) child. Arrows navigate within the group's children.
Escape exits the group. Canvases without focusable groups use flat-list
navigation (backward compatible).

Click-to-focus: clicking an interactive element grants the canvas
iced-level focus and sets internal focus to the clicked element.
Clicking empty canvas area clears internal focus.

### Widget operations

| Op | Payload | Description |
|----|---------|-------------|
| `focus_element` | `target` (canvas ID), `element_id` | Focus the canvas and set internal focus to the specified element |

### Test interact actions

| Action | Payload | Description |
|--------|---------|-------------|
| `click_element` | `x`, `y` | Synthesize click at coordinates |
| `focus_element` | - | Synthesize Tab to enter canvas |

### Style overrides

`hover_style`, `pressed_style`, and `focus_style` on the group are
merged into each child shape's JSON during draw. Any shape property
can be overridden (`fill`, `stroke`, `stroke_width`, `opacity`, etc.).

Priority (highest wins): `pressed_style` > `hover_style` > `focus_style`.

Children can also declare their own per-child overrides at the top
level of the child shape JSON; these take precedence over the group's
style.

Layers with an active style override bypass the geometry cache.

### Accessibility

The canvas widget's accessible role defaults to `Group` when
interactive elements exist, `Image` otherwise. Override with the
`role` prop.

Interactive elements with `a11y` appear as child nodes in the
accessibility tree. `active_descendant` on the canvas node
dynamically tracks the focused element.

Focusable groups create parent-child relationships in the a11y tree
via `traverse()` blocks.

The renderer emits validation diagnostics (as log warnings) for:
- Interactive element without `a11y` metadata
- `switch` role without `toggled`, `radio` without `selected`,
  `check_box` without `toggled`
- Multiple elements without `position_in_set`/`size_of_set`

### Focus ring

The focus ring adapts to the element's hit region geometry:
- **Rect**: rounded rectangle
- **Circle**: circle
- **Line**: capsule (stadium shape) oriented along the line

The ring respects the element's full accumulated transform (translate,
rotate, scale). Suppressed when `show_focus_ring: false` is set (use
`focus_style` for custom focus indicators instead).

### Tooltips

When the cursor hovers over an element with a `tooltip` field, the
text is drawn as an overlay within the canvas.

### Known limitations

- Keyboard-driven drag not yet supported (drag operations have no
  keyboard equivalent).
- Nested focusable groups (focusable inside focusable) are not fully
  supported -- the inner group appears as an arrow-navigable child but
  cannot be "drilled into" via keyboard.
- Role-specific keyboard patterns (slider value adjustment, tree
  expand/collapse) are not implemented.
- The overlay widget auto-flips when `flip: true` is set, but only
  along the primary axis.
