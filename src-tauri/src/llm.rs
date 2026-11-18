//! LLM access — local-first (decision 2026-11-18, supersedes hybrid routing).
//!
//! One client for both model legs, per plan-memory §5.2 / plan-persona §4.2:
//! - dialogue: persona card injected as the system prompt (`LocalBackend`)
//! - extraction/consolidation: structured-JSON replies (`LocalExtractor`)
//!
//! Transport is the Ollama-compatible chat API. The server itself is the
//! Ollama binary bundled as a Tauri sidecar (`bundle.externalBin`), owned by
//! the app: spawned at startup on port 11435 (never 11434 — that belongs to
//! a system-wide Ollama, if any), stopped on exit, models stored in
//! project-local `.llm/models` during dev and app data at runtime.
//! Everything runs on-device; conversations and memory text never leave the
//! machine. Cloud APIs are an explicit user opt-in and deliberately NOT
//! implemented here.
//!
//! Config via env (so the binary needs no UI settings yet):
//! - `OLLAMA_HOST` — base URL, default `http://127.0.0.1:11435`
//! - `BONSAI_LLM_MODEL` — model tag, default `qwen3:4b`

use std::time::Duration;

use serde::{Deserialize, Serialize};

pub const DEFAULT_BASE_URL: &str = "http://127.0.0.1:11435";
pub const DEFAULT_MODEL: &str = "qwen3:4b";
/// One dialogue turn. Long enough for a cold-loaded 7B model on Apple Silicon.
pub const CHAT_TIMEOUT_SECS: u64 = 120;
/// One extraction pass over a day's utterances (plan §5.2: on timeout the
/// consolidate pipeline falls back to `RuleExtractor` and keeps the events
/// unconsumed — no loss, retry next run).
pub const EXTRACT_TIMEOUT_SECS: u64 = 30;
/// Liveness probe.
pub const PROBE_TIMEOUT_SECS: u64 = 2;

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("http transport error: {0}")]
    Transport(String),
    #[error("server returned {0}")]
    Status(u16),
    #[error("response decode failed: {0}")]
    Decode(String),
}

#[derive(Debug, Clone)]
pub struct OllamaClient {
    base_url: String,
    model: String,
}

#[derive(Debug, Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [Msg],
    stream: bool,
    /// Qwen3-class models default to a visible reasoning pass that can burn
    /// the whole token budget before emitting any answer (observed with
    /// qwen3:4b: `content` came back empty, `done_reason: length`). Both legs
    /// want direct output — dialogue for latency, extraction for clean JSON.
    think: bool,
    options: Options<'a>,
}

#[derive(Debug, Serialize)]
struct Options<'a> {
    temperature: f64,
    #[serde(skip_serializing_if = "is_zero")]
    num_ctx: &'a usize,
}

fn is_zero(v: &usize) -> bool {
    *v == 0
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    message: RespMessage,
}

#[derive(Debug, Deserialize)]
struct RespMessage {
    content: String,
}

/// A single chat message. `role` is "system" | "user" | "assistant".
#[derive(Debug, Clone, Serialize)]
pub struct Msg {
    role: String,
    content: String,
}

impl Msg {
    pub fn system(content: impl Into<String>) -> Self {
        Msg { role: "system".into(), content: content.into() }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Msg { role: "user".into(), content: content.into() }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Msg { role: "assistant".into(), content: content.into() }
    }
}

impl OllamaClient {
    /// Base URL from `OLLAMA_HOST`, model from `BONSAI_LLM_MODEL`.
    pub fn from_env() -> Self {
        OllamaClient {
            base_url: std::env::var("OLLAMA_HOST").unwrap_or_else(|_| DEFAULT_BASE_URL.into()),
            model: std::env::var("BONSAI_LLM_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into()),
        }
    }

    pub fn new(base_url: impl Into<String>, model: impl Into<String>) -> Self {
        OllamaClient { base_url: base_url.into(), model: model.into() }
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// Cheap liveness probe: the server answers `/api/version` without
    /// loading any model.
    pub fn is_up(&self) -> bool {
        ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(PROBE_TIMEOUT_SECS))
            .build()
            .get(&format!("{}/api/version", self.base_url))
            .call()
            .is_ok()
    }

    /// One chat completion. `messages` must already carry the system prompt
    /// (persona card for dialogue, extraction instructions for memory).
    pub fn chat(&self, messages: &[Msg], temperature: f64, timeout: u64) -> Result<String, LlmError> {
        let req = ChatRequest {
            model: &self.model,
            messages,
            stream: false,
            think: false,
            options: Options { temperature, num_ctx: &0 },
        };
        let resp = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(timeout))
            .build()
            .post(&format!("{}/api/chat", self.base_url))
            .send_json(&req)
            .map_err(|e| match e {
                ureq::Error::Status(code, _) => LlmError::Status(code),
                other => LlmError::Transport(other.to_string()),
            })?;
        let body: ChatResponse = resp
            .into_json()
            .map_err(|e| LlmError::Decode(e.to_string()))?;
        Ok(body.message.content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_config_falls_back_to_defaults() {
        // Neither variable is set in test runs; even if set, the client must
        // still construct. Defaults are asserted only when unset.
        let client = OllamaClient::from_env();
        assert!(!client.model().is_empty());
        if std::env::var("BONSAI_LLM_MODEL").is_err() {
            assert_eq!(client.model(), DEFAULT_MODEL);
        }
    }

    #[test]
    fn roles_serialize_lowercase() {
        let msgs = vec![Msg::system("s"), Msg::user("u"), Msg::assistant("a")];
        assert_eq!(msgs[0].role, "system");
        assert_eq!(msgs[1].role, "user");
        assert_eq!(msgs[2].role, "assistant");
    }

    #[test]
    fn chat_request_disables_thinking() {
        let msgs = vec![Msg::user("hi")];
        let req = ChatRequest {
            model: "qwen3:4b",
            messages: &msgs,
            stream: false,
            think: false,
            options: Options { temperature: 0.7, num_ctx: &0 },
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["think"], serde_json::json!(false));
    }

    #[test]
    fn is_up_fails_fast_on_closed_port() {
        // Nothing listens here (port 1 on localhost) → probe must return
        // false within the probe timeout, not panic.
        let client = OllamaClient::new("http://127.0.0.1:1", "test");
        assert!(!client.is_up());
    }
}

// -------------------------------------------------------------- sidecar lifecycle

/// Everything that owns the bundled server process, namespaced as `llm::serve`.
pub mod serve {
    use std::path::PathBuf;
    use std::process::{Child, Command, Stdio};
    use std::time::Duration;

    use tauri::{Emitter, Manager};

    use super::{DEFAULT_BASE_URL, DEFAULT_MODEL, OllamaClient};

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

fn sidecar_path() -> Option<PathBuf> {
    sidecar_candidates().into_iter().find(|p| p.exists())
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
        let probe = OllamaClient::new(DEFAULT_BASE_URL, DEFAULT_MODEL);
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
    let models = if tauri::is_dev() {
        dev_models_dir()
    } else {
        app.path()
            .app_data_dir()
            .unwrap_or_else(|_| std::env::temp_dir())
            .join("models")
    };
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
mod serve_tests {
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
}
