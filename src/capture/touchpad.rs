//! Touchpad contact tracking: relative motion, tap and tap-drag, two-finger
//! scroll and the two-finger back swipe.
//!
//! The raw evdev device is grabbed, so the compositor no longer synthesizes any
//! of these gestures; they are recognized here instead.

use std::time::{Duration, Instant};

use evdev::{AbsoluteAxisCode, Device, EventSummary, KeyCode, SynchronizationCode};

use super::{InputEvent, MouseButton};

const TAP_MAX_DURATION: Duration = Duration::from_millis(250);
const TAP_MAX_MOVEMENT_PERCENT: i32 = 1;
const BACK_SWIPE_PERCENT: i32 = 15;

pub struct TouchpadState {
    scroll_percent: i32,
    pub tap_delay: Duration,
    active_slot: i32,
    primary_tracking: bool,
    secondary_tracking: bool,
    finger_count: u8,
    last_x: Option<i32>,
    last_y: Option<i32>,
    current_x: Option<i32>,
    current_y: Option<i32>,
    x_range: i32,
    y_range: i32,
    two_finger_last_x: Option<i32>,
    two_finger_last_y: Option<i32>,
    // Accumulate small deltas so one physical scroll step maps to one wheel
    // event instead of one for every evdev report.
    two_finger_wheel_remainder: f64,
    two_finger_x_total: i32,
    two_finger_y_total: i32,
    back_swipe_started: bool,
    last_tap: Option<(Instant, u64)>,
    next_tap_id: u64,
    tap_dragging: bool,
    contact_started: Option<Instant>,
    tap_start_x: Option<i32>,
    tap_start_y: Option<i32>,
    tap_cancelled: bool,
    // Preserve sub-pixel movement between frames; independent rounding makes
    // slow diagonal motion collapse into horizontal/vertical stair-steps.
    x_remainder: f64,
    y_remainder: f64,
}

impl TouchpadState {
    pub fn new(device: &Device, scroll_percent: i32, tap_delay: Duration) -> Self {
        let info = device.get_abs_state().ok();
        let range = |axis: AbsoluteAxisCode| {
            info.as_ref()
                .and_then(|state| state.get(axis.0 as usize))
                .map(|state| state.maximum - state.minimum)
                .unwrap_or(1)
                .max(1)
        };
        Self::with_ranges(
            scroll_percent,
            tap_delay,
            range(AbsoluteAxisCode::ABS_MT_POSITION_X),
            range(AbsoluteAxisCode::ABS_MT_POSITION_Y),
        )
    }

    fn with_ranges(scroll_percent: i32, tap_delay: Duration, x_range: i32, y_range: i32) -> Self {
        Self {
            scroll_percent,
            tap_delay,
            active_slot: 0,
            primary_tracking: false,
            secondary_tracking: false,
            finger_count: 0,
            last_x: None,
            last_y: None,
            current_x: None,
            current_y: None,
            x_range,
            y_range,
            two_finger_last_x: None,
            two_finger_last_y: None,
            two_finger_wheel_remainder: 0.0,
            two_finger_x_total: 0,
            two_finger_y_total: 0,
            back_swipe_started: false,
            last_tap: None,
            next_tap_id: 0,
            tap_dragging: false,
            contact_started: None,
            tap_start_x: None,
            tap_start_y: None,
            tap_cancelled: false,
            x_remainder: 0.0,
            y_remainder: 0.0,
        }
    }

