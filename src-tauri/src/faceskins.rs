// 表情自定义皮肤（plan-face-custom-skins.md）：用户为 7 个表情槽位上传
// 图片，组成一套本地皮肤。纯外观资源——图片永不进记忆表、永不进 prompt、
// 永不发云端。
//
// 存储布局：app_data_dir()/face_skins/<skin_id>/manifest.json + <slot>.<ext>。
// 安全规则：
// - skin_id 由本模块生成（`c-` + 8 位 alnum），用户输入只进 name，
//   永不进路径；所有文件操作前对 id 做白名单校验 + canonicalize 界内校验。
// - 图片格式以 magic bytes 为准（不信扩展名），单文件 ≤ 2MB。
// - 像素规格（对齐默认大肥鱼素材 1024×1024）：必须正方 1:1，
//   边长 ∈ [MIN_IMAGE_SIDE, MAX_IMAGE_SIDE]（推荐 512–1024）。
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
/// 表情图最短边（像素）。低于此清晰度不足 ChatPet / 空态。
pub const MIN_IMAGE_SIDE: u32 = 256;
/// 表情图最长边（像素）。与默认大肥鱼素材 1024 对齐，禁止过大图。
pub const MAX_IMAGE_SIDE: u32 = 1024;
/// 允许的宽高差（像素）；默认要求严格正方形，容 1px 压缩误差。
pub const MAX_ASPECT_DELTA: u32 = 1;

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

/// 从文件头读宽高（不整图解码）。失败 → None。
pub fn read_image_dims(bytes: &[u8]) -> Option<(u32, u32)> {
    match detect_image_kind(bytes)? {
        "png" => png_dims(bytes),
        "jpg" => jpeg_dims(bytes),
        "webp" => webp_dims(bytes),
        _ => None,
    }
}

fn png_dims(bytes: &[u8]) -> Option<(u32, u32)> {
    // signature(8) + len(4) + "IHDR"(4) + width(4) + height(4)
    if bytes.len() < 24 || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
    Some((w, h))
}

fn jpeg_dims(bytes: &[u8]) -> Option<(u32, u32)> {
    // 扫描 SOF0/2 标记取高宽
    let mut i = 2usize;
    while i + 9 < bytes.len() {
        if bytes[i] != 0xFF {
            i += 1;
            continue;
        }
        while i < bytes.len() && bytes[i] == 0xFF {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        let marker = bytes[i];
        i += 1;
        if marker == 0xD8 || marker == 0xD9 || (0xD0..=0xD7).contains(&marker) {
            continue;
        }
        if i + 1 >= bytes.len() {
            break;
        }
        let seglen = u16::from_be_bytes([bytes[i], bytes[i + 1]]) as usize;
        if seglen < 2 || i + seglen > bytes.len() {
            break;
        }
        // SOF0..SOF3 / SOF5..SOF7 / SOF9..SOF11 / SOF13..SOF15（基线/渐进等）
        if matches!(
            marker,
            0xC0 | 0xC1 | 0xC2 | 0xC3 | 0xC5 | 0xC6 | 0xC7 | 0xC9 | 0xCA | 0xCB | 0xCD | 0xCE | 0xCF
        ) && seglen >= 7
        {
            let h = u16::from_be_bytes([bytes[i + 3], bytes[i + 4]]) as u32;
            let w = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]) as u32;
            return Some((w, h));
        }
        i += seglen;
    }
    None
}

fn webp_dims(bytes: &[u8]) -> Option<(u32, u32)> {
    // RIFF(12) 后逐块扫描 VP8X / VP8 / VP8L
    let mut i = 12usize;
    while i + 8 <= bytes.len() {
        let tag = &bytes[i..i + 4];
        let size = u32::from_le_bytes(bytes[i + 4..i + 8].try_into().ok()?) as usize;
        let data_start = i + 8;
        let data_end = data_start.checked_add(size)?;
        if data_end > bytes.len() {
            break;
        }
        let data = &bytes[data_start..data_end];
        if tag == b"VP8X" && data.len() >= 10 {
            let w = 1 + u32::from_le_bytes([data[4], data[5], data[6], 0]);
            let h = 1 + u32::from_le_bytes([data[7], data[8], data[9], 0]);
            return Some((w, h));
        }
        if tag == b"VP8 " && data.len() >= 10 && data[3] == 0x9d && data[4] == 0x01 && data[5] == 0x2a
        {
            let w = u16::from_le_bytes([data[6], data[7]]) as u32 & 0x3fff;
            let h = u16::from_le_bytes([data[8], data[9]]) as u32 & 0x3fff;
            return Some((w, h));
        }
        if tag == b"VP8L" && data.len() >= 5 && data[0] == 0x2f {
            let b1 = data[1] as u32;
            let b2 = data[2] as u32;
            let b3 = data[3] as u32;
            let b4 = data[4] as u32;
            let w = 1 + (b1 | ((b2 & 0x3f) << 8));
            let h = 1 + (((b2 & 0xc0) >> 6) | (b3 << 2) | ((b4 & 0x0f) << 10));
            return Some((w, h));
        }
        // 块大小为奇数时有 pad byte
        i = data_end + (size % 2);
    }
    None
}

