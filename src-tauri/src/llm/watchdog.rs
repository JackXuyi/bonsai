//! sidecar 运行期看门狗（plan-retry §5.6 / D6=A / §4.2「sidecar 重启」行）。
//!
//! 现状缺口：`serve::start` 只管启动，进程崩了没人发现。本模块补运行期：
//! 每次本地模型请求（成功或失败）都从这里过一眼——失败后先 probe 探活，
//! **连续 2 次** down 且进程 `try_wait()` 确认已退出，才走 `serve::restart`
//! 重启路径（复用 `attempt_start`），**每小时至多 1 次**；重启期间向前端
//! emit `llm:restarting`（前端 T7 在气泡上写「本地模型在重启」）。
//!
//! 两条硬规矩（D6）：
//! - `try_wait()` 没确认退出就不重启——避免与「端口被别的服务占着」混淆；
//! - 每小时至多 1 次——看门狗不许变成重启风暴。
//!
//! 判定逻辑全部抽成纯结构体 [`RestartGate`]（时间以调用方传入的毫秒数
//! 表示，测试不用 sleep 就能锁死每个分支）；接线部分只有 `init` /
//! `record_request` 两个入口，不侵入 persona/eval.rs（那文件另一条线在改）。

use std::sync::{Mutex, OnceLock};
use std::time::UNIX_EPOCH;

use tauri::{Emitter, Manager};

use super::serve::{self, ServerHandle};
use super::{OllamaClient, DEFAULT_BASE_URL};

/// 连续多少次「请求失败 + 探活 down」才判定 sidecar 真死了（§5.6：连续 2 次）。
pub const CONSECUTIVE_DOWNS_TO_RESTART: u32 = 2;
/// 每小时至多重启 1 次（§4.2 sidecar 重启行：上限 1 次/小时）。
pub const RESTART_INTERVAL_MS: u64 = 60 * 60 * 1000;

/// 重启判定结果（供日志与单测断言）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// 请求成功或探活正常：连续计数清零。
    Healthy,
    /// 第 n 次连续 down，未达阈值，继续观察。
    Watching(u32),
    /// `try_wait()` 未能确认进程退出（还活着 / 无子进程可确认）：不重启（D6）。
    ProcessAlive,
    /// 一小时内已重启过：限流。
    RateLimited,
    /// 通过全部门控：可以重启（gate 内部已记账：清零计数、记录本次时刻）。
    RestartNow,
}

/// 纯判定结构。运行期只有一把 Mutex 包着它；探活、try_wait、emit、重启
/// 都在接线层（见 `record_request`），这里不碰任何 IO。
pub struct RestartGate {
    consecutive_downs: u32,
    last_restart_ms: Option<u64>,
}

impl Default for RestartGate {
    fn default() -> Self {
        Self::new()
    }
}

impl RestartGate {
    pub fn new() -> Self {
        RestartGate { consecutive_downs: 0, last_restart_ms: None }
    }

    /// 喂一次模型请求的结果。
    ///
    /// - `ok`：请求本身是否成功（成功就无需探活，直接清零）；
    /// - `probe_up`：失败后的 `/api/version` 探活结果；
    /// - `process_exited`：`try_wait()` 的三态——`Some(true)` 确认退出、
    ///   `Some(false)` 还在跑、`None` 没有我们自己的子进程可确认（比如
    ///   复用了孤儿服务，D6 规定这种情况同样不许重启）；
    /// - `now_ms`：任意纪元的毫秒数，只用于「每小时至多 1 次」的窗口比较。
    pub fn on_request(
        &mut self,
        ok: bool,
        probe_up: bool,
        process_exited: Option<bool>,
        now_ms: u64,
    ) -> Verdict {
        if ok || probe_up {
            self.consecutive_downs = 0;
            return Verdict::Healthy;
        }
        self.consecutive_downs = self.consecutive_downs.saturating_add(1);
        if self.consecutive_downs < CONSECUTIVE_DOWNS_TO_RESTART {
            return Verdict::Watching(self.consecutive_downs);
        }
        // D6：只在进程确认已退出时重启。没退出的「down」可能是端口被别的
        // 服务占着（probe 打不到 ollama 协议上），重启只会撞墙。
        if process_exited != Some(true) {
            return Verdict::ProcessAlive;
        }
        if let Some(t) = self.last_restart_ms {
            if now_ms.saturating_sub(t) < RESTART_INTERVAL_MS {
                return Verdict::RateLimited;
            }
        }
        self.last_restart_ms = Some(now_ms);
        self.consecutive_downs = 0;
        Verdict::RestartNow
    }
}

// ------------------------------------------------------------------ 接线

struct WatchdogRuntime {
    app: tauri::AppHandle,
    gate: Mutex<RestartGate>,
}

static RUNTIME: OnceLock<WatchdogRuntime> = OnceLock::new();

/// 启动时挂上 AppHandle（lib.rs setup 调一次）。没 init 的进程（如单测）
/// `record_request` 是零开销 no-op。
pub fn init(app: tauri::AppHandle) {
    let _ = RUNTIME.set(WatchdogRuntime { app, gate: Mutex::new(RestartGate::new()) });
}

