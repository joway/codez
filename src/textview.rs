//! Virtualized read-only renderer for unified diffs.
//!
//! Renders only the rows currently visible in the viewport via
//! `ScrollArea::show_rows`, so per-frame cost is bounded by window height, not
//! diff size. Rows are tinted green/red by +/- with marker coloring.

use std::borrow::Cow;
use std::ops::Range;

use egui::text::LayoutJob;
use egui::{
    Align, Align2, Color32, Layout, Pos2, Rect, ScrollArea, Sense, TextFormat, TextStyle, Ui,
    UiBuilder, Vec2,
};

use crate::theme;

/// A random-access source of text lines.
pub trait LineSource {
    fn line_count(&self) -> usize;
    fn line(&self, i: usize) -> Cow<'_, str>;
}

impl LineSource for Vec<String> {
    fn line_count(&self) -> usize {
        self.len()
    }
    fn line(&self, i: usize) -> Cow<'_, str> {
        Cow::Borrowed(&self[i])
    }
}

/// Render a unified diff: each visible row tinted by its +/- marker.
pub fn diff_view(ui: &mut Ui, src: &impl LineSource) {
    let font = TextStyle::Monospace.resolve(ui.style());
    let row_h = ui.text_style_height(&TextStyle::Monospace);
    let total = src.line_count();
    let available = ui.available_size_before_wrap();
    if available.x <= 0.0 || available.y <= 0.0 {
        return;
    }
    let (viewport_rect, _) = ui.allocate_exact_size(available, Sense::hover());

    ui.painter().rect_filled(viewport_rect, 0.0, theme::INSET);
    let mut viewport_ui = ui.new_child(
        UiBuilder::new()
            .max_rect(viewport_rect)
            .layout(Layout::top_down(Align::Min)),
    );
    viewport_ui.set_clip_rect(viewport_rect);
    viewport_ui.spacing_mut().item_spacing.y = 0.0;

    ScrollArea::vertical()
        .min_scrolled_height(viewport_rect.height())
        .auto_shrink([false, false])
        .show_rows(&mut viewport_ui, row_h, total, |ui, range| {
            let nums = diff_line_numbers(src, range.clone());
            let char_w = ui.fonts(|f| f.glyph_width(&font, '0'));
            let digits = total.max(1).to_string().len().max(3) as f32;
            let gutter_w = (digits * 2.0 + 3.0) * char_w;
            for (offset, i) in range.enumerate() {
                let line = src.line(i);
                let mut job = LayoutJob::default();
                job.wrap.max_width = f32::INFINITY;
                job.append(
                    line.as_ref(),
                    0.0,
                    TextFormat {
                        font_id: font.clone(),
                        color: diff_fg(&line),
                        ..Default::default()
                    },
                );
                let galley = ui.fonts(|f| f.layout_job(job));
                let row_w = ui.available_width();
                let (rect, _) = ui.allocate_exact_size(Vec2::new(row_w, row_h), Sense::hover());

                let clip = ui.clip_rect();
                let bg_rect = Rect::from_min_max(
                    Pos2::new(clip.left(), rect.top()),
                    Pos2::new(clip.right(), rect.bottom()),
                );
                ui.painter()
                    .rect_filled(bg_rect, 0.0, diff_bg(&line).unwrap_or(theme::INSET));
                paint_line_number(
                    ui,
                    rect,
                    nums[offset].old,
                    digits,
                    font.clone(),
                    rect.left() + 4.0,
                );
                paint_line_number(
                    ui,
                    rect,
                    nums[offset].new,
                    digits,
                    font.clone(),
                    rect.left() + 4.0 + (digits + 1.0) * char_w,
                );
                let sep_x = rect.left() + gutter_w - char_w;
                ui.painter().vline(
                    sep_x,
                    rect.top()..=rect.bottom(),
                    egui::Stroke::new(1.0, theme::BORDER),
                );
                ui.painter().galley(
                    Pos2::new(rect.left() + gutter_w, rect.top()),
                    galley,
                    theme::TEXT,
                );
            }
        });
}

#[derive(Clone, Copy, Default)]
struct DiffLineNumber {
    old: Option<usize>,
    new: Option<usize>,
}

fn diff_line_numbers(src: &impl LineSource, range: Range<usize>) -> Vec<DiffLineNumber> {
    let mut out = Vec::with_capacity(range.len());
    let mut old_line: Option<usize> = None;
    let mut new_line: Option<usize> = None;

    for i in 0..range.end {
        let line = src.line(i);
        let text = line.as_ref();
        let mut nums = DiffLineNumber::default();

        if let Some((old_start, new_start)) = parse_hunk_header(text) {
            old_line = Some(old_start);
            new_line = Some(new_start);
        } else if old_line.is_some() || new_line.is_some() {
            match text.as_bytes().first().copied() {
                Some(b'+') if !text.starts_with("+++") => {
                    nums.new = new_line;
                    if let Some(n) = &mut new_line {
                        *n += 1;
                    }
                }
                Some(b'-') if !text.starts_with("---") => {
                    nums.old = old_line;
                    if let Some(n) = &mut old_line {
                        *n += 1;
                    }
                }
                Some(b' ') => {
                    nums.old = old_line;
                    nums.new = new_line;
                    if let Some(n) = &mut old_line {
                        *n += 1;
                    }
                    if let Some(n) = &mut new_line {
                        *n += 1;
                    }
                }
                _ => {}
            }
        }

        if i >= range.start {
            out.push(nums);
        }
    }

    out
}

fn parse_hunk_header(line: &str) -> Option<(usize, usize)> {
    if !line.starts_with("@@ ") {
        return None;
    }
    let old = parse_hunk_side(line, '-')?;
    let new = parse_hunk_side(line, '+')?;
    Some((old, new))
}

fn parse_hunk_side(line: &str, marker: char) -> Option<usize> {
    let start = line.find(marker)? + marker.len_utf8();
    let num: String = line[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    num.parse().ok()
}

fn paint_line_number(
    ui: &Ui,
    rect: Rect,
    line: Option<usize>,
    digits: f32,
    font: egui::FontId,
    x: f32,
) {
    let Some(line) = line else {
        return;
    };
    ui.painter().text(
        Pos2::new(
            x + digits * ui.fonts(|f| f.glyph_width(&font, '0')),
            rect.top(),
        ),
        Align2::RIGHT_TOP,
        line.to_string(),
        font,
        theme::TEXT_MUTED,
    );
}

fn diff_bg(line: &str) -> Option<Color32> {
    match line.as_bytes().first() {
        Some(b'+') if !line.starts_with("+++") => Some(theme::DIFF_ADD_BG),
        Some(b'-') if !line.starts_with("---") => Some(theme::DIFF_DEL_BG),
        _ => None,
    }
}

fn diff_fg(line: &str) -> Color32 {
    match line.as_bytes().first() {
        Some(b'+') if !line.starts_with("+++") => theme::GREEN,
        Some(b'-') if !line.starts_with("---") => theme::RED,
        _ if line.starts_with("@@") => theme::DIFF_HUNK,
        _ if line.starts_with("diff ")
            || line.starts_with("index ")
            || line.starts_with("+++")
            || line.starts_with("---") =>
        {
            theme::TEXT_MUTED
        }
        _ => theme::TEXT,
    }
}
