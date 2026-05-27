//! File-system tree widget for File Browser mode.

use std::path::{Path, PathBuf};

use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};

use crate::theme;

const ROW_H: f32 = 24.0;
const INDENT: f32 = 14.0;
const LEFT_PAD: f32 = 8.0;

/// A context-menu action requested on a tree node, handled by the app.
pub enum TreeAction {
    NewFile(PathBuf),
    NewFolder(PathBuf),
    Rename(PathBuf),
    Delete(PathBuf),
    Duplicate(PathBuf),
    CopyFile(PathBuf),
    Paste(PathBuf),
    RevealInFinder(PathBuf),
    OpenInTerminal(PathBuf),
    CopyPath(PathBuf),
    CopyRelativePath(PathBuf),
}

struct TreeCtx<'a> {
    selected: Option<&'a Path>,
    can_paste: bool,
    clicked: Option<PathBuf>,
    action: Option<TreeAction>,
}

/// Render the tree rooted at `root`. Returns `(clicked_file, context_action)`
/// requested this frame.
pub fn show(
    ui: &mut egui::Ui,
    root: &Path,
    selected: Option<&Path>,
    can_paste: bool,
) -> (Option<PathBuf>, Option<TreeAction>) {
    let mut ctx = TreeCtx {
        selected,
        can_paste,
        clicked: None,
        action: None,
    };
    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let name = root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| root.to_string_lossy().into_owned());
            let open = tree_row(ui, root, &name, true, true, false, 0, &mut ctx);
            if open {
                dir_contents(ui, root, 1, &mut ctx);
            }
        });
    (ctx.clicked, ctx.action)
}

fn dir_contents(ui: &mut egui::Ui, dir: &Path, depth: usize, ctx: &mut TreeCtx) {
    let entries = match read_sorted(dir) {
        Ok(e) => e,
        Err(_) => {
            tree_message(ui, depth, "unreadable");
            return;
        }
    };

    for (path, name, is_dir) in entries {
        if is_dir {
            let open = tree_row(ui, &path, &name, true, false, false, depth, ctx);
            if open {
                dir_contents(ui, &path, depth + 1, ctx);
            }
        } else {
            let is_sel = ctx.selected == Some(path.as_path());
            let response = tree_file_row(ui, &path, &name, is_sel, depth);
            if response.clicked() {
                ctx.clicked = Some(path.clone());
            }
            attach_menu(&response, &path, false, ctx.can_paste, &mut ctx.action);
        }
    }
}

/// Right-click context menu for a tree node. `is_dir` selects whether "new"
/// and "paste" target this node or its parent directory.
fn attach_menu(
    response: &egui::Response,
    path: &Path,
    is_dir: bool,
    can_paste: bool,
    action: &mut Option<TreeAction>,
) {
    let dir = if is_dir {
        path.to_path_buf()
    } else {
        path.parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| path.to_path_buf())
    };
    fn item(ui: &mut egui::Ui, label: &str, act: TreeAction, out: &mut Option<TreeAction>) {
        if ui.button(label).clicked() {
            *out = Some(act);
            ui.close_menu();
        }
    }
    response.context_menu(|ui| {
        item(ui, "New File…", TreeAction::NewFile(dir.clone()), action);
        item(
            ui,
            "New Folder…",
            TreeAction::NewFolder(dir.clone()),
            action,
        );
        ui.separator();
        item(
            ui,
            "Rename…",
            TreeAction::Rename(path.to_path_buf()),
            action,
        );
        item(
            ui,
            "Duplicate",
            TreeAction::Duplicate(path.to_path_buf()),
            action,
        );
        item(
            ui,
            "Move to Trash",
            TreeAction::Delete(path.to_path_buf()),
            action,
        );
        ui.separator();
        item(ui, "Copy", TreeAction::CopyFile(path.to_path_buf()), action);
        ui.add_enabled_ui(can_paste, |ui| {
            if ui.button("Paste").clicked() {
                *action = Some(TreeAction::Paste(dir.clone()));
                ui.close_menu();
            }
        });
        ui.separator();
        item(
            ui,
            "Reveal in Finder",
            TreeAction::RevealInFinder(path.to_path_buf()),
            action,
        );
        item(
            ui,
            "Open in Terminal",
            TreeAction::OpenInTerminal(dir.clone()),
            action,
        );
        ui.separator();
        item(
            ui,
            "Copy Path",
            TreeAction::CopyPath(path.to_path_buf()),
            action,
        );
        item(
            ui,
            "Copy Relative Path",
            TreeAction::CopyRelativePath(path.to_path_buf()),
            action,
        );
    });
}

