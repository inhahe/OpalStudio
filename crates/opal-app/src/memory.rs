//! The memory the system has free, so that work needing more is refused with the reason
//! rather than started: an allocation that fails ends the program, losing unsaved work.
//! Images' tiles may go to disk (see `opal_core::store`), so room there counts too, as
//! `opal_io::memory` says.
//!
//! On Windows this is the available physical memory `GlobalMemoryStatusEx` reports, or
//! what the process may still commit if that is less (an allocation fails at the commit
//! limit, which a small page file can bring below the physical memory free); on Linux,
//! `MemAvailable` from `/proc/meminfo`. Elsewhere it is not known, and nothing is
//! refused for want of memory.

pub use opal_io::memory::{Free, Need};

/// What work may take now, if the system says: the memory free, less a twentieth of all
/// memory kept back for what estimates leave out and for the system, and less what work
/// running has set aside to take in one go (taken already or not); while tiles may go to
/// disk, those in memory as far as the disk holds them, and the disk's room (once writing
/// there has failed, only the tiles that kept a copy there); and what tiles take now.
pub fn free() -> Option<Free> {
    #[cfg(test)]
    if let Some(free) = testing::FREE.get() {
        return free;
    }
    use opal_core::store;
    let (total, available) = system()?;
    let status = store::status();
    let (reclaimable, disk) = if store::budget().is_none() {
        (0, 0)
    } else if status.trouble.is_some() {
        let kept = store::reclaimable();
        (kept, kept)
    } else {
        // Not known (off Windows): none counted, rather than more than there may be.
        let disk = disk_free().unwrap_or(0);
        (store::reclaimable().min(disk), disk)
    };
    let memory = available.saturating_sub(total / 20).saturating_sub(store::set_aside());
    Some(Free { memory, reclaimable, disk, resident: status.resident })
}

/// The memory this computer has, in bytes, if the system says.
pub fn total() -> Option<u64> {
    system().map(|(total, _)| total)
}

/// The memory this computer has, and how much of it is free now (on Windows the available
/// physical memory, or what the process may still commit if that is less).
fn system() -> Option<(u64, u64)> {
    #[cfg(windows)]
    {
        use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        let mut status =
            MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
        // SAFETY: `status` is a valid output struct whose `dwLength` is set, as the call requires.
        unsafe { GlobalMemoryStatusEx(&mut status) }.ok()?;
        Some((status.ullTotalPhys, status.ullAvailPhys.min(status.ullAvailPageFile)))
    }
    #[cfg(target_os = "linux")]
    {
        Some((meminfo("MemTotal:")?, meminfo("MemAvailable:")?))
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        None
    }
}

/// The free space on the disk the scratch file goes on (the temporary folder's), if known.
fn disk_free() -> Option<u64> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
        let dir: Vec<u16> = std::env::temp_dir().as_os_str().encode_wide().chain([0]).collect();
        let mut free = 0u64;
        // SAFETY: `dir` is a NUL-terminated wide string, and `free` a valid output.
        unsafe { GetDiskFreeSpaceExW(windows::core::PCWSTR(dir.as_ptr()), Some(&mut free), None, None) }.ok()?;
        Some(free)
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// A figure from `/proc/meminfo`, in bytes.
#[cfg(target_os = "linux")]
fn meminfo(key: &str) -> Option<u64> {
    let info = std::fs::read_to_string("/proc/meminfo").ok()?;
    let line = info.lines().find(|l| l.starts_with(key))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib * 1024)
}

/// The memory a piece of work may still take: what was free when it began, less what it
/// has set aside so far. Measured from the system ([`now`](Self::now)), it also caps the
/// tiles' memory while it is kept, at what that measure leaves them beside what the work
/// set aside to take in one go (`Free::cap`): keep it while the work runs.
#[derive(Debug)]
pub struct Allowance {
    /// `None` when not known: then there is no limit.
    free: Option<Free>,
    taken: Need,
    hold: Option<opal_core::store::Hold>,
}

impl Allowance {
    /// What is free now, and the tiles capped at what it leaves them.
    pub fn now() -> Allowance {
        let free = free();
        let hold = free.map(|f| f.hold(0));
        Allowance { free, taken: Need::default(), hold }
    }

    /// `free`, or no limit if `None`, capping nothing.
    #[cfg(test)]
    pub fn of(free: Option<Free>) -> Allowance {
        Allowance { free, taken: Need::default(), hold: None }
    }

    /// No limit, capping nothing: for work that measures what it takes itself.
    pub fn unlimited() -> Allowance {
        Allowance { free: None, taken: Need::default(), hold: None }
    }

    /// `bytes` of memory with nothing going to disk, or no limit if `None`.
    #[cfg(test)]
    pub fn memory(bytes: Option<u64>) -> Allowance {
        Allowance::of(bytes.map(Free::memory))
    }

    /// Sets aside `need` more, or says why there is not that much: how much the work would
    /// need, and how much is free. What it takes in one go lowers the cap on tiles.
    pub fn take(&mut self, need: Need) -> Result<(), String> {
        let total = self.taken.plus(need);
        if let Some(free) = &self.free {
            free.check(total).map_err(|short| format!("This {short}."))?;
        }
        self.taken = total;
        if need.at_once > 0
            && let (Some(hold), Some(free)) = (&mut self.hold, &self.free)
        {
            hold.lower(free.cap(total.at_once), total.at_once);
        }
        Ok(())
    }

