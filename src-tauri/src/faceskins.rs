// 表情自定义皮肤（plan-face-custom-skins.md）：用户为 7 个表情槽位上传
// 图片，组成一套本地皮肤。纯外观资源——图片永不进记忆表、永不进 prompt、
// 永不发云端。
//
// 存储布局：app_data_dir()/face_skins/<skin_id>/manifest.json + <slot>.<ext>。
// 安全规则：
// - skin_id 由本模块生成（`c-` + 8 位 alnum），用户输入只进 name，
//   永不进路径；所有文件操作前对 id 做白名单校验 + canonicalize 界内校验。
// - 图片格式以 magic bytes 为准（不信扩展名），单文件 ≤ 2MB。
// - 所有失败返回 `face_<reason>: 中文说明` 结构化错误，绝不静默成功。
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// 表情槽位 = FACE_STATES（frontend/src/lib/types.ts），顺序即 UI 展示序。
pub const SLOTS: [&str; 7] = [
    "calm",
    "focused",
    "sleepy",
    "greeted",
    "displeased",
    "thinking",
    "proud",
];

pub const MAX_SKINS: usize = 10;
pub const MAX_IMAGE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct FaceSkinManifest {
    pub id: String,
    pub name: String,
    pub created_at: u64,
    /// slot -> 文件名（仅已上传的槽位；缺槽 = 无键）。
    #[serde(default)]
    pub slots: BTreeMap<String, String>,
}

// ---------------------------------------------------------------- paths

pub fn skins_dir(base: &Path) -> PathBuf {
    base.join("face_skins")
}

/// skin_id 白名单：`c-` + 8 位小写字母数字。用户输入永不进路径的第一道闸。
pub fn validate_skin_id(id: &str) -> Result<(), String> {
    let ok = id.len() == 10
        && id.starts_with("c-")
        && id[2..]
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    if ok {
        Ok(())
    } else {
        Err("face_bad_id:皮肤标识不合法".into())
    }
}

/// 解析皮肤目录：id 白名单 + canonicalize 界内校验（防符号链接逃逸）。
pub fn resolve_skin_dir(base: &Path, id: &str) -> Result<PathBuf, String> {
    validate_skin_id(id)?;
    let dir = skins_dir(base).join(id);
    let canon = dir
        .canonicalize()
        .map_err(|_| "face_skin_unknown:皮肤不存在".to_string())?;
    let base_canon = skins_dir(base)
        .canonicalize()
        .map_err(|_| "face_skin_unknown:皮肤不存在".to_string())?;
    if !canon.starts_with(&base_canon) {
        return Err("face_bad_id:皮肤标识不合法".into());
    }
    Ok(canon)
}

// ---------------------------------------------------------------- manifest

fn read_manifest(dir: &Path) -> Result<FaceSkinManifest, String> {
    let raw = fs::read_to_string(dir.join("manifest.json"))
        .map_err(|_| "face_manifest_bad:皮肤元数据读取失败".to_string())?;
    serde_json::from_str(&raw).map_err(|_| "face_manifest_bad:皮肤元数据损坏".to_string())
}

fn write_manifest(dir: &Path, m: &FaceSkinManifest) -> Result<(), String> {
    let json = serde_json::to_string_pretty(m).map_err(|e| e.to_string())?;
    fs::write(dir.join("manifest.json"), json).map_err(|e| e.to_string())
}

/// 列出全部皮肤：坏目录/坏 manifest 容错跳过（不因单个损坏皮肤全灭），
/// 按 created_at 升序。
pub fn list_inner(base: &Path) -> Result<Vec<FaceSkinManifest>, String> {
    let root = skins_dir(base);
    let mut out = Vec::new();
    let entries = match fs::read_dir(&root) {
        Ok(e) => e,
        Err(_) => return Ok(out), // 尚无任何自定义皮肤
    };
    for entry in entries.flatten() {
        if entry.path().is_dir() {
            if let Ok(m) = read_manifest(&entry.path()) {
                out.push(m);
            }
        }
    }
    out.sort_by(|a, b| a.created_at.cmp(&b.created_at).then_with(|| a.id.cmp(&b.id)));
    Ok(out)
}

