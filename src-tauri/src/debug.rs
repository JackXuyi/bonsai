//! Debug logs (spec-m4 §4B): on-disk logger + whitelisted tail reads.
//!
//! Two log sources, both under `app_log_dir`:
//! - `bonsai.log`      — the app's own log crate output (this module inits it)
//! - `llm-sidecar.log` — the bundled Ollama sidecar's stderr (llm/serve.rs)
//!
//! The command layer accepts only the `LogFile` enum — never a raw path —
//! so path traversal is structurally impossible.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::Manager;

/// Rotate threshold: a single log file past 5 MB moves aside once at startup.
pub const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;
/// Tail default / hard cap (spec-m4 §4B table).
pub const TAIL_DEFAULT: u32 = 200;
pub const TAIL_MAX: u32 = 1000;

// ------------------------------------------------------------------ logger

/// Init the process-wide logger at the top of `setup` (the first place a
/// Tauri path resolver exists — everything after this line lands on disk).
/// Debug builds log at Debug level, release at Info (spec-m4 §4B).
pub fn init(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = dir.join("bonsai.log");
    rotate_if_big(&path, MAX_LOG_BYTES).map_err(|e| e.to_string())?;
    let file = std::fs::File::options()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    let level = if cfg!(debug_assertions) {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    };
    simplelog::WriteLogger::init(level, simplelog::Config::default(), file)
        .map_err(|e| e.to_string())
}

/// If `path` is bigger than `max`, rename it to `<path>.1` (single
/// generation — the old `.1` is overwritten; we trade history for zero
/// config, per spec). Missing file / under threshold → no-op.
pub fn rotate_if_big(path: &Path, max: u64) -> std::io::Result<()> {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return Ok(()), // nothing to rotate yet
    };
    if meta.len() <= max {
        return Ok(());
    }
    let mut name = path.as_os_str().to_owned();
    name.push(".1");
    std::fs::rename(path, PathBuf::from(name))
}

// --------------------------------------------------------------- tail read

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogFile {
    AppLog,
    SidecarLog,
}

impl LogFile {
    fn file_name(self) -> &'static str {
        match self {
            LogFile::AppLog => "bonsai.log",
            LogFile::SidecarLog => "llm-sidecar.log",
        }
    }
}

/// Clamp the requested tail size (default 200, hard cap 1000).
pub fn clamp_lines(lines: Option<u32>) -> usize {
    lines.unwrap_or(TAIL_DEFAULT).clamp(1, TAIL_MAX) as usize
}

/// Read the last `n` lines of `path` WITHOUT loading the whole file: walk
/// backwards in 8 KB chunks until enough newlines are seen. Missing file →
/// empty Vec (the frontend shows "暂无日志", an error banner would be noise).
/// Output is chronological (oldest of the tail first).
pub fn tail_lines(path: &Path, n: usize) -> std::io::Result<Vec<String>> {
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let len = f.metadata()?.len();
    const CHUNK: u64 = 8 * 1024;
    let mut buf: Vec<u8> = Vec::new();
    let mut pos = len;
    while buf.iter().filter(|&&b| b == b'\n').count() < n && pos > 0 {
        let start = pos.saturating_sub(CHUNK);
        f.seek(SeekFrom::Start(start))?;
        let take = (pos - start) as usize;
        let mut head = vec![0u8; take];
        f.read_exact(&mut head)?;
        head.extend_from_slice(&buf);
        buf = head;
        pos = start;
    }
    let text = String::from_utf8_lossy(&buf);
    let mut lines: Vec<&str> = text.lines().collect();
    // If we stopped mid-file (pos > 0), the first chunk boundary may have
    // split a line — drop the possibly-truncated head.
    if pos > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    let start = lines.len().saturating_sub(n);
    Ok(lines[start..].iter().map(|s| s.to_string()).collect())
}

// ------------------------------------------------------------- tauri cmds

/// Tail one of the two whitelisted log files. Never Err on a missing file.
#[tauri::command]
pub fn debug_log_tail(
    file: LogFile,
    lines: Option<u32>,
    handle: tauri::AppHandle,
) -> Result<Vec<String>, String> {
    let dir = handle
        .path()
        .app_log_dir()
        .unwrap_or_else(|_| std::env::temp_dir());
    Ok(tail_lines(&dir.join(file.file_name()), clamp_lines(lines)).map_err(|e| e.to_string())?)
}

