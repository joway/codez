//! GitHub-flavored dark theme: an egui `Visuals` plus a color palette used by
//! the syntax highlighter and the diff renderer. Colors mirror GitHub's
//! "dark default" design tokens.

use egui::{Color32, Rounding, Stroke, Visuals};

// --- Surfaces -------------------------------------------------------------
pub const SURFACE: Color32 = Color32::from_rgb(0x16, 0x1b, 0x22); // panels
pub const INSET: Color32 = Color32::from_rgb(0x0d, 0x11, 0x17); // code background
pub const RAISED: Color32 = Color32::from_rgb(0x21, 0x26, 0x2d); // buttons/hover
pub const BORDER: Color32 = Color32::from_rgb(0x30, 0x36, 0x3d);
pub const SIDEBAR: Color32 = Color32::from_rgb(0x1c, 0x21, 0x28);
pub const SIDEBAR_HOVER: Color32 = Color32::from_rgb(0x26, 0x2c, 0x35);
pub const SIDEBAR_SELECTED: Color32 = Color32::from_rgb(0x2d, 0x33, 0x3d);

// --- Text -----------------------------------------------------------------
pub const TEXT: Color32 = Color32::from_rgb(0xc9, 0xd1, 0xd9);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x8b, 0x94, 0x9e);
pub const TEXT_MUTED: Color32 = Color32::from_rgb(0x6e, 0x76, 0x81);

// --- Accents --------------------------------------------------------------
pub const ACCENT: Color32 = Color32::from_rgb(0x58, 0xa6, 0xff);
pub const GREEN: Color32 = Color32::from_rgb(0x3f, 0xb9, 0x50);
pub const RED: Color32 = Color32::from_rgb(0xf8, 0x51, 0x49);
pub const YELLOW: Color32 = Color32::from_rgb(0xd2, 0x99, 0x22);

// --- Syntax tokens --------------------------------------------------------
pub const KEYWORD: Color32 = Color32::from_rgb(0xff, 0x7b, 0x72);
pub const STRING: Color32 = Color32::from_rgb(0xa5, 0xd6, 0xff);
pub const NUMBER: Color32 = Color32::from_rgb(0x79, 0xc0, 0xff);
pub const COMMENT: Color32 = Color32::from_rgb(0x8b, 0x94, 0x9e);
pub const IDENT_FN: Color32 = Color32::from_rgb(0xd2, 0xa8, 0xff);

// --- Diff -----------------------------------------------------------------
pub const DIFF_ADD_BG: Color32 = Color32::from_rgba_premultiplied(0x1c, 0x36, 0x24, 0xff);
pub const DIFF_DEL_BG: Color32 = Color32::from_rgba_premultiplied(0x3a, 0x1d, 0x21, 0xff);
pub const DIFF_HUNK: Color32 = ACCENT;

/// Selection highlight (commit/file rows, selectable labels).
pub const SELECTION: Color32 = Color32::from_rgb(0x1f, 0x6f, 0xeb);

/// Editor text-selection fill (translucent accent).
pub const SELECTION_BG: Color32 = Color32::from_rgba_premultiplied(0x1c, 0x3a, 0x5e, 0xff);

pub fn github_dark() -> Visuals {
    let mut v = Visuals::dark();
    let rounding = Rounding::same(6.0);

    v.override_text_color = Some(TEXT);
    v.panel_fill = SURFACE;
    v.window_fill = SURFACE;
    v.extreme_bg_color = INSET;
    v.faint_bg_color = Color32::from_rgb(0x1c, 0x21, 0x28);
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.window_rounding = Rounding::same(10.0);
    v.hyperlink_color = ACCENT;

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = SURFACE;
    w.noninteractive.weak_bg_fill = SURFACE;
    w.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_DIM);

    w.inactive.bg_fill = RAISED;
    w.inactive.weak_bg_fill = RAISED;
    w.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    w.inactive.rounding = rounding;

    w.hovered.bg_fill = Color32::from_rgb(0x30, 0x36, 0x3d);
    w.hovered.weak_bg_fill = Color32::from_rgb(0x30, 0x36, 0x3d);
    w.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x8b, 0x94, 0x9e));
    w.hovered.fg_stroke = Stroke::new(1.0, TEXT);
    w.hovered.rounding = rounding;

    w.active.bg_fill = SELECTION;
    w.active.weak_bg_fill = SELECTION;
    w.active.bg_stroke = Stroke::new(1.0, ACCENT);
    w.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    w.active.rounding = rounding;

    w.open.bg_fill = RAISED;
    w.open.rounding = rounding;

    v.selection.bg_fill = Color32::from_rgba_premultiplied(0x1f, 0x6f, 0xeb, 0x66);
    v.selection.stroke = Stroke::new(1.0, ACCENT);

    v
}
