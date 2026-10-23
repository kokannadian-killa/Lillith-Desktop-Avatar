//! Embeds the requireAdministrator manifest into `lillith-admin.exe`
//! (via windres), matching apps/lillith's icon embedding.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=lillith-admin.manifest");
    println!("cargo:rerun-if-changed=lillith-admin.rc");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=WINDRES");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    if let Err(e) = embed() {
        println!("cargo:warning=admin manifest not embedded: {e}");
    }
}

fn embed() -> Result<(), String> {
    let out = PathBuf::from(std::env::var("OUT_DIR").map_err(|e| e.to_string())?);
    let res = out.join("lillith-admin-manifest.res");
    let status = Command::new(windres())
        .args(["-O", "coff", "-i", "lillith-admin.rc", "-o"])
        .arg(&res)
        .status()
        .map_err(|e| format!("windres: {e}"))?;
    if !status.success() {
        return Err(format!("windres failed ({status})"));
    }
    println!("cargo:rustc-link-arg={}", res.display());
    Ok(())
}

fn windres() -> String {
    if let Ok(w) = std::env::var("WINDRES") {
        return w;
    }
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let cross = format!("{arch}-w64-mingw32-windres");
    if Command::new(&cross).arg("--version").output().is_ok() {
        cross
    } else {
        "windres".into()
    }
}
