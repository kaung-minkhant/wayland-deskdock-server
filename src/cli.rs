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
    /// Start capture in the background and register a Waybar tray icon.
    Run {
        adb: Option<String>,
        serial: String,
        config: Option<PathBuf>,
        no_tray: bool,
        foreground: bool,
    },
    /// Internal foreground daemon used by `run`.
    RunForeground {
        adb: Option<String>,
        serial: String,
        config: Option<PathBuf>,
        no_tray: bool,
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

        let mut adb = env::var("ADB").ok();
        let mut serial = None;
        let mut x = None;
        let mut y = None;
        let mut config = None;
        let mut no_tray = false;
        let mut foreground = false;
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--adb" => {
                    adb = Some(
                        arguments
                            .next()
                            .ok_or_else(|| "--adb requires a path".to_owned())?,
                    );
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
                "--no-tray" => no_tray = true,
                "--foreground" => foreground = true,
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
            "devices" => Ok(Self::Devices {
                adb: adb.unwrap_or_else(|| "adb".to_owned()),
            }),
            "handshake" => Ok(Self::Handshake {
                adb: adb.unwrap_or_else(|| "adb".to_owned()),
                serial: serial.ok_or_else(|| "handshake requires --serial SERIAL".to_owned())?,
            }),
            "send-move" => Ok(Self::SendMove {
                adb: adb.unwrap_or_else(|| "adb".to_owned()),
                serial: serial.ok_or_else(|| "send-move requires --serial SERIAL".to_owned())?,
                x: x.ok_or_else(|| "send-move requires --x X".to_owned())?,
                y: y.ok_or_else(|| "send-move requires --y Y".to_owned())?,
            }),
            "run" => Ok(Self::Run {
                adb,
                serial: serial.ok_or_else(|| "run requires --serial SERIAL".to_owned())?,
                config,
                no_tray,
                foreground,
            }),
            "run-foreground" => Ok(Self::RunForeground {
                adb,
                serial: serial
                    .ok_or_else(|| "run-foreground requires --serial SERIAL".to_owned())?,
                config,
                no_tray,
            }),
            "capture" => Ok(Self::Capture {
                adb: adb.unwrap_or_else(|| "adb".to_owned()),
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
  deskdock-server run --serial SERIAL [--config PATH] [--adb PATH] [--foreground] [--no-tray]\n\
  deskdock-server capture --serial SERIAL [--config PATH] [--adb PATH]\n\
  deskdock-server toggle | tablet | linux | status\n\n\
Each connection command creates a temporary, automatically allocated ADB forward\n\
to tcp:19672 and removes it when complete.\n\n\
`run` backgrounds the supervised server and registers a Waybar tray icon. Use\n\
`--foreground` for diagnostics and `--no-tray` when no tray host is available.\n\
`capture` forwards the devices in ~/.config/deskdock-server/config.toml. It starts in\n\
Linux mode; `toggle` (for example from a compositor keybind) grabs the devices\n\
and sends them to the tablet, and Super+Shift+T or `linux` returns them. It\n\
reconnects by itself when the tablet disconnects and follows screen rotation."
    );
}

#[cfg(test)]
mod tests {
    use super::Command;
    use std::path::PathBuf;

    fn parse(arguments: &[&str]) -> Command {
        Command::parse(arguments.iter().map(|argument| (*argument).to_owned())).unwrap()
    }

    #[test]
    fn parses_background_run() {
        assert_eq!(
            parse(&["run", "--serial", "tablet", "--config", "/tmp/config"]),
            Command::Run {
                adb: None,
                serial: "tablet".into(),
                config: Some(PathBuf::from("/tmp/config")),
                no_tray: false,
                foreground: false,
            }
        );
    }

    #[test]
    fn parses_foreground_run_without_tray() {
        assert_eq!(
            parse(&["run", "--serial", "tablet", "--foreground", "--no-tray"]),
            Command::Run {
                adb: None,
                serial: "tablet".into(),
                config: None,
                no_tray: true,
                foreground: true,
            }
        );
    }
}
