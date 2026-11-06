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

## Drawn at a level, Nearest softens a stretch along an axis that another axis shrinks

`crates/opal-core/src/composite.rs` (`at_level`). Where the view draws an image object at
the level shown (for its cost, or during a drag), an image drawn with Nearest that is
shrunk there is drawn with Bilinear from its pyramid, which averages as reducing full
resolution would. Shrunk along one axis and enlarged along the other (scaled 8 × 0.25, at
a level), that also softens the enlarged pixels, which reducing would keep hard; point
sampling would instead drop whole rows of the shrunk ones. **Fix:** draw Nearest at a
level by area: each pixel the average of the source pixels its footprint covers, which is
point sampling where enlarged and averaging where shrunk, as reducing full resolution is.

## An edit to a huge image finds its content bounds again over every tile

`crates/opal-core/src/raster.rs` (`content_bounds`). Changing a pixel forgets the image's
content bounds, and the next asker finds them again from every tile: 8 ms for a
300,000-pixel canvas (1.4 million tiles), on every edit, which painting will make on every
stroke. **Fix:** find them from the runs of tiles (each row's first and last tile, at a
lookup a row), asking only the tiles along the edge for their content.

## The scratch drive's free space is only known on Windows

`crates/opal-app/src/memory.rs` (`disk_free`). Elsewhere it is not known, and none is
counted: tiles still go to disk past the budget, but work is admitted only as far as
memory holds it, so a document larger than memory cannot be opened there. **Fix:**
`statvfs` on the temporary folder on Linux and macOS (through `libc`, or `rustix`), once a
non-Windows build exists to test it on.

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
