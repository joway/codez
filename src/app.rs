use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use egui::{
    Align, Align2, Color32, FontId, Layout, Pos2, Rect, RichText, Sense, Stroke, TextStyle, Vec2,
};
use git2::Repository;

use crate::editor::Editor;
use crate::gitmodel::{self, CommitInfo, FileChange};
use crate::menu::Menus;
use crate::settings::Settings;
use crate::{fstree, textview, theme};

const PANEL_LEFT_PAD: f32 = 12.0;

#[derive(PartialEq, Eq, Clone, Copy)]
enum Mode {
    FileBrowser,
    GitDiff,
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum DiffSidebarTab {
    Changed,
    Commits,
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
    diff_tab: DiffSidebarTab,
    local_changes: Vec<FileChange>,
    selected_commit_paths: BTreeSet<String>,
    selected_local_change: Option<usize>,
    commit_summary: String,
    commit_status: String,
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
            diff_tab: DiffSidebarTab::Changed,
            local_changes: Vec::new(),
            selected_commit_paths: BTreeSet::new(),
            selected_local_change: None,
            commit_summary: String::new(),
            commit_status: String::new(),
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
        self.handle_global_shortcuts(ctx);
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
                    ui.add_space(PANEL_LEFT_PAD);
                    ui.label(RichText::new("Diffist").color(theme::TEXT_DIM).strong());
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

    fn handle_global_shortcuts(&mut self, ctx: &egui::Context) {
        if self.settings.is_recording_shortcut() || self.mode == Mode::FileBrowser {
            return;
        }

        let editor_shortcut = self.settings.editor_shortcut();
        let switch_editor =
            ctx.input_mut(|i| i.consume_key(editor_shortcut.modifiers(), editor_shortcut.key()));
        if switch_editor {
            self.mode = Mode::FileBrowser;
            return;
        }

        let diff_shortcut = self.settings.diff_shortcut();
        let switch_diff =
            ctx.input_mut(|i| i.consume_key(diff_shortcut.modifiers(), diff_shortcut.key()));
        if switch_diff {
            self.mode = Mode::GitDiff;
        }
    }

    fn sync_window_title(&mut self, ctx: &egui::Context) {
        if !self.title_dirty {
            return;
        }
        self.title_dirty = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(String::new()));
    }

