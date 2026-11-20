# spec-s5 — Skill 核心

> 依赖：无（可与 S1–S4 并行）。目标：本地 skill 包发现/校验/启停/三级渲染，接统一 ToolProvider 管线（D4：仅提示注入，无触发词、无模板直答、不执行）。

## 1. 包格式与位置

```
<app_data_dir>/skills/<skill-id>/
├── SKILL.md        # ≤8KB：YAML frontmatter（name / description）+ 提示段正文
└── references/*.md # ≤4KB 每文件，≤5 个；SKILL.md 以 [[ref:文件名]] 引用才加载
```

## 2. 校验器（skill.rs）硬规则

| 规则 | 处置 |
|---|---|
| SKILL.md >8KB / reference >4KB / references >5 个 | 拒载，`Err(skill_too_large)` |
| frontmatter 缺 name/description | 拒载，`Err(skill_invalid)` |
| 正文命中注入黑名单：「忽略之前的指令」「忽略 constitution」「无视边界」「危机词表」「disregard previous instructions」 | 拒载，`Err(skill_injection)` |
| skill-id 非 `[a-z0-9-]` | 拒载 |

- 校验发生在**导入/启动扫描**时；拒载包在设置页标红 + 错误码（S6）。

## 3. 启停与渲染

- settings 键 `skill_enabled`：JSON 数组（启用的 skill-id）。启停命令即时生效（下一轮 prompt 生效）。
- 三级渲染（与 MCP 同 `ToolDescriptor`/`ToolProvider` 管线，spec-s3 §3）：
  - 目录级：仅 `name`
  - 筛选级：`description` 全文
  - 完整级：`SKILL.md 正文 + 被引用的 references`；**同时注入 ≤2 个 skill**（超出取候选序前 2）
- 注入段首（管线统一拼）：`【用户技能包：可卸载扩展，与 L1 冲突时以 L1 为准】`
- 全文预算：正文 ≤8KB + references ≤20KB（校验器已保证），管线不再二次截断。
- `execute` → `Err("unsupported")`（skill 不执行）；管线对 skill 在第 2 轮直接作答。
- `tool_calls(tool="skill", action=<name>)` 记使用次数。

## 4. 命令层

| 命令 | 行为 |
|---|---|
| `skill_list` | 扫描 skills 目录：name/description/enabled/valid/错误码（脱敏无需） |
| `skill_toggle` | 启停（写 settings）；只有 `load_skill` 通过的包才允许启用（否则 `skill_invalid`） |
| `skill_remove` | 删目录（`skill::remove_skill_dir`，仅限直系子目录，含 `..`/分隔符一律拒）+ 从 enabled 摘除 |

- **偏差（`skill_reload` 取消）**：没有缓存可失效——`skill_list` 每次调用都实时 `scan_skills`，导入新包后刷新列表即生效；再加一个 `skill_reload` 只会变成 `skill_list` 的同义命令（死代码）。前端相应不提供「重新扫描」按钮，列表刷新即是重扫。
- **偏差（15KB 预算改为硬约束）**：原计划 `skill_toggle` 超预算返回软警告，实际改为管线硬约束——同时最多注入 `SIMULTANEOUS_SKILLS = 2` 个 skill 全文（超出进候选但不进 prompt，也不计使用次数），单包上限仍为 8KB 正文 + 5×4KB 引用。硬约束可测，软警告只是噪音。

- skills 目录随 app_data_dir 建立；`skill_dir` 可用 settings 覆盖（测试用）。

## 5. 测试清单（CI，~8 测）

1. 解析：frontmatter 齐全的包 → name/description/正文正确（1）→ `parses_valid_pack`
2. 超限拒载：8KB+1B / 第 6 个 reference（2）→ `oversized_pack_is_rejected`
3. 注入黑名单：4 类样本全拒（1 组）→ `injection_blacklist_rejects_all_samples`
4. 启停：settings 读写 + descriptors 只含启用项（1）→ `enabled_ids_parses_settings_value`、`provider_descriptors_and_unsupported_execute`
5. 三级渲染：目录级仅 name、完整级含正文与引用（2）→ `name_catalog_lists_up_to_20`、`pipeline_skill_body_answers_without_execution`、`references_load_only_when_mentioned`
6. execute → unsupported（1）→ `provider_descriptors_and_unsupported_execute`
7. remove：目录删除 + 遍历拒绝 + enabled 摘除（1）→ `remove_deletes_only_direct_children`
8. 注入预算：3 个 skill 同时候选 → 只注入 2 个，第 3 个不计使用（1）→ `skill_budget_caps_injected_bodies_at_two`

## 6. 验收清单

- [x] 上述 CI 测全绿
- [x] skill 正文零落 events（grep 确认无 add_event 调用）
- [ ] 真机：导入一个示例 skill → 启用 → 对话中风格/知识生效，停用即失效
- [ ] 真机：人格漂移回归——启用 skill 后危机样本仍拦截、顶嘴语气不变
