//! Where tiles keep their pixels: in memory, or, past a budget, in a scratch file on disk.
//!
//! Every tile's pixels live in a [`Slot`], which the store knows of. While the pixels in
//! memory take more than the budget ([`set_budget`]; none by default), the tiles read
//! least recently go to the scratch file, on the thread that took the memory past it, so
//! memory never runs ahead of the budget. A tile on disk is read back when next read, and
//! keeps its copy there while unchanged, so it can go again without being written. A tile
//! being read or changed is never taken away meanwhile.
//!
//! The scratch file is made in the system's temporary folder on first need and deleted
//! when the program ends (Windows deletes it as the program's handles close, even after a
//! crash). Writing to it failing (a full disk) stops tiles going to disk, and is reported
//! ([`status`]); reading back a tile written there failing loses pixels, and ends the
//! program.

use crate::tile::Data;
use parking_lot::{Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, OnceLock, Weak};

/// Where a tile's pixels are.
pub(crate) enum Body {
    /// In memory; and the region of the scratch file holding them as they are, if one does.
    Here { data: Data, kept: Option<Region> },
    /// In the scratch file only: `srgb8` says how they are stored there.
    Away { region: Region, srgb8: bool },
}

/// A tile's pixels, wherever they are.
pub(crate) struct Slot {
    pub body: RwLock<Body>,
    /// When they were last read: the store's epoch then ([`Store::epoch`]).
    used: AtomicU64,
}

/// A stretch of the scratch file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Region {
    offset: u64,
    len: u64,
}

impl Slot {
    /// A slot holding `data`, known to the store (which may take other tiles away to make
    /// room for it).
    pub fn new(data: Data) -> Arc<Slot> {
        let bytes = data.bytes();
        let slot =
            Arc::new(Slot { body: RwLock::new(Body::Here { data, kept: None }), used: AtomicU64::new(STORE.epoch()) });
        STORE.resident.fetch_add(bytes, Relaxed);
        {
            let mut slots = STORE.slots.lock();
            // Those let go are swept out as the list doubles, so it holds about the live ones.
            if slots.0.len() >= 2 * slots.1 {
                slots.0.retain(|w| w.strong_count() > 0);
                slots.1 = slots.0.len().max(1024);
            }
            slots.0.push(Arc::downgrade(&slot));
        }
        STORE.make_room();
        slot
    }

    /// The pixels to read, brought back first if they are on disk.
    pub fn read(&self) -> RwLockReadGuard<'_, Body> {
        self.used.store(STORE.epoch(), Relaxed);
        {
            let body = self.body.read();
            if matches!(*body, Body::Here { .. }) {
                return body;
            }
        }
        let mut body = self.body.write();
        if let Body::Away { region, srgb8 } = *body {
            let data = STORE.load(region, srgb8);
            STORE.resident.fetch_add(data.bytes(), Relaxed);
            *body = Body::Here { data, kept: Some(region) };
        }
        let body = RwLockWriteGuard::downgrade(body);
        // Brought back past the budget: others go (never this one, which is being read).
        STORE.make_room();
        body
    }

    /// The pixels to change, brought back first if they are on disk. Any copy on disk is
    /// let go, as it will no longer be them.
    pub fn write(&self) -> RwLockWriteGuard<'_, Body> {
        self.used.store(STORE.epoch(), Relaxed);
        let mut body = self.body.write();
        match &mut *body {
            Body::Away { region, srgb8 } => {
                let (region, srgb8) = (*region, *srgb8);
                let data = STORE.load(region, srgb8);
                STORE.resident.fetch_add(data.bytes(), Relaxed);
                STORE.free(region);
                *body = Body::Here { data, kept: None };
            }
            Body::Here { kept, .. } => {
                if let Some(region) = kept.take() {
                    STORE.free(region);
                }
            }
        }
        body
    }

    /// The memory the pixels take in memory, and whether they are kept at 8 bits, without
    /// bringing them back.
    pub fn peek(&self) -> (u64, bool) {
        match &*self.body.read() {
            Body::Here { data, .. } => (data.bytes(), data.is_srgb8()),
            Body::Away { region, srgb8 } => (region.len, *srgb8),
        }
    }

    /// The pixels in memory now take `to` bytes, not `from` (they changed kind).
    pub fn resized(&self, from: u64, to: u64) {
        STORE.resident.fetch_add(to, Relaxed);
        STORE.resident.fetch_sub(from, Relaxed);
        STORE.make_room();
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        match self.body.get_mut() {
            Body::Here { data, kept } => {
                STORE.resident.fetch_sub(data.bytes(), Relaxed);
                if let Some(r) = kept.take() {
                    STORE.free(r);
                }
            }
            Body::Away { region, .. } => STORE.free(*region),
        }
    }
}

