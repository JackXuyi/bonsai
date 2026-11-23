//! 上下文预算（`docs/zh/plan-context.md` v0.1 §4 / T1）。
//!
//! 这一层只回答三个问题，**全部是纯函数**——不读库、不发请求、不依赖模型：
//!
//! 1. 这段文本大概多少 token？ → [`estimate`]
//! 2. 这个模型的窗口有多大？ → [`window_for`]
//! 3. 每个上下文分段最多给多少？ → [`Segment::cap`]
//!
//! 为什么不用 tokenizer：`tiktoken` 要几 MB BPE 文件与网络，而这里只需要
//! 「不低估」。低估的代价是 400 或本地窗口静默截断（连人格宪法一起丢），
//! 高估的代价只是少放一轮历史——所以估算系数一律保守取高，偏差再用
//! provider 的 `usage.prompt_tokens` 回流校准（T7）。
//!
//! T1 只做纯函数（估算器 / 窗口表 / 预算 / 分段上限），T2 接上装配
//! [`build`]（含 L0 按预算丢最旧轮）。轮内压缩（T3）与摘要（T6）仍待接线。

use crate::persona::eval::Turn;

/// 一条消息的结构开销：角色包装 + 分隔符。各家协议略有差异，统一 4。
pub const MSG_OVERHEAD: usize = 4;
/// 工具结果额外开销：`tool_call_id` + 角色名。协议要求一一对应，故单算。
pub const TOOL_OVERHEAD: usize = 8;

/// 未知模型的兜底窗口：宁可早压，不可溢出。
pub const FALLBACK_WINDOW: usize = 32_000;
/// DeepSeek（V3.2 起）官方上下文：128K。
pub const DEEPSEEK_WINDOW: usize = 128_000;
/// Claude 原生协议：200K。
pub const CLAUDE_WINDOW: usize = 200_000;
/// OpenAI 兼容端点：按常用档保守取 128K。
pub const OPENAI_WINDOW: usize = 128_000;

/// 软预算默认值（plan §4.1 / D1）：窗口是硬上限，软预算才是成本闸。
/// 模型窗口再大也不让一轮对话烧过这个数——陪伴场景 32k 已经远超需要。
pub const SOFT_BUDGET_DEFAULT: usize = 32_000;
/// 答复预留：与 `llm::CLOUD_MAX_TOKENS` 对齐，从可用额度里先扣掉。
pub const REPLY_RESERVE_TOKENS: usize = 4096;

/// 本地模型的最小可用窗口：低于此值连「宪法 + 一句」都装不下。
pub const MIN_LOCAL_CTX: usize = 1024;

// ---------------------------------------------------------------- 分段上限
//
// 优先级从高到低：宪法 > 边界 > 环境 > 本轮用户 > 画像/skill/目录 > 历史。
// 数字来自 plan §4.2，改之前先看那张表——这里的常量是它的唯一实现。

/// 人格宪法 + 关系段 + 状态段。
pub const CAP_CONSTITUTION: usize = 1500;
/// 用户边界。安全相关，永不裁。
pub const CAP_BOUNDARIES: usize = 300;
/// 环境段（时间/时区/语言）：一行，永不裁。
pub const CAP_ENVIRONMENT: usize = 100;
/// 用户画像（与 `persona::prompt::PROFILE_BUDGET_CHARS = 200` 字对齐）。
pub const CAP_PROFILE: usize = 250;
/// skill 注入段。
pub const CAP_SKILLS: usize = 800;
/// 工具目录（`tools[]` 每轮全量下发，MCP 描述长度不可控）。
pub const CAP_TOOL_CATALOG: usize = 1500;
/// 早期对话摘要（L1/L2 产物）。
pub const CAP_DIGEST: usize = 600;
/// 本轮用户消息：**不截**（用户原话必须完整），这里只是告警阈值。
pub const CAP_USER_TEXT: usize = 4000;
/// 单条工具结果上限：超了就降级成 head+tail（T3）。
pub const CAP_TOOL_RESULT: usize = 1500;
/// 本轮工具结果合计占可用额度的比例。
pub const TOOL_ROOM_RATIO: f64 = 0.40;
/// 历史轮上限：从 6 放宽到 12，真正的闸门是预算（装不下就丢，T2）。
pub const HISTORY_MAX_TURNS: usize = 12;

