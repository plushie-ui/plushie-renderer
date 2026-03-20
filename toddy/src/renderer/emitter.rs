//! Rate-limited event emission with coalescing.
//!
//! Buffers high-frequency events (mouse moves, scroll, animation frames)
//! and emits them at a configurable rate. Non-coalescable events (clicks,
//! key presses) flush the buffer immediately before emitting.
//!
//! The host controls rates via three mechanisms (highest priority first):
//! 1. Per-widget `event_rate` prop
//! 2. Per-subscription `max_rate` field on Subscribe
//! 3. Global `default_event_rate` in Settings

use std::collections::HashMap;
use std::time::{Duration, Instant};

use iced::Task;

use toddy_core::message::Message;
use toddy_core::protocol::OutgoingEvent;

use super::emitters;

// ---------------------------------------------------------------------------
// Coalesce key and strategy
// ---------------------------------------------------------------------------

/// Identifies a stream of events that can be coalesced together.
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub(super) enum CoalesceKey {
    /// Subscription event keyed by subscription kind (e.g. "on_mouse_move").
    Subscription(String),
    /// Widget event keyed by (widget_id, event_family).
    Widget(String, String),
}

/// How buffered events are merged when a newer one arrives before the
/// rate limit allows emission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CoalesceStrategy {
    /// Keep only the latest event, discarding intermediates.
    Replace,
    /// Sum delta fields across buffered events so no magnitude is lost.
    AccumulateDeltas,
}

// ---------------------------------------------------------------------------
// Pending event buffer
// ---------------------------------------------------------------------------

enum PendingEvent {
    /// Latest-value-wins: only the most recent event is kept.
    Replace(OutgoingEvent),
    /// Delta accumulation: `delta_x` and `delta_y` fields in `data`
    /// are summed across arrivals.
    Accumulate {
        base: OutgoingEvent,
        total_dx: f64,
        total_dy: f64,
    },
}

impl PendingEvent {
    fn into_event(self) -> OutgoingEvent {
        match self {
            PendingEvent::Replace(ev) => ev,
            PendingEvent::Accumulate {
                mut base,
                total_dx,
                total_dy,
            } => {
                // Patch the accumulated deltas back into the event's data.
                if let Some(ref mut data) = base.data
                    && let Some(obj) = data.as_object_mut()
                {
                    obj.insert("delta_x".to_string(), serde_json::json!(total_dx));
                    obj.insert("delta_y".to_string(), serde_json::json!(total_dy));
                }
                base
            }
        }
    }
}

// ---------------------------------------------------------------------------
// EventEmitter
// ---------------------------------------------------------------------------

/// Rate-limited event emission with coalescing.
///
/// Sits between the iced message handlers and the wire protocol. Events
/// classified as coalescable are buffered and emitted at a controlled
/// rate; non-coalescable events flush the buffer and emit immediately.
pub(super) struct EventEmitter {
    /// Pending coalescable events, keyed by coalesce key.
    pending: HashMap<CoalesceKey, PendingEvent>,
    /// Timestamp of last emission per coalesce key.
    last_emits: HashMap<CoalesceKey, Instant>,
    /// Whether a `Message::FlushCoalesce` timer task is outstanding.
    flush_scheduled: bool,
    /// Global default rate from Settings. None = no limit.
    default_rate: Option<u32>,
    /// Per-subscription rates from Subscribe max_rate.
    subscription_rates: HashMap<String, u32>,
    /// Per-widget rates from event_rate prop.
    widget_rates: HashMap<String, u32>,
}

impl EventEmitter {
    pub fn new() -> Self {
        Self {
            pending: HashMap::new(),
            last_emits: HashMap::new(),
            flush_scheduled: false,
            default_rate: None,
            subscription_rates: HashMap::new(),
            widget_rates: HashMap::new(),
        }
    }

    /// Set the global default rate from Settings.
    pub fn set_default_rate(&mut self, rate: Option<u32>) {
        self.default_rate = rate;
    }

    /// Set (or update) the rate for a subscription kind.
    pub fn set_subscription_rate(&mut self, kind: &str, rate: u32) {
        self.subscription_rates.insert(kind.to_string(), rate);
    }

    /// Remove rate tracking for a subscription kind.
    pub fn remove_subscription_rate(&mut self, kind: &str) {
        self.subscription_rates.remove(kind);
    }

    /// Set the rate for a specific widget (from `event_rate` prop).
    pub fn set_widget_rate(&mut self, widget_id: &str, rate: u32) {
        self.widget_rates.insert(widget_id.to_string(), rate);
    }

    /// Clear all widget rates (called on Snapshot -- tree replaced).
    pub fn clear_widget_rates(&mut self) {
        self.widget_rates.clear();
    }

