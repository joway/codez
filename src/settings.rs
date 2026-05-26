//! Appearance settings + the settings window.
//!
//! Keeps the global UI font and the editor/diff monospace font.

use egui::{Context, FontData, FontDefinitions, FontFamily, FontId, TextStyle};

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

pub struct Settings {
    pub open: bool,
    ui_font_size: f32,
    editor_font_size: f32,
    ui_font: UiFont,
    mono: MonoFont,
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
            dirty: true,
        }
    }
}

impl Settings {
    pub fn ui_font_size(&self) -> f32 {
        self.ui_font_size
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

                ui.add_space(8.0);
                ui.weak("Editor font applies to the editor and diff text.");
            });
        self.open = open;
    }
}

fn settings_section(ui: &mut egui::Ui, title: &str) {
    ui.label(egui::RichText::new(title).strong());
    ui.separator();
    ui.add_space(6.0);
}