/// 一个有固定上限的上下文分段。
///
/// 只覆盖「固定上限」的那几段；历史轮与工具结果的上限取决于可用额度，
/// 由 [`Budget`] 在装配时算，不入枚举（避免一个枚举里混两种语义）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Segment {
    Constitution,
    Boundaries,
    Environment,
    Profile,
    Skills,
    ToolCatalog,
    Digest,
}

impl Segment {
    /// 该段的硬上限（tokens）。
    pub fn cap(self) -> usize {
        match self {
            Segment::Constitution => CAP_CONSTITUTION,
            Segment::Boundaries => CAP_BOUNDARIES,
            Segment::Environment => CAP_ENVIRONMENT,
            Segment::Profile => CAP_PROFILE,
            Segment::Skills => CAP_SKILLS,
            Segment::ToolCatalog => CAP_TOOL_CATALOG,
            Segment::Digest => CAP_DIGEST,
        }
    }

    /// 是否属于「永不裁剪」的安全段。装配层据此决定让路顺序。
    pub fn must_keep(self) -> bool {
        matches!(
            self,
            Segment::Constitution | Segment::Boundaries | Segment::Environment
        )
    }

    /// 稳定短名：进 trace / 日志用，避免 Debug 格式漂移。
    pub fn label(self) -> &'static str {
        match self {
            Segment::Constitution => "constitution",
            Segment::Boundaries => "boundaries",
            Segment::Environment => "environment",
            Segment::Profile => "profile",
            Segment::Skills => "skills",
            Segment::ToolCatalog => "tool_catalog",
            Segment::Digest => "digest",
        }
    }
}

/// 一轮对话的预算视图。
///
/// `window` 是模型硬上限（超了报错/截断），`soft` 是成本闸（我们自己设的
/// 天花），`usable` 是扣掉答复预留之后真正能塞上下文的部分。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    pub window: usize,
    pub soft: usize,
    pub reply_reserve: usize,
    pub usable: usize,
}

impl Budget {
    /// 按窗口与软预算构造；`usable` 用饱和减法，窗口比预留还小时得 0 而不是 panic。
    pub fn new(window: usize, soft: usize) -> Self {
        let capped = window.min(soft);
        Budget {
            window,
            soft,
            reply_reserve: REPLY_RESERVE_TOKENS,
            usable: capped.saturating_sub(REPLY_RESERVE_TOKENS),
        }
    }

    /// 从模型名推预算：本地传 `Some(num_ctx)`，云端传 `None`。
    pub fn from_model(model: &str, local_ctx: Option<usize>) -> Self {
        Budget::new(window_for(model, local_ctx), SOFT_BUDGET_DEFAULT)
    }

    /// 本轮工具结果可用的合计额度（按 `usable` 的比例，至少留 0）。
    pub fn tool_room(&self) -> usize {
        if self.usable == 0 {
            return 0;
        }
        (self.usable as f64 * TOOL_ROOM_RATIO).floor() as usize
    }

    /// 历史轮可用的剩余额度：可用额度减去「不可压段」与工具额度。
    ///
    /// 饱和到 0——装配层拿到 0 就该只保留最近 2 轮（plan §6.3 的例外）。
    pub fn history_room(&self, fixed_tokens: usize) -> usize {
        self.usable.saturating_sub(fixed_tokens + self.tool_room())
    }
}

// ---------------------------------------------------------------- 估算器

