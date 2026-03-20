//! Widget extension system.
//!
//! Extensions let Rust crates add custom widget types to the toddy
//! renderer. Each extension implements [`WidgetExtension`] and is
//! registered at startup via [`ToddyAppBuilder`](crate::app::ToddyAppBuilder).
//! The [`ExtensionDispatcher`] routes incoming messages and render
//! calls to the correct extension based on node type names.
//!
//! State is managed through [`ExtensionCaches`], a type-erased
//! key-value store namespaced by extension. Mutation happens in
//! `prepare()` / `handle_event()` / `handle_command()` (mutable
//! phase), reads happen in `render()` (immutable phase), matching
//! iced's `update()`/`view()` split.

use std::any::Any;
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU32, Ordering};

use iced::{Element, Theme};
use serde_json::Value;

use crate::image_registry::ImageRegistry;
use crate::message::Message;
use crate::protocol::{OutgoingEvent, TreeNode};
use crate::widgets::WidgetCaches;

/// Check if panic isolation is disabled via the TODDY_NO_CATCH_UNWIND env var.
/// When true, extension panics propagate normally, preserving stack traces for
/// debugging. Only use during development -- in production, catch_unwind
/// prevents one extension from crashing the entire renderer.
pub(crate) fn catch_unwind_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("TODDY_NO_CATCH_UNWIND").is_err())
}

// ---------------------------------------------------------------------------
// WidgetExtension trait
// ---------------------------------------------------------------------------

/// Trait for native Rust widget extensions.
///
/// Extensions handle custom node types that the built-in renderer doesn't
/// know about. The trait scales from trivial render-only widgets (implement
/// `type_names`, `config_key`, `render`) to full custom iced widgets with
/// autonomous state (implement all methods).
///
/// # Lifecycle
///
/// Methods are called in this order:
///
/// 1. **Registration** -- `type_names()` and `config_key()` are queried once
///    at startup to build the dispatch index. `config_key()` must be unique
///    and must not contain `':'` (reserved as the cache namespace separator).
///
/// 2. **`init(config)`** -- called when a Settings message arrives from the
///    host. Receives the value from `extension_config[config_key]`, or
///    `Value::Null` if absent. Called before any `prepare()`.
///
/// 3. **`prepare(node, caches, theme)`** -- called in the mutable phase
///    (during `update()`) after every tree change (Snapshot or Patch), for
///    each node whose type matches this extension. Use this to create or
///    update per-node state in `ExtensionCaches`. Guaranteed to run before
///    `render()` for the same tree state.
///
/// 4. **`render(node, env)`** -- called in the immutable phase (`view()`)
///    to produce an iced `Element`. Receives read-only access to caches
///    via `WidgetEnv`. May be called multiple times per frame. Must not
///    block or perform I/O.
///
/// 5. **`handle_event(node_id, family, data, caches)`** -- called when a
///    widget event is emitted for a node owned by this extension. Return
///    `EventResult::PassThrough` to forward the event to the host,
///    `Consumed(events)` to suppress it, or `Observed(events)` to forward
///    the original AND emit additional events.
///
/// 6. **`handle_command(node_id, op, payload, caches)`** -- called when the
///    host sends an `ExtensionCommand` targeting a node owned by this
///    extension. Return any events to emit back to the host.
///
/// 7. **`cleanup(node_id, caches)`** -- called when a node is removed from
///    the tree (detected during `prepare_all()`). Use this to release
///    per-node resources from `ExtensionCaches`. Not called on process
///    exit or panic.
///
/// # Panic isolation
///
/// All mutable methods (`init`, `prepare`, `handle_event`,
/// `handle_command`, `cleanup`) are wrapped in `catch_unwind`. A panic
/// poisons the extension -- subsequent calls are skipped and a red
/// placeholder is rendered. Three consecutive `render()` panics also
/// trigger poisoning. Poison state is cleared on the next Snapshot.
///
/// # Cache access
///
/// `prepare()`, `handle_event()`, `handle_command()`, and `cleanup()`
/// receive `&mut ExtensionCaches` for read-write access. `render()`
/// receives read-only access via `WidgetEnv.caches`. This split matches
/// iced's `update()`/`view()` separation -- mutation happens in `update`,
/// reads in `view`.
///
/// # Prop helpers
///
/// The prelude re-exports typed prop extraction functions from
/// [`crate::prop_helpers`] for reading values from `TreeNode.props`:
///
/// - `prop_str(node, "key") -> Option<String>`
/// - `prop_f32(node, "key") -> Option<f32>`
/// - `prop_f64(node, "key") -> Option<f64>`
/// - `prop_i32(node, "key") -> Option<i32>`
/// - `prop_i64(node, "key") -> Option<i64>`
/// - `prop_u32(node, "key") -> Option<u32>`
/// - `prop_u64(node, "key") -> Option<u64>`
/// - `prop_usize(node, "key") -> Option<usize>`
/// - `prop_bool(node, "key") -> Option<bool>`
/// - `prop_bool_default(node, "key", default) -> bool`
/// - `prop_length(node, "key", default) -> Length`
/// - `prop_color(node, "key") -> Option<Color>` (parses `#RRGGBB` / `#RRGGBBAA`)
/// - `prop_str_array(node, "key") -> Option<Vec<String>>`
/// - `prop_f32_array(node, "key") -> Option<Vec<f32>>`
/// - `prop_f64_array(node, "key") -> Option<Vec<f64>>`
/// - `prop_range_f32(node) -> RangeInclusive<f32>` (reads `"range"` prop)
/// - `prop_range_f64(node) -> RangeInclusive<f64>` (reads `"range"` prop)
/// - `prop_object(node, "key") -> Option<&Map<String, Value>>`
/// - `prop_value(node, "key") -> Option<&Value>` (raw JSON access)
/// - `prop_horizontal_alignment(node, "key") -> alignment::Horizontal`
/// - `prop_vertical_alignment(node, "key") -> alignment::Vertical`
/// - `prop_content_fit(node) -> Option<ContentFit>`
/// - `value_to_length(val) -> Option<Length>` (lower-level conversion)
///
/// # Panic safety
///
/// All mutable trait methods (`init`, `prepare`, `handle_event`,
/// `handle_command`, `cleanup`) are wrapped in `catch_unwind`. If your
/// extension panics, the renderer logs the error, poisons the extension
/// (disabling further calls), and renders a red placeholder in its place.
///
/// Because `catch_unwind` uses `AssertUnwindSafe`, the compiler's unwind
/// safety checks are bypassed. This means your `&mut self` state could be
/// observed in a partially-mutated state if a panic interrupts a
/// multi-step mutation. The poisoning mechanism prevents further calls, but
/// if your extension shares state across nodes via `ExtensionCaches`, keep
/// each mutation atomic -- don't leave cache entries in an intermediate
/// state where a panic between two writes would be visible to other nodes.
///
/// `render()` panics are caught by the widget dispatch layer. Three
/// consecutive render panics trigger automatic poisoning.
///
/// # Examples
///
/// A minimal render-only extension that displays a greeting:
///
/// ```rust,ignore
/// use toddy_core::prelude::*;
///
/// struct GreetingExtension;
///
/// impl WidgetExtension for GreetingExtension {
///     fn type_names(&self) -> &[&str] {
///         &["greeting"]
///     }
///
///     fn config_key(&self) -> &str {
///         "greeting"
///     }
///
///     fn render<'a>(&self, node: &'a TreeNode, _env: &WidgetEnv<'a>) -> Element<'a, Message> {
///         use toddy_core::iced::widget::text;
///         let name = node.props.get("name")
///             .and_then(|v| v.as_str())
///             .unwrap_or("world");
///         text(format!("Hello, {name}!")).into()
///     }
/// }
/// ```
pub trait WidgetExtension: Send + Sync + 'static {
    /// Node type names this extension handles (e.g. ["sparkline", "heatmap"]).
    fn type_names(&self) -> &[&str];

    /// Key used to route configuration from the Settings wire message's
    /// `extension_config` object. Must be unique across all extensions.
    fn config_key(&self) -> &str;

    /// Receive configuration and context from the host.
    ///
    /// Called on startup and renderer restart. The `ctx` provides the
    /// extension's config (from `Settings.extension_config[config_key]`),
    /// the current theme, and text rendering defaults. Extensions that
    /// need theme-dependent one-time setup can do it here instead of
    /// deferring to the first `prepare()` call.
    fn init(&mut self, _ctx: &InitCtx<'_>) {}

    /// Initialize or synchronize state for a node.
    ///
    /// Called in the mutable phase (after `Core::apply`, before `view()`)
    /// every time the tree changes (Snapshot or Patch). Nodes are visited
    /// in **depth-first pre-order** (parent before children) -- this is
    /// deterministic for a given tree structure. If an extension has
    /// multiple nodes, they're visited in tree order.
    ///
    /// Use this to populate [`ExtensionCaches`] entries that `render()`
    /// reads. The ensure_caches/render split avoids the need for
    /// `RefCell` or interior mutability in the view phase.
    fn prepare(&mut self, _node: &TreeNode, _caches: &mut ExtensionCaches, _theme: &Theme) {}

    /// Build an iced Element for a node. Called in the immutable phase (view).
    fn render<'a>(&self, node: &'a TreeNode, env: &WidgetEnv<'a>) -> Element<'a, Message>;

    /// Handle an event emitted by this extension's widgets. Called before
    /// the event reaches the wire.
    fn handle_event(
        &mut self,
        _node_id: &str,
        _family: &str,
        _data: &Value,
        _caches: &mut ExtensionCaches,
    ) -> EventResult {
        EventResult::PassThrough
    }

    /// Handle a command sent from the host directly to this extension.
    ///
    /// The host sends `ExtensionCommand` messages with an `op` string and a
    /// JSON `payload`. By convention, `op` names use `snake_case` and are
    /// scoped to the extension (e.g. `"reset_zoom"`, `"set_data"`). The
    /// extension decides what ops it supports; unrecognized ops should be
    /// logged and ignored (return an empty vec).
    ///
    /// Return a vec of `OutgoingEvent`s to emit back to the host. Errors
    /// should be reported as events with family `"extension_error"` and
    /// relevant details in the data payload, rather than panicking.
    fn handle_command(
        &mut self,
        _node_id: &str,
        _op: &str,
        _payload: &Value,
        _caches: &mut ExtensionCaches,
    ) -> Vec<OutgoingEvent> {
        vec![]
    }

    /// Clean up when a node is removed from the tree.
    fn cleanup(&mut self, _node_id: &str, _caches: &mut ExtensionCaches) {}

    /// Create a fresh instance for a new session. Required for
    /// multiplexed mode (`--max-sessions > 1`). Each session gets its
    /// own extension instances so mutable state is fully isolated.
    ///
    /// The default implementation panics. Extensions that support
    /// multiplexed sessions must override this.
    fn new_instance(&self) -> Box<dyn WidgetExtension> {
        unimplemented!(
            "extension `{}` does not support multiplexed sessions; \
             implement new_instance() to enable --max-sessions > 1",
            self.config_key()
        );
    }
}

