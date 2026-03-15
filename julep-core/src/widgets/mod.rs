#[cfg(feature = "widget-canvas")]
mod canvas;
mod display;
mod helpers;
mod input;
mod interactive;
mod layout;
mod table;
#[cfg(debug_assertions)]
mod validate;
use helpers::*;

// Re-alias iced canvas to avoid shadowing by the `canvas` submodule.
#[cfg(any(feature = "widget-canvas", feature = "widget-qr-code"))]
use iced::widget::canvas as iced_canvas;

use std::collections::{HashMap, HashSet};

use crate::protocol::TreeNode;
use iced::widget::keyed;
#[cfg(feature = "widget-markdown")]
use iced::widget::markdown;
use iced::widget::scrollable::Anchor;
use iced::widget::text::LineHeight;
use iced::widget::{
    button, checkbox, column, combo_box, container, grid, mouse_area, pane_grid, pick_list, pin,
    progress_bar, rich_text, row, rule, scrollable, sensor, slider, span, text, text_editor,
    text_input, toggler, tooltip, vertical_slider, Space, Stack,
};
#[allow(unused_imports)]
use iced::{
    alignment, font, keyboard, mouse, widget, Border, Color, ContentFit, Element, Fill, Font,
    Length, Padding, Pixels, Point, Radians, Rotation, Shadow, Size, Vector,
};
use serde_json::Value;
use std::cell::Cell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::Duration;

use crate::extensions::ExtensionDispatcher;
use crate::message::Message;

/// Maximum tree recursion depth for render, ensure_caches, and tree walks.
/// Prevents stack overflow from pathologically nested trees. Normal UI trees
/// rarely exceed 20-30 levels; 256 is generous.
const MAX_TREE_DEPTH: usize = 256;

// ---------------------------------------------------------------------------
// Widget caches
// ---------------------------------------------------------------------------

/// Bundles all per-widget caches into a single struct so render functions
/// don't need to thread 3+ separate HashMap parameters everywhere.
pub struct WidgetCaches {
    pub(crate) editor_contents: HashMap<String, text_editor::Content>,
    /// Tracks the hash of the last-synced "content" prop for each text_editor.
    /// Used to detect host-side prop changes without clobbering user edits.
    pub(crate) editor_content_hashes: HashMap<String, u64>,
    #[cfg(feature = "widget-markdown")]
    pub(crate) markdown_items: HashMap<String, (u64, Vec<markdown::Item>)>,
    pub(crate) combo_states: HashMap<String, combo_box::State<String>>,
    pub(crate) combo_options: HashMap<String, Vec<String>>,
    pub(crate) pane_grid_states: HashMap<String, pane_grid::State<String>>,
    /// Per-canvas, per-layer geometry caches. Outer key is node ID, inner key
    /// is layer name. The u64 is a content hash of the layer's shapes array --
    /// when it changes the cache is cleared so the layer re-tessellates.
    #[cfg(feature = "widget-canvas")]
    pub(crate) canvas_caches: HashMap<String, HashMap<String, (u64, iced_canvas::Cache)>>,
    /// Per-qr_code caches. Key is node ID, value is (content hash, canvas Cache).
    #[cfg(feature = "widget-qr-code")]
    pub(crate) qr_code_caches: HashMap<String, (u64, iced_canvas::Cache)>,
    pub(crate) default_text_size: Option<f32>,
    pub(crate) default_font: Option<Font>,
    pub extension: crate::extensions::ExtensionCaches,
}

impl Default for WidgetCaches {
    fn default() -> Self {
        Self::new()
    }
}

impl WidgetCaches {
    pub fn new() -> Self {
        Self {
            editor_contents: HashMap::new(),
            editor_content_hashes: HashMap::new(),
            #[cfg(feature = "widget-markdown")]
            markdown_items: HashMap::new(),
            combo_states: HashMap::new(),
            combo_options: HashMap::new(),
            pane_grid_states: HashMap::new(),
            #[cfg(feature = "widget-canvas")]
            canvas_caches: HashMap::new(),
            #[cfg(feature = "widget-qr-code")]
            qr_code_caches: HashMap::new(),
            default_text_size: None,
            default_font: None,
            extension: crate::extensions::ExtensionCaches::new(),
        }
    }

    pub fn clear(&mut self) {
        self.clear_builtin();
        self.extension.clear();
    }

    // -- Accessor methods for renderer crate --
    // Fields are pub(crate) to avoid leaking internal HashMap structure to
    // extension authors, but the renderer binary needs access to a few.

    /// Get a mutable reference to a text_editor Content by node ID.
    pub fn editor_content_mut(&mut self, id: &str) -> Option<&mut text_editor::Content> {
        self.editor_contents.get_mut(id)
    }

