#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use crate::apple_intelligence;
use crate::audio_feedback::{play_feedback_sound, play_feedback_sound_blocking, SoundType};
use crate::managers::audio::AudioRecordingManager;
use crate::managers::history::HistoryManager;
use crate::managers::transcription::TranscriptionManager;
use crate::settings::{get_settings, AppSettings, APPLE_INTELLIGENCE_PROVIDER_ID};
use crate::shortcut;
use crate::tray::{change_tray_icon, TrayIconState};
use crate::utils::{self, show_processing_overlay, show_recording_overlay, show_transcribing_overlay};
use crate::window_focus;
use crate::TranscriptionCoordinator;
use ferrous_opencc::{config::BuiltinConfig, OpenCC};
use log::{debug, error, info, warn};
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;
use tauri::AppHandle;
use tauri::Manager;

// ── PasteQueue ─────────────────────────────────────────────────────────

/// Ensures segments within a session paste in recording order.
pub struct PasteQueue {
    next_to_paste: Mutex<u64>,
    condvar: Condvar,
}

impl PasteQueue {
    pub fn new() -> Self {
        Self {
            next_to_paste: Mutex::new(0),
            condvar: Condvar::new(),
        }
    }

    /// Block until it is this segment's turn to paste.
    fn wait_for_turn(&self, my_sequence: u64) {
        let mut next = self.next_to_paste.lock().unwrap();
        while *next != my_sequence {
            next = self.condvar.wait(next).unwrap();
        }
    }

    /// Signal that this segment's paste is done; wake the next.
    fn advance(&self) {
        let mut next = self.next_to_paste.lock().unwrap();
        *next += 1;
        self.condvar.notify_all();
    }
}

// ── SegmentFinishGuard ─────────────────────────────────────────────────

/// Drop guard that notifies the [`TranscriptionCoordinator`] when a
/// segment's async pipeline finishes — whether it completes normally or panics.
struct SegmentFinishGuard(AppHandle);

impl Drop for SegmentFinishGuard {
    fn drop(&mut self) {
        if let Some(c) = self.0.try_state::<TranscriptionCoordinator>() {
            c.notify_segment_finished();
        }
    }
}

// ── Segment lifecycle (called by the coordinator) ──────────────────────

/// Start recording a segment. Returns `true` if recording actually started.
///
/// When `is_first_segment` is `true`, captures the focused window, initiates
/// model loading, plays the start sound, and registers the cancel shortcut.
/// Continuation segments skip these one-time actions.
pub fn start_segment(app: &AppHandle, binding_id: &str, is_first_segment: bool) -> bool {
    let start_time = Instant::now();
    info!(
        "start_segment called for binding: {} (first: {})",
        binding_id, is_first_segment
    );

    // ── Start recording FIRST for minimum latency ──────────────────
    let rm = app.state::<Arc<AudioRecordingManager>>();
    let binding_id_owned = binding_id.to_string();

    let recording_started = rm.try_start_recording(&binding_id_owned);
    info!(
        "Recording started: {} (in {:?})",
        recording_started,
        start_time.elapsed()
    );

    if !recording_started {
        debug!("Failed to start recording");
        return false;
    }

    // ── Now do the rest (window capture, model, UI, sound) ─────────
    let settings = get_settings(app);

    if is_first_segment {
        if settings.restore_focus_before_paste {
            window_focus::capture_focused_app();
        }

        // Load model in the background
        let tm = app.state::<Arc<TranscriptionManager>>();
        tm.initiate_model_load();
    }

    change_tray_icon(app, TrayIconState::Recording);
    show_recording_overlay(app);

    // Audio feedback + mute in background thread
    if is_first_segment {
        let rm_clone = Arc::clone(&rm);
        let app_clone = app.clone();
        std::thread::spawn(move || {
            play_feedback_sound_blocking(&app_clone, SoundType::Start);
            rm_clone.apply_mute();
        });
    } else {
        let rm_clone = Arc::clone(&rm);
        std::thread::spawn(move || {
            rm_clone.apply_mute();
        });
    }

    if recording_started && is_first_segment {
        shortcut::register_cancel_shortcut(app);
    }

    debug!("start_segment completed in {:?}", start_time.elapsed());
    recording_started
}

