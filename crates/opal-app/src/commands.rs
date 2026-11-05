//! The command registry: every one-shot action, defined once.
//!
//! The ribbon, command search, keyboard shortcuts, keytips, context menus and the help
//! pages are all generated from [`Cmd::info`], [`RIBBON`] and [`availability`], so a
//! command looks and behaves the same everywhere.

use crate::icons;
use crate::state::AppState;
use eframe::egui::{Key, KeyboardShortcut, Modifiers};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cmd {
    New,
    Open,
    Save,
    SaveAs,
    Export,
    Close,
    Preferences,
    Exit,
    Undo,
    Redo,
    NewLayer,
    NewGroup,
    Duplicate,
    DeleteLayer,
    MoveUp,
    MoveDown,
    MergeDown,
    Flatten,
    ToggleVisible,
    ToggleLock,
    Rename,
    ZoomIn,
    ZoomOut,
    ZoomFit,
    Zoom100,
    ToggleOffCanvas,
    ToggleTheme,
    ToggleRibbon,
    CycleToolsPlace,
    ResetPanels,
    Search,
    Help,
    Shortcuts,
    About,
    WidgetGallery,
}

/// What the user sees of a command.
pub struct Info {
    pub name: &'static str,
    pub icon: char,
    /// One line: what it does.
    pub description: &'static str,
    /// More, for the help pages.
    pub help: &'static str,
    /// The first is shown; all of them work.
    pub shortcuts: &'static [KeyboardShortcut],
}

const fn ctrl(key: Key) -> KeyboardShortcut {
    KeyboardShortcut::new(Modifiers::COMMAND, key)
}

const fn ctrl_shift(key: Key) -> KeyboardShortcut {
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), key)
}

const fn plain(key: Key) -> KeyboardShortcut {
    KeyboardShortcut::new(Modifiers::NONE, key)
}

impl Cmd {
    pub const ALL: [Cmd; 35] = [
        Cmd::New,
        Cmd::Open,
        Cmd::Save,
        Cmd::SaveAs,
        Cmd::Export,
        Cmd::Close,
        Cmd::Preferences,
        Cmd::Exit,
        Cmd::Undo,
        Cmd::Redo,
        Cmd::NewLayer,
        Cmd::NewGroup,
        Cmd::Duplicate,
        Cmd::DeleteLayer,
        Cmd::MoveUp,
        Cmd::MoveDown,
        Cmd::MergeDown,
        Cmd::Flatten,
        Cmd::ToggleVisible,
        Cmd::ToggleLock,
        Cmd::Rename,
        Cmd::ZoomIn,
        Cmd::ZoomOut,
        Cmd::ZoomFit,
        Cmd::Zoom100,
        Cmd::ToggleOffCanvas,
        Cmd::ToggleTheme,
        Cmd::ToggleRibbon,
        Cmd::CycleToolsPlace,
        Cmd::ResetPanels,
        Cmd::Search,
        Cmd::Help,
        Cmd::Shortcuts,
        Cmd::About,
        Cmd::WidgetGallery,
    ];

