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
use crate::terminal::Terminal;
use crate::{agent, fstree, textview, theme};

const PANEL_LEFT_PAD: f32 = 12.0;

#[derive(PartialEq, Eq, Clone, Copy)]
enum Mode {
    FileBrowser,
    GitDiff,
    Agent,
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum DiffSidebarTab {
    Changed,
    Commits,
}

pub struct CodezApp {
    mode: Mode,
    root: Option<PathBuf>,

    // --- File Browser (Editor) mode ---
    selected_file: Option<PathBuf>,
    editor: Option<Editor>,
    file_status: String,
    last_dirty: bool,
    file_mtime: Option<std::time::SystemTime>,
    external_changed: bool,

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
    /// Background `git push` result channel; `Some` while a push is in flight.
    push_rx: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
    push_status: String,

    // --- Chrome ---
    menus: Menus,
    settings: Settings,
    title_dirty: bool,
    show_shortcuts: bool,
    /// Startup prompt offering to install the `codez` CLI.
    show_cli_prompt: bool,
    update_rx:
        Option<std::sync::mpsc::Receiver<Result<Option<crate::updater::UpdateInfo>, String>>>,
    update_check_started: bool,
    update_available: Option<crate::updater::UpdateInfo>,
    update_dismissed_version: Option<String>,

    // --- File-tree operations ---
    file_clipboard: Option<PathBuf>,
    prompt: Option<FilePrompt>,
    prompt_text: String,
    prompt_error: String,

    // --- Find in Files ---
    project_search: crate::search::ProjectSearch,
    // --- Quick-open / command palette ---
    palette: crate::palette::Palette,

    // --- Agent mode ---
    sessions: Vec<agent::AgentSession>,
    /// Open terminal tabs and the active one.
    terminals: Vec<TermTab>,
    active_term: Option<usize>,
    /// Monotonic counter for naming blank terminals ("shell 1", "shell 2", …).
    term_seq: u32,
    agent_status: String,
    agent_scan_at: std::time::Instant,
    /// Codex / Claude usage readouts, refreshed in the background.
    usage: crate::usage::Usage,
    usage_rx: Option<std::sync::mpsc::Receiver<crate::usage::Usage>>,
    usage_at: Option<std::time::Instant>,
    /// Last time the Diff page re-probed a non-git folder for a new repo.
    git_recheck_at: std::time::Instant,
    git_watch_at: std::time::Instant,
    git_refresh_rx: Option<std::sync::mpsc::Receiver<Result<gitmodel::RepoSnapshot, String>>>,
    git_snapshot_token: Option<String>,
}

/// One terminal tab in Agent mode.
struct TermTab {
    title: String,
    /// The resumed session's id, if this tab was opened from the session list.
    session_id: Option<String>,
    terminal: Terminal,
}

/// A pending file-tree text prompt (modal naming dialog).
#[derive(Clone)]
enum FilePrompt {
    NewFile(PathBuf),
    NewFolder(PathBuf),
    Rename(PathBuf),
}

impl CodezApp {
    /// Build the app (installing the native menu bar), optionally opening
    /// `initial_dir` on startup. Must run on the main thread after the
    /// NSApplication exists, i.e. from eframe's creation closure.
    pub fn new(initial_dir: Option<PathBuf>, initial_file: Option<PathBuf>) -> Self {
        // Optional override so the app can launch straight into Diff mode.
        let mode = match std::env::var("CODEZ_MODE").as_deref() {
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
            file_mtime: None,
            external_changed: false,
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
            push_rx: None,
            push_status: String::new(),
            menus: Menus::install(),
            settings: Settings::load(),
            title_dirty: false,
            show_shortcuts: false,
            // Offer to install the CLI on first launches, until installed or dismissed.
            show_cli_prompt: !crate::cli::installed() && !crate::cli::prompt_dismissed(),
            update_rx: None,
            update_check_started: false,
            update_available: None,
            update_dismissed_version: None,
            file_clipboard: None,
            prompt: None,
            prompt_text: String::new(),
            prompt_error: String::new(),
            project_search: crate::search::ProjectSearch::default(),
            palette: crate::palette::Palette::default(),
            sessions: Vec::new(),
            terminals: Vec::new(),
            active_term: None,
            term_seq: 0,
            usage: crate::usage::Usage::default(),
            usage_rx: None,
            usage_at: None,
            agent_status: String::new(),
            agent_scan_at: std::time::Instant::now(),
            git_recheck_at: std::time::Instant::now(),
            git_watch_at: std::time::Instant::now(),
            git_refresh_rx: None,
            git_snapshot_token: None,
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

impl eframe::App for CodezApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_menu();
        // View switching is via the native View menu: ⌘1 Editor / ⌘2 Diff / ⌘3 Agent.
        // ⌘⇧F → Find in Files. Consume before the editor sees the key (its ⌘F
        // handler ignores Shift and would otherwise also fire).
        if ctx.input_mut(|i| {
            i.consume_key(
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                egui::Key::F,
            )
        }) {
            self.mode = Mode::FileBrowser;
            self.project_search.reveal();
        }
        // ⌘⇧P command palette, ⌘P file quick-open. Consume so the editor (which
        // ignores them anyway) never sees the keys.
        if ctx.input_mut(|i| {
            i.consume_key(
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                egui::Key::P,
            )
        }) {
            self.palette.open_commands();
        } else if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::P)) {
            if let Some(root) = self.root.clone() {
                self.palette.open_files(&root);
            }
        }
        self.handle_palette(ctx);
        self.poll_update(ctx);
        self.check_external_change();
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
            Mode::Agent => self.agent_ui(ctx),
        }
        self.settings.ui(ctx);
        self.file_prompt_ui(ctx);
        self.shortcuts_ui(ctx);
        self.cli_prompt_ui(ctx);
        self.update_prompt_ui(ctx);

        // Native menu clicks don't wake the window, so poll a few times a second.
        ctx.request_repaint_after(std::time::Duration::from_millis(150));
    }
}