    /// Get a mutable reference to a pane_grid State by node ID.
    pub fn pane_grid_state_mut(&mut self, id: &str) -> Option<&mut pane_grid::State<String>> {
        self.pane_grid_states.get_mut(id)
    }

    /// Get an immutable reference to a pane_grid State by node ID.
    pub fn pane_grid_state(&self, id: &str) -> Option<&pane_grid::State<String>> {
        self.pane_grid_states.get(id)
    }

    /// Clear built-in widget caches without touching extension caches.
    ///
    /// Used by the Snapshot handler so that extension cleanup callbacks
    /// (via `ExtensionDispatcher::prepare_all`) can run before the
    /// extension cache entries are removed.
    pub fn clear_builtin(&mut self) {
        self.editor_contents.clear();
        self.editor_content_hashes.clear();
        #[cfg(feature = "widget-markdown")]
        self.markdown_items.clear();
        self.combo_states.clear();
        self.combo_options.clear();
        self.pane_grid_states.clear();
        #[cfg(feature = "widget-canvas")]
        self.canvas_caches.clear();
        #[cfg(feature = "widget-qr-code")]
        self.qr_code_caches.clear();
    }
}

// ---------------------------------------------------------------------------
// Cache pre-population
// ---------------------------------------------------------------------------

/// Walk the tree and ensure that every `text_editor`, `markdown`,
/// `combo_box`, `pane_grid`, `canvas`, and `qr_code` node has an entry in
/// the corresponding cache. This must be called *before* `render` so that
/// `render` can work with shared (`&`) references to the caches.
///
/// After populating caches, prunes stale entries for nodes no longer in the
/// tree across all cache types.
pub fn ensure_caches(node: &TreeNode, caches: &mut WidgetCaches) {
    let mut live_ids = HashSet::new();
    ensure_caches_walk(node, caches, &mut live_ids, 0);
    prune_all_stale_caches(&live_ids, caches);
}

