//! Plain-text clipboard access through the Wayland `wl-clipboard` utilities.
//!
//! Keeping this behind a small boundary lets the session/protocol code remain
//! independent of a display server. `wl-paste` and `wl-copy` use the active
//! Wayland session inherited by the daemon.

use std::io::Write;
use std::process::{Command, Stdio};

pub struct Clipboard {
    current: Option<String>,
    received_from_android: Option<String>,
}

impl Clipboard {
    pub fn new() -> Self {
        Self {
            current: None,
            received_from_android: None,
        }
    }

    /// Return a changed local plain-text clipboard value. The first successful
    /// read establishes a baseline rather than overwriting Android on connect.
    pub fn poll_local_change(&mut self) -> Result<Option<String>, String> {
        let output = Command::new("wl-paste")
            // Do not turn a no-final-newline clipboard value into a different
            // value when it is mirrored to Android.
            .arg("--no-newline")
            .output()
            .map_err(|error| format!("could not run wl-paste: {error}"))?;
        if !output.status.success() {
            // wl-clipboard deliberately exits 1 when there is no clipboard
            // owner. That is a normal idle state, not an integration failure.
            if output.status.code() == Some(1)
                && String::from_utf8_lossy(&output.stderr).trim() == "Nothing is copied"
            {
                self.current = None;
                return Ok(None);
            }
            let error = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "wl-paste failed with status {}: {}",
                output.status,
                error.trim()
            ));
        }
        let text = String::from_utf8(output.stdout)
            .map_err(|_| "wl-paste returned non-UTF-8 clipboard data".to_owned())?;

        let Some(previous) = self.current.replace(text.clone()) else {
            return Ok(None);
        };
        if previous == text {
            return Ok(None);
        }
        if self.received_from_android.as_ref() == Some(&text) {
            self.received_from_android = None;
            return Ok(None);
        }
        Ok(Some(text))
    }

    pub fn set_from_android(&mut self, text: String) -> Result<(), String> {
        let mut child = Command::new("wl-copy")
            .args(["--type", "text/plain;charset=utf-8"])
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("could not run wl-copy: {error}"))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "could not open wl-copy input".to_owned())?;
        stdin
            .write_all(text.as_bytes())
            .map_err(|error| format!("could not write to wl-copy: {error}"))?;
        drop(stdin);
        let output = child
            .wait_with_output()
            .map_err(|error| format!("could not wait for wl-copy: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "wl-copy failed with status {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        self.current = Some(text.clone());
        self.received_from_android = Some(text);
        Ok(())
    }
}
