//! Where tiles keep their pixels: in memory, or, past a budget, in a scratch file on disk.
//!
//! Every tile's pixels live in a [`Slot`], which the store knows of. While the pixels in
//! memory take more than the budget ([`set_budget`]; none by default), the tiles read
//! least recently go to the scratch file. A thread that takes memory past the budget sends
//! as much as it took to disk itself, so memory never runs ahead of the budget however fast
//! tiles are made; a thread of the store's own then takes more, down to a little below it
//! (a tenth, at most 256 MB), so that this is not done for every tile made past it, nor on
//! the thread that crossed it (the interface's, say). A tile on disk is read back when next
//! read, and keeps its copy there while unchanged, so it can go again without being
//! written. A tile being read or changed is never taken away meanwhile.
//!
//! The scratch file is made in the system's temporary folder on first need and deleted
//! when the program ends (Windows deletes it as the program's handles close, even after a
//! crash); space let go at its end is given back to the disk. Writing to it failing (a full
//! disk) stops tiles being written there, and is reported ([`status`]); tiles that kept a
//! copy there still go. Reading back a tile written there failing loses pixels, and ends
//! the program.

use crate::tile::Data;
use parking_lot::{Condvar, Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::collections::BTreeSet;
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
    /// Where they hold anything not fully transparent, once known (see
    /// [`content`](Self::content)): known while they are on disk too.
    content: AtomicU64,
}

/// [`Slot::content`] not yet known, and known to be nothing.
const UNKNOWN: u64 = u64::MAX;
const NOTHING: u64 = u64::MAX - 1;

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
        let slot = Arc::new(Slot {
            body: RwLock::new(Body::Here { data, kept: None }),
            used: AtomicU64::new(STORE.epoch()),
            content: AtomicU64::new(UNKNOWN),
        });
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
        STORE.took(bytes);
        slot
    }

    /// The pixels to read, brought back first if they are on disk. A thread may hold more
    /// than one of these on a slot (tiles are shared: one image's tiles may all be one).
    pub fn read(&self) -> RwLockReadGuard<'_, Body> {
        self.used.store(STORE.epoch(), Relaxed);
        {
            // Recursive: a thread already reading this slot must not queue behind a writer
            // waiting for that very read to end.
            let body = self.body.read_recursive();
            if matches!(*body, Body::Here { .. }) {
                return body;
            }
        }
        // Not here, so no thread is reading it, this one included.
        let mut body = self.body.write();
        let mut brought = 0;
        if let Body::Away { region, srgb8 } = *body {
            let data = STORE.load(region, srgb8);
            brought = data.bytes();
            STORE.resident.fetch_add(brought, Relaxed);
            STORE.kept.fetch_add(brought, Relaxed);
            *body = Body::Here { data, kept: Some(region) };
        }
        let body = RwLockWriteGuard::downgrade(body);
        // Brought back past the budget: others go (never this one, which is being read).
        STORE.took(brought);
        body
    }

    /// The pixels to change, brought back first if they are on disk. Any copy on disk is
    /// let go, as it will no longer be them.
    pub fn write(&self) -> RwLockWriteGuard<'_, Body> {
        self.used.store(STORE.epoch(), Relaxed);
        let mut body = self.body.write();
        self.content.store(UNKNOWN, Relaxed);
        let mut brought = 0;
        match &mut *body {
            Body::Away { region, srgb8 } => {
                let (region, srgb8) = (*region, *srgb8);
                let data = STORE.load(region, srgb8);
                brought = data.bytes();
                STORE.resident.fetch_add(brought, Relaxed);
                STORE.free(region);
                *body = Body::Here { data, kept: None };
            }
            Body::Here { data, kept } => {
                if let Some(region) = kept.take() {
                    STORE.kept.fetch_sub(data.bytes(), Relaxed);
                    STORE.free(region);
                }
            }
        }
        // Never this one, which is being changed.
        STORE.took(brought);
        body
    }

    /// The rectangle (`[x0, y0, x1, y1]` within the tile) holding the pixels that are not
    /// fully transparent (`Some(None)`: none is), if known since they last changed.
    pub fn content(&self) -> Option<Option<[u32; 4]>> {
        match self.content.load(Relaxed) {
            UNKNOWN => None,
            NOTHING => Some(None),
            v => Some(Some([0, 16, 32, 48].map(|s| (v >> s) as u16 as u32))),
        }
    }

    /// Notes what [`content`](Self::content) is, as found from the pixels.
    pub fn found_content(&self, rect: Option<[u32; 4]>) {
        let v = rect.map_or(NOTHING, |r| r.iter().zip([0, 16, 32, 48]).map(|(&c, s)| u64::from(c) << s).sum());
        self.content.store(v, Relaxed);
    }

    /// The memory the pixels take in memory, and whether they are kept at 8 bits, without
    /// bringing them back.
    pub fn peek(&self) -> (u64, bool) {
        match &*self.body.read_recursive() {
            Body::Here { data, .. } => (data.bytes(), data.is_srgb8()),
            Body::Away { region, srgb8 } => (region.len, *srgb8),
        }
    }

    /// The pixels in memory now take `to` bytes, not `from` (they changed kind, under
    /// [`write`](Self::write), so they keep no copy on disk).
    pub fn resized(&self, from: u64, to: u64) {
        STORE.resident.fetch_add(to, Relaxed);
        STORE.resident.fetch_sub(from, Relaxed);
        STORE.took(to.saturating_sub(from));
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        match self.body.get_mut() {
            Body::Here { data, kept } => {
                STORE.resident.fetch_sub(data.bytes(), Relaxed);
                if let Some(r) = kept.take() {
                    STORE.kept.fetch_sub(data.bytes(), Relaxed);
                    STORE.free(r);
                }
            }
            Body::Away { region, .. } => STORE.free(*region),
        }
    }
}

