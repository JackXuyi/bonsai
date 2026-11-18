//! Persona card → system prompt renderer (plan-persona.md §3.3).
//!
//! Deterministic by contract: the same card + context always renders the
//! same prompt (diffable, testable). Section order is the anti-dilution
//! defense: L1 constitution first, L2 expression second, L3 state as one
//! line, user boundaries last — the constitution is never the thing that
//! gets pushed to the end of a growing prompt.
//!
//! L3 is REFERENCED here, not implemented: mood comes from face.rs's state
//! machine, hour/raining from the runtime tick. This file owns no state.

use super::Persona;

pub struct PromptContext<'a> {
    /// One-line mood/state description (from face.rs base state, L3).
    pub mood: &'a str,
    /// Local hour 0..24 (quirk trigger input: 深夜话变少).
    pub hour: u32,
    /// Whether it's raining (quirk trigger input: 下雨天会提一句天气).
    pub raining: bool,
    /// User-individual boundaries from the memory layer's boundaries table
    /// (runtime-collected, merged at render time — plan §3.2 双源对齐).
    pub user_boundaries: &'a [String],
}

/// 23:00–06:59 counts as 深夜 for the「深夜话变少」quirk.
fn is_night(hour: u32) -> bool {
    hour >= 23 || hour < 7
}

pub fn render(card: &Persona, ctx: &PromptContext) -> String {
    let mut s = String::new();

    // ---- L1 constitution: immutable, always first ----
    s.push_str("# 人格宪法（不可变）\n");
    s.push_str(&format!("你是{}。\n\n", card.name));
    s.push_str("## 性格\n");
    for t in &card.l1.traits {
        s.push_str(&format!("- {}\n", t.text));
    }
    s.push_str("\n## 底线价值观\n");
    for v in &card.l1.values {
        s.push_str(&format!("- {}\n", v));
    }
    s.push_str("\n## 人格边界（绝不越过）\n");
    for b in &card.l1.boundaries {
        s.push_str(&format!("- {}\n", b.rule));
    }

    // ---- L2 expression: semi-stable ----
    s.push_str("\n# 表达方式\n");
    s.push_str(&format!("语气：{}\n", card.l2.tone));
    if !card.l2.quirks.is_empty() {
        s.push_str("习惯：\n");
        for q in &card.l2.quirks {
            s.push_str(&format!("- {}\n", q));
        }
    }
    if !card.l2.catchphrases.is_empty() {
        s.push_str("口头禅（可以自然使用，不要每句都用）：\n");
        for c in &card.l2.catchphrases {
            s.push_str(&format!("- 「{}」\n", c));
        }
    }

    // ---- L3 state: one line, volatile ----
    s.push_str("\n# 当前状态\n");
    s.push_str(&format!("心情：{}\n", ctx.mood));
    if is_night(ctx.hour) {
        s.push_str("时段：深夜——按习惯，话变少。\n");
    } else {
        s.push_str("时段：白天。\n");
    }
    if ctx.raining {
        s.push_str("天气：下雨——可以提一句天气。\n");
    }

    // ---- user boundaries: runtime, after constitution & expression ----
    if !ctx.user_boundaries.is_empty() {
        s.push_str("\n# 用户边界（必须遵守，来自用户本人）\n");
        for b in ctx.user_boundaries {
            s.push_str(&format!("- {}\n", b));
        }
    }

    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card() -> Persona {
        super::super::load()
    }

    fn ctx<'a>(
        mood: &'a str,
        hour: u32,
        raining: bool,
        user_boundaries: &'a [String],
    ) -> PromptContext<'a> {
        PromptContext { mood, hour, raining, user_boundaries }
    }

    #[test]
    fn render_is_deterministic() {
        let c = card();
        let b = vec!["别在我专注时弹窗".to_string()];
        let a = render(&c, &ctx("平静", 10, false, &b));
        let d = render(&c, &ctx("平静", 10, false, &b));
        assert_eq!(a, d);
    }

    #[test]
    fn l1_comes_before_l2_and_l3() {
        let c = card();
        let out = render(&c, &ctx("平静", 10, false, &[]));
        let l1 = out.find("# 人格宪法").unwrap();
        let l2 = out.find("# 表达方式").unwrap();
        let l3 = out.find("# 当前状态").unwrap();
        assert!(l1 < l2 && l2 < l3);
    }

    #[test]
    fn night_and_rain_trigger_respective_directives() {
        let c = card();
        let day = render(&c, &ctx("平静", 10, false, &[]));
        let night = render(&c, &ctx("平静", 23, false, &[]));
        let rain = render(&c, &ctx("平静", 10, true, &[]));
        // 断言的是 ctx 派生的指令行（quirk 文本「深夜话变少」本身总在 L2 段出现）。
        assert!(day.contains("时段：白天"));
        assert!(night.contains("时段：深夜"));
        assert!(rain.contains("天气：下雨"));
        assert!(!rain.contains("时段：深夜"));
    }

    #[test]
    fn user_boundaries_rendered_after_constitution() {
        let c = card();
        let b = vec!["别在我专注时弹窗".to_string()];
        let out = render(&c, &ctx("平静", 10, false, &b));
        assert!(out.contains("别在我专注时弹窗"));
        assert!(out.find("用户边界").unwrap() > out.find("# 人格宪法").unwrap());
    }

    #[test]
    fn empty_user_boundaries_skip_the_section() {
        let c = card();
        let out = render(&c, &ctx("平静", 10, false, &[]));
        assert!(!out.contains("用户边界（必须遵守"));
    }
}
