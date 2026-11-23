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
    /// Long-term relationship tone (plan-relationship.md §3.5): "reserved" |
    /// "familiar" | "direct" | "rusty". Kept as a plain string so the persona
    /// renderer stays independent of the relationship module.
    pub relationship_mode: &'a str,
    /// 用户画像（plan-user-profile.md §5）：(中文标签, 值) 已成对的切片——
    /// 传对儿而不传 key，本模块对 memory 层保持零依赖。空 = 整段不渲染。
    pub user_profile: &'a [(&'a str, &'a str)],
}

/// The long-term tone block. `direct` is where the relationship becomes
/// visible in behavior: the hedged openers the card already bans get spelled
/// out as a hard rule (plan §3.5 decision 3).
fn relationship_block(mode: &str, card: &Persona) -> String {
    match mode {
        "familiar" => {
            "熟了。可以口语化，可以引用你记得的旧事（比如「你上次说的那个」）；不用每句都客套。\n"
                .into()
        }
        "direct" => {
            let mut s = String::from(
                "混熟了，你能顶嘴。\n\
                 - 不同意就直说：第一句就给判断，不要先铺垫。\n\
                 - 反对必须指向具体那一点，说清你不同意什么。\n\
                 - 不要为了和气改口。\n",
            );
            if !card.disagreement_policy.banned_forms.is_empty() {
                s.push_str(&format!(
                    "- 以下开场白一律禁止：{}\n",
                    card.disagreement_policy.banned_forms.join(" / ")
                ));
            }
            s
        }
        "rusty" => {
            "很久没聊了，有点生分。语气收一点，别一上来就热络。\n".into()
        }
        _ => "刚认识不久。客气一点，别装熟；想反对先留余地。\n".into(),
    }
}

/// 23:00–06:59 counts as 深夜 for the「深夜话变少」quirk.
fn is_night(hour: u32) -> bool {
    hour >= 23 || hour < 7
}

/// Environment segment (spec-s3 §2, plan D6): time / timezone / language /
/// platform, prepended BEFORE the L1 constitution. The instant comes from
/// the caller — never `Local::now()` inside this fn, so tests can pin the
/// clock (the proactive_spoke_today lesson).
pub fn env_segment(now: chrono::DateTime<chrono::Local>, language: &str) -> String {
    use chrono::Datelike;
    let weekday = match now.weekday() {
        chrono::Weekday::Mon => "周一",
        chrono::Weekday::Tue => "周二",
        chrono::Weekday::Wed => "周三",
        chrono::Weekday::Thu => "周四",
        chrono::Weekday::Fri => "周五",
        chrono::Weekday::Sat => "周六",
        chrono::Weekday::Sun => "周日",
    };
    // "+0800" → "UTC+08:00"
    let z = now.format("%z").to_string();
    let tz = format!("UTC{}:{}", &z[..3], &z[3..]);
    format!(
        "# 环境（系统注入，不可变）\n当前时间：{}（{}）· 时区 {} · 语言：{} · 平台：{}\n\n",
        now.format("%Y-%m-%d %H:%M"),
        weekday,
        tz,
        language,
        std::env::consts::OS
    )
}

/// 画像段预算（plan-user-profile.md §5）：整段 ≤200 字符，超了就截断到
/// 放得下的最后一行——画像压缩成短行，不占人格卡预算。
const PROFILE_BUDGET_CHARS: usize = 200;

