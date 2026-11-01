//! Lifecycle of one ADB-forwarded DeskDock TCP session.

use std::error::Error;
use std::io;
use std::net::TcpStream;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::adb::{Adb, AndroidService, Device};
use crate::input::{self, Event};
use crate::protocol::{
    Configuration, Handshake, decode_configuration, expect_hello, perform_handshake_after_hello,
};

pub const DEVICE_PORT: u16 = 19_672;
pub const PROTOCOL_VERSION: u32 = 18;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const HELLO_TIMEOUT: Duration = Duration::from_millis(500);
const CONNECT_RETRY_WINDOW: Duration = Duration::from_secs(30);
const CONNECT_RETRY_INTERVAL: Duration = Duration::from_millis(200);

/// Owns an ADB forward for its lifetime. Dropping it removes the forward.
pub struct ActiveSession<'a> {
    adb: &'a Adb,
    pub device: Device,
    host_port: u16,
    stream: TcpStream,
    pub handshake: Handshake,
    reader_stop: Arc<AtomicBool>,
    reader: Option<JoinHandle<()>>,
    android_service: Arc<Mutex<Option<AndroidService>>>,
    /// Latest display configuration; Android re-announces it on rotation.
    configuration: Arc<Mutex<Configuration>>,
    /// Set by the reader thread when the connection ended without our asking.
    lost: Arc<AtomicBool>,
}

impl<'a> ActiveSession<'a> {
    /// Forward a port to the device and handshake, giving up early once
    /// `cancel` is set.
    pub fn connect_cancellable(
        adb: &'a Adb,
        device: &Device,
        cancel: &AtomicBool,
    ) -> Result<Self, Box<dyn Error>> {
        let host_port = adb.forward_any_port(device)?;

        match connect_and_handshake(host_port, cancel) {
            Ok((stream, handshake)) => {
                let reader_stream = stream.try_clone()?;
                let response_stream = Arc::new(Mutex::new(stream.try_clone()?));
                let reader_stop = Arc::new(AtomicBool::new(false));
                let android_service = Arc::new(Mutex::new(None));
                let configuration = Arc::new(Mutex::new(handshake.configuration.clone()));
                let lost = Arc::new(AtomicBool::new(false));
                let reader = Some(spawn_packet_reader(
                    reader_stream,
                    Arc::clone(&response_stream),
                    adb.clone(),
                    device.clone(),
                    Arc::clone(&android_service),
                    Arc::clone(&configuration),
                    Arc::clone(&lost),
                    Arc::clone(&reader_stop),
                ));

                Ok(Self {
                    adb,
                    device: device.clone(),
                    host_port,
                    stream,
                    handshake,
                    reader_stop,
                    reader,
                    android_service,
                    configuration,
                    lost,
                })
            }
            Err(error) => {
                adb.remove_forward(device, host_port);
                Err(Box::new(error))
            }
        }
    }

    /// True once the Android side closed the connection or it failed, or the
    /// companion it asked us to start has stopped (its disconnect signal).
    pub fn is_lost(&self) -> bool {
        if self.lost.load(Ordering::SeqCst) {
            return true;
        }
        let mut service = self
            .android_service
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        service.as_mut().is_some_and(AndroidService::has_exited)
    }

    /// Current display size, following any rotation Android announced.
    pub fn display_size(&self) -> (i32, i32) {
        let configuration = self
            .configuration
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (configuration.width, configuration.height)
    }

    /// Send one typed family-2 input event over the established session.
    pub fn send_input(&mut self, event: &Event) -> Result<(), crate::protocol::ProtocolError> {
        input::send(&mut self.stream, event)
    }
}

impl Drop for ActiveSession<'_> {
    fn drop(&mut self) {
        self.reader_stop.store(true, Ordering::Relaxed);
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        if let Ok(mut android_service) = self.android_service.lock() {
            // Drop the live ADB shell after the main socket has closed.
            android_service.take();
        }
        self.adb.remove_forward(&self.device, self.host_port);
    }
}