    /// Check whether a widget rate is already cached.
    pub fn has_widget_rate(&self, widget_id: &str) -> bool {
        self.widget_rates.contains_key(widget_id)
    }

    /// Iterate over the subscription rate keys.
    pub fn subscription_rate_keys(&self) -> impl Iterator<Item = &str> {
        self.subscription_rates.keys().map(|s| s.as_str())
    }

    /// Resolve the effective rate for a given key, following the
    /// priority hierarchy: widget > subscription > global default.
    fn effective_rate(&self, key: &CoalesceKey) -> Option<u32> {
        match key {
            CoalesceKey::Widget(widget_id, _family) => {
                if let Some(&rate) = self.widget_rates.get(widget_id) {
                    return Some(rate);
                }
                self.default_rate
            }
            CoalesceKey::Subscription(kind) => {
                if let Some(&rate) = self.subscription_rates.get(kind) {
                    return Some(rate);
                }
                self.default_rate
            }
        }
    }

    /// Emit a coalescable event, buffering it if the rate limit has
    /// not elapsed. Returns a Task if a flush timer needs scheduling.
    pub fn coalesce(
        &mut self,
        key: CoalesceKey,
        event: OutgoingEvent,
        strategy: CoalesceStrategy,
    ) -> Task<Message> {
        let rate = self.effective_rate(&key);

        // Zero rate = muted, silently drop.
        if rate == Some(0) {
            return Task::none();
        }

        // No rate limit = emit immediately.
        let Some(rate) = rate else {
            self.flush_key(&key);
            return self.do_emit(event);
        };

        let min_interval = Duration::from_secs_f64(1.0 / rate as f64);
        let now = Instant::now();

        let can_emit_now = self
            .last_emits
            .get(&key)
            .map(|last| now.duration_since(*last) >= min_interval)
            .unwrap_or(true);

        if can_emit_now {
            self.pending.remove(&key);
            self.last_emits.insert(key, now);
            return self.do_emit(event);
        }

        // Buffer the event.
        self.buffer_event(&key, event, strategy);

        // Schedule a flush timer if one isn't already running.
        if !self.flush_scheduled {
            self.flush_scheduled = true;
            let remaining = self
                .last_emits
                .get(&key)
                .map(|last| min_interval.saturating_sub(now.duration_since(*last)))
                .unwrap_or(min_interval);
            return Task::perform(
                async move {
                    tokio::time::sleep(remaining).await;
                },
                |_| Message::FlushCoalesce,
            );
        }

        Task::none()
    }

    /// Emit a non-coalescable event immediately, flushing pending
    /// events first to preserve ordering.
    pub fn emit_immediate(&mut self, event: OutgoingEvent) -> Task<Message> {
        self.flush_all();
        self.do_emit(event)
    }

    /// Flush all pending events. Called by the `Message::FlushCoalesce`
    /// handler.
    pub fn flush(&mut self) -> Task<Message> {
        self.flush_scheduled = false;
        self.flush_all();
        Task::none()
    }

    /// Flush pending events for a specific key.
    pub fn flush_key(&mut self, key: &CoalesceKey) {
        if let Some(pending) = self.pending.remove(key) {
            let now = Instant::now();
            self.last_emits.insert(key.clone(), now);
            let _ = self.do_emit(pending.into_event());
        }
    }

    /// Flush all pending events (internal).
    fn flush_all(&mut self) {
        let keys: Vec<CoalesceKey> = self.pending.keys().cloned().collect();
        let now = Instant::now();
        for key in keys {
            if let Some(pending) = self.pending.remove(&key) {
                self.last_emits.insert(key, now);
                let _ = self.do_emit(pending.into_event());
            }
        }
    }

    /// Buffer an event under the given key.
    fn buffer_event(
        &mut self,
        key: &CoalesceKey,
        event: OutgoingEvent,
        strategy: CoalesceStrategy,
    ) {
        match strategy {
            CoalesceStrategy::Replace => {
                self.pending
                    .insert(key.clone(), PendingEvent::Replace(event));
            }
            CoalesceStrategy::AccumulateDeltas => {
                let (dx, dy) = extract_deltas(&event);
                match self.pending.get_mut(key) {
                    Some(PendingEvent::Accumulate {
                        total_dx, total_dy, ..
                    }) => {
                        *total_dx += dx;
                        *total_dy += dy;
                    }
                    _ => {
                        self.pending.insert(
                            key.clone(),
                            PendingEvent::Accumulate {
                                base: event,
                                total_dx: dx,
                                total_dy: dy,
                            },
                        );
                    }
                }
            }
        }
    }

