use std::{env, fs, path::PathBuf};

fn main() {
    copy_ffmpeg_dlls();
    tauri_build::build();
}

/// Copy the bundled FFmpeg DLLs next to the built binary (and the test deps
/// dir) so the exe finds them at runtime. Skips files already present with the
/// same size, so the ~200 MB copy only happens once.
fn copy_ffmpeg_dlls() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let bin = manifest.join("ffmpeg").join("bin");

    println!("cargo:rerun-if-changed={}", bin.display());

    if !bin.exists() {
        println!(
            "cargo:warning=src-tauri/ffmpeg/bin not found — run scripts/fetch-ffmpeg.ps1 \
             (FFmpeg encode/link will fail without it)"
        );
        return;
    }

    // OUT_DIR = target/<profile>/build/<pkg>-<hash>/out  →  profile dir is 3 up.
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let profile_dir = out_dir
        .ancestors()
        .nth(3)
        .expect("OUT_DIR has the expected depth")
        .to_path_buf();
    let targets = [profile_dir.clone(), profile_dir.join("deps")];

    let Ok(entries) = fs::read_dir(&bin) else {
        return;
    };
    for entry in entries.flatten() {
        let src = entry.path();
        if src.extension().and_then(|e| e.to_str()) != Some("dll") {
            continue;
        }
        let name = src.file_name().unwrap();
        let src_len = fs::metadata(&src).map(|m| m.len()).unwrap_or(0);

        for dir in &targets {
            let _ = fs::create_dir_all(dir);
            let dst = dir.join(name);
            let up_to_date = fs::metadata(&dst).map(|m| m.len()).unwrap_or(0) == src_len;
            if !up_to_date {
                if let Err(e) = fs::copy(&src, &dst) {
                    println!("cargo:warning=failed to copy {name:?}: {e}");
                }
            }
        }
    }
}
