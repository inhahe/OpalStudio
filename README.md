# Opal Studio

A 2D graphics editor that aims to be more intuitive than the usual ones: everything stays
editable, every setting that affects a result can be adjusted, and the interface always
says what a command does and, when it cannot run, why.

Early days: milestones 0 (the foundation) and 1 (paste and place) of the roadmap are
built. You can open and create images, paste and place images, move, scale, rotate, crop
and resize, arrange and merge layers, undo through a branching history, and save and
export; scripting, selections and painting come next. See `roadmap.md` for what is planned
and `design.md` for how it is built. Known problems are listed in `known-issues.md`.

Free software under the GNU General Public License, version 3 or later (`LICENSE`).

## Building and running

Needs Rust 1.95 or later. On Windows, the Visual Studio C++ build tools as well: the
project builds for the MSVC target (`.cargo/config.toml`).

```
cargo run -p opal-app --release            # start Opal Studio
cargo run -p opal-app --release -- a.png   # and open files
cargo test --workspace                     # all tests, interface tests included
```

## What works so far

- **Files.** Open PNG and JPEG (8 and 16 bits; EXIF orientation honored) and Opal
  Studio's own `.opal` files, which keep every layer. An 8-bit image takes half the
  memory until it is changed: its pixels are kept as they were read. Images larger than
  memory can be worked on, saved and opened again: past a share of it (a preference), or
  past what is free while something needing memory is done, the parts not looked at
  lately go to a scratch file on disk. Save as `.opal`;
  export the canvas to PNG (8 or 16 bits) or JPEG, at its own size or another. Saving and
  exporting run in the background, so the window stays usable; the status bar shows how far
  they have got and can cancel them. A save that fails or is cancelled never damages the
  existing file. Drop files on the window to open them; several images open as tabs.
- **Paste and place.** Ctrl+V pastes an image (from Opal Studio, another program, or an
  image file copied in Explorer) into a box that moves, scales and rotates it, snapping to
  the canvas and other layers; Enter places it. A pasted image keeps its own pixels, so it
  can be resized again later without losing quality. Paste in Place, Copy and Copy Merged;
  pasting with nothing open makes a new image. Copies keep their full precision when pasted
  back, and on Windows reach other programs as 16-bit PNG. A large image is read in the
  background (Esc cancels it): anything started meanwhile (a dialog, a drag, another
  box) is finished first.
- **Moving and transforming.** The Move tool drags the selected layers (clicking picks the
  layer under the pointer; right-click lists every layer there); the arrow keys nudge;
  Transform (Ctrl+T) scales and rotates layers, losslessly.
- **The image.** The Crop tool, with aspect ratios, guides and straightening; Crop to
  Content; Resize Image and Resize Canvas; quarter turns, flips and rotation by any angle.
  Cropping and resizing the canvas never delete anything unless asked to. Canvases go up
  to 300,000 pixels a side; anything that would need more memory than the computer has
  free (counting room on disk for the parts that can go there) is refused, with how much
  it needs, rather than run out of it.
- **Layers and groups.** Add, duplicate, delete, group, reorder (buttons or dragging,
  into and out of groups), rename, hide, lock, change opacity; select several with Ctrl
  and Shift; merge down, merge selected, merge visible, merge a group, stamp visible,
  flatten; rasterize; thumbnails. Layers have no edges: content past the canvas is kept and
  shown dimmed.
- **History.** Doing something new after undoing starts a branch instead of discarding the
  redo states, and the History panel goes to any state on any branch. History may use a
  set amount of memory (a preference); past it, the oldest states and abandoned branches
  are dropped, never the newest work.
- **Viewing.** Zoom with the wheel, the Zoom tool, pinching or the zoom commands; scroll
  with the Hand tool, Space-drag or the middle button. The image is drawn on the GPU in
  linear light, smoothly when zoomed out and as sharp pixels when zoomed in.
- **The interface.**
  - A ribbon of commands whose groups shrink to fit the window, with tooltips that say
    why a disabled command cannot run.
  - Alt shows a letter on every tab and button; Ctrl+K searches for any command; F1
    opens help for the button under the pointer.
  - Tools are pinned to the ribbon or a side strip.
  - Every preference shows its default, marks changes and can be reset, and tool
    settings can be saved as named presets.
  - Dark and light color schemes.
- **Scripting.** Python scripts can drive the running editor through the `opal` package in
  `python/` (put it on `PYTHONPATH`, or `pip install ./python`):

  ```python
  import opal
  with opal.connect() as editor:
      editor.new(width=1920, height=1080)
      editor.new_layer(name="Sky")
      editor.export(path="sky.png")
  ```

  Most commands can be called this way, with their settings as named parameters, as can
  the queries `documents`, `layers` and `image` (what an image looks like, as a PNG);
  `python/API.md` lists them all. What a script changes in an image becomes one history
  state, named after it, when it ends, so one undo takes it back (from an interactive
  Python session, or with `connect(script=None)`, each call is a step of its own). Only
  programs of the user's own can connect (through a token in the settings folder), and the
  status bar says when one is connected, with a button to disconnect it (which keeps
  scripts out until you let them in again there).
  Run Script (File › Scripts) runs one from the editor, with the Python set in
  Preferences › Scripting (a virtual environment's, say); the Console panel shows what it
  prints and can stop it or run it again.
- **Batch work.** `opal-studio --headless script.py [arguments]` runs a script against an
  editor with no window (the Python is `--python PATH`, or `OPAL_PYTHON`, or the one set in
  the preferences), and ends with the script's exit code once the files it asked for are
  written.
- **AI assistants.** `opal-studio --mcp` is an MCP server for assistants whose clients
  launch their servers, its tools the same commands and queries, the image coming back as
  an image so the assistant sees what it is doing. For Claude Code, for example:
  `claude mcp add opal-studio -- "C:\path\to\opal-studio.exe" --mcp`. The editor itself
  must be running for a tool to act.

## Layout

| Crate | What it holds |
|---|---|
| `crates/opal-color` | Color models and conversions (a port of the author's colorpicker, plus OKLab). |
| `crates/opal-core` | The document: tiles, layers, the history tree, compositing, curves. |
| `crates/opal-io` | PNG and JPEG import and export, and the `.opal` format. |
| `crates/opal-app` | The application: interface, canvas rendering, settings, the scripting API. |
| `python` | The `opal` package for scripts, and the API's reference (`API.md`). |
