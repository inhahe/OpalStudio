# The Opal Studio API

Every method a script or assistant can call (see `design.md`, *Scripting*), made from the
editor's command registry by the test `api::tests::the_python_stubs_and_api_page_are_current`:
run it with `OPAL_WRITE_STUBS=1` to make this page again; do not edit it by hand. From Python,
each is a method of the editor (`editor.new(width=640, height=480)`), taking its parameters by
name; over the pipe, a JSON-RPC method of the same name.

## Connecting

The pipe's name and a token are in `api.json` in the user's settings folder
(`%APPDATA%\Opal Studio\config` on Windows), or in the file `OPAL_STUDIO_API` names. Each
request and each response is one line of JSON-RPC 2.0. A connection's first request must be
`hello`, with `token`; anything else is refused and the connection dropped. `hello` may also
name a `script` (the Python package gives the running script's file name): the connection is
then a script run, and once it closes, what it changed in each image one change after another
becomes one history state, `Script: <name>`. Without it, each call is a state of its own.

## Expressions

Every number, length, angle and pixel count a method takes may be an expression instead, as
text (`"Logo.right + 5mm"`), worked out by the editor when the call runs, against the image as
it is then: the language its fields take (see `design.md`, *Expressions*). It has numbers in
any unit, mixed (`2in + 3mm`; a bare number added to a length is in the call's `unit`, one
multiplying it a plain factor); `+ - * / ^` and parentheses; `min`, `max`, `abs`, `round`,
`floor`, `ceil` (to whole pixels, or a step: `round(x, 1mm)`), `sqrt`, `sin`, `cos`, `tan`,
`asin`, `acos`, `atan`, `atan2`, `pi`; the canvas and layers, by name or `layer(id)`, as bounds
(`left`, `right`, `top`, `bottom`, `width`, `height`, `x`, `y`, and the points `top_left`,
`top_right`, `bottom_left`, `bottom_right`, `center`, each with `.x` and `.y`), positions from
what the call counts from (the canvas's top-left corner, unless `from_layer` and `from_anchor`
say); points (`mid`, `lerp`, `dist`, `point(x, y)`); the image's `ppi`; and for a call that
places a box, the box as it was (`x`, `y`, `width`, `height`, `angle`): `transform(x="x + 1cm")`
moves the selected layers 1 cm right. What cannot be worked out is refused, saying where. The
Python package builds the same text with operators: `opal.layer(logo).right + opal.mm(5)`,
`opal.box.width / 2`, `opal.fn.mid(a, b)` (`opal.expr`).

## Commands

### `new`

Create a new image.

| Parameter | Kind | |
|---|---|---|
| `width` (required) | length | Its width. From 1.0 to 300000.0 pixels. |
| `height` (required) | length | Its height. From 1.0 to 300000.0 pixels. |
| `resolution` | number | Its resolution, in pixels per inch: how large its pixels print, and what lengths in other units than pixels are converted at. From 0.01 to 1000000.0. Default: 72. |
| `unit` | `px`, `pt`, `pc`, `in`, `ft`, `yd`, `mm`, `cm`, `m` | What the lengths given as plain numbers are in: pixels, points, picas, inches, feet, yards, millimeters, centimeters or meters, converted at the image's resolution. A length can also be given as text with a unit of its own: "8.5in", "21 cm". Default: "px". |
| `background` | `white`, `transparent` | What it starts filled with. Default: "white". |

### `open`

Open an image or an Opal Studio file.

| Parameter | Kind | |
|---|---|---|
| `path` (required) | path | The file, by its absolute path: an Opal Studio file, a PNG or a JPEG. |

### `save`

Save the image as an Opal Studio file.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |

### `save_as`

Save the image under a new name.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `path` (required) | path | Where: an Opal Studio file (.opal is added unless it is there), by its absolute path. |
| `overwrite` | bool | Write over a file already there (otherwise that is refused). Default: false. |

### `export`

Save a flattened copy as PNG or JPEG.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `path` (required) | path | Where: a .png, .jpg or .jpeg file (whose extension chooses the format), by its absolute path. |
| `width` | length | The width written. From 1.0 to 300000.0 pixels. Default: the canvas's, or in proportion to the height given. |
| `height` | length | The height written. From 1.0 to 300000.0 pixels. Default: the canvas's, or in proportion to the width given. |
| `unit` | `px`, `pt`, `pc`, `in`, `ft`, `yd`, `mm`, `cm`, `m` | What the lengths given as plain numbers are in: pixels, points, picas, inches, feet, yards, millimeters, centimeters or meters, converted at the image's resolution. A length can also be given as text with a unit of its own: "8.5in", "21 cm". Default: "px". |
| `overwrite` | bool | Write over a file already there (otherwise that is refused). Default: false. |
| `sixteen_bit` | bool | For a PNG: 16 bits a channel rather than 8. Default: the preference. |
| `quality` | int | For a JPEG: its quality. From 1 to 100. Default: the preference. |
| `metadata` | `all`, `no place`, `copyright`, `none` | What of the image's metadata (the EXIF, IPTC and XMP its file had) goes into the file: all; all but where it was taken and serial numbers; only who made it, its copyright and how to reach them; or none. Default: the preference. |

### `close`

Close the image.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `discard` | bool | Close it even with unsaved changes, losing them. Default: false. |

### `undo`

Take back the last action.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |

### `redo`

Repeat the action you undid.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |

### `cut`

Copy what is selected on the selected layers, then erase it.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layers` | layers | The layers, by id. Default: the selected layers. |

### `copy`

Copy the selected layers, as they look, to the clipboard.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |

### `copy_merged`

Copy everything visible within the canvas to the clipboard.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |

### `paste`

Paste the image on the clipboard, then place and size it.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image; with none open, the paste becomes a new image. |
| `x` | length | Where the box's anchor point (its top-left corner, unless anchor says) goes, across from the point from_layer and from_anchor give (the canvas's top-left corner, unless they say). From -10000000.0 to 10000000.0 pixels. Default: where the command puts it. |
| `y` | length | Where the box's anchor point (its top-left corner, unless anchor says) goes, down from the point from_layer and from_anchor give (the canvas's top-left corner, unless they say). From -10000000.0 to 10000000.0 pixels. Default: where the command puts it. |
| `width` | length | The box's width. From 0.01 to 1000000.0 pixels. Default: in proportion to the height given, or as it is. |
| `height` | length | The box's height. From 0.01 to 1000000.0 pixels. Default: in proportion to the width given, or as it is. |
| `angle` | angle | The box's angle, in degrees clockwise, about its anchor point. From -360.0 to 360.0 degrees. Default: as it is. |
| `fit` | `stretch`, `inside`, `cover` | With both width and height given: stretch it to them (along its own sides, turned as it is); or keep its proportions and fit it, as it is turned, inside the rectangle they make on the canvas (placed by its anchor point at x and y), or make it cover that rectangle, centered on it. Default: "stretch". |
| `anchor` | `top-left`, `top`, `top-right`, `left`, `center`, `right`, `bottom-left`, `bottom`, `bottom-right` | Which point of the box x and y give, and what a size or angle given scales and turns it about (a rectangle given with width and height is placed by its same point). Default: "top-left". |
| `from_layer` | layer | A layer x and y count from (from a point of the bounds of what it holds, as from_anchor says), rather than from the canvas. Held where it is when the call begins (a layer moved counts from where it was). Default: the canvas. |
| `from_anchor` | `top-left`, `top`, `top-right`, `left`, `center`, `right`, `bottom-left`, `bottom`, `bottom-right` | Which point of the canvas (or of from_layer) x and y count from. Default: "top-left". |
| `into_active` | bool | Draw it into the pixels of the active layer (a plain, unlocked one) rather than onto a new layer. Default: false. |
| `unit` | `px`, `pt`, `pc`, `in`, `ft`, `yd`, `mm`, `cm`, `m` | What the lengths given as plain numbers are in: pixels, points, picas, inches, feet, yards, millimeters, centimeters or meters, converted at the image's resolution. A length can also be given as text with a unit of its own: "8.5in", "21 cm". Default: "px". |

### `paste_in_place`

Paste where the copy came from.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image; with none open, the paste becomes a new image. |
| `x` | length | Where the box's anchor point (its top-left corner, unless anchor says) goes, across from the point from_layer and from_anchor give (the canvas's top-left corner, unless they say). From -10000000.0 to 10000000.0 pixels. Default: where the command puts it. |
| `y` | length | Where the box's anchor point (its top-left corner, unless anchor says) goes, down from the point from_layer and from_anchor give (the canvas's top-left corner, unless they say). From -10000000.0 to 10000000.0 pixels. Default: where the command puts it. |
| `width` | length | The box's width. From 0.01 to 1000000.0 pixels. Default: in proportion to the height given, or as it is. |
| `height` | length | The box's height. From 0.01 to 1000000.0 pixels. Default: in proportion to the width given, or as it is. |
| `angle` | angle | The box's angle, in degrees clockwise, about its anchor point. From -360.0 to 360.0 degrees. Default: as it is. |
| `fit` | `stretch`, `inside`, `cover` | With both width and height given: stretch it to them (along its own sides, turned as it is); or keep its proportions and fit it, as it is turned, inside the rectangle they make on the canvas (placed by its anchor point at x and y), or make it cover that rectangle, centered on it. Default: "stretch". |
| `anchor` | `top-left`, `top`, `top-right`, `left`, `center`, `right`, `bottom-left`, `bottom`, `bottom-right` | Which point of the box x and y give, and what a size or angle given scales and turns it about (a rectangle given with width and height is placed by its same point). Default: "top-left". |
| `from_layer` | layer | A layer x and y count from (from a point of the bounds of what it holds, as from_anchor says), rather than from the canvas. Held where it is when the call begins (a layer moved counts from where it was). Default: the canvas. |
| `from_anchor` | `top-left`, `top`, `top-right`, `left`, `center`, `right`, `bottom-left`, `bottom`, `bottom-right` | Which point of the canvas (or of from_layer) x and y count from. Default: "top-left". |
| `into_active` | bool | Draw it into the pixels of the active layer (a plain, unlocked one) rather than onto a new layer. Default: false. |
| `unit` | `px`, `pt`, `pc`, `in`, `ft`, `yd`, `mm`, `cm`, `m` | What the lengths given as plain numbers are in: pixels, points, picas, inches, feet, yards, millimeters, centimeters or meters, converted at the image's resolution. A length can also be given as text with a unit of its own: "8.5in", "21 cm". Default: "px". |

### `place`

Place an image file into this image, then position and size it.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `path` (required) | path | The image file, a PNG or a JPEG, by its absolute path. |
| `name` | text | The new layer's name. Default: the file's name. |
| `print_size` | bool | Scale the file by its resolution against the image's, so it prints as large as it did (before a width or height given), rather than place it pixel for pixel. A file that does not say its resolution is placed pixel for pixel either way. Default: the preference (Transform › Place at the file's print size). |
| `x` | length | Where the box's anchor point (its top-left corner, unless anchor says) goes, across from the point from_layer and from_anchor give (the canvas's top-left corner, unless they say). From -10000000.0 to 10000000.0 pixels. Default: where the command puts it. |
| `y` | length | Where the box's anchor point (its top-left corner, unless anchor says) goes, down from the point from_layer and from_anchor give (the canvas's top-left corner, unless they say). From -10000000.0 to 10000000.0 pixels. Default: where the command puts it. |
| `width` | length | The box's width. From 0.01 to 1000000.0 pixels. Default: in proportion to the height given, or as it is. |
| `height` | length | The box's height. From 0.01 to 1000000.0 pixels. Default: in proportion to the width given, or as it is. |
| `angle` | angle | The box's angle, in degrees clockwise, about its anchor point. From -360.0 to 360.0 degrees. Default: as it is. |
| `fit` | `stretch`, `inside`, `cover` | With both width and height given: stretch it to them (along its own sides, turned as it is); or keep its proportions and fit it, as it is turned, inside the rectangle they make on the canvas (placed by its anchor point at x and y), or make it cover that rectangle, centered on it. Default: "stretch". |
| `anchor` | `top-left`, `top`, `top-right`, `left`, `center`, `right`, `bottom-left`, `bottom`, `bottom-right` | Which point of the box x and y give, and what a size or angle given scales and turns it about (a rectangle given with width and height is placed by its same point). Default: "top-left". |
| `from_layer` | layer | A layer x and y count from (from a point of the bounds of what it holds, as from_anchor says), rather than from the canvas. Held where it is when the call begins (a layer moved counts from where it was). Default: the canvas. |
| `from_anchor` | `top-left`, `top`, `top-right`, `left`, `center`, `right`, `bottom-left`, `bottom`, `bottom-right` | Which point of the canvas (or of from_layer) x and y count from. Default: "top-left". |
| `into_active` | bool | Draw it into the pixels of the active layer (a plain, unlocked one) rather than onto a new layer. Default: false. |
| `unit` | `px`, `pt`, `pc`, `in`, `ft`, `yd`, `mm`, `cm`, `m` | What the lengths given as plain numbers are in: pixels, points, picas, inches, feet, yards, millimeters, centimeters or meters, converted at the image's resolution. A length can also be given as text with a unit of its own: "8.5in", "21 cm". Default: "px". |

### `open_as_layers`

Open image files as layers of this image, one each.

| Parameter | Kind | |
|---|---|---|
| `paths` (required) | paths | The files, PNG or JPEG, by their absolute paths, the first the lowest layer: into the active image, or a new one if none is open. |

### `transform`

Move, scale and rotate the selected layers with a box.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layers` | layers | The layers, by id. Default: the selected layers. |
| `x` | length | Where the box's anchor point (its top-left corner, unless anchor says) goes, across from the point from_layer and from_anchor give (the canvas's top-left corner, unless they say). From -10000000.0 to 10000000.0 pixels. Default: where the command puts it. |
| `y` | length | Where the box's anchor point (its top-left corner, unless anchor says) goes, down from the point from_layer and from_anchor give (the canvas's top-left corner, unless they say). From -10000000.0 to 10000000.0 pixels. Default: where the command puts it. |
| `width` | length | The box's width. From 0.01 to 1000000.0 pixels. Default: in proportion to the height given, or as it is. |
| `height` | length | The box's height. From 0.01 to 1000000.0 pixels. Default: in proportion to the width given, or as it is. |
| `angle` | angle | The box's angle, in degrees clockwise, about its anchor point. From -360.0 to 360.0 degrees. Default: as it is. |
| `fit` | `stretch`, `inside`, `cover` | With both width and height given: stretch it to them (along its own sides, turned as it is); or keep its proportions and fit it, as it is turned, inside the rectangle they make on the canvas (placed by its anchor point at x and y), or make it cover that rectangle, centered on it. Default: "stretch". |
| `anchor` | `top-left`, `top`, `top-right`, `left`, `center`, `right`, `bottom-left`, `bottom`, `bottom-right` | Which point of the box x and y give, and what a size or angle given scales and turns it about (a rectangle given with width and height is placed by its same point). Default: "top-left". |
| `from_layer` | layer | A layer x and y count from (from a point of the bounds of what it holds, as from_anchor says), rather than from the canvas. Held where it is when the call begins (a layer moved counts from where it was). Default: the canvas. |
| `from_anchor` | `top-left`, `top`, `top-right`, `left`, `center`, `right`, `bottom-left`, `bottom`, `bottom-right` | Which point of the canvas (or of from_layer) x and y count from. Default: "top-left". |
| `unit` | `px`, `pt`, `pc`, `in`, `ft`, `yd`, `mm`, `cm`, `m` | What the lengths given as plain numbers are in: pixels, points, picas, inches, feet, yards, millimeters, centimeters or meters, converted at the image's resolution. A length can also be given as text with a unit of its own: "8.5in", "21 cm". Default: "px". |

