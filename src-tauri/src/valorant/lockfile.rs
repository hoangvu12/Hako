//! Riot lockfile parsing + local auth.
//!
//! `%LOCALAPPDATA%\Riot Games\Riot Client\Config\lockfile` is written while the
//! Riot client runs and holds `name:pid:port:password:protocol`. We use `port`
//! + `password` for HTTP Basic auth (username `riot`) against
//! `https://127.0.0.1:{port}`, accepting the self-signed cert (localhost only).

#![allow(dead_code)]

use std::path::PathBuf;

// The lockfile *format* + Basic-auth are shared across Riot titles; only the
// Riot-Client lockfile *location* is Valorant-specific and stays here.
pub use crate::games::lockfile::Lockfile;

/// Default lockfile path on this machine (`%LOCALAPPDATA%\Riot Games\...`).
pub fn default_path() -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")?;
    Some(
        PathBuf::from(local)
            .join("Riot Games")
            .join("Riot Client")
            .join("Config")
            .join("lockfile"),
    )
}

/// Read + parse the lockfile from its default location. `Err` if the Riot client
/// isn't running (file absent) or the contents are malformed.
pub fn read() -> Result<Lockfile, String> {
    let path = default_path().ok_or("LOCALAPPDATA not set")?;
    let contents = std::fs::read_to_string(&path).map_err(|e| {
        format!(
            "lockfile not found at {} ({e}) — is Riot running?",
            path.display()
        )
    })?;
    Lockfile::parse(&contents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_lockfile_line() {
        let lf = Lockfile::parse("Riot Client:12345:54321:s3cr3tpass:https\n").unwrap();
        assert_eq!(lf.name, "Riot Client");
        assert_eq!(lf.pid, 12345);
        assert_eq!(lf.port, 54321);
        assert_eq!(lf.password, "s3cr3tpass");
        assert_eq!(lf.protocol, "https");
        assert_eq!(lf.base_url(), "https://127.0.0.1:54321");
    }

    #[test]
    fn basic_auth_is_riot_user() {
        let lf = Lockfile::parse("x:1:2:pw:https").unwrap();
        // base64("riot:pw") == "cmlvdDpwdw=="
        assert_eq!(lf.basic_auth_header(), "Basic cmlvdDpwdw==");
    }

    #[test]
    fn rejects_malformed() {
        assert!(Lockfile::parse("too:few:fields").is_err());
        assert!(Lockfile::parse("n:notanumber:2:pw:https").is_err());
    }
}
