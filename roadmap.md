# Opal Studio — roadmap

What each milestone delivers is specified in `design.md`. Status: milestone 0 is done;
everything after it is planned.

**Order:** 0, 1, S, 2, A, 3, 4, and so on. Photo editing comes first, with vector shapes
taken early (milestone 4), because by then the outline tool has built most of the curve
editing they need. Scripting (S) comes right after milestone 1 so commands take parameters
before many more of them are written; AI selection and extraction (A) right after the
selection core it builds on.

## 0. Foundation (done)
- Cargo workspace; window with egui and dockable panels.
- Canvas viewport: pan and zoom, GPU display of the composited tiles (compositing itself is
  the CPU reference; the GPU compositor moved to milestone 5), transparency checkerboard,
  off-canvas content dimmed.
- Document model: layers holding objects (raster and group objects first).
- Tile store: sparse 256×256 tiles, copy-on-write. Settle the pixel format.
- Undo history tree, and a first history panel.
- Open and save PNG and JPEG (honoring EXIF orientation); native file format, version 0.
- Layers panel: add, delete, reorder, visibility, opacity, lock; active layer highlighted
  and scrolled into view.
- Command registry, and generated from it: the ribbon (tabs, groups that shrink as the
  window narrows, full and single-line sizes, object tabs always present and dimmed when
  not applicable), tooltips with the reason a command is disabled, keytips, command search,
  context menus, and built-in help pages per group (F1, and the ? on group labels).
- The theme: neutral gray and light; Fluent UI icons.
- Free GPU memory queried from the OS.
- Color engine: every color model of the colorpicker, plus OKLab and OKLCh, in Rust (the
  WGSL versions come with their first GPU user, the color picker in milestone 3).
- Settings framework: basic and advanced sections, defaults shown, changed settings marked,
  reset, named presets. The curve editor, both with a chosen color model (the channel curve
  editor) and without one.
- Tools without hidden groups: one button per tool, in a group pinned to the ribbon that can
  move to a side strip; variants as a visible control in the Tool settings panel; shortcuts
  that never cycle variants.
- Tool settings panel: variant picker, basic settings, Advanced section.

## 1. Paste and place
- Paste opens the transform box: aspect lock, snapping (Alt and an on-box button), rotate,
  fit and fill, Enter and Esc; new layer by default, or into the active layer.
- Move tool; click to select a layer (Ctrl+click with other tools; right-click lists the
  layers under the cursor); faint outline around the active layer.
- Crop, resize the image, resize the canvas, rotate.
- Export PNG and JPEG at a chosen size.
- Layer thumbnails in the Layers panel; history thumbnails on hover.
- Selecting several layers in the Layers panel; Merge Selected, Merge Visible, Merge
  Group, Stamp Visible.
