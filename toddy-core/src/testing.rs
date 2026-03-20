//! Test factory helpers for widget extension authors.
//!
//! Provides [`TestEnv`] for setting up a render environment and
//! [`node`] / [`node_with_props`] / [`node_with_children`] for
//! constructing test tree nodes.
//!
//! # Example
//!
//! ```ignore
//! use toddy_core::testing::*;
//! use toddy_core::prelude::*;
//!
//! let test = TestEnv::default();
//! let env = test.env();
//! let element = my_extension.render(&node, &env);
//! ```

use iced::Theme;
use serde_json::{Value, json};

use crate::extensions::{ExtensionCaches, ExtensionDispatcher, RenderCtx, WidgetEnv};
use crate::image_registry::ImageRegistry;
use crate::protocol::TreeNode;
use crate::widgets::WidgetCaches;

// ---------------------------------------------------------------------------
// TreeNode constructors
// ---------------------------------------------------------------------------

/// Create a minimal [`TreeNode`] with empty props and no children.
pub fn node(id: &str, type_name: &str) -> TreeNode {
    TreeNode {
        id: id.to_string(),
        type_name: type_name.to_string(),
        props: json!({}),
        children: vec![],
    }
}

/// Create a [`TreeNode`] with the given props and no children.
pub fn node_with_props(id: &str, type_name: &str, props: Value) -> TreeNode {
    TreeNode {
        id: id.to_string(),
        type_name: type_name.to_string(),
        props,
        children: vec![],
    }
}

/// Create a [`TreeNode`] with children and empty props.
pub fn node_with_children(id: &str, type_name: &str, children: Vec<TreeNode>) -> TreeNode {
    TreeNode {
        id: id.to_string(),
        type_name: type_name.to_string(),
        props: json!({}),
        children,
    }
}

/// Create a [`TreeNode`] with both props and children.
///
/// For container-type extension widgets that need both configuration
/// (via props) and nested content (via children).
pub fn node_with_props_and_children(
    id: &str,
    type_name: &str,
    props: Value,
    children: Vec<TreeNode>,
) -> TreeNode {
    TreeNode {
        id: id.to_string(),
        type_name: type_name.to_string(),
        props,
        children,
    }
}

// ---------------------------------------------------------------------------
// TestEnv: owns all render dependencies
// ---------------------------------------------------------------------------

/// Owns all the dependencies needed to construct a [`WidgetEnv`] for
/// testing extension `render()` methods.
///
/// All fields are public so tests can customize before calling [`env`](Self::env).
///
/// # Example
///
/// ```ignore
/// let test = TestEnv::default();
/// let env = test.env();
/// let element = my_extension.render(&node, &env);
/// ```
///
/// With customization:
///
/// ```ignore
/// let test = TestEnv {
///     theme: Theme::Light,
///     ..TestEnv::default()
/// };
/// let env = test.env();
/// ```
pub struct TestEnv {
    pub ext_caches: ExtensionCaches,
    pub widget_caches: WidgetCaches,
    pub images: ImageRegistry,
    pub theme: Theme,
    pub dispatcher: ExtensionDispatcher,
    pub default_text_size: Option<f32>,
    pub default_font: Option<iced::Font>,
}

impl Default for TestEnv {
    fn default() -> Self {
        Self {
            ext_caches: ExtensionCaches::new(),
            widget_caches: WidgetCaches::new(),
            images: ImageRegistry::new(),
            theme: Theme::Dark,
            dispatcher: ExtensionDispatcher::new(vec![]),
            default_text_size: None,
            default_font: None,
        }
    }
}

