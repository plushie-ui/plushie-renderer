//! Custom overlay widget: renders first child as anchor, second child as an
//! overlay positioned relative to the anchor bounds.
//!
//! Modelled after iced's tooltip widget but without hover delay or container
//! styling -- the overlay is always visible and the caller controls content.

use crate::message::Message;

use iced::advanced::layout::{self, Layout};
use iced::advanced::overlay;
use iced::advanced::renderer;
use iced::advanced::widget::{self, Widget};
use iced::advanced::{Clipboard, Shell};
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector};

/// Overlay position relative to the anchor widget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Position {
    Below,
    Above,
    Left,
    Right,
}

/// A widget that renders its anchor child normally and displays its overlay
/// child as an iced overlay positioned relative to the anchor.
pub(crate) struct OverlayWrapper<'a> {
    anchor: Element<'a, Message>,
    content: Element<'a, Message>,
    position: Position,
    gap: f32,
    offset_x: f32,
    offset_y: f32,
}

impl<'a> OverlayWrapper<'a> {
    pub(crate) fn new(
        anchor: Element<'a, Message>,
        content: Element<'a, Message>,
        position: Position,
        gap: f32,
        offset_x: f32,
        offset_y: f32,
    ) -> Self {
        Self {
            anchor,
            content,
            position,
            gap,
            offset_x,
            offset_y,
        }
    }
}

impl Widget<Message, iced::Theme, iced::Renderer> for OverlayWrapper<'_> {
    fn children(&self) -> Vec<widget::Tree> {
        vec![
            widget::Tree::new(&self.anchor),
            widget::Tree::new(&self.content),
        ]
    }

    fn diff(&self, tree: &mut widget::Tree) {
        tree.diff_children(&[self.anchor.as_widget(), self.content.as_widget()]);
    }

    fn size(&self) -> Size<Length> {
        self.anchor.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.anchor.as_widget().size_hint()
    }

    fn layout(
        &mut self,
        tree: &mut widget::Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.anchor
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
        self.anchor.as_widget().draw(
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
        self.anchor.as_widget_mut().update(
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
        self.anchor.as_widget().mouse_interaction(
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
        let mut children = tree.children.iter_mut();
        let anchor_tree = children
            .next()
            .expect("OverlayWrapper must have anchor tree child");
        let content_tree = children
            .next()
            .expect("OverlayWrapper must have content tree child");

        // Collect any overlay from the anchor child itself.
        let anchor_overlay = self.anchor.as_widget_mut().overlay(
            anchor_tree,
            layout,
            renderer,
            viewport,
            translation,
        );

        let content_overlay = Some(overlay::Element::new(Box::new(OverlayContent {
            content: &mut self.content,
            tree: content_tree,
            position: self.position,
            gap: self.gap,
            offset_x: self.offset_x,
            offset_y: self.offset_y,
            anchor_bounds: layout.bounds(),
            translation,
        })));

        // If the anchor also produces overlays, group them together.
        if anchor_overlay.is_some() || content_overlay.is_some() {
            Some(
                overlay::Group::with_children(
                    anchor_overlay.into_iter().chain(content_overlay).collect(),
                )
                .overlay(),
            )
        } else {
            None
        }
    }

    fn operate(
        &mut self,
        tree: &mut widget::Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        self.anchor
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }
}

impl<'a> From<OverlayWrapper<'a>> for Element<'a, Message> {
    fn from(wrapper: OverlayWrapper<'a>) -> Self {
        Element::new(wrapper)
    }
}

// ---------------------------------------------------------------------------
// Overlay content (the piece that floats above everything)
// ---------------------------------------------------------------------------

struct OverlayContent<'a, 'b> {
    content: &'b mut Element<'a, Message>,
    tree: &'b mut widget::Tree,
    position: Position,
    gap: f32,
    offset_x: f32,
    offset_y: f32,
    anchor_bounds: Rectangle,
    translation: Vector,
}

impl overlay::Overlay<Message, iced::Theme, iced::Renderer> for OverlayContent<'_, '_> {
    fn layout(&mut self, renderer: &iced::Renderer, bounds: Size) -> layout::Node {
        let limits = layout::Limits::new(Size::ZERO, bounds);
        let content_layout = self
            .content
            .as_widget_mut()
            .layout(self.tree, renderer, &limits);
        let content_size = content_layout.bounds().size();

        // Anchor position in absolute coordinates (accounting for translation).
        let anchor = Rectangle {
            x: self.anchor_bounds.x + self.translation.x,
            y: self.anchor_bounds.y + self.translation.y,
            width: self.anchor_bounds.width,
            height: self.anchor_bounds.height,
        };

        let (x, y) = match self.position {
            Position::Below => (
                anchor.x + (anchor.width - content_size.width) / 2.0,
                anchor.y + anchor.height + self.gap,
            ),
            Position::Above => (
                anchor.x + (anchor.width - content_size.width) / 2.0,
                anchor.y - content_size.height - self.gap,
            ),
            Position::Left => (
                anchor.x - content_size.width - self.gap,
                anchor.y + (anchor.height - content_size.height) / 2.0,
            ),
            Position::Right => (
                anchor.x + anchor.width + self.gap,
                anchor.y + (anchor.height - content_size.height) / 2.0,
            ),
        };

        let final_x = (x + self.offset_x).clamp(0.0, (bounds.width - content_size.width).max(0.0));
        let final_y =
            (y + self.offset_y).clamp(0.0, (bounds.height - content_size.height).max(0.0));

        layout::Node::with_children(content_size, vec![content_layout])
            .move_to(Point::new(final_x, final_y))
    }

    fn draw(
        &self,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: iced::mouse::Cursor,
    ) {
        let content_layout = layout
            .children()
            .next()
            .expect("overlay content must have a child layout");
        self.content.as_widget().draw(
            self.tree,
            renderer,
            theme,
            style,
            content_layout,
            cursor,
            &Rectangle::with_size(Size::INFINITE),
        );
    }

    fn update(
        &mut self,
        event: &Event,
        layout: Layout<'_>,
        cursor: iced::mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
    ) {
        let content_layout = layout
            .children()
            .next()
            .expect("overlay content must have a child layout");
        self.content.as_widget_mut().update(
            self.tree,
            event,
            content_layout,
            cursor,
            renderer,
            clipboard,
            shell,
            &Rectangle::with_size(Size::INFINITE),
        );
    }

    fn mouse_interaction(
        &self,
        layout: Layout<'_>,
        cursor: iced::mouse::Cursor,
        renderer: &iced::Renderer,
    ) -> iced::mouse::Interaction {
        let viewport = Rectangle::with_size(Size::INFINITE);
        let content_layout = layout
            .children()
            .next()
            .expect("overlay content must have a child layout");
        self.content.as_widget().mouse_interaction(
            self.tree,
            content_layout,
            cursor,
            &viewport,
            renderer,
        )
    }

    fn operate(
        &mut self,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        let content_layout = layout
            .children()
            .next()
            .expect("overlay content must have a child layout");
        self.content
            .as_widget_mut()
            .operate(self.tree, content_layout, renderer, operation);
    }

    fn overlay<'c>(
        &'c mut self,
        layout: Layout<'c>,
        renderer: &iced::Renderer,
    ) -> Option<overlay::Element<'c, Message, iced::Theme, iced::Renderer>> {
        let content_layout = layout
            .children()
            .next()
            .expect("overlay content must have a child layout");
        self.content.as_widget_mut().overlay(
            self.tree,
            content_layout,
            renderer,
            &layout.bounds(),
            Vector::ZERO,
        )
    }
}