    pub fn info(self) -> Info {
        let i = |name, icon, description, help, shortcuts| Info { name, icon, description, help, shortcuts };
        match self {
            Cmd::New => i(
                "New",
                icons::NEW,
                "Create a new image.",
                "Asks for the size and the background, then opens the new image in its own tab.",
                const { &[ctrl(Key::N)] },
            ),
            Cmd::Open => i(
                "Open",
                icons::OPEN,
                "Open an image or an Opal Studio file.",
                "Opens Opal Studio files (.opal), which keep every layer editable, and PNG and \
                 JPEG images, which open as a single layer. Each opens in its own tab.",
                const { &[ctrl(Key::O)] },
            ),
            Cmd::Save => i(
                "Save",
                icons::SAVE,
                "Save the image as an Opal Studio file.",
                "Saves everything, layers included, to the image's .opal file; the first time, \
                 asks where. To make a PNG or JPEG, use Export.",
                const { &[ctrl(Key::S)] },
            ),
            Cmd::SaveAs => i(
                "Save As",
                icons::SAVE_AS,
                "Save the image under a new name.",
                "Asks for a new .opal file name and saves there; the image then belongs to the \
                 new file.",
                const { &[ctrl_shift(Key::S)] },
            ),
            Cmd::Export => i(
                "Export",
                icons::EXPORT,
                "Save a flattened copy as PNG or JPEG.",
                "Writes the canvas, with all visible layers combined, to a PNG or JPEG file. \
                 Content beyond the canvas edge is left out. The image itself is not changed \
                 and keeps its layers. Options are in Preferences › Export.",
                const { &[ctrl_shift(Key::E)] },
            ),
            Cmd::Close => i(
                "Close",
                icons::CLOSE,
                "Close the image.",
                "Closes the image's tab, offering to save unsaved changes first.",
                const { &[ctrl(Key::W)] },
            ),
            Cmd::Preferences => i(
                "Preferences",
                icons::SETTINGS,
                "Change how Opal Studio looks and behaves.",
                "Every setting that affects the interface, the canvas, navigation and export.",
                const { &[ctrl(Key::Comma)] },
            ),
            Cmd::Exit => i("Exit", icons::CLOSE, "Quit Opal Studio.", "Offers to save unsaved changes first.", &[]),
            Cmd::Undo => i(
                "Undo",
                icons::UNDO,
                "Take back the last action.",
                "Moves back one step in the history. Nothing is lost: what you undid stays in the \
                 history as a branch, and Redo or the History panel brings it back.",
                const { &[ctrl(Key::Z)] },
            ),
            Cmd::Redo => i(
                "Redo",
                icons::REDO,
                "Repeat the action you undid.",
                "Moves forward one step in the history. Where the history branches, Redo follows \
                 the branch you were last on; the History panel reaches the others.",
                const { &[ctrl(Key::Y), ctrl_shift(Key::Z)] },
            ),
            Cmd::NewLayer => i(
                "New Layer",
                icons::LAYER_ADD,
                "Add an empty layer above the active one.",
                "Layers have no edges: whatever you put on one can extend past the canvas.",
                const { &[ctrl_shift(Key::N)] },
            ),
            Cmd::NewGroup => i(
                "New Group",
                icons::GROUP,
                "Add a group to hold layers.",
                "A group combines the layers inside it, then shows the result with the group's \
                 own visibility and opacity. Drag layers onto it in the Layers panel.",
                const { &[ctrl(Key::G)] },
            ),
            Cmd::Duplicate => i(
                "Duplicate Layer",
                icons::COPY,
                "Copy the active layer.",
                "The copy goes just above the original. It shares the original's pixels until \
                 one of them is changed, so duplicating is instant and costs no memory.",
                const { &[ctrl(Key::J)] },
            ),
            Cmd::DeleteLayer => i(
                "Delete Layer",
                icons::DELETE,
                "Remove the active layer.",
                "Removes the layer (and, for a group, everything in it). Undo brings it back.",
                &[],
            ),
            Cmd::MoveUp => i(
                "Move Up",
                icons::ARROW_UP,
                "Move the active layer one place up.",
                "Within its group. Drag layers in the Layers panel to move them anywhere.",
                const { &[ctrl(Key::CloseBracket)] },
            ),
            Cmd::MoveDown => i(
                "Move Down",
                icons::ARROW_DOWN,
                "Move the active layer one place down.",
                "Within its group. Drag layers in the Layers panel to move them anywhere.",
                const { &[ctrl(Key::OpenBracket)] },
            ),
            Cmd::MergeDown => i(
                "Merge Down",
                icons::MERGE,
                "Combine the active layer with the layer below it.",
                "The result keeps the lower layer's name and place. Both layers must be visible, \
                 and the lower one cannot be a group.",
                const { &[ctrl(Key::E)] },
            ),
            Cmd::Flatten => i(
                "Flatten Image",
                icons::STACK,
                "Combine all visible layers into one.",
                "Hidden layers are discarded. Undo brings everything back.",
                &[],
            ),
            Cmd::ToggleVisible => i(
                "Show/Hide Layer",
                icons::EYE,
                "Show or hide the active layer.",
                "Hidden layers are not shown or exported, but are kept.",
                &[],
            ),
            Cmd::ToggleLock => i(
                "Lock/Unlock Layer",
                icons::LOCK,
                "Protect the active layer from changes.",
                "A locked layer cannot be painted on, moved, merged or deleted until unlocked.",
                &[],
            ),
            Cmd::Rename => i(
                "Rename Layer",
                icons::RENAME,
                "Give the active layer a new name.",
                "You can also double-click a layer's name in the Layers panel.",
                const { &[plain(Key::F2)] },
            ),
            Cmd::ZoomIn => i(
                "Zoom In",
                icons::ZOOM_IN,
                "Enlarge the view.",
                "Steps through the preset levels, around the center of the view.",
                const { &[ctrl(Key::Plus), ctrl(Key::Equals)] },
            ),
            Cmd::ZoomOut => i(
                "Zoom Out",
                icons::ZOOM_OUT,
                "Shrink the view.",
                "Steps through the preset levels, around the center of the view.",
                const { &[ctrl(Key::Minus)] },
            ),
            Cmd::ZoomFit => i(
                "Fit to Window",
                icons::ZOOM_FIT,
                "Show the whole canvas.",
                "Zooms so the canvas fills the view, without enlarging past the limit set in \
                 Preferences › Navigation.",
                const { &[ctrl(Key::Num0)] },
            ),
            Cmd::Zoom100 => i(
                "Actual Pixels",
                icons::ZOOM_100,
                "Zoom to 100%: one image pixel per screen pixel.",
                "At 100% every image pixel is exactly one pixel of your screen.",
                const { &[ctrl(Key::Num1)] },
            ),
            Cmd::ToggleOffCanvas => i(
                "Show Beyond Canvas",
                icons::BORDER,
                "Show or hide the parts of layers outside the canvas.",
                "Content past the canvas edge is kept (it reappears if you enlarge the canvas or \
                 move the layer back) but is not exported. Its visibility is set in \
                 Preferences › Canvas.",
                &[],
            ),
            Cmd::ToggleTheme => i(
                "Dark/Light",
                icons::MOON,
                "Switch between the dark and light color schemes.",
                "The dark scheme is neutral gray, so it does not skew how you judge colors.",
                &[],
            ),
            Cmd::ToggleRibbon => i(
                "Ribbon Size",
                icons::CHEVRON_UP,
                "Switch the ribbon between full and a single line.",
                "Full shows large buttons with labels; a single line takes less room. Tools stay \
                 visible either way.",
                const { &[ctrl(Key::F1)] },
            ),
            Cmd::CycleToolsPlace => i(
                "Move Tools",
                icons::PANEL_LEFT,
                "Move the tools between the ribbon and the sides of the window.",
                "Cycles the tool buttons between the left end of the ribbon, a strip on the left, \
                 and a strip on the right. A side strip leaves the ribbon more room on narrow \
                 screens.",
                &[],
            ),
            Cmd::ResetPanels => i(
                "Reset Panels",
                icons::DOCK,
                "Put the panels back where they started.",
                "Restores the layout of the Layers, History and Tool settings panels.",
                &[],
            ),
            Cmd::Search => i(
                "Search Commands",
                icons::SEARCH,
                "Find any command by name.",
                "Type part of a command's name or description; Enter runs the highlighted one.",
                const { &[ctrl(Key::K)] },
            ),
            Cmd::Help => i(
                "Help",
                icons::HELP,
                "Open the help pages.",
                "F1 while the pointer is over a ribbon button opens the help for its group.",
                const { &[plain(Key::F1)] },
            ),
            Cmd::Shortcuts => i(
                "Keyboard Shortcuts",
                icons::KEYBOARD,
                "List every keyboard shortcut.",
                "Press Alt to show a letter on every ribbon tab and button; typing the letters \
                 runs the command.",
                &[],
            ),
            Cmd::About => i("About", icons::INFO, "Version, license and system information.", "", &[]),
            Cmd::WidgetGallery => i(
                "Widget Gallery",
                icons::WRENCH,
                "Try out shared controls such as the curve editor.",
                "For development: the shared controls the tools will use, to try out on their own.",
                &[],
            ),
        }
    }
}

