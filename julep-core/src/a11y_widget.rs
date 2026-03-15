//! Accessibility override widget.
//!
//! Wraps a child widget to intercept [`operate`] calls and apply
//! Elixir-side accessibility overrides (role, label, description, etc.)
//! to the accessibility tree. When `hidden` is set, the widget and all
//! its descendants are removed from the accessibility tree while
//! remaining visible and interactive for sighted users.
//!
//! [`operate`]: iced::advanced::widget::Widget::operate

use crate::message::Message;

use iced::advanced::layout::{self, Layout};
use iced::advanced::overlay;
use iced::advanced::renderer;
use iced::advanced::widget::operation::accessible::{self, Accessible};
use iced::advanced::widget::{self, Widget};
use iced::advanced::{Clipboard, Shell};
use iced::{Element, Event, Length, Rectangle, Size, Vector};
use serde_json::Value;

// ---------------------------------------------------------------------------
// A11yOverrides: parsed from the `a11y` JSON prop
// ---------------------------------------------------------------------------

/// Accessibility overrides parsed from the `a11y` JSON prop.
///
/// When present on a node, wraps the child widget in an [`A11yOverride`]
/// that intercepts [`operate`] to apply these overrides to the
/// accessibility tree.
///
/// [`operate`]: iced::advanced::widget::Widget::operate
pub(crate) struct A11yOverrides {
    /// Semantic role override.
    pub role: Option<accessible::Role>,
    /// Human-readable name override.
    pub label: Option<String>,
    /// Longer description override.
    pub description: Option<String>,
    /// When true, the widget is hidden from the accessibility tree.
    pub hidden: bool,
    /// Expanded state override for collapsible sections.
    pub expanded: Option<bool>,
    /// Whether the widget is required (e.g. a required form field).
    pub required: bool,
    /// Heading level (1--6) for widgets with [`Role::Heading`].
    pub level: Option<usize>,
    /// Live region urgency override.
    pub live: Option<accessible::Live>,
}

impl A11yOverrides {
    /// Parse accessibility overrides from a node's props.
    ///
    /// Returns `None` if no `a11y` key exists, meaning no wrapping is
    /// needed.
    pub fn from_props(props: &Value) -> Option<Self> {
        let a11y = props.get("a11y")?;

        let role = a11y
            .get("role")
            .and_then(|v| v.as_str())
            .and_then(parse_role);

        let label = a11y.get("label").and_then(|v| v.as_str()).map(String::from);

        let description = a11y
            .get("description")
            .and_then(|v| v.as_str())
            .map(String::from);

        let hidden = a11y
            .get("hidden")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let expanded = a11y.get("expanded").and_then(|v| v.as_bool());

        let required = a11y
            .get("required")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let level = a11y.get("level").and_then(|v| v.as_u64()).and_then(|n| {
            let n = n as usize;
            if (1..=6).contains(&n) {
                Some(n)
            } else {
                None
            }
        });

        let live = a11y
            .get("live")
            .and_then(|v| v.as_str())
            .and_then(parse_live);

        Some(Self {
            role,
            label,
            description,
            hidden,
            expanded,
            required,
            level,
            live,
        })
    }
}

