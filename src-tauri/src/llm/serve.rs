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
    let triple = match std::env::consts::OS {
        "macos" => "apple-darwin",
        "windows" => "pc-windows-msvc",
        other => other,
    };
    let name = format!("ollama-{arch}-{triple}");
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name
    }
}

/// Candidate paths for the bundled binary, in lookup order.
///
/// tauri-build strips the target-triple suffix when it copies `externalBin`
/// next to the app executable: dev = `target/debug/ollama`, packaged macOS =
/// `Contents/MacOS/ollama`, packaged Windows = `ollama.exe`. The suffixed name
/// is only present if someone dropped the raw build asset there by hand, so it
/// is just a fallback.
fn sidecar_candidates() -> Vec<PathBuf> {
    let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())) else {
        return Vec::new();
    };
    vec![
        dir.join("ollama"),
        dir.join("ollama.exe"),
        dir.join(sidecar_name()),
    ]
}

/// Runner file name Ollama v0.40+ looks for next to its CLI.
fn runner_name() -> &'static str {
    if cfg!(windows) { "llama-server.exe" } else { "llama-server" }
}

/// Packaged bundles keep the inference runtime under `$RESOURCE`, while the
/// Ollama CLI looks for `llama-server` (and its dylibs/DLLs) next to itself.
/// Stage a copy beside the sidecar once, before the first spawn. Dev builds
/// already get these files from `build.rs`, so they skip this entirely.
fn stage_packaged_runtime(app: &tauri::AppHandle, sidecar_dir: &std::path::Path) {
    if tauri::is_dev() || sidecar_dir.join(runner_name()).exists() {
        return;
    }
    let Ok(resources) = app.path().resource_dir() else {
        log::warn!("cannot resolve resource_dir — packaged local LLM will not start");
        return;
    };
    let src = resources.join("ollama-runtime");
    if !src.is_dir() {
        log::warn!("bundled runtime missing at {} — local LLM will not start", src.display());
        return;
    }
    let Ok(entries) = std::fs::read_dir(&src) else { return };
    for entry in entries.flatten() {
        let from = entry.path();
        let to = sidecar_dir.join(entry.file_name());
        if to.exists() {
            continue;
        }
        let copied = if from.is_dir() {
            copy_dir_recursive(&from, &to)
        } else {
            std::fs::copy(&from, &to).map(|_| ())
        };
        if let Err(e) = copied {
            log::warn!("cannot stage runtime {:?} next to sidecar: {e}", entry.file_name());
        }
    }
}

fn copy_dir_recursive(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let dest = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &dest)?;
        } else {
            std::fs::copy(entry.path(), &dest)?;
        }
    }
    Ok(())
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

    /// 看门狗用的三态（plan-retry §5.6 / D6）：`None` = 没有我们自己的
    /// 子进程（比如复用了孤儿服务）；`Some(true)` = `try_wait()` 确认
    /// 已退出；`Some(false)` = 还在跑。try_wait 出错按「还在跑」算——
    /// 状态不明时宁可不动（不许重启风暴）。
    pub fn child_exited(&self) -> Option<bool> {
        let mut guard = self.child.lock().ok()?;
        Some(
            guard
                .as_mut()?
                .try_wait()
                .map(|status| status.is_some())
                .unwrap_or(false),
        )
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
    // T5 §5.6：启动重试线程也走守卫——panic 记日志，不炸进程。
    crate::failure::spawn_guarded("llm_sidecar_start", move || {
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

/// 运行期重启（plan-retry §5.6 / D6）：看门狗确认进程已退出后调用。
/// 复用 `attempt_start` 的三态循环（与 `start` 同形状）；`llm:restarting`
/// 由调用方（watchdog）先 emit，`llm:ready` 仍由 `attempt_start` 在成功时发。
pub fn restart(app: &tauri::AppHandle) {
    // 先清掉可能残留的子进程句柄（进程已退出时 kill/wait 都是安全空操作）。
    app.state::<ServerHandle>().stop();
    let model = crate::llm_admin::override_model().unwrap_or_else(|| DEFAULT_MODEL.into());
    let probe = OllamaClient::new(DEFAULT_BASE_URL, model);
    for attempt in 1..=3 {
        if attempt > 1 {
            log::warn!("watchdog: retrying llm sidecar restart (attempt {attempt}/3)");
            std::thread::sleep(Duration::from_secs(3));
        }
        match attempt_start(app, &probe) {
            Outcome::Ready | Outcome::Fatal => return,
            Outcome::Retry => continue,
        }
    }
    log::warn!("watchdog: llm sidecar restart failed after 3 attempts; model legs stay on fallbacks");
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
        stage_packaged_runtime(app, dir);
        if !dir.join(runner_name()).exists() {
            log::warn!(
                "{} runner missing next to the sidecar — run 'pnpm llm:sidecar' (chats will fail otherwise)",
                runner_name()
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
        let suffix = if cfg!(windows) { "pc-windows-msvc.exe" } else { "apple-darwin" };
        assert!(name.starts_with("ollama-"));
        assert!(name.ends_with(suffix));
    }

    #[test]
    fn sidecar_lookup_prefers_the_stripped_name_tauri_copies() {
        // tauri-build drops the triple suffix when copying externalBin next to
        // the executable (`ollama` / `ollama.exe`), so plain names must be
        // probed first — this was why the server never started in dev.
        let candidates = sidecar_candidates();
        assert_eq!(candidates.len(), 3);
        assert_eq!(candidates[0].file_name().unwrap().to_str(), Some("ollama"));
        assert_eq!(candidates[1].file_name().unwrap().to_str(), Some("ollama.exe"));
        assert_eq!(
            candidates[2].file_name().unwrap().to_str(),
            Some(sidecar_name().as_str())
        );
    }

    #[test]
    fn dev_models_dir_is_project_local() {
        assert!(dev_models_dir().ends_with(".llm/models"));
    }
}
