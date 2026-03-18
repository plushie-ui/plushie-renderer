//! Bidirectional window ID mapping with associated per-window state.
//!
//! Wraps the julep ID <-> iced window::Id relationship and any
//! per-window state (decoration, theme cache) in a single type.
//! Insertions and removals are atomic -- it's impossible to update
//! one side without the other.

use iced::{Theme, window};
use std::collections::HashMap;

/// Per-window state beyond the ID mapping.
struct WindowState {
    /// Current decoration state. iced only exposes toggle_decorations(),
    /// so we track the boolean to avoid toggling when already correct.
    decorated: bool,
    /// Resolved theme for this window, if set via the tree's theme prop.
    /// None means "use app theme" (system or global).
    theme: Option<Theme>,
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            decorated: true,
            theme: None,
        }
    }
}

pub(super) struct WindowMap {
    /// Julep window ID -> (iced window ID, per-window state).
    forward: HashMap<String, (window::Id, WindowState)>,
    /// Iced window ID -> julep window ID.
    reverse: HashMap<window::Id, String>,
}

impl WindowMap {
    pub(super) fn new() -> Self {
        Self {
            forward: HashMap::new(),
            reverse: HashMap::new(),
        }
    }

    pub(super) fn insert(&mut self, julep_id: String, iced_id: window::Id) {
        self.forward
            .insert(julep_id.clone(), (iced_id, WindowState::default()));
        self.reverse.insert(iced_id, julep_id);
    }

    pub(super) fn remove_by_iced(&mut self, iced_id: &window::Id) -> Option<String> {
        if let Some(julep_id) = self.reverse.remove(iced_id) {
            self.forward.remove(&julep_id);
            Some(julep_id)
        } else {
            None
        }
    }

    pub(super) fn remove_by_julep(&mut self, julep_id: &str) -> Option<window::Id> {
        if let Some((iced_id, _)) = self.forward.remove(julep_id) {
            self.reverse.remove(&iced_id);
            Some(iced_id)
        } else {
            None
        }
    }

    pub(super) fn contains_julep(&self, julep_id: &str) -> bool {
        self.forward.contains_key(julep_id)
    }

    pub(super) fn get_iced(&self, julep_id: &str) -> Option<&window::Id> {
        self.forward.get(julep_id).map(|(id, _)| id)
    }

    pub(super) fn get_julep(&self, iced_id: &window::Id) -> Option<&String> {
        self.reverse.get(iced_id)
    }

    /// Resolve julep ID from iced ID, returning empty string if not found.
    pub(super) fn julep_id_for(&self, iced_id: &window::Id) -> String {
        self.reverse.get(iced_id).cloned().unwrap_or_default()
    }

    pub(super) fn iced_ids(&self) -> impl Iterator<Item = &window::Id> {
        self.reverse.keys()
    }

    pub(super) fn julep_ids(&self) -> impl Iterator<Item = &String> {
        self.forward.keys()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.forward.is_empty()
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = (&String, &window::Id)> {
        self.forward.iter().map(|(jid, (iid, _))| (jid, iid))
    }

    pub(super) fn clear(&mut self) {
        self.forward.clear();
        self.reverse.clear();
    }

    // -- Per-window decoration state --

    pub(super) fn is_decorated(&self, julep_id: &str) -> bool {
        self.forward.get(julep_id).is_none_or(|(_, s)| s.decorated)
    }

    pub(super) fn set_decorated(&mut self, julep_id: &str, decorated: bool) {
        if let Some((_, state)) = self.forward.get_mut(julep_id) {
            state.decorated = decorated;
        }
    }

    // -- Per-window theme cache --

    pub(super) fn cached_theme(&self, julep_id: &str) -> Option<&Theme> {
        self.forward
            .get(julep_id)
            .and_then(|(_, s)| s.theme.as_ref())
    }

    pub(super) fn set_theme(&mut self, julep_id: &str, theme: Option<Theme>) {
        if let Some((_, state)) = self.forward.get_mut(julep_id) {
            state.theme = theme;
        }
    }

    pub(super) fn clear_theme_cache(&mut self) {
        for (_, state) in self.forward.values_mut() {
            state.theme = None;
        }
    }
}
