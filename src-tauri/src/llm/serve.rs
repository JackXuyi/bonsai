//! Everything that owns the bundled server process.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use tauri::{Emitter, Manager};

use super::{OllamaClient, DEFAULT_BASE_URL, DEFAULT_MODEL};

/// The app-owned port. System-wide Ollama (if any) owns 11434; we never
/// touch it — one app, one server, one model store.
pub const PORT: u16 = 11435;

/// Tauri sidecar file name for the current platform (`ollama-<triple>`), as
/// it sits in `src-tauri/binaries/` before the build copies it.
fn sidecar_name() -> String {
    let arch = match std::env::consts::ARCH {
        "aarch64" => "aarch64",
        "x86_64" => "x86_64",
        other => other,
    };
    format!("ollama-{arch}-apple-darwin")
}

/// Candidate paths for the bundled binary, in lookup order.
///
/// tauri-build strips the target-triple suffix when it copies `externalBin`
/// next to the app executable: dev = `target/debug/ollama`, packaged macOS =
/// `Contents/MacOS/ollama`. The suffixed name is only present if someone
/// dropped the raw build asset there by hand, so it is just a fallback.
fn sidecar_candidates() -> Vec<PathBuf> {
    let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())) else {
        return Vec::new();
    };
    vec![dir.join("ollama"), dir.join(sidecar_name())]
}

pub(crate) fn sidecar_path() -> Option<PathBuf> {
    sidecar_candidates().into_iter().find(|p| p.exists())
}

/// Model store root — shared with `llm_admin` (disk-space check before pull).
pub fn models_dir(app: &tauri::AppHandle) -> PathBuf {
    if tauri::is_dev() {
        dev_models_dir()
    } else {
        app.path()
            .app_data_dir()
            .unwrap_or_else(|_| std::env::temp_dir())
            .join("models")
    }
}

/// Model store: project-local during dev (so `pnpm llm:pull` and the app
/// share one store), app data once packaged.
fn dev_models_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri always has a parent")
        .join(".llm/models")
}

/// App-owned state: the spawned server child, if any.
#[derive(Default)]
pub struct ServerHandle {
    child: std::sync::Mutex<Option<Child>>,
}

impl ServerHandle {
    /// Kill the sidecar. Idempotent; safe on the exit path.
    pub fn stop(&self) {
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// How one startup attempt ended.
enum Outcome {
    /// A server answers on the port and `llm:ready` was emitted.
    Ready,
    /// Worth another attempt (port race, child died, cold start timed out).
    Retry,
    /// Retrying cannot help (nothing bundled, spawn refused, app shutting down).
    Fatal,
}

/// Spawn the sidecar (best effort) and emit `llm:ready` once healthy.
/// Runs in a background thread: a cold or missing sidecar must never
/// delay startup — the model legs degrade on their own.
///
/// Retries a few times because attempts collide in practice: a sidecar
/// orphaned by a force-killed dev run keeps the port busy for a moment after
/// it is killed, and `tauri dev` restarts the app faster than that.
pub fn start(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        // Base URL stays fixed at the app-owned port (a user-wide OLLAMA_HOST
        // must not redirect the probe), but the model name follows the UI
        // override so `llm:ready` reports what actually answers (spec-m1).
        let model = crate::llm_admin::override_model().unwrap_or_else(|| DEFAULT_MODEL.into());
        let probe = OllamaClient::new(DEFAULT_BASE_URL, model);
        for attempt in 1..=3 {
            if attempt > 1 {
                log::warn!("retrying llm sidecar start (attempt {attempt}/3)");
                std::thread::sleep(Duration::from_secs(3));
            }
            match attempt_start(&app, &probe) {
                Outcome::Ready | Outcome::Fatal => return,
                Outcome::Retry => continue,
            }
        }
        log::warn!("llm sidecar did not start after 3 attempts; model legs stay on fallbacks");
    });
}

