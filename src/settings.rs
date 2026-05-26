//! Appearance settings + the settings window.
//!
//! Holds the editor/diff font size and family, applies them to the egui
//! context (rebuilding the font set when the family changes, always keeping a
//! CJK fallback), and renders a small settings window.

use egui::{Context, FontData, FontDefinitions, FontFamily, FontId, TextStyle};

const CJK_FONTS: &[&str] = &[
    "/System/Library/Fonts/PingFang.ttc",
    "/System/Library/Fonts/STHeiti Light.ttc",
    "/System/Library/Fonts/Hiragino Sans GB.ttc",
];

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
    font_size: f32,
    mono: MonoFont,
    dirty: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            open: false,
            font_size: 13.0,
            mono: MonoFont::SfMono,
            dirty: true,
        }
    }
}

impl Settings {
    /// Re-apply fonts and text sizes to the context when something changed.
    pub fn apply(&mut self, ctx: &Context) {
        if !self.dirty {
            return;
        }
        self.dirty = false;

        let mut fonts = FontDefinitions::default();

        // Chosen monospace font becomes the primary Monospace family entry.
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

        // CJK fallback appended to both families so Chinese text always renders.
        for path in CJK_FONTS {
            if let Ok(bytes) = std::fs::read(path) {
                fonts
                    .font_data
                    .insert("cjk".to_owned(), FontData::from_owned(bytes).into());
                for fam in [FontFamily::Proportional, FontFamily::Monospace] {
                    fonts.families.entry(fam).or_default().push("cjk".to_owned());
                }
                break;
            }
        }

        ctx.set_fonts(fonts);

        let mut style = (*ctx.style()).clone();
        style
            .text_styles
            .insert(TextStyle::Monospace, FontId::monospace(self.font_size));
        ctx.set_style(style);
    }

    /// Draw the settings window if open.
    pub fn ui(&mut self, ctx: &Context) {
        let mut open = self.open;
        egui::Window::new("Settings")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                egui::Grid::new("settings_grid")
                    .num_columns(2)
                    .spacing([18.0, 12.0])
                    .show(ui, |ui| {
                        ui.label("Font size");
                        if ui
                            .add(egui::Slider::new(&mut self.font_size, 9.0..=28.0).suffix(" px"))
                            .changed()
                        {
                            self.dirty = true;
                        }
                        ui.end_row();

                        ui.label("Editor font");
                        egui::ComboBox::from_id_salt("mono_font")
                            .selected_text(self.mono.label())
                            .show_ui(ui, |ui| {
                                for m in MonoFont::ALL {
                                    if ui
                                        .selectable_value(&mut self.mono, m, m.label())
                                        .changed()
                                    {
                                        self.dirty = true;
                                    }
                                }
                            });
                        ui.end_row();
                    });
                ui.add_space(8.0);
                ui.weak("Applies to the Editor preview and the Diff view.");
            });
        self.open = open;
    }
}
