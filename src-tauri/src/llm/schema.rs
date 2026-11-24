//! 结构化输出 schema 注册表（plan-structured-routing §3.1）。
//!
//! grammar（传输层）与语义（Rust 白名单 / idx 越界）分工：本文件只登记
//! 「形状合法」的 JSON Schema；调用点继续做语义校验。一个消费点一个常量。

use std::sync::OnceLock;

use serde_json::Value;

/// 记忆抽取候选数组（`memory/extract::RawCandidate`）。
///
/// 与 `RawCandidate` 字段互锁：加字段先改两边再跑 `schema_matches_raw_candidate`。
pub fn extraction_facts() -> &'static Value {
    static SCHEMA: OnceLock<Value> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        serde_json::json!({
            "type": "array",
            "items": {
                "type": "object",
                "properties": {
                    "idx":         { "type": "integer", "minimum": 0 },
                    "fact":        { "type": "string" },
                    "category":    { "enum": ["like", "habit", "context", "boundary"] },
                    "confidence":  { "type": "number", "minimum": 0.0, "maximum": 1.0 },
                    "correction":  { "type": "boolean" },
                    "profile":     {
                        "type": ["object", "null"],
                        "properties": {
                            "key":   { "type": "string" },
                            "value": { "type": "string" }
                        }
                    }
                },
                "required": ["idx", "fact", "category"]
            }
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, serde::Deserialize)]
    #[allow(dead_code)]
    struct RawCandidate {
        idx: i64,
        fact: String,
        category: String,
        #[serde(default)]
        confidence: Option<f64>,
        #[serde(default)]
        correction: bool,
        #[serde(default)]
        profile: Option<RawProfile>,
    }

    #[derive(Debug, serde::Deserialize)]
    #[allow(dead_code)]
    struct RawProfile {
        key: String,
        value: String,
    }

    #[test]
    fn schema_matches_raw_candidate() {
        let sample = r#"[
          {"idx":0,"fact":"用户喜欢雨天","category":"like","confidence":0.9,"correction":false,"profile":null},
          {"idx":1,"fact":"用户叫阿盆","category":"context","profile":{"key":"name","value":"阿盆"}}
        ]"#;
        let parsed: Vec<RawCandidate> = serde_json::from_str(sample).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].category, "like");
        assert!(parsed[0].profile.is_none());
        assert_eq!(parsed[1].profile.as_ref().unwrap().key, "name");

        let schema = extraction_facts();
        assert_eq!(schema["type"], "array");
        let cats = schema["items"]["properties"]["category"]["enum"]
            .as_array()
            .expect("category enum");
        for c in ["like", "habit", "context", "boundary"] {
            assert!(cats.iter().any(|v| v.as_str() == Some(c)), "缺 {c}");
        }
        for req in ["idx", "fact", "category"] {
            assert!(
                schema["items"]["required"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|v| v.as_str() == Some(req)),
                "required 缺 {req}"
            );
        }
    }
}