/// Stop recording a segment and spawn the async transcription pipeline.
///
/// Audio samples are retrieved synchronously (fast) so the audio manager
/// is immediately available for a new recording. The actual transcription,
/// post-processing, and paste run in an async task.
pub fn stop_segment(
    app: &AppHandle,
    binding_id: &str,
    post_process: bool,
    cancel_token: Arc<AtomicBool>,
    paste_queue: Arc<PasteQueue>,
    segment_seq: u64,
) {
    let stop_time = Instant::now();
    debug!(
        "stop_segment called for binding: {} (seq: {})",
        binding_id, segment_seq
    );

    let rm = Arc::clone(&app.state::<Arc<AudioRecordingManager>>());
    let tm = Arc::clone(&app.state::<Arc<TranscriptionManager>>());
    let hm = Arc::clone(&app.state::<Arc<HistoryManager>>());

    // Unmute before playing audio feedback so the stop sound is audible
    rm.remove_mute();

    // Play audio feedback for recording stop
    play_feedback_sound(app, SoundType::Stop);

    // Retrieve samples synchronously so the audio manager is immediately
    // free for a new recording. Keep the mic stream open to avoid latency
    // if the user starts a continuation segment right away.
    let binding_id = binding_id.to_string();
    let samples = rm.stop_recording_keep_stream(&binding_id);

    // Update UI to show transcribing state
    change_tray_icon(app, TrayIconState::Transcribing);
    show_transcribing_overlay(app);

    let ah = app.clone();

    tauri::async_runtime::spawn(async move {
        let _guard = SegmentFinishGuard(ah.clone());
        debug!(
            "Starting async transcription task for segment {} (binding: {})",
            segment_seq, binding_id
        );

        let Some(samples) = samples else {
            debug!(
                "No samples retrieved from recording stop (segment {})",
                segment_seq
            );
            paste_queue.wait_for_turn(segment_seq);
            paste_queue.advance();
            return;
        };

        debug!(
            "Segment {} samples retrieved, count: {}",
            segment_seq,
            samples.len()
        );

        // Check cancellation before transcribing
        if cancel_token.load(Ordering::Relaxed) {
            debug!("Segment {} cancelled before transcription", segment_seq);
            paste_queue.wait_for_turn(segment_seq);
            paste_queue.advance();
            return;
        }

        let transcription_time = Instant::now();
        let samples_clone = samples.clone();
        match tm.transcribe(samples) {
            Ok(transcription) => {
                debug!(
                    "Segment {} transcription completed in {:?}: '{}'",
                    segment_seq,
                    transcription_time.elapsed(),
                    transcription
                );

                if transcription.is_empty() {
                    paste_queue.wait_for_turn(segment_seq);
                    paste_queue.advance();
                    return;
                }

                let settings = get_settings(&ah);
                let mut final_text = transcription.clone();
                let mut post_processed_text: Option<String> = None;
                let mut post_process_prompt: Option<String> = None;

                // Chinese variant conversion
                if let Some(converted_text) =
                    maybe_convert_chinese_variant(&settings, &transcription).await
                {
                    final_text = converted_text;
                }

                // Check cancellation before post-processing
                if cancel_token.load(Ordering::Relaxed) {
                    debug!("Segment {} cancelled before post-processing", segment_seq);
                    paste_queue.wait_for_turn(segment_seq);
                    paste_queue.advance();
                    return;
                }

                // LLM post-processing
                if post_process {
                    // Only show processing overlay if not currently recording
                    let is_recording = ah
                        .try_state::<Arc<AudioRecordingManager>>()
                        .is_some_and(|a| a.is_recording());
                    if !is_recording {
                        show_processing_overlay(&ah);
                    }
                }

                let processed = if post_process {
                    post_process_transcription(&settings, &final_text).await
                } else {
                    None
                };

                if let Some(processed_text) = processed {
                    post_processed_text = Some(processed_text.clone());
                    final_text = processed_text;

                    if let Some(prompt_id) = &settings.post_process_selected_prompt_id {
                        if let Some(prompt) = settings
                            .post_process_prompts
                            .iter()
                            .find(|p| &p.id == prompt_id)
                        {
                            post_process_prompt = Some(prompt.prompt.clone());
                        }
                    }
                } else if final_text != transcription {
                    // Chinese conversion was applied but no LLM post-processing
                    post_processed_text = Some(final_text.clone());
                }

                // Save to history
                let hm_clone = Arc::clone(&hm);
                let transcription_for_history = transcription.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = hm_clone
                        .save_transcription(
                            samples_clone,
                            transcription_for_history,
                            post_processed_text,
                            post_process_prompt,
                        )
                        .await
                    {
                        error!("Failed to save transcription to history: {}", e);
                    }
                });

                // Wait for our turn to paste (ensures ordering)
                paste_queue.wait_for_turn(segment_seq);

                // Check cancellation one last time before pasting
                if cancel_token.load(Ordering::Relaxed) {
                    debug!("Segment {} cancelled before paste", segment_seq);
                    paste_queue.advance();
                    return;
                }

                let ah_clone = ah.clone();
                let paste_time = Instant::now();
                let should_restore = settings.restore_focus_before_paste;
                let seg = segment_seq;
                ah.run_on_main_thread(move || {
                    if should_restore {
                        window_focus::restore_focused_app_keep();
                    }
                    match utils::paste(final_text, ah_clone.clone()) {
                        Ok(()) => debug!(
                            "Segment {} text pasted successfully in {:?}",
                            seg,
                            paste_time.elapsed()
                        ),
                        Err(e) => {
                            error!("Failed to paste segment {} transcription: {}", seg, e)
                        }
                    }
                })
                .unwrap_or_else(|e| {
                    error!("Failed to run paste on main thread: {:?}", e);
                });

                paste_queue.advance();
            }
            Err(err) => {
                debug!("Segment {} transcription error: {}", segment_seq, err);
                paste_queue.wait_for_turn(segment_seq);
                paste_queue.advance();
            }
        }
    });

    debug!("stop_segment completed in {:?}", stop_time.elapsed());
}

