//! Application builder for registering widget extensions.
//!
//! Extension packages create a [`JulepAppBuilder`], register their
//! extensions, and pass it to `julep::run()`. The default binary
//! passes an empty builder (no extensions).
//!
//! # Example
//!
//! ```ignore
//! use julep_core::prelude::*;
//!
//! fn main() -> iced::Result {
//!     julep::run(
//!         JulepAppBuilder::new()
//!             .extension(MyExtension::new())
//!             .extension(AnotherExtension::new())
//!     )
//! }
//! ```

use crate::extensions::{ExtensionDispatcher, WidgetExtension};

/// Builder for registering [`WidgetExtension`]s before starting the
/// renderer.
///
/// Each extension must have a unique `config_key()` and unique
/// `type_names()`. Duplicates panic at startup.
pub struct JulepAppBuilder {
    extensions: Vec<Box<dyn WidgetExtension>>,
}

impl JulepAppBuilder {
    /// Create an empty builder with no extensions registered.
    pub fn new() -> Self {
        Self { extensions: vec![] }
    }

    /// Register a widget extension.
    pub fn extension(mut self, ext: impl WidgetExtension + 'static) -> Self {
        self.extensions.push(Box::new(ext));
        self
    }

    /// Consume the builder and produce an [`ExtensionDispatcher`].
    pub fn build_dispatcher(self) -> ExtensionDispatcher {
        ExtensionDispatcher::new(self.extensions)
    }
}

impl Default for JulepAppBuilder {
    fn default() -> Self {
        Self::new()
    }
}
