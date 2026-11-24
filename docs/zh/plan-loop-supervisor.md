# plan-loop-supervisor — 后台循环 Panic 守卫与存活观测

| 文档信息 | 内容 |
|---|---|
| 版本 | v0.4（已实施） |
| 日期 | 2026-11-24 |
| 状态 | **已实施**（T1–T4）；D1–D5 全部拍板 |
| 上游输入 | `plan-retry.md` §5.6 / T5 / A9（后台循环 panic）、`failure.rs` 已落地的 `spawn_guarded` + `PanicGuard` |
| 定位约束 | 本地优先；失败不伪装成功；能力不重复实现；宁漏勿误；后台静默死亡比偶发 panic 更糟 |
| 一句话 | 后台线程不是「启动后忘掉」——每个 loop 必须**可活、可停、可被看见** |
| 用户裁决 | **D1**：默认关，手动开开关后有界自动重启。**D2**：冷却 = 立即 1 次。**D3**：calendar 计入齐套（8 条）。**D4**：设置页八行全列表。**D5**：`LoopRegistry` 落 `failure.rs` 同文件。均 2026-11-24 |

---

## 0. 一句话结论

`plan-retry` T5 承诺「7 个 `spawn` 统一走 `spawn_guarded`」，基础设施（`PanicGuard` / `spawn_guarded`）已在 `failure.rs:602-663` 落地，但按 D3 的 8 条齐套口径，接线只完成 **4/8**；剩余 4 条（perception / face / proactive / calendar）仍是裸 `std::thread::spawn`，一次 panic = 该能力永久停摆且用户零感知。已接线的 4 条也有缺口：连续 5 次 panic 后线程退出**只打日志**，不写审计、不发事件、不进诊断面——「防风暴」做完了，「可观测」没做。

本方案补三件事，顺序是「先齐套，再留痕，最后按开关复活」：

1. **齐套**：剩余 4 条 loop（含 calendar，D3）接入同一套 `spawn_guarded` + `PanicGuard`（零新机制）——验收按 **8 条**；
2. **留痕**：CAP 停机写 `settings` + `events` + emit 事件，诊断面可查；
3. **有界复活（D1+D2）**：默认关；用户手动打开 `loop_autorestart` 后，CAP **立即**再起 1 次（每 loop / 每进程寿命硬顶），不做跨进程 supervisor。

> 关键认知纠偏：这不是再造一套失败分类（那是 `plan-retry` 的 `Failure`）。本方案只解决**线程生命周期**——`catch_unwind` 保活、CAP 停机留痕、存活状态可查。作业退避（`JobGuard`）与模型重试仍归 `plan-retry`，一行不重写。

### 关键取舍（全部已闭合）

| 取舍 | 结论 | 理由 |
|---|---|---|
| 要不要新建 supervisor 进程 / 看门狗线程 | **不**。复用现有双层守卫；`LoopRegistry` 进 `failure.rs`（D5） | 桌面单进程；跨进程 supervisor 与定位不符 |
| CAP 停机后要不要自动重启 | **D1**：默认 off；设置页手动打开后允许有界自动重启 | 盲目重启掩盖 bug；开关交给用户 |
| 自动重启冷却 | **D2**：立即 1 次（无等待） | 与「每进程 ≤1 次」硬顶联立已够防刷；实测刷屏再抬 1h |
| 齐套几条 | **D3**：含 calendar，共 **8 条** | 同一故障模式；漏守卫 = 会前提醒静默停摆 |
| 设置页展示 | **D4**：八行全列表（running 也展示 `last_beat`） | 「它是不是活着」一眼可判；不只告警 |
| Registry 放哪 | **D5**：`failure.rs` 同文件；超 ~900 行再拆 | 与 `PanicGuard` / `spawn_guarded` 同域，避免平行模块 |
| 审计落哪 | **`settings` KV + `events(kind='system')`**，不建新表、不升 schema | 对齐 `job_backoff:*`；schema 已 v17 |
| 前端要不要独立页面 | **不**。设置页 LogsCard 旁：八行状态 + `loop_autorestart` 开关 | 侧栏五项不动；诊断是管理面 |

---

## 1. 现状锚点（全部实测，带行号）

### 1.1 八条常驻 loop（D3 齐套范围）

