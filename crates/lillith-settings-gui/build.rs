//! Windows only: embeds the shared icon (`windows/lillith.ico`, generated
//! by `branding/make-icons.py`) into the `.exe` as resource 1, so Explorer,
//! the taskbar and shortcuts show her. Uses `windres` (mingw, also when
//! cross-building from Linux) or `rc.exe` (MSVC); no extra crates. Other
//! targets: nothing happens. A missing resource compiler is a warning, not
//! an error (the exe then just has the default icon).

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let ico = manifest.join("../../windows/lillith.ico");
    println!("cargo:rerun-if-changed={}", ico.display());
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=WINDRES");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    if let Err(e) = embed_icon(&ico) {
        println!("cargo:warning=icon not embedded: {e}");
    }
}

fn embed_icon(ico: &Path) -> Result<(), String> {
    let ico = ico
        .canonicalize()
        .map_err(|e| format!("{}: {e}", ico.display()))?;
    let out = PathBuf::from(std::env::var("OUT_DIR").map_err(|e| e.to_string())?);
    let rc = out.join("lillith-icon.rc");
    // Forward slashes are fine for both windres and rc.exe.
    let ico_s = ico.display().to_string().replace('\\', "/");
    std::fs::write(&rc, format!("1 ICON \"{ico_s}\"\n")).map_err(|e| e.to_string())?;
    let msvc = std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    let obj = if msvc {
        let res = out.join("lillith-icon.res");
        run(Command::new("rc.exe")
            .arg("/nologo")
            .arg("/fo")
            .arg(&res)
            .arg(&rc))?;
        res
    } else {
        let o = out.join("lillith-icon.o");
        run(Command::new(windres())
            .args(["-O", "coff", "-i"])
            .arg(&rc)
            .arg("-o")
            .arg(&o))?;
        o
    };
    println!("cargo:rustc-link-arg-bins={}", obj.display());
    Ok(())
}

/// `$WINDRES`, else `<arch>-w64-mingw32-windres` when cross-building, else `windres`.
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

fn run(cmd: &mut Command) -> Result<(), String> {
    let o = cmd.output().map_err(|e| format!("{cmd:?}: {e}"))?;
    if o.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{cmd:?} failed: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        ))
    }
}
