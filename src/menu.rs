//! Native macOS menu bar (NSMenu) via `muda`.
//!
//! Built on the main thread inside eframe's creation closure (after the
//! NSApplication exists) and installed with `init_for_nsapp`. Click events
//! arrive on a global channel the app polls each frame. We use plain
//! `MenuItem`s (not check items): the active mode is shown by the in-app
//! Editor/Diff segmented control, and `CheckMenuItem::set_checked` re-emits
//! menu events on macOS, which would spuriously flip the mode.

use muda::{
    accelerator::{Accelerator, Code, Modifiers},
    Menu, MenuId, MenuItem, PredefinedMenuItem, Submenu,
};

pub struct Menus {
    pub open: MenuId,
    pub save: MenuId,
    pub settings: MenuId,
    pub editor: MenuId,
    pub diff: MenuId,
    _menu: Menu, // kept alive for the app's lifetime
}

impl Menus {
    pub fn install() -> Menus {
        let menu = Menu::new();

        let app_menu = Submenu::new("Diffist", true);
        let settings = MenuItem::new(
            "Settings…",
            true,
            Some(Accelerator::new(Some(Modifiers::SUPER), Code::Comma)),
        );
        let _ = app_menu.append_items(&[
            &PredefinedMenuItem::about(Some("About Diffist"), None),
            &PredefinedMenuItem::separator(),
            &settings,
            &PredefinedMenuItem::separator(),
            &PredefinedMenuItem::quit(None),
        ]);

        let file_menu = Submenu::new("File", true);
        let open = MenuItem::new(
            "Open Folder…",
            true,
            Some(Accelerator::new(Some(Modifiers::SUPER), Code::KeyO)),
        );
        let save = MenuItem::new(
            "Save",
            true,
            Some(Accelerator::new(Some(Modifiers::SUPER), Code::KeyS)),
        );
        let _ = file_menu.append_items(&[&open, &save]);

        let view_menu = Submenu::new("View", true);
        let editor = MenuItem::new(
            "Editor",
            true,
            Some(Accelerator::new(Some(Modifiers::SUPER), Code::Digit1)),
        );
        let diff = MenuItem::new(
            "Diff",
            true,
            Some(Accelerator::new(Some(Modifiers::SUPER), Code::Digit2)),
        );
        let _ = view_menu.append_items(&[&editor, &diff]);

        let _ = menu.append_items(&[&app_menu, &file_menu, &view_menu]);

        #[cfg(target_os = "macos")]
        menu.init_for_nsapp();

        Menus {
            open: open.id().clone(),
            save: save.id().clone(),
            settings: settings.id().clone(),
            editor: editor.id().clone(),
            diff: diff.id().clone(),
            _menu: menu,
        }
    }
}