impl CodezApp {
    /// Top strip: folder name on the left, Editor/Diff segmented switch on the
    /// right (the rest of the menu lives in the native macOS menu bar).
    fn mode_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("mode_bar")
            .exact_height(34.0)
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.add_space(PANEL_LEFT_PAD);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.label(RichText::new("Code").color(theme::TEXT).strong());
                        ui.label(RichText::new("Z").color(theme::TEXT_MUTED).strong());
                    });
                    ui.add_space(7.0);
                    if help_icon(ui) {
                        self.show_shortcuts = true;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add_space(4.0);
                        if let Some(mode) = mode_switch(ui, self.mode) {
                            self.mode = mode;
                        }
                    });
                });
            });
    }

    /// Modal listing every keyboard shortcut, opened by the `?` in the top bar.
    fn shortcuts_ui(&mut self, ctx: &egui::Context) {
        if !self.show_shortcuts {
            return;
        }
        let mut open = true;
        egui::Window::new("Keyboard Shortcuts")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(540.0)
                    .show(ui, |ui| {
                        shortcut_group(
                            ui,
                            "General",
                            &[
                                ("⌘1", "Agent"),
                                ("⌘2", "Editor"),
                                ("⌘3", "Diff"),
                                ("⌘O", "Open folder"),
                                ("⌘S", "Save file"),
                                ("⌘,", "Settings"),
                                ("⌘P", "Quick-open file"),
                                ("⌘⇧P", "Command palette"),
                                ("⌘⇧F", "Find in files"),
                            ],
                        );
                        shortcut_group(
                            ui,
                            "Editor",
                            &[
                                ("⌘Z  /  ⌘⇧Z", "Undo / Redo"),
                                ("⌘X  ⌘C  ⌘V", "Cut / Copy / Paste"),
                                ("⌘A", "Select all"),
                                ("⌘F  /  ⌘⌥F", "Find / Find & replace"),
                                ("⌘G  /  ⌘⇧G", "Next / Previous match"),
                                ("⌘D", "Select next occurrence"),
                                ("⌘L  /  ⌘⇧L", "Select line / Split selection to lines"),
                                ("⌘⇧D  /  ⌘⇧K", "Duplicate / Delete lines"),
                                ("⌘/", "Toggle line comment"),
                                ("Tab  /  ⇧Tab", "Indent / Unindent"),
                                ("⌘↵  /  ⌘⇧↵", "Insert line below / above"),
                                ("⌘←  /  ⌘→", "Line start / end"),
                                ("⌥←  /  ⌥→", "Word left / right"),
                                ("⌘⌫", "Delete to line start"),
                                ("Esc", "Collapse selections / cursors"),
                            ],
                        );
                        shortcut_group(
                            ui,
                            "Agent terminal",
                            &[
                                ("⌘C", "Copy selection"),
                                ("⌘V", "Paste"),
                                ("⌘+Click", "Open link under cursor"),
                                ("Drag", "Select text"),
                                ("Double / Triple-click", "Select word / line"),
                            ],
                        );
                        ui.add_space(8.0);
                    });
            });
        if !open {
            self.show_shortcuts = false;
        }
    }

    /// Startup prompt: offer to install the `codez` shell command, with
    /// Install / Not now / Don't remind me again.
    fn cli_prompt_ui(&mut self, ctx: &egui::Context) {
        if !self.show_cli_prompt {
            return;
        }
        egui::Window::new("Install codez command")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.label("Open folders from the terminal with the codez command.");
                ui.label(
                    RichText::new("e.g.   codez .    or    codez <path>").color(theme::TEXT_DIM),
                );
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if theme::pill_button(ui, "Install") {
                        let _ = crate::cli::install();
                        self.show_cli_prompt = false;
                    }
                    ui.add_space(8.0);
                    if theme::pill_button(ui, "Not now") {
                        self.show_cli_prompt = false;
                    }
                    ui.add_space(8.0);
                    if theme::pill_button(ui, "Don't remind me again") {
                        crate::cli::dismiss_prompt();
                        self.show_cli_prompt = false;
                    }
                });
                ui.add_space(4.0);
            });
    }

    /// Start one background update check per app launch and collect the result.
    fn poll_update(&mut self, ctx: &egui::Context) {
        if !self.update_check_started {
            self.update_check_started = true;
            let (tx, rx) = std::sync::mpsc::channel();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(crate::updater::check());
                ctx.request_repaint();
            });
            self.update_rx = Some(rx);
        }

        if let Some(rx) = &self.update_rx {
            if let Ok(result) = rx.try_recv() {
                self.update_rx = None;
                if let Ok(Some(update)) = result {
                    if self.update_dismissed_version.as_deref() != Some(update.version.as_str()) {
                        self.update_available = Some(update);
                    }
                }
            }
        }
    }

    fn update_prompt_ui(&mut self, ctx: &egui::Context) {
        let Some(update) = self.update_available.clone() else {
            return;
        };
        let mut open = true;
        egui::Window::new("Update available")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.label(
                    RichText::new(format!(
                        "CodeZ {} is available. You are running {}.",
                        update.version,
                        env!("CARGO_PKG_VERSION")
                    ))
                    .color(theme::TEXT),
                );
                if let Some(notes) = &update.notes {
                    ui.add_space(8.0);
                    ui.label(RichText::new(notes).color(theme::TEXT_DIM));
                }
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if theme::pill_button(ui, "Download") {
                        let _ = std::process::Command::new("open")
                            .arg(&update.download_url)
                            .spawn();
                        self.update_available = None;
                    }
                    ui.add_space(8.0);
                    if theme::pill_button(ui, "Not now") {
                        self.update_dismissed_version = Some(update.version.clone());
                        self.update_available = None;
                    }
                });
                ui.add_space(4.0);
            });

        if !open {
            self.update_dismissed_version = Some(update.version);
            self.update_available = None;
        }
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
                self.save_current();
            } else if *id == self.menus.settings {
                self.settings.open = true;
            } else if *id == self.menus.editor {
                self.mode = Mode::FileBrowser;
            } else if *id == self.menus.diff {
                self.mode = Mode::GitDiff;
            } else if *id == self.menus.agent {
                self.mode = Mode::Agent;
            }
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
        self.git_refresh_rx = None;
        self.git_snapshot_token = None;

        // Drop any running terminals (sends shutdown) and rescan agent sessions.
        self.terminals.clear();
        self.active_term = None;
        self.agent_status.clear();
        self.sessions = agent::scan(dir);

        self.git_recheck_at = std::time::Instant::now();
        self.load_git_state();
    }

    /// Discover a git repo at the current `root` and load its diff state
    /// (local changes + commit history). Used both when opening a folder and
    /// when the Diff page re-probes a folder that wasn't a repo before.
    /// Returns true once a repository is open.
    fn load_git_state(&mut self) -> bool {
        let Some(root) = self.root.clone() else {
            return false;
        };
        match Repository::discover(&root) {
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
                        self.git_snapshot_token = Some(gitmodel::snapshot_token(
                            &repo,
                            &self.local_changes,
                            &self.commits,
                        ));
                        self.repo = Some(repo);
                    }
                    Err(e) => self.git_status = format!("git error: {e}"),
                }
                true
            }
            Err(_) => {
                self.git_status = "not a git repository".to_string();
                false
            }
        }
    }

    // ---------------- Agent mode ----------------

    /// Two columns: Codex sessions for this folder on the left, an interactive
    /// terminal for the selected session on the right.
    fn agent_ui(&mut self, ctx: &egui::Context) {
        // Re-scan periodically so sessions started just now (whose transcript
        // files are written a moment after launch), or in other terminals,
        // appear without reopening the folder.
        if let Some(root) = self.root.clone() {
            if self.agent_scan_at.elapsed() >= std::time::Duration::from_secs(2) {
                self.sessions = agent::scan(&root);
                self.agent_scan_at = std::time::Instant::now();
            }
        }
        self.poll_usage(ctx);
        let usage = self.usage.clone();

        egui::SidePanel::left("agent_sessions")
            .resizable(true)
            .default_width(300.0)
            .frame(egui::Frame::none().fill(theme::SIDEBAR))
            .show(ctx, |ui| {
                // Usage readouts pinned to the bottom; declared first so the
                // list above fills the remaining height. Shown only when at
                // least one agent (codex / claude) is present on this machine.
                if usage.codex.is_some() || usage.claude.is_some() {
                    egui::TopBottomPanel::bottom("agent_usage")
                        .frame(egui::Frame::none().fill(theme::SIDEBAR))
                        .show_inside(ui, |ui| usage_panel(ui, &usage));
                }

                section_header(ui, "AGENT SESSIONS");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    let width = (ui.available_width() - 24.0).max(80.0);
                    // A blank terminal — the user runs whatever they want
                    // (`codex`, `claude`, a plain shell, …).
                    if ui
                        .add_sized(
                            Vec2::new(width, 30.0),
                            egui::Button::new(RichText::new("+ New Terminal").color(theme::TEXT))
                                .fill(theme::RAISED),
                        )
                        .clicked()
                    {
                        self.open_blank_terminal(ui.ctx());
                    }
                });
                ui.add_space(8.0);

                // A session row is highlighted when the active tab is resuming it.
                let active_sid = self
                    .active_term
                    .and_then(|i| self.terminals.get(i))
                    .and_then(|t| t.session_id.clone());
                let font_size = self.settings.ui_font_size();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let mut select = None;
                        for s in self.sessions.iter() {
                            let selected = active_sid.as_deref() == Some(s.id.as_str());
                            if session_row(ui, s, selected, font_size).clicked() {
                                select = Some(s.id.clone());
                            }
                        }
                        if self.sessions.is_empty() {
                            empty_list_message(ui, "No agent sessions for this folder");
                        }
                        if let Some(id) = select {
                            self.open_session(ui.ctx(), &id);
                        }
                    });
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(theme::INSET))
            .show(ctx, |ui| {
                if self.root.is_none() {
                    diff_empty_state(ui, "Open a folder to use agents");
                    return;
                }
                if self.terminals.is_empty() {
                    diff_empty_state(ui, "Select a session to resume, or open a new terminal");
                    return;
                }

                let font_size = ui_font_size(ui);
                let action = terminal_tab_bar(ui, &self.terminals, self.active_term, font_size);
                match action {
                    TabAction::Switch(i) => {
                        self.active_term = Some(i);
                        if let Some(t) = self.terminals.get_mut(i) {
                            t.terminal.request_focus();
                        }
                    }
                    TabAction::Close(i) => self.close_terminal(i),
                    TabAction::None => {}
                }

                if let Some(tab) = self.active_term.and_then(|i| self.terminals.get_mut(i)) {
                    tab.terminal.ui(ui);
                }
            });
    }

    /// Open a session from the list in a tab — reusing an existing tab for the
    /// same session, or spawning a new one that runs its resume command.
    fn open_session(&mut self, ctx: &egui::Context, id: &str) {
        if let Some(i) = self
            .terminals
            .iter()
            .position(|t| t.session_id.as_deref() == Some(id))
        {
            self.active_term = Some(i);
            self.terminals[i].terminal.request_focus();
            return;
        }
        let Some(session) = self.sessions.iter().find(|s| s.id == id) else {
            return;
        };
        let short: String = id.chars().take(6).collect();
        let title = format!("{} {short}", session.kind.label());
        let command = session.resume_command();
        self.open_terminal(ctx, title, Some(id.to_string()), Some(command));
    }

    /// Open a blank login-shell terminal tab (the user runs whatever they want).
    fn open_blank_terminal(&mut self, ctx: &egui::Context) {
        self.term_seq += 1;
        let title = format!("shell {}", self.term_seq);
        self.open_terminal(ctx, title, None, None);
    }

    /// Spawn a terminal in the open folder and add it as a new active tab.
    fn open_terminal(
        &mut self,
        ctx: &egui::Context,
        title: String,
        session_id: Option<String>,
        command: Option<String>,
    ) {
        let Some(root) = self.root.clone() else {
            return;
        };
        match Terminal::spawn(ctx, root, command) {
            Ok(terminal) => {
                self.terminals.push(TermTab {
                    title,
                    session_id,
                    terminal,
                });
                self.active_term = Some(self.terminals.len() - 1);
                self.agent_status.clear();
            }
            Err(e) => self.agent_status = format!("terminal error: {e}"),
        }
    }

    /// Collect a finished usage fetch and kick off a new one every 60s.
    fn poll_usage(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.usage_rx {
            if let Ok(usage) = rx.try_recv() {
                self.usage = usage;
                self.usage_rx = None;
                self.usage_at = Some(std::time::Instant::now());
            }
        }
        let due = self
            .usage_at
            .map_or(true, |t| t.elapsed() >= std::time::Duration::from_secs(60));
        if due && self.usage_rx.is_none() {
            let (tx, rx) = std::sync::mpsc::channel();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(crate::usage::fetch());
                ctx.request_repaint();
            });
            self.usage_rx = Some(rx);
            self.usage_at = Some(std::time::Instant::now()); // don't respawn each frame
        }
    }

    /// Close tab `i`, dropping its child process, and keep `active_term` valid.
    fn close_terminal(&mut self, i: usize) {
        if i >= self.terminals.len() {
            return;
        }
        self.terminals.remove(i); // drops the Terminal → sends shutdown
        self.active_term = if self.terminals.is_empty() {
            None
        } else {
            let prev = self.active_term.unwrap_or(0);
            let next = if prev > i { prev - 1 } else { prev };
            Some(next.min(self.terminals.len() - 1))
        };
        if let Some(t) = self.active_term.and_then(|a| self.terminals.get_mut(a)) {
            t.terminal.request_focus();
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
                    let can_paste = self.file_clipboard.is_some();
                    let (clicked, action) =
                        fstree::show(ui, &root, self.selected_file.as_deref(), can_paste);
                    if let Some(path) = clicked {
                        self.load_file(&path);
                    }
                    if let Some(action) = action {
                        self.handle_tree_action(action, ui.ctx());
                    }
                } else {
                    ui.weak("File ▸ Open Folder…  (⌘O)");
                }
            });

        // Find-in-Files results (bottom panel, added before the central panel).
        if let Some(root) = self.root.clone() {
            if let Some(target) = self.project_search.ui(ctx, &root) {
                self.load_file(&target.path);
                if let Some(editor) = &mut self.editor {
                    editor.reveal_match(target.line, target.col, target.len);
                }
            }
        }

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
            let dirty = self.editor.as_ref().is_some_and(Editor::is_dirty);
            file_header(ui, &path, &status, dirty);

            // Banner shown when the file changed on disk while you have edits.
            if self.external_changed {
                egui::Frame::none()
                    .fill(theme::DIFF_DEL_BG)
                    .inner_margin(egui::Margin::symmetric(8.0, 6.0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("⚠ File changed on disk").color(theme::YELLOW));
                            if theme::pill_button(ui, "Reload") {
                                self.load_file(&path);
                            }
                            ui.add_space(8.0);
                            if theme::pill_button(ui, "Keep mine") {
                                self.external_changed = false;
                            }
                        });
                    });
            }

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
        self.file_mtime = file_mtime(path);
        self.external_changed = false;
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

    /// Save and refresh the tracked mtime so our own write isn't mistaken for an
    /// external change.
    fn save_current(&mut self) {
        if let Some(editor) = &mut self.editor {
            if editor.save().is_ok() {
                self.file_mtime = self.selected_file.as_deref().and_then(file_mtime);
                self.external_changed = false;
            }
        }
    }

    /// Detect when the open file changed on disk: silently reload if there are
    /// no unsaved edits, otherwise raise a banner.
    fn check_external_change(&mut self) {
        if self.external_changed || self.editor.is_none() {
            return;
        }
        let Some(path) = self.selected_file.clone() else {
            return;
        };
        let Some(cur) = file_mtime(&path) else { return };
        if Some(cur) != self.file_mtime {
            self.file_mtime = Some(cur);
            if self.editor.as_ref().is_some_and(Editor::is_dirty) {
                self.external_changed = true;
            } else {
                self.load_file(&path);
            }
        }
    }

    // ---------------- File-tree operations ----------------

    fn handle_tree_action(&mut self, action: fstree::TreeAction, ctx: &egui::Context) {
        use fstree::TreeAction::*;
        match action {
            NewFile(dir) => self.open_prompt(FilePrompt::NewFile(dir), String::new()),
            NewFolder(dir) => self.open_prompt(FilePrompt::NewFolder(dir), String::new()),
            Rename(path) => {
                let name = file_name_string(&path);
                self.open_prompt(FilePrompt::Rename(path), name);
            }
            Delete(path) => {
                if trash::delete(&path).is_ok() && self.selected_file.as_deref() == Some(&*path) {
                    self.selected_file = None;
                    self.editor = None;
                }
            }
            Duplicate(path) => {
                let _ = copy_into(&path, &unique_sibling(&path));
            }
            CopyFile(path) => self.file_clipboard = Some(path),
            Paste(dir) => {
                if let Some(src) = self.file_clipboard.clone() {
                    let mut dest = dir.join(file_name_string(&src));
                    if dest == src || dest.exists() {
                        dest = unique_sibling(&dest);
                    }
                    let _ = copy_into(&src, &dest);
                }
            }
            RevealInFinder(path) => {
                let _ = std::process::Command::new("open")
                    .arg("-R")
                    .arg(&path)
                    .spawn();
            }
            OpenInTerminal(dir) => {
                let _ = std::process::Command::new("open")
                    .args(["-a", "Terminal"])
                    .arg(&dir)
                    .spawn();
            }
            CopyPath(path) => ctx.copy_text(path.to_string_lossy().into_owned()),
            CopyRelativePath(path) => {
                let rel = self
                    .root
                    .as_ref()
                    .and_then(|r| path.strip_prefix(r).ok())
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.to_string_lossy().into_owned());
                ctx.copy_text(rel);
            }
        }
    }

    fn open_prompt(&mut self, prompt: FilePrompt, initial: String) {
        self.prompt = Some(prompt);
        self.prompt_text = initial;
        self.prompt_error.clear();
    }

    /// Modal naming dialog for new-file / new-folder / rename.
    fn file_prompt_ui(&mut self, ctx: &egui::Context) {
        let Some(prompt) = self.prompt.clone() else {
            return;
        };
        let title = match prompt {
            FilePrompt::NewFile(_) => "New File",
            FilePrompt::NewFolder(_) => "New Folder",
            FilePrompt::Rename(_) => "Rename",
        };
        let mut submit = false;
        let mut cancel = false;
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.prompt_text)
                        .desired_width(300.0)
                        .hint_text("name"),
                );
                resp.request_focus();
                if !self.prompt_error.is_empty() {
                    ui.colored_label(theme::RED, &self.prompt_error);
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    submit |= theme::pill_button(ui, "OK");
                    ui.add_space(8.0);
                    cancel |= theme::pill_button(ui, "Cancel");
                });
                submit |= ui.input(|i| i.key_pressed(egui::Key::Enter));
                cancel |= ui.input(|i| i.key_pressed(egui::Key::Escape));
            });
        if cancel {
            self.prompt = None;
        } else if submit {
            self.apply_prompt(prompt);
        }
    }

    fn apply_prompt(&mut self, prompt: FilePrompt) {
        let name = self.prompt_text.trim().to_owned();
        if name.is_empty() || name.contains('/') {
            self.prompt_error = "Invalid name".to_owned();
            return;
        }
        let result: std::io::Result<Option<PathBuf>> = match &prompt {
            FilePrompt::NewFile(dir) => {
                let p = dir.join(&name);
                if p.exists() {
                    Err(already_exists())
                } else {
                    std::fs::write(&p, b"").map(|_| Some(p))
                }
            }
            FilePrompt::NewFolder(dir) => {
                let p = dir.join(&name);
                if p.exists() {
                    Err(already_exists())
                } else {
                    std::fs::create_dir(&p).map(|_| None)
                }
            }
            FilePrompt::Rename(path) => {
                let dest = path.parent().unwrap_or(Path::new(".")).join(&name);
                if dest.exists() && dest != *path {
                    Err(already_exists())
                } else {
                    let was_open = self.selected_file.as_deref() == Some(path.as_path());
                    std::fs::rename(path, &dest).map(|_| was_open.then_some(dest))
                }
            }
        };
        match result {
            Ok(open) => {
                self.prompt = None;
                self.prompt_error.clear();
                if let Some(p) = open {
                    self.load_file(&p);
                }
            }
            Err(e) => self.prompt_error = e.to_string(),
        }
    }

    fn handle_palette(&mut self, ctx: &egui::Context) {
        use crate::palette::{Command, PaletteAction};
        let Some(action) = self.palette.ui(ctx) else {
            return;
        };
        match action {
            PaletteAction::OpenFile(path) => {
                self.mode = Mode::FileBrowser;
                self.load_file(&path);
            }
            PaletteAction::GotoLine(n) => {
                if let Some(editor) = &mut self.editor {
                    editor.reveal_match(n.saturating_sub(1), 0, 0);
                }
            }
            PaletteAction::Run(cmd) => match cmd {
                Command::OpenFolder => {
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        self.open_folder(&dir);
                    }
                }
                Command::Save => self.save_current(),
                Command::ModeEditor => self.mode = Mode::FileBrowser,
                Command::ModeDiff => self.mode = Mode::GitDiff,
                Command::FindInFiles => {
                    self.mode = Mode::FileBrowser;
                    self.project_search.reveal();
                }
                Command::Settings => self.settings.open = true,
            },
        }
    }

    // ---------------- Git Diff mode ----------------

    fn git_diff_ui(&mut self, ctx: &egui::Context) {
        // If the folder wasn't a git repo when opened, keep watching it: the
        // moment `git init` (or a clone/checkout) turns it into one, load the
        // repo and re-render. We only poll while no repo is open, so this never
        // disturbs in-progress selections, and `request_repaint_after` keeps
        // the poll ticking even when the UI is otherwise idle.
        if self.repo.is_none() && self.root.is_some() {
            let interval = std::time::Duration::from_millis(1200);
            if self.git_recheck_at.elapsed() >= interval {
                self.git_recheck_at = std::time::Instant::now();
                self.load_git_state();
            }
            ctx.request_repaint_after(interval);
        } else {
            self.poll_git_external_changes(ctx);
        }

        // Collect a finished background push, if any.
        if let Some(rx) = &self.push_rx {
            if let Ok(result) = rx.try_recv() {
                self.push_status = match result {
                    Ok(msg) => msg,
                    Err(err) => format!("push failed: {err}"),
                };
                self.push_rx = None;
            }
        }

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
                let branch = self.repo.as_ref().and_then(gitmodel::current_branch);
                if push_toolbar(
                    ui,
                    branch.as_deref(),
                    self.push_rx.is_some(),
                    &self.push_status,
                ) {
                    self.start_push(ui.ctx());
                }
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
                        // Commit box pinned to the bottom; the list fills the
                        // remaining space above it.
                        egui::TopBottomPanel::bottom("commit_panel")
                            .frame(egui::Frame::none().fill(theme::SIDEBAR))
                            .show_inside(ui, |ui| commit_panel(ui, self));
                        diff_list_label(ui, "LOCAL CHANGES");
                        egui::ScrollArea::vertical()
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

    /// Kick off `git push origin <current-branch>` on a background thread.
    fn start_push(&mut self, ctx: &egui::Context) {
        if self.push_rx.is_some() {
            return; // already pushing
        }
        let (Some(repo), Some(branch)) = (
            self.repo.as_ref(),
            self.repo.as_ref().and_then(gitmodel::current_branch),
        ) else {
            self.push_status = "no branch to push (detached HEAD?)".to_string();
            return;
        };
        let Some(workdir) = repo.workdir().map(Path::to_path_buf) else {
            self.push_status = "no working directory".to_string();
            return;
        };

        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = gitmodel::push_origin(&workdir, &branch);
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        self.push_rx = Some(rx);
        self.push_status = "Pushing…".to_string();
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
            self.git_snapshot_token = Some(gitmodel::snapshot_token(
                repo,
                &self.local_changes,
                &self.commits,
            ));
        }
    }

    fn poll_git_external_changes(&mut self, ctx: &egui::Context) {
        let interval = std::time::Duration::from_millis(1200);
        if let Some(rx) = &self.git_refresh_rx {
            if let Ok(result) = rx.try_recv() {
                self.git_refresh_rx = None;
                if let Ok(snapshot) = result {
                    self.apply_git_snapshot(snapshot);
                }
            }
        }

        if self.git_watch_at.elapsed() < interval {
            ctx.request_repaint_after(interval);
            return;
        }
        self.git_watch_at = std::time::Instant::now();

        if self.git_refresh_rx.is_none() {
            if let Some(root) = self.root.clone() {
                let (tx, rx) = std::sync::mpsc::channel();
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    let result = gitmodel::snapshot(&root).map_err(|e| e.to_string());
                    let _ = tx.send(result);
                    ctx.request_repaint();
                });
                self.git_refresh_rx = Some(rx);
            }
        }
        ctx.request_repaint_after(interval);
    }

    fn apply_git_snapshot(&mut self, snapshot: gitmodel::RepoSnapshot) {
        if self.git_snapshot_token.as_deref() == Some(snapshot.token.as_str()) {
            return;
        }

        self.git_snapshot_token = Some(snapshot.token);
        self.local_changes = snapshot.local_changes;
        self.selected_commit_paths = self.local_changes.iter().map(|c| c.path.clone()).collect();
        self.commits = snapshot.commits;
        self.git_status = format!(
            "{} local changes · {} commits",
            self.local_changes.len(),
            self.commits.len()
        );
        self.selected_local_change = None;
        self.selected_commit = None;
        self.selected_change = None;
        self.changes.clear();
        self.diff_lines.clear();
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

/// A repo-level "Push" button (`git push origin <branch>`) plus its last
/// status line. Returns true when clicked. Disabled while a push is running or
/// when there is no current branch.
fn push_toolbar(ui: &mut egui::Ui, branch: Option<&str>, pushing: bool, status: &str) -> bool {
    let font_size = ui_font_size(ui);
    ui.add_space(2.0);
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        let label = match branch {
            Some(b) if pushing => format!("Pushing origin/{b}…"),
            Some(b) => format!("↑ Push  origin/{b}"),
            None => "↑ Push".to_string(),
        };
        let enabled = branch.is_some() && !pushing;
        let width = (ui.available_width() - 28.0).max(80.0);
        let button = egui::Button::new(
            RichText::new(label)
                .color(theme::TEXT)
                .size(font_size * 0.85),
        )
        .fill(if enabled {
            theme::SIDEBAR_SELECTED
        } else {
            theme::RAISED
        });
        clicked = ui
            .add_enabled(enabled, button.min_size(Vec2::new(width, 30.0)))
            .clicked();
    });
    if !status.is_empty() {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            let color = if status.starts_with("push failed") {
                theme::RED
            } else {
                theme::TEXT_DIM
            };
            ui.label(
                RichText::new(ellipsize(
                    ui,
                    status,
                    FontId::proportional(font_size * 0.74),
                    (ui.available_width() - 14.0).max(40.0),
                ))
                .color(color)
                .size(font_size * 0.74),
            );
        });
    }
    ui.add_space(8.0);
    clicked
}