// ---------------------------------------------------------------------------
// EventResult
// ---------------------------------------------------------------------------

/// Result of extension event handling.
///
/// Returned from [`WidgetExtension::handle_event`] to control whether the
/// original event reaches the host and whether additional events are emitted.
#[derive(Debug)]
#[must_use = "an EventResult should not be silently discarded"]
pub enum EventResult {
    /// Don't handle -- forward the original event to the host as-is.
    PassThrough,
    /// The extension consumed the event. The original event is suppressed and
    /// will NOT be forwarded to the host. The contained events (if any) are
    /// emitted instead. Note: `Consumed(vec![])` suppresses the original
    /// event without emitting any replacement -- use this intentionally, as
    /// the host will never see the event.
    Consumed(Vec<OutgoingEvent>),
    /// The extension observed the event. The original event IS forwarded to
    /// the host, and the contained additional events are also emitted.
    Observed(Vec<OutgoingEvent>),
}

// ---------------------------------------------------------------------------
// ExtensionCaches
// ---------------------------------------------------------------------------

/// Type-erased cache storage for extensions.
///
/// Keys are namespaced by extension `config_key()` to prevent collisions
/// between extensions that happen to use the same cache key string. All
/// public methods accept a `namespace` parameter (the extension's
/// `config_key()`) which is prefixed onto the raw key internally.
///
/// # Thread-safety invariant
///
/// `ExtensionCaches` is `Send + Sync` because all stored values are
/// `Any + Send + Sync`. However, the struct itself is only ever accessed
/// from a single thread at a time: mutation happens during `update()`
/// (the mutable phase), and reads happen during `view()` (the immutable
/// phase). In `--max-sessions` mode each session gets its own
/// `ExtensionCaches` instance, so there is no cross-thread sharing. No
/// internal locking is needed.
pub struct ExtensionCaches {
    inner: HashMap<String, Box<dyn Any + Send + Sync>>,
}

impl std::fmt::Debug for ExtensionCaches {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtensionCaches")
            .field("entries", &self.inner.len())
            .field("keys", &self.inner.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl ExtensionCaches {
    pub fn new() -> Self {
        Self {
            inner: HashMap::new(),
        }
    }

    /// Build the internal namespaced key: `"config_key:raw_key"`.
    fn namespaced_key(namespace: &str, key: &str) -> String {
        format!("{namespace}:{key}")
    }

    /// Look up a cached value by namespace and key.
    ///
    /// Returns `None` if the key doesn't exist **or** if the stored type
    /// doesn't match `T`. A type mismatch logs a warning so extension
    /// authors can spot accidental type changes during development.
    pub fn get<T: 'static>(&self, namespace: &str, key: &str) -> Option<&T> {
        let full_key = Self::namespaced_key(namespace, key);
        let entry = self.inner.get(&full_key)?;
        let result = entry.downcast_ref();
        if result.is_none() {
            log::warn!(
                "extension cache type mismatch for `{full_key}`: \
                 stored type does not match requested type"
            );
        }
        result
    }

    /// Look up a cached value mutably by namespace and key.
    ///
    /// Returns `None` if the key doesn't exist **or** if the stored type
    /// doesn't match `T`. A type mismatch logs a warning.
    pub fn get_mut<T: 'static>(&mut self, namespace: &str, key: &str) -> Option<&mut T> {
        let full_key = Self::namespaced_key(namespace, key);
        let entry = self.inner.get_mut(&full_key)?;
        let result = entry.downcast_mut();
        if result.is_none() {
            log::warn!(
                "extension cache type mismatch for `{full_key}`: \
                 stored type does not match requested type"
            );
        }
        result
    }

    pub fn get_or_insert<T: Send + Sync + 'static>(
        &mut self,
        namespace: &str,
        key: &str,
        default: impl FnOnce() -> T,
    ) -> &mut T {
        let ns_key = Self::namespaced_key(namespace, key);

        // Check for type mismatch on an existing entry *before* consuming
        // the default closure, so we can replace the stale value with a
        // fresh default of the correct type.
        let needs_replace = self
            .inner
            .get(&ns_key)
            .is_some_and(|v| v.downcast_ref::<T>().is_none());

        if needs_replace {
            log::warn!(
                "extension cache type mismatch for key `{ns_key}`: \
                 replacing existing entry with new default"
            );
            self.inner.remove(&ns_key);
        }

        self.inner
            .entry(ns_key)
            .or_insert_with(|| Box::new(default()))
            .downcast_mut()
            .expect("downcast must succeed: entry was just inserted with correct type")
    }

    pub fn insert<T: Send + Sync + 'static>(&mut self, namespace: &str, key: &str, value: T) {
        self.inner
            .insert(Self::namespaced_key(namespace, key), Box::new(value));
    }

    pub fn remove(&mut self, namespace: &str, key: &str) -> bool {
        self.inner
            .remove(&Self::namespaced_key(namespace, key))
            .is_some()
    }

    pub fn contains(&self, namespace: &str, key: &str) -> bool {
        self.inner
            .contains_key(&Self::namespaced_key(namespace, key))
    }

    /// Remove all entries for a given namespace prefix.
    pub fn remove_namespace(&mut self, namespace: &str) {
        let prefix = format!("{namespace}:");
        self.inner.retain(|k, _| !k.starts_with(&prefix));
    }

    pub fn clear(&mut self) {
        self.inner.clear();
    }
}

