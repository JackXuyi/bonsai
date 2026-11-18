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
            // symlinks that the runtime's install names rely on.
            if let Ok(target) = std::fs::read_link(&from) {
                let _ = std::fs::remove_file(&to);
                let _ = std::os::unix::fs::symlink(target, &to);
            }
        } else if file_type.is_file() && std::fs::copy(&from, &to).is_ok() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&to, std::fs::Permissions::from_mode(0o755));
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
    let path = std::path::Path::new("binaries").join(format!("ollama-{arch}-apple-darwin"));
    if path.exists() {
        return; // real sidecar (or a previous placeholder) is in place
    }
    if std::fs::create_dir_all("binaries").is_err() {
        return;
    }
    let script = "#!/bin/sh\n# bonsai placeholder: run 'pnpm llm:sidecar' to install the real Ollama binary\nexit 1\n";
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
