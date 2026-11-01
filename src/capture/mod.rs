//! Linux evdev capture of explicitly configured mouse, touchpad and keyboard
//! devices.
//!
//! Each device gets a worker thread. While tablet mode is on, the worker takes
//! an exclusive grab and translates raw events into [`InputEvent`]s; otherwise
//! the device stays with the compositor and its events are ignored. A grab is
//! always dropped when the worker stops, and the kernel also releases it if the
//! process dies.

mod mouse;
mod touchpad;

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::Sender;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use evdev::{AbsoluteAxisCode, Device, EventSummary, KeyCode, RelativeAxisCode};

use crate::config::{Config, Device as ConfiguredDevice, DeviceKind};
use crate::state::Shared;
use mouse::MouseState;
use touchpad::TouchpadState;

/// Short on purpose: long polls make multi-device pointer motion visibly laggy.
const POLL_INTERVAL: Duration = Duration::from_millis(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

impl MouseButton {
    /// AWT button numbering, as used by the original server's packets.
    pub const fn awt_number(self) -> u32 {
        match self {
            Self::Left => 1,
            Self::Middle => 2,
            Self::Right => 3,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum InputEvent {
    Motion {
        x: i32,
        y: i32,
    },
    Button {
        down: bool,
        button: MouseButton,
    },
    /// A tap whose click is held back so a following touch can turn it into a
    /// drag. The worker re-sends it as [`InputEvent::Tap`] after the delay.
    DelayedTap {
        id: u64,
    },
    Tap {
        id: u64,
    },
    StartTapDrag {
        cancel_id: u64,
    },
    Wheel {
        y: i32,
    },
    BackGesture,
    Key {
        code: KeyCode,
        down: bool,
        repeat: bool,
    },
    Error {
        path: PathBuf,
        error: String,
    },
}

/// Open and validate every configured device up front so a bad path fails
/// before the session starts, then start one worker per device.
pub fn spawn(
    config: &Config,
    shared: &Shared,
    events: &Sender<InputEvent>,
) -> Result<Vec<JoinHandle<()>>, Box<dyn Error>> {
    let mut opened = Vec::new();
    for configured in &config.devices {
        let device = Device::open(&configured.path).map_err(|error| {
            format!(
                "could not open {}: {error} (is your user in the `input` group?)",
                configured.path.display()
            )
        })?;
        validate_device(&device, configured)?;
        device.set_nonblocking(true).map_err(|error| {
            format!(
                "could not make {} nonblocking: {error}",
                configured.path.display()
            )
        })?;
        opened.push((configured.clone(), device));
    }

    Ok(opened
        .into_iter()
        .map(|(configured, device)| {
            let tablet = config.tablet.clone();
            let shared = shared.clone();
            let events = events.clone();
            thread::spawn(move || {
                run_worker(
                    configured,
                    device,
                    &shared,
                    &events,
                    tablet.touchpad_scroll_percent,
                    Duration::from_millis(tablet.touchpad_tap_delay_ms),
                );
            })
        })
        .collect())
}

fn run_worker(
    configured: ConfiguredDevice,
    mut device: Device,
    shared: &Shared,
    events: &Sender<InputEvent>,
    scroll_percent: i32,
    tap_delay: Duration,
) {
    let path = configured.path;
    let kind = configured.kind;
    let mut grabbed = false;
    let mut mouse = MouseState::default();
    let mut touchpad = TouchpadState::new(&device, scroll_percent, tap_delay);

    let fail = |message: String| {
        let _ = events.send(InputEvent::Error {
            path: path.clone(),
            error: message,
        });
        shared.set_tablet(false);
    };

    while !shared.is_shutdown() {
        let mut should_grab = shared.is_tablet();
        // The tablet-mode shortcut is still held when the toggle arrives.
        // Grabbing now would hide the key releases from the compositor and
        // leave its modifiers stuck, so a keyboard waits until every key is up.
        if should_grab
            && !grabbed
            && kind == DeviceKind::Keyboard
            && device
                .get_key_state()
                .map(|keys| keys.iter().next().is_some())
                .unwrap_or(false)
        {
            should_grab = false;
        }
        if should_grab != grabbed {
            let result = if should_grab {
                device.grab()
            } else {
                device.ungrab()
            };
            if let Err(error) = result {
                fail(format!(
                    "could not {} {}: {error}",
                    if should_grab { "grab" } else { "release" },
                    path.display()
                ));
                return;
            }
            grabbed = should_grab;
            // Events were ignored in Linux mode, so absolute coordinates cannot
            // be compared with the first contact after the grab.
            if grabbed && kind == DeviceKind::Touchpad {
                touchpad.reset_motion();
            }
        }

        let fetched = match device.fetch_events() {
            Ok(fetched) => fetched,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(POLL_INTERVAL);
                continue;
            }
            Err(error) => {
                fail(format!("could not read {}: {error}", path.display()));
                return;
            }
        };
        for event in fetched {
            // A keyboard must never reach both machines: forward only while it
            // is actually grabbed away from the compositor.
            if !shared.is_tablet() || (kind == DeviceKind::Keyboard && !grabbed) {
                continue;
            }
            let Some(event) = translate(kind, &mut mouse, &mut touchpad, event.destructure())
            else {
                continue;
            };
            if let InputEvent::DelayedTap { id } = event {
                let events = events.clone();
                let delay = touchpad.tap_delay;
                thread::spawn(move || {
                    thread::sleep(delay);
                    let _ = events.send(InputEvent::Tap { id });
                });
            } else if events.send(event).is_err() {
                return;
            }
        }
    }
    // `device` is dropped here, which releases any grab.
}

fn translate(
    kind: DeviceKind,
    mouse: &mut MouseState,
    touchpad: &mut TouchpadState,
    event: EventSummary,
) -> Option<InputEvent> {
    match kind {
        DeviceKind::Mouse => mouse.translate(event),
        DeviceKind::Keyboard => match event {
            EventSummary::Key(_, code, value @ 0..=2) => Some(InputEvent::Key {
                code,
                down: value != 0,
                repeat: value == 2,
            }),
            _ => None,
        },
        DeviceKind::Touchpad => match event {
            // Keep the touchpad's physical click; contact coordinates are used
            // only for relative pointer movement.
            EventSummary::Key(_, KeyCode::BTN_LEFT, value @ (0 | 1)) => Some(InputEvent::Button {
                down: value == 1,
                button: MouseButton::Left,
            }),
            _ => touchpad.translate(event),
        },
    }
}

fn validate_device(device: &Device, configured: &ConfiguredDevice) -> Result<(), Box<dyn Error>> {
    let (matches, label) = match configured.kind {
        DeviceKind::Mouse => (is_relative_mouse(device), "relative mouse"),
        DeviceKind::Touchpad => (is_touchpad(device), "touchpad"),
        DeviceKind::Keyboard => (is_keyboard(device), "keyboard"),
    };
    if matches {
        Ok(())
    } else {
        Err(format!(
            "{} is not a supported {label} device",
            configured.path.display()
        )
        .into())
    }
}

fn is_relative_mouse(device: &Device) -> bool {
    device.supported_relative_axes().is_some_and(|axes| {
        axes.contains(RelativeAxisCode::REL_X) && axes.contains(RelativeAxisCode::REL_Y)
    }) && device
        .supported_keys()
        .is_some_and(|keys| keys.contains(KeyCode::BTN_LEFT))
}

fn is_touchpad(device: &Device) -> bool {
    device.supported_absolute_axes().is_some_and(|axes| {
        axes.contains(AbsoluteAxisCode::ABS_MT_POSITION_X)
            && axes.contains(AbsoluteAxisCode::ABS_MT_POSITION_Y)
    }) && device
        .supported_keys()
        .is_some_and(|keys| keys.contains(KeyCode::BTN_TOUCH))
}

/// Require ordinary typing keys so power buttons, lid switches and media-only
/// HID interfaces are not mistaken for keyboards.
fn is_keyboard(device: &Device) -> bool {
    !is_relative_mouse(device)
        && device.supported_keys().is_some_and(|keys| {
            [
                KeyCode::KEY_A,
                KeyCode::KEY_Z,
                KeyCode::KEY_ENTER,
                KeyCode::KEY_SPACE,
            ]
            .iter()
            .all(|key| keys.contains(*key))
        })
}

fn kind_label(device: &Device) -> Option<&'static str> {
    if is_relative_mouse(device) {
        Some("mouse")
    } else if is_touchpad(device) {
        Some("touchpad")
    } else if is_keyboard(device) {
        Some("keyboard")
    } else {
        None
    }
}

/// Print the devices that can be configured, with stable path aliases.
pub fn list_devices() -> Result<(), Box<dyn Error>> {
    let mut devices = evdev::enumerate()
        .filter_map(|(path, device)| kind_label(&device).map(|kind| (path, device, kind)))
        .collect::<Vec<_>>();
    devices.sort_by(|left, right| left.0.cmp(&right.0));
    if devices.is_empty() {
        return Err(
            "no readable input devices found; check access to /dev/input/event* \
                    (membership of the `input` group needs a new login or `newgrp input`)"
                .into(),
        );
    }

    println!("Supported evdev devices (use kind = \"mouse\", \"touchpad\" or \"keyboard\"):");
    for (path, device, kind) in devices {
        println!("\n{} ({kind})", device.name().unwrap_or("<unnamed>"));
        println!("  current: {}", path.display());
        match stable_device_path(&path) {
            Some(stable) => println!("  stable:  {}", stable.display()),
            None => println!("  stable:  no /dev/input/by-id or by-path alias found"),
        }
    }
    println!("\nAdd only the devices you want captured to the configuration file.");
    Ok(())
}

fn stable_device_path(event_path: &Path) -> Option<PathBuf> {
    let target = fs::canonicalize(event_path).ok()?;
    for directory in ["/dev/input/by-id", "/dev/input/by-path"] {
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        let mut aliases = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                let name = path.to_string_lossy();
                name.ends_with("-event-mouse") || name.ends_with("-event-kbd")
            })
            .filter(|path| fs::canonicalize(path).ok().as_ref() == Some(&target))
            .collect::<Vec<_>>();
        aliases.sort();
        if let Some(alias) = aliases.into_iter().next() {
            return Some(alias);
        }
    }
    None
}

/// Stop workers and wait for them, so every grab is released before returning.
pub fn stop(shared: &Shared, workers: Vec<JoinHandle<()>>) {
    shared.shutdown.store(true, Ordering::SeqCst);
    for worker in workers {
        let _ = worker.join();
    }
}