#[allow(clippy::too_many_arguments)]
fn tree_row(
    ui: &mut egui::Ui,
    path: &Path,
    name: &str,
    is_dir: bool,
    default_open: bool,
    selected: bool,
    depth: usize,
    ctx: &mut TreeCtx,
) -> bool {
    let id = ui.id().with(path);
    let mut open = ui
        .ctx()
        .data_mut(|d| d.get_persisted::<bool>(id).unwrap_or(default_open));
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::click());
    attach_menu(&response, path, is_dir, ctx.can_paste, &mut ctx.action);
    paint_row_bg(ui, rect, selected, response.hovered());

    let x = rect.left() + LEFT_PAD + depth as f32 * INDENT;
    if is_dir {
        ui.painter().text(
            Pos2::new(x, rect.center().y),
            Align2::LEFT_CENTER,
            if open { "▾" } else { "▸" },
            FontId::proportional(12.0),
            theme::TEXT_MUTED,
        );
    }
    let icon_x = x + 16.0;
    paint_folder_icon(ui, Pos2::new(icon_x, rect.center().y), open);
    paint_ellipsized(
        ui,
        name,
        FontId::proportional(ui_text_size(ui) * 0.86),
        Pos2::new(icon_x + 18.0, rect.center().y),
        (rect.right() - icon_x - 28.0).max(16.0),
        if selected {
            theme::TEXT
        } else {
            theme::TEXT_DIM
        },
    );

    if response.clicked() {
        open = !open;
        ui.ctx().data_mut(|d| d.insert_persisted(id, open));
    }
    open
}

fn tree_file_row(
    ui: &mut egui::Ui,
    path: &Path,
    name: &str,
    selected: bool,
    depth: usize,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::click());
    paint_row_bg(ui, rect, selected, response.hovered());
    let x = rect.left() + LEFT_PAD + depth as f32 * INDENT + 16.0;
    paint_file_icon(ui, Pos2::new(x, rect.center().y), name);
    paint_ellipsized(
        ui,
        name,
        FontId::proportional(ui_text_size(ui) * 0.86),
        Pos2::new(x + 18.0, rect.center().y),
        (rect.right() - x - 28.0).max(16.0),
        if selected {
            theme::TEXT
        } else {
            theme::TEXT_DIM
        },
    );
    response.on_hover_text(path.to_string_lossy())
}

fn tree_message(ui: &mut egui::Ui, depth: usize, text: &str) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::hover());
    ui.painter().text(
        Pos2::new(
            rect.left() + LEFT_PAD + depth as f32 * INDENT + 16.0,
            rect.center().y,
        ),
        Align2::LEFT_CENTER,
        text,
        FontId::proportional(ui_text_size(ui) * 0.82),
        theme::TEXT_MUTED,
    );
}

fn paint_row_bg(ui: &egui::Ui, rect: Rect, selected: bool, hovered: bool) {
    let fill = if selected {
        Some(theme::SIDEBAR_SELECTED)
    } else if hovered {
        Some(theme::SIDEBAR_HOVER)
    } else {
        None
    };
    if let Some(fill) = fill {
        ui.painter()
            .rect_filled(rect.shrink2(Vec2::new(4.0, 1.0)), 4.0, fill);
    }
}

fn paint_folder_icon(ui: &egui::Ui, center: Pos2, open: bool) {
    let color = Color32::from_rgb(0x6e, 0x76, 0x81);
    let body = Rect::from_center_size(center + Vec2::new(0.0, 1.5), Vec2::new(13.0, 9.0));
    let tab = Rect::from_min_size(
        Pos2::new(body.left() + 1.0, body.top() - 3.0),
        Vec2::new(6.0, 4.0),
    );
    ui.painter()
        .rect_filled(tab, 1.5, color.linear_multiply(0.8));
    ui.painter().rect_filled(body, 2.0, color);
    if open {
        ui.painter().line_segment(
            [
                Pos2::new(body.left() + 2.0, body.top() + 3.0),
                Pos2::new(body.right() - 2.0, body.top() + 3.0),
            ],
            Stroke::new(1.0, Color32::from_rgb(0x9a, 0xa4, 0xb0)),
        );
    }
}

fn paint_file_icon(ui: &egui::Ui, center: Pos2, name: &str) {
    if let Some(icon) = file_icon(name) {
        let rect = Rect::from_center_size(center, Vec2::new(16.0, 16.0));
        ui.painter().rect_filled(rect, 3.0, icon.bg);
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            icon.label,
            FontId::monospace(icon.font_size),
            icon.fg,
        );
    } else {
        let rect = Rect::from_center_size(center, Vec2::new(11.0, 14.0));
        ui.painter().rect_stroke(
            rect,
            1.5,
            Stroke::new(1.0, Color32::from_rgb(0x8b, 0x94, 0x9e)),
        );
        ui.painter().line_segment(
            [
                Pos2::new(rect.left() + 2.0, rect.top() + 4.0),
                Pos2::new(rect.right() - 2.0, rect.top() + 4.0),
            ],
            Stroke::new(1.0, Color32::from_rgb(0x6e, 0x76, 0x81)),
        );
    }
}

struct FileIcon {
    label: &'static str,
    bg: Color32,
    fg: Color32,
    font_size: f32,
}

