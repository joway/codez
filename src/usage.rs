//! Best-effort usage / rate-limit readouts for Codex and Claude Code, shown at
//! the bottom of the Agent sidebar.
//!
//! * **Codex** records its rate limits in each session transcript: the latest
//!   `token_count` event carries a `rate_limits` object whose `primary` window
//!   (5 hours) and `secondary` window (7 days) hold `used_percent` and reset
//!   timestamps.
//! * **Claude Code** doesn't cache this locally, so we ask the same OAuth
//!   endpoint its `/usage` command uses, authenticating with the token Claude
//!   Code stored in the macOS keychain (or the `.credentials.json` fallback).
//!
//! Both paths shell out / hit the network, so [`fetch`] must run off the UI
//! thread.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

/// Percentage (0–100) of each rate-limit window already consumed.
#[derive(Clone)]
pub struct Limits {
    /// The short rolling window (Codex: 5h; Claude: 5h).
    pub session: f32,
    /// The weekly window (7 days).
    pub week: f32,
    /// When the short rolling window resets, if known.
    pub session_reset: Option<ResetTime>,
}

#[derive(Clone)]
pub enum ResetTime {
    EpochSeconds(u64),
    Label(String),
}

#[derive(Clone, Default)]
pub struct Usage {
    pub codex: Option<Limits>,
    pub claude: Option<Limits>,
}

/// Gather both readouts. Does file IO and a network request — call off the UI thread.
pub fn fetch() -> Usage {
    Usage {
        codex: codex(),
        claude: claude(),
    }
}

// ---------------- Codex ----------------

fn codex() -> Option<Limits> {
    let home = std::env::var_os("HOME")?;
    let base = PathBuf::from(home).join(".codex").join("sessions");
    let file = newest_jsonl(&base)?;
    let text = std::fs::read_to_string(&file).ok()?;
    // The most recent rate-limit snapshot is the last one written.
    let tail = &text[text.rfind("\"rate_limits\"")?..];
    Some(Limits {
        session: f32_after(tail, "\"primary\"", "\"used_percent\"")?,
        week: f32_after(tail, "\"secondary\"", "\"used_percent\"")?,
        session_reset: reset_after_any(tail, "\"primary\"", &["\"resets_at\"", "\"reset_at\""]),
    })
}

fn newest_jsonl(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(SystemTime, PathBuf)> = None;
    collect_newest(dir, &mut best);
    best.map(|(_, p)| p)
}

fn collect_newest(dir: &Path, best: &mut Option<(SystemTime, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_newest(&path, best);
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            if let Ok(modified) = entry.metadata().and_then(|m| m.modified()) {
                if best.as_ref().map_or(true, |(t, _)| modified > *t) {
                    *best = Some((modified, path));
                }
            }
        }
    }
}

// ---------------- Claude Code ----------------

fn claude() -> Option<Limits> {
    let token = claude_token()?;
    let output = Command::new("curl")
        .args([
            "-s",
            "-m",
            "12",
            "-H",
            &format!("Authorization: Bearer {token}"),
            "-H",
            "anthropic-beta: oauth-2025-04-20",
            "https://api.anthropic.com/api/oauth/usage",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let body = String::from_utf8_lossy(&output.stdout);
    Some(Limits {
        session: f32_after(&body, "\"five_hour\"", "\"utilization\"")?,
        week: f32_after(&body, "\"seven_day\"", "\"utilization\"")?,
        session_reset: reset_after_any(
            &body,
            "\"five_hour\"",
            &["\"resets_at\"", "\"reset_at\"", "\"resetAt\""],
        ),
    })
}

/// The Claude Code OAuth access token, from the macOS keychain or the dotfile.
fn claude_token() -> Option<String> {
    let raw = Command::new("security")
        .args(["find-generic-password", "-s", "Claude Code-credentials", "-w"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .or_else(|| {
            let home = std::env::var_os("HOME")?;
            std::fs::read_to_string(PathBuf::from(home).join(".claude").join(".credentials.json"))
                .ok()
        })?;
    let key = "\"accessToken\":\"";
    let start = raw.find(key)? + key.len();
    let token: String = raw[start..].chars().take_while(|&c| c != '"').collect();
    (!token.is_empty()).then_some(token)
}

// ---------------- shared ----------------

/// Parse the first number for `key` that appears after `anchor` in `s`, e.g.
/// the `used_percent` inside the `"primary"` object.
fn f32_after(s: &str, anchor: &str, key: &str) -> Option<f32> {
    let from = &s[s.find(anchor)?..];
    let after = &from[from.find(key)? + key.len()..];
    let num: String = after
        .chars()
        .skip_while(|c| !c.is_ascii_digit() && *c != '-')
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    num.parse().ok()
}

fn reset_after_any(s: &str, anchor: &str, keys: &[&str]) -> Option<ResetTime> {
    keys.iter().find_map(|key| reset_after(s, anchor, key))
}

fn reset_after(s: &str, anchor: &str, key: &str) -> Option<ResetTime> {
    let from = &s[s.find(anchor)?..];
    let after = &from[from.find(key)? + key.len()..];
    let value = after[after.find(':')? + 1..].trim_start();
    if let Some(rest) = value.strip_prefix('"') {
        let label: String = rest
            .chars()
            .scan(false, |escaped, c| {
                if *escaped {
                    *escaped = false;
                    Some(Some(c))
                } else if c == '\\' {
                    *escaped = true;
                    Some(None)
                } else if c == '"' {
                    None
                } else {
                    Some(Some(c))
                }
            })
            .flatten()
            .collect();
        return (!label.is_empty()).then_some(ResetTime::Label(label));
    }
    let num: String = after
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    let epoch = num.parse().ok()?;
    // Guard against strings like "3:29pm" being accidentally treated as epoch
    // seconds. Real reset timestamps are modern Unix seconds.
    (epoch >= 1_000_000_000).then_some(ResetTime::EpochSeconds(epoch))
}
