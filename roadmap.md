# Opal Studio — roadmap

What each milestone delivers is specified in `design.md`: for milestone 5's adjustments and
F's filters, down to each one's settings (*The adjustments and their settings*, *Filters*);
the rest added from the tool and command lists (N, T, P and L, and the items marked *(new)*)
get their sections there as each begins (P's decisions so far: *Photographs*). Status:
milestones 0, 1, S and E are done; everything after them is planned.

**Order:** 0, 1, S, E, 2, N, A, 3, 4, 5, F, 6, T, P, 7, L, 8, 9, 10, 11, then *Later*. Within a
milestone, items are in the order to build them. Photo editing comes first, with vector
shapes taken early (milestone 4), because by then the outline tool has built most of the
curve editing they need. Scripting (S) comes right after milestone 1 so commands take
parameters before many more of them are written. Expressions (E) come before milestone 2, so
the selection boxes' fields take them from the start. N gathers small things that are used
every day (screen modes, aligning, measuring), cheap enough to do at once. AI selection and
extraction (A) comes right after the selection core it builds on. Filters (F) come right
after adjustments (5), whose non-destructive layers they are built on, and before
retouching (6), as blurring, sharpening and noise are used more often. Transforms beyond the
box's (T) come once painting and retouching are done; layer effects (L) right after text,
which they are most used on. Photographs (P: raw files, lens profiles, depth, people and their
parts, and the Photo tab's one-click commands) come once the things they are made of exist
(Develop, filters, AI selection, healing, perspective), and before text, as photo editing
comes first.

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

## 1. Paste and place (done)
- Pixel objects and image objects; the resampler (methods, anti-ringing, pyramids for
  shrinking); the canvas as a rectangle; native format version 1.
- Paste opens the transform box: aspect lock, snapping (Alt and an on-box button), rotate,
  fit and fill, Enter and Esc; new layer by default, or into the active layer. Paste in
  Place; paste with no document open; Copy and Copy Merged; Transform (Ctrl+T) on the
  selected layers.
- Move tool; arrow-key nudging; click to select a layer (Ctrl+click with other tools;
  right-click lists the layers under the cursor); faint outline around the active layer.
- Crop tool with straighten; Crop to Content; Resize Image; Resize Canvas; Rotate and Flip.
- Export PNG and JPEG at a chosen size.
- Layer thumbnails in the Layers panel; history thumbnails on hover.
- Selecting several layers in the Layers panel; Merge Selected, Merge Visible, Merge
  Group, Stamp Visible.
- Rasterize Layer.

## S. Scripting and MCP (done)
- Parameterized commands: every command callable with parameters as well as from the
  interface; Python stubs, API documentation and MCP tools generated from the registry.
- The JSON-RPC API over a local pipe or socket, with token authentication; document queries;
  pixels through shared memory.
- The `opal` Python package; Run Script, the console panel, the interpreter preference;
  a script run as one history state; headless batch mode.
- The MCP server (local HTTP, and the `--mcp` stdio shim), returning the canvas as an image;
  the status bar's connection indicator.
- Units and resolution (done, after S): every size and position in pixels, points, picas,
  inches, feet, yards, millimeters, centimeters or meters at the image's resolution, read
  from and written to PNG and JPEG; New Image's paper and photo sizes; Resize Image's
  resolution. Exact numbers for the transform and crop boxes, by the box; Place. Nothing a
  script needs is left to the window: `paste`, `place`, `transform` and `crop` take exact
  boxes, the preferences can be read and set, `--headless -c` and `-` run code given
  directly, and `--mcp` starts an editor of its own when none is running. Then: a plain
  `python script.py` starts a windowless editor when none runs (`--serve`); the boxes'
  reference point and fitting into a typed rectangle; rulers.

## E. Expressions (done)
- The expression language (`design.md`, *Expressions*):
  - numbers in mixed units, with their kinds checked;
  - arithmetic and functions;
  - references to the canvas, to any layer's bounds (edges, corners and center) by name or
    id, to the box as it was, and to the resolution;
  - points as values (`mid`, `lerp`, `dist`, `point`);
  - names resolved to stable ids, and renamed with their objects.
- Every number and length field takes an expression. It is worked out when entered; what
  it comes to shows as you type; an error says where it is; recent expressions come back
  with Up and Down.
- Every number and length parameter of the API (Python and MCP) takes an expression as
  text, worked out by the same parser when the call runs. The Python package builds
  expressions with operators (`opal.layer(logo).right + opal.mm(5)`), naming layers by id.
- A placement remembers the expression it came from. *Re-apply*, by the box's numbers and
  from scripts, works it out again after what it refers to has moved.

## 2. Selection core
- The mask, and the blend through it that every operation uses.
- Recipe steps and the selection panel.
- Rectangle and ellipse; add, subtract, intersect, replace. Their boxes take exact numbers
  in any unit, by the box, as the transform and crop boxes do (`box_fields`). *(new)* Single
  row and single column, as variants of the rectangle.
- The outline tool: freehand, anchors, curves.
- *(new)* Editing outlines (the Pen of milestone 4 reuses it all): adding a point (a click on
  a segment), deleting one (Alt+click, or Delete), turning a point smooth or sharp, moving
  points and segments, and selecting and moving whole outlines. These are ways of the
  outline tool, as a button each while an outline is edited, not tools of their own.
- Symmetric outlines: drawn as one slice, mirrored, turned or both (`design.md`,
  *Symmetry*).
- Reshape: right-drag, red preview, Tab flips the side, R + left-drag.
- Feather (inward, outward, centered; falloff curve), grow/shrink (round, square, diamond),
  invert. *(new)* Smooth (rounding off jagged outlines and closing pinholes, by a radius) and
  Border (a band of a given width along the outline, inward, outward or centered), as
  modifier steps.
- *(new)* Selection from a layer: its opaque pixels (Ctrl+click its thumbnail), or as much as
  each is opaque; add, subtract and intersect with what is selected.
- *(new)* Reselect (the selection last deselected), and selections saved by name in the
  image, to load back, combine, and keep in the file.
- *(new)* Transform Selection: the selection itself in a transform box, its shapes carried
  exactly where they can be (`Selection::transformed`).
- *(new)* Layer via Copy (Ctrl+J) and Layer via Cut: what is selected, on a layer of its own
  just above (as much of each pixel as it is selected).
- Marching ants; the mask as a red overlay.
- Selections in the history tree.
- Diagnostics: "why nothing happened", starting with selection, hidden, locked, opacity and
  covered-by-a-layer-above.

## N. Navigating, aligning and measuring *(new)*
- Screen modes (F cycles them): standard; full screen with the panels; full screen with the
  image alone.
- Align and distribute layers: edges and centers, to the canvas, to the selection, or to a
  key layer; distributing spacing evenly. In the box's fields and from scripts too.
- Arranging: Bring to Front and Send to Back (within the group), Reverse the selected
  layers' order; Select All Layers and Deselect Layers.
- Transform Again (the last transform, on the layers selected now) and Transform Again as a
  Copy (a copy each time, so that a few presses make a ring or a row of them).
- Open as Layers: several files into one image, a layer each.
- *(new)* Metadata kept: a file's EXIF, IPTC and XMP kept from opening, through the native
  file, to export (the orientation and size made true to the image exported). Image › File
  Info shows the capture (camera, lens, settings, date, place) and edits what describes the
  image (title, caption, creator, copyright, keywords). Export chooses what it writes: all;
  all but the place and the camera's serial numbers; copyright and contact only; or none (the
  default a preference). Templates (creator and copyright) to fill in at once. Scripts read
  and write all of it, which is how a folder's capture times are shifted or its files
  renamed by their metadata.
- *(new)* Comparing: images side by side (two to four panes), their zoom and position
  locked together or matched once; a reference image pinned beside the one being edited, to
  match its look; before and after: the image beside any state of its history (the one it
  was opened as, by default), or split with it by a line dragged across the view.
- Rotate the view (not the image), by a drag or a typed angle, back with one key; painting
  needs it most.
- Crop to Content by color: trim borders of the corners' color, within a tolerance (as well
  as transparent ones); and Reveal All, the canvas grown to all there is, hidden layers too.