/// A circled "?" help icon. Returns true when clicked.
fn help_icon(ui: &mut egui::Ui) -> bool {
    let d = 12.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(d), Sense::click());
    let resp = resp
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Keyboard shortcuts");
    let color = if resp.hovered() {
        theme::TEXT
    } else {
        theme::TEXT_DIM
    };
    let center = rect.center();
    ui.painter()
        .circle_stroke(center, d * 0.5 - 1.5, Stroke::new(1.3, color));
    ui.painter().text(
        center,
        Align2::CENTER_CENTER,
        "?",
        FontId::proportional(d * 0.64),
        color,
    );
    resp.clicked()
}

/// One titled block of `(keys, description)` rows in the shortcuts modal.
fn shortcut_group(ui: &mut egui::Ui, title: &str, rows: &[(&str, &str)]) {
    let fs = ui_font_size(ui);
    ui.add_space(10.0);
    ui.label(
        RichText::new(title)
            .strong()
            .color(theme::ACCENT)
            .size(fs * 0.82),
    );
    ui.add_space(4.0);
    egui::Grid::new(("shortcuts", title))
        .num_columns(2)
        .spacing([24.0, 6.0])
        .show(ui, |ui| {
            for (keys, desc) in rows {
                ui.label(RichText::new(*keys).monospace().color(theme::TEXT));
                ui.label(RichText::new(*desc).color(theme::TEXT_DIM));
                ui.end_row();
            }
        });
}

