//! An interactive PTY-backed terminal pane.
//!
//! The heavy lifting — spawning a PTY, the child process, VT/ANSI parsing and
//! the grid/scrollback state — is all done by `alacritty_terminal`. Its `tty`
//! plus `event_loop` run a reader thread that feeds bytes through the VT parser
//! into a shared [`Term`] behind a `FairMutex`. This module only adds the three
//! things alacritty leaves to the embedder: launching the process, painting the
//! grid with egui's `Painter`, and translating egui key/text events back into
//! the byte sequences a terminal expects.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::path::PathBuf;

use alacritty_terminal::event::{Event, EventListener, Notify, OnResize, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Msg, Notifier};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Direction, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::search::{Match, RegexIter, RegexSearch};
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{self, viewport_to_point, Term, TermMode};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::{Color, NamedColor};

use egui::{
    Align2, Color32, CursorIcon, EventFilter, FontId, Key, Modifiers, Painter, Pos2, Rect, Sense,
    Stroke, TextStyle, Ui, Vec2,
};

use crate::theme;

/// Forwards alacritty's parser events out of the reader thread.
#[derive(Clone)]
struct EventProxy(mpsc::Sender<Event>);

impl EventListener for EventProxy {
    fn send_event(&self, event: Event) {
        let _ = self.0.send(event);
    }
}

/// A grid geometry that satisfies alacritty's `Dimensions` trait.
#[derive(Clone, Copy)]
struct Grid {
    cols: u16,
    lines: u16,
}

impl Dimensions for Grid {
    fn total_lines(&self) -> usize {
        self.lines as usize
    }
    fn screen_lines(&self) -> usize {
        self.lines as usize
    }
    fn columns(&self) -> usize {
        self.cols as usize
    }
    fn last_column(&self) -> Column {
        Column(self.cols.max(1) as usize - 1)
    }
    fn bottommost_line(&self) -> Line {
        Line(self.lines as i32 - 1)
    }
}

pub struct Terminal {
    term: Arc<FairMutex<Term<EventProxy>>>,
    notifier: Notifier,
    exited: Arc<AtomicBool>,
    cols: u16,
    lines: u16,
    cell: Vec2,
    needs_focus: bool,
    /// Sub-line trackpad scroll remainder (pixels), accumulated across frames.
    scroll_accum: f32,
    /// Regex used to find clickable URLs in the grid.
    url_regex: RegexSearch,
    /// Link under the pointer this frame while ⌘ is held (for underline + open).
    hovered_link: Option<Match>,
}

impl Terminal {
    /// Spawn a login shell in `working_dir`. If `command` is given it is typed
    /// into the shell as the first line (e.g. `codex resume <id>`), so the child
    /// inherits the user's full PATH from their shell profile.
    pub fn spawn(
        ctx: &egui::Context,
        working_dir: PathBuf,
        command: Option<String>,
    ) -> std::io::Result<Self> {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);

        // Provisional geometry; the first `ui()` call resizes to fit the pane.
        let (cols, lines) = (80u16, 24u16);
        let (cell_w, cell_h) = (8u16, 16u16);
        let window = WindowSize {
            num_cols: cols,
            num_lines: lines,
            cell_width: cell_w,
            cell_height: cell_h,
        };

        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
        let pty_options = tty::Options {
            shell: Some(tty::Shell::new(shell, vec!["-l".into(), "-i".into()])),
            working_directory: Some(working_dir),
            ..tty::Options::default()
        };
        let pty = tty::new(&pty_options, window, id)?;

        let (event_tx, event_rx) = mpsc::channel();
        let proxy = EventProxy(event_tx);
        let term = Term::new(term::Config::default(), &Grid { cols, lines }, proxy.clone());
        let term = Arc::new(FairMutex::new(term));

        let event_loop = EventLoop::new(term.clone(), proxy, pty, false, false)?;
        let notifier = Notifier(event_loop.channel());
        let pty_notifier = Notifier(event_loop.channel());
        let _ = event_loop.spawn();

