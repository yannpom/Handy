use crate::actions::{start_segment, stop_segment, PasteQueue, ACTION_MAP};
use crate::managers::audio::AudioRecordingManager;
use crate::managers::transcription::TranscriptionManager;
use crate::shortcut;
use crate::tray::{change_tray_icon, TrayIconState};
use crate::utils::hide_recording_overlay;
use crate::window_focus;
use log::{debug, error, warn};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

const DEBOUNCE: Duration = Duration::from_millis(30);

/// Commands processed sequentially by the coordinator thread.
enum Command {
    Input {
        binding_id: String,
        is_pressed: bool,
        push_to_talk: bool,
    },
    Cancel,
    SegmentFinished,
}

/// Pipeline lifecycle, owned exclusively by the coordinator thread.
///
/// A "session" begins when the user starts recording the first segment and
/// ends when all segments have been transcribed and pasted (or cancelled).
/// Within a session the user may record multiple segments back-to-back;
/// each segment's transcription runs in the background while the next
/// segment is being recorded.
enum Stage {
    Idle,
    Active {
        /// `Some(binding_id)` while the microphone is recording a segment.
        recording: Option<String>,
        /// Number of async transcription tasks that have not yet finished.
        pending_transcriptions: u32,
        /// The binding that started this session (determines post-processing).
        session_binding_id: String,
        /// Shared flag — set to `true` on cancel so in-flight tasks skip work.
        cancel_token: Arc<AtomicBool>,
        /// Ordering primitive so segments paste in recording order.
        paste_queue: Arc<PasteQueue>,
        /// Monotonically increasing counter for the next segment.
        segment_counter: u64,
    },
}

/// Serialises all transcription lifecycle events through a single thread
/// to eliminate race conditions between keyboard shortcuts, signals, and
/// the async transcribe-paste pipeline.
pub struct TranscriptionCoordinator {
    tx: Sender<Command>,
}

pub fn is_transcribe_binding(id: &str) -> bool {
    id == "transcribe" || id == "transcribe_with_post_process"
}

fn is_post_process_binding(id: &str) -> bool {
    id == "transcribe_with_post_process"
}

