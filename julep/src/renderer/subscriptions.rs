use iced::{Subscription, event, system, window};

use julep_core::message::{KeyEventData, Message};

use super::App;
use super::constants::*;
use super::stdin::stdin_subscription;

impl App {
    pub(super) fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![
            Subscription::run(stdin_subscription).map(Message::Stdin),
            // Always listen for window close events so we can clean up maps.
            window::close_events().map(Message::WindowClosed),
        ];

        let has_on_event = self.core.active_subscriptions.contains_key(SUB_EVENT);

        self.keyboard_subscriptions(has_on_event, &mut subs);
        self.mouse_subscriptions(has_on_event, &mut subs);
        self.touch_subscriptions(has_on_event, &mut subs);
        self.ime_subscriptions(has_on_event, &mut subs);
        self.window_subscriptions(&mut subs);
        self.system_subscriptions(&mut subs);

        // -- Catch-all event subscription --
        // Subscribes to all keyboard, mouse, and touch events via a single
        // listener, re-using existing Message variants. Window events are
        // handled separately by on_window_event.
        //
        // To avoid duplicate event delivery when both on_event and a specific
        // subscription (e.g. on_key_press) are active, skip event categories
        // that already have a dedicated subscription listener above.
        if self.core.active_subscriptions.contains_key(SUB_EVENT) {
            subs.push(event::listen_with(|evt, status, window| {
                let captured = status == iced::event::Status::Captured;
                match evt {
                    // Keyboard
                    iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
                        key,
                        modified_key,
                        physical_key,
                        location,
                        modifiers,
                        text,
                        repeat,
                    }) => Some(Message::KeyPressed(KeyEventData {
                        key,
                        modified_key,
                        physical_key,
                        location,
                        modifiers,
                        text: text.map(|s| s.to_string()),
                        repeat,
                        captured,
                    })),
                    iced::Event::Keyboard(iced::keyboard::Event::KeyReleased {
                        key,
                        modified_key,
                        physical_key,
                        location,
                        modifiers,
                    }) => Some(Message::KeyReleased(KeyEventData {
                        key,
                        modified_key,
                        physical_key,
                        location,
                        modifiers,
                        text: None,
                        repeat: false,
                        captured,
                    })),
                    iced::Event::Keyboard(iced::keyboard::Event::ModifiersChanged(mods)) => {
                        Some(Message::ModifiersChanged(mods, captured))
                    }
                    // Mouse
                    iced::Event::Mouse(iced::mouse::Event::CursorMoved { position }) => {
                        Some(Message::CursorMoved(position, window, captured))
                    }
                    iced::Event::Mouse(iced::mouse::Event::CursorEntered) => {
                        Some(Message::CursorEntered(window, captured))
                    }
                    iced::Event::Mouse(iced::mouse::Event::CursorLeft) => {
                        Some(Message::CursorLeft(window, captured))
                    }
                    iced::Event::Mouse(iced::mouse::Event::ButtonPressed(button)) => {
                        Some(Message::MouseButtonPressed(button, window, captured))
                    }
                    iced::Event::Mouse(iced::mouse::Event::ButtonReleased(button)) => {
                        Some(Message::MouseButtonReleased(button, window, captured))
                    }
                    iced::Event::Mouse(iced::mouse::Event::WheelScrolled { delta }) => {
                        Some(Message::WheelScrolled(delta, window, captured))
                    }
                    // Touch
                    iced::Event::Touch(iced::touch::Event::FingerPressed { id, position }) => {
                        Some(Message::FingerPressed(id, position, window, captured))
                    }
                    iced::Event::Touch(iced::touch::Event::FingerMoved { id, position }) => {
                        Some(Message::FingerMoved(id, position, window, captured))
                    }
                    iced::Event::Touch(iced::touch::Event::FingerLifted { id, position }) => {
                        Some(Message::FingerLifted(id, position, window, captured))
                    }
                    iced::Event::Touch(iced::touch::Event::FingerLost { id, position }) => {
                        Some(Message::FingerLost(id, position, window, captured))
                    }
                    // IME
                    iced::Event::InputMethod(iced::advanced::input_method::Event::Opened) => {
                        Some(Message::ImeOpened(captured))
                    }
                    iced::Event::InputMethod(iced::advanced::input_method::Event::Preedit(
                        text,
                        cursor,
                    )) => Some(Message::ImePreedit(text, cursor, captured)),
                    iced::Event::InputMethod(iced::advanced::input_method::Event::Commit(text)) => {
                        Some(Message::ImeCommit(text, captured))
                    }
                    iced::Event::InputMethod(iced::advanced::input_method::Event::Closed) => {
                        Some(Message::ImeClosed(captured))
                    }
                    // Window events handled by on_window_event
                    _ => None,
                }
            }));
        }

        Subscription::batch(subs)
    }

    fn keyboard_subscriptions(&self, has_on_event: bool, subs: &mut Vec<Subscription<Message>>) {
        // When on_event is active, its catch-all listener already covers keyboard,
        // mouse, touch, and IME events. Skip specific subscriptions to avoid
        // duplicate event delivery (M-16).
        if !has_on_event && self.core.active_subscriptions.contains_key(SUB_KEY_PRESS) {
            subs.push(event::listen_with(|evt, status, _window| {
                if let iced::Event::Keyboard(iced::keyboard::Event::KeyPressed {
                    key,
                    modified_key,
                    physical_key,
                    location,
                    modifiers,
                    text,
                    repeat,
                }) = evt
                {
                    Some(Message::KeyPressed(KeyEventData {
                        key,
                        modified_key,
                        physical_key,
                        location,
                        modifiers,
                        text: text.map(|s| s.to_string()),
                        repeat,
                        captured: status == iced::event::Status::Captured,
                    }))
                } else {
                    None
                }
            }));
        }

        if !has_on_event && self.core.active_subscriptions.contains_key(SUB_KEY_RELEASE) {
            subs.push(event::listen_with(|evt, status, _window| {
                if let iced::Event::Keyboard(iced::keyboard::Event::KeyReleased {
                    key,
                    modified_key,
                    physical_key,
                    location,
                    modifiers,
                }) = evt
                {
                    Some(Message::KeyReleased(KeyEventData {
                        key,
                        modified_key,
                        physical_key,
                        location,
                        modifiers,
                        text: None,
                        repeat: false,
                        captured: status == iced::event::Status::Captured,
                    }))
                } else {
                    None
                }
            }));
        }

        if !has_on_event
            && self
                .core
                .active_subscriptions
                .contains_key(SUB_MODIFIERS_CHANGED)
        {
            subs.push(event::listen_with(|evt, status, _window| {
                if let iced::Event::Keyboard(iced::keyboard::Event::ModifiersChanged(mods)) = evt {
                    Some(Message::ModifiersChanged(
                        mods,
                        status == iced::event::Status::Captured,
                    ))
                } else {
                    None
                }
            }));
        }
    }

    fn mouse_subscriptions(&self, has_on_event: bool, subs: &mut Vec<Subscription<Message>>) {
        if !has_on_event && self.core.active_subscriptions.contains_key(SUB_MOUSE_MOVE) {
            subs.push(event::listen_with(|evt, status, window| {
                let captured = status == iced::event::Status::Captured;
                match evt {
                    iced::Event::Mouse(iced::mouse::Event::CursorMoved { position }) => {
                        Some(Message::CursorMoved(position, window, captured))
                    }
                    iced::Event::Mouse(iced::mouse::Event::CursorEntered) => {
                        Some(Message::CursorEntered(window, captured))
                    }
                    iced::Event::Mouse(iced::mouse::Event::CursorLeft) => {
                        Some(Message::CursorLeft(window, captured))
                    }
                    _ => None,
                }
            }));
        }

        if !has_on_event
            && self
                .core
                .active_subscriptions
                .contains_key(SUB_MOUSE_BUTTON)
        {
            subs.push(event::listen_with(|evt, status, window| {
                let captured = status == iced::event::Status::Captured;
                match evt {
                    iced::Event::Mouse(iced::mouse::Event::ButtonPressed(button)) => {
                        Some(Message::MouseButtonPressed(button, window, captured))
                    }
                    iced::Event::Mouse(iced::mouse::Event::ButtonReleased(button)) => {
                        Some(Message::MouseButtonReleased(button, window, captured))
                    }
                    _ => None,
                }
            }));
        }

        if !has_on_event
            && self
                .core
                .active_subscriptions
                .contains_key(SUB_MOUSE_SCROLL)
        {
            subs.push(event::listen_with(|evt, status, window| {
                if let iced::Event::Mouse(iced::mouse::Event::WheelScrolled { delta }) = evt {
                    Some(Message::WheelScrolled(
                        delta,
                        window,
                        status == iced::event::Status::Captured,
                    ))
                } else {
                    None
                }
            }));
        }
    }

    fn touch_subscriptions(&self, has_on_event: bool, subs: &mut Vec<Subscription<Message>>) {
        if !has_on_event && self.core.active_subscriptions.contains_key(SUB_TOUCH) {
            subs.push(event::listen_with(|evt, status, window| {
                let captured = status == iced::event::Status::Captured;
                match evt {
                    iced::Event::Touch(iced::touch::Event::FingerPressed { id, position }) => {
                        Some(Message::FingerPressed(id, position, window, captured))
                    }
                    iced::Event::Touch(iced::touch::Event::FingerMoved { id, position }) => {
                        Some(Message::FingerMoved(id, position, window, captured))
                    }
                    iced::Event::Touch(iced::touch::Event::FingerLifted { id, position }) => {
                        Some(Message::FingerLifted(id, position, window, captured))
                    }
                    iced::Event::Touch(iced::touch::Event::FingerLost { id, position }) => {
                        Some(Message::FingerLost(id, position, window, captured))
                    }
                    _ => None,
                }
            }));
        }
    }

    fn ime_subscriptions(&self, has_on_event: bool, subs: &mut Vec<Subscription<Message>>) {
        if !has_on_event && self.core.active_subscriptions.contains_key(SUB_IME) {
            subs.push(event::listen_with(|evt, status, _window| {
                let captured = status == iced::event::Status::Captured;
                match evt {
                    iced::Event::InputMethod(iced::advanced::input_method::Event::Opened) => {
                        Some(Message::ImeOpened(captured))
                    }
                    iced::Event::InputMethod(iced::advanced::input_method::Event::Preedit(
                        text,
                        cursor,
                    )) => Some(Message::ImePreedit(text, cursor, captured)),
                    iced::Event::InputMethod(iced::advanced::input_method::Event::Commit(text)) => {
                        Some(Message::ImeCommit(text, captured))
                    }
                    iced::Event::InputMethod(iced::advanced::input_method::Event::Closed) => {
                        Some(Message::ImeClosed(captured))
                    }
                    _ => None,
                }
            }));
        }
    }

    fn window_subscriptions(&self, subs: &mut Vec<Subscription<Message>>) {
        if self
            .core
            .active_subscriptions
            .contains_key(SUB_WINDOW_EVENT)
            || self.core.active_subscriptions.contains_key(SUB_FILE_DROP)
            || self.core.active_subscriptions.contains_key(SUB_WINDOW_OPEN)
            || self
                .core
                .active_subscriptions
                .contains_key(SUB_WINDOW_RESIZE)
            || self
                .core
                .active_subscriptions
                .contains_key(SUB_WINDOW_FOCUS)
            || self
                .core
                .active_subscriptions
                .contains_key(SUB_WINDOW_UNFOCUS)
            || self.core.active_subscriptions.contains_key(SUB_WINDOW_MOVE)
        {
            subs.push(window::events().map(|(id, evt)| Message::WindowEvent(id, evt)));
        }

        if self
            .core
            .active_subscriptions
            .contains_key(SUB_WINDOW_CLOSE)
        {
            subs.push(window::close_requests().map(Message::WindowCloseRequested));
        }

        // -- Animation frame subscription --
        if self
            .core
            .active_subscriptions
            .contains_key(SUB_ANIMATION_FRAME)
        {
            subs.push(window::frames().map(Message::AnimationFrame));
        }
    }

    fn system_subscriptions(&self, subs: &mut Vec<Subscription<Message>>) {
        // Track system theme changes when theme follows system OR when subscribed
        if self.theme_follows_system
            || self
                .core
                .active_subscriptions
                .contains_key(SUB_THEME_CHANGE)
        {
            subs.push(system::theme_changes().map(Message::ThemeChanged));
        }
    }
}