/// Parse a role string into an [`accessible::Role`].
///
/// Covers all variants of the iced `Role` enum using lowercase string
/// matching. Returns `None` for unrecognised strings.
fn parse_role(s: &str) -> Option<accessible::Role> {
    let role = match s {
        "alert" => accessible::Role::Alert,
        "alert_dialog" | "alertdialog" => accessible::Role::AlertDialog,
        "button" => accessible::Role::Button,
        "canvas" => accessible::Role::Canvas,
        "checkbox" | "check_box" => accessible::Role::CheckBox,
        "combo_box" | "combobox" => accessible::Role::ComboBox,
        "dialog" => accessible::Role::Dialog,
        "document" => accessible::Role::Document,
        "group" | "generic_container" | "generic" | "container" => accessible::Role::Group,
        "heading" => accessible::Role::Heading,
        "image" => accessible::Role::Image,
        "label" => accessible::Role::Label,
        "link" => accessible::Role::Link,
        "list" => accessible::Role::List,
        "list_item" => accessible::Role::ListItem,
        "menu" => accessible::Role::Menu,
        "menu_bar" => accessible::Role::MenuBar,
        "menu_item" => accessible::Role::MenuItem,
        "meter" => accessible::Role::Meter,
        "multiline_text_input" | "text_editor" => accessible::Role::MultilineTextInput,
        "navigation" => accessible::Role::Navigation,
        "progress_indicator" | "progressbar" => accessible::Role::ProgressIndicator,
        "radio" | "radio_button" => accessible::Role::RadioButton,
        "region" => accessible::Role::Region,
        "scrollbar" | "scroll_bar" => accessible::Role::ScrollBar,
        "scroll_view" => accessible::Role::ScrollView,
        "search" => accessible::Role::Search,
        "separator" => accessible::Role::Separator,
        "slider" => accessible::Role::Slider,
        "static_text" => accessible::Role::StaticText,
        "status" => accessible::Role::Status,
        "switch" => accessible::Role::Switch,
        "tab" => accessible::Role::Tab,
        "tab_list" => accessible::Role::TabList,
        "tab_panel" => accessible::Role::TabPanel,
        "table" => accessible::Role::Table,
        "text_input" => accessible::Role::TextInput,
        "toolbar" => accessible::Role::Toolbar,
        "tooltip" => accessible::Role::Tooltip,
        "tree" => accessible::Role::Tree,
        "tree_item" => accessible::Role::TreeItem,
        "window" => accessible::Role::Window,
        _ => return None,
    };
    Some(role)
}

