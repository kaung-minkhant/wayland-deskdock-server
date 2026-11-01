# deskdock-server

A headless Rust replacement for the DeskDock desktop server. It speaks the native
DeskDock protocol to the Android app over ADB and forwards a Linux host's mouse,
touchpad and keyboard to the tablet. It reads raw evdev devices, so it works on
Wayland and X11 alike.

## Setup

1. Install `adb`, enable USB debugging on the tablet, and start the DeskDock app.
2. Your user needs read access to `/dev/input/event*`, normally via the `input`
   group. Log in again after joining it. Check with `id`.
3. Find your devices and write a config:

   ```sh
   cargo run -- list-input
   mkdir -p ~/.config/deskdock-server
   cp config.example.toml ~/.config/deskdock-server/config.toml
   $EDITOR ~/.config/deskdock-server/config.toml
   ```

   Use the stable `/dev/input/by-id/...` or `by-path/...` names, not `eventN`.
   Only devices listed in the config are ever opened or grabbed.

## Running

```sh
# Starts in the background and adds a StatusNotifier icon to Waybar's tray.
cargo run --release -- run --serial <ADB SERIAL>    # `devices` lists serials
```

The Waybar tray menu reports the connection/mode and can switch input to the
Tablet or Linux, restart capture, or quit cleanly. Use `run --foreground` for
terminal diagnostics and `run --no-tray` when no StatusNotifier host is
available.

Open the DeskDock app on the tablet and tap **CONNECT**. `capture` starts in
**Linux mode**: your devices stay with the desktop. Then, from another terminal
or a compositor keybind:

| Command | Effect |
|---|---|
| `deskdock-server toggle` | Switch between Linux and tablet mode |
| `deskdock-server tablet` / `linux` | Switch to a specific mode |
| `deskdock-server status` | Print the current mode |

In **tablet mode** the configured devices are grabbed exclusively and sent to the
tablet. **Super+Shift+T** on the grabbed keyboard always returns to Linux mode.
Ctrl+C or SIGTERM releases everything cleanly.

### Connection handling

* If the tablet disconnects (cable pulled, app closed), the devices are released
  at once and `capture` retries with a 2 s to 15 s backoff.
* Tablet mode is refused while there is no connection.
* Screen rotation is followed: the pointer is rescaled to the new size.

## Gestures

| Input | Result |
|---|---|
| Mouse move, buttons, wheel | Pointer, clicks, scroll (right button is Android Back) |
| Touchpad one finger | Move |
| Touchpad tap / tap and drag | Click / drag |
| Touchpad two-finger scroll | Scroll |
| Touchpad two-finger swipe back | Android Back |

The touchpad's physical button is not forwarded. If your laptop's firmware
doesn't report it as an evdev event, nothing can detect it.

## Commands

```text
devices | handshake | send-move                           diagnostics
list-input                                               list host input devices
run --serial S [--config P] [--foreground] [--no-tray] start the supervised tray service
capture --serial S [--config P]                          forward input to the tablet
toggle | tablet | linux | status                         control a running capture
```

`--adb PATH` (or the `ADB` environment variable) selects the adb binary.

## Development

```sh
cargo fmt && cargo build && cargo test
nix build .#deskdock-server
```

The tests include a fake Android peer and a fake `adb` script, so they need no
device. Reverse-engineering evidence and protocol analysis remain in the separate
research repository.

## CI and Cachix

Pushes to `main` build `.#deskdock-server` on GitHub Actions and upload the
result to the [`wayland-deskdock-server` Cachix cache](https://wayland-deskdock-server.cachix.org).
Pull requests build and may substitute from the cache, but never upload.

Repository setup is required once:

1. Create a Cachix binary cache named `wayland-deskdock-server` (managed
   signing is sufficient).
2. In that cache's **Settings**, generate a per-cache token with write access.
3. In GitHub, open this repository's **Settings → Secrets and variables →
   Actions**. Add the token as the repository secret `CACHIX_AUTH_TOKEN`.
4. Push the workflow to `main`.

Do not store the token in this repository. Anyone able to modify workflows on
`main` can access its repository secrets, so restrict write access accordingly.
