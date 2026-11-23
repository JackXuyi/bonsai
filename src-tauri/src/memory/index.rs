//! 检索索引层（S2，2026-11-23）：自建 2-gram 分词 + FTS5 外部内容索引。
//!
//! ## 为什么不是 trigram
//!
//! 原计划是「FTS5 trigram 替换 LIKE 全扫」。实测证否（`cargo test` 里的
//! `bundled_sqlite_has_fts5_and_trigram_cannot_do_two_chars` 把这个结论
//! 钉住了）：**trigram 对少于 3 个字符的查询返回 0 行**——`报销`/`窗户`/
//! `咖啡` 全部零命中，`报销流` 才命中。而中文的核心检索单位恰恰是 2 字词
//! （`memory_search` 的腿也几乎全是 2 字）。照原计划直换，会把 S1 修好的
//! 召回重新打回 0。
//!
//! 所以索引侧自己做分词：正文切成「空格分隔的 token 串」存进普通列，用
//! unicode61（按空格切）建 FTS5 索引。**索引侧与查询侧共用同一个
//! `tokens()`**——只要两边对称，2 字词、ASCII 词都能对上。
//!
//! ## 与 `memory_search::bigrams_of` 的关系（AGENTS.md 硬约束 1）
//!
//! 两者不是同一件事，不能合并：LIKE 腿必须是**原文里真实存在的子串**
//! （否则 SQL 永远匹配不到），所以 `bigrams_of` 只能取原文相邻切片；
//! FTS token 只需**与查询侧对称**（分词损失由两侧共同承担）。一个是
//! 子串约束，一个是等价类约束——判据不同，实现不能是同一份。
//!
//! ## 维护方式
//!
//! - 新增：水位线增量同步（`search_sync.last_id`）。插入需要 Rust 分词，
//!   触发器做不到，所以走 `sync()`——每次检索前跑一遍，正常情况下只做
//!   4 条 `id > ?` 的空查询。
//! - 删除：`events` / `facts` / `preferences` / `boundaries` 上的 AFTER
//!   DELETE 触发器级联删 `search_docs`（再级联到 FTS）。用户「忘掉」必须
//!   连索引一起忘，不能靠应用层记得调。
//! - 更新：正文从不原地改写（invalidate 只打时间戳，不改 content），因此
//!   不需要 UPDATE 触发器；失效率由查询侧按 `invalid_at` 打折（与 v1 一致）。

use std::collections::HashSet;

use rusqlite::{params, Connection};

/// 每次同步的批大小。上限存在是为了让首次 backfill 不长时间占住写锁。
const SYNC_CHUNK: usize = 500;

/// 单篇文档最多留多少个 token。正文可以很长（一整段用户消息），
/// 而 2-gram 数量随长度线性增长；512 足够覆盖一次检索的区分度需求。
const MAX_DOC_TOKENS: usize = 512;

/// 能进 token 的字符：Unicode 字母/数字。全角标点（`，` `。` `「`）、
/// 空白、ASCII 标点都是分词边界——用 `is_alphanumeric` 而不是
/// `is_ascii_punctuation`，否则全角标点会被当成实字塞进 token。
///
/// 注意：这里**不套** `memory_search::STOP_CHARS`（「的/了/我」那 22 个）。
/// 那份表服务的是 LIKE 腿（腿必须是原文子串，停用字切碎后腿就废了），
/// 本模块只要求「两侧对称」，停用字留在 token 里只是让 bm25 多几个高频
/// 低权重的词，不会造成漏召回。
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric()
}

/// 正文 → token 列表。索引与查询共用；对称性是唯一的正确性判据。
///
/// - ASCII 段：长度 ≥2 整段保留并小写（`topic01` 这种标识符要能被整词
///   命中），单字母丢弃（只会污染索引）；
/// - 非 ASCII 段（中文）：整段留一份 + 所有相邻二字切片
///   （`报销流程` → 整段 + 报销/销流/流程）。整段留是为了让「原话命中」
///   在 bm25 里分数更高。
pub(crate) fn tokens(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut run = String::new();
    let mut run_ascii = false;

    for c in text.chars() {
        if is_word_char(c) {
            let ascii = c.is_ascii();
            if !run.is_empty() && ascii != run_ascii {
                flush(&mut run, run_ascii, &mut out, &mut seen);
            }
            run_ascii = ascii;
            run.push(c);
        } else {
            flush(&mut run, run_ascii, &mut out, &mut seen);
        }
        if out.len() >= MAX_DOC_TOKENS {
            break;
        }
    }
    flush(&mut run, run_ascii, &mut out, &mut seen);
    out.truncate(MAX_DOC_TOKENS);
    out
}

