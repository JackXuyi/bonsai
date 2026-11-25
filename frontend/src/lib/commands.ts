// 全部 Tauri 命令的类型化封装（签名对照 src-tauri/src/lib.rs generate_handler，
// payload 字段以 ui/main.js 实际消费 + Rust serde snake_case 为准）。
// 一律经 invokeWithTimeout（15s 超时兜底）；浏览器预览时返回兜底值而不是抛错。
import { invokeWithTimeout, inTauri } from "./tauriInvoke";
import type {
  AbilityStatus,
  Boundary,
  CalendarStatus,
  CatalogRow,
  CloudEntryInput,
  CloudEntryView,
  CloudTestReport,
  ConsolidateResult,
  DialogueBubble,
  DiscoveredServer,
  FsStatus,
  GrowthStatus,
  LlmConfigView,
  McpSaveInput,
  McpServerRow,
  McpImportReport,
  OllamaModelRow,
  OnboardingAnswer,
  OnboardingStatus,
  LogFile,
  MemoryOverview,
  OnboardingSubmitResult,
  PersonaSnapshot,
  PlanTaskRow,
  ResumePrepareResult,
  JobRow,
  PerceptionConfig,
  PerceptionStatus,
  ProfileEntry,
  RelationshipStatus,
  SessionRow,
  SkillCreateInput,
  SkillDetail,
  SkillAutoCandidate,
  SkillView,
  ToolCallRecord,
  ToolDescriptor,
  ProactiveReply,
  TurnTrace,
  WebStatus,
  MailStatus,
  DayTimelineExport,
  FaceSkinManifest,
  FailureRecent,
  LoopStatus,
} from "./types";

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  return invokeWithTimeout<T>(cmd, args);
}

// ---------- face ----------
export const faceCurrent = () => call<string>("face_current");
export const facePoke = () => call<void>("face_poke");
export const facePreview = (state: string) =>
  call<void>("face_preview", { state });

// ---------- growth / relationship ----------
export const growthStatus = () => call<GrowthStatus>("growth_status");
export const growthAck = (id: number) => call<void>("growth_ack", { id });
export const relationshipStatus = () =>
  call<RelationshipStatus>("relationship_status");
export const relationshipAck = (id: number) =>
  call<void>("relationship_ack", { id });
export const relationshipReset = () => call<void>("relationship_reset");
export const abilityStatus = () => call<AbilityStatus>("ability_status");
export const abilityAck = (id: number) => call<void>("ability_ack", { id });
export const abilityReset = () => call<void>("ability_reset");

// ---------- dialogue ----------
/** 一轮对话是最长的 IPC，命令本身 **fire-and-forget**（立即返回受理结果），
 *  正文与终态都走 Channel：delta* → turn_done / turn_error（P1，2026-11-23）。
 *  保留 10 分钟兜底只防 IPC 真挂死——业务慢由前端的「事件静默」看门狗判。 */
const DIALOGUE_IPC_TIMEOUT_MS = 600000;
export const dialogueChatStream = (
  userText: string,
  sessionId: string,
  onDelta: unknown
) =>
  invokeWithTimeout<void>(
    "dialogue_chat_stream",
    { userText, sessionId, onDelta },
    DIALOGUE_IPC_TIMEOUT_MS
  );
/** 中止当前回合（后端 T4 的取消标志）：超时/用户点「停」时前后端对齐。 */
export const dialogueCancel = (taskId: string) =>
  call<void>("dialogue_cancel", { taskId });

/** 对话页启动回放 + 向上分页：最近消息流平铺气泡（旧→新）；
 *  beforeId 为滚动加载游标（只取更早的行）。 */
export const dialogueHistory = (sessionId: string, limit?: number, beforeId?: number) =>
  call<DialogueBubble[]>("dialogue_history", { sessionId, limit, beforeId });
export const dialogueProactiveNow = () => call<ProactiveReply>("dialogue_proactive_now");

// ---------- 会话（session.rs，plan-context.md §6.7）----------
/** 会话索引。`includeArchived` 缺省 false：抽屉默认只看活的，
 *  归档区要单独拉一次（或直接一次全拉回来自己分组）。 */
export const sessionList = (includeArchived?: boolean) =>
  call<SessionRow[]>("session_list", { includeArchived });
export const sessionCreate = (title?: string) =>
  call<SessionRow>("session_create", { title });
export const sessionRename = (id: string, title: string) =>
  call<void>("session_rename", { id, title });