    /// Set the working folder for both modes, and try to open it as a git repo.
    fn open_folder(&mut self, dir: &Path) {
        self.root = Some(dir.to_path_buf());
        self.title_dirty = true;
        self.selected_file = None;
        self.editor = None;
        self.file_status.clear();

        self.repo = None;
        self.local_changes.clear();
        self.selected_commit_paths.clear();
        self.selected_local_change = None;
        self.commit_summary.clear();
        self.commit_status.clear();
        self.commits.clear();
        self.selected_commit = None;
        self.changes.clear();
        self.selected_change = None;
        self.diff_lines.clear();

        match Repository::discover(dir) {
            Ok(repo) => {
                match gitmodel::workdir_changes(&repo) {
                    Ok(changes) => {
                        self.selected_commit_paths =
                            changes.iter().map(|c| c.path.clone()).collect();
                        self.local_changes = changes;
                    }
                    Err(e) => self.git_status = format!("local diff error: {e}"),
                }
                match gitmodel::list_commits(&repo, 500) {
                    Ok(commits) => {
                        self.git_status = format!(
                            "{} local changes · {} commits",
                            self.local_changes.len(),
                            commits.len()
                        );
                        self.commits = commits;
                        self.repo = Some(repo);
                    }
                    Err(e) => self.git_status = format!("git error: {e}"),
                }
            }
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
        // Land on something useful instead of an empty pane.
        if self.diff_tab == DiffSidebarTab::Changed
            && self.selected_local_change.is_none()
            && !self.local_changes.is_empty()
        {
            self.select_local_change(0);
        }
        if self.diff_tab == DiffSidebarTab::Commits
            && self.selected_commit.is_none()
            && !self.commits.is_empty()
        {
            self.select_commit(0);
        }
        if self.diff_tab == DiffSidebarTab::Commits
            && self.selected_change.is_none()
            && !self.changes.is_empty()
        {
            self.select_change(0);
        }

        egui::SidePanel::left("diff_sidebar")
            .resizable(true)
            .default_width(320.0)
            .frame(egui::Frame::none().fill(theme::SIDEBAR))
            .show(ctx, |ui| {
                let repo_name = self
                    .root
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Repository".to_string());
                diff_sidebar_header(ui, &repo_name, &self.git_status);
                if let Some(tab) = diff_tab_bar(
                    ui,
                    self.diff_tab,
                    self.local_changes.len(),
                    self.commits.len(),
                ) {
                    self.switch_diff_tab(tab);
                }

                match self.diff_tab {
                    DiffSidebarTab::Changed => {
                        diff_list_label(ui, "LOCAL CHANGES");
                        let list_height = (ui.available_height() - 156.0).max(80.0);
                        egui::ScrollArea::vertical()
                            .max_height(list_height)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                let mut select = None;
                                let mut toggled = None;
                                for (i, f) in self.local_changes.iter().enumerate() {
                                    let selected = self.selected_local_change == Some(i);
                                    let checked = self.selected_commit_paths.contains(&f.path);
                                    let row = changed_commit_row(
                                        ui,
                                        f,
                                        selected,
                                        checked,
                                        self.settings.ui_font_size(),
                                    );
                                    if row.toggle_clicked {
                                        toggled = Some((f.path.clone(), !checked));
                                    } else if row.response.clicked() {
                                        select = Some(i);
                                    }
                                }
                                if self.local_changes.is_empty() {
                                    empty_list_message(ui, "No local changes");
                                }
                                if let Some(i) = select {
                                    self.select_local_change(i);
                                }
                                if let Some((path, checked)) = toggled {
                                    if checked {
                                        self.selected_commit_paths.insert(path);
                                    } else {
                                        self.selected_commit_paths.remove(&path);
                                    }
                                }
                            });
                        commit_panel(ui, self);
                    }
                    DiffSidebarTab::Commits => {
                        diff_list_label(ui, "COMMITS");
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                let mut select = None;
                                for (i, c) in self.commits.iter().enumerate() {
                                    let selected = self.selected_commit == Some(i);
                                    if commit_row(ui, c, selected, self.settings.ui_font_size())
                                        .clicked()
                                    {
                                        select = Some(i);
                                    }
                                }
                                if let Some(i) = select {
                                    self.select_commit(i);
                                }
                            });
                    }
                }
            });

        if self.diff_tab == DiffSidebarTab::Commits {
            egui::SidePanel::left("changed_files")
                .resizable(true)
                .default_width(280.0)
                .frame(egui::Frame::none().fill(theme::SURFACE))
                .show(ctx, |ui| {
                    diff_secondary_header(ui, "Changed files");
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let mut select = None;
                            for (i, f) in self.changes.iter().enumerate() {
                                let selected = self.selected_change == Some(i);
                                if change_row(ui, f, selected, self.settings.ui_font_size())
                                    .clicked()
                                {
                                    select = Some(i);
                                }
                            }
                            if let Some(i) = select {
                                self.select_change(i);
                            }
                        });
                });
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            if self.diff_lines.is_empty() {
                let empty = match self.diff_tab {
                    DiffSidebarTab::Changed => "Select a changed file",
                    DiffSidebarTab::Commits => "Select a commit, then a file",
                };
                diff_empty_state(ui, empty);
                return;
            }
            match self.diff_tab {
                DiffSidebarTab::Changed => {
                    if let Some(i) = self.selected_local_change {
                        diff_file_header(
                            ui,
                            Path::new(&self.local_changes[i].path),
                            "Local changes",
                        );
                    }
                }
                DiffSidebarTab::Commits => {
                    if let Some(i) = self.selected_change {
                        let status = self
                            .selected_commit
                            .map(|idx| self.commits[idx].short_id.as_str())
                            .unwrap_or("Commit");
                        diff_file_header(ui, Path::new(&self.changes[i].path), status);
                    }
                }
            }
            textview::diff_view(ui, &self.diff_lines);
        });
    }

    fn switch_diff_tab(&mut self, tab: DiffSidebarTab) {
        if self.diff_tab == tab {
            return;
        }
        self.diff_tab = tab;
        self.selected_local_change = None;
        self.selected_commit = None;
        self.selected_change = None;
        self.changes.clear();
        self.diff_lines.clear();
    }

    fn select_local_change(&mut self, idx: usize) {
        self.selected_local_change = Some(idx);
        self.selected_commit = None;
        self.selected_change = None;
        let path = self.local_changes[idx].path.clone();
        if let Some(repo) = &self.repo {
            match gitmodel::workdir_file_patch(repo, &path) {
                Ok(text) => self.diff_lines = text.lines().map(str::to_owned).collect(),
                Err(e) => self.diff_lines = vec![format!("diff error: {e}")],
            }
        }
    }

    fn commit_selected_changes(&mut self) {
        let summary = self.commit_summary.trim().to_string();
        if summary.is_empty() || self.selected_commit_paths.is_empty() {
            return;
        }
        let paths: Vec<String> = self.selected_commit_paths.iter().cloned().collect();
        if let Some(repo) = &self.repo {
            match gitmodel::commit_paths(repo, &paths, &summary) {
                Ok(oid) => {
                    self.commit_status = format!("Committed {}", &oid.to_string()[..8]);
                    self.commit_summary.clear();
                    self.refresh_git_diff_state();
                }
                Err(e) => self.commit_status = format!("commit error: {e}"),
            }
        }
    }

    fn refresh_git_diff_state(&mut self) {
        if let Some(repo) = &self.repo {
            match gitmodel::workdir_changes(repo) {
                Ok(changes) => {
                    self.local_changes = changes;
                    self.selected_commit_paths =
                        self.local_changes.iter().map(|c| c.path.clone()).collect();
                    self.selected_local_change = None;
                    self.diff_lines.clear();
                }
                Err(e) => self.git_status = format!("local diff error: {e}"),
            }
            match gitmodel::list_commits(repo, 500) {
                Ok(commits) => {
                    self.git_status = format!(
                        "{} local changes · {} commits",
                        self.local_changes.len(),
                        commits.len()
                    );
                    self.commits = commits;
                }
                Err(e) => self.git_status = format!("git error: {e}"),
            }
        }
    }

    fn select_commit(&mut self, idx: usize) {
        self.selected_commit = Some(idx);
        self.selected_local_change = None;
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

fn diff_sidebar_header(ui: &mut egui::Ui, repo_name: &str, status: &str) {
    ui.add_space(14.0);
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        ui.vertical(|ui| {
            ui.label(
                RichText::new(repo_name)
                    .strong()
                    .color(theme::TEXT)
                    .size(ui_font_size(ui)),
            );
            ui.add_space(2.0);
            ui.label(
                RichText::new(status)
                    .color(theme::TEXT_DIM)
                    .size(ui_font_size(ui) * 0.78),
            );
        });
    });
    ui.add_space(14.0);
}

