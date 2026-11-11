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
    RunScript,
    ConnectAssistant,
    Undo,
    Redo,
    Cut,
    Copy,
    CopyMerged,
    Paste,
    PasteInPlace,
    Place,
    Transform,
    ReapplyPlacement,
    ReapplyAllPlacements,
    NewLayer,
    NewGroup,
    Duplicate,
    GroupLayers,
    DeleteLayer,
    MoveUp,
    MoveDown,
    MergeDown,
    MergeSelected,
    MergeVisible,
    MergeGroup,
    StampVisible,
    Flatten,
    Rasterize,
    SelectAll,
    Deselect,
    InvertSelection,
    Clear,
    CropToSelection,
    CropToContent,
    ResizeImage,
    ResizeCanvas,
    RotateCw,
    RotateCcw,
    Rotate180,
    RotateBy,
    FlipHorizontal,
    FlipVertical,
    ToggleVisible,
    ToggleLock,
    Rename,
    ZoomIn,
    ZoomOut,
    ZoomFit,
    Zoom100,
    ToggleOffCanvas,
    ToggleRulers,
    ToggleSelectionOverlay,
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

const fn ctrl_alt(key: Key) -> KeyboardShortcut {
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::ALT), key)
}

const fn ctrl_shift_alt(key: Key) -> KeyboardShortcut {
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT).plus(Modifiers::ALT), key)
}

const fn plain(key: Key) -> KeyboardShortcut {
    KeyboardShortcut::new(Modifiers::NONE, key)
}