        // Drain parser events: wake the UI on output, mirror PtyWrite replies
        // (e.g. cursor/device-status reports) back to the child, note exit.
        let exited = Arc::new(AtomicBool::new(false));
        let exited_thread = exited.clone();
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name(format!("codez-pty-{id}"))
            .spawn(move || {
                while let Ok(event) = event_rx.recv() {
                    ctx.request_repaint();
                    match event {
                        Event::Exit => {
                            exited_thread.store(true, Ordering::Relaxed);
                            ctx.request_repaint();
                            break;
                        }
                        Event::PtyWrite(text) => pty_notifier.notify(text.into_bytes()),
                        _ => {}
                    }
                }
            })?;

        if let Some(cmd) = command {
            let mut line = cmd.into_bytes();
            line.push(b'\r');
            notifier.notify(line);
        }

        Ok(Self {
            term,
            notifier,
            exited,
            cols,
            lines,
            cell: Vec2::new(cell_w as f32, cell_h as f32),
            needs_focus: true,
            scroll_accum: 0.0,
            url_regex: RegexSearch::new(
                r#"(ipfs:|ipns:|magnet:|mailto:|gemini://|gopher://|https://|http://|news:|file://|git://|ssh:|ftp://)[^\u{0000}-\u{001F}\u{007F}-\u{009F}<>"\s{-}\^⟨⟩`]+"#,
            )
            .expect("valid url regex"),
            hovered_link: None,
        })
    }

    /// True once the child process has exited.
    pub fn exited(&self) -> bool {
        self.exited.load(Ordering::Relaxed)
    }

    /// Grab keyboard focus on the next frame (e.g. when its tab is activated).
    pub fn request_focus(&mut self) {
        self.needs_focus = true;
    }

    /// Render the terminal into the remaining space of `ui` and, while focused,
    /// forward keyboard input to the PTY.
    pub fn ui(&mut self, ui: &mut Ui) {
        let font = FontId::monospace(TextStyle::Monospace.resolve(ui.style()).size);
        let cell = ui.fonts(|f| Vec2::new(f.glyph_width(&font, 'M'), f.row_height(&font)));

        let (response, painter) =
            ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let rect = response.rect;

        self.resize(rect.size(), cell);

        // Mouse-wheel / trackpad scrolling through the scrollback works whenever
        // the pointer is over the pane, even without keyboard focus.
        if response.hovered() || response.has_focus() {
            self.process_scroll(ui);
        }

        // Text selection (drag) and ⌘-click link opening.
        self.handle_mouse(ui, &response, rect, cell);

        if self.needs_focus || response.clicked() {
            response.request_focus();
            self.needs_focus = false;
        }
        if response.has_focus() {
            // Keep Tab / arrows / Esc from triggering egui focus navigation so
            // they reach the child program instead.
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    response.id,
                    EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: true,
                    },
                )
            });
            self.process_input(ui);
            // Tell the platform to enable IME and anchor the candidate window at
            // the terminal cursor, so CJK input methods work and pop up in the
            // right place.
            let cursor_rect = self.cursor_rect(rect, cell);
            ui.ctx().output_mut(|o| {
                o.ime = Some(egui::output::IMEOutput {
                    rect: cursor_rect,
                    cursor_rect,
                })
            });
        }

        self.paint(&painter, rect, cell, &font);
    }

    fn resize(&mut self, px: Vec2, cell: Vec2) {
        let cols = (px.x / cell.x).floor().max(1.0) as u16;
        let lines = (px.y / cell.y).floor().max(1.0) as u16;
        if cols == self.cols && lines == self.lines && cell == self.cell {
            return;
        }
        self.cols = cols;
        self.lines = lines;
        self.cell = cell;
        self.notifier.on_resize(WindowSize {
            num_cols: cols,
            num_lines: lines,
            cell_width: cell.x as u16,
            cell_height: cell.y as u16,
        });
        self.term
            .lock()
            .resize(TermSize::new(cols as usize, lines as usize));
    }

    /// Screen rectangle of the current text cursor cell (for IME placement).
    fn cursor_rect(&self, rect: Rect, cell: Vec2) -> Rect {
        let term = self.term.lock();
        let grid = term.grid();
        let point = grid.cursor.point;
        let line = (point.line.0 + grid.display_offset() as i32) as f32;
        let x = rect.min.x + cell.x * point.column.0 as f32;
        let y = rect.min.y + cell.y * line;
        Rect::from_min_size(Pos2::new(x, y), cell)
    }

    /// Translate wheel/trackpad events into scrollback movement.
    fn process_scroll(&mut self, ui: &Ui) {
        let mut lines = 0i32;
        for event in ui.input(|i| i.events.clone()) {
            if let egui::Event::MouseWheel { unit, delta, .. } = event {
                match unit {
                    egui::MouseWheelUnit::Line => {
                        lines += (delta.y.signum() * delta.y.abs().ceil()) as i32;
                    }
                    egui::MouseWheelUnit::Point => {
                        self.scroll_accum -= delta.y;
                        let whole = (self.scroll_accum / self.cell.y).trunc();
                        self.scroll_accum %= self.cell.y;
                        lines -= whole as i32;
                    }
                    egui::MouseWheelUnit::Page => {}
                }
            }
        }
        if lines != 0 {
            self.scroll(lines);
        }
    }

    /// Scroll the display by `lines` (positive = toward older output). In a
    /// full-screen app's alternate scroll mode there is no scrollback, so
    /// translate the wheel into arrow keys the way xterm does.
    fn scroll(&mut self, lines: i32) {
        let mut term = self.term.lock();
        if term
            .mode()
            .contains(TermMode::ALTERNATE_SCROLL | TermMode::ALT_SCREEN)
        {
            let arrow = if lines > 0 { b'A' } else { b'B' };
            let mut bytes = Vec::with_capacity(lines.unsigned_abs() as usize * 3);
            for _ in 0..lines.abs() {
                bytes.extend_from_slice(&[0x1b, b'O', arrow]);
            }
            drop(term);
            self.notifier.notify(bytes);
        } else {
            term.grid_mut().scroll_display(Scroll::Delta(lines));
        }
    }

    /// Drag-to-select text, double/triple-click word/line selection, and
    /// ⌘-hover/⌘-click for opening URLs.
    fn handle_mouse(&mut self, ui: &Ui, resp: &egui::Response, rect: Rect, cell: Vec2) {
        let cmd = ui.input(|i| i.modifiers.mac_cmd || i.modifiers.command);

        // ⌘ + hover: highlight the link under the pointer.
        self.hovered_link = None;
        if cmd {
            if let Some(pos) = resp.hover_pos() {
                if let Some(m) = self.link_at(self.point_at(pos, rect, cell)) {
                    ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
                    self.hovered_link = Some(m);
                }
            }
        }

        if resp.triple_clicked() {
            self.begin_selection(SelectionType::Lines, resp, rect, cell);
        } else if resp.double_clicked() {
            self.begin_selection(SelectionType::Semantic, resp, rect, cell);
        } else if resp.drag_started() {
            self.begin_selection(SelectionType::Simple, resp, rect, cell);
        } else if resp.dragged() {
            if let Some(pos) = resp.interact_pointer_pos() {
                let (point, side) = (self.point_at(pos, rect, cell), self.side_at(pos, cell));
                let mut term = self.term.lock();
                if let Some(sel) = term.selection.as_mut() {
                    sel.update(point, side);
                }
            }
        } else if resp.clicked() {
            match (cmd, self.hovered_link.clone()) {
                (true, Some(link)) => self.open_link(&link),
                _ => self.term.lock().selection = None,
            }
        }
    }

    fn begin_selection(
        &self,
        ty: SelectionType,
        resp: &egui::Response,
        rect: Rect,
        cell: Vec2,
    ) {
        if let Some(pos) = resp.interact_pointer_pos() {
            let point = self.point_at(pos, rect, cell);
            let side = self.side_at(pos, cell);
            self.term.lock().selection = Some(Selection::new(ty, point, side));
        }
    }

    /// Map a screen position to a grid point, accounting for scrollback offset.
    fn point_at(&self, pos: Pos2, rect: Rect, cell: Vec2) -> Point {
        let col = (((pos.x - rect.min.x) / cell.x) as i32).clamp(0, self.cols as i32 - 1) as usize;
        let line = (((pos.y - rect.min.y) / cell.y) as i32).clamp(0, self.lines as i32 - 1) as usize;
        let offset = self.term.lock().grid().display_offset();
        viewport_to_point(offset, Point::new(line, Column(col)))
    }

    fn side_at(&self, pos: Pos2, cell: Vec2) -> Side {
        let within = (pos.x).rem_euclid(cell.x);
        if within > cell.x / 2.0 {
            Side::Right
        } else {
            Side::Left
        }
    }

    /// Find a URL match at `point`, scanning the visible viewport (± a margin).
    fn link_at(&self, point: Point) -> Option<Match> {
        let mut regex = self.url_regex.clone();
        let term = self.term.lock();
        let viewport_start = Line(-(term.grid().display_offset() as i32));
        let viewport_end = viewport_start + term.bottommost_line();
        let mut start = term.line_search_left(Point::new(viewport_start, Column(0)));
        let mut end = term.line_search_right(Point::new(viewport_end, Column(0)));
        start.line = start.line.max(viewport_start - 100);
        end.line = end.line.min(viewport_end + 100);
        RegexIter::new(start, end, Direction::Right, &term, &mut regex)
            .skip_while(|m| m.end().line < viewport_start)
            .take_while(|m| m.start().line <= viewport_end)
            .find(|m| m.contains(&point))
    }

    fn open_link(&self, m: &Match) {
        let url = {
            let term = self.term.lock();
            let grid = term.grid();
            let start = *m.start();
            let mut s = String::from(grid[start].c);
            for indexed in grid.iter_from(start) {
                s.push(indexed.c);
                if indexed.point == *m.end() {
                    break;
                }
            }
            s
        };
        let url = url.trim().to_string();
        #[cfg(target_os = "macos")]
        let _ = std::process::Command::new("open").arg(&url).spawn();
        #[cfg(not(target_os = "macos"))]
        let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
    }

    fn process_input(&self, ui: &Ui) {
        let (app_cursor, bracketed_paste) = {
            let mode = *self.term.lock().mode();
            (
                mode.contains(TermMode::APP_CURSOR),
                mode.contains(TermMode::BRACKETED_PASTE),
            )
        };
        let mut out: Vec<u8> = Vec::new();
        for event in ui.input(|i| i.events.clone()) {
            match event {
                // Printable text. Skip control chars — those arrive as Key
                // events and we encode them ourselves (avoids double-sending
                // Enter/Tab on platforms that emit both).
                egui::Event::Text(t) => {
                    if t.chars().all(|c| !c.is_control()) {
                        out.extend_from_slice(t.as_bytes());
                    }
                }
                // Clipboard paste: send verbatim (newlines and all). When the
                // program enabled bracketed paste, wrap it so multi-line pastes
                // arrive as one chunk instead of being run line by line.
                egui::Event::Paste(t) => {
                    if bracketed_paste {
                        out.extend_from_slice(b"\x1b[200~");
                        out.extend_from_slice(t.as_bytes());
                        out.extend_from_slice(b"\x1b[201~");
                    } else {
                        out.extend_from_slice(t.as_bytes());
                    }
                }
                // Committed IME composition (e.g. Chinese/Japanese input). The
                // in-progress preedit arrives as `Preedit` and is ignored — the
                // OS draws the candidate window itself.
                egui::Event::Ime(egui::ImeEvent::Commit(t)) => {
                    out.extend_from_slice(t.as_bytes());
                }
                // ⌘C (mac) / Ctrl+Shift+C copies the current selection rather
                // than sending a byte to the child.
                egui::Event::Copy => {
                    if let Some(text) = self.term.lock().selection_to_string() {
                        if !text.is_empty() {
                            ui.ctx().copy_text(text);
                        }
                    }
                }
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    if let Some(bytes) = key_to_bytes(key, modifiers, app_cursor) {
                        out.extend_from_slice(&bytes);
                    }
                }
                _ => {}
            }
        }
        if !out.is_empty() {
            self.notifier.notify(out);
            let mut term = self.term.lock();
            term.scroll_display(Scroll::Bottom);
            term.selection = None; // typing dismisses the selection
        }
    }

    fn paint(&self, painter: &Painter, rect: Rect, cell: Vec2, font: &FontId) {
        painter.rect_filled(rect, 0.0, theme::INSET);

        let term = self.term.lock();
        let selection = term.selection.as_ref().and_then(|s| s.to_range(&term));
        let grid = term.grid();
        let display_offset = grid.display_offset() as i32;
        let cursor = grid.cursor.point;
        let global_bg = theme::INSET;

        for indexed in grid.display_iter() {
            let flags = indexed.cell.flags;
            if flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            let col = indexed.point.column.0 as f32;
            let line = (indexed.point.line.0 + display_offset) as f32;
            let x = rect.min.x + cell.x * col;
            let y = rect.min.y + cell.y * line;
            let cw = if flags.contains(Flags::WIDE_CHAR) {
                cell.x * 2.0
            } else {
                cell.x
            };

            let mut fg = ansi_color(indexed.fg);
            let mut bg = ansi_color(indexed.bg);
            if flags.intersects(Flags::DIM | Flags::DIM_BOLD) {
                fg = fg.linear_multiply(0.7);
            }
            let is_selected = selection.as_ref().is_some_and(|r| r.contains(indexed.point));
            if flags.contains(Flags::INVERSE) || is_selected {
                std::mem::swap(&mut fg, &mut bg);
            }
            let is_link = self
                .hovered_link
                .as_ref()
                .is_some_and(|m| m.contains(&indexed.point));

            let is_cursor = indexed.point == cursor;

            if bg != global_bg {
                painter.rect_filled(
                    Rect::from_min_size(Pos2::new(x, y), Vec2::new(cw + 1.0, cell.y + 1.0)),
                    0.0,
                    bg,
                );
            }
            if is_cursor {
                painter.rect_filled(
                    Rect::from_min_size(Pos2::new(x, y), Vec2::new(cw, cell.y)),
                    0.0,
                    theme::TEXT,
                );
            }

            let ch = indexed.c;
            if is_link {
                fg = theme::ACCENT;
                let uy = y + cell.y - 1.0;
                painter.line_segment(
                    [Pos2::new(x, uy), Pos2::new(x + cw, uy)],
                    Stroke::new(1.0, theme::ACCENT),
                );
            }
            if ch != ' ' && ch != '\t' {
                painter.text(
                    Pos2::new(x + cw / 2.0, y),
                    Align2::CENTER_TOP,
                    ch,
                    font.clone(),
                    if is_cursor { global_bg } else { fg },
                );
            }
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.notifier.0.send(Msg::Shutdown);
    }
}

