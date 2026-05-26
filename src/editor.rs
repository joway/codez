//! A multi-caret, virtualized text editor (Sublime-style foundation).
//!
//! Text lives in a `ropey::Rope` so edits are O(log n) even in large files.
//! Carets are a `Vec<Caret>` from the start — every edit/movement is applied to
//! all of them — so multi-cursor is built in rather than bolted on. Rendering
//! stays virtualized: only the visible rows are laid out and painted each frame.
//!
//! Implemented this phase: typing/IME, Enter/Backspace/Delete, arrow movement
//! with Shift-selection, Home/End, click / Shift-click / Cmd-click (add caret) /
//! drag-select, Cmd+A, copy/cut/paste, Esc to collapse. Deferred: undo/redo,
//! Cmd+D (select next occurrence), word-wise motion.

use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};

use egui::text::{CCursor, LayoutJob};
use egui::{Align2, Color32, Pos2, Rect, Sense, TextFormat, TextStyle, Vec2};
use ropey::Rope;

use crate::highlight::{self, Lang};
use crate::theme;

const CARET_COLOR: Color32 = Color32::from_rgb(0xe6, 0xed, 0xf3);

/// One caret with an optional selection (`anchor != head`). Positions are char
/// indices into the rope; `head` is the moving end.
#[derive(Clone, Copy)]
struct Caret {
    anchor: usize,
    head: usize,
    /// Target column for vertical movement (preserved across short lines).
    goal_col: Option<usize>,
}

impl Caret {
    fn point(p: usize) -> Self {
        Caret { anchor: p, head: p, goal_col: None }
    }
    fn min(&self) -> usize {
        self.anchor.min(self.head)
    }
    fn max(&self) -> usize {
        self.anchor.max(self.head)
    }
    fn is_empty(&self) -> bool {
        self.anchor == self.head
    }
}

/// An undo/redo point. Rope clones are cheap (structural sharing + CoW), so
/// snapshotting per edit-group is affordable even for large files.
struct Snapshot {
    rope: Rope,
    carets: Vec<Caret>,
}

/// Edit categories for undo coalescing: consecutive same-kind edits collapse
/// into one undo step; `Hard` (newline/paste/etc.) always starts a new step.
#[derive(Clone, Copy, PartialEq, Eq)]
enum EditKind {
    Insert,
    Delete,
    Hard,
}

pub struct Editor {
    rope: Rope,
    carets: Vec<Caret>,
    lang: Lang,
    path: PathBuf,
    dragging: bool,
    drag: usize, // index of the caret being dragged
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    last_kind: Option<EditKind>,
    saved_undo_len: usize,
}

/// Pointer state sampled once per frame (avoids per-row interactive widgets,
/// which would steal egui focus).
struct Pointer {
    pos: Option<Pos2>,
    pressed: bool,
    down: bool,
    cmd: bool,
    shift: bool,
}

