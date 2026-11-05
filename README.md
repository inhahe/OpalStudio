# Opal Studio

A 2D graphics editor that aims to be more intuitive than the usual ones: everything stays
editable, every setting that affects a result can be adjusted, and the interface always
says what a command does and, when it cannot run, why.

Early days: this is milestone 0 of the roadmap, the foundation. You can open and create
images, arrange layers, undo through a branching history, and save and export; painting
and selection tools come next. See `roadmap.md` for what is planned and `design.md` for how
it is built. Known problems are listed in `known-issues.md`.

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
  Studio's own `.opal` files, which keep every layer. Save as `.opal`; export the canvas
  to PNG (8 or 16 bits) or JPEG. Saving never damages the existing file if it fails.
  Drop files on the window to open them; several images open as tabs.
- **Layers and groups.** Add, duplicate, delete, reorder (buttons or dragging, into and
  out of groups), rename, hide, lock, change opacity, merge down, flatten. Layers have no
  edges: content past the canvas is kept and shown dimmed.
- **History.** Undo and redo never discard anything: doing something new after undoing
  starts a branch, and the History panel goes to any state on any branch.
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

## Layout

| Crate | What it holds |
|---|---|
| `crates/opal-color` | Color models and conversions (a port of the author's colorpicker, plus OKLab). |
| `crates/opal-core` | The document: tiles, layers, the history tree, compositing, curves. |
| `crates/opal-io` | PNG and JPEG import and export, and the `.opal` format. |
| `crates/opal-app` | The application: interface, canvas rendering, settings. |
