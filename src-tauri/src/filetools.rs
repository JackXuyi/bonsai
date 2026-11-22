//! Built-in file tools (P3-2, docs/zh/plan-files.md, roadmap G-05).
//!
//! One provider, four tools, all scoped to directories the user explicitly
//! added in the tools pane (`tools.fs.roots` settings key, default empty =
//! not mounted at all). The safety story rests on ONE kernel —
//! [`resolve_in_roots`] — that every tool call must pass before any I/O:
//!
//! 1. absolute paths only (the model must list/search first, never guess);
//! 2. canonicalize (expands symlinks — an in-root link pointing at
//!    `/etc/passwd` resolves OUTSIDE and is rejected by the prefix check);
//! 3. component-wise prefix match, not `starts_with` on strings (root
//!    `/a/b` must not accept `/a/b-evil`);
//! 4. write-target filenames reject separators, `.`/`..` and control chars
//!    (deviation from plan §3.2's allowlist: the original `[A-Za-z0-9._\- ]`
//!    would ban CJK filenames; the denylist below blocks the actual escape
//!    vectors, and the prefix check above is the real boundary).
//!
//! Deleting / moving / renaming has NO execution branch anywhere in this
//! file — `ToolAccess::Forbidden` as a shape, not a runtime check.

use std::path::{Component, Path, PathBuf};

// ---------------------------------------------------------------- errors

#[derive(Debug, thiserror::Error)]
#[error("{code}: {message}")]
pub struct FileError {
    pub code: &'static str,
    pub message: String,
}

impl FileError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
    pub fn relative() -> Self {
        Self::new("fs_relative_path", "路径必须是绝对路径（先用 fs_search/fs_list 拿到真实路径）")
    }
    pub fn outside() -> Self {
        Self::new("fs_outside_roots", "路径不在授权目录内")
    }
    pub fn root_missing() -> Self {
        Self::new("fs_root_missing", "授权目录已不存在，请到工具页移除后重新添加")
    }
    pub fn too_large(limit: usize) -> Self {
        Self::new("fs_too_large", format!("文件超过 {limit} 字节上限"))
    }
    pub fn binary(ext: &str) -> Self {
        Self::new("fs_binary_type", format!("不支持读取 .{ext} 这类二进制文件"))
    }
    pub fn not_found() -> Self {
        Self::new("fs_not_found", "文件不存在（先用 fs_search/fs_list 确认路径）")
    }
    pub fn exists() -> Self {
        Self::new("fs_exists", "文件已存在，fs_write 只新建、不覆盖")
    }
    pub fn bad_filename(name: &str) -> Self {
        Self::new("fs_bad_filename", format!("非法文件名：{name}"))
    }
    pub fn io(e: std::io::Error) -> Self {
        Self::new("fs_io", e.to_string())
    }
}

// ---------------------------------------------------------------- path kernel

/// Canonicalize the longest *existing* prefix of `path` and re-attach the
/// missing tail. Used for write targets whose final component does not
/// exist yet (their parent must exist — we never create nested dirs).
fn canonicalize_longest(path: &Path) -> Result<PathBuf, FileError> {
    if let Ok(c) = path.canonicalize() {
        return Ok(c);
    }
    let file_name = path.file_name().ok_or_else(FileError::relative)?;
    let parent = path.parent().ok_or_else(FileError::relative)?;
    let canon_parent = parent
        .canonicalize()
        .map_err(|_| FileError::not_found())?;
    Ok(canon_parent.join(file_name))
}

/// Component-wise containment: does `target` live inside `root` (or equal
/// it)? Component comparison, never string prefixes — `/a/b-evil` is NOT
/// inside root `/a/b`.
fn within_root(target: &Path, root: &Path) -> bool {
    let t: Vec<Component> = target.components().collect();
    let r: Vec<Component> = root.components().collect();
    t.len() >= r.len() && t[..r.len()] == r[..]
}

/// Filename denylist (any path component): no separators (impossible inside
/// a component), no `.`/`..`, no control characters. Everything else (CJK,
/// spaces, dashes) passes; the real boundary is the prefix check.
fn filename_ok(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', '\0'])
        && !name.chars().any(char::is_control)
}

