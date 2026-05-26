use std::path::{Path, PathBuf};

use egui::text::LayoutJob;
use egui::{Align, Color32, FontId, Layout, RichText, TextFormat};
use git2::Repository;

use crate::editor::Editor;
use crate::gitmodel::{self, CommitInfo, FileChange};
use crate::menu::Menus;
use crate::settings::Settings;
use crate::{fstree, textview, theme};

#[derive(PartialEq, Eq, Clone, Copy)]
enum Mode {
    FileBrowser,
    GitDiff,
}

pub struct DiffistApp {
    mode: Mode,
    root: Option<PathBuf>,

    // --- File Browser (Editor) mode ---
    selected_file: Option<PathBuf>,
    editor: Option<Editor>,
    file_status: String,
    last_dirty: bool,

    // --- Git Diff mode ---
    repo: Option<Repository>,
    commits: Vec<CommitInfo>,
    selected_commit: Option<usize>,
    changes: Vec<FileChange>,
    selected_change: Option<usize>,
    diff_lines: Vec<String>,
    git_status: String,

    // --- Chrome ---
    menus: Menus,
    settings: Settings,
    title_dirty: bool,
}

impl DiffistApp {
    /// Build the app (installing the native menu bar), optionally opening
    /// `initial_dir` on startup. Must run on the main thread after the
    /// NSApplication exists, i.e. from eframe's creation closure.
    pub fn new(initial_dir: Option<PathBuf>, initial_file: Option<PathBuf>) -> Self {
        // Optional override so the app can launch straight into Diff mode.
        let mode = match std::env::var("DIFFIST_MODE").as_deref() {
            Ok("diff") => Mode::GitDiff,
            _ => Mode::FileBrowser,
        };
        let mut app = Self {
            mode,
            root: None,
            selected_file: None,
            editor: None,
            file_status: String::new(),
            last_dirty: false,
            repo: None,
            commits: Vec::new(),
            selected_commit: None,
            changes: Vec::new(),
            selected_change: None,
            diff_lines: Vec::new(),
            git_status: String::new(),
            menus: Menus::install(),
            settings: Settings::default(),
            title_dirty: false,
        };
        if let Some(dir) = initial_dir {
            app.open_folder(&dir);
        }
        if let Some(file) = initial_file {
            app.load_file(&file);
        }
        app
    }
}

impl eframe::App for DiffistApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_menu();
        self.settings.apply(ctx);
        // Reflect unsaved-changes state in the title bar.
        let dirty = self.editor.as_ref().is_some_and(|e| e.is_dirty());
        if dirty != self.last_dirty {
            self.last_dirty = dirty;
            self.title_dirty = true;
        }
        self.sync_window_title(ctx);

        self.mode_bar(ctx);
        match self.mode {
            Mode::FileBrowser => self.file_browser_ui(ctx),
            Mode::GitDiff => self.git_diff_ui(ctx),
        }
        self.settings.ui(ctx);

        // Native menu clicks don't wake the window, so poll a few times a second.
        ctx.request_repaint_after(std::time::Duration::from_millis(150));
    }
}

