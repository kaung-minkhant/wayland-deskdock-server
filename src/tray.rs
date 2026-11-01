//! Waybar/StatusNotifier tray control for a running DeskDock capture process.

use ksni::{Category, Icon, MenuItem, Status, ToolTip, blocking::TrayMethods, menu::StandardItem};
use std::{
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

const SIZE: i32 = 32;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Waiting,
    Linux,
    Tablet,
}

pub struct TrayState {
    executable: String,
    stopped: Arc<AtomicBool>,
    restart_requested: Arc<AtomicBool>,
    mode: Mode,
}

impl Mode {
    fn from_status(status: &[u8]) -> Option<Self> {
        let status = String::from_utf8_lossy(status);
        if status.contains("waiting for the tablet") {
            Some(Self::Waiting)
        } else if status.starts_with("tablet") {
            Some(Self::Tablet)
        } else if status.starts_with("linux") {
            Some(Self::Linux)
        } else {
            None
        }
    }
}

impl TrayState {
    fn refresh(&mut self) {
        self.mode = Command::new(&self.executable)
            .arg("status")
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| Mode::from_status(&output.stdout))
            .unwrap_or(Mode::Waiting);
    }

    fn summary(&self) -> &'static str {
        match self.mode {
            Mode::Waiting => "Waiting for the Android tablet",
            Mode::Linux => "Linux mode: input stays on this computer",
            Mode::Tablet => "Tablet mode: input goes to Android",
        }
    }

    fn control(&self, action: &str) {
        let _ = Command::new(&self.executable).arg(action).output();
    }
}

impl ksni::Tray for TrayState {
    const MENU_ON_ACTIVATE: bool = true;

    fn id(&self) -> String {
        "wayland-deskdock-server".into()
    }
    fn category(&self) -> Category {
        Category::Hardware
    }
    fn title(&self) -> String {
        "Wayland DeskDock".into()
    }
    fn status(&self) -> Status {
        Status::Active
    }
    fn icon_pixmap(&self) -> Vec<Icon> {
        vec![icon(self.mode)]
    }
    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            title: "Wayland DeskDock".into(),
            description: self.summary().into(),
            ..Default::default()
        }
    }
    fn menu(&self) -> Vec<MenuItem<Self>> {
        let connected = self.mode != Mode::Waiting;
        let tablet = self.mode == Mode::Tablet;
        vec![
            StandardItem {
                label: self.summary().into(),
                enabled: false,
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: if tablet {
                    "Return input to Linux"
                } else {
                    "Send input to tablet"
                }
                .into(),
                enabled: connected,
                activate: Box::new(|this: &mut Self| this.control("toggle")),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Restart server".into(),
                activate: Box::new(|this: &mut Self| {
                    this.restart_requested.store(true, Ordering::SeqCst)
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|this: &mut Self| this.stopped.store(true, Ordering::SeqCst)),
                ..Default::default()
            }
            .into(),
        ]
    }
    fn watcher_offline(&self, _: ksni::OfflineReason) -> bool {
        true
    }
}

/// Register a Waybar-compatible StatusNotifierItem and keep its state current.
pub fn spawn(
    executable: String,
    stopped: Arc<AtomicBool>,
    restart_requested: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut state = TrayState {
            executable,
            stopped: Arc::clone(&stopped),
            restart_requested,
            mode: Mode::Waiting,
        };
        state.refresh();
        let handle = match state.assume_sni_available(true).spawn() {
            Ok(handle) => handle,
            Err(error) => {
                eprintln!("DeskDock tray icon unavailable: {error}");
                return;
            }
        };
        while !stopped.load(Ordering::SeqCst) && !handle.is_closed() {
            handle.update(|tray| tray.refresh());
            thread::sleep(Duration::from_millis(500));
        }
        let _ = handle.shutdown();
    })
}

fn icon(mode: Mode) -> Icon {
    let (red, green, blue) = match mode {
        Mode::Waiting => (0x9a, 0x9a, 0x9a),
        Mode::Linux => (0x4c, 0x9b, 0xf0),
        Mode::Tablet => (0x3f, 0xcf, 0x6e),
    };
    let mut data = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let body = x >= 6 && x < 26 && y >= 2 && y < 30;
            let screen = x >= 9 && x < 23 && y >= 6 && y < 25;
            let alpha = if body && !screen {
                0xff
            } else if screen {
                0x55
            } else {
                0
            };
            data.extend_from_slice(&[alpha, red, green, blue]);
        }
    }
    Icon {
        width: SIZE,
        height: SIZE,
        data,
    }
}
