//! File-system tree widget for File Browser mode.
//!
//! Directories are read lazily: a folder's children are only enumerated when
//! its `CollapsingHeader` is open, so opening a deep tree stays cheap.

use std::path::{Path, PathBuf};

/// Render the tree rooted at `root`. Returns the path of a file clicked this
/// frame, if any.
pub fn show(ui: &mut egui::Ui, root: &Path, selected: Option<&Path>) -> Option<PathBuf> {
    let mut clicked = None;
    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let name = root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| root.to_string_lossy().into_owned());
            egui::CollapsingHeader::new(format!("📁 {name}"))
                .default_open(true)
                .show(ui, |ui| {
                    dir_contents(ui, root, selected, &mut clicked);
                });
        });
    clicked
}

fn dir_contents(
    ui: &mut egui::Ui,
    dir: &Path,
    selected: Option<&Path>,
    clicked: &mut Option<PathBuf>,
) {
    let entries = match read_sorted(dir) {
        Ok(e) => e,
        Err(_) => {
            ui.weak("⚠ unreadable");
            return;
        }
    };

    for (path, name, is_dir) in entries {
        if is_dir {
            egui::CollapsingHeader::new(format!("📁 {name}"))
                .id_salt(&path)
                .show(ui, |ui| {
                    dir_contents(ui, &path, selected, clicked);
                });
        } else {
            let is_sel = selected == Some(path.as_path());
            if ui
                .selectable_label(is_sel, format!("📄 {name}"))
                .clicked()
            {
                *clicked = Some(path.clone());
            }
        }
    }
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