fn flush(run: &mut String, ascii: bool, out: &mut Vec<String>, seen: &mut HashSet<String>) {
    if run.is_empty() {
        return;
    }
    if ascii {
        if run.chars().count() >= 2 {
            push(out, seen, run.to_lowercase());
        }
    } else {
        push(out, seen, run.clone());
        let chars: Vec<char> = run.chars().collect();
        for pair in chars.windows(2) {
            push(out, seen, pair.iter().collect());
        }
    }
    run.clear();
}

fn push(out: &mut Vec<String>, seen: &mut HashSet<String>, tok: String) {
    if !tok.is_empty() && seen.insert(tok.clone()) {
        out.push(tok);
    }
}

/// 这种文本能不能被索引/检索命中（全标点或空串 = 不能）。
/// 调用方（`memory_search::beyond_window`）用它区分「有内容词」与
/// 「纯指代」两条策略，避免把「还记得吗」当查询词去 MATCH。
pub(crate) fn has_tokens(text: &str) -> bool {
    !tokens(text).is_empty()
}

/// 索引列的写法：空格分隔。空 token 集返回空串（这种文档不进索引，
/// 因为它永远不可能被查询命中）。
fn index_body(text: &str) -> String {
    tokens(text).join(" ")
}

/// FTS5 查询表达式：token 之间 OR，每个带前缀通配（`tok*`）。
///
/// 前缀对 2 字中文是恒等操作（token 本来就 2 字），对 ASCII 则补回
/// 「输 `coff` 找 `coffee`」这种 LIKE 时代的直觉。token 只含字母数字
/// （`is_word_char` 保证），不含 FTS5 语法字符，因此无需引号转义。
fn match_expr(query: &str) -> Option<String> {
    let toks = tokens(query);
    if toks.is_empty() {
        return None;
    }
    Some(
        toks.iter()
            .map(|t| format!("{t}*"))
            .collect::<Vec<_>>()
            .join(" OR "),
    )
}

/// (kind, 增量取新行的 SQL)。SQL 的 ?1 = 水位线 id，?2 = 批大小。
///
/// events 不做 kind 过滤：`db::search`（记忆页搜索）历史上就覆盖全部事件，
/// 收窄成 dialogue 会让「工具调用 / 感知事件」搜不到——那是行为回退，不是
/// 优化。对话召回（memory_search）自己在渲染侧按 kind 过滤。
fn sources() -> [(&'static str, &'static str); 4] {
    [
        (
            "event",
            "SELECT id, ts, content FROM events WHERE id > ?1 ORDER BY id LIMIT ?2",
        ),
        (
            "fact",
            "SELECT id, ts, content FROM facts WHERE id > ?1 ORDER BY id LIMIT ?2",
        ),
        (
            "preference",
            "SELECT id, ts, content FROM preferences WHERE id > ?1 ORDER BY id LIMIT ?2",
        ),
        (
            "boundary",
            "SELECT id, ts, content FROM boundaries WHERE id > ?1 ORDER BY id LIMIT ?2",
        ),
    ]
}

/// 把水位线之后的新行灌进索引。幂等，可在任意检索前调用。
///
/// 整体包在 `SAVEPOINT` 里：首次 backfill 有上万行，逐条自动提交等于
/// 上万次 fsync（秒级到十秒级启动卡顿）；SAVEPOINT 的好处是即使调用方
/// 已经在事务里也不会 `cannot start a transaction within a transaction`。
pub(crate) fn sync(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch("SAVEPOINT search_index_sync")?;
    let result = sync_inner(conn);
    match result {
        Ok(()) => conn.execute_batch("RELEASE search_index_sync"),
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK TO search_index_sync; RELEASE search_index_sync;");
            Err(e)
        }
    }
}