- Rasterize Layer (first for a pasted image's transform).

## S. Scripting and MCP (right after milestone 1)
- Parameterized commands: every command callable with parameters as well as from the
  interface; Python stubs, API documentation and MCP tools generated from the registry.
- The JSON-RPC API over a local pipe or socket, with token authentication; document queries;
  pixels through shared memory.
- The `opal` Python package; Run Script, the console panel, the interpreter preference;
  a script run as one history state; headless batch mode.
- The MCP server (local HTTP, and the `--mcp` stdio shim), returning the canvas as an image;
  the status bar's connection indicator.

## 2. Selection core
- The mask, and the blend through it that every operation uses.
- Recipe steps and the selection panel.
- Rectangle and ellipse; add, subtract, intersect, replace.
- The outline tool: freehand, anchors, curves.
- Reshape: right-drag, red preview, Tab flips the side, R + left-drag.
- Feather (inward, outward, centered; falloff curve), grow/shrink (round, square, diamond),
  invert.
- Marching ants; the mask as a red overlay.
- Selections in the history tree.
- Diagnostics: "why nothing happened", starting with selection, hidden, locked, opacity and
  covered-by-a-layer-above.

## A. AI selection and extraction (right after milestone 2)
- ONNX Runtime with DirectML; the model manager (download on first use, license shown,
  delete from Preferences).
- Select subject (BiRefNet); the AI object and paint-the-area tool (SAM 2.1); matting
  (ViTMatte) and color decontamination.
- Copy to new layer, Cut to new layer; Lift out and Remove object, filled by LaMa on a layer
  of their own.
- SAM 3 as an optional download.

## 3. Painting
- The brush engine that every painting and retouching tool is a preset of: what it paints
  with, how it combines, where it may act, dynamics.
- Brush settings, basic and advanced: tip falloff curve, roundness, angle, image tips;
  spacing, scatter, jitter; dynamics curves for pressure, tilt and speed; smoothing;
  antialiasing (the Pencil variant) and pixel-perfect lines; build-up; color dynamics in a
  chosen color model; compositing space.
- Paints with a color, a gradient, or a pattern (patterns from the selection).
- Flat and elliptical tips at a fixed angle, so the brush can letter calligraphically on
  pixels before the Calligraphy tool's vector strokes arrive in milestone 4.
- Pen input on Windows through `WM_POINTER` (pressure, tilt, eraser end); simulated pen
  input for tests.
- Blend modes, the full set, shared later by layers: one-line descriptions and live preview
  in the picker; blending space; the color model behind Hue, Saturation, Color and
  Luminosity; Soft Light formulas; the Channels mode. Behind and Clear for brushes.
- Eraser (Brush variant), erasing to transparency or into the layer's mask.
- Fill, gradient, eyedropper.
- Painting on masks.
- Color picker, version 1: the 2D picker and sliders.
- Compositing caches for each group and for everything below the active layer, so painting
  recombines only what is above it.

## 4. Vector shapes
- Pen tool reusing the outline tool's curve editing; rectangle, ellipse, polygon, star.
- Fill and stroke; editable brush strokes on vector shapes.
- SVG import and export.
- Boolean operations on paths.
- The Calligraphy tool: broad-edge, pointed and brush variants; strokes as editable vector
  objects (centerline plus nib); pressure, tilt and rotation; the guide sheet.

## 5. Adjustments
- Adjustments: levels, curves, hue/saturation, gradient map, channel matrix (3×3 weights
  plus offsets in a chosen color model, curves in place of weights, presets). Each as an
  adjustment layer or applied once to the selection.
- Layer masks; the blend modes from milestone 3 on layers.
- Blend If: curves over channels of a chosen color model, for this layer and for the pixels
  beneath it.
- Color management with ICC profiles (embedded profiles honored on import); the
  floating-point pipeline end to end.
- A GPU compositor matching the CPU reference, so adjustments preview at interactive speed
  on large images.
- Previews at screen resolution first while a setting is dragged; the performance view
  (which layers cost the most, with Rasterize and Stamp Visible offered).

## 6. Retouching
- *Sample from*: current, below, all visible, chosen and named sets.
- Clone: offset from two points, aligned or not, ghost source preview; source rotation,
  scale and flip.
- Heal: picked source (gradient-domain blending); Automatic (spot healing: nearby texture,
  PatchMatch).
- History brush: from any state on any branch, or a named snapshot.
- Mix: wet paint and smudge; linear, OKLab and pigment (Kubelka–Munk) mixing.
- Adjust brush: lighten, darken, saturate, desaturate, and any adjustment from milestone 5
  (the channel matrix included); tonal-range curve; option to paint into an adjustment
  layer's mask.
- Detail: blur to sharpen; channels it acts on; halo suppression.

## 7. Text
- Area text and point text; editable permanently, even inside an existing layer.
- Shaping, bidirectional text, line breaking, font fallback.

## 8. Edge snapping and color matching
- Edge snapping in the outline tool (live-wire), with adjustable edge-cost weights.
- Color-range selection: curves per channel, weights, strictness slider, eyedropper start,
  distance mode.
- Color match (sampling, extent, matching, protected colors), and the tools that use it:
  magic wand; Replace color (Brush variant); Background and Click erasers, with edge color
  decontamination.

## 9. Vectorization
- Heuristic tracing with the parameters in `design.md`; within the selection.

## 10. Color picker, complete
- Gradient editor from the palette editor; harmony, history, collections; 3D view.

## 11. Constraints
- Units; the geometry solver; degrees-of-freedom display; conflict flags; springs.
- Then the layout solver, and nesting units of either kind.

## Later
- Content-aware fill by PatchMatch (LaMa's arrives with milestone A); LaMa as a method for
  automatic healing.
- Auto-stroke brush (like Photoshop's Art History Brush).
- Perspective, warp and liquify transforms.
- PSD import; TIFF, WebP and AVIF.
- Pen input on macOS and Linux.