export const sessionArchive = (id: string, archived: boolean) =>
  call<void>("session_archive", { id, archived });
/** 删除 = 合规意义上的「忘掉这段」：连带清 trace / 任务。返回删掉的事件条数。 */
export const sessionDelete = (id: string) => call<number>("session_delete", { id });

// ---------- 对话链路追踪（dialogue.rs，spec-s8）----------
/** 单回合完整生成链路；None = 追踪上线前的旧消息（前端空态）。 */
export const turnTraceGet = (turnId: number) =>
  call<TurnTrace | null>("turn_trace_get", { turnId });
/** 按对话事件 id 反查 trace turn id（气泡只带 eventId 时的详情入口）。 */
export const turnTraceFindByEvent = (eventId: number) =>
  call<number | null>("turn_trace_find_by_event", { eventId });

// ---------- prediction ----------
export const predictionFeedback = (id: number, verdict: string) =>
  call<boolean>("prediction_feedback", { id, verdict });

// ---------- perception ----------
export const perceptionGetConfig = () =>
  call<PerceptionConfig>("perception_get_config");
export const perceptionSetConfig = (cfg: PerceptionConfig) =>
  call<void>("perception_set_config", { cfg });
export const perceptionStatus = () => call<PerceptionStatus>("perception_status");

// ---------- calendar ----------
export const calendarStatus = () => call<CalendarStatus>("calendar_status");
export const calendarRequestAccess = () =>
  call<CalendarStatus>("calendar_request_access");
export const calendarSetConfig = (opts: { enabled?: boolean; remind?: boolean }) =>
  call<CalendarStatus>("calendar_set_config", opts);
export const calendarClearCache = () =>
  call<CalendarStatus>("calendar_clear_cache");
export const calendarMuteToday = () => call<void>("calendar_mute_today");
export const calendarOpenSettings = () => call<void>("calendar_open_settings");

// ---------- webtools（P3-3，plan-web.md §3.7） ----------
export const webStatus = () => call<WebStatus>("web_status");
export const webSetConfig = (opts: { enabled?: boolean }) =>
  call<WebStatus>("web_set_config", opts);

// ---------- mail（P3-1，plan-mail/v1） ----------
export const mailStatus = () => call<MailStatus>("mail_status");
export const mailSetEnabled = (enabled: boolean) =>
  call<MailStatus>("mail_set_enabled", { enabled });
export const mailProbe = () =>
  call<{ ok: boolean; count: number }>("mail_probe");

// ---------- observability（P4-3 / G-18） ----------
export const observabilityDayExport = (day?: string) =>
  call<DayTimelineExport>("observability_day_export", day ? { day } : {});

// ---------- filetools（P3-2，plan-files.md §3.6） ----------
export const fsStatus = () => call<FsStatus>("fs_status");
export const fsSetConfig = (opts: { enabled?: boolean; cloud_read?: boolean }) =>
  call<FsStatus>("fs_set_config", opts);
export const fsRootAdd = (path: string) =>
  call<FsStatus>("fs_root_add", { path });
export const fsRootRemove = (path: string) =>
  call<FsStatus>("fs_root_remove", { path });
/** 系统目录选择器；null = 用户取消。拿到路径后必须再走 fsRootAdd 校验。 */
export const fsPickDirectory = () =>
  call<string | null>("fs_pick_directory");

// ---------- faceskins（表情自定义皮肤，plan-face-custom-skins.md §5.1） ----------
export const faceSkinList = () => call<FaceSkinManifest[]>("face_skin_list");
export const faceSkinCreate = (name: string) =>
  call<FaceSkinManifest>("face_skin_create", { name });
/** 系统文件选择器 + 校验 + 落盘，Rust 全权处理（字节不过 IPC）。 */
export const faceSkinPickAndImport = (skinId: string, slot: string) =>
  call<FaceSkinManifest>("face_skin_pick_and_import", { skinId, slot });
export const faceSkinClearSlot = (skinId: string, slot: string) =>
  call<FaceSkinManifest>("face_skin_clear_slot", { skinId, slot });
export const faceSkinDelete = (skinId: string) =>
  call<void>("face_skin_delete", { skinId });

// ---------- onboarding / boundaries ----------
export const onboardingStatus = () => call<OnboardingStatus>("onboarding_status");
export const onboardingSubmit = (payload: {
  ageGroup: string | null;
  crisisAck: boolean;
  answers: OnboardingAnswer[];
}) => call<OnboardingSubmitResult>("onboarding_submit", { payload });
export const onboardingReset = () => call<void>("onboarding_reset");
export const boundaryList = () => call<Boundary[]>("boundary_list");
export const boundaryDelete = (id: number) => call<void>("boundary_delete", { id });