fn sync_inner(conn: &Connection) -> rusqlite::Result<()> {
    for (kind, sql) in sources() {
        let mut last: i64 = conn
            .query_row(
                "SELECT last_id FROM search_sync WHERE kind = ?1",
                params![kind],
                |r| r.get(0),
            )
            .unwrap_or(0);
        loop {
            let mut stmt = conn.prepare(sql)?;
            let rows: Vec<(i64, String, String)> = stmt
                .query_map(params![last, SYNC_CHUNK as i64], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            if rows.is_empty() {
                break;
            }
            let full = rows.len() == SYNC_CHUNK;
            for (id, ts, content) in &rows {
                let body = index_body(content);
                if body.is_empty() {
                    continue;
                }
                conn.execute(
                    "INSERT OR IGNORE INTO search_docs (kind, ref_id, ts, body, bigrams)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![kind, id, ts, content, body],
                )?;
            }
            last = rows.last().map(|r| r.0).unwrap_or(last);
            conn.execute(
                "INSERT INTO search_sync (kind, last_id) VALUES (?1, ?2)
                 ON CONFLICT(kind) DO UPDATE SET last_id = excluded.last_id",
                params![kind, last],
            )?;
            if !full {
                break;
            }
        }
    }
    Ok(())
}

/// 索引命中（`(kind, ref_id)`），按 bm25 相关性排序。
/// 最终排序由调用方按自己的打分公式（decay / importance）决定。
pub(crate) fn search(
    conn: &Connection,
    query: &str,
    limit: usize,
) -> rusqlite::Result<Vec<(String, i64)>> {
    let Some(expr) = match_expr(query) else {
        return Ok(Vec::new());
    };
    let mut stmt = conn.prepare(
        "SELECT d.kind, d.ref_id
           FROM (SELECT rowid AS rid, bm25(search_fts) AS score FROM search_fts
                  WHERE search_fts MATCH ?1 ORDER BY score LIMIT ?2) m
           JOIN search_docs d ON d.id = m.rid
          ORDER BY m.score",
    )?;
    let rows = stmt.query_map(params![expr, limit as i64], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
    })?;
    rows.collect()
}

