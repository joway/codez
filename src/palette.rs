//! Centered command palette: fuzzy file quick-open (⌘P), `:line` go-to-line,
//! and a command palette (⌘⇧P). One overlay, three modes.

use std::path::PathBuf;

use egui::{Align2, FontId, Key, Pos2, ScrollArea, Sense, TextEdit, Vec2};

use crate::search;
use crate::theme;

const MAX_RESULTS: usize = 50;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Command {
    OpenFolder,
    Save,
    ModeEditor,
    ModeDiff,
    FindInFiles,
    Settings,
}

impl Command {
    const ALL: &'static [Command] = &[
        Command::OpenFolder,
        Command::Save,
        Command::ModeEditor,
        Command::ModeDiff,
        Command::FindInFiles,
        Command::Settings,
    ];

    fn label(self) -> &'static str {
        match self {
            Command::OpenFolder => "Open Folder…",
            Command::Save => "Save",
            Command::ModeEditor => "View: Editor",
            Command::ModeDiff => "View: Diff",
            Command::FindInFiles => "Find in Files",
            Command::Settings => "Settings",
        }
    }
}

pub enum PaletteAction {
    OpenFile(PathBuf),
    GotoLine(usize),
    Run(Command),
}

struct Entry {
    label: String,
    action: PaletteAction,
}

pub struct Palette {
    pub open: bool,
    commands: bool,
    query: String,
    focus: bool,
    selected: usize,
    files: Vec<(PathBuf, String)>, // (absolute, relative display)
    files_root: Option<PathBuf>,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            open: false,
            commands: false,
            query: String::new(),
            focus: false,
            selected: 0,
            files: Vec::new(),
            files_root: None,
        }
    }
}

impl Palette {
    pub fn open_files(&mut self, root: &std::path::Path) {
        if self.files_root.as_deref() != Some(root) || self.files.is_empty() {
            self.files = search::collect_files(root)
                .into_iter()
                .map(|p| {
                    let rel = p
                        .strip_prefix(root)
                        .unwrap_or(&p)
                        .to_string_lossy()
                        .into_owned();
                    (p, rel)
                })
                .collect();
            self.files_root = Some(root.to_path_buf());
        }
        self.commands = false;
        self.begin();
    }

    pub fn open_commands(&mut self) {
        self.commands = true;
        self.begin();
    }

    fn begin(&mut self) {
        self.open = true;
        self.focus = true;
        self.query.clear();
        self.selected = 0;
    }

