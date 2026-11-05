//! Images on the system clipboard.
//!
//! Reading tries, in order: a copy made by this application, if it is still what the
//! clipboard holds (it keeps full precision and where it was copied from); PNG (which keeps
//! 16-bit color and transparency); image files copied in a file manager; the system's own
//! bitmap format; JPEG. Writing puts the system bitmap (8-bit) and, on Windows, a 16-bit
//! PNG on the clipboard. Both run off the interface thread: encoding or decoding a large
//! image takes a while.

use crate::background::JobThread;
use eframe::egui;
use opal_core::geom::IRect;
use opal_core::raster::RasterImage;
use opal_core::tile::Pixel;
use opal_io::image_files;
use opal_io::pixels::pixels_from_rgba8;
#[cfg(not(windows))]
use opal_io::pixels::pixels_to_rgba8;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

/// An image from the clipboard, with its top-left corner at the origin.
#[derive(Clone, Debug)]
pub struct Pasted {
    pub width: u32,
    pub height: u32,
    pub image: RasterImage,
    /// For a copy made in Opal Studio: the document pixels it was copied from.
    pub from: Option<IRect>,
    /// Things the user should know about how it was read (an unsupported color profile).
    pub warnings: Vec<String>,
}

/// An image to put on the clipboard: `pixels` row by row, copied from `from` in the
/// document.
pub struct Copied {
    pub from: IRect,
    pub pixels: Vec<Pixel>,
}

impl Copied {
    /// The copy as it reads back in this application.
    fn pasted(&self) -> Pasted {
        let (w, h) = (self.from.width() as u32, self.from.height() as u32);
        let image = RasterImage::from_pixels(w, h, &self.pixels);
        Pasted { width: w, height: h, image, from: Some(self.from), warnings: Vec::new() }
    }
}

/// The memory a copy takes per pixel while it is put on the clipboard: the pixels (8
/// bytes), the 16-bit PNG and its encoding (up to 16), the bitmap (4), the system's copies
/// of both (up to 12), and the copy kept for pasting back here (8).
pub const COPY_BYTES: u64 = 48;

/// Why `w × h` pixels cannot go on the system clipboard, if they cannot: on Windows, its
/// bitmaps (the format every program reads) hold at most 4 GB, about a billion pixels.
pub fn check_copy(w: u64, h: u64) -> Result<(), String> {
    #[cfg(windows)]
    if 4 * w * h > u64::from(u32::MAX) {
        return Err(format!(
            "{w} × {h} pixels is more than the clipboard can hold: its bitmaps hold at most 4 GB, about a \
             billion pixels."
        ));
    }
    let _ = (w, h);
    Ok(())
}

/// The image on the clipboard, `None` if it holds none, or why it could not be read.
pub type ReadResult = Result<Option<Pasted>, String>;

/// How putting a copy on the clipboard went: where it was copied from, or why it failed.
pub type WriteResult = Result<IRect, String>;

/// Reading the clipboard: done already, or work to run off the interface thread (reading
/// and decoding a large image can take a while).
pub enum Read {
    Ready(Box<ReadResult>),
    Later(Box<dyn FnOnce() -> ReadResult + Send>),
}

pub trait Clipboard {
    /// Reads the clipboard's image (after any copy still being written).
    fn read_image(&mut self) -> Read;
    /// Starts putting an image on the clipboard. Copies are written in the order asked;
    /// `ctx` is asked to repaint when one is done, for [`written`](Self::written).
    fn write_image(&mut self, copy: Copied, ctx: &egui::Context);
    /// How the copies finished since the last call went, in order.
    fn written(&mut self) -> Vec<WriteResult>;
    /// Whether the clipboard holds text (to explain why there is nothing to paste).
    fn has_text(&mut self) -> bool;
}

/// The last copy made by this application, kept at full precision.
#[derive(Clone)]
#[cfg_attr(test, allow(dead_code, reason = "tests use TestClipboard"))]
struct OwnCopy {
    /// The clipboard's sequence number once the copy was on it: while it is unchanged, the
    /// clipboard still holds this copy.
    #[cfg_attr(not(windows), allow(dead_code))]
    sequence: Option<u32>,
    pasted: Pasted,
}

