//! Local Steam install resolution for the generic scan.
//!
//! A Steam game's exe lives under `…\steamapps\common\<installdir>\…`. The
//! sibling `steamapps` folder — the exe's *own* library, so multiple Steam
//! libraries work without parsing `libraryfolders.vdf` (see the plan §9) — holds
//! `appmanifest_<appid>.acf` files; the one whose `"installdir"` matches ours
//! carries the game's `"name"`. Everything here is public, local data (Steam's own
//! catalog on disk) — no web API, and we never ship Medal's DB (plan §1.6).

use std::path::{Path, PathBuf};

/// Locate the Steam library that owns `exe`: returns the `steamapps` directory and
/// the install-folder name (the path component right after `steamapps\common\`),
/// or `None` if `exe` isn't under `steamapps\common\`. Component matching is
/// case-insensitive (`SteamApps`, `Common`, … all vary in the wild).
pub fn steam_library_from_exe(exe: &Path) -> Option<(PathBuf, String)> {
    // Walk ancestors upward. The install dir is the ancestor whose parent is
    // `common` and grandparent is `steamapps`: `…\steamapps\common\<installdir>\…`.
    for dir in exe.ancestors() {
        let Some(installdir) = dir.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let Some(common) = dir.parent() else { continue };
        if !name_eq(common, "common") {
            continue;
        }
        let Some(steamapps) = common.parent() else {
            continue;
        };
        if !name_eq(steamapps, "steamapps") {
            continue;
        }
        return Some((steamapps.to_path_buf(), installdir.to_string()));
    }
    None
}

/// Resolve the display name of the game installed at `installdir` within
/// `steamapps` by reading the matching `appmanifest_*.acf`. Falls back to
/// `installdir` itself when no manifest matches or none carries a usable `"name"`.
pub fn resolve_steam_name(steamapps: &Path, installdir: &str) -> String {
    name_from_manifests(steamapps, installdir).unwrap_or_else(|| installdir.to_string())
}

/// Scan `steamapps` for the `appmanifest_*.acf` whose `"installdir"` matches and
/// return its `"name"`. `None` if no manifest matches or the match has no name.
fn name_from_manifests(steamapps: &Path, installdir: &str) -> Option<String> {
    for entry in std::fs::read_dir(steamapps).ok()?.flatten() {
        let path = entry.path();
        let is_manifest = path
            .file_name()
            .and_then(|s| s.to_str())
            .map(|n| {
                let n = n.to_ascii_lowercase();
                n.starts_with("appmanifest_") && n.ends_with(".acf")
            })
            .unwrap_or(false);
        if !is_manifest {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let matches = acf_value(&text, "installdir")
            .map(|d| d.eq_ignore_ascii_case(installdir))
            .unwrap_or(false);
        if matches {
            return acf_value(&text, "name").filter(|n| !n.trim().is_empty());
        }
    }
    None
}

/// The first top-level string value for `key` in `.acf`/VDF text — lines shaped
/// `"key"<whitespace>"value"`. Splitting on `"` puts the quoted tokens at odd
/// indices (`["", key, ws, value, ""]`), which is all this format needs.
fn acf_value(text: &str, key: &str) -> Option<String> {
    for line in text.lines() {
        let parts: Vec<&str> = line.split('"').collect();
        if parts.len() >= 4 && parts[1].eq_ignore_ascii_case(key) {
            return Some(parts[3].to_string());
        }
    }
    None
}

/// Whether `path`'s final component equals `name` (case-insensitive).
fn name_eq(path: &Path, name: &str) -> bool {
    path.file_name()
        .and_then(|s| s.to_str())
        .map(|n| n.eq_ignore_ascii_case(name))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_library_and_installdir_from_exe() {
        let exe = PathBuf::from(r"D:\SteamLibrary\steamapps\common\Elden Ring\Game\eldenring.exe");
        let (lib, installdir) = steam_library_from_exe(&exe).unwrap();
        assert_eq!(lib, PathBuf::from(r"D:\SteamLibrary\steamapps"));
        assert_eq!(installdir, "Elden Ring");
    }

    #[test]
    fn steam_components_match_case_insensitively() {
        let exe =
            PathBuf::from(r"C:\Program Files (x86)\Steam\SteamApps\Common\Hades\Hades.exe");
        let (lib, installdir) = steam_library_from_exe(&exe).unwrap();
        assert_eq!(installdir, "Hades");
        assert!(name_eq(&lib, "steamapps"));
    }

    #[test]
    fn non_steam_path_is_none() {
        let exe = PathBuf::from(r"C:\Games\Standalone\game.exe");
        assert!(steam_library_from_exe(&exe).is_none());
    }

    #[test]
    fn parses_name_and_installdir_from_acf() {
        let acf = "\"AppState\"\n{\n\t\"appid\"\t\t\"1245620\"\n\t\"name\"\t\t\"ELDEN RING\"\n\t\"installdir\"\t\t\"Elden Ring\"\n}\n";
        assert_eq!(acf_value(acf, "name").as_deref(), Some("ELDEN RING"));
        assert_eq!(acf_value(acf, "installdir").as_deref(), Some("Elden Ring"));
        assert_eq!(acf_value(acf, "missing"), None);
    }
}
