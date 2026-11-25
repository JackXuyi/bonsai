# plan-store-lock — 单 Mutex&lt;Connection&gt; 与对话路径锁点收敛

| 文档信息 | 内容 |
|---|---|
| 版本 | v0.3（已实施） |
| 日期 | 2026-11-24 |
| 状态 | **已实施**（T1–T4 采样；巩固分片按 D4 不做）；D1–D5 按推荐闭合 |
| 上游输入 | 优化盘点 P0「MemoryStore 单 Mutex 瓶颈」、`plan-memory.md` §5.4/§10、`plan-retry.md` §5.6（中毒出口）、`plan-loop-supervisor.md`（8 条 loop 齐套）、2026-11-20 lock-narrowing |
| 定位约束 | 本地优先；失败不伪装成功；能力不重复实现；**MUST NOT** 擅自新增依赖；变更最小化；长任务不得持锁 |
| 一句话 | WAL 已经开了，但一把进程内 Mutex 把读写全串死——先收对话热路径锁点，再让只读旁路真正并发 |
| 用户裁决 | **D1–D5 已闭合**（2026-11-24，按推荐）：含 T3；N=2；query_only；V1 不分片；删 `lock_conn` |

---

## 0. 一句话结论

`MemoryStore(Mutex<Connection>)` 是全仓**唯一**的 SQLite 入口：对话、感知、日历、巩固、MCP 审计、8 条后台 loop 全部抢同一把锁。2026-11-20 的 **lock-narrowing** 已经把「持锁跑 LLM / MCP 子进程」这条最致命路径砍掉了，但剩下的问题换成另一种形态：

1. **锁点极多**：对话目录内约 **34** 处 `lock_conn` / `.0.lock`；全仓约 **119** 处；云端一轮工具循环单独再抢 **~8** 次；
2. **WAL 并发被 Mutex 废掉**：文件层允许「多读 + 单写」，进程层却强制全串行——后台 tick 与对话读历史互相堵；
3. **入口不统一**：`dialogue::persist::lock_conn`（中毒 → `internal_poisoned`）与各处裸 `store.0.lock().map_err(...)` 并存；`busy_timeout` **未**在 `db::open` 设置。

本方案按「先可观测、再收点、最后读写分离」推进，**不**引入 `r2d2` / 异步 SQLite / DB actor（桌面单用户过重，且违反「不擅自加依赖」）。

> 关键认知纠偏：问题不是「Mutex 本身错了」，而是 **一把锁同时承担「线程安全」和「SQLite 写串行」两件事**，又把只读查询绑死在同一把上。lock-narrowing 解决的是「持锁做 IO」；本方案解决的是「只读被写路径与后台 tick 无谓阻塞」+「对话路径反复抢锁抖动」。

### 关键取舍（先看这五条；细节进 D1–D5）

| 取舍 | 推荐 | 理由 |
|---|---|---|
| 立刻上连接池 crate？ | **不上** | AGENTS §6；手写 1 writer + N reader 足够；池化抽象收益对单用户桌面接近零 |
| 先改并发模型还是先收对话锁点？ | **先收点（T1–T2），再读写分离（T3）** | 收点零行为风险、立刻降抖动；分离动面大，需不变量与单测齐套 |
| 读写分离后写路径还要不要 Mutex？ | **要** | SQLite 同库同时只能一个写者；进程内 Mutex 仍是写权威，不是文件锁替代品 |
| 后台 loop 要不要合并调度器？ | **本方案不做** | 属「收敛 sleep 轮询」另一题；这里只要求 tick **短持锁**，调度器可另开 plan |
| Mutex 中毒策略？ | **不动** | 继续 `internal_poisoned` + 重启出口（`plan-retry` §5.6）；不做 `into_inner` 自愈 |

---

## 1. 现状锚点（全部实测，带行号）

### 1.1 存储入口

| 现状 | 位置 | 对本方案的约束 |
|---|---|---|
| `MemoryStore(pub Mutex<Connection>)` | `memory/mod.rs:28-33` | 全仓权威；改形状必须保持 Tauri `State` 与测试夹具可编译 |
| `db::open`：`journal_mode=WAL` + `foreign_keys=ON` | `memory/db.rs:73-78` | WAL 已开 → 文件层允许多读；**缺** `busy_timeout` |
| `lock_conn`：短锁约定 + 中毒 → `internal_poisoned` | `dialogue/persist.rs:72-87` | 对话侧单点；其它模块大量绕开 |
| consolidate `lock_store`：读/写两段释放锁跑抽取 | `memory/consolidate.rs:104-136` | 范式正确，必须保留；写阶段仍占 writer |
| `needs_conn`：MCP 执行不持 store 锁 | `agent_tools.rs:117-124`、`mcp_admin.rs:662-667` | lock-narrowing 不变量，禁止回退 |
| schema v17 | `memory/db.rs:58` | 本方案**不升 schema**（无新表）；设置项可用 settings KV |

