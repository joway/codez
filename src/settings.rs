//! Appearance settings + the settings window.
//!
//! Keeps the global UI font, editor/diff monospace font, and a small set of
//! configurable app-level shortcuts.

use egui::{Context, FontData, FontDefinitions, FontFamily, FontId, Key, Modifiers, TextStyle};

const CJK_FONTS: &[&str] = &[
    "/System/Library/Fonts/PingFang.ttc",
    "/System/Library/Fonts/STHeiti Light.ttc",
    "/System/Library/Fonts/Hiragino Sans GB.ttc",
];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum UiFont {
    BuiltIn,
    SfPro,
    Helvetica,
    NewYork,
}

impl UiFont {
    const ALL: [UiFont; 4] = [
        UiFont::SfPro,
        UiFont::Helvetica,
        UiFont::NewYork,
        UiFont::BuiltIn,
    ];

    fn label(self) -> &'static str {
        match self {
            UiFont::BuiltIn => "Built-in",
            UiFont::SfPro => "SF Pro",
            UiFont::Helvetica => "Helvetica Neue",
            UiFont::NewYork => "New York",
        }
    }

    fn path(self) -> Option<&'static str> {
        match self {
            UiFont::BuiltIn => None,
            UiFont::SfPro => Some("/System/Library/Fonts/SFNS.ttf"),
            UiFont::Helvetica => Some("/System/Library/Fonts/HelveticaNeue.ttc"),
            UiFont::NewYork => Some("/System/Library/Fonts/NewYork.ttf"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MonoFont {
    BuiltIn,
    SfMono,
    Menlo,
    Monaco,
}

impl MonoFont {
    const ALL: [MonoFont; 4] = [
        MonoFont::SfMono,
        MonoFont::Menlo,
        MonoFont::Monaco,
        MonoFont::BuiltIn,
    ];

    fn label(self) -> &'static str {
        match self {
            MonoFont::BuiltIn => "Built-in",
            MonoFont::SfMono => "SF Mono",
            MonoFont::Menlo => "Menlo",
            MonoFont::Monaco => "Monaco",
        }
    }

    fn path(self) -> Option<&'static str> {
        match self {
            MonoFont::BuiltIn => None,
            MonoFont::SfMono => Some("/System/Library/Fonts/SFNSMono.ttf"),
            MonoFont::Menlo => Some("/System/Library/Fonts/Menlo.ttc"),
            MonoFont::Monaco => Some("/System/Library/Fonts/Monaco.ttf"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ShortcutAction {
    EditorPage,
    DiffPage,
}

#[derive(Clone, Copy)]
pub struct KeyBinding {
    key: Key,
    modifiers: Modifiers,
}

impl KeyBinding {
    fn new(key: Key, modifiers: Modifiers) -> Self {
        Self { key, modifiers }
    }

    pub fn key(self) -> Key {
        self.key
    }

    pub fn modifiers(self) -> Modifiers {
        self.modifiers
    }

    fn label(self) -> String {
        let mut parts = Vec::new();
        if self.modifiers.command {
            parts.push("Cmd");
        }
        if self.modifiers.ctrl {
            parts.push("Ctrl");
        }
        if self.modifiers.alt {
            parts.push("Option");
        }
        if self.modifiers.shift {
            parts.push("Shift");
        }
        parts.push(key_label(self.key));
        parts.join("+")
    }
}

pub struct Settings {
    pub open: bool,
    ui_font_size: f32,
    editor_font_size: f32,
    ui_font: UiFont,
    mono: MonoFont,
    editor_shortcut: KeyBinding,
    diff_shortcut: KeyBinding,
    recording: Option<ShortcutAction>,
    dirty: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            open: false,
            ui_font_size: 16.0,
            editor_font_size: 16.0,
            ui_font: UiFont::SfPro,
            mono: MonoFont::SfMono,
            editor_shortcut: KeyBinding::new(Key::E, Modifiers::COMMAND),
            diff_shortcut: KeyBinding::new(Key::D, Modifiers::COMMAND),
            recording: None,
            dirty: true,
        }
    }
}

impl Settings {
    pub fn ui_font_size(&self) -> f32 {
        self.ui_font_size
    }

    pub fn editor_shortcut(&self) -> KeyBinding {
        self.editor_shortcut
    }

    pub fn diff_shortcut(&self) -> KeyBinding {
        self.diff_shortcut
    }

    pub fn is_recording_shortcut(&self) -> bool {
        self.recording.is_some()
    }

    /// Re-apply fonts and text sizes to the context when something changed.
    pub fn apply(&mut self, ctx: &Context) {
        if !self.dirty {
            return;
        }
        self.dirty = false;

        let mut fonts = FontDefinitions::default();

        if let Some(path) = self.ui_font.path() {
            if let Ok(bytes) = std::fs::read(path) {
                fonts
                    .font_data
                    .insert("user_ui".to_owned(), FontData::from_owned(bytes).into());
                fonts
                    .families
                    .entry(FontFamily::Proportional)
                    .or_default()
                    .insert(0, "user_ui".to_owned());
            }
        }

        if let Some(path) = self.mono.path() {
            if let Ok(bytes) = std::fs::read(path) {
                fonts
                    .font_data
                    .insert("user_mono".to_owned(), FontData::from_owned(bytes).into());
                fonts
                    .families
                    .entry(FontFamily::Monospace)
                    .or_default()
                    .insert(0, "user_mono".to_owned());
            }
        }

        for path in CJK_FONTS {
            if let Ok(bytes) = std::fs::read(path) {
                fonts
                    .font_data
                    .insert("cjk".to_owned(), FontData::from_owned(bytes).into());
                for fam in [FontFamily::Proportional, FontFamily::Monospace] {
                    fonts
                        .families
                        .entry(fam)
                        .or_default()
                        .push("cjk".to_owned());
                }
                break;
            }
        }

        ctx.set_fonts(fonts);

        let mut style = (*ctx.style()).clone();
        style.text_styles.insert(
            TextStyle::Small,
            FontId::proportional(self.ui_font_size * 0.82),
        );
        style
            .text_styles
            .insert(TextStyle::Body, FontId::proportional(self.ui_font_size));
        style.text_styles.insert(
            TextStyle::Button,
            FontId::proportional(self.ui_font_size * 0.95),
        );
        style.text_styles.insert(
            TextStyle::Heading,
            FontId::proportional(self.ui_font_size * 1.25),
        );
        style.text_styles.insert(
            TextStyle::Monospace,
            FontId::monospace(self.editor_font_size),
        );
        ctx.set_style(style);
    }

    /// Draw the settings window if open.
    pub fn ui(&mut self, ctx: &Context) {
        self.capture_recorded_shortcut(ctx);

        let mut open = self.open;
        egui::Window::new("Settings")
            .open(&mut open)
            .default_width(430.0)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                settings_section(ui, "Appearance");
                egui::Grid::new("appearance_settings_grid")
                    .num_columns(2)
                    .spacing([18.0, 12.0])
                    .show(ui, |ui| {
                        ui.label("UI text size");
                        if ui
                            .add(
                                egui::Slider::new(&mut self.ui_font_size, 9.0..=28.0).suffix(" px"),
                            )
                            .changed()
                        {
                            self.dirty = true;
                        }
                        ui.end_row();

                        ui.label("UI font");
                        egui::ComboBox::from_id_salt("ui_font")
                            .selected_text(self.ui_font.label())
                            .show_ui(ui, |ui| {
                                for font in UiFont::ALL {
                                    if ui
                                        .selectable_value(&mut self.ui_font, font, font.label())
                                        .changed()
                                    {
                                        self.dirty = true;
                                    }
                                }
                            });
                        ui.end_row();

                        ui.label("Editor text size");
                        if ui
                            .add(
                                egui::Slider::new(&mut self.editor_font_size, 9.0..=32.0)
                                    .suffix(" px"),
                            )
                            .changed()
                        {
                            self.dirty = true;
                        }
                        ui.end_row();

                        ui.label("Editor font");
                        egui::ComboBox::from_id_salt("mono_font")
                            .selected_text(self.mono.label())
                            .show_ui(ui, |ui| {
                                for font in MonoFont::ALL {
                                    if ui
                                        .selectable_value(&mut self.mono, font, font.label())
                                        .changed()
                                    {
                                        self.dirty = true;
                                    }
                                }
                            });
                        ui.end_row();
                    });

                ui.add_space(14.0);
                settings_section(ui, "Shortcuts");
                egui::Grid::new("shortcut_settings_grid")
                    .num_columns(3)
                    .spacing([18.0, 10.0])
                    .show(ui, |ui| {
                        self.shortcut_row(ui, "Editor page", ShortcutAction::EditorPage);
                        self.shortcut_row(ui, "Diff page", ShortcutAction::DiffPage);
                    });

                ui.add_space(8.0);
                match self.recording {
                    Some(_) => ui.weak("Press the new shortcut. Esc cancels recording."),
                    None => ui.weak("Editor font applies to the editor and diff text."),
                };
            });
        self.open = open;
    }

    fn shortcut_row(&mut self, ui: &mut egui::Ui, label: &str, action: ShortcutAction) {
        ui.label(label);
        ui.monospace(self.shortcut(action).label());
        let recording = self.recording == Some(action);
        let button = if recording { "Recording..." } else { "Record" };
        if ui.button(button).clicked() {
            self.recording = Some(action);
        }
        ui.end_row();
    }

    fn shortcut(&self, action: ShortcutAction) -> KeyBinding {
        match action {
            ShortcutAction::EditorPage => self.editor_shortcut,
            ShortcutAction::DiffPage => self.diff_shortcut,
        }
    }

    fn set_shortcut(&mut self, action: ShortcutAction, binding: KeyBinding) {
        match action {
            ShortcutAction::EditorPage => self.editor_shortcut = binding,
            ShortcutAction::DiffPage => self.diff_shortcut = binding,
        }
    }

    fn capture_recorded_shortcut(&mut self, ctx: &Context) {
        let Some(action) = self.recording else {
            return;
        };

        let events = ctx.input(|i| i.events.clone());
        for event in events {
            if let egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } = event
            {
                if key == Key::Escape {
                    self.recording = None;
                    return;
                }
                let normalized = Modifiers {
                    alt: modifiers.alt,
                    ctrl: modifiers.ctrl,
                    shift: modifiers.shift,
                    mac_cmd: false,
                    command: modifiers.command,
                };
                self.set_shortcut(action, KeyBinding::new(key, normalized));
                self.recording = None;
                return;
            }
        }
    }
}

