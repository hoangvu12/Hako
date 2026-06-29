//! Riot lockfile parsing + local Basic auth, shared by every Riot title.
//!
//! Both Valorant's Riot Client lockfile and League's LCU lockfile hold the same
//! `name:pid:port:password:protocol` line and authenticate the same way (HTTP
//! Basic, username `riot`, against `https://127.0.0.1:{port}` with a self-signed
//! cert). Only the file's *location* differs, so that's left to each game.

#![allow(dead_code)]

use base64::Engine;

/// Parsed lockfile contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lockfile {
    pub name: String,
    pub pid: u32,
    pub port: u16,
    pub password: String,
    /// Usually `https`.
    pub protocol: String,
}

impl Lockfile {
    /// Parse the `name:pid:port:password:protocol` line.
    pub fn parse(contents: &str) -> Result<Lockfile, String> {
        let line = contents.trim();
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() != 5 {
            return Err(format!(
                "lockfile has {} fields, expected 5 (name:pid:port:password:protocol)",
                parts.len()
            ));
        }
        Ok(Lockfile {
            name: parts[0].to_string(),
            pid: parts[1].parse().map_err(|_| "bad pid in lockfile")?,
            port: parts[2].parse().map_err(|_| "bad port in lockfile")?,
            password: parts[3].to_string(),
            protocol: parts[4].to_string(),
        })
    }

    /// Local API base URL, e.g. `https://127.0.0.1:51234`.
    pub fn base_url(&self) -> String {
        format!("{}://127.0.0.1:{}", self.protocol, self.port)
    }

    /// `Authorization: Basic ...` header value (user `riot`, lockfile password).
    pub fn basic_auth_header(&self) -> String {
        let raw = format!("riot:{}", self.password);
        let b64 = base64::engine::general_purpose::STANDARD.encode(raw.as_bytes());
        format!("Basic {b64}")
    }
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
