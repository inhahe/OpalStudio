# Opal Studio — roadmap

What each milestone delivers is specified in `design.md`. Status: everything is planned and
nothing is built yet.

**Order:** photo editing first, with vector shapes taken early (milestone 4), because by then
the outline tool has built most of the curve editing they need. This order is proposed, not
settled. Milestones 0–2 are the same in either order; design first would follow them with
4 (vector shapes), 7 (text), 11 (constraints) and 9 (vectorization), and then the photo
milestones.

## 0. Foundation
- Cargo workspace; window with egui and dockable panels.
- Canvas viewport: pan and zoom, GPU compositing of tiles, transparency checkerboard,
  off-canvas content dimmed.
- Document model: layers holding objects (raster and group objects first).
- Tile store: sparse 256×256 tiles, copy-on-write. Settle the pixel format.
- Undo history tree, and a first history panel.
- Open and save PNG and JPEG (honoring EXIF orientation); native file format, version 0.
- Layers panel: add, delete, reorder, visibility, opacity, lock; active layer highlighted
  and scrolled into view.
- Command palette.
- Free GPU memory queried from the OS.
- Color engine: every color model of the colorpicker, plus OKLab and OKLCh, in Rust and
  WGSL.
- Settings framework: basic and advanced sections, defaults shown, changed settings marked,
  reset, named presets. The curve editor, both with a chosen color model (the channel curve
  editor) and without one.
- Toolbar without hidden tool groups: one button per tool, variants as a visible control in
  the options bar, shortcuts that never cycle variants.

## 1. Paste and place
- Paste opens the transform box: aspect lock, snapping (Alt and an on-box button), rotate,
  fit and fill, Enter and Esc; new layer by default, or into the active layer.
- Move tool; click to select a layer (Ctrl+click with other tools; right-click lists the
  layers under the cursor); faint outline around the active layer.
- Crop, resize the image, resize the canvas, rotate.
- Export PNG and JPEG at a chosen size.

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

## 3. Painting
- The brush engine that every painting and retouching tool is a preset of: what it paints
  with, how it combines, where it may act, dynamics.
- Brush settings, basic and advanced: tip falloff curve, roundness, angle, image tips;
  spacing, scatter, jitter; dynamics curves for pressure, tilt and speed; smoothing;
  antialiasing (the Pencil variant) and pixel-perfect lines; build-up; color dynamics in a
  chosen color model; compositing space.
- Paints with a color, a gradient, or a pattern (patterns from the selection).
- Pen input on Windows through `WM_POINTER` (pressure, tilt, eraser end); simulated pen
  input for tests.
- Blend modes, the full set, shared later by layers: one-line descriptions and live preview
  in the picker; blending space; the color model behind Hue, Saturation, Color and
  Luminosity; Soft Light formulas; the Channels mode. Behind and Clear for brushes.
- Eraser (Brush variant), erasing to transparency or into the layer's mask.
- Fill, gradient, eyedropper.
- Painting on masks.
- Color picker, version 1: the 2D picker and sliders.

## 4. Vector shapes
- Pen tool reusing the outline tool's curve editing; rectangle, ellipse, polygon, star.
- Fill and stroke; editable brush strokes on vector shapes.
- SVG import and export.
- Boolean operations on paths.

## 5. Adjustments
- Adjustment layers: levels, curves, hue/saturation, gradient map.
- Layer masks; the blend modes from milestone 3 on layers.
- Blend If: curves over channels of a chosen color model, for this layer and for the pixels
  beneath it.
- Color management with ICC profiles; the floating-point pipeline end to end.

## 6. Retouching
- *Sample from*: current, below, all visible, chosen and named sets.
- Clone: offset from two points, aligned or not, ghost source preview; source rotation,
  scale and flip.
- Heal: picked source (gradient-domain blending); Automatic (spot healing: nearby texture,
  PatchMatch).
- History brush: from any state on any branch, or a named snapshot.
- Mix: wet paint and smudge; linear, OKLab and pigment (Kubelka–Munk) mixing.
- Tone: lighten, darken, saturate, desaturate; tonal-range curve; option to paint into an
  adjustment layer's mask.
- Detail: blur to sharpen; channels it acts on; halo suppression.

## 7. Text
- Area text and point text; editable permanently, even inside an existing layer.
- Shaping, bidirectional text, line breaking, font fallback.

## 8. Smart selection and color matching
- Edge snapping in the outline tool (live-wire), with adjustable edge-cost weights.
- Color-range selection: curves per channel, weights, strictness slider, eyedropper start,
  distance mode.
- Color match (sampling, extent, matching, protected colors), and the tools that use it:
  magic wand; Replace color (Brush variant); Background and Click erasers, with edge color
  decontamination.
- ONNX Runtime; model downloads.
- AI object and paint-the-area: SAM 2.1, then matting and color decontamination.
- SAM 3 as an optional download.

## 9. Vectorization
- Heuristic tracing with the parameters in `design.md`; within the selection.

## 10. Color picker, complete
- Gradient editor from the palette editor; harmony, history, collections; 3D view.

## 11. Constraints
- Units; the geometry solver; degrees-of-freedom display; conflict flags; springs.
- Then the layout solver, and nesting units of either kind.

## Later
- Content-aware fill (PatchMatch or LaMa); LaMa as a method for automatic healing.
- Auto-stroke brush (like Photoshop's Art History Brush).
- Perspective, warp and liquify transforms.
- PSD import; TIFF, WebP and AVIF.
- Pen input on macOS and Linux.