### `transform_again`

Do to the selected layers what the last Transform did.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layers` | layers | The layers, by id. Default: the selected layers. |

### `transform_again_copy`

Copy the selected layers and do to the copies what the last Transform did.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layers` | layers | The layers, by id. Default: the selected layers. |

### `reapply_placement`

Place the active layer again as the expressions it was placed by say, now.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layer` | layer | The layer, by id. Default: the active layer. |

### `reapply_all_placements`

Place every layer placed by expressions again, as they say now.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |

### `new_layer`

Add an empty layer above the active one.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `name` | text | Its name. Default: a numbered one. |

### `new_group`

Add a group to hold layers.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `name` | text | Its name. Default: a numbered one. |

### `duplicate`

Copy the selected layers.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layers` | layers | The layers, by id. Default: the selected layers. |

### `layer_via_copy`

Put what is selected on the selected layers on a layer of its own (Ctrl+J).

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layers` | layers | The layers, by id. Default: the selected layers. |

### `layer_via_cut`

Move what is selected on the selected layers onto a layer of its own.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layers` | layers | The layers, by id. Default: the selected layers. |

### `group_layers`

Put the selected layers into a new group.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layers` | layers | The layers, by id. Default: the selected layers. |

### `delete_layer`

Remove the selected layers.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layers` | layers | The layers, by id. Default: the selected layers. |

