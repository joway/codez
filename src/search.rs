//! Project-wide "Find in Files": a background search over the open folder with
//! a streaming results panel.
//!
//! The walk + match runs on a worker thread and streams per-file results back
//! over a channel, so the UI stays responsive on large projects. Starting a new
//! search cancels the previous one via a shared atomic flag.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use egui::{RichText, ScrollArea, Sense, TextEdit};

use crate::theme;

const MAX_FILE_BYTES: u64 = 2_000_000;
const MAX_TOTAL_HITS: usize = 5000;
const MAX_HITS_PER_FILE: usize = 200;
const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    ".venv",
    "vendor",
];

pub struct Hit {
    pub line: usize,
    pub col: usize,
    pub text: String,
}

pub struct FileHits {
    pub path: PathBuf,
    pub rel: String,
    pub hits: Vec<Hit>,
}

enum Msg {
    File(FileHits),
    Done,
}

/// A click on a result: open this file and reveal the match.
pub struct OpenTarget {
    pub path: PathBuf,
    pub line: usize,
    pub col: usize,
    pub len: usize,
}

pub struct ProjectSearch {
    pub open: bool,
    query: String,
    focus: bool,
    results: Vec<FileHits>,
    total: usize,
    rx: Option<Receiver<Msg>>,
    cancel: Option<Arc<AtomicBool>>,
    running: bool,
}

impl Default for ProjectSearch {
    fn default() -> Self {
        Self {
            open: false,
            query: String::new(),
            focus: false,
            results: Vec::new(),
            total: 0,
            rx: None,
            cancel: None,
            running: false,
        }
    }
}

impl ProjectSearch {
    pub fn reveal(&mut self) {
        self.open = true;
        self.focus = true;
    }

    fn start(&mut self, root: &Path) {
        if let Some(c) = &self.cancel {
            c.store(true, Ordering::Relaxed);
        }
        self.results.clear();
        self.total = 0;
        self.rx = None;
        self.running = false;

        let query = self.query.trim().to_string();
        if query.is_empty() {
            return;
        }
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(cancel.clone());
        self.rx = Some(rx);
        self.running = true;
        let root = root.to_path_buf();
        std::thread::spawn(move || run_search(root, query, tx, cancel));
    }

    fn poll(&mut self) {
        let mut done = false;
        if let Some(rx) = &self.rx {
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    Msg::File(fh) => {
                        self.total += fh.hits.len();
                        self.results.push(fh);
                    }
                    Msg::Done => {
                        done = true;
                        break;
                    }
                }
            }
        }
        if done {
            self.running = false;
            self.rx = None;
        }
    }

    /// Draw the bottom results panel. Returns a click target, if any.
    pub fn ui(&mut self, ctx: &egui::Context, root: &Path) -> Option<OpenTarget> {
        if !self.open {
            return None;
        }
        self.poll();

        let mut clicked = None;
        let mut close = false;
        let query_len = self.query.chars().count();

        egui::TopBottomPanel::bottom("find_in_files")
            .resizable(true)
            .default_height(240.0)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let resp = ui.add(
                        TextEdit::singleline(&mut self.query)
                            .hint_text("Find in files")
                            .desired_width(300.0),
                    );
                    if self.focus {
                        resp.request_focus();
                        self.focus = false;
                    }
                    let submit = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if theme::pill_button(ui, "Search") || submit {
                        self.start(root);
                    }
                    ui.label(
                        RichText::new(format!(
                            "{} matches · {} files{}",
                            self.total,
                            self.results.len(),
                            if self.running { " …" } else { "" }
                        ))
                        .color(theme::TEXT_DIM),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::icon_button(ui, "×", "Close") {
                            close = true;
                        }
                    });
                });
                ui.separator();

                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for file in &self.results {
                            // The filename header jumps to the file's first match.
                            let header = ui
                                .add(
                                    egui::Label::new(
                                        RichText::new(&file.rel)
                                            .color(theme::ACCENT)
                                            .strong()
                                            .monospace(),
                                    )
                                    .sense(Sense::click()),
                                )
                                .on_hover_cursor(egui::CursorIcon::PointingHand);
                            if header.clicked() {
                                let first = file.hits.first();
                                clicked = Some(OpenTarget {
                                    path: file.path.clone(),
                                    line: first.map_or(0, |h| h.line),
                                    col: first.map_or(0, |h| h.col),
                                    len: query_len,
                                });
                            }
                            for hit in &file.hits {
                                let label = format!("{:>5}  {}", hit.line + 1, hit.text.trim_end());
                                if ui
                                    .add(
                                        egui::Label::new(
                                            RichText::new(label).monospace().color(theme::TEXT_DIM),
                                        )
                                        .sense(Sense::click())
                                        .truncate(),
                                    )
                                    .clicked()
                                {
                                    clicked = Some(OpenTarget {
                                        path: file.path.clone(),
                                        line: hit.line,
                                        col: hit.col,
                                        len: query_len,
                                    });
                                }
                            }
                            ui.add_space(4.0);
                        }
                    });

                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    close = true;
                }
            });

        if close {
            self.open = false;
        }
        if self.running {
            ctx.request_repaint();
        }
        clicked
    }
}

/// Collect project files (for the file quick-open palette), skipping the same
/// noise directories and capping the count.
pub fn collect_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            let path = entry.path();
            if ft.is_dir() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if SKIP_DIRS.contains(&name.as_str()) {
                    continue;
                }
                stack.push(path);
            } else if ft.is_file() {
                out.push(path);
                if out.len() >= 20_000 {
                    return out;
                }
            }
        }
    }
    out
}

fn run_search(root: PathBuf, query: String, tx: Sender<Msg>, cancel: Arc<AtomicBool>) {
    let ql = query.to_lowercase();
    let mut stack = vec![root.clone()];
    let mut total = 0usize;

    while let Some(dir) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            let path = entry.path();
            if ft.is_dir() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if SKIP_DIRS.contains(&name.as_str()) {
                    continue;
                }
                stack.push(path);
            } else if ft.is_file() {
                if let Some(fh) = search_file(&path, &root, &ql) {
                    total += fh.hits.len();
                    if tx.send(Msg::File(fh)).is_err() {
                        return;
                    }
                    if total >= MAX_TOTAL_HITS {
                        let _ = tx.send(Msg::Done);
                        return;
                    }
                }
            }
        }
    }
    let _ = tx.send(Msg::Done);
}

fn search_file(path: &Path, root: &Path, ql: &str) -> Option<FileHits> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_FILE_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes.iter().take(1024).any(|&b| b == 0) {
        return None; // binary
    }
    let text = String::from_utf8(bytes).ok()?;

    let mut hits = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let lower = line.to_lowercase();
        if let Some(b) = lower.find(ql) {
            let col = lower[..b].chars().count();
            let text: String = line.chars().take(300).collect();
            hits.push(Hit { line: i, col, text });
            if hits.len() >= MAX_HITS_PER_FILE {
                break;
            }
        }
    }
    if hits.is_empty() {
        return None;
    }
    let rel = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned();
    Some(FileHits {
        path: path.to_path_buf(),
        rel,
        hits,
    })
}
