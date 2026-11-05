# Known issues and technical debt

Each entry says where the problem lives and what the proper fix is. Fixed entries are
deleted (the commit that fixes one says so).

## Embedded color profiles are read as sRGB

`crates/opal-io/src/image_files.rs` (`import_bytes`). A PNG or JPEG with an embedded profile
other than sRGB (Adobe RGB, Display P3, a camera profile) is decoded as if it were sRGB, so
its colors are off; the user is warned when the image opens. Whether a profile is sRGB is
judged from its description text. **Fix:** color management with lcms2 in milestone 5:
convert from the embedded profile into the working space on import.

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

## Zoomed out on a very large canvas, the view keys every tile beneath it

`crates/opal-core/src/composite.rs` (`Projection::update`). A zoomed-out view is built
bottom-up: every level-0 tile under it gets a cache entry (its key, a reference to its
pixels), and each level above is keyed on the four below. On a 300,000-pixel white canvas
shown whole that is 1.37 million entries: about 800 MB, and a second after each edit to
rebuild the keys (the "Drawing…" sign shows meanwhile). Zoomed in, only the tiles in view
are keyed, as on any canvas. **Fix:** store large uniform areas as one entry (a sparse
quadtree of tiles in `RasterImage`, a uniform block standing for all the tiles it covers),
so that the view can key a block of alike tiles once at the level it is shown. A change
to how every layer stores its pixels, so it waits for a decision on that design.

## Programs built on the `image` crate cannot read the bitmap Opal Studio copies

`crates/opal-app/src/clipboard.rs` (`dib_v5`). A copy puts a `CF_DIBV5` on the clipboard:
a `BITMAPV5HEADER` with `BI_BITFIELDS`, the masks in the header, the pixels right after
it. The `image` crate's BMP decoder (0.25) takes a V4 or V5 header with `BI_BITFIELDS` to
be followed by three more mask DWORDs, as a `BITMAPINFOHEADER` is, so it reads 12 bytes
too far and fails ("failed to fill whole buffer"); `arboard`, which reads only
`CF_DIBV5` and decodes it so, fails on our bitmap, and so would any program reading it
that way and not our PNG. Microsoft's documents are ambiguous on whether the masks
repeat after a V5 header. **Fix:** find what Windows itself writes (`GetDIBits` with a V5
header, its `CF_DIB`→`CF_DIBV5` synthesis) and what common readers accept, and write the
form every one of them reads (perhaps `BI_RGB` with the alpha mask set, which has no
masks to place).
