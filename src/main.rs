mod adb;
mod bridge;
mod capture;
mod cli;
mod clipboard;
mod config;
mod control;
mod input;
mod keyboard;
mod keymap;
mod protocol;
mod session;
mod state;
mod supervisor;
mod tray;

use std::env;
use std::error::Error;
use std::sync::atomic::AtomicBool;

use adb::Adb;
use cli::Command;
use input::Event;
use session::ActiveSession;

fn connect_selected_device<'a>(
    adb: &'a Adb,
    serial: &str,
) -> Result<ActiveSession<'a>, Box<dyn Error>> {
    session::connect_by_serial(adb, serial, &AtomicBool::new(false))
}

fn validate_coordinates(active: &ActiveSession<'_>, x: u32, y: u32) -> Result<(), std::io::Error> {
    let width = u32::try_from(active.handshake.configuration.width).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "device announced an invalid display width",
        )
    })?;
    let height = u32::try_from(active.handshake.configuration.height).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "device announced an invalid display height",
        )
    })?;
    if x >= width || y >= height {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("coordinates {x},{y} are outside the announced display {width}x{height}"),
        ));
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("deskdock-server: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    match Command::parse(env::args().skip(1))? {
        Command::Help => {
            cli::print_usage();
            Ok(())
        }
        Command::Devices { adb } => {
            let adb = Adb::new(adb);
            for device in adb.devices()? {
                println!("{}\t{}", device.serial, device.selector());
            }
            Ok(())
        }
        Command::Handshake { adb, serial } => {
            let adb = Adb::new(adb);
            let active = connect_selected_device(&adb, &serial)?;
            session::print_connection(&active);
            Ok(())
        }
        Command::ListInput => capture::list_devices(),
        Command::Run {
            adb,
            serial,
            config,
            no_tray,
            foreground,
        } => {
            if foreground {
                supervisor::run_foreground(adb, serial, config, no_tray)
            } else {
                supervisor::run_background(adb, serial, config, no_tray)
            }
        }
        Command::RunForeground {
            adb,
            serial,
            config,
            no_tray,
        } => supervisor::run_foreground(adb, serial, config, no_tray),
        Command::Control(request) => {
            println!("{}", control::send(request)?);
            Ok(())
        }
        Command::Capture {
            adb,
            serial,
            config: config_path,
        } => {
            let path = match config_path {
                Some(path) => path,
                None => config::default_path()?,
            };
            let config = config::Config::load(&path)?;
            let shared = state::Shared::default();
            // Ctrl+C and SIGTERM request a clean stop so grabs are released and
            // Android is told to disable input before the process exits.
            let shutdown = shared.shutdown.clone();
            ctrlc::set_handler(move || {
                shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
            })?;
            let adb = Adb::new(adb);
            bridge::run(&adb, &serial, config, shared)
        }
        Command::SendMove { adb, serial, x, y } => {
            let adb = Adb::new(adb);
            let mut active = connect_selected_device(&adb, &serial)?;
            validate_coordinates(&active, x, y)?;
            active.send_input(&Event::PointerMove { x, y })?;
            println!(
                "sent main-session pointer move to {x},{y} for device {}",
                active.device.serial
            );
            Ok(())
        }
    }
}
