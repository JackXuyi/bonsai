# 方案：文件工具增加删除（plan-files v3）

> **状态**：已实施（2026-11-25）  
> **承接**：[plan-files.md](../../plan-files.md)、[plan-files/v2](../v2/index.md)（v2 仍写「删除不做」；本版对 **单文件删除** 开例外）  
> **一句话**：在授权目录内增加 `fs_delete`（Ask、仅普通文件、不递归、不删目录）。

---

## 1. 为何改口

v1/v2 把删除标成「永远不做」，是为挡住不可逆、尤其是 `rm -rf` 式目录清空。真实诉求是「删掉刚导出的错文件 / 清理授权目录里的单个草稿」，与目录树销毁失败模式不同。

本版只开 **单文件 + Ask**；目录、递归、通配、清空 root **仍不做**。

## 2. 工具表（相对 v2 增量）

| 工具名 | access | 行为 |
| --- | --- | --- |
| `fs_delete` | **Ask** | `path` 过 `resolve_in_roots`；须为已存在**普通文件**；目录 → `fs_is_dir`；缺文件 → `fs_not_found`；越界 → `fs_outside_roots` |

`fs_copy` / `fs_move` / 读写工具不变。权威实现：`src-tauri/src/filetools.rs`。

## 3. 不变量

1. 路径必须在用户授权 roots 内（同一内核）。
2. **只删文件**：`is_dir` / 非普通文件一律拒绝；无 `remove_dir` / `remove_dir_all` 分支。
3. AskFirstGate 强制弹卡（与 write/copy/move 同档；无视全局 auto）。
4. 审计只记路径与动作，不记正文。
5. 本地模式仍零工具调用（架构保证）。

## 4. 非目标（本期仍不做）

- 删除目录 / 递归清空 / 通配批量删
- 回收站 / 软删 / 撤销
- 覆盖写、chmod
- 把删除自动写进日 memory 或 facts

## 5. 验收要点

- [x] `builtin_descriptors` 含 `fs_delete`，access = Ask
- [x] 授权目录内删文件成功；再删同路径 → `fs_not_found`
- [x] 目录路径 → `fs_is_dir`，目录仍在
- [x] 越界路径 → `fs_outside_roots`
- [ ] 真机：Ask 卡拒绝后不落盘；允许后文件消失

## 6. 变更记录

| 版本 | 日期 | 说明 |
|---|---|---|
| v3 | 2026-11-25 | 新增 `fs_delete`；修订 v1/v2「删除永远不做」对单文件的例外 |