/// 校验表情图像素规格：正方 + 边长范围。
pub fn validate_image_dims(w: u32, h: u32) -> Result<(), String> {
    if w == 0 || h == 0 {
        return Err("face_bad_dims:无法识别图片尺寸".into());
    }
    let delta = w.abs_diff(h);
    if delta > MAX_ASPECT_DELTA {
        return Err(format!(
            "face_bad_aspect:表情图须为正方形（1:1），当前 {w}×{h}"
        ));
    }
    let side = w.max(h);
    if side < MIN_IMAGE_SIDE {
        return Err(format!(
            "face_too_small:边长至少 {MIN_IMAGE_SIDE}px（推荐 512–1024），当前 {side}"
        ));
    }
    if side > MAX_IMAGE_SIDE {
        return Err(format!(
            "face_too_big:边长最多 {MAX_IMAGE_SIDE}px（与默认大肥鱼素材一致），当前 {side}"
        ));
    }
    Ok(())
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
    let (w, h) = read_image_dims(&bytes).ok_or("face_bad_dims:无法识别图片尺寸")?;
    validate_image_dims(w, h)?;

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
    // base = app_data_dir 本身；face_skins 一层由 skins_dir 统一 join。
    // 此前这里错误地多 join 了一层 face_skins，数据落在
    // <app_data>/face_skins/face_skins/<id>/，而前端 asset URL 与
    // assetProtocol scope 都按一层构造 → 图片全部 404（2026-11-23）。
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?;
    migrate_legacy_nesting(&dir);
    Ok(dir)
}