/// Reveal the log directory in Finder (macOS-first app; same pattern as
/// calendar_open_settings).
#[tauri::command]
pub fn debug_open_log_dir(handle: tauri::AppHandle) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let dir = handle
            .path()
            .app_log_dir()
            .unwrap_or_else(|_| std::env::temp_dir());
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        std::process::Command::new("open")
            .arg(dir)
            .status()
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("unsupported platform".into())
    }
}

// ------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bonsai-debug-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn rotate_moves_big_file_aside() {
        let p = tmp("rot-big.log");
        std::fs::write(&p, vec![b'x'; 100]).unwrap();
        rotate_if_big(&p, 50).unwrap();
        assert!(!p.exists());
        let rotated = tmp("rot-big.log.1");
        assert_eq!(std::fs::metadata(&rotated).unwrap().len(), 100);
        // cleanup
        let _ = std::fs::remove_file(&rotated);
    }

    #[test]
    fn rotate_skips_small_or_missing() {
        let p = tmp("rot-small.log");
        std::fs::write(&p, vec![b'x'; 10]).unwrap();
        rotate_if_big(&p, 50).unwrap();
        assert!(p.exists()); // 未超阈值不动
        assert!(!tmp("rot-small.log.1").exists());

        let missing = tmp("rot-none.log");
        rotate_if_big(&missing, 1).unwrap(); // 缺失文件不报错
        assert!(!missing.exists());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn rotate_overwrites_previous_generation() {
        let p = tmp("rot-over.log");
        let old = tmp("rot-over.log.1");
        std::fs::write(&p, vec![b'a'; 90]).unwrap();
        std::fs::write(&old, vec![b'o'; 10]).unwrap();
        rotate_if_big(&p, 50).unwrap();
        assert_eq!(std::fs::metadata(&old).unwrap().len(), 90); // .1 被新代覆盖
        let _ = std::fs::remove_file(&old);
    }

    #[test]
    fn tail_reads_last_n_lines() {
        let p = tmp("tail.log");
        let mut f = std::fs::File::create(&p).unwrap();
        for i in 0..30 {
            writeln!(f, "line-{i}").unwrap();
        }
        drop(f);
        let got = tail_lines(&p, 3).unwrap();
        assert_eq!(got, vec!["line-27", "line-28", "line-29"]);

        // 请求超过总行数 → 全量返回，不 panic
        let got = tail_lines(&p, 1000).unwrap();
        assert_eq!(got.len(), 30);
        assert_eq!(got[0], "line-0");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn tail_crosses_chunk_boundaries() {
        let p = tmp("tail-chunk.log");
        let mut f = std::fs::File::create(&p).unwrap();
        // 8KB × 2 chunk 边界两侧都要正确拼接
        let padding = "-".repeat(40);
        let mut line_no = 0usize;
        let mut written = 0u64;
        while written < CHUNK0 * 2 + 100 {
            let line = format!("row-{line_no:04} {padding} padding\n");
            written += line.len() as u64;
            write!(f, "{line}").unwrap();
            line_no += 1;
        }
        drop(f);
        let got = tail_lines(&p, 2).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(
            got[1],
            format!("row-{:04} {padding} padding", line_no - 1).trim_end().to_string()
        );
        let _ = std::fs::remove_file(&p);
    }

    const CHUNK0: u64 = 8 * 1024;

    #[test]
    fn tail_missing_file_is_empty_vec() {
        let got = tail_lines(&tmp("tail-absent.log"), 10).unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn clamp_lines_defaults_and_caps() {
        assert_eq!(clamp_lines(None), TAIL_DEFAULT as usize);
        assert_eq!(clamp_lines(Some(0)), 1); // 下限 1
        assert_eq!(clamp_lines(Some(99_999)), TAIL_MAX as usize); // 上限 1000
        assert_eq!(clamp_lines(Some(50)), 50);
    }

    #[test]
    fn log_file_names_are_whitelisted() {
        assert_eq!(LogFile::AppLog.file_name(), "bonsai.log");
        assert_eq!(LogFile::SidecarLog.file_name(), "llm-sidecar.log");
    }
}