impl Default for ExtensionCaches {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// WidgetEnv and RenderCtx
// ---------------------------------------------------------------------------

/// Context provided to extension `render()` methods.
///
/// All fields are immutable references -- mutation happens in `prepare()`,
/// reads happen here. This mirrors iced's `update()`/`view()` split.
///
/// # Available data
///
/// - `caches` -- extension caches (read-only). Use
///   `caches.get::<T>(config_key, node_id)` to read per-node state
///   populated in `prepare()`.
/// - `ctx` -- the shared [`RenderCtx`] carrying images, theme,
///   defaults, and child rendering. Convenience methods below
///   delegate to it: `images()`, `theme()`, `default_text_size()`,
///   `default_font()`, `render_child()`.
pub struct WidgetEnv<'a> {
    pub caches: &'a ExtensionCaches,
    pub ctx: RenderCtx<'a>,
}

impl std::fmt::Debug for WidgetEnv<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WidgetEnv")
            .field("caches", self.caches)
            .field("ctx", &self.ctx)
            .finish()
    }
}

impl<'a> WidgetEnv<'a> {
    pub fn images(&self) -> &'a ImageRegistry {
        self.ctx.images
    }
    pub fn theme(&self) -> &'a Theme {
        self.ctx.theme
    }
    pub fn default_text_size(&self) -> Option<f32> {
        self.ctx.default_text_size
    }
    pub fn default_font(&self) -> Option<iced::Font> {
        self.ctx.default_font
    }
    pub fn render_child(&self, node: &'a TreeNode) -> Element<'a, Message> {
        self.ctx.render_child(node)
    }
    /// The toddy window ID this render is for, or `""` in headless/test.
    pub fn window_id(&self) -> &'a str {
        self.ctx.window_id
    }
    /// Display scale factor for this window (1.0 = no scaling).
    pub fn scale_factor(&self) -> f32 {
        self.ctx.scale_factor
    }
}

/// Context passed to [`WidgetExtension::init`].
///
/// Provides the extension's config (from the host's Settings message)
/// along with the current theme and text rendering defaults. This
/// allows extensions to do theme-dependent initialization without
/// deferring to the first `prepare()` call.
#[derive(Debug)]
pub struct InitCtx<'a> {
    /// Extension-specific config from `Settings.extension_config[config_key]`.
    /// `Value::Null` if the host didn't provide config for this extension.
    pub config: &'a Value,
    /// The current theme at init time.
    pub theme: &'a Theme,
    /// Global default text size, if set by the host.
    pub default_text_size: Option<f32>,
    /// Global default font, if set by the host.
    pub default_font: Option<iced::Font>,
}

/// Renders child nodes through the main dispatch. Copy-able (all shared refs).
///
/// Extensions receive this via [`WidgetEnv`] in their `render()` method.
/// It carries everything needed for rendering: the widget tree state,
/// image handles, theme, text defaults, and per-window context.
#[derive(Clone, Copy)]
pub struct RenderCtx<'a> {
    pub caches: &'a WidgetCaches,
    pub images: &'a ImageRegistry,
    pub theme: &'a Theme,
    pub extensions: &'a ExtensionDispatcher,
    pub default_text_size: Option<f32>,
    pub default_font: Option<iced::Font>,
    /// The toddy window ID this render is for, or `""` in headless/test.
    pub window_id: &'a str,
    /// The display scale factor for this window (1.0 = no scaling).
    /// Useful for DPI-aware canvas rendering.
    pub scale_factor: f32,
}

impl std::fmt::Debug for RenderCtx<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderCtx")
            .field("window_id", &self.window_id)
            .field("scale_factor", &self.scale_factor)
            .field("default_text_size", &self.default_text_size)
            .field("default_font", &self.default_font)
            .finish_non_exhaustive()
    }
}

impl<'a> RenderCtx<'a> {
    /// Render a child node through the main dispatch.
    pub fn render_child(&self, node: &'a TreeNode) -> Element<'a, Message> {
        crate::widgets::render(node, *self)
    }

    /// Create a new RenderCtx with a different theme, preserving all other fields.
    pub fn with_theme(&self, theme: &'a Theme) -> Self {
        RenderCtx { theme, ..*self }
    }

    /// Render all children of a node through the main dispatch.
    pub fn render_children(&self, node: &'a TreeNode) -> Vec<Element<'a, Message>> {
        node.children.iter().map(|c| self.render_child(c)).collect()
    }
}

// ---------------------------------------------------------------------------
// ExtensionDispatcher
// ---------------------------------------------------------------------------

/// Number of consecutive render panics before an extension is poisoned.
const RENDER_PANIC_THRESHOLD: u32 = 3;

/// Owns registered extensions and routes messages to them.
///
/// Maintains a type-name index for O(1) dispatch, a node-to-extension
/// map for event/command routing, and per-extension poison state for
/// panic isolation. Created via
/// [`ToddyAppBuilder::build_dispatcher`](crate::app::ToddyAppBuilder::build_dispatcher).
pub struct ExtensionDispatcher {
    extensions: Vec<Box<dyn WidgetExtension>>,
    type_name_index: HashMap<String, usize>,
    node_extension_map: HashMap<String, usize>,
    poisoned: Vec<bool>,
    /// Per-extension consecutive render panic counter. Stored as AtomicU32
    /// so `record_render_panic` can be called with `&self` (the dispatcher
    /// is borrowed immutably during view/render).
    render_panic_counts: Vec<AtomicU32>,
}

impl std::fmt::Debug for ExtensionDispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let type_names: Vec<_> = self.type_name_index.keys().collect();
        f.debug_struct("ExtensionDispatcher")
            .field("extensions", &self.extensions.len())
            .field("type_names", &type_names)
            .field("poisoned", &self.poisoned)
            .finish()
    }
}

impl ExtensionDispatcher {
    pub fn new(extensions: Vec<Box<dyn WidgetExtension>>) -> Self {
        let n = extensions.len();

        // Validate extension metadata before building the index.
        for ext in &extensions {
            if ext.config_key().is_empty() {
                panic!(
                    "extension registered with empty config_key() \
                     (type_names: {:?})",
                    ext.type_names()
                );
            }
            if ext.config_key().contains(':') {
                panic!(
                    "extension config_key `{}` contains ':' (reserved as \
                     cache namespace separator); type_names: {:?}",
                    ext.config_key(),
                    ext.type_names()
                );
            }
            if ext.type_names().is_empty() {
                log::warn!(
                    "extension `{}` registered with empty type_names(); \
                     it will never match any node type",
                    ext.config_key()
                );
            }
        }

        // Check for duplicate config_key values.
        let mut seen_config_keys: HashMap<&str, usize> = HashMap::new();
        for (idx, ext) in extensions.iter().enumerate() {
            let key = ext.config_key();
            if let Some(prev_idx) = seen_config_keys.insert(key, idx) {
                panic!(
                    "duplicate extension config_key `{key}`: \
                     extension at index {prev_idx} (type_names: {:?}) and \
                     extension at index {idx} (type_names: {:?}) both use it",
                    extensions[prev_idx].type_names(),
                    ext.type_names(),
                );
            }
        }

        let mut type_name_index = HashMap::new();
        for (idx, ext) in extensions.iter().enumerate() {
            for &name in ext.type_names() {
                if let Some(prev_idx) = type_name_index.insert(name.to_string(), idx) {
                    panic!(
                        "duplicate extension type name `{name}`: \
                         extension `{}` (index {prev_idx}) and \
                         extension `{}` (index {idx}) both claim it",
                        extensions[prev_idx].config_key(),
                        ext.config_key(),
                    );
                }
            }
        }

        let render_panic_counts = (0..n).map(|_| AtomicU32::new(0)).collect();

        Self {
            extensions,
            type_name_index,
            node_extension_map: HashMap::new(),
            poisoned: vec![false; n],
            render_panic_counts,
        }
    }