| # | 循环 | 周期 | 位置 | 守卫现状 | 停机后果 |
|---|---|---|---|---|---|
| 1 | 夜间巩固 `scheduler` | 60s | `memory/scheduler.rs:94-112` | ✅ `spawn_guarded` + `PanicGuard` | 当晚不整理；已有 `JobGuard` 退避 |
| 2 | 预测-验证 `predict` | 60s | `predict.rs:347-418` | ✅ 双层守卫 | 当日不再猜 |
| 3 | 形态线 `growth` | 60s | `morph.rs:188-219` | ✅ 双层守卫 | 升阶/休眠停更 |
| 4 | 关系线 `relationship` | 60s | `relationship.rs:415-450` | ✅ 双层守卫 | 亲疏停更 |
| 5 | 感知采样 `perception` | ~poll | `perception.rs:339-370` | ❌ 裸 `thread::spawn` | L1 信号冻结 → L4 永久按旧 idle 决策 |
| 6 | 表情态机 `face` | 5s | `face.rs:110-130` | ❌ 裸 spawn | 脸停在最后一帧；`face:state` 不再广播 |
| 7 | 主动搭话 `proactive` | 60s | `dialogue.rs:2235-2248+` | ❌ 裸 spawn | 每日主动开口永久静默 |
| 8 | 日历提醒 `calendar` | poll | `calendar.rs:731-756` | ❌ 裸 spawn | 会前提醒 lane 停摆 |

> **D3 裁决**：`plan-retry.md:57` 旧文写「7 个」不含 calendar；本方案齐套验收按上表 **8 条**（已守卫 4 + 未守卫 4，calendar 与另三条裸奔同等接线）。

### 1.2 已有基础设施（必须复用，禁止平行实现）

| 现状 | 位置 | 对本方案的约束 |
|---|---|---|
| `PanicGuard`：逐 tick `catch_unwind`，连续 5 次 `stopped()` | `failure.rs:607-650` | **不改 CAP=5**；不改 `tick` 签名 |
| `spawn_guarded`：外层兜底，线程名 `guarded:{name}`，panic 记 error | `failure.rs:655-663` | 未守卫的 4 条改走这里；禁止再写裸 `thread::spawn` 常驻 loop |
| 成功 tick 清零连续计数 | `failure.rs:634-636` | 偶发 panic 可自愈——这是正确语义，保留 |
| CAP 后只 `log::error!` + `break` | 四处：`scheduler.rs:103-105`、`predict.rs:413-415`、`morph.rs:214-216`、`relationship.rs:445-447` | **缺口**：无 settings、无 events、无 emit——本方案补留痕 |
| `JobGuard` 作业退避 | `failure.rs:558-599`、`scheduler.rs:136-142` | 与 panic 守卫正交；本方案不动 |
| 一次性：`mcp_reconnect` / `llm_sidecar_start` 已 `spawn_guarded` | `lib.rs:173`、`llm/serve.rs:168` | 不进 Registry |
| 诊断面只有日志文件 | `LogsCard.tsx`、`debug.rs` | **D4**：设置页八行全列表（含 running 的 `last_beat`）+ 开关，不新开路由 |
| schema 已是 v17 | `memory/db.rs:58` | 本方案**不升版本**；只用 settings KV + 既有 `events` |

### 1.3 根因（不是缺 crate，是缺契约）

```text
裸 spawn 的 loop：
  panic → 线程死 → 无日志约定 → 无审计 → 用户以为「今天它不搭理我」

已守卫但 CAP 退出的 loop：
  panic×5 → log::error → break → 线程死 → 同样零审计零 UI
```

两边终点一样：**能力静默消失**。`catch_unwind` 只解决了「偶发 panic 不杀进程」；没解决「系统性坏掉之后谁知道」。

---

## 2. 能力边界

### 2.1 做

| 能力 | 做到什么程度 |
|---|---|
| 齐套接线（D3） | 4 条未守卫 loop（perception / face / proactive / **calendar**）全部改 `spawn_guarded` + 逐 tick `PanicGuard`；验收共 8 条 |
| CAP 留痕 | 停机时写 `settings.loop_status:{name}` + `events(kind='system')` + emit `loop:stopped` |
| 存活注册表（D5） | 进程内 `LoopRegistry` 落 `failure.rs`；启动登记、心跳、停机；命令 `loops_status` 只读 |
| 设置页（D4） | LogsCard 旁：**八行全列表**（running 含 `last_beat`）+ **`loop_autorestart` 手动开关**（D1） |
| 有界自动重启（D1+D2） | 默认 `off`；手动打开后，CAP **立即**再起——每 loop / 每进程寿命 **≤1 次**，无冷却等待 |