### 1.2 对话路径锁点（热路径）

一次用户回合在 `chat_turn_inner`（`dialogue/turn.rs`）上的典型抢锁序列：

| 阶段 | 位置 | 次数（约） | 持锁做什么 |
|---|---|---|---|
| 危机命中 | `turn.rs:161` | 1 | 落 user/asst + trace |
| 日历规则 | `turn.rs:196` + `:223` | **2** | 答 + 落库 与 persist 拆开（可合并） |
| 上下文装配 | `turn.rs:254` | 1 | env/system/history/digest 读 + 可能写 digest |
| 总结腿 | `summary_turn.rs:33,91` | 2 | 素材读 + 落库 |
| 云端 tool loop | `cloud_loop.rs` 多处 | **~8** | skill 审计、confirm 设置、开卡、步审计、收尾… |
| 本地 recall/perceive | `local_legs` → `execute_audited` | 1+/工具 | 仅 `needs_conn=true` 时持锁 |
| 落库 + 反谄媚证据 | `persist.rs:39` + `turn.rs:407/449` | **2** | persist 后再锁一次写 disagreement（可合并） |

目录合计（不含 `tests.rs`）：约 **34** 处。模型推理与 MCP 等待**已不持锁**（注释钉在 `turn.rs:22-24`、`persist.rs:72-74`）——这是底线，本方案不得破坏。

### 1.3 全仓争用面

| 消费者 | 周期 / 触发 | 抢锁方式 |
|---|---|---|
| 对话命令 / 流式回合 | 用户 | `lock_conn` + 命令内裸 lock |
| 主动搭话 | 60s | `lock_conn` + 裸 lock |
| perception / face / predict / morph / relationship / calendar / scheduler | poll / 60s / 5s | **几乎全是裸** `store.0.lock()` |
| MCP 审计 / 工具 | 每步 | 混用 |
| Tauri 读命令（history / search / settings…） | UI | 裸 lock，与对话同队列 |

**根因公式**：

```text
WAL 多读能力
  ×  单 Mutex 包住唯一 Connection
  ×  8 条 loop + 对话 + UI 读 同抢
  ────────────────────────────────
  =  只读也被串行；对话路径 N 次 lock 放大尾延迟抖动
```

不是缺 crate，是**并发契约没写完**：lock-narrowing 写了「长 IO 不持锁」，没写「只读可否旁路」和「一回合最多抢几次」。

---

## 2. 能力边界

### 2.1 做

| 能力 | 做到什么程度 |
|---|---|
| 统一加锁入口 | 全仓写路径走 `store.with_write`；只读走 `store.with_read`（T3 后）；废弃业务侧裸 `.0.lock()` |
| 连接 pragma | `db::open` 设 `busy_timeout`（推荐 5000ms）；写连接与读连接同一套 FK/WAL |
| 对话锁点收敛 | 日历「答+落库」、`persist`+disagreement 合并为同一次持锁；cloud_loop 同相位相邻审计合并（见 §5） |
| 持锁预算 | 文档化不变量：持锁禁止 LLM/MCP/sleep/网络；单次临界区目标 &lt; 50ms（巩固 write_phase 例外，须可中断让步——见 D4） |
| 读写分离（T3） | 1× writer `Mutex&lt;Connection&gt;` + N× reader（推荐 N=2，D2）；只读命令与对话装配的纯读段落走 reader |
| 观测 | debug 计数：`lock_wait_ms` / `lock_hold_ms` / `lock_count_per_turn`（settings 或 trace 步，不升 schema） |

### 2.2 明确不做

| 不做 | 原因 |
|---|---|
| 引入 `r2d2` / `deadpool` / `sqlx` | 新依赖；桌面单库用不上池生命周期管理 |
| 异步 SQLite / `tokio-rusqlite` 全仓改造 | 与现有同步 command + `spawn_blocking` 模型冲突；改动面爆炸 |
| DB actor（channel 串行化一切） | 把 Mutex 换成队列，只读仍无法并行；调试成本更高 |
| 合并 8 条后台 scheduler | 另案；本方案只约束其持锁时长 |
| Mutex 中毒 `into_inner` 自愈 | `plan-retry` 已裁决重启出口 |
| 升 schema / 新表 | settings KV + 既有 trace 足够 |
| 改 lock-narrowing 语义（持锁跑模型） | 绝对禁止回退 |

