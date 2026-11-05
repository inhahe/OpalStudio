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

## Copies to other programs are 8-bit on Linux and macOS

`crates/opal-app/src/clipboard.rs` (`write_system`). On Windows a copy goes on the clipboard
as a 16-bit PNG and a bitmap; elsewhere `arboard` writes only an 8-bit image, so another
program pasting a copy from Opal Studio gets 8 bits a channel (Opal Studio itself still gets
its full-precision copy back). Waits on a non-Windows build to test against. **Fix:** offer
`image/png` at 16 bits through the platform's clipboard directly (X11 and Wayland
selections, `NSPasteboard`'s `public.png`).