/// Inner recursive walk: populate caches and collect live node IDs.
fn ensure_caches_walk(
    node: &TreeNode,
    caches: &mut WidgetCaches,
    live_ids: &mut HashSet<String>,
    depth: usize,
) {
    if depth > MAX_TREE_DEPTH {
        log::warn!(
            "[id={}] ensure_caches depth exceeds {MAX_TREE_DEPTH}, skipping subtree",
            node.id
        );
        return;
    }
    live_ids.insert(node.id.clone());

    match node.type_name.as_str() {
        "text_editor" => {
            let props = node.props.as_object();
            let content_str = prop_str(props, "content").unwrap_or_default();
            let prop_hash = hash_str(&content_str);
            let prev_hash = caches.editor_content_hashes.get(&node.id).copied();
            if prev_hash != Some(prop_hash) {
                // Host changed the content prop -- (re)create the Content.
                caches.editor_contents.insert(
                    node.id.clone(),
                    text_editor::Content::with_text(&content_str),
                );
                caches
                    .editor_content_hashes
                    .insert(node.id.clone(), prop_hash);
            }
            // If hash matches, Content is already initialized and we preserve
            // any user edits that happened since the last prop sync.
        }
        #[cfg(feature = "widget-markdown")]
        "markdown" => {
            let props = node.props.as_object();
            let content = prop_str(props, "content").unwrap_or_default();
            let hash = hash_str(&content);
            match caches.markdown_items.get(&node.id) {
                Some((existing_hash, _)) if *existing_hash == hash => {}
                _ => {
                    let items: Vec<_> = markdown::parse(&content).collect();
                    caches.markdown_items.insert(node.id.clone(), (hash, items));
                }
            }
        }
        "combo_box" => {
            let props = node.props.as_object();
            let options: Vec<String> = props
                .and_then(|p| p.get("options"))
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            let cached_options = caches.combo_options.get(&node.id);
            let options_changed = cached_options.is_none_or(|cached| *cached != options);
            if options_changed {
                caches
                    .combo_states
                    .insert(node.id.clone(), combo_box::State::new(options.clone()));
                caches.combo_options.insert(node.id.clone(), options);
            }
        }
        "pane_grid" => {
            let props = node.props.as_object();
            let axis = match prop_str(props, "split_axis").as_deref() {
                Some("horizontal") => pane_grid::Axis::Horizontal,
                _ => pane_grid::Axis::Vertical,
            };
            let child_ids: HashSet<String> = node.children.iter().map(|c| c.id.clone()).collect();

            if let Some(state) = caches.pane_grid_states.get_mut(&node.id) {
                // Prune panes whose child nodes no longer exist.
                let stale_panes: Vec<pane_grid::Pane> = state
                    .panes
                    .iter()
                    .filter(|(_pane, id)| !child_ids.contains(*id))
                    .map(|(pane, _id)| *pane)
                    .collect();
                for pane in stale_panes {
                    state.close(pane);
                }
                // Add panes for new children that don't have a pane yet.
                // Collect owned IDs to avoid holding an immutable borrow on
                // state.panes while we call state.split() (mutable).
                let existing_ids: HashSet<String> = state.panes.values().cloned().collect();
                let new_child_ids: Vec<String> = node
                    .children
                    .iter()
                    .filter(|c| !existing_ids.contains(&c.id))
                    .map(|c| c.id.clone())
                    .collect();
                for new_id in new_child_ids {
                    if let Some((&anchor, _)) = state.panes.iter().next() {
                        let _ = state.split(axis, anchor, new_id);
                    }
                }
            } else {
                let child_list: Vec<String> = node.children.iter().map(|c| c.id.clone()).collect();
                let new_state = if child_list.is_empty() {
                    let (state, _) = pane_grid::State::new("default".to_string());
                    state
                } else if child_list.len() == 1 {
                    let (state, _) = pane_grid::State::new(child_list[0].clone());
                    state
                } else {
                    let (mut state, first_pane) = pane_grid::State::new(child_list[0].clone());
                    let mut last_pane = first_pane;
                    for id in child_list.iter().skip(1) {
                        if let Some((new_pane, _)) = state.split(axis, last_pane, id.clone()) {
                            last_pane = new_pane;
                        }
                    }
                    state
                };
                caches.pane_grid_states.insert(node.id.clone(), new_state);
            }
        }
        #[cfg(feature = "widget-canvas")]
        "canvas" => {
            let props = node.props.as_object();
            // Build layer map: either from "layers" (object) or "shapes" (array -> single layer).
            let layer_map = canvas_layer_map(props);
            let node_caches = caches.canvas_caches.entry(node.id.clone()).or_default();

            // Update or create caches for each layer.
            for (layer_name, shapes_val) in &layer_map {
                let hash = {
                    let mut hasher = DefaultHasher::new();
                    hash_json_value(shapes_val, &mut hasher);
                    hasher.finish()
                };
                match node_caches.get_mut(layer_name) {
                    Some((existing_hash, cache)) => {
                        if *existing_hash != hash {
                            cache.clear();
                            // Update just the hash, keep the same cache object.
                            *existing_hash = hash;
                        }
                    }
                    None => {
                        node_caches.insert(layer_name.clone(), (hash, iced_canvas::Cache::new()));
                    }
                }
            }

            // Remove stale layers that are no longer in the tree.
            node_caches.retain(|name, _| layer_map.contains_key(name));
        }
        #[cfg(feature = "widget-qr-code")]
        "qr_code" => {
            let props = node.props.as_object();
            let data = prop_str(props, "data").unwrap_or_default();
            let cell_size = prop_f32(props, "cell_size").unwrap_or(4.0);
            let ec = prop_str(props, "error_correction").unwrap_or_default();
            // Hash data + cell_size + error_correction for cache invalidation.
            let mut hasher = DefaultHasher::new();
            data.hash(&mut hasher);
            cell_size.to_bits().hash(&mut hasher);
            ec.hash(&mut hasher);
            let hash = hasher.finish();

            match caches.qr_code_caches.get_mut(&node.id) {
                Some((existing_hash, cache)) => {
                    if *existing_hash != hash {
                        cache.clear();
                        // Update just the hash, keep the same cache object.
                        *existing_hash = hash;
                    }
                }
                None => {
                    caches
                        .qr_code_caches
                        .insert(node.id.clone(), (hash, iced_canvas::Cache::new()));
                }
            }
        }
        _ => {}
    }

    for child in &node.children {
        ensure_caches_walk(child, caches, live_ids, depth + 1);
    }
}

/// Prune all cache types, removing entries whose node IDs are no longer live.
fn prune_all_stale_caches(live_ids: &HashSet<String>, caches: &mut WidgetCaches) {
    caches.editor_contents.retain(|id, _| live_ids.contains(id));
    caches
        .editor_content_hashes
        .retain(|id, _| live_ids.contains(id));
    #[cfg(feature = "widget-markdown")]
    caches.markdown_items.retain(|id, _| live_ids.contains(id));
    caches.combo_states.retain(|id, _| live_ids.contains(id));
    caches.combo_options.retain(|id, _| live_ids.contains(id));
    caches
        .pane_grid_states
        .retain(|id, _| live_ids.contains(id));
    #[cfg(feature = "widget-canvas")]
    caches.canvas_caches.retain(|id, _| live_ids.contains(id));
    #[cfg(feature = "widget-qr-code")]
    caches.qr_code_caches.retain(|id, _| live_ids.contains(id));
}