---

## 3. 不变量（违反即回滚）

1. **长 IO 零持锁**：LLM 推理、MCP/子进程、`thread::sleep`、网络 fetch **不得**在 `with_write` / `with_read` 闭包内发生（延续 2026-11-20）。
2. **写权威唯一**：同一时刻至多一个 writer guard；禁止「多连接并发写同一库」。
3. **读可并行（T3 起）**：只读查询不得阻塞在 writer Mutex 上排队，除非正在等 WAL 写完成（由 `busy_timeout` 覆盖）。
4. **中毒可辨识**：写/读锁中毒一律映射为带 `internal_poisoned` 前缀的错误；禁止静默 `into_inner`。
5. **能力不平行**：DB 访问仍只经 `memory/db.rs` 原语；本方案只改**谁持连接**，不复制 SQL。
6. **行为可测**：对话单测与 consolidate 单测在 T1–T3 后仍全绿；读写分离不得改变查询结果与落库顺序语义。
7. **一回合锁次数有上限（T2 验收）**：无工具直答路径 `lock_count ≤ 4`；云端工具循环「每工具步 ≤ 2 次写锁」（审计+状态），禁止每字段一次 lock。

---

## 4. 总体架构

### 4.1 目标形状（T3 后）

```text
MemoryStore
  ├─ write: Mutex<Connection>          // 唯一写者；对话落库 / consolidate 写 / settings 写
  └─ read:  ReadPool { conns: [Connection; N] }  // 只读；history / search / 装配纯读

业务
  store.with_write(|conn| { …短事务… })
  store.with_read(|conn|  { …SELECT… })

WAL 文件层：读者之间并行；读者与写者按 SQLite 规则交错（busy_timeout）
进程层：不再用一把 Mutex 假装「连接池」
```

### 4.2 API 草图（落 `memory/mod.rs`，禁止平行封装）

```rust
pub struct MemoryStore {
    write: Mutex<Connection>,
    /// T1 可先空着；T3 填 N 条只读连接。
    read: ReadPool,
}

impl MemoryStore {
    pub fn open(path: &Path) -> rusqlite::Result<Self> { /* WAL + busy_timeout + FK */ }

    pub fn with_write<T>(
        &self,
        f: impl FnOnce(&mut Connection) -> rusqlite::Result<T>,
    ) -> rusqlite::Result<T> { /* lock write; poison → internal_poisoned */ }

    pub fn with_read<T>(
        &self,
        f: impl FnOnce(&Connection) -> rusqlite::Result<T>,
    ) -> rusqlite::Result<T> { /* T3: 从池借只读连接；T1: 暂降级为 with_write 只读用法 */ }
}
```

- T1：`with_read` **实现上先等同** `with_write`（仍一把锁），但调用点开始分类——为 T3 铺迁移面，避免二次大爆改。
- `dialogue::lock_conn` 改为对 `with_write` 的薄包装或删除，**全仓只留一处 poison 文案**。

### 4.3 对话路径目标锁序（T2）

```text
危机？     → 1× write（落库+trace）→ return
日历命中？ → 1× write（答+双事件+trace）→ return
上下文     → 1× read（纯读）[+ 可选 1× write 仅当 digest 需落盘]
总结？     → read 素材 →（模型无锁）→ 1× write 落库
云端循环   → 开卡/计划 1× write；每工具步 ≤2× write（执行若 needs_conn 另计）；收尾 1× write
直答       →（模型无锁）→ 1× write（persist + disagreement 合并）
```

---

## 5. 对话锁点收敛细则（T2）

| # | 现状 | 改法 | 风险 |
|---|---|---|---|
| C1 | 日历 `turn.rs:196` 与 `:223` 两次锁 | 命中分支内一次持锁完成答、双事件、`rec.persist` | 低；同事务更佳 |
| C2 | `persist_model_turn` 后再锁写 disagreement | 把 disagreement 事件写入打进 `persist_model_turn`（或同一次 `with_write`） | 低；单测已覆盖 filter 事件 |
| C3 | cloud_loop：confirm 读设置与 `task_start` 相邻两次锁（`:493`/`:503`） | 合并为一次 write（读 setting + 开卡 + plan_put） | 低 |
| C4 | cloud_loop：每步审计多次短锁 | 步内「开步 / 审计 / 更新 plan」同一次 write；**仍禁止**把 tool 执行包进锁 | 中；需盯 `needs_conn` 路径 |
| C5 | 命令层裸 `store.0.lock()`（`commands.rs` 6 处） | 改 `with_read` / `with_write` | 低 |