    /// Create a new dispatcher for a multiplexed session.
    ///
    /// Calls [`WidgetExtension::new_instance()`] on each registered
    /// extension to produce independent instances with isolated mutable
    /// state. The type-name index is rebuilt from the new instances.
    ///
    /// Returns `Err` with a description if any extension panics in
    /// `new_instance()` (the default implementation panics when not
    /// overridden). The caller should log the error and reject the
    /// session rather than crashing the reader thread.
    pub fn clone_for_session(&self) -> Result<Self, String> {
        let mut extensions: Vec<Box<dyn WidgetExtension>> =
            Vec::with_capacity(self.extensions.len());
        for ext in &self.extensions {
            let key = ext.config_key().to_string();
            if catch_unwind_enabled() {
                match catch_unwind(AssertUnwindSafe(|| ext.new_instance())) {
                    Ok(instance) => extensions.push(instance),
                    Err(payload) => {
                        let msg = panic_message(&payload);
                        return Err(format!(
                            "extension `{key}` panicked in new_instance(): {msg}"
                        ));
                    }
                }
            } else {
                extensions.push(ext.new_instance());
            }
        }
        Ok(Self::new(extensions))
    }

    /// Check if a node type is handled by an extension.
    pub fn handles_type(&self, type_name: &str) -> bool {
        self.type_name_index.contains_key(type_name)
    }

    /// Maximum tree recursion depth for walk_prepare.
    const MAX_WALK_DEPTH: usize = crate::widgets::MAX_TREE_DEPTH;

    /// Called after Core::apply() on tree changes.
    pub fn prepare_all(&mut self, root: &TreeNode, caches: &mut ExtensionCaches, theme: &Theme) {
        let mut new_map = HashMap::new();
        self.walk_prepare(root, caches, theme, &mut new_map, 0);

        // Prune stale nodes
        for (old_id, ext_idx) in &self.node_extension_map {
            if !new_map.contains_key(old_id) {
                let ns = self.extensions[*ext_idx].config_key().to_string();
                if self.poisoned[*ext_idx] {
                    caches.remove(&ns, old_id);
                    log::warn!(
                        "skipping cleanup for poisoned extension `{ns}`; \
                         cache entry removed for node `{old_id}`",
                    );
                } else if catch_unwind_enabled() {
                    let result = catch_unwind(AssertUnwindSafe(|| {
                        self.extensions[*ext_idx].cleanup(old_id, caches);
                    }));
                    if let Err(panic) = result {
                        let msg = panic_message(&panic);
                        log::error!("extension `{ns}` panicked in cleanup: {msg}",);
                        self.poisoned[*ext_idx] = true;
                        caches.remove(&ns, old_id);
                    }
                } else {
                    self.extensions[*ext_idx].cleanup(old_id, caches);
                }
            }
        }

        self.node_extension_map = new_map;

        // Check render panic counters -- poison extensions that exceeded
        // the threshold. Also reset counters for non-poisoned extensions
        // (a successful prepare cycle implies the tree was rebuilt, so
        // we give extensions a fresh chance).
        for idx in 0..self.extensions.len() {
            let count = self.render_panic_counts[idx].load(Ordering::Relaxed);
            if count >= RENDER_PANIC_THRESHOLD && !self.poisoned[idx] {
                log::error!(
                    "extension `{}` hit {} consecutive render panics, poisoning",
                    self.extensions[idx].config_key(),
                    count,
                );
                self.poisoned[idx] = true;
            }
            if !self.poisoned[idx] {
                self.render_panic_counts[idx].store(0, Ordering::Relaxed);
            }
        }
    }

    fn walk_prepare(
        &mut self,
        node: &TreeNode,
        caches: &mut ExtensionCaches,
        theme: &Theme,
        map: &mut HashMap<String, usize>,
        depth: usize,
    ) {
        if depth > Self::MAX_WALK_DEPTH {
            log::warn!(
                "[id={}] walk_prepare depth exceeds {}, skipping subtree",
                node.id,
                Self::MAX_WALK_DEPTH
            );
            return;
        }
        if let Some(&idx) = self.type_name_index.get(node.type_name.as_str()) {
            if !self.poisoned[idx] {
                if catch_unwind_enabled() {
                    let result = catch_unwind(AssertUnwindSafe(|| {
                        self.extensions[idx].prepare(node, caches, theme);
                    }));
                    if let Err(panic) = result {
                        let msg = panic_message(&panic);
                        log::error!(
                            "extension `{}` panicked in prepare: {msg}",
                            self.extensions[idx].config_key()
                        );
                        self.poisoned[idx] = true;
                    }
                } else {
                    self.extensions[idx].prepare(node, caches, theme);
                }
            }
            map.insert(node.id.clone(), idx);
        }
        for child in &node.children {
            self.walk_prepare(child, caches, theme, map, depth + 1);
        }
    }

    /// Handle a Message::Event.
    pub fn handle_event(
        &mut self,
        id: &str,
        family: &str,
        data: &Value,
        caches: &mut ExtensionCaches,
    ) -> EventResult {
        let ext_idx = match self.node_extension_map.get(id) {
            Some(&idx) => idx,
            None => return EventResult::PassThrough,
        };
        if self.poisoned[ext_idx] {
            log::error!(
                "extension `{}` is poisoned, dropping event `{family}` for node `{id}`",
                self.extensions[ext_idx].config_key()
            );
            return EventResult::PassThrough;
        }
        if catch_unwind_enabled() {
            match catch_unwind(AssertUnwindSafe(|| {
                self.extensions[ext_idx].handle_event(id, family, data, caches)
            })) {
                Ok(result) => result,
                Err(panic) => {
                    let msg = panic_message(&panic);
                    log::error!(
                        "extension `{}` panicked in handle_event: {msg}",
                        self.extensions[ext_idx].config_key()
                    );
                    self.poisoned[ext_idx] = true;
                    EventResult::PassThrough
                }
            }
        } else {
            self.extensions[ext_idx].handle_event(id, family, data, caches)
        }
    }

    /// Handle an ExtensionCommand.
    pub fn handle_command(
        &mut self,
        node_id: &str,
        op: &str,
        payload: &Value,
        caches: &mut ExtensionCaches,
    ) -> Vec<OutgoingEvent> {
        let ext_idx = match self.node_extension_map.get(node_id) {
            Some(&idx) => idx,
            None => {
                log::warn!("extension command for unknown node `{node_id}`, ignoring");
                return vec![OutgoingEvent::generic(
                    "extension_error".to_string(),
                    node_id.to_string(),
                    Some(serde_json::json!({
                        "error": format!("no extension handles node `{node_id}`"),
                        "op": op,
                    })),
                )];
            }
        };
        if self.poisoned[ext_idx] {
            return vec![OutgoingEvent::generic(
                "extension_error".to_string(),
                node_id.to_string(),
                Some(serde_json::json!({
                    "error": "extension is disabled due to previous panics",
                    "op": op,
                })),
            )];
        }
        if catch_unwind_enabled() {
            match catch_unwind(AssertUnwindSafe(|| {
                self.extensions[ext_idx].handle_command(node_id, op, payload, caches)
            })) {
                Ok(events) => events,
                Err(panic) => {
                    let msg = panic_message(&panic);
                    log::error!(
                        "extension `{}` panicked in handle_command: {msg}",
                        self.extensions[ext_idx].config_key()
                    );
                    self.poisoned[ext_idx] = true;
                    // Report the panic back to the host so it can handle it.
                    let error_data = serde_json::json!({
                        "error": msg,
                        "op": op,
                    });
                    vec![OutgoingEvent::generic(
                        "extension_error",
                        node_id.to_string(),
                        Some(error_data),
                    )]
                }
            }
        } else {
            self.extensions[ext_idx].handle_command(node_id, op, payload, caches)
        }
    }

    /// Route configuration and context to extensions.
    ///
    /// `config` is the value of `extension_config` from Settings -- a
    /// JSON object keyed by each extension's `config_key()`. Each
    /// extension receives an [`InitCtx`] with its own config slice plus
    /// the current theme and text defaults.
    pub fn init_all(
        &mut self,
        config: &Value,
        theme: &Theme,
        default_text_size: Option<f32>,
        default_font: Option<iced::Font>,
    ) {
        for (idx, ext) in self.extensions.iter_mut().enumerate() {
            if self.poisoned[idx] {
                continue;
            }
            let key = ext.config_key().to_string();
            let ext_config = config.get(&key).unwrap_or(&Value::Null);
            let ctx = InitCtx {
                config: ext_config,
                theme,
                default_text_size,
                default_font,
            };
            if catch_unwind_enabled() {
                let result = catch_unwind(AssertUnwindSafe(|| {
                    ext.init(&ctx);
                }));
                if let Err(panic) = result {
                    let msg = panic_message(&panic);
                    log::error!("extension `{key}` panicked in init: {msg}");
                    self.poisoned[idx] = true;
                }
            } else {
                ext.init(&ctx);
            }
        }
    }