// ---------- user profile（memory/profile.rs，plan-user-profile.md §6.3） ----------
/** UI 保存一律落 manual——从界面进来的都是用户亲口定的。 */
export const profileList = () => call<ProfileEntry[]>("profile_list");
export const profileSet = (key: string, value: string) =>
  call<boolean>("profile_set", { key, value });
export const profileDelete = (key: string) =>
  call<number>("profile_delete", { key });

// ---------- memory ----------
export const memoryConsolidate = () =>
  call<ConsolidateResult>("consolidate");
export const memoryForgetSession = (sessionId?: string) =>
  call<void>("forget_session", { sessionId });
export const memoryForgetAll = () => call<void>("forget_all");
export const memoryAddEvent = (kind: string, text: string) =>
  call<void>("add_event", { kind, text });
export const memoryAddFact = (key: string, value: string) =>
  call<void>("add_fact", { key, value });
export const memorySearch = (query: string, limit?: number) =>
  call<unknown[]>("search", { query, limit });
/** 记忆页总览（spec-m3）：只读快照，limit 缺省 20、后端上限 100。 */
export const memoryOverview = (limit?: number) =>
  call<MemoryOverview>("memory_overview", { limit });

// ---------- llm（llm_admin.rs，spec-m1）----------
export const llmConfigGet = () => call<LlmConfigView>("llm_config_get");
/** 切换模型含 1-token 探测（冷加载可达数十秒），超时放宽到 90s。 */
export const llmConfigSetActive = (id: string) =>
  invokeWithTimeout<LlmConfigView>("llm_config_set_active", { id }, 90000);
/** 后台任务只用本地模型（Auto / Manual，plan-structured-routing）。 */
export const llmRoutingSet = (backgroundLocalOnly: boolean) =>
  call<LlmConfigView>("llm_routing_set", { backgroundLocalOnly });
export const llmModelListInstalled = () =>
  invokeWithTimeout<OllamaModelRow[]>("llm_model_list_installed", undefined, 8000);
export const llmCatalog = () => call<CatalogRow[]>("llm_catalog");
export const llmModelDownload = (tag: string) =>
  call<void>("llm_model_download", { tag });
export const llmModelCancel = () => call<void>("llm_model_cancel");
export const llmModelRemove = (tag: string) =>
  call<void>("llm_model_remove", { tag });

// ---------- 云端模型（llm_cloud.rs，spec-m2）----------
export const llmCloudEntriesGet = () =>
  call<CloudEntryView[]>("llm_cloud_entries_get");
export const llmCloudEntrySave = (entry: CloudEntryInput) =>
  call<CloudEntryView>("llm_cloud_entry_save", { entry });
export const llmCloudEntryRemove = (id: string) =>
  call<void>("llm_cloud_entry_remove", { id });
/** 一枪测试要真跑一次 1-token 请求，超时放宽到 45s（慢服务商）。 */
export const llmCloudTest = (entry: CloudEntryInput) =>
  invokeWithTimeout<CloudTestReport>("llm_cloud_test", { entry }, 45000);

// ---------- misc ----------
export const getPersona = () => call<Record<string, unknown>>("get_persona");
/** 更新人格显示名（plan-presence-im v4）；Rust 校验 1–12 字。 */
export const setPersonaName = (name: string) =>
  call<Record<string, unknown>>("set_persona_name", { name });
export const scaffoldReply = (args: Record<string, unknown>) =>
  call<string>("scaffold_reply", args);
export const evaluate = (args: Record<string, unknown>) =>
  call<unknown>("evaluate", args);
export const predictionStats = () => call<unknown>("prediction_stats");
export const dailyReport = () => call<string>("daily_report");
/** 本周总结（plan-summary-skill）：与对话里说「这周总结一下」同一套素材引擎。 */
export const weeklyReport = () => call<string>("weekly_report");

// ---------- 人格快照 / debug 日志（spec-m4）----------
export const personaSnapshot = () => call<PersonaSnapshot>("persona_snapshot");
export const debugLogTail = (file: LogFile, lines?: number) =>
  call<string[]>("debug_log_tail", { file, lines });
export const debugOpenLogDir = () => call<void>("debug_open_log_dir");