#[cfg_attr(test, allow(dead_code, reason = "tests use TestClipboard"))]
impl OwnCopy {
    /// Whether the clipboard still holds this copy.
    #[cfg(windows)]
    fn held(&self) -> bool {
        let now = clipboard_win::raw::seq_num().map(|n| n.get());
        now.is_some() && now == self.sequence
    }

    /// Whether `img`, which the clipboard holds, is this copy. (Without a sequence number,
    /// the image is compared.)
    #[cfg(not(windows))]
    fn is(&self, img: &arboard::ImageData) -> bool {
        let p = &self.pasted;
        img.width == p.width as usize
            && img.height == p.height as usize
            && *img.bytes == pixels_to_rgba8(&p.image.read_rect(&IRect::from_size(img.width as i64, img.height as i64)))
    }
}

/// What the writer thread and the readers share.
#[derive(Default)]
struct Shared {
    state: Mutex<Writes>,
    /// Signalled when a write finishes.
    finished: Condvar,
}

#[derive(Default)]
struct Writes {
    own: Option<OwnCopy>,
    /// Copies asked for and not yet written.
    pending: usize,
    /// How the finished ones went, not yet reported.
    done: Vec<WriteResult>,
}

/// How long reading waits for a copy being written, and closing for the copies.
const WAIT_FOR_WRITES: Duration = Duration::from_secs(10);

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Writes> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The state once no copy is being written (or, if one takes too long, as it is).
    fn when_written(&self) -> MutexGuard<'_, Writes> {
        let s = self.lock();
        match self.finished.wait_timeout_while(s, WAIT_FOR_WRITES, |s| s.pending > 0) {
            Ok((s, _)) => s,
            Err(e) => e.into_inner().0,
        }
    }
}

/// The operating system's clipboard.
#[cfg_attr(test, allow(dead_code, reason = "tests use TestClipboard"))]
pub struct SystemClipboard {
    shared: Arc<Shared>,
    /// Writes copies to the clipboard, one after another.
    writer: JobThread,
}

impl Default for SystemClipboard {
    fn default() -> SystemClipboard {
        SystemClipboard { shared: Arc::default(), writer: JobThread::new("clipboard writer") }
    }
}

impl Drop for SystemClipboard {
    /// Waits for the copies still being written, so that one made just before quitting is
    /// on the clipboard.
    fn drop(&mut self) {
        drop(self.shared.when_written());
    }
}

impl Clipboard for SystemClipboard {
    fn read_image(&mut self) -> Read {
        #[cfg(windows)]
        {
            let s = self.shared.lock();
            if s.pending == 0
                && let Some(own) = s.own.as_ref().filter(|o| o.held())
            {
                return Read::Ready(Box::new(Ok(Some(own.pasted.clone()))));
            }
        }
        let shared = self.shared.clone();
        Read::Later(Box::new(move || {
            let own = shared.when_written().own.clone();
            read_system(own)
        }))
    }

    fn write_image(&mut self, copy: Copied, ctx: &egui::Context) {
        let from = copy.from;
        self.shared.lock().pending += 1;
        // Everything slow happens before the lock is taken (the interface takes it every
        // frame); a panic still counts the copy as done.
        let work = move || write_system(&copy).map(|sequence| OwnCopy { sequence, pasted: copy.pasted() });
        let shared = self.shared.clone();
        let done = move |result: std::thread::Result<Result<OwnCopy, String>>| {
            let mut s = shared.lock();
            match result.unwrap_or_else(|_| Err("copying failed unexpectedly".into())) {
                Ok(own) => {
                    s.own = Some(own);
                    s.done.push(Ok(from));
                }
                Err(e) => s.done.push(Err(e)),
            }
            s.pending -= 1;
            drop(s);
            shared.finished.notify_all();
        };
        self.writer.run(work, done, ctx);
    }

    fn written(&mut self) -> Vec<WriteResult> {
        std::mem::take(&mut self.shared.lock().done)
    }

    fn has_text(&mut self) -> bool {
        arboard::Clipboard::new().and_then(|mut c| c.get_text()).is_ok_and(|t| !t.trim().is_empty())
    }
}

#[cfg_attr(test, allow(dead_code, reason = "tests use TestClipboard"))]
fn from_import(i: image_files::Imported) -> Pasted {
    Pasted { width: i.width, height: i.height, image: i.image, from: None, warnings: i.warnings }
}