    /// Render an extension node. Returns None if no extension handles this type.
    ///
    /// The caller must construct the `WidgetEnv` and pass it in. This avoids
    /// a borrow-checker issue where a locally-constructed env would be dropped
    /// before the returned Element (which borrows from the env).
    ///
    /// Note: catch_unwind happens in the caller (`widgets::render`) because
    /// the returned Element borrows from env and can't be wrapped in a
    /// closure. When a render panic is caught, the caller should call
    /// `record_render_panic` to track consecutive failures.
    pub fn render<'a>(
        &'a self,
        node: &'a TreeNode,
        env: &WidgetEnv<'a>,
    ) -> Option<Element<'a, Message>> {
        let &idx = self.type_name_index.get(node.type_name.as_str())?;
        if self.poisoned[idx] {
            return Some(render_poisoned_placeholder(node));
        }
        let element = self.extensions[idx].render(node, env);
        // Successful render -- reset consecutive panic counter.
        self.render_panic_counts[idx].store(0, Ordering::Relaxed);
        Some(element)
    }

    /// Record a render panic for the extension that handles `type_name`.
    /// Called by the catch_unwind wrapper in `widgets::render` (which has
    /// only `&self`). Uses AtomicU32 so no `&mut self` is required.
    /// Returns `true` if the extension has reached the poison threshold.
    pub fn record_render_panic(&self, type_name: &str) -> bool {
        if let Some(&idx) = self.type_name_index.get(type_name) {
            let prev = self.render_panic_counts[idx].fetch_add(1, Ordering::Relaxed);
            prev + 1 >= RENDER_PANIC_THRESHOLD
        } else {
            false
        }
    }

    /// Reset all poisoned flags and render panic counters. Called on Snapshot.
    pub fn clear_poisoned(&mut self) {
        self.poisoned.fill(false);
        for counter in &self.render_panic_counts {
            counter.store(0, Ordering::Relaxed);
        }
    }

    /// Call cleanup() for every node currently tracked by the dispatcher.
    ///
    /// Used before a full state reset (e.g. Reset message) so extensions
    /// get a chance to release per-node resources before their cache
    /// entries are wiped.
    pub fn cleanup_all(&mut self, caches: &mut ExtensionCaches) {
        for (node_id, &ext_idx) in &self.node_extension_map {
            if self.poisoned[ext_idx] {
                continue;
            }
            if catch_unwind_enabled() {
                let result = catch_unwind(AssertUnwindSafe(|| {
                    self.extensions[ext_idx].cleanup(node_id, caches);
                }));
                if let Err(panic) = result {
                    let msg = panic_message(&panic);
                    log::error!(
                        "extension `{}` panicked in cleanup: {msg}",
                        self.extensions[ext_idx].config_key()
                    );
                    self.poisoned[ext_idx] = true;
                }
            } else {
                self.extensions[ext_idx].cleanup(node_id, caches);
            }
        }
    }

    /// Full reset: call cleanup for all tracked nodes, clear the node map,
    /// clear extension caches, and reset poisoned state.
    ///
    /// Extensions themselves (the registered trait objects) are preserved --
    /// only per-node runtime state is wiped.
    pub fn reset(&mut self, caches: &mut ExtensionCaches) {
        self.cleanup_all(caches);
        self.node_extension_map.clear();
        caches.clear();
        self.clear_poisoned();
    }

    /// Check if any extensions are registered.
    pub fn is_empty(&self) -> bool {
        self.extensions.is_empty()
    }

    /// Check if a specific extension (by index) is poisoned.
    #[cfg(test)]
    pub fn is_poisoned(&self, idx: usize) -> bool {
        self.poisoned.get(idx).copied().unwrap_or(false)
    }

    /// Number of registered extensions.
    pub fn len(&self) -> usize {
        self.extensions.len()
    }
}

impl Default for ExtensionDispatcher {
    fn default() -> Self {
        Self::new(vec![])
    }
}

// ---------------------------------------------------------------------------
// GenerationCounter
// ---------------------------------------------------------------------------

/// A monotonically increasing counter for tracking data changes.
///
/// Store in `ExtensionCaches` alongside your data. Call `bump()` when data
/// changes (in `handle_command` or `prepare`). In your `canvas::Program`
/// implementation, compare the generation against a saved value in your
/// `Program::State` to decide whether to clear and redraw the cache.
///
/// # Example
///
/// ```ignore
/// struct MyState {
///     generation: u64,
///     cache: canvas::Cache,
/// }
///
/// impl canvas::Program<Message> for MyProgram {
///     type State = MyState;
///
///     // update() has &mut State -- clear the cache here when data changes.
///     fn update(&self, state: &mut MyState, ...) -> Option<Action<Message>> {
///         if state.generation != self.current_generation {
///             state.cache.clear();
///             state.generation = self.current_generation;
///         }
///         None
///     }
///
///     // draw() has &State -- the cache handles re-tessellation automatically
///     // when cleared above.
///     fn draw(&self, state: &MyState, ...) -> Vec<Geometry> {
///         vec![state.cache.draw(renderer, bounds.size(), |frame| { ... })]
///     }
/// }
/// ```
#[derive(Debug, Clone)]
pub struct GenerationCounter {
    value: u64,
}

impl GenerationCounter {
    /// Create a new counter starting at zero.
    pub fn new() -> Self {
        Self { value: 0 }
    }

    /// Return the current generation value.
    pub fn get(&self) -> u64 {
        self.value
    }

    /// Increment the generation. Wraps on overflow (u64 -- effectively never).
    pub fn bump(&mut self) {
        self.value = self.value.wrapping_add(1);
    }
}

impl Default for GenerationCounter {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Rendered in place of a poisoned extension. Shows the type and node ID
/// so the developer can identify which extension failed. The panic details
/// (which method, the panic message) are logged at error level -- check
/// stderr or RUST_LOG output for the full diagnostic.
fn render_poisoned_placeholder<'a>(node: &TreeNode) -> Element<'a, Message> {
    use iced::Color;
    use iced::widget::text;
    text(format!(
        "Extension error: type `{}`, node `{}` (see logs)",
        node.type_name, node.id
    ))
    .color(Color::from_rgb(1.0, 0.0, 0.0))
    .into()
}

fn panic_message(panic: &Box<dyn Any + Send>) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Test extension implementations --------------------------------------

    /// Minimal test extension that renders a text widget.
    struct TestExtension {
        type_names: Vec<&'static str>,
        config_key: &'static str,
        init_called: bool,
    }

    impl TestExtension {
        fn new(type_names: Vec<&'static str>, config_key: &'static str) -> Self {
            Self {
                type_names,
                config_key,
                init_called: false,
            }
        }
    }

    impl WidgetExtension for TestExtension {
        fn type_names(&self) -> &[&str] {
            &self.type_names
        }

        fn config_key(&self) -> &str {
            self.config_key
        }

        fn init(&mut self, _ctx: &InitCtx<'_>) {
            self.init_called = true;
        }

        fn render<'a>(&self, node: &'a TreeNode, _env: &WidgetEnv<'a>) -> Element<'a, Message> {
            use iced::widget::text;
            text(format!("test:{}", node.id)).into()
        }
    }

    /// Extension with empty type_names (valid but useless -- should warn).
    struct EmptyTypesExtension;

    impl WidgetExtension for EmptyTypesExtension {
        fn type_names(&self) -> &[&str] {
            &[]
        }
        fn config_key(&self) -> &str {
            "empty_types"
        }
        fn render<'a>(&self, _node: &'a TreeNode, _env: &WidgetEnv<'a>) -> Element<'a, Message> {
            use iced::widget::text;
            text("empty").into()
        }
    }

    use crate::testing::node as make_node;

    // -- Registration and type_name_index ------------------------------------