fn diff_tab_bar(
    ui: &mut egui::Ui,
    selected: DiffSidebarTab,
    changed_count: usize,
    commit_count: usize,
) -> Option<DiffSidebarTab> {
    let width = (ui.available_width() - 24.0).max(120.0);
    let height = 34.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, height), Sense::hover());
    let rect = rect.translate(Vec2::new(12.0, 0.0));
    ui.painter().rect_filled(rect, 6.0, theme::INSET);
    ui.painter()
        .rect_stroke(rect, 6.0, Stroke::new(1.0, theme::BORDER));

    let half = rect.width() / 2.0;
    let tabs = [
        (
            DiffSidebarTab::Changed,
            format!("Changed {changed_count}"),
            Rect::from_min_size(rect.min, Vec2::new(half, rect.height())),
        ),
        (
            DiffSidebarTab::Commits,
            format!("Commits {commit_count}"),
            Rect::from_min_size(
                Pos2::new(rect.left() + half, rect.top()),
                Vec2::new(half, rect.height()),
            ),
        ),
    ];

    let mut clicked = None;
    for (tab, label, tab_rect) in tabs {
        let response = ui.interact(tab_rect, ui.id().with(label.as_str()), Sense::click());
        let active = selected == tab;
        if active {
            ui.painter()
                .rect_filled(tab_rect.shrink(3.0), 5.0, theme::SIDEBAR_SELECTED);
        } else if response.hovered() {
            ui.painter()
                .rect_filled(tab_rect.shrink(3.0), 5.0, theme::SIDEBAR_HOVER);
        }
        ui.painter().text(
            tab_rect.center(),
            Align2::CENTER_CENTER,
            label,
            FontId::proportional(ui_font_size(ui) * 0.82),
            if active { theme::TEXT } else { theme::TEXT_DIM },
        );
        if response.clicked() {
            clicked = Some(tab);
        }
    }
    ui.add_space(12.0);
    clicked
}

