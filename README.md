# Handy (fork with window focus restore)

Fork of [cjpais/Handy](https://github.com/cjpais/Handy) — a free, open source, offline speech-to-text app.

## What this fork adds

**Window focus restoration before pasting.**

With the original Handy, if you press the transcription shortcut in a window, speak, and then switch to another window while the transcription is processing, the text gets pasted into the *wrong* window — whichever one happens to be focused when the transcription finishes.

This fork fixes that. When you press the shortcut, Handy remembers which **exact window** was focused (not just the application — the specific window). Before pasting the result, it re-focuses that window. So even if you switch apps or switch between multiple windows of the same app during transcription, your text always lands where you intended.

### How it works

- **macOS**: Captures the frontmost window's title, position, and size via the Accessibility API (`AXFocusedWindow`). On restore, it re-activates the app and `AXRaise`s the matching window. Falls back gracefully if the window was closed.
- **Windows**: Uses `GetForegroundWindow` / `SetForegroundWindow` which already works at the window level.
- **Linux**: Not supported (Wayland limitation). Degrades to default behavior.

### Settings

The feature is **enabled by default**. You can toggle it off in **Settings > Advanced > Output > Restore Window Focus**.

## Building

```bash
bun install
CMAKE_POLICY_VERSION_MINIMUM=3.5 bun run tauri dev    # development
CMAKE_POLICY_VERSION_MINIMUM=3.5 bun run tauri build   # production
```

See the [upstream README](https://github.com/cjpais/Handy) for full documentation, model setup, and platform notes.
