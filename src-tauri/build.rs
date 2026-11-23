fn main() {
    // The Ollama sidecar is declared via `bundle.externalBin` in
    // tauri.conf.json, and tauri-build hard-fails when the file is missing.
    // `pnpm llm:sidecar` fetches the real binary; until then, a placeholder
    // executable keeps `cargo check` / `tauri dev` working. The placeholder
    // exits immediately when spawned — the app logs one warning and the
    // model legs degrade to their rule-based fallbacks, exactly like a
    // missing binary.
    ensure_sidecar_placeholder();
    stage_sidecar_runtime();
    tauri_build::build()
}

/// Since v0.40 Ollama's inference engine is a separate `llama-server`
/// executable (+ dylibs) that `ollama` looks for next to its own binary.
/// tauri-build only copies the externalBin itself into `target/<profile>/`,
/// so mirror the staged runtime from `binaries/ollama-runtime/` alongside it.
/// (Packaged bundles still need `bundle.resources` wiring — dev only for now.)
fn stage_sidecar_runtime() {
    let src = std::path::Path::new("binaries").join("ollama-runtime");
    if !src.is_dir() {
        return; // `pnpm llm:sidecar` has not staged the runtime yet
    }
    println!("cargo:rerun-if-changed=binaries/ollama-runtime");

    // OUT_DIR nests as target/<profile>/build/<pkg>/<hash>/out, so walk up to
    // the ancestor whose name is the profile ("debug" / "release").
    let Ok(out_dir) = std::env::var("OUT_DIR") else {
        return;
    };
    let profile = std::env::var("PROFILE").unwrap_or_default();
    if profile.is_empty() {
        return;
    }
    let Some(dest) = std::path::Path::new(&out_dir)
        .ancestors()
        .find(|p| p.file_name().and_then(|n| n.to_str()) == Some(profile.as_str()))
    else {
        return;
    };

    let Ok(entries) = std::fs::read_dir(&src) else { return };
    for entry in entries.flatten() {
        let from = entry.path();
        let to = dest.join(entry.file_name());
        let Ok(file_type) = entry.file_type() else { continue };
        if file_type.is_symlink() {
            // Versioned dylib names are real files; the unversioned ones are
            // symlinks that the runtime's install names rely on. Windows
            // runners often cannot create symlinks without developer mode,
            // so materialise a real copy there instead.
            if let Ok(target) = std::fs::read_link(&from) {
                let _ = std::fs::remove_file(&to);
                #[cfg(unix)]
                let _ = std::os::unix::fs::symlink(&target, &to);
                #[cfg(windows)]
                {
                    let parent = from.parent().unwrap_or(std::path::Path::new("."));
                    let resolved = parent.join(target);
                    let _ = std::fs::copy(&resolved, &to);
                }
            }
        } else if file_type.is_dir() {
            // MLX runtime lives in mlx_metal_v3/ / mlx_metal_v4/ subdirs.
            copy_dir_contents(&from, &to);
        } else if std::fs::copy(&from, &to).is_ok() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&to, std::fs::Permissions::from_mode(0o755));
            }
        }
    }
}

fn copy_dir_contents(from: &std::path::Path, to: &std::path::Path) {
    if std::fs::create_dir_all(to).is_err() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(from) else { return };
    for entry in entries.flatten() {
        let src_path = entry.path();
        let dst_path = to.join(entry.file_name());
        let Ok(file_type) = entry.file_type() else { continue };
        if file_type.is_dir() {
            copy_dir_contents(&src_path, &dst_path);
        } else if std::fs::copy(&src_path, &dst_path).is_ok() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&dst_path, std::fs::Permissions::from_mode(0o755));
            }
        }
    }
}

fn ensure_sidecar_placeholder() {
    let arch = match std::env::consts::ARCH {
        "aarch64" => "aarch64",
        "x86_64" => "x86_64",
        _ => return, // unsupported platform: leave validation to tauri-build
    };
    let triple = match std::env::consts::OS {
        "macos" => "apple-darwin",
        "windows" => "pc-windows-msvc",
        _ => return, // Linux sidecar is not shipped yet
    };
    let windows = triple == "pc-windows-msvc";
    let file = format!("ollama-{arch}-{triple}{}", if windows { ".exe" } else { "" });
    let path = std::path::Path::new("binaries").join(file);
    if path.exists() {
        return; // real sidecar (or a previous placeholder) is in place
    }
    if std::fs::create_dir_all("binaries").is_err() {
        return;
    }
    // A tiny text file is enough for tauri-build's existence check; it exits
    // non-zero at runtime so the app logs a warning and uses rule fallbacks.
    let script = if windows {
        "@echo off\r\nREM bonsai placeholder: run 'pnpm llm:sidecar' for the real Ollama binary\r\nexit /b 1\r\n"
    } else {
        "#!/bin/sh\n# bonsai placeholder: run 'pnpm llm:sidecar' for the real Ollama binary\nexit 1\n"
    };
    if std::fs::write(&path, script).is_ok() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
        }
        println!(
            "cargo:warning=llm sidecar placeholder created at {} — run 'pnpm llm:sidecar' for the real binary",
            path.display()
        );
    }
}
