//! Runs the unit test of `tools/check-release-zip.py` in the cargo gate.
//! Release zips are packed by hand and ship crate `tests/` folders, so the
//! script (not this crate) is the guard that keeps the test-only TLS key
//! (`tests/fixtures/tls/srv.key.der`), other keys, `.gguf` weights,
//! `master.zip` and `tools/lillith-py` out of a release.

use std::path::Path;
use std::process::Command;

#[test]
fn check_release_zip_script_passes_its_unit_test() {
    let tools = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools");
    assert!(tools.join("check-release-zip.py").is_file());
    let out = Command::new("python3")
        .arg(tools.join("check_release_zip_test.py"))
        .output()
        .expect("python3 is needed to run tools/check_release_zip_test.py");
    assert!(
        out.status.success(),
        "tools/check_release_zip_test.py failed:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
