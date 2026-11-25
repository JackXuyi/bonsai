//! Agent managed state (MCP pool + skills dir + session allow).

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::mcp::McpManager;

use super::approval::ApprovalBroker;

pub struct AgentState {
    pub manager: McpManager,
    pub skills_dir: PathBuf,
    /// `approval_mode=ask` handshake slots (spec-s4). Separate from the DB
    /// on purpose: the chat turn holds the store mutex while it waits.
    pub approvals: Arc<ApprovalBroker>,
    /// 本会话「同工具批量授权」白名单（plan-chat-agent-inline v2）。
    pub session_allow: Arc<SessionToolAllow>,
    /// `None` in unit tests → ask mode denies instead of waiting forever.
    pub app: Option<tauri::AppHandle>,
}

/// 按会话记住用户点过「本会话允许此工具」的工具名；换会话清空。
#[derive(Default)]
pub struct SessionToolAllow {
    inner: Mutex<SessionAllowInner>,
}

#[derive(Default)]
struct SessionAllowInner {
    session_id: String,
    tools: HashSet<String>,
}

impl SessionToolAllow {
    fn lock(&self) -> std::sync::MutexGuard<'_, SessionAllowInner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// 对话命令进入时绑定会话；换会话则清空白名单。
    pub fn bind_session(&self, session_id: &str) {
        let mut g = self.lock();
        if g.session_id != session_id {
            g.session_id = session_id.to_string();
            g.tools.clear();
        }
    }

    pub fn allows(&self, tool: &str) -> bool {
        self.lock().tools.contains(tool)
    }

    pub fn grant(&self, tool: &str) {
        if tool.is_empty() {
            return;
        }
        self.lock().tools.insert(tool.to_string());
    }
}