// ---------------------------------------------------------------------------
// Main render dispatch
// ---------------------------------------------------------------------------

/// Map a TreeNode to an iced Element. Unknown types render as an empty container.
///
/// This is the immutable side of the ensure_caches/render split. All mutable
/// cache state (text_editor Content, markdown Items, combo_box State, canvas
/// Cache, etc.) must be pre-populated by [`ensure_caches`] before calling
/// this function. `render` works exclusively with shared (`&`) references
/// to caches, so it can run inside iced's `view()` which only has `&self`.
pub fn render<'a>(
    node: &'a TreeNode,
    caches: &'a WidgetCaches,
    images: &'a crate::image_registry::ImageRegistry,
    theme: &'a iced::Theme,
    dispatcher: &'a ExtensionDispatcher,
) -> Element<'a, Message> {
    // Track recursion depth via thread-local counter. Each call increments
    // on entry; the DepthGuard decrements on drop (including early returns).
    thread_local! {
        static RENDER_DEPTH: Cell<usize> = const { Cell::new(0) };
    }
    struct DepthGuard;
    impl Drop for DepthGuard {
        fn drop(&mut self) {
            RENDER_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
        }
    }

    let depth = RENDER_DEPTH.with(|d| {
        let new = d.get() + 1;
        d.set(new);
        new
    });
    let _guard = DepthGuard;

    if depth > MAX_TREE_DEPTH {
        log::warn!(
            "[id={}] render depth exceeds {MAX_TREE_DEPTH}, returning placeholder",
            node.id
        );
        return text("Max depth exceeded")
            .color(Color::from_rgb(1.0, 0.0, 0.0))
            .into();
    }

    #[cfg(debug_assertions)]
    validate::validate_props(node);

    let element = match node.type_name.as_str() {
        // Layout widgets
        "column" => layout::render_column(node, caches, images, theme, dispatcher),
        "row" => layout::render_row(node, caches, images, theme, dispatcher),
        "container" => layout::render_container(node, caches, images, theme, dispatcher),
        "stack" => layout::render_stack(node, caches, images, theme, dispatcher),
        "grid" => layout::render_grid(node, caches, images, theme, dispatcher),
        "pin" => layout::render_pin(node, caches, images, theme, dispatcher),
        "keyed_column" => layout::render_keyed_column(node, caches, images, theme, dispatcher),
        "float" => layout::render_float(node, caches, images, theme, dispatcher),
        "responsive" => layout::render_responsive(node, caches, images, theme, dispatcher),
        "scrollable" => layout::render_scrollable(node, caches, images, theme, dispatcher),
        "pane_grid" => layout::render_pane_grid(node, caches, images, theme, dispatcher),
        // Display widgets
        "text" => display::render_text(node, caches),
        "rich_text" | "rich" => display::render_rich_text(node, caches),
        "space" => display::render_space(node),
        "rule" => display::render_rule(node),
        "progress_bar" => display::render_progress_bar(node),
        #[cfg(feature = "widget-image")]
        "image" => display::render_image(node, images),
        #[cfg(not(feature = "widget-image"))]
        "image" => render_feature_disabled("image", "widget-image"),
        #[cfg(feature = "widget-svg")]
        "svg" => display::render_svg(node),
        #[cfg(not(feature = "widget-svg"))]
        "svg" => render_feature_disabled("svg", "widget-svg"),
        #[cfg(feature = "widget-markdown")]
        "markdown" => display::render_markdown(node, caches, theme),
        #[cfg(not(feature = "widget-markdown"))]
        "markdown" => render_feature_disabled("markdown", "widget-markdown"),
        #[cfg(feature = "widget-qr-code")]
        "qr_code" => display::render_qr_code(node, caches),
        #[cfg(not(feature = "widget-qr-code"))]
        "qr_code" => render_feature_disabled("qr_code", "widget-qr-code"),
        // Input widgets
        "text_input" => input::render_text_input(node, caches),
        "text_editor" => input::render_text_editor(node, caches),
        "checkbox" => input::render_checkbox(node, caches),
        "toggler" => input::render_toggler(node, caches),
        "radio" => input::render_radio(node, caches),
        "slider" => input::render_slider(node),
        "vertical_slider" => input::render_vertical_slider(node),
        "pick_list" => input::render_pick_list(node, caches),
        "combo_box" => input::render_combo_box(node, caches),
        // Interactive widgets
        "button" => interactive::render_button(node, caches, images, theme, dispatcher),
        "mouse_area" => interactive::render_mouse_area(node, caches, images, theme, dispatcher),
        "sensor" => interactive::render_sensor(node, caches, images, theme, dispatcher),
        "tooltip" => interactive::render_tooltip(node, caches, images, theme, dispatcher),
        "themer" => interactive::render_themer(node, caches, images, theme, dispatcher),
        "window" => interactive::render_window(node, caches, images, theme, dispatcher),
        "overlay" => interactive::render_overlay(node, caches, images, theme, dispatcher),
        // Canvas
        #[cfg(feature = "widget-canvas")]
        "canvas" => canvas::render_canvas(node, caches, images, theme, dispatcher),
        #[cfg(not(feature = "widget-canvas"))]
        "canvas" => render_feature_disabled("canvas", "widget-canvas"),
        // Table
        "table" => table::render_table(node),
        // Extension dispatch
        unknown => {
            if dispatcher.handles_type(unknown) {
                let render_ctx = crate::extensions::RenderContext {
                    caches,
                    images,
                    theme,
                    extensions: dispatcher,
                };
                let env = crate::extensions::WidgetEnv {
                    caches: &caches.extension,
                    images,
                    theme,
                    render_ctx,
                    default_text_size: caches.default_text_size,
                    default_font: caches.default_font,
                };
                // catch_unwind at the render boundary: extension panics produce
                // a red placeholder instead of crashing the renderer.
                // We track consecutive render panics via an atomic counter
                // on the dispatcher; after N consecutive panics, the
                // extension is poisoned on the next prepare_all cycle.
                if crate::extensions::catch_unwind_enabled() {
                    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        dispatcher.render(node, &env)
                    })) {
                        Ok(Some(element)) => element,
                        Ok(None) => container(Space::new()).into(),
                        Err(_) => {
                            let at_threshold = dispatcher.record_render_panic(unknown);
                            if at_threshold {
                                log::error!(
                                    "[id={}] extension for type `{unknown}` hit render panic \
                                     threshold, will be poisoned on next prepare cycle",
                                    node.id
                                );
                            } else {
                                log::error!("extension panicked in render for node `{}`", node.id);
                            }
                            iced::widget::text(format!(
                                "Extension error: type `{unknown}`, node `{}`",
                                node.id
                            ))
                            .color(iced::Color::from_rgb(1.0, 0.0, 0.0))
                            .into()
                        }
                    }
                } else {
                    match dispatcher.render(node, &env) {
                        Some(element) => element,
                        None => container(Space::new()).into(),
                    }
                }
            } else {
                log::warn!(
                    "[id={}] unknown node type `{unknown}`, rendering as empty container",
                    node.id
                );
                container(Space::new()).into()
            }
        }
    };

    #[cfg(feature = "a11y")]
    if let Some(overrides) = crate::a11y_widget::A11yOverrides::from_props(&node.props) {
        return crate::a11y_widget::A11yOverride::wrap(element, overrides).into();
    }

    element
}

