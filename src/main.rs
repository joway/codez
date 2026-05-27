// CodeZ — a fast, native (wgpu/Metal) git diff viewer for macOS.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agent;
mod app;
mod cli;
mod editor;
mod fstree;
mod gitmodel;
mod highlight;
mod menu;
mod palette;
mod search;
mod settings;
mod terminal;
mod textview;
mod theme;
mod usage;

use std::path::PathBuf;

use app::CodezApp;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([800.0, 480.0])
            .with_title("")
            .with_icon(app_icon()),
        ..Default::default()
    };

    let (dir, file) = startup_dir();
    eframe::run_native(
        "",
        options,
        Box::new(move |cc| {
            // Force the GitHub-flavored dark theme regardless of system appearance.
            cc.egui_ctx.set_theme(egui::ThemePreference::Dark);
            cc.egui_ctx.set_visuals(theme::github_dark());
            // Darken the native window chrome (titlebar) to match, instead of
            // following the system's light appearance.
            #[cfg(target_os = "macos")]
            force_dark_window_chrome();
            // Fonts (incl. CJK fallback) are installed by Settings on first frame.
            Ok(Box::new(CodezApp::new(dir, file)))
        }),
    )
}

fn app_icon() -> egui::IconData {
    const ICON: &[u8] = include_bytes!("../assets/logo-256.png");

    let decoder = png::Decoder::new(std::io::Cursor::new(ICON));
    let mut reader = decoder
        .read_info()
        .expect("embedded app icon should be a valid PNG");
    let mut rgba = vec![0; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut rgba)
        .expect("embedded app icon should decode");
    rgba.truncate(info.buffer_size());

    assert_eq!(
        (info.color_type, info.bit_depth),
        (png::ColorType::Rgba, png::BitDepth::Eight),
        "embedded app icon must be 8-bit RGBA"
    );

    egui::IconData {
        rgba,
        width: info.width,
        height: info.height,
    }
}

/// Set the whole application's appearance to dark aqua so the native titlebar
/// renders dark (matching our content) even when macOS is in light mode. Uses
/// the real native titlebar — unlike `fullsize_content_view`, which mis-sizes
/// the egui drawable on wgpu and cuts off the bottom of the content.
#[cfg(target_os = "macos")]
fn force_dark_window_chrome() {
    use objc2_app_kit::{NSAppearance, NSAppearanceNameDarkAqua, NSApplication};
    use objc2_foundation::MainThreadMarker;

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    let dark = unsafe { NSAppearance::appearanceNamed(NSAppearanceNameDarkAqua) };
    app.setAppearance(dark.as_deref());
}

/// Resolve the launch target from the path argument (or the current working
/// directory): the folder to open, plus an optional file to show immediately
/// when the argument points at a file.
///
/// When launched as a double-clicked `.app` bundle the working directory is
/// `/`, so we skip auto-opening there to avoid scanning the whole filesystem.
fn startup_dir() -> (Option<PathBuf>, Option<PathBuf>) {
    let raw = match std::env::args_os().nth(1) {
        Some(arg) => PathBuf::from(arg),
        None => match std::env::current_dir() {
            Ok(d) => d,
            Err(_) => return (None, None),
        },
    };
    let path = std::fs::canonicalize(&raw).unwrap_or(raw);
    let (dir, file) = if path.is_dir() {
        (path, None)
    } else {
        match path.parent() {
            Some(p) => (p.to_path_buf(), Some(path.clone())),
            None => return (None, None),
        }
    };
    if dir == std::path::Path::new("/") {
        return (None, None);
    }
    (Some(dir), file)
}
