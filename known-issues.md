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

## Images needing over 512 MiB once decoded do not open

`crates/opal-io/src/image_files.rs` (`import_bytes`). The `image` crate's readers default to
refusing to allocate more than 512 MiB, and import keeps that default, so a PNG or JPEG of
more than about 134 million pixels at 8 bits (an 11,600-pixel square), or half that at 16
bits, fails to open with "Limits exceeded" (and so does a paste of one copied as a file).
**Fix:** lift the decoder's limit, and refuse instead, with the reason, an image whose
decoding (the decoder's buffer, the conversion to the working format, the tiles) would
need more memory than is free.