/// The pixels in memory, and in the scratch file (where a tile brought back keeps its copy
/// while unchanged), in bytes; and why tiles stopped going to disk, if they did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub resident: u64,
    pub on_disk: u64,
    pub trouble: Option<String>,
}

/// How tiles stand now.
pub fn status() -> Status {
    Status {
        resident: STORE.resident.load(Relaxed),
        on_disk: STORE.away.load(Relaxed),
        trouble: STORE.trouble.lock().clone(),
    }
}

/// Lets the pixels of tiles in memory take up to `bytes` (`None`: any amount, nothing going
/// to disk), taking tiles away now if they take more.
pub fn set_budget(bytes: Option<u64>) {
    STORE.budget.store(bytes.unwrap_or(u64::MAX), Relaxed);
    STORE.make_room();
}

/// The memory that taking tiles to disk could give back now: the pixels in memory, while
/// a budget is set and tiles have not stopped going to disk.
pub fn reclaimable() -> u64 {
    if STORE.budget.load(Relaxed) == u64::MAX || STORE.stopped.load(Relaxed) { 0 } else { STORE.resident.load(Relaxed) }
}

struct Store {
    /// Every slot made (those let go are swept out now and then), and how many there were
    /// after the last sweep.
    slots: Mutex<(Vec<Weak<Slot>>, usize)>,
    /// The bytes of pixels in memory, and in the scratch file (a tile brought back keeps
    /// its copy there).
    resident: AtomicU64,
    away: AtomicU64,
    budget: AtomicU64,
    /// Advanced each time room is made: tiles read since have the latest.
    clock: AtomicU64,
    /// One thread makes room at a time.
    making_room: Mutex<()>,
    scratch: OnceLock<Result<std::fs::File, String>>,
    /// The scratch file's free regions, by length, and its end.
    free: Mutex<(BTreeMap<u64, Vec<u64>>, u64)>,
    /// Set once writing to disk failed; tiles go no more.
    stopped: AtomicBool,
    trouble: Mutex<Option<String>>,
}

static STORE: Store = Store {
    slots: Mutex::new((Vec::new(), 1024)),
    resident: AtomicU64::new(0),
    away: AtomicU64::new(0),
    budget: AtomicU64::new(u64::MAX),
    clock: AtomicU64::new(0),
    making_room: Mutex::new(()),
    scratch: OnceLock::new(),
    free: Mutex::new((BTreeMap::new(), 0)),
    stopped: AtomicBool::new(false),
    trouble: Mutex::new(None),
};

impl Store {
    fn epoch(&self) -> u64 {
        self.clock.load(Relaxed)
    }

    /// While the pixels in memory take more than the budget, takes the tiles read least
    /// recently to disk, down to nine tenths of it (so this is not done for every tile
    /// made past it), skipping any being read or changed. A thread finding another at it
    /// waits, then looks again, so none goes on past the budget.
    fn make_room(&self) {
        let over = || self.resident.load(Relaxed) > self.budget.load(Relaxed) && !self.stopped.load(Relaxed);
        if !over() {
            return;
        }
        let _turn = self.making_room.lock();
        if !over() {
            return;
        }
        self.clock.fetch_add(1, Relaxed);
        let target = self.budget.load(Relaxed) / 10 * 9;
        let mut live: Vec<Arc<Slot>> = {
            let mut slots = self.slots.lock();
            slots.0.retain(|w| w.strong_count() > 0);
            slots.1 = slots.0.len().max(1024);
            slots.0.iter().filter_map(Weak::upgrade).collect()
        };
        live.sort_by_key(|s| s.used.load(Relaxed));
        for slot in &live {
            if self.resident.load(Relaxed) <= target || self.stopped.load(Relaxed) {
                break;
            }
            // Being read or changed: it stays.
            let Some(mut body) = slot.body.try_write() else { continue };
            if let Body::Here { data, kept } = &*body {
                let region = match kept {
                    Some(r) => Some(*r),
                    None => self.store(data),
                };
                if let Some(region) = region {
                    let (bytes, srgb8) = (data.bytes(), data.is_srgb8());
                    *body = Body::Away { region, srgb8 };
                    self.resident.fetch_sub(bytes, Relaxed);
                }
            }
        }
        // Let go of these after the locks above (the last holder may be here).
        drop(live);
    }

