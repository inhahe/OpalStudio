//! Images on the system clipboard.
//!
//! Reading tries, in order: a copy made by this application, if it is still what the
//! clipboard holds (it keeps full precision and where it was copied from); PNG (which keeps
//! 16-bit color and transparency); image files copied in a file manager; the system's own
//! bitmap format; JPEG. Writing puts the system bitmap (8-bit) and, on Windows, a 16-bit
//! PNG on the clipboard. Both run off the interface thread: encoding or decoding a large
//! image takes a while.

use eframe::egui;
use opal_core::geom::IRect;
use opal_core::raster::RasterImage;
use opal_core::tile::Pixel;
use opal_io::image_files;
use opal_io::pixels::pixels_from_rgba8;
#[cfg(not(windows))]
use opal_io::pixels::pixels_to_rgba8;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, mpsc};
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
#[derive(Default)]
#[cfg_attr(test, allow(dead_code, reason = "tests use TestClipboard"))]
pub struct SystemClipboard {
    shared: Arc<Shared>,
    /// The writer thread's queue, once there is one.
    writer: Option<mpsc::Sender<(Copied, egui::Context)>>,
}

#[cfg_attr(test, allow(dead_code, reason = "tests use TestClipboard"))]
impl SystemClipboard {
    /// The thread that writes copies to the clipboard, one after another.
    fn start_writer(shared: Arc<Shared>) -> mpsc::Sender<(Copied, egui::Context)> {
        let (tx, rx) = mpsc::channel::<(Copied, egui::Context)>();
        std::thread::Builder::new()
            .name("clipboard writer".into())
            .spawn(move || {
                for (copy, ctx) in rx {
                    // Everything slow happens before the lock is taken (the interface takes
                    // it every frame). A panic is caught so the copy still counts as done.
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        write_system(&copy).map(|sequence| OwnCopy { sequence, pasted: copy.pasted() })
                    }))
                    .unwrap_or_else(|_| Err("copying failed unexpectedly".into()));
                    let mut s = shared.lock();
                    match result {
                        Ok(own) => {
                            s.own = Some(own);
                            s.done.push(Ok(copy.from));
                        }
                        Err(e) => s.done.push(Err(e)),
                    }
                    s.pending -= 1;
                    drop(s);
                    shared.finished.notify_all();
                    ctx.request_repaint();
                }
            })
            .expect("starting the clipboard writer");
        tx
    }
}

impl Drop for SystemClipboard {
    /// Waits for the copies still being written, so that one made just before quitting is
    /// on the clipboard.
    fn drop(&mut self) {
        self.writer = None;
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
        let shared = &self.shared;
        let tx = self.writer.get_or_insert_with(|| Self::start_writer(shared.clone()));
        if tx.send((copy, ctx.clone())).is_err() {
            self.writer = None;
            let mut s = self.shared.lock();
            s.pending -= 1;
            s.done.push(Err(format!("the clipboard writer stopped (copying {} × {})", from.width(), from.height())));
        }
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

/// Reads and decodes what the system clipboard holds (any thread may), which is `own` if
/// the clipboard still holds this application's last copy.
#[cfg_attr(test, allow(dead_code, reason = "tests use TestClipboard"))]
fn read_system(own: Option<OwnCopy>) -> ReadResult {
    // Formats are fetched one at a time, each with the clipboard opened for it alone, so the
    // clipboard can change between them (the user copying again): then everything is read
    // again, from what it holds now.
    #[cfg(windows)]
    for _ in 0..3 {
        let before = clipboard_win::raw::seq_num();
        let read = read_system_once(own.clone());
        if clipboard_win::raw::seq_num() == before {
            return read;
        }
    }
    read_system_once(own)
}

/// One reading of the system clipboard (see [`read_system`]).
#[cfg_attr(test, allow(dead_code, reason = "tests use TestClipboard"))]
fn read_system_once(own: Option<OwnCopy>) -> ReadResult {
    #[cfg(windows)]
    {
        if let Some(own) = own.filter(OwnCopy::held) {
            return Ok(Some(own.pasted));
        }
        if let Some(p) = read_windows_formats()? {
            return Ok(Some(p));
        }
    }
    let mut cb = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    #[cfg(not(windows))]
    if let Ok(files) = cb.get().file_list() {
        for f in files {
            if let Ok(i) = image_files::import(&f) {
                return Ok(Some(from_import(i)));
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
    u32s(&mut out, &[BI_BITFIELDS, 4 * w * h, 0, 0, 0, 0]);
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
/// would hold them, and copied files. Each is fetched only if the ones before it gave no
/// image, and the clipboard is held only while one is fetched, not while it is decoded.
#[cfg(windows)]
#[cfg_attr(test, allow(dead_code, reason = "tests use TestClipboard"))]
fn read_windows_formats() -> Result<Option<Pasted>, String> {
    use clipboard_win::{formats, raw};
    let named = |name: &str| raw::register_format(name).map(|f| f.get());
    // The bytes the clipboard holds as `format`, if it offers it and `also` holds.
    let fetch = |format: Option<u32>, also: &dyn Fn() -> bool| -> Result<Option<Vec<u8>>, String> {
        let Some(format) = format else { return Ok(None) };
        let _open = open_clipboard()?;
        if !raw::is_format_avail(format) || !also() {
            return Ok(None);
        }
        let mut bytes = Vec::new();
        Ok(raw::get_vec(format, &mut bytes).is_ok().then_some(bytes))
    };
    let decoded = |bytes: Vec<u8>, as_file: &str| {
        image_files::import_bytes(&bytes, std::path::Path::new(as_file)).ok().map(from_import)
    };
    for name in ["PNG", "image/png"] {
        if let Some(p) = fetch(named(name), &|| true)?.and_then(|b| decoded(b, "clipboard.png")) {
            return Ok(Some(p));
        }
    }
    let files = {
        let _open = open_clipboard()?;
        let mut files = Vec::new();
        if raw::is_format_avail(formats::CF_HDROP) {
            let _ = raw::get_file_list_path(&mut files);
        }
        files
    };
    for f in files {
        if let Ok(i) = image_files::import(&f) {
            return Ok(Some(from_import(i)));
        }
    }
    // The system bitmap (what most programs put on the clipboard) is read by arboard;
    // JPEG is the last resort, since a program offering it offers a bitmap too.
    let no_bitmap = || !raw::is_format_avail(formats::CF_DIB) && !raw::is_format_avail(formats::CF_DIBV5);
    Ok(fetch(named("JFIF"), &no_bitmap)?.and_then(|b| decoded(b, "clipboard.jpg")))
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
        let back = image_files::import_bytes(&bytes, std::path::Path::new("clip.png")).unwrap();
        assert_eq!(opal_io::pixels::to_srgb16(back.image.pixel(1, 1)), opal_io::pixels::to_srgb16(px[0]));
        // And, once written, Opal Studio reads back its own full-precision copy at once.
        let Read::Ready(own) = cb.read_image() else { panic!("its own copy is read at once") };
        assert_eq!((*own).unwrap().unwrap().from, Some(IRect::from_size(2, 2)));
        // Without its own copy, it reads the 16-bit PNG, as another program's.
        let Read::Later(read) = SystemClipboard::default().read_image() else { panic!("read off the thread") };
        let other = read().unwrap().unwrap();
        assert_eq!(other.from, None);
        assert_eq!(opal_io::pixels::to_srgb16(other.image.pixel(1, 1)), opal_io::pixels::to_srgb16(px[0]));
    }
}