    #[test]
    fn registration_builds_type_name_index() {
        let ext = TestExtension::new(vec!["sparkline", "heatmap"], "charts");
        let dispatcher = ExtensionDispatcher::new(vec![Box::new(ext)]);

        assert!(dispatcher.handles_type("sparkline"));
        assert!(dispatcher.handles_type("heatmap"));
        assert!(!dispatcher.handles_type("unknown"));
    }

    #[test]
    fn registration_with_multiple_extensions() {
        let ext_a = TestExtension::new(vec!["sparkline"], "charts");
        let ext_b = TestExtension::new(vec!["gauge"], "instruments");
        let dispatcher = ExtensionDispatcher::new(vec![Box::new(ext_a), Box::new(ext_b)]);

        assert!(dispatcher.handles_type("sparkline"));
        assert!(dispatcher.handles_type("gauge"));
        assert_eq!(dispatcher.len(), 2);
    }

    #[test]
    fn empty_dispatcher_handles_nothing() {
        let dispatcher = ExtensionDispatcher::default();
        assert!(dispatcher.is_empty());
        assert!(!dispatcher.handles_type("anything"));
    }

    // -- Duplicate type name detection ---------------------------------------

    #[test]
    #[should_panic(expected = "duplicate extension type name `sparkline`")]
    fn duplicate_type_name_panics() {
        let ext_a = TestExtension::new(vec!["sparkline"], "charts_a");
        let ext_b = TestExtension::new(vec!["sparkline"], "charts_b");
        ExtensionDispatcher::new(vec![Box::new(ext_a), Box::new(ext_b)]);
    }

    #[test]
    #[should_panic(expected = "both claim it")]
    fn duplicate_type_name_error_identifies_conflicting_extensions() {
        let ext_a = TestExtension::new(vec!["widget_x"], "ext_alpha");
        let ext_b = TestExtension::new(vec!["widget_x"], "ext_beta");
        ExtensionDispatcher::new(vec![Box::new(ext_a), Box::new(ext_b)]);
    }

    // -- Empty config_key validation -----------------------------------------

    #[test]
    #[should_panic(expected = "empty config_key()")]
    fn empty_config_key_panics() {
        let ext = TestExtension::new(vec!["widget"], "");
        ExtensionDispatcher::new(vec![Box::new(ext)]);
    }

    // -- Colon in config_key validation -----------------------------------------

    #[test]
    #[should_panic(expected = "contains ':'")]
    fn config_key_with_colon_panics() {
        let ext = TestExtension::new(vec!["widget"], "bad:key");
        ExtensionDispatcher::new(vec![Box::new(ext)]);
    }

    // -- Duplicate config_key validation ---------------------------------------

    #[test]
    #[should_panic(expected = "duplicate extension config_key `charts`")]
    fn duplicate_config_key_panics() {
        let ext_a = TestExtension::new(vec!["sparkline"], "charts");
        let ext_b = TestExtension::new(vec!["heatmap"], "charts");
        ExtensionDispatcher::new(vec![Box::new(ext_a), Box::new(ext_b)]);
    }

    // -- Empty type_names validation (warn, don't panic) ---------------------

    #[test]
    fn empty_type_names_does_not_panic() {
        // Should log a warning but not panic.
        let ext = EmptyTypesExtension;
        let dispatcher = ExtensionDispatcher::new(vec![Box::new(ext)]);
        assert_eq!(dispatcher.len(), 1);
        assert!(!dispatcher.handles_type("anything"));
    }

    // -- ExtensionCaches: get/insert/get_or_insert ---------------------------

    #[test]
    fn cache_insert_and_get() {
        let mut caches = ExtensionCaches::new();
        caches.insert("charts", "node1", 42u32);

        assert_eq!(caches.get::<u32>("charts", "node1"), Some(&42));
        assert_eq!(caches.get::<u32>("charts", "node2"), None);
    }

    #[test]
    fn cache_get_mut() {
        let mut caches = ExtensionCaches::new();
        caches.insert("ns", "key", vec![1, 2, 3]);

        if let Some(v) = caches.get_mut::<Vec<i32>>("ns", "key") {
            v.push(4);
        }
        assert_eq!(caches.get::<Vec<i32>>("ns", "key"), Some(&vec![1, 2, 3, 4]));
    }

    #[test]
    fn cache_get_or_insert_creates_default() {
        let mut caches = ExtensionCaches::new();
        let val = caches.get_or_insert::<String>("ns", "key", || "hello".to_string());
        assert_eq!(val, "hello");

        // Second call returns existing value, doesn't overwrite.
        let val = caches.get_or_insert::<String>("ns", "key", || "world".to_string());
        assert_eq!(val, "hello");
    }

    #[test]
    fn cache_get_or_insert_type_mismatch_replaces_with_default() {
        let mut caches = ExtensionCaches::new();
        caches.insert("ns", "key", 42u32);
        // Previously this panicked. Now it logs a warning, replaces the
        // stale entry, and returns a fresh default of the requested type.
        let val = caches.get_or_insert::<String>("ns", "key", || "replaced".to_string());
        assert_eq!(val, "replaced");
    }

    #[test]
    fn cache_wrong_type_returns_none() {
        let mut caches = ExtensionCaches::new();
        caches.insert("ns", "key", 42u32);

        // Asking for a different type returns None (not a panic for get).
        assert_eq!(caches.get::<String>("ns", "key"), None);
    }

    #[test]
    fn cache_remove_and_contains() {
        let mut caches = ExtensionCaches::new();
        caches.insert("ns", "key", 1u8);

        assert!(caches.contains("ns", "key"));
        assert!(caches.remove("ns", "key"));
        assert!(!caches.contains("ns", "key"));
        assert!(!caches.remove("ns", "key"));
    }

    #[test]
    fn cache_clear_removes_everything() {
        let mut caches = ExtensionCaches::new();
        caches.insert("a", "k1", 1u32);
        caches.insert("b", "k2", 2u32);

        caches.clear();
        assert!(!caches.contains("a", "k1"));
        assert!(!caches.contains("b", "k2"));
    }

    // -- Cache namespace isolation -------------------------------------------

    #[test]
    fn cache_namespace_isolation() {
        let mut caches = ExtensionCaches::new();

        // Two extensions use the same raw key "data" -- they shouldn't collide.
        caches.insert("charts", "data", vec![1.0f64, 2.0, 3.0]);
        caches.insert("gauges", "data", 42u32);

        assert_eq!(
            caches.get::<Vec<f64>>("charts", "data"),
            Some(&vec![1.0, 2.0, 3.0])
        );
        assert_eq!(caches.get::<u32>("gauges", "data"), Some(&42));
    }

    #[test]
    fn cache_remove_namespace() {
        let mut caches = ExtensionCaches::new();
        caches.insert("charts", "a", 1u32);
        caches.insert("charts", "b", 2u32);
        caches.insert("gauges", "a", 3u32);

        caches.remove_namespace("charts");

        assert!(!caches.contains("charts", "a"));
        assert!(!caches.contains("charts", "b"));
        assert!(caches.contains("gauges", "a"));
    }

    // -- Poison flag management ----------------------------------------------

    #[test]
    fn poison_flag_set_and_clear() {
        let ext = TestExtension::new(vec!["sparkline"], "charts");
        let mut dispatcher = ExtensionDispatcher::new(vec![Box::new(ext)]);

        assert!(!dispatcher.is_poisoned(0));

        // Simulate poisoning via render panic counter.
        for _ in 0..RENDER_PANIC_THRESHOLD {
            dispatcher.record_render_panic("sparkline");
        }

        // Poisoning happens on next prepare_all call.
        let root = make_node("root", "column");
        let mut caches = ExtensionCaches::new();
        dispatcher.prepare_all(&root, &mut caches, &Theme::Dark);

        assert!(dispatcher.is_poisoned(0));

        // clear_poisoned resets everything.
        dispatcher.clear_poisoned();
        assert!(!dispatcher.is_poisoned(0));
    }

    // -- Render panic tracking -----------------------------------------------