/// Parse a live-region urgency string into [`accessible::Live`].
fn parse_live(s: &str) -> Option<accessible::Live> {
    match s {
        "polite" => Some(accessible::Live::Polite),
        "assertive" => Some(accessible::Live::Assertive),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// A11yOverride: transparent single-child wrapper widget
// ---------------------------------------------------------------------------

/// A widget that wraps a single child and intercepts [`operate`] to
/// apply accessibility overrides from the Elixir-side `a11y` prop.
///
/// All methods except [`operate`] delegate directly to the child.
///
/// [`operate`]: Widget::operate
pub(crate) struct A11yOverride<'a> {
    child: Element<'a, Message>,
    overrides: A11yOverrides,
}

impl<'a> A11yOverride<'a> {
    /// Wrap `child` with the given accessibility overrides.
    pub(crate) fn wrap(child: Element<'a, Message>, overrides: A11yOverrides) -> Self {
        Self { child, overrides }
    }
}

impl Widget<Message, iced::Theme, iced::Renderer> for A11yOverride<'_> {
    fn children(&self) -> Vec<widget::Tree> {
        vec![widget::Tree::new(&self.child)]
    }

    fn diff(&self, tree: &mut widget::Tree) {
        tree.diff_children(&[self.child.as_widget()]);
    }

    fn size(&self) -> Size<Length> {
        self.child.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.child.as_widget().size_hint()
    }

    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.child
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn draw(
        &self,
        tree: &widget::Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: iced::mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.child.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn update(
        &mut self,
        tree: &mut widget::Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: iced::mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &widget::Tree,
        layout: Layout<'_>,
        cursor: iced::mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> iced::mouse::Interaction {
        self.child.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut widget::Tree,
        layout: Layout<'b>,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        self.child.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }

    fn operate(
        &mut self,
        tree: &mut widget::Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        if self.overrides.hidden {
            let mut interceptor = HiddenInterceptor { inner: operation };
            self.child.as_widget_mut().operate(
                &mut tree.children[0],
                layout,
                renderer,
                &mut interceptor,
            );
        } else {
            let mut interceptor = A11yInterceptor {
                inner: operation,
                overrides: &self.overrides,
            };
            self.child.as_widget_mut().operate(
                &mut tree.children[0],
                layout,
                renderer,
                &mut interceptor,
            );
        }
    }
}

impl<'a> From<A11yOverride<'a>> for Element<'a, Message> {
    fn from(wrapper: A11yOverride<'a>) -> Self {
        Element::new(wrapper)
    }
}

// ---------------------------------------------------------------------------
// A11yInterceptor: applies overrides to accessible() calls
// ---------------------------------------------------------------------------

/// An [`Operation`] wrapper that intercepts [`accessible`] calls to
/// apply the configured overrides before forwarding to the inner
/// operation.
///
/// [`accessible`]: widget::Operation::accessible
struct A11yInterceptor<'a, 'b> {
    inner: &'a mut dyn widget::Operation,
    overrides: &'b A11yOverrides,
}

impl widget::Operation for A11yInterceptor<'_, '_> {
    fn accessible(
        &mut self,
        id: Option<&widget::Id>,
        bounds: Rectangle,
        accessible: &Accessible<'_>,
    ) {
        // Build a new Accessible with overrides applied. The new struct
        // borrows label/description from self.overrides (owned Strings)
        // for the duration of this call.
        let overridden = Accessible {
            role: self.overrides.role.unwrap_or(accessible.role),
            label: self.overrides.label.as_deref().or(accessible.label),
            description: self
                .overrides
                .description
                .as_deref()
                .or(accessible.description),
            value: accessible.value,
            disabled: accessible.disabled,
            toggled: accessible.toggled,
            selected: accessible.selected,
            expanded: self.overrides.expanded.or(accessible.expanded),
            live: self.overrides.live.or(accessible.live),
            level: self.overrides.level.or(accessible.level),
            required: self.overrides.required || accessible.required,
            labelled_by: accessible.labelled_by,
            described_by: accessible.described_by,
        };
        self.inner.accessible(id, bounds, &overridden);
    }

    fn container(&mut self, id: Option<&widget::Id>, bounds: Rectangle) {
        // If overrides specify a role, label, or description, upgrade this
        // container to an accessible node so the overrides take effect.
        // Without this, container-type widgets (column, row, stack, etc.)
        // would silently ignore a11y overrides because they only call
        // container(), never accessible().
        if self.overrides.role.is_some()
            || self.overrides.label.is_some()
            || self.overrides.description.is_some()
        {
            let base = Accessible {
                role: self.overrides.role.unwrap_or_default(),
                label: self.overrides.label.as_deref(),
                description: self.overrides.description.as_deref(),
                expanded: self.overrides.expanded,
                live: self.overrides.live,
                level: self.overrides.level,
                required: self.overrides.required,
                ..Accessible::default()
            };
            self.inner.accessible(id, bounds, &base);
        } else {
            self.inner.container(id, bounds);
        }
    }

    fn text(&mut self, id: Option<&widget::Id>, bounds: Rectangle, text: &str) {
        self.inner.text(id, bounds, text);
    }

    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn widget::Operation)) {
        // Overrides apply only to the direct child; grandchildren pass
        // through to the inner operation unmodified.
        self.inner.traverse(operate);
    }

    fn focusable(
        &mut self,
        id: Option<&widget::Id>,
        bounds: Rectangle,
        state: &mut dyn widget::operation::focusable::Focusable,
    ) {
        self.inner.focusable(id, bounds, state);
    }

    fn scrollable(
        &mut self,
        id: Option<&widget::Id>,
        bounds: Rectangle,
        content_bounds: Rectangle,
        translation: Vector,
        state: &mut dyn widget::operation::scrollable::Scrollable,
    ) {
        self.inner
            .scrollable(id, bounds, content_bounds, translation, state);
    }

    fn text_input(
        &mut self,
        id: Option<&widget::Id>,
        bounds: Rectangle,
        state: &mut dyn widget::operation::text_input::TextInput,
    ) {
        self.inner.text_input(id, bounds, state);
    }

    fn custom(
        &mut self,
        id: Option<&widget::Id>,
        bounds: Rectangle,
        state: &mut dyn std::any::Any,
    ) {
        self.inner.custom(id, bounds, state);
    }

    fn finish(&self) -> widget::operation::Outcome<()> {
        self.inner.finish()
    }
}

