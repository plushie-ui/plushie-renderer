//! Bidirectional window ID mapping with associated per-window state.
//!
//! Wraps the toddy ID <-> iced window::Id relationship and any
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

/// Bidirectional toddy ID <-> iced window::Id mapping with per-window
/// state. All mutations keep both maps in sync -- callers cannot
/// accidentally desync the forward and reverse maps.
pub struct WindowMap {
    /// Toddy window ID -> (iced window ID, per-window state).
    forward: HashMap<String, (window::Id, WindowState)>,
    /// Iced window ID -> toddy window ID.
    reverse: HashMap<window::Id, String>,
}

impl Default for WindowMap {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowMap {
    pub fn new() -> Self {
        Self {
            forward: HashMap::new(),
            reverse: HashMap::new(),
        }
    }

    /// Insert a new window mapping. If the toddy_id already exists,
    /// the old iced_id is removed from the reverse map to prevent
    /// dangling entries.
    pub fn insert(&mut self, toddy_id: String, iced_id: window::Id) {
        if let Some((old_iced_id, _)) = self.forward.get(&toddy_id) {
            self.reverse.remove(old_iced_id);
        }
        self.forward
            .insert(toddy_id.clone(), (iced_id, WindowState::default()));
        self.reverse.insert(iced_id, toddy_id);
    }

    pub fn remove_by_iced(&mut self, iced_id: &window::Id) -> Option<String> {
        if let Some(toddy_id) = self.reverse.remove(iced_id) {
            self.forward.remove(&toddy_id);
            Some(toddy_id)
        } else {
            None
        }
    }

    pub fn remove_by_toddy(&mut self, toddy_id: &str) -> Option<window::Id> {
        if let Some((iced_id, _)) = self.forward.remove(toddy_id) {
            self.reverse.remove(&iced_id);
            Some(iced_id)
        } else {
            None
        }
    }

    pub fn contains_toddy(&self, toddy_id: &str) -> bool {
        self.forward.contains_key(toddy_id)
    }

    pub fn get_iced(&self, toddy_id: &str) -> Option<&window::Id> {
        self.forward.get(toddy_id).map(|(id, _)| id)
    }

    pub fn get_toddy(&self, iced_id: &window::Id) -> Option<&String> {
        self.reverse.get(iced_id)
    }

    /// Resolve toddy ID from iced ID, returning empty string if not found.
    pub fn toddy_id_for(&self, iced_id: &window::Id) -> String {
        self.reverse.get(iced_id).cloned().unwrap_or_default()
    }

    pub fn iced_ids(&self) -> impl Iterator<Item = &window::Id> {
        self.reverse.keys()
    }

    pub fn toddy_ids(&self) -> impl Iterator<Item = &String> {
        self.forward.keys()
    }

    pub fn is_empty(&self) -> bool {
        self.forward.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &window::Id)> {
        self.forward.iter().map(|(jid, (iid, _))| (jid, iid))
    }

    pub fn clear(&mut self) {
        self.forward.clear();
        self.reverse.clear();
    }

    // -- Per-window decoration state --

    pub fn is_decorated(&self, toddy_id: &str) -> bool {
        self.forward.get(toddy_id).is_none_or(|(_, s)| s.decorated)
    }

    pub fn set_decorated(&mut self, toddy_id: &str, decorated: bool) {
        if let Some((_, state)) = self.forward.get_mut(toddy_id) {
            state.decorated = decorated;
        }
    }

    // -- Per-window theme cache --

    pub fn cached_theme(&self, toddy_id: &str) -> Option<&Theme> {
        self.forward
            .get(toddy_id)
            .and_then(|(_, s)| s.theme.as_ref())
    }

    pub fn set_theme(&mut self, toddy_id: &str, theme: Option<Theme>) {
        if let Some((_, state)) = self.forward.get_mut(toddy_id) {
            state.theme = theme;
        }
    }

    pub fn clear_theme_cache(&mut self) {
        for (_, state) in self.forward.values_mut() {
            state.theme = None;
        }
    }
}