/// Translate a non-text key press into the bytes a terminal expects, honoring
/// DECCKM (application cursor) mode for the arrow/Home/End keys.
fn key_to_bytes(key: Key, m: Modifiers, app_cursor: bool) -> Option<Vec<u8>> {
    // Ctrl+<letter> control codes (^A = 0x01 … ^Z = 0x1a, plus a few symbols).
    if m.ctrl && !m.alt {
        if let Some(b) = ctrl_byte(key) {
            return Some(vec![b]);
        }
    }
    let bytes: &[u8] = match key {
        Key::Enter => b"\r",
        Key::Tab if m.shift => b"\x1b[Z".as_slice(),
        Key::Tab => b"\t",
        Key::Backspace => b"\x7f",
        Key::Escape => b"\x1b",
        Key::Insert => b"\x1b[2~".as_slice(),
        Key::Delete => b"\x1b[3~".as_slice(),
        Key::PageUp => b"\x1b[5~".as_slice(),
        Key::PageDown => b"\x1b[6~".as_slice(),
        Key::Home if app_cursor => b"\x1bOH".as_slice(),
        Key::Home => b"\x1b[H".as_slice(),
        Key::End if app_cursor => b"\x1bOF".as_slice(),
        Key::End => b"\x1b[F".as_slice(),
        Key::ArrowUp if app_cursor => b"\x1bOA".as_slice(),
        Key::ArrowUp => b"\x1b[A".as_slice(),
        Key::ArrowDown if app_cursor => b"\x1bOB".as_slice(),
        Key::ArrowDown => b"\x1b[B".as_slice(),
        Key::ArrowRight if app_cursor => b"\x1bOC".as_slice(),
        Key::ArrowRight => b"\x1b[C".as_slice(),
        Key::ArrowLeft if app_cursor => b"\x1bOD".as_slice(),
        Key::ArrowLeft => b"\x1b[D".as_slice(),
        _ => return None,
    };
    Some(bytes.to_vec())
}

