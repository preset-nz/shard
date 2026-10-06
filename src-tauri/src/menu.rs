//! Shard's own commands, for app-kit's command table (epic 32, story 4).
//!
//! app-kit builds the menu bar around them: the App menu, Undo and Redo named
//! after what they would do, Cut, Copy, Paste, the Window menu. Every
//! activation reaches the webview as app-kit's `command` event carrying the
//! id, and what each command does lives in TypeScript (`src/App.tsx`), in the
//! same functions the keyboard fallback calls.
//!
//! **A menu accelerator is consumed by `NSMenu` before the key reaches the
//! webview,** so a command's shortcut is written here and nowhere else. The
//! ids and accelerators are the ones Shard had before app-kit.

use preset_app_kit::{Command, MenuName};
use shard_dsp::fx::Kind;

/// `fx-add:chorus` adds a chorus; the part after the colon is `Kind::name`.
pub const FX_ADD_PREFIX: &str = "fx-add:";

/// The app's name, as the App menu and About show it.
pub const APP_NAME: &str = "Shard";

/// app-kit's switches for its built-in items: File's New, Open, Open Recent,
/// Close, Save, Save As and Revert run against the session.
pub const MENU_CONFIG: &str = include_str!("../menu.toml");

/// Every accelerator Shard binds, by command id: written here and nowhere
/// else, and held unique by a test.
pub const SHORTCUTS: &[(&str, &str)] = &[
    ("file-reset-sound", "CmdOrCtrl+Shift+R"),
    ("file-add-material", "CmdOrCtrl+Shift+O"),
    ("view-tracker", "CmdOrCtrl+1"),
    ("view-soundscape", "CmdOrCtrl+2"),
    ("view-inspector", "CmdOrCtrl+I"),
    ("fx-remove", "CmdOrCtrl+Shift+Backspace"),
    ("fx-earlier", "CmdOrCtrl+Alt+Up"),
    ("fx-later", "CmdOrCtrl+Alt+Down"),
    ("pattern-copy", "CmdOrCtrl+Alt+C"),
    ("pattern-paste", "CmdOrCtrl+Alt+V"),
    ("pattern-repeat", "CmdOrCtrl+D"),
    ("pattern-clear", "CmdOrCtrl+Alt+Backspace"),
    ("pattern-up", "CmdOrCtrl+Ctrl+Up"),
    ("pattern-down", "CmdOrCtrl+Ctrl+Down"),
    ("pattern-octave-up", "CmdOrCtrl+Ctrl+Shift+Up"),
    ("pattern-octave-down", "CmdOrCtrl+Ctrl+Shift+Down"),
    ("pattern-rotate-back", "CmdOrCtrl+["),
    ("pattern-rotate-forward", "CmdOrCtrl+]"),
];

/// A command with its accelerator from `SHORTCUTS`, if it has one.
fn item(id: &str, label: &str) -> Command {
    let c = Command::item(id, label);
    match SHORTCUTS.iter().find(|(i, _)| *i == id) {
        Some((_, accelerator)) => c.accelerator(accelerator),
        None => c,
    }
}

/// Every command Shard adds to app-kit's.
pub fn commands() -> Vec<Command> {
    let file = |id, label, section| item(id, label).menu(MenuName::File).section(section);
    let view = |id, label, section| item(id, label).menu(MenuName::View).section(section);
    let effect = |id, label| item(id, label).domain("Effect").section(1);
    let pattern = |id, label, section| item(id, label).domain("Pattern").section(section);
    let mut all = vec![
        file("file-reset-sound", "Reset Sound", 1),
        file("file-add-material", "Add Material…", 1),
        view("view-tracker", "Tracker", 0),
        view("view-soundscape", "Sound Scaping", 0),
        view("view-inspector", "Grain Inspector", 1),
    ];
    // The process lane's palette (`design/effect-palette.md`).
    for kind in Kind::ALL {
        all.push(
            item(&format!("{FX_ADD_PREFIX}{}", kind.name()), kind.label())
                .domain("Effect")
                .section(0)
                .submenu("Add Effect"),
        );
    }
    all.extend([
        effect("fx-remove", "Remove Effect"),
        effect("fx-earlier", "Move Earlier"),
        effect("fx-later", "Move Later"),
        // The tracker's bar commands, on the step last focused.
        pattern("pattern-copy", "Copy Bar", 0),
        pattern("pattern-paste", "Paste Bar", 0),
        pattern("pattern-repeat", "Repeat Bar", 0),
        pattern("pattern-clear", "Clear Bar", 0),
        pattern("pattern-up", "Transpose Up", 1),
        pattern("pattern-down", "Transpose Down", 1),
        pattern("pattern-octave-up", "Transpose Up an Octave", 1),
        pattern("pattern-octave-down", "Transpose Down an Octave", 1),
        pattern("pattern-rotate-back", "Rotate Earlier", 2),
        pattern("pattern-rotate-forward", "Rotate Later", 2),
    ]);
    all
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
    fn the_menu_config_parses() {
        preset_app_kit::MenuConfig::parse(MENU_CONFIG).expect("menu.toml is valid");
    }

    /// What app-kit and macOS bind in the menu bar before Shard adds anything:
    /// New, Open, Save, Save As, Close, Undo, Redo, Settings, Quit, Hide, Hide
    /// Others, Minimise, Full Screen, Cut, Copy, Paste and Select All.
    const BUILT_IN: &[&str] = &[
        "CmdOrCtrl+N",
        "CmdOrCtrl+O",
        "CmdOrCtrl+S",
        "CmdOrCtrl+Shift+S",
        "CmdOrCtrl+Z",
        "CmdOrCtrl+Shift+Z",
        "CmdOrCtrl+,",
        "CmdOrCtrl+Q",
        "CmdOrCtrl+H",
        "CmdOrCtrl+Alt+H",
        "CmdOrCtrl+M",
        "CmdOrCtrl+W",
        "CmdOrCtrl+Ctrl+F",
        "CmdOrCtrl+X",
        "CmdOrCtrl+C",
        "CmdOrCtrl+V",
        "CmdOrCtrl+A",
    ];

    #[test]
    fn no_two_commands_share_an_accelerator_or_an_id() {
        let table = commands();
        let ids: Vec<&str> = table.iter().map(|c| c.id()).collect();
        for (n, id) in ids.iter().enumerate() {
            assert!(!ids[n + 1..].contains(id), "{id} twice");
        }
        let mut taken: Vec<&str> = BUILT_IN.to_vec();
        for (id, accelerator) in SHORTCUTS {
            assert!(ids.contains(id), "{id} has a shortcut but no command");
            assert!(
                !taken.contains(accelerator),
                "{id} takes {accelerator}, which is taken"
            );
            taken.push(accelerator);
        }
    }
}