- Matting: Defringe (edge pixels given their neighbors' color), Remove Black Matte and Remove
  White Matte (the color an edge was blended with taken back out).
- The measuring tool: a distance and an angle between two points, in any unit, kept until
  the next measure; Straighten by the line measured.
- Color samplers: points pinned on the image that show their color in chosen color models,
  live while adjustments are previewed; read by scripts.

## A. AI selection and extraction (right after milestones 2 and N)
- ONNX Runtime with DirectML; the model manager (download on first use, license shown,
  delete from Preferences).
- Select subject (BiRefNet); the AI object and paint-the-area tool (SAM 2.1); matting
  (ViTMatte) and color decontamination. *(new)* Refining an edge as one step of the recipe:
  smooth, feather, contrast and shifting it in or out, beside matting and decontamination;
  Select sky; masks for the objects found in a layer.
- Lift out and Remove object, filled by LaMa on a layer of their own. *(new)* Remove brush: paint over what should go, and LaMa fills it on a layer
  of its own. Remove background (Select subject, inverted, on a mask).
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
- Fill, gradient, eyedropper. *(new)* Edit › Fill and Stroke: the selection filled with a
  color, the secondary color, 50% gray, a pattern or a gradient, or its outline stroked
  (width; inside, centered or outside), with a blend mode and opacity.
- *(new)* Primary and secondary colors, D to reset them to black and white, X to swap them.
- *(new)* Define Brush (a brush tip from the selection) and Define Pattern.
- Painting on masks. *(new)* Quick Mask (Q): the selection painted with any brush and
  eraser, kept as a step of its recipe.
- Symmetric painting: each stroke repeated in mirror, in rotation of a chosen order, or
  both, about a center and axis on the canvas (*Symmetry*).
- Color picker, version 1: the 2D picker and sliders.
- Compositing caches for each group and for everything below the active layer, so painting
  recombines only what is above it.

## 4. Vector shapes
- Pen tool reusing the outline tool's curve editing (milestone 2's editing of outlines
  included); rectangle, ellipse, polygon, star. *(new)* The Curvature pen (points placed on
  the curve, its handles worked out), as a variant of the Pen; the Line tool, with
  arrowheads; rounded corners kept editable on rectangles and polygons.