fn attempt_start(app: &tauri::AppHandle, probe: &OllamaClient) -> Outcome {
    // Reuse a server that is already answering (orphan from a force-killed
    // dev run, or a second app instance). Spawning into a taken port would
    // only produce a child that exits at once.
    if probe.is_up() {
        log::info!("llm server already answering on :{PORT} — reusing it");
        let _ = app.emit("llm:ready", probe.model());
        return Outcome::Ready;
    }

    let Some(path) = sidecar_path() else {
        log::warn!("llm sidecar not bundled; model legs stay on fallbacks");
        return Outcome::Fatal;
    };
    // Since v0.40 inference needs the separate `llama-server` runner next
    // to the CLI; without it the server starts but every chat call fails.
    if let Some(dir) = path.parent() {
        if !dir.join("llama-server").exists() {
            log::warn!(
                "llama-server runner missing next to the sidecar — run 'pnpm llm:sidecar' (chats will fail otherwise)"
            );
        }
    }
    let models = models_dir(app);
    if let Err(e) = std::fs::create_dir_all(&models) {
        log::warn!("cannot create model dir {}: {e}", models.display());
        return Outcome::Fatal;
    }

    // A bare `ollama` invocation is the CLI (it prints usage / tries to
    // reach a server and exits) — the server only starts with `serve`.
    // Sidecar stderr goes to a file so a failed start is diagnosable.
    let log_path = app
        .path()
        .app_log_dir()
        .unwrap_or_else(|_| std::env::temp_dir())
        .join("llm-sidecar.log");
    if let Some(dir) = log_path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let stderr = std::fs::File::create(&log_path)
        .map(Stdio::from)
        .unwrap_or_else(|_| Stdio::null());

    let child = Command::new(&path)
        .arg("serve")
        .env("OLLAMA_MODELS", &models)
        .env("OLLAMA_HOST", format!("127.0.0.1:{PORT}"))
        .stdout(Stdio::null())
        .stderr(stderr)
        .spawn();
    let Ok(child) = child else {
        log::warn!("failed to spawn llm sidecar at {}", path.display());
        return Outcome::Fatal;
    };
    let handle = app.state::<ServerHandle>();
    *handle.child.lock().unwrap() = Some(child);

    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while std::time::Instant::now() < deadline {
        // Bail early if the server process died (placeholder binary,
        // port clash, crash) instead of burning the whole timeout.
        if let Some(child) = handle.child.lock().unwrap().as_mut() {
            if child.try_wait().map(|s| s.is_some()).unwrap_or(false) {
                // Someone else may have bound the port between our probe
                // and the spawn — if a server answers now, that is fine.
                if probe.is_up() {
                    log::info!("llm sidecar exited but :{PORT} is served — reusing it");
                    let _ = app.emit("llm:ready", probe.model());
                    return Outcome::Ready;
                }
                log::warn!(
                    "llm sidecar exited immediately (port {PORT} taken?) — see {}",
                    log_path.display()
                );
                return Outcome::Retry;
            }
        }
        if probe.is_up() {
            log::info!("llm sidecar ready on :{PORT} (model {})", probe.model());
            let _ = app.emit("llm:ready", probe.model());
            return Outcome::Ready;
        }
        if handle.child.lock().unwrap().is_none() {
            return Outcome::Fatal; // stopped via ServerHandle::stop (app exit)
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    log::warn!("llm sidecar did not become healthy within 60s");
    Outcome::Retry
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_name_follows_tauri_convention() {
        let name = sidecar_name();
        assert!(name.starts_with("ollama-"));
        assert!(name.ends_with("-apple-darwin"));
    }

    #[test]
    fn sidecar_lookup_prefers_the_stripped_name_tauri_copies() {
        // tauri-build drops the triple suffix when copying externalBin next to
        // the executable, so plain `ollama` must be probed first — this was
        // exactly why the server never started in dev.
        let candidates = sidecar_candidates();
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].file_name().unwrap().to_str(), Some("ollama"));
        assert_eq!(
            candidates[1].file_name().unwrap().to_str(),
            Some(sidecar_name().as_str())
        );
    }

    #[test]
    fn dev_models_dir_is_project_local() {
        assert!(dev_models_dir().ends_with(".llm/models"));
    }
}