/// A ribbon tab: its name, the letter its keytip uses, and its groups.
pub struct Tab {
    pub name: &'static str,
    pub groups: &'static [Group],
}

/// A ribbon group: a name, what it is for (for its help page), and its buttons.
pub struct Group {
    pub name: &'static str,
    pub description: &'static str,
    /// The first `large` commands get large buttons; the rest are stacked small ones.
    pub large: usize,
    pub cmds: &'static [Cmd],
}

pub static RIBBON: &[Tab] = &[
    Tab {
        name: "File",
        groups: &[
            Group {
                name: "File",
                description: "Creating, opening, saving and exporting images.",
                large: 3,
                cmds: &[Cmd::New, Cmd::Open, Cmd::Save, Cmd::SaveAs, Cmd::Export, Cmd::Close],
            },
            Group {
                name: "Application",
                description: "Settings for Opal Studio itself.",
                large: 1,
                cmds: &[Cmd::Preferences, Cmd::Exit],
            },
        ],
    },
    Tab {
        name: "Home",
        groups: &[
            Group {
                name: "History",
                description: "Moving back and forth through what you have done. The History \
                              panel shows every step, branches included.",
                large: 2,
                cmds: &[Cmd::Undo, Cmd::Redo],
            },
            Group {
                name: "Layers",
                description: "The most used layer commands. The Layer tab has all of them.",
                large: 1,
                cmds: &[Cmd::NewLayer, Cmd::Duplicate, Cmd::DeleteLayer],
            },
            Group {
                name: "View",
                description: "Seeing the whole image, or its actual pixels.",
                large: 2,
                cmds: &[Cmd::ZoomFit, Cmd::Zoom100],
            },
        ],
    },
    Tab {
        name: "Layer",
        groups: &[
            Group {
                name: "New",
                description: "Adding layers and groups.",
                large: 2,
                cmds: &[Cmd::NewLayer, Cmd::NewGroup, Cmd::Duplicate],
            },
            Group {
                name: "Arrange",
                description: "Changing the order of layers. Dragging in the Layers panel also \
                              moves layers into and out of groups.",
                large: 0,
                cmds: &[Cmd::MoveUp, Cmd::MoveDown],
            },
            Group {
                name: "Combine",
                description: "Combining layers into one.",
                large: 2,
                cmds: &[Cmd::MergeDown, Cmd::Flatten],
            },
            Group {
                name: "Properties",
                description: "Changing the active layer's visibility, protection and name.",
                large: 0,
                cmds: &[Cmd::ToggleVisible, Cmd::ToggleLock, Cmd::Rename],
            },
            Group { name: "Remove", description: "Removing layers.", large: 1, cmds: &[Cmd::DeleteLayer] },
        ],
    },
    Tab {
        name: "View",
        groups: &[
            Group {
                name: "Zoom",
                description: "How large the image is shown. The mouse wheel and the Zoom tool \
                              zoom too.",
                large: 2,
                cmds: &[Cmd::ZoomFit, Cmd::Zoom100, Cmd::ZoomIn, Cmd::ZoomOut],
            },
            Group {
                name: "Canvas",
                description: "What is shown around the canvas.",
                large: 1,
                cmds: &[Cmd::ToggleOffCanvas],
            },
            Group {
                name: "Interface",
                description: "How Opal Studio itself looks.",
                large: 0,
                cmds: &[Cmd::ToggleTheme, Cmd::ToggleRibbon, Cmd::CycleToolsPlace, Cmd::ResetPanels],
            },
        ],
    },
    Tab {
        name: "Help",
        groups: &[
            Group {
                name: "Help",
                description: "Finding out how things work.",
                large: 1,
                cmds: &[Cmd::Help, Cmd::Search, Cmd::Shortcuts, Cmd::About],
            },
            Group {
                name: "Developer",
                description: "For trying out parts of Opal Studio during development.",
                large: 0,
                cmds: &[Cmd::WidgetGallery],
            },
        ],
    },
];