// ---------------------------------------------------------------- id

/// `c-` + 8 位 base36，熵源 = 纳秒时间 + 进程 id（本地单用户应用足够）。
pub fn new_skin_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut n = (nanos as u64) ^ ((std::process::id() as u64) << 32);
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut id = String::from("c-");
    for _ in 0..8 {
        id.push(ALPHABET[(n % 36) as usize] as char);
        n = n.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    }
    id
}

fn validate_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 20 {
        return Err("face_bad_name:名称需为 1–20 个字符".into());
    }
    Ok(trimmed.to_string())
}

// ---------------------------------------------------------------- commands' inner

pub fn create_inner(base: &Path, name: &str) -> Result<FaceSkinManifest, String> {
    let name = validate_name(name)?;
    let existing = list_inner(base)?;
    if existing.len() >= MAX_SKINS {
        return Err(format!("face_skins_max:最多 {MAX_SKINS} 套自定义皮肤"));
    }
    let dir = skins_dir(base).join(new_skin_id());
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let m = FaceSkinManifest {
        id: dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string(),
        name,
        created_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
        slots: BTreeMap::new(),
    };
    write_manifest(&dir, &m)?;
    Ok(m)
}

/// magic bytes 判格式（不信扩展名）。返回归一化扩展名。
pub fn detect_image_kind(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() >= 4 && bytes[..4] == [0x89, b'P', b'N', b'G'] {
        return Some("png");
    }
    if bytes.len() >= 3 && bytes[..3] == [0xFF, 0xD8, 0xFF] {
        return Some("jpg");
    }
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("webp");
    }
    None
}

pub fn import_inner(base: &Path, skin_id: &str, slot: &str, src: &Path) -> Result<FaceSkinManifest, String> {
    if !SLOTS.contains(&slot) {
        return Err("face_bad_slot:表情槽位不合法".into());
    }
    let dir = resolve_skin_dir(base, skin_id)?;
    let mut m = read_manifest(&dir)?;

    // 大小前置拒绝（不读入内存再拒）
    let meta = fs::metadata(src).map_err(|_| "face_no_file:文件不可读".to_string())?;
    if meta.len() > MAX_IMAGE_BYTES {
        return Err(format!("face_too_large:图片需 ≤ {} MB", MAX_IMAGE_BYTES / 1024 / 1024));
    }
    let bytes = fs::read(src).map_err(|_| "face_no_file:文件不可读".to_string())?;
    let ext = detect_image_kind(&bytes)
        .ok_or("face_bad_image:仅支持 PNG / JPEG / WebP 图片（以文件内容为准，改名无效）")?;

    // 新文件落盘 `<slot>.<ext>`；旧槽位文件扩展名不同则一并清掉
    let filename = format!("{slot}.{ext}");
    fs::write(dir.join(&filename), &bytes).map_err(|e| e.to_string())?;
    if let Some(old) = m.slots.get(slot) {
        if old != &filename {
            let _ = fs::remove_file(dir.join(old));
        }
    }
    m.slots.insert(slot.to_string(), filename);
    write_manifest(&dir, &m)?;
    Ok(m)
}

pub fn clear_slot_inner(base: &Path, skin_id: &str, slot: &str) -> Result<FaceSkinManifest, String> {
    if !SLOTS.contains(&slot) {
        return Err("face_bad_slot:表情槽位不合法".into());
    }
    let dir = resolve_skin_dir(base, skin_id)?;
    let mut m = read_manifest(&dir)?;
    if let Some(filename) = m.slots.remove(slot) {
        let _ = fs::remove_file(dir.join(filename));
        write_manifest(&dir, &m)?;
    }
    Ok(m)
}

