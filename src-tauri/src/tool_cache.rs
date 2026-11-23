//! Tool-result memoization for a single chat turn (plan-tool-cache v2).
//!
//! Lifecycle: the cache lives on the per-turn `AgentKit` (`build_agent_kit`
//! constructs a fresh kit per turn), so it is born and dies with one call —
//! no TTL, no LRU, no cross-turn staleness. Within the turn, the model may
//! repeat the same (tool, args) pair; the second execution is a pure waste,
//! and this module turns it into a hit. When the turn ends, the kit (and the
//! cache) is dropped.
//!
//! Rules (plan-tool-cache §2):
//! - Only `ToolAccess::Read` tools are cached. Ask/Forbidden tools never
//!   read or write the cache — the approval gate must see every attempt.
//! - Any non-Read execution success clears the whole cache: a write must
//!   never be followed by a stale read in the same turn.
//! - Errors are never cached.
//!
//! The key is `tool_name + args.to_string()`. serde_json's Map is a BTreeMap
//! (the crate does not enable `preserve_order`), so the serialization is
//! key-sorted: two model-generated arg objects differing only in key order
//! collapse to the same key (locked by a unit test below).

use std::collections::HashMap;
use std::sync::Mutex;

use crate::agent_tools::ToolOutput;

/// Canonical cache key. The serialization is canonical because serde_json
/// maps are BTreeMaps — see module docs.
pub fn cache_key(name: &str, args: &serde_json::Value) -> String {
    format!("{name}:{}", args.to_string())
}

/// Per-turn memoization store. A poisoned lock degrades to an empty cache
/// (miss) — memoization must never turn into a correctness hazard, mirroring
/// `ApprovalBroker::lock`'s recover-the-guard stance.
#[derive(Default)]
pub struct TurnToolCache {
    inner: Mutex<HashMap<String, ToolOutput>>,
}

impl TurnToolCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Memoized output for this (tool, args) pair, if any.
    pub fn get(&self, name: &str, args: &serde_json::Value) -> Option<ToolOutput> {
        let key = cache_key(name, args);
        let map = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        map.get(&key).cloned()
    }

    /// Record a successful Read-tool execution.
    pub fn put(&self, name: &str, args: &serde_json::Value, out: ToolOutput) {
        let key = cache_key(name, args);
        let mut map = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        map.insert(key, out);
    }

    /// Write-invalidation: drop everything after a non-Read tool succeeded,
    /// so a later read in the same turn sees fresh state.
    pub fn clear(&self) {
        let mut map = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        map.clear();
    }
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn out(summary: &str) -> ToolOutput {
        ToolOutput { summary: summary.into(), full: summary.into() }
    }

    #[test]
    fn key_collapses_arg_objects_that_differ_only_in_key_order() {
        // The model may emit the same args with keys in any order; the
        // BTreeMap-backed serialization must make them one key.
        let a = json!({"city": "杭州", "days": 3, "unit": "c"});
        let b = json!({"unit": "c", "days": 3, "city": "杭州"});
        let nested_a = json!({"query": {"from": "a", "to": "b"}, "k": 1});
        let nested_b = json!({"k": 1, "query": {"to": "b", "from": "a"}});
        assert_eq!(cache_key("weather", &a), cache_key("weather", &b));
        assert_eq!(cache_key("search", &nested_a), cache_key("search", &nested_b));
    }

    #[test]
    fn different_tool_or_args_are_different_keys() {
        let args = json!({"q": "预算"});
        assert_ne!(cache_key("memory_search", &args), cache_key("perception_search", &args));
        assert_ne!(
            cache_key("memory_search", &args),
            cache_key("memory_search", &json!({"q": "部门"}))
        );
    }

    #[test]
    fn put_then_get_roundtrips_and_other_entries_survive() {
        let cache = TurnToolCache::new();
        let a = json!({"q": "18号"});
        let b = json!({"q": "部门"});
        assert!(cache.get("memory_search", &a).is_none(), "cold miss");
        cache.put("memory_search", &a, out("hit-a"));
        cache.put("memory_search", &b, out("hit-b"));
        let got = cache.get("memory_search", &a).unwrap();
        assert_eq!(got.summary, "hit-a");
        assert_eq!(got.full, "hit-a");
        assert_eq!(cache.get("memory_search", &b).unwrap().summary, "hit-b");
    }

    #[test]
    fn clear_drops_everything() {
        let cache = TurnToolCache::new();
        cache.put("fs_read", &json!({"path": "/a"}), out("stale"));
        cache.put("web_fetch", &json!({"url": "https://x"}), out("stale2"));
        cache.clear();
        assert!(cache.get("fs_read", &json!({"path": "/a"})).is_none());
        assert!(cache.get("web_fetch", &json!({"url": "https://x"})).is_none());
    }

    #[test]
    fn poisoned_lock_recovers_without_false_hits() {
        let cache = std::sync::Arc::new(TurnToolCache::new());
        cache.put("fs_read", &json!({"path": "/a"}), out("v"));
        // Poison the mutex from a panicking holder.
        let poisoner = std::sync::Arc::clone(&cache);
        let _ = std::thread::spawn(move || {
            let _g = poisoner.inner.lock().unwrap();
            panic!("poison");
        })
        .join();
        // Recover-the-guard semantics (mirrors ApprovalBroker): pre-existing
        // entries stay exactly what they were — a hit is never fabricated.
        assert_eq!(cache.get("fs_read", &json!({"path": "/a"})).unwrap().summary, "v");
        assert!(cache.get("fs_read", &json!({"path": "/b"})).is_none());
        // And read/write keep working through the poisoned lock.
        cache.put("fs_read", &json!({"path": "/b"}), out("v2"));
        assert_eq!(cache.get("fs_read", &json!({"path": "/b"})).unwrap().summary, "v2");
    }
}
