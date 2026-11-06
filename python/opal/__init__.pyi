# The editor's methods, for editors and type checkers. Made from its command
# registry by the test `api::tests::the_python_stubs_and_api_page_are_current`: run
# it with OPAL_WRITE_STUBS=1 to make this again. Do not edit by hand.

import os
from pathlib import Path
from typing import Any, Literal

PROTOCOL: int

class OpalError(Exception):
    code: int | None
    def __init__(self, message: str, code: int | None = None) -> None: ...

def connection_file() -> Path: ...
def connect(file: str | os.PathLike[str] | None = None) -> Editor: ...

class Editor:
    version: str | None
    def __init__(self, pipe: str, token: str) -> None: ...
    def call(self, method: str, **params: Any) -> Any: ...
    def methods(self) -> list[dict[str, Any]]: ...
    def close(self) -> None: ...
    def __enter__(self) -> Editor: ...
    def __exit__(self, *exc: Any) -> None: ...
    def new(self, *, width: int, height: int, background: Literal["white", "transparent"] = "white") -> Any:
        """Create a new image.

        width: Its width, in pixels. From 1 to 300000.
        height: Its height, in pixels. From 1 to 300000.
        background: What it starts filled with. Default: "white".
        """
    def open(self, *, path: str | os.PathLike[str]) -> Any:
        """Open an image or an Opal Studio file.

        path: The file: an Opal Studio file, a PNG or a JPEG.
        """
    def save(self, *, document: int | None = None) -> Any:
        """Save the image as an Opal Studio file.

        document: The image, by id. Default: the active image.
        """
    def save_as(self, *, document: int | None = None, path: str | os.PathLike[str]) -> Any:
        """Save the image under a new name.

        document: The image, by id. Default: the active image.
        path: Where: an Opal Studio file (.opal is added if it has no extension).
        """
    def export(self, *, document: int | None = None, path: str | os.PathLike[str], width: int | None = None, height: int | None = None, sixteen_bit: bool | None = None, quality: int | None = None) -> Any:
        """Save a flattened copy as PNG or JPEG.

        document: The image, by id. Default: the active image.
        path: Where: a .png, .jpg or .jpeg file, whose extension chooses the format.
        width: The width written, in pixels. From 1 to 300000. Default: the canvas's.
        height: The height written, in pixels. From 1 to 300000. Default: the canvas's.
        sixteen_bit: For a PNG: 16 bits a channel rather than 8. Default: the preference.
        quality: For a JPEG: its quality. From 1 to 100. Default: the preference.
        """
    def close(self, *, document: int | None = None, discard: bool = False) -> Any:
        """Close the image.

        document: The image, by id. Default: the active image.
        discard: Close it even with unsaved changes, losing them. Default: false.
        """
    def undo(self, *, document: int | None = None) -> Any:
        """Take back the last action.

        document: The image, by id. Default: the active image.
        """
    def redo(self, *, document: int | None = None) -> Any:
        """Repeat the action you undid.

        document: The image, by id. Default: the active image.
        """
    def copy(self, *, document: int | None = None) -> Any:
        """Copy the selected layers, as they look, to the clipboard.

        document: The image, by id. Default: the active image.
        """
    def copy_merged(self, *, document: int | None = None) -> Any:
        """Copy everything visible within the canvas to the clipboard.

        document: The image, by id. Default: the active image.
        """
    def new_layer(self, *, document: int | None = None, name: str | None = None) -> Any:
        """Add an empty layer above the active one.

        document: The image, by id. Default: the active image.
        name: Its name. Default: a numbered one.
        """
    def new_group(self, *, document: int | None = None, name: str | None = None) -> Any:
        """Add a group to hold layers.

        document: The image, by id. Default: the active image.
        name: Its name. Default: a numbered one.
        """
    def duplicate(self, *, document: int | None = None, layers: list[int] | None = None) -> Any:
        """Copy the selected layers.

        document: The image, by id. Default: the active image.
        layers: The layers, by id. Default: the selected layers.
        """
    def group_layers(self, *, document: int | None = None, layers: list[int] | None = None) -> Any:
        """Put the selected layers into a new group.

        document: The image, by id. Default: the active image.
        layers: The layers, by id. Default: the selected layers.
        """
    def delete_layer(self, *, document: int | None = None, layers: list[int] | None = None) -> Any:
        """Remove the selected layers.

        document: The image, by id. Default: the active image.
        layers: The layers, by id. Default: the selected layers.
        """
    def move_up(self, *, document: int | None = None, layers: list[int] | None = None) -> Any:
        """Move the active layer one place up.

        document: The image, by id. Default: the active image.
        layers: The layers, by id. Default: the selected layers.
        """
    def move_down(self, *, document: int | None = None, layers: list[int] | None = None) -> Any:
        """Move the active layer one place down.

        document: The image, by id. Default: the active image.
        layers: The layers, by id. Default: the selected layers.
        """
    def merge_down(self, *, document: int | None = None, layer: int | None = None) -> Any:
        """Combine the active layer with the layer below it.

        document: The image, by id. Default: the active image.
        layer: The layer, by id. Default: the active layer.
        """
    def merge_selected(self, *, document: int | None = None, layers: list[int] | None = None) -> Any:
        """Combine the selected layers into one.

        document: The image, by id. Default: the active image.
        layers: The layers, by id. Default: the selected layers.
        """
    def merge_visible(self, *, document: int | None = None) -> Any:
        """Combine every visible layer into one.

        document: The image, by id. Default: the active image.
        """
    def merge_group(self, *, document: int | None = None, layer: int | None = None) -> Any:
        """Turn the active group into one pixel layer.

        document: The image, by id. Default: the active image.
        layer: The layer, by id. Default: the active layer.
        """
    def stamp_visible(self, *, document: int | None = None) -> Any:
        """Add a layer holding everything visible, combined.

        document: The image, by id. Default: the active image.
        """
    def flatten(self, *, document: int | None = None) -> Any:
        """Combine all visible layers into one.

        document: The image, by id. Default: the active image.
        """
    def rasterize(self, *, document: int | None = None, layers: list[int] | None = None) -> Any:
        """Turn the selected layers' images into plain pixels.

        document: The image, by id. Default: the active image.
        layers: The layers, by id. Default: the selected layers.
        """
    def crop_to_content(self, *, document: int | None = None) -> Any:
        """Shrink (or grow) the canvas to fit what the visible layers hold.

        document: The image, by id. Default: the active image.
        """
    def resize_image(self, *, document: int | None = None, width: int, height: int, resample: bool | None = None) -> Any:
        """Change the image's size, scaling everything in it.

        document: The image, by id. Default: the active image.
        width: The new width, in pixels. From 1 to 300000.
        height: The new height, in pixels. From 1 to 300000.
        resample: Resample the pixel layers now (smaller, but enlarging again later cannot bring their detail back), rather than keep their pixels and draw them at the new size. Default: the preference.
        """
    def resize_canvas(self, *, document: int | None = None, width: int | None = None, height: int | None = None, anchor: Literal["top-left", "top", "top-right", "left", "center", "right", "bottom-left", "bottom", "bottom-right"] = "center") -> Any:
        """Change the canvas's size without scaling what is on it.

        document: The image, by id. Default: the active image.
        width: The new width, in pixels. From 1 to 300000. Default: the canvas's.
        height: The new height, in pixels. From 1 to 300000. Default: the canvas's.
        anchor: Where the old canvas sits in the new. Default: "center".
        """
    def rotate_cw(self, *, document: int | None = None) -> Any:
        """Turn the whole image a quarter turn clockwise.

        document: The image, by id. Default: the active image.
        """
    def rotate_ccw(self, *, document: int | None = None) -> Any:
        """Turn the whole image a quarter turn counterclockwise.

        document: The image, by id. Default: the active image.
        """
    def rotate180(self, *, document: int | None = None) -> Any:
        """Turn the whole image upside down.

        document: The image, by id. Default: the active image.
        """
    def rotate_by(self, *, document: int | None = None, degrees: float, grow: bool = True) -> Any:
        """Turn the whole image by any angle.

        document: The image, by id. Default: the active image.
        degrees: The angle, clockwise. From -360.0 to 360.0.
        grow: Grow the canvas so that nothing is cut off. Default: true.
        """
    def flip_horizontal(self, *, document: int | None = None) -> Any:
        """Mirror the whole image left to right.

        document: The image, by id. Default: the active image.
        """
    def flip_vertical(self, *, document: int | None = None) -> Any:
        """Mirror the whole image top to bottom.

        document: The image, by id. Default: the active image.
        """
    def toggle_visible(self, *, document: int | None = None, layers: list[int] | None = None, visible: bool | None = None) -> Any:
        """Show or hide the active layer.

        document: The image, by id. Default: the active image.
        layers: The layers, by id. Default: the selected layers.
        visible: Shown, or hidden. Default: each the opposite of what it is.
        """
    def toggle_lock(self, *, document: int | None = None, layers: list[int] | None = None, locked: bool | None = None) -> Any:
        """Protect the active layer from changes.

        document: The image, by id. Default: the active image.
        layers: The layers, by id. Default: the selected layers.
        locked: Locked, or not. Default: each the opposite of what it is.
        """
    def rename(self, *, document: int | None = None, layer: int | None = None, name: str) -> Any:
        """Give the active layer a new name.

        document: The image, by id. Default: the active image.
        layer: The layer, by id. Default: the active layer.
        name: Its new name.
        """
    def documents(self) -> Any:
        """The images open: for each, its id, title, file, canvas size and whether it has unsaved changes."""
    def layers(self, *, document: int | None = None) -> Any:
        """An image's layers, top first: for each, its id, name, kind, whether it is shown and locked, its opacity, whether it is selected, and the layers inside a group.

        document: The image, by id. Default: the active image.
        """
    def image(self, *, document: int | None = None, max_side: int = 1024) -> Any:
        """What an image shows, as a PNG, scaled down to fit a size (never enlarged): its bytes in base64, with its width and height.

        document: The image, by id. Default: the active image.
        max_side: The most pixels it may be across or down. From 16 to 8192. Default: 1024.
        """