**明确不合并的**：上下文装配中「读完 → 无锁跑模型 → 再写」必须保持两段；巩固抽取同理。

---

## 6. 读写分离细则（T3）

### 6.1 谁走 read

| 走 `with_read` | 示例 |
|---|---|
| UI 列表 / 历史 | `dialogue_history`、`turn_trace_*`、memory search |
| 装配纯读 | `env_prefix` / `system_prompt` / `dialogue_turns_meta` / `session_digest` **读半段** |
| 后台只读 tick | face 读状态、predict 读特征（若该 tick 无写） |
| 设置读取 | `get_setting` 只读 |

### 6.2 谁走 write

| 走 `with_write` | 示例 |
|---|---|
| 任何 INSERT/UPDATE/DELETE | 对话落库、digest 回写、审计、tasks、consolidate write_phase |
| 「读后写」事务 | 日历缓存刷新、开卡+plan_put、settings 读改写 |

### 6.3 Reader 连接约定

- `open` 同样路径；`PRAGMA query_only=ON`（若平台支持）或约定闭包内禁止写（debug assert / 包装类型）。
- N 默认 **2**（D2）：一条给对话装配，一条给 UI/后台只读，避免 UI 翻历史堵住下一轮 context 读。
- 借还用短临界区；禁止长期占用 reader（持着连接跨 await）。

### 6.4 与 busy_timeout

- Writer 持锁时，reader 在 SQLite 层可能 `SQLITE_BUSY` → 连接级 `busy_timeout=5000` 让驱动自旋等待，而不是立刻失败。
- 进程内 writer Mutex 仍避免「多写」；`busy_timeout` 管的是**跨连接**的文件锁，不是替代 Mutex。

---

## 7. 分期

| 期 | 内容 | 门禁 |
|---|---|---|
| **T0** | 本文；D1–D5 闭合 | 文档评审 |
| **T1** | `MemoryStore` API：`with_write` / `with_read`（read 暂降级）；`db::open` 加 `busy_timeout`；全仓裸 lock 迁入口（行为不变） | `cargo test --lib` 全绿；A1–A2 |
| **T2** | 对话锁点收敛 C1–C5；直答路径 `lock_count≤4`；cloud 步内 ≤2 | dialogue 单测全绿；A3–A5 |
| **T3** | 真正 ReadPool（N=D2）；只读命令与装配读段切 `with_read`；写仍 Mutex | A6–A8；巩固与对话交叉压测（手工） |
| **T4**（可选） | 持锁采样进 settings 诊断（`store_lock_stats`）；巩固 write_phase 分片让步（D4 仍不做） | 采样落地；分片按 D4 不做 |

> 顺序理由：T1 把「入口」收束，T2 降对话抖动且不动并发，T3 才吃 WAL 红利。禁止 T1 未完就开 T3。

---

## 8. 验收标准

| # | 场景 | 通过条件 |
|---|---|---|
| A1 | 任意模块 `store.0.lock()` 业务调用 | **零处**（测试夹具除外）；一律 `with_*` |
| A2 | 新开库 | `journal_mode=WAL` 且 `busy_timeout` 为约定值（单测 pragma 可读则断言） |
| A3 | 无工具直答一回合 | 写锁获取次数 ≤ 4；模型期间无 guard 存活 |
| A4 | 日历规则命中 | **一次** write 完成答+落库+trace |
| A5 | 云端单工具步（`needs_conn=false`） | 执行期无 store 锁；步前后写锁合计 ≤ 2 |
| A6 | T3：对话 context 读 + 同时 UI `dialogue_history` | 二者可重叠进行（不在同一 Mutex 队列）；结果一致 |
| A7 | T3：consolidate write_phase 进行中发聊天 | 聊天**读**历史不踩 writer Mutex；落库写仍排队；不卡死 UI 超过 busy_timeout |
| A8 | reader 上误写 | 失败可辨（query_only 或封装拒绝），不得脏写 |
| A9 | 中毒 | 仍返回 `internal_poisoned`；前端重启出口不变 |

---

## 9. 风险矩阵

