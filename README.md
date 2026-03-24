# Handy (fork)

Fork of [cjpais/Handy](https://github.com/cjpais/Handy) — a free, open source, offline speech-to-text app.

This fork adds two features to the upstream project: **multi-segment recording** and **precise window focus restoration**.

---

## Multi-segment recording

The upstream app has a strictly sequential pipeline: you record, wait for the transcription to finish, then record again. With Whisper Large, a 30-second recording takes ~10 seconds to transcribe — during which the app is blocked.

This fork lets you **start a new recording while the previous one is still being transcribed**. You control the sentence boundaries yourself by releasing and re-pressing the shortcut key. Each segment is transcribed in the background and pasted as soon as it's ready, in recording order.

### How it works

1. Press the shortcut, speak a sentence, release the shortcut.
2. The transcription starts in the background.
3. Press the shortcut again immediately and speak the next sentence.
4. Segment 1 is pasted as soon as its transcription finishes.
5. Segment 2 is pasted right after, in order.

There is no quality loss from splitting — you choose where to cut, so Whisper always gets complete sentences.

### Technical details

- The `TranscriptionCoordinator` uses a session-based state machine (`Idle` / `Active`) that tracks the current recording and pending transcriptions.
- A `transcribe_lock` in `TranscriptionManager` serializes engine access so queued segments wait instead of failing.
- A `PasteQueue` guarantees segments are pasted in the order they were recorded.
- The microphone stream stays open between segments to avoid the ~120ms latency of closing and reopening the audio device.
- Recording starts before any UI setup (tray icon, overlay, model loading) for minimum key-press-to-recording latency.

### Trailing punctuation

When the "Append trailing space" setting is enabled, the fork appends `. ` (period + space) instead of just a space — unless the text already ends with sentence-ending punctuation (`.` `!` `?` `…` and CJK equivalents).

---

## Window focus restoration

With the original Handy, if you press the transcription shortcut in one window, then switch to another window while the transcription is processing, the text gets pasted into the *wrong* window.

This fork fixes that. When you press the shortcut, Handy remembers which **exact window** was focused. Before pasting, it re-focuses that window. Your text always lands where you intended.

### Single-window focus

On macOS, activating an application normally brings **all** its windows to the front. This fork avoids that: it uses `AXRaise` on the specific window first, then activates the app via `NSRunningApplication.activateWithOptions` with only the `NSApplicationActivateIgnoringOtherApps` flag (without `NSApplicationActivateAllWindows`). If you have five Terminal windows behind Safari, only the one that was originally focused comes to the front.

### Mouse drag awareness

If you start dragging a window during transcription (e.g. moving Safari with the touchpad held down), Handy waits for the mouse button to be released before restoring focus and pasting. This prevents the paste from landing in the dragged window instead of the intended target.

### Platform support

- **macOS**: Captures the focused window's title, position, and size via the Accessibility API (`AXFocusedWindow`). Restores with `AXRaise` + `NSRunningApplication`. Falls back gracefully if the window was closed.
- **Windows**: Uses `GetForegroundWindow` / `SetForegroundWindow` which already works at the window level.
- **Linux**: Not supported (Wayland limitation). Degrades to default behavior.

### Settings

The feature is **enabled by default**. Toggle it off in **Settings > Advanced > Output > Restore Window Focus**.

---

## Building

```bash
bun install
CMAKE_POLICY_VERSION_MINIMUM=3.5 bun run tauri dev    # development
CMAKE_POLICY_VERSION_MINIMUM=3.5 bun run tauri build   # production (.dmg on macOS)
```

See the [upstream README](https://github.com/cjpais/Handy) for full documentation, model setup, and platform notes.
