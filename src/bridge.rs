//! Forward captured host input to Android over the native DeskDock session.

use std::error::Error;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use crate::adb::Adb;
use crate::capture::{self, InputEvent, MouseButton};
use crate::clipboard::Clipboard;
use crate::config::Config;
use crate::control::ControlSocket;
use crate::input::Event;
use crate::keyboard::{KeyAction, KeyboardState};
use crate::protocol::ProtocolError;
use crate::session::{ActiveSession, connect_by_serial, print_connection};
use crate::state::Shared;

const POLL_INTERVAL: Duration = Duration::from_millis(2);
const RETRY_MIN: Duration = Duration::from_secs(2);
const RETRY_MAX: Duration = Duration::from_secs(15);
const CLIPBOARD_POLL_INTERVAL: Duration = Duration::from_millis(700);
const MAX_CLIPBOARD_BYTES: usize = 1024 * 1024;

/// Pointer position on the Android display, in its coordinate space.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Pointer {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    cancelled_tap: Option<u64>,
}

impl Pointer {
    fn centered(width: i32, height: i32) -> Self {
        Self {
            x: width / 2,
            y: height / 2,
            width,
            height,
            cancelled_tap: None,
        }
    }

    fn center(&mut self) {
        self.x = self.width / 2;
        self.y = self.height / 2;
    }

    fn move_by(&mut self, x: i32, y: i32) {
        self.x = (self.x + x).clamp(0, self.width - 1);
        self.y = (self.y + y).clamp(0, self.height - 1);
    }

    /// Adopt a new display size, keeping the pointer at the same relative spot.
    /// Returns true when the size actually changed.
    fn resize(&mut self, width: i32, height: i32) -> bool {
        if (width, height) == (self.width, self.height) || width <= 0 || height <= 0 {
            return false;
        }
        self.x = (self.x * width / self.width).clamp(0, width - 1);
        self.y = (self.y * height / self.height).clamp(0, height - 1);
        self.width = width;
        self.height = height;
        true
    }

    fn position(&self) -> (u32, u32) {
        (self.x as u32, self.y as u32)
    }

    fn cancel_tap(&mut self, id: u64) {
        self.cancelled_tap = Some(id);
    }

    /// False when a later touch turned this delayed tap into a drag.
    fn confirm_tap(&mut self, id: u64) -> bool {
        if self.cancelled_tap == Some(id) {
            self.cancelled_tap = None;
            false
        } else {
            true
        }
    }
}

/// Translates captured events into family-2 packets. It never touches the
/// network itself, so the translation is unit-testable.
struct Forwarder {
    pointer: Pointer,
    keyboard: KeyboardState,
    sensitivity: i32,
}

impl Forwarder {
    fn new(width: i32, height: i32, sensitivity: i32) -> Self {
        Self {
            pointer: Pointer::centered(width, height),
            keyboard: KeyboardState::default(),
            sensitivity,
        }
    }

    /// React to a display size change announced by Android (rotation). The
    /// returned move keeps Android's cursor at the rescaled position.
    fn resize(&mut self, width: i32, height: i32) -> Option<Event> {
        self.pointer.resize(width, height).then(|| {
            let (x, y) = self.pointer.position();
            Event::PointerMove { x, y }
        })
    }

    fn enter_tablet(&mut self) -> Vec<Event> {
        self.pointer.center();
        let (x, y) = self.pointer.position();
        vec![Event::SetEnabled(true), Event::PointerMove { x, y }]
    }

    fn leave_tablet(&mut self) -> Vec<Event> {
        // Never leave a key or modifier stuck down on Android.
        let mut events = self.keyboard.release_all();
        events.push(Event::SetEnabled(false));
        events
    }

