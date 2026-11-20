//! 模型下载（`ollama pull` 子进程 + NDJSON 进度事件，spec-m1 §1.2，拆自
//! `llm_admin.rs`）。
//!
//! 并发上限 1：第二个下载直接被拒（前端也拦，这里是硬闸）。
//! 磁盘不足只预警不拦截（plan §5.6 裁决）。

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde::Serialize;
use tauri::Manager;

use crate::llm::serve;
use crate::llm_models::catalog;

/// 下载子进程 + 并发闸（上限 1）。exit 时随 ServerHandle 一并 kill。
#[derive(Default)]
pub struct PullState {
    child: Mutex<Option<Child>>,
    tag: Mutex<Option<String>>,
    cancelled: AtomicBool,
}

impl PullState {
    pub fn busy(&self) -> bool {
        self.child.lock().map(|g| g.is_some()).unwrap_or(false)
    }

    /// kill 当前 pull（取消 / 应用退出）。幂等。
    pub fn stop(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Ok(mut g) = self.child.lock() {
            if let Some(mut c) = g.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    }
}

/// 进度事件载荷（`llm:pull:progress`，spec §A4 点分命名）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullProgress {
    pub tag: String,
    /// pulling | warn_disk | done | error | cancelled
    pub status: String,
    pub percent: u8,
    pub completed: u64,
    pub total: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `ollama pull` 的一行 NDJSON（纯函数，协议处理可单测）。
#[derive(Debug, PartialEq)]
pub enum PullLine {
    Progress { percent: u8, completed: u64, total: u64 },
    /// manifest / verifying 这类无计数的状态行：保持上一个 percent。
    StatusOnly,
    Done,
    Failed(String),
}

pub fn parse_pull_line(line: &str) -> Option<PullLine> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    #[derive(serde::Deserialize)]
    struct L {
        #[serde(default)]
        status: String,
        #[serde(default)]
        total: u64,
        #[serde(default)]
        completed: u64,
        #[serde(default)]
        error: Option<String>,
    }
    let l: L = serde_json::from_str(line).ok()?; // 垃圾行 → None
    if let Some(err) = l.error.filter(|e| !e.trim().is_empty()) {
        return Some(PullLine::Failed(err));
    }
    if l.status == "success" {
        return Some(PullLine::Done);
    }
    if l.total > 0 {
        let clamped = l.completed.min(l.total);
        let percent = ((clamped.saturating_mul(100)) / l.total) as u8;
        return Some(PullLine::Progress { percent, completed: l.completed, total: l.total });
    }
    if l.status.is_empty() {
        return None;
    }
    Some(PullLine::StatusOnly)
}

/// 目标卷剩余字节（磁盘预警用；拿不到 → None，不拦截下载）。
fn free_bytes(dir: &std::path::Path) -> Option<u64> {
    let out = Command::new("df").arg("-k").arg(dir).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let avail_kb: u64 = text.lines().nth(1)?.split_whitespace().nth(3)?.parse().ok()?;
    Some(avail_kb.saturating_mul(1024))
}

fn emit(app: &tauri::AppHandle, p: PullProgress) {
    use tauri::Emitter;
    let _ = app.emit("llm:pull:progress", p);
}

