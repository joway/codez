//! A multi-caret, virtualized text editor (Sublime-style foundation).
//!
//! Text lives in a `ropey::Rope` so edits are O(log n) even in large files.
//! Carets are a `Vec<Caret>` from the start — every edit/movement is applied to
//! all of them — so multi-cursor is built in rather than bolted on. Rendering
//! stays virtualized: only the visible rows are laid out and painted each frame.
//!
//! Implemented: typing/IME, Enter/Backspace/Delete, undo/redo, multi-caret
//! selection/editing, Sublime-style Cmd+D next occurrence, line commands,
//! indentation, word-wise motion, copy/cut/paste, and mouse selection.

use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};

use egui::text::{CCursor, LayoutJob};
use egui::{Align2, Color32, Pos2, Rect, Sense, TextFormat, TextStyle, Vec2};
use ropey::Rope;

use crate::highlight::{self, Lang};
use crate::theme;

const CARET_COLOR: Color32 = Color32::from_rgb(0xe6, 0xed, 0xf3);
const DOUBLE_CLICK_SECONDS: f64 = 0.35;

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
        Caret {
            anchor: p,
            head: p,
            goal_col: None,
        }
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

/// Find/replace state for the current file.
struct Search {
    open: bool,
    replace: bool,
    query: String,
    replacement: String,
    matches: Vec<(usize, usize)>, // char ranges
    current: Option<usize>,
    needs_recompute: bool,
    focus: bool,
}

impl Default for Search {
    fn default() -> Self {
        Self {
            open: false,
            replace: false,
            query: String::new(),
            replacement: String::new(),
            matches: Vec::new(),
            current: None,
            needs_recompute: true,
            focus: false,
        }
    }
}

pub struct Editor {
    rope: Rope,
    carets: Vec<Caret>,
    lang: Lang,
    path: PathBuf,
    dirty: bool,
    /// In-progress IME composition (CJK input), shown inline at the primary
    /// caret until the input method commits it.
    preedit: String,
    dragging: bool,
    drag: usize, // index of the caret being dragged
    last_click_time: f64,
    last_click_idx: Option<usize>,
    pending_scroll_to: Option<usize>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    last_kind: Option<EditKind>,
    saved_undo_len: usize,
    search: Search,
}