    pub fn translate(&mut self, event: EventSummary) -> Option<InputEvent> {
        match event {
            // Position events are scoped to the most recently selected slot.
            // Follow only slot 0, the primary contact.
            EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_MT_SLOT, slot) => {
                self.active_slot = slot;
                None
            }
            EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_MT_TRACKING_ID, tracking_id)
                if self.active_slot == 0 =>
            {
                self.primary_tracking = tracking_id >= 0;
                if self.primary_tracking {
                    self.contact_started = Some(Instant::now());
                    self.tap_start_x = None;
                    self.tap_start_y = None;
                    self.current_x = None;
                    self.current_y = None;
                    self.tap_cancelled = self.finger_count > 1;
                    let cancel_id = self
                        .last_tap
                        .take()
                        .and_then(|(tap, id)| (tap.elapsed() <= self.tap_delay).then_some(id));
                    self.tap_dragging = cancel_id.is_some();
                    cancel_id.map(|cancel_id| InputEvent::StartTapDrag { cancel_id })
                } else {
                    let was_tap = self.is_tap();
                    let was_dragging = std::mem::take(&mut self.tap_dragging);
                    self.contact_started = None;
                    self.tap_start_x = None;
                    self.tap_start_y = None;
                    self.reset_motion();
                    if was_dragging {
                        Some(InputEvent::Button {
                            down: false,
                            button: MouseButton::Left,
                        })
                    } else if was_tap {
                        self.next_tap_id = self.next_tap_id.wrapping_add(1);
                        let id = self.next_tap_id;
                        self.last_tap = Some((Instant::now(), id));
                        Some(InputEvent::DelayedTap { id })
                    } else {
                        self.last_tap = None;
                        None
                    }
                }
            }
            EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_MT_TRACKING_ID, tracking_id)
                if self.active_slot == 1 =>
            {
                self.secondary_tracking = tracking_id >= 0;
                None
            }
            EventSummary::Key(_, KeyCode::BTN_TOOL_FINGER, value @ (0 | 1)) => {
                self.finger_count = value as u8;
                if self.finger_count != 1 {
                    if self.finger_count > 1 {
                        self.cancel_tap();
                    }
                    self.reset_motion();
                    self.reset_two_finger_gesture();
                }
                None
            }
            EventSummary::Key(_, KeyCode::BTN_TOOL_DOUBLETAP, 1) => {
                self.start_multi_finger(2);
                None
            }
            EventSummary::Key(_, KeyCode::BTN_TOOL_TRIPLETAP, 1) => {
                self.start_multi_finger(3);
                None
            }
            EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_MT_POSITION_X, value)
                if self.active_slot == 0 =>
            {
                self.current_x = Some(value);
                self.tap_start_x.get_or_insert(value);
                self.update_tap_movement();
                None
            }
            EventSummary::AbsoluteAxis(_, AbsoluteAxisCode::ABS_MT_POSITION_Y, value)
                if self.active_slot == 0 =>
            {
                self.current_y = Some(value);
                self.tap_start_y.get_or_insert(value);
                self.update_tap_movement();
                None
            }
            // `fetch_events` does not repair state after SYN_DROPPED. Forget
            // the baseline so the next complete report only establishes a
            // position; never infer motion across lost input events.
            EventSummary::Synchronization(_, SynchronizationCode::SYN_DROPPED, _) => {
                self.cancel_tap();
                self.last_tap = None;
                self.reset_motion();
                self.reset_two_finger_gesture();
                None
            }
            EventSummary::Synchronization(_, SynchronizationCode::SYN_REPORT, _)
                if self.primary_tracking && self.secondary_tracking && self.finger_count == 2 =>
            {
                self.two_finger_frame()
            }
            EventSummary::Synchronization(_, SynchronizationCode::SYN_REPORT, _)
                if self.primary_tracking && self.finger_count == 1 =>
            {
                self.one_finger_frame()
            }
            _ => None,
        }
    }

    fn start_multi_finger(&mut self, fingers: u8) {
        self.finger_count = fingers;
        self.cancel_tap();
        self.reset_motion();
        self.reset_two_finger_gesture();
    }

    fn two_finger_frame(&mut self) -> Option<InputEvent> {
        let (x, y) = (self.current_x?, self.current_y?);
        let previous = (
            self.two_finger_last_x.replace(x),
            self.two_finger_last_y.replace(y),
        );
        let (Some(last_x), Some(last_y)) = previous else {
            return None;
        };
        let delta_x = x - last_x;
        let delta_y = y - last_y;
        self.two_finger_x_total += delta_x;
        self.two_finger_y_total += delta_y;

        if !self.back_swipe_started
            && self.two_finger_x_total * 100 >= self.x_range * BACK_SWIPE_PERCENT
            && self.two_finger_x_total.abs() > self.two_finger_y_total.abs()
        {
            self.back_swipe_started = true;
            Some(InputEvent::BackGesture)
        } else if !self.back_swipe_started && delta_y != 0 && delta_y.abs() >= delta_x.abs() {
            // Wheel events are discrete: accumulate touchpad travel so scrolling
            // stays responsive without a full step for every evdev report.
            self.two_finger_wheel_remainder += f64::from(delta_y) * 100.0
                / (f64::from(self.y_range) * f64::from(self.scroll_percent));
            let steps = self.two_finger_wheel_remainder.trunc() as i32;
            self.two_finger_wheel_remainder -= f64::from(steps);
            (steps != 0).then_some(InputEvent::Wheel { y: steps })
        } else {
            None
        }
    }

    fn one_finger_frame(&mut self) -> Option<InputEvent> {
        let (x, y) = (self.current_x?, self.current_y?);
        let previous = (self.last_x.replace(x), self.last_y.replace(y));
        let (Some(last_x), Some(last_y)) = previous else {
            return None;
        };
        self.x_remainder += f64::from(x - last_x) * 1000.0 / f64::from(self.x_range);
        self.y_remainder += f64::from(y - last_y) * 1000.0 / f64::from(self.y_range);
        let delta_x = self.x_remainder.trunc() as i32;
        let delta_y = self.y_remainder.trunc() as i32;
        self.x_remainder -= f64::from(delta_x);
        self.y_remainder -= f64::from(delta_y);
        (delta_x != 0 || delta_y != 0).then_some(InputEvent::Motion {
            x: delta_x,
            y: delta_y,
        })
    }

    fn reset_two_finger_gesture(&mut self) {
        self.two_finger_last_x = None;
        self.two_finger_last_y = None;
        self.two_finger_wheel_remainder = 0.0;
        self.two_finger_x_total = 0;
        self.two_finger_y_total = 0;
        self.back_swipe_started = false;
    }

    fn is_tap(&self) -> bool {
        !self.tap_cancelled
            && self
                .contact_started
                .is_some_and(|started| started.elapsed() <= TAP_MAX_DURATION)
    }

    fn update_tap_movement(&mut self) {
        let (Some(start_x), Some(start_y), Some(x), Some(y)) = (
            self.tap_start_x,
            self.tap_start_y,
            self.current_x,
            self.current_y,
        ) else {
            return;
        };
        let max_x = (self.x_range * TAP_MAX_MOVEMENT_PERCENT / 100).max(1);
        let max_y = (self.y_range * TAP_MAX_MOVEMENT_PERCENT / 100).max(1);
        if (x - start_x).abs() > max_x || (y - start_y).abs() > max_y {
            self.cancel_tap();
        }
    }

    fn cancel_tap(&mut self) {
        self.tap_cancelled = true;
    }

    /// Forget the motion baseline. Events are ignored in Linux mode, so the
    /// first contact after a grab must not be compared with stale coordinates.
    pub fn reset_motion(&mut self) {
        self.last_x = None;
        self.last_y = None;
        self.current_x = None;
        self.current_y = None;
        self.x_remainder = 0.0;
        self.y_remainder = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use evdev::{AbsoluteAxisEvent, KeyEvent, SynchronizationEvent};

    fn touchpad() -> TouchpadState {
        let mut state = TouchpadState::with_ranges(5, Duration::from_millis(500), 1_000, 1_000);
        state.finger_count = 1;
        state
    }

    fn abs(state: &mut TouchpadState, axis: AbsoluteAxisCode, value: i32) -> Option<InputEvent> {
        state.translate(EventSummary::AbsoluteAxis(
            AbsoluteAxisEvent::new(axis, value),
            axis,
            value,
        ))
    }

    fn sync(state: &mut TouchpadState) -> Option<InputEvent> {
        state.translate(EventSummary::Synchronization(
            SynchronizationEvent::new(SynchronizationCode::SYN_REPORT, 0),
            SynchronizationCode::SYN_REPORT,
            0,
        ))
    }

    fn position(state: &mut TouchpadState, x: i32, y: i32) -> Option<InputEvent> {
        abs(state, AbsoluteAxisCode::ABS_MT_POSITION_X, x);
        abs(state, AbsoluteAxisCode::ABS_MT_POSITION_Y, y);
        sync(state)
    }

    #[test]
    fn single_finger_tap_emits_delayed_tap() {
        let mut state = touchpad();
        assert!(abs(&mut state, AbsoluteAxisCode::ABS_MT_TRACKING_ID, 1).is_none());
        position(&mut state, 500, 500);
        assert!(matches!(
            abs(&mut state, AbsoluteAxisCode::ABS_MT_TRACKING_ID, -1),
            Some(InputEvent::DelayedTap { id: 1 })
        ));
    }

    #[test]
    fn moving_contact_is_not_a_tap() {
        let mut state = touchpad();
        abs(&mut state, AbsoluteAxisCode::ABS_MT_TRACKING_ID, 1);
        position(&mut state, 500, 500);
        position(&mut state, 520, 500);
        assert!(abs(&mut state, AbsoluteAxisCode::ABS_MT_TRACKING_ID, -1).is_none());
    }

    #[test]
    fn first_frame_sets_baseline_then_motion_is_relative() {
        let mut state = touchpad();
        abs(&mut state, AbsoluteAxisCode::ABS_MT_TRACKING_ID, 1);
        assert!(position(&mut state, 100, 100).is_none());
        assert!(matches!(
            position(&mut state, 110, 100),
            Some(InputEvent::Motion { x: 10, y: 0 })
        ));
    }

    #[test]
    fn host_click_is_not_handled_here() {
        // BTN_LEFT is translated by the device worker, not the contact tracker.
        let mut state = touchpad();
        assert!(
            state
                .translate(EventSummary::Key(
                    KeyEvent::new(KeyCode::BTN_LEFT, 1),
                    KeyCode::BTN_LEFT,
                    1
                ))
                .is_none()
        );
    }
}