/// The one gate every file tool call passes. Returns the matched root index
/// and the fully resolved (canonical) path. Non-existent targets are
/// allowed through — callers decide whether that is an error for their
/// operation (`fs_read` → not_found; `fs_write` → fine).
pub fn resolve_in_roots(roots: &[PathBuf], raw: &str) -> Result<(usize, PathBuf), FileError> {
    if raw.is_empty() || raw.contains('\0') {
        return Err(FileError::bad_filename(raw));
    }
    let p = Path::new(raw);
    if !p.is_absolute() {
        return Err(FileError::relative());
    }
    // Denylist over EVERY component (not just the final name): `../x.md`
    // must die here, before any canonicalize gives it a legal-looking home.
    for comp in p.components() {
        match comp {
            Component::Normal(name) => {
                let name = name.to_string_lossy();
                if !filename_ok(&name) {
                    return Err(FileError::bad_filename(&name));
                }
            }
            Component::CurDir | Component::ParentDir => {
                return Err(FileError::bad_filename(&comp.as_os_str().to_string_lossy()))
            }
            Component::RootDir | Component::Prefix(_) => {}
        }
    }
    // Roots are validated first: a deleted root must surface as
    // fs_root_missing, never leak into the target's error.
    let canon_roots = roots
        .iter()
        .map(|root| {
            if !root.exists() {
                return Err(FileError::root_missing());
            }
            root.canonicalize().map_err(|_| FileError::root_missing())
        })
        .collect::<Result<Vec<PathBuf>, FileError>>()?;
    let canon = canonicalize_longest(p)?;
    for (idx, canon_root) in canon_roots.iter().enumerate() {
        if within_root(&canon, canon_root) {
            return Ok((idx, canon));
        }
    }
    Err(FileError::outside())
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bonsai-filetools-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn err_code(r: Result<(usize, PathBuf), FileError>) -> &'static str {
        r.unwrap_err().code
    }

    #[test]
    fn relative_and_empty_paths_are_rejected() {
        let root = temp_root("rel");
        let roots = vec![root.clone()];
        assert_eq!(err_code(resolve_in_roots(&roots, "")), "fs_bad_filename");
        assert_eq!(err_code(resolve_in_roots(&roots, "notes.md")), "fs_relative_path");
        assert_eq!(err_code(resolve_in_roots(&roots, "./notes.md")), "fs_relative_path");
    }

    #[test]
    fn dotdot_escape_is_rejected() {
        let root = temp_root("dotdot");
        let inside = root.join("inside.md");
        fs::write(&inside, "hi").unwrap();
        let roots = vec![root.clone()];
        let escape = format!("{}/../../secrets.txt", root.display());
        // `..` 组件在 denylist 就地拦截（比 canonicalize 更早，语义更直白）
        assert_eq!(err_code(resolve_in_roots(&roots, &escape)), "fs_bad_filename");
        // the legit file passes
        assert!(resolve_in_roots(&roots, &inside.display().to_string()).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_rejected() {
        let root = temp_root("symlink");
        let outside = temp_root("symlink-outside");
        let secret = outside.join("secret.txt");
        fs::write(&secret, "top secret").unwrap();
        let link = root.join("innocent.txt");
        std::os::unix::fs::symlink(&secret, &link).unwrap();
        let roots = vec![root.clone()];
        assert_eq!(
            err_code(resolve_in_roots(&roots, &link.display().to_string())),
            "fs_outside_roots"
        );
    }

    #[test]
    fn sibling_prefix_lookalike_is_rejected() {
        // root /a/b must NOT accept /a/b-evil — string starts_with would.
        let base = temp_root("prefix");
        let root = base.join("b");
        let evil = base.join("b-evil");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&evil).unwrap();
        let evil_file = evil.join("x.txt");
        fs::write(&evil_file, "nope").unwrap();
        let roots = vec![root.clone()];
        assert_eq!(
            err_code(resolve_in_roots(&roots, &evil_file.display().to_string())),
            "fs_outside_roots"
        );
    }

    #[test]
    fn write_target_bad_filenames_are_rejected() {
        let root = temp_root("wname");
        let roots = vec![root.clone()];
        // 注：`.` 会被 std components() 规范化掉（等价 root 自身，界内无害），
        // 不在拒绝之列；`..`（ParentDir）保留且必须拒绝。
        for name in ["../evil.txt", "a\\b.txt", "..", "na\0me"] {
            let target = root.join(name);
            assert_eq!(
                err_code(resolve_in_roots(&roots, &target.display().to_string())),
                "fs_bad_filename",
                "name: {name}"
            );
        }
        // 嵌套目录不存在 → not_found（我们不创建嵌套目录），不是坏文件名
        let nested = root.join("a/b.txt");
        assert_eq!(err_code(resolve_in_roots(&roots, &nested.display().to_string())), "fs_not_found");
        // CJK + spaces pass the denylist (deviation from plan allowlist)
        let ok = root.join("会议纪要 2026.md");
        assert!(resolve_in_roots(&roots, &ok.display().to_string()).is_ok());
    }

    #[test]
    fn missing_root_is_reported_not_silently_skipped() {
        let root = temp_root("gone");
        let inside = root.join("a.md");
        fs::write(&inside, "x").unwrap();
        let roots = vec![root.clone()];
        fs::remove_dir_all(&root).unwrap();
        assert_eq!(
            err_code(resolve_in_roots(&roots, &inside.display().to_string())),
            "fs_root_missing"
        );
    }

    #[test]
    fn non_existent_write_target_resolves_through_existing_parent() {
        let root = temp_root("newfile");
        let target = root.join("会议纪要 2026.md");
        let roots = vec![root.clone()];
        let (idx, canon) = resolve_in_roots(&roots, &target.display().to_string()).unwrap();
        assert_eq!(idx, 0);
        assert!(!canon.exists());
        assert_eq!(canon.file_name().unwrap(), "会议纪要 2026.md");
    }

    #[test]
    fn root_itself_is_inside() {
        let root = temp_root("rootself");
        let roots = vec![root.clone()];
        assert!(resolve_in_roots(&roots, &root.display().to_string()).is_ok());
    }
}