/// The top-right Editor / Diff / Agent switch, drawn as a rounded segmented
/// control (like the Changed/Commits tabs) rather than separate buttons.
/// Returns the newly chosen mode when a segment is clicked.
fn mode_switch(ui: &mut egui::Ui, current: Mode) -> Option<Mode> {
    let segments = [
        (Mode::Agent, "Agent"),
        (Mode::FileBrowser, "Editor"),
        (Mode::GitDiff, "Diff"),
    ];
    let font = FontId::proportional(ui_font_size(ui) * 0.82);
    let seg_w = 60.0;
    let height = 24.0;
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(seg_w * segments.len() as f32, height),
        Sense::hover(),
    );
    ui.painter().rect_filled(rect, 6.0, theme::INSET);
    ui.painter()
        .rect_stroke(rect, 6.0, Stroke::new(1.0, theme::BORDER));

    let mut clicked = None;
    for (i, (mode, label)) in segments.iter().enumerate() {
        let seg = Rect::from_min_size(
            Pos2::new(rect.left() + seg_w * i as f32, rect.top()),
            Vec2::new(seg_w, height),
        );
        let resp = ui.interact(seg, ui.id().with(("mode_seg", i)), Sense::click());
        let active = current == *mode;
        if active {
            ui.painter()
                .rect_filled(seg.shrink(3.0), 5.0, theme::SIDEBAR_SELECTED);
        } else if resp.hovered() {
            ui.painter()
                .rect_filled(seg.shrink(3.0), 5.0, theme::SIDEBAR_HOVER);
        }
        ui.painter().text(
            seg.center(),
            Align2::CENTER_CENTER,
            *label,
            font.clone(),
            if active { theme::TEXT } else { theme::TEXT_DIM },
        );
        if resp.clicked() {
            clicked = Some(*mode);
        }
    }
    clicked
}

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

