# The editor's methods, for editors and type checkers. Made from its command
# registry by the test `api::tests::the_python_stubs_and_api_page_are_current`: run
# it with OPAL_WRITE_STUBS=1 to make this again. Do not edit by hand.

import contextlib
import os
from pathlib import Path
from typing import Any, Literal

from opal.expr import Expr as Expr, box as box, canvas as canvas, cm as cm, deg as deg, fn as fn, ft as ft, inch as inch, layer as layer, m as m, mm as mm, pc as pc, pi as pi, ppi as ppi, pt as pt, px as px, rad as rad, yd as yd

PROTOCOL: int

class OpalError(Exception):
    code: int | None
    def __init__(self, message: str, code: int | None = None) -> None: ...

def connection_file() -> Path: ...
def editor_program() -> str | None: ...
def connect(file: str | os.PathLike[str] | None = None, *, script: str | None = ..., windowless: bool | None = None) -> Editor: ...

class Editor:
    version: str | None
    def __init__(self, pipe: str, token: str, script: str | None = None) -> None: ...
    def call(self, method: str, **params: Any) -> Any: ...
    def methods(self) -> list[dict[str, Any]]: ...
    def step(self, name: str) -> contextlib.AbstractContextManager[Editor]: ...
    def pixels(self, document: int | None = None, layer: int | None = None, x: int = 0, y: int = 0, width: int | None = None, height: int | None = None, format: Literal["srgb8", "srgb16", "linear"] = "srgb8") -> Any: ...
    def put_pixels(self, pixels: Any, document: int | None = None, layer: int | None = None, x: int = 0, y: int = 0, name: str | None = None) -> int: ...
    def disconnect(self) -> None: ...
    def __enter__(self) -> Editor: ...
    def __exit__(self, *exc: Any) -> None: ...
    def new(self, *, width: float | str | Expr, height: float | str | Expr, resolution: float | str | Expr = 72, unit: Literal["px", "pt", "pc", "in", "ft", "yd", "mm", "cm", "m"] = "px", background: Literal["white", "transparent"] = "white") -> Any:
        """Create a new image.

        width: Its width. From 1.0 to 300000.0 pixels.
        height: Its height. From 1.0 to 300000.0 pixels.
        resolution: Its resolution, in pixels per inch: how large its pixels print, and what lengths in other units than pixels are converted at. From 0.01 to 1000000.0. Default: 72.
        unit: What the lengths given as plain numbers are in: pixels, points, picas, inches, feet, yards, millimeters, centimeters or meters, converted at the image's resolution. A length can also be given as text with a unit of its own: "8.5in", "21 cm". Default: "px".
        background: What it starts filled with. Default: "white".
        """
    def open(self, *, path: str | os.PathLike[str]) -> Any:
        """Open an image or an Opal Studio file.

        path: The file, by its absolute path: an Opal Studio file, a PNG or a JPEG.
        """
    def save(self, *, document: int | None = None) -> Any:
        """Save the image as an Opal Studio file.

        document: The image, by id. Default: the active image.
        """
    def save_as(self, *, document: int | None = None, path: str | os.PathLike[str], overwrite: bool = False) -> Any:
        """Save the image under a new name.

        document: The image, by id. Default: the active image.
        path: Where: an Opal Studio file (.opal is added unless it is there), by its absolute path.
        overwrite: Write over a file already there (otherwise that is refused). Default: false.
        """
    def export(self, *, document: int | None = None, path: str | os.PathLike[str], width: float | str | Expr | None = None, height: float | str | Expr | None = None, unit: Literal["px", "pt", "pc", "in", "ft", "yd", "mm", "cm", "m"] = "px", overwrite: bool = False, sixteen_bit: bool | None = None, quality: int | str | Expr | None = None) -> Any:
        """Save a flattened copy as PNG or JPEG.

        document: The image, by id. Default: the active image.
        path: Where: a .png, .jpg or .jpeg file (whose extension chooses the format), by its absolute path.
        width: The width written. From 1.0 to 300000.0 pixels. Default: the canvas's, or in proportion to the height given.
        height: The height written. From 1.0 to 300000.0 pixels. Default: the canvas's, or in proportion to the width given.
        unit: What the lengths given as plain numbers are in: pixels, points, picas, inches, feet, yards, millimeters, centimeters or meters, converted at the image's resolution. A length can also be given as text with a unit of its own: "8.5in", "21 cm". Default: "px".
        overwrite: Write over a file already there (otherwise that is refused). Default: false.
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
    def paste(self, *, document: int | None = None, x: float | str | Expr | None = None, y: float | str | Expr | None = None, width: float | str | Expr | None = None, height: float | str | Expr | None = None, angle: float | str | Expr | None = None, fit: Literal["stretch", "inside", "cover"] = "stretch", anchor: Literal["top-left", "top", "top-right", "left", "center", "right", "bottom-left", "bottom", "bottom-right"] = "top-left", from_layer: int | None = None, from_anchor: Literal["top-left", "top", "top-right", "left", "center", "right", "bottom-left", "bottom", "bottom-right"] = "top-left", into_active: bool = False, unit: Literal["px", "pt", "pc", "in", "ft", "yd", "mm", "cm", "m"] = "px") -> Any:
        """Paste the image on the clipboard, then place and size it.

        document: The image, by id. Default: the active image; with none open, the paste becomes a new image.
        x: Where the box's anchor point (its top-left corner, unless anchor says) goes, across from the point from_layer and from_anchor give (the canvas's top-left corner, unless they say). From -10000000.0 to 10000000.0 pixels. Default: where the command puts it.
        y: Where the box's anchor point (its top-left corner, unless anchor says) goes, down from the point from_layer and from_anchor give (the canvas's top-left corner, unless they say). From -10000000.0 to 10000000.0 pixels. Default: where the command puts it.
        width: The box's width. From 0.01 to 1000000.0 pixels. Default: in proportion to the height given, or as it is.
        height: The box's height. From 0.01 to 1000000.0 pixels. Default: in proportion to the width given, or as it is.
        angle: The box's angle, in degrees clockwise, about its anchor point. From -360.0 to 360.0 degrees. Default: as it is.
        fit: With both width and height given: stretch it to them (along its own sides, turned as it is); or keep its proportions and fit it, as it is turned, inside the rectangle they make on the canvas (placed by its anchor point at x and y), or make it cover that rectangle, centered on it. Default: "stretch".
        anchor: Which point of the box x and y give, and what a size or angle given scales and turns it about (a rectangle given with width and height is placed by its same point). Default: "top-left".
        from_layer: A layer x and y count from (from a point of the bounds of what it holds, as from_anchor says), rather than from the canvas. Held where it is when the call begins (a layer moved counts from where it was). Default: the canvas.
        from_anchor: Which point of the canvas (or of from_layer) x and y count from. Default: "top-left".
        into_active: Draw it into the pixels of the active layer (a plain, unlocked one) rather than onto a new layer. Default: false.
        unit: What the lengths given as plain numbers are in: pixels, points, picas, inches, feet, yards, millimeters, centimeters or meters, converted at the image's resolution. A length can also be given as text with a unit of its own: "8.5in", "21 cm". Default: "px".
        """
    def paste_in_place(self, *, document: int | None = None, x: float | str | Expr | None = None, y: float | str | Expr | None = None, width: float | str | Expr | None = None, height: float | str | Expr | None = None, angle: float | str | Expr | None = None, fit: Literal["stretch", "inside", "cover"] = "stretch", anchor: Literal["top-left", "top", "top-right", "left", "center", "right", "bottom-left", "bottom", "bottom-right"] = "top-left", from_layer: int | None = None, from_anchor: Literal["top-left", "top", "top-right", "left", "center", "right", "bottom-left", "bottom", "bottom-right"] = "top-left", into_active: bool = False, unit: Literal["px", "pt", "pc", "in", "ft", "yd", "mm", "cm", "m"] = "px") -> Any:
        """Paste where the copy came from.

        document: The image, by id. Default: the active image; with none open, the paste becomes a new image.
        x: Where the box's anchor point (its top-left corner, unless anchor says) goes, across from the point from_layer and from_anchor give (the canvas's top-left corner, unless they say). From -10000000.0 to 10000000.0 pixels. Default: where the command puts it.
        y: Where the box's anchor point (its top-left corner, unless anchor says) goes, down from the point from_layer and from_anchor give (the canvas's top-left corner, unless they say). From -10000000.0 to 10000000.0 pixels. Default: where the command puts it.
        width: The box's width. From 0.01 to 1000000.0 pixels. Default: in proportion to the height given, or as it is.
        height: The box's height. From 0.01 to 1000000.0 pixels. Default: in proportion to the width given, or as it is.
        angle: The box's angle, in degrees clockwise, about its anchor point. From -360.0 to 360.0 degrees. Default: as it is.
        fit: With both width and height given: stretch it to them (along its own sides, turned as it is); or keep its proportions and fit it, as it is turned, inside the rectangle they make on the canvas (placed by its anchor point at x and y), or make it cover that rectangle, centered on it. Default: "stretch".
        anchor: Which point of the box x and y give, and what a size or angle given scales and turns it about (a rectangle given with width and height is placed by its same point). Default: "top-left".
        from_layer: A layer x and y count from (from a point of the bounds of what it holds, as from_anchor says), rather than from the canvas. Held where it is when the call begins (a layer moved counts from where it was). Default: the canvas.
        from_anchor: Which point of the canvas (or of from_layer) x and y count from. Default: "top-left".
        into_active: Draw it into the pixels of the active layer (a plain, unlocked one) rather than onto a new layer. Default: false.
        unit: What the lengths given as plain numbers are in: pixels, points, picas, inches, feet, yards, millimeters, centimeters or meters, converted at the image's resolution. A length can also be given as text with a unit of its own: "8.5in", "21 cm". Default: "px".
        """
    def place(self, *, document: int | None = None, path: str | os.PathLike[str], name: str | None = None, print_size: bool | None = None, x: float | str | Expr | None = None, y: float | str | Expr | None = None, width: float | str | Expr | None = None, height: float | str | Expr | None = None, angle: float | str | Expr | None = None, fit: Literal["stretch", "inside", "cover"] = "stretch", anchor: Literal["top-left", "top", "top-right", "left", "center", "right", "bottom-left", "bottom", "bottom-right"] = "top-left", from_layer: int | None = None, from_anchor: Literal["top-left", "top", "top-right", "left", "center", "right", "bottom-left", "bottom", "bottom-right"] = "top-left", into_active: bool = False, unit: Literal["px", "pt", "pc", "in", "ft", "yd", "mm", "cm", "m"] = "px") -> Any:
        """Place an image file into this image, then position and size it.

        document: The image, by id. Default: the active image.
        path: The image file, a PNG or a JPEG, by its absolute path.
        name: The new layer's name. Default: the file's name.
        print_size: Scale the file by its resolution against the image's, so it prints as large as it did (before a width or height given), rather than place it pixel for pixel. A file that does not say its resolution is placed pixel for pixel either way. Default: the preference (Transform › Place at the file's print size).
        x: Where the box's anchor point (its top-left corner, unless anchor says) goes, across from the point from_layer and from_anchor give (the canvas's top-left corner, unless they say). From -10000000.0 to 10000000.0 pixels. Default: where the command puts it.
        y: Where the box's anchor point (its top-left corner, unless anchor says) goes, down from the point from_layer and from_anchor give (the canvas's top-left corner, unless they say). From -10000000.0 to 10000000.0 pixels. Default: where the command puts it.
        width: The box's width. From 0.01 to 1000000.0 pixels. Default: in proportion to the height given, or as it is.
        height: The box's height. From 0.01 to 1000000.0 pixels. Default: in proportion to the width given, or as it is.
        angle: The box's angle, in degrees clockwise, about its anchor point. From -360.0 to 360.0 degrees. Default: as it is.
        fit: With both width and height given: stretch it to them (along its own sides, turned as it is); or keep its proportions and fit it, as it is turned, inside the rectangle they make on the canvas (placed by its anchor point at x and y), or make it cover that rectangle, centered on it. Default: "stretch".
        anchor: Which point of the box x and y give, and what a size or angle given scales and turns it about (a rectangle given with width and height is placed by its same point). Default: "top-left".
        from_layer: A layer x and y count from (from a point of the bounds of what it holds, as from_anchor says), rather than from the canvas. Held where it is when the call begins (a layer moved counts from where it was). Default: the canvas.
        from_anchor: Which point of the canvas (or of from_layer) x and y count from. Default: "top-left".
        into_active: Draw it into the pixels of the active layer (a plain, unlocked one) rather than onto a new layer. Default: false.
        unit: What the lengths given as plain numbers are in: pixels, points, picas, inches, feet, yards, millimeters, centimeters or meters, converted at the image's resolution. A length can also be given as text with a unit of its own: "8.5in", "21 cm". Default: "px".
        """
    def transform(self, *, document: int | None = None, layers: list[int] | None = None, x: float | str | Expr | None = None, y: float | str | Expr | None = None, width: float | str | Expr | None = None, height: float | str | Expr | None = None, angle: float | str | Expr | None = None, fit: Literal["stretch", "inside", "cover"] = "stretch", anchor: Literal["top-left", "top", "top-right", "left", "center", "right", "bottom-left", "bottom", "bottom-right"] = "top-left", from_layer: int | None = None, from_anchor: Literal["top-left", "top", "top-right", "left", "center", "right", "bottom-left", "bottom", "bottom-right"] = "top-left", unit: Literal["px", "pt", "pc", "in", "ft", "yd", "mm", "cm", "m"] = "px") -> Any:
        """Move, scale and rotate the selected layers with a box.

        document: The image, by id. Default: the active image.
        layers: The layers, by id. Default: the selected layers.
        x: Where the box's anchor point (its top-left corner, unless anchor says) goes, across from the point from_layer and from_anchor give (the canvas's top-left corner, unless they say). From -10000000.0 to 10000000.0 pixels. Default: where the command puts it.
        y: Where the box's anchor point (its top-left corner, unless anchor says) goes, down from the point from_layer and from_anchor give (the canvas's top-left corner, unless they say). From -10000000.0 to 10000000.0 pixels. Default: where the command puts it.
        width: The box's width. From 0.01 to 1000000.0 pixels. Default: in proportion to the height given, or as it is.
        height: The box's height. From 0.01 to 1000000.0 pixels. Default: in proportion to the width given, or as it is.
        angle: The box's angle, in degrees clockwise, about its anchor point. From -360.0 to 360.0 degrees. Default: as it is.
        fit: With both width and height given: stretch it to them (along its own sides, turned as it is); or keep its proportions and fit it, as it is turned, inside the rectangle they make on the canvas (placed by its anchor point at x and y), or make it cover that rectangle, centered on it. Default: "stretch".
        anchor: Which point of the box x and y give, and what a size or angle given scales and turns it about (a rectangle given with width and height is placed by its same point). Default: "top-left".
        from_layer: A layer x and y count from (from a point of the bounds of what it holds, as from_anchor says), rather than from the canvas. Held where it is when the call begins (a layer moved counts from where it was). Default: the canvas.
        from_anchor: Which point of the canvas (or of from_layer) x and y count from. Default: "top-left".
        unit: What the lengths given as plain numbers are in: pixels, points, picas, inches, feet, yards, millimeters, centimeters or meters, converted at the image's resolution. A length can also be given as text with a unit of its own: "8.5in", "21 cm". Default: "px".
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
    def move_up(self, *, document: int | None = None, layer: int | None = None) -> Any:
        """Move the active layer one place up.

        document: The image, by id. Default: the active image.
        layer: The layer, by id. Default: the active layer.
        """
    def move_down(self, *, document: int | None = None, layer: int | None = None) -> Any:
        """Move the active layer one place down.

        document: The image, by id. Default: the active image.
        layer: The layer, by id. Default: the active layer.
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
    def resize_image(self, *, document: int | None = None, width: float | str | Expr | None = None, height: float | str | Expr | None = None, resolution: float | str | Expr | None = None, unit: Literal["px", "pt", "pc", "in", "ft", "yd", "mm", "cm", "m"] = "px", resample: bool | None = None) -> Any:
        """Change the image's size, scaling everything in it.

        document: The image, by id. Default: the active image.
        width: The new width. From 1.0 to 300000.0 pixels. Default: the canvas's, or in proportion to the height given.
        height: The new height. From 1.0 to 300000.0 pixels. Default: the canvas's, or in proportion to the width given.
        resolution: The new resolution, in pixels per inch, which lengths in other units than pixels are converted at. Given without a size, it changes no pixels: only how large they print. From 0.01 to 1000000.0. Default: the image's.
        unit: What the lengths given as plain numbers are in: pixels, points, picas, inches, feet, yards, millimeters, centimeters or meters, converted at the image's resolution. A length can also be given as text with a unit of its own: "8.5in", "21 cm". Default: "px".
        resample: Resample the pixel layers now (smaller, but enlarging again later cannot bring their detail back), rather than keep their pixels and draw them at the new size. Default: the preference.
        """
    def resize_canvas(self, *, document: int | None = None, width: float | str | Expr | None = None, height: float | str | Expr | None = None, anchor: Literal["top-left", "top", "top-right", "left", "center", "right", "bottom-left", "bottom", "bottom-right"] | None = None, left: float | str | Expr | None = None, top: float | str | Expr | None = None, right: float | str | Expr | None = None, bottom: float | str | Expr | None = None, unit: Literal["px", "pt", "pc", "in", "ft", "yd", "mm", "cm", "m"] = "px") -> Any:
        """Change the canvas's size without scaling what is on it.

        document: The image, by id. Default: the active image.
        width: The new width. From 1.0 to 300000.0 pixels. Default: the canvas's.
        height: The new height. From 1.0 to 300000.0 pixels. Default: the canvas's.
        anchor: Where the old canvas sits in the new. Default: the center.
        left: How much to add on that side, rather than give a new size (a negative length cuts it). From -300000.0 to 300000.0 pixels. Default: none: the width and height say.
        top: How much to add on that side, rather than give a new size (a negative length cuts it). From -300000.0 to 300000.0 pixels. Default: none: the width and height say.
        right: How much to add on that side, rather than give a new size (a negative length cuts it). From -300000.0 to 300000.0 pixels. Default: none: the width and height say.
        bottom: How much to add on that side, rather than give a new size (a negative length cuts it). From -300000.0 to 300000.0 pixels. Default: none: the width and height say.
        unit: What the lengths given as plain numbers are in: pixels, points, picas, inches, feet, yards, millimeters, centimeters or meters, converted at the image's resolution. A length can also be given as text with a unit of its own: "8.5in", "21 cm". Default: "px".
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
    def rotate_by(self, *, document: int | None = None, degrees: float | str | Expr, grow: bool = True) -> Any:
        """Turn the whole image by any angle.

        document: The image, by id. Default: the active image.
        degrees: The angle, clockwise. From -360.0 to 360.0 degrees.
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
    def image(self, *, document: int | None = None, layer: int | None = None, max_side: int | str | Expr = 1024) -> Any:
        """What an image shows (or one of its layers alone), as a PNG, scaled down to fit a size (never enlarged): its bytes in base64, with its width and height.

        document: The image, by id. Default: the active image.
        layer: One layer alone, by id: as if shown, at full opacity. Default: the image as shown, all its visible layers together.
        max_side: The most pixels it may be across or down. From 16 to 8192 pixels. Default: 1024.
        """
    def history(self, *, document: int | None = None) -> Any:
        """An image's history, a tree: each state kept (its id, what made it, when, the state before it and those after, the one redo goes to), the state it is in, the one last saved, and how many states were dropped to keep within the memory the history may use.

        document: The image, by id. Default: the active image.
        """
    def preferences(self) -> Any:
        """Every preference: for each, its key, label, section, help, value and default, and what it takes (its range or choices). set_preference changes them."""
    def crop(self, *, document: int | None = None, x: float | str | Expr | None = None, y: float | str | Expr | None = None, width: float | str | Expr | None = None, height: float | str | Expr | None = None, angle: float | str | Expr = 0, delete: bool | None = None, anchor: Literal["top-left", "top", "top-right", "left", "center", "right", "bottom-left", "bottom", "bottom-right"] = "top-left", from_layer: int | None = None, from_anchor: Literal["top-left", "top", "top-right", "left", "center", "right", "bottom-left", "bottom", "bottom-right"] = "top-left", unit: Literal["px", "pt", "pc", "in", "ft", "yd", "mm", "cm", "m"] = "px") -> Any:
        """Crops the image to a rectangle of its canvas, which may be turned: the canvas becomes the rectangle, and if it was turned, everything turns the other way, so its contents come out upright. Only the canvas moves (enlarging it again brings back what was cropped away), unless delete says.

        document: The image, by id. Default: the active image.
        x: Where the rectangle's anchor point is, across from the point from_layer and from_anchor give (the canvas's top-left corner, unless they say). From -10000000.0 to 10000000.0 pixels. Default: where that point of the canvas is.
        y: Where the rectangle's anchor point is, down from the point from_layer and from_anchor give (the canvas's top-left corner, unless they say). From -10000000.0 to 10000000.0 pixels. Default: where that point of the canvas is.
        width: The rectangle's width. From 1.0 to 300000.0 pixels. Default: the canvas's.
        height: The rectangle's height. From 1.0 to 300000.0 pixels. Default: the canvas's.
        angle: The rectangle's angle, in degrees clockwise, about its anchor point. From -360.0 to 360.0 degrees. Default: 0.
        delete: Remove the pixels outside the new canvas, rather than keep them for later. Default: the Crop tool's Delete cropped pixels.
        anchor: Which point of the rectangle x and y give, and what the angle turns it about. Default: "top-left".
        from_layer: A layer x and y count from (from a point of the bounds of what it holds, as from_anchor says), rather than from the canvas. Held where it is when the call begins (a layer moved counts from where it was). Default: the canvas.
        from_anchor: Which point of the canvas (or of from_layer) x and y count from. Default: "top-left".
        unit: What the lengths given as plain numbers are in: pixels, points, picas, inches, feet, yards, millimeters, centimeters or meters, converted at the image's resolution. A length can also be given as text with a unit of its own: "8.5in", "21 cm". Default: "px".
        """
    def set_preference(self, *, key: str, value: Any | None = None) -> Any:
        """Changes a preference, as the Preferences window would, or puts it back to its default. Those of scripting (what runs scripts, who may connect) are changed in the Preferences window only.

        key: The preference, by its key (the preferences query lists them).
        value: Its new value: true or false, a number in its range, one of its choices, a color as [red, green, blue] or [red, green, blue, alpha] (0 to 255, sRGB), or text. Default: its default.
        """
    def begin_step(self, *, name: str) -> Any:
        """Begins a step of history: what the calls until end_step change in each image becomes one state, called name, which one undo takes back. A step begun inside another is part of it. One left open ends when the connection does, or, on a connection that is no script run (an assistant's), before a call that comes 10 minutes or more after the last.

        name: The step's name, as the History panel shows it.
        """
    def end_step(self) -> Any:
        """Ends the step begin_step began (the outermost, if they are inside one another)."""
