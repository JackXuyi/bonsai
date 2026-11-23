//! 多会话（docs/zh/plan-context.md §5、§8）。
//!
//! 会话只是**对话窗口的索引**：切会话切的是「历史 + 摘要」，记忆
//! （facts / preferences / 用户画像）始终全局。所以这里没有独立的
//! 存储层——行与维护逻辑都在 `memory::db`，本模块只做 IPC 门面
//! （命令住在各自模块里，`lib.rs` 只登记）。
//!
//! 三条边界，别在这里破：
//! - `proactive`（主动搭话）永不建行、不进列表（D6）。
//! - 删除 = `db::forget_session`：连带清 trace / 任务 / 会话行，不是
//!   只把行标成已删（合规 Art. 16 的解释必须彻底）。
//! - 标题空串交给前端显示「新对话」；后端不发明中文占位串。

use tauri::State;

use crate::memory::{db, MemoryStore};

fn lock<'a>(
    state: &'a State<'_, MemoryStore>,
) -> Result<std::sync::MutexGuard<'a, rusqlite::Connection>, String> {
    state.inner().0.lock().map_err(|e| e.to_string())
}

/// 会话列表。`include_archived` 缺省 false（抽屉默认只看活的）。
#[tauri::command]
pub fn session_list(
    state: State<'_, MemoryStore>,
    include_archived: Option<bool>,
) -> Result<Vec<db::SessionRow>, String> {
    let conn = lock(&state)?;
    db::session_list(&conn, include_archived.unwrap_or(false)).map_err(|e| e.to_string())
}

/// 新建空会话。返回行本身（前端拿 id 立刻切换，不用再查一次）。
#[tauri::command]
pub fn session_create(
    state: State<'_, MemoryStore>,
    title: Option<String>,
) -> Result<db::SessionRow, String> {
    let conn = lock(&state)?;
    let id = crate::mcp_admin::uuid_v4();
    db::session_create(&conn, &id, title.as_deref().unwrap_or(""))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn session_rename(
    state: State<'_, MemoryStore>,
    id: String,
    title: String,
) -> Result<(), String> {
    let conn = lock(&state)?;
    db::session_rename(&conn, &id, &title)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn session_archive(
    state: State<'_, MemoryStore>,
    id: String,
    archived: bool,
) -> Result<(), String> {
    let conn = lock(&state)?;
    db::session_set_archived(&conn, &id, archived)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// 删除会话 = 合规意义上的「忘掉这段」：events / facts / trace / 任务 /
/// 会话行一起删。返回删掉的事件条数（前端用来确认「清掉了几条」）。
#[tauri::command]
pub fn session_delete(state: State<'_, MemoryStore>, id: String) -> Result<usize, String> {
    let conn = lock(&state)?;
    db::forget_session(&conn, &id).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        db::init_schema_for_test(&conn).unwrap();
        conn
    }

    #[test]
    fn list_hides_archived_and_sorts_by_activity() {
        let conn = mem();
        db::session_create(&conn, "a", "早上").unwrap();
        db::session_create(&conn, "b", "下午").unwrap();
        db::session_touch(&conn, "a", Some("刚说了一句")).unwrap();
        let rows = db::session_list(&conn, false).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "a", "最近活跃在前");
        assert_eq!(rows[0].message_count, 1);

        db::session_set_archived(&conn, "a", true).unwrap();
        let live = db::session_list(&conn, false).unwrap();
        assert_eq!(live.len(), 1);
        assert_eq!(live[0].id, "b");
        assert_eq!(db::session_list(&conn, true).unwrap().len(), 2);
    }

    #[test]
    fn touch_names_once_then_keeps_the_users_rename() {
        let conn = mem();
        db::session_touch(&conn, "s1", Some("帮我看下黄山\n天气")).unwrap();
        let row = db::session_get(&conn, "s1").unwrap().unwrap();
        assert_eq!(row.title, "帮我看下黄山 天气", "换行折成空格");
        assert_eq!(row.message_count, 1);

        // 第二条用户消息不改名，只累加。
        db::session_touch(&conn, "s1", Some("再查一下明天")).unwrap();
        let row = db::session_get(&conn, "s1").unwrap().unwrap();
        assert_eq!(row.title, "帮我看下黄山 天气");
        assert_eq!(row.message_count, 2);

        // 手动改名后，后续自动命名不得覆盖。
        db::session_rename(&conn, "s1", "黄山行程").unwrap();
        db::session_touch(&conn, "s1", Some("还有别的吗")).unwrap();
        assert_eq!(db::session_get(&conn, "s1").unwrap().unwrap().title, "黄山行程");
    }

    #[test]
    fn blank_first_message_leaves_the_title_open() {
        let conn = mem();
        db::session_touch(&conn, "s1", Some("   \n ")).unwrap();
        assert_eq!(db::session_get(&conn, "s1").unwrap().unwrap().title, "");
        // 下一条有内容的消息补上名字。
        db::session_touch(&conn, "s1", Some("正经问题")).unwrap();
        assert_eq!(db::session_get(&conn, "s1").unwrap().unwrap().title, "正经问题");
    }

    #[test]
    fn auto_title_is_clipped_to_24_chars() {
        let conn = mem();
        let long = "很".repeat(80);
        db::session_touch(&conn, "s1", Some(&long)).unwrap();
        let title = db::session_get(&conn, "s1").unwrap().unwrap().title;
        assert_eq!(title.chars().count(), 24);
    }
}