fn commit_panel(ui: &mut egui::Ui, app: &mut CodezApp) {
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
        // Fixed-height box: a multiline message can be any length, but it must
        // scroll inside this 72px frame rather than growing the panel and
        // pushing the Commit button off-screen.
        egui::ScrollArea::vertical()
            .id_salt("commit_message")
            .max_height(72.0)
            .max_width(width)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut app.commit_summary)
                        .hint_text(RichText::new("Message").color(theme::TEXT_MUTED))
                        .desired_width(width)
                        .desired_rows(3),
                );
            });
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
    // Breathing room below the button so it doesn't sit flush against the edge.
    ui.add_space(14.0);
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

fn session_row(
    ui: &mut egui::Ui,
    session: &agent::AgentSession,
    selected: bool,
    font_size: f32,
) -> egui::Response {
    let row_h = 52.0;
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), Sense::click());
    if let Some(fill) = row_bg(selected, response.hovered()) {
        ui.painter()
            .rect_filled(rect.shrink2(Vec2::new(8.0, 3.0)), 6.0, fill);
    }

    // Kind badge (e.g. "codex" / "claude").
    let badge_color = agent_kind_color(session.kind);
    let badge_font = FontId::monospace(font_size * 0.66);
    let label = session.kind.label();
    let badge_w = text_width(ui, label, badge_font.clone()) + 12.0;
    let badge = Rect::from_min_size(
        Pos2::new(rect.left() + 14.0, rect.top() + 9.0),
        Vec2::new(badge_w, 16.0),
    );
    ui.painter()
        .rect_filled(badge, 4.0, badge_color.linear_multiply(0.22));
    ui.painter().text(
        badge.center(),
        Align2::CENTER_CENTER,
        label,
        badge_font,
        badge_color,
    );
    // Start time, to the right of the badge.
    ui.painter().text(
        Pos2::new(badge.right() + 8.0, badge.center().y),
        Align2::LEFT_CENTER,
        agent::format_started(&session.started),
        FontId::proportional(font_size * 0.72),
        theme::TEXT_MUTED,
    );

    // Second line: the opening prompt if known, otherwise the short id.
    let short_id: String = session.id.chars().take(8).collect();
    let second = if session.summary.is_empty() {
        short_id
    } else {
        session.summary.clone()
    };
    let second_font = FontId::proportional(font_size * 0.85);
    let avail = (rect.right() - rect.left() - 28.0).max(20.0);
    ui.painter().text(
        Pos2::new(rect.left() + 14.0, rect.top() + 30.0),
        Align2::LEFT_TOP,
        ellipsize(ui, &second, second_font.clone(), avail),
        second_font,
        if selected {
            theme::TEXT
        } else {
            theme::TEXT_DIM
        },
    );
    response.on_hover_text(&session.id)
}