    /// Events to send, and whether the user asked to return to Linux.
    fn translate(&mut self, event: InputEvent) -> (Vec<Event>, bool) {
        let (x, y) = self.pointer.position();
        let click = |button: MouseButton| {
            vec![
                Event::PointerButton {
                    button: button.awt_number(),
                    is_down: true,
                    x,
                    y,
                },
                Event::PointerButton {
                    button: button.awt_number(),
                    is_down: false,
                    x,
                    y,
                },
            ]
        };
        let events = match event {
            InputEvent::Key { code, down, repeat } => {
                let (action, events) = self.keyboard.handle(code, down, repeat);
                return (events, action == KeyAction::ReturnToLinux);
            }
            InputEvent::Motion { x, y } => {
                self.pointer
                    .move_by(x * self.sensitivity, y * self.sensitivity);
                let (x, y) = self.pointer.position();
                vec![Event::PointerMove { x, y }]
            }
            InputEvent::Button { down, button } => vec![Event::PointerButton {
                button: button.awt_number(),
                is_down: down,
                x,
                y,
            }],
            InputEvent::Tap { id } if self.pointer.confirm_tap(id) => click(MouseButton::Left),
            InputEvent::StartTapDrag { cancel_id } => {
                self.pointer.cancel_tap(cancel_id);
                vec![Event::PointerButton {
                    button: MouseButton::Left.awt_number(),
                    is_down: true,
                    x,
                    y,
                }]
            }
            InputEvent::Wheel { y: steps } => vec![Event::Scroll {
                x,
                y,
                horizontal: 0,
                vertical: steps.signum(),
            }],
            // DeskDock maps its right mouse button to Android Back.
            InputEvent::BackGesture => click(MouseButton::Right),
            InputEvent::Tap { .. } | InputEvent::DelayedTap { .. } | InputEvent::Error { .. } => {
                Vec::new()
            }
        };
        (events, false)
    }
}

/// How a session's event loop ended.
enum Exit {
    Shutdown,
    /// The tablet went away; the supervisor should reconnect.
    Lost(String),
}

pub fn run(adb: &Adb, serial: &str, config: Config, shared: Shared) -> Result<(), Box<dyn Error>> {
    let (sender, receiver) = mpsc::channel();
    let workers = capture::spawn(&config, &shared, &sender)?;
    drop(sender);
    let socket = ControlSocket::bind(shared.clone())?;
    eprintln!(
        "Linux mode: devices stay with the compositor. Run `deskdock-server toggle` \
         (control socket {}) to hand them to the tablet.",
        socket.path().display()
    );
    if config
        .devices
        .iter()
        .any(|device| device.kind == crate::config::DeviceKind::Keyboard)
    {
        eprintln!("Keyboard forwarding is on: press Super+Shift+T to return to Linux.");
    }

    let result = supervise(adb, serial, &config, &receiver, &shared);

    // Whatever happened, give the devices back to the compositor.
    shared.set_connected(false);
    shared.set_tablet(false);
    capture::stop(&shared, workers);
    drop(socket);
    result
}

/// Keep a session with the tablet alive: connect, forward input, and start
/// over with a growing delay whenever the tablet disconnects.
fn supervise(
    adb: &Adb,
    serial: &str,
    config: &Config,
    receiver: &Receiver<InputEvent>,
    shared: &Shared,
) -> Result<(), Box<dyn Error>> {
    let mut delay = RETRY_MIN;
    while !shared.is_shutdown() {
        eprintln!("Connecting to {serial}...");
        match connect_by_serial(adb, serial, &shared.shutdown) {
            Ok(mut active) => {
                let (width, height) = active.display_size();
                if width <= 0 || height <= 0 {
                    eprintln!("device announced an invalid display size; retrying");
                } else {
                    print_connection(&active);
                    delay = RETRY_MIN;
                    match run_session(&mut active, config, receiver, shared)? {
                        Exit::Shutdown => return Ok(()),
                        Exit::Lost(reason) => {
                            eprintln!("Lost the tablet ({reason}); devices were released.")
                        }
                    }
                }
            }
            Err(_) if shared.is_shutdown() => return Ok(()),
            Err(error) => eprintln!("Could not connect: {error}"),
        }

        eprintln!("Retrying in {} s.", delay.as_secs());
        wait_for_retry(receiver, shared, delay)?;
        delay = (delay * 2).min(RETRY_MAX);
    }
    Ok(())
}

/// Sleep until the next attempt while still noticing worker failures.
fn wait_for_retry(
    receiver: &Receiver<InputEvent>,
    shared: &Shared,
    delay: Duration,
) -> Result<(), Box<dyn Error>> {
    let deadline = Instant::now() + delay;
    while Instant::now() < deadline && !shared.is_shutdown() {
        match receiver.recv_timeout(POLL_INTERVAL) {
            Ok(InputEvent::Error { path, error }) => {
                return Err(format!("input worker for {} stopped: {error}", path.display()).into());
            }
            Ok(_) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                return Err("all input workers stopped".into());
            }
        }
    }
    Ok(())
}

