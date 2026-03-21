# Building for WASM

toddy compiles to WebAssembly via the `toddy-wasm` crate. The WASM
module runs the full iced renderer in the browser (or any WASM host)
and communicates with the host via JavaScript callbacks.

## Prerequisites

```bash
rustup target add wasm32-unknown-unknown
cargo install wasm-pack
```

## Quick build

```bash
wasm-pack build toddy-wasm --target web
```

Output lands in `toddy-wasm/pkg/`:

```
toddy-wasm/pkg/
  toddy_wasm.js          # JS glue code (ESM)
  toddy_wasm.d.ts        # TypeScript declarations
  toddy_wasm_bg.wasm     # WASM binary
  toddy_wasm_bg.wasm.d.ts
  package.json
```

Other targets: `--target nodejs` for Node.js, `--target bundler`
for webpack/vite. The `web` target works without a bundler.

## JavaScript API

```typescript
import init, { ToddyApp } from './toddy_wasm.js';

await init();

const app = new ToddyApp(settingsJson, (event: string) => {
    const parsed = JSON.parse(event);
    console.log(parsed.type, parsed);
});

// Send protocol messages (Snapshot, Patch, Subscribe, etc.)
app.send_message(JSON.stringify({
    type: "snapshot",
    tree: { type: "window", id: "main", children: [...] }
}));
```

The constructor validates the protocol version, emits the hello
handshake, and starts the iced daemon in the background. Messages
sent via `send_message()` are processed on the next event loop tick.

## Custom builds with extensions

Extensions are Rust code compiled into the WASM binary. Create a
crate that depends on `toddy-wasm` and registers extensions:

```rust
use toddy_wasm::ToddyApp;
use toddy_core::app::ToddyAppBuilder;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub fn create_app(
    settings: &str,
    on_event: js_sys::Function,
) -> Result<ToddyApp, JsValue> {
    let mut builder = ToddyAppBuilder::new();
    builder.register(Box::new(MyCustomWidget));
    ToddyApp::with_extensions(settings, on_event, builder)
}
```

## Size optimization

The default `wasm-pack build` produces a ~10 MB WASM binary (~4.1 MB
gzipped). This includes the full iced renderer, text shaping
(cosmic-text), canvas, markdown, syntax highlighting, image/SVG
decoding, and accessibility.

### Profile settings

Uncomment the release profile in the workspace `Cargo.toml`:

```toml
[profile.release]
lto = true
codegen-units = 1
opt-level = "z"
strip = true
```

These increase compile times (~3-5x) but apply to both native and
WASM targets.

### wasm-opt post-processing

wasm-pack runs wasm-opt automatically, but the bundled version may
not support bulk memory operations emitted by Rust 1.82+. If
wasm-pack fails at the optimization step, run wasm-opt manually:

```bash
# Build without wasm-opt
cargo build --target wasm32-unknown-unknown --release -p toddy-wasm

# Run wasm-opt manually with all WASM features enabled
wasm-opt target/wasm32-unknown-unknown/release/toddy_wasm.wasm \
    -Oz --all-features \
    -o toddy-wasm/pkg/toddy_wasm_bg.wasm
```

Install a recent wasm-opt via `npm install -g binaryen` or your
system package manager if the wasm-pack bundled version is too old.

### Size comparison

Measured with Rust 1.92, toddy-iced 0.6, wasm-opt from binaryen:

| | Default | Profile opts | + wasm-opt -Oz |
|---|---|---|---|
| Raw | 10.0 MB | 10.0 MB | 8.1 MB |
| Gzip | 4.1 MB | 3.6 MB | 3.5 MB |
| Brotli | -- | -- | 2.7 MB |

Most CDNs and browsers support Brotli, so 2.7 MB is the effective
transfer size for production deployments.

### What contributes to binary size

The largest contributors (approximate, based on feature analysis):

- **iced renderer** (wgpu shaders, layout, text) -- unavoidable core
- **cosmic-text** (text shaping, fontdb) -- unavoidable for text
- **markdown + highlighter** -- pulldown-cmark, syntect, themes
- **image** -- PNG/JPEG/etc. decoding
- **svg** -- resvg/usvg vector rendering
- **canvas** -- 2D drawing, hit testing, tessellation

Feature-gating `markdown`, `highlighter`, `image`, and `svg` in
toddy-core would let WASM builds exclude unused capabilities. This
is not yet implemented but would be the next meaningful size
reduction (estimated 20-30% for a minimal build).

## Known issues

**wasm-opt compatibility.** Rust 1.82+ emits `memory.copy` (bulk
memory operations) which older wasm-opt versions reject. Use
`--all-features` flag or upgrade binaryen. wasm-pack's bundled
wasm-opt may lag behind.

**Effects.** File dialogs, clipboard, and notifications are stubbed
as unsupported. Web API implementations (Clipboard API, File System
Access API, Notification API) can be added to `WebEffectHandler`.

**Fonts.** File path fonts in the Settings `fonts` array are not
supported on WASM (no filesystem). Use inline font data
(`{"data": "base64..."}` objects) or the `load_font` widget op
with base64-encoded bytes.