/// 估算一段文本的 token 数（不含消息结构开销）。
///
/// 系数按「宁高勿低」取：汉字/全角标点 1 token/字（实际约 0.6-0.7），
/// 英文数字与空白 4 字符/token，其余符号 0.75 token/字符。结果向上取整，
/// 保证同输入同输出（预算判断不该有抖动）。
pub fn estimate(text: &str) -> usize {
    let mut cjk = 0usize;
    let mut latin = 0usize;
    let mut other = 0usize;
    for c in text.chars() {
        if is_cjk(c) {
            cjk += 1;
        } else if c.is_ascii_alphanumeric() || c.is_ascii_whitespace() {
            latin += 1;
        } else {
            other += 1;
        }
    }
    let tokens = cjk as f64 + latin as f64 / 4.0 + other as f64 * 0.75;
    tokens.ceil() as usize
}

/// 一条普通消息（user / assistant / system）的估算，含结构开销。
pub fn estimate_msg(text: &str) -> usize {
    estimate(text) + MSG_OVERHEAD
}

/// 一条 `tool` 结果消息的估算，含 `tool_call_id` 与角色开销。
pub fn estimate_tool_result(text: &str) -> usize {
    estimate(text) + TOOL_OVERHEAD
}

/// 一对 user/assistant 轮次的估算（历史累加用）。
pub fn estimate_turn(user: &str, assistant: &str) -> usize {
    estimate_msg(user) + estimate_msg(assistant)
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3000..=0x303F       // CJK 标点
        | 0x3400..=0x4DBF     // 扩展 A
        | 0x4E00..=0x9FFF     // 基本区
        | 0xF900..=0xFAFF     // 兼容表意
        | 0xFF00..=0xFFEF     // 全角形式
    )
}

/// 模型 → 硬窗口。
///
/// 本地传输必须传 `Some(num_ctx)`（T2 会把 `ollama.rs` 里现在缺省的
/// `num_ctx = 0` 改成显式值）：本地模型的窗口是我们自己定的，模型名推不出来。
/// 云端传 `None`，按模型名匹配；匹配不上取 [`FALLBACK_WINDOW`]。
pub fn window_for(model: &str, local_ctx: Option<usize>) -> usize {
    if let Some(ctx) = local_ctx {
        return ctx.max(MIN_LOCAL_CTX);
    }
    let m = model.to_ascii_lowercase();
    if m.contains("deepseek") {
        DEEPSEEK_WINDOW
    } else if m.contains("claude") {
        CLAUDE_WINDOW
    } else if m.starts_with("gpt-") || m.contains("openai") {
        OPENAI_WINDOW
    } else {
        FALLBACK_WINDOW
    }
}

// ---------------------------------------------------------------- 装配（L0）

/// 最近 2 轮即使装不下也留（plan §6.3 第 3 步的例外）：陪伴场景里
/// 「上一句说了什么」比预算本身重要，硬压到 0 轮会让对话立刻失忆。
pub const MIN_KEEP_TURNS: usize = 2;

/// 摘要段的标题。措辞刻意写明「系统压缩，非用户原话」——避免模型把
/// 摘要当成用户说过的话去引用，也避免用户以为自己的话被改写了。
pub const DIGEST_HEADER: &str = "# 较早对话摘要（系统压缩，非用户原话）\n";

/// 装配输入。系统段拆成 `env` 与 `body` 两截只是为了与现状拼接顺序一致
/// （`format!("{env}{body}")`，环境段在最前）。
pub struct BuildInput<'a> {
    pub env: &'a str,
    pub body: &'a str,
    /// L1/L2 摘要正文（T6 才写库，暂无时传 `None`）；进 system 末尾。
    pub digest: Option<&'a str>,
    /// 已按 [`HISTORY_MAX_TURNS`] 上限读出的历史，oldest first。
    pub history: Vec<Turn>,
    /// 本轮用户消息：**不截**，只计入估算。
    pub user_text: &'a str,
    /// 是否给工具结果留额度。云端工具循环会往消息序列里插 `tool` 消息，
    /// 必须留；本地模式零工具调用，留了就是白占 2B 的 8192 窗口。
    pub reserve_tool_room: bool,
}

