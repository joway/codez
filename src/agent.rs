//! Discovery of coding-agent sessions on disk (Codex and Claude Code).
//!
//! Both tools record each session as a JSON-lines transcript, but in different
//! layouts:
//!
//! * **Codex** — `~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<uuid>.jsonl`. The
//!   first line is a `session_meta` object carrying the `id`, the `cwd`, and a
//!   `timestamp`.
//! * **Claude Code** — `~/.claude/projects/<encoded-cwd>/<uuid>.jsonl`, where
//!   the directory is the cwd with `/` and `.` replaced by `-`. The session id
//!   is the file stem; the transcript lines carry `timestamp` and the first
//!   `user` message holds the human's opening prompt.
//!
//! We list the sessions whose working directory matches the open folder so the
//! Agent pane can resume them with the right CLI.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AgentKind {
    Codex,
    ClaudeCode,
}

impl AgentKind {
    pub fn label(self) -> &'static str {
        match self {
            AgentKind::Codex => "codex",
            AgentKind::ClaudeCode => "claude",
        }
    }
}

pub struct AgentSession {
    pub kind: AgentKind,
    /// Session id, passed to the resume command.
    pub id: String,
    /// ISO-8601 start timestamp (also the sort key).
    pub started: String,
    /// First human prompt, when recoverable (Claude Code only); else empty.
    pub summary: String,
}

impl AgentSession {
    /// The shell command line that resumes this session.
    pub fn resume_command(&self) -> String {
        match self.kind {
            AgentKind::Codex => format!("codex resume {}", self.id),
            AgentKind::ClaudeCode => format!("claude --resume {}", self.id),
        }
    }
}

/// List Codex and Claude Code sessions recorded for `root`, most recent first.
pub fn scan(root: &Path) -> Vec<AgentSession> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    let want = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let want = want.to_string_lossy().into_owned();

    let mut out = Vec::new();
    scan_codex(&home.join(".codex").join("sessions"), &want, &mut out);
    scan_claude(&home.join(".claude").join("projects"), &want, &mut out);
    out.sort_by(|a, b| b.started.cmp(&a.started));
    out
}

/// Format a start timestamp for display: `2026-03-05T01:43:51.579Z` → `2026-03-05 01:43`.
pub fn format_started(ts: &str) -> String {
    if ts.len() >= 16 && ts.is_char_boundary(16) {
        format!("{} {}", &ts[..10], &ts[11..16])
    } else {
        ts.to_string()
    }
}

// ---------------- Codex ----------------

fn scan_codex(dir: &Path, want_cwd: &str, out: &mut Vec<AgentSession>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_codex(&path, want_cwd, out); // YYYY/MM/DD nesting
        } else if is_jsonl(&path) {
            if let Some(session) = parse_codex(&path, want_cwd) {
                out.push(session);
            }
        }
    }
}

fn parse_codex(path: &Path, want_cwd: &str) -> Option<AgentSession> {
    let file = File::open(path).ok()?;
    let mut line = String::new();
    BufReader::new(file).read_line(&mut line).ok()?;
    if !line.contains("\"type\":\"session_meta\"") || json_str(&line, "cwd")? != want_cwd {
        return None;
    }
    Some(AgentSession {
        kind: AgentKind::Codex,
        id: json_str(&line, "id")?,
        started: json_str(&line, "timestamp").unwrap_or_default(),
        summary: String::new(),
    })
}

// ---------------- Claude Code ----------------

fn scan_claude(projects_dir: &Path, want_cwd: &str, out: &mut Vec<AgentSession>) {
    let dir = projects_dir.join(encode_project(want_cwd));
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if is_jsonl(&path) {
            if let Some(session) = parse_claude(&path) {
                out.push(session);
            }
        }
    }
}

/// Claude Code's per-project directory name: the cwd with `/` and `.` → `-`.
fn encode_project(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c == '/' || c == '.' { '-' } else { c })
        .collect()
}

fn parse_claude(path: &Path) -> Option<AgentSession> {
    let id = path.file_stem()?.to_str()?.to_string();
    let mut reader = BufReader::new(File::open(path).ok()?);
    let mut started = String::new();
    let mut summary = String::new();
    let mut line = String::new();
    // The opening prompt is near the top; cap the scan so huge transcripts stay cheap.
    for _ in 0..120 {
        line.clear();
        if reader.read_line(&mut line).ok()? == 0 {
            break;
        }
        if started.is_empty() {
            if let Some(ts) = json_str(&line, "timestamp") {
                started = ts;
            }
        }
        if summary.is_empty() && line.contains("\"type\":\"user\"") {
            // `content` is a plain string only for genuine text messages; tool
            // results use an array, which `json_str` skips. Drop slash-command
            // expansions (`<command-…>`).
            if let Some(content) = json_str(&line, "content") {
                let text = content.trim();
                if !text.is_empty() && !text.starts_with('<') {
                    summary = text.lines().next().unwrap_or("").to_string();
                }
            }
        }
        if !started.is_empty() && !summary.is_empty() {
            break;
        }
    }
    Some(AgentSession {
        kind: AgentKind::ClaudeCode,
        id,
        started,
        summary,
    })
}

// ---------------- shared ----------------

fn is_jsonl(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("jsonl")
}

/// Extract the string value for `"key":"..."` from a JSON line, decoding the
/// common escape sequences. Returns `None` if the value isn't a string (e.g.
/// `"content":[...]`).
fn json_str(s: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\":\"");
    let start = s.find(&pat)? + pat.len();
    let mut out = String::new();
    let mut chars = s[start..].chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        out.push(ch);
                    }
                }
                other => out.push(other),
            },
            _ => out.push(c),
        }
    }
    None
}


