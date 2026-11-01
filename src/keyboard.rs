//! Held-key tracking for the forwarded keyboard.
//!
//! The state machine is pure: it returns the family-2 events to send, so key
//! handling, stuck-key release and the escape chord are testable without a
//! device or a socket.

use std::collections::BTreeMap;

use evdev::KeyCode;

use crate::input::Event;
use crate::keymap;

#[derive(Debug, PartialEq, Eq)]
pub enum KeyAction {
    /// Keys were forwarded (or deliberately ignored).
    Handled,
    /// Super+Shift+T was pressed: held keys were released and control should
    /// return to Linux. The chord matches the compositor binding, which can no
    /// longer see a grabbed keyboard.
    ReturnToLinux,
}

#[derive(Debug, Clone, Copy)]
struct HeldKey {
    android: u32,
    character: u16,
}

#[derive(Default)]
pub struct KeyboardState {
    /// evdev code to what was sent on press, so release always matches.
    held: BTreeMap<u16, HeldKey>,
}

impl KeyboardState {
    pub fn handle(&mut self, code: KeyCode, down: bool, repeat: bool) -> (KeyAction, Vec<Event>) {
        if repeat {
            // Android keeps no repeat state for injected events, so a host
            // repeat is forwarded as another press of the same key.
            let events = self
                .held
                .get(&code.code())
                .map(|held| vec![key_event(*held, true)])
                .unwrap_or_default();
            return (KeyAction::Handled, events);
        }

        if down {
            if code == KeyCode::KEY_T && self.escape_chord_held() {
                return (KeyAction::ReturnToLinux, self.release_all());
            }
            if self.held.contains_key(&code.code()) {
                return (KeyAction::Handled, Vec::new());
            }
            let Some(mapped) = keymap::lookup(code) else {
                return (KeyAction::Handled, Vec::new());
            };
            let character = if self.suppresses_characters() {
                0
            } else {
                mapped
                    .chars
                    .map(|(plain, shifted)| if self.shift_held() { shifted } else { plain })
                    .map_or(0, |character| character as u16)
            };
            let held = HeldKey {
                android: mapped.android,
                character,
            };
            self.held.insert(code.code(), held);
            (KeyAction::Handled, vec![key_event(held, true)])
        } else {
            let events = self
                .held
                .remove(&code.code())
                .map(|held| vec![key_event(held, false)])
                .unwrap_or_default();
            (KeyAction::Handled, events)
        }
    }

    /// Release everything Android believes is held, so no key or modifier is
    /// left stuck when control returns to Linux or the session ends.
    pub fn release_all(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.held)
            .into_values()
            .map(|held| key_event(held, false))
            .collect()
    }

    fn escape_chord_held(&self) -> bool {
        self.shift_held() && self.any_held(&[KeyCode::KEY_LEFTMETA, KeyCode::KEY_RIGHTMETA])
    }

    fn shift_held(&self) -> bool {
        self.any_held(&[KeyCode::KEY_LEFTSHIFT, KeyCode::KEY_RIGHTSHIFT])
    }

    /// Ctrl, Alt and Super turn a printable key into a shortcut, so it is sent
    /// as a bare key code with no character, as the original server does.
    fn suppresses_characters(&self) -> bool {
        self.any_held(&[
            KeyCode::KEY_LEFTCTRL,
            KeyCode::KEY_RIGHTCTRL,
            KeyCode::KEY_LEFTALT,
            KeyCode::KEY_RIGHTALT,
            KeyCode::KEY_LEFTMETA,
            KeyCode::KEY_RIGHTMETA,
        ])
    }

    fn any_held(&self, codes: &[KeyCode]) -> bool {
        codes
            .iter()
            .any(|code| self.held.contains_key(&code.code()))
    }
}

fn key_event(held: HeldKey, is_down: bool) -> Event {
    Event::Key {
        key_code: held.android,
        // Releases never carry a character, matching the original server.
        character: if is_down { held.character } else { 0 },
        is_down,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(state: &mut KeyboardState, code: KeyCode) -> Vec<Event> {
        state.handle(code, true, false).1
    }

    #[test]
    fn plain_and_shifted_letters_carry_characters() {
        let mut state = KeyboardState::default();
        assert_eq!(
            press(&mut state, KeyCode::KEY_A),
            [Event::Key {
                key_code: 29,
                character: 'a' as u16,
                is_down: true
            }]
        );
        press(&mut state, KeyCode::KEY_LEFTSHIFT);
        assert_eq!(
            press(&mut state, KeyCode::KEY_B),
            [Event::Key {
                key_code: 30,
                character: 'B' as u16,
                is_down: true
            }]
        );
    }

    #[test]
    fn shortcuts_send_bare_key_codes() {
        let mut state = KeyboardState::default();
        press(&mut state, KeyCode::KEY_LEFTCTRL);
        assert_eq!(
            press(&mut state, KeyCode::KEY_C),
            [Event::Key {
                key_code: 31,
                character: 0,
                is_down: true
            }]
        );
    }

    #[test]
    fn release_matches_press_and_unknown_keys_are_ignored() {
        let mut state = KeyboardState::default();
        assert!(press(&mut state, KeyCode::KEY_VOLUMEUP).is_empty());
        press(&mut state, KeyCode::KEY_A);
        assert_eq!(
            state.handle(KeyCode::KEY_A, false, false).1,
            [Event::Key {
                key_code: 29,
                character: 0,
                is_down: false
            }]
        );
        assert!(state.handle(KeyCode::KEY_A, false, false).1.is_empty());
    }

    #[test]
    fn repeat_resends_the_pressed_key_only_while_held() {
        let mut state = KeyboardState::default();
        assert!(state.handle(KeyCode::KEY_A, true, true).1.is_empty());
        press(&mut state, KeyCode::KEY_A);
        assert_eq!(state.handle(KeyCode::KEY_A, true, true).1.len(), 1);
    }

    #[test]
    fn release_all_releases_every_held_key_once() {
        let mut state = KeyboardState::default();
        press(&mut state, KeyCode::KEY_LEFTSHIFT);
        press(&mut state, KeyCode::KEY_A);
        let events = state.release_all();
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|event| matches!(
            event,
            Event::Key {
                is_down: false,
                character: 0,
                ..
            }
        )));
        assert!(state.release_all().is_empty());
    }

    #[test]
    fn super_shift_t_returns_to_linux_and_releases() {
        let mut state = KeyboardState::default();
        press(&mut state, KeyCode::KEY_LEFTMETA);
        press(&mut state, KeyCode::KEY_LEFTSHIFT);
        let (action, events) = state.handle(KeyCode::KEY_T, true, false);
        assert_eq!(action, KeyAction::ReturnToLinux);
        assert_eq!(events.len(), 2);
        assert!(state.release_all().is_empty());
    }
}