/// The memory pasting the bitmap on the clipboard would take (see [`bitmap_memory`]), if it
/// holds one.
#[cfg(windows)]
#[cfg_attr(test, allow(dead_code, reason = "tests use TestClipboard"))]
fn clipboard_bitmap_memory() -> Option<u64> {
    use clipboard_win::{formats, raw};
    let _open = open_clipboard().ok()?;
    let size = raw::size(formats::CF_DIB)?.get() as u64;
    let mut header = [0u8; 16];
    raw::get(formats::CF_DIB, &mut header).ok()?;
    bitmap_memory(&header, size)
}

/// The memory pasting a bitmap of `size` bytes, whose header (a `BITMAPINFOHEADER`) begins
/// `header`, takes as it is read: the bitmap, the image at 8 bits a channel (4 bytes a
/// pixel), the working pixels (8) and their tiles (8).
#[cfg(any(windows, test))]
fn bitmap_memory(header: &[u8], size: u64) -> Option<u64> {
    let int = |at: usize| Some(i32::from_le_bytes(header.get(at..at + 4)?.try_into().ok()?));
    let pixels = u64::from(int(4)?.unsigned_abs()) * u64::from(int(8)?.unsigned_abs());
    Some(size.saturating_add(pixels.saturating_mul(20)))
}

/// What reading an image the clipboard offers gave: the image, `None` if it was not one
/// that can be read (so that what else is offered is tried), or an error if it is one but
/// would need more memory than is free (so that the paste says why).
#[cfg_attr(test, allow(dead_code, reason = "tests use TestClipboard"))]
fn imported(read: Result<image_files::Imported, opal_io::IoError>) -> Result<Option<Pasted>, String> {
    match read {
        Ok(i) => Ok(Some(from_import(i))),
        Err(e @ opal_io::IoError::TooLarge { .. }) => Err(e.to_string()),
        Err(_) => Ok(None),
    }
}

