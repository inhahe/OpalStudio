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

/// What work may take now, if the system says: the memory free; while tiles may go to
/// disk, those in memory as far as the disk holds them, and the disk's room (once writing
/// there has failed, only the tiles that kept a copy there).
pub fn free() -> Option<Free> {
    #[cfg(test)]
    if let Some(free) = testing::FREE.get() {
        return free;
    }
    use opal_core::store;
    let memory = physical_free()?;
    let (reclaimable, disk) = if store::budget().is_none() {
        (0, 0)
    } else if store::status().trouble.is_some() {
        let kept = store::reclaimable();
        (kept, kept)
    } else {
        // Not known (off Windows): none counted, rather than more than there may be.
        let disk = disk_free().unwrap_or(0);
        (store::reclaimable().min(disk), disk)
    };
    Some(Free { memory, reclaimable, disk })
}

/// For work taking `at_once` bytes in one go (and tiles as it goes), while what this
/// returns is kept: tiles may take no more memory than they take now and what is free
/// leaves beside that, as measured now (see `opal_io::memory::Free::hold`); past it they go
/// to disk rather than into the memory the work and the system need.
pub fn hold_for(at_once: u64) -> Option<opal_core::store::Hold> {
    free().map(|f| f.hold(at_once))
}

/// The memory this computer has, in bytes, if the system says.
pub fn total() -> Option<u64> {
    #[cfg(windows)]
    {
        use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        let mut status =
            MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
        // SAFETY: `status` is a valid output struct whose `dwLength` is set, as the call requires.
        unsafe { GlobalMemoryStatusEx(&mut status) }.ok()?;
        Some(status.ullTotalPhys)
    }
    #[cfg(target_os = "linux")]
    {
        meminfo("MemTotal:")
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

/// Physical memory free now, in bytes, if the system says.
pub fn physical_free() -> Option<u64> {
    #[cfg(windows)]
    {
        use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        let mut status =
            MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
        // SAFETY: `status` is a valid output struct whose `dwLength` is set, as the call requires.
        unsafe { GlobalMemoryStatusEx(&mut status) }.ok()?;
        Some(status.ullAvailPhys.min(status.ullAvailPageFile))
    }
    #[cfg(target_os = "linux")]
    {
        meminfo("MemAvailable:")
    }
    #[cfg(not(any(windows, target_os = "linux")))]
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
/// has set aside so far.
#[derive(Clone, Copy, Debug)]
pub struct Allowance {
    /// `None` when not known: then there is no limit.
    free: Option<Free>,
    taken: Need,
}

impl Allowance {
    /// What is free now.
    pub fn now() -> Allowance {
        Allowance::of(free())
    }

    /// `free`, or no limit if `None`.
    pub fn of(free: Option<Free>) -> Allowance {
        Allowance { free, taken: Need::default() }
    }

    /// `bytes` of memory with nothing going to disk, or no limit if `None`.
    #[cfg(test)]
    pub fn memory(bytes: Option<u64>) -> Allowance {
        Allowance::of(bytes.map(Free::memory))
    }

    /// Sets aside `need` more, or says why there is not that much: how much the work would
    /// need, and how much is free.
    pub fn take(&mut self, need: Need) -> Result<(), String> {
        let total = self.taken.plus(need);
        if let Some(free) = &self.free {
            free.check(total).map_err(|short| format!("This {short}."))?;
        }
        self.taken = total;
        Ok(())
    }

    /// Sets aside `bytes` more of tiles ([`take`](Self::take)).
    pub fn take_tiles(&mut self, bytes: u64) -> Result<(), String> {
        self.take(Need::tiles(bytes))
    }

    /// For the work, while what this returns is kept: tiles may take no more memory than
    /// what is free now leaves beside what it set aside to take in one go ([`hold_for`]).
    pub fn hold(&self) -> Option<opal_core::store::Hold> {
        hold_for(self.taken.at_once)
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
        let mut a = Allowance::of(Some(Free { memory: 100, reclaimable: 0, disk: 1000 }));
        assert!(a.take_tiles(900).is_ok());
        assert!(a.take(Need::at_once(101)).is_err());
        assert!(a.take(Need::at_once(100)).is_ok());
    }
}