    /// Encode and write an event to the wire. Returns Task::none() on
    /// success, iced::exit() on broken pipe.
    fn do_emit(&self, event: OutgoingEvent) -> Task<Message> {
        emitters::emit_or_exit(event)
    }
}

/// Extract delta_x and delta_y from an event's data object.
fn extract_deltas(event: &OutgoingEvent) -> (f64, f64) {
    let data = match &event.data {
        Some(d) => d,
        None => return (0.0, 0.0),
    };
    let obj = match data.as_object() {
        Some(o) => o,
        None => return (0.0, 0.0),
    };
    let dx = obj.get("delta_x").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let dy = obj.get("delta_y").and_then(|v| v.as_f64()).unwrap_or(0.0);
    (dx, dy)
}

// ---------------------------------------------------------------------------
// Helpers for classifying events
// ---------------------------------------------------------------------------

/// Event families that are coalescable (high-frequency, latest-value-wins
/// or delta-accumulation).
const COALESCABLE_WIDGET_FAMILIES: &[&str] = &[
    "slide",
    "mouse_area_move",
    "canvas_move",
    "sensor_resize",
    "pane_resized",
    "mouse_area_scroll",
    "canvas_scroll",
    "scroll",
];

/// Returns true if the event family is coalescable at the widget level.
pub(super) fn is_coalescable_widget_event(event: &OutgoingEvent) -> bool {
    COALESCABLE_WIDGET_FAMILIES.contains(&event.family.as_str())
}

/// Returns the coalesce strategy for a widget event.
pub(super) fn widget_coalesce_strategy(event: &OutgoingEvent) -> CoalesceStrategy {
    match event.family.as_str() {
        "mouse_area_scroll" | "canvas_scroll" => CoalesceStrategy::AccumulateDeltas,
        _ => CoalesceStrategy::Replace,
    }
}