/// Reads and decodes what the system clipboard holds (any thread may), which is `own` if
/// the clipboard still holds this application's last copy.
#[cfg_attr(test, allow(dead_code, reason = "tests use TestClipboard"))]
fn read_system(own: Option<OwnCopy>) -> ReadResult {
    #[cfg(windows)]
    {
        if let Some(own) = own.filter(OwnCopy::held) {
            return Ok(Some(own.pasted));
        }
        if let Some(p) = read_windows_formats()? {
            return Ok(Some(p));
        }
    }
    #[cfg(windows)]
    if let (Some(need), Some(free)) = (clipboard_bitmap_memory(), crate::memory::free())
        && need > free
    {
        return Err(format!(
            "pasting the image on the clipboard would need about {} of memory, but only {} is free",
            opal_io::bytes(need),
            opal_io::bytes(free)
        ));
    }
    let mut cb = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    #[cfg(not(windows))]
    if let Ok(files) = cb.get().file_list() {
        for f in files {
            if let Some(p) = imported(image_files::import(&f, crate::memory::free()))? {
                return Ok(Some(p));
            }
        }
    }
    match cb.get_image() {
        Ok(img) => {
            #[cfg(not(windows))]
            if let Some(own) = own.filter(|o| o.is(&img)) {
                return Ok(Some(own.pasted));
            }
            let (w, h) = (img.width as u32, img.height as u32);
            let image = RasterImage::from_pixels(w, h, &pixels_from_rgba8(&img.bytes));
            Ok(Some(Pasted { width: w, height: h, image, from: None, warnings: Vec::new() }))
        }
        Err(arboard::Error::ContentNotAvailable) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Puts `copy` on the system clipboard (any thread may). Returns the clipboard's sequence
/// number once it is there, where the system has one.
#[cfg_attr(test, allow(dead_code, reason = "tests use TestClipboard"))]
fn write_system(copy: &Copied) -> Result<Option<u32>, String> {
    let (w, h) = (copy.from.width() as u32, copy.from.height() as u32);
    #[cfg(windows)]
    {
        use clipboard_win::{formats, raw};
        let png = image_files::encode_png16(&copy.pixels, w, h);
        let dib = dib_v5(&copy.pixels, w, h);
        {
            let _open = open_clipboard()?;
            raw::empty().map_err(|e| format!("the clipboard could not be emptied ({e})"))?;
            // PNG first, since some programs take the first image format offered. It is
            // what programs that read PNG (Photoshop, GIMP, Krita) get, at 16 bits a
            // channel; the bitmap, which every program reads, is the one that must be there.
            if let Some(f) = raw::register_format("PNG") {
                let _ = raw::set_without_clear(f.get(), &png);
            }
            raw::set_without_clear(formats::CF_DIBV5, &dib).map_err(|e| e.to_string())?;
        }
        Ok(raw::seq_num().map(|n| n.get()))
    }
    #[cfg(not(windows))]
    {
        let bytes = pixels_to_rgba8(&copy.pixels);
        let mut cb = arboard::Clipboard::new().map_err(|e| e.to_string())?;
        let data = arboard::ImageData { width: w as usize, height: h as usize, bytes: bytes.into() };
        cb.set_image(data).map_err(|e| e.to_string())?;
        Ok(None)
    }
}

/// Opens the clipboard, waiting up to half a second for another program to let go of it
/// (clipboard managers and remote desktops open it briefly whenever it changes).
#[cfg(windows)]
#[cfg_attr(test, allow(dead_code, reason = "tests use TestClipboard"))]
fn open_clipboard() -> Result<clipboard_win::Clipboard, String> {
    let mut tries = 100;
    loop {
        match clipboard_win::Clipboard::new() {
            Ok(c) => return Ok(c),
            Err(e) if tries == 0 => return Err(format!("the clipboard is busy ({e})")),
            Err(_) => tries -= 1,
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// `pixels` (`w × h`, row by row) as the clipboard's 32-bit bitmap with transparency
/// (`CF_DIBV5`): the header, then the rows bottom up, each pixel blue, green, red, alpha,
/// in sRGB with straight alpha. (Bottom up because some programs, Word among them, cannot
/// paste a bitmap stored top down.)
#[cfg(any(windows, test))]
fn dib_v5(pixels: &[Pixel], w: u32, h: u32) -> Vec<u8> {
    const HEADER: u32 = 124;
    const BI_BITFIELDS: u32 = 3;
    const LCS_SRGB: u32 = 0x7352_4742;
    const LCS_GM_IMAGES: u32 = 4;
    fn u32s(out: &mut Vec<u8>, vs: &[u32]) {
        vs.iter().for_each(|v| out.extend(v.to_le_bytes()));
    }
    let mut out = Vec::with_capacity(HEADER as usize + 4 * pixels.len());
    u32s(&mut out, &[HEADER, w, h]);
    out.extend(1u16.to_le_bytes()); // planes
    out.extend(32u16.to_le_bytes()); // bits per pixel
    let size = u32::try_from(4 * u64::from(w) * u64::from(h)).expect("a copy the clipboard can hold (check_copy)");
    u32s(&mut out, &[BI_BITFIELDS, size, 0, 0, 0, 0]);
    u32s(&mut out, &[0x00ff_0000, 0x0000_ff00, 0x0000_00ff, 0xff00_0000, LCS_SRGB]);
    u32s(&mut out, &[0; 9]); // endpoints, unused with sRGB
    u32s(&mut out, &[0; 3]); // gamma, likewise
    u32s(&mut out, &[LCS_GM_IMAGES, 0, 0, 0]);
    debug_assert_eq!(out.len(), HEADER as usize);
    for row in pixels.chunks_exact(w.max(1) as usize).rev() {
        for &p in row {
            let [r, g, b, a] = opal_io::pixels::to_srgb8(p);
            out.extend([b, g, r, a]);
        }
    }
    out
}

/// The formats `arboard` does not read, or reads only at 8 bits: PNG and JPEG as files
/// would hold them, and copied files. All are fetched in one opening of the clipboard, so
/// they come from one state of it, and decoded once it is closed again (other programs are
/// not kept from it meanwhile). Only one large one is fetched: the first PNG offered, or,
/// with no PNG to be had and no bitmap, the JPEG.
#[cfg(windows)]
#[cfg_attr(test, allow(dead_code, reason = "tests use TestClipboard"))]
fn read_windows_formats() -> Result<Option<Pasted>, String> {
    use clipboard_win::{formats, raw};
    let (png, files, jpeg) = {
        let _open = open_clipboard()?;
        let offered = |name: &str| raw::register_format(name).map(|f| f.get()).filter(|&f| raw::is_format_avail(f));
        let bytes = |f: u32| {
            let mut bytes = Vec::new();
            raw::get_vec(f, &mut bytes).is_ok().then_some(bytes)
        };
        let png = offered("PNG").or_else(|| offered("image/png")).and_then(bytes);
        let mut files = Vec::new();
        if raw::is_format_avail(formats::CF_HDROP) {
            let _ = raw::get_file_list_path(&mut files);
        }
        // The system bitmap (what most programs put on the clipboard) is read by arboard;
        // JPEG is the last resort, since a program offering it offers a bitmap too.
        let bitmap = raw::is_format_avail(formats::CF_DIB) || raw::is_format_avail(formats::CF_DIBV5);
        let jpeg = if png.is_none() && !bitmap { offered("JFIF").and_then(bytes) } else { None };
        (png, files, jpeg)
    };
    let decoded = |bytes: Vec<u8>, as_file: &str| {
        imported(image_files::import_bytes(&bytes, std::path::Path::new(as_file), crate::memory::free()))
    };
    if let Some(b) = png
        && let Some(p) = decoded(b, "clipboard.png")?
    {
        return Ok(Some(p));
    }
    for f in files {
        if let Some(p) = imported(image_files::import(&f, crate::memory::free()))? {
            return Ok(Some(p));
        }
    }
    jpeg.map_or(Ok(None), |b| decoded(b, "clipboard.jpg"))
}

/// A clipboard for tests: holds what was last written, or what a test put there. With
/// `slow`, reading it is done off the interface thread, as the system clipboard's is.
#[cfg(test)]
#[derive(Default)]
pub struct TestClipboard {
    pub image: Option<Pasted>,
    pub text: bool,
    pub slow: bool,
    pub done: Vec<WriteResult>,
}

#[cfg(test)]
impl Clipboard for TestClipboard {
    fn read_image(&mut self) -> Read {
        let image = self.image.clone();
        if self.slow { Read::Later(Box::new(move || Ok(image))) } else { Read::Ready(Box::new(Ok(image))) }
    }

    fn write_image(&mut self, copy: Copied, _ctx: &egui::Context) {
        self.image = Some(copy.pasted());
        self.done.push(Ok(copy.from));
    }

    fn written(&mut self) -> Vec<WriteResult> {
        std::mem::take(&mut self.done)
    }

    fn has_text(&mut self) -> bool {
        self.text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opal_core::tile::pixel_from_f32;

    #[test]
    fn a_bitmap_s_memory_is_read_from_its_header() {
        // 40-byte header; 30,000 wide, 20,000 tall (stored top-down, so negative).
        let mut header = vec![0u8; 16];
        header[0..4].copy_from_slice(&40u32.to_le_bytes());
        header[4..8].copy_from_slice(&30_000i32.to_le_bytes());
        header[8..12].copy_from_slice(&(-20_000i32).to_le_bytes());
        let size = 40 + 4 * 30_000 * 20_000;
        assert_eq!(bitmap_memory(&header, size), Some(size + 20 * 600_000_000));
        assert_eq!(bitmap_memory(&header[..6], size), None, "a header cut short");
    }

    #[test]
    fn a_clipboard_image_too_large_to_open_is_said_not_passed_over() {
        let path = std::path::PathBuf::from("clipboard.png");
        let too_large = opal_io::IoError::TooLarge { path: path.clone(), need: 2_000_000_000, free: 1_000_000_000 };
        let why = imported(Err(too_large)).unwrap_err();
        assert!(why.contains("2.0 GB") && why.contains("1.0 GB"), "{why}");
        let unreadable = opal_io::IoError::Decode { path, message: "not an image".into() };
        assert!(matches!(imported(Err(unreadable)), Ok(None)), "what else is offered is not tried");
    }

    #[test]
    #[cfg(windows)]
    fn copies_too_large_for_a_bitmap_are_refused() {
        assert!(check_copy(32_767, 32_768).is_ok());
        let why = check_copy(32_768, 32_768).unwrap_err();
        assert!(why.contains("4 GB"), "{why}");
    }

    #[test]
    fn the_bitmap_is_stored_bottom_up_in_blue_green_red_alpha() {
        let red = pixel_from_f32([1.0, 0.0, 0.0, 1.0]);
        let clear_blue = pixel_from_f32([0.0, 0.0, 0.5, 0.5]);
        let dib = dib_v5(&[red, red, clear_blue, clear_blue], 2, 2);
        assert_eq!(dib.len(), 124 + 16);
        let u32_at = |i: usize| u32::from_le_bytes(dib[i..i + 4].try_into().unwrap());
        assert_eq!((u32_at(0), u32_at(4), u32_at(8)), (124, 2, 2));
        assert_eq!(u32_at(16), 3, "bit fields, so the alpha channel counts");
        // The bottom row first: the half-transparent blue (straight alpha), then the red.
        assert_eq!(&dib[124..128], &[255, 0, 0, 128]);
        assert_eq!(&dib[132..136], &[0, 0, 255, 255]);
    }
}

#[cfg(all(test, windows))]
mod system_tests {
    use super::*;

    /// Copies through the real clipboard, so it replaces what the clipboard holds: run by
    /// hand (`--ignored`), after saving the clipboard.
    #[test]
    #[ignore]
    fn the_system_clipboard_gets_a_16_bit_png_and_a_bitmap() {
        let px = vec![opal_core::tile::pixel_from_f32([0.30001, 0.5, 0.7, 1.0]); 4];
        let mut cb = SystemClipboard::default();
        cb.write_image(Copied { from: IRect::from_size(2, 2), pixels: px.clone() }, &egui::Context::default());
        // Read at once, while the copy may still be being written: the read waits for it.
        let own = match cb.read_image() {
            Read::Ready(r) => *r,
            Read::Later(read) => read(),
        };
        let own = own.unwrap().unwrap();
        assert_eq!(own.image.pixel(0, 0), px[0]);
        assert_eq!(own.from, Some(IRect::from_size(2, 2)), "it read the copy, not the clipboard before it");
        assert_eq!(cb.written(), vec![Ok(IRect::from_size(2, 2))]);
        let _open = open_clipboard().unwrap();
        let format = clipboard_win::raw::register_format("PNG").unwrap().get();
        let mut bytes = Vec::new();
        clipboard_win::raw::get_vec(format, &mut bytes).unwrap();
        assert!(clipboard_win::raw::is_format_avail(clipboard_win::formats::CF_DIBV5));
        drop(_open);
        let back = image_files::import_bytes(&bytes, std::path::Path::new("clip.png"), None).unwrap();
        assert_eq!(opal_io::pixels::to_srgb16(back.image.pixel(1, 1)), opal_io::pixels::to_srgb16(px[0]));
        // And, once written, Opal Studio reads back its own full-precision copy at once.
        let Read::Ready(own) = cb.read_image() else { panic!("its own copy is read at once") };
        assert_eq!((*own).unwrap().unwrap().from, Some(IRect::from_size(2, 2)));
        // Without its own copy, it reads the 16-bit PNG, as another program's.
        let Read::Later(read) = SystemClipboard::default().read_image() else { panic!("read off the thread") };
        let other = read().unwrap().unwrap();
        assert_eq!(other.from, None);
        assert_eq!(opal_io::pixels::to_srgb16(other.image.pixel(1, 1)), opal_io::pixels::to_srgb16(px[0]));
        // A bitmap alone (32-bit, BI_RGB), as most programs copy: its size is checked, then
        // it is read.
        {
            let mut dib: Vec<u8> = Vec::new();
            for v in [40u32, 2, 2] {
                dib.extend(v.to_le_bytes());
            }
            dib.extend(1u16.to_le_bytes());
            dib.extend(32u16.to_le_bytes());
            for v in [0u32, 16, 0, 0, 0, 0] {
                dib.extend(v.to_le_bytes());
            }
            dib.extend([200u8, 100, 50, 255].repeat(4));
            let _open = open_clipboard().unwrap();
            clipboard_win::raw::empty().unwrap();
            clipboard_win::raw::set_without_clear(clipboard_win::formats::CF_DIB, &dib).unwrap();
        }
        assert!(clipboard_bitmap_memory().is_some_and(|n| n > 0), "the bitmap's size was not read");
        let Read::Later(read) = SystemClipboard::default().read_image() else { panic!("read off the thread") };
        let bitmap = read().unwrap().expect("the bitmap was read");
        assert_eq!((bitmap.width, bitmap.height), (2, 2));
    }
}