pub fn delete_inner(base: &Path, skin_id: &str) -> Result<(), String> {
    let dir = resolve_skin_dir(base, skin_id)?;
    fs::remove_dir_all(dir).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------- tauri commands

fn skins_base(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    use tauri::Manager;
    app.path()
        .app_data_dir()
        .map_err(|e| e.to_string())
        .map(|p| p.join("face_skins"))
}

#[tauri::command]
pub async fn face_skin_list(app: tauri::AppHandle) -> Result<Vec<FaceSkinManifest>, String> {
    let base = tauri::async_runtime::spawn_blocking(move || {
        let base = skins_base(&app)?;
        list_inner(&base)
    })
    .await
    .map_err(|e| e.to_string())?;
    base
}

#[tauri::command]
pub async fn face_skin_create(app: tauri::AppHandle, name: String) -> Result<FaceSkinManifest, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let base = skins_base(&app)?;
        create_inner(&base, &name)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn face_skin_pick_and_import(
    app: tauri::AppHandle,
    skin_id: String,
    slot: String,
) -> Result<FaceSkinManifest, String> {
    // rfd 文件选择器（字节不过 IPC，对齐 fs_pick_directory 先例）。
    let picked = rfd::AsyncFileDialog::new()
        .set_title(format!("选择「{slot}」表情图片"))
        .add_filter("图片", &["png", "jpg", "jpeg", "webp"])
        .pick_file()
        .await;
    let Some(handle) = picked else {
        return Err("face_pick_cancelled:已取消选择".into());
    };
    let path = handle.path().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        let base = skins_base(&app)?;
        import_inner(&base, &skin_id, &slot, &path)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn face_skin_clear_slot(
    app: tauri::AppHandle,
    skin_id: String,
    slot: String,
) -> Result<FaceSkinManifest, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let base = skins_base(&app)?;
        clear_slot_inner(&base, &skin_id, &slot)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn face_skin_delete(app: tauri::AppHandle, skin_id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let base = skins_base(&app)?;
        delete_inner(&base, &skin_id)
    })
    .await
    .map_err(|e| e.to_string())?
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_base(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bonsai-faceskins-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn png_bytes() -> Vec<u8> {
        let mut v = vec![0x89u8, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        v.extend(std::iter::repeat(0u8).take(64));
        v
    }

    #[test]
    fn skin_id_whitelist() {
        assert!(validate_skin_id("c-k3f9q2ab").is_ok());
        for bad in [
            "",
            "c-K3F9Q2AB",     // 大写
            "c-k3f9q2a",      // 8 位不够
            "c-k3f9q2abc",    // 超长
            "x-k3f9q2ab",     // 前缀
            "c-k3f9_q2a",     // 非法字符
            "../evil0000",
            "c-../../..",
        ] {
            assert!(validate_skin_id(bad).is_err(), "id: {bad}");
        }
    }

    #[test]
    fn resolve_rejects_unknown_and_outside() {
        let base = temp_base("resolve");
        assert!(resolve_skin_dir(&base, "c-aaaaaaaa").is_err());
        // 合法形状但目录不存在 → unknown
        fs::create_dir_all(skins_dir(&base)).unwrap();
        assert!(resolve_skin_dir(&base, "c-aaaaaaaa").is_err());
    }

    #[test]
    fn create_and_list_roundtrip() {
        let base = temp_base("roundtrip");
        let m = create_inner(&base, "  我的猫  ").unwrap();
        assert_eq!(m.name, "我的猫");
        assert!(validate_skin_id(&m.id).is_ok());
        assert!(m.slots.is_empty());

        let m2 = create_inner(&base, "第二套").unwrap();
        let all = list_inner(&base).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].id, m.id); // created_at 升序
        assert_eq!(all[1].id, m2.id);

        // 名称校验
        assert!(create_inner(&base, "").is_err());
        assert!(create_inner(&base, "  ").is_err());
        assert!(create_inner(&base, &"长".repeat(21)).is_err());
        // 上限
        for i in 0..8 {
            create_inner(&base, &format!("皮肤{i}")).unwrap();
        }
        assert!(create_inner(&base, "第十套").is_err());
    }

    #[test]
    fn detect_image_kind_by_magic_not_extension() {
        assert_eq!(detect_image_kind(&png_bytes()), Some("png"));
        let mut jpg = vec![0xFF, 0xD8, 0xFF, 0xE0];
        jpg.extend(std::iter::repeat(0u8).take(32));
        assert_eq!(detect_image_kind(&jpg), Some("jpg"));
        let mut webp = b"RIFFxxxxWEBPVP8 ".to_vec();
        webp.extend(std::iter::repeat(0u8).take(16));
        assert_eq!(detect_image_kind(&webp), Some("webp"));
        // 假 PNG（扩展名改的 JPEG/文本）→ 拒绝
        assert_eq!(detect_image_kind(b"GIF89a......"), None);
        assert_eq!(detect_image_kind(b"<html>...."), None);
    }

    #[test]
    fn import_slot_lifecycle() {
        let base = temp_base("import");
        let m = create_inner(&base, "测试").unwrap();

        // 非法槽位
        assert!(import_inner(&base, &m.id, "angry", &PathBuf::from("x")).is_err());

        // 真 PNG
        let png = base.join("src.png");
        fs::write(&png, png_bytes()).unwrap();
        let m = import_inner(&base, &m.id, "calm", &png).unwrap();
        assert_eq!(m.slots.get("calm").unwrap(), "calm.png");
        assert!(skins_dir(&base).join(&m.id).join("calm.png").exists());

        // 同槽位换格式：旧文件清掉
        let mut jpg = vec![0xFF, 0xD8, 0xFF, 0xE0];
        jpg.extend(std::iter::repeat(0u8).take(32));
        let jpg_path = base.join("src.jpg");
        fs::write(&jpg_path, jpg).unwrap();
        let m = import_inner(&base, &m.id, "calm", &jpg_path).unwrap();
        assert_eq!(m.slots.get("calm").unwrap(), "calm.jpg");
        assert!(!skins_dir(&base).join(&m.id).join("calm.png").exists());

        // 假 PNG（文本改名 .png）→ face_bad_image
        let fake = base.join("fake.png");
        fs::write(&fake, b"not an image at all..........").unwrap();
        let err = import_inner(&base, &m.id, "proud", &fake).unwrap_err();
        assert!(err.starts_with("face_bad_image"), "err: {err}");

        // 超限
        let big = base.join("big.png");
        fs::write(&big, [0x89u8, b'P', b'N', b'G'].into_iter().chain(std::iter::repeat(0u8).take(64)).collect::<Vec<u8>>()).unwrap();
        // 正常大小通过后再测超限：写一个 >2MB 的文件
        let big = base.join("big2.png");
        let mut content = png_bytes();
        content.extend(std::iter::repeat(0u8).take((2 * 1024 * 1024 + 1) as usize));
        fs::write(&big, content).unwrap();
        let err = import_inner(&base, &m.id, "proud", &big).unwrap_err();
        assert!(err.starts_with("face_too_large"), "err: {err}");

        // clear_slot：文件删除 + manifest 置空
        let m = clear_slot_inner(&base, &m.id, "calm").unwrap();
        assert!(!m.slots.contains_key("calm"));
        assert!(!skins_dir(&base).join(&m.id).join("calm.jpg").exists());
        // 再 clear 已空槽位：幂等
        clear_slot_inner(&base, &m.id, "calm").unwrap();
    }

    #[test]
    fn delete_removes_dir() {
        let base = temp_base("delete");
        let m = create_inner(&base, "删除我").unwrap();
        delete_inner(&base, &m.id).unwrap();
        assert!(!skins_dir(&base).join(&m.id).exists());
        assert!(list_inner(&base).unwrap().is_empty());
        // 删不存在 → unknown
        assert!(delete_inner(&base, &m.id).is_err());
    }

    #[test]
    fn list_tolerates_corrupt_manifest() {
        let base = temp_base("corrupt");
        let m = create_inner(&base, "好的").unwrap();
        // 造一个坏目录
        let bad = skins_dir(&base).join("c-bbbbbbbb");
        fs::create_dir_all(&bad).unwrap();
        fs::write(bad.join("manifest.json"), "not json").unwrap();
        let all = list_inner(&base).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, m.id);
    }
}
