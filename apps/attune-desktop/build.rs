//! attune-desktop build script.
//!
//! Beyond the standard `tauri_build::build()`, this stages the **SenseVoice native
//! shared libraries** (sherpa-onnx + its bundled onnxruntime) so the packaged desktop
//! app can load them at runtime.
//!
//! Why this is needed (rc.5 packaging gate, spec §9):
//!   - `sherpa-rs` (feature `download-binaries`) links `libsherpa-onnx-c-api` into the
//!     main binary as a **hard DT_NEEDED dependency** (unlike `ort`, which `dlopen`s its
//!     `libonnxruntime` lazily and searches next to the exe). So the main Attune binary
//!     will not even *launch* from a package unless the dynamic linker can resolve the 3
//!     sherpa libs (`libsherpa-onnx-c-api`, `libsherpa-onnx-cxx-api`, `libonnxruntime`).
//!   - `sherpa-rs-sys` copies those libs next to the build artifact in the cargo target
//!     dir, but emits **no rpath** on the consuming binary. In a cargo build the libs are
//!     found only because cargo sets `LD_LIBRARY_PATH` to `target/<profile>/deps`. A deb /
//!     AppImage / MSI has no such env, so we must (a) ship the libs and (b) give the main
//!     binary an `$ORIGIN`-relative rpath.
//!
//! What this build script does:
//!   1. Locate the 3 sherpa libs in the cargo target dir (the sys crate already copied
//!      them beside our artifact, since dependency build scripts run first).
//!   2. Copy them into `resources/lib/<platform>/` so `tauri.conf.json` can bundle them.
//!   3. On Unix, add an rpath to the main binary so it finds those libs once installed —
//!      covering both the Tauri "libs beside the binary" layout (AppImage / Windows) and
//!      the deb/rpm layout (binary in `/usr/bin`, resources under `/usr/lib/Attune`).
//!
//! When the `asr-sensevoice` feature is compiled out of attune-core (size-constrained /
//! pure-native builds), there are no sherpa libs to stage and this is a no-op — ASR then
//! degrades to whisper-cli (spec §2 / §7), so the build still succeeds.

use std::path::{Path, PathBuf};

/// The 3 sherpa-onnx shared libs that must travel with the binary, per platform.
/// (sherpa-rs `download-binaries` names; on Windows these are `.dll`, macOS `.dylib`.)
const SHERPA_LIB_STEMS: &[&str] = &[
    "libonnxruntime",
    "libsherpa-onnx-c-api",
    "libsherpa-onnx-cxx-api",
];

fn main() {
    // Stage the SenseVoice libs FIRST: `tauri_build::build()` eagerly validates the
    // `resources` / `files` paths in tauri.conf.json, so the per-platform `resources/lib/<os>/`
    // directories must exist before it runs (even when empty on this target's build).
    ensure_resource_lib_dirs();
    stage_sherpa_libs();
    set_sensevoice_rpath();

    tauri_build::build();
}

/// Ensure all per-platform `resources/lib/<os>/` dirs exist before tauri validates config.
/// On a Linux build the `windows`/`macos` dirs stay empty (their resource/files entries copy
/// nothing), but they must EXIST or tauri's resource-path validation fails the build.
fn ensure_resource_lib_dirs() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    for plat in ["linux", "windows", "macos"] {
        let dir = manifest_dir.join("resources").join("lib").join(plat);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            println!(
                "cargo:warning=attune-desktop build.rs: mkdir {} failed: {e}",
                dir.display()
            );
        }
    }
}

/// Resolve `target/<profile>/` — where sherpa-rs-sys copied the dylibs and where our
/// own artifact lands. `OUT_DIR` is `.../target/<profile>/build/<pkg>-<hash>/out`, so we
/// walk up until a parent directory name matches `PROFILE`.
fn cargo_target_profile_dir() -> Option<PathBuf> {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").ok()?);
    let profile = std::env::var("PROFILE").ok()?;
    let mut cur: &Path = &out_dir;
    while let Some(parent) = cur.parent() {
        if parent.file_name().and_then(|n| n.to_str()) == Some(profile.as_str()) {
            return Some(parent.to_path_buf());
        }
        cur = parent;
    }
    None
}

