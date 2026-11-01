//! A small wrapper around the ADB commands used by the headless server.

use std::error::Error;
use std::fmt;
use std::io;
use std::process::{Command, Output, Stdio};

use crate::session::DEVICE_PORT;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub serial: String,
    usb_transport: Option<String>,
}

impl Device {
    #[cfg(test)]
    pub fn for_test(serial: &str) -> Self {
        Self {
            serial: serial.to_owned(),
            usb_transport: None,
        }
    }

    /// Return the selector format preferred by the original Java server.
    pub fn selector(&self) -> String {
        self.usb_transport
            .as_ref()
            .map(|transport| format!("usb:{transport}"))
            .unwrap_or_else(|| self.serial.clone())
    }
}

#[derive(Clone)]
pub struct Adb {
    program: String,
}

impl Adb {
    pub fn new(program: String) -> Self {
        Self { program }
    }

    pub fn devices(&self) -> Result<Vec<Device>, AdbError> {
        let output = self.run(&["devices", "-l"])?;
        Ok(parse_devices(&String::from_utf8_lossy(&output.stdout)))
    }

    /// Create a forward using ADB's `tcp:0` allocation mode.
    ///
    /// Reserving a port ourselves and then releasing it before invoking ADB
    /// creates a race: another process can bind it first. Letting ADB allocate
    /// the listener is atomic, and ADB prints the selected port on stdout.
    pub fn forward_any_port(&self, device: &Device) -> Result<u16, AdbError> {
        self.forward_any_port_to(device, DEVICE_PORT)
    }

    /// Atomically allocate a host port and forward it to any localhost TCP
    /// listener on the device.
    pub fn forward_any_port_to(&self, device: &Device, device_port: u16) -> Result<u16, AdbError> {
        let output = self.run(&[
            "-s",
            &device.selector(),
            "forward",
            "tcp:0",
            &format!("tcp:{device_port}"),
        ])?;
        parse_forward_port(&output.stdout).ok_or_else(|| AdbError::InvalidForwardPort {
            output: String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        })
    }

    pub fn remove_forward(&self, device: &Device, host_port: u16) {
        // Cleanup is best effort: the device may already have been unplugged.
        let _ = self.run(&[
            "-s",
            &device.selector(),
            "forward",
            "--remove",
            &format!("tcp:{host_port}"),
        ]);
    }

    /// Launch the privileged Android companion process requested by family-1
    /// command 1. The original Java server starts it through a live ADB shell
    /// process; its stdin must remain open while the companion accepts the
    /// desktop-side connection.
    pub fn start_android_service(
        &self,
        device: &Device,
        apk_path: &str,
    ) -> Result<AndroidService, AdbError> {
        let classpath = shell_quote(apk_path);
        let command = format!(
            "CLASSPATH={classpath} app_process /system/bin com.floriandraschbacher.deskdock.systemservice.Service"
        );
        let child = Command::new(&self.program)
            .args(["-s", &device.selector(), "shell", &command])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|source| AdbError::Launch {
                program: self.program.clone(),
                source,
            })?;
        Ok(AndroidService { child })
    }

    fn run(&self, arguments: &[&str]) -> Result<Output, AdbError> {
        let output = Command::new(&self.program)
            .args(arguments)
            .output()
            .map_err(|source| AdbError::Launch {
                program: self.program.clone(),
                source,
            })?;

        if output.status.success() {
            Ok(output)
        } else {
            Err(AdbError::Failed {
                program: self.program.clone(),
                arguments: arguments
                    .iter()
                    .map(|argument| (*argument).to_owned())
                    .collect(),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            })
        }
    }
}

/// Owns the ADB shell process running Android's privileged DeskDock service.
/// Dropping it closes the shell and lets Android clean up its companion.
pub struct AndroidService {
    child: std::process::Child,
}

impl AndroidService {
    /// True once the `adb shell` hosting the companion has exited. DeskDock's
    /// Android app kills the companion when the user disconnects but leaves
    /// the TCP socket open, so this is the only reliable disconnect signal.
    pub fn has_exited(&mut self) -> bool {
        !matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for AndroidService {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Debug)]
pub enum AdbError {
    Launch {
        program: String,
        source: io::Error,
    },
    Failed {
        program: String,
        arguments: Vec<String>,
        stderr: String,
    },
    InvalidForwardPort {
        output: String,
    },
}

impl fmt::Display for AdbError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Launch { program, source } => {
                write!(formatter, "could not run {program}: {source}")
            }
            Self::Failed {
                program,
                arguments,
                stderr,
            } => write!(
                formatter,
                "{program} {} failed: {stderr}",
                arguments.join(" ")
            ),
            Self::InvalidForwardPort { output } => write!(
                formatter,
                "ADB returned an invalid automatically allocated forward port: {output:?}"
            ),
        }
    }
}

impl Error for AdbError {}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\\"'\\\"'"))
}

fn parse_forward_port(output: &[u8]) -> Option<u16> {
    String::from_utf8_lossy(output).trim().parse().ok()
}

fn parse_devices(output: &str) -> Vec<Device> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let serial = fields.next()?;
            let state = fields.next()?;
            if serial == "List" || state != "device" {
                return None;
            }

            let usb_transport =
                fields.find_map(|field| field.strip_prefix("usb:").map(ToOwned::to_owned));
            Some(Device {
                serial: serial.to_owned(),
                usb_transport,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_automatically_allocated_forward_port() {
        assert_eq!(parse_forward_port(b"45749\n"), Some(45_749));
        assert_eq!(parse_forward_port(b"not-a-port\n"), None);
    }

    #[test]
    fn parses_online_adb_devices_and_prefers_usb_transport() {
        let devices = parse_devices(
            "List of devices attached\n\
             emulator-5554 device product:sdk model:sdk_gphone64_x86_64 device:emu64xa transport_id:1\n\
             R58N123ABC device usb:1-4 product:foo model:bar device:baz transport_id:2\n\
             offline-device offline usb:1-5\n",
        );
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].selector(), "emulator-5554");
        assert_eq!(devices[1].selector(), "usb:1-4");
    }
}
