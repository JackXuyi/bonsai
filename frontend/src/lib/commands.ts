// 全部 Tauri 命令的类型化封装（签名对照 src-tauri/src/lib.rs generate_handler，
// payload 字段以 ui/main.js 实际消费 + Rust serde snake_case 为准）。
// 一律经 invokeWithTimeout（15s 超时兜底）；浏览器预览时返回兜底值而不是抛错。
import { invokeWithTimeout, inTauri } from "./tauriInvoke";
import type {
  Boundary,
  CalendarStatus,
  CatalogRow,
  CloudEntryInput,
  CloudEntryView,
  CloudTestReport,
  ConsolidateResult,
  DialogueOutcome,
  GrowthStatus,
  LlmConfigView,
  McpSaveInput,
  McpServerRow,
  OllamaModelRow,
  OnboardingAnswer,
  OnboardingStatus,
  LogFile,
  MemoryOverview,
  OnboardingSubmitResult,
  PersonaSnapshot,
  PerceptionConfig,
  PerceptionStatus,
  RelationshipStatus,
  SkillView,
  ToolCallRecord,
  ToolDescriptor,
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

// ---------- dialogue ----------
export const dialogueChatStream = (
  userText: string,
  sessionId: string,
  onDelta: unknown
) =>
  call<DialogueOutcome>("dialogue_chat_stream", {
    userText,
    sessionId,
    onDelta,
  });
export const dialogueProactiveNow = () => call<string>("dialogue_proactive_now");

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
export const scaffoldReply = (args: Record<string, unknown>) =>
  call<string>("scaffold_reply", args);
export const evaluate = (args: Record<string, unknown>) =>
  call<unknown>("evaluate", args);
export const predictionStats = () => call<unknown>("prediction_stats");
export const dailyReport = () => call<string>("daily_report");

// ---------- 人格快照 / debug 日志（spec-m4）----------
export const personaSnapshot = () => call<PersonaSnapshot>("persona_snapshot");
export const debugLogTail = (file: LogFile, lines?: number) =>
  call<string[]>("debug_log_tail", { file, lines });
export const debugOpenLogDir = () => call<void>("debug_open_log_dir");

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
export const skillList = () => call<SkillView[]>("skill_list");
export const skillToggle = (id: string, enable: boolean) =>
  call<void>("skill_toggle", { id, enable });
export const skillRemove = (id: string) =>
  call<void>("skill_remove", { id });
export const toolCallRecords = (limit?: number) =>
  call<ToolCallRecord[]>("tool_call_records", { limit });

/** 审批回执：返回 false 表示该请求已过期（30s 超时已拒绝或已答复过）。 */
export const mcpApprovalReply = (id: number, allow: boolean) =>
  call<boolean>("mcp_approval_reply", { id, allow });

/** 工具调用确认模式：auto（默认放行）/ ask（每次询问）。 */
export const agentApprovalGet = () => call<"auto" | "ask">("agent_approval_get");
export const agentApprovalSet = (mode: "auto" | "ask") =>
  call<void>("agent_approval_set", { mode });

/** agent 错误码 → 用户能懂的一句话（humanizeLlmError 同风格）。 */
export function humanizeAgentError(err: unknown): string {
  const s = String(err);
  if (s.includes("mcp_timeout")) return "工具连接超时了（60 秒没响应）";
  if (s.includes("mcp_unreachable")) return "工具连不上：检查命令/地址是否正确";
  if (s.includes("bad_url")) return "地址不对：要以 http:// 或 https:// 开头";
  if (s.includes("bad_command")) return "要填可执行文件的路径";
  if (s.includes("server_not_found")) return "这条配置已经不在了，刷新一下";
  if (s.includes("skill_invalid")) return "技能包格式不对（缺 name/description）";
  if (s.includes("skill_too_large")) return "技能包太大：SKILL.md ≤8KB，引用 ≤4KB×5";
  if (s.includes("skill_injection")) return "技能包试图改写人格边界，已拒绝载入";
  if (s.includes("bad_skill_id")) return "技能 ID 不合法";
  if (s.includes("no_tool_cache")) return "还没拿到工具清单：先启用或测试连接";
  if (s.includes("denied")) return "这次调用被拒绝了";
  return s;
}
