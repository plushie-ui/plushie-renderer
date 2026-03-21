//! Accessibility override widget.
//!
//! Wraps a child widget to intercept [`operate`] calls and apply
//! host-side accessibility overrides (role, label, description, etc.)
//! to the accessibility tree. When `hidden` is set, the widget and all
//! its descendants are removed from the accessibility tree while
//! remaining visible and interactive for sighted users.
//!
//! [`operate`]: iced::advanced::widget::Widget::operate

use crate::message::Message;

use iced::advanced::Shell;
use iced::advanced::layout::{self, Layout};
use iced::advanced::overlay;
use iced::advanced::renderer;
use iced::advanced::widget::operation::accessible::{self, Accessible};
use iced::advanced::widget::{self, Widget};
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
#[derive(Debug, Clone, Default)]
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
    /// Whether the widget is busy (loading/processing).
    pub busy: bool,
    /// Whether the widget's value is invalid (form validation).
    pub invalid: bool,
    /// Whether this dialog is modal (restricts AT navigation).
    pub modal: bool,
    /// Whether the widget is read-only (viewable but not editable).
    pub read_only: bool,
    /// Keyboard mnemonic (Alt+letter shortcut).
    pub mnemonic: Option<char>,
    /// Toggle state for custom checkbox/switch widgets.
    pub toggled: Option<bool>,
    /// Selection state for custom radio/tab widgets.
    pub selected: Option<bool>,
    /// Text value announced by assistive technology.
    pub value: Option<String>,
    /// Widget orientation (horizontal or vertical).
    pub orientation: Option<accessible::Orientation>,
    /// Another widget that provides this widget's label.
    pub labelled_by: Option<widget::Id>,
    /// Another widget that provides this widget's description.
    pub described_by: Option<widget::Id>,
    /// A widget that describes why the value is invalid.
    ///
    /// To wire up error state for form fields, the host should:
    /// 1. Set `a11y.invalid = true` on the input widget when validation fails
    /// 2. Render an error message as a separate text node with a stable ID
    /// 3. Set `a11y.error_message = "<error-text-node-id>"` on the input
    ///
    /// This causes AT to announce the error text when the user focuses the
    /// invalid field. The `invalid` and `error_message` fields work
    /// together: `invalid` marks the field's state, `error_message`
    /// provides the explanation.
    pub error_message: Option<widget::Id>,
    /// Whether the widget is disabled (not interactive).
    ///
    /// Overrides the widget-native disabled state when `Some`. Unlike
    /// bool-OR fields (required, busy), this replaces the base value
    /// so the host can explicitly enable or disable a widget.
    pub disabled: Option<bool>,
    /// Position of this item in a set (1-based).
    ///
    /// Used for list items, radio buttons, tabs, and similar ordered
    /// collections so assistive technology can announce "item 3 of 5".
    pub position_in_set: Option<usize>,
    /// Total number of items in the set containing this item.
    ///
    /// Paired with `position_in_set` to give AT full context about
    /// the item's position within its group.
    pub size_of_set: Option<usize>,
    /// The type of popup this widget triggers when activated.
    ///
    /// Tells AT what kind of popup to expect (listbox, menu, dialog,
    /// tree, or grid) so it can adjust navigation accordingly.
    pub has_popup: Option<accessible::HasPopup>,
}

impl A11yOverrides {
    /// Parse accessibility overrides from a node's props.
    ///
    /// Returns `None` if no `a11y` key exists or if the `a11y` object
    /// contains no meaningful overrides.
    pub fn from_props(props: &Value) -> Option<Self> {
        Self::from_a11y_value(props.get("a11y")?)
    }