impl Cmd {
    pub const ALL: [Cmd; 68] = [
        Cmd::New,
        Cmd::Open,
        Cmd::Save,
        Cmd::SaveAs,
        Cmd::Export,
        Cmd::Close,
        Cmd::Preferences,
        Cmd::Exit,
        Cmd::RunScript,
        Cmd::ConnectAssistant,
        Cmd::Undo,
        Cmd::Redo,
        Cmd::Cut,
        Cmd::Copy,
        Cmd::CopyMerged,
        Cmd::Paste,
        Cmd::PasteInPlace,
        Cmd::Place,
        Cmd::Transform,
        Cmd::ReapplyPlacement,
        Cmd::ReapplyAllPlacements,
        Cmd::NewLayer,
        Cmd::NewGroup,
        Cmd::Duplicate,
        Cmd::GroupLayers,
        Cmd::DeleteLayer,
        Cmd::MoveUp,
        Cmd::MoveDown,
        Cmd::MergeDown,
        Cmd::MergeSelected,
        Cmd::MergeVisible,
        Cmd::MergeGroup,
        Cmd::StampVisible,
        Cmd::Flatten,
        Cmd::Rasterize,
        Cmd::SelectAll,
        Cmd::Deselect,
        Cmd::InvertSelection,
        Cmd::Clear,
        Cmd::CropToSelection,
        Cmd::CropToContent,
        Cmd::ResizeImage,
        Cmd::ResizeCanvas,
        Cmd::RotateCw,
        Cmd::RotateCcw,
        Cmd::Rotate180,
        Cmd::RotateBy,
        Cmd::FlipHorizontal,
        Cmd::FlipVertical,
        Cmd::ToggleVisible,
        Cmd::ToggleLock,
        Cmd::Rename,
        Cmd::ZoomIn,
        Cmd::ZoomOut,
        Cmd::ZoomFit,
        Cmd::Zoom100,
        Cmd::ToggleOffCanvas,
        Cmd::ToggleRulers,
        Cmd::ToggleSelectionOverlay,
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
            Cmd::RunScript => i(
                "Run Script",
                icons::SCRIPT,
                "Run a Python script that drives Opal Studio.",
                "Asks for a .py file and runs it, in its own folder, with the Python named in \
                 Preferences › Scripting. What it prints shows in the Console panel, which can \
                 stop it, and what it changes in an image becomes one step of history, which \
                 one Undo takes back. Scripts use the opal package; python/API.md lists what \
                 they can call.",
                &[],
            ),
            Cmd::ConnectAssistant => i(
                "Connect an Assistant",
                icons::BOT,
                "Set up an AI assistant to drive Opal Studio.",
                "Shows what to put in an assistant's settings (Claude Code, Claude Desktop, Cursor and \
                 others) so that it can use Opal Studio's commands and see the image, through MCP: by \
                 starting opal-studio --mcp, or over HTTP on this computer, with a token.",
                &[],
            ),
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
            Cmd::Cut => i(
                "Cut",
                icons::CUT,
                "Copy what is selected on the selected layers, then erase it.",
                "What the selected layers show within the selection and the canvas goes to the \
                 clipboard, and is erased from them, as much of each pixel as it is selected (what \
                 lies past the canvas, which a copy leaves out, stays). Should the copy not reach the \
                 clipboard, the erasing is taken back.",
                const { &[ctrl(Key::X)] },
            ),
            Cmd::Copy => i(
                "Copy",
                icons::COPY,
                "Copy the selected layers, as they look, to the clipboard.",
                "Copies what the selected layers show within the canvas (and within the selection, \
                 if there is one: a partly selected pixel as much as it is selected), trimmed to \
                 their content, for pasting here or into another program. Pasting back into Opal \
                 Studio keeps full precision, and Paste in Place puts it back where it came from.",
                const { &[ctrl(Key::C)] },
            ),
            Cmd::CopyMerged => i(
                "Copy Merged",
                icons::COPY_MERGED,
                "Copy everything visible within the canvas to the clipboard.",
                "All visible layers combined, as the canvas shows them (within the selection, if \
                 there is one), trimmed to their content.",
                const { &[ctrl_shift(Key::C)] },
            ),
            Cmd::Paste => i(
                "Paste",
                icons::PASTE,
                "Paste the image on the clipboard, then place and size it.",
                "The image goes onto a new layer above the active one and keeps its own pixels, so \
                 it can be resized again later without losing quality. A box around it moves, \
                 scales and rotates it; Enter or the check mark places it, Esc cancels. With no \
                 image open, pasting makes a new image of the clipboard. Images copied from other \
                 programs and image files copied in Explorer both work.",
                const { &[ctrl(Key::V)] },
            ),
            Cmd::PasteInPlace => i(
                "Paste in Place",
                icons::PASTE_IN_PLACE,
                "Paste where the copy came from.",
                "A copy made in Opal Studio goes back exactly where it was copied from; anything \
                 else goes at the canvas's top-left corner. Otherwise like Paste.",
                const { &[ctrl_shift(Key::V)] },
            ),
            Cmd::Place => i(
                "Place",
                icons::PLACE,
                "Place an image file into this image, then position and size it.",
                "The file's image goes onto a new layer above the active one, in the transform box, \
                 as a paste does: drag the box, or type its exact position and size in any unit \
                 (the numbers button by the box). Enter or the check mark places it, Esc cancels. \
                 It keeps its own pixels, so it can be resized again later without losing quality.",
                const { &[ctrl_alt(Key::O)] },
            ),
            Cmd::Transform => i(
                "Transform",
                icons::TRANSFORM,
                "Move, scale and rotate the selected layers with a box.",
                "Drag inside the box to move, a corner or edge to scale, the handle above it (or \
                 anywhere outside it) to rotate. Shift frees the aspect ratio (or locks it, when \
                 the box's lock is open), Ctrl scales from the center, Alt turns snapping off or \
                 on while dragging. Enter or the check mark applies it; Esc cancels. Pixel layers \
                 transformed this way keep their pixels, so they can be transformed again without \
                 losing quality.",
                const { &[ctrl(Key::T)] },
            ),
            Cmd::ReapplyPlacement => i(
                "Re-apply Placement",
                icons::REAPPLY,
                "Place the active layer again as the expressions it was placed by say, now.",
                "A layer placed with numbers that name the canvas or other layers (Logo.right + \
                 5mm, typed into the box's numbers or given by a script) remembers them. Once what \
                 they name has moved, this works them out again and puts the layer (and those \
                 placed with it) where they say now, as one step of history. The box's numbers \
                 list them, under Placed by. Placing the layer with the box again replaces them.",
                &[],
            ),
            Cmd::ReapplyAllPlacements => i(
                "Re-apply All Placements",
                icons::REAPPLY_ALL,
                "Place every layer placed by expressions again, as they say now.",
                "Re-applies each placement in the image, as one step of history, each after those \
                 it is placed relative to: a layer placed by another that is placed by a third \
                 follows both. Layers placed relative to one another in a loop are left as they \
                 are (placed again, each would move the other), as are those that cannot be: \
                 what was left, and why, is told.",
                &[],
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
                 own visibility and opacity. Drag layers onto it in the Layers panel. To put \
                 layers into a new group at once, select them and use Group Layers.",
                &[],
            ),
            Cmd::Duplicate => i(
                "Duplicate Layer",
                icons::COPY,
                "Copy the selected layers.",
                "Each copy goes just above its original. It shares the original's pixels until \
                 one of them is changed, so duplicating is instant and costs no memory.",
                const { &[ctrl(Key::J)] },
            ),
            Cmd::GroupLayers => i(
                "Group Layers",
                icons::GROUP_LAYERS,
                "Put the selected layers into a new group.",
                "The group goes where the topmost of them was. Select several layers with Ctrl+click \
                 or Shift+click in the Layers panel.",
                const { &[ctrl(Key::G)] },
            ),
            Cmd::DeleteLayer => i(
                "Delete Layer",
                icons::DELETE,
                "Remove the selected layers.",
                "Removes the layers (and, for a group, everything in it). Undo brings them back.",
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
            Cmd::MergeSelected => i(
                "Merge Selected",
                icons::MERGE_SELECTED,
                "Combine the selected layers into one.",
                "The result takes the place and name of the topmost of them. Hidden selected layers \
                 are left as they are. When the layers were not next to each other, the ones that \
                 lay between them end up below the result, and the status bar says so.",
                &[],
            ),
            Cmd::MergeVisible => i(
                "Merge Visible",
                icons::MERGE_VISIBLE,
                "Combine every visible layer into one.",
                "The result takes the place of the topmost visible layer. Hidden layers stay, \
                 including those inside groups, in their order.",
                &[],
            ),
            Cmd::MergeGroup => i(
                "Merge Group",
                icons::GROUP,
                "Turn the active group into one pixel layer.",
                "The layer keeps the group's name, opacity and visibility, and looks just as the \
                 group did. Hidden layers inside the group stay, just below it.",
                &[],
            ),
            Cmd::StampVisible => i(
                "Stamp Visible",
                icons::STAMP,
                "Add a layer holding everything visible, combined.",
                "The layers themselves stay as they are: the stamp is a flat copy to retouch on, \
                 placed above the active layer (outside any group).",
                const { &[ctrl_shift_alt(Key::E)] },
            ),
            Cmd::Flatten => i(
                "Flatten Image",
                icons::STACK,
                "Combine all visible layers into one.",
                "Hidden layers are discarded. Undo brings everything back.",
                &[],
            ),
            Cmd::Rasterize => i(
                "Rasterize",
                icons::RASTERIZE,
                "Turn the selected layers' images into plain pixels.",
                "A pasted or transformed image keeps its own pixels so that transforming it again \
                 loses nothing. Rasterizing draws it into ordinary pixels as the canvas shows it, \
                 which makes the file smaller and the layer cheaper to show. It is never \
                 required: everything else works on images as they are.",
                &[],
            ),
            Cmd::SelectAll => i(
                "Select All",
                icons::SELECT_ALL,
                "Select the whole canvas.",
                "Everything within the canvas; what lies past it is not selected. Commands that act \
                 on the selection then act on all of it.",
                const { &[ctrl(Key::A)] },
            ),
            Cmd::Deselect => i(
                "Deselect",
                icons::DESELECT,
                "Select nothing.",
                "Commands then act on whole layers again. Undo brings the selection back.",
                const { &[ctrl(Key::D)] },
            ),
            Cmd::InvertSelection => i(
                "Invert Selection",
                icons::INVERT_SELECTION,
                "Select what is not selected, and nothing of what is.",
                "A partly selected pixel becomes as selected as it was not. What lies past the \
                 canvas becomes selected too.",
                const { &[ctrl_shift(Key::I)] },
            ),
            Cmd::Clear => i(
                "Clear",
                icons::CLEAR,
                "Erase what is selected on the selected layers.",
                "Each pixel is erased as much as it is selected, so a soft edge fades out, past the \
                 canvas too. Inside a selected group, the layers shown are cleared (a hidden one is \
                 left as it is); a selected layer or group that is hidden, or a lock on a layer to \
                 clear or a group it is in, refuses it. A layer holding a placed image keeps that \
                 image's pixels as they were placed: rasterize it first.",
                const { &[plain(Key::Delete)] },
            ),
            Cmd::CropToSelection => i(
                "Crop to Selection",
                icons::CROP,
                "Make the canvas the rectangle round the selection.",
                "Only the canvas changes: no pixels are removed, so enlarging the canvas again \
                 brings back anything cropped away.",
                &[],
            ),
            Cmd::CropToContent => i(
                "Crop to Content",
                icons::CROP_TO_CONTENT,
                "Shrink (or grow) the canvas to fit what the visible layers hold.",
                "Only the canvas changes: no pixels are removed, so enlarging the canvas again \
                 brings back anything cropped away. For a crop of your own, use the Crop tool (C).",
                &[],
            ),
            Cmd::ResizeImage => i(
                "Resize Image",
                icons::RESIZE_IMAGE,
                "Change the image's size, scaling everything in it.",
                "Asks for the new size in pixels or percent, and how to resample. Layers keep \
                 their pixels and are drawn at the new size, so resizing again later loses \
                 nothing, unless you choose to resample them, which makes the file smaller.",
                const { &[ctrl_alt(Key::I)] },
            ),
            Cmd::ResizeCanvas => i(
                "Resize Canvas",
                icons::RESIZE_CANVAS,
                "Change the canvas's size without scaling what is on it.",
                "Asks for the new size and where the old canvas sits in it. Nothing outside the \
                 new canvas is removed: it is only hidden past the edge, and enlarging the canvas \
                 again brings it back.",
                const { &[ctrl_alt(Key::C)] },
            ),
            Cmd::RotateCw => i(
                "Rotate Right",
                icons::ROTATE_CW,
                "Turn the whole image a quarter turn clockwise.",
                "Pixel layers are turned exactly, without resampling.",
                &[],
            ),
            Cmd::RotateCcw => i(
                "Rotate Left",
                icons::ROTATE_CCW,
                "Turn the whole image a quarter turn counterclockwise.",
                "Pixel layers are turned exactly, without resampling.",
                &[],
            ),
            Cmd::Rotate180 => i(
                "Rotate 180°",
                icons::ROTATE_180,
                "Turn the whole image upside down.",
                "Pixel layers are turned exactly, without resampling.",
                &[],
            ),
            Cmd::RotateBy => i(
                "Rotate by Angle",
                icons::ROTATE_BY,
                "Turn the whole image by any angle.",
                "Asks for the angle, and whether to grow the canvas so nothing is cut off. Layers \
                 keep their pixels, so turning again later loses nothing. To straighten a photo \
                 while cropping it, use the Crop tool.",
                &[],
            ),
            Cmd::FlipHorizontal => i(
                "Flip Horizontally",
                icons::FLIP_H,
                "Mirror the whole image left to right.",
                "Pixel layers are mirrored exactly, without resampling.",
                &[],
            ),
            Cmd::FlipVertical => i(
                "Flip Vertically",
                icons::FLIP_V,
                "Mirror the whole image top to bottom.",
                "Pixel layers are mirrored exactly, without resampling.",
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
                "A locked layer cannot be painted on, changed, merged or deleted until it is \
                 unlocked, and neither can a group with a locked layer inside. It can still be \
                 moved up and down the stack.",
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
            Cmd::ToggleRulers => i(
                "Rulers",
                icons::RULER,
                "Show or hide the rulers along the image's view.",
                "Along the top and left of the view, outside the image: in the unit sizes are shown in \
                 (Preferences › Interface, or the menu beside any size), counted from the canvas's \
                 top-left corner at the image's resolution, with the pointer's place marked on both.",
                const { &[ctrl(Key::R)] },
            ),
            Cmd::ToggleSelectionOverlay => i(
                "Selection Overlay",
                icons::OVERLAY,
                "Show what is not selected under red.",
                "Red covers what is not selected, and fades out as pixels are more selected, so a \
                 soft selection's falloff shows; what is fully selected stays clear. How red it is \
                 is set in Preferences › Selection.",
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
                description: "Creating, opening, saving and exporting images, and placing image files \
                              into them.",
                large: 3,
                cmds: &[Cmd::New, Cmd::Open, Cmd::Save, Cmd::Place, Cmd::SaveAs, Cmd::Export, Cmd::Close],
            },
            Group {
                name: "Scripts",
                description: "Running Python scripts that drive Opal Studio. What they print \
                              shows in the Console panel.",
                large: 1,
                cmds: &[Cmd::RunScript, Cmd::ConnectAssistant],
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
                name: "Clipboard",
                description: "Copying and pasting images, here and between programs.",
                large: 1,
                cmds: &[Cmd::Paste, Cmd::Cut, Cmd::Copy, Cmd::CopyMerged, Cmd::PasteInPlace],
            },
            Group {
                name: "Transform",
                description: "Moving, scaling and rotating layers. The Move tool (V) moves them too.",
                large: 1,
                cmds: &[Cmd::Transform, Cmd::ReapplyPlacement, Cmd::ReapplyAllPlacements],
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
        name: "Image",
        groups: &[
            Group {
                name: "Size",
                description: "The size of the image, and of the canvas it is shown on.",
                large: 2,
                cmds: &[Cmd::ResizeImage, Cmd::ResizeCanvas, Cmd::CropToContent],
            },
            Group {
                name: "Rotate",
                description: "Turning and mirroring the whole image.",
                large: 2,
                cmds: &[
                    Cmd::RotateCw,
                    Cmd::RotateCcw,
                    Cmd::Rotate180,
                    Cmd::RotateBy,
                    Cmd::FlipHorizontal,
                    Cmd::FlipVertical,
                ],
            },
        ],
    },
    Tab {
        name: "Select",
        groups: &[
            Group {
                name: "Select",
                description: "What commands act on: all of the canvas, nothing, or the opposite. The \
                              Select tool (M) draws rectangles and ellipses.",
                large: 1,
                cmds: &[Cmd::SelectAll, Cmd::Deselect, Cmd::InvertSelection],
            },
            Group {
                name: "Selected",
                description: "Acting on what is selected, as much of each pixel as it is selected.",
                large: 1,
                cmds: &[Cmd::Clear, Cmd::Cut, Cmd::CropToSelection],
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
                cmds: &[Cmd::NewLayer, Cmd::NewGroup, Cmd::Duplicate, Cmd::GroupLayers],
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
                description: "Combining layers into one. Combining keeps the look exactly.",
                large: 2,
                cmds: &[
                    Cmd::MergeDown,
                    Cmd::MergeSelected,
                    Cmd::MergeVisible,
                    Cmd::MergeGroup,
                    Cmd::StampVisible,
                    Cmd::Flatten,
                ],
            },
            Group {
                name: "Convert",
                description: "Turning editable content into plain pixels.",
                large: 1,
                cmds: &[Cmd::Rasterize],
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
                description: "What is shown on and around the canvas.",
                large: 1,
                cmds: &[Cmd::ToggleOffCanvas, Cmd::ToggleRulers, Cmd::ToggleSelectionOverlay],
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
        New | Open | Preferences | Exit | ConnectAssistant | ToggleOffCanvas | ToggleRulers | ToggleSelectionOverlay
        | ToggleTheme | ToggleRibbon | CycleToolsPlace | ResetPanels | Search | Help | Shortcuts | About | WidgetGallery => Ok(()),
        Save | SaveAs | Export | Close | ZoomIn | ZoomOut | ZoomFit | Zoom100 | NewLayer | NewGroup | CopyMerged
        | StampVisible | ResizeCanvas | CropToContent | Place | SelectAll => needs_doc(),
        Deselect | InvertSelection | CropToSelection => {
            let d = st.active().ok_or("Open or create an image first.")?;
            d.pixel_selection().map(|_| ()).ok_or_else(|| "Nothing is selected.".to_owned())
        }
        Clear | Cut => crate::select::clear_refused(st.active().ok_or("Open or create an image first.")?),
        Paste | PasteInPlace => Ok(()),
        RunScript => match (&st.scripts_file, st.console.running()) {
            (None, _) => Err(st.scripts_unserved.clone().unwrap_or_else(|| crate::console::NOT_SERVING.to_owned())),
            (_, Some(name)) => Err(format!("{name} is running: stop it in the Console panel first.")),
            _ => Ok(()),
        },
        Copy => selected_layers(st).map(|_| ()),
        GroupLayers => {
            // The new group goes where the layers are, and they move into it.
            let (d, sel) = selected_layers(st)?;
            match sel.iter().find_map(|&l| groups_above(d.doc(), l).into_iter().find(|g| g.locked)) {
                Some(g) => Err(format!("\"{}\" is locked: what it holds stays as it is. Unlock it first.", g.name)),
                None => Ok(()),
            }
        }
        ReapplyPlacement => {
            let d = st.active().ok_or("Open or create an image first.")?;
            if d.transform.is_some() {
                return Err("Apply or cancel the box first: its numbers re-apply a placement in it.".into());
            }
            let layer = d.active_layer().filter(|&l| d.doc().layer(l).is_some()).ok_or("Select a layer first.")?;
            if d.doc().layer(layer).is_some_and(|l| l.placed.is_none()) {
                return Err("The active layer was not placed by expressions naming the canvas or other layers.".into());
            }
            // It moves every layer placed with it.
            let (_, layers) = crate::transform::placement_group(d.doc(), layer)?;
            if let Some(l) = layers.iter().filter_map(|&l| d.doc().layer(l)).find_map(|l| locked_for(d.doc(), l)) {
                return Err(format!("\"{}\" is locked. Unlock it first.", l.name));
            }
            Ok(())
        }
        ReapplyAllPlacements => {
            let d = st.active().ok_or("Open or create an image first.")?;
            if d.transform.is_some() {
                return Err("Apply or cancel the box first.".into());
            }
            let mut any = false;
            d.doc().for_each_layer(&mut |l, _| any |= l.placed.is_some());
            if !any {
                return Err("No layer in the image was placed by expressions naming the canvas or other layers.".into());
            }
            Ok(())
        }
        Transform | Rasterize => {
            let (d, sel) = selected_layers(st)?;
            let layers: Vec<&opal_core::document::Layer> = sel.iter().filter_map(|&l| d.doc().layer(l)).collect();
            if let Some(l) = layers.iter().find_map(|l| locked_for(d.doc(), l)) {
                return Err(format!("\"{}\" is locked. Unlock it first.", l.name));
            }
            if cmd == Rasterize && !layers.iter().any(|l| crate::layer_ops::has_image_object(l)) {
                return Err("Nothing to rasterize: the selected layers hold only pixels already.".into());
            }
            if cmd == Transform && layers.iter().all(|l| l.all_objects().is_empty()) {
                return Err("The selected layers are empty.".into());
            }
            Ok(())
        }
        MergeSelected => {
            let (d, sel) = selected_layers(st)?;
            let doc = d.doc();
            let shown: Vec<&opal_core::document::Layer> =
                sel.iter().filter(|&&l| crate::layer_ops::shows_in(doc, l)).filter_map(|&l| doc.layer(l)).collect();
            if shown.len() < 2 {
                return Err("Select two or more visible layers (Ctrl+click in the Layers panel).".into());
            }
            match shown.iter().find_map(|l| locked_for(doc, l)) {
                Some(l) => Err(format!("\"{}\" is locked. Unlock it first.", l.name)),
                None => Ok(()),
            }
        }
        MergeVisible => {
            let d = st.active().ok_or("Open or create an image first.")?;
            let visible: Vec<_> = d.doc().layers.iter().filter(|l| l.visible).collect();
            if visible.len() < 2 && visible.iter().all(|l| !l.is_group()) {
                return Err("There is only one visible layer.".into());
            }
            match visible.iter().find_map(|l| first_locked(l)) {
                Some(l) => Err(format!("\"{}\" is locked. Unlock it first.", l.name)),
                None => Ok(()),
            }
        }
        MergeGroup => {
            let l = active_layer(st)?;
            if !l.is_group() {
                return Err(format!("\"{}\" is not a group.", l.name));
            }
            let doc = st.active().expect("one with an active layer").doc();
            match locked_for(doc, l) {
                Some(l) => Err(format!("\"{}\" is locked. Unlock it first.", l.name)),
                None => Ok(()),
            }
        }
        // (Resize Image may give a new resolution alone, which changes no layer; a new size is
        // refused while one is locked, by `resize_image`, through `locked_layer`.)
        ResizeImage => needs_doc(),
        RotateCw | RotateCcw | Rotate180 | RotateBy | FlipHorizontal | FlipVertical => {
            let d = st.active().ok_or("Open or create an image first.")?;
            match d.doc().layers.iter().find_map(|l| first_locked(l)) {
                Some(l) => Err(format!("\"{}\" is locked, and this changes every layer. Unlock it first.", l.name)),
                None => Ok(()),
            }
        }
        Flatten => {
            let d = st.active().ok_or("Open or create an image first.")?;
            match d.doc().layers.iter().find_map(|l| first_locked(l)) {
                Some(l) => Err(format!("\"{}\" is locked. Unlock it first.", l.name)),
                None => Ok(()),
            }
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
        ToggleVisible | ToggleLock | Rename => active_layer(st).map(|_| ()),
        Duplicate => selected_layers(st).map(|_| ()),
        DeleteLayer => {
            let (d, sel) = selected_layers(st)?;
            match sel.iter().filter_map(|&l| d.doc().layer(l)).find_map(|l| locked_for(d.doc(), l)) {
                Some(l) => Err(format!("\"{}\" is locked. Unlock it first.", l.name)),
                None => Ok(()),
            }
        }
        MoveUp | MoveDown => {
            let d = st.active().ok_or("Open or create an image first.")?;
            let l = active_layer(st)?;
            if let Some(g) = groups_above(d.doc(), l.id).into_iter().find(|g| g.locked) {
                return Err(format!("\"{}\" is locked: what it holds stays as it is. Unlock it first.", g.name));
            }
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
            } else if let Some(locked) = locked_for(d.doc(), l).or(first_locked(below)) {
                Err(format!("\"{}\" is locked. Unlock it first.", locked.name))
            } else {
                Ok(())
            }
        }
    }
}

/// The name of a layer of `doc` that is locked, or holds one that is, if any.
pub fn locked_layer(doc: &opal_core::document::Document) -> Option<String> {
    doc.layers.iter().find_map(|l| first_locked(l)).map(|l| l.name.clone())
}

/// The groups `layer` of `doc` is in, the one holding it first.
pub(crate) fn groups_above(doc: &opal_core::document::Document, layer: opal_core::document::LayerId) -> Vec<&opal_core::document::Layer> {
    let mut out = Vec::new();
    let mut above = doc.position(layer).and_then(|p| p.parent);
    while let Some(g) = above.and_then(|g| doc.layer(g)) {
        out.push(g);
        above = doc.position(g.id).and_then(|p| p.parent);
    }
    out
}

/// What keeps `layer` of `doc` from being changed, if anything: a locked group it is in
/// (what a group holds is part of it), the layer itself if locked, or a locked layer inside
/// it.
pub(crate) fn locked_for<'a>(
    doc: &'a opal_core::document::Document,
    layer: &'a opal_core::document::Layer,
) -> Option<&'a opal_core::document::Layer> {
    groups_above(doc, layer.id).into_iter().find(|g| g.locked).or_else(|| first_locked(layer))
}

/// The layer itself if it is locked, or else the first locked layer inside it.
pub(crate) fn first_locked(layer: &opal_core::document::Layer) -> Option<&opal_core::document::Layer> {
    if layer.locked {
        return Some(layer);
    }
    layer.children()?.iter().find_map(|c| first_locked(c))
}

/// The active document and its selected layers (see `DocState::selected_layers`).
fn selected_layers(st: &AppState) -> Result<(&crate::doc::DocState, Vec<opal_core::document::LayerId>), String> {
    let d = st.active().ok_or("Open or create an image first.")?;
    let sel = d.selected_layers();
    if sel.is_empty() { Err("Select a layer in the Layers panel first.".into()) } else { Ok((d, sel)) }
}

fn active_layer(st: &AppState) -> Result<&opal_core::document::Layer, String> {
    let d = st.active().ok_or("Open or create an image first.")?;
    let id = d.active_layer().ok_or("Select a layer in the Layers panel first.")?;
    d.doc().layer(id).ok_or_else(|| "Select a layer in the Layers panel first.".into())
}

/// For toggle commands, whether the toggle is on.
pub fn checked(st: &AppState, cmd: Cmd) -> Option<bool> {
    match cmd {
        Cmd::ToggleOffCanvas => Some(st.prefs.bool("canvas.off_canvas_shown")),
        Cmd::ToggleRulers => Some(st.prefs.bool("canvas.rulers")),
        Cmd::ToggleSelectionOverlay => Some(st.prefs.bool("selection.overlay")),
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