### 2.2 明确不做

| 不做 | 原因 |
|---|---|
| 跨进程 supervisor / 独立守护进程 | 桌面单机；复杂度与收益不成比 |
| 改 `PanicGuard::CAP` 或 `tick` 语义 | 已有单测锁死（`failure.rs:679-701`）；本方案只消费 |
| 新建 `loops` / `diagnostics` 表 | settings + events 足够；对齐 plan-retry D8 |
| 把业务 `Err` 当 panic 处理 | `Result` 失败走既有 `log::warn` / `JobGuard`；守卫只拦 unwind |
| 前端「重启循环」按钮进对话流 | 管理动作留在设置页；对话气泡不掺运维 |
| 为测试 panic 引入 `#[should_panic]` 真线程 | 单元测 `PanicGuard` 即可；集成用可控 `tick` 闭包 |

---

## 3. 不变量（违反即回滚）

1. **偶发 panic 不杀线程**：单次 unwind 后下一 tick 必须继续（A9 原义）。
2. **CAP 后必须留痕**：停机路径至少写 settings + 一条 system 事件；禁止只打日志。
3. **不吞掉可观测性**：`log::error!` 仍保留（含 panic 摘要）；留痕是加法不是替代。
4. **自动重启必须经手动开关（D1）**：默认 `loop_autorestart=off`；未开时 CAP 只留痕不复活；打开后每 loop 每进程寿命 ≤1 次。
5. **复活无冷却等待（D2）**：开关开着时 CAP 后**立即**再起；防刷靠「每进程 ≤1 次」硬顶，不靠延时。
6. **守卫零业务逻辑**：`PanicGuard` / Registry 不知道 consolidate / 日历；只知道名字与生死。
7. **禁止平行实现**：不得另写第二套 `catch_unwind` 包装；一律走 `failure::spawn_guarded`；Registry 不另起文件（D5）。
8. **齐套含 calendar（D3）**：新增常驻 loop 必须进 Registry + 双层守卫；漏接 = 不允许合入。
9. **设置页八行全列表（D4）**：running / stopped 都展示；不得只做「有告警才出现」的折叠告警条。

---

## 4. 总体架构

```text
lib.rs setup
  └─ 各模块 spawn(...)
        └─ failure::spawn_guarded(name)          // 外层：线程级兜底
              └─ PanicGuard::new(name)           // 内层：逐 tick
                    └─ loop {
                         sleep(period)
                         registry.heartbeat(name)
                         match guard.tick(|| module_tick(...)) {
                           Some(Ok/Err) => …既有业务…
                           None if stopped => {
                             record_stop(name, reason)  // settings + event + emit
                             maybe_autorestart(name)    // D1：开关 off → no-op；on → 有界再起
                             break
                           }
                         }
                       }
```

### 4.1 `LoopRegistry`（D5：落 `failure.rs`，进程内不持久化存活态）

```rust
// failure.rs —— 与 PanicGuard / spawn_guarded 同文件（D5）
// 若本文件超过 ~900 行再拆 loop_registry.rs；V1 不预拆。
pub struct LoopRegistry {
    // name → LoopHealth { state, last_beat, stopped_at, consecutive_panics, restarts }
}

pub enum LoopState { Running, Stopped }

impl LoopRegistry {
    pub fn register(&self, name: &'static str);
    pub fn heartbeat(&self, name: &str);
    pub fn mark_stopped(&self, name: &str, summary: &str);
    pub fn snapshot(&self) -> Vec<LoopStatusDto>; // 给命令用
}
```

- 在 `lib.rs` `app.manage(LoopRegistry::default())`，各 spawn 闭包里 `try_state` 心跳。
- **持久化**只在停机时写 `settings`（见 §5）；Registry 是运行时投影，重启后从「全 Running」再积累。

### 4.2 停机留痕（单点函数）

