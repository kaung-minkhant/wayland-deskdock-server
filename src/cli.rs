//! Command-line parsing and help output.

use std::env;
use std::path::PathBuf;

use crate::control::Request;

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Help,
    Devices {
        adb: String,
    },
    Handshake {
        adb: String,
        serial: String,
    },
    SendMove {
        adb: String,
        serial: String,
        x: u32,
        y: u32,
    },
    /// Forward configured host input devices to the Android display.
    Capture {
        adb: String,
        serial: String,
        config: Option<PathBuf>,
    },
    /// List the host input devices that can be configured for capture.
    ListInput,
    /// Ask a running `capture` process to change mode.
    Control(Request),
}

impl Command {
    pub fn parse(arguments: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut arguments = arguments.into_iter();
        let Some(subcommand) = arguments.next() else {
            return Ok(Self::Help);
        };

        if matches!(subcommand.as_str(), "-h" | "--help" | "help") {
            return Ok(Self::Help);
        }

        match subcommand.as_str() {
            "list-input" => {
                return match arguments.next() {
                    None => Ok(Self::ListInput),
                    Some(argument) => Err(format!("unknown argument: {argument}")),
                };
            }
            "toggle" | "tablet" | "linux" | "status" => {
                return match arguments.next() {
                    None => Ok(Self::Control(
                        Request::parse(&subcommand).expect("subcommand is a request name"),
                    )),
                    Some(argument) => Err(format!("unknown argument: {argument}")),
                };
            }
            _ => {}
        }

        let mut adb = env::var("ADB").unwrap_or_else(|_| "adb".to_owned());
        let mut serial = None;
        let mut x = None;
        let mut y = None;
        let mut config = None;
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--adb" => {
                    adb = arguments
                        .next()
                        .ok_or_else(|| "--adb requires a path".to_owned())?;
                }
                "--serial" => {
                    serial = Some(
                        arguments
                            .next()
                            .ok_or_else(|| "--serial requires a device serial".to_owned())?,
                    );
                }
                "--config" => {
                    config = Some(PathBuf::from(
                        arguments
                            .next()
                            .ok_or_else(|| "--config requires a path".to_owned())?,
                    ));
                }
                "--x" => {
                    x = Some(parse_coordinate(
                        arguments
                            .next()
                            .ok_or_else(|| "--x requires a non-negative coordinate".to_owned())?,
                        "--x",
                    )?);
                }
                "--y" => {
                    y = Some(parse_coordinate(
                        arguments
                            .next()
                            .ok_or_else(|| "--y requires a non-negative coordinate".to_owned())?,
                        "--y",
                    )?);
                }
                _ => return Err(format!("unknown argument: {argument}")),
            }
        }

        match subcommand.as_str() {
            "devices" => Ok(Self::Devices { adb }),
            "handshake" => Ok(Self::Handshake {
                adb,
                serial: serial.ok_or_else(|| "handshake requires --serial SERIAL".to_owned())?,
            }),
            "send-move" => Ok(Self::SendMove {
                adb,
                serial: serial.ok_or_else(|| "send-move requires --serial SERIAL".to_owned())?,
                x: x.ok_or_else(|| "send-move requires --x X".to_owned())?,
                y: y.ok_or_else(|| "send-move requires --y Y".to_owned())?,
            }),
            "capture" => Ok(Self::Capture {
                adb,
                serial: serial.ok_or_else(|| "capture requires --serial SERIAL".to_owned())?,
                config,
            }),
            _ => Err(format!("unknown command: {subcommand}")),
        }
    }
}

fn parse_coordinate(value: String, option: &str) -> Result<u32, String> {
    value
        .parse()
        .map_err(|_| format!("{option} requires a non-negative 32-bit integer"))
}

pub fn print_usage() {
    println!(
        "DeskDock-compatible headless server\n\n\
Usage:\n\
  deskdock-server devices [--adb PATH]\n\
  deskdock-server handshake --serial SERIAL [--adb PATH]\n\
  deskdock-server send-move --serial SERIAL --x X --y Y [--adb PATH]\n\
  deskdock-server list-input\n\
  deskdock-server capture --serial SERIAL [--config PATH] [--adb PATH]\n\
  deskdock-server toggle | tablet | linux | status\n\n\
Each connection command creates a temporary, automatically allocated ADB forward\n\
to tcp:19672 and removes it when complete.\n\n\
`capture` forwards the devices in ~/.config/deskdock-server/config.toml. It starts\n\
in Linux mode; `toggle` (for example from a compositor keybind) grabs the devices\n\
and sends them to the tablet, and Super+Shift+T or `linux` returns them. It\n\
reconnects by itself when the tablet disconnects and follows screen rotation."
    );
}
