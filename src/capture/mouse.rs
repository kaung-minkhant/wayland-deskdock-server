//! Relative mouse frames to pointer motion, buttons and wheel steps.

use evdev::{EventSummary, KeyCode, RelativeAxisCode, SynchronizationCode};

use super::{InputEvent, MouseButton};

#[derive(Default)]
pub struct MouseState {
    pending_x: i32,
    pending_y: i32,
}

impl MouseState {
    pub fn translate(&mut self, event: EventSummary) -> Option<InputEvent> {
        match event {
            EventSummary::RelativeAxis(_, RelativeAxisCode::REL_X, x) => {
                self.pending_x += x;
                None
            }
            EventSummary::RelativeAxis(_, RelativeAxisCode::REL_Y, y) => {
                self.pending_y += y;
                None
            }
            // A dropped event means relative movement is incomplete; discard
            // it rather than turning an invalid frame boundary into a jump.
            EventSummary::Synchronization(_, SynchronizationCode::SYN_DROPPED, _) => {
                self.pending_x = 0;
                self.pending_y = 0;
                None
            }
            EventSummary::Synchronization(_, SynchronizationCode::SYN_REPORT, _)
                if self.pending_x != 0 || self.pending_y != 0 =>
            {
                Some(InputEvent::Motion {
                    x: std::mem::take(&mut self.pending_x),
                    y: std::mem::take(&mut self.pending_y),
                })
            }
            EventSummary::RelativeAxis(_, RelativeAxisCode::REL_WHEEL, y) if y != 0 => {
                Some(InputEvent::Wheel { y })
            }
            EventSummary::Key(_, KeyCode::BTN_LEFT, value @ (0 | 1)) => Some(InputEvent::Button {
                down: value == 1,
                button: MouseButton::Left,
            }),
            EventSummary::Key(_, KeyCode::BTN_RIGHT, value @ (0 | 1)) => Some(InputEvent::Button {
                down: value == 1,
                button: MouseButton::Right,
            }),
            EventSummary::Key(_, KeyCode::BTN_MIDDLE, value @ (0 | 1)) => {
                Some(InputEvent::Button {
                    down: value == 1,
                    button: MouseButton::Middle,
                })
            }
            _ => None,
        }
    }
}