/// Forward input over one established session. `Err` is fatal (a capture
/// worker failed); a lost connection is reported as [`Exit::Lost`].
fn run_session(
    active: &mut ActiveSession<'_>,
    config: &Config,
    receiver: &Receiver<InputEvent>,
    shared: &Shared,
) -> Result<Exit, Box<dyn Error>> {
    // Anything captured while disconnected is stale.
    while receiver.try_recv().is_ok() {}

    let (width, height) = active.display_size();
    let mut forwarder = Forwarder::new(width, height, config.tablet.sensitivity);
    let mut clipboard = Clipboard::new();
    let mut next_clipboard_poll = Instant::now();
    shared.set_connected(true);
    let result = event_loop(
        active,
        &mut forwarder,
        &mut clipboard,
        &mut next_clipboard_poll,
        receiver,
        shared,
    );

    // Release the devices first, so a dead connection can never leave them
    // grabbed, then tell Android to stand down if it can still hear us.
    shared.set_connected(false);
    shared.set_tablet(false);
    if !matches!(result, Ok(Exit::Lost(_)))
        && let Err(error) = send_all(active, &forwarder.leave_tablet())
    {
        eprintln!("could not tell Android to disable input: {error}");
    }
    result
}

fn event_loop(
    active: &mut ActiveSession<'_>,
    forwarder: &mut Forwarder,
    clipboard: &mut Clipboard,
    next_clipboard_poll: &mut Instant,
    receiver: &Receiver<InputEvent>,
    shared: &Shared,
) -> Result<Exit, Box<dyn Error>> {
    let mut in_tablet = false;
    while !shared.is_shutdown() {
        if active.is_lost() {
            return Ok(Exit::Lost("the connection closed".to_owned()));
        }

        let (width, height) = active.display_size();
        if let Some(event) = forwarder.resize(width, height)
            && in_tablet
            && let Err(error) = active.send_input(&event)
        {
            return Ok(Exit::Lost(error.to_string()));
        }

        for text in active.take_clipboard_updates() {
            if text.len() > MAX_CLIPBOARD_BYTES {
                eprintln!("ignoring Android clipboard text larger than 1 MiB");
            } else if let Err(error) = clipboard.set_from_android(text) {
                eprintln!("could not update the host clipboard: {error}");
            } else {
                eprintln!("updated host clipboard from Android");
            }
        }
        if Instant::now() >= *next_clipboard_poll {
            match clipboard.poll_local_change() {
                Ok(Some(text)) if text.len() <= MAX_CLIPBOARD_BYTES => {
                    if let Err(error) = active.send_input(&Event::Clipboard { text }) {
                        return Ok(Exit::Lost(error.to_string()));
                    }
                }
                Ok(Some(_)) => eprintln!("ignoring clipboard text larger than 1 MiB"),
                Ok(None) => {}
                Err(error) => eprintln!("could not read the host clipboard: {error}"),
            }
            *next_clipboard_poll = Instant::now() + CLIPBOARD_POLL_INTERVAL;
        }

        let want_tablet = shared.is_tablet();
        if want_tablet != in_tablet {
            let events = if want_tablet {
                eprintln!("Tablet mode: devices are captured and sent to Android.");
                forwarder.enter_tablet()
            } else {
                eprintln!("Linux mode: devices were released.");
                forwarder.leave_tablet()
            };
            if let Err(error) = send_all(active, &events) {
                return Ok(Exit::Lost(error.to_string()));
            }
            in_tablet = want_tablet;
        }

        match receiver.recv_timeout(POLL_INTERVAL) {
            Ok(event) => {
                if let Some(exit) = handle(active, forwarder, shared, event, in_tablet)? {
                    return Ok(exit);
                }
                // Drain queued frames so a busy pointer cannot build a backlog.
                while let Ok(event) = receiver.try_recv() {
                    if let Some(exit) = handle(active, forwarder, shared, event, in_tablet)? {
                        return Ok(exit);
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                return Err("all input workers stopped".into());
            }
        }
    }
    Ok(Exit::Shutdown)
}

fn handle(
    active: &mut ActiveSession<'_>,
    forwarder: &mut Forwarder,
    shared: &Shared,
    event: InputEvent,
    in_tablet: bool,
) -> Result<Option<Exit>, Box<dyn Error>> {
    if let InputEvent::Error { path, error } = &event {
        return Err(format!("input worker for {} stopped: {error}", path.display()).into());
    }
    if !in_tablet {
        return Ok(None);
    }
    let (events, return_to_linux) = forwarder.translate(event);
    if let Err(error) = send_all(active, &events) {
        return Ok(Some(Exit::Lost(error.to_string())));
    }
    if return_to_linux {
        shared.set_tablet(false);
    }
    Ok(None)
}

fn send_all(active: &mut ActiveSession<'_>, events: &[Event]) -> Result<(), ProtocolError> {
    events.iter().try_for_each(|event| active.send_input(event))
}

#[cfg(test)]
mod tests {
    use super::*;
    use evdev::KeyCode;

    fn forwarder() -> Forwarder {
        Forwarder::new(1000, 800, 3)
    }

    #[test]
    fn entering_tablet_enables_input_and_centers_the_pointer() {
        assert_eq!(
            forwarder().enter_tablet(),
            [
                Event::SetEnabled(true),
                Event::PointerMove { x: 500, y: 400 }
            ]
        );
    }

    #[test]
    fn rotation_rescales_the_pointer_and_the_clamp() {
        let mut forwarder = forwarder();
        forwarder.translate(InputEvent::Motion { x: 10, y: 0 }); // x = 530
        assert_eq!(
            forwarder.resize(800, 1000),
            Some(Event::PointerMove { x: 424, y: 500 })
        );
        assert_eq!(forwarder.resize(800, 1000), None);
        assert_eq!(forwarder.resize(0, 5), None);
        let (events, _) = forwarder.translate(InputEvent::Motion {
            x: 10_000,
            y: 10_000,
        });
        assert_eq!(events, [Event::PointerMove { x: 799, y: 999 }]);
    }

    #[test]
    fn motion_is_scaled_and_clamped_to_the_display() {
        let mut forwarder = forwarder();
        let (events, _) = forwarder.translate(InputEvent::Motion { x: 10, y: -5 });
        assert_eq!(events, [Event::PointerMove { x: 530, y: 385 }]);
        let (events, _) = forwarder.translate(InputEvent::Motion {
            x: 10_000,
            y: -10_000,
        });
        assert_eq!(events, [Event::PointerMove { x: 999, y: 0 }]);
    }

    #[test]
    fn buttons_use_the_current_pointer_position() {
        let mut forwarder = forwarder();
        let (events, _) = forwarder.translate(InputEvent::Button {
            down: true,
            button: MouseButton::Right,
        });
        assert_eq!(
            events,
            [Event::PointerButton {
                button: 3,
                is_down: true,
                x: 500,
                y: 400
            }]
        );
    }

    #[test]
    fn a_tap_that_became_a_drag_is_not_clicked() {
        let mut forwarder = forwarder();
        forwarder.translate(InputEvent::StartTapDrag { cancel_id: 7 });
        assert!(forwarder.translate(InputEvent::Tap { id: 7 }).0.is_empty());
        assert_eq!(forwarder.translate(InputEvent::Tap { id: 8 }).0.len(), 2);
    }

    #[test]
    fn wheel_steps_are_normalized_to_one_notch() {
        let mut forwarder = forwarder();
        let (events, _) = forwarder.translate(InputEvent::Wheel { y: -4 });
        assert_eq!(
            events,
            [Event::Scroll {
                x: 500,
                y: 400,
                horizontal: 0,
                vertical: -1
            }]
        );
    }

    #[test]
    fn leaving_tablet_releases_keys_before_disabling_input() {
        let mut forwarder = forwarder();
        forwarder.translate(InputEvent::Key {
            code: KeyCode::KEY_A,
            down: true,
            repeat: false,
        });
        let events = forwarder.leave_tablet();
        assert!(matches!(
            events.as_slice(),
            [Event::Key { is_down: false, .. }, Event::SetEnabled(false)]
        ));
    }

    #[test]
    fn the_escape_chord_requests_a_return_to_linux() {
        let mut forwarder = forwarder();
        for code in [KeyCode::KEY_LEFTMETA, KeyCode::KEY_LEFTSHIFT] {
            forwarder.translate(InputEvent::Key {
                code,
                down: true,
                repeat: false,
            });
        }
        let (_, back) = forwarder.translate(InputEvent::Key {
            code: KeyCode::KEY_T,
            down: true,
            repeat: false,
        });
        assert!(back);
    }
}