impl DiffistApp {
    /// Top strip: folder name on the left, Editor/Diff segmented switch on the
    /// right (the rest of the menu lives in the native macOS menu bar).
    fn mode_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("mode_bar")
            .exact_height(34.0)
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.add_space(10.0);
                    if let Some(name) = self.root.as_ref().and_then(|p| p.file_name()) {
                        ui.label(
                            RichText::new(name.to_string_lossy())
                                .color(theme::TEXT_DIM)
                                .strong(),
                        );
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add_space(4.0);
                        // Right-to-left: "Diff" hugs the corner, "Editor" to its left.
                        ui.selectable_value(&mut self.mode, Mode::GitDiff, "  Diff  ");
                        ui.selectable_value(&mut self.mode, Mode::FileBrowser, "  Editor  ");
                    });
                });
            });
    }

    /// Drain native menu events. Plain menu items (no checkmarks) so there is no
    /// programmatic state to feed back and spuriously re-fire.
    fn handle_menu(&mut self) {
        while let Ok(event) = muda::MenuEvent::receiver().try_recv() {
            let id = &event.id;
            if *id == self.menus.open {
                if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                    self.open_folder(&dir);
                }
            } else if *id == self.menus.save {
                if let Some(editor) = &mut self.editor {
                    let _ = editor.save();
                }
            } else if *id == self.menus.settings {
                self.settings.open = true;
            } else if *id == self.menus.editor {
                self.mode = Mode::FileBrowser;
            } else if *id == self.menus.diff {
                self.mode = Mode::GitDiff;
            }
        }
    }

    fn sync_window_title(&mut self, ctx: &egui::Context) {
        if !self.title_dirty {
            return;
        }
        self.title_dirty = false;
        let dot = if self.last_dirty { "● " } else { "" };
        let title = match &self.root {
            Some(p) => format!("{dot}Diffist — {}", p.display()),
            None => format!("{dot}Diffist"),
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
    }

    /// Set the working folder for both modes, and try to open it as a git repo.
    fn open_folder(&mut self, dir: &Path) {
        self.root = Some(dir.to_path_buf());
        self.title_dirty = true;
        self.selected_file = None;
        self.editor = None;
        self.file_status.clear();

        self.repo = None;
        self.commits.clear();
        self.selected_commit = None;
        self.changes.clear();
        self.selected_change = None;
        self.diff_lines.clear();

        match Repository::discover(dir) {
            Ok(repo) => match gitmodel::list_commits(&repo, 500) {
                Ok(commits) => {
                    self.git_status = format!("{} commits", commits.len());
                    self.commits = commits;
                    self.repo = Some(repo);
                }
                Err(e) => self.git_status = format!("git error: {e}"),
            },
            Err(_) => self.git_status = "not a git repository".to_string(),
        }
    }

    // ---------------- File Browser mode ----------------

    fn file_browser_ui(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("tree")
            .resizable(true)
            .default_width(280.0)
            .show(ctx, |ui| {
                section_header(ui, "EXPLORER");
                if let Some(root) = self.root.clone() {
                    if let Some(path) = fstree::show(ui, &root, self.selected_file.as_deref()) {
                        self.load_file(&path);
                    }
                } else {
                    ui.weak("File ▸ Open Folder…  (⌘O)");
                }
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            let Some(path) = self.selected_file.clone() else {
                ui.centered_and_justified(|ui| ui.weak("Select a file"));
                return;
            };
            let status = self
                .editor
                .as_ref()
                .map(|e| e.status())
                .unwrap_or_else(|| self.file_status.clone());
            file_header(ui, &path, &status);
            match &mut self.editor {
                Some(editor) => editor.ui(ui),
                None => {
                    ui.add_space(8.0);
                    ui.weak(&self.file_status);
                }
            }
        });
    }

    fn load_file(&mut self, path: &Path) {
        self.selected_file = Some(path.to_path_buf());
        match Editor::open(path) {
            Ok(editor) => {
                self.editor = Some(editor);
                self.last_dirty = false;
            }
            Err(e) => {
                self.editor = None;
                self.file_status = format!("cannot edit (not UTF-8?): {e}");
            }
        }
    }

    // ---------------- Git Diff mode ----------------

    fn git_diff_ui(&mut self, ctx: &egui::Context) {
        // Land on something useful instead of an empty pane: newest commit, first file.
        if self.selected_commit.is_none() && !self.commits.is_empty() {
            self.select_commit(0);
        }
        if self.selected_change.is_none() && !self.changes.is_empty() {
            self.select_change(0);
        }

        egui::SidePanel::left("commits")
            .resizable(true)
            .default_width(320.0)
            .show(ctx, |ui| {
                section_header(ui, "COMMITS");
                ui.weak(&self.git_status);
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let mut select = None;
                        for (i, c) in self.commits.iter().enumerate() {
                            let selected = self.selected_commit == Some(i);
                            if ui.selectable_label(selected, commit_job(c)).clicked() {
                                select = Some(i);
                            }
                        }
                        if let Some(i) = select {
                            self.select_commit(i);
                        }
                    });
            });

        egui::SidePanel::left("changed_files")
            .resizable(true)
            .default_width(280.0)
            .show(ctx, |ui| {
                section_header(ui, "CHANGED FILES");
                egui::ScrollArea::both()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let mut select = None;
                        for (i, f) in self.changes.iter().enumerate() {
                            let selected = self.selected_change == Some(i);
                            if ui.selectable_label(selected, change_job(f)).clicked() {
                                select = Some(i);
                            }
                        }
                        if let Some(i) = select {
                            self.select_change(i);
                        }
                    });
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            if self.diff_lines.is_empty() {
                ui.centered_and_justified(|ui| ui.weak("Select a commit, then a file"));
                return;
            }
            if let Some(i) = self.selected_change {
                file_header(ui, Path::new(&self.changes[i].path), "");
            }
            textview::diff_view(ui, &self.diff_lines);
        });
    }

    fn select_commit(&mut self, idx: usize) {
        self.selected_commit = Some(idx);
        self.selected_change = None;
        self.diff_lines.clear();
        let oid = self.commits[idx].oid;
        if let Some(repo) = &self.repo {
            match gitmodel::commit_changes(repo, oid) {
                Ok(c) => self.changes = c,
                Err(e) => {
                    self.changes.clear();
                    self.git_status = format!("diff error: {e}");
                }
            }
        }
    }

    fn select_change(&mut self, idx: usize) {
        self.selected_change = Some(idx);
        let Some(commit_idx) = self.selected_commit else {
            return;
        };
        let oid = self.commits[commit_idx].oid;
        let path = self.changes[idx].path.clone();
        if let Some(repo) = &self.repo {
            match gitmodel::file_patch(repo, oid, &path) {
                Ok(text) => self.diff_lines = text.lines().map(str::to_owned).collect(),
                Err(e) => self.diff_lines = vec![format!("diff error: {e}")],
            }
        }
    }
}