### `move_up`

Move the active layer one place up.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layer` | layer | The layer, by id. Default: the active layer. |

### `move_down`

Move the active layer one place down.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layer` | layer | The layer, by id. Default: the active layer. |

### `bring_to_front`

Move the selected layers to the top of their group.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layers` | layers | The layers, by id. Default: the selected layers. |

### `send_to_back`

Move the selected layers to the bottom of their group.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layers` | layers | The layers, by id. Default: the selected layers. |

### `reverse_layers`

Turn the selected layers' order round.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layers` | layers | The layers, by id. Default: the selected layers. |

### `select_all_layers`

Select every layer.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |

### `deselect_layers`

Select no layer.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |

### `align_left`

Line the selected layers' left edges up.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layers` | layers | The layers, by id: the first the active one, which they line up to with `to` key. Default: the selected layers. |
| `to` | `layers`, `canvas`, `selection`, `key` | What they line up with, or spread across: the box round them (aligning one, the canvas; distributing, among themselves, the outermost two staying), the canvas, the box round what is selected, or the active layer (the first given), which stays. Default: "layers". |

### `align_horizontal_centers`

Line the selected layers' centers up on a vertical line.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layers` | layers | The layers, by id: the first the active one, which they line up to with `to` key. Default: the selected layers. |
| `to` | `layers`, `canvas`, `selection`, `key` | What they line up with, or spread across: the box round them (aligning one, the canvas; distributing, among themselves, the outermost two staying), the canvas, the box round what is selected, or the active layer (the first given), which stays. Default: "layers". |

### `align_right`

Line the selected layers' right edges up.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layers` | layers | The layers, by id: the first the active one, which they line up to with `to` key. Default: the selected layers. |
| `to` | `layers`, `canvas`, `selection`, `key` | What they line up with, or spread across: the box round them (aligning one, the canvas; distributing, among themselves, the outermost two staying), the canvas, the box round what is selected, or the active layer (the first given), which stays. Default: "layers". |

### `align_top`

Line the selected layers' top edges up.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layers` | layers | The layers, by id: the first the active one, which they line up to with `to` key. Default: the selected layers. |
| `to` | `layers`, `canvas`, `selection`, `key` | What they line up with, or spread across: the box round them (aligning one, the canvas; distributing, among themselves, the outermost two staying), the canvas, the box round what is selected, or the active layer (the first given), which stays. Default: "layers". |

### `align_vertical_centers`

Line the selected layers' centers up on a horizontal line.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layers` | layers | The layers, by id: the first the active one, which they line up to with `to` key. Default: the selected layers. |
| `to` | `layers`, `canvas`, `selection`, `key` | What they line up with, or spread across: the box round them (aligning one, the canvas; distributing, among themselves, the outermost two staying), the canvas, the box round what is selected, or the active layer (the first given), which stays. Default: "layers". |

### `align_bottom`

Line the selected layers' bottom edges up.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layers` | layers | The layers, by id: the first the active one, which they line up to with `to` key. Default: the selected layers. |
| `to` | `layers`, `canvas`, `selection`, `key` | What they line up with, or spread across: the box round them (aligning one, the canvas; distributing, among themselves, the outermost two staying), the canvas, the box round what is selected, or the active layer (the first given), which stays. Default: "layers". |

