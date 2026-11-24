//! MCP server 自动发现（spec-s2 §6）：扫描本机各客户端的 MCP 配置文件 +
//! 通用 manifest sweep，把候选以「默认停用」导入 mcp_servers。
//!
//! 边界（方案讨论结论）：stdio 服务没有任何配置就无法发现——命令 + 魔法
//! 参数（如 `prune mcp`）从二进制推断不出来；端口探测 / mDNS 不做。
//! 导入永远不改 enabled（RK-04），启用仍走手动 toggle + connect 验证。
//!
//! 解析与文件 IO 分离：`parse_config_text` / `dedup_views` /
//! `import_with` 是纯函数，单测直接覆盖；`scan_with` 只加一层 read_dir。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde::Serialize;

use crate::agent_tools::mask_secrets;
use crate::mcp_admin::{insert_server, list_raw, row_signature};

/// One candidate found on disk. Plaintext lives only here and in the DB —
/// the serialized view masks env/headers like every admin surface.
#[derive(Debug, Clone)]
pub(crate) struct DiscoveredServer {
    pub name: String,
    pub transport: String, // "stdio" | "http"
    pub command: Option<String>,
    pub args: Vec<String>,
    pub url: Option<String>,
    pub headers: Vec<(String, String)>,
    pub env: Vec<(String, String)>,
    /// "cursor" / "claude-code" / … / "manifest:<dir-name>"
    pub source: String,
}

impl DiscoveredServer {
    /// Dedup key, byte-identical to `mcp_admin::row_signature` so existing
    /// rows compare equal to freshly parsed candidates.
    pub fn signature(&self) -> String {
        match self.transport.as_str() {
            "http" => format!("http|{}", self.url.clone().unwrap_or_default()),
            _ => format!(
                "stdio|{}|{}",
                self.command.clone().unwrap_or_default(),
                self.args.join("\u{1f}")
            ),
        }
    }
}

/// Serialized candidate for the frontend (secrets masked).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredView {
    pub name: String,
    pub transport: String,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub url: Option<String>,
    pub headers: Vec<(String, String)>,
    pub env: Vec<(String, String)>,
    pub source: String,
    pub signature: String,
    pub imported: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpImportReport {
    pub inserted: usize,
    /// Requested but already in the DB (double import / raced with another save).
    pub skipped: usize,
}

// ---------------------------------------------------------------- parsing

/// One client config file → candidates. Accepts both `mcpServers` (Cursor /
/// Claude / Windsurf) and `servers` (VS Code's mcp.json). Unparseable file
/// or malformed entries are skipped — one bad config never aborts a scan.
pub(crate) fn parse_config_text(text: &str, source: &str, home: &str) -> Vec<DiscoveredServer> {
    let Ok(root) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    let Some(map) = root
        .get("mcpServers")
        .or_else(|| root.get("servers"))
        .and_then(|v| v.as_object())
    else {
        return Vec::new();
    };
    map.iter()
        .filter_map(|(name, cfg)| parse_entry(name, cfg, source, home))
        .collect()
}

fn parse_entry(name: &str, cfg: &serde_json::Value, source: &str, home: &str) -> Option<DiscoveredServer> {
    let obj = cfg.as_object()?;
    let url = obj
        .get("url")
        .and_then(|v| v.as_str())
        .map(|s| expand_str(s, home))
        .filter(|u| u.starts_with("http://") || u.starts_with("https://"));
    let command = obj
        .get("command")
        .and_then(|v| v.as_str())
        .map(|s| expand_str(s, home))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    if let Some(url) = url {
        Some(DiscoveredServer {
            name: name.to_string(),
            transport: "http".into(),
            command: None,
            args: Vec::new(),
            url: Some(url),
            headers: kv_from(obj.get("headers"), home),
            env: Vec::new(),
            source: source.to_string(),
        })
    } else if let Some(command) = command {
        let args = obj
            .get("args")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(|s| expand_str(s, home)))
                    .collect()
            })
            .unwrap_or_default();
        Some(DiscoveredServer {
            name: name.to_string(),
            transport: "stdio".into(),
            command: Some(command),
            args,
            url: None,
            headers: Vec::new(),
            env: kv_from(obj.get("env"), home),
            source: source.to_string(),
        })
    } else {
        None // no usable transport fields → not an MCP server entry
    }
}