fn diff_list_label(ui: &mut egui::Ui, text: &str) {
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        ui.label(
            RichText::new(text)
                .color(theme::TEXT_MUTED)
                .size(ui_font_size(ui) * 0.72)
                .strong(),
        );
    });
    ui.add_space(6.0);
}

fn diff_secondary_header(ui: &mut egui::Ui, text: &str) {
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.label(RichText::new(text).strong().color(theme::TEXT));
    });
    ui.add_space(10.0);
    ui.separator();
}

fn change_row(
    ui: &mut egui::Ui,
    change: &FileChange,
    selected: bool,
    font_size: f32,
) -> egui::Response {
    let row_h = 38.0;
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), Sense::click());
    let bg = row_bg(selected, response.hovered());
    if let Some(fill) = bg {
        ui.painter()
            .rect_filled(rect.shrink2(Vec2::new(8.0, 2.0)), 6.0, fill);
    }

    let badge = Rect::from_min_size(
        Pos2::new(rect.left() + 14.0, rect.center().y - 9.0),
        Vec2::new(20.0, 18.0),
    );
    ui.painter().rect_filled(
        badge,
        4.0,
        status_color(change.status).linear_multiply(0.22),
    );
    ui.painter().text(
        badge.center(),
        Align2::CENTER_CENTER,
        change.status,
        FontId::monospace(font_size * 0.78),
        status_color(change.status),
    );
    ui.painter().text(
        Pos2::new(badge.right() + 10.0, rect.center().y),
        Align2::LEFT_CENTER,
        ellipsize(
            ui,
            &change.path,
            FontId::proportional(font_size * 0.9),
            (rect.right() - badge.right() - 26.0).max(20.0),
        ),
        FontId::proportional(font_size * 0.9),
        if selected {
            theme::TEXT
        } else {
            theme::TEXT_DIM
        },
    );
    response.on_hover_text(&change.path)
}

struct ChangedRow {
    response: egui::Response,
    toggle_clicked: bool,
}

fn changed_commit_row(
    ui: &mut egui::Ui,
    change: &FileChange,
    selected: bool,
    checked: bool,
    font_size: f32,
) -> ChangedRow {
    let row_h = 38.0;
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), Sense::click());
    let bg = row_bg(selected, response.hovered());
    if let Some(fill) = bg {
        ui.painter()
            .rect_filled(rect.shrink2(Vec2::new(8.0, 2.0)), 6.0, fill);
    }

    let check_rect = Rect::from_center_size(
        Pos2::new(rect.left() + 20.0, rect.center().y),
        Vec2::new(14.0, 14.0),
    );
    let check_response = ui.interact(
        check_rect.expand(4.0),
        ui.id().with(("commit_check", &change.path)),
        Sense::click(),
    );
    paint_checkbox(ui, check_rect, checked, check_response.hovered());

    let badge = Rect::from_min_size(
        Pos2::new(rect.left() + 36.0, rect.center().y - 9.0),
        Vec2::new(20.0, 18.0),
    );
    ui.painter().rect_filled(
        badge,
        4.0,
        status_color(change.status).linear_multiply(0.22),
    );
    ui.painter().text(
        badge.center(),
        Align2::CENTER_CENTER,
        change.status,
        FontId::monospace(font_size * 0.78),
        status_color(change.status),
    );
    ui.painter().text(
        Pos2::new(badge.right() + 10.0, rect.center().y),
        Align2::LEFT_CENTER,
        ellipsize(
            ui,
            &change.path,
            FontId::proportional(font_size * 0.9),
            (rect.right() - badge.right() - 26.0).max(20.0),
        ),
        FontId::proportional(font_size * 0.9),
        if selected {
            theme::TEXT
        } else {
            theme::TEXT_DIM
        },
    );
    ChangedRow {
        response: response.on_hover_text(&change.path),
        toggle_clicked: check_response.clicked(),
    }
}