### `distribute_horizontal_centers`

Space the selected layers' centers evenly from side to side.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layers` | layers | The layers, by id: the first the active one, which they line up to with `to` key. Default: the selected layers. |
| `to` | `layers`, `canvas`, `selection`, `key` | What they line up with, or spread across: the box round them (aligning one, the canvas; distributing, among themselves, the outermost two staying), the canvas, the box round what is selected, or the active layer (the first given), which stays. Default: "layers". |

### `distribute_vertical_centers`

Space the selected layers' centers evenly from top to bottom.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layers` | layers | The layers, by id: the first the active one, which they line up to with `to` key. Default: the selected layers. |
| `to` | `layers`, `canvas`, `selection`, `key` | What they line up with, or spread across: the box round them (aligning one, the canvas; distributing, among themselves, the outermost two staying), the canvas, the box round what is selected, or the active layer (the first given), which stays. Default: "layers". |

### `distribute_horizontal_space`

Make the gaps between the selected layers, side to side, all alike.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layers` | layers | The layers, by id: the first the active one, which they line up to with `to` key. Default: the selected layers. |
| `to` | `layers`, `canvas`, `selection`, `key` | What they line up with, or spread across: the box round them (aligning one, the canvas; distributing, among themselves, the outermost two staying), the canvas, the box round what is selected, or the active layer (the first given), which stays. Default: "layers". |

### `distribute_vertical_space`

Make the gaps between the selected layers, top to bottom, all alike.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layers` | layers | The layers, by id: the first the active one, which they line up to with `to` key. Default: the selected layers. |
| `to` | `layers`, `canvas`, `selection`, `key` | What they line up with, or spread across: the box round them (aligning one, the canvas; distributing, among themselves, the outermost two staying), the canvas, the box round what is selected, or the active layer (the first given), which stays. Default: "layers". |

### `merge_down`

Combine the active layer with the layer below it.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layer` | layer | The layer, by id. Default: the active layer. |

### `merge_selected`

Combine the selected layers into one.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layers` | layers | The layers, by id. Default: the selected layers. |

### `merge_visible`

Combine every visible layer into one.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |

### `merge_group`

Turn the active group into one pixel layer.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layer` | layer | The layer, by id. Default: the active layer. |

### `stamp_visible`

Add a layer holding everything visible, combined.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |

### `flatten`

Combine all visible layers into one.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |

### `rasterize`

Turn the selected layers' images into plain pixels.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layers` | layers | The layers, by id. Default: the selected layers. |

### `defringe`

Give the selected layers' edge pixels the colour of the pixels further in.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layers` | layers | The layers, by id. Default: the selected layers. |
| `width` | int | How close to transparency (in pixels, diagonals one) an edge pixel is to be given the colour of those further in. From 1 to 200. Default: 1. |

### `remove_black_matte`

Take black back out of the selected layers' partly transparent edge pixels.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layers` | layers | The layers, by id. Default: the selected layers. |

### `remove_white_matte`

Take white back out of the selected layers' partly transparent edge pixels.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layers` | layers | The layers, by id. Default: the selected layers. |

### `select_all`

Select the whole canvas.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |

### `deselect`

Select nothing.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |

### `reselect`

Select again what was selected when you last deselected.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |

### `invert_selection`

Select what is not selected, and nothing of what is.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |

### `select_layer_pixels`

Select the active layer's pixels, each as much as it is opaque.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layer` | layer | The layer, by id. Default: the active layer. |
| `combine` | `replace`, `add`, `subtract`, `intersect` | How it combines with what is selected: in its place, added to it, taken away from it, or keeping only what is in both. Default: "replace". |

### `clear`

