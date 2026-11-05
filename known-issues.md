# Known issues and technical debt

Each entry says where the problem lives and what the proper fix is. Fixed entries are
deleted (the commit that fixes one says so).

## Embedded color profiles are read as sRGB

`crates/opal-io/src/image_files.rs` (`import_bytes`). A PNG or JPEG with an embedded profile
other than sRGB (Adobe RGB, Display P3, a camera profile) is decoded as if it were sRGB, so
its colors are off; the user is warned when the image opens. Whether a profile is sRGB is
judged from its description text. **Fix:** color management with lcms2 in milestone 5:
convert from the embedded profile into the working space on import.

## The projection is computed on the UI thread

`crates/opal-core/src/composite.rs` (`Projection::update`), called from
`crates/opal-app/src/canvas.rs` every frame. Stale tiles are computed in parallel with
rayon, but the frame waits for them, so opening or zooming out of a very large image
(hundreds of megapixels) stalls the interface for a moment. **Fix:** compute stale tiles on
worker threads and show the coarser pyramid level (or the previous result) until they
arrive.

## The projection cache and GPU textures are never trimmed to a budget

`Projection` keeps every tile it has computed for content still in the document, and the
GPU texture cache (`crates/opal-app/src/gpu.rs`) frees textures only after 600 frames
unused. Fine for ordinary images; a huge document panned end to end keeps all of it in
memory. **Fix:** evict least recently used tiles beyond a memory budget, using the GPU
memory query for the texture side.

## History keeps every state

`crates/opal-core/src/history.rs`. Nothing is ever dropped, so a long session of large
edits grows without limit. The design's history memory budget is not implemented yet.
**Fix:** with milestone 3 (painting, the first source of large edits), count the tile
bytes each state alone holds and drop the oldest states beyond the budget set in the
preferences.

## Exporting and saving hold whole copies of the image in memory

`crates/opal-io/src/image_files.rs` (`export`) flattens the canvas into one buffer, converts
it to another, and hands that to the encoder; `native.rs` (`save`) shuffles every tile into
memory before writing, and compresses them one at a time. A very large canvas therefore
needs several times its size in memory to export, which is why the New Image dialog stops at
30,000 pixels a side. **Fix:** stream exports row by row into the PNG and JPEG encoders, and
compress tiles in parallel while writing them as they are produced.

## Panning a zoomed-out view recomputes the cache keys of every tile in view

`crates/opal-core/src/composite.rs` (`Projection::update`). An unchanged view of an unchanged
document costs nothing, but each frame of panning walks every tile of every layer and
rebuilds the key of every level-0 tile under the view, which for a huge canvas shown whole
is a lot of work per frame. **Fix:** keep the set of occupied tiles per layer up to date
incrementally, and skip key rebuilding for tiles whose layers did not change.

## GPU memory is only reported on Windows

`crates/opal-app/src/gpu_memory.rs`. **Fix:** query `VK_EXT_memory_budget` through wgpu's
Vulkan backend (and Metal's `recommendedMaxWorkingSetSize`) on other platforms.

## Building on Linux and macOS needs the target override removed

`.cargo/config.toml` sets the MSVC target, because the MinGW toolchain's test binary
crashed at startup. On other platforms that setting must be overridden
(`cargo build --target <host>`). **Fix:** when the first non-Windows build is set up, move
the override into a Windows-only build script or document per-platform `--config`.

## Dragging a large image object redraws it at full resolution

`crates/opal-app/src/transform_ui.rs` and `crates/opal-core/src/composite.rs`. The
projection is computed at full resolution and reduced from there, so while a transform box
is dragged over a large pasted image, every frame redraws all of its full-resolution tiles
under the view, even zoomed out. A 24-megapixel image takes about 25 ms to redraw (with the
faster method used while dragging) on a 16-core processor, so the drag keeps up there but
would lag on a slower one, or over many layers. **Fix:** while dragging, composite the
pyramid level on screen directly from each layer's reduced copy (as the design's
*Previews first* plans), and recompute full resolution when the drag ends.

## Copies to other programs are 8-bit

`crates/opal-app/src/clipboard.rs` (`write_image`). Copying puts the image on the system
clipboard as 8-bit PNG and bitmap, through `arboard`; pasting back into Opal Studio uses
the full-precision copy it keeps, but other programs get 8 bits per channel. **Fix:** also
write a 16-bit PNG under the registered "PNG" format with `clipboard-win`.

## Pasting decodes the clipboard on the interface thread

`crates/opal-app/src/paste.rs`. Reading and decoding a very large image from the clipboard
(a 50-megapixel PNG) holds up the interface for as long as that takes. **Fix:** read and
decode on a worker thread and open the transform box when the image is ready.