/// Whether `cmd` can run now, and if not, why not (shown on its disabled button).
pub fn availability(st: &AppState, cmd: Cmd) -> Result<(), String> {
    use Cmd::*;
    let needs_doc = || st.active().map(|_| ()).ok_or_else(|| "Open or create an image first.".to_owned());
    match cmd {
        New | Open | Preferences | Exit | ToggleOffCanvas | ToggleTheme | ToggleRibbon | CycleToolsPlace
        | ResetPanels | Search | Help | Shortcuts | About | WidgetGallery => Ok(()),
        Save | SaveAs | Export | Close | ZoomIn | ZoomOut | ZoomFit | Zoom100 | NewLayer | NewGroup | Flatten => {
            needs_doc()
        }
        Undo => {
            let d = st.active().ok_or("Open or create an image first.")?;
            if d.history.can_undo() || d.pending.is_some() { Ok(()) } else { Err("Nothing to undo yet.".into()) }
        }
        Redo => {
            let d = st.active().ok_or("Open or create an image first.")?;
            if d.history.can_redo() {
                Ok(())
            } else {
                Err("Nothing to redo: you have not undone anything since the last change.".into())
            }
        }
        Duplicate | ToggleVisible | ToggleLock | Rename => active_layer(st).map(|_| ()),
        DeleteLayer => {
            let l = active_layer(st)?;
            if l.locked { Err(format!("\"{}\" is locked. Unlock it first.", l.name)) } else { Ok(()) }
        }
        MoveUp | MoveDown => {
            let d = st.active().ok_or("Open or create an image first.")?;
            let l = active_layer(st)?;
            let pos = d.doc().position(l.id).expect("the active layer exists");
            let n = d.doc().siblings(pos.parent).map_or(0, <[_]>::len);
            match cmd {
                MoveUp if pos.index + 1 >= n => Err(format!("\"{}\" is already at the top of its group.", l.name)),
                MoveDown if pos.index == 0 => Err(format!("\"{}\" is already at the bottom of its group.", l.name)),
                _ => Ok(()),
            }
        }
        MergeDown => {
            let d = st.active().ok_or("Open or create an image first.")?;
            let l = active_layer(st)?;
            let pos = d.doc().position(l.id).expect("the active layer exists");
            if pos.index == 0 {
                return Err(format!("There is no layer below \"{}\" in its group.", l.name));
            }
            let below = &d.doc().siblings(pos.parent).expect("its parent exists")[pos.index - 1];
            if below.is_group() {
                Err(format!(
                    "The layer below, \"{}\", is a group; layers can only be merged into a plain layer.",
                    below.name
                ))
            } else if !l.visible || !below.visible {
                Err("Both layers must be visible to merge them.".into())
            } else if l.locked || below.locked {
                Err("One of the layers is locked. Unlock it first.".into())
            } else {
                Ok(())
            }
        }
    }
}

