// SPDX-License-Identifier: Apache-2.0

//! Compiles tests/smoke.c against the static library with the system `cc`
//! and runs it on a synthetic roll. Skipped (with a note) when `cc` is not
//! on PATH, and on Windows, where the MSVC static library is `negg_ffi.lib`
//! and the `cc` on PATH, if any, is MinGW, which cannot link it.

mod common;

use std::path::PathBuf;
use std::process::Command;

use common::{TempDir, write_roll};

#[test]
fn c_smoke_test() {
    if cfg!(windows) {
        eprintln!("skipped: the smoke test links with a Unix `cc`, not MSVC");
        return;
    }
    if Command::new("cc").arg("--version").output().is_err() {
        eprintln!("skipped: no `cc` on PATH");
        return;
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // target/<profile>/deps/smoke-xxxx -> target/<profile>
    let exe = std::env::current_exe().unwrap();
    let profile_dir = exe.parent().unwrap().parent().unwrap().to_path_buf();
    let mut build = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
    build.args(["build", "-p", "negg-ffi"]);
    if profile_dir.file_name().is_some_and(|n| n == "release") {
        build.arg("--release");
    }
    let st = build.status().expect("cargo build");
    assert!(st.success(), "cargo build -p negg-ffi failed");
    let lib = profile_dir.join("libnegg_ffi.a");
    assert!(lib.exists(), "missing {}", lib.display());

    let work = TempDir::new("smoke");
    let roll = work.path().join("roll");
    let out = work.path().join("out");
    std::fs::create_dir_all(&roll).unwrap();
    write_roll(&roll);
    let bin = work.path().join("smoke");

    let mut cc = Command::new("cc");
    cc.arg(manifest.join("tests/smoke.c"))
        .arg("-I")
        .arg(manifest.join("include"))
        .arg(&lib)
        .arg("-o")
        .arg(&bin)
        .args(["-lm", "-lpthread"]);
    if cfg!(target_os = "macos") {
        cc.args([
            "-framework",
            "CoreFoundation",
            "-framework",
            "Security",
            "-liconv",
        ]);
    } else {
        cc.args(["-ldl"]);
    }
    let o = cc.output().expect("run cc");
    assert!(
        o.status.success(),
        "cc failed:\n{}",
        String::from_utf8_lossy(&o.stderr)
    );

    let o = Command::new(&bin)
        .arg(&roll)
        .arg(&out)
        .output()
        .expect("run smoke");
    assert!(
        o.status.success(),
        "smoke failed:\n{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(String::from_utf8_lossy(&o.stdout).contains("smoke ok"));
    assert!(std::fs::read_dir(&out).unwrap().count() >= 1);
}