#[cfg(test)]
mod tests {
    /// Mirrors the clamping logic in OverlayContent::layout().
    fn clamp_position(
        x: f32,
        y: f32,
        content_w: f32,
        content_h: f32,
        viewport_w: f32,
        viewport_h: f32,
    ) -> (f32, f32) {
        let final_x = x.clamp(0.0, (viewport_w - content_w).max(0.0));
        let final_y = y.clamp(0.0, (viewport_h - content_h).max(0.0));
        (final_x, final_y)
    }

    #[test]
    fn clamp_within_viewport() {
        let (x, y) = clamp_position(100.0, 100.0, 50.0, 50.0, 800.0, 600.0);
        assert_eq!((x, y), (100.0, 100.0));
    }

    #[test]
    fn clamp_right_edge() {
        // Content at x=780 with width=50 would extend to 830, beyond viewport 800.
        let (x, _) = clamp_position(780.0, 100.0, 50.0, 50.0, 800.0, 600.0);
        assert_eq!(x, 750.0);
    }

    #[test]
    fn clamp_bottom_edge() {
        // Content at y=580 with height=50 would extend to 630, beyond viewport 600.
        let (_, y) = clamp_position(100.0, 580.0, 50.0, 50.0, 800.0, 600.0);
        assert_eq!(y, 550.0);
    }

    #[test]
    fn clamp_negative() {
        let (x, y) = clamp_position(-10.0, -20.0, 50.0, 50.0, 800.0, 600.0);
        assert_eq!((x, y), (0.0, 0.0));
    }

    #[test]
    fn clamp_content_larger_than_viewport() {
        // Content bigger than viewport -- best we can do is pin to origin.
        let (x, y) = clamp_position(100.0, 100.0, 900.0, 700.0, 800.0, 600.0);
        assert_eq!((x, y), (0.0, 0.0));
    }

    #[test]
    fn clamp_exact_fit() {
        // Content exactly fills viewport -- only valid position is (0, 0).
        let (x, y) = clamp_position(50.0, 50.0, 800.0, 600.0, 800.0, 600.0);
        assert_eq!((x, y), (0.0, 0.0));
    }

    #[test]
    fn clamp_zero_size_content() {
        // Zero-size content can go anywhere within the viewport.
        let (x, y) = clamp_position(400.0, 300.0, 0.0, 0.0, 800.0, 600.0);
        assert_eq!((x, y), (400.0, 300.0));
    }
}