#[cfg(not(all(
    feature = "widget-image",
    feature = "widget-svg",
    feature = "widget-canvas",
    feature = "widget-markdown",
    feature = "widget-qr-code"
)))]
fn render_feature_disabled<'a>(widget_name: &str, feature_name: &str) -> Element<'a, Message> {
    log::warn!(
        "widget `{widget_name}` requires feature `{feature_name}` -- \
         enable it with: cargo build --features {feature_name}"
    );
    iced::widget::text(format!(
        "Widget '{}' requires feature '{}'",
        widget_name, feature_name
    ))
    .into()
}

// ---------------------------------------------------------------------------
// Child rendering helper
// ---------------------------------------------------------------------------

fn render_children<'a>(
    node: &'a TreeNode,
    caches: &'a WidgetCaches,
    images: &'a crate::image_registry::ImageRegistry,
    theme: &'a iced::Theme,
    dispatcher: &'a ExtensionDispatcher,
) -> Vec<Element<'a, Message>> {
    node.children
        .iter()
        .map(|c| render(c, caches, images, theme, dispatcher))
        .collect()
}

// ---------------------------------------------------------------------------
// Canvas cache helpers (used by ensure_caches)
// ---------------------------------------------------------------------------

#[cfg(feature = "widget-canvas")]
/// Build a sorted layer map from canvas props. Supports two prop formats:
/// - `"layers"`: a JSON object mapping layer_name -> array of shapes (preferred)
/// - `"shapes"`: a flat JSON array of shapes (legacy, wrapped as a single "default" layer)
///
/// If both are present, `"layers"` wins. Returns a BTreeMap of references so
/// layer order is deterministic (alphabetical by name) without allocating
/// serialized strings.
fn canvas_layer_map(
    props: Option<&serde_json::Map<String, Value>>,
) -> std::collections::BTreeMap<String, &Value> {
    let mut map = std::collections::BTreeMap::new();

    if let Some(layers_obj) = props
        .and_then(|p| p.get("layers"))
        .and_then(|v| v.as_object())
    {
        for (name, shapes_val) in layers_obj {
            map.insert(name.clone(), shapes_val);
        }
    } else if let Some(shapes_arr) = props.and_then(|p| p.get("shapes")) {
        map.insert("default".to_string(), shapes_arr);
    }

    map
}