/// 一次性迁移（幂等）：旧实现的 face_skins/face_skins/<id>/ 双层目录
/// 整体上提一层。legacy 不存在即 no-op；仅当目标位无同名目录才 rename
/// （冲突文件留在 legacy 不吞数据，最后 remove_dir 只能删空壳）。
fn migrate_legacy_nesting(app_data: &Path) {
    let root = app_data.join("face_skins");
    let legacy = root.join("face_skins");
    let Ok(entries) = fs::read_dir(&legacy) else {
        return;
    };
    for entry in entries.flatten() {
        let target = root.join(entry.file_name());
        if !target.exists() {
            let _ = fs::rename(entry.path(), target);
        }
    }
    let _ = fs::remove_dir(&legacy);
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

    /// 最小可读 PNG 头（仅 IHDR 有意义，供 magic + 尺寸校验）。
    fn png_bytes_sized(w: u32, h: u32) -> Vec<u8> {
        let mut v = vec![0x89u8, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        v.extend_from_slice(&13u32.to_be_bytes()); // IHDR length
        v.extend_from_slice(b"IHDR");
        v.extend_from_slice(&w.to_be_bytes());
        v.extend_from_slice(&h.to_be_bytes());
        v.extend_from_slice(&[8, 2, 0, 0, 0]); // bit depth / color / filter / interlace
        v.extend_from_slice(&[0, 0, 0, 0]); // fake CRC
        v
    }

    fn png_bytes() -> Vec<u8> {
        png_bytes_sized(512, 512)
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

        std::thread::sleep(std::time::Duration::from_millis(2));
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

        // 真 PNG（512 正方）
        let png = base.join("src.png");
        fs::write(&png, png_bytes()).unwrap();
        let m = import_inner(&base, &m.id, "calm", &png).unwrap();
        assert_eq!(m.slots.get("calm").unwrap(), "calm.png");
        assert!(skins_dir(&base).join(&m.id).join("calm.png").exists());

        // 同槽位换图：旧文件清掉
        let png2 = base.join("src2.png");
        fs::write(&png2, png_bytes_sized(256, 256)).unwrap();
        let m = import_inner(&base, &m.id, "calm", &png2).unwrap();
        assert_eq!(m.slots.get("calm").unwrap(), "calm.png");

        // 假 PNG（文本改名 .png）→ face_bad_image
        let fake = base.join("fake.png");
        fs::write(&fake, b"not an image at all..........").unwrap();
        let err = import_inner(&base, &m.id, "proud", &fake).unwrap_err();
        assert!(err.starts_with("face_bad_image"), "err: {err}");

        // 非正方 → face_bad_aspect
        let wide = base.join("wide.png");
        fs::write(&wide, png_bytes_sized(512, 400)).unwrap();
        let err = import_inner(&base, &m.id, "proud", &wide).unwrap_err();
        assert!(err.starts_with("face_bad_aspect"), "err: {err}");

        // 边长过小
        let tiny = base.join("tiny.png");
        fs::write(&tiny, png_bytes_sized(64, 64)).unwrap();
        let err = import_inner(&base, &m.id, "proud", &tiny).unwrap_err();
        assert!(err.starts_with("face_too_small"), "err: {err}");

        // 边长过大
        let huge = base.join("huge.png");
        fs::write(&huge, png_bytes_sized(2048, 2048)).unwrap();
        let err = import_inner(&base, &m.id, "proud", &huge).unwrap_err();
        assert!(err.starts_with("face_too_big"), "err: {err}");

        // 字节超限
        let big = base.join("big2.png");
        let mut content = png_bytes();
        content.extend(std::iter::repeat(0u8).take((2 * 1024 * 1024 + 1) as usize));
        fs::write(&big, content).unwrap();
        let err = import_inner(&base, &m.id, "proud", &big).unwrap_err();
        assert!(err.starts_with("face_too_large"), "err: {err}");

        // clear_slot：文件删除 + manifest 置空
        let m = clear_slot_inner(&base, &m.id, "calm").unwrap();
        assert!(!m.slots.contains_key("calm"));
        assert!(!skins_dir(&base).join(&m.id).join("calm.png").exists());
        // 再 clear 已空槽位：幂等
        clear_slot_inner(&base, &m.id, "calm").unwrap();
    }

    #[test]
    fn image_dims_and_validate() {
        assert_eq!(read_image_dims(&png_bytes_sized(512, 512)), Some((512, 512)));
        assert!(validate_image_dims(512, 512).is_ok());
        assert!(validate_image_dims(256, 256).is_ok());
        assert!(validate_image_dims(1024, 1024).is_ok());
        assert!(validate_image_dims(512, 500).is_err());
        assert!(validate_image_dims(128, 128).is_err());
        assert!(validate_image_dims(2048, 2048).is_err());
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

    #[test]
    fn migrate_lifts_legacy_double_nesting() {
        let base = temp_base("migrate");
        let root = base.join("face_skins");
        // 造 legacy 双层：face_skins/face_skins/c-aaaaaaaa/manifest.json
        let legacy = root.join("face_skins").join("c-aaaaaaaa");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(
            legacy.join("manifest.json"),
            r#"{"id":"c-aaaaaaaa","name":"旧数据","created_at":1,"slots":{}}"#,
        )
        .unwrap();
        // 幂等调用（skins_base 同款入口）
        migrate_legacy_nesting(&base);
        migrate_legacy_nesting(&base);
        // 上提到一层，legacy 空壳被删除
        assert!(root.join("c-aaaaaaaa").join("manifest.json").exists());
        assert!(!root.join("face_skins").exists());
        // 迁移后 inner 函数能在正确路径读到它
        let all = list_inner(&base).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, "c-aaaaaaaa");
        // 目标位已有同名目录 → 冲突残留保留，legacy 不删
        let legacy2 = root.join("face_skins").join("c-bbbbbbbb");
        fs::create_dir_all(&root.join("c-bbbbbbbb")).unwrap();
        fs::create_dir_all(&legacy2).unwrap();
        fs::write(legacy2.join("manifest.json"), "{}").unwrap();
        migrate_legacy_nesting(&base);
        assert!(legacy2.exists(), "冲突时 legacy 残留不吞数据");
    }
}
