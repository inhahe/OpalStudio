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

## Zoomed out over a very large image object, the view makes every tile beneath it

`crates/opal-core/src/composite.rs` (`Plan::of`, `uniform_block`). Zoomed out, a block is
made at its level from one tile when its pixel objects repeat one tile, but never where an
image object reaches, whose pixels differ at every place: beneath one, the exact view
composites every level-0 tile, of 512 KB each, and reduces them. An image scaled across a
300,000-pixel canvas and shown whole is 1.4 million such tiles, some 700 GB: more than any
computer holds, so that view cannot be shown at all; across a 20,000-pixel canvas, 6,100
tiles and 3 GB (see `image_object_across_a_large_canvas` in `large_files.rs`). The exact
view is defined as full resolution reduced, so this is the cost of that definition.
**Fix, needing a decision:** let the exact view draw image objects at the level shown, from
the image's own pyramid with its own sampling, as previews already do (which differs
slightly from reducing full resolution); the plan would then skip those tiles and list
image objects by rectangle rather than every tile beneath. Built on the branch
`image-objects-at-zoom`.

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