// ---------------------------------------------------------------------------
// HiddenInterceptor: drops accessible() calls entirely
// ---------------------------------------------------------------------------

/// An [`Operation`] wrapper that suppresses all accessibility-related
/// calls, hiding the widget and its descendants from the accessibility
/// tree. Non-accessibility operations (focus, scroll, text input) are
/// forwarded normally so the widget remains interactive for sighted
/// users.
struct HiddenInterceptor<'a> {
    inner: &'a mut dyn widget::Operation,
}

impl widget::Operation for HiddenInterceptor<'_> {
    fn accessible(
        &mut self,
        _id: Option<&widget::Id>,
        _bounds: Rectangle,
        _accessible: &Accessible<'_>,
    ) {
        // Intentionally dropped -- hidden from AT.
    }

    fn container(&mut self, _id: Option<&widget::Id>, _bounds: Rectangle) {
        // Intentionally dropped -- hide container from AT tree.
    }

    fn text(&mut self, _id: Option<&widget::Id>, _bounds: Rectangle, _text: &str) {
        // Intentionally dropped -- hide text from AT tree.
    }

    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn widget::Operation)) {
        // Propagate suppression through the entire subtree.
        self.inner.traverse(&mut |inner_op| {
            let mut nested = HiddenInterceptor { inner: inner_op };
            operate(&mut nested);
        });
    }

    fn focusable(
        &mut self,
        id: Option<&widget::Id>,
        bounds: Rectangle,
        state: &mut dyn widget::operation::focusable::Focusable,
    ) {
        self.inner.focusable(id, bounds, state);
    }

    fn scrollable(
        &mut self,
        id: Option<&widget::Id>,
        bounds: Rectangle,
        content_bounds: Rectangle,
        translation: Vector,
        state: &mut dyn widget::operation::scrollable::Scrollable,
    ) {
        self.inner
            .scrollable(id, bounds, content_bounds, translation, state);
    }

    fn text_input(
        &mut self,
        id: Option<&widget::Id>,
        bounds: Rectangle,
        state: &mut dyn widget::operation::text_input::TextInput,
    ) {
        self.inner.text_input(id, bounds, state);
    }

    fn custom(
        &mut self,
        id: Option<&widget::Id>,
        bounds: Rectangle,
        state: &mut dyn std::any::Any,
    ) {
        self.inner.custom(id, bounds, state);
    }

    fn finish(&self) -> widget::operation::Outcome<()> {
        self.inner.finish()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn from_props_none_when_no_a11y() {
        let props = json!({"label": "Click me"});
        assert!(A11yOverrides::from_props(&props).is_none());
    }

    #[test]
    fn from_props_parses_label() {
        let props = json!({"a11y": {"label": "Close dialog"}});
        let overrides = A11yOverrides::from_props(&props).unwrap();
        assert_eq!(overrides.label.as_deref(), Some("Close dialog"));
        assert!(!overrides.hidden);
        assert!(overrides.role.is_none());
    }

    #[test]
    fn from_props_parses_role() {
        let props = json!({"a11y": {"role": "heading"}});
        let overrides = A11yOverrides::from_props(&props).unwrap();
        assert_eq!(overrides.role, Some(accessible::Role::Heading));
    }

    #[test]
    fn from_props_parses_hidden() {
        let props = json!({"a11y": {"hidden": true}});
        let overrides = A11yOverrides::from_props(&props).unwrap();
        assert!(overrides.hidden);
    }

    #[test]
    fn from_props_parses_all_fields() {
        let props = json!({
            "a11y": {
                "role": "alert",
                "label": "Error message",
                "description": "Something went wrong",
                "hidden": false,
                "expanded": true,
                "required": true,
                "level": 2,
                "live": "assertive"
            }
        });
        let overrides = A11yOverrides::from_props(&props).unwrap();
        assert_eq!(overrides.role, Some(accessible::Role::Alert));
        assert_eq!(overrides.label.as_deref(), Some("Error message"));
        assert_eq!(
            overrides.description.as_deref(),
            Some("Something went wrong")
        );
        assert!(!overrides.hidden);
        assert_eq!(overrides.expanded, Some(true));
        assert!(overrides.required);
        assert_eq!(overrides.level, Some(2));
        assert_eq!(overrides.live, Some(accessible::Live::Assertive));
    }

    #[test]
    fn parse_role_mapping() {
        // Spot-check representative roles from each category.
        assert_eq!(parse_role("button"), Some(accessible::Role::Button));
        assert_eq!(parse_role("heading"), Some(accessible::Role::Heading));
        assert_eq!(parse_role("checkbox"), Some(accessible::Role::CheckBox));
        assert_eq!(parse_role("check_box"), Some(accessible::Role::CheckBox));
        assert_eq!(parse_role("slider"), Some(accessible::Role::Slider));
        assert_eq!(parse_role("text_input"), Some(accessible::Role::TextInput));
        assert_eq!(parse_role("combo_box"), Some(accessible::Role::ComboBox));
        assert_eq!(parse_role("combobox"), Some(accessible::Role::ComboBox));
        assert_eq!(
            parse_role("alert_dialog"),
            Some(accessible::Role::AlertDialog)
        );
        assert_eq!(
            parse_role("alertdialog"),
            Some(accessible::Role::AlertDialog)
        );
        assert_eq!(parse_role("navigation"), Some(accessible::Role::Navigation));
        assert_eq!(parse_role("separator"), Some(accessible::Role::Separator));
        assert_eq!(
            parse_role("static_text"),
            Some(accessible::Role::StaticText)
        );
        assert_eq!(parse_role("scroll_bar"), Some(accessible::Role::ScrollBar));
        assert_eq!(parse_role("window"), Some(accessible::Role::Window));
        assert_eq!(parse_role("table"), Some(accessible::Role::Table));
        assert_eq!(parse_role("unknown_thing"), None);
    }

    #[test]
    fn parse_live_mapping() {
        assert_eq!(parse_live("polite"), Some(accessible::Live::Polite));
        assert_eq!(parse_live("assertive"), Some(accessible::Live::Assertive));
        assert_eq!(parse_live("off"), None);
        assert_eq!(parse_live(""), None);
    }

    #[test]
    fn from_props_ignores_invalid_level() {
        let props = json!({"a11y": {"level": 0}});
        let overrides = A11yOverrides::from_props(&props).unwrap();
        assert!(overrides.level.is_none());

        let props = json!({"a11y": {"level": 7}});
        let overrides = A11yOverrides::from_props(&props).unwrap();
        assert!(overrides.level.is_none());
    }

    #[test]
    fn from_props_valid_levels() {
        for n in 1..=6 {
            let props = json!({"a11y": {"level": n}});
            let overrides = A11yOverrides::from_props(&props).unwrap();
            assert_eq!(overrides.level, Some(n as usize));
        }
    }

    #[test]
    fn container_upgrade_when_role_overridden() {
        let props = json!({"a11y": {"role": "navigation", "label": "Main nav"}});
        let overrides = A11yOverrides::from_props(&props).unwrap();
        assert_eq!(overrides.role, Some(accessible::Role::Navigation));
        assert_eq!(overrides.label.as_deref(), Some("Main nav"));
        // The actual interception test would require an Operation mock,
        // but the parsing confirms the override would trigger the
        // container-to-accessible upgrade (role.is_some()).
    }
}