/// ollama 传输的每次 chat / chat_stream（成功或失败）都从这里过一眼（§5.6）。
/// 失败时同步做一次 2s 探活 + try_wait 门控判定；只有判定为重启时才开线程，
/// 60s 级的重启流程绝不阻塞已经失败的那次请求返回。
pub fn record_request(ok: bool) {
    let Some(rt) = RUNTIME.get() else { return };
    // 探活用固定端口（与 serve 一致，OLLAMA_HOST 不许劫持看门狗的视线）。
    let probe_up = if ok {
        true
    } else {
        OllamaClient::new(DEFAULT_BASE_URL, "").is_up()
    };
    let process_exited = rt.app.state::<ServerHandle>().child_exited();
    let now_ms = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let verdict = match rt.gate.lock() {
        Ok(mut gate) => gate.on_request(ok, probe_up, process_exited, now_ms),
        Err(_) => {
            // 中毒不恢复、不重启：宁可停在「不自动动作」，也不在状态可疑时
            // 去杀进程（§3 不变量 5：动作必须有据）。
            log::warn!("watchdog: gate mutex poisoned — 观察中止，不再自动重启");
            return;
        }
    };
    match verdict {
        Verdict::Healthy | Verdict::Watching(_) | Verdict::ProcessAlive | Verdict::RateLimited => {
            log::debug!("watchdog: {verdict:?}");
        }
        Verdict::RestartNow => spawn_restart(rt.app.clone()),
    }
}

/// 重启在独立线程做：`attempt_start` 最长要等 60s 健康检查，绝不能压在
/// 模型请求的调用线程上。`llm:restarting` 先发，`llm:ready` 由
/// `attempt_start` 在成功时发（前端两个事件都认）。
fn spawn_restart(app: tauri::AppHandle) {
    log::warn!(
        "watchdog: sidecar 连续 {CONSECUTIVE_DOWNS_TO_RESTART} 次探活失败且进程已退出 — 自动重启（每小时至多 1 次）"
    );
    std::thread::spawn(move || {
        let _ = app.emit("llm:restarting", ());
        serve::restart(&app);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXITED: Option<bool> = Some(true);
    const RUNNING: Option<bool> = Some(false);
    const NO_CHILD: Option<bool> = None;

    #[test]
    fn needs_two_consecutive_down_probes_before_restarting() {
        // §5.6：连续 2 次 down 才动，第一次只是观察。
        let mut g = RestartGate::new();
        assert_eq!(g.on_request(false, false, EXITED, 1_000), Verdict::Watching(1));
        assert_eq!(g.on_request(false, false, EXITED, 2_000), Verdict::RestartNow);
    }

    #[test]
    fn success_or_live_probe_resets_the_streak() {
        // 抖动（一次失败后服务其实活着）不许积累成「连续 down」。
        let mut g = RestartGate::new();
        assert_eq!(g.on_request(false, false, EXITED, 0), Verdict::Watching(1));
        assert_eq!(g.on_request(true, true, RUNNING, 1_000), Verdict::Healthy);
        assert_eq!(
            g.on_request(false, false, EXITED, 2_000),
            Verdict::Watching(1),
            "连续被打断必须重新数"
        );
    }

    #[test]
    fn restart_requires_try_wait_confirmation_d6() {
        // D6：进程没退出就不重启（避免与「端口被别的服务占着」混淆）。
        let mut g = RestartGate::new();
        assert_eq!(g.on_request(false, false, RUNNING, 0), Verdict::Watching(1));
        assert_eq!(g.on_request(false, false, RUNNING, 1_000), Verdict::ProcessAlive);
        // 复用的孤儿服务没有我们的子进程可 try_wait，同样不确认不重启。
        assert_eq!(g.on_request(false, false, NO_CHILD, 2_000), Verdict::ProcessAlive);
        // 之后进程真的退出（或被确认了），下一次失败才放行。
        assert_eq!(g.on_request(false, false, EXITED, 3_000), Verdict::RestartNow);
    }

    #[test]
    fn restarts_at_most_once_per_hour() {
        // §4.2：每小时至多 1 次。窗口边界：整 1 小时放行，差 1ms 都算限流。
        // 注意重启后「连续 down」清零，限流判定发生在再次攒满 2 次 down 时。
        let mut g = RestartGate::new();
        assert_eq!(g.on_request(false, false, EXITED, 0), Verdict::Watching(1));
        assert_eq!(g.on_request(false, false, EXITED, 1_000), Verdict::RestartNow);
        assert_eq!(g.on_request(false, false, EXITED, 60_000), Verdict::Watching(1));
        assert_eq!(g.on_request(false, false, EXITED, 61_000), Verdict::RateLimited);
        assert_eq!(
            g.on_request(false, false, EXITED, 1_000 + RESTART_INTERVAL_MS - 1),
            Verdict::RateLimited
        );
        assert_eq!(g.on_request(false, false, EXITED, 1_000 + RESTART_INTERVAL_MS), Verdict::RestartNow);
    }
}
