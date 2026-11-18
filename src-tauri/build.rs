fn main() {
    // The Ollama sidecar is declared via `bundle.externalBin` in
    // tauri.conf.json, and tauri-build hard-fails when the file is missing.
    // `pnpm llm:sidecar` fetches the real binary; until then, a placeholder
    // executable keeps `cargo check` / `tauri dev` working. The placeholder
    // exits immediately when spawned — the app logs one warning and the
    // model legs degrade to their rule-based fallbacks, exactly like a
    // missing binary.
    ensure_sidecar_placeholder();
    tauri_build::build()
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
