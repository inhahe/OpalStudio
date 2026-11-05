//! Images on the system clipboard.
//!
//! Reading tries, in order: a copy made by this application, if it is still what the
//! clipboard holds (it keeps full precision and where it was copied from); PNG (which keeps
//! 16-bit color and transparency); image files copied in a file manager; the system's own
//! bitmap format; JPEG. Writing puts PNG and the system bitmap on the clipboard, through
//! `arboard`.

use opal_core::geom::IRect;
use opal_core::raster::RasterImage;
use opal_core::tile::Pixel;
use opal_io::image_files;
use opal_io::pixels::{pixels_from_rgba8, pixels_to_rgba8};

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

pub trait Clipboard {
    /// The image on the clipboard, `None` if it holds none, or why it could not be read.
    fn read_image(&mut self) -> Result<Option<Pasted>, String>;
    /// Puts an image on the clipboard.
    fn write_image(&mut self, copy: Copied) -> Result<(), String>;
    /// Whether the clipboard holds text (to explain why there is nothing to paste).
    fn has_text(&mut self) -> bool;
}

/// The last copy made by this application, kept at full precision.
#[cfg_attr(test, allow(dead_code, reason = "tests use TestClipboard"))]
struct OwnCopy {
    /// The clipboard's sequence number once the copy was on it: while it is unchanged, the
    /// clipboard still holds this copy.
    #[cfg_attr(not(windows), allow(dead_code))]
    sequence: Option<u32>,
    pasted: Pasted,
}

/// The operating system's clipboard.
#[derive(Default)]
#[cfg_attr(test, allow(dead_code, reason = "tests use TestClipboard"))]
pub struct SystemClipboard {
    own: Option<OwnCopy>,
}

#[cfg_attr(test, allow(dead_code, reason = "tests use TestClipboard"))]
impl SystemClipboard {
    /// This application's own copy, if the clipboard still holds it.
    fn own_copy(&self) -> Option<&Pasted> {
        let own = self.own.as_ref()?;
        #[cfg(windows)]
        {
            let now = clipboard_win::raw::seq_num().map(|n| n.get());
            (now.is_some() && now == own.sequence).then_some(&own.pasted)
        }
        #[cfg(not(windows))]
        {
            // Without a sequence number, compare what the clipboard holds with the copy.
            let mut cb = arboard::Clipboard::new().ok()?;
            let img = cb.get_image().ok()?;
            let same = img.width == own.pasted.width as usize
                && img.height == own.pasted.height as usize
                && *img.bytes
                    == pixels_to_rgba8(
                        &own.pasted.image.read_rect(&IRect::from_size(img.width as i64, img.height as i64)),
                    );
            same.then_some(&own.pasted)
        }
    }
}

#[cfg_attr(test, allow(dead_code, reason = "tests use TestClipboard"))]
fn from_import(i: image_files::Imported) -> Pasted {
    Pasted { width: i.width, height: i.height, image: i.image, from: None, warnings: i.warnings }
}

impl Clipboard for SystemClipboard {
    fn read_image(&mut self) -> Result<Option<Pasted>, String> {
        if let Some(own) = self.own_copy() {
            return Ok(Some(own.clone()));
        }
        #[cfg(windows)]
        if let Some(p) = read_windows_formats()? {
            return Ok(Some(p));
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
                let (w, h) = (img.width as u32, img.height as u32);
                let image = RasterImage::from_pixels(w, h, &pixels_from_rgba8(&img.bytes));
                Ok(Some(Pasted { width: w, height: h, image, from: None, warnings: Vec::new() }))
            }
            Err(arboard::Error::ContentNotAvailable) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    fn write_image(&mut self, copy: Copied) -> Result<(), String> {
        let (w, h) = (copy.from.width() as usize, copy.from.height() as usize);
        let bytes = pixels_to_rgba8(&copy.pixels);
        let mut cb = arboard::Clipboard::new().map_err(|e| e.to_string())?;
        cb.set_image(arboard::ImageData { width: w, height: h, bytes: bytes.into() }).map_err(|e| e.to_string())?;
        #[cfg(windows)]
        let sequence = clipboard_win::raw::seq_num().map(|n| n.get());
        #[cfg(not(windows))]
        let sequence = None;
        let image = RasterImage::from_pixels(w as u32, h as u32, &copy.pixels);
        let pasted = Pasted { width: w as u32, height: h as u32, image, from: Some(copy.from), warnings: Vec::new() };
        self.own = Some(OwnCopy { sequence, pasted });
        Ok(())
    }

    fn has_text(&mut self) -> bool {
        arboard::Clipboard::new().and_then(|mut c| c.get_text()).is_ok_and(|t| !t.trim().is_empty())
    }
}

/// The formats `arboard` does not read, or reads only at 8 bits: PNG and JPEG as files
/// would hold them, and copied files.
#[cfg(windows)]
#[cfg_attr(test, allow(dead_code, reason = "tests use TestClipboard"))]
fn read_windows_formats() -> Result<Option<Pasted>, String> {
    use clipboard_win::{formats, raw};
    let _open = clipboard_win::Clipboard::new_attempts(10).map_err(|e| format!("the clipboard is busy ({e})"))?;
    let named = |name: &str| raw::register_format(name).map(|f| f.get()).filter(|&f| raw::is_format_avail(f));
    for (name, as_file) in [("PNG", "clipboard.png"), ("image/png", "clipboard.png")] {
        if let Some(f) = named(name) {
            let mut bytes = Vec::new();
            if raw::get_vec(f, &mut bytes).is_ok()
                && let Ok(i) = image_files::import_bytes(&bytes, std::path::Path::new(as_file))
            {
                return Ok(Some(from_import(i)));
            }
        }
    }
    if raw::is_format_avail(formats::CF_HDROP) {
        let mut files = Vec::new();
        if raw::get_file_list_path(&mut files).is_ok() {
            for f in files {
                if let Ok(i) = image_files::import(&f) {
                    return Ok(Some(from_import(i)));
                }
            }
        }
    }
    // The system bitmap (what most programs put on the clipboard) is read by arboard;
    // JPEG is the last resort, since a program offering it offers a bitmap too.
    if !raw::is_format_avail(formats::CF_DIB)
        && !raw::is_format_avail(formats::CF_DIBV5)
        && let Some(f) = named("JFIF")
    {
        let mut bytes = Vec::new();
        if raw::get_vec(f, &mut bytes).is_ok()
            && let Ok(i) = image_files::import_bytes(&bytes, std::path::Path::new("clipboard.jpg"))
        {
            return Ok(Some(from_import(i)));
        }
    }
    Ok(None)
}

/// A clipboard for tests: holds what was last written, or what a test put there.
#[cfg(test)]
#[derive(Default)]
pub struct TestClipboard {
    pub image: Option<Pasted>,
    pub text: bool,
}

#[cfg(test)]
impl Clipboard for TestClipboard {
    fn read_image(&mut self) -> Result<Option<Pasted>, String> {
        Ok(self.image.clone())
    }

    fn write_image(&mut self, copy: Copied) -> Result<(), String> {
        let (w, h) = (copy.from.width() as u32, copy.from.height() as u32);
        let image = RasterImage::from_pixels(w, h, &copy.pixels);
        self.image = Some(Pasted { width: w, height: h, image, from: Some(copy.from), warnings: Vec::new() });
        Ok(())
    }

    fn has_text(&mut self) -> bool {
        self.text
    }
}