    fn entries(&self) -> Vec<Entry> {
        // Go-to-line: ⌘P then ":N".
        if !self.commands {
            if let Some(rest) = self.query.strip_prefix(':') {
                if let Ok(n) = rest.trim().parse::<usize>() {
                    return vec![Entry {
                        label: format!("Go to line {n}"),
                        action: PaletteAction::GotoLine(n.max(1)),
                    }];
                }
                return vec![Entry {
                    label: "Go to line…".to_string(),
                    action: PaletteAction::GotoLine(1),
                }];
            }
        }

        let q = self.query.trim().to_lowercase();
        if self.commands {
            let mut scored: Vec<(i32, Command)> = Command::ALL
                .iter()
                .filter_map(|&c| fuzzy(&c.label().to_lowercase(), &q).map(|s| (s, c)))
                .collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0));
            scored
                .into_iter()
                .map(|(_, c)| Entry {
                    label: c.label().to_string(),
                    action: PaletteAction::Run(c),
                })
                .collect()
        } else {
            let mut scored: Vec<(i32, usize)> = self
                .files
                .iter()
                .enumerate()
                .filter_map(|(i, (_, rel))| fuzzy(&rel.to_lowercase(), &q).map(|s| (s, i)))
                .collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            scored
                .into_iter()
                .take(MAX_RESULTS)
                .map(|(_, i)| Entry {
                    label: self.files[i].1.clone(),
                    action: PaletteAction::OpenFile(self.files[i].0.clone()),
                })
                .collect()
        }
    }

    pub fn ui(&mut self, ctx: &egui::Context) -> Option<PaletteAction> {
        if !self.open {
            return None;
        }
        let mut entries = self.entries();
        let mut chosen: Option<usize> = None;
        let mut close = false;

        // Keyboard navigation (the text field ignores up/down).
        ctx.input(|i| {
            if i.key_pressed(Key::ArrowDown) {
                self.selected += 1;
            }
            if i.key_pressed(Key::ArrowUp) && self.selected > 0 {
                self.selected -= 1;
            }
            if i.key_pressed(Key::Escape) {
                close = true;
            }
            if i.key_pressed(Key::Enter) {
                chosen = Some(self.selected);
            }
        });
        if !entries.is_empty() {
            self.selected = self.selected.min(entries.len() - 1);
        }

        let title = if self.commands { "Command Palette" } else { "Go to File" };
        egui::Window::new(title)
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_TOP, [0.0, 90.0])
            .fixed_size([620.0, 0.0])
            .show(ctx, |ui| {
                let hint = if self.commands {
                    "Type a command"
                } else {
                    "Search files by name, or :line"
                };
                let resp = ui.add(
                    TextEdit::singleline(&mut self.query)
                        .hint_text(hint)
                        .desired_width(f32::INFINITY),
                );
                if self.focus {
                    resp.request_focus();
                    self.focus = false;
                }
                if resp.changed() {
                    self.selected = 0;
                }
                ui.add_space(6.0);

                let body = egui::TextStyle::Body.resolve(ui.style()).size;
                let row_h = 32.0;
                // Fixed-height list (VS Code-style): the box stays this tall even
                // with few results, instead of collapsing to fit.
                ui.allocate_ui(Vec2::new(ui.available_width(), 420.0), |ui| {
                    ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            for (idx, entry) in entries.iter().enumerate() {
                                let selected = idx == self.selected;
                                let (rect, resp) = ui.allocate_exact_size(
                                    Vec2::new(ui.available_width(), row_h),
                                    Sense::click(),
                                );
                                if selected {
                                    ui.painter().rect_filled(rect, 5.0, theme::SIDEBAR_SELECTED);
                                } else if resp.hovered() {
                                    ui.painter().rect_filled(rect, 5.0, theme::SIDEBAR_HOVER);
                                }
                                let x = rect.left() + 10.0;
                                let cy = rect.center().y;
                                match &entry.action {
                                    PaletteAction::OpenFile(_) => {
                                        // filename in full, parent directory dimmed beside it.
                                        let (dir, name) = split_path(&entry.label);
                                        let galley = ui.painter().layout_no_wrap(
                                            name,
                                            FontId::proportional(body),
                                            theme::TEXT,
                                        );
                                        let nw = galley.size().x;
                                        ui.painter().galley(
                                            Pos2::new(x, cy - galley.size().y / 2.0),
                                            galley,
                                            theme::TEXT,
                                        );
                                        if !dir.is_empty() {
                                            ui.painter().text(
                                                Pos2::new(x + nw + 8.0, cy),
                                                Align2::LEFT_CENTER,
                                                dir,
                                                FontId::proportional(body * 0.82),
                                                theme::TEXT_MUTED,
                                            );
                                        }
                                    }
                                    _ => {
                                        ui.painter().text(
                                            Pos2::new(x, cy),
                                            Align2::LEFT_CENTER,
                                            &entry.label,
                                            FontId::proportional(body),
                                            theme::TEXT,
                                        );
                                    }
                                }
                                if resp.clicked() {
                                    chosen = Some(idx);
                                }
                                if selected {
                                    resp.scroll_to_me(None);
                                }
                            }
                            if entries.is_empty() {
                                ui.add_space(10.0);
                                ui.weak("  No matches");
                            }
                        });
                });
            });

        if close {
            self.open = false;
            return None;
        }
        if let Some(idx) = chosen {
            if idx < entries.len() {
                self.open = false;
                return Some(entries.swap_remove(idx).action);
            }
        }
        None
    }
}

/// Split a relative path into `(parent_dir, file_name)` for VS Code-style rows.
fn split_path(rel: &str) -> (String, String) {
    match rel.rsplit_once('/') {
        Some((dir, name)) => (dir.to_string(), name.to_string()),
        None => (String::new(), rel.to_string()),
    }
}

/// Subsequence fuzzy match with boundary/contiguity bonuses. Returns `None` if
/// `needle` is not a subsequence of `hay` (both already lowercased).
fn fuzzy(hay: &str, needle: &str) -> Option<i32> {
    if needle.is_empty() {
        return Some(0);
    }
    let h: Vec<char> = hay.chars().collect();
    let mut hi = 0usize;
    let mut score = 0i32;
    let mut prev: Option<usize> = None;
    for nc in needle.chars() {
        let mut found = None;
        while hi < h.len() {
            if h[hi] == nc {
                found = Some(hi);
                break;
            }
            hi += 1;
        }
        let f = found?;
        score += 10;
        if let Some(p) = prev {
            if f == p + 1 {
                score += 15;
            }
        }
        if f == 0 || matches!(h[f - 1], '/' | '_' | '-' | '.' | ' ') {
            score += 12;
        }
        prev = Some(f);
        hi = f + 1;
    }
    Some(score - h.len() as i32 / 8)
}