// ── Shortcut Action Trait ──────────────────────────────────────────────

pub trait ShortcutAction: Send + Sync {
    fn start(&self, app: &AppHandle, binding_id: &str, shortcut_str: &str);
    fn stop(&self, app: &AppHandle, binding_id: &str, shortcut_str: &str);
}

// ── Cancel Action ──────────────────────────────────────────────────────

struct CancelAction;

impl ShortcutAction for CancelAction {
    fn start(&self, app: &AppHandle, _binding_id: &str, _shortcut_str: &str) {
        utils::cancel_current_operation(app);
    }

    fn stop(&self, _app: &AppHandle, _binding_id: &str, _shortcut_str: &str) {
        // Nothing to do on stop for cancel
    }
}

// ── Test Action ────────────────────────────────────────────────────────

struct TestAction;

impl ShortcutAction for TestAction {
    fn start(&self, app: &AppHandle, binding_id: &str, shortcut_str: &str) {
        log::info!(
            "Shortcut ID '{}': Started - {} (App: {})",
            binding_id,
            shortcut_str,
            app.package_info().name
        );
    }

    fn stop(&self, app: &AppHandle, binding_id: &str, shortcut_str: &str) {
        log::info!(
            "Shortcut ID '{}': Stopped - {} (App: {})",
            binding_id,
            shortcut_str,
            app.package_info().name
        );
    }
}