    /// Writes `data` to a free region of the scratch file; `None` if it could not be (and
    /// tiles go to disk no more).
    fn store(&self, data: &Data) -> Option<Region> {
        let file = match self.scratch.get_or_init(open_scratch) {
            Ok(f) => f,
            Err(e) => return self.stop(e.clone()),
        };
        let bytes = data.as_bytes();
        let region = self.take(bytes.len() as u64);
        if let Err(e) = write_at(file, bytes, region.offset) {
            self.free(region);
            return self.stop(format!("tiles could not be written to the scratch file ({e})"));
        }
        self.away.fetch_add(region.len, Relaxed);
        Some(region)
    }

    /// The pixels stored in `region`.
    fn load(&self, region: Region, srgb8: bool) -> Data {
        let Some(Ok(file)) = self.scratch.get() else { unreachable!("a region was written, so the file is open") };
        let mut data = Data::empty(srgb8);
        if let Err(e) = read_at(file, data.as_bytes_mut(), region.offset) {
            panic!("pixels kept in the scratch file could not be read back, and are lost ({e})");
        }
        data
    }

    /// A free region of `len` bytes.
    fn take(&self, len: u64) -> Region {
        let mut free = self.free.lock();
        if let Some(offset) = free.0.get_mut(&len).and_then(Vec::pop) {
            return Region { offset, len };
        }
        let offset = free.1;
        free.1 += len;
        Region { offset, len }
    }

    /// `region` is free again.
    fn free(&self, region: Region) {
        self.away.fetch_sub(region.len, Relaxed);
        self.free.lock().0.entry(region.len).or_default().push(region.offset);
    }

    fn stop(&self, why: String) -> Option<Region> {
        self.stopped.store(true, Relaxed);
        *self.trouble.lock() = Some(why);
        None
    }
}

/// The scratch file, made for this process and deleted when it ends.
fn open_scratch() -> Result<std::fs::File, String> {
    let path = std::env::temp_dir().join(format!("opal-studio-{}.scratch", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_DELETE_ON_CLOSE: u32 = 0x0400_0000;
        options.custom_flags(FILE_FLAG_DELETE_ON_CLOSE).share_mode(0);
    }
    let file =
        options.open(&path).map_err(|e| format!("the scratch file {} could not be made ({e})", path.display()))?;
    // Elsewhere, gone from the folder at once, and kept while open.
    #[cfg(not(windows))]
    let _ = std::fs::remove_file(&path);
    Ok(file)
}

fn write_at(file: &std::fs::File, mut bytes: &[u8], mut offset: u64) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        while !bytes.is_empty() {
            let n = file.seek_write(bytes, offset)?;
            if n == 0 {
                return Err(std::io::ErrorKind::WriteZero.into());
            }
            bytes = &bytes[n..];
            offset += n as u64;
        }
        Ok(())
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        file.write_all_at(bytes, offset)
    }
}

fn read_at(file: &std::fs::File, mut out: &mut [u8], mut offset: u64) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        while !out.is_empty() {
            let n = file.seek_read(out, offset)?;
            if n == 0 {
                return Err(std::io::ErrorKind::UnexpectedEof.into());
            }
            out = &mut out[n..];
            offset += n as u64;
        }
        Ok(())
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        file.read_exact_at(out, offset)
    }
}