/// Pointer state sampled once per frame (avoids per-row interactive widgets,
/// which would steal egui focus).
struct Pointer {
    pos: Option<Pos2>,
    pressed: bool,
    down: bool,
    cmd: bool,
    shift: bool,
    time: f64,
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
            preedit: String::new(),
            dragging: false,
            drag: 0,
            last_click_time: f64::NEG_INFINITY,
            last_click_idx: None,
            pending_scroll_to: None,
            undo: Vec::new(),
            redo: Vec::new(),
            last_kind: None,
            saved_undo_len: 0,
            search: Search::default(),
        })
    }

    pub fn is_dirty(&self) -> bool {
        self.undo.len() != self.saved_undo_len
    }

    /// Select the matched range at `line`/`col` and scroll it into view
    /// (used when jumping from a Find-in-Files result).
    pub fn reveal_match(&mut self, line: usize, col: usize, len: usize) {
        let line = line.min(self.rope.len_lines().saturating_sub(1));
        let start = (self.rope.line_to_char(line) + col).min(self.rope.len_chars());
        let end = (start + len).min(self.rope.len_chars());
        self.carets = vec![Caret {
            anchor: start,
            head: end,
            goal_col: None,
        }];
        self.pending_scroll_to = Some(start);
    }

    pub fn save(&mut self) -> std::io::Result<()> {
        self.rope
            .write_to(BufWriter::new(File::create(&self.path)?))?;
        self.saved_undo_len = self.undo.len();
        self.dirty = false;
        Ok(())
    }

    pub fn status(&self) -> String {
        let lines = self.rope.len_lines();
        let chars = self.rope.len_chars();
        let n = self.carets.len();
        let cursors = if n > 1 {
            format!(" · {n} cursors")
        } else {
            String::new()
        };
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

    fn line_end_idx(&self, line: usize, include_newline: bool) -> usize {
        let mut end = self.rope.line_to_char(line) + self.line_len_chars(line);
        if include_newline && line + 1 < self.rope.len_lines() {
            let next = self.rope.line_to_char(line + 1);
            end = next;
        }
        end
    }

    fn selected_lines(&self) -> Vec<usize> {
        let mut lines = Vec::new();
        let last = self.rope.len_lines().saturating_sub(1);
        for c in &self.carets {
            let start = self.rope.char_to_line(c.min().min(self.rope.len_chars()));
            let mut end_pos = c.max().min(self.rope.len_chars());
            if !c.is_empty()
                && end_pos > 0
                && end_pos == self.rope.line_to_char(self.rope.char_to_line(end_pos))
            {
                end_pos -= 1;
            }
            let end = self.rope.char_to_line(end_pos).min(last);
            for line in start..=end {
                lines.push(line);
            }
        }
        lines.sort_unstable();
        lines.dedup();
        lines
    }

    fn current_snapshot(&self) -> Snapshot {
        Snapshot {
            rope: self.rope.clone(),
            carets: self.carets.clone(),
        }
    }

    fn restore_snapshot(&mut self, snapshot: Snapshot) {
        self.rope = snapshot.rope;
        self.carets = snapshot.carets;
        self.dragging = false;
        self.last_kind = None;
        self.dirty = self.is_dirty();
    }

    fn begin_edit(&mut self, kind: EditKind) {
        let coalesce =
            self.last_kind == Some(kind) && kind != EditKind::Hard && self.redo.is_empty();
        if !coalesce {
            self.undo.push(self.current_snapshot());
        }
        self.redo.clear();
        self.last_kind = Some(kind);
        self.search.needs_recompute = true;
    }

    fn undo(&mut self) {
        if let Some(snapshot) = self.undo.pop() {
            self.redo.push(self.current_snapshot());
            self.restore_snapshot(snapshot);
        }
    }

    fn redo(&mut self) {
        if let Some(snapshot) = self.redo.pop() {
            self.undo.push(self.current_snapshot());
            self.restore_snapshot(snapshot);
        }
    }

    // ---- editing (applied to every caret) ----

    fn insert(&mut self, text: &str) {
        let kind = if text == "\n" || text.chars().count() > 1 {
            EditKind::Hard
        } else {
            EditKind::Insert
        };
        self.insert_with_kind(text, kind);
    }

    fn insert_with_kind(&mut self, text: &str, kind: EditKind) {
        let s = text.replace("\r\n", "\n").replace('\r', "\n");
        if s.is_empty() {
            return;
        }
        self.begin_edit(kind);
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
        if self.carets.iter().all(|c| c.is_empty() && c.head == 0) {
            return;
        }
        self.begin_edit(EditKind::Delete);
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
                let prev = self.rope.char(a - 1);
                let pair = a < self.rope.len_chars()
                    && matching_close(prev) == Some(self.rope.char(a));
                if pair {
                    self.rope.remove(a - 1..a + 1);
                    self.carets[i] = Caret::point(a - 1);
                    shift -= 2;
                } else {
                    self.rope.remove(a - 1..a);
                    self.carets[i] = Caret::point(a - 1);
                    shift -= 1;
                }
            } else {
                self.carets[i] = Caret::point(a);
            }
        }
        self.dirty = true;
        self.normalize();
    }

    fn delete_forward(&mut self) {
        let len = self.rope.len_chars();
        if self.carets.iter().all(|c| c.is_empty() && c.head >= len) {
            return;
        }
        self.begin_edit(EditKind::Delete);
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

    /// Route typed text: single bracket/quote chars get auto-pairing, the rest
    /// is a plain insert.
    fn type_text(&mut self, t: &str) {
        let mut chars = t.chars();
        let (first, second) = (chars.next(), chars.next());
        if let (Some(c), None) = (first, second) {
            if matching_close(c).is_some() {
                self.type_open(c);
                return;
            }
            if is_closer(c) {
                self.type_close(c);
                return;
            }
        }
        self.insert(t);
    }

    /// Type an opener (or quote): wrap a selection, over-type a matching quote,
    /// otherwise insert the pair and place the caret between.
    fn type_open(&mut self, open: char) {
        let close = matching_close(open).unwrap_or(open);

        // Quote over-type: typing " just before an existing " steps over it.
        if open == close
            && self.carets.iter().all(|c| {
                c.is_empty() && c.head < self.rope.len_chars() && self.rope.char(c.head) == open
            })
        {
            self.last_kind = None;
            for i in 0..self.carets.len() {
                let h = self.carets[i].head + 1;
                self.carets[i] = Caret::point(h);
            }
            self.normalize();
            return;
        }

        self.begin_edit(EditKind::Hard);
        let mut order: Vec<usize> = (0..self.carets.len()).collect();
        order.sort_by_key(|&i| self.carets[i].min());
        let mut shift: isize = 0;
        for &i in &order {
            let a = (self.carets[i].min() as isize + shift) as usize;
            let b = (self.carets[i].max() as isize + shift) as usize;
            if b > a {
                // Wrap the selection, preserving its direction.
                self.rope.insert(b, &close.to_string());
                self.rope.insert(a, &open.to_string());
                let forward = self.carets[i].head >= self.carets[i].anchor;
                self.carets[i] = if forward {
                    Caret { anchor: a + 1, head: b + 1, goal_col: None }
                } else {
                    Caret { anchor: b + 1, head: a + 1, goal_col: None }
                };
            } else {
                self.rope.insert(a, &format!("{open}{close}"));
                self.carets[i] = Caret::point(a + 1);
            }
            shift += 2;
        }
        self.dirty = true;
        self.normalize();
    }

    /// Type a closer: step over an existing matching closer, else insert it.
    fn type_close(&mut self, close: char) {
        let over = self.carets.iter().all(|c| {
            c.is_empty() && c.head < self.rope.len_chars() && self.rope.char(c.head) == close
        });
        if over && !self.carets.is_empty() {
            self.last_kind = None;
            for i in 0..self.carets.len() {
                let h = self.carets[i].head + 1;
                self.carets[i] = Caret::point(h);
            }
            self.normalize();
        } else {
            self.insert(&close.to_string());
        }
    }

    /// Newline that inherits the current line's indent, adding one level when
    /// splitting an empty bracket pair `{|}`.
    fn newline_smart(&mut self) {
        self.begin_edit(EditKind::Hard);
        let mut order: Vec<usize> = (0..self.carets.len()).collect();
        order.sort_by_key(|&i| self.carets[i].min());
        let mut shift: isize = 0;
        for &i in &order {
            let a = (self.carets[i].min() as isize + shift) as usize;
            let b = (self.carets[i].max() as isize + shift) as usize;
            if b > a {
                self.rope.remove(a..b);
            }
            let line = self.rope.char_to_line(a);
            let indent: String = self
                .rope
                .line(line)
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            let before = (a > 0).then(|| self.rope.char(a - 1));
            let after = (a < self.rope.len_chars()).then(|| self.rope.char(a));
            let expand = matches!(
                (before, after),
                (Some('{'), Some('}')) | (Some('['), Some(']')) | (Some('('), Some(')'))
            );
            let inner = format!("{indent}    ");
            let text = if expand {
                format!("\n{inner}\n{indent}")
            } else {
                format!("\n{indent}")
            };
            let n = text.chars().count();
            self.rope.insert(a, &text);
            let caret = if expand {
                a + 1 + inner.chars().count()
            } else {
                a + n
            };
            self.carets[i] = Caret::point(caret);
            shift += n as isize - (b - a) as isize;
        }
        self.dirty = true;
        self.normalize();
    }

    // ---- movement ----

    fn move_h(&mut self, dir: isize, extend: bool) {
        self.last_kind = None;
        let len = self.rope.len_chars();
        for i in 0..self.carets.len() {
            let c = self.carets[i];
            let new_head = if !extend && !c.is_empty() {
                if dir < 0 {
                    c.min()
                } else {
                    c.max()
                }
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
        self.last_kind = None;
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
        self.last_kind = None;
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
        self.last_kind = None;
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

    fn move_word(&mut self, dir: isize, extend: bool) {
        let text = self.rope.to_string();
        let chars: Vec<char> = text.chars().collect();
        for i in 0..self.carets.len() {
            let c = self.carets[i];
            let from = if !extend && !c.is_empty() {
                if dir < 0 {
                    c.min()
                } else {
                    c.max()
                }
            } else {
                c.head
            };
            let new_head = word_boundary(&chars, from, dir);
            self.carets[i].head = new_head;
            if !extend {
                self.carets[i].anchor = new_head;
            }
            self.carets[i].goal_col = None;
        }
        self.last_kind = None;
        self.normalize();
    }

    fn select_all(&mut self) {
        self.last_kind = None;
        self.carets = vec![Caret {
            anchor: 0,
            head: self.rope.len_chars(),
            goal_col: None,
        }];
    }

    fn collapse(&mut self) {
        self.last_kind = None;
        let head = self.carets.last().map(|c| c.head).unwrap_or(0);
        self.carets = vec![Caret::point(head)];
    }

    fn selected_text(&self) -> Option<String> {
        self.carets
            .iter()
            .rev()
            .find(|c| !c.is_empty())
            .map(|c| self.rope.slice(c.min()..c.max()).to_string())
            .filter(|s| !s.is_empty())
    }

    fn word_range_at_char(&self, idx: usize) -> Option<(usize, usize)> {
        let head = idx.min(self.rope.len_chars());
        let text = self.rope.to_string();
        let chars: Vec<char> = text.chars().collect();
        if chars.is_empty() {
            return None;
        }

        let mut pos = head.min(chars.len().saturating_sub(1));
        if pos == chars.len() || !is_word_char(chars[pos]) {
            if pos > 0 && is_word_char(chars[pos - 1]) {
                pos -= 1;
            } else {
                return None;
            }
        }

        let mut start = pos;
        while start > 0 && is_word_char(chars[start - 1]) {
            start -= 1;
        }
        let mut end = pos + 1;
        while end < chars.len() && is_word_char(chars[end]) {
            end += 1;
        }

        Some((start, end))
    }

    fn select_word_at_char(&mut self, idx: usize, scroll_to_word: bool) -> Option<String> {
        let (start, end) = self.word_range_at_char(idx)?;
        let word = self.rope.slice(start..end).to_string();
        self.carets = vec![Caret {
            anchor: start,
            head: end,
            goal_col: None,
        }];
        self.drag = 0;
        self.dragging = false;
        self.last_kind = None;
        if scroll_to_word {
            self.pending_scroll_to = Some(start);
        }
        Some(word)
    }

    fn select_word_at_last_caret(&mut self) -> Option<String> {
        let head = self
            .carets
            .last()
            .map(|c| c.head)
            .unwrap_or(0)
            .min(self.rope.len_chars());
        let (start, end) = self.word_range_at_char(head)?;
        let word = self.rope.slice(start..end).to_string();
        if let Some(last) = self.carets.last_mut() {
            *last = Caret {
                anchor: start,
                head: end,
                goal_col: None,
            };
        }
        self.pending_scroll_to = Some(start);
        Some(word)
    }

    fn select_next_occurrence(&mut self) {
        self.last_kind = None;
        let needle = match self.selected_text() {
            Some(s) => s,
            None => match self.select_word_at_last_caret() {
                Some(s) => s,
                None => return,
            },
        };
        let text = self.rope.to_string();
        let start_char = self.carets.iter().map(Caret::max).max().unwrap_or(0);
        let mut search_ranges = Vec::new();
        let start_byte = char_to_byte_idx(&text, start_char.min(self.rope.len_chars()));
        search_ranges.push((start_byte, text.len()));
        search_ranges.push((0, start_byte));

        for (range_start, range_end) in search_ranges {
            let haystack = &text[range_start..range_end];
            let mut offset = 0;
            while let Some(rel_idx) = haystack[offset..].find(&needle) {
                let byte_idx = range_start + offset + rel_idx;
                let start = text[..byte_idx].chars().count();
                let end = start + needle.chars().count();
                let next = Caret {
                    anchor: start,
                    head: end,
                    goal_col: None,
                };
                if !self
                    .carets
                    .iter()
                    .any(|c| c.min() == next.min() && c.max() == next.max())
                {
                    self.carets.push(next);
                    self.normalize();
                    self.pending_scroll_to = Some(start);
                    return;
                }
                offset += rel_idx + needle.len();
                if offset >= haystack.len() {
                    break;
                }
            }
        }

        if let Some(last) = self.carets.last() {
            self.pending_scroll_to = Some(last.min());
        }
    }

    fn select_lines(&mut self) {
        self.last_kind = None;
        let mut next = Vec::new();
        for line in self.selected_lines() {
            next.push(Caret {
                anchor: self.rope.line_to_char(line),
                head: self.line_end_idx(line, false),
                goal_col: None,
            });
        }
        if !next.is_empty() {
            self.carets = next;
            self.normalize();
        }
    }

    fn split_selection_into_lines(&mut self) {
        self.last_kind = None;
        let mut next = Vec::new();
        for line in self.selected_lines() {
            next.push(Caret {
                anchor: self.rope.line_to_char(line),
                head: self.line_end_idx(line, false),
                goal_col: None,
            });
        }
        if !next.is_empty() {
            self.carets = next;
        }
    }

    fn delete_lines(&mut self) {
        let lines = self.selected_lines();
        if lines.is_empty() {
            return;
        }
        self.begin_edit(EditKind::Hard);
        let target = self
            .rope
            .line_to_char(lines[0].min(self.rope.len_lines().saturating_sub(1)));
        for &line in lines.iter().rev() {
            let start = self.rope.line_to_char(line);
            let mut end = self.line_end_idx(line, true);
            if start == end && line > 0 {
                end = start;
                let prev = self.rope.line_to_char(line - 1);
                self.rope.remove(prev..end);
                continue;
            }
            self.rope.remove(start..end);
        }
        let pos = target.min(self.rope.len_chars());
        self.carets = vec![Caret::point(pos)];
        self.dirty = true;
    }

    fn duplicate_lines(&mut self) {
        let lines = self.selected_lines();
        if lines.is_empty() {
            return;
        }
        self.begin_edit(EditKind::Hard);
        let first = lines[0];
        let last = *lines.last().unwrap();
        let start = self.rope.line_to_char(first);
        let end = self.line_end_idx(last, true);
        let mut text = self.rope.slice(start..end).to_string();
        if !text.ends_with('\n') {
            text.push('\n');
        }
        self.rope.insert(end, &text);
        let offset = text.chars().count();
        self.carets = self
            .carets
            .iter()
            .map(|c| Caret {
                anchor: c.anchor + offset,
                head: c.head + offset,
                goal_col: c.goal_col,
            })
            .collect();
        self.dirty = true;
        self.normalize();
    }

    fn delete_to_line_start(&mut self) {
        if self.carets.iter().all(|c| {
            let (line, _) = self.idx_to_lc(c.head);
            c.is_empty() && c.head == self.rope.line_to_char(line)
        }) {
            return;
        }
        self.begin_edit(EditKind::Hard);
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
            } else {
                let line = self.rope.char_to_line(a);
                let start = self.rope.line_to_char(line);
                if a > start {
                    self.rope.remove(start..a);
                    self.carets[i] = Caret::point(start);
                    shift -= (a - start) as isize;
                }
            }
        }
        self.dirty = true;
        self.normalize();
    }

    fn insert_line_after(&mut self) {
        self.begin_edit(EditKind::Hard);
        let mut positions: Vec<usize> = self
            .carets
            .iter()
            .map(|c| {
                let line = self.rope.char_to_line(c.head.min(self.rope.len_chars()));
                self.line_end_idx(line, false)
            })
            .collect();
        positions.sort_unstable();
        positions.dedup();
        let mut shift = 0usize;
        let mut next = Vec::new();
        for pos in positions {
            let p = pos + shift;
            self.rope.insert(p, "\n");
            next.push(Caret::point(p + 1));
            shift += 1;
        }
        self.carets = next;
        self.dirty = true;
    }

    fn insert_line_before(&mut self) {
        self.begin_edit(EditKind::Hard);
        let mut positions: Vec<usize> = self
            .carets
            .iter()
            .map(|c| {
                let line = self.rope.char_to_line(c.head.min(self.rope.len_chars()));
                self.rope.line_to_char(line)
            })
            .collect();
        positions.sort_unstable();
        positions.dedup();
        let mut shift = 0usize;
        let mut next = Vec::new();
        for pos in positions {
            let p = pos + shift;
            self.rope.insert(p, "\n");
            next.push(Caret::point(p));
            shift += 1;
        }
        self.carets = next;
        self.dirty = true;
    }

    fn toggle_line_comment(&mut self) {
        let lines = self.selected_lines();
        if lines.is_empty() {
            return;
        }
        self.begin_edit(EditKind::Hard);
        let all_commented = lines.iter().all(|&line| {
            let start = self.rope.line_to_char(line);
            let end = self.line_end_idx(line, false);
            self.rope
                .slice(start..end)
                .to_string()
                .trim_start()
                .starts_with("//")
        });
        for &line in lines.iter().rev() {
            let start = self.rope.line_to_char(line);
            let end = self.line_end_idx(line, false);
            let text = self.rope.slice(start..end).to_string();
            let indent = text.chars().take_while(|c| *c == ' ' || *c == '\t').count();
            let pos = start + indent;
            if all_commented {
                if self.rope.slice(pos..end).to_string().starts_with("// ") {
                    self.rope.remove(pos..pos + 3);
                } else if self.rope.slice(pos..end).to_string().starts_with("//") {
                    self.rope.remove(pos..pos + 2);
                }
            } else {
                self.rope.insert(pos, "// ");
            }
        }
        self.dirty = true;
    }

    fn indent(&mut self) {
        if self.carets.iter().all(Caret::is_empty) {
            self.insert_with_kind("    ", EditKind::Hard);
            return;
        }
        let lines = self.selected_lines();
        if lines.is_empty() {
            return;
        }
        self.begin_edit(EditKind::Hard);
        for &line in &lines {
            let pos = self.rope.line_to_char(line);
            self.rope.insert(pos, "    ");
        }
        let line_set = lines;
        for c in &mut self.carets {
            let anchor_line = self.rope.char_to_line(c.anchor.min(self.rope.len_chars()));
            let head_line = self.rope.char_to_line(c.head.min(self.rope.len_chars()));
            let anchor_shift = line_set
                .iter()
                .filter(|&&line| line < anchor_line || line == anchor_line)
                .count()
                * 4;
            let head_shift = line_set
                .iter()
                .filter(|&&line| line < head_line || line == head_line)
                .count()
                * 4;
            c.anchor += anchor_shift;
            c.head += head_shift;
        }
        self.dirty = true;
        self.normalize();
    }

    fn unindent(&mut self) {
        let lines = self.selected_lines();
        if lines.is_empty() {
            return;
        }
        self.begin_edit(EditKind::Hard);
        let mut removed_before = Vec::new();
        for &line in lines.iter().rev() {
            let start = self.rope.line_to_char(line);
            let end = (start + self.line_len_chars(line)).min(self.rope.len_chars());
            let line_text = self.rope.slice(start..end).to_string();
            let remove = if line_text.starts_with("    ") {
                4
            } else if line_text.starts_with('\t') || line_text.starts_with(' ') {
                1
            } else {
                0
            };
            if remove > 0 {
                self.rope.remove(start..start + remove);
                removed_before.push((line, remove));
            }
        }
        for c in &mut self.carets {
            let anchor_line = self.rope.char_to_line(c.anchor.min(self.rope.len_chars()));
            let head_line = self.rope.char_to_line(c.head.min(self.rope.len_chars()));
            let anchor_shift: usize = removed_before
                .iter()
                .filter(|&&(line, _)| line <= anchor_line)
                .map(|&(_, n)| n)
                .sum();
            let head_shift: usize = removed_before
                .iter()
                .filter(|&&(line, _)| line <= head_line)
                .map(|&(_, n)| n)
                .sum();
            c.anchor = c.anchor.saturating_sub(anchor_shift);
            c.head = c.head.saturating_sub(head_shift);
        }
        self.dirty = true;
        self.normalize();
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

    // ---- find / replace ----

    fn open_find(&mut self, replace: bool) {
        self.search.open = true;
        self.search.replace = replace;
        self.search.focus = true;
        if let Some(sel) = self.selected_text() {
            self.search.query = sel;
        }
        self.search.needs_recompute = true;
    }

    fn recompute_matches(&mut self) {
        self.search.needs_recompute = false;
        self.search.matches.clear();
        let q: Vec<char> = self.search.query.chars().collect();
        if q.is_empty() {
            self.search.current = None;
            return;
        }
        let lc = |c: char| c.to_lowercase().next().unwrap_or(c);
        let ql: Vec<char> = q.iter().map(|&c| lc(c)).collect();
        let text: Vec<char> = self.rope.chars().collect();
        let mut i = 0;
        while i + ql.len() <= text.len() {
            if (0..ql.len()).all(|k| lc(text[i + k]) == ql[k]) {
                self.search.matches.push((i, i + ql.len()));
                i += ql.len();
            } else {
                i += 1;
            }
        }
        let caret = self.carets.last().map(Caret::min).unwrap_or(0);
        self.search.current = self
            .search
            .matches
            .iter()
            .position(|&(s, _)| s >= caret)
            .or(if self.search.matches.is_empty() {
                None
            } else {
                Some(0)
            });
    }

    fn goto_current(&mut self) {
        if let Some(&(s, e)) = self.search.current.and_then(|c| self.search.matches.get(c)) {
            self.carets = vec![Caret {
                anchor: s,
                head: e,
                goal_col: None,
            }];
            self.pending_scroll_to = Some(s);
        }
    }

    fn find_next(&mut self) {
        if self.search.needs_recompute {
            self.recompute_matches();
        }
        let n = self.search.matches.len();
        if n == 0 {
            return;
        }
        self.search.current = Some(self.search.current.map_or(0, |c| (c + 1) % n));
        self.goto_current();
    }

    fn find_prev(&mut self) {
        if self.search.needs_recompute {
            self.recompute_matches();
        }
        let n = self.search.matches.len();
        if n == 0 {
            return;
        }
        self.search.current = Some(self.search.current.map_or(0, |c| (c + n - 1) % n));
        self.goto_current();
    }

    fn replace_current(&mut self) {
        if self.search.needs_recompute {
            self.recompute_matches();
        }
        let Some(&(s, e)) = self.search.current.and_then(|c| self.search.matches.get(c)) else {
            return;
        };
        self.begin_edit(EditKind::Hard);
        self.rope.remove(s..e);
        let repl = self.search.replacement.clone();
        self.rope.insert(s, &repl);
        self.dirty = true;
        let after = s + repl.chars().count();
        self.carets = vec![Caret::point(after)];
        self.recompute_matches();
        self.search.current = self.search.matches.iter().position(|&(ms, _)| ms >= after);
        self.goto_current();
    }

    fn replace_all(&mut self) {
        if self.search.needs_recompute {
            self.recompute_matches();
        }
        if self.search.matches.is_empty() {
            return;
        }
        self.begin_edit(EditKind::Hard);
        let repl = self.search.replacement.clone();
        for &(s, e) in self.search.matches.clone().iter().rev() {
            self.rope.remove(s..e);
            self.rope.insert(s, &repl);
        }
        self.dirty = true;
        self.carets = vec![Caret::point(0)];
        self.recompute_matches();
    }

    /// The find/replace bar, drawn at the top of the editor area when open.
    fn draw_find_bar(&mut self, ui: &mut egui::Ui) {
        if self.search.needs_recompute {
            self.recompute_matches();
        }
        let mut close = false;
        egui::Frame::none()
            .fill(theme::SURFACE)
            .inner_margin(egui::Margin::symmetric(8.0, 6.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.search.query)
                            .hint_text("Find")
                            .desired_width(240.0),
                    );
                    if self.search.focus {
                        resp.request_focus();
                        self.search.focus = false;
                    }
                    if resp.changed() {
                        self.recompute_matches();
                    }
                    let count = if self.search.matches.is_empty() {
                        "0/0".to_string()
                    } else {
                        format!(
                            "{}/{}",
                            self.search.current.map_or(0, |c| c + 1),
                            self.search.matches.len()
                        )
                    };
                    ui.label(egui::RichText::new(count).color(theme::TEXT_DIM));
                    if theme::icon_button(ui, "‹", "Previous (⇧⌘G)") {
                        self.find_prev();
                    }
                    if theme::icon_button(ui, "›", "Next (⌘G)") {
                        self.find_next();
                    }
                    if theme::icon_button(ui, "×", "Close") {
                        close = true;
                    }
                    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        self.find_next();
                        self.search.focus = true;
                    }
                });
                if self.search.replace {
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.search.replacement)
                                .hint_text("Replace")
                                .desired_width(240.0),
                        );
                        if theme::pill_button(ui, "Replace") {
                            self.replace_current();
                        }
                        if theme::pill_button(ui, "All") {
                            self.replace_all();
                        }
                    });
                }
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    close = true;
                }
            });
        if close {
            self.search.open = false;
        }
        ui.separator();
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
                    *last = Caret {
                        anchor,
                        head,
                        goal_col: None,
                    };
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
                egui::Event::Text(t) if !t.is_empty() => self.type_text(&t),
                // CJK / IME input arrives here, not as `Text`.
                egui::Event::Ime(ime) => match ime {
                    egui::ImeEvent::Preedit(t) => self.preedit = t,
                    egui::ImeEvent::Commit(t) => {
                        self.preedit.clear();
                        if !t.is_empty() {
                            self.insert(&t);
                        }
                    }
                    egui::ImeEvent::Enabled => {}
                    egui::ImeEvent::Disabled => self.preedit.clear(),
                },
                egui::Event::Paste(t) => self.insert(&t),
                egui::Event::Copy => self.copy(ctx),
                egui::Event::Cut => {
                    self.copy(ctx);
                    if self.carets.iter().all(Caret::is_empty) {
                        self.delete_lines();
                    } else {
                        self.backspace();
                    }
                }
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    use egui::Key::*;
                    match key {
                        Z if modifiers.command && modifiers.shift => self.redo(),
                        Z if modifiers.command => self.undo(),
                        Y if modifiers.command => self.redo(),
                        K if modifiers.command && modifiers.shift => self.delete_lines(),
                        D if modifiers.command && modifiers.shift => self.duplicate_lines(),
                        D if modifiers.command => self.select_next_occurrence(),
                        L if modifiers.command && modifiers.shift => {
                            self.split_selection_into_lines()
                        }
                        L if modifiers.command => self.select_lines(),
                        Slash if modifiers.command => self.toggle_line_comment(),
                        A if modifiers.command => self.select_all(),
                        F if modifiers.command && modifiers.alt => self.open_find(true),
                        F if modifiers.command => self.open_find(false),
                        G if modifiers.command && modifiers.shift => self.find_prev(),
                        G if modifiers.command => self.find_next(),
                        Enter if modifiers.command && modifiers.shift => self.insert_line_before(),
                        Enter if modifiers.command => self.insert_line_after(),
                        Enter => self.newline_smart(),
                        Backspace if modifiers.command => self.delete_to_line_start(),
                        Backspace => self.backspace(),
                        Delete => self.delete_forward(),
                        Tab if modifiers.shift => self.unindent(),
                        Tab => self.indent(),
                        ArrowLeft if modifiers.command => self.move_home(modifiers.shift),
                        ArrowRight if modifiers.command => self.move_end(modifiers.shift),
                        ArrowLeft if modifiers.alt => self.move_word(-1, modifiers.shift),
                        ArrowRight if modifiers.alt => self.move_word(1, modifiers.shift),
                        ArrowLeft => self.move_h(-1, modifiers.shift),
                        ArrowRight => self.move_h(1, modifiers.shift),
                        ArrowUp => self.move_v(-1, modifiers.shift),
                        ArrowDown => self.move_v(1, modifiers.shift),
                        Home => self.move_home(modifiers.shift),
                        End => self.move_end(modifiers.shift),
                        Escape => self.collapse(),
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
                let double_click = !p.cmd
                    && !p.shift
                    && self
                        .last_click_idx
                        .is_some_and(|last| last.abs_diff(idx) <= 1)
                    && p.time - self.last_click_time <= DOUBLE_CLICK_SECONDS;
                self.last_click_time = p.time;
                self.last_click_idx = Some(idx);

                if double_click {
                    self.select_word_at_char(idx, false);
                    return;
                } else if p.cmd {
                    self.last_kind = None;
                    self.carets.push(Caret::point(idx));
                    self.drag = self.carets.len() - 1;
                } else if p.shift {
                    self.last_kind = None;
                    let li = self.carets.len() - 1;
                    self.carets[li].head = idx;
                    self.drag = li;
                } else {
                    self.last_kind = None;
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
        let editor_active = !ui.ctx().wants_keyboard_input();
        if editor_active {
            self.handle_keys(ui.ctx());
        }

        if self.search.open {
            self.draw_find_bar(ui);
        }

        let p = ui.ctx().input(|i| Pointer {
            pos: i.pointer.interact_pos(),
            pressed: i.pointer.primary_pressed(),
            down: i.pointer.primary_down(),
            cmd: i.modifiers.command,
            shift: i.modifiers.shift,
            time: i.time,
        });

        let font = TextStyle::Monospace.resolve(ui.style());
        let row_h = ui.text_style_height(&TextStyle::Monospace);
        let row_step = row_h + ui.spacing().item_spacing.y;
        let char_w = ui.fonts(|f| f.glyph_width(&font, '0'));
        let total = self.rope.len_lines();
        let num_w = ((total.max(1) as f32).log10().floor() as usize) + 1;
        let gutter_w = (num_w as f32 + 2.0) * char_w + 6.0;
        let scroll_to = self.pending_scroll_to.take();
        let scroll_offset = scroll_to.map(|idx| {
            let line = self.rope.char_to_line(idx.min(self.rope.len_chars()));
            (line as f32 * row_step - ui.available_height() * 0.45).max(0.0)
        });

        let mut hit: Option<usize> = None;
        let mut ime_rect: Option<Rect> = None;
        let view = &*self; // immutable view for the render closure

        let mut scroll_area = egui::ScrollArea::both()
            .auto_shrink([false, false])
            .drag_to_scroll(false);
        if let Some(offset) = scroll_offset {
            scroll_area = scroll_area.vertical_scroll_offset(offset);
        }
        let scroll_out = scroll_area.show_rows(ui, row_h, total, |ui, range| {
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
                    let start_col = if line_idx == s_line {
                        c.min() - line_start
                    } else {
                        0
                    };
                    let end_col = if line_idx == e_line {
                        c.max() - line_start
                    } else {
                        llen
                    };
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

                // Find-match highlights (queries never contain newlines).
                if view.search.open {
                    for (mi, &(ms, me)) in view.search.matches.iter().enumerate() {
                        if ms < line_start || ms >= line_start + llen + 1 {
                            continue;
                        }
                        let sc = ms - line_start;
                        let ec = (me - line_start).min(llen);
                        let x0 = text_origin.x + col_x(&galley, sc.min(llen));
                        let x1 = text_origin.x + col_x(&galley, ec);
                        let color = if Some(mi) == view.search.current {
                            Color32::from_rgb(0x6b, 0x52, 0x1c)
                        } else {
                            Color32::from_rgb(0x3d, 0x35, 0x16)
                        };
                        painter.rect_filled(
                            Rect::from_min_max(
                                Pos2::new(x0, rect.top()),
                                Pos2::new(x1, rect.bottom()),
                            ),
                            2.0,
                            color,
                        );
                    }
                }

                painter.galley(text_origin, galley.clone(), theme::TEXT);

                // Carets on this line.
                let primary = view.carets.len().saturating_sub(1);
                for (ci, c) in view.carets.iter().enumerate() {
                    if view.rope.char_to_line(c.head) != line_idx {
                        continue;
                    }
                    let col = c.head - line_start;
                    let mut x = text_origin.x + col_x(&galley, col.min(llen));

                    // Inline IME composition at the primary caret.
                    if ci == primary && !view.preedit.is_empty() {
                        let pre = ui.fonts(|f| {
                            f.layout(view.preedit.clone(), font.clone(), theme::TEXT, f32::INFINITY)
                        });
                        let w = pre.size().x;
                        painter.rect_filled(
                            Rect::from_min_size(Pos2::new(x, rect.top()), Vec2::new(w, row_h)),
                            0.0,
                            theme::INSET,
                        );
                        painter.galley(Pos2::new(x, rect.top()), pre, theme::TEXT);
                        painter.hline(
                            x..=x + w,
                            rect.bottom() - 1.5,
                            egui::Stroke::new(1.0, theme::ACCENT),
                        );
                        x += w;
                    }

                    let caret_rect = Rect::from_min_max(
                        Pos2::new(x, rect.top() + 1.0),
                        Pos2::new(x + 2.0, rect.bottom() - 1.0),
                    );
                    painter.rect_filled(caret_rect, 0.0, CARET_COLOR);
                    if ci == primary {
                        ime_rect = Some(caret_rect);
                    }
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

        // Show the text (I-beam) cursor over the editing area, not the arrow.
        if ui.rect_contains_pointer(scroll_out.inner_rect) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
        }

        self.apply_pointer(&p, hit);

        // Enable and position the native IME. egui-winit only calls
        // `set_ime_allowed(true)` when `output.ime` is `Some`, so without this
        // macOS never sends composition events and CJK input is impossible.
        if editor_active {
            let rect = ime_rect
                .unwrap_or_else(|| Rect::from_min_size(ui.min_rect().min, Vec2::new(1.0, row_h)));
            ui.ctx().output_mut(|o| {
                o.ime = Some(egui::output::IMEOutput {
                    rect,
                    cursor_rect: rect,
                });
            });
        }

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

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Openers that auto-insert a closer. Single quote `'` is excluded so it
/// doesn't fight Rust lifetimes / char literals.
fn matching_close(open: char) -> Option<char> {
    match open {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        '"' => Some('"'),
        '`' => Some('`'),
        _ => None,
    }
}

fn is_closer(c: char) -> bool {
    matches!(c, ')' | ']' | '}')
}

fn char_to_byte_idx(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(idx, _)| idx)
        .unwrap_or(s.len())
}

fn word_boundary(chars: &[char], from: usize, dir: isize) -> usize {
    if dir < 0 {
        let mut i = from.min(chars.len());
        while i > 0 && chars[i - 1].is_whitespace() {
            i -= 1;
        }
        if i > 0 && is_word_char(chars[i - 1]) {
            while i > 0 && is_word_char(chars[i - 1]) {
                i -= 1;
            }
        } else {
            while i > 0 && !chars[i - 1].is_whitespace() && !is_word_char(chars[i - 1]) {
                i -= 1;
            }
        }
        i
    } else {
        let mut i = from.min(chars.len());
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if i < chars.len() && is_word_char(chars[i]) {
            while i < chars.len() && is_word_char(chars[i]) {
                i += 1;
            }
        } else {
            while i < chars.len() && !chars[i].is_whitespace() && !is_word_char(chars[i]) {
                i += 1;
            }
        }
        i
    }
}
