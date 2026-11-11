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
    def cut(self, *, document: int | None = None, layers: list[int] | None = None) -> Any:
        """Copy what is selected on the selected layers, then erase it.

        document: The image, by id. Default: the active image.
        layers: The layers, by id. Default: the selected layers.
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
    def reapply_placement(self, *, document: int | None = None, layer: int | None = None) -> Any:
        """Place the active layer again as the expressions it was placed by say, now.

        document: The image, by id. Default: the active image.
        layer: The layer, by id. Default: the active layer.
        """
    def reapply_all_placements(self, *, document: int | None = None) -> Any:
        """Place every layer placed by expressions again, as they say now.

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
    def layer_via_copy(self, *, document: int | None = None, layers: list[int] | None = None) -> Any:
        """Put what is selected on the selected layers on a layer of its own (Ctrl+J).

        document: The image, by id. Default: the active image.
        layers: The layers, by id. Default: the selected layers.
        """
    def layer_via_cut(self, *, document: int | None = None, layers: list[int] | None = None) -> Any:
        """Move what is selected on the selected layers onto a layer of its own.

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
    def select_all(self, *, document: int | None = None) -> Any:
        """Select the whole canvas.

        document: The image, by id. Default: the active image.
        """
    def deselect(self, *, document: int | None = None) -> Any:
        """Select nothing.

        document: The image, by id. Default: the active image.
        """
    def reselect(self, *, document: int | None = None) -> Any:
        """Select again what was selected when you last deselected.

        document: The image, by id. Default: the active image.
        """
    def invert_selection(self, *, document: int | None = None) -> Any:
        """Select what is not selected, and nothing of what is.

        document: The image, by id. Default: the active image.
        """
    def select_layer_pixels(self, *, document: int | None = None, layer: int | None = None, combine: Literal["replace", "add", "subtract", "intersect"] = "replace") -> Any:
        """Select the active layer's pixels, each as much as it is opaque.

        document: The image, by id. Default: the active image.
        layer: The layer, by id. Default: the active layer.
        combine: How it combines with what is selected: in its place, added to it, taken away from it, or keeping only what is in both. Default: "replace".
        """
    def clear(self, *, document: int | None = None, layers: list[int] | None = None) -> Any:
        """Erase what is selected on the selected layers.

        document: The image, by id. Default: the active image.
        layers: The layers, by id. Default: the selected layers.
        """
    def crop_to_selection(self, *, document: int | None = None) -> Any:
        """Make the canvas the rectangle round the selection.

        document: The image, by id. Default: the active image.
        """
    def grow_selection(self, *, document: int | None = None, by: float | str | Expr, corners: Literal["round", "square", "diamond"] = "round", unit: Literal["px", "pt", "pc", "in", "ft", "yd", "mm", "cm", "m"] = "px") -> Any:
        """Grow the selection by a distance, or shrink it.

        document: The image, by id. Default: the active image.
        by: How far the outline moves out; less than 0, in. From -1000.0 to 1000.0 pixels.
        corners: How its corners grow: round, square, or to a point as a diamond's. Default: "round".
        unit: What the lengths given as plain numbers are in: pixels, points, picas, inches, feet, yards, millimeters, centimeters or meters, converted at the image's resolution. A length can also be given as text with a unit of its own: "8.5in", "21 cm". Default: "px".
        """
    def feather_selection(self, *, document: int | None = None, width: float | str | Expr, side: Literal["inward", "outward", "centered"] = "centered", falloff: Literal["gaussian", "linear"] = "gaussian", unit: Literal["px", "pt", "pc", "in", "ft", "yd", "mm", "cm", "m"] = "px") -> Any:
        """Soften the selection's edge over a width.

        document: The image, by id. Default: the active image.
        width: How wide the falloff from fully selected to not at all is. From 0.0 to 1000.0 pixels.
        side: Where the falloff lies: inside the outline, outside it, or half each way. Default: "centered".
        falloff: Its shape: as a Gaussian blur's edge, or straight. Default: "gaussian".
        unit: What the lengths given as plain numbers are in: pixels, points, picas, inches, feet, yards, millimeters, centimeters or meters, converted at the image's resolution. A length can also be given as text with a unit of its own: "8.5in", "21 cm". Default: "px".
        """
    def smooth_selection(self, *, document: int | None = None, radius: float | str | Expr, unit: Literal["px", "pt", "pc", "in", "ft", "yd", "mm", "cm", "m"] = "px") -> Any:
        """Smooth the selection's outline over a radius.

        document: The image, by id. Default: the active image.
        radius: Pinholes, notches, specks and spurs narrower than twice it go, and corners round to it. From 0.0 to 1000.0 pixels.
        unit: What the lengths given as plain numbers are in: pixels, points, picas, inches, feet, yards, millimeters, centimeters or meters, converted at the image's resolution. A length can also be given as text with a unit of its own: "8.5in", "21 cm". Default: "px".
        """
    def border_selection(self, *, document: int | None = None, width: float | str | Expr, side: Literal["inward", "outward", "centered"] = "centered", unit: Literal["px", "pt", "pc", "in", "ft", "yd", "mm", "cm", "m"] = "px") -> Any:
        """Select a band along the selection's outline.

        document: The image, by id. Default: the active image.
        width: How wide the band along the outline is. From 0.0 to 1000.0 pixels.
        side: Where the band lies: inside the outline, outside it, or half each way. Default: "centered".
        unit: What the lengths given as plain numbers are in: pixels, points, picas, inches, feet, yards, millimeters, centimeters or meters, converted at the image's resolution. A length can also be given as text with a unit of its own: "8.5in", "21 cm". Default: "px".
        """
    def transform_selection(self, *, document: int | None = None, x: float | str | Expr | None = None, y: float | str | Expr | None = None, width: float | str | Expr | None = None, height: float | str | Expr | None = None, angle: float | str | Expr | None = None, fit: Literal["stretch", "inside", "cover"] = "stretch", anchor: Literal["top-left", "top", "top-right", "left", "center", "right", "bottom-left", "bottom", "bottom-right"] = "top-left", from_layer: int | None = None, from_anchor: Literal["top-left", "top", "top-right", "left", "center", "right", "bottom-left", "bottom", "bottom-right"] = "top-left", unit: Literal["px", "pt", "pc", "in", "ft", "yd", "mm", "cm", "m"] = "px") -> Any:
        """Move, size or turn the selection itself in a box.

        document: The image, by id. Default: the active image.
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
    def save_selection(self, *, document: int | None = None, name: str) -> Any:
        """Keep what is selected under a name, to load back.

        document: The image, by id. Default: the active image.
        name: The name it is saved under: a selection saved under it already (whatever the case of its letters) is replaced.
        """
    def load_selection(self, *, document: int | None = None, name: str, combine: Literal["replace", "add", "subtract", "intersect"] = "replace", invert: bool = False) -> Any:
        """Select a saved selection again, or combine it with what is selected.

        document: The image, by id. Default: the active image.
        name: The selection saved in the image under this name (whatever the case of its letters).
        combine: How it combines with what is selected: in its place, added to it, taken away from it, or keeping only what is in both. Default: "replace".
        invert: Invert it first: what it does not select, before it combines. Default: false.
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
    def selection(self, *, document: int | None = None) -> Any:
        """What is selected in an image: the rectangle round it, as [x, y, width, height] from the canvas's corner (null if nothing is; "unbounded" where it reaches past every edge, inverted), and the steps that made it, in order: shapes (rectangles and ellipses, by their middle, size and angle, and how each combines), inversions, grow steps (by, corners), feather steps (width, side, falloff), smooth steps (radius) and border steps (width, side), as grow_selection, feather_selection, smooth_selection and border_selection take them, and masks (a "mask" step is, as its of says, a layer's pixels, from select_layer_pixels, or what the steps before it came to, carried pixel by pixel by Rotate, Flip or Resize Image, which could not carry them exactly; it gives no numbers, only how it combines). select_all, deselect, reselect, invert_selection and select_layer_pixels answer with it too.

        document: The image, by id. Default: the active image.
        """
    def saved_selections(self, *, document: int | None = None) -> Any:
        """The selections saved in an image (save_selection), in the order they were first saved: each one's name, the rectangle round what it selects (as the selection query tells it, carried as the image has been turned, flipped, resized or cropped since it was saved), its steps as they were when it was saved, and whether the image has changed since (load_selection carries it so).

        document: The image, by id. Default: the active image.
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
    def select_outline(self, *, document: int | None = None, points: list[tuple[float, float]], combine: Literal["replace", "add", "subtract", "intersect"] = "replace", smooth: bool | None = None) -> Any:
        """Selects what an outline encloses, as the outline tool does: its points joined by straight sides, the last back to the first (a loop crossing itself selects each of its lobes). Answers with what is selected then, as the selection query does.

        document: The image, by id. Default: the active image.
        points: The outline's points, in order, each [x, y] in pixels from the canvas's top-left corner.
        combine: How it combines with what is selected: in its place, added to it, taken away from it, or keeping only what is in both. Default: "replace".
        smooth: Select a pixel the outline crosses as much as it encloses it; else all or nothing, by the pixel's middle. Default: the outline tool's Smooth edges.
        """
    def delete_saved_selection(self, *, document: int | None = None, name: str) -> Any:
        """Deletes a selection saved in the image (as the Selection panel does), by its name. Answers with the selections saved then, as saved_selections tells them.

        document: The image, by id. Default: the active image.
        name: The selection saved in the image under this name (whatever the case of its letters).
        """
    def remove_selection_step(self, *, document: int | None = None, index: int | str | Expr) -> Any:
        """Takes a step away from the recipe of what is selected, by its number among the selection query's steps; the selection is worked out again from the rest (as the selection panel does). Answers with what is selected then.

        document: The image, by id. Default: the active image.
        index: Which step: its number among the selection query's steps, from 0. From 0 to 1048576.
        """
    def edit_selection_step(self, *, document: int | None = None, index: int | str | Expr, combine: Literal["replace", "add", "subtract", "intersect"] | None = None, by: float | str | Expr | None = None, corners: Literal["round", "square", "diamond"] | None = None, width: float | str | Expr | None = None, side: Literal["inward", "outward", "centered"] | None = None, falloff: Literal["gaussian", "linear"] | None = None, radius: float | str | Expr | None = None, unit: Literal["px", "pt", "pc", "in", "ft", "yd", "mm", "cm", "m"] = "px") -> Any:
        """Changes a step of the recipe of what is selected, by its number among the selection query's steps: a shape's or a mask's combine; a grow's by and corners; a feather's width, side and falloff; a smooth's radius; a border's width and side (what is not given stays as it is; what the step does not take is refused). The selection is worked out again with it changed (as the selection panel does). Answers with what is selected then.

        document: The image, by id. Default: the active image.
        index: Which step: its number among the selection query's steps, from 0. From 0 to 1048576.
        combine: A shape's or a mask's: how it combines with the steps before it. Default: as it is.
        by: A grow's: how far it grows the selection (shrinks it, less than 0). From -1000.0 to 1000.0 pixels. Default: as it is.
        corners: A grow's: how its corners grow. Default: as it is.
        width: A feather's or a border's: how wide its falloff or its band is. From 0.0 to 1000.0 pixels. Default: as it is.
        side: A feather's or a border's: on which side of the outline it lies. Default: as it is.
        falloff: A feather's: the shape of its falloff. Default: as it is.
        radius: A smooth's: the radius it smooths over. From 0.0 to 1000.0 pixels. Default: as it is.
        unit: What the lengths given as plain numbers are in: pixels, points, picas, inches, feet, yards, millimeters, centimeters or meters, converted at the image's resolution. A length can also be given as text with a unit of its own: "8.5in", "21 cm". Default: "px".
        """
    def select_rectangle(self, *, document: int | None = None, x: float | str | Expr | None = None, y: float | str | Expr | None = None, width: float | str | Expr | None = None, height: float | str | Expr | None = None, angle: float | str | Expr = 0, combine: Literal["replace", "add", "subtract", "intersect"] = "replace", smooth: bool | None = None, anchor: Literal["top-left", "top", "top-right", "left", "center", "right", "bottom-left", "bottom", "bottom-right"] = "top-left", from_layer: int | None = None, from_anchor: Literal["top-left", "top", "top-right", "left", "center", "right", "bottom-left", "bottom", "bottom-right"] = "top-left", unit: Literal["px", "pt", "pc", "in", "ft", "yd", "mm", "cm", "m"] = "px") -> Any:
        """Selects a rectangle, which may be turned, as the Select tool's box numbers give it: the canvas, unless the numbers say otherwise. Answers with what is selected then, as the selection query does.

        document: The image, by id. Default: the active image.
        x: Where the shape's anchor point is (of the rectangle round an ellipse), across from the point from_layer and from_anchor give (the canvas's top-left corner, unless they say). From -10000000.0 to 10000000.0 pixels. Default: where that point of the canvas is.
        y: Where the shape's anchor point is, down from the point from_layer and from_anchor give (the canvas's top-left corner, unless they say). From -10000000.0 to 10000000.0 pixels. Default: where that point of the canvas is.
        width: The shape's width (an ellipse's across). From 0.01 to 10000000.0 pixels. Default: the canvas's.
        height: The shape's height. From 0.01 to 10000000.0 pixels. Default: the canvas's.
        angle: How far the shape is turned, in degrees clockwise, about its anchor point. From -360.0 to 360.0 degrees. Default: 0.
        combine: How it combines with what is selected: in its place, added to it, taken away from it, or keeping only what is in both. Default: "replace".
        smooth: Select a pixel its edge crosses as much as the shape covers it; else all or nothing, by the pixel's middle. Default: the Select tool's Smooth edges.
        anchor: Which point of the shape (of the rectangle round an ellipse) x and y give, and what the angle turns it about. Default: "top-left".
        from_layer: A layer x and y count from (from a point of the bounds of what it holds, as from_anchor says), rather than from the canvas. Held where it is when the call begins (a layer moved counts from where it was). Default: the canvas.
        from_anchor: Which point of the canvas (or of from_layer) x and y count from. Default: "top-left".
        unit: What the lengths given as plain numbers are in: pixels, points, picas, inches, feet, yards, millimeters, centimeters or meters, converted at the image's resolution. A length can also be given as text with a unit of its own: "8.5in", "21 cm". Default: "px".
        """
    def select_ellipse(self, *, document: int | None = None, x: float | str | Expr | None = None, y: float | str | Expr | None = None, width: float | str | Expr | None = None, height: float | str | Expr | None = None, angle: float | str | Expr = 0, combine: Literal["replace", "add", "subtract", "intersect"] = "replace", smooth: bool | None = None, anchor: Literal["top-left", "top", "top-right", "left", "center", "right", "bottom-left", "bottom", "bottom-right"] = "top-left", from_layer: int | None = None, from_anchor: Literal["top-left", "top", "top-right", "left", "center", "right", "bottom-left", "bottom", "bottom-right"] = "top-left", unit: Literal["px", "pt", "pc", "in", "ft", "yd", "mm", "cm", "m"] = "px") -> Any:
        """Selects an ellipse, given by the rectangle round it (which may be turned), as the Select tool's box numbers give it: the one within the canvas, unless the numbers say otherwise. Answers with what is selected then, as the selection query does.

        document: The image, by id. Default: the active image.
        x: Where the shape's anchor point is (of the rectangle round an ellipse), across from the point from_layer and from_anchor give (the canvas's top-left corner, unless they say). From -10000000.0 to 10000000.0 pixels. Default: where that point of the canvas is.
        y: Where the shape's anchor point is, down from the point from_layer and from_anchor give (the canvas's top-left corner, unless they say). From -10000000.0 to 10000000.0 pixels. Default: where that point of the canvas is.
        width: The shape's width (an ellipse's across). From 0.01 to 10000000.0 pixels. Default: the canvas's.
        height: The shape's height. From 0.01 to 10000000.0 pixels. Default: the canvas's.
        angle: How far the shape is turned, in degrees clockwise, about its anchor point. From -360.0 to 360.0 degrees. Default: 0.
        combine: How it combines with what is selected: in its place, added to it, taken away from it, or keeping only what is in both. Default: "replace".
        smooth: Select a pixel its edge crosses as much as the shape covers it; else all or nothing, by the pixel's middle. Default: the Select tool's Smooth edges.
        anchor: Which point of the shape (of the rectangle round an ellipse) x and y give, and what the angle turns it about. Default: "top-left".
        from_layer: A layer x and y count from (from a point of the bounds of what it holds, as from_anchor says), rather than from the canvas. Held where it is when the call begins (a layer moved counts from where it was). Default: the canvas.
        from_anchor: Which point of the canvas (or of from_layer) x and y count from. Default: "top-left".
        unit: What the lengths given as plain numbers are in: pixels, points, picas, inches, feet, yards, millimeters, centimeters or meters, converted at the image's resolution. A length can also be given as text with a unit of its own: "8.5in", "21 cm". Default: "px".
        """
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