fn active_layer(st: &AppState) -> Result<&opal_core::document::Layer, String> {
    let d = st.active().ok_or("Open or create an image first.")?;
    let id = d.active_layer.ok_or("Select a layer in the Layers panel first.")?;
    d.doc().layer(id).ok_or_else(|| "Select a layer in the Layers panel first.".into())
}

/// For toggle commands, whether the toggle is on.
pub fn checked(st: &AppState, cmd: Cmd) -> Option<bool> {
    match cmd {
        Cmd::ToggleOffCanvas => Some(st.prefs.f64("canvas.off_canvas") > 0.0),
        Cmd::ToggleTheme => Some(!st.palette.dark),
        Cmd::ToggleRibbon => Some(st.prefs.choice("ui.ribbon") == "single"),
        Cmd::ToggleVisible => active_layer(st).ok().map(|l| !l.visible),
        Cmd::ToggleLock => active_layer(st).ok().map(|l| l.locked),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_command_is_listed_once_in_all() {
        let mut v: Vec<String> = Cmd::ALL.iter().map(|c| format!("{c:?}")).collect();
        v.sort();
        let n = v.len();
        v.dedup();
        assert_eq!(v.len(), n);
    }

    #[test]
    fn shortcuts_do_not_collide() {
        let mut seen: Vec<(KeyboardShortcut, Cmd)> = Vec::new();
        for c in Cmd::ALL {
            for s in c.info().shortcuts {
                if let Some((_, other)) = seen.iter().find(|(t, _)| t == s) {
                    panic!("{c:?} and {other:?} share a shortcut");
                }
                seen.push((*s, c));
            }
        }
    }

    #[test]
    fn every_ribbon_command_has_a_description() {
        for tab in RIBBON {
            for g in tab.groups {
                assert!(g.large <= g.cmds.len(), "group {} in {}", g.name, tab.name);
                for c in g.cmds {
                    assert!(!c.info().description.is_empty(), "{c:?}");
                }
            }
        }
    }
}