/// The pixels in memory, and in the scratch file (where a tile brought back keeps its copy
/// while unchanged), in bytes; how many tiles have been read back from there; and why tiles
/// stopped being written to disk, if they did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub resident: u64,
    pub on_disk: u64,
    pub read_back: u64,
    pub trouble: Option<String>,
}

/// How tiles stand now.
pub fn status() -> Status {
    Status {
        resident: STORE.resident.load(Relaxed),
        on_disk: STORE.away.load(Relaxed),
        read_back: STORE.loads.load(Relaxed),
        trouble: STORE.trouble.lock().clone(),
    }
}

/// Lets the pixels of tiles in memory take up to `bytes` (`None`: any amount, nothing going
/// to disk; then [`hold`] does nothing either). Past it, the store's own thread takes tiles
/// away; this does not wait for it.
pub fn set_budget(bytes: Option<u64>) {
    STORE.budget.store(bytes.unwrap_or(u64::MAX), Relaxed);
    if STORE.over() {
        STORE.wake();
    }
}

/// A cap on the memory tiles may take, while work that needs memory runs ([`hold`]); let go
/// of when dropped.
#[must_use = "the cap holds only while this is kept"]
#[derive(Debug)]
pub struct Hold {
    id: u64,
}

/// Lets the pixels of tiles in memory take no more than `bytes` while what this returns is
/// kept (the lowest cap held applies, and the budget, if lower; with no budget nothing
/// does): for work whose own memory, with the tiles', must fit in what is free. Tiles past
/// it are taken to disk at once, on the calling thread, a batch at a time.
pub fn hold(bytes: u64) -> Hold {
    let id = STORE.next_hold.fetch_add(1, Relaxed);
    {
        let mut holds = STORE.holds.lock();
        holds.push((id, bytes));
        STORE.cap.store(holds.iter().map(|h| h.1).min().unwrap_or(u64::MAX), Relaxed);
    }
    while STORE.over() {
        let until = STORE.resident.load(Relaxed).saturating_sub(BATCH).max(STORE.limit());
        if !STORE.take_away(|| STORE.resident.load(Relaxed) > until.max(STORE.limit())) {
            break;
        }
    }
    if STORE.over_target() {
        STORE.wake();
    }
    Hold { id }
}

impl Drop for Hold {
    fn drop(&mut self) {
        let mut holds = STORE.holds.lock();
        holds.retain(|h| h.0 != self.id);
        STORE.cap.store(holds.iter().map(|h| h.1).min().unwrap_or(u64::MAX), Relaxed);
    }
}

/// The memory tiles may take before going to disk, if a budget is set ([`set_budget`]).
pub fn budget() -> Option<u64> {
    let b = STORE.budget.load(Relaxed);
    (b != u64::MAX).then_some(b)
}

/// The memory that taking tiles to disk could give back now, while a budget is set: the
/// pixels in memory, or once writing to disk has failed, those that kept a copy there.
pub fn reclaimable() -> u64 {
    if STORE.budget.load(Relaxed) == u64::MAX {
        0
    } else if STORE.stopped.load(Relaxed) {
        STORE.kept.load(Relaxed)
    } else {
        STORE.resident.load(Relaxed)
    }
}

