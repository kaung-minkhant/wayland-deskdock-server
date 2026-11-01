//! Unix control socket used to hand input between Linux and the tablet.
//!
//! A compositor keybind runs `deskdock-server toggle`, which talks to the
//! running `capture` process over this socket. The socket is private to the
//! user (mode 0600 inside `$XDG_RUNTIME_DIR`).

use std::error::Error;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::state::Shared;

const SOCKET_NAME: &str = "deskdock-server.sock";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Toggle,
    Tablet,
    Linux,
    Status,
}

impl Request {
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "toggle" => Some(Self::Toggle),
            "tablet" => Some(Self::Tablet),
            "linux" => Some(Self::Linux),
            "status" => Some(Self::Status),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Toggle => "toggle",
            Self::Tablet => "tablet",
            Self::Linux => "linux",
            Self::Status => "status",
        }
    }
}

pub fn socket_path() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(SOCKET_NAME)
}

/// Removes the socket file when dropped.
pub struct ControlSocket {
    path: PathBuf,
    thread: Option<JoinHandle<()>>,
}

impl ControlSocket {
    pub fn bind(shared: Shared) -> Result<Self, Box<dyn Error>> {
        Self::bind_at(socket_path(), shared)
    }

    fn bind_at(path: PathBuf, shared: Shared) -> Result<Self, Box<dyn Error>> {
        if path.exists() {
            if UnixStream::connect(&path).is_ok() {
                return Err(format!(
                    "another capture process is already running (control socket {})",
                    path.display()
                )
                .into());
            }
            fs::remove_file(&path)?;
        }
        let listener = UnixListener::bind(&path)
            .map_err(|error| format!("could not bind {}: {error}", path.display()))?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;

        let thread = thread::spawn(move || {
            while !shared.is_shutdown() {
                match listener.accept() {
                    Ok((stream, _)) => handle(stream, &shared),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(25));
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            path,
            thread: Some(thread),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ControlSocket {
    fn drop(&mut self) {
        // The accept loop exits once shutdown is set by the owner.
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = fs::remove_file(&self.path);
    }
}

fn handle(mut stream: UnixStream, shared: &Shared) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let mut buffer = [0_u8; 64];
    let Ok(read) = stream.read(&mut buffer) else {
        return;
    };
    let reply = match Request::parse(&String::from_utf8_lossy(&buffer[..read])) {
        Some(request) => apply(request, shared),
        None => "error: expected toggle, tablet, linux or status".to_owned(),
    };
    let _ = writeln!(stream, "{reply}");
}

fn apply(request: Request, shared: &Shared) -> String {
    let wants_tablet = match request {
        Request::Toggle => Some(!shared.is_tablet()),
        Request::Tablet => Some(true),
        Request::Linux => Some(false),
        Request::Status => None,
    };
    if wants_tablet == Some(true) && !shared.is_connected() {
        return "error: the tablet is not connected; staying in linux mode".to_owned();
    }
    if let Some(tablet) = wants_tablet {
        shared.set_tablet(tablet);
    }
    let mode = if shared.is_tablet() {
        "tablet"
    } else {
        "linux"
    };
    if shared.is_connected() {
        mode.to_owned()
    } else {
        format!("{mode} (waiting for the tablet)")
    }
}

/// Client side: send one request to the running capture process.
pub fn send(request: Request) -> Result<String, Box<dyn Error>> {
    let path = socket_path();
    let mut stream = UnixStream::connect(&path).map_err(|error| {
        format!(
            "no capture process is listening on {} ({error}); start `deskdock-server capture`",
            path.display()
        )
    })?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.write_all(request.name().as_bytes())?;
    let mut reply = String::new();
    stream.read_to_string(&mut reply)?;
    Ok(reply.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn parses_requests() {
        assert_eq!(Request::parse("toggle\n"), Some(Request::Toggle));
        assert_eq!(Request::parse(" status "), Some(Request::Status));
        assert_eq!(Request::parse("bogus"), None);
    }

    #[test]
    fn requests_switch_modes() {
        let shared = Shared::default();
        shared.set_connected(true);
        assert_eq!(apply(Request::Toggle, &shared), "tablet");
        assert_eq!(apply(Request::Status, &shared), "tablet");
        assert_eq!(apply(Request::Linux, &shared), "linux");
        assert_eq!(apply(Request::Tablet, &shared), "tablet");
    }

    #[test]
    fn tablet_mode_is_refused_while_disconnected() {
        let shared = Shared::default();
        assert!(apply(Request::Toggle, &shared).starts_with("error:"));
        assert!(apply(Request::Tablet, &shared).starts_with("error:"));
        assert!(!shared.is_tablet());
        assert_eq!(
            apply(Request::Linux, &shared),
            "linux (waiting for the tablet)"
        );
        assert_eq!(
            apply(Request::Status, &shared),
            "linux (waiting for the tablet)"
        );
    }

    #[test]
    fn socket_round_trip_and_cleanup() {
        let directory = std::env::temp_dir().join(format!("deskdock-ctl-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("test.sock");
        let shared = Shared::default();
        shared.set_connected(true);
        let socket = ControlSocket::bind_at(path.clone(), shared.clone()).unwrap();

        let mut stream = UnixStream::connect(&path).unwrap();
        stream.write_all(b"toggle").unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).unwrap();
        assert_eq!(reply.trim(), "tablet");
        assert!(shared.is_tablet());

        shared.shutdown.store(true, Ordering::SeqCst);
        drop(socket);
        assert!(!path.exists());
        fs::remove_dir_all(&directory).unwrap();
    }
}
