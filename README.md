# Handy (fork)

Fork of [cjpais/Handy](https://github.com/cjpais/Handy) — a free, open source, offline speech-to-text app.

This fork adds three improvements over the original:

## Talk without waiting

The original app blocks while transcribing — you have to wait before dictating the next sentence. This fork lets you **keep talking immediately**. Release the shortcut, press it again, and speak your next sentence. Each segment is transcribed in the background and pasted in order, as fast as your machine allows.

No quality loss: you choose where to cut, so the model always gets complete sentences.

## Your text lands in the right window

Ever start dictating in one window, switch to another while waiting, and find the text pasted in the wrong place? This fork remembers which exact window was focused when you pressed the shortcut and restores it before pasting. It even waits if you're mid-drag with your mouse.

On macOS, only the target window comes to the front — not every window of that app.

## Distil Large V3 FR — fast French transcription

This fork includes [Distil Large V3 FR](https://huggingface.co/eustlb/distil-large-v3-fr), a distilled Whisper model optimized for French. It's **5.9x faster** than Whisper Large with nearly the same accuracy. Available directly from the model selector — no manual setup needed.

---

See the [upstream project](https://github.com/cjpais/Handy) for full documentation, supported models, and platform notes.
