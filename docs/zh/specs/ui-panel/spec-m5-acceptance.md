# spec-m5 — 真机验收 + 文档收尾

> 依赖：M0a–M4 全部完成。目标：打包真机回归（含 TCC）+ 文档同步。无新代码（修 bug 除外，bug 修复回写进对应 spec 的回归项）。

## 1. 打包真机（`tauri build`，.app 环境）

按 plan-calendar §11 的经验：**TCC 身份属于 .app，`tauri dev` 跑不了真机验收；ad-hoc 签名每次构建算新 App，权限会丢**。

- [ ] 打包产物启动：sidecar + ollama-runtime 完整进 bundle（验证 externalBin + runtime 镜像路径，参照 45adb9a 的坑）
- [ ] 模型目录：打包后用 app_data_dir/models（非 dev 的 `.llm/models`）——首次启动空目录，验证目录列表接口的 sidecar_down/空列表两态
- [ ] 日志：`~/Library/Logs/<bundle-id>/bonsai.log` 落盘；打开目录按钮指向正确

## 2. 日历 TCC 三态真机走查（plan §2.7 遗留验收）

- [ ] 未授权 → 拨开关弹系统授权 → 授予 → 状态行「已授权」
- [ ] 拒绝 → 状态行显式报错（不是空列表）→「去授权」深链隐私面板
- [ ] 授予后撤销（系统设置关掉）→ 重启 → 状态正确回落

## 3. 全功能回归（520×560 新壳上）

- [ ] 对话：本地/云端两模式流式、危机样本资源卡、未成年门禁（onboarding 重做走未成年分支）、工具意图（「今天有什么会」）标注
- [ ] 成长线：形态四阶段 + 休眠 + 开窗播报幂等；关系线三档 + 生疏 + 顶嘴；预测卡 + 48h 反馈
- [ ] 记忆页与库实况一致；忘记全部 → 重引导
- [ ] 模型：本地卡片切换 / 下载 / 删除 / 重启持久；云端清单 CRUD / opt-in / 删除激活回落
- [ ] 快捷键 ⌘1–5；窗口关闭 → 托盘常驻 → 重开窗口

## 4. 文档收尾

- [ ] `docs/zh/plan-ui-panel.md` 各里程碑状态更新为已实施 + 偏差记录（沿 plan-calendar 惯例：实现与 spec 的偏差逐条列出）
- [ ] `README.md` / `README.zh-CN.md` 功能清单补：模型配置、记忆总览、日志
- [ ] 工作区记忆（.workbuddy/memory）当日记录补实施摘要
- [ ] `design/ui-mockup.html` 状态标注「已实施」或标注与实现的差异点

## 5. 退出标准

全部勾选 + `cargo test` / `pnpm test` 全绿 + 零警告 → plan-ui-panel 整体关闭，roadmap P2 收尾项（语音/屏幕感知/数据可携带）照旧推进。