fn settings_section(ui: &mut egui::Ui, title: &str) {
    ui.label(egui::RichText::new(title).strong());
    ui.separator();
    ui.add_space(6.0);
}

fn key_label(key: Key) -> &'static str {
    match key {
        Key::ArrowDown => "Down",
        Key::ArrowLeft => "Left",
        Key::ArrowRight => "Right",
        Key::ArrowUp => "Up",
        Key::Escape => "Esc",
        Key::Tab => "Tab",
        Key::Backspace => "Backspace",
        Key::Enter => "Enter",
        Key::Space => "Space",
        Key::Insert => "Insert",
        Key::Delete => "Delete",
        Key::Home => "Home",
        Key::End => "End",
        Key::PageUp => "PageUp",
        Key::PageDown => "PageDown",
        Key::Num0 => "0",
        Key::Num1 => "1",
        Key::Num2 => "2",
        Key::Num3 => "3",
        Key::Num4 => "4",
        Key::Num5 => "5",
        Key::Num6 => "6",
        Key::Num7 => "7",
        Key::Num8 => "8",
        Key::Num9 => "9",
        Key::A => "A",
        Key::B => "B",
        Key::C => "C",
        Key::D => "D",
        Key::E => "E",
        Key::F => "F",
        Key::G => "G",
        Key::H => "H",
        Key::I => "I",
        Key::J => "J",
        Key::K => "K",
        Key::L => "L",
        Key::M => "M",
        Key::N => "N",
        Key::O => "O",
        Key::P => "P",
        Key::Q => "Q",
        Key::R => "R",
        Key::S => "S",
        Key::T => "T",
        Key::U => "U",
        Key::V => "V",
        Key::W => "W",
        Key::X => "X",
        Key::Y => "Y",
        Key::Z => "Z",
        _ => "?",
    }
}