fn paint_checkbox(ui: &egui::Ui, rect: Rect, checked: bool, hovered: bool) {
    let fill = if checked {
        theme::ACCENT
    } else if hovered {
        theme::SIDEBAR_HOVER
    } else {
        theme::INSET
    };
    ui.painter().rect_filled(rect, 3.0, fill);
    ui.painter().rect_stroke(
        rect,
        3.0,
        Stroke::new(
            1.0,
            if checked {
                theme::ACCENT
            } else {
                theme::BORDER
            },
        ),
    );
    if checked {
        let a = Pos2::new(rect.left() + 3.0, rect.center().y);
        let b = Pos2::new(rect.left() + 6.0, rect.bottom() - 4.0);
        let c = Pos2::new(rect.right() - 3.0, rect.top() + 4.0);
        ui.painter()
            .line_segment([a, b], Stroke::new(1.6, Color32::WHITE));
        ui.painter()
            .line_segment([b, c], Stroke::new(1.6, Color32::WHITE));
    }
}

fn commit_panel(ui: &mut egui::Ui, app: &mut DiffistApp) {
    ui.add_space(10.0);
    ui.separator();
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.label(
            RichText::new(format!("{} selected", app.selected_commit_paths.len()))
                .color(theme::TEXT_DIM)
                .size(ui_font_size(ui) * 0.78),
        );
    });
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        let width = (ui.available_width() - 24.0).max(80.0);
        ui.add_sized(
            Vec2::new(width, 30.0),
            egui::TextEdit::singleline(&mut app.commit_summary)
                .hint_text("Summary")
                .desired_width(width),
        );
    });
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        let enabled =
            !app.commit_summary.trim().is_empty() && !app.selected_commit_paths.is_empty();
        let width = (ui.available_width() - 24.0).max(80.0);
        let button = egui::Button::new(RichText::new("Commit").strong().color(theme::TEXT)).fill(
            if enabled {
                theme::SIDEBAR_SELECTED
            } else {
                theme::RAISED
            },
        );
        if ui
            .add_enabled(enabled, button.min_size(Vec2::new(width, 32.0)))
            .clicked()
        {
            app.commit_selected_changes();
        }
    });
    if !app.commit_status.is_empty() {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.label(
                RichText::new(&app.commit_status)
                    .color(theme::TEXT_DIM)
                    .size(ui_font_size(ui) * 0.78),
            );
        });
    }
}

fn commit_row(
    ui: &mut egui::Ui,
    commit: &CommitInfo,
    selected: bool,
    font_size: f32,
) -> egui::Response {
    let row_h = 62.0;
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), Sense::click());
    let bg = row_bg(selected, response.hovered());
    if let Some(fill) = bg {
        ui.painter()
            .rect_filled(rect.shrink2(Vec2::new(8.0, 3.0)), 6.0, fill);
    }
    let x = rect.left() + 14.0;
    let y = rect.top() + 11.0;
    let summary_font = FontId::proportional(font_size * 0.9);
    let meta_font = FontId::proportional(font_size * 0.72);
    let text_w = (rect.right() - x - 14.0).max(20.0);
    ui.painter().text(
        Pos2::new(x, y),
        Align2::LEFT_TOP,
        ellipsize(ui, &commit.summary, summary_font.clone(), text_w),
        summary_font,
        if selected {
            theme::TEXT
        } else {
            theme::TEXT_DIM
        },
    );
    ui.painter().text(
        Pos2::new(x, y + 24.0),
        Align2::LEFT_TOP,
        ellipsize(
            ui,
            &format!("{}  {}  {}", commit.short_id, commit.author, commit.date),
            meta_font.clone(),
            text_w,
        ),
        meta_font,
        theme::TEXT_MUTED,
    );
    response.on_hover_text(format!(
        "{}\n{}  {}  {}",
        commit.summary, commit.short_id, commit.author, commit.date
    ))
}