// ---------- background loops（plan-loop-supervisor）----------
export const loopsStatus = () => call<LoopStatus[]>("loops_status");
export const loopAutorestartGet = () => call<boolean>("loop_autorestart_get");
export const loopAutorestartSet = (enabled: boolean) =>
  call<boolean>("loop_autorestart_set", { enabled });

/** 设置页诊断：最近失败（turn + consolidate），默认 10 条。 */
export const failuresRecent = (limit?: number) =>
  call<FailureRecent[]>("failures_recent", { limit });

// ---------- agent 工具管线（mcp_admin.rs，spec-s2/s4/s5）----------
export const mcpServerList = () => call<McpServerRow[]>("mcp_server_list");
export const mcpServerSave = (input: McpSaveInput) =>
  call<string>("mcp_server_save", input as unknown as Record<string, unknown>);
export const mcpServerRemove = (id: string) =>
  call<void>("mcp_server_remove", { id });
export const mcpServerToggle = (id: string, enable: boolean) =>
  invokeWithTimeout<void>("mcp_server_toggle", { id, enable }, 30000);
/** 连接测试 = 真 spawn + tools/list，超时放宽（stdio 冷启动可能十几秒）。 */
export const mcpServerTest = (id: string) =>
  invokeWithTimeout<number>("mcp_server_test", { id }, 30000);
export const mcpServerTools = (id: string) =>
  call<ToolDescriptor[]>("mcp_server_tools", { id });
/** 内置工具清单（随版本发布，不走 MCP 连接，settings 页「内置工具」模块用）。 */
export const builtinToolsList = () => call<ToolDescriptor[]>("builtin_tools_list");
/** 本机 MCP 自动发现：扫客户端配置文件 + 应用 manifest，候选默认停用。 */
export const mcpDiscoverScan = () => call<DiscoveredServer[]>("mcp_discover_scan");
/** 导入选中的候选（按 signature，后端重扫落盘，明文不经过前端）。 */
export const mcpDiscoverImport = (signatures: string[]) =>
  call<McpImportReport>("mcp_discover_import", { signatures });
export const skillList = () => call<SkillView[]>("skill_list");
export const skillToggle = (id: string, enable: boolean) =>
  call<void>("skill_toggle", { id, enable });
export const skillRemove = (id: string) =>
  call<void>("skill_remove", { id });
/** 技能全文预览（body + references，skill_read）。 */
export const skillRead = (id: string) => call<SkillDetail>("skill_read", { id });
/** 从本地目录导入技能包（目录名即 id，含 SKILL.md），返回导入的 id。 */
export const skillImport = (source: string) =>
  call<string>("skill_import", { source });
/** 手写创建技能包（四项必填，后端走同一套校验）。 */
export const skillCreate = (input: SkillCreateInput) =>
  call<void>("skill_create", input as unknown as Record<string, unknown>);
/** 覆盖写已有包（plan-skill-auto-extract）。 */
export const skillUpdate = (input: SkillCreateInput) =>
  call<void>("skill_update", input as unknown as Record<string, unknown>);
/** 从远端加载技能包（GitHub repo/tree/raw、GitLab blob、任意 https SKILL.md）。
 *  联网只发生在这次显式点击；id 留空则从链接推导。返回最终 id。 */
export const skillLoadRemote = (url: string, id?: string) =>
  invokeWithTimeout<string>("skill_load_remote", { url, id: id ?? null }, 60000);

/** 自动提取：立即从 task 成包（可无视总开关，便于审核）。 */
export const skillExtractNow = (taskId: string) =>
  invokeWithTimeout<{ id: string; name: string }>(
    "skill_extract_now",
    { taskId },
    90000,
  );
export const skillAutoCandidates = () =>
  call<SkillAutoCandidate[]>("skill_auto_candidates");
export const skillAutoDismiss = (taskId: string) =>
  call<void>("skill_auto_dismiss", { taskId });
export const skillAutoGetEnabled = () => call<boolean>("skill_auto_get_enabled");
export const skillAutoSetEnabled = (enabled: boolean) =>
  call<void>("skill_auto_set_enabled", { enabled });
export const toolCallRecords = (limit?: number) =>
  call<ToolCallRecord[]>("tool_call_records", { limit });
/** 计划历史（plan-planning §5.6）：进度面板首屏基线，含解析后的 plan_json。 */
export const planList = (limit?: number) =>
  call<PlanTaskRow[]>("plan_list", { limit });

/** Phase 3 A1：显式准备恢复（不自动烧工具）。 */
export const taskResume = (taskId: string) =>
  call<ResumePrepareResult>("task_resume", { taskId });