/// 索引与源表的漂移量（诊断 / 测试用）：每个 kind 的「源行数 − 索引行数」。
#[cfg(test)]
pub(crate) fn drift(conn: &Connection) -> rusqlite::Result<Vec<(String, i64)>> {
    let mut out = Vec::new();
    for (kind, _) in sources() {
        let src = match kind {
            "event" => "SELECT COUNT(*) FROM events",
            "fact" => "SELECT COUNT(*) FROM facts",
            "preference" => "SELECT COUNT(*) FROM preferences",
            _ => "SELECT COUNT(*) FROM boundaries",
        };
        let total: i64 = conn.query_row(src, [], |r| r.get(0))?;
        let indexed: i64 = conn.query_row(
            "SELECT COUNT(*) FROM search_docs WHERE kind = ?1",
            params![kind],
            |r| r.get(0),
        )?;
        out.push((kind.to_string(), total - indexed));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(super::super::db::SCHEMA).unwrap();
        conn
    }

    /// bundled SQLite 必须带 FTS5——这是 S2 的编译期前提，也是「为什么
    /// 不用 trigram」那条结论的复现证据。
    #[test]
    fn bundled_sqlite_has_fts5_and_trigram_cannot_do_two_chars() {
        let conn = mem();
        conn.execute_batch(
            "CREATE VIRTUAL TABLE probe USING fts5(content, tokenize='trigram');
             INSERT INTO probe(content) VALUES ('我在走报销流程，还没走完');",
        )
        .unwrap();
        let n = |q: &str| -> i64 {
            conn.query_row(
                "SELECT COUNT(*) FROM probe WHERE probe MATCH ?1",
                params![q],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(
            n("报销"),
            0,
            "trigram 对 2 字中文零命中：这就是不用它的原因"
        );
        assert_eq!(n("报销流"), 1);
    }

    #[test]
    fn tokens_cover_two_char_words_and_identifiers() {
        let doc = index_body("所有权登记要等两周");
        let q = match_expr("所有权").unwrap();
        assert!(doc.contains("所有"), "doc={doc}");
        assert!(q.contains("所有*"), "q={q}");
        // ASCII 标识符整词保留，供 eval 的 topic01 这类查询使用
        assert!(index_body("topic01 是关于报销的").contains("topic01"));
        // 全角标点是边界，不能进 token
        assert!(!index_body("报销，流程").contains('，'));
        assert_eq!(
            match_expr("  ，。").map(|_| ()),
            None,
            "无 token 时不该生成 MATCH"
        );
    }

    #[test]
    fn two_char_cjk_query_hits() {
        let conn = mem();
        conn.execute_batch(
            "INSERT INTO events (ts, kind, content, role, session_id)
             VALUES ('2026-11-01T00:00:00+00:00', 'dialogue', '我在走报销流程，还没走完', 'user', 's1');",
        )
        .unwrap();
        sync(&conn).unwrap();
        assert_eq!(
            search(&conn, "报销", 10).unwrap(),
            vec![("event".to_string(), 1)]
        );
        assert!(search(&conn, "咖啡", 10).unwrap().is_empty());
    }

    #[test]
    fn sync_is_incremental_and_delete_cascades() {
        let conn = mem();
        conn.execute(
            "INSERT INTO facts (ts, content) VALUES ('2026-11-01T00:00:00+00:00', '阳台装窗户的测量结果')",
            [],
        )
        .unwrap();
        sync(&conn).unwrap();
        sync(&conn).unwrap(); // 空转幂等
        assert!(drift(&conn).unwrap().iter().all(|(_, d)| *d == 0));

        // 用户「忘掉」→ 源行删除 → 触发器级联删索引 → FTS 不再命中
        let id: i64 = conn
            .query_row("SELECT id FROM facts", [], |r| r.get(0))
            .unwrap();
        conn.execute("DELETE FROM facts WHERE id = ?1", params![id])
            .unwrap();
        assert!(
            search(&conn, "窗户", 10).unwrap().is_empty(),
            "忘掉之后索引里不能还留着"
        );
        assert_eq!(
            drift(&conn)
                .unwrap()
                .iter()
                .find(|(k, _)| k == "fact")
                .unwrap()
                .1,
            0
        );
    }

    #[test]
    fn ranking_prefers_more_matched_tokens() {
        let conn = mem();
        conn.execute_batch(
            "INSERT INTO facts (ts, content) VALUES
               ('2026-11-01T00:00:00+00:00', '报销流程走完了'),
               ('2026-11-01T00:00:00+00:00', '报销的额度是三千，流程要看财务'),
               ('2026-11-01T00:00:00+00:00', '今天天气不错');",
        )
        .unwrap();
        sync(&conn).unwrap();
        let hits = search(&conn, "报销流程", 10).unwrap();
        assert_eq!(hits.len(), 2, "只沾「报销」的那条也要在内：{hits:?}");
        assert_eq!(hits[0].1, 1, "全 token 命中的排前面：{hits:?}");
    }

    /// 成本量尺（不常驻 CI）：`sync()` 每次检索前都会跑，必须知道「已经
    /// 同步过」时的空转开销，以及 2 万条事件（约一年）首次 backfill 的代价。
    /// 跑法：`cargo test --lib backfill_cost -- --ignored --nocapture`
    #[test]
    #[ignore = "perf probe：手动跑，记录量级"]
    fn backfill_cost_probe() {
        use std::time::Instant;
        let conn = mem();
        conn.execute_batch("BEGIN").unwrap();
        for i in 0..20_000 {
            conn.execute(
                "INSERT INTO events (ts, kind, content, role, session_id)
                 VALUES ('2026-11-01T00:00:00+00:00', 'dialogue', ?1, 'user', 's')",
                params![format!(
                    "第{i}条：我在走报销流程，先找直属主管审批再去财务系统，顺便说下阳台封窗的尺寸"
                )],
            )
            .unwrap();
        }
        conn.execute_batch("COMMIT").unwrap();

        let t = Instant::now();
        sync(&conn).unwrap();
        let backfill = t.elapsed();

        let t = Instant::now();
        for _ in 0..20 {
            sync(&conn).unwrap();
        }
        let idle = t.elapsed() / 20;

        let t = Instant::now();
        let hits = search(&conn, "报销流程", 16).unwrap();
        let query = t.elapsed();

        println!(
            "2 万事件：backfill {:?} / 已同步空转 {:?} / 检索 {:?}（{} 命中）",
            backfill,
            idle,
            query,
            hits.len()
        );
        assert_eq!(
            drift(&conn).unwrap().iter().map(|(_, d)| *d).sum::<i64>(),
            0
        );
    }
}