- *(new)* Paths to selections and selections to paths; filling a path, and stroking it with
  any brush of milestone 3.
- Paths' nodes as points the boxes' positions can count from (`box_fields::Base`), beside
  the canvas and layers, from the fields and from scripts, and that expressions can name
  (`Outline.node(3).x`). Nodes get names you can see, backed by stable ids, so renaming or
  deleting another node never redirects an expression.
- Fill and stroke; editable brush strokes on vector shapes.
- SVG import and export.
- Boolean operations on paths.
- *(new)* A library of shapes, to add to and draw from.
- Symmetric shapes: order n, with or without a mirror, about a movable center and axis;
  edited as one slice, the seams kept smooth; Expand symmetry; the center named in
  expressions (*Symmetry*).
- The Calligraphy tool: broad-edge, pointed and brush variants; strokes as editable vector
  objects (centerline plus nib); pressure, tilt and rotation; the guide sheet, with the
  Chinese practice grids.
- The ink brush for Chinese calligraphy: bristles that spread and splay, ink that runs dry
  into flying white, dipping to add ink, density and wetness, rice-paper bleeding; strokes
  re-renderable with other ink settings.
- The ink can: pots of dense ink, light ink and water at the canvas's edge to dip the brush
  in, and a gauge of the ink left in the brush.

## 5. Adjustments
- Adjustments: levels, curves, hue/saturation, gradient map, channel matrix (3×3 weights
  plus offsets in a chosen color model, curves in place of weights, presets). Each as an
  adjustment layer or applied once to the selection.
- *(new)* The Histogram panel (each channel, or luminosity; of the image, the layer or the
  selection; with statistics), and its histogram in Levels and Curves. Clipping shown: while
  an adjustment is previewed, a key shows the pixels it clips to black or white (in any
  channel) in color on the image, and the histogram's ends say how many there are.
- *(new)* Fill layers: a solid color, a gradient or a pattern, kept editable.
- *(new)* More adjustments, in this order: brightness and contrast; exposure (stops, offset,
  gamma); black and white (a weight for each range of hues; tint); vibrance; color balance
  (shadows, midtones and highlights); shadows and highlights; color lookup tables (`.cube`
  and `.3dl`); photo filter (warming, cooling, any color, lightness kept); invert,
  threshold, posterize and desaturate; auto tone, auto contrast and auto color; clarity
  (local contrast at a chosen scale); equalize; solarize; selective color.