/// Hash a serde_json::Value recursively without allocating a serialized string.
/// Each variant is discriminated by a type tag byte to avoid collisions.
///
/// NOTE: DefaultHasher output is not stable across Rust versions or builds.
/// These hashes must never be persisted or compared across process restarts.
fn hash_json_value(v: &serde_json::Value, h: &mut impl std::hash::Hasher) {
    match v {
        serde_json::Value::Null => 0u8.hash(h),
        serde_json::Value::Bool(b) => {
            1u8.hash(h);
            b.hash(h);
        }
        serde_json::Value::Number(n) => {
            2u8.hash(h);
            if let Some(f) = n.as_f64() {
                f.to_bits().hash(h);
            } else if let Some(i) = n.as_i64() {
                i.hash(h);
            } else if let Some(u) = n.as_u64() {
                u.hash(h);
            }
        }
        serde_json::Value::String(s) => {
            3u8.hash(h);
            s.hash(h);
        }
        serde_json::Value::Array(arr) => {
            4u8.hash(h);
            arr.len().hash(h);
            for item in arr {
                hash_json_value(item, h);
            }
        }
        serde_json::Value::Object(obj) => {
            5u8.hash(h);
            obj.len().hash(h);
            for (k, v) in obj {
                k.hash(h);
                hash_json_value(v, h);
            }
        }
    }
}

