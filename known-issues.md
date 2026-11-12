# Known issues and technical debt

Each entry says where the problem lives and what the proper fix is. Fixed entries are
deleted (the commit that fixes one says so).

## Embedded color profiles are read as sRGB

`crates/opal-io/src/image_files.rs` (`import_bytes`). A PNG or JPEG with an embedded profile
other than sRGB (Adobe RGB, Display P3, a camera profile) is decoded as if it were sRGB, so
its colors are off; the user is warned when the image opens. Whether a profile is sRGB is
judged from its description text. **Fix:** color management with lcms2 in milestone 5:
convert from the embedded profile into the working space on import.

## Open, Place and Open as Layers read files on the interface thread

`crates/opal-app/src/state.rs` (`open_path`) and `paste.rs` (`place_file`,
`open_as_layers`): each reads and decodes its files before it returns, so the window stops
answering meanwhile, with nothing on screen saying why and no way to stop it. A large 16-bit
PNG takes seconds; Open as Layers with twenty camera JPEGs, longer. A paste is read in the
background already (`paste_answering`, `poll_paste`). **Fix:** read files the same way: on a
worker, the status bar saying which file is being read and that Esc cancels, the result
placed when it is ready (waiting, as a paste does, for a dialog, a drag or a box to be done),
on the image that was active; scripts answered when it is placed.

## GPU memory is only reported on Windows

`crates/opal-app/src/gpu_memory.rs`. **Fix:** query `VK_EXT_memory_budget` through wgpu's
Vulkan backend (and Metal's `recommendedMaxWorkingSetSize`) on other platforms.

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

## Pixels pass through shared memory only on Windows

`crates/opal-app/src/shared_memory.rs`. Elsewhere `pixels` and `put_pixels` carry the pixels
in base64 in the connection's lines, and a request's line is at most 1 MB, so a script can
give only small images there. Waits on a non-Windows build to test against. **Fix:** POSIX
shared memory (`shm_open` and `mmap`, through `rustix`), which Python reaches through
`multiprocessing.shared_memory`; the Python package then uses it there as on Windows.

## The scripting connection's Unix side is untried

`crates/opal-app/src/server.rs` (`read_first_line`, and the socket's permissions). The
non-Windows path of the hello deadline uses the socket's receive timeout and has never been
built; the Unix socket lives in the abstract namespace, which any local user can connect to
(the token still keeps them out). And `is_running` takes an editor that crashed, leaving
its socket file behind, for one running (a socket file would; the abstract namespace does
not leave one). Waits on a non-Windows build. **Fix:** build and test it there; consider a
socket file in the user's runtime folder (mode 0600) instead, connected to (not merely
found) to tell whether an editor is there.
