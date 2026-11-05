# Opal Studio — design

Status: nothing is implemented yet. This file records what was decided in the design
discussions that began on 2026-11-04. Items marked **TBD** are deliberately undecided and
get settled when the milestone that needs them starts (see `roadmap.md`).

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
4. **Discoverable without documentation.** A command palette finds any action by name, and
   every task on the *Basic-tasks checklist* must be doable by a first-time user.

## Tech stack

| Concern | Choice |
|---|---|
| Language | Rust |
| GPU | wgpu. Free GPU memory is queried directly from the OS: DXGI `QueryVideoMemoryInfo` on Windows, `VK_EXT_memory_budget` on Vulkan (wgpu does not expose it). |
| UI | egui, with egui_dock for dockable panels; restyled from egui's default look |
| Windowing and input | winit. Its current release reports pen pressure on Windows only and without tilt or the eraser end, so Opal Studio handles Windows' `WM_POINTER` messages itself for those; macOS and Linux pen input come later. |
| ML inference | ONNX Runtime, run locally |
| Color management | lcms2 (ICC profiles) |
| Text shaping and layout | **TBD** (a HarfBuzz-class shaper with bidi, line breaking and font fallback) |
| Vector rendering | **TBD** (vello, tiny-skia, or our own) |
| Tests | egui's test harness (simulated input, screenshot comparison); the canvas renders to PNG without a window |

egui and wgpu also compile to WebAssembly, so a browser build remains possible later; it is
not a goal.

## Document model

- A **document** has a canvas (size, color profile) and a stack of **layers**.
- A **layer** is a container of **objects**: raster (a tiled bitmap plus a transform), vector
  path (fill and stroke), text, brush strokes, adjustment, group. Layers have **no
  boundary**, not even the canvas edge: content past the canvas is kept, shown dimmed, and
  left out of exports.
- A **pasted image** is a raster object that keeps its original pixels and a transform, so
  resizing it later never loses quality. Pixels are baked only when pasting into an existing
  layer.
- **Tiles.** Raster content is stored in 256×256-pixel tiles, and only tiles with content
  exist. Tiles are copy-on-write: an edit copies only the tiles it touches, and untouched
  tiles are shared by every history state and every branch. Tiles are a storage detail and
  never appear in the UI.
- **Pixels** are floating point in linear light; the exact format (f16 or f32, premultiplied
  or not) is **TBD** in the foundation milestone.
- The default perceptual color space, for edge detection, color-range selection and
  vectorization, is **OKLab**.

## Selections

### The mask

A selection is a per-pixel value from 0 (not selected) to 1 (fully selected). Every
operation (filters, adjustments, painting, fills) computes its result and blends it in:

```
new pixel = old pixel × (1 − m) + changed pixel × m        (m = the mask value there)
```

Because every operation goes through this one step, limiting an operation to the selection
and soft (feathered) selections work for all operations without per-operation code. The
marching ants show where the mask crosses 0.5; a view option shows the mask as a red overlay.

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
centered on the outline. It is a recipe step, so it stays adjustable.

### The outline tool

Freehand, anchor-and-curve, polygon and edge-snapping selection are one tool whose segments
differ by gesture:

- drag: freehand segment
- click: anchor (straight segment)
- drag from an anchor: curve
- hold a key: segments snap to edges (live-wire / Intelligent Scissors: a shortest-path
  search over an edge-cost map computed in OKLab). Controls: snap radius and edge
  sensitivity; contrast thresholds per channel in a chosen color model are in an advanced
  panel.

The snapped segment is previewed live before it is committed; Backspace removes the last
anchor.

### Reshaping an outline

- **Right-drag** starting on an existing outline redraws part of it. While dragging, the
  stretch about to be replaced is shown in red with the new line drawn in.
- The two ends of the new line split the closed outline into two stretches. The one nearer
  the new line is replaced; **Tab**, pressed before releasing, flips to the other.
- Releasing far from the outline, or Esc, cancels. A right-click without a drag still opens
  the context menu. **R + left-drag** does the same as right-drag, for touchpads.
- Points of the outline can also be dragged directly.

### AI object and paint-the-area selection

One tool; its prompts are include/exclude clicks, a box, a "somewhere in here" brush and a
"definitely in" brush (everything else counts as out, which forms a trimap). The pipeline:

1. Segmentation: SAM 2.1 (Apache-2.0) bundled; SAM 3 (Meta's own SAM License, adds text
   prompts) as an optional download, not bundled.
2. Matting in a band around the edge (for hair), with a ViTMatte-class model.
3. Color decontamination, which removes background color showing through semi-transparent
   pixels.

Adjustable: the clicks and brushes, mask threshold, edge-band width, decontamination
strength, and a hard-edge toggle. Model licenses other than SAM's are to be verified before
bundling.

### Color-range selection