/// 装配结果（plan §6.3）。
#[derive(Debug, Clone)]
pub struct Built {
    /// env + body（+ 摘要段）。
    pub system: String,
    /// 已按预算裁剪，oldest first；内容与顺序未改动。
    pub turns: Vec<Turn>,
    /// 被 L0 丢掉的轮数（0 = 没压），给 trace 与 T7 观测用。
    pub dropped: usize,
    /// 本轮请求的输入估算（system + 历史 + 用户话，不含答复预留）。
    pub est_tokens: usize,
}

/// 按预算装配一轮上下文（plan §6.3）。
///
/// 预算充足时**逐条等价**于现状：system 仍是 `env + body`，历史原样返回，
/// `dropped = 0`——这一条由单测锁定，避免「加预算」顺手改变对话手感。
/// 装不下时从最旧开始丢（不跳着塞，保持连续），最近 [`MIN_KEEP_TURNS`]
/// 轮无条件保留。
pub fn build(input: BuildInput<'_>, budget: Budget) -> Built {
    let mut system = String::with_capacity(input.env.len() + input.body.len() + CAP_DIGEST * 3);
    system.push_str(input.env);
    system.push_str(input.body);
    if let Some(digest) = input.digest.filter(|d| !d.trim().is_empty()) {
        system.push_str(DIGEST_HEADER);
        system.push_str(digest.trim_end());
        system.push('\n');
    }

    // 不可压段全部算进 fixed：宪法/边界/环境/画像/skill/摘要都不参与 L0。
    let fixed = estimate_msg(&system);
    let room = if input.reserve_tool_room {
        budget.history_room(fixed)
    } else {
        budget.usable.saturating_sub(fixed)
    };

    let total = input.history.len();
    let mut kept: Vec<Turn> = Vec::with_capacity(total.min(HISTORY_MAX_TURNS));
    let mut used = 0usize;
    for (i, turn) in input
        .history
        .into_iter()
        .rev()
        .take(HISTORY_MAX_TURNS)
        .enumerate()
    {
        let cost = estimate_turn(&turn.user, &turn.assistant);
        if i >= MIN_KEEP_TURNS && used + cost > room {
            break;
        }
        used += cost;
        kept.push(turn);
    }
    let dropped = total - kept.len();
    kept.reverse();

    Built {
        system,
        turns: kept,
        dropped,
        est_tokens: fixed + used + estimate_msg(input.user_text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------ 估算器

    #[test]
    fn estimate_counts_cjk_near_one_token_per_char() {
        // 中文按 1 token/字（保守高估；实测约 0.6-0.7）。
        assert_eq!(estimate("你好"), 2);
        assert_eq!(estimate("今天天气不错，适合出门。"), 12);
    }

    #[test]
    fn estimate_counts_ascii_by_quarter() {
        assert_eq!(estimate("abcd"), 1);
        // 空白并入拉丁字符桶：11 字符 / 4 = 2.75 → 3
        assert_eq!(estimate("hello world"), 3);
    }

    #[test]
    fn estimate_mixed_text_adds_up() {
        // 2 CJK + 6 拉丁（world + 空格）+ 1 符号 0.75 = 4.25 → 5
        assert_eq!(estimate("你好 world!"), 5);
    }

    #[test]
    fn estimate_rounds_up_and_never_undershoots_len_over_four() {
        for s in [
            "😀😀ab",
            "fn main() { println!(\"hi\"); }",
            "\n\n\t  ",
            "混合 mixed 文本 123",
            "https://example.com/a/b?c=d",
        ] {
            let lower_bound = s.chars().count() / 4;
            assert!(
                estimate(s) >= lower_bound,
                "estimate({s:?}) 低于字符数/4 的下界"
            );
        }
    }

    #[test]
    fn estimate_empty_is_zero() {
        assert_eq!(estimate(""), 0);
    }

    #[test]
    fn message_and_tool_overheads_are_added_on_top() {
        assert_eq!(estimate_msg(""), MSG_OVERHEAD);
        assert_eq!(estimate_tool_result(""), TOOL_OVERHEAD);
        assert_eq!(
            estimate_turn("你好", "在"),
            2 + MSG_OVERHEAD + 1 + MSG_OVERHEAD
        );
    }

    // ------------------------------------------------------------ 窗口表

    #[test]
    fn window_for_known_cloud_models() {
        assert_eq!(window_for("deepseek-chat", None), DEEPSEEK_WINDOW);
        assert_eq!(window_for("deepseek-reasoner", None), DEEPSEEK_WINDOW);
        assert_eq!(window_for("claude-3-5-sonnet", None), CLAUDE_WINDOW);
        assert_eq!(window_for("gpt-4o", None), OPENAI_WINDOW);
    }

    #[test]
    fn window_for_local_uses_the_ctx_we_set() {
        assert_eq!(window_for("qwen3.5:2b", Some(8192)), 8192);
        // 模型名自带 "qwen" 也不该盖过显式 ctx。
        assert_eq!(window_for("qwen3.5:2b", Some(4096)), 4096);
        // 不合理的 0 被抬到最小可用窗口，避免算出 usable=0 的黑洞。
        assert_eq!(window_for("qwen3.5:2b", Some(0)), MIN_LOCAL_CTX);
    }

    #[test]
    fn window_for_unknown_model_falls_back_conservatively() {
        assert_eq!(window_for("acme-9000", None), FALLBACK_WINDOW);
        assert_eq!(window_for("", None), FALLBACK_WINDOW);
    }

    // ------------------------------------------------------------ 预算

    #[test]
    fn budget_caps_at_soft_and_reserves_the_reply() {
        let b = Budget::from_model("deepseek-chat", None);
        assert_eq!(b.window, DEEPSEEK_WINDOW);
        assert_eq!(b.soft, SOFT_BUDGET_DEFAULT);
        // 窗口再大也不越过软预算
        assert_eq!(b.usable, SOFT_BUDGET_DEFAULT - REPLY_RESERVE_TOKENS);
    }

    #[test]
    fn budget_takes_the_smaller_of_window_and_soft() {
        let b = Budget::new(8192, SOFT_BUDGET_DEFAULT);
        assert_eq!(b.usable, 8192 - REPLY_RESERVE_TOKENS);
    }

    #[test]
    fn budget_never_panics_on_tiny_or_zero_window() {
        let b = Budget::new(1000, SOFT_BUDGET_DEFAULT);
        assert_eq!(b.usable, 0, "窗口小于预留时可用额度饱和为 0");
        assert_eq!(b.tool_room(), 0);
        assert_eq!(b.history_room(0), 0);

        let z = Budget::new(0, 0);
        assert_eq!(z.usable, 0);
        assert_eq!(z.history_room(12345), 0, "fixed 超过可用额度时也饱和");
    }

    #[test]
    fn tool_room_is_forty_percent_and_history_gets_the_rest() {
        let b = Budget::new(SOFT_BUDGET_DEFAULT, SOFT_BUDGET_DEFAULT);
        let usable = b.usable;
        assert_eq!(
            b.tool_room(),
            (usable as f64 * TOOL_ROOM_RATIO).floor() as usize
        );
        assert_eq!(
            b.history_room(1000),
            usable - 1000 - b.tool_room(),
            "历史额度 = 可用 − 不可压段 − 工具额度"
        );
    }

    // ------------------------------------------------------------ 分段表

    #[test]
    fn fixed_segment_caps_match_the_plan_table() {
        assert_eq!(Segment::Constitution.cap(), 1500);
        assert_eq!(Segment::Boundaries.cap(), 300);
        assert_eq!(Segment::Environment.cap(), 100);
        assert_eq!(Segment::Profile.cap(), 250);
        assert_eq!(Segment::Skills.cap(), 800);
        assert_eq!(Segment::ToolCatalog.cap(), 1500);
        assert_eq!(Segment::Digest.cap(), 600);
    }

    #[test]
    fn only_safety_segments_are_untrimmable() {
        assert!(Segment::Constitution.must_keep());
        assert!(Segment::Boundaries.must_keep());
        assert!(Segment::Environment.must_keep());
        for s in [
            Segment::Profile,
            Segment::Skills,
            Segment::ToolCatalog,
            Segment::Digest,
        ] {
            assert!(!s.must_keep(), "{} 应该可让路", s.label());
        }
    }

    #[test]
    fn fixed_caps_plus_default_history_fit_inside_the_soft_budget() {
        let b = Budget::from_model("deepseek-chat", None);
        let fixed: usize = [
            Segment::Constitution,
            Segment::Boundaries,
            Segment::Environment,
            Segment::Profile,
            Segment::Skills,
            Segment::ToolCatalog,
            Segment::Digest,
        ]
        .iter()
        .map(|s| s.cap())
        .sum();
        // 不可压段 + 工具额度 + 一句用户话，必须给历史留出正数空间。
        let floor = fixed + b.tool_room() + CAP_USER_TEXT;
        assert!(
            b.history_room(fixed) > 0,
            "默认预算下历史额度为正（fixed={fixed}, usable={}, floor={floor}）",
            b.usable
        );
    }

    // ------------------------------------------------------------ 装配（L0）

    fn turn(user: &str, assistant: &str) -> Turn {
        Turn {
            user: user.into(),
            assistant: assistant.into(),
        }
    }

    fn pairs(turns: &[Turn]) -> Vec<(String, String)> {
        turns
            .iter()
            .map(|t| (t.user.clone(), t.assistant.clone()))
            .collect()
    }

    #[test]
    fn build_with_ample_budget_matches_the_old_assembly() {
        // A2：预算充足时不许改变行为——system 仍是 env + body，历史原样。
        let env = "现在是周一 21:00，杭州。\n";
        let body = "你是盆栽。\n规则：不许谄媚。\n";
        let history = vec![
            turn("在吗", "在。"),
            turn("今天有点累", "那就早点睡。"),
            turn("帮我看看日程", "明天上午有个会。"),
        ];
        let budget = Budget::from_model("deepseek-chat", None);
        let built = build(
            BuildInput {
                env,
                body,
                digest: None,
                history: history.clone(),
                user_text: "明天几点开会",
                reserve_tool_room: true,
            },
            budget,
        );

        assert_eq!(built.system, format!("{env}{body}"));
        assert_eq!(pairs(&built.turns), pairs(&history), "顺序与内容都不许动");
        assert_eq!(built.dropped, 0);
        let expect = estimate_msg(&built.system)
            + history
                .iter()
                .map(|t| estimate_turn(&t.user, &t.assistant))
                .sum::<usize>()
            + estimate_msg("明天几点开会");
        assert_eq!(built.est_tokens, expect);
    }

    #[test]
    fn build_drops_the_oldest_turns_first_and_keeps_the_newest_two() {
        // A3：历史远超预算 → 从最旧开始丢，最近 2 轮必在，dropped 被计数。
        let long = "很长的内容".repeat(40);
        let history: Vec<Turn> = (0..30)
            .map(|i| turn(&format!("{i}{long}"), &format!("答{i}{long}")))
            .collect();
        let budget = Budget::new(8192, SOFT_BUDGET_DEFAULT); // usable 4096
        let built = build(
            BuildInput {
                env: "",
                body: "你是盆栽。",
                digest: None,
                history: history.clone(),
                user_text: "继续",
                reserve_tool_room: false,
            },
            budget,
        );

        assert!(built.dropped > 0, "这么长的历史必须触发压缩");
        assert!(built.turns.len() >= MIN_KEEP_TURNS);
        assert!(built.turns.len() < history.len());
        // 留下的一定是**最新**那一截，且顺序仍是 oldest first。
        let tail = &history[history.len() - built.turns.len()..];
        assert_eq!(
            pairs(&built.turns),
            pairs(tail),
            "留下的必须是最新的连续一段"
        );
        assert_eq!(built.dropped, history.len() - built.turns.len());
        assert!(built.est_tokens <= budget.usable.max(1) || built.turns.len() == MIN_KEEP_TURNS);
    }

    #[test]
    fn build_keeps_two_turns_even_when_the_budget_is_zero() {
        let history = vec![turn("一", "1"), turn("二", "2"), turn("三", "3")];
        let built = build(
            BuildInput {
                env: "",
                body: "",
                digest: None,
                history,
                user_text: "",
                reserve_tool_room: true,
            },
            Budget::new(1000, 1000), // usable = 0
        );
        assert_eq!(built.turns.len(), MIN_KEEP_TURNS);
        assert_eq!(built.dropped, 1);
        assert_eq!(built.turns.last().map(|t| t.user.as_str()), Some("三"));
    }

    #[test]
    fn build_caps_history_at_twelve_turns() {
        let history: Vec<Turn> = (0..20).map(|i| turn(&format!("u{i}"), "a")).collect();
        let built = build(
            BuildInput {
                env: "",
                body: "你是盆栽。",
                digest: None,
                history,
                user_text: "hi",
                reserve_tool_room: true,
            },
            Budget::from_model("deepseek-chat", None),
        );
        assert_eq!(built.turns.len(), HISTORY_MAX_TURNS);
        assert_eq!(built.dropped, 20 - HISTORY_MAX_TURNS);
        assert_eq!(built.turns[0].user, "u8", "留下的是最新 12 轮");
    }

    #[test]
    fn build_puts_the_digest_at_the_end_of_the_system_prompt() {
        let built = build(
            BuildInput {
                env: "环境。\n",
                body: "宪法。",
                digest: Some("- [11-20] 用户问了天气 → 答了明天有雨"),
                history: vec![],
                user_text: "再问一次",
                reserve_tool_room: true,
            },
            Budget::from_model("deepseek-chat", None),
        );
        assert!(built.system.starts_with("环境。\n宪法。"));
        assert!(built.system.contains(DIGEST_HEADER.trim()));
        assert!(built.system.trim_end().ends_with("答了明天有雨"));
    }

    #[test]
    fn empty_digest_is_not_rendered() {
        let built = build(
            BuildInput {
                env: "",
                body: "宪法。",
                digest: Some("   \n"),
                history: vec![],
                user_text: "hi",
                reserve_tool_room: true,
            },
            Budget::from_model("deepseek-chat", None),
        );
        assert_eq!(built.system, "宪法。");
        assert!(!built.system.contains("较早对话摘要"));
    }

    #[test]
    fn local_mode_does_not_reserve_tool_room() {
        // 本地零工具调用：同样的预算下，不预留工具额度能多装几轮。
        let history: Vec<Turn> = (0..12)
            .map(|i| turn(&format!("第{i}轮问题").repeat(20), &"答".repeat(200)))
            .collect();
        let budget = Budget::new(8192, SOFT_BUDGET_DEFAULT); // usable 4096
        let mut with_tools = 0;
        let mut without_tools = 0;
        for reserve in [true, false] {
            let built = build(
                BuildInput {
                    env: "",
                    body: &"宪法".repeat(800),
                    digest: None,
                    history: history.clone(),
                    user_text: "继续",
                    reserve_tool_room: reserve,
                },
                budget,
            );
            if reserve {
                with_tools = built.turns.len();
            } else {
                without_tools = built.turns.len();
            }
        }
        assert!(
            without_tools > with_tools,
            "不留工具额度应能多装历史（with={with_tools}, without={without_tools}）"
        );
    }
}
