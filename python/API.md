# The Opal Studio API

Every method a script or assistant can call (see `design.md`, *Scripting*), made from the
editor's command registry by the test `api::tests::the_python_stubs_and_api_page_are_current`:
run it with `OPAL_WRITE_STUBS=1` to make this page again; do not edit it by hand. From Python,
each is a method of the editor (`editor.new(width=640, height=480)`), taking its parameters by
name; over the pipe, a JSON-RPC method of the same name.

## Commands

### `new`

Create a new image.

| Parameter | Kind | |
|---|---|---|
| `width` (required) | int | Its width, in pixels. From 1 to 300000. |
| `height` (required) | int | Its height, in pixels. From 1 to 300000. |
| `background` | `white`, `transparent` | What it starts filled with. Default: "white". |

### `open`

Open an image or an Opal Studio file.

| Parameter | Kind | |
|---|---|---|
| `path` (required) | path | The file: an Opal Studio file, a PNG or a JPEG. |

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
| `path` (required) | path | Where: an Opal Studio file (.opal is added if it has no extension). |

### `export`

Save a flattened copy as PNG or JPEG.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `path` (required) | path | Where: a .png, .jpg or .jpeg file, whose extension chooses the format. |
| `width` | int | The width written, in pixels. From 1 to 300000. Default: the canvas's. |
| `height` | int | The height written, in pixels. From 1 to 300000. Default: the canvas's. |
| `sixteen_bit` | bool | For a PNG: 16 bits a channel rather than 8. Default: the preference. |
| `quality` | int | For a JPEG: its quality. From 1 to 100. Default: the preference. |

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
| `layers` | layers | The layers, by id. Default: the selected layers. |

### `move_down`

Move the active layer one place down.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `layers` | layers | The layers, by id. Default: the selected layers. |

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

### `crop_to_content`

Shrink (or grow) the canvas to fit what the visible layers hold.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |

### `resize_image`

Change the image's size, scaling everything in it.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `width` (required) | int | The new width, in pixels. From 1 to 300000. |
| `height` (required) | int | The new height, in pixels. From 1 to 300000. |
| `resample` | bool | Resample the pixel layers now (smaller, but enlarging again later cannot bring their detail back), rather than keep their pixels and draw them at the new size. Default: the preference. |

### `resize_canvas`

Change the canvas's size without scaling what is on it.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |
| `width` | int | The new width, in pixels. From 1 to 300000. Default: the canvas's. |
| `height` | int | The new height, in pixels. From 1 to 300000. Default: the canvas's. |
| `anchor` | `top-left`, `top`, `top-right`, `left`, `center`, `right`, `bottom-left`, `bottom`, `bottom-right` | Where the old canvas sits in the new. Default: "center". |

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
| `degrees` (required) | number | The angle, clockwise. From -360.0 to 360.0. |
| `grow` | bool | Grow the canvas so that nothing is cut off. Default: true. |

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

## Queries

### `documents`

The images open: for each, its id, title, file, canvas size and whether it has unsaved changes.

### `layers`

An image's layers, top first: for each, its id, name, kind, whether it is shown and locked, its opacity, whether it is selected, and the layers inside a group.

| Parameter | Kind | |
|---|---|---|
| `document` | document | The image, by id. Default: the active image. |

### `methods`

Every method of the API, with its parameters: what this list is generated from.