fn file_icon(name: &str) -> Option<FileIcon> {
    let lower = name.to_ascii_lowercase();
    let ext = lower.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("");
    let icon = match ext {
        "rs" => FileIcon::new("RS", 0xde, 0x6b, 0x48),
        "js" | "mjs" | "cjs" => FileIcon::dark_text("JS", 0xf1, 0xe0, 0x5a),
        "ts" => FileIcon::new("TS", 0x31, 0x78, 0xc6),
        "tsx" => FileIcon::new("TSX", 0x31, 0x78, 0xc6),
        "jsx" => FileIcon::dark_text("JSX", 0x61, 0xda, 0xfb),
        "py" => FileIcon::new("PY", 0x37, 0x71, 0xa1),
        "go" => FileIcon::dark_text("GO", 0x6a, 0xd7, 0xe5),
        "java" => FileIcon::new("JV", 0xc7, 0x46, 0x34),
        "kt" | "kts" => FileIcon::new("KT", 0x8b, 0x5c, 0xd6),
        "swift" => FileIcon::new("SW", 0xf0, 0x6b, 0x32),
        "c" | "h" => FileIcon::new("C", 0x55, 0x6f, 0xb5),
        "cc" | "cpp" | "cxx" | "hpp" => FileIcon::new("C++", 0x62, 0x91, 0xcf),
        "cs" => FileIcon::new("C#", 0x7b, 0x4f, 0xc9),
        "html" | "htm" => FileIcon::new("HT", 0xe4, 0x4d, 0x26),
        "css" => FileIcon::new("CSS", 0x26, 0x4d, 0xe4),
        "scss" | "sass" => FileIcon::new("SC", 0xc6, 0x53, 0x8c),
        "json" => FileIcon::dark_text("{}", 0xf0, 0xc6, 0x74),
        "toml" => FileIcon::new("TM", 0x9c, 0x6b, 0x3f),
        "yaml" | "yml" => FileIcon::new("Y", 0xcb, 0x4b, 0x4b),
        "md" | "markdown" => FileIcon::new("MD", 0x5b, 0x6f, 0x8f),
        "sh" | "bash" | "zsh" | "fish" => FileIcon::new("$", 0x57, 0xab, 0x5a),
        "sql" => FileIcon::new("DB", 0x51, 0x84, 0xc6),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "tiff" => FileIcon::new("IMG", 0x8a, 0x63, 0xd2),
        "svg" => FileIcon::new("SVG", 0xd9, 0x8b, 0x32),
        "lock" => FileIcon::new("LK", 0x77, 0x7f, 0x8b),
        _ => match lower.as_str() {
            "dockerfile" => FileIcon::new("DK", 0x24, 0x96, 0xed),
            "makefile" => FileIcon::new("MK", 0x8b, 0x94, 0x9e),
            "cargo.toml" => FileIcon::new("RS", 0xde, 0x6b, 0x48),
            "package.json" => FileIcon::dark_text("N", 0x83, 0xcd, 0x29),
            _ => return None,
        },
    };
    Some(icon)
}

impl FileIcon {
    fn new(label: &'static str, r: u8, g: u8, b: u8) -> Self {
        Self {
            label,
            bg: Color32::from_rgb(r, g, b).linear_multiply(0.28),
            fg: Color32::from_rgb(r, g, b),
            font_size: if label.len() > 2 { 7.0 } else { 8.0 },
        }
    }

    fn dark_text(label: &'static str, r: u8, g: u8, b: u8) -> Self {
        Self {
            label,
            bg: Color32::from_rgb(r, g, b),
            fg: Color32::from_rgb(0x16, 0x1b, 0x22),
            font_size: if label.len() > 2 { 7.0 } else { 8.0 },
        }
    }
}

fn paint_ellipsized(
    ui: &egui::Ui,
    text: &str,
    font: FontId,
    pos: Pos2,
    max_width: f32,
    color: Color32,
) {
    ui.painter().text(
        pos,
        Align2::LEFT_CENTER,
        ellipsize(ui, text, font.clone(), max_width),
        font,
        color,
    );
}

fn ellipsize(ui: &egui::Ui, text: &str, font: FontId, max_width: f32) -> String {
    if text_width(ui, text, font.clone()) <= max_width {
        return text.to_string();
    }
    let ellipsis = "...";
    let target = (max_width - text_width(ui, ellipsis, font.clone())).max(0.0);
    let chars: Vec<char> = text.chars().collect();
    let mut lo = 0;
    let mut hi = chars.len();
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let candidate: String = chars[..mid].iter().collect();
        if text_width(ui, &candidate, font.clone()) <= target {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    let mut out: String = chars[..lo].iter().collect();
    out.push_str(ellipsis);
    out
}

fn text_width(ui: &egui::Ui, text: &str, font: FontId) -> f32 {
    ui.fonts(|f| {
        f.layout_no_wrap(text.to_owned(), font, theme::TEXT)
            .size()
            .x
    })
}

fn ui_text_size(ui: &egui::Ui) -> f32 {
    egui::TextStyle::Body.resolve(ui.style()).size
}

/// Read a directory's entries sorted with directories first, then by name.
fn read_sorted(dir: &Path) -> std::io::Result<Vec<(PathBuf, String, bool)>> {
    let mut entries: Vec<(PathBuf, String, bool)> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name == ".git" {
                return None;
            }
            let is_dir = e.file_type().ok()?.is_dir();
            Some((e.path(), name, is_dir))
        })
        .collect();
    entries.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.1.cmp(&b.1)));
    Ok(entries)
}