/// Build a CoalesceKey for a widget event.
pub(super) fn widget_coalesce_key(event: &OutgoingEvent) -> CoalesceKey {
    CoalesceKey::Widget(event.id.clone(), event.family.clone())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_event(family: &str, id: &str) -> OutgoingEvent {
        OutgoingEvent {
            message_type: "event",
            session: String::new(),
            family: family.to_string(),
            id: id.to_string(),
            value: None,
            tag: None,
            modifiers: None,
            data: None,
            captured: None,
        }
    }

    fn make_event_with_data(family: &str, id: &str, data: serde_json::Value) -> OutgoingEvent {
        OutgoingEvent {
            message_type: "event",
            session: String::new(),
            family: family.to_string(),
            id: id.to_string(),
            value: None,
            tag: None,
            modifiers: None,
            data: Some(data),
            captured: None,
        }
    }

    // -- effective_rate hierarchy --

    #[test]
    fn effective_rate_no_config_returns_none() {
        let emitter = EventEmitter::new();
        let key = CoalesceKey::Subscription("on_mouse_move".into());
        assert_eq!(emitter.effective_rate(&key), None);
    }

    #[test]
    fn effective_rate_uses_default() {
        let mut emitter = EventEmitter::new();
        emitter.set_default_rate(Some(60));
        let key = CoalesceKey::Subscription("on_mouse_move".into());
        assert_eq!(emitter.effective_rate(&key), Some(60));
    }

    #[test]
    fn effective_rate_subscription_overrides_default() {
        let mut emitter = EventEmitter::new();
        emitter.set_default_rate(Some(60));
        emitter.set_subscription_rate("on_mouse_move", 30);
        let key = CoalesceKey::Subscription("on_mouse_move".into());
        assert_eq!(emitter.effective_rate(&key), Some(30));
    }

    #[test]
    fn effective_rate_widget_overrides_default() {
        let mut emitter = EventEmitter::new();
        emitter.set_default_rate(Some(60));
        emitter.set_widget_rate("slider-1", 15);
        let key = CoalesceKey::Widget("slider-1".into(), "slide".into());
        assert_eq!(emitter.effective_rate(&key), Some(15));
    }

    #[test]
    fn effective_rate_widget_without_override_falls_to_default() {
        let mut emitter = EventEmitter::new();
        emitter.set_default_rate(Some(60));
        let key = CoalesceKey::Widget("slider-1".into(), "slide".into());
        assert_eq!(emitter.effective_rate(&key), Some(60));
    }

    // -- clear_widget_rates --

    #[test]
    fn clear_widget_rates_removes_all() {
        let mut emitter = EventEmitter::new();
        emitter.set_widget_rate("a", 10);
        emitter.set_widget_rate("b", 20);
        emitter.clear_widget_rates();
        assert!(emitter.widget_rates.is_empty());
    }

    // -- remove_subscription_rate --

    #[test]
    fn remove_subscription_rate_clears_rate() {
        let mut emitter = EventEmitter::new();
        emitter.set_subscription_rate("on_mouse_move", 30);
        emitter.remove_subscription_rate("on_mouse_move");
        assert!(!emitter.subscription_rates.contains_key("on_mouse_move"));
    }

    // -- buffer_event --

    #[test]
    fn buffer_replace_keeps_latest() {
        let mut emitter = EventEmitter::new();
        let key = CoalesceKey::Widget("w1".into(), "slide".into());

        let ev1 = make_event("slide", "w1");
        emitter.buffer_event(&key, ev1, CoalesceStrategy::Replace);

        let ev2 = make_event("slide", "w1");
        emitter.buffer_event(&key, ev2, CoalesceStrategy::Replace);

        assert_eq!(emitter.pending.len(), 1);
    }

    #[test]
    fn buffer_accumulate_sums_deltas() {
        let mut emitter = EventEmitter::new();
        let key = CoalesceKey::Widget("ma1".into(), "mouse_area_scroll".into());

        let ev1 = make_event_with_data(
            "mouse_area_scroll",
            "ma1",
            json!({"delta_x": 1.0, "delta_y": 2.0}),
        );
        emitter.buffer_event(&key, ev1, CoalesceStrategy::AccumulateDeltas);

        let ev2 = make_event_with_data(
            "mouse_area_scroll",
            "ma1",
            json!({"delta_x": 3.0, "delta_y": 4.0}),
        );
        emitter.buffer_event(&key, ev2, CoalesceStrategy::AccumulateDeltas);

        match emitter.pending.get(&key).unwrap() {
            PendingEvent::Accumulate {
                total_dx, total_dy, ..
            } => {
                assert!((total_dx - 4.0).abs() < f64::EPSILON);
                assert!((total_dy - 6.0).abs() < f64::EPSILON);
            }
            _ => panic!("expected Accumulate variant"),
        }
    }

    // -- PendingEvent::into_event --

    #[test]
    fn accumulate_into_event_patches_deltas() {
        let base = make_event_with_data(
            "canvas_scroll",
            "c1",
            json!({"delta_x": 1.0, "delta_y": 2.0, "cursor_x": 50.0}),
        );
        let pending = PendingEvent::Accumulate {
            base,
            total_dx: 10.0,
            total_dy: 20.0,
        };
        let event = pending.into_event();
        let data = event.data.unwrap();
        assert_eq!(data["delta_x"], 10.0);
        assert_eq!(data["delta_y"], 20.0);
        // Other fields preserved.
        assert_eq!(data["cursor_x"], 50.0);
    }

    // -- classification helpers --

    #[test]
    fn coalescable_widget_families() {
        assert!(is_coalescable_widget_event(&make_event("slide", "s1")));
        assert!(is_coalescable_widget_event(&make_event(
            "mouse_area_move",
            "m1"
        )));
        assert!(is_coalescable_widget_event(&make_event(
            "canvas_scroll",
            "c1"
        )));
        assert!(!is_coalescable_widget_event(&make_event("click", "b1")));
        assert!(!is_coalescable_widget_event(&make_event("input", "i1")));
    }

    #[test]
    fn widget_strategy_scroll_accumulates() {
        assert_eq!(
            widget_coalesce_strategy(&make_event("mouse_area_scroll", "m1")),
            CoalesceStrategy::AccumulateDeltas
        );
        assert_eq!(
            widget_coalesce_strategy(&make_event("canvas_scroll", "c1")),
            CoalesceStrategy::AccumulateDeltas
        );
    }

    #[test]
    fn widget_strategy_non_scroll_replaces() {
        assert_eq!(
            widget_coalesce_strategy(&make_event("slide", "s1")),
            CoalesceStrategy::Replace
        );
        assert_eq!(
            widget_coalesce_strategy(&make_event("sensor_resize", "s1")),
            CoalesceStrategy::Replace
        );
    }

    // -- extract_deltas --

    #[test]
    fn extract_deltas_from_data() {
        let ev = make_event_with_data("scroll", "s1", json!({"delta_x": 5.5, "delta_y": -3.2}));
        let (dx, dy) = extract_deltas(&ev);
        assert!((dx - 5.5).abs() < f64::EPSILON);
        assert!((dy - (-3.2)).abs() < f64::EPSILON);
    }

    #[test]
    fn extract_deltas_missing_data_returns_zero() {
        let ev = make_event("scroll", "s1");
        let (dx, dy) = extract_deltas(&ev);
        assert!((dx).abs() < f64::EPSILON);
        assert!((dy).abs() < f64::EPSILON);
    }
}
