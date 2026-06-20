use std::{env, fs, path::PathBuf};

fn main() {
    copy_ffmpeg_dlls();
    bake_oauth_credentials();
    tauri_build::build();
}

/// The consumer-cloud OAuth app credentials Hako embeds at build time. They are
/// not user secrets — they identify Hako to Google/Dropbox/Microsoft — so they
/// ship inside the binary. `cloud::oauth` reads each via `option_env!`.
const OAUTH_ENV_VARS: &[&str] = &[
    "HAKO_GOOGLE_CLIENT_ID",
    "HAKO_GOOGLE_CLIENT_SECRET",
    "HAKO_DROPBOX_CLIENT_ID",
    "HAKO_DROPBOX_CLIENT_SECRET",
    "HAKO_MICROSOFT_CLIENT_ID",
    "HAKO_MICROSOFT_CLIENT_SECRET",
];

/// Bake the OAuth credentials so `option_env!` picks them up when compiling the
/// crate. Sources, in precedence order:
///   1. The build process env — how CI passes GitHub secrets (`dotenvy` never
///      overrides an already-set var, so CI always wins).
///   2. A local `.env` (gitignored) in the crate dir or the repo root — the dev
///      path; create one from `.env.example`.
/// Missing creds are simply not baked: that provider's "Connect" button then
/// returns an actionable "not configured" error at runtime, and the others
/// keep working.
fn bake_oauth_credentials() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    // Crate dir first, then repo root. `from_path` loads into the build env
    // without clobbering existing vars, so CI's real secrets take precedence.
    let candidates = [manifest.join(".env"), manifest.join("..").join(".env")];
    for env_file in &candidates {
        println!("cargo:rerun-if-changed={}", env_file.display());
        let _ = dotenvy::from_path(env_file);
    }

    let mut baked = Vec::new();
    for var in OAUTH_ENV_VARS {
        // Rebuild when a credential changes (so a new value is re-baked).
        println!("cargo:rerun-if-env-changed={var}");
        if let Ok(val) = env::var(var) {
            if !val.trim().is_empty() {
                println!("cargo:rustc-env={var}={val}");
                baked.push(*var);
            }
        }
    }
    if baked.is_empty() {
        println!(
            "cargo:warning=no HAKO_* OAuth credentials found (cloud Connect buttons will be \
             disabled until you add a src-tauri/.env — see .env.example)"
        );
    } else {
        // Names only, never values.
        println!("cargo:warning=baked OAuth credentials: {}", baked.join(", "));
    }
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
