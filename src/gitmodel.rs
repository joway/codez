//! Thin wrappers over libgit2 (via the `git2` crate) for the data the Git Diff
//! mode needs: commit history, per-commit file changes, and per-file patches.

use std::path::Path;

use git2::{Delta, DiffFormat, DiffOptions, Index, Oid, Repository, Signature};

pub struct CommitInfo {
    pub oid: Oid,
    pub short_id: String,
    pub summary: String,
    pub author: String,
    pub date: String,
}

pub struct FileChange {
    pub path: String,
    pub status: char,
}

/// List local working tree/index changes relative to HEAD.
pub fn workdir_changes(repo: &Repository) -> Result<Vec<FileChange>, git2::Error> {
    let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
    let mut opts = DiffOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_typechange(true);
    let diff = repo.diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut opts))?;

    let mut out = Vec::new();
    for delta in diff.deltas() {
        let path = delta
            .new_file()
            .path()
            .or_else(|| delta.old_file().path())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        out.push(FileChange {
            path,
            status: status_char(delta.status()),
        });
    }
    Ok(out)
}

pub fn commit_paths(
    repo: &Repository,
    paths: &[String],
    message: &str,
) -> Result<Oid, git2::Error> {
    let signature = repo
        .signature()
        .or_else(|_| Signature::now("Diffist", "diffist@local"))?;
    let head_commit = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
    let mut index = repo.index()?;

    if let Some(parent) = &head_commit {
        let tree = parent.tree()?;
        index.read_tree(&tree)?;
    } else {
        index = Index::new()?;
    }

    for path in paths {
        let repo_path = Path::new(path);
        if repo
            .workdir()
            .map(|root| root.join(repo_path).exists())
            .unwrap_or(false)
        {
            index.add_path(repo_path)?;
        } else {
            let _ = index.remove_path(repo_path);
        }
    }

    let tree_id = index.write_tree_to(repo)?;
    let tree = repo.find_tree(tree_id)?;
    let oid = match head_commit.as_ref() {
        Some(parent) => repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            message,
            &tree,
            &[parent],
        )?,
        None => repo.commit(Some("HEAD"), &signature, &signature, message, &tree, &[])?,
    };

    let mut real_index = repo.index()?;
    real_index.read_tree(&tree)?;
    real_index.write()?;

    Ok(oid)
}

/// The current branch's short name (e.g. `main`), or `None` when HEAD is
/// detached or the repo has no commits yet.
pub fn current_branch(repo: &Repository) -> Option<String> {
    let head = repo.head().ok()?;
    if head.is_branch() {
        head.shorthand().map(str::to_string)
    } else {
        None
    }
}

/// Push `branch` to `origin` by shelling out to the system `git`, so it reuses
/// the user's existing credential setup (ssh-agent, credential helpers) rather
/// than reimplementing auth through libgit2. Returns a short status line on
/// success, or an error message. Blocks on the network — run off the UI thread.
pub fn push_origin(workdir: &Path, branch: &str) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .current_dir(workdir)
        .args(["push", "origin", branch])
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    // git push writes its human-readable result to stderr in both cases.
    let stderr = String::from_utf8_lossy(&output.stderr);
    let last = stderr.lines().last().unwrap_or("").trim();
    if output.status.success() {
        Ok(if last.is_empty() {
            "Pushed".to_string()
        } else {
            last.to_string()
        })
    } else {
        Err(if last.is_empty() {
            format!("git push failed ({})", output.status)
        } else {
            last.to_string()
        })
    }
}