| 风险 | 概率 | 影响 | 缓解 |
|---|---|---|---|
| 读写分离后「读到未提交写」错觉 | 中 | 中 | 对话装配在 digest **回写后**若需同一快照，回写用 write，随后读走 write 或接受「下一轮才见」并单测钉死 |
| `query_only` 与迁移/临时表不兼容 | 低 | 中 | reader 永不跑 migrate；migrate 只在 open(writer) |
| 全仓替换裸 lock 的大 diff | 高 | 低 | T1 机械替换 + 行为零变；可按目录分 PR（dialogue / memory / loops / 其余） |
| 合并锁导致长临界区回潮 | 中 | 高 | C4 审查清单：合并审计≠合并 tool 执行；CI 单测 + code review 对照 §3.1 |
| busy_timeout 掩盖死锁 | 低 | 中 | 持锁禁止 sleep；超时后仍报错，不无限等 |
| 与 loop-supervisor 心跳抢锁 | 低 | 低 | Registry 不进 SQLite；heartbeat 内存完成（已落地） |

---

## 10. 决策点（全部闭合）

| # | 问题 | 裁决 | 落地约束 |
|---|---|---|---|
| **D1** | 是否含 T3 读写分离 | **含 T3**（2026-11-24） | T1→T2→T3 同方案合入；T4 观测可选 |
| **D2** | Reader 连接数 N | **2**（2026-11-24） | `memory::READER_COUNT = 2` |
| **D3** | Reader 防写 | **`query_only` + 封装**（2026-11-24） | `db::open_reader`；单测 `reader_rejects_writes` |
| **D4** | consolidate 写阶段分片 | **V1 不分片**（2026-11-24） | 抽取仍零持锁；写阶段整段事务 |
| **D5** | `lock_conn` 去留 | **删除**（2026-11-24） | 统一 `write_guard` / `with_write` / `with_read`；中毒文案单点在 `MemoryStore` |

---

## 11. 与上下游的关系

| 文档 / 能力 | 关系 |
|---|---|
| `plan-memory.md` §5.4 / §10 | 原「WAL + Mutex，窗口 &lt;1s」升级为本方案的持锁预算 + 读写分离 |
| `plan-retry.md` §5.6 | 中毒出口不变；`internal_poisoned` 文案继续唯一 |
| `plan-loop-supervisor.md` | loop 存活与本方案正交；tick 内继续短锁，禁止持锁跨 tick sleep |
| `plan-dialogue-split.md` | 文件拆分已完成目录化则锁收敛按新路径改；拆分与锁收敛可并行但**不要同一 PR** |
| `plan-files.md` / `needs_conn` | 继续保证工具执行默认不持锁；T3 后 `needs_conn` 工具走 `with_write` 而非池 |

**不承接**：后台 loop 合并调度、db.rs 按表拆文件、向量检索、跨进程多写。

---

## 12. 实施记录

| 期 | 门禁 | 说明 |
|---|---|---|
| T0 | 文档 | D1–D5 按推荐闭合 |
| T1 | `cargo test --lib` **587 passed** | `MemoryStore` 改为 write Mutex + ReadPool；`busy_timeout=5s`；全仓 `.0.lock` / `lock_conn` / `lock_store` 退役 |
| T2 | 同上 | C1 日历答+落库合并；C2 disagreement 并入 `persist_model_turn`；C3 cloud 计划 confirm+开卡合并 |
| T3 | 同上 + store_lock 单测 | N=2 `query_only` readers；history / search / status / overview 等只读命令走 `with_read` |
| T4 | 采样已做（2026-11-25） | `MemoryStore` 原子计数 + `store_lock_stats` + 诊断卡；consolidate 分片按 D4 不做；未进 turn_trace |

---

## 13. 变更记录

| 版本 | 日期 | 说明 |
|---|---|---|
| v0.4 | 2026-11-25 | T4 写锁采样：`store_lock_stats` + 诊断卡；D4 分片仍不做 |
| v0.3 | 2026-11-24 | T1–T3 落地：读写分离 + 对话锁点收敛；`cargo test --lib` 587 passed；T4 观测未做 |
| v0.2 | 2026-11-24 | 实施中：D1–D5 按推荐闭合 |
| v0.1 | 2026-11-24 | 初稿：实测 ~119 全仓锁点 / 对话 ~34；lock-narrowing 已到位；提议 T1 统一入口+busy_timeout → T2 对话收点 → T3 手写 ReadPool；D1–D5 待裁决 |
