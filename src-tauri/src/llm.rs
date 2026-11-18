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

/// Tauri sidecar file name for the current platform (`ollama-<triple>`).
fn sidecar_name() -> String {
    let arch = match std::env::consts::ARCH {
        "aarch64" => "aarch64",
        "x86_64" => "x86_64",
        other => other,
    };
    format!("ollama-{arch}-apple-darwin")
}

/// The bundled binary sits next to the app executable in both layouts:
/// dev = `target/debug/`, packaged macOS = `Contents/MacOS/`.
fn sidecar_path() -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let path = dir.join(sidecar_name());
    path.exists().then_some(path)
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

/// Spawn the sidecar (best effort) and emit `llm:ready` once healthy.
/// Runs in a background thread: a cold or missing sidecar must never
/// delay startup — the model legs degrade on their own.
pub fn start(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        let Some(path) = sidecar_path() else {
            log::warn!("llm sidecar not bundled; model legs stay on fallbacks");
            return;
        };
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
            return;
        }

        let child = Command::new(&path)
            .env("OLLAMA_MODELS", &models)
            .env("OLLAMA_HOST", format!("127.0.0.1:{PORT}"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let Ok(child) = child else {
            log::warn!("failed to spawn llm sidecar at {}", path.display());
            return;
        };
        let handle = app.state::<ServerHandle>();
        *handle.child.lock().unwrap() = Some(child);

        let probe = OllamaClient::new(DEFAULT_BASE_URL, DEFAULT_MODEL);
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        while std::time::Instant::now() < deadline {
            // Bail early if the server process died (placeholder binary,
            // port clash, crash) instead of burning the whole timeout.
            if let Some(child) = handle.child.lock().unwrap().as_mut() {
                if child.try_wait().map(|s| s.is_some()).unwrap_or(false) {
                    log::warn!("llm sidecar process exited immediately — run 'pnpm llm:sidecar'?");
                    return;
                }
            }
            if probe.is_up() {
                log::info!("llm sidecar ready on :{PORT} (model {})", probe.model());
                let _ = app.emit("llm:ready", probe.model());
                return;
            }
            if handle.child.lock().unwrap().is_none() {
                return; // stopped via ServerHandle::stop
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        log::warn!("llm sidecar did not become healthy within 60s");
    });
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
    fn dev_models_dir_is_project_local() {
        assert!(dev_models_dir().ends_with(".llm/models"));
    }
}
}