- *(new)* Develop: the photographic controls in one adjustment layer, as raw developers have
  them: white balance (temperature and tint), exposure, contrast, highlights, shadows,
  whites and blacks, texture, clarity, dehaze (both ways: haze added too), vibrance and
  saturation, a tone curve (by regions, darks, shadows, lights and highlights with movable
  splits, as well as by points, each channel), hue, saturation and lightness by color, color
  grading (shadows, midtones, highlights and the whole, a wheel each, with blending and
  balance), point color (colors picked on the image, each shifted in hue, saturation and
  lightness, with its range's width and softness, the range shown), a black-and-white
  treatment (Black & White's weights), grain and vignette (kept centered on the canvas as it
  is cropped). Auto: the tone and the white balance worked out from the image. Its detail
  section (sharpening with its mask, shown while its slider is dragged with Alt; noise
  reduction) once milestone F's filters exist. Dragging on the Histogram panel's regions
  changes Develop's blacks, shadows, exposure, highlights and whites.
- Expression layers (`design.md`, *Expression layers*): each pixel filled by an expression
  per component of a chosen color model, and one for alpha, of its place (`x`, `y`; `u`, `v`
  across the selection; distance and angle from its center), noise, and the pixels beneath it
  (`below.L`, `below.h`); editable, worked out a tile at a time off the interface thread,
  compiled once. `let` and `if … then … else` added to expressions. Select › Fill with
  Expression makes one within the selection; scripts too (`fill_expression`).
- Layer masks; the blend modes from milestone 3 on layers. *(new)* Masks in full: revealing
  or hiding all, or the selection; turned off and on; applied, deleted, unlinked to move
  apart; their density, feather and inversion kept as settings; a selection from a mask.
  Vector masks from paths. Paste Into (into the selection, through a mask made of it).
  Clipping a layer to the one below (its shape the other's). Fill opacity apart from the
  layer's; which channels a layer blends into.
- *(new)* Masks that keep their recipe: a mask made from a selection keeps the recipe it came
  from (`design.md`, *Photographs*), worked out again when asked, and when its layer is
  copied into another image (its AI and color steps finding what they find there, its shapes
  carried to the new canvas). A gradient selection step (fully selected on one side of a
  band, not at all on the other, falling off across it), so that a graduated adjustment is a
  layer masked by one, as a radial one is by a feathered ellipse.
- *(new)* Copying adjustments: the adjustment layers chosen (filter layers too, once
  milestone F has them), from one image into others, their masks worked out again in each.
  Looks: such layers saved together as one preset, applied with an amount (every setting
  moved that far from neutral, past 100% too), previewed on the image while hovered in the
  list.
- Blend If: curves over channels of a chosen color model, for this layer and for the pixels
  beneath it.
- Color management with ICC profiles (embedded profiles honored on import); the
  floating-point pipeline end to end.
- A GPU compositor matching the CPU reference, so adjustments preview at interactive speed
  on large images.
- Previews at screen resolution first while a setting is dragged; the performance view
  (which layers cost the most, with Rasterize and Stamp Visible offered).

## F. Filters *(new)*
- Filters as layers that act on what is beneath them, or on one layer, editable as
  adjustment layers are (milestone 5's machinery); or applied once to the selection.
  Previewed at screen resolution first. Several stacked make a chain to reorder. Repeat the
  last filter, with its settings (Ctrl+F) or showing them.
- In this order: Gaussian blur; unsharp mask and smart sharpen (radius, amount, threshold,
  halos suppressed); high pass; add noise and film grain; reduce noise (luminance and color
  apart); median, despeckle and dust and scratches; motion blur; radial blur (spin and zoom);
  surface blur (edges kept); vignette; *(new)* glow (the Orton effect: a blurred, brightened
  copy blended over); lens correction (barrel and pincushion, chromatic aberration,
  vignetting; *(new)* purple and green fringes taken out by their hues); lens blur (bokeh, with a depth map or a gradient); tilt-shift,
  iris and field blur; box blur and average; clouds (the noise expression layers have);
  offset (the image shifted, wrapped round at the edges, for seamless textures); minimum
  and maximum (shrinking or spreading light areas); a custom convolution kernel.
- Then distortions: twirl, pinch and spherize, wave, ripple and zigzag, polar coordinates,
  displace (by a grayscale image), glass, shear.
- Then stylizing: emboss, find edges and glowing edges, pixelate (mosaic, crystallize,
  pointillize, facet, fragment, mezzotint), color halftone, oil paint, diffuse glow, wind,
  trace contour, tiles, diffuse; fibers.
- Then the blur gallery's spin and path blurs (paths from milestone 4).

## 6. Retouching
- *Sample from*: current, below, all visible, chosen and named sets.
- Clone: offset from two points, aligned or not, ghost source preview; source rotation,
  scale and flip.
- Heal: picked source (gradient-domain blending); Automatic (spot healing: nearby texture,
  PatchMatch). *(new)* Patch: a selection healed from another place, dragged to.
- *(new)* Spots: a view in which dust and spots stand out (the image as edges in black and
  white, its threshold a slider) while healing; Find Spots, which finds them (small,
  soft-edged, darker than what is round them, up to a size and down to a strength) and heals
  each as a spot of its own, to look over, move or delete.
- *(new)* Red eye: a click on a pupil finds the red and darkens it.
- History brush: from any state on any branch, or a named snapshot. *(new)* Fill from a
  state, in Edit › Fill.
- *(new)* Versions: named snapshots kept in the file, as versions of the image to switch
  between, compare side by side (milestone N) and export each; and the whole history kept in
  the file, when chosen (`design.md`, *Undo history tree*).
- Mix: wet paint and smudge; linear, OKLab and pigment (Kubelka–Munk) mixing.
- Adjust brush: lighten, darken, saturate, desaturate, and any adjustment from milestone 5
  (the channel matrix included); tonal-range curve; option to paint into an adjustment
  layer's mask.
- Detail: blur to sharpen; channels it acts on; halo suppression.

## T. Transforms *(new)*
- Skew, by the box (Ctrl and a side's handle), and in its fields.
- Distort and perspective: the box's corners moved freely, the image drawn through the
  projective map they make; the Crop tool's perspective variant (a crop whose corners are
  placed on something seen at an angle, straightened as it is cropped).
- *(new)* Upright: the perspective corrected from lines drawn on the image that should be
  vertical or horizontal (two to four), or from the lines found in it (level, upright, or
  both); Auto Straighten (the horizon or the verticals found, the image turned to them). After
  either, and as an option of any transform and of the Crop tool: the crop constrained to the
  image, the largest rectangle (of the aspect ratio chosen, if one is) with nothing empty in
  it.
- Warp: a grid of Bézier patches, and its presets (arc, arch, bulge, flag, wave, fish,
  rise, squeeze, twist).
- Liquify: push, twirl, pucker, bloat, reconstruct; masks to freeze areas.
- Puppet warp: pins on a mesh over the layer.
- Content-aware scale: seam carving, with areas to protect.

## P. Photographs *(new)*
- Raw files, as raw objects (`design.md`, *Photographs*): read by rawler (DNGLab's,
  LGPL-2.1) and developed from the raw data the object keeps, in the native file too, so that
  every raw setting stays editable: demosaicing (RCD by default; AMaZE, AHD and VNG, and
  Markesteijn for X-Trans sensors, ported from darktable and RawTherapee, GPL as Opal Studio
  is; a quick one while a setting is dragged); white balance on the sensor's own channels (as
  shot, the camera's presets, temperature and tint, picked on something neutral, automatic);
  highlights rebuilt from the channels not clipped; the camera's color (the file's matrices,
  DCP camera profiles, the primaries calibrated); DNG's own corrections. What comes out is
  linear floating point, with a Develop layer clipped to it, whose starting settings may
  differ by camera and by ISO. Thumbnails from the files' own previews.
- Lens profiles: the lensfun database (CC-BY-SA 3.0), read by code of our own, correcting the
  distortion, chromatic aberration and vignetting of the camera and lens the metadata names;
  in Lens Correction and in raw objects.
- Depth from a photograph (moved from *Later*): Depth Anything V2 Small (Apache-2.0; its larger
  sizes are non-commercial), making a depth layer for Lens Blur, refined by painting on it;
  Select by Depth, a range from near to far, feathered.
- People and their parts: each person found (a detector whose license allows it, each box
  refined by SAM 2.1), one or all; for each, face skin, body skin, hair and clothes
  (MediaPipe's multiclass segmenter, Apache-2.0), and eyes, irises, eye whites, eyebrows,
  lips and teeth (from the points of MediaPipe's Face Landmarker, as outline steps; the teeth
  by their color within the lips): Select › People. Then a landscape's parts beside the sky
  (water, plants, mountains, ground, buildings), by SAM 3's words or a model whose terms
  allow it.
- The Photo tab: one-click commands made of ordinary layers, each a Develop or filter layer
  named for what it does, masked by a recipe that finds what it acts on, its settings the
  command's defaults; all editable afterwards, and found again in another image. In order:
  Auto (Develop's Auto, as a layer); Subject (Brighten Subject, Darken Background, Desaturate
  Background, Blur Background, by depth); Sky (Darken Sky, Deepen Blue, Warm Sunset);
  Portrait, for each person or the one chosen (Brighten Face, Smooth Skin, Whiten Teeth,
  Brighten Eye Whites, Enhance Irises, Lip Color, Define Hair); Cleanup (Find Spots, Remove
  People: the people besides the subject taken out by LaMa, a layer each); Geometry (Lens
  Profile, Upright, Auto Straighten); Landscape (Enhance Foliage, Clear Water, Define
  Mountains). The user's own: layers with recipe masks saved as a command, in a group of the
  user's.
- Many images at once: a look, copied adjustments or a Photo-tab command applied to the images
  open or chosen, the masks found in each; to a folder of files by *Later*'s batches.
- Print: one image, at a size, fitted to the paper or at its print size, placed on the page;
  color managed (the printer's profile, the rendering intent, black point compensation) and
  soft-proofed first; the file's name and caption beneath it, if wanted.

## 7. Text
- Area text and point text; editable permanently, even inside an existing layer.
- *(new)* Vertical text (columns right to left, for Chinese and Japanese), and turning text
  between horizontal and vertical; writing direction; point text to area text and back.
- *(new)* Paragraphs: alignment and justification, hyphenation by language; OpenType
  features; the antialiasing method; case changes (upper, lower, title, sentence).
- *(new)* Text as a selection (a maker step of the recipe); warped text, with the warp
  presets of milestone T; text to outlines, to a path, and to pixels.
- *(new)* Spelling (by language); find and replace across the text layers; missing fonts
  found and replaced.
- Text boxes' corners as points the boxes' positions can count from and expressions can
  name, as paths' nodes are.
- Shaping, bidirectional text, line breaking, font fallback.

## L. Layer effects *(new)*
- Effects on a layer, editable and in the order they are drawn: drop shadow, stroke, outer
  glow, color, gradient and pattern overlays, inner shadow, inner glow, bevel and emboss,
  satin. Global light; effects scaled with the layer; effects turned into layers of their
  own; several of one kind on a layer.

## 8. Edge snapping and color matching
- Edge snapping in the outline tool (live-wire), with adjustable edge-cost weights.
- Color-range selection: curves per channel, weights, strictness slider, eyedropper start,
  distance mode.
- *(new)* Luminosity masks: Select › Lights, Darks and Midtones, each at a few strictnesses,
  one command each (a color-range step on lightness, adjustable after).
- Color match (sampling, extent, matching, protected colors), and the tools that use it:
  magic wand; Replace color (Brush variant); Background and Click erasers, with edge color
  decontamination.

## 9. Vectorization
- Heuristic tracing with the parameters in `design.md`; within the selection.

## 10. Color picker, complete
- Gradient editor from the palette editor; harmony, history, collections; 3D view.

## 11. Constraints
- Units; the geometry solver; degrees-of-freedom display; conflict flags; springs.
- Kept expressions: *keep linked* by a field, and `linked` in the API, turn an expression
  into a constraint. It uses the same syntax and parser as milestone E's expressions,
  which the solver differentiates. A placement's remembered expression can be made one
  too. Scripts make constraints with the same expressions.
- Then the layout solver, and nesting units of either kind.

## Later
In this order:
- Recording: what is done, written as a script to run again (an action), the commands being
  parameterized already; batches: a script or recording run over a folder of files, with
  their output's format, size and color profile (as Image Processor does). *(new)* The
  output's names made from a pattern (the original name, the capture date, the camera, a
  number counting up); a watched folder, each file processed as it arrives (with a camera's
  own tethering software saving into it, a tethered workflow).
- *(new)* The Browse panel: a folder's images as thumbnails (raw files by their own previews),
  sorted and filtered by name, date, capture settings, rating and label; opened, opened as
  layers, compared side by side, or handed to a batch or a merge; ratings, flags and labels
  kept in the files' XMP (beside raw files), where Lightroom, Bridge, digiKam and darktable
  read them. Not a catalog: the folders are what it shows. And Opal Studio as the external
  editor those catalogs open a photograph in (TIFF in and out, the metadata kept).
- Content-aware fill by PatchMatch (LaMa's arrives with milestone A), with its own
  workspace (where to sample from, how much to rotate and scale); LaMa as a method for
  automatic healing; content-aware move and extend.
- PSD import; TIFF, WebP and AVIF.
- AI upscaling and AI noise reduction; JPEG artifact removal; colorizing black-and-white
  photographs; restoring damaged ones and faces; skin retouched by a model (beyond P's Smooth
  Skin, a masked Develop layer); color and style transfer.
- Merging photographs: aligning layers by their content, panoramas, HDR, focus stacking,
  stacking for less noise. *(new)* Ghosts taken out (what moved between exposures taken from
  one of them); a panorama's projection chosen (spherical, cylindrical, perspective) and its
  empty edges filled (LaMa) or cropped; HDR panoramas; the result kept in floating point.
- *(new)* HDR images: shown on HDR displays with their highlights above white; JPEGs and
  AVIFs with gain maps (Ultra HDR, ISO 21496-1, as phones take them) read and written.
- Generative fill and expand (a diffusion model run locally), if one fits the model manager's
  terms and the computer.
- Linked and embedded objects (a group, or a file, kept as one object, replaced in place).
- Channels: the Channels panel, split and merge, calculations and Apply Image between
  channels and images, saved selections as alpha channels.
- Image modes: grayscale, bitmap, indexed color and palettes (and GIF export), duotone;
  CMYK (soft proofing, separations, export) and Lab; 8, 16 and 32 bits a channel for export
  (the image itself is floating point). Match color; HDR toning.
- Stacks: layers combined by mean, median, minimum or maximum.
- Lighting effects and lens flare.
- Artboards; notes on the image; customizing which tools the tool strip shows.
- Importing from a scanner or camera (WIA); frame animation, with GIF and APNG export.
- The artistic gallery's chosen few (`design.md`, *Filters*): watercolor, colored pencil,
  charcoal, ink outlines, sumi-e, cutout, poster edges, graphic pen, halftone pattern, bas
  relief, stained glass, mosaic tiles, craquelure, grain, texturizer; and
  Photoshop-compatible filter plug-ins (8BF).
- Auto-stroke brush (like Photoshop's Art History Brush).
- Non-square pixels (separate horizontal and vertical resolutions, then showing pixels at
  their shape), as `todo.txt` describes.
- Pen input on macOS and Linux.

Not planned: slices (for web pages cut into images, which nobody makes any more), the count
tool, the frame tool (placements and clipping masks do its work), the render filters for
trees, flames and picture frames; 3D; video (the timeline, video layers and the video
filters); contact sheets and picture packages; AI that changes faces (age, expression,
gaze), which needs large models for little use in an editor like this one. *(new)* A photo
catalog, as Lightroom's Library is: a database of photographs across drives, collections
and smart collections, keyword hierarchies, stacks, maps and GPS tracks, previews to edit
offline, publishing services and cloud sync, books, slideshows, and print layouts of many
photographs. That is another application, which digiKam (GPL) and darktable already are;
Opal Studio works beside them (the Browse panel, ratings in XMP, editing what they open in
it). Tethered capture of its own (a watched folder serves); AI demosaicing and reflection
removal, until models whose terms allow them do them well.