fn agent_kind_color(kind: agent::AgentKind) -> Color32 {
    match kind {
        agent::AgentKind::Codex => theme::GREEN,
        agent::AgentKind::ClaudeCode => theme::ACCENT,
    }
}

/// Bottom-of-sidebar usage readout: a session/week bar pair per agent.
fn usage_panel(ui: &mut egui::Ui, usage: &crate::usage::Usage) {
    let fs = ui_font_size(ui);
    ui.add_space(6.0);
    ui.separator();
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.label(
            RichText::new("USAGE")
                .color(theme::TEXT_MUTED)
                .size(fs * 0.72)
                .strong(),
        );
    });
    ui.add_space(4.0);
    // Only agents that are actually present (data available) are shown.
    if let Some(l) = &usage.codex {
        usage_agent_row(ui, "codex", theme::GREEN, l, fs);
    }
    if let Some(l) = &usage.claude {
        usage_agent_row(ui, "claude", theme::ACCENT, l, fs);
    }
    ui.add_space(8.0);
}

fn usage_agent_row(
    ui: &mut egui::Ui,
    name: &str,
    color: Color32,
    l: &crate::usage::Limits,
    fs: f32,
) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.label(RichText::new(name).color(color).size(fs * 0.78).strong());
    });
    usage_bar(ui, "session", l.session, fs);
    if let Some(reset) = &l.session_reset {
        usage_reset_time(ui, reset, fs);
    }
    usage_bar(ui, "week", l.week, fs);
}