    /// Parse accessibility overrides from an `a11y` JSON value directly.
    ///
    /// Like [`from_props`](Self::from_props) but takes the `a11y` value
    /// itself, avoiding the need to wrap it in a parent object. Used by
    /// canvas interactive shapes where the `a11y` field is nested inside
    /// the `interactive` object.
    pub fn from_a11y_value(a11y: &Value) -> Option<Self> {
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
            if (1..=6).contains(&n) { Some(n) } else { None }
        });

        let live = a11y
            .get("live")
            .and_then(|v| v.as_str())
            .and_then(parse_live);

        let busy = a11y.get("busy").and_then(|v| v.as_bool()).unwrap_or(false);

        let invalid = a11y
            .get("invalid")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let modal = a11y.get("modal").and_then(|v| v.as_bool()).unwrap_or(false);

        let read_only = a11y
            .get("read_only")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let mnemonic = a11y
            .get("mnemonic")
            .and_then(|v| v.as_str())
            .and_then(|s| s.chars().next());

        let toggled = a11y.get("toggled").and_then(|v| v.as_bool());

        let selected = a11y.get("selected").and_then(|v| v.as_bool());

        let value = a11y.get("value").and_then(|v| v.as_str()).map(String::from);

        let orientation = a11y
            .get("orientation")
            .and_then(|v| v.as_str())
            .and_then(parse_orientation);

        let labelled_by = a11y
            .get("labelled_by")
            .and_then(|v| v.as_str())
            .map(|s| widget::Id::from(s.to_owned()));

        let described_by = a11y
            .get("described_by")
            .and_then(|v| v.as_str())
            .map(|s| widget::Id::from(s.to_owned()));

        let error_message = a11y
            .get("error_message")
            .and_then(|v| v.as_str())
            .map(|s| widget::Id::from(s.to_owned()));

        let disabled = a11y.get("disabled").and_then(|v| v.as_bool());

        let position_in_set = a11y
            .get("position_in_set")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);

        let size_of_set = a11y
            .get("size_of_set")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);

        let has_popup = a11y
            .get("has_popup")
            .and_then(|v| v.as_str())
            .and_then(parse_has_popup);

        let result = Self {
            role,
            label,
            description,
            hidden,
            expanded,
            required,
            level,
            live,
            busy,
            invalid,
            modal,
            read_only,
            mnemonic,
            toggled,
            selected,
            value,
            orientation,
            labelled_by,
            described_by,
            error_message,
            disabled,
            position_in_set,
            size_of_set,
            has_popup,
        };

        // Only wrap when there's something to do.
        if result.hidden || result.has_overrides() {
            Some(result)
        } else {
            None
        }
    }

    /// Returns true if any override would affect the accessible node.
    ///
    /// Excludes `hidden` which is handled separately (subtree
    /// suppression rather than property override).
    pub(crate) fn has_overrides(&self) -> bool {
        self.role.is_some()
            || self.label.is_some()
            || self.description.is_some()
            || self.expanded.is_some()
            || self.live.is_some()
            || self.level.is_some()
            || self.mnemonic.is_some()
            || self.required
            || self.busy
            || self.invalid
            || self.modal
            || self.read_only
            || self.toggled.is_some()
            || self.selected.is_some()
            || self.value.is_some()
            || self.orientation.is_some()
            || self.labelled_by.is_some()
            || self.described_by.is_some()
            || self.error_message.is_some()
            || self.disabled.is_some()
            || self.position_in_set.is_some()
            || self.size_of_set.is_some()
            || self.has_popup.is_some()
    }

    /// Merge these overrides into a base [`Accessible`], returning a
    /// new struct with override values taking precedence.
    ///
    /// - `Option` fields: override wins if `Some`, falls back to base.
    /// - `bool` fields: OR-ed (override enables, never disables).
    fn apply_to<'a>(&'a self, base: &Accessible<'a>) -> Accessible<'a> {
        let value_override = self.value.as_deref().map(accessible::Value::Text);

        Accessible {
            role: self.role.unwrap_or(base.role),
            label: self.label.as_deref().or(base.label),
            description: self.description.as_deref().or(base.description),
            expanded: self.expanded.or(base.expanded),
            live: self.live.or(base.live),
            level: self.level.or(base.level),
            required: self.required || base.required,
            busy: self.busy || base.busy,
            invalid: self.invalid || base.invalid,
            modal: self.modal || base.modal,
            read_only: self.read_only || base.read_only,
            mnemonic: self.mnemonic.or(base.mnemonic),
            toggled: self.toggled.or(base.toggled),
            selected: self.selected.or(base.selected),
            value: value_override.or(base.value),
            orientation: self.orientation.or(base.orientation),
            labelled_by: self.labelled_by.as_ref().or(base.labelled_by),
            described_by: self.described_by.as_ref().or(base.described_by),
            error_message: self.error_message.as_ref().or(base.error_message),
            disabled: self.disabled.unwrap_or(base.disabled),
            position_in_set: self.position_in_set.or(base.position_in_set),
            size_of_set: self.size_of_set.or(base.size_of_set),
            has_popup: self.has_popup.or(base.has_popup),
            // `hidden` is intentionally omitted -- it's handled at the
            // interception layer (subtree suppression) rather than as a
            // property merge. See the operate() and traverse() methods.
            ..base.clone()
        }
    }

    /// Build an [`Accessible`] from overrides alone, using defaults
    /// for all widget-internal fields.
    ///
    /// Used when upgrading a container (which normally has no accessible
    /// node) to an accessible node because the host set a11y overrides.
    pub(crate) fn to_accessible(&self) -> Accessible<'_> {
        self.apply_to(&Accessible::default())
    }

    /// Create overrides with just a description (for placeholder auto-inference).
    pub(crate) fn with_description(description: String) -> Self {
        Self {
            description: Some(description),
            ..Self::default()
        }
    }
}

