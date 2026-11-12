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
    OpenAsLayers,
    Transform,
    TransformAgain,
    TransformAgainCopy,
    ReapplyPlacement,
    ReapplyAllPlacements,
    NewLayer,
    NewGroup,
    Duplicate,
    LayerViaCopy,
    LayerViaCut,
    GroupLayers,
    DeleteLayer,
    MoveUp,
    MoveDown,
    BringToFront,
    SendToBack,
    ReverseLayers,
    SelectAllLayers,
    DeselectLayers,
    AlignLeft,
    AlignHorizontalCenters,
    AlignRight,
    AlignTop,
    AlignVerticalCenters,
    AlignBottom,
    DistributeHorizontalCenters,
    DistributeVerticalCenters,
    DistributeHorizontalSpace,
    DistributeVerticalSpace,
    MergeDown,
    MergeSelected,
    MergeVisible,
    MergeGroup,
    StampVisible,
    Flatten,
    Rasterize,
    Defringe,
    RemoveBlackMatte,
    RemoveWhiteMatte,
    SelectAll,
    Deselect,
    Reselect,
    InvertSelection,
    SelectLayerPixels,
    Clear,
    CropToSelection,
    GrowSelection,
    FeatherSelection,
    SmoothSelection,
    BorderSelection,
    TransformSelection,
    SaveSelection,
    LoadSelection,
    CropToContent,
    Trim,
    RevealAll,
    ResizeImage,
    ResizeCanvas,
    RotateCw,
    RotateCcw,
    Rotate180,
    RotateBy,
    Straighten,
    StraightenLayers,
    ClearMeasurement,
    FlipHorizontal,
    FlipVertical,
    FileInfo,
    ToggleVisible,
    ToggleLock,
    Rename,
    ZoomIn,
    ZoomOut,
    RotateViewLeft,
    RotateViewRight,
    ResetRotation,
    ZoomFit,
    Zoom100,
    ToggleOffCanvas,
    ToggleRulers,
    ToggleSelectionOverlay,
    ToggleTheme,
    ToggleRibbon,
    CycleToolsPlace,
    ResetPanels,
    TileImages,
    TabImages,
    MatchViews,
    LockViews,
    BeforeAfter,
    OpenReference,
    ScreenMode,
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
    pub const ALL: [Cmd; 116] = [
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
        Cmd::OpenAsLayers,
        Cmd::Transform,
        Cmd::TransformAgain,
        Cmd::TransformAgainCopy,
        Cmd::ReapplyPlacement,
        Cmd::ReapplyAllPlacements,
        Cmd::NewLayer,
        Cmd::NewGroup,
        Cmd::Duplicate,
        Cmd::LayerViaCopy,
        Cmd::LayerViaCut,
        Cmd::GroupLayers,
        Cmd::DeleteLayer,
        Cmd::MoveUp,
        Cmd::MoveDown,
        Cmd::BringToFront,
        Cmd::SendToBack,
        Cmd::ReverseLayers,
        Cmd::SelectAllLayers,
        Cmd::DeselectLayers,
        Cmd::AlignLeft,
        Cmd::AlignHorizontalCenters,
        Cmd::AlignRight,
        Cmd::AlignTop,
        Cmd::AlignVerticalCenters,
        Cmd::AlignBottom,
        Cmd::DistributeHorizontalCenters,
        Cmd::DistributeVerticalCenters,
        Cmd::DistributeHorizontalSpace,
        Cmd::DistributeVerticalSpace,
        Cmd::MergeDown,
        Cmd::MergeSelected,
        Cmd::MergeVisible,
        Cmd::MergeGroup,
        Cmd::StampVisible,
        Cmd::Flatten,
        Cmd::Rasterize,
        Cmd::Defringe,
        Cmd::RemoveBlackMatte,
        Cmd::RemoveWhiteMatte,
        Cmd::SelectAll,
        Cmd::Deselect,
        Cmd::Reselect,
        Cmd::InvertSelection,
        Cmd::SelectLayerPixels,
        Cmd::Clear,
        Cmd::CropToSelection,
        Cmd::GrowSelection,
        Cmd::FeatherSelection,
        Cmd::SmoothSelection,
        Cmd::BorderSelection,
        Cmd::TransformSelection,
        Cmd::SaveSelection,
        Cmd::LoadSelection,
        Cmd::CropToContent,
        Cmd::Trim,
        Cmd::RevealAll,
        Cmd::ResizeImage,
        Cmd::ResizeCanvas,
        Cmd::RotateCw,
        Cmd::RotateCcw,
        Cmd::Rotate180,
        Cmd::RotateBy,
        Cmd::Straighten,
        Cmd::StraightenLayers,
        Cmd::ClearMeasurement,
        Cmd::FlipHorizontal,
        Cmd::FlipVertical,
        Cmd::FileInfo,
        Cmd::ToggleVisible,
        Cmd::ToggleLock,
        Cmd::Rename,
        Cmd::ZoomIn,
        Cmd::ZoomOut,
        Cmd::RotateViewLeft,
        Cmd::RotateViewRight,
        Cmd::ResetRotation,
        Cmd::ZoomFit,
        Cmd::Zoom100,
        Cmd::ToggleOffCanvas,
        Cmd::ToggleRulers,
        Cmd::ToggleSelectionOverlay,
        Cmd::ToggleTheme,
        Cmd::ToggleRibbon,
        Cmd::CycleToolsPlace,
        Cmd::ResetPanels,
        Cmd::TileImages,
        Cmd::TabImages,
        Cmd::MatchViews,
        Cmd::LockViews,
        Cmd::BeforeAfter,
        Cmd::OpenReference,
        Cmd::ScreenMode,
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
            Cmd::Exit => i("Exit", icons::EXIT, "Quit Opal Studio.", "Offers to save unsaved changes first.", &[]),
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
            Cmd::OpenAsLayers => i(
                "Open as Layers",
                icons::OPEN_AS_LAYERS,
                "Open image files as layers of this image, one each.",
                "Each file a layer named after it, pixel for pixel, centered on the canvas, above the \
                 active layer, all in one step; with no image open, a new one the size of the first \
                 file. A file that cannot be read is left out, and said. Place puts one file in the \
                 transform box instead, to position and size it.",
                &[],
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
            Cmd::TransformAgain => i(
                "Transform Again",
                icons::TRANSFORM_AGAIN,
                "Do to the selected layers what the last Transform did.",
                "The same move, scale and turn, about the same place on the image: a layer turned \
                 by 30 degrees about a point turns 30 more about it. What the last transform box \
                 applied to layers did (not a paste's placing, nor Transform Selection); an undo \
                 leaves it to do again.",
                const { &[ctrl_shift(Key::T)] },
            ),
            Cmd::TransformAgainCopy => i(
                "Transform Again as a Copy",
                icons::TRANSFORM_AGAIN_COPY,
                "Copy the selected layers and do to the copies what the last Transform did.",
                "Each copy goes just above its layer and becomes the selected one, so that pressed \
                 again, the next copies go on from them: a few presses make a row, or a ring \
                 about the point the transform turned round. One history state each time.",
                const { &[ctrl_shift_alt(Key::T)] },
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
                icons::DUPLICATE,
                "Copy the selected layers.",
                "Each copy goes just above its original. It shares the original's pixels until \
                 one of them is changed, so duplicating is instant and costs no memory.",
                &[],
            ),
            Cmd::LayerViaCopy => i(
                "Layer via Copy",
                icons::LAYER_VIA_COPY,
                "Put what is selected on the selected layers on a layer of its own (Ctrl+J).",
                "What the selected layers show within the selection, as much of each pixel as it is \
                 selected (past the canvas too), becomes a new layer just above the topmost of them. \
                 With nothing selected, the layers are duplicated.",
                const { &[ctrl(Key::J)] },
            ),
            Cmd::LayerViaCut => i(
                "Layer via Cut",
                icons::LAYER_VIA_CUT,
                "Move what is selected on the selected layers onto a layer of its own.",
                "As Layer via Copy, and what it took is erased from the layers it came from, as \
                 much of each pixel as it is selected.",
                const { &[ctrl_shift(Key::J)] },
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
            Cmd::BringToFront => i(
                "Bring to Front",
                icons::TO_FRONT,
                "Move the selected layers to the top of their group.",
                "Each to the top of the group it is in (or of the image), the selected ones keeping \
                 the order they had.",
                // (Shift with ] gives }, as egui names the key.)
                const { &[ctrl_shift(Key::CloseBracket), ctrl_shift(Key::CloseCurlyBracket)] },
            ),
            Cmd::SendToBack => i(
                "Send to Back",
                icons::TO_BACK,
                "Move the selected layers to the bottom of their group.",
                "Each to the bottom of the group it is in (or of the image), the selected ones \
                 keeping the order they had.",
                const { &[ctrl_shift(Key::OpenBracket), ctrl_shift(Key::OpenCurlyBracket)] },
            ),
            Cmd::ReverseLayers => i(
                "Reverse",
                icons::REVERSE,
                "Turn the selected layers' order round.",
                "In the places they hold: the lowest takes the highest's place, and so on. They \
                 must be in one group (or all outside groups).",
                &[],
            ),
            Cmd::SelectAllLayers => i(
                "Select All Layers",
                icons::SELECT_LAYERS,
                "Select every layer.",
                "Every layer outside a group, and every group as a whole (a command acting on a \
                 group acts on all inside it); the topmost becomes the active layer.",
                const { &[ctrl_alt(Key::A)] },
            ),
            Cmd::DeselectLayers => i(
                "Deselect Layers",
                icons::DESELECT_LAYERS,
                "Select no layer.",
                "Commands acting on layers then say to select one first.",
                &[],
            ),
            Cmd::AlignLeft => {
                i("Align Left Edges", icons::ALIGN_LEFT, "Line the selected layers' left edges up.", ALIGN_HELP, &[])
            }
            Cmd::AlignHorizontalCenters => i(
                "Align Horizontal Centers",
                icons::ALIGN_HORIZONTAL_CENTERS,
                "Line the selected layers' centers up on a vertical line.",
                ALIGN_HELP,
                &[],
            ),
            Cmd::AlignRight => {
                i("Align Right Edges", icons::ALIGN_RIGHT, "Line the selected layers' right edges up.", ALIGN_HELP, &[])
            }
            Cmd::AlignTop => {
                i("Align Top Edges", icons::ALIGN_TOP, "Line the selected layers' top edges up.", ALIGN_HELP, &[])
            }
            Cmd::AlignVerticalCenters => i(
                "Align Vertical Centers",
                icons::ALIGN_VERTICAL_CENTERS,
                "Line the selected layers' centers up on a horizontal line.",
                ALIGN_HELP,
                &[],
            ),
            Cmd::AlignBottom => i(
                "Align Bottom Edges",
                icons::ALIGN_BOTTOM,
                "Line the selected layers' bottom edges up.",
                ALIGN_HELP,
                &[],
            ),
            Cmd::DistributeHorizontalCenters => i(
                "Distribute Horizontally",
                icons::DISTRIBUTE_HORIZONTAL,
                "Space the selected layers' centers evenly from side to side.",
                DISTRIBUTE_HELP,
                &[],
            ),
            Cmd::DistributeVerticalCenters => i(
                "Distribute Vertically",
                icons::DISTRIBUTE_VERTICAL,
                "Space the selected layers' centers evenly from top to bottom.",
                DISTRIBUTE_HELP,
                &[],
            ),
            Cmd::DistributeHorizontalSpace => i(
                "Distribute Horizontal Space",
                icons::SPACE_HORIZONTAL,
                "Make the gaps between the selected layers, side to side, all alike.",
                DISTRIBUTE_HELP,
                &[],
            ),
            Cmd::DistributeVerticalSpace => i(
                "Distribute Vertical Space",
                icons::SPACE_VERTICAL,
                "Make the gaps between the selected layers, top to bottom, all alike.",
                DISTRIBUTE_HELP,
                &[],
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
                icons::MERGE_GROUP,
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
            Cmd::Defringe => i(
                "Defringe",
                icons::DEFRINGE,
                "Give the selected layers' edge pixels the colour of the pixels further in.",
                "Asks how wide the fringe is (in pixels). Each pixel that shows and lies that close \
                 to a transparent one takes the colour of the pixels further in, its transparency \
                 kept: a halo of the old background round a cut-out goes. Placed images are \
                 defringed in their own pixels.",
                &[],
            ),
            Cmd::RemoveBlackMatte => i(
                "Remove Black Matte",
                icons::REMOVE_BLACK_MATTE,
                "Take black back out of the selected layers' partly transparent edge pixels.",
                "For an edge anti-aliased against black (a cut-out from a black background, or an \
                 image premultiplied with black): each partly transparent pixel's colour is worked \
                 out as it was before black was blended into it, so no dark rim shows.",
                &[],
            ),
            Cmd::RemoveWhiteMatte => i(
                "Remove White Matte",
                icons::REMOVE_WHITE_MATTE,
                "Take white back out of the selected layers' partly transparent edge pixels.",
                "For an edge anti-aliased against white: each partly transparent pixel's colour is \
                 worked out as it was before white was blended into it, so no light rim shows.",
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
            Cmd::GrowSelection => i(
                "Grow or Shrink",
                icons::GROW,
                "Grow the selection by a distance, or shrink it.",
                "Moves the selection's outline out (or in, by a distance less than 0), its corners \
                 growing round, square or to a point as you choose. It works from the outline (where \
                 pixels are half selected), keeping the edge smooth if it was. It is a step of the \
                 selection's recipe, so Rotate, Flip and Resize Image carry it.",
                &[],
            ),
            Cmd::SmoothSelection => i(
                "Smooth",
                icons::SMOOTH,
                "Smooth the selection's outline over a radius.",
                "Fills pinholes and notches narrower than twice the radius, takes away specks and \
                 spurs as thin, and rounds corners, the outline staying where it is elsewhere. A step \
                 of the selection's recipe.",
                &[],
            ),
            Cmd::BorderSelection => i(
                "Border",
                icons::BORDER_SELECTION,
                "Select a band along the selection's outline.",
                "A band of the width you give along the outline, inside it, outside it, or half each \
                 way, in place of what was selected. A step of the selection's recipe.",
                &[],
            ),
            Cmd::TransformSelection => i(
                "Transform Selection",
                icons::TRANSFORM_SELECTION,
                "Move, size or turn the selection itself in a box.",
                "Puts the transform box round what is selected: drag inside it to move the selection, \
                 a corner or edge to size it, outside it to turn it, or type its numbers; Enter \
                 applies, Esc cancels. Only the selection changes, not the pixels. Its shapes are \
                 carried exactly, and its modifiers (grow, feather, smooth, border) are worked out \
                 again where they are carried to.",
                &[],
            ),
            Cmd::SaveSelection => i(
                "Save Selection",
                icons::SAVE_SELECTION,
                "Keep what is selected under a name, to load back.",
                "Kept in the image, and in its file, steps and all; a name taken already replaces the \
                 selection saved under it. The Selection panel lists the selections saved.",
                &[],
            ),
            Cmd::LoadSelection => i(
                "Load Selection",
                icons::LOAD_SELECTION,
                "Select a saved selection again, or combine it with what is selected.",
                "In place of what is selected (its steps come back, to change in the Selection panel), \
                 added to it, taken away from it, or keeping only what is in both; inverted first, if \
                 you ask. Carried as the image has been turned, flipped, resized or cropped since it \
                 was saved.",
                &[],
            ),
            Cmd::FeatherSelection => i(
                "Feather",
                icons::FEATHER,
                "Soften the selection's edge over a width.",
                "The edge falls off from fully selected to not at all over the width you give, \
                 inside the outline, outside it, or half each way, as a Gaussian blur's edge does or \
                 straight. What acts on the selection then acts on each pixel of the falloff as much \
                 as it is selected.",
                &[],
            ),
            Cmd::Reselect => i(
                "Reselect",
                icons::RESELECT,
                "Select again what was selected when you last deselected.",
                "Brings back the selection Deselect last took away in this image, steps and all, as a \
                 history state of its own.",
                const { &[ctrl_shift(Key::D)] },
            ),
            Cmd::SelectLayerPixels => i(
                "Select Layer's Pixels",
                icons::LAYER_PIXELS,
                "Select the active layer's pixels, each as much as it is opaque.",
                "Selects what the layer holds, however its opacity and visibility are set, a pixel \
                 half transparent selected half. Ctrl+click a layer's thumbnail in the Layers panel \
                 for the same, with Shift to add it to what is selected, Alt to take it away, both \
                 to keep only what is in both.",
                &[],
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
                 canvas too. Inside a selected group, the layers shown are cleared (one hidden or \
                 at 0% opacity is left as it is); a selected layer or group that is hidden or at \
                 0% opacity, or a lock on a layer to clear or a group it is in, refuses it. A layer \
                 holding a placed image keeps that image's pixels as they were placed: rasterize it \
                 first. When nothing you can see changes, the status bar says why: no pixels there, \
                 a layer above covering them (named), or what lies beneath looking the same.",
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
            Cmd::Trim => i(
                "Trim",
                icons::TRIM,
                "Cut the canvas in to the content, by transparency or the colour of a corner.",
                "Asks what the border is: transparent pixels, or the colour of the top-left or \
                 bottom-right corner, within a tolerance (a scan's uneven white margin, say), and \
                 which sides to trim. Rows and columns go from each side while every pixel of them \
                 is border, as the image shows (its visible layers together). Only the canvas \
                 changes: Reveal All brings back what was trimmed.",
                &[],
            ),
            Cmd::RevealAll => i(
                "Reveal All",
                icons::REVEAL_ALL,
                "Grow the canvas to show everything its layers hold, hidden ones too.",
                "What a move or a paste left past the canvas's edges comes back into view. The \
                 canvas only grows: what it holds now stays.",
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
            Cmd::Straighten => i(
                "Straighten",
                icons::STRAIGHTEN,
                "Turn the image so the line measured is level, or plumb.",
                "Measure a line along what should be level or upright (a horizon, a wall) with the \
                 Measure tool (N): Straighten turns the whole image by the least that makes it so, \
                 and crops it to the largest rectangle of its shape within what was there, so no \
                 corner is left empty (or, without the tool's Crop when straightening, grows the \
                 canvas to hold all of it). Layers keep their pixels: cropping only moves the \
                 canvas.",
                &[],
            ),
            Cmd::StraightenLayers => i(
                "Straighten Layers",
                icons::STRAIGHTEN_LAYERS,
                "Turn the selected layers so the line measured is level, or plumb.",
                "Measure a line along what should be level or upright in them with the Measure tool \
                 (N): the selected layers turn about its middle by the least that makes it so, the \
                 canvas as it is, as one history state. The line turns with them.",
                &[],
            ),
            Cmd::ClearMeasurement => i(
                "Clear Measurement",
                icons::CLEAR_MEASUREMENT,
                "Remove the line the Measure tool measured.",
                "Esc removes it too, with the Measure tool chosen. It is not part of the image's \
                 history: nothing to undo.",
                &[],
            ),
            Cmd::FlipHorizontal => i(
                "Flip Horizontally",
                icons::FLIP_H,
                "Mirror the whole image left to right.",
                "Pixel layers are mirrored exactly, without resampling.",
                &[],
            ),
            Cmd::FileInfo => i(
                "File Info",
                icons::FILE_INFO,
                "See how the photograph was taken, and edit its title, caption, creators, copyright and keywords.",
                "What the image's file said of it besides its pixels (the camera's EXIF, IPTC, XMP), kept in \
                 the image and its file and written on export as Export's Metadata setting says. What \
                 describes it is read from and written to all three alike, as the Metadata Working Group's \
                 guidelines say, as one step of history. A template fills in the creators, contact and \
                 copyright fields at once.",
                const { &[ctrl_shift_alt(Key::I)] },
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
            Cmd::RotateViewLeft => i(
                "Rotate View Left",
                icons::ROTATE_VIEW_LEFT,
                "Turn the view a step counterclockwise (the image itself does not change).",
                "One step of the Rotate View tool's settings (15° unless they say otherwise), about \
                 the middle of the view. Reset Rotation puts it straight.",
                &[],
            ),
            Cmd::RotateViewRight => i(
                "Rotate View Right",
                icons::ROTATE_VIEW_RIGHT,
                "Turn the view a step clockwise (the image itself does not change).",
                "One step of the Rotate View tool's settings (15° unless they say otherwise), about \
                 the middle of the view. Reset Rotation puts it straight.",
                &[],
            ),
            Cmd::ResetRotation => i(
                "Reset Rotation",
                icons::STRAIGHT_VIEW,
                "Put the view straight again.",
                "Undoes turning the view, however it was turned (the Rotate View tool, or Rotate View \
                 Left and Right). With the Rotate View tool, Esc does it too.",
                &[],
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
            Cmd::ScreenMode => i(
                "Screen Mode",
                icons::FULL_SCREEN,
                "Show the window full screen, then the image alone, then as usual again (F).",
                "Full screen hides the window's title bar and the taskbar. The image alone hides \
                 the ribbon, the panels and the status bar too, so that it has the whole screen; F \
                 brings them back.",
                const { &[plain(Key::F)] },
            ),
            Cmd::TileImages => i(
                "Tile All",
                icons::TILE_IMAGES,
                "Show each open image in a pane of its own, side by side.",
                "Where their tabs were, laid out to show them largest: three photographs in a wide \
                 window in two rows, three tall images in one. Consolidate to Tabs puts them back. A \
                 tab can also be dragged out into a pane of its own.",
                &[],
            ),
            Cmd::TabImages => i(
                "Consolidate to Tabs",
                icons::TAB_IMAGES,
                "Put the open images back in one set of tabs.",
                "Where the tiled images were, the active one in front.",
                &[],
            ),
            Cmd::MatchViews => i(
                "Match Zoom and Location",
                icons::MATCH_VIEWS,
                "Show the other images at the active one's zoom, on the same place.",
                "Each image shown beside the active one takes its zoom, and shows in the middle of \
                 its view the place of its image (counted from the canvas's corner) the active one \
                 shows in the middle of its own. Lock Views keeps them so.",
                &[],
            ),
            Cmd::LockViews => i(
                "Lock Views",
                icons::LOCK_VIEWS,
                "Keep the images shown at the active one's zoom and place as it moves.",
                "While on, zooming or scrolling the active image does the same to the others shown \
                 beside it, as Match Zoom and Location does once: for going over versions of a \
                 photograph together.",
                &[],
            ),
            Cmd::BeforeAfter => i(
                "Before and After",
                icons::BEFORE_AFTER,
                "Split the view: the image as it was on the left, as it is on the right.",
                "As it was opened or made, unless the History panel's Compare with This State chose \
                 another state. Drag the line to move it; Esc, or this again, ends it.",
                &[],
            ),
            Cmd::OpenReference => i(
                "Open Reference",
                icons::OPEN_REFERENCE,
                "Open an image beside those being edited, to match its look by eye.",
                "In a pane of its own at the right of the images' tabs, there whichever of them is \
                 chosen. It is an image like any other, to be closed when done with.",
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
    /// Whether its buttons show their icons alone (their names in their tooltips), stacked three
    /// to a column, however wide the ribbon: for many commands whose icons say what they do.
    pub icons: bool,
    pub cmds: &'static [Cmd],
}

/// What every Align command's help says.
const ALIGN_HELP: &str = "Each layer goes by the bounds of what it shows (a group by all inside it), and moves by \
                          whole pixels. What they line up with is the Move tool's Align to: the box round the \
                          selected layers (with one selected, the canvas), the canvas, the box round what is \
                          selected, or the active layer, which stays where it is. While the transform box is \
                          open, the box is aligned instead, to the canvas or the selection.";

/// What every Distribute command's help says.
const DISTRIBUTE_HELP: &str = "In the order they lie along the line. Among themselves (Align to: the selected \
                               layers, or the active layer) the outermost two stay, and three are needed; across \
                               the canvas or the selection, the first goes to its one side and the last to the \
                               other, and two are enough. Each layer goes by the bounds of what it shows, and \
                               moves by whole pixels.";

pub static RIBBON: &[Tab] = &[
    Tab {
        name: "File",
        groups: &[
            Group {
                name: "File",
                description: "Creating, opening, saving and exporting images, and placing image files \
                              into them.",
                large: 3,
                icons: false,
                cmds: &[
                    Cmd::New,
                    Cmd::Open,
                    Cmd::Save,
                    Cmd::Place,
                    Cmd::OpenAsLayers,
                    Cmd::SaveAs,
                    Cmd::Export,
                    Cmd::Close,
                ],
            },
            Group {
                name: "Scripts",
                description: "Running Python scripts that drive Opal Studio. What they print \
                              shows in the Console panel.",
                large: 1,
                icons: false,
                cmds: &[Cmd::RunScript, Cmd::ConnectAssistant],
            },
            Group {
                name: "Application",
                description: "Settings for Opal Studio itself.",
                large: 1,
                icons: false,
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
                icons: false,
                cmds: &[Cmd::Undo, Cmd::Redo],
            },
            Group {
                name: "Clipboard",
                description: "Copying and pasting images, here and between programs.",
                large: 1,
                icons: false,
                cmds: &[Cmd::Paste, Cmd::Cut, Cmd::Copy, Cmd::CopyMerged, Cmd::PasteInPlace],
            },
            Group {
                name: "Transform",
                description: "Moving, scaling and rotating layers. The Move tool (V) moves them too.",
                large: 1,
                icons: false,
                cmds: &[
                    Cmd::Transform,
                    Cmd::TransformAgain,
                    Cmd::TransformAgainCopy,
                    Cmd::ReapplyPlacement,
                    Cmd::ReapplyAllPlacements,
                ],
            },
            Group {
                name: "Layers",
                description: "The most used layer commands. The Layer tab has all of them.",
                large: 1,
                icons: false,
                cmds: &[Cmd::NewLayer, Cmd::Duplicate, Cmd::DeleteLayer],
            },
            Group {
                name: "View",
                description: "Seeing the whole image, or its actual pixels.",
                large: 2,
                icons: false,
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
                icons: false,
                cmds: &[Cmd::ResizeImage, Cmd::ResizeCanvas, Cmd::CropToContent, Cmd::Trim, Cmd::RevealAll],
            },
            Group {
                name: "Rotate",
                description: "Turning and mirroring the whole image.",
                large: 2,
                icons: false,
                cmds: &[
                    Cmd::RotateCw,
                    Cmd::RotateCcw,
                    Cmd::Rotate180,
                    Cmd::RotateBy,
                    Cmd::Straighten,
                    Cmd::FlipHorizontal,
                    Cmd::FlipVertical,
                ],
            },
            Group {
                name: "Info",
                description: "What the image's file says of it besides its pixels: how it was taken, and \
                              what describes it.",
                large: 1,
                icons: false,
                cmds: &[Cmd::FileInfo],
            },
        ],
    },
    Tab {
        name: "Select",
        groups: &[
            Group {
                name: "Select",
                description: "What commands act on: all of the canvas, nothing, or the opposite. The \
                              Select tool (M) draws rectangles and ellipses, and single rows and columns.",
                large: 1,
                icons: false,
                cmds: &[Cmd::SelectAll, Cmd::Deselect, Cmd::Reselect, Cmd::InvertSelection, Cmd::SelectLayerPixels],
            },
            Group {
                name: "Layers",
                description: "Which layers commands act on: all of them, or none. Click, Ctrl+click and \
                              Shift+click in the Layers panel choose them one by one.",
                large: 0,
                icons: false,
                cmds: &[Cmd::SelectAllLayers, Cmd::DeselectLayers],
            },
            Group {
                name: "Modify",
                description: "Changing the selection: its edge moved out or in, softened, smoothed or made \
                              a band, or the whole of it moved, sized or turned in a box.",
                large: 0,
                icons: false,
                cmds: &[
                    Cmd::GrowSelection,
                    Cmd::FeatherSelection,
                    Cmd::SmoothSelection,
                    Cmd::BorderSelection,
                    Cmd::TransformSelection,
                ],
            },
            Group {
                name: "Saved",
                description: "Selections kept by name in the image, and its file, to load back.",
                large: 0,
                icons: false,
                cmds: &[Cmd::SaveSelection, Cmd::LoadSelection],
            },
            Group {
                name: "Selected",
                description: "Acting on what is selected, as much of each pixel as it is selected.",
                large: 1,
                icons: false,
                cmds: &[Cmd::Clear, Cmd::Cut, Cmd::CropToSelection, Cmd::LayerViaCopy, Cmd::LayerViaCut],
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
                icons: false,
                cmds: &[Cmd::NewLayer, Cmd::NewGroup, Cmd::Duplicate, Cmd::LayerViaCopy, Cmd::GroupLayers],
            },
            Group {
                name: "Arrange",
                description: "Changing the order of layers. Dragging in the Layers panel also \
                              moves layers into and out of groups.",
                large: 0,
                icons: false,
                cmds: &[Cmd::MoveUp, Cmd::MoveDown, Cmd::BringToFront, Cmd::SendToBack, Cmd::ReverseLayers],
            },
            Group {
                name: "Align",
                description: "Lining the selected layers up, and spacing them evenly, to what the Move tool's \
                              Align to says: the layers themselves, the canvas, the selection, or the active \
                              layer.",
                large: 0,
                icons: true,
                cmds: &[
                    Cmd::AlignLeft,
                    Cmd::AlignHorizontalCenters,
                    Cmd::AlignRight,
                    Cmd::AlignTop,
                    Cmd::AlignVerticalCenters,
                    Cmd::AlignBottom,
                    Cmd::DistributeHorizontalCenters,
                    Cmd::DistributeVerticalCenters,
                    Cmd::DistributeHorizontalSpace,
                    Cmd::DistributeVerticalSpace,
                ],
            },
            Group {
                name: "Combine",
                description: "Combining layers into one. Combining keeps the look exactly.",
                large: 2,
                icons: false,
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
                icons: false,
                cmds: &[Cmd::Rasterize],
            },
            Group {
                name: "Matting",
                description: "Setting right the edge colours of something cut out of one background \
                              and put on another.",
                large: 0,
                icons: false,
                cmds: &[Cmd::Defringe, Cmd::RemoveBlackMatte, Cmd::RemoveWhiteMatte],
            },
            Group {
                name: "Properties",
                description: "Changing the active layer's visibility, protection and name.",
                large: 0,
                icons: false,
                cmds: &[Cmd::ToggleVisible, Cmd::ToggleLock, Cmd::Rename],
            },
            Group {
                name: "Remove",
                description: "Removing layers.",
                large: 1,
                icons: false,
                cmds: &[Cmd::DeleteLayer],
            },
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
                icons: false,
                cmds: &[Cmd::ZoomFit, Cmd::Zoom100, Cmd::ZoomIn, Cmd::ZoomOut],
            },
            Group {
                name: "Rotate",
                description: "Turning the view, not the image, as a drawing is turned on a desk. The \
                              Rotate View tool (R) turns it by dragging.",
                large: 0,
                icons: false,
                cmds: &[Cmd::RotateViewLeft, Cmd::RotateViewRight, Cmd::ResetRotation],
            },
            Group {
                name: "Canvas",
                description: "What is shown on and around the canvas.",
                large: 1,
                icons: false,
                cmds: &[Cmd::ToggleOffCanvas, Cmd::ToggleRulers, Cmd::ToggleSelectionOverlay],
            },
            Group {
                name: "Interface",
                description: "How Opal Studio itself looks.",
                large: 0,
                icons: false,
                cmds: &[Cmd::ScreenMode, Cmd::ToggleTheme, Cmd::ToggleRibbon, Cmd::CycleToolsPlace, Cmd::ResetPanels],
            },
            Group {
                name: "Compare",
                description: "Images side by side, their views matched or locked together, a reference \
                              beside them, and an image before and after.",
                large: 1,
                icons: false,
                cmds: &[
                    Cmd::TileImages,
                    Cmd::TabImages,
                    Cmd::MatchViews,
                    Cmd::LockViews,
                    Cmd::BeforeAfter,
                    Cmd::OpenReference,
                ],
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
                icons: false,
                cmds: &[Cmd::Help, Cmd::Search, Cmd::Shortcuts, Cmd::About],
            },
            Group {
                name: "Developer",
                description: "For trying out parts of Opal Studio during development.",
                large: 0,
                icons: false,
                cmds: &[Cmd::WidgetGallery],
            },
        ],
    },
];

/// Whether `cmd` can run now, and if not, why not (shown on its disabled button).
pub fn availability(st: &AppState, cmd: Cmd) -> Result<(), String> {
    use Cmd::*;
    let needs_doc = || st.active().map(|_| ()).ok_or_else(|| "Open or create an image first.".to_owned());
    // While an edit is under way on the image, nothing may change it, nor save it as it was
    // before (see `edits`); looking at it, copying from it, closing it, and other images, are
    // free, and a paste waits for it.
    if let Some(b) = st.active().and_then(|d| d.busy.as_ref())
        && !matches!(
            cmd,
            New | Open
                | Preferences
                | Exit
                | ConnectAssistant
                | RunScript
                | Copy
                | CopyMerged
                | Paste
                | PasteInPlace
                | Close
                | ZoomIn
                | ZoomOut
                | RotateViewLeft
                | RotateViewRight
                | ResetRotation
                | ZoomFit
                | Zoom100
                | ToggleOffCanvas
                | ToggleRulers
                | ToggleSelectionOverlay
                | ToggleTheme
                | ToggleRibbon
                | CycleToolsPlace
                | ResetPanels
                | TileImages
                | TabImages
                | MatchViews
                | LockViews
                | BeforeAfter
                | OpenReference
                | ScreenMode
                | Search
                | Help
                | Shortcuts
                | About
                | WidgetGallery
        )
    {
        return Err(b.why());
    }
    match cmd {
        New
        | Open
        | OpenAsLayers
        | Preferences
        | Exit
        | ConnectAssistant
        | ToggleOffCanvas
        | ToggleRulers
        | ToggleSelectionOverlay
        | ToggleTheme
        | ToggleRibbon
        | CycleToolsPlace
        | ResetPanels
        | LockViews
        | ScreenMode
        | Search
        | Help
        | Shortcuts
        | About
        | WidgetGallery => Ok(()),
        TileImages => {
            if st.docs.len() < 2 {
                return Err("Open two images or more to tile them.".into());
            }
            Ok(())
        }
        TabImages => needs_doc(),
        MatchViews => {
            needs_doc()?;
            if st.shown.len() < 2 {
                return Err("Show two images side by side first (Tile All).".into());
            }
            Ok(())
        }
        OpenReference => needs_doc(),
        BeforeAfter => {
            let d = st.active().ok_or("Open or create an image first.")?;
            if d.compare.is_none() && d.history.nodes().count() < 2 {
                return Err("The image is as it was opened or made: there is nothing to compare yet.".into());
            }
            Ok(())
        }
        Save | SaveAs | Export | Close | ZoomIn | ZoomOut | ZoomFit | Zoom100 | NewLayer | NewGroup | CopyMerged
        | StampVisible | ResizeCanvas | CropToContent | Trim | RevealAll | Place | SelectAll | RotateViewLeft
        | RotateViewRight => needs_doc(),
        ResetRotation => {
            if st.active().ok_or("Open or create an image first.")?.view.angle == 0.0 {
                return Err("The view is straight already.".into());
            }
            Ok(())
        }
        LoadSelection => {
            let d = st.active().ok_or("Open or create an image first.")?;
            if d.doc().saved.is_empty() {
                return Err("No selection is saved in this image: Save Selection keeps one.".into());
            }
            Ok(())
        }
        Deselect | InvertSelection | CropToSelection | GrowSelection | FeatherSelection | SmoothSelection
        | BorderSelection | TransformSelection | SaveSelection => {
            let d = st.active().ok_or("Open or create an image first.")?;
            if cmd == TransformSelection
                && d.pixel_selection().is_some_and(|s| s.mask().extent() == opal_core::mask::Extent::Unbounded)
            {
                return Err(crate::transform::NO_BOX.into());
            }
            // The dialog shows what it makes as the image's pending edit: the box's first.
            if matches!(cmd, GrowSelection | FeatherSelection | SmoothSelection | BorderSelection)
                && d.transform.is_some()
            {
                return Err("Apply or cancel the box first.".into());
            }
            d.pixel_selection().map(|_| ()).ok_or_else(|| "Nothing is selected.".to_owned())
        }
        Clear | Cut => crate::select::clear_refused(st.active().ok_or("Open or create an image first.")?),
        Reselect => {
            let d = st.active().ok_or("Open or create an image first.")?;
            let d = d.doc();
            d.deselected.as_ref().map(|_| ()).ok_or_else(|| "Nothing has been deselected in this image.".to_owned())
        }
        SelectLayerPixels => active_layer(st).map(|_| ()),
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
        TransformAgain | TransformAgainCopy => {
            if let Some(d) = st.active() {
                // Applying the box would make it the last transform: which one is meant is
                // for the user to say, by applying or cancelling it.
                if d.transform.is_some() {
                    return Err("Apply or cancel the box first.".into());
                }
                if d.transform_again_map().is_none() {
                    return Err(crate::transform::NOTHING_DONE_YET.into());
                }
            }
            availability(st, Transform)
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
        FileInfo => needs_doc(),
        Straighten | StraightenLayers | ClearMeasurement => {
            let d = st.active().ok_or("Open or create an image first.")?;
            let m = d.measured().ok_or(crate::measure_tool::NOTHING_MEASURED)?;
            if cmd == ClearMeasurement {
                return Ok(());
            }
            if m.straightening() == 0.0 {
                return Err(crate::measure_tool::STRAIGHT_ALREADY.into());
            }
            if cmd == Straighten {
                return availability(st, RotateBy);
            }
            // The box applied at once would end one open, which is the user's to apply or not.
            if d.transform.is_some() {
                return Err("Apply or cancel the box first.".into());
            }
            availability(st, Transform)
        }
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
        Duplicate | LayerViaCopy => selected_layers(st).map(|_| ()),
        LayerViaCut => crate::select::clear_refused(st.active().ok_or("Open or create an image first.")?),
        Defringe | RemoveBlackMatte | RemoveWhiteMatte => {
            let (d, sel) = selected_layers(st)?;
            let layers: Vec<&opal_core::document::Layer> = sel.iter().filter_map(|&l| d.doc().layer(l)).collect();
            if let Some(l) = layers.iter().find_map(|l| locked_for(d.doc(), l)) {
                return Err(format!("\"{}\" is locked. Unlock it first.", l.name));
            }
            if layers.iter().all(|l| l.all_objects().is_empty()) {
                return Err("The layers selected hold no pixels.".into());
            }
            Ok(())
        }
        DeleteLayer => {
            let (d, sel) = selected_layers(st)?;
            match sel.iter().filter_map(|&l| d.doc().layer(l)).find_map(|l| locked_for(d.doc(), l)) {
                Some(l) => Err(format!("\"{}\" is locked. Unlock it first.", l.name)),
                None => Ok(()),
            }
        }
        AlignLeft
        | AlignHorizontalCenters
        | AlignRight
        | AlignTop
        | AlignVerticalCenters
        | AlignBottom
        | DistributeHorizontalCenters
        | DistributeVerticalCenters
        | DistributeHorizontalSpace
        | DistributeVerticalSpace => {
            let how = crate::arrange::Arrange::of(cmd).expect("one of them");
            crate::arrange::refused(st, how, crate::arrange::To::chosen(st))
        }
        BringToFront | SendToBack | ReverseLayers => crate::layer_ops::order_refused(st, cmd),
        SelectAllLayers => {
            let d = st.active().ok_or("Open or create an image first.")?;
            if d.doc().layers.is_empty() { Err("The image has no layers.".into()) } else { Ok(()) }
        }
        DeselectLayers => selected_layers(st).map(|_| ()),
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
pub(crate) fn groups_above(
    doc: &opal_core::document::Document,
    layer: opal_core::document::LayerId,
) -> Vec<&opal_core::document::Layer> {
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
        Cmd::LockViews => Some(st.lock_views),
        Cmd::BeforeAfter => Some(st.active().is_some_and(|d| d.compare.is_some())),
        Cmd::ToggleSelectionOverlay => Some(st.prefs.bool("selection.overlay")),
        Cmd::ToggleTheme => Some(!st.palette.dark),
        Cmd::ToggleRibbon => Some(st.prefs.choice("ui.ribbon") == "single"),
        Cmd::ScreenMode => Some(st.screen != crate::state::ScreenMode::Standard),
        Cmd::ToggleVisible => active_layer(st).ok().map(|l| !l.visible),
        Cmd::ToggleLock => active_layer(st).ok().map(|l| l.locked),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_help_text_has_a_run_of_spaces_left_by_a_lost_line_continuation() {
        let runs = |s: &str| s.contains("   ");
        for cmd in Cmd::ALL {
            let info = cmd.info();
            assert!(!runs(info.description) && !runs(info.help), "{cmd:?}");
        }
        for g in RIBBON.iter().flat_map(|t| t.groups) {
            assert!(!runs(g.description), "{}", g.name);
        }
        for m in crate::api::methods() {
            assert!(m.params.iter().all(|p| !runs(p.help)), "{}", m.name);
        }
        for t in crate::tools::ToolId::ALL {
            let info = t.info();
            assert!(!runs(info.description) && !runs(info.help), "{}", info.name);
            assert!(info.settings.iter().all(|d| !runs(d.help)), "{}", info.name);
        }
        assert!(crate::prefs::PREFS.iter().all(|d| !runs(d.help)));
    }

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