fn usage_reset_time(ui: &mut egui::Ui, reset: &crate::usage::ResetTime, fs: f32) {
    ui.horizontal(|ui| {
        ui.add_space(18.0);
        ui.add_sized(Vec2::new(46.0, fs * 0.85), egui::Label::new(""));
        ui.label(
            RichText::new(format!("resets {}", format_reset_time(reset)))
                .color(theme::TEXT_MUTED)
                .size(fs * 0.66),
        );
    });
}

fn format_reset_time(reset: &crate::usage::ResetTime) -> String {
    use chrono::{Local, TimeZone};

    match reset {
        crate::usage::ResetTime::Label(label) => chrono::DateTime::parse_from_rfc3339(label)
            .map(|dt| format_local_reset_time(dt.with_timezone(&Local)))
            .unwrap_or_else(|_| label.clone()),
        crate::usage::ResetTime::EpochSeconds(reset_at) => {
            let Some(reset) = Local.timestamp_opt(*reset_at as i64, 0).single() else {
                return "--".to_owned();
            };
            format_local_reset_time(reset)
        }
    }
}

fn format_local_reset_time(reset: chrono::DateTime<chrono::Local>) -> String {
    let now: chrono::DateTime<chrono::Local> = chrono::Local::now();
    if reset.date_naive() == now.date_naive() {
        reset.format("%H:%M").to_string()
    } else {
        reset.format("%b %-d %H:%M").to_string()
    }
}

fn usage_bar(ui: &mut egui::Ui, label: &str, pct: f32, fs: f32) {
    ui.horizontal(|ui| {
        ui.add_space(18.0);
        ui.add_sized(
            Vec2::new(46.0, fs),
            egui::Label::new(RichText::new(label).color(theme::TEXT_DIM).size(fs * 0.7)),
        );
        // Reserve room on the right for the percentage text.
        let bar_w = (ui.available_width() - 40.0).max(30.0);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(bar_w, 6.0), Sense::hover());
        ui.painter().rect_filled(rect, 3.0, theme::INSET);
        let frac = (pct / 100.0).clamp(0.0, 1.0);
        let color = if pct >= 90.0 {
            theme::RED
        } else if pct >= 70.0 {
            theme::YELLOW
        } else {
            theme::GREEN
        };
        ui.painter().rect_filled(
            Rect::from_min_size(rect.min, Vec2::new(rect.width() * frac, rect.height())),
            3.0,
            color,
        );
        ui.label(
            RichText::new(format!("{pct:.0}%"))
                .color(theme::TEXT_DIM)
                .size(fs * 0.7),
        );
    });
}

