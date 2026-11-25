//! Model output validation and prompt build.

use crate::skill;

use super::evidence::Evidence;
use super::BODY_HARD_MAX;

#[derive(Debug, Clone)]
pub struct Generated {
    pub name: String,
    pub description: String,
    pub body: String,
}

/// 结构质量门槛（§4.4 / 测试 5–6）。
pub fn validate_generated(raw: &str) -> Result<Generated, &'static str> {
    let parsed = parse_model_output(raw).ok_or("parse_failed")?;
    if parsed.name.chars().count() == 0 || parsed.name.chars().count() > 40 {
        return Err("bad_name");
    }
    if parsed.description.chars().count() == 0 || parsed.description.chars().count() > 120 {
        return Err("bad_description");
    }
    if parsed.body.chars().count() > BODY_HARD_MAX {
        return Err("body_too_long");
    }
    for h in ["适用场景", "步骤", "注意"] {
        if !parsed.body.contains(h) {
            return Err("missing_section");
        }
    }
    if skill::check_injection(&parsed.body).is_err()
        || skill::check_injection(&parsed.name).is_err()
        || skill::check_injection(&parsed.description).is_err()
    {
        return Err("injection");
    }
    Ok(parsed)
}

fn parse_model_output(raw: &str) -> Option<Generated> {
    let text = raw.trim();
    // NAME: / DESC: / --- body
    let mut name = None;
    let mut description = None;
    let mut body_start = None;
    for (i, line) in text.lines().enumerate() {
        let t = line.trim();
        if let Some(v) = t.strip_prefix("NAME:") {
            name = Some(v.trim().to_string());
        } else if let Some(v) = t.strip_prefix("DESC:") {
            description = Some(v.trim().to_string());
        } else if t == "---" {
            body_start = Some(i + 1);
            break;
        }
    }
    let body = if let Some(idx) = body_start {
        text.lines().skip(idx).collect::<Vec<_>>().join("\n").trim().to_string()
    } else {
        // fallback: whole text as body if headings present
        text.to_string()
    };
    let name = name.or_else(|| {
        // try first heading line
        text.lines().next().map(|s| s.trim().trim_start_matches('#').trim().to_string())
    })?;
    let description = description.unwrap_or_else(|| {
        "自动提取的可复用流程。".into()
    });
    if body.is_empty() {
        return None;
    }
    Some(Generated {
        name: name.chars().take(40).collect(),
        description: description.chars().take(120).collect(),
        body,
    })
}

fn slug_from(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else if c.is_ascii_whitespace() || c == '-' || c == '_' {
                '-'
            } else {
                // 非 ascii：用占位，后面用 task 前缀保证合法
                '-'
            }
        })
        .collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    let s = s.trim_matches('-').chars().take(24).collect::<String>();
    if s.chars().any(|c| c.is_ascii_alphanumeric()) {
        s
    } else {
        "flow".into()
    }
}

pub fn make_auto_id(name: &str, task_id: &str) -> String {
    let slug = slug_from(name);
    let suffix: String = task_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(6)
        .collect::<String>()
        .to_ascii_lowercase();
    let id = format!("auto-{slug}-{suffix}");
    let id: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    id.trim_matches('-').to_string()
}

pub(crate) fn build_prompt(ev: &Evidence) -> String {
    format!(
        "你是技能包起草助手。根据下面的执行轨迹，写一份可复用的流程说明。\n\
         只输出以下格式，不要其它前言：\n\
         NAME: <≤40字动宾短语>\n\
         DESC: <≤120字，写清何时用、产出什么>\n\
         ---\n\
         ## 适用场景\n\
         ...\n\
         ## 步骤\n\
         ...\n\
         ## 注意\n\
         ...\n\
         禁止：第一人称人格声明、危机词表、指令覆盖、引用其它技能正文。\n\
         body 目标 400–2000 字，硬上限 4096 字。\n\n\
         【目标】{}\n\
         【最终回复】{}\n\
         【用户原话】\n{}\n\
         【工具摘要】\n{}\n\
         【其它回复】\n{}\n",
        ev.goal,
        ev.final_reply,
        ev.user_texts.join("\n"),
        ev.tool_summaries.join("\n"),
        ev.reply_texts.join("\n"),
    )
}