/// Parse a role string into an [`accessible::Role`].
///
/// Covers all variants of the iced `Role` enum using lowercase string
/// matching. Returns `None` for unrecognised strings.
///
/// **Maintenance note:** When new variants are added to iced's
/// `accessible::Role` enum (in the toddy-iced fork), they must be
/// manually added here with appropriate string aliases. There is no
/// compile-time exhaustiveness check since this maps from strings.
fn parse_role(s: &str) -> Option<accessible::Role> {
    // One canonical underscore form per role. Semantic aliases (different
    // names for the same concept) are allowed where they map to toddy
    // widget names. No spelling variants (no concatenated forms).
    let role = match s {
        "alert" => accessible::Role::Alert,
        "alert_dialog" => accessible::Role::AlertDialog,
        "button" => accessible::Role::Button,
        "canvas" => accessible::Role::Canvas,
        "check_box" => accessible::Role::CheckBox,
        "combo_box" => accessible::Role::ComboBox,
        "dialog" => accessible::Role::Dialog,
        "document" => accessible::Role::Document,
        "group" => accessible::Role::Group,
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
        // "progress_bar" alias: matches toddy's widget name.
        "progress_indicator" | "progress_bar" => accessible::Role::ProgressIndicator,
        // "radio" alias: universally understood short form.
        "radio_button" | "radio" => accessible::Role::RadioButton,
        "region" => accessible::Role::Region,
        "scroll_bar" => accessible::Role::ScrollBar,
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
        // "row"/"cell" aliases: short forms for table contexts.
        "table_row" | "row" => accessible::Role::Row,
        "table_cell" | "cell" => accessible::Role::Cell,
        "column_header" => accessible::Role::ColumnHeader,
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

/// Parse an orientation string into [`accessible::Orientation`].
fn parse_orientation(s: &str) -> Option<accessible::Orientation> {
    match s {
        "horizontal" => Some(accessible::Orientation::Horizontal),
        "vertical" => Some(accessible::Orientation::Vertical),
        _ => None,
    }
}

/// Parse a has-popup type string into [`accessible::HasPopup`].
fn parse_has_popup(s: &str) -> Option<accessible::HasPopup> {
    match s {
        "listbox" => Some(accessible::HasPopup::Listbox),
        "menu" => Some(accessible::HasPopup::Menu),
        "dialog" => Some(accessible::HasPopup::Dialog),
        "tree" => Some(accessible::HasPopup::Tree),
        "grid" => Some(accessible::HasPopup::Grid),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// A11yOverride: transparent single-child wrapper widget
// ---------------------------------------------------------------------------

/// A widget that wraps a single child and intercepts [`operate`] to
/// apply accessibility overrides from the host-side `a11y` prop.
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
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.child.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
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

impl<'a> From<A11yOverride<'a>> for Element<'a, Message> {
    fn from(wrapper: A11yOverride<'a>) -> Self {
        Element::new(wrapper)
    }
}

// ---------------------------------------------------------------------------
// A11yInterceptor: intercepts accessible/container calls
// ---------------------------------------------------------------------------

/// An [`Operation`] wrapper that intercepts accessibility-related calls
/// to apply overrides or suppress them entirely (when hidden).
///
/// - `accessible()`: merges host overrides with widget-declared values.
/// - `container()`: upgrades to an accessible node when overrides are set.
/// - When `hidden`: drops accessible/container/text calls for the entire
///   subtree while forwarding non-a11y operations normally.
struct A11yInterceptor<'a, 'b> {
    inner: &'a mut dyn widget::Operation,
    overrides: &'b A11yOverrides,
}

/// Forwards non-intercepted [`Operation`] methods to `self.inner`.
/// Centralises the delegation so it only needs updating in one place
/// if iced adds new methods to the trait.
macro_rules! forward_operation {
    () => {
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
    };
}

impl widget::Operation for A11yInterceptor<'_, '_> {
    fn accessible(
        &mut self,
        id: Option<&widget::Id>,
        bounds: Rectangle,
        accessible: &Accessible<'_>,
    ) {
        if self.overrides.hidden {
            return; // Drop -- hidden from AT.
        }
        let overridden = self.overrides.apply_to(accessible);
        self.inner.accessible(id, bounds, &overridden);
    }

    fn container(&mut self, id: Option<&widget::Id>, bounds: Rectangle) {
        if self.overrides.hidden {
            return; // Drop -- hidden from AT.
        }
        if self.overrides.has_overrides() {
            // Upgrade container to accessible node so overrides take
            // effect. Without this, container-type widgets (column, row,
            // etc.) would silently ignore a11y overrides because they
            // only call container(), never accessible().
            let node = self.overrides.to_accessible();
            self.inner.accessible(id, bounds, &node);
        } else {
            self.inner.container(id, bounds);
        }
    }

    fn text(&mut self, id: Option<&widget::Id>, bounds: Rectangle, text: &str) {
        if self.overrides.hidden {
            return; // Drop -- hidden from AT.
        }
        self.inner.text(id, bounds, text);
    }

    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn widget::Operation)) {
        if self.overrides.hidden {
            // Propagate suppression through the entire subtree.
            self.inner.traverse(&mut |inner_op| {
                let mut nested = A11yInterceptor {
                    inner: inner_op,
                    overrides: self.overrides,
                };
                operate(&mut nested);
            });
        } else {
            // Overrides apply only to the direct child; grandchildren
            // pass through to the inner operation unmodified.
            self.inner.traverse(operate);
        }
    }

    forward_operation!();
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use iced::advanced::widget::Operation;
    use serde_json::json;

    // -- from_props -----------------------------------------------------------

    #[test]
    fn from_props_none_when_no_a11y() {
        let props = json!({"label": "Click me"});
        assert!(A11yOverrides::from_props(&props).is_none());
    }

    #[test]
    fn from_props_none_when_empty_a11y() {
        let props = json!({"a11y": {}});
        assert!(A11yOverrides::from_props(&props).is_none());
    }

    #[test]
    fn from_props_none_when_all_defaults() {
        let props = json!({"a11y": {"hidden": false, "required": false}});
        assert!(A11yOverrides::from_props(&props).is_none());
    }

    #[test]
    fn from_props_parses_label() {
        let overrides = A11yOverrides::from_props(&json!({"a11y": {"label": "Close"}})).unwrap();
        assert_eq!(overrides.label.as_deref(), Some("Close"));
    }

    #[test]
    fn from_props_parses_role() {
        let overrides = A11yOverrides::from_props(&json!({"a11y": {"role": "heading"}})).unwrap();
        assert_eq!(overrides.role, Some(accessible::Role::Heading));
    }

    #[test]
    fn from_props_parses_hidden() {
        let overrides = A11yOverrides::from_props(&json!({"a11y": {"hidden": true}})).unwrap();
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
                "live": "assertive",
                "busy": true,
                "invalid": true,
                "modal": true,
                "read_only": true,
                "mnemonic": "E",
                "toggled": true,
                "selected": false,
                "value": "42%",
                "orientation": "vertical",
                "labelled_by": "label-id",
                "described_by": "desc-id",
                "error_message": "err-id",
                "disabled": true,
                "position_in_set": 3,
                "size_of_set": 10,
                "has_popup": "menu"
            }
        });
        let o = A11yOverrides::from_props(&props).unwrap();
        assert_eq!(o.role, Some(accessible::Role::Alert));
        assert_eq!(o.label.as_deref(), Some("Error message"));
        assert_eq!(o.description.as_deref(), Some("Something went wrong"));
        assert!(!o.hidden);
        assert_eq!(o.expanded, Some(true));
        assert!(o.required);
        assert_eq!(o.level, Some(2));
        assert_eq!(o.live, Some(accessible::Live::Assertive));
        assert!(o.busy);
        assert!(o.invalid);
        assert!(o.modal);
        assert!(o.read_only);
        assert_eq!(o.mnemonic, Some('E'));
        assert_eq!(o.toggled, Some(true));
        assert_eq!(o.selected, Some(false));
        assert_eq!(o.value.as_deref(), Some("42%"));
        assert_eq!(o.orientation, Some(accessible::Orientation::Vertical));
        assert!(o.labelled_by.is_some());
        assert!(o.described_by.is_some());
        assert!(o.error_message.is_some());
        assert_eq!(o.disabled, Some(true));
        assert_eq!(o.position_in_set, Some(3));
        assert_eq!(o.size_of_set, Some(10));
        assert_eq!(o.has_popup, Some(accessible::HasPopup::Menu));
    }

    #[test]
    fn from_a11y_value_parses_directly() {
        let a11y = json!({"role": "button", "label": "Save", "disabled": true});
        let result = A11yOverrides::from_a11y_value(&a11y).unwrap();
        assert_eq!(result.role, Some(accessible::Role::Button));
        assert_eq!(result.label.as_deref(), Some("Save"));
        assert_eq!(result.disabled, Some(true));
    }

    // -- parse helpers --------------------------------------------------------

    #[test]
    fn parse_role_covers_all_variants() {
        // Canonical underscore forms for every role.
        let cases = [
            ("alert", accessible::Role::Alert),
            ("alert_dialog", accessible::Role::AlertDialog),
            ("button", accessible::Role::Button),
            ("canvas", accessible::Role::Canvas),
            ("check_box", accessible::Role::CheckBox),
            ("combo_box", accessible::Role::ComboBox),
            ("dialog", accessible::Role::Dialog),
            ("document", accessible::Role::Document),
            ("group", accessible::Role::Group),
            ("heading", accessible::Role::Heading),
            ("image", accessible::Role::Image),
            ("label", accessible::Role::Label),
            ("link", accessible::Role::Link),
            ("list", accessible::Role::List),
            ("list_item", accessible::Role::ListItem),
            ("menu", accessible::Role::Menu),
            ("menu_bar", accessible::Role::MenuBar),
            ("menu_item", accessible::Role::MenuItem),
            ("meter", accessible::Role::Meter),
            ("multiline_text_input", accessible::Role::MultilineTextInput),
            ("navigation", accessible::Role::Navigation),
            ("progress_indicator", accessible::Role::ProgressIndicator),
            ("radio_button", accessible::Role::RadioButton),
            ("region", accessible::Role::Region),
            ("scroll_bar", accessible::Role::ScrollBar),
            ("scroll_view", accessible::Role::ScrollView),
            ("search", accessible::Role::Search),
            ("separator", accessible::Role::Separator),
            ("slider", accessible::Role::Slider),
            ("static_text", accessible::Role::StaticText),
            ("status", accessible::Role::Status),
            ("switch", accessible::Role::Switch),
            ("tab", accessible::Role::Tab),
            ("tab_list", accessible::Role::TabList),
            ("tab_panel", accessible::Role::TabPanel),
            ("table", accessible::Role::Table),
            ("table_row", accessible::Role::Row),
            ("table_cell", accessible::Role::Cell),
            ("column_header", accessible::Role::ColumnHeader),
            ("text_input", accessible::Role::TextInput),
            ("toolbar", accessible::Role::Toolbar),
            ("tooltip", accessible::Role::Tooltip),
            ("tree", accessible::Role::Tree),
            ("tree_item", accessible::Role::TreeItem),
            ("window", accessible::Role::Window),
        ];
        for (input, expected) in cases {
            assert_eq!(parse_role(input), Some(expected), "parse_role({input:?})");
        }
        // Semantic aliases (different names, not spelling variants).
        assert_eq!(parse_role("radio"), Some(accessible::Role::RadioButton));
        assert_eq!(
            parse_role("text_editor"),
            Some(accessible::Role::MultilineTextInput)
        );
        assert_eq!(
            parse_role("progress_bar"),
            Some(accessible::Role::ProgressIndicator)
        );
        assert_eq!(parse_role("row"), Some(accessible::Role::Row));
        assert_eq!(parse_role("cell"), Some(accessible::Role::Cell));
        // Concatenated forms are NOT accepted (one canonical form only).
        assert_eq!(parse_role("alertdialog"), None);
        assert_eq!(parse_role("combobox"), None);
        assert_eq!(parse_role("listitem"), None);
        assert_eq!(parse_role("menubar"), None);
        assert_eq!(parse_role("scrollbar"), None);
        assert_eq!(parse_role("columnheader"), None);
        assert_eq!(parse_role("unknown_thing"), None);
    }

    #[test]
    fn parse_live_mapping() {
        assert_eq!(parse_live("polite"), Some(accessible::Live::Polite));
        assert_eq!(parse_live("assertive"), Some(accessible::Live::Assertive));
        assert_eq!(parse_live("off"), None);
    }

    #[test]
    fn parse_orientation_mapping() {
        assert_eq!(
            parse_orientation("horizontal"),
            Some(accessible::Orientation::Horizontal)
        );
        assert_eq!(
            parse_orientation("vertical"),
            Some(accessible::Orientation::Vertical)
        );
        assert_eq!(parse_orientation("diagonal"), None);
    }

    // -- level validation -----------------------------------------------------

    #[test]
    fn level_rejects_out_of_range() {
        for n in [0, 7, 100] {
            let props = json!({"a11y": {"level": n}});
            // level alone doesn't trigger has_overrides (it's None for invalid)
            assert!(A11yOverrides::from_props(&props).is_none());
        }
    }

    #[test]
    fn level_accepts_1_through_6() {
        for n in 1..=6 {
            let props = json!({"a11y": {"level": n}});
            let o = A11yOverrides::from_props(&props).unwrap();
            assert_eq!(o.level, Some(n as usize));
        }
    }

    // -- mnemonic edge cases --------------------------------------------------

    #[test]
    fn mnemonic_takes_first_char() {
        let o = A11yOverrides::from_props(&json!({"a11y": {"mnemonic": "Save"}})).unwrap();
        assert_eq!(o.mnemonic, Some('S'));
    }

    #[test]
    fn mnemonic_none_when_empty_string() {
        let props = json!({"a11y": {"mnemonic": ""}});
        // Empty mnemonic doesn't trigger has_overrides
        assert!(A11yOverrides::from_props(&props).is_none());
    }

    // -- has_overrides --------------------------------------------------------

    #[test]
    fn has_overrides_false_when_default() {
        assert!(!A11yOverrides::default().has_overrides());
    }

    #[test]
    fn has_overrides_true_for_each_field() {
        // Test representative fields from each category.
        let cases: Vec<A11yOverrides> = vec![
            A11yOverrides {
                role: Some(accessible::Role::Button),
                ..Default::default()
            },
            A11yOverrides {
                label: Some("x".into()),
                ..Default::default()
            },
            A11yOverrides {
                required: true,
                ..Default::default()
            },
            A11yOverrides {
                toggled: Some(true),
                ..Default::default()
            },
            A11yOverrides {
                orientation: Some(accessible::Orientation::Horizontal),
                ..Default::default()
            },
            A11yOverrides {
                labelled_by: Some(widget::Id::from("x".to_owned())),
                ..Default::default()
            },
        ];
        for (i, o) in cases.iter().enumerate() {
            assert!(o.has_overrides(), "case {i} should have overrides");
        }
    }

    // -- apply_to -------------------------------------------------------------

    #[test]
    fn apply_to_overrides_win() {
        let overrides = A11yOverrides {
            label: Some("Override".into()),
            role: Some(accessible::Role::Navigation),
            ..Default::default()
        };
        let base = Accessible {
            role: accessible::Role::Group,
            label: Some("Original"),
            ..Default::default()
        };
        let merged = overrides.apply_to(&base);
        assert_eq!(merged.role, accessible::Role::Navigation);
        assert_eq!(merged.label, Some("Override"));
    }

    #[test]
    fn apply_to_falls_back_to_base() {
        let overrides = A11yOverrides::default();
        let base = Accessible {
            role: accessible::Role::Button,
            label: Some("Click"),
            disabled: true,
            ..Default::default()
        };
        let merged = overrides.apply_to(&base);
        assert_eq!(merged.role, accessible::Role::Button);
        assert_eq!(merged.label, Some("Click"));
        assert!(merged.disabled); // Preserved from base (widget-internal).
    }

    #[test]
    fn apply_to_bools_are_ored() {
        let overrides = A11yOverrides {
            required: true,
            ..Default::default()
        };
        let base = Accessible {
            busy: true,
            ..Default::default()
        };
        let merged = overrides.apply_to(&base);
        assert!(merged.required); // From override.
        assert!(merged.busy); // From base.
    }

    #[test]
    fn to_accessible_uses_defaults_for_base() {
        let overrides = A11yOverrides {
            role: Some(accessible::Role::Navigation),
            label: Some("Main nav".into()),
            ..Default::default()
        };
        let node = overrides.to_accessible();
        assert_eq!(node.role, accessible::Role::Navigation);
        assert_eq!(node.label, Some("Main nav"));
        assert!(!node.disabled); // Default.
    }

    // -- with_description -----------------------------------------------------

    #[test]
    fn with_description_sets_only_description() {
        let overrides = A11yOverrides::with_description("Placeholder hint".to_string());
        assert_eq!(overrides.description.as_deref(), Some("Placeholder hint"));
        assert!(overrides.label.is_none());
        assert!(overrides.role.is_none());
        assert!(!overrides.hidden);
    }

    // -- new fields: disabled, position_in_set, size_of_set, has_popup --------

    #[test]
    fn from_props_parses_disabled() {
        let o = A11yOverrides::from_props(&json!({"a11y": {"disabled": true}})).unwrap();
        assert_eq!(o.disabled, Some(true));
    }

    #[test]
    fn from_props_parses_disabled_false() {
        let o = A11yOverrides::from_props(&json!({"a11y": {"disabled": false}})).unwrap();
        assert_eq!(o.disabled, Some(false));
    }

    #[test]
    fn from_props_parses_position_in_set() {
        let o = A11yOverrides::from_props(&json!({"a11y": {"position_in_set": 3}})).unwrap();
        assert_eq!(o.position_in_set, Some(3));
    }

    #[test]
    fn from_props_parses_size_of_set() {
        let o = A11yOverrides::from_props(&json!({"a11y": {"size_of_set": 10}})).unwrap();
        assert_eq!(o.size_of_set, Some(10));
    }

    #[test]
    fn from_props_parses_has_popup() {
        let cases = [
            ("listbox", accessible::HasPopup::Listbox),
            ("menu", accessible::HasPopup::Menu),
            ("dialog", accessible::HasPopup::Dialog),
            ("tree", accessible::HasPopup::Tree),
            ("grid", accessible::HasPopup::Grid),
        ];
        for (input, expected) in cases {
            let o = A11yOverrides::from_props(&json!({"a11y": {"has_popup": input}})).unwrap();
            assert_eq!(o.has_popup, Some(expected), "has_popup({input:?})");
        }
    }

    #[test]
    fn parse_has_popup_unknown_returns_none() {
        assert!(parse_has_popup("tooltip").is_none());
        assert!(parse_has_popup("").is_none());
    }

    #[test]
    fn has_overrides_true_for_new_fields() {
        let cases: Vec<A11yOverrides> = vec![
            A11yOverrides {
                disabled: Some(true),
                ..Default::default()
            },
            A11yOverrides {
                position_in_set: Some(1),
                ..Default::default()
            },
            A11yOverrides {
                size_of_set: Some(5),
                ..Default::default()
            },
            A11yOverrides {
                has_popup: Some(accessible::HasPopup::Dialog),
                ..Default::default()
            },
        ];
        for (i, o) in cases.iter().enumerate() {
            assert!(
                o.has_overrides(),
                "new field case {i} should have overrides"
            );
        }
    }

    #[test]
    fn apply_to_disabled_override_replaces_base() {
        let overrides = A11yOverrides {
            disabled: Some(true),
            ..Default::default()
        };
        let base = Accessible {
            disabled: false,
            ..Default::default()
        };
        let merged = overrides.apply_to(&base);
        assert!(merged.disabled);
    }

    #[test]
    fn apply_to_disabled_none_preserves_base() {
        let overrides = A11yOverrides::default();
        let base = Accessible {
            disabled: true,
            ..Default::default()
        };
        let merged = overrides.apply_to(&base);
        assert!(merged.disabled);
    }

    #[test]
    fn apply_to_disabled_can_enable() {
        let overrides = A11yOverrides {
            disabled: Some(false),
            ..Default::default()
        };
        let base = Accessible {
            disabled: true,
            ..Default::default()
        };
        let merged = overrides.apply_to(&base);
        assert!(!merged.disabled);
    }

    #[test]
    fn apply_to_position_in_set_override_wins() {
        let overrides = A11yOverrides {
            position_in_set: Some(5),
            ..Default::default()
        };
        let base = Accessible {
            position_in_set: Some(1),
            ..Default::default()
        };
        let merged = overrides.apply_to(&base);
        assert_eq!(merged.position_in_set, Some(5));
    }

    #[test]
    fn apply_to_size_of_set_falls_back_to_base() {
        let overrides = A11yOverrides::default();
        let base = Accessible {
            size_of_set: Some(10),
            ..Default::default()
        };
        let merged = overrides.apply_to(&base);
        assert_eq!(merged.size_of_set, Some(10));
    }

    #[test]
    fn apply_to_has_popup_override_wins() {
        let overrides = A11yOverrides {
            has_popup: Some(accessible::HasPopup::Grid),
            ..Default::default()
        };
        let base = Accessible {
            has_popup: Some(accessible::HasPopup::Listbox),
            ..Default::default()
        };
        let merged = overrides.apply_to(&base);
        assert_eq!(merged.has_popup, Some(accessible::HasPopup::Grid));
    }

    #[test]
    fn apply_to_has_popup_falls_back_to_base() {
        let overrides = A11yOverrides::default();
        let base = Accessible {
            has_popup: Some(accessible::HasPopup::Menu),
            ..Default::default()
        };
        let merged = overrides.apply_to(&base);
        assert_eq!(merged.has_popup, Some(accessible::HasPopup::Menu));
    }

    // -- A11yInterceptor integration tests ------------------------------------

    /// A recording operation that captures accessible(), container(),
    /// and text() calls for assertion.
    struct RecordingOperation {
        accessible_calls: Vec<RecordedAccessible>,
        container_calls: Vec<bool>,
        text_calls: Vec<String>,
    }

    #[allow(dead_code)]
    struct RecordedAccessible {
        role: accessible::Role,
        label: Option<String>,
        disabled: bool,
        position_in_set: Option<usize>,
        size_of_set: Option<usize>,
        has_popup: Option<accessible::HasPopup>,
    }

    impl RecordingOperation {
        fn new() -> Self {
            Self {
                accessible_calls: Vec::new(),
                container_calls: Vec::new(),
                text_calls: Vec::new(),
            }
        }
    }

    impl widget::Operation for RecordingOperation {
        fn accessible(
            &mut self,
            _id: Option<&widget::Id>,
            _bounds: Rectangle,
            accessible: &Accessible<'_>,
        ) {
            self.accessible_calls.push(RecordedAccessible {
                role: accessible.role,
                label: accessible.label.map(String::from),
                disabled: accessible.disabled,
                position_in_set: accessible.position_in_set,
                size_of_set: accessible.size_of_set,
                has_popup: accessible.has_popup,
            });
        }

        fn container(&mut self, _id: Option<&widget::Id>, _bounds: Rectangle) {
            self.container_calls.push(true);
        }

        fn text(&mut self, _id: Option<&widget::Id>, _bounds: Rectangle, text: &str) {
            self.text_calls.push(text.to_owned());
        }

        fn focusable(
            &mut self,
            _id: Option<&widget::Id>,
            _bounds: Rectangle,
            _state: &mut dyn widget::operation::focusable::Focusable,
        ) {
        }

        fn scrollable(
            &mut self,
            _id: Option<&widget::Id>,
            _bounds: Rectangle,
            _content_bounds: Rectangle,
            _translation: Vector,
            _state: &mut dyn widget::operation::scrollable::Scrollable,
        ) {
        }

        fn text_input(
            &mut self,
            _id: Option<&widget::Id>,
            _bounds: Rectangle,
            _state: &mut dyn widget::operation::text_input::TextInput,
        ) {
        }

        fn custom(
            &mut self,
            _id: Option<&widget::Id>,
            _bounds: Rectangle,
            _state: &mut dyn std::any::Any,
        ) {
        }

        fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
            operate(self);
        }

        fn finish(&self) -> widget::operation::Outcome<()> {
            widget::operation::Outcome::None
        }
    }

    #[test]
    fn interceptor_merges_overrides_with_base_accessible() {
        let overrides = A11yOverrides {
            label: Some("Override label".into()),
            role: Some(accessible::Role::Link),
            ..Default::default()
        };
        let base = Accessible {
            role: accessible::Role::Button,
            label: Some("Click me"),
            disabled: true,
            ..Default::default()
        };
        let mut recording = RecordingOperation::new();
        {
            let mut interceptor = A11yInterceptor {
                inner: &mut recording,
                overrides: &overrides,
            };
            interceptor.accessible(None, Rectangle::default(), &base);
        }
        assert_eq!(recording.accessible_calls.len(), 1);
        let call = &recording.accessible_calls[0];
        assert_eq!(call.role, accessible::Role::Link);
        assert_eq!(call.label.as_deref(), Some("Override label"));
        assert!(call.disabled); // Preserved from base.
    }

    #[test]
    fn interceptor_hidden_suppresses_accessible() {
        let overrides = A11yOverrides {
            hidden: true,
            ..Default::default()
        };
        let base = Accessible {
            role: accessible::Role::Button,
            label: Some("Hidden button"),
            ..Default::default()
        };
        let mut recording = RecordingOperation::new();
        {
            let mut interceptor = A11yInterceptor {
                inner: &mut recording,
                overrides: &overrides,
            };
            interceptor.accessible(None, Rectangle::default(), &base);
        }
        assert!(recording.accessible_calls.is_empty());
    }

    #[test]
    fn interceptor_hidden_suppresses_text() {
        let overrides = A11yOverrides {
            hidden: true,
            ..Default::default()
        };
        let mut recording = RecordingOperation::new();
        {
            let mut interceptor = A11yInterceptor {
                inner: &mut recording,
                overrides: &overrides,
            };
            interceptor.text(None, Rectangle::default(), "should not appear");
        }
        assert!(recording.text_calls.is_empty());
    }

    #[test]
    fn interceptor_container_upgrades_when_overrides_present() {
        let overrides = A11yOverrides {
            role: Some(accessible::Role::Group),
            label: Some("Nav group".into()),
            ..Default::default()
        };
        let mut recording = RecordingOperation::new();
        {
            let mut interceptor = A11yInterceptor {
                inner: &mut recording,
                overrides: &overrides,
            };
            interceptor.container(None, Rectangle::default());
        }
        // Container was upgraded to accessible.
        assert!(recording.container_calls.is_empty());
        assert_eq!(recording.accessible_calls.len(), 1);
        let call = &recording.accessible_calls[0];
        assert_eq!(call.role, accessible::Role::Group);
        assert_eq!(call.label.as_deref(), Some("Nav group"));
    }

    #[test]
    fn interceptor_container_passes_through_without_overrides() {
        let overrides = A11yOverrides::default();
        let mut recording = RecordingOperation::new();
        {
            let mut interceptor = A11yInterceptor {
                inner: &mut recording,
                overrides: &overrides,
            };
            interceptor.container(None, Rectangle::default());
        }
        assert_eq!(recording.container_calls.len(), 1);
        assert!(recording.accessible_calls.is_empty());
    }

    #[test]
    fn interceptor_hidden_suppresses_container() {
        let overrides = A11yOverrides {
            hidden: true,
            ..Default::default()
        };
        let mut recording = RecordingOperation::new();
        {
            let mut interceptor = A11yInterceptor {
                inner: &mut recording,
                overrides: &overrides,
            };
            interceptor.container(None, Rectangle::default());
        }
        assert!(recording.container_calls.is_empty());
        assert!(recording.accessible_calls.is_empty());
    }

    #[test]
    fn interceptor_traverse_propagates_hidden_to_children() {
        let overrides = A11yOverrides {
            hidden: true,
            ..Default::default()
        };
        let mut recording = RecordingOperation::new();
        {
            let mut interceptor = A11yInterceptor {
                inner: &mut recording,
                overrides: &overrides,
            };
            interceptor.traverse(&mut |child_op| {
                // The child operation should also suppress accessible calls.
                let base = Accessible {
                    role: accessible::Role::Button,
                    label: Some("Child button"),
                    ..Default::default()
                };
                child_op.accessible(None, Rectangle::default(), &base);
                child_op.text(None, Rectangle::default(), "child text");
            });
        }
        assert!(recording.accessible_calls.is_empty());
        assert!(recording.text_calls.is_empty());
    }
}
