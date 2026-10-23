fn main() {
    println!("cargo:rerun-if-changed=assets/flick.ico");
    println!("cargo:rerun-if-changed=vendor/mpv/libmpv-2.dll");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    copy_libmpv();
    if !std::path::Path::new("assets/flick.ico").exists() {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/flick.ico");
    res.set("FileDescription", "Flick video player");
    res.set("ProductName", "Flick");
    // A missing resource compiler shouldn't block a build; the exe just lacks an icon.
    if let Err(e) = res.compile() {
        println!("cargo:warning=could not embed icon: {e}");
    }
}

/// Put libmpv next to the built exes so `cargo run` and `target\release` work as-is.
fn copy_libmpv() {
    let src = std::path::Path::new("vendor/mpv/libmpv-2.dll");
    if !src.exists() {
        println!("cargo:warning=vendor/mpv/libmpv-2.dll missing: run `pwsh scripts/fetch-mpv.ps1`");
        return;
    }
    // OUT_DIR is target/<profile>/build/<pkg>/out; the exes live in target/<profile>.
    let Some(out) = std::env::var_os("OUT_DIR") else { return };
    let Some(profile_dir) = std::path::Path::new(&out).ancestors().nth(3) else { return };
    let dst = profile_dir.join("libmpv-2.dll");
    let stale = match (std::fs::metadata(src), std::fs::metadata(&dst)) {
        (Ok(a), Ok(b)) => a.len() != b.len() || a.modified().ok() > b.modified().ok(),
        _ => true,
    };
    if stale {
        if let Err(e) = std::fs::copy(src, &dst) {
            println!("cargo:warning=could not copy libmpv-2.dll to {}: {e}", dst.display());
        }
    }
}