fn row_bg(selected: bool, hovered: bool) -> Option<Color32> {
    if selected {
        Some(theme::SIDEBAR_SELECTED)
    } else if hovered {
        Some(theme::SIDEBAR_HOVER)
    } else {
        None
    }
}

fn empty_list_message(ui: &mut egui::Ui, text: &str) {
    ui.add_space(20.0);
    ui.centered_and_justified(|ui| {
        ui.label(RichText::new(text).color(theme::TEXT_MUTED));
    });
}

fn diff_file_header(ui: &mut egui::Ui, path: &Path, context: &str) {
    let rect = ui.available_rect_before_wrap();
    let height = 54.0;
    let (header, _) = ui.allocate_exact_size(Vec2::new(rect.width(), height), Sense::hover());
    ui.painter().rect_filled(header, 0.0, theme::SURFACE);
    ui.painter().line_segment(
        [header.left_bottom(), header.right_bottom()],
        Stroke::new(1.0, theme::BORDER),
    );
    ui.painter().text(
        Pos2::new(header.left() + 14.0, header.top() + 10.0),
        Align2::LEFT_TOP,
        ellipsize(
            ui,
            &path.to_string_lossy(),
            FontId::monospace(ui_font_size(ui) * 0.9),
            (header.width() - 28.0).max(20.0),
        ),
        FontId::monospace(ui_font_size(ui) * 0.9),
        theme::TEXT,
    );
    ui.painter().text(
        Pos2::new(header.left() + 14.0, header.top() + 32.0),
        Align2::LEFT_TOP,
        context,
        FontId::proportional(ui_font_size(ui) * 0.72),
        theme::TEXT_MUTED,
    );
}

fn diff_empty_state(ui: &mut egui::Ui, text: &str) {
    let rect = ui.available_rect_before_wrap();
    ui.painter().rect_filled(rect, 0.0, theme::INSET);
    ui.centered_and_justified(|ui| {
        ui.label(RichText::new(text).color(theme::TEXT_MUTED).strong());
    });
}

/// A dim, uppercase section label like GitHub's sidebar headers.
fn section_header(ui: &mut egui::Ui, text: &str) {
    let font_size = ui_font_size(ui);
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(PANEL_LEFT_PAD);
        ui.label(
            RichText::new(text)
                .color(theme::TEXT_MUTED)
                .size(font_size * 0.78)
                .strong(),
        );
    });
    ui.add_space(4.0);
    ui.separator();
}

/// A breadcrumb-style header for the content pane: path + optional status.
fn file_header(ui: &mut egui::Ui, path: &Path, status: &str) {
    let font_size = ui_font_size(ui);
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(path.to_string_lossy())
                .monospace()
                .color(theme::TEXT),
        );
        if !status.is_empty() {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new(status)
                        .color(theme::TEXT_DIM)
                        .size(font_size * 0.85),
                );
            });
        }
    });
    ui.separator();
}

fn ui_font_size(ui: &egui::Ui) -> f32 {
    TextStyle::Body.resolve(ui.style()).size
}

fn ellipsize(ui: &egui::Ui, text: &str, font: FontId, max_width: f32) -> String {
    if text_width(ui, text, font.clone()) <= max_width {
        return text.to_string();
    }

    let ellipsis = "...";
    let ellipsis_w = text_width(ui, ellipsis, font.clone());
    let target = (max_width - ellipsis_w).max(0.0);
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

fn status_color(s: char) -> Color32 {
    match s {
        'A' => theme::GREEN,
        'M' => theme::YELLOW,
        'D' => theme::RED,
        'R' | 'C' => theme::ACCENT,
        _ => theme::TEXT_DIM,
    }
}