/// `{"KEY": "str"}` → pairs. Non-string scalars (numbers in env blocks) are
/// stringified; objects/arrays inside a kv value are dropped.
fn kv_from(v: Option<&serde_json::Value>, home: &str) -> Vec<(String, String)> {
    v.and_then(|v| v.as_object())
        .map(|m| {
            m.iter()
                .filter_map(|(k, val)| {
                    let s = match val {
                        serde_json::Value::String(s) => expand_str(s, home),
                        serde_json::Value::Number(n) => n.to_string(),
                        serde_json::Value::Bool(b) => b.to_string(),
                        _ => return None,
                    };
                    Some((k.clone(), s))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// VS Code-style `${env:NAME}` / plain `${NAME}`; a missing var stays
/// literal (visible in the UI instead of silently emptying a path).
/// A leading `~/` expands to the home directory.
pub(crate) fn expand_str(s: &str, home: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            if let Some(end_rel) = s[i + 2..].find('}') {
                let inner = &s[i + 2..i + 2 + end_rel];
                let var = inner.strip_prefix("env:").unwrap_or(inner);
                out.push_str(&std::env::var(var).unwrap_or_else(|_| s[i..i + end_rel + 3].to_string()));
                i += end_rel + 3;
                continue;
            }
        }
        out.push(s[i..].chars().next().unwrap());
        i += s[i..].chars().next().unwrap().len_utf8();
    }
    if let Some(rest) = out.strip_prefix("~/") {
        return format!("{home}/{rest}");
    }
    out
}

// ---------------------------------------------------------------- sources

/// Known MCP client config locations (macOS first, matches the project's
/// platform plan). Path order = display priority.
fn known_sources(home: &Path) -> Vec<(&'static str, PathBuf)> {
    vec![
        ("cursor", home.join(".cursor/mcp.json")),
        ("claude-desktop", home.join("Library/Application Support/Claude/claude_desktop_config.json")),
        ("claude-code", home.join(".claude.json")),
        ("vscode", home.join("Library/Application Support/Code/User/mcp.json")),
        ("vscode-insiders", home.join("Library/Application Support/Code - Insiders/User/mcp.json")),
        ("windsurf", home.join(".codeium/windsurf/mcp_config.json")),
    ]
}

/// Directory cap for the generic sweep — a filesystem walk must stay bounded
/// (calendar lesson: hard bounds everywhere).
const SWEEP_DIR_CAP: usize = 200;

/// Generic manifest sweep: `~/Library/Application Support/*/mcp*.json` and
/// `~/.config/*/mcp*.json` — apps that self-declare without being in a known
/// client. One level deep only; dirs already covered by known sources are
/// skipped so a server never reports two different sources.
fn manifest_sweep(home: &Path, covered: &HashSet<PathBuf>) -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let roots = [home.join("Library/Application Support"), home.join(".config")];
    for root in roots {
        let Ok(apps) = std::fs::read_dir(&root) else { continue };
        for app in apps.flatten().take(SWEEP_DIR_CAP) {
            let dir = app.path();
            if !dir.is_dir() || covered.contains(&dir) {
                continue;
            }
            let Ok(files) = std::fs::read_dir(&dir) else { continue };
            for f in files.flatten() {
                let name = f.file_name().to_string_lossy().to_lowercase();
                if name.starts_with("mcp") && name.ends_with(".json") {
                    let label = dir
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    out.push((format!("manifest:{label}"), f.path()));
                    break; // one manifest per app dir
                }
            }
        }
    }
    out
}

/// Full disk scan: known sources first, then the sweep. Same signature from
/// two files appears once — the first occurrence wins (i.e. the better-
/// labeled known source over a bare manifest hit).
pub(crate) fn scan_with(home: &Path) -> Vec<DiscoveredServer> {
    let home_str = home.to_string_lossy().to_string();
    let mut out: Vec<DiscoveredServer> = Vec::new();
    let mut seen = HashSet::new();
    let mut covered: HashSet<PathBuf> = HashSet::new();
    for (source, path) in known_sources(home) {
        if let Some(parent) = path.parent() {
            covered.insert(parent.to_path_buf());
        }
        if let Ok(text) = std::fs::read_to_string(&path) {
            for s in parse_config_text(&text, source, &home_str) {
                if seen.insert(s.signature()) {
                    out.push(s);
                }
            }
        }
    }
    for (source, path) in manifest_sweep(home, &covered) {
        if let Ok(text) = std::fs::read_to_string(&path) {
            for s in parse_config_text(&text, &source, &home_str) {
                if seen.insert(s.signature()) {
                    out.push(s);
                }
            }
        }
    }
    out
}

/// Dedup within the scan + flag what's already in `mcp_servers`.
pub(crate) fn dedup_views(
    servers: Vec<DiscoveredServer>,
    existing: &HashSet<String>,
) -> Vec<DiscoveredView> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for s in servers {
        let sig = s.signature();
        if !seen.insert(sig.clone()) {
            continue;
        }
        out.push(DiscoveredView {
            name: s.name,
            transport: s.transport,
            command: s.command,
            args: s.args,
            url: s.url,
            headers: s
                .headers
                .into_iter()
                .map(|(k, v)| (k, mask_secrets(&v)))
                .collect(),
            env: s.env.into_iter().map(|(k, v)| (k, mask_secrets(&v))).collect(),
            source: s.source,
            signature: sig.clone(),
            imported: existing.contains(&sig),
        });
    }
    out
}

pub(crate) fn discover_candidates(conn: &Connection) -> Result<Vec<DiscoveredView>, String> {
    let home = std::env::var("HOME").map_err(|_| "no_home".to_string())?;
    let existing: HashSet<String> = list_raw(conn)
        .map_err(|e| e.to_string())?
        .iter()
        .map(row_signature)
        .collect();
    Ok(dedup_views(scan_with(Path::new(&home)), &existing))
}

/// Import core: re-resolve candidates from disk by signature (the frontend
/// never passes env/headers back — plaintext secrets cross no IPC boundary),
/// then insert each as a disabled row (RK-04).
pub(crate) fn import_with(
    conn: &Connection,
    home: &Path,
    signatures: &[String],
) -> Result<McpImportReport, String> {
    let want: HashSet<&str> = signatures.iter().map(|s| s.as_str()).collect();
    let existing: HashSet<String> = list_raw(conn)
        .map_err(|e| e.to_string())?
        .iter()
        .map(row_signature)
        .collect();
    let mut inserted = 0usize;
    let mut skipped = 0usize;
    let mut reported = HashSet::new();
    for s in scan_with(home) {
        let sig = s.signature();
        if !want.contains(sig.as_str()) || !reported.insert(sig.clone()) {
            continue;
        }
        if existing.contains(&sig) {
            skipped += 1;
            continue;
        }
        insert_server(
            conn,
            &s.name,
            &s.transport,
            s.command.as_deref(),
            &s.args,
            s.url.as_deref(),
            &s.headers,
            &s.env,
        )?;
        inserted += 1;
    }
    Ok(McpImportReport { inserted, skipped })
}

// ---------------------------------------------------------------- commands

#[tauri::command]
pub fn mcp_discover_scan(
    store: tauri::State<'_, crate::memory::MemoryStore>,
) -> Result<Vec<DiscoveredView>, String> {
    let conn = store.inner().write_guard().map_err(|e| e.to_string())?;
    discover_candidates(&conn)
}

#[tauri::command]
pub fn mcp_discover_import(
    store: tauri::State<'_, crate::memory::MemoryStore>,
    signatures: Vec<String>,
) -> Result<McpImportReport, String> {
    let home = std::env::var("HOME").map_err(|_| "no_home".to_string())?;
    let conn = store.inner().write_guard().map_err(|e| e.to_string())?;
    import_with(&conn, Path::new(&home), &signatures)
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    fn home_of(dir: &Path) -> String {
        dir.to_string_lossy().to_string()
    }

    #[test]
    fn parse_cursor_style_and_http_entries() {
        let text = r#"{
          "mcpServers": {
            "prune": { "command": "/Users/x/.local/bin/prune", "args": ["mcp"], "env": {"PATH": "/usr/bin"} },
            "followup": { "url": "https://api.followup.us/api/v1/mcp", "headers": {"Authorization": "Bearer abc"} },
            "obs": { "type": "command", "command": "npx", "args": ["mcp-observatory"] }
          }
        }"#;
        let got = parse_config_text(text, "cursor", "/Users/x");
        assert_eq!(got.len(), 3);
        let prune = got.iter().find(|s| s.name == "prune").unwrap();
        assert_eq!(prune.transport, "stdio");
        assert_eq!(prune.command.as_deref(), Some("/Users/x/.local/bin/prune"));
        assert_eq!(prune.args, vec!["mcp"]);
        assert_eq!(prune.env, vec![("PATH".into(), "/usr/bin".into())]);
        assert_eq!(prune.source, "cursor");

        let http = got.iter().find(|s| s.name == "followup").unwrap();
        assert_eq!(http.transport, "http");
        assert_eq!(http.url.as_deref(), Some("https://api.followup.us/api/v1/mcp"));
        assert_eq!(http.headers, vec![("Authorization".into(), "Bearer abc".into())]);

        // `type: "command"` is ignored — command presence decides.
        assert_eq!(got.iter().find(|s| s.name == "obs").unwrap().transport, "stdio");
    }

    #[test]
    fn parse_skips_invalid_entries_and_bad_files() {
        let text = r#"{
          "mcpServers": {
            "no-fields": { "note": "nothing usable" },
            "bad-url": { "url": "ftp://x" },
            "empty-command": { "command": "  " },
            "ok": { "command": "/bin/echo" }
          }
        }"#;
        let got = parse_config_text(text, "cursor", "/Users/x");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "ok");

        assert!(parse_config_text("not json at all", "cursor", "/Users/x").is_empty());
        assert!(parse_config_text(r#"{"mcpServers": 3}"#, "cursor", "/Users/x").is_empty());
        // VS Code's "servers" key works too.
        assert_eq!(
            parse_config_text(r#"{"servers": {"a": {"command": "x"}}}"#, "vscode", "/Users/x").len(),
            1
        );
    }

    #[test]
    fn env_and_path_expansion() {
        std::env::set_var("BONSAI_DISCOVER_TEST", "/opt/tools");
        let text = r#"{"mcpServers": {"a": {
            "command": "${env:BONSAI_DISCOVER_TEST}/run",
            "args": ["~/docs"],
            "env": {"HOME_DIR": "${HOME}", "PORT": 3000}
        }}}"#;
        let got = parse_config_text(text, "vscode", "/Users/x");
        let a = &got[0];
        assert_eq!(a.command.as_deref(), Some("/opt/tools/run"));
        assert_eq!(a.args, vec!["/Users/x/docs"]);
        let env: std::collections::HashMap<&str, &str> =
            a.env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        assert_eq!(env["PORT"], "3000");
        assert_eq!(env["HOME_DIR"], std::env::var("HOME").unwrap());
        // Missing var stays literal — visible, not silently emptied.
        let text2 = r#"{"mcpServers": {"b": {"command": "/bin/b", "args": ["${env:NOPE_XYZ}"]}}}"#;
        assert_eq!(parse_config_text(text2, "cursor", "/h")[0].args, vec!["${env:NOPE_XYZ}"]);
    }

    #[test]
    fn signature_matches_row_signature() {
        // The imported-vs-existing comparison relies on the two signature
        // functions agreeing byte-for-byte.
        let conn = crate::mcp_admin::tests::mem_conn();
        let id = insert_server(
            &conn,
            "本地",
            "stdio",
            Some("/Users/x/.local/bin/prune"),
            &["mcp".to_string()],
            None,
            &[],
            &[("PATH".into(), "/usr/bin".into())],
        )
        .unwrap();
        let row = list_raw(&conn).unwrap().into_iter().find(|r| r.id == id).unwrap();
        let cand = DiscoveredServer {
            name: "prune".into(),
            transport: "stdio".into(),
            command: Some("/Users/x/.local/bin/prune".into()),
            args: vec!["mcp".into()],
            url: None,
            headers: vec![],
            env: vec![("PATH".into(), "/usr/bin".into())],
            source: "cursor".into(),
        };
        assert_eq!(cand.signature(), row_signature(&row));

        let id2 = insert_server(
            &conn,
            "远程",
            "http",
            None,
            &[],
            Some("https://api.followup.us/api/v1/mcp"),
            &[],
            &[],
        )
        .unwrap();
        let row2 = list_raw(&conn).unwrap().into_iter().find(|r| r.id == id2).unwrap();
        let cand2 = DiscoveredServer {
            name: "followup".into(),
            transport: "http".into(),
            command: None,
            args: vec![],
            url: Some("https://api.followup.us/api/v1/mcp".into()),
            headers: vec![],
            env: vec![],
            source: "cursor".into(),
        };
        assert_eq!(cand2.signature(), row_signature(&row2));
    }

    fn write(dir: &Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    #[test]
    fn scan_reads_known_sources_and_sweep_with_dedup() {
        let root = std::env::temp_dir().join(format!("bonsai-discover-{}-{}", std::process::id(), crate::mcp_admin::tests::next_seq()));
        std::fs::create_dir_all(&root).unwrap();
        let home = home_of(&root);
        write(&root, ".cursor/mcp.json", r#"{"mcpServers": {"prune": {"command": "/bin/prune", "args": ["mcp"]}}}"#);
        // Same server self-declared via a manifest → dedup keeps the known source.
        write(&root, "Library/Application Support/prune-app/mcp.json", r#"{"mcpServers": {"prune": {"command": "/bin/prune", "args": ["mcp"]}}}"#);
        // A different manifest server is picked up by the sweep.
        write(&root, "Library/Application Support/toolbox/mcp.json", r#"{"servers": {"box": {"command": "/bin/box"}}}"#);
        // A stray json that isn't an MCP config is ignored.
        write(&root, "Library/Application Support/noisy/mcp-notes.json", r#"{"hello": 1}"#);

        let got = scan_with(&root);
        assert_eq!(got.len(), 2, "dedup within scan: {got:?}");
        let prune = got.iter().find(|s| s.name == "prune").unwrap();
        assert_eq!(prune.source, "cursor", "known source wins the label");
        let box_ = got.iter().find(|s| s.name == "box").unwrap();
        assert_eq!(box_.source, "manifest:toolbox");

        let existing: HashSet<String> = got.iter().map(|s| s.signature()).take(1).collect();
        let views = dedup_views(got, &existing);
        assert_eq!(views[0].imported, true);
        assert_eq!(views[1].imported, false);
        let _ = home;
    }

    #[test]
    fn import_inserts_disabled_and_skips_duplicates() {
        let conn = crate::mcp_admin::tests::mem_conn();
        let root = std::env::temp_dir().join(format!("bonsai-import-{}-{}", std::process::id(), crate::mcp_admin::tests::next_seq()));
        std::fs::create_dir_all(&root).unwrap();
        write(&root, ".cursor/mcp.json", r#"{"mcpServers": {"prune": {"command": "/bin/prune", "args": ["mcp"], "env": {"TOKEN": "sk-secret42"}}}}"#);
        let candidates = scan_with(&root);
        let sigs: Vec<String> = candidates.iter().map(|s| s.signature()).collect();

        let report = import_with(&conn, &root, &sigs).unwrap();
        assert_eq!(report.inserted, 1);
        assert_eq!(report.skipped, 0);

        let rows = list_raw(&conn).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].enabled, "import never enables (RK-04)");
        assert_eq!(rows[0].command.as_deref(), Some("/bin/prune"));
        // Plaintext env lands in the DB; the view path masks it.
        assert!(rows[0].env_json.contains("sk-secret42"));
        let view = crate::mcp_admin::tests::view_for_test(&rows[0], &conn);
        let rendered = serde_json::to_string(&view).unwrap();
        assert!(!rendered.contains("sk-secret42"));

        // Importing the same signature again → skipped, no second row.
        let report2 = import_with(&conn, &root, &sigs).unwrap();
        assert_eq!(report2.inserted, 0);
        assert_eq!(report2.skipped, 1);
        assert_eq!(list_raw(&conn).unwrap().len(), 1);

        // Unknown signatures are simply not found → nothing happens.
        let report3 = import_with(&conn, &root, &["stdio|ghost|".to_string()]).unwrap();
        assert_eq!(report3.inserted, 0);
        assert_eq!(report3.skipped, 0);
    }
}
