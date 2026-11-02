# Android-to-Linux Clipboard Synchronization Options

## Context

The current DeskDock-compatible server supports plain-text Wayland clipboard synchronization using `wl-copy` and `wl-paste`. Linux-to-Android transport works through the existing DeskDock family-2 clipboard protocol.

Android-to-Linux synchronization was tested on:

- Device: Xiaomi Pad 6 (`pipa`, model `23043RP34G`)
- OS: Android 14 / HyperOS 2.0.16.0
- DeskDock Free: 1.2.4
- Connection: USB ADB, active and confirmed connected
- DeskDock **Share clipboard** setting: enabled and verified through the app UI

Despite the connected session and enabled preference, copying text on Android emitted no clipboard frame on the DeskDock TCP connection. This was tested with DeskDock visible/foreground and with a split-screen workflow. The server reader remained connected and received the normal Android service-start request, but received no subsequent clipboard or unknown frame.

This indicates that the Android application is not able to observe or emit the clipboard update on this Android 14 / HyperOS device. It is not an issue with the Linux Wayland clipboard backend or the server-side packet decoder.

## Options

| Option | Reliability on Android 14 / HyperOS | User setup | Android implementation effort | Notes |
| --- | --- | --- | --- | --- |
| Keep current DeskDock clipboard listener | Poor | None | None | Tested and ineffective on this device: no clipboard event was emitted. |
| Accessibility-service companion | Good | User manually enables an accessibility service | Medium | Could observe relevant UI events, but clipboard behavior varies across Android/OEM versions. Requires clear privacy disclosure because it may access sensitive user data. |
| Default keyboard / IME companion | Best | User selects companion as the default keyboard while syncing | High | A default IME has a supported, privileged clipboard route. Most robust continuous-sync option, but needs a usable keyboard UI and careful typing workflow. |
| Share-sheet `Send to Desktop` action | Excellent, manual | User selects the DeskDock action in Android’s share sheet | Low to medium | Explicitly shares selected text to the existing connection. Avoids clipboard restrictions and works on HyperOS. Not continuous synchronization. |
| Notification or Quick Settings tile | Good, manual | User taps the tile/action | Medium | Suitable for an explicit send workflow, but cannot reliably read arbitrary clipboard contents without a privileged route. |
| ADB host-side clipboard polling | Not viable | USB debugging is already enabled | Low | The stock shell clipboard command is unavailable/restricted on the tested device. It is not a general or secure solution. |
| Root or Shizuku helper | Strong where available | Root or Shizuku setup | Medium | Can potentially bypass restrictions, but has substantial setup and security costs. Not appropriate as the default path. |

## Recommendation

### Immediate, reliable path: Android share-sheet action

Implement an Android share target labelled, for example, **Send text to DeskDock server**.

- It receives text deliberately shared by the user.
- It sends that text over the existing family-2 clipboard transport.
- The existing Linux receiver can write it through `wl-copy`.
- It works without root, an accessibility service, or default-IME status.
- It has clear user intent and a better privacy model than background clipboard monitoring.

This is the recommended first Android-side feature if Android-to-Linux transfer is needed now.

### Optional continuous synchronization: IME companion

If transparent, bidirectional continuous synchronization is required, implement a companion default keyboard / IME.

- This is the most reliable platform-supported route for clipboard access on modern Android.
- It requires substantially more implementation work than a share-sheet receiver.
- The keyboard experience must be designed so selecting it does not degrade ordinary typing.

## Do not rely on

Do not rely on the existing DeskDock 1.2.4 `ClipboardManager` listener for Android-to-Linux synchronization on Android 14 / HyperOS. The preference can be enabled and the DeskDock connection can be healthy, while the Android OS still does not deliver an observable clipboard-change event to the app.
