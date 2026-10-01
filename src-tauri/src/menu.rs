//! The native macOS menu bar (`design/native-apps.md` rule 1: the menu owns
//! every command, with its shortcut shown beside it).
//!
//! Rust is plumbing: it builds the menu and forwards every activation to the
//! webview as one event, `menu`, whose payload is the item's id. What each
//! command does lives in TypeScript, in the same functions the keyboard
//! fallback calls (`src/menu.ts`), so there is one handler per command.
//!
//! **A menu accelerator is consumed by `NSMenu` before the key reaches the
//! webview.** So a command here has its shortcut here and nowhere else, and
//! the webview's own key handlers stay only as the fallback for when the menu
//! could not be built. It also means a shortcut the strip used to handle
//! itself, such as ⌘C, belongs to the Edit menu now; the tracker's bar
//! commands have their own accelerators in the Pattern menu.
//!
//! Undo and Redo are custom items rather than the predefined ones, which talk
//! to the focused text field's undo manager, not to the document's history
//! (`history.rs`). Their titles carry the name of what they would do.

use shard_dsp::fx::Kind;
use tauri::menu::{AboutMetadata, Menu, MenuItem, PredefinedMenuItem, Submenu, SubmenuBuilder};
use tauri::{AppHandle, Emitter, Manager, Runtime, Wry};

/// The one event every activation arrives as; the payload is the item id.
pub const EVENT: &str = "menu";

/// Bug reports go here.
const CONTACT_EMAIL: &str = "georg@preset.nz";

/// `fx-add:chorus` adds a chorus; the part after the colon is `Kind::name`.
pub const FX_ADD_PREFIX: &str = "fx-add:";

/// The two items whose titles change with the history.
pub struct HistoryItems {
    undo: MenuItem<Wry>,
    redo: MenuItem<Wry>,
}

impl HistoryItems {
    /// "Undo Change Size", or plain "Undo" and greyed out when there is
    /// nothing to undo.
    pub fn set(&self, undo: Option<&str>, redo: Option<&str>) {
        for (item, verb, what) in [(&self.undo, "Undo", undo), (&self.redo, "Redo", redo)] {
            let _ = item.set_text(title(verb, what));
            let _ = item.set_enabled(what.is_some());
        }
    }
}

/// A menu title for a command with an optional object.
pub fn title(verb: &str, what: Option<&str>) -> String {
    match what {
        Some(what) => format!("{verb} {what}"),
        None => verb.to_string(),
    }
}

fn item<R: Runtime>(
    app: &AppHandle<R>,
    id: &str,
    text: &str,
    accelerator: Option<&str>,
) -> tauri::Result<MenuItem<R>> {
    MenuItem::with_id(app, id, text, true, accelerator)
}

