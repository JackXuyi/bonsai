// 全部 Tauri 命令的类型化封装（签名对照 src-tauri/src/lib.rs generate_handler，
// payload 字段以 ui/main.js 实际消费 + Rust serde snake_case 为准）。
// 一律经 invokeWithTimeout（15s 超时兜底）；浏览器预览时返回兜底值而不是抛错。
import { invokeWithTimeout, inTauri } from "./tauriInvoke";
import type {
  Boundary,
  CalendarStatus,
  ConsolidateResult,
  DialogueOutcome,
  GrowthStatus,
  OnboardingAnswer,
  OnboardingStatus,
  OnboardingSubmitResult,
  PerceptionConfig,
  PerceptionStatus,
  RelationshipStatus,
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

// ---------- misc ----------
export const getPersona = () => call<Record<string, unknown>>("get_persona");
export const scaffoldReply = (args: Record<string, unknown>) =>
  call<string>("scaffold_reply", args);
export const evaluate = (args: Record<string, unknown>) =>
  call<unknown>("evaluate", args);
export const predictionStats = () => call<unknown>("prediction_stats");
export const dailyReport = () => call<string>("daily_report");
