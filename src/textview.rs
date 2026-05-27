//! Virtualized read-only renderer for unified diffs.
//!
//! Renders only the rows currently visible in the viewport via
//! `ScrollArea::show_rows`, so per-frame cost is bounded by window height, not
//! diff size. Rows are tinted green/red by +/- with marker coloring.

use std::borrow::Cow;

use egui::text::LayoutJob;
use egui::{
    Align, Color32, Layout, Pos2, Rect, ScrollArea, Sense, TextFormat, TextStyle, Ui, UiBuilder,
    Vec2,
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
            for i in range {
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
                ui.painter().galley(rect.left_top(), galley, theme::TEXT);
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