impl TranscriptionCoordinator {
    pub fn new(app: AppHandle) -> Self {
        let (tx, rx) = mpsc::channel();

        thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut stage = Stage::Idle;
                let mut last_press: Option<Instant> = None;

                while let Ok(cmd) = rx.recv() {
                    match cmd {
                        Command::Input {
                            binding_id,
                            is_pressed,
                            push_to_talk,
                        } => {
                            // Debounce rapid-fire press events (key repeat / double-tap).
                            // Releases always pass through for push-to-talk.
                            if is_pressed {
                                let now = Instant::now();
                                if last_press.map_or(false, |t| now.duration_since(t) < DEBOUNCE) {
                                    debug!("Debounced press for '{binding_id}'");
                                    continue;
                                }
                                last_press = Some(now);
                            }

                            // Non-transcribe bindings: delegate to ACTION_MAP directly.
                            if !is_transcribe_binding(&binding_id) {
                                if let Some(action) = ACTION_MAP.get(&binding_id) {
                                    if is_pressed {
                                        action.start(&app, &binding_id, "");
                                    } else {
                                        action.stop(&app, &binding_id, "");
                                    }
                                }
                                continue;
                            }

                            // ── Determine what to do ──────────────────────────
                            let should_start = match (&stage, is_pressed, push_to_talk) {
                                // Idle + press (toggle or push-to-talk) → new session
                                (Stage::Idle, true, _) => true,
                                // Active, not recording, same binding → continuation
                                (
                                    Stage::Active {
                                        recording: None,
                                        session_binding_id,
                                        ..
                                    },
                                    true,
                                    _,
                                ) if *session_binding_id == binding_id => true,
                                _ => false,
                            };

                            let should_stop = match (&stage, is_pressed, push_to_talk) {
                                // Toggle: press while recording same binding → stop
                                (
                                    Stage::Active {
                                        recording: Some(active_id),
                                        ..
                                    },
                                    true,
                                    false,
                                ) if *active_id == binding_id => true,
                                // Push-to-talk: release while recording same binding → stop
                                (
                                    Stage::Active {
                                        recording: Some(active_id),
                                        ..
                                    },
                                    false,
                                    true,
                                ) if *active_id == binding_id => true,
                                _ => false,
                            };

                            if should_start {
                                let is_first = matches!(stage, Stage::Idle);
                                let recording_started =
                                    start_segment(&app, &binding_id, is_first);

                                if recording_started {
                                    if is_first {
                                        stage = Stage::Active {
                                            recording: Some(binding_id.clone()),
                                            pending_transcriptions: 0,
                                            session_binding_id: binding_id.clone(),
                                            cancel_token: Arc::new(AtomicBool::new(false)),
                                            paste_queue: Arc::new(PasteQueue::new()),
                                            segment_counter: 0,
                                        };
                                    } else if let Stage::Active {
                                        ref mut recording, ..
                                    } = stage
                                    {
                                        *recording = Some(binding_id.clone());
                                    }
                                } else {
                                    debug!(
                                        "Start for '{binding_id}' did not begin recording"
                                    );
                                }
                            } else if should_stop {
                                if let Stage::Active {
                                    ref mut recording,
                                    ref mut pending_transcriptions,
                                    ref session_binding_id,
                                    ref cancel_token,
                                    ref paste_queue,
                                    ref mut segment_counter,
                                } = stage
                                {
                                    let post_process =
                                        is_post_process_binding(session_binding_id);
                                    let seq = *segment_counter;
                                    *segment_counter += 1;
                                    *pending_transcriptions += 1;
                                    *recording = None;

                                    stop_segment(
                                        &app,
                                        &binding_id,
                                        post_process,
                                        Arc::clone(cancel_token),
                                        Arc::clone(paste_queue),
                                        seq,
                                    );
                                }
                            } else if is_pressed {
                                debug!(
                                    "Ignoring press for '{binding_id}': session busy or different binding"
                                );
                            }
                        }

                        Command::Cancel => {
                            if let Stage::Active {
                                ref recording,
                                ref cancel_token,
                                ..
                            } = stage
                            {
                                debug!("Cancelling active session");

                                // Signal all in-flight async tasks to skip
                                cancel_token.store(true, Ordering::Relaxed);

                                // Cancel current recording if active
                                if recording.is_some() {
                                    if let Some(am) =
                                        app.try_state::<Arc<AudioRecordingManager>>()
                                    {
                                        am.cancel_recording();
                                    }
                                }

                                // Clean up
                                window_focus::clear_captured_app();
                                shortcut::unregister_cancel_shortcut(&app);
                                hide_recording_overlay(&app);
                                change_tray_icon(&app, TrayIconState::Idle);

                                // Close mic stream kept open for continuation segments
                                if let Some(am) =
                                    app.try_state::<Arc<AudioRecordingManager>>()
                                {
                                    am.close_stream_if_on_demand();
                                }

                                if let Some(tm) =
                                    app.try_state::<Arc<TranscriptionManager>>()
                                {
                                    tm.maybe_unload_immediately("cancellation");
                                }

                                stage = Stage::Idle;
                            } else {
                                debug!("Cancel received but already idle");
                            }
                        }

                        Command::SegmentFinished => {
                            if let Stage::Active {
                                ref recording,
                                ref mut pending_transcriptions,
                                ..
                            } = stage
                            {
                                *pending_transcriptions =
                                    pending_transcriptions.saturating_sub(1);
                                debug!(
                                    "Segment finished. Pending: {}, Recording: {}",
                                    pending_transcriptions,
                                    recording.is_some()
                                );

                                if *pending_transcriptions == 0 && recording.is_none() {
                                    debug!("Session complete, returning to idle");
                                    window_focus::clear_captured_app();
                                    shortcut::unregister_cancel_shortcut(&app);
                                    hide_recording_overlay(&app);
                                    change_tray_icon(&app, TrayIconState::Idle);

                                    // Close mic stream kept open for continuation segments
                                    if let Some(am) =
                                        app.try_state::<Arc<AudioRecordingManager>>()
                                    {
                                        am.close_stream_if_on_demand();
                                    }

                                    if let Some(tm) =
                                        app.try_state::<Arc<TranscriptionManager>>()
                                    {
                                        tm.maybe_unload_immediately("session complete");
                                    }

                                    stage = Stage::Idle;
                                }
                            } else {
                                // SegmentFinished after cancel — ignore
                                debug!(
                                    "SegmentFinished received but already idle (post-cancel)"
                                );
                            }
                        }
                    }
                }
                debug!("Transcription coordinator exited");
            }));
            if let Err(e) = result {
                error!("Transcription coordinator panicked: {e:?}");
            }
        });

        Self { tx }
    }

    /// Send a keyboard/signal input event for a transcribe binding.
    /// For signal-based toggles, use `is_pressed: true` and `push_to_talk: false`.
    pub fn send_input(
        &self,
        binding_id: &str,
        hotkey_string: &str,
        is_pressed: bool,
        push_to_talk: bool,
    ) {
        let _ = hotkey_string; // reserved for future use
        if self
            .tx
            .send(Command::Input {
                binding_id: binding_id.to_string(),
                is_pressed,
                push_to_talk,
            })
            .is_err()
        {
            warn!("Transcription coordinator channel closed");
        }
    }

    pub fn notify_cancel(&self) {
        if self.tx.send(Command::Cancel).is_err() {
            warn!("Transcription coordinator channel closed");
        }
    }

    pub fn notify_segment_finished(&self) {
        if self.tx.send(Command::SegmentFinished).is_err() {
            warn!("Transcription coordinator channel closed");
        }
    }
}