/// What the user did on the terminal tab strip this frame.
enum TabAction {
    None,
    Switch(usize),
    Close(usize),
}

/// iTerm2-style tab strip above the active terminal. The active tab is filled
/// with the terminal background so it merges with the content below, marked by
/// a thin accent bar on top; inactive tabs sit on a darker strip with hairline
/// separators. Each tab has a close × that lights up on hover. (New terminals
/// are opened from the sidebar button, so there is no + here.)
fn terminal_tab_bar(
    ui: &mut egui::Ui,
    tabs: &[TermTab],
    active: Option<usize>,
    font_size: f32,
) -> TabAction {
    let mut action = TabAction::None;
    let strip_h = 32.0;
    let full = ui.available_rect_before_wrap();
    let (strip, _) = ui.allocate_exact_size(Vec2::new(full.width(), strip_h), Sense::hover());

    ui.painter().rect_filled(strip, 0.0, theme::SURFACE);
    ui.painter().line_segment(
        [strip.left_bottom(), strip.right_bottom()],
        Stroke::new(1.0, theme::BORDER),
    );

    let n = tabs.len().max(1);
    // Shrink to fit so tabs never overflow the strip; cap the width like iTerm2.
    let tab_w = (strip.width() / n as f32).min(220.0);
    let title_font = FontId::proportional(font_size * 0.82);

    for (i, tab) in tabs.iter().enumerate() {
        let x0 = strip.left() + tab_w * i as f32;
        let tab_rect = Rect::from_min_size(Pos2::new(x0, strip.top()), Vec2::new(tab_w, strip_h));
        let is_active = active == Some(i);

        let tab_resp = ui.interact(tab_rect, ui.id().with(("term_tab", i)), Sense::click());
        let close_rect = Rect::from_center_size(
            Pos2::new(tab_rect.right() - 13.0, tab_rect.center().y),
            Vec2::splat(16.0),
        );
        let close_resp = ui.interact(close_rect, ui.id().with(("term_close", i)), Sense::click());

        let fill = if is_active {
            theme::INSET
        } else if tab_resp.hovered() {
            theme::SIDEBAR_HOVER
        } else {
            theme::SURFACE
        };
        let fg = if tab.terminal.exited() {
            theme::TEXT_MUTED
        } else if is_active {
            theme::TEXT
        } else {
            theme::TEXT_DIM
        };

        let painter = ui.painter();
        painter.rect_filled(tab_rect, 0.0, fill);
        if is_active {
            // Accent bar on top, and hide the strip's bottom border under the
            // active tab so it visually connects to the terminal below.
            painter.rect_filled(
                Rect::from_min_size(tab_rect.left_top(), Vec2::new(tab_w, 2.0)),
                0.0,
                theme::ACCENT,
            );
            painter.line_segment(
                [tab_rect.left_bottom(), tab_rect.right_bottom()],
                Stroke::new(1.0, theme::INSET),
            );
        }
        // Hairline separator on the right edge (skip after the active tab).
        if !is_active {
            painter.line_segment(
                [
                    Pos2::new(tab_rect.right(), tab_rect.top() + 6.0),
                    Pos2::new(tab_rect.right(), tab_rect.bottom() - 6.0),
                ],
                Stroke::new(1.0, theme::BORDER),
            );
        }

        let text_left = tab_rect.left() + 12.0;
        let text_max = (close_rect.left() - text_left - 4.0).max(10.0);
        let title = ellipsize(ui, &tab.title, title_font.clone(), text_max);
        ui.painter().text(
            Pos2::new(text_left, tab_rect.center().y),
            Align2::LEFT_CENTER,
            title,
            title_font.clone(),
            fg,
        );

        if close_resp.hovered() {
            ui.painter().rect_filled(close_rect, 3.0, theme::RAISED);
        }
        ui.painter().text(
            close_rect.center(),
            Align2::CENTER_CENTER,
            "×",
            FontId::proportional(font_size * 0.95),
            if close_resp.hovered() {
                theme::TEXT
            } else {
                fg
            },
        );

        if close_resp.clicked() {
            action = TabAction::Close(i);
        } else if tab_resp.clicked() {
            action = TabAction::Switch(i);
        }
    }

    action
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
fn file_header(ui: &mut egui::Ui, path: &Path, status: &str, dirty: bool) {
    let font_size = ui_font_size(ui);
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        // VS Code-style unsaved dot before the filename.
        if dirty {
            ui.label(
                RichText::new("●")
                    .color(theme::YELLOW)
                    .size(font_size * 0.7),
            )
            .on_hover_text("Unsaved changes");
        }
        ui.label(
            RichText::new(path.to_string_lossy())
                .monospace()
                .color(theme::TEXT),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            // Explicit "Unsaved" badge on the right, plus the line/char stats.
            if dirty {
                ui.label(
                    RichText::new("● Unsaved")
                        .color(theme::YELLOW)
                        .size(font_size * 0.85)
                        .strong(),
                );
            }
            if !status.is_empty() {
                ui.label(
                    RichText::new(status)
                        .color(theme::TEXT_DIM)
                        .size(font_size * 0.85),
                );
            }
        });
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

// ---------------- file-tree helpers ----------------

fn already_exists() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::AlreadyExists, "already exists")
}

fn file_mtime(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
}

fn file_name_string(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Build a non-existing sibling path by appending " copy" (then " copy 2", …),
/// preserving the extension for files.
fn unique_sibling(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let is_dir = path.is_dir();
    let (stem, ext) = if is_dir {
        (file_name_string(path), None)
    } else {
        (
            path.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            path.extension().map(|e| e.to_string_lossy().into_owned()),
        )
    };
    let build = |label: &str| -> PathBuf {
        let name = match &ext {
            Some(e) => format!("{stem} {label}.{e}"),
            None => format!("{stem} {label}"),
        };
        parent.join(name)
    };
    let mut candidate = build("copy");
    let mut i = 2;
    while candidate.exists() {
        candidate = build(&format!("copy {i}"));
        i += 1;
    }
    candidate
}

/// Recursively copy a file or directory to `dest`.
fn copy_into(src: &Path, dest: &Path) -> std::io::Result<()> {
    if src.is_dir() {
        std::fs::create_dir_all(dest)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            copy_into(&entry.path(), &dest.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(src, dest).map(|_| ())
    }
}
