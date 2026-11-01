//! State shared between the bridge loop, capture workers and control socket.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Default)]
pub struct Shared {
    /// True while configured devices are grabbed and forwarded to Android.
    pub tablet_mode: Arc<AtomicBool>,
    /// True while a session with the tablet is established. Tablet mode can
    /// only be entered while connected, otherwise grabbed input would vanish.
    pub connected: Arc<AtomicBool>,
    /// Set to ask every helper thread to stop.
    pub shutdown: Arc<AtomicBool>,
}

impl Shared {
    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }

    pub fn is_tablet(&self) -> bool {
        self.tablet_mode.load(Ordering::SeqCst)
    }

    pub fn set_tablet(&self, tablet: bool) {
        self.tablet_mode.store(tablet, Ordering::SeqCst);
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    pub fn set_connected(&self, connected: bool) {
        self.connected.store(connected, Ordering::SeqCst);
    }
}