/// Find `serial` among the online ADB devices and open a session to it.
pub fn connect_by_serial<'a>(
    adb: &'a Adb,
    serial: &str,
    cancel: &AtomicBool,
) -> Result<ActiveSession<'a>, Box<dyn Error>> {
    let device = adb
        .devices()?
        .into_iter()
        .find(|device| device.serial == serial || device.selector() == serial)
        .ok_or_else(|| format!("ADB device not found: {serial}"))?;
    ActiveSession::connect_cancellable(adb, &device, cancel)
}

pub fn print_connection(active: &ActiveSession<'_>) {
    println!(
        "connected: serial={}, package={}, device_id={}, display={}x{}, value3={}, value4={}",
        active.device.serial,
        active.handshake.package_name,
        active.handshake.device_id,
        active.handshake.configuration.width,
        active.handshake.configuration.height,
        active.handshake.configuration.value3,
        active.handshake.configuration.value4,
    );
}

/// Mirrors the original server's `cz` thread: after the handshake it must
/// continuously consume Android-originated frames. The packet formats beyond
/// their family tag are still being reconstructed, so preserve them in logs.
fn spawn_packet_reader(
    mut stream: TcpStream,
    response_stream: Arc<Mutex<TcpStream>>,
    adb: Adb,
    device: Device,
    android_service: Arc<Mutex<Option<AndroidService>>>,
    configuration: Arc<Mutex<Configuration>>,
    lost: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
) -> JoinHandle<()> {
    thread::spawn(move || {
        read_packets(
            &mut stream,
            &response_stream,
            &adb,
            &device,
            &android_service,
            &configuration,
            &stop,
        );
        if !stop.load(Ordering::Relaxed) {
            lost.store(true, Ordering::SeqCst);
        }
    })
}

fn read_packets(
    stream: &mut TcpStream,
    response_stream: &Arc<Mutex<TcpStream>>,
    adb: &Adb,
    device: &Device,
    android_service: &Arc<Mutex<Option<AndroidService>>>,
    configuration: &Mutex<Configuration>,
    stop: &AtomicBool,
) {
    if let Err(error) = stream.set_read_timeout(Some(Duration::from_millis(100))) {
        eprintln!("deskdock-server: unable to configure packet reader: {error}");
        return;
    }

    while !stop.load(Ordering::Relaxed) {
        match crate::protocol::read_frame(stream) {
            Ok(payload) => {
                let family = payload
                    .get(..4)
                    .and_then(|bytes| bytes.try_into().ok())
                    .map(i32::from_be_bytes);

                // Android sends a fresh family-3 packet whenever its display
                // configuration changes (rotation, resolution).
                if family == Some(3) {
                    apply_configuration_update(&payload, configuration);
                    continue;
                }

                eprintln!(
                    "received Android frame: family={}, bytes={}, payload={}",
                    family
                        .map(|family| family.to_string())
                        .unwrap_or_else(|| "<missing>".to_owned()),
                    payload.len(),
                    payload_preview(&payload)
                );

                if let Err(error) = start_and_acknowledge_android_service_request(
                    &payload,
                    response_stream,
                    adb,
                    device,
                    android_service,
                ) {
                    eprintln!("could not handle Android service request: {error}");
                }
            }
            Err(crate::protocol::ProtocolError::Io(error))
                if matches!(
                    error.kind(),
                    io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
                ) => {}
            Err(error) => {
                if !stop.load(Ordering::Relaxed) {
                    eprintln!("Android packet reader stopped: {error}");
                }
                break;
            }
        }
    }
}

fn apply_configuration_update(payload: &[u8], configuration: &Mutex<Configuration>) {
    match decode_configuration(payload) {
        Ok(update) if update.width > 0 && update.height > 0 => {
            eprintln!(
                "Android display is now {}x{} (value3={}, value4={})",
                update.width, update.height, update.value3, update.value4
            );
            *configuration
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = update;
        }
        Ok(update) => eprintln!(
            "ignoring Android display update with invalid size {}x{}",
            update.width, update.height
        ),
        Err(error) => eprintln!("ignoring malformed Android display update: {error}"),
    }
}

