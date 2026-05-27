//! Best-effort static update check.
//!
//! CodeZ's website hosts a tiny JSON manifest at `/update.json`. The app reads
//! it on a background thread and prompts when the published version is newer
//! than the compiled `CARGO_PKG_VERSION`.

use std::process::Command;

pub const WEBSITE_URL: &str = "https://codez.elsetech.app";
pub const UPDATE_MANIFEST_URL: &str = "https://codez.elsetech.app/update.json";

#[derive(Clone, Debug)]
pub struct UpdateInfo {
    pub version: String,
    pub download_url: String,
    pub release_notes_url: Option<String>,
    pub notes: Option<String>,
}

pub fn check() -> Result<Option<UpdateInfo>, String> {
    let output = Command::new("curl")
        .args(["-fsSL", "-m", "8", UPDATE_MANIFEST_URL])
        .output()
        .map_err(|e| format!("failed to run curl: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "update check failed: curl exited {}",
            output.status
        ));
    }

    let body = String::from_utf8(output.stdout).map_err(|e| format!("invalid UTF-8: {e}"))?;
    let version = json_string(&body, "version").ok_or("update manifest missing version")?;
    if !is_newer_version(&version, env!("CARGO_PKG_VERSION")) {
        return Ok(None);
    }

    Ok(Some(UpdateInfo {
        version,
        download_url: json_string(&body, "download_url").unwrap_or_else(|| WEBSITE_URL.to_owned()),
        release_notes_url: json_string(&body, "release_notes_url"),
        notes: json_string(&body, "notes"),
    }))
}

fn is_newer_version(remote: &str, current: &str) -> bool {
    let mut remote_parts = version_numbers(remote);
    let mut current_parts = version_numbers(current);
    let len = remote_parts.len().max(current_parts.len()).max(3);
    remote_parts.resize(len, 0);
    current_parts.resize(len, 0);
    remote_parts > current_parts
}

fn version_numbers(version: &str) -> Vec<u32> {
    version
        .trim_start_matches('v')
        .split(|c: char| !c.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse().ok())
        .collect()
}

fn json_string(s: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let after_key = &s[s.find(&needle)? + needle.len()..];
    let after_colon = &after_key[after_key.find(':')? + 1..];
    let mut chars = after_colon.trim_start().chars();
    if chars.next()? != '"' {
        return None;
    }

    let mut out = String::new();
    let mut escaped = false;
    for c in chars {
        if escaped {
            let decoded = match c {
                '"' => '"',
                '\\' => '\\',
                '/' => '/',
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                other => other,
            };
            out.push(decoded);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '"' {
            return Some(out);
        } else {
            out.push(c);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer_version("0.1.1", "0.1.0"));
        assert!(is_newer_version("v1.0.0", "0.9.9"));
        assert!(!is_newer_version("0.1.0", "0.1.0"));
        assert!(!is_newer_version("0.1.0", "0.1.1"));
    }

    #[test]
    fn parses_json_strings() {
        let body = r#"{"version":"0.2.0","notes":"hello\nworld"}"#;
        assert_eq!(json_string(body, "version").as_deref(), Some("0.2.0"));
        assert_eq!(json_string(body, "notes").as_deref(), Some("hello\nworld"));
    }
}