```rust
/// CAP 或外层 catch_unwind 退出时调用一次。
fn record_loop_stop(app: &AppHandle, name: &str, detail: &str) {
    // 1) settings: loop_status:{name} = {"state":"stopped","at":ISO,"detail":"…"}
    // 2) events: kind=system, content= JSON {loop, event:"stopped", detail}
    // 3) registry.mark_stopped
    // 4) emit "loop:stopped" { name, detail }
}
```

四个已守卫模块的 `if pg.stopped() { log; break; }` 分支**统一改调**此函数——消除四处复制。

### 4.3 有界自动重启（D1+D2 已裁决）

**契约**：默认关；**仅当用户在设置页手动打开** `loop_autorestart` 后，CAP 停机才允许复活；复活**立即执行**（D2，无冷却等待），靠「每进程寿命每 name ≤1」防刷。

```text
stopped
  → record_loop_stop（无论开关，留痕必做）
  → 若 settings.loop_autorestart != "on"
        → 结束（等用户重启应用；拨开开关不追溯已停 loop）
  → 若 on
        → 且本进程该 name 的 restarts == 0（硬顶）
        → 立即再 spawn_guarded 一次；registry.restarts += 1；emit loop:restarted
        → 否则只留痕，不再起
```

**开关语义（钉死）**：

| 场景 | 行为 |
|---|---|
| 默认 / 用户关着 | CAP → 留痕 + 停；不复活 |
| 用户事先打开 | CAP → 留痕 + **立即**再起 1 次（D2） |
| CAP 之后才打开开关 | **不追溯**：已停的 loop 不因拨开关而复活；下次 CAP（若用户先手动重启应用把 loop 拉起来）才生效 |
| 再起后又 CAP | `restarts` 已是 1 → 只留痕，不再起；用户需重启应用 |

设置页：开关文案建议「后台循环挂掉后自动拉起一次（每个循环本趟最多一次）」——诚实写出有界，不承诺无限续命。

---

## 5. 数据与观测

| 载体 | 键 / 形状 | 用途 |
|---|---|---|
| `settings` | `loop_status:{name}` → `{"state":"stopped","at":"…","detail":"panic:…","restarts":0}` | 重启后仍能看见「上次哪个 loop 挂了」 |
| `settings` | `loop_autorestart` → `on` \| `off`（默认 `off`） | **D1**：设置页手动开关；未开禁止复活 |
| `events` | `kind='system'`，content JSON | 可进记忆页事件流；不进对话 / 不进抽取（system 本就不进 FactExtractor 热路径——若现状会进，加 kind 黑名单，见风险） |
| 事件总线 | `loop:stopped` / `loop:restarted` | 设置页即时刷新；不进聊天气泡 |
| 命令 | `loops_status() -> Vec<LoopStatusDto>` | 合并 Registry 快照 + settings 上次停机 |

**不升 schema。** 不改 `consolidate_runs` / `turn_trace`。

`LoopStatusDto`：

```rust
pub struct LoopStatusDto {
    pub name: String,           // scheduler | predict | growth | …
    pub state: String,          // running | stopped
    pub last_beat: Option<String>, // ISO，仅 running
    pub stopped_at: Option<String>,
    pub detail: Option<String>,
    pub restarts: u32,
}
```

---

## 6. 接线清单（按文件）

| 文件 | 改动 |
|---|---|
| `failure.rs`（D5） | 同文件新增 `LoopRegistry` + `record_loop_stop` / `maybe_autorestart`；单测：登记 / 心跳 / 停机快照 / 立即再起硬顶 |
| `lib.rs` | `manage(LoopRegistry)`；确认 8 条 spawn 均在 setup 内 |
| `perception.rs:339` | 裸 spawn → `spawn_guarded("perception")` + `PanicGuard`；tick 包 `collector` + 写库 |
| `face.rs:110` | 同上，name=`face` |
| `dialogue.rs:2235` | `spawn_proactive` 同上，name=`proactive` |
| `calendar.rs:731` | 同上，name=`calendar` |
| `scheduler` / `predict` / `morph` / `relationship` | CAP 分支改调 `record_loop_stop`；循环内加 `heartbeat` |
| `debug.rs` 或新命令 | `loops_status` / `set_loop_autorestart` |
| `frontend`：`LogsCard` 或旁卡（D4） | **八行全列表**（running 含 last_beat）+ **`loop_autorestart` 开关**；监听 `loop:stopped` / `loop:restarted` |
| `commands.ts` / `types.ts` | `loopsStatus` / `setLoopAutorestart` 类型化封装 |