impl TestEnv {
    /// Build a [`RenderCtx`] from the owned test state.
    pub fn render_ctx(&self) -> RenderCtx<'_> {
        RenderCtx {
            caches: &self.widget_caches,
            images: &self.images,
            theme: &self.theme,
            extensions: &self.dispatcher,
            default_text_size: self.default_text_size,
            default_font: self.default_font,
            window_id: "",
            scale_factor: 1.0,
        }
    }

    /// Borrow a [`WidgetEnv`] using an externally-held [`RenderCtx`].
    ///
    /// Usage:
    /// ```ignore
    /// let test = TestEnv::default();
    /// let ctx = test.render_ctx();
    /// let env = test.env(&ctx);
    /// let element = my_extension.render(&node, &env);
    /// ```
    pub fn env<'a>(&'a self, ctx: &RenderCtx<'a>) -> WidgetEnv<'a> {
        WidgetEnv {
            caches: &self.ext_caches,
            ctx: *ctx,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::GenerationCounter;
    use crate::prop_helpers::{prop_f32, prop_str};

    // -- TreeNode constructors ------------------------------------------------

    #[test]
    fn node_has_empty_props_and_no_children() {
        let n = node("btn-1", "button");
        assert_eq!(n.id, "btn-1");
        assert_eq!(n.type_name, "button");
        assert!(n.children.is_empty());
        assert_eq!(n.props, json!({}));
    }

    #[test]
    fn node_with_props_stores_props() {
        let n = node_with_props("txt-1", "text", json!({"content": "hello", "size": 14}));
        assert_eq!(n.props["content"], "hello");
        assert_eq!(n.props["size"], 14);
    }

    #[test]
    fn node_with_children_stores_children() {
        let children = vec![node("a", "text"), node("b", "button")];
        let n = node_with_children("col-1", "column", children);
        assert_eq!(n.children.len(), 2);
        assert_eq!(n.children[0].id, "a");
        assert_eq!(n.children[1].id, "b");
    }

    #[test]
    fn node_props_work_with_prop_helpers() {
        let n = node_with_props("s-1", "sparkline", json!({"label": "cpu", "max": 100.0}));
        let props = n.props.as_object();
        assert_eq!(prop_str(props, "label"), Some("cpu".to_string()));
        assert!((prop_f32(props, "max").unwrap() - 100.0).abs() < 0.001);
    }

    // -- TestEnv --------------------------------------------------------------

    #[test]
    fn default_env_has_no_text_defaults() {
        let test = TestEnv::default();
        let ctx = test.render_ctx();
        let env = test.env(&ctx);
        assert!(env.default_text_size().is_none());
        assert!(env.default_font().is_none());
    }

    #[test]
    fn default_env_has_empty_state() {
        let test = TestEnv::default();
        let ctx = test.render_ctx();
        let env = test.env(&ctx);
        assert!(!env.caches.contains("test", "anything"));
        assert!(ctx.extensions.is_empty());
    }

    #[test]
    fn env_inherits_text_defaults() {
        let test = TestEnv {
            default_text_size: Some(18.0),
            default_font: Some(iced::Font::MONOSPACE),
            ..TestEnv::default()
        };

        let ctx = test.render_ctx();
        let env = test.env(&ctx);
        assert_eq!(env.default_text_size(), Some(18.0));
        assert_eq!(env.default_font(), Some(iced::Font::MONOSPACE));
    }

    #[test]
    fn env_theme_is_customizable() {
        let test = TestEnv {
            theme: Theme::Light,
            ..TestEnv::default()
        };
        let ctx = test.render_ctx();
        let _env = test.env(&ctx);
    }

    // -- GenerationCounter in ExtensionCaches ---------------------------------

    #[test]
    fn generation_counter_lifecycle() {
        let mut counter = GenerationCounter::new();
        assert_eq!(counter.get(), 0);
        counter.bump();
        counter.bump();
        assert_eq!(counter.get(), 2);
    }

    #[test]
    fn generation_counter_in_caches() {
        let mut test = TestEnv::default();
        test.ext_caches
            .insert("spark", "spark-1:gen", GenerationCounter::new());

        let counter = test
            .ext_caches
            .get_mut::<GenerationCounter>("spark", "spark-1:gen")
            .unwrap();
        counter.bump();

        let counter = test
            .ext_caches
            .get::<GenerationCounter>("spark", "spark-1:gen")
            .unwrap();
        assert_eq!(counter.get(), 1);
    }
}
