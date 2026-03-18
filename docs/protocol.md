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
  "name": "julep"
}
```

The host should check that `protocol` matches the version it expects.
The `session` field on `hello` is always empty -- it is a
process-level message, not scoped to any session.

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

**Startup-only fields** (ignored if sent after the first Settings):
`antialiasing`, `vsync`, `fonts`, `scale_factor`, `validate_props`.

**Runtime fields** (can be updated by sending Settings again):
`default_text_size`, `default_font`, `extension_config`.

### Snapshot

Replace the entire tree. The simplest way to update the UI -- no
diffing required on the host side.

```json
{
  "type": "snapshot",
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

### Patch

Incrementally update the existing tree. More efficient than Snapshot
for large trees with small changes.

```json
{
  "type": "patch",
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

### SubscriptionRegister

Subscribe to a category of events. The `tag` is included in events of
this kind so the host can route them.

```json
{
  "type": "subscription_register",
  "kind": "on_key_press",
  "tag": "my_key_handler"
}
```

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

### SubscriptionUnregister

Remove a subscription.

```json
{
  "type": "subscription_unregister",
  "kind": "on_key_press"
}
```

### WidgetOp

Perform an operation on a widget (focus, scroll, etc.).

```json
{
  "type": "widget_op",
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
| `tree_hash` | `tag` (optional) | Compute SHA-256 hash of current tree; response via query |
| `load_font` | `data` (base64 TTF/OTF) | Load a font at runtime |
| `list_images` | `tag` (optional) | List all image handle names; response via query |
| `clear_images` | -- | Remove all in-memory image handles |

### WindowOp

Manage windows directly (outside of tree-driven sync).

```json
{
  "type": "window_op",
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

**Query operations** (response sent as effect_response):

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
| `get_system_theme` | system theme (light/dark) |
| `get_system_info` | CPU, memory, GPU info |

Query operations accept an optional `request_id` field in settings,
echoed back in the response for correlation.

### EffectRequest

Request a platform effect (file dialog, clipboard, notification).

```json
{
  "type": "effect_request",
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
  "op": "create_image",
  "handle": "sprite-1",
  "data": "<base64-encoded PNG/JPEG bytes>"
}
```

Or with raw RGBA pixels:

```json
{
  "type": "image_op",
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
  "node_id": "chart-1",
  "op": "append_data",
  "payload": { "values": [1.0, 2.5, 3.7] }
}
```

### ExtensionCommandBatch

Send multiple extension commands in a single message.

```json
{
  "type": "extension_command_batch",
  "commands": [
    { "node_id": "chart-1", "op": "append_data", "payload": {...} },
    { "node_id": "chart-2", "op": "clear", "payload": {} }
  ]
}
```

---

## Outgoing messages (renderer -> host)

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
| `canvas_scroll` | id, data: {cursor_x, cursor_y, delta_x, delta_y} | Scroll on canvas |

Pane grid events:

| Family | Fields | Description |
|--------|--------|-------------|
| `pane_resized` | id, data: {split, ratio} | Pane divider moved |
| `pane_dragged` | id, data: {pane, target} | Pane dragged |
| `pane_clicked` | id, data: {pane} | Pane clicked |

#### Subscription events

Produced by registered subscriptions. The `tag` field contains the
tag from the subscription registration.

**Keyboard:**

| Family | Fields |
|--------|--------|
| `key_press` | tag, data: {key, modified_key, physical_key, location, text, repeat}, modifiers |
| `key_release` | tag, data: {key, modified_key, physical_key, location}, modifiers |
| `modifiers_changed` | tag, modifiers: {shift, ctrl, alt, logo} |

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

### effect_response

Response to an EffectRequest.

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

## Scripting messages

The following message types are available in both daemon and
headless modes for programmatic inspection and interaction:

- **Query** -- Inspect the tree or find widgets
- **Interact** -- Simulate user interactions (click, type, etc.)
- **SnapshotCapture** -- Capture a structural tree hash
- **ScreenshotCapture** -- Capture rendered pixels
- **Reset** -- Reset all state

These are commonly used for integration testing but are always
available as part of the standard protocol.

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
    "error_message": "error-node-id"
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

**Auto-inference:** Image and SVG widgets with an `alt` prop auto-populate
`label` from the alt text. Text input and text editor widgets auto-populate
`description` from their `placeholder` prop. Explicit `a11y` values always
take priority.

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