pub fn render(card: &Persona, ctx: &PromptContext) -> String {
    let mut s = String::new();

    // ---- 用户画像：身份快照，每轮固定注入，不走红召回门控 ----
    // 位于环境段（dialogue 侧前置拼接）之后、L1 宪法之前：先知道你是谁，
    // 再谈人格怎么表达。全空则整段不渲染（不产生提示泄漏）。
    if !ctx.user_profile.is_empty() {
        let mut section = String::from("# 用户画像\n");
        let mut used = section.chars().count();
        for (label, value) in ctx.user_profile {
            let line = format!("{label}：{value}\n");
            let n = line.chars().count();
            if used + n > PROFILE_BUDGET_CHARS {
                break;
            }
            section.push_str(&line);
            used += n;
        }
        s.push_str(&section);
        s.push('\n');
    }

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

    // ---- relationship: long-term tone, stabler than L3 ----
    s.push_str("\n# 关系\n");
    s.push_str(&relationship_block(ctx.relationship_mode, card));

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
        PromptContext {
            mood,
            hour,
            raining,
            user_boundaries,
            relationship_mode: "reserved",
            user_profile: &[],
        }
    }

    fn ctx_mode<'a>(mode: &'a str) -> PromptContext<'a> {
        PromptContext {
            mood: "平静",
            hour: 10,
            raining: false,
            user_boundaries: &[],
            relationship_mode: mode,
            user_profile: &[],
        }
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

    // ---- 用户画像段（plan-user-profile.md §5）----

    #[test]
    fn profile_section_renders_between_env_and_l1() {
        let c = card();
        let profile = [("称呼", "许一"), ("职业", "QA / 质量管理")];
        let pc = PromptContext {
            mood: "平静",
            hour: 10,
            raining: false,
            user_boundaries: &[],
            relationship_mode: "reserved",
            user_profile: &profile,
        };
        let out = render(&c, &pc);
        assert!(out.contains("# 用户画像\n称呼：许一\n职业：QA / 质量管理\n"));
        // 画像在宪法之前（先知道你是谁，再谈人格）。
        assert!(out.find("# 用户画像").unwrap() < out.find("# 人格宪法").unwrap());
    }

    #[test]
    fn empty_profile_skips_the_whole_section() {
        let c = card();
        let out = render(&c, &ctx("平静", 10, false, &[]));
        assert!(!out.contains("用户画像"), "全空不渲染——不产生提示泄漏");
    }

    #[test]
    fn profile_section_respects_the_200_char_budget() {
        let c = card();
        let long: Vec<(&str, &str)> = (0..20)
            .map(|i| (Box::leak(format!("字段{i}").into_boxed_str()) as &str, Box::leak("值".repeat(30).into_boxed_str()) as &str))
            .collect();
        let pc = PromptContext {
            mood: "平静",
            hour: 10,
            raining: false,
            user_boundaries: &[],
            relationship_mode: "reserved",
            user_profile: &long,
        };
        let out = render(&c, &pc);
        let start = out.find("# 用户画像").unwrap();
        let end = out.find("# 人格宪法").unwrap();
        let section: String = out[start..end].trim_end().to_string();
        assert!(
            section.chars().count() <= 201, // 200 预算 + 段首换行余量
            "画像段超预算：{} chars",
            section.chars().count()
        );
    }

    /// R2 会顶嘴 is a BEHAVIOR change, not a badge: the hedged openers the
    /// card already bans must appear as a hard rule in the prompt.
    #[test]
    fn direct_mode_spells_out_the_banned_hedges() {
        let c = card();
        let out = render(&c, &ctx_mode("direct"));
        assert!(out.contains("# 关系"));
        assert!(out.contains("不同意就直说"));
        for banned in &c.disagreement_policy.banned_forms {
            assert!(out.contains(banned), "banned form missing from prompt: {banned}");
        }
        // The other modes must not carry the hard ban.
        let reserved = render(&c, &ctx_mode("reserved"));
        assert!(!reserved.contains("一律禁止"));
        assert!(reserved.contains("客气一点"));
        let rusty = render(&c, &ctx_mode("rusty"));
        assert!(rusty.contains("生分"));
        assert!(!rusty.contains("一律禁止"));
        let familiar = render(&c, &ctx_mode("familiar"));
        assert!(familiar.contains("口语化"));
    }

    /// Relationship sits between expression (L2) and current state (L3):
    /// stabler than mood, more flexible than the constitution.
    #[test]
    fn relationship_block_sits_between_l2_and_l3() {
        let c = card();
        let out = render(&c, &ctx_mode("familiar"));
        let l2 = out.find("# 表达方式").unwrap();
        let rel = out.find("# 关系").unwrap();
        let l3 = out.find("# 当前状态").unwrap();
        assert!(l2 < rel && rel < l3);
    }
}

// The env_segment tests live at the bottom of the file to keep the render
// tests above untouched.
#[cfg(test)]
mod env_tests {
    use super::env_segment;

    #[test]
    fn env_segment_renders_pinned_instant() {
        use chrono::TimeZone;
        let now = chrono::FixedOffset::east_opt(8 * 3600)
            .unwrap()
            .with_ymd_and_hms(2026, 11, 20, 11, 45, 0)
            .unwrap()
            .with_timezone(&chrono::Local);
        let s = env_segment(now, "简体中文");
        assert!(s.starts_with("# 环境（系统注入，不可变）"));
        assert!(s.contains("2026-11-20 11:45（周五）"));
        assert!(s.contains("语言：简体中文"));
        assert!(s.contains("平台：macos") || s.contains("平台：windows") || s.contains("平台：linux"));
        assert!(s.contains("UTC+08:00"));
    }
}