/// Walk history from HEAD (newest first), up to `max` commits.
pub fn list_commits(repo: &Repository, max: usize) -> Result<Vec<CommitInfo>, git2::Error> {
    let mut walk = repo.revwalk()?;
    walk.push_head()?;
    walk.set_sorting(git2::Sort::TIME)?;

    let mut out = Vec::new();
    for oid in walk {
        if out.len() >= max {
            break;
        }
        let oid = oid?;
        let commit = repo.find_commit(oid)?;
        let author = commit.author();
        let t = commit.time();
        out.push(CommitInfo {
            oid,
            short_id: oid.to_string()[..8].to_string(),
            summary: commit.summary().unwrap_or("").to_string(),
            author: author.name().unwrap_or("?").to_string(),
            date: format_time(t.seconds(), t.offset_minutes()),
        });
    }
    Ok(out)
}

/// List the files changed by `oid` relative to its first parent (or the empty
/// tree for a root commit).
pub fn commit_changes(repo: &Repository, oid: Oid) -> Result<Vec<FileChange>, git2::Error> {
    let commit = repo.find_commit(oid)?;
    let tree = commit.tree()?;
    let parent_tree = if commit.parent_count() > 0 {
        Some(commit.parent(0)?.tree()?)
    } else {
        None
    };
    let diff = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None)?;

    let mut out = Vec::new();
    for delta in diff.deltas() {
        let path = delta
            .new_file()
            .path()
            .or_else(|| delta.old_file().path())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        out.push(FileChange {
            path,
            status: status_char(delta.status()),
        });
    }
    Ok(out)
}

/// Build a unified-diff patch string for a single file within `oid`.
pub fn file_patch(repo: &Repository, oid: Oid, path: &str) -> Result<String, git2::Error> {
    let commit = repo.find_commit(oid)?;
    let tree = commit.tree()?;
    let parent_tree = if commit.parent_count() > 0 {
        Some(commit.parent(0)?.tree()?)
    } else {
        None
    };

    let mut opts = DiffOptions::new();
    opts.pathspec(path);
    opts.context_lines(3);
    let diff = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut opts))?;

    let mut buf = String::new();
    diff.print(DiffFormat::Patch, |_delta, _hunk, line| {
        // For content lines (+/-/context) the origin marker is not part of
        // `content`, so re-add it; header lines already contain their text.
        match line.origin() {
            '+' | '-' | ' ' => buf.push(line.origin()),
            _ => {}
        }
        buf.push_str(std::str::from_utf8(line.content()).unwrap_or(""));
        true
    })?;
    Ok(buf)
}

/// Build a unified-diff patch string for a local working tree/index file.
pub fn workdir_file_patch(repo: &Repository, path: &str) -> Result<String, git2::Error> {
    let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
    let mut opts = DiffOptions::new();
    opts.pathspec(path)
        .context_lines(3)
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_typechange(true);
    let diff = repo.diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut opts))?;

    diff_to_patch(&diff)
}

fn diff_to_patch(diff: &git2::Diff<'_>) -> Result<String, git2::Error> {
    let mut buf = String::new();
    diff.print(DiffFormat::Patch, |_delta, _hunk, line| {
        // For content lines (+/-/context) the origin marker is not part of
        // `content`, so re-add it; header lines already contain their text.
        match line.origin() {
            '+' | '-' | ' ' => buf.push(line.origin()),
            _ => {}
        }
        buf.push_str(std::str::from_utf8(line.content()).unwrap_or(""));
        true
    })?;
    Ok(buf)
}

fn status_char(s: Delta) -> char {
    match s {
        Delta::Added => 'A',
        Delta::Deleted => 'D',
        Delta::Modified => 'M',
        Delta::Renamed => 'R',
        Delta::Copied => 'C',
        Delta::Typechange => 'T',
        _ => '?',
    }
}

/// Format a unix timestamp (+ tz offset in minutes) as `YYYY-MM-DD HH:MM`,
/// dependency-free via Howard Hinnant's civil-from-days algorithm.
fn format_time(secs: i64, offset_min: i32) -> String {
    let total = secs + offset_min as i64 * 60;
    let days = total.div_euclid(86_400);
    let rem = total.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let hh = rem / 3600;
    let mm = (rem % 3600) / 60;
    format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}")
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}
