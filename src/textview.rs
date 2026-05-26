//! Virtualized read-only renderer for unified diffs.
//!
//! Renders only the rows currently visible in the viewport via
//! `ScrollArea::show_rows`, so per-frame cost is bounded by window height, not
//! diff size. Rows are tinted green/red by +/- with marker coloring.

use std::borrow::Cow;

use egui::text::LayoutJob;
use egui::{Color32, Pos2, Rect, ScrollArea, TextFormat, TextStyle, Ui};

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

    ScrollArea::both()
        .auto_shrink([false, false])
        .show_rows(ui, row_h, total, |ui, range| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for i in range {
                let line = src.line(i);
                if let Some(bg) = diff_bg(&line) {
                    let clip = ui.clip_rect();
                    let top = ui.cursor().top();
                    let rect = Rect::from_min_max(
                        Pos2::new(clip.left(), top),
                        Pos2::new(clip.right(), top + row_h),
                    );
                    ui.painter().rect_filled(rect, 0.0, bg);
                }
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
                ui.label(job);
            }
        });
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