    #[test]
    fn record_render_panic_increments_counter() {
        let ext = TestExtension::new(vec!["sparkline"], "charts");
        let dispatcher = ExtensionDispatcher::new(vec![Box::new(ext)]);

        // Below threshold -- returns false.
        assert!(!dispatcher.record_render_panic("sparkline"));
        assert!(!dispatcher.record_render_panic("sparkline"));

        // At threshold -- returns true.
        assert!(dispatcher.record_render_panic("sparkline"));
    }

    #[test]
    fn record_render_panic_unknown_type_returns_false() {
        let dispatcher = ExtensionDispatcher::default();
        assert!(!dispatcher.record_render_panic("nonexistent"));
    }

    // -- EventResult variants ------------------------------------------------

    #[test]
    fn event_result_pass_through() {
        let result = EventResult::PassThrough;
        assert!(matches!(result, EventResult::PassThrough));
    }

    #[test]
    fn event_result_consumed_with_events() {
        let events = vec![OutgoingEvent::generic("test", "n1".to_string(), None)];
        let result = EventResult::Consumed(events);
        match result {
            EventResult::Consumed(e) => assert_eq!(e.len(), 1),
            _ => panic!("expected Consumed"),
        }
    }

    #[test]
    fn event_result_observed_with_events() {
        let events = vec![OutgoingEvent::generic("test", "n1".to_string(), None)];
        let result = EventResult::Observed(events);
        match result {
            EventResult::Observed(e) => assert_eq!(e.len(), 1),
            _ => panic!("expected Observed"),
        }
    }

    // -- GenerationCounter ---------------------------------------------------

    #[test]
    fn generation_counter_starts_at_zero() {
        let counter = GenerationCounter::new();
        assert_eq!(counter.get(), 0);
    }

    #[test]
    fn generation_counter_bumps() {
        let mut counter = GenerationCounter::new();
        counter.bump();
        assert_eq!(counter.get(), 1);
        counter.bump();
        assert_eq!(counter.get(), 2);
    }

    #[test]
    fn generation_counter_default() {
        let counter = GenerationCounter::default();
        assert_eq!(counter.get(), 0);
    }

    // -- init_all ------------------------------------------------------------

    #[test]
    fn init_all_routes_config_by_key() {
        let ext = TestExtension::new(vec!["sparkline"], "charts");
        let mut dispatcher = ExtensionDispatcher::new(vec![Box::new(ext)]);

        let config = serde_json::json!({"charts": {"color": "red"}});
        dispatcher.init_all(&config, &Theme::Dark, None, None);

        // Can't easily inspect init_called through the trait object, but
        // at least verify no panic occurred.
        assert!(!dispatcher.is_poisoned(0));
    }

    // -- panic_message helper ------------------------------------------------

    #[test]
    fn panic_message_extracts_str() {
        let p: Box<dyn Any + Send> = Box::new("boom");
        assert_eq!(panic_message(&p), "boom");
    }

    #[test]
    fn panic_message_extracts_string() {
        let p: Box<dyn Any + Send> = Box::new("kaboom".to_string());
        assert_eq!(panic_message(&p), "kaboom");
    }

    #[test]
    fn panic_message_unknown_type() {
        let p: Box<dyn Any + Send> = Box::new(42u32);
        assert_eq!(panic_message(&p), "unknown panic");
    }

    // -- handle_command panic emits error event ------------------------------

    /// Extension that panics on handle_command.
    struct PanickingCommandExtension;

    impl WidgetExtension for PanickingCommandExtension {
        fn type_names(&self) -> &[&str] {
            &["panicker"]
        }
        fn config_key(&self) -> &str {
            "panicker"
        }
        fn render<'a>(&self, _node: &'a TreeNode, _env: &WidgetEnv<'a>) -> Element<'a, Message> {
            use iced::widget::text;
            text("panicker").into()
        }
        fn handle_command(
            &mut self,
            _node_id: &str,
            _op: &str,
            _payload: &Value,
            _caches: &mut ExtensionCaches,
        ) -> Vec<OutgoingEvent> {
            panic!("command went boom");
        }
    }

    #[test]
    fn handle_command_panic_emits_error_event() {
        let ext = PanickingCommandExtension;
        let mut dispatcher = ExtensionDispatcher::new(vec![Box::new(ext)]);
        let mut caches = ExtensionCaches::new();

        // Register the node in the extension map via prepare_all.
        let mut root = make_node("root", "column");
        root.children.push(make_node("p1", "panicker"));
        dispatcher.prepare_all(&root, &mut caches, &Theme::Dark);

        let events = dispatcher.handle_command("p1", "do_thing", &Value::Null, &mut caches);

        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert_eq!(event.family, "extension_error");
        assert_eq!(event.id, "p1");
        let data = event.data.as_ref().expect("should have data");
        assert_eq!(
            data.get("error").and_then(|v| v.as_str()),
            Some("command went boom")
        );
        assert_eq!(data.get("op").and_then(|v| v.as_str()), Some("do_thing"));

        // Extension should also be poisoned.
        assert!(dispatcher.is_poisoned(0));
    }

    #[test]
    fn handle_command_poisoned_returns_error_event() {
        let ext = PanickingCommandExtension;
        let mut dispatcher = ExtensionDispatcher::new(vec![Box::new(ext)]);
        let mut caches = ExtensionCaches::new();

        // Register the node.
        let mut root = make_node("root", "column");
        root.children.push(make_node("p1", "panicker"));
        dispatcher.prepare_all(&root, &mut caches, &Theme::Dark);

        // Poison via render panic threshold.
        for _ in 0..RENDER_PANIC_THRESHOLD {
            dispatcher.record_render_panic("panicker");
        }
        dispatcher.prepare_all(&root, &mut caches, &Theme::Dark);
        assert!(dispatcher.is_poisoned(0));

        // Command on a poisoned extension should return an error event.
        let events = dispatcher.handle_command("p1", "do_thing", &Value::Null, &mut caches);
        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert_eq!(event.family, "extension_error");
        assert_eq!(event.id, "p1");
        let data = event.data.as_ref().expect("should have data");
        assert_eq!(
            data.get("error").and_then(|v| v.as_str()),
            Some("extension is disabled due to previous panics")
        );
        assert_eq!(data.get("op").and_then(|v| v.as_str()), Some("do_thing"));
    }

    // -- cleanup_all ----------------------------------------------------------

    /// Extension that tracks cleanup calls.
    struct CleanupTracker {
        cleaned_ids: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl CleanupTracker {
        fn new(tracker: std::sync::Arc<std::sync::Mutex<Vec<String>>>) -> Self {
            Self {
                cleaned_ids: tracker,
            }
        }
    }

    impl WidgetExtension for CleanupTracker {
        fn type_names(&self) -> &[&str] {
            &["tracked"]
        }
        fn config_key(&self) -> &str {
            "tracker"
        }
        fn render<'a>(&self, _node: &'a TreeNode, _env: &WidgetEnv<'a>) -> Element<'a, Message> {
            use iced::widget::text;
            text("tracked").into()
        }
        fn cleanup(&mut self, node_id: &str, _caches: &mut ExtensionCaches) {
            self.cleaned_ids.lock().unwrap().push(node_id.to_string());
        }
    }

    #[test]
    fn cleanup_all_calls_cleanup_for_tracked_nodes() {
        let tracker = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let ext = CleanupTracker::new(tracker.clone());
        let mut dispatcher = ExtensionDispatcher::new(vec![Box::new(ext)]);
        let mut caches = ExtensionCaches::new();

        // Register two nodes via prepare_all.
        let mut root = make_node("root", "column");
        root.children.push(make_node("t1", "tracked"));
        root.children.push(make_node("t2", "tracked"));
        dispatcher.prepare_all(&root, &mut caches, &Theme::Dark);

        // cleanup_all should fire cleanup for both tracked nodes.
        dispatcher.cleanup_all(&mut caches);
        let cleaned = tracker.lock().unwrap();
        assert!(cleaned.contains(&"t1".to_string()));
        assert!(cleaned.contains(&"t2".to_string()));
        assert_eq!(cleaned.len(), 2);
    }