/// Waits for tiles being taken to disk to be (those already chosen).
#[doc(hidden)]
pub fn settle() {
    drop(STORE.queue.lock());
}

/// Stops tiles being written to disk, as a failure to write would.
#[doc(hidden)]
pub fn stop_writing_for_tests() {
    STORE.stop("stopped for a test".into());
}

struct Store {
    /// Every slot made (those let go are swept out now and then), and how many there were
    /// after the last sweep.
    slots: Mutex<(Vec<Weak<Slot>>, usize)>,
    /// The bytes of pixels in memory; of those, those with a copy on disk; and the bytes in
    /// the scratch file (a tile brought back keeps its copy there).
    resident: AtomicU64,
    kept: AtomicU64,
    away: AtomicU64,
    /// How many tiles have been read back.
    loads: AtomicU64,
    budget: AtomicU64,
    /// The caps held ([`hold`]), by number, the lowest of them, and the next number.
    holds: Mutex<Vec<(u64, u64)>>,
    cap: AtomicU64,
    next_hold: AtomicU64,
    /// Advanced each time the order of tiles to take away is made: tiles read since have
    /// the latest.
    clock: AtomicU64,
    /// The tiles to take away next; held while taking them, so one thread does at a time.
    queue: Mutex<Queue>,
    /// Whether the store's thread is wanted, and the signal waking it.
    wanted: Mutex<bool>,
    wake_up: Condvar,
    worker: OnceLock<()>,
    scratch: OnceLock<Result<std::fs::File, String>>,
    space: Mutex<Space>,
    /// Set once writing to disk failed; tiles are written there no more.
    stopped: AtomicBool,
    trouble: Mutex<Option<String>>,
}

/// The tiles to take away, least recently read last, as of epoch `epoch`.
struct Queue {
    order: Vec<Weak<Slot>>,
    epoch: u64,
}

static STORE: Store = Store {
    slots: Mutex::new((Vec::new(), 1024)),
    resident: AtomicU64::new(0),
    kept: AtomicU64::new(0),
    away: AtomicU64::new(0),
    loads: AtomicU64::new(0),
    budget: AtomicU64::new(u64::MAX),
    holds: Mutex::new(Vec::new()),
    cap: AtomicU64::new(u64::MAX),
    next_hold: AtomicU64::new(0),
    clock: AtomicU64::new(0),
    queue: Mutex::new(Queue { order: Vec::new(), epoch: 0 }),
    wanted: Mutex::new(false),
    wake_up: Condvar::new(),
    worker: OnceLock::new(),
    scratch: OnceLock::new(),
    space: Mutex::new(Space::new()),
    stopped: AtomicBool::new(false),
    trouble: Mutex::new(None),
};

/// How many bytes the store's thread takes away at a turn, letting other threads in between.
const BATCH: u64 = 32 << 20;

impl Store {
    fn epoch(&self) -> u64 {
        self.clock.load(Relaxed)
    }

    /// The memory tiles may take: the budget, or the lowest cap held if lower (none without
    /// a budget).
    fn limit(&self) -> u64 {
        let budget = self.budget.load(Relaxed);
        if budget == u64::MAX { budget } else { budget.min(self.cap.load(Relaxed)) }
    }

    /// What the store's thread takes memory down to: a tenth below the limit, or 256 MB if
    /// less (a cap may be held for a moment: it should not send gigabytes more to disk).
    fn target(&self) -> u64 {
        let limit = self.limit();
        limit - (limit / 10).min(256 << 20)
    }

    /// Whether the pixels in memory take more than the limit, and some can go.
    fn over(&self) -> bool {
        self.resident.load(Relaxed) > self.limit() && self.can_go()
    }

    /// Whether they take more than the target, and some can go.
    fn over_target(&self) -> bool {
        self.resident.load(Relaxed) > self.target() && self.can_go()
    }

    /// Whether any tile could go: any, while writing works; else those with a copy there.
    fn can_go(&self) -> bool {
        !self.stopped.load(Relaxed) || self.kept.load(Relaxed) > 0
    }

    /// A thread brought `bytes` more into memory: past the limit, it takes as much away
    /// (or what brings memory back to the limit, if less), and the store's thread is asked
    /// for the rest. Bringing nothing (a tile changed where it was), it takes nothing, nor
    /// waits for a turn to.
    fn took(&self, bytes: u64) {
        if !self.over() {
            return;
        }
        if bytes > 0 {
            let until = self.resident.load(Relaxed).saturating_sub(bytes);
            self.take_away(|| self.resident.load(Relaxed) > until.max(self.limit()));
        }
        self.wake();
    }