/// Platform-specific shared-lib extension + the `resources/lib/<platform>/` subdir name
/// used in tauri.conf.json `resources` (so Windows `.dll` never lands in a Linux package
/// and vice-versa — the exact bug class this work removes).
fn platform_lib_info() -> (&'static str, &'static str) {
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    match os.as_str() {
        "windows" => ("dll", "windows"),
        "macos" => ("dylib", "macos"),
        _ => ("so", "linux"),
    }
}

/// Copy the 3 sherpa libs from the cargo target dir into `resources/lib/<platform>/`.
///
/// Missing libs (feature off, or first build before sherpa-rs-sys ran) are skipped with a
/// warning rather than failing — a build without ASR libs is valid (degrades to whisper).
fn stage_sherpa_libs() {
    let (ext, plat_dir) = platform_lib_info();
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let dst_dir = manifest_dir.join("resources").join("lib").join(plat_dir);

    let Some(target_dir) = cargo_target_profile_dir() else {
        println!(
            "cargo:warning=attune-desktop build.rs: could not resolve target profile dir; \
             SenseVoice libs not staged (ASR will degrade to whisper-cli if libs absent)"
        );
        return;
    };

    // On Windows the dll naming may drop the `lib` prefix; match either form.
    let candidates_for = |stem: &str| -> Vec<String> {
        let bare = stem.strip_prefix("lib").unwrap_or(stem);
        vec![format!("{stem}.{ext}"), format!("{bare}.{ext}")]
    };

    let mut staged = 0;
    for stem in SHERPA_LIB_STEMS {
        let mut found = false;
        for name in candidates_for(stem) {
            let src = target_dir.join(&name);
            println!("cargo:rerun-if-changed={}", src.display());
            if src.exists() {
                if let Err(e) = std::fs::create_dir_all(&dst_dir) {
                    println!(
                        "cargo:warning=attune-desktop build.rs: mkdir {} failed: {e}",
                        dst_dir.display()
                    );
                    return;
                }
                let dst = dst_dir.join(&name);
                if let Err(e) = std::fs::copy(&src, &dst) {
                    println!(
                        "cargo:warning=attune-desktop build.rs: copy {} -> {} failed: {e}",
                        src.display(),
                        dst.display()
                    );
                } else {
                    staged += 1;
                    found = true;
                }
                break;
            }
        }
        if !found {
            println!(
                "cargo:warning=attune-desktop build.rs: SenseVoice lib '{stem}.{ext}' not found \
                 in {} (ASR degrades to whisper-cli if this is a release build)",
                target_dir.display()
            );
        }
    }
    println!(
        "cargo:warning=attune-desktop build.rs: staged {staged}/{} SenseVoice native libs into {}",
        SHERPA_LIB_STEMS.len(),
        dst_dir.display()
    );
}

/// Add an `$ORIGIN`-relative rpath to the main binary so the dynamically-linked sherpa
/// libs resolve at runtime in a packaged install.
///
/// Two layouts to satisfy with one rpath set:
///   - AppImage / Windows: Tauri puts bundled resources **beside** the executable →
///     libs in `<exe-dir>/lib/<platform>/` and also `<exe-dir>/` (we add both).
///   - deb / rpm: binary in `/usr/bin/Attune`, resources under `/usr/lib/Attune/` →
///     reachable from the binary via `$ORIGIN/../lib/Attune/lib/<platform>`.
///
/// Windows has no rpath concept; the loader searches the exe's own directory, where the
/// `.dll`s are bundled — so this Unix-only emission is sufficient. macOS uses the same
/// `@loader_path`-style mechanism via `$ORIGIN` understood by `rustc-link-arg`.
fn set_sensevoice_rpath() {
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if os == "windows" {
        return; // DLLs load from the exe directory; no rpath needed.
    }
    let (_, plat_dir) = platform_lib_info();
    // All entries are searched by the loader; we cover both bundle layouts.
    let rpaths = [
        "$ORIGIN".to_string(),
        format!("$ORIGIN/lib/{plat_dir}"),
        format!("$ORIGIN/../lib/Attune/lib/{plat_dir}"),
        format!("$ORIGIN/../lib/attune/lib/{plat_dir}"),
    ];
    for rp in rpaths {
        println!("cargo:rustc-link-arg=-Wl,-rpath,{rp}");
    }
    // DT_RUNPATH (new dtags). The sherpa libs themselves already carry `$ORIGIN`
    // rpath (verified on the sys-crate output), so once co-located they chain to each
    // other and to libonnxruntime without further help.
    println!("cargo:rustc-link-arg=-Wl,--enable-new-dtags");
}