/// Hash a string using DefaultHasher for same-process cache invalidation.
/// NOTE: DefaultHasher output is not stable across Rust versions or builds.
/// These hashes must never be persisted or compared across process restarts.
fn hash_str(s: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- WidgetCaches --

    #[test]
    fn widget_caches_new_is_empty() {
        let c = WidgetCaches::new();
        assert!(c.editor_contents.is_empty());
        #[cfg(feature = "widget-markdown")]
        assert!(c.markdown_items.is_empty());
        assert!(c.combo_states.is_empty());
        assert!(c.combo_options.is_empty());
        assert!(c.pane_grid_states.is_empty());
        assert!(c.default_text_size.is_none());
        assert!(c.default_font.is_none());
    }

    #[test]
    fn widget_caches_clear_empties_maps_but_preserves_defaults() {
        let mut c = WidgetCaches::new();
        c.default_text_size = Some(14.0);
        c.default_font = Some(Font::MONOSPACE);
        c.combo_options.insert("x".into(), vec!["a".into()]);
        c.clear();
        assert!(c.combo_options.is_empty());
        assert_eq!(c.default_text_size, Some(14.0));
        assert_eq!(c.default_font, Some(Font::MONOSPACE));
    }

    // -- Image registry handle lookup --

    #[cfg(feature = "widget-image")]
    #[test]
    fn image_registry_handle_lookup() {
        use crate::image_registry::ImageRegistry;

        let mut registry = ImageRegistry::new();
        // Minimal valid 1x1 RGB PNG.
        let png_bytes: Vec<u8> = vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // signature
            0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR chunk
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, // 1x1
            0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, // 8-bit RGB
            0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, // IDAT
            0x54, 0x08, 0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01, 0xE2,
            0x21, 0xBC, 0x33, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, // IEND
            0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        registry.create_from_bytes("test_sprite".to_string(), png_bytes);
        assert!(
            registry.get("test_sprite").is_some(),
            "registered handle should be retrievable"
        );
        assert!(
            registry.get("nonexistent").is_none(),
            "unregistered name should return None"
        );
    }

    // -- clear_builtin vs clear --

    #[test]
    fn clear_builtin_preserves_extension_caches() {
        let mut caches = WidgetCaches::new();

        // Add a built-in cache entry and an extension cache entry.
        caches
            .editor_contents
            .insert("ed1".to_string(), iced::widget::text_editor::Content::new());
        caches.extension.insert("ext", "key".to_string(), 42u32);

        caches.clear_builtin();

        // Built-in caches should be empty.
        assert!(caches.editor_contents.is_empty());
        // Extension caches should survive.
        assert_eq!(caches.extension.get::<u32>("ext", "key"), Some(&42));
    }

    #[test]
    fn clear_wipes_both_builtin_and_extension() {
        let mut caches = WidgetCaches::new();

        caches
            .editor_contents
            .insert("ed1".to_string(), iced::widget::text_editor::Content::new());
        caches.extension.insert("ext", "key".to_string(), 42u32);

        caches.clear();

        assert!(caches.editor_contents.is_empty());
        assert!(!caches.extension.contains("ext", "key"));
    }

    // -- hash_json_value --

    #[test]
    fn hash_json_value_same_input_same_hash() {
        use std::collections::hash_map::DefaultHasher;

        let val = serde_json::json!({"shapes": [{"type": "rect", "x": 0, "y": 0}]});
        let h1 = {
            let mut h = DefaultHasher::new();
            hash_json_value(&val, &mut h);
            h.finish()
        };
        let h2 = {
            let mut h = DefaultHasher::new();
            hash_json_value(&val, &mut h);
            h.finish()
        };
        assert_eq!(h1, h2);
    }

    #[test]
    fn hash_json_value_different_input_different_hash() {
        use std::collections::hash_map::DefaultHasher;

        let a = serde_json::json!({"type": "rect"});
        let b = serde_json::json!({"type": "circle"});
        let hash_a = {
            let mut h = DefaultHasher::new();
            hash_json_value(&a, &mut h);
            h.finish()
        };
        let hash_b = {
            let mut h = DefaultHasher::new();
            hash_json_value(&b, &mut h);
            h.finish()
        };
        assert_ne!(hash_a, hash_b);
    }

    #[test]
    fn hash_json_value_type_discrimination() {
        use std::collections::hash_map::DefaultHasher;

        // null, false, and 0 should produce different hashes
        let vals = [
            serde_json::json!(null),
            serde_json::json!(false),
            serde_json::json!(0),
            serde_json::json!(""),
            serde_json::json!([]),
            serde_json::json!({}),
        ];
        let hashes: Vec<u64> = vals
            .iter()
            .map(|v| {
                let mut h = DefaultHasher::new();
                hash_json_value(v, &mut h);
                h.finish()
            })
            .collect();

        // All should be distinct
        for (i, h1) in hashes.iter().enumerate() {
            for (j, h2) in hashes.iter().enumerate() {
                if i != j {
                    assert_ne!(h1, h2, "type {i} and {j} should hash differently");
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Render smoke tests -- verify render() doesn't panic for common types
    // -----------------------------------------------------------------------

    use crate::extensions::ExtensionDispatcher;
    use crate::image_registry::ImageRegistry;
    use crate::protocol::TreeNode;

    fn smoke_node(id: &str, type_name: &str, props: serde_json::Value) -> TreeNode {
        TreeNode {
            id: id.to_string(),
            type_name: type_name.to_string(),
            props,
            children: vec![],
        }
    }

    fn smoke_node_with_children(
        id: &str,
        type_name: &str,
        props: serde_json::Value,
        children: Vec<TreeNode>,
    ) -> TreeNode {
        TreeNode {
            id: id.to_string(),
            type_name: type_name.to_string(),
            props,
            children,
        }
    }

    fn smoke_text_child() -> TreeNode {
        smoke_node("child", "text", serde_json::json!({"content": "hi"}))
    }

    #[test]
    fn render_smoke_text() {
        let node = smoke_node("t", "text", serde_json::json!({"content": "hello"}));
        let caches = WidgetCaches::new();
        let images = ImageRegistry::new();
        let theme = iced::Theme::Dark;
        let dispatcher = ExtensionDispatcher::default();
        let _elem = render(&node, &caches, &images, &theme, &dispatcher);
    }

    #[test]
    fn render_smoke_column_empty() {
        let node = smoke_node("c", "column", serde_json::json!({}));
        let caches = WidgetCaches::new();
        let images = ImageRegistry::new();
        let theme = iced::Theme::Dark;
        let dispatcher = ExtensionDispatcher::default();
        let _elem = render(&node, &caches, &images, &theme, &dispatcher);
    }

    #[test]
    fn render_smoke_row_empty() {
        let node = smoke_node("r", "row", serde_json::json!({}));
        let caches = WidgetCaches::new();
        let images = ImageRegistry::new();
        let theme = iced::Theme::Dark;
        let dispatcher = ExtensionDispatcher::default();
        let _elem = render(&node, &caches, &images, &theme, &dispatcher);
    }

    #[test]
    fn render_smoke_container_with_child() {
        let node = smoke_node_with_children(
            "ct",
            "container",
            serde_json::json!({}),
            vec![smoke_text_child()],
        );
        let caches = WidgetCaches::new();
        let images = ImageRegistry::new();
        let theme = iced::Theme::Dark;
        let dispatcher = ExtensionDispatcher::default();
        let _elem = render(&node, &caches, &images, &theme, &dispatcher);
    }

    #[test]
    fn render_smoke_button_with_child() {
        let node = smoke_node_with_children(
            "btn",
            "button",
            serde_json::json!({}),
            vec![smoke_text_child()],
        );
        let caches = WidgetCaches::new();
        let images = ImageRegistry::new();
        let theme = iced::Theme::Dark;
        let dispatcher = ExtensionDispatcher::default();
        let _elem = render(&node, &caches, &images, &theme, &dispatcher);
    }

    #[test]
    fn render_smoke_checkbox() {
        let node = smoke_node(
            "cb",
            "checkbox",
            serde_json::json!({"label": "Accept", "checked": true}),
        );
        let caches = WidgetCaches::new();
        let images = ImageRegistry::new();
        let theme = iced::Theme::Dark;
        let dispatcher = ExtensionDispatcher::default();
        let _elem = render(&node, &caches, &images, &theme, &dispatcher);
    }

    #[test]
    fn render_smoke_space() {
        let node = smoke_node("sp", "space", serde_json::json!({}));
        let caches = WidgetCaches::new();
        let images = ImageRegistry::new();
        let theme = iced::Theme::Dark;
        let dispatcher = ExtensionDispatcher::default();
        let _elem = render(&node, &caches, &images, &theme, &dispatcher);
    }

    #[test]
    fn render_smoke_rule() {
        let node = smoke_node("rl", "rule", serde_json::json!({"direction": "horizontal"}));
        let caches = WidgetCaches::new();
        let images = ImageRegistry::new();
        let theme = iced::Theme::Dark;
        let dispatcher = ExtensionDispatcher::default();
        let _elem = render(&node, &caches, &images, &theme, &dispatcher);
    }

    #[test]
    fn render_smoke_progress_bar() {
        let node = smoke_node(
            "pb",
            "progress_bar",
            serde_json::json!({"value": 50.0, "min": 0.0, "max": 100.0}),
        );
        let caches = WidgetCaches::new();
        let images = ImageRegistry::new();
        let theme = iced::Theme::Dark;
        let dispatcher = ExtensionDispatcher::default();
        let _elem = render(&node, &caches, &images, &theme, &dispatcher);
    }

    #[test]
    fn render_smoke_slider() {
        let node = smoke_node(
            "sl",
            "slider",
            serde_json::json!({"min": 0.0, "max": 100.0, "value": 50.0}),
        );
        let caches = WidgetCaches::new();
        let images = ImageRegistry::new();
        let theme = iced::Theme::Dark;
        let dispatcher = ExtensionDispatcher::default();
        let _elem = render(&node, &caches, &images, &theme, &dispatcher);
    }

    #[test]
    fn render_smoke_text_input() {
        let node = smoke_node(
            "ti",
            "text_input",
            serde_json::json!({"placeholder": "Type here", "value": ""}),
        );
        let caches = WidgetCaches::new();
        let images = ImageRegistry::new();
        let theme = iced::Theme::Dark;
        let dispatcher = ExtensionDispatcher::default();
        let _elem = render(&node, &caches, &images, &theme, &dispatcher);
    }

    #[test]
    fn render_smoke_toggler() {
        let node = smoke_node("tg", "toggler", serde_json::json!({"is_toggled": false}));
        let caches = WidgetCaches::new();
        let images = ImageRegistry::new();
        let theme = iced::Theme::Dark;
        let dispatcher = ExtensionDispatcher::default();
        let _elem = render(&node, &caches, &images, &theme, &dispatcher);
    }

    #[test]
    fn render_smoke_stack_empty() {
        let node = smoke_node("st", "stack", serde_json::json!({}));
        let caches = WidgetCaches::new();
        let images = ImageRegistry::new();
        let theme = iced::Theme::Dark;
        let dispatcher = ExtensionDispatcher::default();
        let _elem = render(&node, &caches, &images, &theme, &dispatcher);
    }

    // -----------------------------------------------------------------------
    // Error path tests -- unknown type and missing props
    // -----------------------------------------------------------------------

    #[test]
    fn render_unknown_type_returns_element_without_panic() {
        let node = smoke_node("unk", "definitely_not_a_widget", serde_json::json!({}));
        let caches = WidgetCaches::new();
        let images = ImageRegistry::new();
        let theme = iced::Theme::Dark;
        let dispatcher = ExtensionDispatcher::default();
        // Should produce the empty container fallback, not panic.
        let _elem = render(&node, &caches, &images, &theme, &dispatcher);
    }

    #[test]
    fn render_text_input_missing_props_does_not_panic() {
        let node = smoke_node("ti_empty", "text_input", serde_json::json!({}));
        let caches = WidgetCaches::new();
        let images = ImageRegistry::new();
        let theme = iced::Theme::Dark;
        let dispatcher = ExtensionDispatcher::default();
        let _elem = render(&node, &caches, &images, &theme, &dispatcher);
    }
}