// ---------------- small view helpers ----------------

/// A dim, uppercase section label like GitHub's sidebar headers.
fn section_header(ui: &mut egui::Ui, text: &str) {
    ui.add_space(6.0);
    ui.label(
        RichText::new(text)
            .color(theme::TEXT_MUTED)
            .size(11.0)
            .strong(),
    );
    ui.add_space(4.0);
    ui.separator();
}

/// A breadcrumb-style header for the content pane: path + optional status.
fn file_header(ui: &mut egui::Ui, path: &Path, status: &str) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(path.to_string_lossy()).monospace().color(theme::TEXT));
        if !status.is_empty() {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new(status).color(theme::TEXT_DIM).size(12.0));
            });
        }
    });
    ui.separator();
}

/// Two-line commit row: summary, then `sha · author · date`.
fn commit_job(c: &CommitInfo) -> LayoutJob {
    let mut job = LayoutJob::default();
    seg(&mut job, &c.summary, FontId::proportional(13.5), theme::TEXT);
    seg(&mut job, "\n", FontId::proportional(13.5), theme::TEXT);
    seg(&mut job, &c.short_id, FontId::monospace(11.0), theme::ACCENT);
    seg(
        &mut job,
        &format!("  {} · {}", c.author, c.date),
        FontId::proportional(11.0),
        theme::TEXT_DIM,
    );
    job
}

/// File-change row: a colored status letter followed by the path.
fn change_job(f: &FileChange) -> LayoutJob {
    let mut job = LayoutJob::default();
    seg(
        &mut job,
        &format!("{}  ", f.status),
        FontId::monospace(12.5),
        status_color(f.status),
    );
    seg(&mut job, &f.path, FontId::proportional(13.0), theme::TEXT);
    job
}

fn status_color(s: char) -> Color32 {
    match s {
        'A' => theme::GREEN,
        'M' => theme::YELLOW,
        'D' => theme::RED,
        'R' | 'C' => theme::ACCENT,
        _ => theme::TEXT_DIM,
    }
}

fn seg(job: &mut LayoutJob, text: &str, font: FontId, color: Color32) {
    job.append(
        text,
        0.0,
        TextFormat {
            font_id: font,
            color,
            ..Default::default()
        },
    );
}
