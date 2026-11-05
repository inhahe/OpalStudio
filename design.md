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

One shared component, used for color-range selection, color matching, the tone brush's
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

## Layers UI

- **Click to select a layer**: with the Move/Select tool, or Ctrl+click with any other tool.
  It picks the topmost layer that visibly contributes at that point (how opaque counts as
  visible is adjustable); right-click lists every layer under the cursor.
- The active layer is highlighted, and scrolled into view, in the layers panel, and has a
  faint outline on the canvas.
- **Sample from** (which layers a tool reads): current layer, layers below, all visible, or a
  chosen set (checkboxes in the layers panel; sets can be named and reused). Present on every
  tool that reads pixels: Clone, Heal, Mix, Detail, the eyedropper, the magic wand, color
  match, color range and AI selection.
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
  Alt toggles snapping during a drag, and a button on the box toggles it until clicked again;
  the snapping distance is adjustable
- a rotate handle, and buttons to fit or fill the canvas
- Enter commits and Esc cancels, both shown on screen, so the box never lingers like GIMP's
  floating selection
- goes on a new layer by default; a button puts it on the active layer instead (baked)

## Text

Text objects stay editable permanently, including when placed into an existing layer.
**Area text**: dragging the box reflows the text. **Point text**: dragging scales it. Full
Unicode: shaping, bidirectional text, line breaking, font fallback (emoji, CJK).

## Painting and retouching tools

### One engine; tools are presets of it

Every painting and retouching tool is a preset of one brush engine, so every brush setting
(dynamics, blend mode, *Sample from*, the selection limit) works in every tool it applies
to. A stroke is defined by:

- **what it paints with:** a color, a gradient, a pattern, a clone source, or an earlier
  history state;
- **how it combines** with the layer: a blend mode, erasing, replacing channels, a tone
  adjustment, blur/sharpen, or paint mixing;
- **where it may act:** always within the selection mask, and optionally only where colors
  match (*Color match*, below);
- **how it responds** to the pen: dynamics.

### No hidden tool groups

Photoshop hides three or four tools behind each toolbar button and its shortcut, so pressing
B can leave you on the Pencil when you expected the Brush. In Opal Studio:

- Each toolbar button names and shows what it will do. A tool's variants are a visible
  control in its options bar, never a flyout.
- A shortcut selects a tool; it never cycles through hidden variants.
- The cursor shows the active variant.

| Tool | Variants | Covers Photoshop's |
|---|---|---|
| Brush | Paint · Pencil (antialiasing off) · Replace color | Brush, Pencil, Color Replacement; Pattern Stamp, through "paints with: pattern" |
| Mix | Wet paint · Smudge | Mixer Brush, Smudge |
| Tone | Lighten · Darken · Saturate · Desaturate | Dodge, Burn, Sponge |
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
- **Tone:** strength and tonal range. Advanced: the tonal range is a curve over the lightness
  channel of a chosen model, with Photoshop's fixed shadows, midtones and highlights ranges
  as presets; which channels count as lightness and as saturation; how the adjustment is
  applied (exposure-style multiplication in linear light, or addition to the channel);
  protect tones (a soft-clipping curve, and a hue lock); for Saturate, a curve of how
  strongly already-saturated colors are affected (vibrance). Option: paint into an
  adjustment layer's mask instead of the pixels, so the effect stays editable.
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

## Undo history tree

- History is a tree: a new action after an undo starts a branch instead of discarding the
  redo states.
- Ctrl+Z and Ctrl+Y walk the current branch; at a fork, redo follows the branch last visited.
  The history panel shows the whole tree, with an automatic description per entry and a
  thumbnail on hover.
- Selections are part of history; zoom and pan are not. A slider drag is one entry.
- The tile store makes branches cheap. History can optionally be saved in the file. How much
  memory history may use is a setting; past it, the oldest states are dropped first.
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

## Constraints

For mathematically exact vector design, and later for layouts that adapt to their size.

- A **unit** is a group with exactly one solver, decided by the kind of constraints placed in
  it and shown on the unit:
  - **Geometry solver** (nonlinear): coincident, tangent, parallel, perpendicular, symmetric,
    equal, distances and angles given by expressions such as `width = 2*r + gap`. It shows
    how many degrees of freedom remain, colors fully constrained geometry, and flags
    conflicts. Soft "spring" constraints are opt-in, each with an adjustable stiffness, and
    how far each is stretched is shown. The solver's tolerance is adjustable.
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