fn start_and_acknowledge_android_service_request(
    payload: &[u8],
    response_stream: &Mutex<TcpStream>,
    adb: &Adb,
    device: &Device,
    android_service: &Arc<Mutex<Option<AndroidService>>>,
) -> Result<(), Box<dyn Error>> {
    if payload.get(..8) != Some(&[0, 0, 0, 1, 0, 0, 0, 1]) {
        return Ok(());
    }

    let apk_path = decode_service_apk_path(payload)?;
    eprintln!("starting requested Android service from {apk_path}");
    let process = adb.start_android_service(device, apk_path)?;

    // This acknowledgement is the transition Android uses to mark the main
    // DeskDock session connected. Do not wait for the companion socket here:
    // it has its own startup window and blocking this response deadlocks the
    // Android state machine at "Connecting".
    let mut stream = response_stream
        .lock()
        .map_err(|_| io::Error::other("service response stream lock poisoned"))?;
    input::send_service_started(&mut *stream)?;
    drop(stream);
    eprintln!("started and acknowledged Android service request");

    // Keep the ADB shell alive for the companion's full lifetime. The
    // companion's private 39344 protocol is deliberately not probed here: a
    // probe can race its listener and causes spurious bind failures on MIUI.
    let mut service = android_service
        .lock()
        .map_err(|_| io::Error::other("Android service lock poisoned"))?;
    *service = Some(process);
    Ok(())
}

fn decode_service_apk_path(payload: &[u8]) -> Result<&str, crate::protocol::ProtocolError> {
    if payload.len() < 12 {
        return Err(crate::protocol::ProtocolError::InvalidControlPacketLength(
            payload.len(),
        ));
    }
    let path_length = u32::from_be_bytes(payload[8..12].try_into().expect("four bytes")) as usize;
    let path =
        payload
            .get(12..)
            .ok_or(crate::protocol::ProtocolError::InvalidControlPacketLength(
                payload.len(),
            ))?;
    if path.len() != path_length {
        return Err(crate::protocol::ProtocolError::InvalidControlPacketLength(
            payload.len(),
        ));
    }
    std::str::from_utf8(path).map_err(|_| crate::protocol::ProtocolError::InvalidUtf8("APK path"))
}

fn payload_preview(payload: &[u8]) -> String {
    const PREVIEW_BYTES: usize = 32;
    let mut preview = payload
        .iter()
        .take(PREVIEW_BYTES)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if payload.len() > PREVIEW_BYTES {
        preview.push_str("…");
    }
    preview
}

fn connect_and_handshake(
    host_port: u16,
    cancel: &AtomicBool,
) -> Result<(TcpStream, Handshake), crate::protocol::ProtocolError> {
    let mut stream = connect_after_hello(host_port, cancel)?;
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    let handshake = perform_handshake_after_hello(&mut stream, PROTOCOL_VERSION)?;
    stream.set_read_timeout(None)?;
    Ok((stream, handshake))
}

