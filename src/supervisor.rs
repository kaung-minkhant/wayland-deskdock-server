//! Background launcher and supervisor for capture plus the tray icon.

use std::{
    error::Error,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use crate::tray;

pub fn run_background(
    adb: Option<String>,
    serial: String,
    config: Option<PathBuf>,
    no_tray: bool,
) -> Result<(), Box<dyn Error>> {
    let executable = std::env::current_exe()?;
    let mut command = Command::new(&executable);
    command.arg("run-foreground").arg("--serial").arg(serial);
    if let Some(adb) = adb {
        command.arg("--adb").arg(adb);
    }
    if let Some(config) = config {
        command.arg("--config").arg(config);
    }
    if no_tray {
        command.arg("--no-tray");
    }
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    println!(
        "Started Wayland DeskDock server (PID {}). Use the Waybar tray icon to control it.",
        child.id()
    );
    Ok(())
}

pub fn run_foreground(
    adb: Option<String>,
    serial: String,
    config: Option<PathBuf>,
    no_tray: bool,
) -> Result<(), Box<dyn Error>> {
    let stopped = Arc::new(AtomicBool::new(false));
    let signal_stop = Arc::clone(&stopped);
    ctrlc::set_handler(move || signal_stop.store(true, Ordering::SeqCst))?;
    let restart_requested = Arc::new(AtomicBool::new(false));
    let executable = std::env::current_exe()?.to_string_lossy().into_owned();
    let tray = (!no_tray).then(|| {
        tray::spawn(
            executable,
            Arc::clone(&stopped),
            Arc::clone(&restart_requested),
        )
    });
    let mut child = spawn_capture(adb.as_deref(), &serial, config.as_ref())?;

    while !stopped.load(Ordering::SeqCst) {
        if restart_requested.swap(false, Ordering::SeqCst) {
            eprintln!("Restarting DeskDock capture.");
            let _ = child.kill();
            let _ = child.wait();
            if !stopped.load(Ordering::SeqCst) {
                child = spawn_capture(adb.as_deref(), &serial, config.as_ref())?;
            }
        } else if child.try_wait()?.is_some() {
            eprintln!("DeskDock capture stopped; restarting in 2 seconds.");
            for _ in 0..20 {
                if stopped.load(Ordering::SeqCst) || restart_requested.load(Ordering::SeqCst) {
                    break;
                }
                thread::sleep(Duration::from_millis(100));
            }
            if !stopped.load(Ordering::SeqCst) {
                restart_requested.store(false, Ordering::SeqCst);
                child = spawn_capture(adb.as_deref(), &serial, config.as_ref())?;
            }
        }
        thread::sleep(Duration::from_millis(200));
    }
    let _ = child.kill();
    let _ = child.wait();
    if let Some(tray) = tray {
        let _ = tray.join();
    }
    Ok(())
}

fn spawn_capture(
    adb: Option<&str>,
    serial: &str,
    config: Option<&PathBuf>,
) -> Result<Child, Box<dyn Error>> {
    let executable = std::env::current_exe()?;
    let mut command = Command::new(executable);
    command.arg("capture").arg("--serial").arg(serial);
    if let Some(adb) = adb {
        command.arg("--adb").arg(adb);
    }
    if let Some(config) = config {
        command.arg("--config").arg(config);
    }
    command.spawn().map_err(Into::into)
}