/// Build the menu bar and install it. Called once from `setup`; if it fails
/// the app keeps Tauri's default menu and the webview's key handlers carry
/// the shortcuts.
pub fn install(app: &AppHandle<Wry>) -> tauri::Result<()> {
    let name = app.package_info().name.clone();
    let about = AboutMetadata {
        name: Some(name.clone()),
        version: Some(app.package_info().version.to_string()),
        copyright: Some("Free, pre-release build. No warranty.".into()),
        credits: Some(format!("Bugs and feedback: {CONTACT_EMAIL}")),
        ..Default::default()
    };

    let app_menu = Submenu::with_items(
        app,
        &name,
        true,
        &[
            &PredefinedMenuItem::about(app, None, Some(about))?,
            &PredefinedMenuItem::separator(app)?,
            &item(app, "app-settings", "Settings…", Some("CmdOrCtrl+,"))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::services(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::quit(app, None)?,
        ],
    )?;

    let file = Submenu::with_items(
        app,
        "File",
        true,
        &[
            &item(app, "file-open", "Open…", Some("CmdOrCtrl+O"))?,
            &item(app, "file-save", "Save", Some("CmdOrCtrl+S"))?,
            &PredefinedMenuItem::separator(app)?,
            &item(
                app,
                "file-reset-sound",
                "Reset Sound",
                Some("CmdOrCtrl+Shift+R"),
            )?,
            &item(
                app,
                "file-add-material",
                "Add Material…",
                Some("CmdOrCtrl+Shift+O"),
            )?,
        ],
    )?;

    let undo = item(app, "edit-undo", "Undo", Some("CmdOrCtrl+Z"))?;
    let redo = item(app, "edit-redo", "Redo", Some("CmdOrCtrl+Shift+Z"))?;
    undo.set_enabled(false)?;
    redo.set_enabled(false)?;
    let edit = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &undo,
            &redo,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;

    // The process lane's palette (`design/effect-palette.md`).
    let mut add = SubmenuBuilder::new(app, "Add Effect");
    for kind in Kind::ALL {
        add = add.item(&item(
            app,
            &format!("{FX_ADD_PREFIX}{}", kind.name()),
            kind.label(),
            None,
        )?);
    }
    let effects = Submenu::with_items(
        app,
        "Effects",
        true,
        &[
            &add.build()?,
            &PredefinedMenuItem::separator(app)?,
            &item(
                app,
                "fx-remove",
                "Remove Effect",
                Some("CmdOrCtrl+Shift+Backspace"),
            )?,
            &item(app, "fx-earlier", "Move Earlier", Some("CmdOrCtrl+Alt+Up"))?,
            &item(app, "fx-later", "Move Later", Some("CmdOrCtrl+Alt+Down"))?,
        ],
    )?;

    // The tracker's bar commands, on the step last focused.
    let pattern = Submenu::with_items(
        app,
        "Pattern",
        true,
        &[
            &item(app, "pattern-copy", "Copy Bar", Some("CmdOrCtrl+Alt+C"))?,
            &item(app, "pattern-paste", "Paste Bar", Some("CmdOrCtrl+Alt+V"))?,
            &item(app, "pattern-repeat", "Repeat Bar", Some("CmdOrCtrl+D"))?,
            &item(
                app,
                "pattern-clear",
                "Clear Bar",
                Some("CmdOrCtrl+Alt+Backspace"),
            )?,
            &PredefinedMenuItem::separator(app)?,
            &item(app, "pattern-up", "Transpose Up", Some("CmdOrCtrl+Ctrl+Up"))?,
            &item(
                app,
                "pattern-down",
                "Transpose Down",
                Some("CmdOrCtrl+Ctrl+Down"),
            )?,
            &item(
                app,
                "pattern-octave-up",
                "Transpose Up an Octave",
                Some("CmdOrCtrl+Ctrl+Shift+Up"),
            )?,
            &item(
                app,
                "pattern-octave-down",
                "Transpose Down an Octave",
                Some("CmdOrCtrl+Ctrl+Shift+Down"),
            )?,
            &PredefinedMenuItem::separator(app)?,
            &item(
                app,
                "pattern-rotate-back",
                "Rotate Earlier",
                Some("CmdOrCtrl+["),
            )?,
            &item(
                app,
                "pattern-rotate-forward",
                "Rotate Later",
                Some("CmdOrCtrl+]"),
            )?,
        ],
    )?;

    let view = Submenu::with_items(
        app,
        "View",
        true,
        &[
            &item(app, "view-tracker", "Tracker", Some("CmdOrCtrl+1"))?,
            &item(app, "view-soundscape", "Sound Scaping", Some("CmdOrCtrl+2"))?,
            &PredefinedMenuItem::separator(app)?,
            &item(
                app,
                "view-inspector",
                "Grain Inspector",
                Some("CmdOrCtrl+I"),
            )?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::fullscreen(app, None)?,
        ],
    )?;

    let window = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, None)?,
        ],
    )?;

    let menu = Menu::with_items(
        app,
        &[&app_menu, &file, &edit, &effects, &pattern, &view, &window],
    )?;
    app.set_menu(menu)?;
    app.manage(HistoryItems { undo, redo });
    Ok(())
}

/// Forward a menu activation to the webview.
pub fn forward(app: &AppHandle<Wry>, id: &str) {
    if let Err(e) = app.emit(EVENT, id) {
        eprintln!("shard: could not forward menu item {id}: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_add_id_is_the_prefix_and_the_kinds_name() {
        // The webview strips the prefix and hands the rest to `fx_add`, which
        // reads it with `Kind::from_name`; the two must agree.
        for kind in Kind::ALL {
            let id = format!("{FX_ADD_PREFIX}{}", kind.name());
            assert_eq!(
                Kind::from_name(id.strip_prefix(FX_ADD_PREFIX).unwrap()),
                Some(kind)
            );
        }
    }

    #[test]
    fn a_title_carries_what_it_would_do() {
        assert_eq!(title("Undo", Some("Change Size")), "Undo Change Size");
        assert_eq!(title("Redo", None), "Redo");
    }
}