Erase what is selected on the selected layers.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layers` | layers | The layers, by id. Default: the selected layers. |

### `crop_to_selection`

Make the canvas the rectangle round the selection.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |

### `grow_selection`

Grow the selection by a distance, or shrink it.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `by` (required) | length | How far the outline moves out; less than 0, in. From -1000.0 to 1000.0 pixels. |
| `corners` | `round`, `square`, `diamond` | How its corners grow: round, square, or to a point as a diamond's. Default: "round". |
| `unit` | `px`, `pt`, `pc`, `in`, `ft`, `yd`, `mm`, `cm`, `m` | What the lengths given as plain numbers are in: pixels, points, picas, inches, feet, yards, millimeters, centimeters or meters, converted at the image's resolution. A length can also be given as text with a unit of its own: "8.5in", "21 cm". Default: "px". |

### `feather_selection`

Soften the selection's edge over a width.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `width` (required) | length | How wide the falloff from fully selected to not at all is. From 0.0 to 1000.0 pixels. |
| `side` | `inward`, `outward`, `centered` | Where the falloff lies: inside the outline, outside it, or half each way. Default: "centered". |
| `falloff` | `gaussian`, `linear` | Its shape: as a Gaussian blur's edge, or straight. Default: "gaussian". |
| `unit` | `px`, `pt`, `pc`, `in`, `ft`, `yd`, `mm`, `cm`, `m` | What the lengths given as plain numbers are in: pixels, points, picas, inches, feet, yards, millimeters, centimeters or meters, converted at the image's resolution. A length can also be given as text with a unit of its own: "8.5in", "21 cm". Default: "px". |

### `smooth_selection`

Smooth the selection's outline over a radius.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `radius` (required) | length | Pinholes, notches, specks and spurs narrower than twice it go, and corners round to it. From 0.0 to 1000.0 pixels. |
| `unit` | `px`, `pt`, `pc`, `in`, `ft`, `yd`, `mm`, `cm`, `m` | What the lengths given as plain numbers are in: pixels, points, picas, inches, feet, yards, millimeters, centimeters or meters, converted at the image's resolution. A length can also be given as text with a unit of its own: "8.5in", "21 cm". Default: "px". |

### `border_selection`

Select a band along the selection's outline.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `width` (required) | length | How wide the band along the outline is. From 0.0 to 1000.0 pixels. |
| `side` | `inward`, `outward`, `centered` | Where the band lies: inside the outline, outside it, or half each way. Default: "centered". |
| `unit` | `px`, `pt`, `pc`, `in`, `ft`, `yd`, `mm`, `cm`, `m` | What the lengths given as plain numbers are in: pixels, points, picas, inches, feet, yards, millimeters, centimeters or meters, converted at the image's resolution. A length can also be given as text with a unit of its own: "8.5in", "21 cm". Default: "px". |

### `transform_selection`

Move, size or turn the selection itself in a box.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `x` | length | Where the box's anchor point (its top-left corner, unless anchor says) goes, across from the point from_layer and from_anchor give (the canvas's top-left corner, unless they say). From -10000000.0 to 10000000.0 pixels. Default: where the command puts it. |
| `y` | length | Where the box's anchor point (its top-left corner, unless anchor says) goes, down from the point from_layer and from_anchor give (the canvas's top-left corner, unless they say). From -10000000.0 to 10000000.0 pixels. Default: where the command puts it. |
| `width` | length | The box's width. From 0.01 to 1000000.0 pixels. Default: in proportion to the height given, or as it is. |
| `height` | length | The box's height. From 0.01 to 1000000.0 pixels. Default: in proportion to the width given, or as it is. |
| `angle` | angle | The box's angle, in degrees clockwise, about its anchor point. From -360.0 to 360.0 degrees. Default: as it is. |
| `fit` | `stretch`, `inside`, `cover` | With both width and height given: stretch it to them (along its own sides, turned as it is); or keep its proportions and fit it, as it is turned, inside the rectangle they make on the canvas (placed by its anchor point at x and y), or make it cover that rectangle, centered on it. Default: "stretch". |
| `anchor` | `top-left`, `top`, `top-right`, `left`, `center`, `right`, `bottom-left`, `bottom`, `bottom-right` | Which point of the box x and y give, and what a size or angle given scales and turns it about (a rectangle given with width and height is placed by its same point). Default: "top-left". |
| `from_layer` | layer | A layer x and y count from (from a point of the bounds of what it holds, as from_anchor says), rather than from the canvas. Held where it is when the call begins (a layer moved counts from where it was). Default: the canvas. |
| `from_anchor` | `top-left`, `top`, `top-right`, `left`, `center`, `right`, `bottom-left`, `bottom`, `bottom-right` | Which point of the canvas (or of from_layer) x and y count from. Default: "top-left". |
| `unit` | `px`, `pt`, `pc`, `in`, `ft`, `yd`, `mm`, `cm`, `m` | What the lengths given as plain numbers are in: pixels, points, picas, inches, feet, yards, millimeters, centimeters or meters, converted at the image's resolution. A length can also be given as text with a unit of its own: "8.5in", "21 cm". Default: "px". |

### `save_selection`

Keep what is selected under a name, to load back.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `name` (required) | text | The name it is saved under: a selection saved under it already (whatever the case of its letters) is replaced. |

### `load_selection`

Select a saved selection again, or combine it with what is selected.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `name` (required) | text | The selection saved in the image under this name (whatever the case of its letters). |
| `combine` | `replace`, `add`, `subtract`, `intersect` | How it combines with what is selected: in its place, added to it, taken away from it, or keeping only what is in both. Default: "replace". |
| `invert` | bool | Invert it first: what it does not select, before it combines. Default: false. |

### `crop_to_content`

Shrink (or grow) the canvas to fit what the visible layers hold.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |

### `trim`

Cut the canvas in to the content, by transparency or the colour of a corner.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `based_on` | `transparent`, `top_left`, `bottom_right` | What the border is: transparent pixels, or the colour of the top-left or bottom-right pixel. Default: "transparent". |
| `tolerance` | number | How far a pixel may differ from the border and be border still: the largest difference of a channel (alpha included), from 0 to 255, in 8-bit sRGB terms. From 0.0 to 255.0. Default: 0. |
| `top` | bool | Trim the top. Default: true. |
| `bottom` | bool | Trim the bottom. Default: true. |
| `left` | bool | Trim the left. Default: true. |
| `right` | bool | Trim the right. Default: true. |

### `reveal_all`

Grow the canvas to show everything its layers hold, hidden ones too.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |

### `resize_image`

Change the image's size, scaling everything in it.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `width` | length | The new width. From 1.0 to 300000.0 pixels. Default: the canvas's, or in proportion to the height given. |
| `height` | length | The new height. From 1.0 to 300000.0 pixels. Default: the canvas's, or in proportion to the width given. |
| `resolution` | number | The new resolution, in pixels per inch, which lengths in other units than pixels are converted at. Given without a size, it changes no pixels: only how large they print. From 0.01 to 1000000.0. Default: the image's. |
| `unit` | `px`, `pt`, `pc`, `in`, `ft`, `yd`, `mm`, `cm`, `m` | What the lengths given as plain numbers are in: pixels, points, picas, inches, feet, yards, millimeters, centimeters or meters, converted at the image's resolution. A length can also be given as text with a unit of its own: "8.5in", "21 cm". Default: "px". |
| `resample` | bool | Resample the pixel layers now (smaller, but enlarging again later cannot bring their detail back), rather than keep their pixels and draw them at the new size. Default: the preference. |

### `resize_canvas`

Change the canvas's size without scaling what is on it.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `width` | length | The new width. From 1.0 to 300000.0 pixels. Default: the canvas's. |
| `height` | length | The new height. From 1.0 to 300000.0 pixels. Default: the canvas's. |
| `anchor` | `top-left`, `top`, `top-right`, `left`, `center`, `right`, `bottom-left`, `bottom`, `bottom-right` | Where the old canvas sits in the new. Default: the center. |
| `left` | length | How much to add on that side, rather than give a new size (a negative length cuts it). From -300000.0 to 300000.0 pixels. Default: none: the width and height say. |
| `top` | length | How much to add on that side, rather than give a new size (a negative length cuts it). From -300000.0 to 300000.0 pixels. Default: none: the width and height say. |
| `right` | length | How much to add on that side, rather than give a new size (a negative length cuts it). From -300000.0 to 300000.0 pixels. Default: none: the width and height say. |
| `bottom` | length | How much to add on that side, rather than give a new size (a negative length cuts it). From -300000.0 to 300000.0 pixels. Default: none: the width and height say. |
| `unit` | `px`, `pt`, `pc`, `in`, `ft`, `yd`, `mm`, `cm`, `m` | What the lengths given as plain numbers are in: pixels, points, picas, inches, feet, yards, millimeters, centimeters or meters, converted at the image's resolution. A length can also be given as text with a unit of its own: "8.5in", "21 cm". Default: "px". |

### `rotate_cw`

Turn the whole image a quarter turn clockwise.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |

### `rotate_ccw`

Turn the whole image a quarter turn counterclockwise.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |

### `rotate180`

Turn the whole image upside down.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |

### `rotate_by`

Turn the whole image by any angle.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `degrees` (required) | angle | The angle, clockwise. From -360.0 to 360.0 degrees. |
| `grow` | bool | Grow the canvas so that nothing is cut off. Default: true. |

### `straighten`

Turn the image so the line measured is level, or plumb.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `crop` | bool | Crop the turned image to the largest rectangle of its shape within what was there; false grows the canvas to hold all of it. Default: the Measure tool's Crop when straightening. |

### `straighten_layers`

Turn the selected layers so the line measured is level, or plumb.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layers` | layers | The layers, by id. Default: the selected layers. |

### `clear_measurement`

Remove the line the Measure tool measured.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |

### `flip_horizontal`

Mirror the whole image left to right.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |

### `flip_vertical`

Mirror the whole image top to bottom.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |

### `toggle_visible`

Show or hide the active layer.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layers` | layers | The layers, by id. Default: the selected layers. |
| `visible` | bool | Shown, or hidden. Default: each the opposite of what it is. |

### `toggle_lock`

Protect the active layer from changes.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layers` | layers | The layers, by id. Default: the selected layers. |
| `locked` | bool | Locked, or not. Default: each the opposite of what it is. |

### `rename`

Give the active layer a new name.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layer` | layer | The layer, by id. Default: the active layer. |
| `name` (required) | text | Its new name. |

### `set_metadata`

Changes an image's metadata (see the metadata query) as one step of history, as File Info does, and answers with all of it as the metadata query does. Export writes it as its metadata setting says.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `changes` (required) | any | What to change, as the metadata query tells it: description (the fields given; those not given kept), exif ({ifd: {tag: value}}), iptc ({"record:number": text or texts}) and xmp ({key: value}); null removes what it names. The description is written to XMP, IPTC and EXIF alike, as File Info writes it; the rest as given, and where another part holds the same (an IPTC dataset XMP holds too, as the city or the date created; EXIF's ImageDescription, Artist and Copyright), changed there too, unless the changes change that as well. |

### `set_file_metadata`

Rewrites a JPEG's or PNG's metadata in place, its image data untouched (not decoded, nor encoded again: no quality lost), and answers with all of it as file_metadata does. How a folder of photographs has its capture times shifted, or its captions filled in, without opening them. A part the file has that Opal Studio could not read (XMP no XML reader takes, say) is not written over: a change to it is refused, the file left as it is.

| Parameter | Kind | |
|---|---|---|
| `path` (required) | path | The file, by its absolute path. |
| `changes` (required) | any | What to change, as set_metadata takes it. |

### `measure`

Measures on an image as the Measure tool does, between points from the canvas's corner: two, a line from the first to the second; three, a protractor's second arm too, from the first to the third. What is measured stays until the next measure, and goes with the image turned or resized; straighten and straighten_layers use it. Answers as the measurement query does.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `points` (required) | points | The line's start and end, and a protractor's second arm's end if it has one, each [x, y] in pixels from the canvas's top-left corner. |

### `select_outline`

Selects what an outline encloses, as the outline tool does: points joined by straight sides, or an outline with curves, as the selection query tells one; the last point back to the first (a loop crossing itself selects each of its lobes). Give points or outline, one of them. Answers with what is selected then, as the selection query does.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `points` | points | The outline's points, in order, each [x, y] in pixels from the canvas's top-left corner, joined by straight sides. Default: outline, given instead. |
| `outline` | outline | The outline, as the selection query tells an outline step: its start [x, y]; its segments, each a line ({"kind": "line", "to": [x, y]}) or a curve ({"kind": "curve", "c1": [x, y], "c2": [x, y], "to": [x, y]}, pulled toward c1 and c2); and, if wanted, how its line goes through each point (joins: "sharp", "smooth" or "symmetric", one for each, its start first), and its symmetry (symmetry: {"order": copies round, "mirror": true or false, "center": [x, y], "angle": the axis's, in degrees clockwise from up}; then start and segments are one slice, not closed, which its copies complete). In pixels from the canvas's top-left corner. Default: points, given instead. |
| `combine` | `replace`, `add`, `subtract`, `intersect` | How it combines with what is selected: in its place, added to it, taken away from it, or keeping only what is in both. Default: "replace". |
| `smooth` | bool | Select a pixel the outline crosses as much as it encloses it; else all or nothing, by the pixel's middle. Default: the outline tool's Smooth edges. |

### `delete_saved_selection`

Deletes a selection saved in the image (as the Selection panel does), by its name. Answers with the selections saved then, as saved_selections tells them.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `name` (required) | text | The selection saved in the image under this name (whatever the case of its letters). |

### `remove_selection_step`

Takes a step away from the recipe of what is selected, by its number among the selection query's steps; the selection is worked out again from the rest (as the selection panel does). Answers with what is selected then.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `index` (required) | int | Which step: its number among the selection query's steps, from 0. From 0 to 1048576. |

### `edit_selection_step`

Changes a step of the recipe of what is selected, by its number among the selection query's steps: a shape's or a mask's combine; an outline's combine and outline (its points, curves and joins); a grow's by and corners; a feather's width, side and falloff; a smooth's radius; a border's width and side (what is not given stays as it is; what the step does not take is refused). The selection is worked out again with it changed (as the selection panel does). Answers with what is selected then.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `index` (required) | int | Which step: its number among the selection query's steps, from 0. From 0 to 1048576. |
| `combine` | `replace`, `add`, `subtract`, `intersect` | A shape's, an outline's or a mask's: how it combines with the steps before it. Default: as it is. |
| `outline` | outline | An outline's: the outline it becomes, as select_outline takes one (its points, curves and joins, from the canvas's top-left corner; its smoothing as it was, unless it says). Default: as it is. |
| `by` | length | A grow's: how far it grows the selection (shrinks it, less than 0). From -1000.0 to 1000.0 pixels. Default: as it is. |
| `corners` | `round`, `square`, `diamond` | A grow's: how its corners grow. Default: as it is. |
| `width` | length | A feather's or a border's: how wide its falloff or its band is. From 0.0 to 1000.0 pixels. Default: as it is. |
| `side` | `inward`, `outward`, `centered` | A feather's or a border's: on which side of the outline it lies. Default: as it is. |
| `falloff` | `gaussian`, `linear` | A feather's: the shape of its falloff. Default: as it is. |
| `radius` | length | A smooth's: the radius it smooths over. From 0.0 to 1000.0 pixels. Default: as it is. |
| `unit` | `px`, `pt`, `pc`, `in`, `ft`, `yd`, `mm`, `cm`, `m` | What the lengths given as plain numbers are in: pixels, points, picas, inches, feet, yards, millimeters, centimeters or meters, converted at the image's resolution. A length can also be given as text with a unit of its own: "8.5in", "21 cm". Default: "px". |

### `select_rectangle`

Selects a rectangle, which may be turned, as the Select tool's box numbers give it: the canvas, unless the numbers say otherwise. Answers with what is selected then, as the selection query does.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `x` | length | Where the shape's anchor point is (of the rectangle round an ellipse), across from the point from_layer and from_anchor give (the canvas's top-left corner, unless they say). From -10000000.0 to 10000000.0 pixels. Default: where that point of the canvas is. |
| `y` | length | Where the shape's anchor point is, down from the point from_layer and from_anchor give (the canvas's top-left corner, unless they say). From -10000000.0 to 10000000.0 pixels. Default: where that point of the canvas is. |
| `width` | length | The shape's width (an ellipse's across). From 0.01 to 10000000.0 pixels. Default: the canvas's. |
| `height` | length | The shape's height. From 0.01 to 10000000.0 pixels. Default: the canvas's. |
| `angle` | angle | How far the shape is turned, in degrees clockwise, about its anchor point. From -360.0 to 360.0 degrees. Default: 0. |
| `combine` | `replace`, `add`, `subtract`, `intersect` | How it combines with what is selected: in its place, added to it, taken away from it, or keeping only what is in both. Default: "replace". |
| `smooth` | bool | Select a pixel its edge crosses as much as the shape covers it; else all or nothing, by the pixel's middle. Default: the Select tool's Smooth edges. |
| `anchor` | `top-left`, `top`, `top-right`, `left`, `center`, `right`, `bottom-left`, `bottom`, `bottom-right` | Which point of the shape (of the rectangle round an ellipse) x and y give, and what the angle turns it about. Default: "top-left". |
| `from_layer` | layer | A layer x and y count from (from a point of the bounds of what it holds, as from_anchor says), rather than from the canvas. Held where it is when the call begins (a layer moved counts from where it was). Default: the canvas. |
| `from_anchor` | `top-left`, `top`, `top-right`, `left`, `center`, `right`, `bottom-left`, `bottom`, `bottom-right` | Which point of the canvas (or of from_layer) x and y count from. Default: "top-left". |
| `unit` | `px`, `pt`, `pc`, `in`, `ft`, `yd`, `mm`, `cm`, `m` | What the lengths given as plain numbers are in: pixels, points, picas, inches, feet, yards, millimeters, centimeters or meters, converted at the image's resolution. A length can also be given as text with a unit of its own: "8.5in", "21 cm". Default: "px". |

### `select_ellipse`

Selects an ellipse, given by the rectangle round it (which may be turned), as the Select tool's box numbers give it: the one within the canvas, unless the numbers say otherwise. Answers with what is selected then, as the selection query does.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `x` | length | Where the shape's anchor point is (of the rectangle round an ellipse), across from the point from_layer and from_anchor give (the canvas's top-left corner, unless they say). From -10000000.0 to 10000000.0 pixels. Default: where that point of the canvas is. |
| `y` | length | Where the shape's anchor point is, down from the point from_layer and from_anchor give (the canvas's top-left corner, unless they say). From -10000000.0 to 10000000.0 pixels. Default: where that point of the canvas is. |
| `width` | length | The shape's width (an ellipse's across). From 0.01 to 10000000.0 pixels. Default: the canvas's. |
| `height` | length | The shape's height. From 0.01 to 10000000.0 pixels. Default: the canvas's. |
| `angle` | angle | How far the shape is turned, in degrees clockwise, about its anchor point. From -360.0 to 360.0 degrees. Default: 0. |
| `combine` | `replace`, `add`, `subtract`, `intersect` | How it combines with what is selected: in its place, added to it, taken away from it, or keeping only what is in both. Default: "replace". |
| `smooth` | bool | Select a pixel its edge crosses as much as the shape covers it; else all or nothing, by the pixel's middle. Default: the Select tool's Smooth edges. |
| `anchor` | `top-left`, `top`, `top-right`, `left`, `center`, `right`, `bottom-left`, `bottom`, `bottom-right` | Which point of the shape (of the rectangle round an ellipse) x and y give, and what the angle turns it about. Default: "top-left". |
| `from_layer` | layer | A layer x and y count from (from a point of the bounds of what it holds, as from_anchor says), rather than from the canvas. Held where it is when the call begins (a layer moved counts from where it was). Default: the canvas. |
| `from_anchor` | `top-left`, `top`, `top-right`, `left`, `center`, `right`, `bottom-left`, `bottom`, `bottom-right` | Which point of the canvas (or of from_layer) x and y count from. Default: "top-left". |
| `unit` | `px`, `pt`, `pc`, `in`, `ft`, `yd`, `mm`, `cm`, `m` | What the lengths given as plain numbers are in: pixels, points, picas, inches, feet, yards, millimeters, centimeters or meters, converted at the image's resolution. A length can also be given as text with a unit of its own: "8.5in", "21 cm". Default: "px". |

### `crop`

Crops the image to a rectangle of its canvas, which may be turned: the canvas becomes the rectangle, and if it was turned, everything turns the other way, so its contents come out upright. Only the canvas moves (enlarging it again brings back what was cropped away), unless delete says.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `x` | length | Where the rectangle's anchor point is, across from the point from_layer and from_anchor give (the canvas's top-left corner, unless they say). From -10000000.0 to 10000000.0 pixels. Default: where that point of the canvas is. |
| `y` | length | Where the rectangle's anchor point is, down from the point from_layer and from_anchor give (the canvas's top-left corner, unless they say). From -10000000.0 to 10000000.0 pixels. Default: where that point of the canvas is. |
| `width` | length | The rectangle's width. From 1.0 to 300000.0 pixels. Default: the canvas's. |
| `height` | length | The rectangle's height. From 1.0 to 300000.0 pixels. Default: the canvas's. |
| `angle` | angle | The rectangle's angle, in degrees clockwise, about its anchor point. From -360.0 to 360.0 degrees. Default: 0. |
| `delete` | bool | Remove the pixels outside the new canvas, rather than keep them for later. Default: the Crop tool's Delete cropped pixels. |
| `anchor` | `top-left`, `top`, `top-right`, `left`, `center`, `right`, `bottom-left`, `bottom`, `bottom-right` | Which point of the rectangle x and y give, and what the angle turns it about. Default: "top-left". |
| `from_layer` | layer | A layer x and y count from (from a point of the bounds of what it holds, as from_anchor says), rather than from the canvas. Held where it is when the call begins (a layer moved counts from where it was). Default: the canvas. |
| `from_anchor` | `top-left`, `top`, `top-right`, `left`, `center`, `right`, `bottom-left`, `bottom`, `bottom-right` | Which point of the canvas (or of from_layer) x and y count from. Default: "top-left". |
| `unit` | `px`, `pt`, `pc`, `in`, `ft`, `yd`, `mm`, `cm`, `m` | What the lengths given as plain numbers are in: pixels, points, picas, inches, feet, yards, millimeters, centimeters or meters, converted at the image's resolution. A length can also be given as text with a unit of its own: "8.5in", "21 cm". Default: "px". |

### `set_preference`

Changes a preference, as the Preferences window would, or puts it back to its default. Those of scripting (what runs scripts, who may connect) are changed in the Preferences window only.

| Parameter | Kind | |
|---|---|---|
| `key` (required) | text | The preference, by its key (the preferences query lists them). |
| `value` | any | Its new value: true or false, a number in its range, one of its choices, a color as [red, green, blue] or [red, green, blue, alpha] (0 to 255, sRGB), or text. Default: its default. |

### `put_pixels`

Puts pixels into an image, at a place on its canvas: as a new layer above the active one, or into a pixel layer, replacing its pixels there. They come in shared memory the caller made, or in base64.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layer` | layer | A pixel layer, by id, whose pixels they replace there. Default: a new layer, above the active one. |
| `x` | pixels | Where their left edge goes, in pixels from the canvas's. From -300000 to 600000 pixels. Default: 0. |
| `y` | pixels | Where their top edge goes, in pixels from the canvas's. From -300000 to 600000 pixels. Default: 0. |
| `width` (required) | pixels | How many pixels across. From 1 to 300000 pixels. |
| `height` (required) | pixels | How many pixels down. From 1 to 300000 pixels. |
| `format` | `srgb8`, `srgb16`, `linear` | How each pixel is given, row by row from the top left, red, green, blue and alpha (straight, not premultiplied): srgb8 as bytes in sRGB; srgb16 as 16-bit values in sRGB, little-endian; linear as 32-bit floats in linear light, little-endian. Default: "srgb8". |
| `shm` | text | The shared memory holding them, by name: made by the caller, with width × height pixels. Default: none: they are in data. |
| `data` | text | The pixels in base64, if not in shared memory (a request is at most 1 MB). Default: none: they are in shm. |
| `name` | text | The new layer's name. Default: a numbered one. |