/** Phase 3 A2：ask_user 步骤选择。 */
export const taskReentryChoice = (taskId: string, choice: "retry" | "skip" | "cancel") =>
  call<ResumePrepareResult>("task_reentry_choice", { taskId, choice });

/** P3-5：常驻 job 列表 / 开关。 */
export const jobList = () => call<JobRow[]>("job_list");
export const jobSetEnabled = (name: string, enabled: boolean) =>
  call<boolean>("job_set_enabled", { name, enabled });

/** 审批范围：once=仅本次；session=本会话同工具批量放行。 */
export type ApprovalReplyScope = "once" | "session";

/** 审批回执：返回 false 表示该请求已过期（60s 超时已拒绝或已答复过）。 */
export const mcpApprovalReply = (
  id: number,
  allow: boolean,
  scope: ApprovalReplyScope = "once",
) => call<boolean>("mcp_approval_reply", { id, allow, scope });

/** 工具调用确认模式：auto（默认放行）/ ask（每次询问）。 */
export const agentApprovalGet = () => call<"auto" | "ask">("agent_approval_get");
export const agentApprovalSet = (mode: "auto" | "ask") =>
  call<void>("agent_approval_set", { mode });

/** 云端循环预算开关（默认关；上限固定 32）。 */
export interface AgentBudgetCaps {
  toolCapEnabled: boolean;
  requestCapEnabled: boolean;
  toolCap: number;
  requestCap: number;
}
export const agentBudgetGet = () => call<AgentBudgetCaps>("agent_budget_get");
export const agentBudgetSet = (toolCapEnabled: boolean, requestCapEnabled: boolean) =>
  call<void>("agent_budget_set", { toolCapEnabled, requestCapEnabled });

/** agent 错误码 → 用户能懂的一句话（humanizeLlmError 同风格）。 */
export function humanizeAgentError(err: unknown): string {
  const s = String(err);
  if (s.includes("mcp_timeout")) return "工具连接超时了（60 秒没响应）";
  if (s.includes("mcp_unreachable")) return "工具连不上：检查命令/地址是否正确";
  if (s.includes("bad_url")) return "地址不对：要以 http:// 或 https:// 开头";
  if (s.includes("bad_command")) return "要填可执行文件的路径";
  if (s.includes("server_not_found")) return "这条配置已经不在了，刷新一下";
  if (s.includes("skill_invalid")) {
    if (s.includes("source is not a directory"))
      return "目录不对：所选文件夹里要有 SKILL.md";
    if (s.includes("missing fields")) return "名称 / 描述 / 正文都不能为空";
    if (s.includes("newline in frontmatter field"))
      return "名称和描述里不能有换行";
    if (s.includes("bad skill id"))
      return "技能 ID 不合法：只能小写字母、数字和连字符";
    if (s.includes("bad_skill_id")) return "技能 ID 不合法";
    return "技能包格式不对（缺 name/description）";
  }
  if (s.includes("skill_too_large")) return "技能包太大：SKILL.md ≤16KB，引用 ≤4KB×5";
  if (s.includes("skill_injection")) return "技能包试图改写人格边界，已拒绝载入";
  if (s.includes("skill_exists")) return "同名技能包已存在，换个 id 或先删掉旧的";
  if (s.includes("skill_reserved")) return "这个 id 是内置引擎包（日报/周报）保留的";
  if (s.includes("skill_io")) return "读写失败：源目录里要有 SKILL.md（引用为可选 references/*.md）";
  if (s.includes("skill_not_found")) return "链接里没找到 SKILL.md：确认仓库/分支/目录写对了";
  if (s.includes("skill_rate_limited")) return "GitHub 限流了（未登录每小时 60 次），过一会儿再试";
  if (s.includes("skill_network")) return "网络不通：检查网络后重试（本地技能不受影响）";
  if (s.includes("bad_skill_url"))
    return "链接形式不支持：贴 GitHub 仓库/目录/raw 链接，或任意以 .md 结尾的 https 链接";
  if (s.includes("missing SKILL.md")) return "这个文件夹里没有 SKILL.md";
  if (s.includes("bad_skill_id")) return "技能 ID 不合法";
  if (s.includes("no_tool_cache")) return "还没拿到工具清单：先启用或测试连接";
  if (s.includes("no_home")) return "读不到用户主目录，无法扫描本机";
  if (s.includes("denied")) return "这次调用被拒绝了";
  return s;
}