impl Editor {
    pub fn open(path: &Path) -> std::io::Result<Editor> {
        let rope = Rope::from_reader(BufReader::new(File::open(path)?))?;
        Ok(Editor {
            rope,
            carets: vec![Caret::point(0)],
            lang: Lang::from_path(path),
            path: path.to_path_buf(),
            dirty: false,
            dragging: false,
            drag: 0,
        })
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn save(&mut self) -> std::io::Result<()> {
        self.rope
            .write_to(BufWriter::new(File::create(&self.path)?))?;
        self.dirty = false;
        Ok(())
    }

    pub fn status(&self) -> String {
        let lines = self.rope.len_lines();
        let chars = self.rope.len_chars();
        let n = self.carets.len();
        let cursors = if n > 1 { format!(" · {n} cursors") } else { String::new() };
        format!("{lines} lines · {chars} chars{cursors}")
    }

    // ---- char/line helpers ----

    fn line_text(&self, line: usize) -> String {
        let mut s = self.rope.line(line).to_string();
        while s.ends_with('\n') || s.ends_with('\r') {
            s.pop();
        }
        s
    }

    fn line_len_chars(&self, line: usize) -> usize {
        let slice = self.rope.line(line);
        let mut n = slice.len_chars();
        if n > 0 && slice.char(n - 1) == '\n' {
            n -= 1;
            if n > 0 && slice.char(n - 1) == '\r' {
                n -= 1;
            }
        }
        n
    }

    fn idx_to_lc(&self, idx: usize) -> (usize, usize) {
        let line = self.rope.char_to_line(idx);
        (line, idx - self.rope.line_to_char(line))
    }

    fn lc_to_idx(&self, line: usize, col: usize) -> usize {
        let line = line.min(self.rope.len_lines().saturating_sub(1));
        self.rope.line_to_char(line) + col.min(self.line_len_chars(line))
    }

    // ---- editing (applied to every caret) ----

    fn insert(&mut self, text: &str) {
        let s = text.replace("\r\n", "\n").replace('\r', "\n");
        let n = s.chars().count();
        let mut order: Vec<usize> = (0..self.carets.len()).collect();
        order.sort_by_key(|&i| self.carets[i].min());

        let mut shift: isize = 0;
        for &i in &order {
            let a = (self.carets[i].min() as isize + shift) as usize;
            let b = (self.carets[i].max() as isize + shift) as usize;
            if b > a {
                self.rope.remove(a..b);
            }
            self.rope.insert(a, &s);
            self.carets[i] = Caret::point(a + n);
            shift += n as isize - (b - a) as isize;
        }
        self.dirty = true;
        self.normalize();
    }

    fn backspace(&mut self) {
        let mut order: Vec<usize> = (0..self.carets.len()).collect();
        order.sort_by_key(|&i| self.carets[i].min());
        let mut shift: isize = 0;
        for &i in &order {
            let a = (self.carets[i].min() as isize + shift) as usize;
            let b = (self.carets[i].max() as isize + shift) as usize;
            if b > a {
                self.rope.remove(a..b);
                self.carets[i] = Caret::point(a);
                shift -= (b - a) as isize;
            } else if a > 0 {
                self.rope.remove(a - 1..a);
                self.carets[i] = Caret::point(a - 1);
                shift -= 1;
            } else {
                self.carets[i] = Caret::point(a);
            }
        }
        self.dirty = true;
        self.normalize();
    }

    fn delete_forward(&mut self) {
        let len = self.rope.len_chars();
        let mut order: Vec<usize> = (0..self.carets.len()).collect();
        order.sort_by_key(|&i| self.carets[i].min());
        let mut shift: isize = 0;
        for &i in &order {
            let a = (self.carets[i].min() as isize + shift) as usize;
            let b = (self.carets[i].max() as isize + shift) as usize;
            if b > a {
                self.rope.remove(a..b);
                shift -= (b - a) as isize;
            } else if a < len + shift as usize {
                if a < self.rope.len_chars() {
                    self.rope.remove(a..a + 1);
                    shift -= 1;
                }
            }
            self.carets[i] = Caret::point(a);
        }
        self.dirty = true;
        self.normalize();
    }

    // ---- movement ----

    fn move_h(&mut self, dir: isize, extend: bool) {
        let len = self.rope.len_chars();
        for i in 0..self.carets.len() {
            let c = self.carets[i];
            let new_head = if !extend && !c.is_empty() {
                if dir < 0 { c.min() } else { c.max() }
            } else {
                (c.head as isize + dir).clamp(0, len as isize) as usize
            };
            self.carets[i].head = new_head;
            if !extend {
                self.carets[i].anchor = new_head;
            }
            self.carets[i].goal_col = None;
        }
        self.normalize();
    }

    fn move_v(&mut self, dir: isize, extend: bool) {
        let last_line = self.rope.len_lines().saturating_sub(1);
        for i in 0..self.carets.len() {
            let c = self.carets[i];
            let (line, col) = self.idx_to_lc(c.head);
            let goal = c.goal_col.unwrap_or(col);
            let nl = (line as isize + dir).clamp(0, last_line as isize) as usize;
            let new_head = self.lc_to_idx(nl, goal);
            self.carets[i].head = new_head;
            if !extend {
                self.carets[i].anchor = new_head;
            }
            self.carets[i].goal_col = Some(goal);
        }
        self.normalize();
    }

    fn move_home(&mut self, extend: bool) {
        for i in 0..self.carets.len() {
            let (line, _) = self.idx_to_lc(self.carets[i].head);
            let new_head = self.rope.line_to_char(line);
            self.carets[i].head = new_head;
            if !extend {
                self.carets[i].anchor = new_head;
            }
            self.carets[i].goal_col = None;
        }
        self.normalize();
    }

    fn move_end(&mut self, extend: bool) {
        for i in 0..self.carets.len() {
            let (line, _) = self.idx_to_lc(self.carets[i].head);
            let new_head = self.rope.line_to_char(line) + self.line_len_chars(line);
            self.carets[i].head = new_head;
            if !extend {
                self.carets[i].anchor = new_head;
            }
            self.carets[i].goal_col = None;
        }
        self.normalize();
    }

    fn select_all(&mut self) {
        self.carets = vec![Caret {
            anchor: 0,
            head: self.rope.len_chars(),
            goal_col: None,
        }];
    }

    fn collapse(&mut self) {
        let head = self.carets.last().map(|c| c.head).unwrap_or(0);
        self.carets = vec![Caret::point(head)];
    }

    fn copy(&self, ctx: &egui::Context) {
        let parts: Vec<String> = self
            .carets
            .iter()
            .filter(|c| !c.is_empty())
            .map(|c| self.rope.slice(c.min()..c.max()).to_string())
            .collect();
        let text = if parts.is_empty() {
            self.carets
                .iter()
                .map(|c| self.line_text(self.rope.char_to_line(c.head)))
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            parts.join("\n")
        };
        ctx.copy_text(text);
    }

    /// Sort carets and merge overlapping/duplicate ones, keeping ≥1.
    fn normalize(&mut self) {
        self.carets.sort_by_key(|c| (c.min(), c.max()));
        let mut merged: Vec<Caret> = Vec::with_capacity(self.carets.len());
        for &c in &self.carets {
            if let Some(last) = merged.last_mut() {
                let overlap = c.min() <= last.max();
                let both_points = c.is_empty() && last.is_empty();
                if both_points && c.min() == last.min() {
                    continue; // duplicate caret
                }
                if overlap && !(both_points) {
                    let anchor = last.min().min(c.min());
                    let head = last.max().max(c.max());
                    *last = Caret { anchor, head, goal_col: None };
                    continue;
                }
            }
            merged.push(c);
        }
        if merged.is_empty() {
            merged.push(Caret::point(0));
        }
        self.carets = merged;
    }

    // ---- input ----

    fn handle_keys(&mut self, ctx: &egui::Context) {
        let events = ctx.input(|i| i.events.clone());
        for ev in events {
            match ev {
                egui::Event::Text(t) if !t.is_empty() => self.insert(&t),
                egui::Event::Paste(t) => self.insert(&t),
                egui::Event::Copy => self.copy(ctx),
                egui::Event::Cut => {
                    self.copy(ctx);
                    self.backspace();
                }
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    use egui::Key::*;
                    match key {
                        Enter => self.insert("\n"),
                        Backspace => self.backspace(),
                        Delete => self.delete_forward(),
                        ArrowLeft => self.move_h(-1, modifiers.shift),
                        ArrowRight => self.move_h(1, modifiers.shift),
                        ArrowUp => self.move_v(-1, modifiers.shift),
                        ArrowDown => self.move_v(1, modifiers.shift),
                        Home => self.move_home(modifiers.shift),
                        End => self.move_end(modifiers.shift),
                        Escape => self.collapse(),
                        A if modifiers.command => self.select_all(),
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }

    fn apply_pointer(&mut self, p: &Pointer, hit: Option<usize>) {
        if let Some(idx) = hit {
            if p.pressed {
                if p.cmd {
                    self.carets.push(Caret::point(idx));
                    self.drag = self.carets.len() - 1;
                } else if p.shift {
                    let li = self.carets.len() - 1;
                    self.carets[li].head = idx;
                    self.drag = li;
                } else {
                    self.carets = vec![Caret::point(idx)];
                    self.drag = 0;
                }
                self.dragging = true;
            } else if p.down && self.dragging {
                let di = self.drag.min(self.carets.len() - 1);
                self.carets[di].head = idx;
            }
        }
        if !p.down && self.dragging {
            self.dragging = false;
            self.normalize();
        }
    }

    // ---- rendering ----

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        // Keyboard first (skip if an egui widget — e.g. a settings field — wants keys).
        if !ui.ctx().wants_keyboard_input() {
            self.handle_keys(ui.ctx());
        }

        let p = ui.ctx().input(|i| Pointer {
            pos: i.pointer.interact_pos(),
            pressed: i.pointer.primary_pressed(),
            down: i.pointer.primary_down(),
            cmd: i.modifiers.command,
            shift: i.modifiers.shift,
        });

        let font = TextStyle::Monospace.resolve(ui.style());
        let row_h = ui.text_style_height(&TextStyle::Monospace);
        let char_w = ui.fonts(|f| f.glyph_width(&font, '0'));
        let total = self.rope.len_lines();
        let num_w = ((total.max(1) as f32).log10().floor() as usize) + 1;
        let gutter_w = (num_w as f32 + 2.0) * char_w + 6.0;

        let mut hit: Option<usize> = None;
        let view = &*self; // immutable view for the render closure

        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .drag_to_scroll(false)
            .show_rows(ui, row_h, total, |ui, range| {
                for line_idx in range {
                    let text = view.line_text(line_idx);
                    let mut job = LayoutJob::default();
                    job.wrap.max_width = f32::INFINITY;
                    for sp in highlight::spans(&text, view.lang) {
                        job.append(
                            &text[sp.start..sp.end],
                            0.0,
                            TextFormat {
                                font_id: font.clone(),
                                color: sp.color,
                                ..Default::default()
                            },
                        );
                    }
                    let galley = ui.fonts(|f| f.layout_job(job));

                    let row_w = (gutter_w + galley.size().x + char_w).max(ui.available_width());
                    let (rect, _) = ui.allocate_exact_size(Vec2::new(row_w, row_h), Sense::hover());
                    let painter = ui.painter();
                    let text_origin = Pos2::new(rect.left() + gutter_w, rect.top());

                    painter.text(
                        Pos2::new(rect.left() + 4.0, rect.top()),
                        Align2::LEFT_TOP,
                        format!("{:>nw$}", line_idx + 1, nw = num_w),
                        font.clone(),
                        theme::TEXT_MUTED,
                    );

                    let line_start = view.rope.line_to_char(line_idx);
                    let llen = view.line_len_chars(line_idx);

                    // Selection highlights.
                    for c in &view.carets {
                        if c.is_empty() {
                            continue;
                        }
                        let s_line = view.rope.char_to_line(c.min());
                        let e_line = view.rope.char_to_line(c.max());
                        if line_idx < s_line || line_idx > e_line {
                            continue;
                        }
                        let start_col = if line_idx == s_line { c.min() - line_start } else { 0 };
                        let end_col = if line_idx == e_line { c.max() - line_start } else { llen };
                        let x0 = text_origin.x + col_x(&galley, start_col.min(llen));
                        let mut x1 = text_origin.x + col_x(&galley, end_col.min(llen));
                        if line_idx < e_line {
                            x1 = (text_origin.x + galley.size().x + char_w).max(x1);
                        }
                        painter.rect_filled(
                            Rect::from_min_max(Pos2::new(x0, rect.top()), Pos2::new(x1, rect.bottom())),
                            0.0,
                            theme::SELECTION_BG,
                        );
                    }

                    painter.galley(text_origin, galley.clone(), theme::TEXT);

                    // Carets on this line.
                    for c in &view.carets {
                        if view.rope.char_to_line(c.head) != line_idx {
                            continue;
                        }
                        let col = c.head - line_start;
                        let x = text_origin.x + col_x(&galley, col.min(llen));
                        painter.rect_filled(
                            Rect::from_min_max(
                                Pos2::new(x, rect.top() + 1.0),
                                Pos2::new(x + 2.0, rect.bottom() - 1.0),
                            ),
                            0.0,
                            CARET_COLOR,
                        );
                    }

                    // Hit-test for mouse interaction.
                    if (p.pressed || p.down) && hit.is_none() {
                        if let Some(pos) = p.pos {
                            if pos.y >= rect.top() && pos.y < rect.bottom() {
                                let local = pos - text_origin;
                                let cur = galley.cursor_from_pos(local);
                                hit = Some(line_start + cur.ccursor.index.min(llen));
                            }
                        }
                    }
                }
            });

        self.apply_pointer(&p, hit);

        // Keep animating while interacting so drag-select stays responsive.
        if p.down {
            ui.ctx().request_repaint();
        }
    }
}

/// X offset (relative to the galley origin) of the given char column.
fn col_x(galley: &egui::Galley, col: usize) -> f32 {
    let cursor = galley.from_ccursor(CCursor::new(col));
    galley.pos_from_cursor(&cursor).min.x
}
