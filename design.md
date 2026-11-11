# Opal Studio — design

This file records what was decided in the design discussions that began on 2026-11-04, and
is kept current as features are built; `roadmap.md` says which milestone each feature
belongs to and which are done. Items marked **TBD** are deliberately undecided and get
settled when the milestone that needs them starts.

## Goal

A 2D graphics editor that is more intuitive than Photoshop, GIMP and Affinity, even for the
most basic tasks, and that grows until it is generally useful for photo manipulation and
graphic design.

- **Vector and raster in one document, with no mode to choose.** Affinity (free since
  October 2025) merges its three apps but still makes you switch between Vector, Pixel and
  Layout studios; Opal Studio has no studios and no personas.
- **Free and open source, GPL-3.0-or-later** (`LICENSE`). This lets the project use GPL
  libraries (Potrace, SolveSpace's solver) and borrow code from GIMP, Krita and Inkscape.
  MIT and Apache-2.0 code (the colorpicker, SAM 2.1, Graphite) is compatible.

## Principles

1. **Everything stays editable.** Pixels are produced only when an operation forces it. No
   tool ever refuses to work or asks "rasterize this layer?": a brush on a vector shape makes
   an editable stroke, a blur on text is a live effect, erasing part of text adds a mask.
2. **No hidden state without a visible sign.** When an action has no visible effect, the
   editor says why (see *Diagnostics*).
3. **New things get their own layer.** Pastes, text and shapes go on a new layer by default,
   with a visible control to put them on the active layer instead.
4. **Discoverable without documentation.** The ribbon shows commands grouped and labeled,
   command search finds any action by name, and every task on the *Basic-tasks checklist*
   must be doable by a first-time user.
5. **No hidden constants.** Every value that affects an operation's result can be adjusted,
   in advanced settings if not in the basic ones (see *Adjustable settings*).

## Adjustable settings

- **What is exposed:** every threshold, falloff, kernel, weight, response curve, range and
  formula choice that an operation uses. This covers values other editors hardcode, such as
  the shape of a brush's hardness falloff, the tonal ranges of dodge and burn, or the luma
  weights behind the Luminosity blend mode. Implementation constants that do not change
  results (tile size, cache sizes) are not settings.
- **Basic and advanced.** Each tool and operation shows its few main settings; the rest sit
  in an *Advanced* section, collapsed by default (its open/closed state is remembered).
- **Every setting shows its default, a changed setting is marked, and each can be reset.** A
  tool's settings can be saved as named presets.
- **Ranges and curves over color are edited in a color model the user picks**, with the
  channel curve editor below, wherever the value is about color.

### Channel curve editor

One shared component, used for color-range selection, color matching, the Adjust brush's
tonal range, Blend If, the Channels blend mode, and any other setting defined over the
channels of a color model.

- Pick a color model: any the color engine supports.
- One curve per channel, mapping the channel's value to an amount from 0 to 1. Control
  points can be added, dragged and removed.
- The histogram of the relevant pixels is drawn behind each curve; hue channels wrap around.
- Presets, and reset.

The same component, without a color model, edits every other curve: brush dynamics, tip
falloff, feather falloff, and the falloff of distance-based color matching.

## Tech stack

| Concern | Choice |
|---|---|
| Language | Rust |
| GPU | wgpu. Free GPU memory is queried directly from the OS: DXGI `QueryVideoMemoryInfo` on Windows, `VK_EXT_memory_budget` on Vulkan (wgpu does not expose it). |
| UI | egui, with egui_tiles for the dockable panel layout (egui_dock was planned, but it lags a version behind egui); restyled from egui's default look |
| Windowing and input | winit. Its current release reports pen pressure on Windows only and without tilt or the eraser end, so Opal Studio handles Windows' `WM_POINTER` messages itself for those; macOS and Linux pen input come later. |
| ML inference | ONNX Runtime, run locally |
| Color management | lcms2 (ICC profiles) |
| Text shaping and layout | **TBD** (a HarfBuzz-class shaper with bidi, line breaking and font fallback) |
| Vector rendering | **TBD** (vello, tiny-skia, or our own) |
| Tests | egui's test harness, egui_kittest (simulated input, rendering with wgpu to an image); unit tests for everything below the interface |
| Memory allocator | mimalloc. A tile is 512 KiB, just past the size from which Windows' own heap returns each block to the system when freed, so with it every new tile costs fresh pages, and the page faults serialize the threads compositing in parallel. |
| Toolchain on Windows | the MSVC target (`.cargo/config.toml`). With the MinGW toolchain the app's test binary crashed at startup in a static initializer the GNU linker left unrelocated. |

egui and wgpu also compile to WebAssembly, so a browser build remains possible later; it is
not a goal.

## Document model

- A **document** has a **canvas** (later with a color profile) and a tree of **layers**.
  The canvas is a rectangle in the document's coordinates: what is exported, and how big
  the image is. Cropping and resizing the canvas move or resize that rectangle and change
  no pixels, so what falls outside can be brought back by enlarging the canvas again.
  Coordinates shown to the user count from the canvas's top-left corner.
- **Resolution and units.** A document also has a **resolution**, in pixels per inch
  (from 0.01 to 1,000,000; 72 unless a file or the user says otherwise): how large its
  pixels print. Sizes are always kept in pixels; everything else is a way of showing them.
  The user picks a **unit** (`opal_core::units`: pixels, points, picas, inches, feet, yards,
  millimeters, centimeters, meters; Preferences › Interface, or the menu in any size
  dialog or box's fields, which all share it) and the resolution converts it; a length
  typed with a unit of its own ("21 cm") is read in that unit. A field writes back only
  what the user changes, so showing a length in another unit never moves it. Resolution
  shows as pixels per inch or per centimeter (a preference). The resolution is part of
  the history (Resize Image changes it), read from PNG (`pHYs`) and JPEG (JFIF density,
  else EXIF resolution) files on opening, written into PNG and JPEG exports, and kept in
  `.opal` files. A file's per-meter or per-centimeter density reads as the round number
  its writer most likely meant (2,835 pixels per meter is 72 ppi). Pixels are square: a
  file whose aren't takes the resolution across, and says so. Where it comes from: the
  file; New Image's field (remembered; its paper and photo sizes at 300 ppi); for a file
  that does not say, a preference (72 by default; 96 is Windows' habit); for a paste made
  a new image, what the clipboard says (a PNG's, or the image a copy made here came from),
  else that preference. The status bar shows it beside the size, and clicking it opens
  Resize Image, where it is changed.
- A **layer** is either a **group**, holding child layers, or holds **objects**: pixel
  objects and image objects (below), and later vector paths, text, brush strokes and
  adjustments. A group composites its children by themselves first, then applies its own
  visibility and opacity. Layers have **no boundary**, not even the canvas edge: content
  past the canvas is kept, shown dimmed, and left out of exports.
- **Pixel objects and image objects.** Bitmaps come in two kinds.
  - A **pixel object** is a tiled bitmap on the document's pixel grid. It moves only by
    whole pixels, so moving it never resamples it.
  - An **image object** (a pasted image, or a pixel object transformed freely) keeps its
    own pixels and draws them through a transform: an affine map (position, scale,
    rotation; skew later) and a resampling method of its own (see *Resampling*).
    Transforming it again redraws from those pixels, so any number of transforms loses
    nothing. Rasterize Layer turns it into a pixel object; pasting into an existing layer
    bakes it into that layer's pixels at once.
  - Whole-image operations (Resize Image, Rotate, Straighten) keep the originals the same
    way: pixel objects become image objects, except where the change is exact on the pixel
    grid (quarter turns and flips), which pixel objects undergo directly. Resize Image can
    resample instead, when a smaller file is the point.
  - How painting treats an image object is **TBD** (milestone 3): painting into its own
    pixels through the transform, or onto a pixel object above it in the same layer.
- **Identifiers.** Layers and objects get identifiers that stay unique across every history
  branch, so the generator lives beside the history, not inside a snapshot (two branches
  that each add a layer must not reuse an identifier). The `.opal` file stores the next
  free one.
- **Tiles.** Raster content is stored in 256×256-pixel tiles, and only tiles with content
  exist. Tiles are copy-on-write: an edit copies only the tiles it touches, and untouched
  tiles are shared by every history state and every branch. An image keeps its tiles in
  chunks of 16 × 16 places, each shared between the image's copies until one changes a tile
  in it, so the copy an edit makes of a layer costs a pointer a chunk rather than one a tile
  (a 300,000-pixel canvas: 0.2 ms, against 12 ms). Tiles are a storage detail and never
  appear in the UI.
- **Pixels** are RGBA half floats (8 bytes): linear light, sRGB/Rec.709 primaries with a
  D65 white, premultiplied by alpha, and extended range (out-of-gamut colors are stored as
  values below 0 or above 1, as scRGB does). Half floats keep about 11 significant bits at
  every brightness, more than 8-bit sRGB anywhere, and GPUs sample them natively.
  Computation converts to single precision. A tile whose pixels are no finer than an
  8-bit file's (an 8-bit PNG or JPEG opened, an 8-bit image pasted) keeps them as the
  8-bit sRGB values with straight alpha they came from, in half the memory, and reads as
  exactly the working pixels converting them makes (through a table, one lookup a
  channel); copies that move pixels without changing them (quarter turns, flips,
  cropping) stay 8-bit, and changing a tile makes it working pixels.
- **Tiles go to disk past a budget** (`opal_core::store`): the pixels of open images may
  take a share of the computer's memory (Preferences › Memory, 60% by default), and while
  work that needs memory runs (opening, pasting, copying, exporting, merging, drawing into
  pixels), no more than they took when it began and what was free then, less what the work
  takes in one go (`store::hold`, held by the memory check that admits the work, from the
  same measure, and lowered as the work sets more aside): past either, the tiles read least
  recently go to a scratch file in the temporary folder (deleted when the program ends) and
  come back when next read. The sum of what tiles took and what was free stays right
  however late the cap is applied, as tiles made since only moved memory from one to the
  other; it is measured once a piece of work, not every frame, since tiles written to the
  file sit in the system's cache for a while, uncounted as free, and a cap measured again
  would send more after them. Outside such work the system pages as it does. A cap takes
  away at once only what is past itself; past the budget alone, tiles are left to the
  store's thread. A thread taking memory past the limit sends as much as it took to disk itself, so
  memory never runs ahead of it however fast tiles are made (bringing nothing, a tile
  changed where it lies, it neither sends nor waits); a thread of the store's own takes
  memory down a tenth below it (at most 256 MB), a batch of 32 MB at a time, asking the
  limit again as it goes, so that a lower limit, or a tile brought back by the interface,
  never holds up the thread that crossed it. The order is kept in a queue, made by sorting the
  tiles by when each was last read (read once each, as tiles read meanwhile change it) and
  made again when used up, skipping tiles read since. A tile being read or changed never
  goes meanwhile (each tile's pixels sit behind a read-write lock, which a thread may read
  more than once, as one image's tiles may all be one; loops over many rows hold a tile's
  `Reader` once). A tile brought back keeps its copy on disk while unchanged, so it can go
  again without being written. The scratch file is kept in blocks the size of a tile of
  working pixels, each holding one or two 8-bit tiles, so space let go by either kind
  serves the other; blocks let go at its end are given back to the disk. The status bar
  says how much is on disk; if writing there fails (a full disk), tiles are no longer
  written, those with a copy there still go, and the user is told once.
- **Compositing.** The CPU reference compositor flattens the layers into the
  **projection**, cached per tile, with a pyramid of half-size levels for zoomed-out views.
  A cached tile keeps the list of what produced it: source tiles (as pointers, with the
  offset of pixels moved off the tile grid), each image object's pixels, transform and
  sampling, and opacities. It is recomputed exactly when that list changes, so nothing has
  to report changes to the cache. Stale tiles are computed in parallel, from a thread each
  document has for its projection, on the pool of background threads (which saves, exports
  and copies use too, so a small parallel job of the interface's never waits behind any of
  them): the canvas asks for the view it shows and waits for it
  only briefly (20 ms, on the frame that asks), drawing the last view done meanwhile (the
  previous zoom, or the image before a change), so a huge image never holds up the
  interface; a view behind for more than 150 ms says so, with a spinner and "Drawing…" in
  the canvas's corner. Keys name tiles and
  images by weak reference (compared by address, which cannot be reused while named), and
  a layer's own tile shown as it is is held weakly too, so the cache never keeps alive
  what the document has let go of. A pyramid tile is
  keyed on the versions of the four below it (a version names what a tile shows), so the
  pixels of a tile can be let go while its key stays. Zoomed out, the tiles are worked out
  from the level shown down: a tile over a **uniform block**, where each pixel object
  repeats one stored tile (or none) and no image object reaches, would have every level-0
  tile beneath it alike, so it is made from one of them, halved to its level, and keyed on
  that tile's key, with nothing cached beneath it. Only the tiles over mixed blocks are
  made from the four below, down to level 0. Whether a block is uniform, and whether a
  tile has content under it at all, is asked of the pixel objects' own tiles (a large
  area of an index each image keeps, built on first use, of its tile rows as runs of one
  tile, at a lookup a row) and of each image object (the area carried back into its
  image's pixels, asked of the same index), so the view lists
  no tiles beneath a uniform block: a huge, mostly blank canvas costs about what the
  screen shows (a 300,000-pixel white canvas shown whole: 17 ms after an edit, 10 MB).
  Zoomed out, where the image objects' full-resolution tiles in view (the tiles each
  crosses, or, where its image's tiles leave much of it empty, those its runs of tiles
  cross) would together take more than half the cache's budget (1 GB without one), the
  largest of them, until the rest fit, have the tiles they draw into drawn at the level
  shown instead, as a preview draws them (each object from its own copy at the level: a
  pixel object's reduced tiles, an image object's pyramid with its own sampling, but
  Nearest by area: each pixel the average of what its footprint covers, which is what
  reducing it from full resolution gives), kept while their key holds. An object drawn so stays so
  until the rest would fit in half that, so that panning or zooming near the limit does
  not switch it back and forth, making all of it in view at full resolution each time.
  The reduced tiles those tiles read stay, at that level alone, kept by image (so layers
  showing one image share them); when a pixel object's image changes, its new image's
  start from its last one's, but for those over a tile the change touched (told apart by
  the two images' rows of tiles, which an edit shares where it leaves them alone), so an
  edit beneath makes again only the reduced tiles over what it changed, each from full
  resolution (a tile a layer repeats is halved once a level, and its halvings kept, and
  counted against the budget, while a reduced tile kept is one of them). So an image object
  across a huge canvas costs the tiles shown too (one across a 300,000-pixel canvas shown
  whole: 80 ms; 4 ms when its opacity changes, 9 ms
  when a pixel of the canvas beneath does). Those tiles
  look very slightly different from reducing full resolution where layers overlap, and a
  pixel layer beneath them, off the level's pixel grid, slightly softer; everywhere else
  the view is exact. The cache
  may hold a set amount of memory (a preference, 2 GB by default), counting each tile it
  made once (a layer's own tile shown as it is costs nothing); past it, the tiles farthest
  from the view go first,
  then the pixels of the finer levels under the view, which are made again, locally, only
  when an edit or a zoom needs them. Entries without pixels (keys, a layer's own tiles)
  cost little, but are bounded too, at four times as many as the budget holds in tiles,
  farthest first. The tiles the view shows always stay. The GPU only
  shows projection tiles; a GPU
  compositor matching the reference comes when adjustments need interactive speed
  (milestone 5).
- **Display.** Each projection tile is uploaded as a 258×258 texture: the tile plus a
  one-pixel border from its neighbors, so filtering never shows seams. Below 100% the
  level just finer (or, by preference, just coarser) than the screen is used; at and above
  a set zoom (200% by default) pixels are sharp squares. The viewport is aligned to device
  pixels, so 100% maps pixels one to one, and the shader composites over the checkerboard
  inside the canvas and dims what lies beyond it. Textures not drawn for 600 frames are
  freed, and so are the least recently drawn beyond half the GPU memory the system allows
  the process (1 GB where it does not say); those drawn in the frame always stay.
- The default perceptual color space, for edge detection, color-range selection and
  vectorization, is **OKLab**.

### Resampling

One resampler draws image objects, and serves Resize Image, Rotate, export at a chosen
size, and Rasterize Layer, so each gives the same pixels the canvas shows.

- **Methods:** Nearest (hard pixels, for pixel art), Bilinear, Bicubic (the
  Mitchell–Netravali family, with B and C adjustable; Catmull–Rom by default) and Lanczos
  (2 to 4 lobes). Bicubic is the default.
- **Anti-ringing**, on by default: each result is kept within the range of the source
  pixels nearest it, which removes the halos sharp filters draw along hard edges.
- **Shrinking never aliases.** An image keeps reduced copies (each a power of two
  smaller, made by halving), built when first needed and only those needed (a quarter of
  the image's memory at half size, a sixteenth at a quarter; tiles made from the same four
  tiles are made once and shared, so a uniform area costs a tile at any level); a shrunk image is sampled from the copy just finer than the result along
  its less shrunk axis, with the filter widened by the remaining factor along each axis
  (up to 64 times for a drawing that is not turned, so squashing one axis leaves the
  other's detail alone; only a squash past that ratio takes a coarser copy).
- **Exact cases are exact:** moves by whole pixels, quarter turns and flips copy pixels
  without filtering; tiles copied from the same stored tiles come out alike and are made
  once (a flipped white canvas is still a few tiles). Drawing visits only the tiles some
  stored tile reaches, not every tile of the bounding box.
- While a transform is being dragged, Bilinear is used so the drag keeps up (a setting);
  the object's own method takes over on release.

## Selections

### The mask

A selection is a per-pixel value from 0 (not selected) to 1 (fully selected). Every
operation (filters, adjustments, painting, fills) computes its result and blends it in:

```
new pixel = old pixel × (1 − m) + changed pixel × m        (m = the mask value there)
```

Because every operation goes through this one step, limiting an operation to the selection
and soft (feathered) selections work for all operations without per-operation code. The
marching ants show where the mask crosses a threshold (default 0.5, adjustable:
Preferences › Selection, *Marching ants at*). **View › Selection Overlay** shows the mask as
red over what is not selected, fading out as pixels are more selected (`mask_view`): over the
canvas only, uniform stretches as rectangles, tiles with levels of their own as textures (one
for alike tiles, which share their levels, held weakly; a tile of a level a column or a row,
one line of texture), or as their mean level once smaller on screen than a few points; how
red, a preference.

- **Storage** (`opal_core::mask`). The mask covers the whole plane, as layers do: 16-bit
  levels in 256×256 tiles on the images' grid, sparse, so that a selection across a huge
  canvas costs its edges, not its area. A tile not kept has the mask's *outside* level (0,
  or 1 once inverted, so that a selection can be the plane but for a hole); tiles side by
  side all of one level are kept as one run along their row of tiles; a tile whose levels
  change from column to column alone, or from row to row (along a straight edge on the
  pixel grid), keeps a level a column or a row, alike ones (along one side) sharing them;
  only the rest keep a level a pixel. A Select All on a canvas 300,000 pixels a side takes a
  few hundred kilobytes. A mask keeps no tile past `MASK_REACH` tiles from the origin (33.5
  million pixels either way, the pixels past that at its level outside): every mask is cut
  to it as it is made, so that every one can be written to a file and read back, and what
  makes a mask of a shape or outline, or carries one, works nothing out past it, however far
  the shape reaches. A shape's or an outline's mask is made within the memory free: each row
  of tiles admits its edge tiles, at a level a pixel, before they are worked out (giving back
  what they come to less), and an outline its flattened points and its edges' lists by row
  of tiles before making them (`Mask::of_within`, `Outline::filled_within`); a tool's or a
  script's new selection past that is refused, saying so (`DocState::select_with`), and one
  worked out again in an edit (a step changed, the selection carried) ends the edit so. Copies share tiles until one changes, and combining keeps sharing a
  tile the other mask leaves as it is (adding nothing to it, say); history counts a state's
  masks (the one its selection comes to, and those its mask steps keep) with its layers'
  tiles. Operations
  through the mask skip the tiles it leaves wholly alone, and Clear drops those it selects
  wholly, without looking at their pixels.
- **Shapes** (rectangles and ellipses, turned or not) give each pixel the part of it they
  cover, worked out from the exact distance to their edge (an ellipse's by bisection), or,
  with smoothing off, all or nothing by the pixel's middle. Tiles wholly inside or outside
  are told from their middles' distance, so only the tiles the edge crosses are worked out
  pixel by pixel.
- **Combining** soft masks: *add* keeps the greater level, *intersect* the lesser, and
  *subtract* the lesser of the level before and the opposite of the new one; so adding a
  selection to itself changes nothing.
- **The outline** (the marching ants' path) runs along the pixels' edges, round those at the
  threshold or above, as Photoshop's and GIMP's do: at high zoom, where pixels are sharp
  squares, it follows them. Pixels meeting only at a corner have lines of their own. It is
  worked out where tiles differ and within those with levels of their own, an edge along
  tiles of one level taken whole, so it costs its length, not the area it goes round.
- `.opal` files keep the selections saved in the image (*Saved selections*), not what is
  selected.

### Commands

On the Select tab (and for scripts, by the same names):

- **Select All** (Ctrl+A) selects the canvas, as Photoshop's does: what lies past it is
  not selected. **Deselect** (Ctrl+D), or a click with the Select tool. **Reselect**
  (Ctrl+Shift+D) brings back what either last took away in the image, steps and all
  (`Document::deselected`: kept with each history state, its masks counted in history's
  memory, not saved). It is not worked out again as the image is turned, flipped, resized or
  cropped: it keeps how (`Deselected::since`), and Reselect carries it so once, off the
  interface thread, memory measured as it goes. **Invert Selection**
  (Ctrl+Shift+I): a partly selected pixel becomes as selected as it was not, and what lies
  past the canvas becomes selected. Each is a history state.
- **Select Layer's Pixels** selects the active layer's pixels, each as much as it is opaque
  (a pixel half transparent selected half), as the layer holds them, whatever its opacity
  and visibility (`Mask::of_opacity`), past the canvas too. Ctrl+click on a layer's
  thumbnail does the same for that layer, Ctrl+Shift adding it to what is selected,
  Ctrl+Alt taking it away, Ctrl+Shift+Alt keeping only what is in both (the row takes the
  click and tells it apart, so that the thumbnail's other clicks are the row's: a
  right-click's menu, a double-click's rename). It is a step of the
  recipe, a mask step combined as a shape is (a mask step combines as its `combine` says;
  one replacing is what a carried selection becomes; with nothing selected, as a shape's,
  `selection::with_maker`); worked out off the interface thread (*Edits that take long*),
  the memory of the mask's levels asked for as it is made, each tile's as it keeps it (a tile
  of one level, opaque or transparent, takes none: `Mask::of_opacity_within`, as the modifiers
  ask, see *Feather*). One that would select nothing (an empty layer, or nothing left once
  combined) is refused, saying so, and what was selected stays.
- **Clear** (Delete) erases what is selected on the selected layers, as much of each pixel
  as it is selected; inside a selected group, on the layers shown (a hidden one is left as it
  is). It is refused, saying why, when a selected layer or group, or a group one is in, is
  hidden, when a layer it would clear, or a group that layer is in, is locked, and on a layer holding a placed image,
  whose pixels are its own, drawn through its transform (rasterize it first).
- **Copy** and **Copy Merged** copy within the selection; **Cut** (Ctrl+X, on the Home tab
  too) copies, then clears what it copied: within the canvas, so that what lies past it,
  which a copy leaves out, stays. The copy reaches the clipboard off the interface thread,
  after the clearing; should it not get there, the clearing is taken back, gone from history
  as though never made (`History::take_back`: undone first if it is the current state, and
  dropped if it was undone meanwhile, so that no redo cuts without the copy), or, if
  something was made from it since (or a script's run it was part of was joined into one
  state), the user is told to undo it.
- **Layer via Copy** (Ctrl+J; on the Layer tab too) and **Layer via Cut** (Ctrl+Shift+J):
  what Copy would take from the selected layers, as much of each pixel as it is selected,
  but past the canvas too, on a new layer just above the topmost of them (above a locked
  group it is in, as new layers go), named for it and selected; Layer via Cut then erases
  it, as Clear does (refused as Clear is). One history state, worked out off the interface
  thread (*Edits that take long*). With nothing selected, Layer via Copy duplicates the
  layers, as Ctrl+J does in Photoshop (Duplicate Layer keeps its button, without a key).
- **Crop to Selection** makes the canvas the rectangle round the selection (refused for one
  that reaches past every edge).
- **Rotate**, **Flip**, **Resize Image** and the Crop tool's straightening carry what is
  selected with the layers (`Selection::transformed`). Its shapes are carried exactly, so a
  rectangle on the grid turned a quarter is one on the grid again; a turned rectangle
  stretched along another way, no longer a rectangle, cannot be, so the selection up to it
  becomes a *mask* step: its mask, carried pixel by pixel (exactly through quarter turns and
  flips, read between pixels otherwise), the steps after it carried as before. A mask step
  is told (to scripts) without its mask, and so is never read back as one.
- The `selection` query tells a script the rectangle round what is selected (or that
  nothing is, or that it is unbounded) and the steps that made it, positions counted from the
  canvas's corner as everywhere in the API; Select All, Deselect and Invert Selection answer
  with the same.
- The canvas shows the outline as marching ants: white, with black dashes moving along it a
  step every 100 ms, drawn through the middles of the screen's pixels so that they stay
  sharp. They march only while some of the outline is in view and the window has the focus,
  as each step is a frame of the whole window. The outline is worked out once a selection,
  and kept with a weak reference to its mask, so that it keeps alive no mask history has let
  go; it goes once nothing is selected.

### The Select tool

The Select tool (M) draws rectangles and ellipses, its variants (`select_tool.rs`). A drag
selects the shape from where it began to where it ends, both on the pixels' corners nearest
the pointer, so its edges fall on whole pixels; while it goes on, the shape shows as marching
ants. How it combines with what is selected is the tool's *Combine* setting (replace by
default), or the keys held as it begins: Shift adds, Alt takes away, both keep what is in
both. Shift pressed once it has begun makes it a square or a circle, and Alt draws it from
its middle (the keys held from its beginning count only once let go and pressed again). Esc
gives a drag up, as does showing another image before it is let go; a click with no keys
held selects nothing, and a drag that would change nothing (taking away with nothing
selected) makes no history state. *Smooth edges* (on by default)
selects a pixel the edge crosses as much as the shape covers it. Each selection is a history
state, *Rectangle selection* or *Ellipse selection*.

**Single row and single column**, its other two variants, select a line of pixels a pixel
thin, the canvas across (or down): the one under the pointer, not the nearest corner, and
the nearest within the canvas when the pointer is past it. A click is enough; while the
button is held it follows the pointer, as marching ants, and the line it is let go on is
selected (*Single row selection*, *Single column selection*), combined as a shape's drag
is. Each is a rectangle step, so the box below edits it once a rectangle or an ellipse is
chosen again, and `select_rectangle` with a `height` (or `width`) of 1 selects one from a
script; while a row or column is chosen the box is not shown, since round a line a pixel
thin it would catch the presses meant for the next one.

**Its box.** The selection's last step, when it is a shape, has a box round it (`edit_box`,
shared with the Crop tool; an ellipse's is the rectangle round it, turned with it). Where a
drag would replace what is selected (no key held, *Combine* replace), a drag from inside it
moves the shape, from a handle sizes it (the opposite corner or side staying; Shift keeps its
proportions), and from just outside a corner turns it; upright, a side dragged goes to the
nearest pixel edge (the others stay where they are), and a move puts its corner on a pixel's
corner, keeping its size as it is. Elsewhere, or where a drag would combine a new shape with
what is selected, a drag draws one as above, and the pointer says so. A drag edits that step in place, the steps
before it kept as they were worked out (a selection keeps the mask its steps but the last
make, which its own mask was worked out from), as one history state: *Move selection*,
*Resize selection* or *Rotate selection* (none, if it comes back to where it began); Esc
gives it up, as does its image not being drawn (shown in no pane) before it is let go
(`AppState::leave_undrawn`, which also drops that image's overlay textures). An edit that
leaves nothing selected keeps its steps while it goes on, so that the box stays to be put
right; once done, nothing is selected. The numbers button under the box
shows its exact position, size and angle beside it (`box_fields::selection`), as the
transform and crop boxes' do; what is typed edits the step at once, as one pending edit,
*Edit selection*, until something else is done. Scripts select shapes by the same numbers:
`select_rectangle` and `select_ellipse`, the canvas's box unless told otherwise, with
`combine` and `smooth`, and the box's `x`, `y`, `width`, `height`, `angle`, `anchor`,
`from_layer` and `from_anchor`, which take expressions.

### Saved selections

**Save Selection** (Select › Saved) keeps what is selected under a name in the image
(`Document::saved`, in the order first saved), steps and all, as a history state: what is
selected once an edit in progress (a transform box) is applied, read as the state is made. A
name taken already replaces the selection saved under it, the dialog saying so; names are
told apart without the spaces round them and without regard to case
(`SavedSelection::named`), and one that is empty is refused where it would be kept, whatever
asked. The name offered is the first free "Selection N".
**Load Selection** selects one again: in place of what is selected (its steps come back, to
change in the selection panel), added to it, taken away from it, or keeping what is in both
(a mask step of its own, `MaskOf::Saved`); inverted first, if asked; as a history state,
worked out off the interface thread, memory measured as it goes; refused, saying why, if it
would leave nothing selected. A saved selection is a `KeptSelection`, as what Deselect took
away is: turning, flipping, resizing or cropping the image adds to how it has changed since
(`since`), and loading it carries it so, once. The selection panel lists the saved ones above
the steps, each with buttons to load it (in place of what is selected) and to delete it
(*Delete saved selection*). History counts their masks; they are part of what a file saves,
so saving or deleting one makes the image unsaved. Files keep them (*Native file format*,
version 3): each one's recipe, the mask it comes to and the one its steps but the last come
to (neither worked out again on opening, nor when its last shape's box is first touched) and
how the image has changed since. Load Selection and Reselect bring a kept selection back the
same way (`select::brought_back`), refused if it selects nothing. Scripts: `save_selection(name)`, `load_selection(name,
combine, invert)`, `delete_saved_selection(name)`, and the `saved_selections` query (each
one's name, the rectangle round what it selects, carried as the image has changed since, its
steps as saved, and `changed_since`).

### Transform Selection

Transform Selection (Select › Modify) puts the transform box (*Paste and transform*) round
what is selected, as a session of its own (`transform::Kind::Selection`): its frame the
rectangle round the selection; its handles, numbers, snapping, Fit and Fill as for layers;
no drawing method, since nothing is drawn. Only the selection changes, not the pixels: it is
carried by the box's change as Rotate, Flip and Resize Image carry it
(`Selection::transformed`), its shapes exactly, its modifiers worked out again where they
are carried to, a step that cannot be carried becoming a mask carried pixel by pixel. While
the box is open, the marching ants go with it (the outline as it was, carried by the box's
change; the selection overlay, of the selection as it was, is not shown), and the selection
is worked out only once the box is applied: by Enter or the box's Apply, off the interface
thread (`AppState::commit_box`, an edit of `edits`), memory measured as it goes; by any
other change made meanwhile, which applies the box first (`DocState::commit_pending`), at
once. Either way it is one history state, *Transform selection*, unless the box is left where
it was; Esc gives it up. A selection reaching past every edge (inverted) has no box, and the
command says so. Scripts: `transform_selection`, with the transform box's numbers (`x`, `y`,
`width`, `height`, `angle`, `fit`, `anchor`, `from_layer`, `from_anchor`), answering with
what is selected then.

### The recipe

A selection is kept as an ordered list of steps, not only as its final mask:

- **Makers** produce a mask and combine with the steps before them by *add*, *subtract*,
  *intersect* or *replace*: outline, rectangle, ellipse, AI object, paint-the-area, color
  range, magic wand, painting on the mask.
- **Modifiers** change the mask so far: feather, grow/shrink, smooth, invert, refine edge.

The **selection panel** (a tab beside Layers; `selection_panel`) lists the steps of the active
image's selection, in the order they were taken, each with what it does in a few words, its
lengths in the user's unit, a shape placed by its top-left corner (turned with it) from the
canvas's corner, as its box's numbers place it ("Rectangle 100 × 70 px at 100 px, 50 px",
"Grown by 5 px, square corners", "A layer's pixels", "The steps before, carried pixel by
pixel": a mask step says which it is, `MaskOf`). A modifier's button opens its dialog on it,
which shows what the selection then comes to with that step changed, as it does when adding
one (`Modifying::at`): the steps before it worked out once, by the first preview that needs
them, and kept for the rest; the steps after it added again; a step set back to what it was
shows the selection as it is, without working anything out. A maker's menu changes how it
combines; each step's button takes it away. Each is one history state (*Change selection
step*, *Remove selection step*), the selection worked out again off the interface thread
(`AppState::rework_selection`, an edit of `edits`, memory measured as it goes); coming to
nothing, what was selected is kept for Reselect, as Deselect keeps it. A step made to replace
what was before it leaves the earlier ones out, as one made so would; one that leaves the
selection as it is (a grow by nothing) is no step, as when it is added. While a box is open
on the image, the panel's buttons wait for it (greyed, saying why), as the modifier commands
do: changing the steps would apply the box under the user. Scripts: `edit_selection_step` (a step by its number among the `selection` query's
steps, and the settings of its kind: a maker's `combine`; a grow's `by` and `corners`; a
feather's `width`, `side`, `falloff`; a smooth's `radius`; a border's `width`, `side`; others
refused) and `remove_selection_step`, answering with what is selected then. Outlines are to be
stored as Bézier curves; makers that produce pixels (AI, color range, painted masks) store
their settings and their resulting mask. AI steps cache their result and rerun only when
their own inputs change. Users who never open the panel just make selections as usual.

The makers that look at the pixels (AI, color range, magic wand) work within a selection
that exists (intersect); AI selection inside a selection runs the model on that crop. The
shape makers (rectangle, ellipse, outline) combine as their mode says, replacing it by
default.

### Feather

The feather setting is the width of the falloff zone; it can fall inward, outward or be
centered on the outline. The shape of the falloff is a curve (Gaussian by default; linear and
custom curves are options). It is a recipe step, so it stays adjustable. Grow/shrink has an
adjustable shape (round, square, diamond).

Both are built (`opal_core::modify`, `Step::Grow`, `Step::Feather`), from each pixel's distance
to the outline: measured from where the outline crosses between pixels, read between their
levels so that a smooth edge stays smooth (a smooth circle grown is the larger circle to
within a tenth of a pixel), spread to every pixel by Danielsson's sweeps, in the measure the
shape asks for (round: straight across; square: the farther of across and down; diamond:
across and down together; a sharp edge's corners exactly), and only in a band of tiles
round the outline, each side as far as the levels vary there (grown by 900, 900 pixels out
but only beside the outline in, where every pixel stays selected), found run by run, in time
with the outline's length (a tile wholly on one side of half is none of it), worked out in
blocks of tiles, a window each (the block and the band round it), of the size that should
take the least time for the tiles to work out (larger blocks repeat less of their margin, but
fewer fit at once, and may cover more far from the outline), by as many workers as their
windows fit in half a gigabyte (and no more than it has blocks), the other threads left free.
The memory it takes is asked for as it goes (`modify::Bounds`, which also stop it when it is
not wanted): the windows before it begins (given back once it is done, so that the next step
of a recipe may have them), then each block's tiles as they are kept, at the size they take (a
row of levels a tile, for a straight edge; what alike tiles side by side save, once kept as
one, given back), against what was free when it began, measured on its own thread
(`memory::Admission`). A modifier's mask that goes once the next is made (a
pass of Smooth, a step of a recipe worked out again but the last two, whose masks a selection
keeps) gives back what it took. Refused, it stops: the dialog says why, a script's call is
refused, and Rotate, Flip or Resize Image working a carried recipe out again fail, saying
why.
Distances go to 1000 pixels at most (`modify::MAX_DISTANCE`).
Two more modifiers are built the same way. **Smooth** (a radius) closes the outline (grown,
then shrunk back), so that pinholes and notches narrower than twice the radius fill and inward
corners round, then opens it (shrunk, then grown back), so that specks and spurs as thin go
and outward corners round: four passes of exact distances, round, each within the radius (the
two shrinks, one twice as far, would need a band and windows twice as wide, which costs more
than the pass it saves). **Border** (a width; inward,
outward or centered) selects the band of that width along the outline in place of what was
selected, each pixel as much as it lies in the band, in one pass.
They work from the outline, where pixels are half selected, so a soft selection grown or
feathered loses its own softness; growing by nothing, or a feather or smoothing of no width,
is no step.
On the Select tab, *Grow or Shrink*, *Feather*, *Smooth* and *Border* open dialogs (refused
while a box is open) that show what they make on their own image (a pending edit, *Grow
selection*, *Shrink selection*, *Feather selection*, *Smooth selection* or *Border selection*,
which a save leaves pending, as it changes nothing saved),
worked out off the interface thread as the numbers are set (lengths in that image's
resolution), the first included, the image showing the last done meanwhile. The jobs run one
at a time on a thread with a pool of threads of their own (`background::LatestJob`), so they
neither wait behind saves and exports nor keep them waiting; a job set while another runs
replaces any waiting, and work a later setting makes needless stops before it begins or
between blocks. Each job tells its end on a channel, whatever came of it. OK keeps what is
shown once it is what the step set makes: while that is worked out, the dialog waits, saying
so (only Cancel can be pressed), the interface going on. A step that could not be worked out
(not memory enough, or a failure) shows the selection as it was, the dialog saying why, and
OK keeps nothing until another is set. A save applies an open box. Scripts call `grow_selection` (`by`, `corners`) and
`feather_selection` (`width`, `side`, `falloff`), the names the recipe gives them. Rotate,
Flip and Resize Image carry them when they scale every length alike, their distances
scaled, up to the farthest a step may reach; square and diamond corners, measured along the
pixel grid, only by quarter turns and flips. Otherwise the selection up to them is taken as
its mask. Custom falloff curves, and editing them later in the selection panel, are to come.

### The outline tool

Freehand, anchor-and-curve, polygon and edge-snapping selection are one tool whose segments
differ by gesture:

- drag: freehand segment
- click: anchor (straight segment)
- drag from an anchor: curve
- hold a key: segments snap to edges (live-wire / Intelligent Scissors: a shortest-path
  search over an edge-cost map computed in OKLab). Controls: snap radius and edge
  sensitivity. Advanced: contrast thresholds per channel in a chosen color model, and the
  weights of the edge-cost terms (edge strength, zero crossings, edge direction).

The snapped segment is previewed live before it is committed; Backspace removes the last
anchor.

**Built so far** (`outline_tool`, the Outline tool, L): a drag draws a loop freehand (a point
kept every 2 screen points), closed and selected once the button is let go. A click begins
one point by point instead: each click a point joined to the last by a straight side; a drag
from the last point (within 7 screen points) pulls the side to it into a cubic curve, smooth
through the point, the handle pulled carried on to the next side; a drag elsewhere draws a
freehand stretch. Enter, a double-click, or a click on the first point (marked) closes it;
Backspace takes the last point back (all of them, and it is given up), ending a pull or
stretch under way, the point now last giving again the handle it gave; Esc gives it up, as
do changing tool, a box opening, an edit beginning on the image, or the image no longer
drawn, but not Space held to scroll the view, while which it waits; Ctrl+click picks a layer
and adds no point. While one is drawn it takes Enter, Esc and Backspace before any command
(Backspace is not Clear then). It holds at most a million points, as a file's may. Keys held as it begins combine
it as the Select tool's shapes do (or the tool's *Combine*); *Smooth edges* (on by default).
It shows as marching ants, closed back to its first point, with the side to the pointer that
a click would add (a curve, after a handle was pulled). It
is a recipe step of its own (`Step::Outline`, an `outline::Outline`: a start and its line
and curve segments; *Outline selection*), carried exactly by any transform (its points
carried), its combine changed in the selection panel ("Outline of N points"). Scripts:
`select_outline(points, combine, smooth)`, points `[x, y]` from the canvas's corner (as far
as a box's may be put, 1e7 pixels; at most a million of them) joined by straight sides; the `selection` query tells an outline step's start and segments.

**What an outline selects** (`outline::Filled`): what it encloses by the nonzero rule (a loop
crossing itself selects each lobe; loops one inside another going the same way select all of
them), curves flattened to within 0.05 pixels (four million points at most for all its
curves, fewer for each in proportion past that). Its mask is made a row of tiles at a time
(`Coverage::row`): the tiles its edges pass through are worked out, and those between them,
wholly in or out, are told by the winding at their middles, so that the work follows the
outline's length, not the area it encloses. Smooth, each pixel as much as the polygon covers
it, exactly, however it crosses itself or nests (`Coverage::tile`): each row of pixels is cut
where the edges reaching into it begin, end or cross, and between the cuts each stretch the
outline goes round (from where the winding leaves 0 to where it comes back) adds the area it
covers along the row's cells, those left of the tile standing at its left side, a running
sum along the row giving each pixel's share. Sharp, each pixel by whether its middle is
enclosed, counting the edges crossing its row.
Edge snapping (live-wire) and editing an outline's points come next.

### Reshaping an outline

- **Right-drag** starting on an existing outline redraws part of it. While dragging, the
  stretch about to be replaced is shown in red with the new line drawn in.
- The two ends of the new line split the closed outline into two stretches. The one nearer
  the new line is replaced; **Tab**, pressed before releasing, flips to the other.
- Releasing far from the outline, or Esc, cancels. How near counts as "on the outline" is
  adjustable. A right-click without a drag still opens the context menu. **R + left-drag**
  does the same as right-drag, for touchpads.
- Points of the outline can also be dragged directly.

### AI object and paint-the-area selection

One tool; its prompts are include/exclude clicks, a box, a "somewhere in here" brush and a
"definitely in" brush (everything else counts as out, which forms a trimap), and a **Select
subject** button that needs no prompt at all. The pipeline:

1. Segmentation: for Select subject, **BiRefNet** (MIT), which finds the main subject on its
   own; for prompts, **SAM 2.1** (Apache-2.0), whose image encoder runs once per image and
   whose prompt decoder then answers each click at interactive speed. SAM 3 (Meta's own
   SAM License, which adds text prompts such as "the dog") as an optional download.
2. Matting in a band around the edge (for hair and fur), with **ViTMatte** (MIT).
3. Color decontamination, which removes background color showing through semi-transparent
   pixels.

Adjustable: the clicks and brushes, the model and its size (SAM 2.1 tiny to large, BiRefNet
general, portrait or high-resolution), mask threshold, edge-band width, decontamination
strength, and a hard-edge toggle.

All models run locally through ONNX Runtime (DirectML on Windows, so on the GPU; the CPU as
fallback). None is bundled: each is downloaded on first use, after saying what it is, how
large it is and what its license says, and Preferences › AI models lists them with their
sizes and can delete them. The licenses were checked (November 2026): SAM 2.1 Apache-2.0,
BiRefNet MIT, ViTMatte MIT, LaMa Apache-2.0; the training data of ViTMatte and LaMa carries
its own terms, which the model manager shows.

Models whose terms allow only non-commercial use are offered too where they do better (BRIA's
RMBG-2.0 for Select subject, say), marked so before they are downloaded and in the model
manager. Their terms bind whoever uses them, not only the project, so a model that allows
commercial use stays the default for each task, and someone editing a client's photographs is
never caught out. A preference (Preferences › AI models, *My use is non-commercial*) makes
the better model the default wherever one is non-commercial.

### Extracting objects

Because an AI selection is an ordinary selection (a soft mask), everything works on what it
selects. The selection panel and the Select tab add the commands extraction needs:

- **Copy to new layer** (Ctrl+J with a selection) and **Cut to new layer**
  (Ctrl+Shift+J): the object, soft edges included, becomes its own layer, positioned where
  it was. From there it can be moved, transformed, adjusted, hidden or copied into another
  image, where it arrives in the paste box.
- **Lift out:** cut to a new layer *and* fill the hole it leaves, so the object can be moved
  or resized and the scene behind it still looks whole. The fill is painted by **LaMa**
  (Apache-2.0) onto a layer of its own between the two, so it can be inspected, touched
  up or deleted, and the original pixels stay underneath.
- **Remove object:** the fill alone, for making something disappear.

Adjustable: how far the hole is grown before filling (so no fringe of the object survives
around it), the fill's working resolution (LaMa sees a 512×512 crop around the hole, scaled),
and whether the fill goes on its own layer (default) or into the original.

### Color-range selection

Each channel `c` of a chosen color model gets a curve, in the channel curve editor, giving a
membership `m_c` from 0 to 1. The memberships combine by a
weighted power mean whose exponent `p` is the "strictness" slider:

```
S = ( Σ w_c · m_c^p  /  Σ w_c )^(1/p)
p = +∞ → max (OR);  p = 1 → weighted average;  p → 0 → weighted geometric mean;  p = −∞ → min (AND)
```

The weights `w_c` are adjustable. Curves start from eyedropper samples and are then adjusted.
Because curves per channel can only select box-shaped regions of color space, a second mode
selects by distance from the sampled colors in OKLab, with an editable falloff curve. The
same machinery drives *Color match* (see *Painting and retouching tools*).

## Interface layout

Commands (one-shot actions such as Crop, Feather or Merge Down) are on a ribbon; tools (what
the pointer does on the canvas) are in a group pinned to the ribbon; the active tool's
settings are in a panel.

### Command registry

Every command is defined once: name, icon, one-line description, longer help text, keyboard
shortcut, and the condition under which it is available, with the reason shown when it is
not. The ribbon, command search, context menus, keyboard shortcuts, keytips and the help
pages are all generated from it, so a command looks and behaves the same everywhere.

### Ribbon

- Tabs of grouped command buttons, each with an icon and a short label: File, Home, Image,
  Layer, Select, Adjust and View to begin with; Filter, Vector, Photo (shortcuts made of
  ordinary layers: *Photographs*) and others as features arrive.
- **Tabs that apply only to some objects** (Text, Shape, and later others) are always
  present. They are dimmed while nothing they apply to is selected, and light up when
  something is. A dimmed tab can still be opened to browse its commands, which are disabled
  and say what to select first. Tabs never switch on their own.
- **Tooltips**, not text on the ribbon, carry the details: name, shortcut, the one-line
  description, and for a disabled command the reason. The hover delay is adjustable, and
  tooltips can be turned off.
- **Help:** F1 while hovering a command anywhere (ribbon, panel, menu) opens the built-in
  help page for its group, at that button; over a tool, the Tools page. A small ? appears while the pointer is over a group's label and opens the same
  page. Help pages ship with the application and work offline.
- **Keytips:** pressing Alt shows a letter on every tab and button, so everything can be
  reached from the keyboard without memorized shortcuts. A button's is the first of its
  name's initials, then of its letters, not taken by one before it on its tab; a few have
  their own whatever comes before (Cut's X, as in Office, so that Copy keeps C). Tools get
  none: their own single keys (H, Z) already work without Alt.
- **Command search:** a search box at the ribbon's right end finds any command by name.
- **Sizes:** full (icons and labels) or a single line. As the window narrows, groups shrink
  in steps: large buttons become small ones, then a whole group collapses into a dropdown.

### Tools

- The tool buttons sit in a group pinned at the left end of the ribbon, the same on every
  tab and styled so it reads as separate from the tabs. A single-line ribbon still shows
  them, and when the window narrows the command groups collapse first.
- The group can be moved to a vertical strip at either side of the window, for narrow
  screens or by preference.
- Tool buttons are icons, named by their tooltips. At about 34 px per button in two rows,
  the twenty or so tools planned take about 375 px: comfortable on a screen 1920 px wide,
  but on a laptop showing 1280–1536 px it leaves the command groups noticeably less room.
  That is what the side strip is for.

### Tool settings panel

A docked panel: the active tool's variant picker at the top, then its basic settings, then
its Advanced section. It is a panel rather than a bar because the Advanced settings include
curve editors.

- **The heading is one row**, so that in a short panel the settings start high: the tool's
  name, which says what the tool does on hover, and a **Presets** menu beside it (save the
  settings under a name, typed in a row that shows below the heading only meanwhile, which
  takes the keyboard once, when asked, and not back from a field the user goes to; apply a
  saved one; delete one, from a submenu of its own so that a slip applies rather than
  deletes). What the tool does is said by the status bar too, which, while a transform box
  is open, says instead how the box is used (`transform_ui::HINT`), the canvas being the
  box's then (see *Status bar*).
- It scrolls, its scroll area as wide as the pane whatever it holds, so that the scroll bar
  is at the pane's edge.

### The window

- It starts 1440 × 900 logical pixels (`app::START_SIZE`), in the middle of its monitor.
  Once its place is known (from its second frame, the first measuring it before the
  interface's zoom applies; not while it is minimized), what shows of it (its frame, less
  the borders Windows keeps unseen at its sides and bottom) is put all where windows go
  there (what the taskbar and other docks leave, as Windows reports it): moved there if it
  fits, maximized if not, as on a full-HD screen at 125% (864 logical pixels tall).
  Elsewhere, where the system does not say, it is maximized if it does not fit the monitor
  with 48 logical pixels for a taskbar; where it does not say where the window is either
  (Wayland), nothing is done. (`app::fit_to_monitor`, once.)
- The starting layout's split gives the tool settings half the right column, which shows
  the crop box's numbers without scrolling in a window maximized on such a screen.
- Screen modes (View › Screen Mode, F): standard; full screen (the window's title bar and the
  taskbar gone); and the image alone, full screen with only the active image in it, no
  ribbon, panels, tool strip or status bar, its tools and keys working as ever (while no
  image is open, the window is shown as usual); what the status bar would show, a recent
  notice and an edit under way with Cancel, shows over the image's foot. A notice says how
  to go on or back. A window that leaves full screen another way (the system's key) is
  shown as usual again. Not kept between sessions (`AppState::screen`).
- Files dropped on it open as images, in the order dropped, but not while a dialog is open
  (one opened behind it would become the image the dialog acts on): they wait for it to
  close (`AppState::open_dropped`), in the frame its closing brings.

### Status bar

- At its left, what the canvas does (the tool's description, or the transform box's hint),
  or for a few seconds a notice; at its right, parts from its end: scripts connected, an
  edit under way (see below), a save or export under way, tiles on disk, GPU memory, and the
  active image's zoom, size, resolution and the pointer's place.
- The text is laid out first, so that it is read out first, in the room the parts took
  when last shown (another pass at once where they grew), and cut short, whole on hover,
  where it does not fit. What other threads change (scripts connected, tiles on disk) is
  read once a frame, the parts chosen and drawn by it. Where the parts would leave it less
  than 160 points,
  the least needed are left out (tiles on disk, GPU memory, the pointer's place, and so
  on, a save under way last); the scripts' open steps and the file's name are cut short
  past 260 points (`app::StatusPart`).

### Edits that take long

Merging (Merge Down, Selected, Visible and Group), Stamp Visible, Flatten, Rasterize, Rotate
and Flip, Rotate by Angle, Resize Image, Crop and straighten, and a script's
`grow_selection` and `feather_selection` are worked out off the interface thread
(`edits`), from the image as it was (an edit in progress committed first, and a rename left
open in it finished), their parallel parts on threads of their own (`edits::pool`), so that
they neither wait behind the background threads' work (the images' views, saves and exports)
nor keep it waiting, and applied as one history state once done (none, if they changed
nothing). Their memory is checked before they begin.

- **The image stays as it was meanwhile, and nothing may change it** (`DocState::busy`):
  its commands are unavailable, saying why ("Merge down is under way: wait for it to
  finish, or cancel it in the status bar"), and so are undo and redo, saving and exporting
  it; its tools are the hand (and the zoom), its keys change nothing, and its Layers and
  History panels are greyed. Looking at it, copying from it, closing it (the edit given up
  as it goes, not when closing is asked, so that answering Cancel to the question keeps it)
  and every other image stay free; a paste into it waits for the edit, and so do scripts'
  calls, the next call in the same pass too (`Server::serve` asks again after each).
  Quitting waits for it, as it waits for files being written, and then asks about the
  image's changes. Changing the image would make the edit's result wrong, and refusing the
  result then would throw away work the user asked for. Whatever would change it anyway
  (a path the interface does not guard) is refused where history is changed
  (`DocState::apply`, `edit_pending`, undo, redo, going to a state, taking one back), the
  status bar saying why; should the image change all the same, the result is not applied,
  and the user is told.
- **Shown only if it takes a while.** The interface waits 60 ms for an edit as it begins:
  one done by then is applied at once, as if it had been worked out there (the image never
  busy, a script answered at once). One still under way after 300 ms shows in the status
  bar, with how far it has got (the merges count tiles, the others objects; a spinner where
  it cannot tell) and Cancel, which frees the image at once and leaves it as it was (the
  work stops as soon as it next looks); its tab is marked, and the greyed panels say why.
- **A script's call** that begins one is answered once it is applied, with what it would
  have been answered with at once, of the image the edit was of (whichever is active by
  then), and what applying it said as its notes or its failure. If the edit fails then,
  the user's view is put back as the call found it, as for a call refused at once, unless
  another image has been made active since. Its failure is named by the call however soon
  it comes (`grow_selection: ...`), a refusal told as one (not memory enough, nothing
  selected to copy), a panic as an error; a notification's, which no answer carries, is
  told to the user (`api::Calling`, known to the edit as it begins).
- Edits that read as long but are not stay where they are asked for: measured on 67
  megapixels, a quarter turn applied by the transform box takes 50 ms, Clear 8 to 14 ms,
  and a paste turned and drawn into the active layer 0.3 s.
- A failure, or a panic, in the work is told, the image left as it was.

### Look

- egui restyled. Three choices of color scheme: **Dark** (the default: neutral gray, so the
  interface does not skew judgment of the image's colors, as in most image editors),
  **Light**, and **Follow the system** (switches along with the operating system's light
  or dark mode).
- One accent color, slightly rounded corners, subtle hover and press animations.
- Scroll bars are always shown, thin until the pointer is on them, with room kept for
  them beside what scrolls, so that a panel cut short shows there is more of it (egui's
  own appear only under the pointer).
- Fluent UI System Icons (Microsoft's, MIT), rendered sharp at any display scaling.
- The system font: Segoe UI on Windows; a bundled open font elsewhere.
- **Rulers** (`rulers.rs`) along the top and left of each image's view, outside it, which
  is that much smaller (View › Rulers, Ctrl+R; a preference, shown by default). They count
  from the canvas's top-left corner in the unit sizes are shown in, at the image's
  resolution, labeled at 1, 2 or 5 times a power of ten of the unit (of pixels, at least
  one) far enough apart for the labels (64 points), divided in tens, quarters or fifths
  (fewer where those would crowd); the corner names the unit, and the pointer's place is
  marked on both.

## Layers UI

- **Click to select a layer**: with the Move tool, or Ctrl+click with any other tool. It
  picks the topmost layer that visibly contributes at that point (how opaque counts as
  visible is adjustable); right-click lists every layer under the cursor. Clicking selects
  layers, never groups; Alt+click selects the group around the layer instead.
- The active layer is highlighted, and scrolled into view, in the layers panel, and has a
  faint outline on the canvas (around its content; the transformed outline for an image
  object).
- **Several layers can be selected:** Ctrl+click in the layers panel adds or removes one,
  Shift+click selects a range. The active layer is the one last clicked. Commands that act
  on layers (Delete, Duplicate, Group, Merge Selected, Copy, Rasterize, Move, Transform)
  act on all of them.
- **The Move tool** drags the selected layers by whole pixels; Shift keeps the move
  horizontal, vertical or diagonal. It snaps as the transform box does (*Paste and
  transform*). The arrow keys nudge by one pixel, and with Shift by ten (adjustable), with
  any tool. Clicking first selects the layer under the pointer (a setting turns that off,
  to drag the selected layers from anywhere).
- **Sample from** (which layers a tool reads): current layer, layers below, all visible, or a
  chosen set (checkboxes in the layers panel; sets can be named and reused). Present on every
  tool that reads pixels: Clone, Heal, Mix, Detail, the eyedropper, the magic wand, color
  match, color range and AI selection.
- **Apply to** (which layers an operation writes) is a separate control, for moving,
  transforming or adjusting several layers at once. Until it is built, operations apply to
  the selected layers.

### Merging and rasterizing

- **Merge Down** and **Flatten Image** bake layers into one pixel layer (built).
- **Locks:** a locked layer cannot be changed, merged or deleted, and neither can a group
  with a locked layer inside, nor a layer inside a locked group (what a group holds is part
  of it: `commands::locked_for`). What a locked group holds is not grouped, faded, moved in
  or out, or moved up and down in it; new layers, pastes and copies of a layer in one go above
  it, not into it, in their order (`layer_ops::above`). A locked layer itself can still be
  moved up and down the stack.
- **Merge Selected** merges the selected layers into one, where the topmost of them was.
  Selected layers that do not show (hidden, at 0% opacity, or in a hidden group) are left
  as they are, and hidden layers inside a selected group stay, just below the result.
- **Merge Visible** merges every visible layer into one, where the topmost of them was;
  hidden layers stay, and keep their order among themselves.
- **Merge Group** turns a group into one pixel layer with the group's name, opacity and
  visibility.
- **Stamp Visible** adds a layer holding everything visible merged, above the active layer
  (outside any group), the originals kept, for retouching on a flat copy.
- **Rasterize Layer** turns the selected layers' editable content (image objects, and later
  shapes, text and effects) into pixel objects at the document's resolution. It is never
  required: no tool refuses to work on editable content. It exists for when pixels are
  wanted, to make a file smaller, or to make a heavy layer cost nothing; like everything,
  it can be undone.
- Merging and rasterizing keep the look exactly: the result composites like what it
  replaces. Tiles made from the same stored tiles in the same way (the same tiles, phase
  and opacities, with no image object, whose pixels depend on where the tile is) are
  composited once and shared, and a tile showing one stored tile unchanged is that tile,
  uncopied: a uniform background costs a few tiles however large. A new white image is
  likewise four tiles at most (inside, right edge, bottom edge, corner).
- **Size limits.** An image object enlarged far past the canvas could ask for more memory
  than the computer has when drawn into pixels. Merging and rasterizing refuse, with the
  reason, a result needing more than 2 GiB beyond four canvases' worth; baking a paste
  into a layer, or resampling on Resize Image, keeps only the part around the canvas then,
  and says so. Within those limits, nothing is drawn into pixels that would need more
  memory than is free when the work begins, since a failed allocation ends the program
  (`opal_io::memory`, measured by the app's `memory.rs`). Work needs memory two ways: what
  it takes in one go (a decoded file and the file's own bytes, an `.opal` file's tile
  places as read, the bitmaps of a copy, an export's bands), which must fit in the memory
  free (the available physical memory, on Windows and Linux, less a twentieth of all
  memory kept back for what estimates leave out and for the system) together with what
  tiles in memory going to disk could make room for, as far as the disk holds them (the
  work holds a cap on tiles, which sends them there before it is taken); and the
  tiles it makes as it goes, which may also take the disk's room (known on Windows only;
  none is counted elsewhere), since past the cap they go there as they come. Merging counts
  exactly the tiles it would make (none for those it shares), drawing into pixels exactly
  the tiles it draws (`render_cost`), summed over everything one command draws; Rasterize
  counts every image object it draws, hidden or not, and no pixel object. A transform box
  that leaves pixel layers turned or flipped draws them back into pixels only if that
  fits; otherwise they stay images, drawn as exactly, and the status bar says why. The command is refused with
  how much it needs and how much is free; a paste that cannot be baked stays an image in
  the layer. Exports and copies are checked the same way: an export takes its bands in
  flight and, at another size, the image it is drawn from with its copied edges
  (`export_memory`); a copy takes
  about 64 bytes a pixel on Windows while it goes onto the clipboard (the pixels, the
  16-bit PNG, the bitmaps, the system's copies of them, and the copy kept for pasting
  back; 24 elsewhere), and on
  Windows no more than the 4 GB a clipboard bitmap holds. Opening a PNG or JPEG (or
  pasting one) is checked before it is decoded: in one go the file's bytes, the decoder's
  buffer and a turned copy if its orientation turns it; and its tiles, made straight from
  the decoder's samples whatever their channels, in parallel, a tile at a time (4 bytes a
  pixel, kept at 8 bits, or 8), with flat areas one tile for each value;
  opening an `.opal` file before its tiles are read, which are then read 64 at a time and
  may go to disk as they come, so a document larger than memory opens again; and, on
  Windows, pasting a system bitmap from its header before it is read (in one go the
  bitmap and 4 bytes a pixel, and 4 of tiles). (Merging layers that are not adjacent moves the lower ones above whatever lay
  between them, which can change the look where those overlap; the status bar says so.)

### Performance with many layers

Editable content does not mean recomputing everything on each change.

- **Only what changed is recomputed.** Every cached tile of the projection knows the
  source tiles that produced it (see *Compositing*), so an edit recomputes the tiles it
  touches and nothing else. A tile also remembers the set of layers it was last checked
  against, and where image objects draw is worked out once for each set (pixel objects
  are asked of their own tiles), so panning an unchanged document checks only the tiles
  newly in view.
- **Editable objects are cached as pixels.** Shapes, text, effects and image objects are
  drawn per tile and redrawn only when they or what they are composited with change; for
  display they cost what a pixel layer costs. Editability costs memory more than time.
  Image objects are cheap enough to redraw as part of the projection tiles they touch;
  objects that are expensive to draw (text, shapes, effects) get a cache of their own.
- **Intermediate caches:** the composite of each group, and of everything below the active
  layer, so painting recombines only the active layer and what is above it, over a stored
  picture of the rest.
- **Previews first:** while something is dragged (the transform box, the Move tool), the
  tiles it changes are drawn at the resolution on screen, so the drag costs what the
  screen holds rather than what the image does. Pixel layers are reduced only under those
  tiles (kept at the level shown for the drag's length, as it comes back over them, while
  they fit the cache's budget beside its own tiles, and none beneath), and where a layer
  repeats one tile its reduced tile is that tile halved, made
  once a level, with nothing beneath it reduced (a drag over a 300,000-pixel white canvas
  shown whole: 23 ms a frame); image objects are drawn through their transform with their
  own resampling (Nearest by area, as above). Tiles the drag leaves alone stay exact,
  and the full-resolution result follows on release (but for the tiles of image objects
  too costly to reduce, which stay drawn at the level: see *Compositing*). A tile over a
  uniform block is made exactly even during a drag, costing no more than a preview. A
  24-megapixel image dragged at 25% takes about 13 ms a frame this way, against about
  90 ms. Heavy effects will preview the same way while their settings are dragged.
- **Saying what is slow:** a performance view lists the layers that cost the most time and
  memory, and offers Rasterize Layer or Stamp Visible for them.

### Diagnostics

When an action changes nothing visible, the editor says why: painting outside the active
selection; on a hidden layer, a locked one, or one whose transparency is locked; at 0%
opacity; where a layer above completely covers the spot; on the layer's mask instead of the
layer; or off the canvas (kept, but not exported).

## Paste and transform

Pasting opens the **transform box** instead of placing the pixels. The same box transforms
existing layers: Transform (Ctrl+T) opens it on the selected layers; Transform Selection, on
the selection itself (see *Transform Selection*).

- **Where a paste goes:** onto a new layer above the active one, as an image object. A
  button on the box puts it into the active layer instead, baked into that layer's pixels
  when committed. Pasting with no document open makes a new image of the clipboard.
- **Where it starts:** centered on the view at its own size, or shrunk to fit the canvas
  when larger (a setting). Paste in Place (Ctrl+Shift+V) puts a copy made in Opal Studio
  back where it was copied from, and anything else at the canvas's top-left corner.
- **Handles:** drag inside the box to move; corners scale, edges scale one side, and a
  handle above the box rotates around its center. The aspect ratio is locked by default;
  Shift, or the lock button on the box, frees it. Ctrl scales from the center. Shift while
  rotating steps by 15° (adjustable). Dragging outside the box rotates too.
- **Snapping** to the canvas's edges and center and to other layers' edges (later the
  selection's bounds too). Alt toggles it during a drag, a button on the box toggles it
  until clicked again, and the snapping distance is adjustable. Moving, the box's edges
  and center snap; scaling, the corner or edge dragged does. A corner with the aspect
  kept, and an edge, can only move along one line (the diagonal, or across the edge): it
  stops where that line crosses a snap line, if it is that near the crossing. A snap that
  would leave the box thinner than the snapping distance (onto a line through the corner
  held still) is not made.
- **On the box:** Fit (into the canvas), Fill (cover the canvas), the aspect lock, snapping,
  Into active layer, and Commit and Cancel, also on Enter and Esc, so the box never
  lingers like GIMP's floating selection. While dragging, the box shows the size and angle.
- **Settings:** the resampling method (*Resampling*), and the numbers: position, size in
  the user's unit and in percent, and angle, each editable.
- **Exact numbers by the box:** a button on the box opens the same numbers beside it
  (`box_fields`), beside the box and its toolbar (to the right, else the left, else the
  view's corner), so a paste can be put in a rectangle typed in any unit without leaving
  the canvas. The crop box has the same fields and button, and so has the box round the
  selection's last shape. The fields give the numbers first and how they are measured after them
  (the unit, the reference point, and what the position counts from), so that the tool
  settings, which have half the right column in the starting layout, show a paste's
  numbers without scrolling (the crop box's come after the crop tool's own settings, and
  show so in a window maximized on a full-HD screen at 125%); another unit, point or origin chosen shows
  them measured so in the same frame (egui is asked for another pass). Each grid of
  points is one stop for Tab, the point chosen, and the arrow keys move the choice, as in
  a group of radio buttons. The position is that of the box's **reference point** (a 3 × 3 grid in
  the fields, a preference: its top-left corner unless another is chosen), turned with
  it, from the canvas's corner; a size typed scales along the box's own sides about that
  point, and an angle turns about it, so the numbers typed stay as typed. **From**: the
  position counts from a point (one of the same nine) of the canvas or of a layer (the
  bounds of what it holds), kept per image (`box_fields::Origin`), located in the image as
  it was before the box opened, so a layer the box moves counts from where it was (its
  numbers then are how far it has moved); a layer gone or emptied shows the canvas's
  corner, without forgetting the choice. The menu lists the layers only while open. Vector
  paths' nodes, text boxes' corners and guides are to join the layers as things to count
  from (`box_fields::Base`) when they come. Scripts name it as `from_layer` and
  `from_anchor` (place, paste, transform, crop), located when the call is made (a paste
  read later counts from where the layer was then).
- **Fit into a rectangle:** the transform box's fields also hold a rectangle, kept per
  image (the canvas, whatever it is then, until one is typed; at the same reference
  point), with *Fit inside* and *Fill*, which keep the box's proportions and its turn and
  fit it, as it lies, inside the rectangle or over it, centered (the same fitting as the
  box's Fit and Fill buttons do with the canvas: `transform::fitted`), and *Stretch*, which
  makes the box exactly the rectangle, upright. Scripts' `fit` goes through the same code
  (`api::place_box`). A mirrored box's points are where their names say (its height runs
  the other way from its frame's).
- **Place** (File, Ctrl+Alt+O) opens the transform box on an image file, as a paste of it
  (onto a new layer named after the file; the history state is "Place"), by default at
  the file's **print size**, as most image editors place (scaled by its resolution against
  the image's, across and down, so a file whose pixels are not square keeps its shape: a
  2-inch logo at 300 ppi is 144 pixels in a 72 ppi image), before the fit to the canvas a
  large one gets. A file that does not say its resolution is placed pixel for pixel
  (taking it for 72 ppi, as some editors do, would make a screenshot tiny in a 300 ppi
  image), and so, with a warning, is one whose resolution is too far from the image's
  for its box to stay within what an image's placing may scale by (`MAX_SCALE`). A
  preference (Transform › Place at the file's print size) chooses; a button on the box,
  shown when the file's resolution is not the image's, switches one placing between the
  two and becomes the preference, as the box's other switches do. The session's
  `PasteInto.print` records the scale each placing gives (fit included), so the switch
  gives exactly the other one's size, scaled along the box's own sides about its center
  (a size or turn given meanwhile stays, relative to it), and lands on whole pixels when
  that is the file's own size, upright. Scripts: `place(print_size=…)`, the preference
  unless given.
- **One history state:** the whole session in the box, from paste to commit, is one entry
  ("Paste" or "Transform"). Cancel leaves the document as it was.
- **The clipboard:** PNG first (keeping 16-bit color and transparency), then the other
  image formats the clipboard offers, then copied image files. Reading and decoding run on
  a worker thread, so a large image never holds up the interface; the box opens when it
  is ready, on the image that was active. What the user started in the meantime comes
  first: the paste waits for a dialog to close, a drag to end, or a box opened (or crop
  box changed) on that image to be finished, and is dropped if the image was closed.
  Esc cancels a paste being read, unless a box on screen takes Esc first (the status bar
  says Esc cancels only while it does); a paste waiting for a dialog or box to close is
  placed in a box of its own, where Esc cancels it.
  Copy (Ctrl+C) copies the selected layers within the canvas; Copy Merged (Ctrl+Shift+C)
  everything visible within the canvas. Copies are encoded and written on a worker
  thread too, in the order made, and a paste waits for one still being written. On
  Windows a copy goes on the clipboard as a 16-bit PNG (for programs that read PNG), an
  8-bit bitmap with transparency (`CF_DIBV5`: bit fields, with the colour masks repeated
  after the header, where Windows writes them and Paint.NET and the `image` crate read past
  them), and a plain bitmap over white as `CF_DIB` and as the system's `CF_BITMAP` (given
  rather than left to Windows, which makes both from the `CF_DIBV5` and, missing its
  masks, garbles them), in one go; elsewhere as an 8-bit image. A copy made in Opal Studio keeps
  its full precision for pasting back into Opal Studio.

## Image size, crop and rotation

- **Crop tool** (C): a box over the canvas with handles, starting as the whole canvas;
  dragging outside it draws a new one. Aspect ratio free, the original, or a preset (1:1,
  4:3, 3:2, 16:9, 5:4) or custom one, either way round; guides (rule of thirds, grid, or
  none); what is cropped away is darkened (adjustable). **Straighten:** dragging outside
  the box's corners rotates it; the content is turned to match when committed. Enter
  crops, Esc cancels. Cropping only moves the canvas, so nothing is lost, unless *Delete
  cropped pixels* is set. A crop box larger than the canvas enlarges it. Straightening and
  deleting cropped pixels change every layer, so they wait while a layer is locked.
- **Crop to Content** shrinks the canvas to the visible content.
- **Resize Image:** a size in any unit or in percent (aspect ratio locked by default), a
  resolution, and a resampling method. A new resolution keeps the pixels and changes how
  large they print (one history state, "Change resolution", and no pixel touched, so it
  may be given while a layer is locked, which a new size may not), or, with *Keep the
  print size*, changes the pixel size with it; the size the fields show is
  at the new resolution, and the dialog says how large the image will print. Image objects are scaled with the canvas. Pixel objects too become
  image objects, keeping their pixels (enlarging again later loses nothing), unless
  *Resample pixels* is set, which resamples them now for a smaller file; the dialog says
  which happens.
- **Resize Canvas:** a size, with an anchor (one of nine points) for where the old canvas
  sits in the new, or the margins to add on each side, in any unit. Only the canvas
  changes.
- **New Image:** a size in any unit at a resolution, with presets for screens, paper (A
  sizes, Letter, Legal, Tabloid) and photos, the paper and photo ones at 300 ppi. Changing
  the resolution keeps a size shown in a unit of length (the pixels follow), and one shown
  in pixels; rounding to whole pixels is done from the size the resolution change began
  at, so dragging the resolution does not drift.
- **Rotate:** quarter turns either way, half a turn, flip horizontally or vertically
  (exact for pixel objects), and Rotate by an angle, which can grow the canvas to fit.
- **The largest canvas** is 300,000 pixels a side (Photoshop's limit, for its
  large-document format; zooming out reaches 1/1024 so that it fits a window). A layer
  filling it would take 720 GB, so a canvas that large is for content that leaves most of
  it empty or uniform: those tiles cost nothing (a new white canvas is four tiles), and
  work needing more memory than is free is refused (see *Size limits*). Shown whole, it
  costs the view about what the screen shows (see *Compositing*).

## Export

Export writes PNG (8 or 16 bits) or JPEG (with quality, and the background that
transparency is flattened onto), at the canvas's size or a chosen one: any unit or
percent, aspect ratio locked by default, with a resampling method. Export never changes
the document. The file records the document's resolution (PNG `pHYs` in pixels per
meter; JPEG JFIF density in whole pixels per inch or per centimeter, whichever is nearer),
unchanged when exporting at another size, so a size typed in inches is the size the file
prints at. The image is made and encoded a band of rows (a tile's height) at a time and
streamed into the PNG or JPEG encoder, so exporting at the canvas's size holds one band,
never the whole image; at another size it holds the canvas once more (averaged in blocks
first when shrinking by 2 or more; otherwise composited as one, tiles alike made once,
cancellable between batches of tiles), with its edge pixels copied outward as far as the
filter reaches, which the bands are drawn from. A JPEG is at most
65,535 pixels a side (the format's limit): the dialog says so and offers no export past
it.

## Thumbnails

Each layer in the layers panel shows a thumbnail of its content within the canvas
(small, large, or off), and hovering a state in the history panel shows a thumbnail of
the image at that state. A thumbnail averages a few point samples of the composite per
thumbnail pixel, which costs a millisecond or two however large the image, so it is made
when first shown and remade only when what it shows changes.

## Text

Text objects stay editable permanently, including when placed into an existing layer.
**Area text**: dragging the box reflows the text. **Point text**: dragging scales it. Full
Unicode: shaping, bidirectional text, line breaking, font fallback (emoji, CJK).

## Adjustments

Levels, curves, hue/saturation, gradient map, and the channel matrix. Each adjustment can be
used three ways:

- as an **adjustment layer**, which changes the layers beneath it and stays editable;
- **applied once** to the selection, or to the whole layer when nothing is selected;
- **painted** with the Adjust brush (see *Painting and retouching tools*), optionally limited
  to matching colors by *Color match*.

### The adjustments and their settings

What each adjustment of milestone 5 offers. Throughout:

- **Channels are a chosen color model's** where an adjustment works on channels (Levels and
  Curves on RGB, on Lab's or OKLab's lightness, on any channel of any model), and colors are
  judged in OKLCh by default (hue ranges, vibrance, saturation), where equal steps look
  equal; the RGB-based way stays a choice.
- **Samplers** (black, gray and white points) and **on-image adjustment** (dragging on the
  image changes the setting for the color under the pointer) wherever they apply; the
  histogram is shown beside settings that read it; every adjustment's settings can be saved
  as named presets.
- **No legacy algorithms** are carried (Photoshop's legacy Brightness/Contrast, its legacy
  sharpening, Variations): one way each, done well.

The adjustments, in the order milestone 5 builds them:

- **Levels:** channel (composite or each); input black, white and midtone (gamma); output
  black and white; the three samplers; Auto, with its options (the method: contrast per
  channel, monochromatic contrast, finding dark and light colors, brightness and contrast;
  clipping fractions for shadows and highlights; target colors for shadows, midtones and
  highlights; neutral midtones snapped); clipping shown while a point is dragged with Alt.
- **Curves** (the curve editor of milestone 0): channel; points added, moved, deleted and
  typed (input and output); each point smooth or a corner; a curve drawn by hand, and
  smoothed; on-image adjustment; the samplers; Auto with Levels' options; the display (grid
  4×4 or 10×10, histogram, each channel's curve over the others, baseline, intersection
  lines, values as light 0-255 or as ink 0-100%).
- **Hue/Saturation:** range (all, or a hue family with its boundaries and fall-off widths
  dragged on the hue bar); hue, saturation, lightness; colorize (one hue, its saturation and
  lightness); on-image adjustment (a drag picks the range and changes saturation; with Ctrl,
  hue).
- **Gradient Map:** the gradient (color and opacity stops, midpoints), reversed, dithered;
  the color model it interpolates in (OKLab by default, sRGB as Photoshop does, or smooth
  through the stops).
- **Channel Matrix** (the Channel Mixer, and more): see *Channel matrix*.
- **Brightness/Contrast:** brightness and contrast, worked on perceptual lightness so that
  extremes roll off rather than clip.
- **Exposure:** exposure in stops, offset, gamma; in linear light.
- **Black & White:** a weight for each of reds, yellows, greens, cyans, blues and magentas,
  or finer, a curve over hue; tint (hue, saturation); on-image adjustment; Auto (weights
  from the image's colors); presets.
- **Vibrance:** vibrance (muted colors more, skin tones protected) and saturation.
- **Color Balance:** the tonal range (shadows, midtones, highlights, where they split
  adjustable); cyan-red, magenta-green and yellow-blue; preserve luminosity.
- **Shadows/Highlights:** for shadows and for highlights each, amount, tonal width and
  radius; color correction; midtone contrast; black and white clipping.
- **Color Lookup:** 3D lookup tables (`.cube`, `.3dl`), abstract and device-link ICC
  profiles (through the color management of milestone 5); the order of the table's data
  where a format needs it; trilinear or tetrahedral interpolation; the files loaded kept in
  the image, so that it looks the same on another computer.
- **Photo Filter:** a preset (warming and cooling filters, and colors), or any color;
  density; preserve luminosity.
- **Invert** (all channels, or lightness only), **Threshold** (the level, on the histogram;
  by luminance or a chosen channel; an edge width, so that the edge can be smooth),
  **Posterize** (levels per channel; even steps in a perceptual space, or in RGB), and
  **Desaturate** (to lightness, in linear light).
- **Auto Tone, Auto Contrast and Auto Color:** Levels' Auto, each with its method, as
  commands.
- **Clarity:** local contrast at a chosen scale (amount, radius), with halos held back.
- **Equalize:** brightness spread evenly; from the selection's histogram, applied to the
  selection or to all.
- **Solarize:** a threshold above which tones are inverted, and how softly.
- **Selective Color:** the range (reds, yellows, greens, cyans, blues, magentas, whites,
  neutrals, blacks); cyan, magenta, yellow and black; relative or absolute.
- **Develop:** see milestone 5 in `roadmap.md`; its settings are the raw developer's.
- **Replace Color** comes with color-range selection (milestone 8), whose sampling it uses:
  the sampled colors (added to and taken from), fuzziness, localized clusters and their
  range, the replacement (hue, saturation and lightness, or a color), and the mask shown.
- Not carried: Variations (live previews make it pointless); HDR Toning and Match Color come
  later (*Later* in `roadmap.md`).

### Channel matrix

Each pixel's color, in a color model the user picks, is multiplied by a 3×3 matrix of
weights, and an offset is added:

```
out = M · in + k          M: 3×3 weights;  k: one offset per channel
column j of M: where input channel j goes (into itself and into the other two)
row i of M:    what output channel i is made of
```

- **Two views of the same matrix:** by source ("where does this channel go", the columns)
  and by output ("what is this channel made of", the rows, as in Photoshop's Channel Mixer,
  which works only in RGB and CMYK).
- **Channels are normalized** before mixing (to 0..1, or −1..1 for signed channels such as
  Lab's a and b), so weights mean the same thing for channels whose units differ.
- **Hue angles cannot be mixed.** In models with a hue angle (HSL, HSB, LCh, OKLCh) a
  weighted sum of angles means nothing: 359° and 1° are neighbors but far apart as numbers.
  In those models the hue channel takes only an offset, which rotates hue. To mix hue with
  other channels, use a rectangular model such as Lab or OKLab (the rectangular forms of LCh
  and OKLCh), where rotating a and b shifts hue and scaling them changes saturation.
- The sum of each row is shown, and a *preserve lightness* option keeps lightness unchanged.
- **Out-of-range results** are clipped per channel, or brought into gamut by reducing chroma
  while keeping hue and lightness (setting). In RGB models the matrix acts on linear or on
  encoded values (setting).
- **Advanced:** any weight can be replaced by a curve, giving `out_i = Σ_j f_ij(in_j) + k_i`.
  With curves only on the diagonal this is the Curves adjustment.
- **Presets:** identity, channel swaps, grayscale conversions, sepia, color vision
  deficiency simulation (Viénot's matrices, which the colorpicker already uses), and the
  colorpicker's Channel Mix presets.

### Expression layers

An **expression layer** fills its pixels by expressions (*Expressions*), worked out for each
pixel: one for each component of a color model the user picks (any opal-color has: sRGB and
linear sRGB, HSL, HSB, Lab, LCh, OKLab, OKLCh), in that model's display units, and one for
alpha. It keeps its expressions, not its pixels, so it stays editable: it is worked out again
when they change, when the canvas changes, and when what it reads beneath it changes, as an
adjustment layer is. Select › Fill with Expression makes one within the selection, through
its mask (a soft edge blends), or over the canvas when nothing is selected; it can be
rasterized as any layer can.

- **What its expressions may use:**
  - `x`, `y`: the pixel's middle, from the canvas's corner, a length (so `x < 2in` works);
  - `u`, `v`: 0 to 1 across the selection's bounds (the canvas's, if nothing was selected);
  - `r`, `a`: the distance and angle from the selection's center (radial and conic fills);
  - `below`: the pixel of the layers beneath it, in the model chosen, as an adjustment layer
    reads it (`below.L * 1.1`, `below.h + 30`, `below.alpha`), so that it is a custom
    adjustment too;
  - `noise(x, y)` (smooth, with a seed) and `random(seed)`, the same for a pixel each time it
    is worked out;
  - what other expressions may: the canvas's and layers' bounds (`Logo.right`), units.
  - `let` bindings and `if … then … else`, added to the language for it, so that a value is
    worked out once for every component, and stripes, rings and thresholds can be written.
- **Fast enough for millions of pixels:** each expression is compiled once (its units and
  names resolved, its kinds checked) into a flat program over plain numbers, worked out a
  tile at a time on the background threads, with progress and Cancel (*Edits that take
  long*); later on the GPU, for a live preview at full size. The dialog previews on the
  screen's resolution first.
- **Out of gamut:** clipped per component, or brought into gamut in OKLCh (hue and lightness
  kept, chroma given up), the channel matrix's setting.
- **Errors** are marked in the expression where they are, as the box's fields mark them; a
  pixel an expression has no value for (a division by zero) is left transparent, and the
  dialog says how many there were.
- **Scripts:** `fill_expression` (the model, an expression per component, `alpha`), and the
  layer's expressions read and set as its other settings are.

## Filters

Filters (milestone F) are layers acting on what lies beneath them, or attached to one layer,
editable as adjustment layers are; or applied once to the selection. Several stacked make a
chain (the Filter Gallery's: chosen, stacked, reordered, turned off, deleted, previewed,
zoomed, saved as presets). Throughout:

- Radii and distances are lengths, in any unit and as expressions; centers and points are
  dragged on the canvas, not only typed.
- What a filter does where it reads past the image is a setting where it matters: wrap
  round, repeat the edge's pixels, or transparent.
- A randomized filter has a seed, so that it comes out the same each time it is worked out
  again, and a button for another.
- Previews at screen resolution first; the full render off the interface thread (*Edits that
  take long*). Repeat Last Filter applies it again with its settings (Ctrl+F), or showing
  them.
- Fixed versions of others (Blur, Blur More, Sharpen, Sharpen More, Sharpen Edges) are not
  carried: their adjustable forms do their work.

**Blur:**
- Gaussian: radius. Box: radius (across and down apart, if wanted). Average (no settings).
- Shape: a kernel of any shape from the shape library (milestone 4), its radius.
- Motion: angle and distance. Radial: amount, spin or zoom, the center.
- Surface: radius and threshold (edges kept). Smart: radius, threshold, and its mode
  (normal, edges only, edges over the image).
- Lens: the depth source (none, a layer's transparency or mask, a layer as a depth map; a
  depth estimated from the photograph later), the focal distance (clicked on the image),
  inverted; the iris (shape by its blades, radius, blade curvature, rotation); specular
  highlights (brightness, threshold); noise (amount, uniform or Gaussian, monochromatic).
- The blur gallery's, each placed on the canvas: field blur (pins, each with an amount,
  between them interpolated); iris blur (the focus's center, shape and roundness, the inner
  boundary that stays sharp, the outer where full blur begins); tilt-shift (the focus band,
  its transitions, rotation, distortion, symmetric or not); spin blur (center, ellipse,
  angle, feather; several); path blur (paths from milestone 4, speed, taper, centered, the
  ends' speed and shape; several). Shared: bokeh (amount, color, the range of light that
  makes it) and grain (uniform, Gaussian or grain; amount, size, roughness, color, kept out
  of highlights).

**Sharpen:**
- Unsharp Mask: amount, radius, threshold; on lightness only, or every channel.
- Smart Sharpen: amount, radius, noise held back; what it removes (Gaussian blur, lens
  blur, or motion blur at an angle); fading in shadows and in highlights (amount, tonal
  width, radius each).
- High Pass: radius (under Other in Photoshop).

**Noise:**
- Add Noise: amount, uniform or Gaussian, monochromatic; film grain (size, roughness, color).
- Reduce Noise: strength, details kept, color noise, details sharpened again, JPEG
  artifacts; per channel (strength and details kept for each).
- Dust & Scratches: radius, threshold. Median: radius. Despeckle (no settings).

**Distort** (centers dragged on the canvas):
- Twirl: angle. Pinch: amount. Spherize: amount; both ways, across or down. Zigzag: amount,
  ridges, style (around the center, out from it, pond ripples).
- Ripple: amount, size. Ocean Ripple: size, magnitude. Wave: generators, wavelength and
  amplitude (least and most), scale across and down, sine, triangle or square, randomized.
- Polar Coordinates: rectangular to polar, or back. Shear: a curve (the curve editor) the
  rows follow. Displace: scale across and down, the map (a layer or a file) stretched or
  tiled. Glass: distortion, smoothness, texture (frosted, blocks, canvas, tiny lens, or an
  image), scaling, inverted. Diffuse Glow: graininess, glow, clear amount (with the secondary
  color).
- Lens correction (geometric distortion, chromatic aberration, vignetting) is under
  *Photographic*, below.

**Pixelate:** Mosaic (cell size); Crystallize (cell size); Pointillize (cell size, the
secondary color behind); Color Halftone (largest dot, a screen angle for each channel);
Facet and Fragment (no settings); Mezzotint (dots, lines or strokes, fine to coarse).

**Stylize:** Emboss (angle, height, amount); Find Edges and Glowing Edges (edge width,
brightness, smoothness); Oil Paint (stylization, cleanliness, scale, bristle detail;
lighting on or off, its angle and shine); Diffuse (normal, darken only, lighten only,
anisotropic); Wind (wind, blast or stagger; left or right); Tiles (how many, how far they
shift, what fills the gaps: the secondary color, the primary, the image inverted, or as it
was); Trace Contour (level, the edge above or below it); Solarize (see Adjustments).

**Render:** Clouds and Difference Clouds (the primary and secondary colors; scale, detail
and seed added); Fibers (variance, strength, seed). Lighting Effects and Lens Flare come
later; flames, trees and picture frames are not planned.

**Photographic:** Vignette (amount, midpoint, roundness, feather, highlights kept; its center
the canvas's center by default, an expression, so that it stays centered when the image is
cropped); Glow (the Orton effect: a blurred, brightened copy blended over; amount, size,
brightness); Lens Correction (barrel and pincushion, chromatic aberration, vignetting; purple
and green fringes, each by its range of hues and an amount; by hand, or from lens profiles,
milestone P's: *Photographs*).

**Other:** Custom (a convolution matrix, 5×5 or larger, its scale and offset, saved and
loaded); Minimum and Maximum (radius, keeping squareness or roundness); Offset (across,
down, and what fills: wrapped round, the edge repeated, or transparent).

**The artistic gallery** (later, `roadmap.md`): the chosen few, each with its settings as
Photoshop has them: Watercolor (brush detail, shadow intensity, texture), Colored Pencil
(width, pressure, paper brightness), Charcoal (thickness, detail, balance), Ink Outlines
(stroke length, dark and light intensity), Sumi-e (stroke width, pressure, contrast), Cutout
(levels, edge simplicity and fidelity), Poster Edges (thickness, intensity, posterization),
Graphic Pen (stroke length, balance, direction), Halftone Pattern (size, contrast; dots,
lines or circles), Bas Relief (detail, smoothness, light direction), Stained Glass (cell
size, border, light), Mosaic Tiles (tile size, grout width and lightness), Craquelure
(spacing, depth, brightness), Grain (intensity, contrast, and its kinds), Texturizer
(texture, scaling, relief, light direction, inverted). The others (Chrome, Plastic Wrap,
Note Paper, Plaster, Reticulation, Stamp, Torn Edges, Water Paper, Conte Crayon, Chalk &
Charcoal, the remaining Brush Strokes and Artistic ones, Patchwork, Extrude) are not
carried, nor the video filters (de-interlacing, NTSC colors).

## Photographs

What a raw developer such as Lightroom offers, built as Opal Studio's own pieces rather than as
a second application inside it: milestone P in `roadmap.md`, and items of N, 5, F, 6, T and
*Later* (metadata, comparing, Develop's sections, masks that keep their recipe, looks, spots,
versions, Upright, the Browse panel). Decided so far; the rest is settled when P begins.

- **Not a catalog.** Opal Studio edits images and keeps no database of photographs. Choosing
  among many, rating them and finding them again is a catalog's work (digiKam, darktable,
  Lightroom), and Opal Studio works beside one: the Browse panel (*Later*) shows folders as
  they are, keeps ratings and labels in XMP where those programs read them, and Opal Studio
  can be the external editor they open a photograph in.
- **The Photo tab's commands are made of ordinary things.** Brighten Subject, Whiten Teeth,
  Darken Sky and the rest are shortcuts: each adds what could be made by hand, an adjustment
  or filter layer named for the command, its settings the command's defaults, masked by a
  recipe whose maker step finds what it acts on (the subject, the sky, a person's teeth).
  Nothing is hidden (Principle 2): what a command made is edited as any layer is, and the
  defaults it uses are a preset to change (Principle 5). They have a tab of their own so that
  they read as shortcuts and leave Adjust, Select and Filter to the regular commands; command
  search and scripts find them as they find any other. A user's own are made the same way:
  layers whose masks keep their recipes, saved as a command.
- **Masks that keep their recipe.** A mask made from a selection keeps the recipe it came
  from beside its pixels. When asked, and when its layer is copied into another image, it is
  worked out again: an AI step finds the new image's subject (or sky, or teeth), a color or
  lightness step reads the new pixels, and a shape is carried to the new canvas. This is what
  lets a look, copied adjustments or a Photo-tab command work on a second photograph as they
  did on the first (Lightroom's adaptive presets). What a painted step (Quick Mask's,
  milestone 3) becomes in another image is **TBD**.
- **Raw objects.** A raw file opens as a raw object: an image object whose pixels are
  developed from the raw data it keeps, in the native file too (the file grows by the raw
  file's tens of megabytes). What needs the sensor's own data belongs to the object and stays
  editable at any time (Principle 1): demosaicing, white balance on the sensor's channels,
  highlights rebuilt, the camera's color and profile, the lens profile. Everything after it
  (tone, color, detail, effects) is a Develop layer clipped to it, the same Develop that works
  on any layer, so that a JPEG and a raw file are edited alike. The object gives linear
  floating point in the working space, nothing clipped. Settings a camera or an ISO range
  starts with: Preferences.
- **Develop is one adjustment layer**, Lightroom's sections in one panel, each of which can be
  turned off; its presets keep all of them or the ones chosen. A look's amount moves each
  setting from its neutral value toward the look's, or past it.
- **Metadata** is kept as it was read (EXIF, IPTC and XMP kept whole, and read for showing
  and editing), and written on export as export's choice says. The orientation is written as
  upright, since the pixels already are; the size is the exported one; an embedded thumbnail
  is remade or dropped. The choice names what gives away the place and the camera.
- **Lens profiles:** lensfun's library is LGPL-3.0 and needs GLib, so its database (XML,
  CC-BY-SA 3.0) is read by code of our own. Its models are few and documented (distortion:
  poly3, poly5, PTLens; chromatic aberration: linear, poly3; vignetting: pa). The database
  ships as data, credited with the licenses.
- **Demosaicing** methods are ported from darktable and RawTherapee (GPL-3.0, as Opal Studio
  is); rawler (LGPL-2.1) reads the files.
- **Models**, downloaded and shown as milestone A's are (*AI object and paint-the-area
  selection*): Depth Anything V2 Small (Apache-2.0); MediaPipe's multiclass selfie segmenter
  (background, hair, body skin, face skin, clothes, accessories) and Face Landmarker (478
  points, the irises included), Apache-2.0 by their model cards, converted from TFLite to
  ONNX; a person detector (RT-DETR and D-FINE are candidates); SAM 3, optional, for a
  landscape's parts. Better ones offered beside them under non-commercial terms (as that
  paragraph says): Depth Anything V2 Base and Large (CC-BY-NC-4.0); a face parser trained on
  CelebAMask-HQ (19 parts at 512 pixels: skin, brows, eyes, lips, the mouth's inside, hair,
  ears, neck, clothes), finer than the landmarks' outlines; SegFormer trained on ADE20K
  (NVIDIA's non-commercial license), which finds water, mountains, trees, grass, ground and
  buildings without SAM 3. Each license, and its training data's terms, checked when P
  begins.

## Painting and retouching tools

### One engine; tools are presets of it

Every painting and retouching tool is a preset of one brush engine, so every brush setting
(dynamics, blend mode, *Sample from*, the selection limit) works in every tool it applies
to. A stroke is defined by:

- **what it paints with:** a color, a gradient, a pattern, a clone source, or an earlier
  history state;
- **how it combines** with the layer: a blend mode, erasing, replacing channels, an
  adjustment, blur/sharpen, or paint mixing;
- **where it may act:** always within the selection mask, and optionally only where colors
  match (*Color match*, below);
- **how it responds** to the pen: dynamics.

### No hidden tool groups

Photoshop hides three or four tools behind each toolbar button and its shortcut, so pressing
B can leave you on the Pencil when you expected the Brush. In Opal Studio:

- Each tool button shows what it will do: its icon reflects the active variant, and its
  tooltip names it. A tool's variants are a visible control at the top of the *Tool
  settings* panel, never a flyout.
- A shortcut selects a tool; it never cycles through hidden variants.
- A tool's variants are one of its settings, a choice (`ToolInfo::variants` names it, with
  an icon for each option), so presets keep the variant too. The Tool settings panel shows
  it as a row of buttons above the other settings, and leaves it out of their list; the
  tool's button, the panel's heading and the status bar show the variant's icon.
- The cursor shows the active variant.

| Tool | Variants | Covers Photoshop's |
|---|---|---|
| Brush | Paint · Pencil (antialiasing off) · Replace color | Brush, Pencil, Color Replacement; Pattern Stamp, through "paints with: pattern" |
| Mix | Wet paint · Smudge | Mixer Brush, Smudge |
| Adjust | Lighten · Darken · Saturate · Desaturate · Any adjustment | Dodge, Burn, Sponge |
| Detail | one amount, from blur to sharpen | Blur, Sharpen |
| Clone | | Clone Stamp |
| Heal | Picked source · Automatic | Healing Brush, Spot Healing Brush |
| History brush | | History Brush |
| Eraser | Brush · Background · Click | Eraser, Background Eraser, Magic Eraser |

### Brush settings

Basic: size, opacity, flow, hardness, tip shape, what it paints with.

Advanced:
- **Tip falloff:** a curve of opacity against distance from the tip's center. The hardness
  slider is a preset of this curve. Roundness, angle, and image tips.
- **Spacing** between dabs (as a percentage of size), scatter, jitter.
- **Dynamics:** pressure, tilt and speed each drive size, opacity, flow, hardness or angle
  through an editable response curve with its own minimum and maximum.
- **Smoothing** (stroke stabilization): method and strength. Not the same as antialiasing.
- **Antialiasing:** amount; 0 is the Pencil. A pixel-perfect option removes the doubled
  corner pixels of 1-pixel lines.
- **Build-up** (airbrush): how fast paint accumulates while the pen holds still.
- **Color dynamics:** variation per dab of each channel of a chosen color model, each with
  its own range and distribution; and foreground-to-background color along the stroke,
  interpolated in a chosen color model.
- **Compositing space:** linear light, OKLab, or sRGB-encoded (how Photoshop blends by
  default, for matching its results).
- **Patterns:** scale, rotation, offset, anchored to the canvas or to the stroke's start. A
  pattern can be made from the selection.

### Blend modes

Brushes and layers share one set: Normal, Dissolve; Darken, Multiply, Color Burn, Linear
Burn, Darker Color; Lighten, Screen, Color Dodge, Linear Dodge (Add), Lighter Color; Overlay,
Soft Light, Hard Light, Vivid Light, Linear Light, Pin Light, Hard Mix; Difference,
Exclusion, Subtract, Divide; Hue, Saturation, Color, Luminosity; Channels (below). Brushes
add Behind (paints only where the layer is transparent) and Clear.

The picker describes each mode in a line, in terms of what it is for (Multiply: "darkens;
for painting shadows"; Soft Light: "gently lightens or darkens; for shading"), and previews
it live on the canvas while hovered.

Advanced:
- **Blending space:** linear light, sRGB-encoded (what PSD files expect), or OKLab.
- **Hue, Saturation, Color and Luminosity** work in a chosen color model. The default is
  OKLCh, where Luminosity shifts no hues. The W3C/PDF definition, with its fixed luma weights
  0.30, 0.59 and 0.11 (themselves adjustable), is an option for matching PSD files.
- **Soft Light formula:** Photoshop's, Pegtop's, or W3C's.
- **Channels:** pick a color model; each channel of the result comes from the base, from the
  blend, or from a mix of the two weighted by a curve. Hue, Saturation, Color and Luminosity
  are special cases of it.

**Blend If** (layers): curves over channels of a chosen color model, applied to this layer's
pixels and to the pixels beneath it, scale the layer's opacity per pixel. It is Photoshop's
Blend If, generalized from two-point ranges to curves in any color model, and it uses the
channel curve editor.

### Color match

A limit shared by Replace color, the Background and Click erasers, and the magic wand:

- **Sampling:** continuously under the brush center, once at the start of the stroke, or
  fixed colors.
- **Extent:** contiguous, anywhere, or contiguous but stopping at edges (edge threshold and
  color model adjustable).
- **Matching:** curves per channel around the sampled color in a chosen color model,
  combined by the strictness slider (the machinery of color-range selection); or distance
  from the sample in OKLab, with an editable falloff curve.
- **Protected colors,** each with its own tolerance.
- Matches are fractional, so edges are soft, instead of a yes/no match smoothed by
  antialiasing.

### Each tool

- **Replace color:** color match, plus the Channels blend replacing chosen channels of a
  chosen model. Replacing OKLCh hue and chroma while keeping lightness keeps the texture and
  shading.
- **Mix** (wet paint, smudge): wet (how much canvas color the brush picks up), load (how much
  paint the brush holds), mix (the ratio of the two), flow. Advanced: the mixing model, which
  is linear RGB, OKLab, or pigment (Kubelka–Munk spectral mixing, where blue and yellow make
  green; spectral.js, MIT, is a reference implementation); pickup and depletion rates;
  cleaning or reloading the brush after each stroke; for Smudge, how far color is dragged
  and how it fades along the way (a curve).
- **Adjust:** paints an adjustment where the brush goes. Lighten, Darken, Saturate and
  Desaturate (Photoshop's Dodge, Burn and Sponge) are presets; *Any adjustment* paints any
  of the *Adjustments*, the channel matrix included. Settings: strength and tonal range.
  Advanced: the tonal range is a curve over the lightness channel of a chosen model, with
  Photoshop's fixed shadows, midtones and highlights ranges as presets; which channels count
  as lightness and as saturation; how Lighten and Darken apply (exposure-style
  multiplication in linear light, or addition to the channel); protect tones (a
  soft-clipping curve, and a hue lock); for Saturate, a curve of how strongly
  already-saturated colors are affected (vibrance). Option: paint into an adjustment layer's
  mask instead of the pixels, so the effect stays editable.
- **Detail:** one amount, from blur to sharpen. Advanced: blur kernel (Gaussian, box, lens)
  and radius; sharpening radius, amount and threshold (unsharp mask); the channels it acts
  on in a chosen model (by default lightness only, which avoids color fringes); halo
  suppression.
- **Clone:** two initial points define the source offset; adjustable brush size; aligned or
  non-aligned source; a ghost preview of the source under the cursor; source rotation, scale
  and flip; overlay opacity; *Sample from*.
- **Heal:** like Clone, but blends the copied texture into its surroundings (gradient-domain
  blending). Advanced: width of the blending border; diffusion; which channels of a chosen
  model are matched to the surroundings. The Automatic variant (spot healing) needs no
  source: paint over a blemish. Its method (nearby texture, PatchMatch, later the LaMa
  inpainting model), search radius and patch size are adjustable.
- **History brush:** paints from any history state on any branch, picked in the history
  panel, or from a named snapshot. It can bring back part of the image from a branch you
  abandoned. The source is the same layer in that state (matched by the layer's ID) or that
  state's whole image.
- **Eraser:** erases to transparency, or optionally into the layer's mask, which keeps the
  pixels. *Background* uses color match sampled continuously, and decontaminates the color
  of partially erased edge pixels, as hair matting does. *Click* erases the region that a
  click matches, contiguous or anywhere: the magic wand and Delete in one step, with soft
  edges.

**Content-aware fill** (later): PatchMatch, or LaMa.

**Auto-stroke brush** (later, low priority): repaints from a source state with automatically
generated strokes, like Photoshop's Art History Brush; style, area, tolerance and stroke
length adjustable.

## Calligraphy

A **Calligraphy** tool draws strokes the way a nib does: the stroke's width comes from the
nib's shape and angle relative to the direction of travel, and from pressure.

- **Each stroke is an editable vector object**: its centerline, plus the nib and its
  settings. The outline is the area the nib sweeps along the centerline, so the stroke is
  sharp at any zoom, and afterwards the centerline can be reshaped (as selection outlines
  are), and the nib, angle or width changed, with the stroke redrawn. On a raster layer it
  can also be painted into the pixels.
- **Variants** (the tool's visible variant control):
  - *Broad-edge*: a flat nib at a fixed angle (italic, blackletter, uncial). The thick and
    thin strokes come from direction alone, so it works fully with a mouse.
  - *Pointed*: width from pressure (copperplate, Spencerian), from speed with a mouse.
  - *Brush*: a round, soft tip that spreads with pressure (brush lettering).
  - *Ink brush*: a Chinese calligraphy brush (see below).
- **Settings:** nib width; nib angle; *fixation* (how firmly the nib keeps its angle rather
  than turning with the stroke, from 1, fixed, to 0, always across it); thinning by speed;
  pressure-to-width curve; mass (inertia that steadies the hand) and smoothing; tremor;
  stroke ends (square, round, tapered). Advanced: the nib's own shape (flat, elliptical, or
  an image), its thickness, and the response curves.
- **Pen input:** tilt and barrel rotation can set the nib angle (pens that report them).
- **Guide sheet:** a non-printing overlay of the lines calligraphers rule: baseline, x-height,
  ascender and descender lines and slant lines, sized in nib widths (italic's x-height is
  about five nib widths), with the nib-width ladder drawn at the margin. The slant angle, the
  proportions and the spacing are settings, with presets for common hands. For Chinese
  calligraphy, the practice grids: 田字格 (four squares), 米字格 (with diagonals) and 九宫格
  (nine squares), in columns for vertical writing.

### The ink brush

Chinese calligraphy depends on the ink in the brush as much as on the stroke. Its four
qualities of ink, 浓淡干湿 (dense, light, dry, wet), are each part of the brush:

- **Bristles.** The brush is a bundle of bristles: pressing spreads the tip from a point to
  the full belly, tilt moves between center-tip (中锋) and side-tip (侧锋) strokes, and the
  bristles splay and gather as the stroke turns. Without pressure (a mouse), speed stands in.
- **Ink load and running dry.** Each bristle carries ink that the stroke uses up, faster
  under pressure and at speed. As bristles run dry they leave paper showing through in
  streaks: 飞白, "flying white", the dry-brush texture prized in calligraphy.
- **Adding ink.** The brush is reloaded by dipping (a key or button), or automatically at
  the start of each stroke, or every few strokes (setting); how much it takes up is a
  setting.
- **The ink can.** Ink also comes from pots on a small dish at the edge of the canvas, which
  is never printed or exported: dense ink (浓墨), light ink (淡墨) and clear water. Moving the
  brush into a pot and lifting it out dips it, as with a real brush: ink loads it at that
  pot's density, and water thins what the brush holds. A small gauge beside the cursor shows
  how much ink is left in the brush, so a dry stroke can be planned for. More pots, each with
  its own density, can be added; the dish can be moved or hidden. (The idea, and its name,
  come from a friend's pun: "dry is good, add ink can too".)
- **Density and wetness.** Ink density (浓/淡, from thick black to pale gray) and wetness
  (湿/干) are separate settings: wetness is how much water the brush holds, which governs
  how far ink spreads into the paper.
- **Paper.** Absorbent raw rice paper (生宣) lets wet ink bleed into the fibers with soft,
  feathered edges; sized paper (熟宣) keeps strokes crisp. The paper's absorbency and
  grain texture are settings (bleeding follows a diffusion model in the spirit of MoXi,
  Chu and Tai's ink dispersion in absorbent paper).
- **Still editable.** A stroke is recorded as its centerline with the pressure, tilt and
  speed at each point, the brush and ink settings, and the random seed of its bristles, so
  it renders the same way every time and can be re-rendered with other ink, density or
  wetness afterwards.


## Undo history tree

- History is a tree: a new action after an undo starts a branch instead of discarding the
  redo states.
- Ctrl+Z and Ctrl+Y walk the current branch; at a fork, redo follows the branch last visited.
  The history panel shows the whole tree, with an automatic description per entry and a
  thumbnail on hover.
- Selections are part of history; zoom and pan are not. A slider drag is one entry: while
  it lasts the change is *pending* (shown live, not yet in the history) and it is committed
  as one state on release, or as none if it came back to where it began (`Document::same_as`:
  its layers each the same layer, or alike in every setting with the very same bitmaps).
- Which layer is active is not part of history. After an action or a move through history,
  a layer that appeared (one the action added, or one undo restored) becomes active; if the
  active layer disappeared, the layer now in its place does.
- The tile store makes branches cheap. History can optionally be saved in the file. How much
  memory history may use is a setting (4 GB by default): what the other states hold that
  the current one does not, counted once per tile, with the reduced copies their images
  keep. Tiles are counted by the chunks images keep them in, so committing an edit walks
  the tiles of the chunks it changed, not every tile of the layer (a pixel of a
  300,000-pixel canvas: under a millisecond). Past it, the oldest states are dropped first: the root when it has one child
  (which becomes the root), otherwise the oldest state with no children, so the history
  stays one tree. The current state, the one before it and every state redo leads to from
  it always stay, so the last action can be undone however large it is, and undoing to
  look back never loses the newest work. Moving through history trims as committing does
  (the state left behind may put the others over the limit), from the oldest states and
  abandoned branches only; past the limit with nothing else to drop, history waits for
  the next action. The History panel shows what history holds, and the first drop is
  announced, once the status bar is free (it neither hides another notice nor is hidden).
- Every state on every branch stays usable as a source for the History brush.

## Vectorization (raster to vector)

Heuristic, not ML. Parameters: how different colors must be to start a new region (measured
in a chosen color model), speckle filter, corner threshold, curve-fitting error tolerance
(how far a curve may deviate before it is split into more detailed curves), and minimum
segment length. VTracer (MIT) is the model for color images; Potrace (GPL) is usable for
two-color images. Works within the selection like any other operation.

## Color picker

A port of the author's colorpicker (`D:\visual studio projects\colorpicker`, MIT): its color
engine and shaders translate to Rust and WGSL, and its panels are rebuilt in egui. OKLab and
OKLCh are added. The color engine is ported first, in the foundation milestone, because the
channel curve editor and every setting defined in a chosen color model depend on it. Its
palette editor becomes the gradient editor, used by the gradient tool, a gradient-map
adjustment and indexed-color export.

## Symmetry

One setting, shared by vector shapes, the brushes and the outline tool: symmetry of
**order n** (n copies around a center; 1 for none), with or without a **mirror**, about a
**center** and an **axis angle**. Order 1 with a mirror is one mirror axis; order 2 with a
mirror, both axes; order n alone, rotational symmetry of order n; order n with a mirror, a
kaleidoscope of 2n parts.

- **Vector shapes (milestone 4).** A shape given symmetry is edited as one slice (1/n of it,
  or 1/2n with a mirror); the rest is made from it and joined into one closed path, so it
  stays exactly symmetric without a solver (as Inkscape's mirror and rotate-copies path
  effects keep theirs).
  - The center is a point kept with the shape, where its middle was when symmetry was
    turned on, not its bounding box's middle, which moves as it is edited and would make it
    drift. The center and the axis are moved by handles on the canvas.
  - A node on a seam (an axis) stays on it, its handles mirrored so that the joined curve
    is smooth there, unless it is made a corner.
  - The slice is clipped to its part, so that the copies do not overlap; an option lets it
    cross, the fill rule deciding what the overlap fills.
  - *Expand symmetry* makes it an ordinary path: how symmetry is broken, and what is
    exported to SVG, which has no such thing.
  - A calligraphic stroke reflected has its nib's angle reflected; an option keeps the
    angle as drawn.
  - Symmetry between separate objects is the constraint solver's (*Constraints*:
    symmetric).
  - Expressions can name the center (`Badge.symmetry.center`).
- **Painting (milestone 3).** A stroke is repeated as the symmetry says while it is
  painted, about a center and an axis shown on the canvas and moved by dragging them. Every
  tool built on the brush engine takes it, the retouching tools too.
- **The outline tool (milestone 2).** An outline drawn with symmetry is made and closed as
  a shape's is: symmetric selections.

## Expressions

One small language for every number and length the editor takes, typed into a field or
given to the API, and later for the constraints between objects (*Constraints*). It
extends the lengths the API takes now (*Scripting*), and the fields of the boxes'
numbers (*Paste and transform*): a length such as `8.5in` is the simplest expression.

- **What it has:**
  - Numbers with units of their own, mixed (`2in + 3mm`). A bare number added to a length
    is in the field's or the call's unit; one multiplying or dividing it is a plain factor.
    A length times a length is an error, as is any other mismatch of kinds (length, angle,
    plain number). Angles are in degrees unless given in `rad`.
  - Arithmetic (`+ - * /`, `^` or `**` for powers, parentheses; `×`, `÷` and `−` too), and
    functions: `min`, `max`, `abs`, `round`, `floor`, `ceil` (to whole pixels or degrees,
    or to a multiple of a second argument: `round(x, 1mm)`), `sqrt`, `sin`, `cos`, `tan`,
    `asin`, `acos`, `atan`, `atan2`, and `pi`.
  - References:
    - the canvas, and any layer by name, as bounds: their edges `left`, `right`, `top`,
      `bottom` (`x` and `y` are the left and top), `width` and `height`, and their points
      `top_left`, `top_right`, `bottom_left`, `bottom_right` and `center` (`Logo.right`,
      `canvas.center.x`). An edge's middle is `point(Logo.center.x, Logo.top)`: `top` is the
      edge's coordinate, as `-` in `bottom-left` would be a minus. A name with spaces, or
      one that clashes with a word of the language, is quoted, `layer("Back ground").top`,
      and a layer may be named by its id, `layer(12)`; a name in another case is found if
      only one layer has it, and two layers of one name are an error, never a guess;
    - the box being placed as it was before this change (`x`, `y`, `width`, `height`,
      `angle`);
    - the image's resolution (`ppi`);
    - later, paths' nodes and text boxes' corners (milestones 4 and 7);
    - points as values, with `mid(A, B)`, `lerp(A, B, t)` (of lengths too), `dist(A, B)`
      and `point(x, y)`, so "halfway between two nodes" is `mid(A, B).x`.
  - Positions are measured as the field's are: from the point its box counts from (the
    canvas's top-left corner unless *From* says), so `Logo.right + 5mm` in the x field puts
    the box 5 mm right of the logo however it is counted. Lengths (`Logo.width`) are the
    same from anywhere.
  - Limits, for text from scripts and files: at most 4096 bytes, nesting 64 deep, and
    lengths and angles to powers of at most 64; anything past them is refused, never taken
    down the stack.
- **Its own language, not Python.** The constraint solver must differentiate each
  expression to solve with it, so expressions are parsed into a tree the editor
  understands. That also makes them instant, safe, and independent of any Python install.
- **Names resolve to stable ids.** A reference is resolved to the object's id when the
  expression is entered, and displayed by its name. Renaming a layer renames it in a
  remembered expression. A reference to something deleted makes the expression an error
  that names what is missing; it is never silently redirected to something else.
- **Worked out once, or kept.**
  - In a field or a call, an expression is worked out once against the document as it is
    then (when Enter is pressed, or when the call runs), and the result is a plain number.
  - A field being typed in keeps its size, so that nothing beside it moves: a longer
    expression scrolls in it, and the preview below it shows the expression whole, with
    what it comes to, as you type. An error says where it is,
    the part at fault marked. What was typed is entered by Enter, Tab, or leaving the field
    another way; Enter on what cannot be worked out keeps it, to be put right, and Esc
    leaves the value as it was. Left with nothing typed, the value stays exactly as it is,
    not as it shows rounded.
  - Each field remembers its last 20 entries (Up and Down). One given a name
    (`NumberField::remember`: `box.x`, `size.width`, a preference's key) shares them with
    every field of that name, so the tool settings' x and the numbers panel's x share
    theirs, and keeps them between sessions, with the settings (`AppState::field_entries`,
    read in through egui's memory at start and saved when they change); one without keeps
    them for the session. A length's are kept apart in each unit (`box.x.px`, `box.x.in`),
    and a resolution's in each of its units, as a bare number in them is in the unit it was
    typed in: brought back in another, it would mean another length. The rule of "the
    latest first, each once, so many at most" is `recent::latest_first`, which the recent
    scripts keep to as well.
  - The fields are `number_field::NumberField`, dragged as egui's `DragValue` is (and with
    its accessibility), on every number the interface takes: lengths (`lengths::length_field`),
    angles, percentages, resolutions, the zoom and the layer's opacity, and the
    preferences' numbers beside their sliders. Each is given an `expr::Scope`: a box's
    fields the image as it was before the box opened, positions from what it counts from,
    and the box; a dialog's the image it is about; one with no image, units alone.
  - Keys pressed while a field has the keyboard are the field's: Esc there leaves the
    field, not the box the field is about (egui lets go of the focus on Esc before the
    frame's keys are read, so the app remembers who had them last frame).
  - A placement remembers the expressions it came from (`Layer::placed`, a `Placement`).
    The box's session keeps each of its numbers given as an expression, typed into its
    fields or given by a script, with how it was measured (`transform::Given`: the unit, the
    box's point, what it counts from, and for a side whether the other kept its
    proportion). When the box is applied, those that name the image (the canvas, a layer)
    and still give what the box has then (a drag since has not moved it) are kept with the
    layers it places (`Session::placed`: a paste's new layer, or the layers selected to
    transform, a group as itself), their layers by id (`Placed`), the last of each number.
    Not kept, as working it out again would move the box again: a number naming the box
    (`x + 1cm`), naming a layer that moves with it (one it places, in one, or holding one),
    or counting from such a layer. Nor one naming nothing of the image, which would come
    out the same, nor the sides and position of a rectangle the box was fitted into
    (`fit` inside or cover: they are the rectangle's), nor a number whose kept form would
    not read back. A box left where it was but given a placement its layers have not is
    applied all the same. Placed by a box again (moved, or new), the layers keep what that
    box was given, or nothing; only drawn another way where they are, moved otherwise (the
    Move tool), renamed, saved and opened, they keep it; a copy has none. A file's
    placement is refused if it names the box itself, which none ever kept does.
  - *Re-apply Placement* (Home › Transform, and `reapply_placement` for scripts) opens the
    box on the layers placed together with the active one (`transform::placement_group`),
    gives it the numbers again, each as it was measured and worked out now, and applies it
    as one history state. It is refused, changing nothing and leaving the selection as it
    was, if one of those layers has gone or been placed again since (the box they made is
    no more), is locked, or a layer it names or counts from is gone. With the box open on
    exactly the layers placed together, its numbers list how (*Placed by*, the layers by
    their names now) with a Re-apply that does the same in the open box.
  - *Re-apply All Placements* (Home › Transform, `reapply_all_placements`) re-applies every
    set of layers placed together, as one history state, each after the sets it is placed
    relative to (those the layers it names or counts from move with): the sets ordered by
    their strongly connected components (`transform::components`, Tarjan's), so a layer
    placed by another that is placed by a third follows both. A component of more than one
    set is a loop: placed again, each would move the other, and no order places them all
    as they say, so its layers are left as they are and told (placing one of them with
    the box again breaks it); a set placed relative to one of them is placed by it as it
    is. A set whose layers are in, or hold, another's counts as placed relative to it both
    ways (placing either moves the other), so the two are such a loop too. Sets that cannot
    be placed again, or are locked, are left and told too, as are sets naming, or counting
    from, a layer that moves with them now (one since put into a group they place:
    `placement_group` refuses them, for Re-apply too, as placing them again would move them
    again). The state it began from is held from history's trimming
    (`History::hold`) until the states each set made are joined, so undoing it gets back
    there; what placing the sets said (that some stay images, say) is told with the rest,
    and the selection is left as it was. Once constraints exist, *keep linked* turns a
    placement into a constraint without retyping, and a loop of them becomes something to
    solve.
- **The same from scripts.** Every number and length parameter of the API (Python and MCP)
  also takes an expression as text (*Scripting* has how).
  - The editor works it out when the call runs, with the same parser as the fields, so a
    script and the window give the same answer.
  - Because it is worked out inside the call, nothing can move between a query and the
    call that uses its answer, and an AI client needs one call rather than a query, its
    own arithmetic, and a call.
  - The Python package builds expressions with operators:
    `opal.layer(logo).right + opal.mm(5)` gives the text of one, naming the layer by its
    id. The editor's parser checks it; there is no copy of the parser in Python.
  - Live constraints will be made from scripts with the same expressions.

## Constraints

For mathematically exact vector design, and later for layouts that adapt to their size.

- A **unit** is a group with exactly one solver, decided by the kind of constraints placed in
  it and shown on the unit:
  - **Geometry solver** (nonlinear): coincident, tangent, parallel, perpendicular, symmetric,
    equal, distances and angles given by expressions (*Expressions*) such as
    `width = 2*r + gap`. It shows how many degrees of freedom remain, colors fully
    constrained geometry, and flags conflicts. Soft "spring" constraints are opt-in, each
    with an adjustable stiffness, and how far each is stretched is shown. The solver's
    tolerance is adjustable.
  - **Layout solver** (linear constraints with priorities, Cassowary-style): even spacing,
    grow to fit content, keep proportions, adapt to another aspect ratio.
- To mix solvers, nest units, to any depth. Between levels, limits go down, sizes come up,
  and the parent places the child.
- Constraints connect only objects within one unit, or anchor points a child unit exposes.
- **Kept expressions:** *keep linked*, by a field or as `linked` in a call, makes an
  expression a constraint that stays true as what it refers to moves. Like every
  constraint, it relates only objects within one unit. It uses the same
  syntax and parser as an expression worked out once. A placement's remembered expression
  can be made one too.
- The geometry solver comes first.

## Scripting

Opal Studio is scriptable, from Python first, and drivable by AI assistants through an MCP
server. Both rest on one language-neutral API.

- **One API, generated from the command registry.** Every command gets a parameterized
  form (Open takes a path, Export a path and options); the interface is one way of
  supplying the parameters, a script another. The registry's names, descriptions and
  parameter types generate the API's documentation, its Python stubs and the MCP tool list,
  so they cannot drift apart. Document queries (layers, history, pixels, later selections)
  join the commands.
- **Out of process.** The API is a JSON-RPC protocol over a local connection (a named pipe
  on Windows, a Unix socket elsewhere). Scripts run in the user's own Python with its own
  packages (numpy, Pillow, OpenCV, scikit-image, ML libraries), so nothing has to be
  embedded or bundled, any Python version works, and a crashing script cannot take the
  editor down. Pixel data travels through shared memory and arrives as numpy arrays without
  copying.
- **The protocol.** JSON-RPC 2.0, a request or response a line, over the pipe named in
  `api.json` in the user's settings folder, beside a random token (32 bytes from the
  system's generator) made at each start; the file goes when the editor closes. A
  connection's first request is `hello` with the token, within 5 seconds; anything else
  first, or the wrong token, is refused and the connection dropped. A command's method is
  its name in the registry in snake case (`save_as`, `merge_down`), its parameters each
  with a kind (a whole number, a number, a length, an angle, a whole number of pixels, one
  of some words, text that is not blank, a path, an image's or layers' ids, any value),
  and either a default or what the editor holds now (`document`: the active image;
  `layers`: the selected ones); `methods` lists them all, which is what the stubs and
  tools are made from. Each of the numeric kinds takes an expression (*Expressions*) as
  text too; that it reads is checked with the rest, and it is worked out
  (`Args::worked_out`) before the call is carried out, in the image the call acts on (else
  the active one), positions from its canvas's corner: the `resolution` first, as lengths
  are converted at it. A **length** (a size, a position, a margin) is a number in the
  call's `unit` (pixels by default), or an expression (`"8.5in"` is one), converted at the
  resolution the call gives (`new`, `resize_image`), else its image's, else 72 ppi; its
  range is checked in pixels, and a refusal says what the length came to (and what it was
  given as). An **angle** is in degrees; a whole number of **pixels** (a region's corner, a
  thumbnail's side) may be given as a length, and is rounded. A call that places a box
  (`paste`, `place`, `transform`, `crop`: those counting from `from_anchor`) leaves its
  `x`, `y`, `width`, `height` and `angle` expressions for the box (`api::Typed`): they are
  worked out once it is open (`BoxArgs::worked_out`), naming the box as it is then
  (`transform(x="x + 1cm")`), in the image as it was before it opened, from the point the
  call counts from. The Python package's `opal.expr` builds the same text with operators,
  parenthesized only where the editor's parser needs it, and sends an `Expr` as its
  text; the stubs type each numeric parameter as a number, text or an `Expr`, and the MCP
  tools as a number or a string. The
  commands that open the transform box take where to put it and apply it at once:
  `paste` and `paste_in_place` (answered once the clipboard is read and the paste placed;
  a paste asked for again, or cancelled, before then is told so), `place` (an image file),
  and `transform`, each with the box's `x`, `y` (its top-left corner from the canvas's),
  `width`, `height` (one alone keeps the proportions; both stretch the box's own sides to
  them, or, with `fit`, fit the box, turned as it is, inside or over the rectangle they
  make on the canvas, centered), the `anchor` (which point of the box, and of the rectangle, `x` and `y` give, and what a size
  or angle scales and turns about: the reference point's names), `angle`, and for a paste
  `into_active`; they answer with the layer and the box (at that point). What a box applies is made in the call's groups, as any change is (a script run, a
  step), the asynchronous paste's too. Changes the API alone makes: `put_pixels`, `crop` (a
  rectangle of the canvas, which may be turned), and `set_preference` (any but those of
  scripting, which would let a script or assistant choose what runs scripts or open the
  editor to HTTP clients; given no value, the default; applied at once, as the Preferences
  window applies it). Queries so far: `documents`,
  `preferences`, `layers`, `image` (of
  the image as shown or of one layer alone, made in memory on a thread of its own, out of
  the user's sight, unlike an export), `pixels`, and `history` (an image's history as a
  tree: each state kept, with its id, its label, when it was made, the state before it,
  those after and the one redo goes to; the state the image is in, the one last saved, and
  how many were dropped).
- **Pixels.** `pixels` reads a rectangle of the canvas (the whole of it by default), of the
  image as shown or of one layer alone (as if shown, at full opacity), as `srgb8` (bytes),
  `srgb16` (16-bit values) or `linear` (32-bit floats in linear light), always red, green,
  blue and alpha, straight, little-endian, row by row. It composites and converts on a
  thread of its own (its parallel work in the background pool), into a named section of
  shared memory (the user's own, named `Local\opal-pixels-<pid>-<random>`) that the answer
  names, kept until the caller releases it (`release_pixels`, which asks nothing of a
  section already gone) or for a minute; or, with `shared` false (and always on systems
  without shared memory yet), into the answer in base64. `put_pixels` takes them back the
  same two ways (shared memory the caller made, opened for reading and checked to hold what
  is said, or base64, within a request's 1 MB), read and made into tiles on a thread of its
  own (8-bit ones kept at 8 bits, for a new layer), then put in on the interface thread when
  the user is not busy, as a call waits (refused once they have asked to quit), as the call's
  script run if it was one: as a new layer above the active one, placed at `x`, `y` of the
  canvas, or into a layer holding nothing yet or pixels alone in one object, replacing them
  there (refused for a locked layer, a group, or one holding an image or several objects),
  as one history state. Linear values that are not numbers are taken as 0, and those past a
  half float's range as its largest (as infinity, compositing would make them NaN). The
  Python package wraps both as numpy arrays (`editor.pixels()`, `editor.put_pixels(array)`,
  the format from the array's type, by kind and size whatever its byte order: `uint8`,
  `uint16` or floats; three channels taken as opaque). It opens the editor's memory by
  `OpenFileMapping`, so that memory it cannot see (another Windows session's) is an error,
  not an empty section of its own, before releasing it. Where there is no shared memory, it
  sends a band of rows a request, each within the 1 MB, the first making the layer. These
  and `release_pixels` are left out of the generated stubs (written by hand) and of the MCP
  tools (an assistant sees an image through `image`).
- **Carrying out a call.** Between frames, as the interface would, without a dialog (a
  frame goes on with the calls that follow one another for a few milliseconds, looking for
  the next rather than sleeping, since a sleep shorter than the timer's tick lasts the
  tick; so a script makes thousands of calls a second, not one a frame). What a dialog
  would have said comes back with the call: an error or a refusal (which the interface
  tells as information) as its failure, a warning or a status-bar notice as its `notes`;
  for a notification, which no one answers, it is told in a dialog after all. A refused
  call leaves the user's active image and selection as they were; one that would change
  nothing (hiding a hidden layer) records nothing. A save, export or copy is answered once
  written. Paths must be absolute (the Python package makes them so against the current
  folder, as Python's `open` would); a save or export over a file already there needs
  `overwrite`, Save As adds `.opal` as its dialog does, and an export given one side gets
  the other in proportion, refused past the largest side allowed. While the user has a
  dialog open, a button held or a transform box open (an edit a command would commit
  under them), calls wait; once they have asked to quit, they are refused. A box a call
  opens is applied within the call (or cancelled, with the reason, if it cannot be), so
  none is left open for the user.
- **Script runs.** A `hello` may name a `script` (the Python package names the script
  running, but not an interactive session): the connection is then a script run, and once
  it closes, the states it made in each image one after another are joined into one,
  labeled `Script: <name>` (`History::join`: a line of them becomes its last state, in its
  first's place; what the script undid goes with it; a state the user made, or made
  something from, ends a line). The states stay separate while it runs, so a script's own
  undo takes back one step.
- **Steps.** `begin_step` (with a `name`) and `end_step`, which the connection's thread
  answers itself (their arguments checked as any method's), make what the calls between
  them change in each image one state, called as named. A call is recorded for each group
  it is in, its step and its connection's script run: a step's states are joined when it
  ends, and kept apart when the run's are (`History::join`'s `apart`), so a script's
  history reads as its run before a step, the step, and its run after (each line named for
  its group, even a line of one state); what a group undid goes, wherever it hangs off the
  way to the current state (beside a step too), so a step the script undid goes with the
  run. A step begun inside another is part of it; one left open ends with the connection,
  or, on a connection that is no script run (an assistant's, lasting its whole session),
  before the next call once it has gone 10 minutes without one, counted from when the last
  was answered (a forgotten `end_step` would otherwise take in all it does after; a
  script's ends with it, however long it thinks), an `end_step` coming after it told so in
  its notes; the status bar names the steps open, and a disconnect forgets them. The
  Python package wraps them as `with editor.step(name):` (ended even when the body fails,
  its error not hidden; a `begin_step` refused begins nothing, so ends nothing); an
  assistant has them as tools, to make several of its calls one undo.
- **Who is connected.** Disconnecting (the status bar) drops every client, refuses what
  they asked that is still waiting, makes a new token, and shuts scripts out until the user
  lets them in again (a client that rereads `api.json`, as the MCP shim does, cannot come
  straight back). A line from a client is at most 1 MB (4 KB before `hello`), and at most
  16 connections wait to say hello at once. Scripts reach the first editor started: a
  second one serves none until the first has gone (it looks every 2 seconds whether
  `api.json` is, and every 15 asks the first, which may have ended without removing it),
  and one started with files hands them to the first, over a `hello` with `files` (which
  may only open them, and is let in even while scripts are shut out), lets it come to the
  front, and quits; an editor that is quitting refuses every hello, so the new one opens
  them itself. Commands of the view, the panels and the windows are not callable: all a
  script needs to make and change images is, so none needs the window.
- **Python package** `opal-studio` (module `opal`): connects to the running editor or starts
  a headless one (`opal-studio --headless script.py` for batch work). *Built:* with no
  editor running (no connection file, or one whose pipe has gone), `connect()` starts one
  with no window, `opal-studio --serve FILE`, on a connection file of its own in the
  temporary folder, waits (up to a minute) for it to print `serving`, and connects; it ends
  when its standard input does, which the script's process holds, so it ends with the
  script however that ends (a crash, `os._exit`), once the files asked of it are written,
  and `disconnect` waits for that. `connect(windowless=True)` starts one even when the
  editor is running (batch work that leaves the user's images alone); `windowless=False`,
  or a connection file named, only connects. The program is `OPAL_STUDIO_EXE`, else the
  one in the folder the package is in (as built and shipped), else the one last started
  as a window, which notes its path in `program.json` beside the settings, else
  `opal-studio` on the `PATH` (`editor_program()`). *Built:* headless mode
  runs the editor's state and its server with no window, the user's preferences loaded
  (not saved back) and applied as the window applies them between frames (the history's
  memory, the tiles'), and runs the script (with `--python`, `OPAL_PYTHON` or the Python
  preference) with the package on its `PYTHONPATH` and a connection file of the run's own.
  It ends once the script has ended and every client of the run has gone (a worker the
  script started may go on calling), with the script's exit code, once the files asked for
  are written, printing what a dialog would have said. The script is given to Python as
  given, so `-c "code"` and `-` (standard input) work as they do for Python. What is done
  between frames is the window's (writes, copies, a paste whose clipboard has been read),
  so a script's paste is placed headless too; and with no window nothing waits for a
  dialog, which no one could close. Between calls it waits for the next (`Server::wait`)
  rather than looking for it again and again: every millisecond or so while work a call
  waits for is under way, every 50 ms otherwise (`headless::serve_windowless`, which the
  editor `--mcp` starts runs too). The editor being a windowed
  program, which a command prompt does not wait for, `opal-studio.com` beside it (the
  `opal-studio-console` binary, which a prompt finds first) runs it with the same
  arguments on the same console, waiting for `--headless` and `--mcp` and ending with
  their exit code.
- **In the editor:** a Run Script command, a console panel showing output and errors, and a
  preference for which Python interpreter to use. A script's changes are grouped into one
  history state, so one undo takes back a whole script run. *Built:* all of it. Run Script
  (File › Scripts) asks for a `.py` file and runs it in its own folder with the Python
  preference (Preferences › Scripting: a command on the `PATH`, with arguments if need be,
  such as `py -3.12`, or a `python.exe`'s path, quoted or not; never blank), with the
  package first on its `PYTHONPATH` (an empty entry dropped, which Python would read as
  the current folder), its output unbuffered and in UTF-8,
  and no console window of its own; one at a time, and only in the editor that serves
  scripts. Starting one lets scripts in again if the user had shut them out. The Console
  panel (a tab beside Layers and History, brought to the front when a script starts)
  shows what it prints as it comes, its errors in red, and how it ended (its exit code,
  how long it took); it can stop it, and every process it started (on Windows, all of them
  are in a job object; what they did stays), run the last one again, copy everything
  shown, and clear; it keeps the last 5000 lines. Its Recent menu holds the last
  ten scripts run (each once, the last first, kept in `settings.json`, but for a path that
  is not Unicode, which JSON cannot hold), two of one name told apart by their folder, one
  no longer there shown but not offered (whether each is there looked at off the interface
  thread, every few seconds while the panel shows: a share that does not answer cannot hold
  the window up), and a way to forget them. A script still running when the editor
  closes is stopped the same way (one that ended by itself leaves what it started running). The grouping (see *Script runs*, above) holds for every
  script run, whoever started it.
- **MCP server.** The editor serves MCP (streamable HTTP on localhost) to clients that
  connect to it, and `opal-studio --mcp` is a stdio shim for clients that launch their
  servers. Its tools are the command registry; it can return the canvas, or a layer, as an
  image, so an assistant can see what it is doing. *Built:* the stdio shim, which opens no
  window: its tools are the API's methods (but those clients give in their own way:
  `methods`, which the tool list is, and the pixels methods), each with a JSON Schema of its
  parameters, made from the registry; each call goes to the editor running on this
  computer through its pipe and token, as a script's would, and a refusal (or no editor
  running) comes back as the tool's error text. With no editor running, the shim starts
  one of its own on the first call (an editor's state and server with no window, on a
  thread, with a connection file of its own, as `--headless` runs them, stopped with the
  shim, which waits for it to have taken its connection file away) and connects to that; that call's answer says so in a text of its own, since what
  it makes lasts only as long as the shim. The `image` query (the canvas, or a layer
  alone, as a PNG scaled to fit a size) comes back as an image. And MCP over HTTP
  (`mcp_http.rs`), off unless the preference turns it on (Preferences › Scripting, with its
  port, 47860 by default), served only by the editor that serves scripts: on 127.0.0.1
  alone, at `/mcp`, the streamable HTTP transport answering every request in JSON (`POST`;
  `DELETE` ends a session; nothing is streamed, so `GET` is refused). An `initialize` makes
  a session (`Mcp-Session-Id`), each with a connection of its own to the editor's pipe, as
  the shim has; one unused for 30 minutes goes (its client gone without ending it, as one
  restarted is), and at most 32 are kept, the one used longest ago going first. The HTTP is read
  strictly, by code of its own (the small crates were found to have open CVEs for request
  smuggling and header injection): request and header lines ended by CR LF, with no control
  characters, at most 8 KB and 64 of them; bodies framed by `Content-Length` alone, at most
  1 MB (`Transfer-Encoding` is refused, and so is a framing header given twice; a client
  sending `Expect: 100-continue` is told to go on, any other expectation refused); at most 16
  connections at once, each given 30 seconds to send a request, every byte of it (however
  they trickle in), and closed past that. A request is refused unless its
  `Host` is `127.0.0.1` or `localhost` at the port, its `Origin` (if any) the same, and it
  gives `Authorization: Bearer` (the scheme's name in any case) with the token from `mcp-http-token.txt` beside `api.json`:
  made the first time and kept from one start to the next, so a client is set up once
  (Connect an Assistant can make a new one, which refuses clients given the old). A port
  that cannot be opened is said so there, with a Try again. Connect an
  Assistant (File › Scripts) shows what to put in a client's settings for either: the
  shim's command and JSON, or the HTTP address, header and Claude Code command, each with a
  Copy button.
- **Safety.** Local connections only; clients authenticate with a random token kept in a
  file only the user can read; MCP requests with a foreign `Origin` are refused (against
  DNS rebinding). While a script or assistant is connected, the status bar says so and can
  disconnect it, keeping them out until the user lets them in again.
- **Other languages** (JavaScript, Lua) can be thin clients of the same protocol later.

## Native file format

`.opal`, version 3: a zip archive holding `mimetype` (stored first, uncompressed:
`application/x-opal-studio`), `document.json` (the canvas rectangle, the resolution, the
layer tree and the next free identifier; a file without a resolution, as files before it
are, has 72 ppi, and versions before it ignore it, so it needed no new version; a layer's
`placed`, its placement, is read the same way, and refused if it could not have been made:
a point past the nine, a number given twice, or an expression that does not read; and,
version 3, `selections`, the selections saved in the image: each one's name, its steps as the
API's `selection` query tells them, a mask step naming its mask's number, the number of the
mask its steps come to, and how the image has changed since, if it has, `[a, b, c, d, e, f]`),
`tiles/<n>.bin` or `tiles/<n>.srgb8`, one tile each, and `masks/<n>.bin`, the saved
selections' masks, each once (`Mask::write_to`, written as made: a magic, the level outside
its rows, and its rows of runs, each tile one level, a level a column or a row, or a level a
pixel). The steps are kept in the form the `selection` query tells them but in document
coordinates, and the mask the steps but the last come to is kept too. Opening counts the
masks (as read, and as made) with the tiles against what memory is free, and reads each no
larger than the archive says it is; it refuses a damaged or hostile file rather than trust
it: a mask whose rows and runs are out of order, reach past `MASK_REACH` tiles, or hold more
or less than its bytes; a name empty or given twice; a step unknown, or one the editor would
not make (a shape or outline with a place or size that is not a number, an outline of more
than a million points, a modifier's distance past `MAX_DISTANCE`; a shape however far off is
read, since its mask is worked out only within `MASK_REACH`); a mask missing; a change since
that the editor's own changes would not reach (`ImageObject::transform_ok`; a selection kept
whose change since would pass that is carried at once instead, as an image object past it is
drawn into pixels). A file holding
no saved selections is written as version 2, which the versions before them read. Objects are
pixel objects (their tiles and whole-pixel offset) or image objects (their tiles, frame,
transform and sampling). A tile shared by several layers is stored once. A `.bin` tile is
the tile's half floats byte-shuffled (all low bytes, then all high bytes), which lets
deflate compress photos far better; a `.srgb8` tile (version 2) one kept at 8 bits, its
reds, then greens, blues and alphas. Versions 1 (no `.srgb8` tiles) and 2 (no saved
selections) are read as well. Saving compresses the tiles in parallel, a batch at a time, so it holds
a batch of them beyond the document rather than a copy of every tile. Numbers are written
so they read back exactly. `document.json` is written compactly (about 12 bytes a tile
place) and read without serde's buffering of tagged enums (`LayerRead`, `ObjectRead`), as
large as 256 MB or a tenth of the memory free (reading takes up to ten times the text).
Version 0 (pixel objects
only, a canvas at the origin) is still read; a newer version is refused with a message
rather than misread, and so are values out of range (a transform that cannot be inverted,
say). The editor never makes a transform a file could not hold: the transform box stops
at those limits, and a resize that would pass them draws the image into pixels. Later versions add text and optionally the history tree.

Every write, `.opal` and exports alike, goes to a temporary file beside the target that
replaces it only when complete, so a failed save never damages the existing file. Writes
run on a thread of their own, one after another in the order asked (so two writes to one
file never overlap), with the document as it was when the write was asked for: the
interface stays live, and the status bar says what is being written, how far it has got,
and offers to cancel it. Progress is shared through an `opal_io::Progress` (work done of a
total, and a cancel flag): an export counts pixels (the canvas's as they are composited,
when exporting at another size, then the result's as the encoder takes them), a save
counts tiles. Once cancelled, the temporary file takes no more writes, so an encoder or
archive stops at its next one; compositing the source of a resized export checks between
tile rows; and the rename is skipped. Any failure after a cancel counts as the cancel. The
image takes the file chosen at once (so a save asked for before the first is written goes
there too; a failure or cancel gives it back the file it had), and once written it is
marked saved at the history state written, so an edit made meanwhile leaves it unsaved.
It has unsaved changes while what it would save differs from that state's (its layers not
the very same, its canvas or resolution changed): a selection alone, which files do not keep,
is none (`Document::saves_as`).
"Save" in the close and quit questions closes or quits once the writes are done, asking
again about an image changed meanwhile; quitting otherwise waits for writes still under
way; any failed or cancelled write stops quitting (and a close waiting for it), so that it
is seen. A close question never replaces another on screen: it
waits its turn (and for a free moment: no dialog, no box open on the image, the button
not held), and one about an image that closes goes with it. Quitting likewise waits for
a close question on screen to be answered.

## Settings storage

Preferences, tool settings and tool presets (and the last New Image's size, background and
resolution, the scripts run lately, and what was entered lately in each named number
field) are kept in `settings.json` in the per-user
configuration folder (`%APPDATA%\Opal Studio\config` on Windows). Only values changed from
their defaults are written, so a default changed in a later version reaches everyone who
never changed it. Loading reads entry by entry: an unknown or unreadable entry (one written
by a newer version, say) is skipped and the rest still load. A file that cannot be read at
all is kept aside as `settings.json.unreadable` rather than overwritten. Changes are written
a second after the last one, and on exit if any are still unwritten, by the same
replace-when-complete write as documents.

## Out of scope

Multi-page desktop publishing: pages, and text flowing between frames. A photo catalog
(*Photographs*; what it would hold is listed under *Not planned* in `roadmap.md`).

## Prior art

Affinity (unified, but with studios), Graphite (Rust, node-based, raster still experimental),
GIMP, Krita and Inkscape (GPL: code can be borrowed), Photopea.

## Basic-tasks checklist

Each must be doable by a first-time user without documentation. It is the intuitiveness test
for every milestone that touches these tasks.

- Open an image; crop, resize, rotate and straighten it.
- Paste one image into another, then place and size it.
- Cut out an object, hair included, and put it on another background.
- Remove a blemish or an unwanted object.
- Brighten or recolor part of an image.
- Add text, and edit it later.
- Draw a shape and change its color.
- Undo back to an earlier branch of history.
- Export PNG or JPEG at a chosen size.