    #[test]
    fn cleanup_all_skips_poisoned_extensions() {
        let tracker = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let ext = CleanupTracker::new(tracker.clone());
        let mut dispatcher = ExtensionDispatcher::new(vec![Box::new(ext)]);
        let mut caches = ExtensionCaches::new();

        let mut root = make_node("root", "column");
        root.children.push(make_node("t1", "tracked"));
        dispatcher.prepare_all(&root, &mut caches, &Theme::Dark);

        // Poison the extension via render panics.
        for _ in 0..RENDER_PANIC_THRESHOLD {
            dispatcher.record_render_panic("tracked");
        }
        dispatcher.prepare_all(&root, &mut caches, &Theme::Dark);
        assert!(dispatcher.is_poisoned(0));

        // cleanup_all should skip poisoned extensions.
        dispatcher.cleanup_all(&mut caches);
        assert!(tracker.lock().unwrap().is_empty());
    }

    // -- reset ----------------------------------------------------------------

    #[test]
    fn reset_clears_node_map_and_caches() {
        let ext = TestExtension::new(vec!["sparkline"], "charts");
        let mut dispatcher = ExtensionDispatcher::new(vec![Box::new(ext)]);
        let mut caches = ExtensionCaches::new();

        // Register a node and insert cache data.
        let mut root = make_node("root", "column");
        root.children.push(make_node("s1", "sparkline"));
        dispatcher.prepare_all(&root, &mut caches, &Theme::Dark);
        caches.insert("charts", "s1", 42u32);
        assert!(caches.contains("charts", "s1"));

        // reset() should clean up everything.
        dispatcher.reset(&mut caches);

        assert!(!caches.contains("charts", "s1"));
        assert!(!dispatcher.is_poisoned(0));
        // After reset, the dispatcher should not track any nodes.
        // Verify by checking that handle_event returns PassThrough.
        let result = dispatcher.handle_event("s1", "click", &Value::Null, &mut caches);
        assert!(matches!(result, EventResult::PassThrough));
    }

    #[test]
    fn reset_clears_poisoned_state() {
        let ext = TestExtension::new(vec!["sparkline"], "charts");
        let mut dispatcher = ExtensionDispatcher::new(vec![Box::new(ext)]);
        let mut caches = ExtensionCaches::new();

        // Poison the extension.
        for _ in 0..RENDER_PANIC_THRESHOLD {
            dispatcher.record_render_panic("sparkline");
        }
        let root = make_node("root", "column");
        dispatcher.prepare_all(&root, &mut caches, &Theme::Dark);
        assert!(dispatcher.is_poisoned(0));

        // reset() should clear poisoned state.
        dispatcher.reset(&mut caches);
        assert!(!dispatcher.is_poisoned(0));
    }

    // -- Full poison lifecycle (render panics -> poisoned -> clear) -----------

    /// Extension that panics on render.
    struct PanickingRenderExtension;

    impl WidgetExtension for PanickingRenderExtension {
        fn type_names(&self) -> &[&str] {
            &["panicky_render"]
        }
        fn config_key(&self) -> &str {
            "panicky_render"
        }
        fn render<'a>(&self, _node: &'a TreeNode, _env: &WidgetEnv<'a>) -> Element<'a, Message> {
            panic!("render goes boom");
        }
    }

    #[test]
    fn poison_lifecycle_render_panics_then_clear() {
        let ext: Box<dyn WidgetExtension> = Box::new(PanickingRenderExtension);
        let mut dispatcher = ExtensionDispatcher::new(vec![ext]);
        let mut caches = ExtensionCaches::new();
        let images = crate::image_registry::ImageRegistry::new();
        let theme = Theme::Dark;

        // Register the node.
        let mut root = make_node("root", "column");
        root.children.push(make_node("pr1", "panicky_render"));
        dispatcher.prepare_all(&root, &mut caches, &theme);

        // 1) Extension should not be poisoned yet.
        assert!(!dispatcher.is_poisoned(0));

        // 2) Record RENDER_PANIC_THRESHOLD render panics.
        //    In real usage, catch_unwind in widgets::render calls
        //    record_render_panic. We simulate the same sequence.
        for i in 0..RENDER_PANIC_THRESHOLD {
            let at_threshold = dispatcher.record_render_panic("panicky_render");
            if i < RENDER_PANIC_THRESHOLD - 1 {
                assert!(!at_threshold, "should not be at threshold yet (i={i})");
            } else {
                assert!(at_threshold, "should be at threshold now");
            }
        }

        // 3) prepare_all triggers the poisoning check.
        dispatcher.prepare_all(&root, &mut caches, &theme);
        assert!(
            dispatcher.is_poisoned(0),
            "extension should be poisoned after threshold + prepare_all"
        );

        // 4) Verify the poisoned extension renders a placeholder via the
        //    dispatcher (returns Some with red error text, not a panic).
        let node = make_node("pr1", "panicky_render");
        {
            let widget_caches = crate::widgets::WidgetCaches::new();
            let ctx = RenderCtx {
                caches: &widget_caches,
                images: &images,
                theme: &theme,
                extensions: &dispatcher,
                default_text_size: None,
                default_font: None,
                window_id: "",
                scale_factor: 1.0,
            };
            let env = WidgetEnv {
                caches: &caches,
                ctx,
            };
            let result = dispatcher.render(&node, &env);
            assert!(
                result.is_some(),
                "poisoned extension should still return Some (placeholder)"
            );
        } // borrows released here

        // 5) clear_poisoned simulates what happens on Snapshot.
        dispatcher.clear_poisoned();
        assert!(
            !dispatcher.is_poisoned(0),
            "poison should be cleared after clear_poisoned"
        );

        // 6) After clearing, the extension can render again (will panic
        //    again in this test, but the point is it's no longer skipped).
        //    We verify by checking that render() actually calls the
        //    extension (which panics) rather than returning the placeholder.
        //    We use catch_unwind to contain the panic.
        let widget_caches2 = crate::widgets::WidgetCaches::new();
        let ctx2 = RenderCtx {
            caches: &widget_caches2,
            images: &images,
            theme: &theme,
            extensions: &dispatcher,
            default_text_size: None,
            default_font: None,
            window_id: "",
            scale_factor: 1.0,
        };
        let env2 = WidgetEnv {
            caches: &caches,
            ctx: ctx2,
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            dispatcher.render(&node, &env2)
        }));
        assert!(
            result.is_err(),
            "after clearing poison, render should call the extension again (which panics)"
        );
    }

    // -- new_instance ---------------------------------------------------------

    /// Extension that implements new_instance() for session cloning.
    struct CloneableExtension {
        label: &'static str,
    }

    impl CloneableExtension {
        fn new(label: &'static str) -> Self {
            Self { label }
        }
    }

    impl WidgetExtension for CloneableExtension {
        fn type_names(&self) -> &[&str] {
            &["cloneable_widget"]
        }
        fn config_key(&self) -> &str {
            "cloneable"
        }
        fn render<'a>(&self, _node: &'a TreeNode, _env: &WidgetEnv<'a>) -> Element<'a, Message> {
            use iced::widget::text;
            text(self.label).into()
        }
        fn new_instance(&self) -> Box<dyn WidgetExtension> {
            Box::new(CloneableExtension::new(self.label))
        }
    }

    #[test]
    fn new_instance_default_panics() {
        let ext = TestExtension::new(vec!["sparkline"], "charts");
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
            ext.new_instance();
        }));
        assert!(result.is_err(), "default new_instance() should panic");
    }

    #[test]
    fn new_instance_custom_returns_fresh_instance() {
        let ext = CloneableExtension::new("original");
        let fresh = ext.new_instance();
        assert_eq!(fresh.type_names(), &["cloneable_widget"]);
        assert_eq!(fresh.config_key(), "cloneable");
    }

    #[test]
    fn clone_for_session_uses_new_instance() {
        let ext = CloneableExtension::new("session");
        let dispatcher = ExtensionDispatcher::new(vec![Box::new(ext)]);
        let cloned = dispatcher
            .clone_for_session()
            .expect("clone should succeed");
        assert!(cloned.handles_type("cloneable_widget"));
        assert_eq!(cloned.len(), 1);
    }

    #[test]
    fn clone_for_session_returns_err_on_panic() {
        let ext = TestExtension::new(vec!["sparkline"], "charts");
        let dispatcher = ExtensionDispatcher::new(vec![Box::new(ext)]);
        let result = dispatcher.clone_for_session();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.contains("charts"),
            "error should name the extension: {err}"
        );
    }
}