pub fn start_pull(app: tauri::AppHandle, tag: String) -> Result<(), String> {
    {
        let state = app.state::<PullState>();
        if state.busy() {
            return Err("download_busy".into());
        }
        state.cancelled.store(false, Ordering::SeqCst);
    }
    let bin = serve::sidecar_path().ok_or_else(|| "sidecar_missing".to_string())?;

    // 磁盘预警：不拦截（plan §5.6 裁决），先发一条 warn_disk 让前端提示。
    if let Some(row) = catalog().iter().find(|r| r.tag == tag) {
        if let Some(free) = free_bytes(&serve::models_dir(&app)) {
            let need = (row.size_bytes as f64 * 1.2) as u64;
            if free < need {
                emit(
                    &app,
                    PullProgress {
                        tag: tag.clone(),
                        status: "warn_disk".into(),
                        percent: 0,
                        completed: 0,
                        total: row.size_bytes,
                        error: None,
                    },
                );
            }
        }
    }

    let mut child = Command::new(&bin)
        .arg("pull")
        .arg(&tag)
        .env("OLLAMA_HOST", format!("127.0.0.1:{}", serve::PORT))
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("spawn_failed: {e}"))?;
    let stdout = child.stdout.take().ok_or_else(|| "spawn_failed: no stdout".to_string())?;

    {
        let state = app.state::<PullState>();
        *state.child.lock().map_err(|e| e.to_string())? = Some(child);
        *state.tag.lock().map_err(|e| e.to_string())? = Some(tag.clone());
    }

    let app2 = app.clone();
    std::thread::spawn(move || {
        let mut percent = 0u8;
        let mut completed = 0u64;
        let mut total = 0u64;
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            match parse_pull_line(&line) {
                Some(PullLine::Progress { percent: p, completed: c, total: t }) => {
                    percent = p;
                    completed = c;
                    total = t;
                    emit(
                        &app2,
                        PullProgress {
                            tag: tag.clone(),
                            status: "pulling".into(),
                            percent,
                            completed,
                            total,
                            error: None,
                        },
                    );
                }
                Some(PullLine::Done) => percent = 100,
                Some(PullLine::Failed(e)) => {
                    emit(
                        &app2,
                        PullProgress {
                            tag: tag.clone(),
                            status: "error".into(),
                            percent,
                            completed,
                            total,
                            error: Some(e),
                        },
                    );
                }
                Some(PullLine::StatusOnly) | None => {}
            }
        }

        let state = app2.state::<PullState>();
        let ok = {
            let mut g = state.child.lock().unwrap_or_else(|e| e.into_inner());
            match g.take() {
                Some(mut c) => c.wait().map(|s| s.success()).unwrap_or(false),
                None => false,
            }
        };
        if let Ok(mut t) = state.tag.lock() {
            *t = None;
        }
        let cancelled = state.cancelled.swap(false, Ordering::SeqCst);
        let (status, error) = if cancelled {
            ("cancelled", None)
        } else if ok || percent >= 100 {
            ("done", None)
        } else {
            ("error", Some("pull_failed".to_string()))
        };
        emit(
            &app2,
            PullProgress {
                tag: tag.clone(),
                status: status.into(),
                percent: if status == "done" { 100 } else { percent },
                completed,
                total,
                error,
            },
        );
    });
    Ok(())
}

// ------------------------------------------------------------ 命令层

#[tauri::command]
pub fn llm_model_download(app: tauri::AppHandle, tag: String) -> Result<(), String> {
    start_pull(app, tag)
}

#[tauri::command]
pub fn llm_model_cancel(app: tauri::AppHandle) -> Result<(), String> {
    app.state::<PullState>().stop();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pull_line_progress_computes_percent() {
        let line = r#"{"status":"pulling 8f3","digest":"sha256:x","total":1000,"completed":425}"#;
        assert_eq!(
            parse_pull_line(line),
            Some(PullLine::Progress { percent: 42, completed: 425, total: 1000 })
        );
        // 溢出边界：completed > total 时钳位
        let over = r#"{"status":"x","total":10,"completed":99}"#;
        assert_eq!(
            parse_pull_line(over),
            Some(PullLine::Progress { percent: 100, completed: 99, total: 10 })
        );
    }

    #[test]
    fn pull_line_status_only_has_no_percent() {
        assert_eq!(
            parse_pull_line(r#"{"status":"verifying sha256 digest"}"#),
            Some(PullLine::StatusOnly)
        );
        assert_eq!(parse_pull_line(r#"{"status":"writing manifest"}"#), Some(PullLine::StatusOnly));
    }

    #[test]
    fn pull_line_done() {
        assert_eq!(parse_pull_line(r#"{"status":"success"}"#), Some(PullLine::Done));
    }

    #[test]
    fn pull_line_error() {
        assert_eq!(
            parse_pull_line(r#"{"error":"pull model manifest: file does not exist"}"#),
            Some(PullLine::Failed("pull model manifest: file does not exist".into()))
        );
    }

    #[test]
    fn pull_line_garbage_is_none() {
        assert_eq!(parse_pull_line(""), None);
        assert_eq!(parse_pull_line("   "), None);
        assert_eq!(parse_pull_line("not json at all"), None);
        assert_eq!(parse_pull_line(r#"{"status":""}"#), None);
    }

    #[test]
    fn pull_state_starts_idle() {
        let st = PullState::default();
        assert!(!st.busy());
        st.stop(); // 幂等：没有子进程也不得 panic
        assert!(!st.busy());
    }
}