// ── Action Map ─────────────────────────────────────────────────────────
// Transcribe actions are handled directly by the coordinator via
// start_segment / stop_segment.  Only non-transcribe actions live here.

pub static ACTION_MAP: Lazy<HashMap<String, Arc<dyn ShortcutAction>>> = Lazy::new(|| {
    let mut map = HashMap::new();
    map.insert(
        "cancel".to_string(),
        Arc::new(CancelAction) as Arc<dyn ShortcutAction>,
    );
    map.insert(
        "test".to_string(),
        Arc::new(TestAction) as Arc<dyn ShortcutAction>,
    );
    map
});

// ── Helper functions (unchanged) ───────────────────────────────────────

/// Field name for structured output JSON schema
const TRANSCRIPTION_FIELD: &str = "transcription";

/// Strip invisible Unicode characters that some LLMs may insert
fn strip_invisible_chars(s: &str) -> String {
    s.replace(['\u{200B}', '\u{200C}', '\u{200D}', '\u{FEFF}'], "")
}

/// Build a system prompt from the user's prompt template.
/// Removes `${output}` placeholder since the transcription is sent as the user message.
fn build_system_prompt(prompt_template: &str) -> String {
    prompt_template.replace("${output}", "").trim().to_string()
}

async fn post_process_transcription(settings: &AppSettings, transcription: &str) -> Option<String> {
    let provider = match settings.active_post_process_provider().cloned() {
        Some(provider) => provider,
        None => {
            debug!("Post-processing enabled but no provider is selected");
            return None;
        }
    };

    let model = settings
        .post_process_models
        .get(&provider.id)
        .cloned()
        .unwrap_or_default();

    if model.trim().is_empty() {
        debug!(
            "Post-processing skipped because provider '{}' has no model configured",
            provider.id
        );
        return None;
    }

    let selected_prompt_id = match &settings.post_process_selected_prompt_id {
        Some(id) => id.clone(),
        None => {
            debug!("Post-processing skipped because no prompt is selected");
            return None;
        }
    };

    let prompt = match settings
        .post_process_prompts
        .iter()
        .find(|prompt| prompt.id == selected_prompt_id)
    {
        Some(prompt) => prompt.prompt.clone(),
        None => {
            debug!(
                "Post-processing skipped because prompt '{}' was not found",
                selected_prompt_id
            );
            return None;
        }
    };

    if prompt.trim().is_empty() {
        debug!("Post-processing skipped because the selected prompt is empty");
        return None;
    }

    debug!(
        "Starting LLM post-processing with provider '{}' (model: {})",
        provider.id, model
    );

    let api_key = settings
        .post_process_api_keys
        .get(&provider.id)
        .cloned()
        .unwrap_or_default();

    if provider.supports_structured_output {
        debug!("Using structured outputs for provider '{}'", provider.id);

        let system_prompt = build_system_prompt(&prompt);
        let user_content = transcription.to_string();

        // Handle Apple Intelligence separately since it uses native Swift APIs
        if provider.id == APPLE_INTELLIGENCE_PROVIDER_ID {
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            {
                if !apple_intelligence::check_apple_intelligence_availability() {
                    debug!(
                        "Apple Intelligence selected but not currently available on this device"
                    );
                    return None;
                }

                let token_limit = model.trim().parse::<i32>().unwrap_or(0);
                return match apple_intelligence::process_text_with_system_prompt(
                    &system_prompt,
                    &user_content,
                    token_limit,
                ) {
                    Ok(result) => {
                        if result.trim().is_empty() {
                            debug!("Apple Intelligence returned an empty response");
                            None
                        } else {
                            let result = strip_invisible_chars(&result);
                            debug!(
                                "Apple Intelligence post-processing succeeded. Output length: {} chars",
                                result.len()
                            );
                            Some(result)
                        }
                    }
                    Err(err) => {
                        error!("Apple Intelligence post-processing failed: {}", err);
                        None
                    }
                };
            }

            #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
            {
                debug!("Apple Intelligence provider selected on unsupported platform");
                return None;
            }
        }

        // Define JSON schema for transcription output
        let json_schema = serde_json::json!({
            "type": "object",
            "properties": {
                (TRANSCRIPTION_FIELD): {
                    "type": "string",
                    "description": "The cleaned and processed transcription text"
                }
            },
            "required": [TRANSCRIPTION_FIELD],
            "additionalProperties": false
        });

        match crate::llm_client::send_chat_completion_with_schema(
            &provider,
            api_key.clone(),
            &model,
            user_content,
            Some(system_prompt),
            Some(json_schema),
        )
        .await
        {
            Ok(Some(content)) => {
                // Parse the JSON response to extract the transcription field
                match serde_json::from_str::<serde_json::Value>(&content) {
                    Ok(json) => {
                        if let Some(transcription_value) =
                            json.get(TRANSCRIPTION_FIELD).and_then(|t| t.as_str())
                        {
                            let result = strip_invisible_chars(transcription_value);
                            debug!(
                                "Structured output post-processing succeeded for provider '{}'. Output length: {} chars",
                                provider.id,
                                result.len()
                            );
                            return Some(result);
                        } else {
                            error!("Structured output response missing 'transcription' field");
                            return Some(strip_invisible_chars(&content));
                        }
                    }
                    Err(e) => {
                        error!(
                            "Failed to parse structured output JSON: {}. Returning raw content.",
                            e
                        );
                        return Some(strip_invisible_chars(&content));
                    }
                }
            }
            Ok(None) => {
                error!("LLM API response has no content");
                return None;
            }
            Err(e) => {
                warn!(
                    "Structured output failed for provider '{}': {}. Falling back to legacy mode.",
                    provider.id, e
                );
                // Fall through to legacy mode below
            }
        }
    }

    // Legacy mode: Replace ${output} variable in the prompt with the actual text
    let processed_prompt = prompt.replace("${output}", transcription);
    debug!("Processed prompt length: {} chars", processed_prompt.len());

    match crate::llm_client::send_chat_completion(&provider, api_key, &model, processed_prompt)
        .await
    {
        Ok(Some(content)) => {
            let content = strip_invisible_chars(&content);
            debug!(
                "LLM post-processing succeeded for provider '{}'. Output length: {} chars",
                provider.id,
                content.len()
            );
            Some(content)
        }
        Ok(None) => {
            error!("LLM API response has no content");
            None
        }
        Err(e) => {
            error!(
                "LLM post-processing failed for provider '{}': {}. Falling back to original transcription.",
                provider.id,
                e
            );
            None
        }
    }
}

async fn maybe_convert_chinese_variant(
    settings: &AppSettings,
    transcription: &str,
) -> Option<String> {
    // Check if language is set to Simplified or Traditional Chinese
    let is_simplified = settings.selected_language == "zh-Hans";
    let is_traditional = settings.selected_language == "zh-Hant";

    if !is_simplified && !is_traditional {
        debug!("selected_language is not Simplified or Traditional Chinese; skipping translation");
        return None;
    }

    debug!(
        "Starting Chinese translation using OpenCC for language: {}",
        settings.selected_language
    );

    // Use OpenCC to convert based on selected language
    let config = if is_simplified {
        // Convert Traditional Chinese to Simplified Chinese
        BuiltinConfig::Tw2sp
    } else {
        // Convert Simplified Chinese to Traditional Chinese
        BuiltinConfig::S2twp
    };

    match OpenCC::from_config(config) {
        Ok(converter) => {
            let converted = converter.convert(transcription);
            debug!(
                "OpenCC translation completed. Input length: {}, Output length: {}",
                transcription.len(),
                converted.len()
            );
            Some(converted)
        }
        Err(e) => {
            error!("Failed to initialize OpenCC converter: {}. Falling back to original transcription.", e);
            None
        }
    }
}