fn ctrl_byte(key: Key) -> Option<u8> {
    let b = match key {
        Key::A => 0x01,
        Key::B => 0x02,
        Key::C => 0x03,
        Key::D => 0x04,
        Key::E => 0x05,
        Key::F => 0x06,
        Key::G => 0x07,
        Key::H => 0x08,
        Key::I => 0x09,
        Key::J => 0x0a,
        Key::K => 0x0b,
        Key::L => 0x0c,
        Key::M => 0x0d,
        Key::N => 0x0e,
        Key::O => 0x0f,
        Key::P => 0x10,
        Key::Q => 0x11,
        Key::R => 0x12,
        Key::S => 0x13,
        Key::T => 0x14,
        Key::U => 0x15,
        Key::V => 0x16,
        Key::W => 0x17,
        Key::X => 0x18,
        Key::Y => 0x19,
        Key::Z => 0x1a,
        Key::OpenBracket => 0x1b,
        Key::Backslash => 0x1c,
        Key::CloseBracket => 0x1d,
        _ => return None,
    };
    Some(b)
}

// ---------------- color mapping ----------------

/// GitHub-dark-flavored 16-color ANSI palette (0..7 normal, 8..15 bright).
const ANSI16: [Color32; 16] = [
    Color32::from_rgb(0x16, 0x1b, 0x22),
    Color32::from_rgb(0xf8, 0x51, 0x49),
    Color32::from_rgb(0x3f, 0xb9, 0x50),
    Color32::from_rgb(0xd2, 0x99, 0x22),
    Color32::from_rgb(0x58, 0xa6, 0xff),
    Color32::from_rgb(0xbc, 0x8c, 0xff),
    Color32::from_rgb(0x39, 0xc5, 0xcf),
    Color32::from_rgb(0xc9, 0xd1, 0xd9),
    Color32::from_rgb(0x6e, 0x76, 0x81),
    Color32::from_rgb(0xff, 0x7b, 0x72),
    Color32::from_rgb(0x56, 0xd3, 0x64),
    Color32::from_rgb(0xe3, 0xb3, 0x41),
    Color32::from_rgb(0x79, 0xc0, 0xff),
    Color32::from_rgb(0xd2, 0xa8, 0xff),
    Color32::from_rgb(0x56, 0xd4, 0xdd),
    Color32::from_rgb(0xff, 0xff, 0xff),
];