    /// Asks the store's thread to take tiles away, starting it on first need.
    fn wake(&self) {
        self.worker.get_or_init(|| {
            std::thread::Builder::new()
                .name("tiles to disk".into())
                .spawn(|| STORE.work())
                .expect("a thread for taking tiles to disk");
        });
        *self.wanted.lock() = true;
        self.wake_up.notify_one();
    }

    /// The store's thread: when woken, takes tiles away down to the target, a batch at a
    /// time.
    fn work(&self) {
        loop {
            {
                let mut wanted = self.wanted.lock();
                while !*wanted {
                    self.wake_up.wait(&mut wanted);
                }
                *wanted = false;
            }
            while self.over_target() {
                // The target is asked again as tiles go: the budget may be raised meanwhile.
                let until = self.resident.load(Relaxed).saturating_sub(BATCH);
                if !self.take_away(|| self.resident.load(Relaxed) > until.max(self.target())) {
                    break;
                }
            }
        }
    }

    /// Takes the tiles read least recently to disk while `more` says so, skipping any being
    /// read or changed, or read since the order was made; returns whether any went.
    fn take_away(&self, more: impl Fn() -> bool) -> bool {
        let mut queue = self.queue.lock();
        let mut refilled = false;
        let mut any = false;
        while more() && self.can_go() {
            let Some(next) = queue.order.pop() else {
                if refilled {
                    break;
                }
                refilled = true;
                self.refill(&mut queue);
                continue;
            };
            let Some(slot) = next.upgrade() else { continue };
            // Held by nothing else: let go of here, and freed.
            if Arc::strong_count(&slot) == 1 || slot.used.load(Relaxed) > queue.epoch {
                continue;
            }
            let Some(mut body) = slot.body.try_write() else { continue };
            let Body::Here { data, kept } = &*body else { continue };
            let region = match kept {
                Some(r) => {
                    self.kept.fetch_sub(data.bytes(), Relaxed);
                    *r
                }
                None if self.stopped.load(Relaxed) => continue,
                None => match self.store(data) {
                    Some(r) => r,
                    None => continue,
                },
            };
            let (bytes, srgb8) = (data.bytes(), data.is_srgb8());
            *body = Body::Away { region, srgb8 };
            self.resident.fetch_sub(bytes, Relaxed);
            any = true;
        }
        any
    }

    /// Orders every live tile, least recently read last.
    fn refill(&self, queue: &mut Queue) {
        queue.epoch = self.clock.fetch_add(1, Relaxed);
        let mut live: Vec<(u64, Weak<Slot>)> = {
            let mut slots = self.slots.lock();
            slots.0.retain(|w| w.strong_count() > 0);
            slots.1 = slots.0.len().max(1024);
            // Each read once: tiles read meanwhile change it, and a sort must see each
            // keep its place.
            slots.0.iter().filter_map(|w| Some((w.upgrade()?.used.load(Relaxed), w.clone()))).collect()
        };
        live.sort_unstable_by_key(|(used, _)| std::cmp::Reverse(*used));
        queue.order = live.into_iter().map(|(_, w)| w).collect();
    }

    /// Writes `data` to a free region of the scratch file; `None` if it could not be (and
    /// tiles are written no more).
    fn store(&self, data: &Data) -> Option<Region> {
        let file = match self.scratch.get_or_init(open_scratch) {
            Ok(f) => f,
            Err(e) => return self.stop(e.clone()),
        };
        let bytes = data.as_bytes();
        let region = self.space.lock().take(bytes.len() as u64);
        if let Err(e) = write_at(file, bytes, region.offset) {
            self.give_back(region);
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
        self.loads.fetch_add(1, Relaxed);
        data
    }

    /// `region`, written to, is free again.
    fn free(&self, region: Region) {
        self.away.fetch_sub(region.len, Relaxed);
        self.give_back(region);
    }

    /// `region` is free again; space freed at the file's end goes back to the disk.
    fn give_back(&self, region: Region) {
        let mut space = self.space.lock();
        let end = space.end;
        space.give_back(region);
        if space.end < end
            && let Some(Ok(file)) = self.scratch.get()
        {
            // Only space: if the file cannot shrink now, its end is reused later.
            let _ = file.set_len(space.end);
        }
    }

    fn stop(&self, why: String) -> Option<Region> {
        self.stopped.store(true, Relaxed);
        *self.trouble.lock() = Some(why);
        None
    }
}

/// The space in the scratch file: blocks of [`BLOCK`] bytes, each holding a tile of working
/// pixels or two 8-bit ones, so that space either kind let go serves the other.
struct Space {
    /// Free blocks, and free halves of blocks whose other half is used, by offset.
    blocks: BTreeSet<u64>,
    halves: BTreeSet<u64>,
    /// The file's end: what lies past it is free.
    end: u64,
}

/// The bytes a tile of working pixels takes on disk; an 8-bit tile takes half.
const BLOCK: u64 = crate::raster::TILE_BYTES;

impl Space {
    const fn new() -> Space {
        Space { blocks: BTreeSet::new(), halves: BTreeSet::new(), end: 0 }
    }