**模板（未守卫 → 已守卫，以 perception 为例）**：

```rust
pub fn spawn(app: AppHandle, cfg_path: PathBuf) {
    crate::failure::spawn_guarded("perception", move || {
        let mut guard = crate::failure::PanicGuard::new("perception");
        let mut collector = Collector::primed();
        // registry.register("perception");
        loop {
            std::thread::sleep(Duration::from_secs(POLL_INTERVAL_SECS));
            // registry.heartbeat("perception");
            let res = guard.tick(|| { /* 现有 tick 体 */ });
            if guard.stopped() {
                // record_loop_stop(&app, "perception", "consecutive panic cap");
                break;
            }
            let _ = res;
        }
    });
}
```

注意：`Collector` 等非 `UnwindSafe` 状态用 `AssertUnwindSafe` 包在 `PanicGuard::tick` 内部已有先例（`failure.rs:633`）；**不要**把 `Mutex` 守卫跨出 `tick` 闭包，否则毒锁语义变复杂。

---

## 7. 分期

| 期 | 内容 | 门禁 |
|---|---|---|
| **T0** | 文档：D1–D5 全部闭合 | ✅ v0.3 |
| **T1** | 齐套（D3）：4 条裸 loop（含 **calendar**）改双层守卫 | `cargo test --lib` 全绿；A1–A4 |
| **T2** | `LoopRegistry`（D5，`failure.rs`）+ `record_loop_stop`；8 条 CAP 统一留痕 | A5–A6；settings 可查 |
| **T3** | 命令 `loops_status` + 设置页**八行全列表**（D4）+ 事件监听 | A7；`tsc` + vitest |
| **T4** | `loop_autorestart` 开关（D1）+ **立即**有界复活（D2）；默认 off 与 T2 逐字节等价 | A8–A9；开关关着时零复活 |

> 顺序理由：T1 零产品可见、纯可靠性；T2 让「死了能查」；T3 让用户看见（D4 全列表）；T4 按 D1/D2 落地开关与立即有界复活。

---

## 8. 验收标准

| # | 场景 | 通过条件 |
|---|---|---|
| A1 | `perception` tick 内 `panic!` 一次 | 线程存活；下一 poll 正常采样；日志一条 error |
| A2 | `face` 连续 5 次 panic | 线程退出；`settings.loop_status:face` 存在；`events` 有 system 行；emit `loop:stopped` |
| A3 | `proactive` / `calendar` 同 A1 | 同 A1 |
| A4 | 已守卫的 `growth` 偶发一次 panic | 连续计数清零后继续升阶；无 stopped 痕迹 |
| A5 | `scheduler` CAP 停机 | 与 A2 同等留痕；夜间巩固停但聊天仍可用 |
| A6 | 应用重启后 | Registry 全 Running；settings 仍保留上次 stopped 记录（只读历史） |
| A7 | 设置页（D4） | **八行全在**（含 calendar / running）；running 行有 `last_beat`；stopped 行有 detail；`loop_autorestart` 开关默认关 |
| A8 | 开关 **关** 时 CAP | 只留痕 + 停；**不**再起（D1 默认路径） |
| A9 | 开关 **开** 时 CAP（D2） | 同 name **立即**再起 1 次（无 sleep 冷却）；emit `loop:restarted`；第二次 CAP 不再起；`restarts=1`；CAP 之后才拨开开关 → 不追溯已停 loop |

---

## 9. 风险矩阵

