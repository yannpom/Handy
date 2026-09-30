# Handy (fork)

Fork of [cjpais/Handy](https://github.com/cjpais/Handy) — a free, open source, offline speech-to-text app.

This fork adds four improvements over the original:

## Talk without waiting

The original app blocks while transcribing — you have to wait before dictating the next sentence. This fork lets you **keep talking immediately**. Release the shortcut, press it again, and speak your next sentence. Each segment is transcribed in the background and pasted in order, as fast as your machine allows.

No quality loss: you choose where to cut, so the model always gets complete sentences.

## Your text lands in the right window

Ever start dictating in one window, switch to another while waiting, and find the text pasted in the wrong place? This fork remembers which exact window was focused when you pressed the shortcut and restores it before pasting. It even waits if you're mid-drag with your mouse.

On macOS, only the target window comes to the front — not every window of that app.

## Distil Large V3 FR — fast French transcription

This fork includes [Distil Large V3 FR](https://huggingface.co/eustlb/distil-large-v3-fr), a distilled Whisper model optimized for French. It's **5.9x faster** than Whisper Large with nearly the same accuracy. Available directly from the model selector — no manual setup needed.

## Transcription stays fast during a 3D render

A Blender render on the GPU can make transcription 5–10x slower: macOS has no way to give one app's GPU work priority over another's. Turn on **Pause Apps During Transcription** (Settings → Advanced → Transcription) and Handy freezes the apps you list — Blender by default — for the few seconds it takes to transcribe, then lets them carry on exactly where they stopped. They keep running while you speak; only the transcription itself pauses them.

macOS and Linux only. A watchdog resumes everything after two minutes, and Handy thaws the listed apps at startup in case it ever quit while they were paused.

---

See the [upstream project](https://github.com/cjpais/Handy) for full documentation, supported models, and platform notes.