    /// A free region of `len` bytes: a block, or half of one.
    fn take(&mut self, len: u64) -> Region {
        if len == BLOCK / 2 {
            if let Some(offset) = self.halves.pop_first() {
                return Region { offset, len };
            }
            let block = self.block();
            self.halves.insert(block + len);
            return Region { offset: block, len };
        }
        assert_eq!(len, BLOCK, "tiles take a block or half of one");
        Region { offset: self.block(), len }
    }

    /// A free block's offset.
    fn block(&mut self) -> u64 {
        self.blocks.pop_first().unwrap_or_else(|| {
            self.end += BLOCK;
            self.end - BLOCK
        })
    }

    /// `region` is free again: two free halves make a free block, and free blocks at the
    /// end are no longer the file's.
    fn give_back(&mut self, region: Region) {
        let block = if region.len == BLOCK {
            region.offset
        } else {
            let other = region.offset ^ (BLOCK / 2);
            if !self.halves.remove(&other) {
                self.halves.insert(region.offset);
                return;
            }
            region.offset & !(BLOCK - 1)
        };
        self.blocks.insert(block);
        while self.end > 0 && self.blocks.remove(&(self.end - BLOCK)) {
            self.end -= BLOCK;
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    const HALF: u64 = BLOCK / 2;

    #[test]
    fn space_either_kind_of_tile_lets_go_serves_the_other_and_the_end_is_given_back() {
        let mut s = Space::new();
        let a = s.take(HALF);
        let b = s.take(HALF);
        assert_eq!((a.offset, b.offset, s.end), (0, HALF, BLOCK), "two halves share a block");
        let c = s.take(BLOCK);
        assert_eq!((c.offset, s.end), (BLOCK, 2 * BLOCK));
        // Half a block let go: the block is not free while its other half is used.
        s.give_back(a);
        let d = s.take(BLOCK);
        assert_eq!(d.offset, 2 * BLOCK);
        s.give_back(d);
        assert_eq!(s.end, 2 * BLOCK, "the last block, let go, is the file's no more");
        // Both halves let go make a block a working tile can take.
        s.give_back(b);
        let e = s.take(BLOCK);
        assert_eq!(e.offset, 0);
        // A block let go serves two halves.
        s.give_back(e);
        assert_eq!((s.take(HALF).offset, s.take(HALF).offset), (0, HALF));
        // Let go at the end, the file shrinks, past free blocks before it too.
        s.give_back(Region { offset: HALF, len: HALF });
        s.give_back(Region { offset: 0, len: HALF });
        assert_eq!(s.end, 2 * BLOCK, "the last block is still used");
        s.give_back(c);
        assert_eq!(s.end, 0);
        assert!(s.blocks.is_empty() && s.halves.is_empty());
    }

    #[test]
    fn a_thread_reading_a_slot_may_read_it_again_while_a_writer_waits() {
        use std::sync::mpsc::channel;
        use std::time::Duration;
        let slot = Slot::new(Data::empty(false));
        let (held, is_held) = channel();
        let (go, may_go) = channel::<()>();
        let (done, is_done) = channel();
        let reader = {
            let slot = slot.clone();
            std::thread::spawn(move || {
                let first = slot.read();
                held.send(()).unwrap();
                may_go.recv().unwrap();
                let second = slot.read();
                done.send(()).unwrap();
                drop((second, first));
            })
        };
        is_held.recv().unwrap();
        let writer = {
            let slot = slot.clone();
            std::thread::spawn(move || drop(slot.body.write()))
        };
        // Long enough for the writer to be waiting.
        std::thread::sleep(Duration::from_millis(100));
        go.send(()).unwrap();
        assert!(
            is_done.recv_timeout(Duration::from_secs(5)).is_ok(),
            "the second read waited for the writer, which waits for the first"
        );
        reader.join().unwrap();
        writer.join().unwrap();
    }
}