/// The Android listener sends `HELLO` immediately after accepting a socket. A
/// failed early probe makes it discard that socket, so retry with a new TCP
/// connection just as the original Java server does.
fn connect_after_hello(
    host_port: u16,
    cancel: &AtomicBool,
) -> Result<TcpStream, crate::protocol::ProtocolError> {
    let deadline = Instant::now() + CONNECT_RETRY_WINDOW;
    let mut last_error = None;

    while Instant::now() < deadline && !cancel.load(Ordering::SeqCst) {
        match TcpStream::connect(("127.0.0.1", host_port)) {
            Ok(mut stream) => {
                stream.set_nodelay(true)?;
                stream.set_read_timeout(Some(HELLO_TIMEOUT))?;
                match expect_hello(&mut stream) {
                    Ok(()) => return Ok(stream),
                    Err(error) => last_error = Some(error),
                }
            }
            Err(error) => last_error = Some(error.into()),
        }
        thread::sleep(CONNECT_RETRY_INTERVAL);
    }

    Err(last_error.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            "timed out waiting for Android HELLO",
        )
        .into()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{read_frame, write_frame};
    use std::fs;
    use std::net::TcpListener;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    const APK_PATH: &str = "/data/app/~~x/com.example-1/base.apk";

    struct ScratchDir(PathBuf);

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A shell script standing in for `adb`: `forward tcp:0` prints the fake
    /// peer's port, `shell` records its arguments and idles like a live shell.
    fn write_fake_adb(dir: &ScratchDir, port: u16) -> String {
        let log = dir.0.join("shell.log");
        let script = format!(
            "#!/bin/sh\n\
             case \"$*\" in\n\
             *\"forward tcp:0\"*) echo {port};;\n\
             *\"forward --remove\"*) echo removed >> '{removed}';;\n\
             *\" shell \"*) echo \"$*\" >> '{log}'; [ -e '{exits}' ] && exit 0; exec cat;;\n\
             esac\n",
            removed = dir.0.join("removed.log").display(),
            exits = dir.0.join("shell-exits").display(),
            log = log.display(),
        );
        let path = dir.0.join("adb");
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path.to_str().unwrap().to_owned()
    }

    fn service_request() -> Vec<u8> {
        let mut payload = vec![0, 0, 0, 1, 0, 0, 0, 1];
        payload.extend((APK_PATH.len() as u32).to_be_bytes());
        payload.extend(APK_PATH.as_bytes());
        payload
    }

    #[test]
    fn handshakes_acknowledges_the_service_request_and_sends_input() {
        let dir = ScratchDir(
            std::env::temp_dir().join(format!("deskdock-session-test-{}", std::process::id())),
        );
        fs::create_dir_all(&dir.0).unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let adb = Adb::new(write_fake_adb(&dir, listener.local_addr().unwrap().port()));

        // The fake Android side: HELLO, the client handshake, then the
        // family-1 command-1 service request.
        let peer = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            write_frame(&mut stream, b"HELLO").unwrap();
            assert_eq!(read_frame(&mut stream).unwrap(), b"Server 18");
            write_frame(&mut stream, b"Client 18").unwrap();
            write_frame(&mut stream, b"com.example.deskdock").unwrap();
            write_frame(&mut stream, b"device-1").unwrap();
            let mut config = Vec::new();
            for value in [3_i32, 2880, 1800, 308, 50] {
                config.extend(value.to_be_bytes());
            }
            write_frame(&mut stream, &config).unwrap();
            write_frame(&mut stream, &service_request()).unwrap();

            let ack = read_frame(&mut stream).unwrap();
            let input = read_frame(&mut stream).unwrap();
            // The server closes the socket when the session is dropped.
            let closed = read_frame(&mut stream).is_err();
            (ack, input, closed)
        });

        let device = Device::for_test("test-serial");
        let mut session =
            ActiveSession::connect_cancellable(&adb, &device, &AtomicBool::new(false)).unwrap();
        assert_eq!(session.handshake.package_name, "com.example.deskdock");
        assert_eq!(session.handshake.device_id, "device-1");
        assert_eq!(session.handshake.configuration.width, 2880);
        assert_eq!(session.handshake.configuration.height, 1800);

        // Wait until the reader thread has launched the companion.
        let log = dir.0.join("shell.log");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !log.exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        let shell = fs::read_to_string(&log).expect("companion was not launched");
        assert!(shell.contains("-s test-serial shell CLASSPATH="), "{shell}");
        assert!(shell.contains(APK_PATH), "{shell}");
        assert!(shell.contains("systemservice.Service"), "{shell}");

        // The ack was queued before this write, so frame order is fixed.
        session
            .send_input(&Event::PointerMove { x: 10, y: 20 })
            .unwrap();
        drop(session);

        let (ack, input, closed) = peer.join().unwrap();
        assert_eq!(ack, [0, 0, 0, 1, 0, 0, 0, 2]);
        assert_eq!(input, Event::PointerMove { x: 10, y: 20 }.encode());
        assert!(closed);
        assert!(dir.0.join("removed.log").exists(), "forward not removed");
    }

    fn config_frame(width: i32, height: i32) -> Vec<u8> {
        let mut config = Vec::new();
        for value in [3_i32, width, height, 308, 50] {
            config.extend(value.to_be_bytes());
        }
        config
    }

    #[test]
    fn follows_rotation_updates_and_notices_a_lost_connection() {
        let dir = ScratchDir(
            std::env::temp_dir().join(format!("deskdock-rotation-test-{}", std::process::id())),
        );
        fs::create_dir_all(&dir.0).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let adb = Adb::new(write_fake_adb(&dir, listener.local_addr().unwrap().port()));

        let (rotate, rotated) = std::sync::mpsc::channel::<()>();
        let peer = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            write_frame(&mut stream, b"HELLO").unwrap();
            read_frame(&mut stream).unwrap();
            write_frame(&mut stream, b"Client 18").unwrap();
            write_frame(&mut stream, b"pkg").unwrap();
            write_frame(&mut stream, b"id").unwrap();
            write_frame(&mut stream, &config_frame(2880, 1800)).unwrap();
            rotated.recv().unwrap();
            write_frame(&mut stream, &config_frame(1800, 2880)).unwrap();
            // An invalid update must be ignored.
            write_frame(&mut stream, &config_frame(0, 0)).unwrap();
            rotated.recv().unwrap();
            // Dropping the stream simulates the tablet going away.
        });

        let device = Device::for_test("serial");
        let session =
            ActiveSession::connect_cancellable(&adb, &device, &AtomicBool::new(false)).unwrap();
        assert_eq!(session.display_size(), (2880, 1800));
        assert!(!session.is_lost());

        let wait_until = |condition: &dyn Fn() -> bool| {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !condition() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            assert!(condition());
        };
        rotate.send(()).unwrap();
        wait_until(&|| session.display_size() == (1800, 2880));
        thread::sleep(Duration::from_millis(250));
        assert_eq!(session.display_size(), (1800, 2880));
        assert!(!session.is_lost());

        rotate.send(()).unwrap();
        peer.join().unwrap();
        wait_until(&|| session.is_lost());
    }

    #[test]
    fn a_stopped_companion_counts_as_a_lost_connection() {
        let dir = ScratchDir(
            std::env::temp_dir().join(format!("deskdock-companion-test-{}", std::process::id())),
        );
        fs::create_dir_all(&dir.0).unwrap();
        // The fake `adb shell` exits at once, as when the app kills the companion.
        fs::write(dir.0.join("shell-exits"), "").unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let adb = Adb::new(write_fake_adb(&dir, listener.local_addr().unwrap().port()));

        let (done, finished) = std::sync::mpsc::channel::<()>();
        let peer = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            write_frame(&mut stream, b"HELLO").unwrap();
            read_frame(&mut stream).unwrap();
            write_frame(&mut stream, b"Client 18").unwrap();
            write_frame(&mut stream, b"pkg").unwrap();
            write_frame(&mut stream, b"id").unwrap();
            write_frame(&mut stream, &config_frame(2880, 1800)).unwrap();
            write_frame(&mut stream, &service_request()).unwrap();
            read_frame(&mut stream).unwrap(); // the ack
            // Keep the socket open: Android does not close it on disconnect.
            finished.recv().unwrap();
        });

        let device = Device::for_test("serial");
        let session =
            ActiveSession::connect_cancellable(&adb, &device, &AtomicBool::new(false)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !session.is_lost() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(session.is_lost(), "the exited companion was not noticed");
        done.send(()).unwrap();
        drop(session);
        peer.join().unwrap();
    }

    #[test]
    fn a_cancelled_connection_attempt_returns_promptly() {
        let dir = ScratchDir(
            std::env::temp_dir().join(format!("deskdock-cancel-test-{}", std::process::id())),
        );
        fs::create_dir_all(&dir.0).unwrap();
        // Nothing ever answers on this port.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let adb = Adb::new(write_fake_adb(&dir, listener.local_addr().unwrap().port()));
        let device = Device::for_test("serial");
        let started = Instant::now();
        let cancel = AtomicBool::new(true);
        assert!(ActiveSession::connect_cancellable(&adb, &device, &cancel).is_err());
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn ignores_frames_that_are_not_service_requests() {
        let adb = Adb::new("/nonexistent/adb".to_owned());
        let device = Device::for_test("serial");
        let stream = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(stream.local_addr().unwrap()).unwrap();
        let response = Mutex::new(client);
        let service = Arc::new(Mutex::new(None));
        start_and_acknowledge_android_service_request(
            &[0, 0, 0, 2, 3],
            &response,
            &adb,
            &device,
            &service,
        )
        .unwrap();
        assert!(service.lock().unwrap().is_none());
    }
}
