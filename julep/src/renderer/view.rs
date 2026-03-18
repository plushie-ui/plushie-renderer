use iced::widget::{container, text};
use iced::{Element, Fill, window};

use julep_core::message::Message;

use super::App;

impl App {
    pub(super) fn view_window(&self, window_id: window::Id) -> Element<'_, Message> {
        let julep_id = match self.windows.get_julep(&window_id) {
            Some(id) => id,
            None => {
                return container(text("unknown window"))
                    .width(Fill)
                    .height(Fill)
                    .center(Fill)
                    .into();
            }
        };

        let resolved_theme = self.theme_ref_for_window(window_id);

        match self.core.tree.find_window(julep_id) {
            Some(window_node) => {
                let ctx = julep_core::extensions::RenderCtx {
                    caches: &self.core.caches,
                    images: &self.image_registry,
                    theme: resolved_theme,
                    extensions: &self.dispatcher,
                    default_text_size: self.core.default_text_size,
                    default_font: self.core.default_font,
                };
                julep_core::widgets::render(window_node, ctx)
            }
            None => container(text("waiting for snapshot..."))
                .width(Fill)
                .height(Fill)
                .center(Fill)
                .into(),
        }
    }
}