    /// Gives back `need`, set aside before and no longer held (the cap on tiles stays as it
    /// was lowered).
    pub fn give_back(&mut self, need: Need) {
        self.taken = Need {
            at_once: self.taken.at_once.saturating_sub(need.at_once),
            tiles: self.taken.tiles.saturating_sub(need.tiles),
        };
        if need.at_once > 0
            && let (Some(hold), Some(free)) = (&mut self.hold, &self.free)
        {
            hold.lower(free.cap(self.taken.at_once), self.taken.at_once);
        }
    }

    /// Sets aside `bytes` more of tiles ([`take`](Self::take)).
    pub fn take_tiles(&mut self, bytes: u64) -> Result<(), String> {
        self.take(Need::tiles(bytes))
    }

    /// The cap on tiles, to keep while work carried on elsewhere runs (letting go of the rest).
    pub fn into_hold(self) -> Option<opal_core::store::Hold> {
        self.hold
    }
}

/// What a selection modifier's work (`opal_core::modify::Bounds`, asked from its workers) may
/// take as it goes, set aside from an allowance, and why it was refused, once it was.
pub struct Admission {
    memory: std::sync::Mutex<Allowance>,
    refused: std::sync::Mutex<Option<String>>,
}

impl Admission {
    pub fn new(memory: Allowance) -> Admission {
        Admission { memory: std::sync::Mutex::new(memory), refused: Default::default() }
    }

    /// What `work` makes, given bounds that stop it when `stop` says to and admit what it takes
    /// from `memory` as it goes; or why it did not: `edits::CANCELLED` if it was stopped, else
    /// why there was not memory enough.
    pub fn within<T>(
        memory: Allowance,
        stop: &(dyn Fn() -> bool + Sync),
        work: impl FnOnce(opal_core::modify::Bounds) -> Result<T, opal_core::modify::Halt>,
    ) -> Result<T, String> {
        use opal_core::modify::{Bounds, Halt};
        let admission = Admission::new(memory);
        let (admit, give_back) = (|n| admission.admit(n), |n| admission.give_back(n));
        work(Bounds { stop, admit: &admit, give_back: &give_back }).map_err(|halt| match halt {
            Halt::Stopped => crate::edits::CANCELLED.to_owned(),
            Halt::NoMemory => admission.refused(),
        })
    }

    /// Gives back `bytes` taken before.
    fn give_back(&self, bytes: u64) {
        self.memory.lock().expect("not poisoned").give_back(Need::at_once(bytes));
    }

    /// Whether `bytes` more may be taken, all of them in memory (set aside if so).
    fn admit(&self, bytes: u64) -> bool {
        match self.memory.lock().expect("not poisoned").take(Need::at_once(bytes)) {
            Ok(()) => true,
            Err(why) => {
                self.refused.lock().expect("not poisoned").get_or_insert(why);
                false
            }
        }
    }

    /// Why it was refused.
    fn refused(&self) -> String {
        let why = self.refused.lock().expect("not poisoned").clone();
        why.unwrap_or_else(|| "There was not memory enough for it.".into())
    }
}

/// Where the memory free for work is measured: on any thread, when the work begins, as on the
/// thread that made it (where tests may pretend: `testing::pretend_free`).
#[derive(Clone, Copy, Debug)]
pub struct Measure {
    #[cfg(test)]
    pretended: Option<Option<Free>>,
}

impl Measure {
    /// Measured as on this thread.
    pub fn here() -> Measure {
        Measure {
            #[cfg(test)]
            pretended: testing::FREE.get(),
        }
    }

    /// What is free now ([`Allowance::now`]).
    pub fn now(self) -> Allowance {
        #[cfg(test)]
        if let Some(free) = self.pretended {
            let was = testing::FREE.replace(Some(free));
            let allowance = Allowance::now();
            testing::FREE.set(was);
            return allowance;
        }
        Allowance::now()
    }
}

/// For tests: what is free, made up.
#[cfg(test)]
pub mod testing {
    use super::Free;
    use std::cell::Cell;

    thread_local! {
        /// What [`free`](super::free) says on this thread, while pretended.
        pub(super) static FREE: Cell<Option<Option<Free>>> = const { Cell::new(None) };
    }

    /// Makes [`free`](super::free) say `free` bytes of memory, with nothing going to disk,
    /// on this thread (only, so tests running beside it are not affected) until what this
    /// returns is dropped.
    pub fn pretend_free(free: Option<u64>) -> Pretend {
        FREE.set(Some(free.map(Free::memory)));
        Pretend
    }

    pub struct Pretend;

    impl Drop for Pretend {
        fn drop(&mut self) {
            FREE.set(None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(any(windows, target_os = "linux"))]
    fn the_system_says_how_much_is_free() {
        let free = free().expect("known on Windows and Linux");
        assert!(free.memory > 0);
    }

    #[test]
    fn an_allowance_refuses_what_would_pass_what_is_free() {
        let mut a = Allowance::memory(Some(1_000_000_000));
        assert!(a.take_tiles(600_000_000).is_ok());
        let why = a.take_tiles(600_000_000).unwrap_err();
        assert!(why.contains("1.2 GB") && why.contains("1.0 GB"), "{why}");
        assert!(a.take_tiles(400_000_000).is_ok(), "a refusal set something aside");
        assert!(Allowance::memory(None).take_tiles(u64::MAX).is_ok(), "no limit when not known");
        // Tiles may go to disk; what is taken in one go may not.
        let mut a = Allowance::of(Some(Free { memory: 100, reclaimable: 0, disk: 1000, resident: 0 }));
        assert!(a.take_tiles(900).is_ok());
        assert!(a.take(Need::at_once(101)).is_err());
        assert!(a.take(Need::at_once(100)).is_ok());
    }
}
