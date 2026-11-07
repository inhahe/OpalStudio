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
marching ants show where the mask crosses a threshold (default 0.5, adjustable); a view
option shows the mask as a red overlay.

### The recipe

A selection is kept as an ordered list of steps, not only as its final mask:

- **Makers** produce a mask and combine with the steps before them by *add*, *subtract*,
  *intersect* or *replace*: outline, rectangle, ellipse, AI object, paint-the-area, color
  range, magic wand, painting on the mask.
- **Modifiers** change the mask so far: feather, grow/shrink, smooth, invert, refine edge.

The selection panel lists the steps. Editing any step recomputes the later ones. Outlines are
stored as Bézier curves; makers that produce pixels (AI, color range, painted masks) store
their settings and their resulting mask. AI steps cache their result and rerun only when
their own inputs change. Users who never open the panel just make selections as usual.

A selection maker used while a selection exists works within it (intersect). AI selection
inside a selection runs the model on that crop.

### Feather

The feather setting is the width of the falloff zone; it can fall inward, outward or be
centered on the outline. The shape of the falloff is a curve (Gaussian by default; linear and
custom curves are options). It is a recipe step, so it stays adjustable. Grow/shrink has an
adjustable shape (round, square, diamond).

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
  Layer, Select, Adjust and View to begin with; Filter, Vector and others as features
  arrive.
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
  reached from the keyboard without memorized shortcuts. Tools get none: their own single
  keys (H, Z) already work without Alt.
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

### Look

- egui restyled. Three choices of color scheme: **Dark** (the default: neutral gray, so the
  interface does not skew judgment of the image's colors, as in most image editors),
  **Light**, and **Follow the system** (switches along with the operating system's light
  or dark mode).
- One accent color, slightly rounded corners, subtle hover and press animations.
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
  with a locked layer inside; locked layers can still be moved up and down the stack.
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
existing layers: Transform (Ctrl+T) opens it on the selected layers.

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
  the canvas. The crop box has the same fields and button; the selection tools' boxes will
  take them too. The position is that of the box's **reference point** (a 3 × 3 grid in
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
  as one state on release.
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
  - A field being typed in widens to what is typed, so an expression can be read. It shows
    what its expression comes to as you type, and an error says where it is,
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

`.opal`, version 2: a zip archive holding `mimetype` (stored first, uncompressed:
`application/x-opal-studio`), `document.json` (the canvas rectangle, the resolution, the
layer tree and the next free identifier; a file without a resolution, as files before it
are, has 72 ppi, and versions before it ignore it, so it needed no new version; a layer's
`placed`, its placement, is read the same way, and refused if it could not have been made:
a point past the nine, a number given twice, or an expression that does not read) and `tiles/<n>.bin` or `tiles/<n>.srgb8`, one tile each. Objects are
pixel objects (their tiles and whole-pixel offset) or image objects (their tiles, frame,
transform and sampling). A tile shared by several layers is stored once. A `.bin` tile is
the tile's half floats byte-shuffled (all low bytes, then all high bytes), which lets
deflate compress photos far better; a `.srgb8` tile (version 2) one kept at 8 bits, its
reds, then greens, blues and alphas. Version 1 (no `.srgb8` tiles) is read as well. Saving compresses the tiles in parallel, a batch at a time, so it holds
a batch of them beyond the document rather than a copy of every tile. Numbers are written
so they read back exactly. `document.json` is written compactly (about 12 bytes a tile
place) and read without serde's buffering of tagged enums (`LayerRead`, `ObjectRead`), as
large as 256 MB or a tenth of the memory free (reading takes up to ten times the text).
Version 0 (pixel objects
only, a canvas at the origin) is still read; a newer version is refused with a message
rather than misread, and so are values out of range (a transform that cannot be inverted,
say). The editor never makes a transform a file could not hold: the transform box stops
at those limits, and a resize that would pass them draws the image into pixels. Later versions add text, selection recipes and optionally the history tree.

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

Multi-page desktop publishing: pages, and text flowing between frames.

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