fn ansi_color(c: Color) -> Color32 {
    match c {
        Color::Spec(rgb) => Color32::from_rgb(rgb.r, rgb.g, rgb.b),
        Color::Named(n) => named_color(n),
        Color::Indexed(i) => indexed_color(i),
    }
}

fn named_color(n: NamedColor) -> Color32 {
    use NamedColor::*;
    match n {
        Foreground | BrightForeground => theme::TEXT,
        Background => theme::INSET,
        DimForeground => theme::TEXT_DIM,
        Black | DimBlack => ANSI16[0],
        Red | DimRed => ANSI16[1],
        Green | DimGreen => ANSI16[2],
        Yellow | DimYellow => ANSI16[3],
        Blue | DimBlue => ANSI16[4],
        Magenta | DimMagenta => ANSI16[5],
        Cyan | DimCyan => ANSI16[6],
        White | DimWhite => ANSI16[7],
        BrightBlack => ANSI16[8],
        BrightRed => ANSI16[9],
        BrightGreen => ANSI16[10],
        BrightYellow => ANSI16[11],
        BrightBlue => ANSI16[12],
        BrightMagenta => ANSI16[13],
        BrightCyan => ANSI16[14],
        BrightWhite => ANSI16[15],
        _ => theme::TEXT,
    }
}

fn indexed_color(i: u8) -> Color32 {
    match i {
        0..=15 => ANSI16[i as usize],
        16..=231 => {
            let i = i - 16;
            let step = |v: u8| if v == 0 { 0 } else { v * 40 + 55 };
            Color32::from_rgb(step(i / 36), step((i % 36) / 6), step(i % 6))
        }
        _ => {
            let v = (i - 232) * 10 + 8;
            Color32::from_rgb(v, v, v)
        }
    }
}