Each channel `c` of a chosen color model gets a curve giving a membership `m_c` from 0 to 1,
drawn over that channel's histogram (hue curves wrap around). The memberships combine by a
weighted power mean whose exponent `p` is the "strictness" slider:

```
S = ( Σ w_c · m_c^p  /  Σ w_c )^(1/p)
p = +∞ → max (OR);  p = 1 → weighted average;  p → 0 → weighted geometric mean;  p = −∞ → min (AND)
```

Curves start from eyedropper samples and are then adjusted. Because curves per channel can
only select box-shaped regions of color space, a second mode selects by distance from the
sampled colors in OKLab.

## Layers UI

- **Click to select a layer**: with the Move/Select tool, or Ctrl+click with any other tool.
  It picks the topmost layer that visibly contributes at that point; right-click lists every
  layer under the cursor.
- The active layer is highlighted, and scrolled into view, in the layers panel, and has a
  faint outline on the canvas.
- **Sample from** (which layers a tool reads): current layer, layers below, all visible, or a
  chosen set (checkboxes in the layers panel; sets can be named and reused). Present on every
  tool that reads pixels: clone, heal, eyedropper, magic wand, color range, AI selection,
  smudge, blur brush.
- **Apply to** (which layers an operation writes) is a separate control, for moving,
  transforming or adjusting several layers at once.

### Diagnostics

When an action changes nothing visible, the editor says why: painting outside the active
selection; on a hidden layer, a locked one, or one whose transparency is locked; at 0%
opacity; where a layer above completely covers the spot; on the layer's mask instead of the
layer; or off the canvas (kept, but not exported).

## Paste

Pasting opens a transform box instead of placing the pixels:

- aspect ratio locked by default; Shift, or a lock icon on the box, frees it
- snapping to canvas edges and centers, other layers' edges, and the selection's bounds;
  Alt toggles snapping during a drag, and a button on the box toggles it until clicked again
- a rotate handle, and buttons to fit or fill the canvas
- Enter commits and Esc cancels, both shown on screen, so the box never lingers like GIMP's
  floating selection
- goes on a new layer by default; a button puts it on the active layer instead (baked)

## Text

Text objects stay editable permanently, including when placed into an existing layer.
**Area text**: dragging the box reflows the text. **Point text**: dragging scales it. Full
Unicode: shaping, bidirectional text, line breaking, font fallback (emoji, CJK).

## Retouching

- **Clone**: two initial points define the source offset; adjustable brush size; aligned or
  non-aligned source; a ghost preview of the source under the cursor; *Sample from*.
- **Heal**: like clone, but blends the copied texture into the surrounding lighting.
- **Content-aware fill** (later): PatchMatch, or the LaMa inpainting model.

## Undo history tree

- History is a tree: a new action after an undo starts a branch instead of discarding the
  redo states.
- Ctrl+Z and Ctrl+Y walk the current branch; at a fork, redo follows the branch last visited.
  The history panel shows the whole tree, with an automatic description per entry and a
  thumbnail on hover.
- Selections are part of history; zoom and pan are not. A slider drag is one entry.
- The tile store makes branches cheap. History can optionally be saved in the file.

## Vectorization (raster to vector)

Heuristic, not ML. Parameters: how different colors must be to start a new region (measured
in a chosen color model), speckle filter, corner threshold, curve-fitting error tolerance
(how far a curve may deviate before it is split into more detailed curves), and minimum
segment length. VTracer (MIT) is the model for color images; Potrace (GPL) is usable for
two-color images. Works within the selection like any other operation.

## Color picker

A port of the author's colorpicker (`D:\visual studio projects\colorpicker`, MIT): its color
engine and shaders translate to Rust and WGSL, and its panels are rebuilt in egui. OKLab and
OKLCh are added. Its palette editor becomes the gradient editor, used by the gradient tool,
a gradient-map adjustment and indexed-color export.

## Constraints

For mathematically exact vector design, and later for layouts that adapt to their size.

- A **unit** is a group with exactly one solver, decided by the kind of constraints placed in
  it and shown on the unit:
  - **Geometry solver** (nonlinear): coincident, tangent, parallel, perpendicular, symmetric,
    equal, distances and angles given by expressions such as `width = 2*r + gap`. It shows
    how many degrees of freedom remain, colors fully constrained geometry, and flags
    conflicts. Soft "spring" constraints are opt-in, and how far each is stretched is shown.
  - **Layout solver** (linear constraints with priorities, Cassowary-style): even spacing,
    grow to fit content, keep proportions, adapt to another aspect ratio.
- To mix solvers, nest units, to any depth. Between levels, limits go down, sizes come up,
  and the parent places the child.
- Constraints connect only objects within one unit, or anchor points a child unit exposes.
- The geometry solver comes first.

## Native file format

**TBD** in detail: a zip containing a JSON document and the tile data, keeping everything
editable (objects, text, selection recipes, optionally the history tree).

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