| 风险 | 概率 | 影响 | 缓解 |
|---|---|---|---|
| `catch_unwind` 吞掉真实 bug | 中 | 中 | 必打 `log::error` + settings 留痕；设置页可见；不默认 autorestart |
| `system` 事件被 FactExtractor 吃进事实 | 低 | 中 | 抽取入口按 `kind` 黑名单跳过 `system`（若现状未跳，T2 一并加，单测锁） |
| 心跳写 Registry 抢锁拖慢 5s face 循环 | 低 | 低 | Registry 用 `RwLock` 或短临界区；heartbeat 只改时间戳 |
| 用户打开开关后撞上确定性 bug → 立即再崩一次 | 中 | 中 | D1+D2：立即再起但每进程每 name 仅 1 次；第二次 CAP 只留痕；设置页仍显示 stopped |
| `AssertUnwindSafe` 掩盖毒锁 | 中 | 高 | tick 内不跨 await/跨线程持锁；Mutex 中毒走既有 `into_inner` / Internal（plan-retry §5.6） |
| 与 plan-retry T5 文档状态漂移 | 高 | 低 | 本方案落地后回填 `plan-retry` §13 T5 行，标明「齐套见 plan-loop-supervisor」 |

---

## 10. 决策点（全部闭合）

| # | 问题 | 裁决 | 落地约束 |
|---|---|---|---|
| **D1** | CAP 后要不要自动重启 | **手动开启开关后允许有界自动重启**（2026-11-24） | 默认 `off`；设置页显式开关；on 时每 loop / 每进程寿命 ≤1 次；不追溯「先 CAP 后开开关」；见 §4.3 / A8–A9 / T4 |
| **D2** | 自动重启冷却 | **立即 1 次**（2026-11-24，按推荐） | 无 sleep / 无 1h 门；防刷只靠 D1 硬顶；见 §4.3 / A9 |
| **D3** | calendar 是否计入齐套 | **计入**（2026-11-24） | 齐套 8 条；T1 必须改 `calendar.rs:731`；A7 含 `calendar` |
| **D4** | 设置页展示粒度 | **八行全列表**（2026-11-24，按推荐） | running 也展示 `last_beat`；不得只做折叠告警；见 A7 / T3 |
| **D5** | Registry 放哪 | **`failure.rs` 同文件**（2026-11-24，按推荐） | V1 不预拆；本文件超 ~900 行再拆 `loop_registry.rs`；见 §4.1 / T2 |

---

## 11. 与上游的关系

| 上游条款 | 本方案承接 |
|---|---|
| `plan-retry` §5.6「7 个 spawn 统一 spawn_guarded」 | T1 完成齐套；**D3** 扩到 calendar，按 8 条验收 |
| `plan-retry` A9「连续 5 次后停止」 | 保留；补留痕满足「失败必留痕」不变量（plan-retry §3.3） |
| `plan-retry` 风险「吞 panic 更难发现」 | T2/T3 诊断计数与设置页 |
| `plan-retry` T5 实施记录空白 | 落地后回填，并链到本文 |

**不承接**：模型/工具/轮层重试、sidecar 看门狗、DB 自愈——仍属 `plan-retry` T2–T4 / T6。

---

## 12. 实施记录

| 期 | 门禁 | 说明 |
|---|---|---|
| T1 | `cargo test --lib` **580 passed** | perception / face / proactive / calendar 改双层守卫；既有 4 条 CAP 改 `on_loop_cap` |
| T2 | 同上 + registry 单测 | `LoopRegistry` / `record_loop_stop` / `on_loop_cap` 落 `failure.rs`；`kind=system` 抽取黑名单 |
| T3 | `tsc` 0 错；`vitest` LoopsCard + commands 绿 | 命令 `loops_status`；设置页 `LoopsCard` 八行全列表 |
| T4 | 同上 | `loop_autorestart_get/set`；开关默认 off；on 时立即有界再起（每进程每 name ≤1） |

## 13. 变更记录

| 版本 | 日期 | 说明 |
|---|---|---|
| v0.4 | 2026-11-24 | T1–T4 落地：8 条齐套守卫 + Registry 留痕 + 设置页 + 有界自动重启；consolidate 跳过 `system` |
| v0.3 | 2026-11-24 | **D2/D4/D5 按推荐闭合**：冷却=立即 1 次；设置页八行全列表；Registry 落 `failure.rs` |
| v0.2 | 2026-11-24 | **D1 裁决**：默认关，手动开后有界自动重启；T4 升必做；A8/A9。**D3 裁决**：calendar 计入齐套（8 条） |
| v0.1 | 2026-11-24 | 初稿：实况 4/7 已守卫、4 裸奔；复用 PanicGuard；Registry + 留痕 + 可选有界重启；T0–T4 / A1–A8 / D1–D5 |