## Queries

### `metadata`

What an image's file said of it besides its pixels (EXIF, IPTC, XMP), kept with it: description (as File Info edits it: title, caption, creators, email, website, copyright, copyright_status (copyrighted, public domain or unknown), copyright_url, keywords), capture (as File Info shows it, label by label), exif (IFD by IFD: image, exif, gps, interop, thumbnail; each tag by its name, or 0xNNNN if it has none: text as text, numbers as numbers, a fraction worked out), iptc (each dataset by record:number, as 2:120 for the caption, a list of texts) and xmp (each property by prefix:name, or {namespace}name for a namespace with no usual prefix: text, a list, {language: text} for an alternative, {field: value} for a structure).

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |

### `file_metadata`

An image file's metadata, as the metadata query tells it, read without opening its image: a JPEG's, a PNG's, or an Opal Studio file's.

| Parameter | Kind | |
|---|---|---|
| `path` (required) | path | The file, by its absolute path. |

### `selection`

What is selected in an image: the rectangle round it, as [x, y, width, height] from the canvas's corner (null if nothing is; "unbounded" where it reaches past every edge, inverted), and the steps that made it, in order: shapes (rectangles and ellipses, by their middle, size and angle, and how each combines), inversions, grow steps (by, corners), feather steps (width, side, falloff), smooth steps (radius) and border steps (width, side), as grow_selection, feather_selection, smooth_selection and border_selection take them, and masks (a "mask" step is, as its of says, a layer's pixels, from select_layer_pixels; what the steps before it came to, carried pixel by pixel by Rotate, Flip or Resize Image, which could not carry them exactly; or a saved selection, combined by load_selection; it gives no numbers, only how it combines), and outlines (their start and segments, lines and curves, from select_outline or the outline tool). select_all, deselect, reselect, invert_selection and select_layer_pixels answer with it too.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |

### `measurement`

What is measured on an image (the Measure tool, or measure): start and end, from the canvas's corner, in pixels, as it is now (turned, flipped or resized with the image since); width and height (how far the end is from the start, across and down); length; angle (degrees counterclockwise from the right, as a protractor reads it); straightening (the turn, in degrees clockwise, that straighten makes: the least that makes the line level or plumb); and for a protractor, arm (its second arm's end), arm_length and between (the angle between the arms, 0 to 180). Null if nothing is measured.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |

### `saved_selections`

The selections saved in an image (save_selection), in the order they were first saved: each one's name, the rectangle round what it selects (as the selection query tells it; if the image has been turned, flipped, resized or cropped since it was saved, the rectangle round that rectangle carried so, which may be larger than what load_selection then selects), its steps as they were when it was saved, and whether the image has changed since (load_selection carries it so).

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |

### `documents`

The images open: for each, its id, title, file, canvas size and whether it has unsaved changes.

### `layers`

An image's layers, top first: for each, its id, name, kind, whether it is shown and locked, its opacity, whether it is selected, and the layers inside a group.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |

### `image`

What an image shows (or one of its layers alone), as a PNG, scaled down to fit a size (never enlarged): its bytes in base64, with its width and height.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layer` | layer | One layer alone, by id: as if shown, at full opacity. Default: the image as shown, all its visible layers together. |
| `max_side` | pixels | The most pixels it may be across or down. From 16 to 8192 pixels. Default: 1024. |

### `pixels`

The pixels of an image, as shown, or of one layer alone, within a rectangle of the canvas: in shared memory that the answer names (to be released once opened), or in the answer, in base64.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layer` | layer | One layer alone, by id: as if shown, at full opacity. Default: the image as shown, all its visible layers together. |
| `x` | pixels | The rectangle's left edge, in pixels from the canvas's. From -300000 to 600000 pixels. Default: 0. |
| `y` | pixels | The rectangle's top edge, in pixels from the canvas's. From -300000 to 600000 pixels. Default: 0. |
| `width` | pixels | The rectangle's width, in pixels. From 1 to 300000 pixels. Default: the canvas's. |
| `height` | pixels | The rectangle's height, in pixels. From 1 to 300000 pixels. Default: the canvas's. |
| `format` | `srgb8`, `srgb16`, `linear` | How each pixel is given, row by row from the top left, red, green, blue and alpha (straight, not premultiplied): srgb8 as bytes in sRGB; srgb16 as 16-bit values in sRGB, little-endian; linear as 32-bit floats in linear light, little-endian. Default: "srgb8". |
| `shared` | bool | In shared memory, which the answer names as shm, rather than in the answer (on Windows: elsewhere they always come in the answer). Default: true. |

### `release_pixels`

Lets go of the shared memory pixels gave, once it has been opened (it goes by itself a minute after).

| Parameter | Kind | |
|---|---|---|
| `shm` (required) | text | The shared memory, by the name pixels gave it. |

### `history`

An image's history, a tree: each state kept (its id, what made it, when, the state before it and those after, the one redo goes to), the state it is in, the one last saved, and how many states were dropped to keep within the memory the history may use.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |

### `preferences`

Every preference: for each, its key, label, section, help, value and default, and what it takes (its range or choices). set_preference changes them.

### `methods`

Every method of the API, with its parameters: what this list is generated from.

## The connection

### `begin_step`

Begins a step of history: what the calls until end_step change in each image becomes one state, called name, which one undo takes back. A step begun inside another is part of it. One left open ends when the connection does, or, on a connection that is no script run (an assistant's), before a call that comes 10 minutes or more after the last.

| Parameter | Kind | |
|---|---|---|
| `name` (required) | text | The step's name, as the History panel shows it. |

### `end_step`

Ends the step begin_step began (the outermost, if they are inside one another).
