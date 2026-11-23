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
//! T1 的边界：本文件**不被任何调用方引用**。装配（`build`）、裁剪（L0）、
//! 摘要（L1）与轮内压缩都在 T2/T3 接线，避免一次提交同时改行为与改预算。

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
/// 历史轮上限（`MAX_HISTORY_TURNS=6` 的放宽版，靠预算而非轮数兜底）。
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
}
