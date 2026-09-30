//! Suspends user-listed applications (e.g. a Blender render) while a
//! transcription runs, so the speech model gets the GPU and CPU to itself.
//!
//! macOS exposes no API to prioritise one app's GPU work over another's, so a
//! render saturating the GPU can make Whisper many times slower. Freezing the
//! render with `SIGSTOP` frees the GPU within about a second, and `SIGCONT`
//! lets it carry on exactly where it stopped.
//!
//! Pauses are reference-counted: segments queued behind one another keep the
//! apps paused until the last one has been transcribed. A watchdog resumes
//! everything if a transcription runs for longer than [`MAX_PAUSE`], and
//! [`resume_listed_apps`] is called at startup to thaw anything left frozen
//! by a crash (the release profile aborts on panic, so drop guards don't run).

use log::{debug, info, warn};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

/// Longest time an app may stay paused, even if a transcription hangs.
const MAX_PAUSE: Duration = Duration::from_secs(120);

#[cfg(unix)]
/// Processes that must never be frozen: doing so would hang the desktop or
/// stop Handy itself from recording and pasting.
const PROTECTED_PROCESSES: &[&str] = &[
    "handy",
    "launchd",
    "kernel_task",
    "windowserver",
    "loginwindow",
    "dock",
    "finder",
    "systemuiserver",
    "coreaudiod",
    "systemd",
    "xorg",
    "gnome-shell",
    "kwin_wayland",
    "kwin_x11",
    "pipewire",
    "pulseaudio",
];

struct PauseState {
    /// Number of transcriptions currently waiting for or running the engine.
    holders: usize,
    /// PIDs this module stopped and still has to resume.
    paused: Vec<i32>,
    /// Bumped on every pause so a stale watchdog does not resume a newer one.
    generation: u64,
}

static STATE: Mutex<PauseState> = Mutex::new(PauseState {
    holders: 0,
    paused: Vec::new(),
    generation: 0,
});

/// Keeps the listed apps paused until dropped.
pub struct PauseGuard(());

impl Drop for PauseGuard {
    fn drop(&mut self) {
        let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.holders = state.holders.saturating_sub(1);
        if state.holders == 0 {
            resume_pids(&state.paused);
            state.paused.clear();
        }
    }
}

/// Pauses every running process whose name matches one of `app_names`
/// (case-insensitive) and keeps them paused until the returned guard and all
/// other outstanding guards are dropped.
pub fn pause_apps(app_names: &[String]) -> PauseGuard {
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.holders += 1;

    if state.holders == 1 {
        let pids = find_pids(app_names);
        state.paused = pids.into_iter().filter(|&pid| stop_pid(pid)).collect();

        if !state.paused.is_empty() {
            info!(
                "Paused {} process(es) for transcription: {:?}",
                state.paused.len(),
                state.paused
            );
            state.generation += 1;
            spawn_watchdog(state.generation);
        }
    }

    PauseGuard(())
}

/// Resumes every running process matching `app_names`. Called at startup to
/// recover from a crash that happened while apps were paused.
pub fn resume_listed_apps(app_names: &[String]) {
    let pids = find_pids(app_names);
    if !pids.is_empty() {
        debug!("Resuming listed apps at startup: {:?}", pids);
        resume_pids(&pids);
    }
}

/// Resumes whatever is currently paused. Called when the app exits.
pub fn resume_all() {
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    resume_pids(&state.paused);
    state.paused.clear();
}

fn spawn_watchdog(generation: u64) {
    thread::spawn(move || {
        thread::sleep(MAX_PAUSE);
        let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
        if state.generation == generation && !state.paused.is_empty() {
            warn!(
                "Transcription still running after {:?}, resuming paused apps",
                MAX_PAUSE
            );
            resume_pids(&state.paused);
            state.paused.clear();
        }
    });
}

#[cfg(unix)]
fn is_protected(name: &str) -> bool {
    PROTECTED_PROCESSES
        .iter()
        .any(|p| p.eq_ignore_ascii_case(name))
}

#[cfg(unix)]
fn find_pids(app_names: &[String]) -> Vec<i32> {
    use std::process::Command;

    let own_pid = std::process::id() as i32;
    let mut pids = Vec::new();

    for name in app_names.iter().map(|n| n.trim()) {
        if name.is_empty() {
            continue;
        }
        if is_protected(name) {
            warn!("Refusing to pause protected process '{}'", name);
            continue;
        }

        // pgrep takes an extended regex, so escape the name to match it literally.
        let pattern = regex::escape(name);
        let output = match Command::new("pgrep").args(["-i", "-x", &pattern]).output() {
            Ok(output) => output,
            Err(e) => {
                warn!("Failed to run pgrep for '{}': {}", name, e);
                continue;
            }
        };

        pids.extend(
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter_map(|line| line.trim().parse::<i32>().ok())
                .filter(|&pid| pid > 1 && pid != own_pid),
        );
    }

    pids.sort_unstable();
    pids.dedup();
    pids
}

#[cfg(unix)]
fn stop_pid(pid: i32) -> bool {
    // SAFETY: kill() has no memory-safety preconditions.
    let ok = unsafe { libc::kill(pid, libc::SIGSTOP) } == 0;
    if !ok {
        warn!(
            "Failed to pause process {}: {}",
            pid,
            std::io::Error::last_os_error()
        );
    }
    ok
}

#[cfg(unix)]
fn resume_pids(pids: &[i32]) {
    for &pid in pids {
        // SAFETY: kill() has no memory-safety preconditions.
        if unsafe { libc::kill(pid, libc::SIGCONT) } != 0 {
            debug!(
                "Failed to resume process {}: {}",
                pid,
                std::io::Error::last_os_error()
            );
        }
    }
    if !pids.is_empty() {
        info!("Resumed {} paused process(es)", pids.len());
    }
}

// Windows has no SIGSTOP equivalent that is safe to use on arbitrary apps,
// so pausing is a no-op there.
#[cfg(not(unix))]
fn find_pids(_app_names: &[String]) -> Vec<i32> {
    Vec::new()
}

#[cfg(not(unix))]
fn stop_pid(_pid: i32) -> bool {
    false
}

#[cfg(not(unix))]
fn resume_pids(_pids: &[i32]) {}
