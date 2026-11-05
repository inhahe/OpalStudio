//! The memory the system has free, so that work needing more is refused with the reason
//! rather than started: an allocation that fails ends the program, losing unsaved work.
//! What tiles going to disk could give back counts as free too (see `opal_core::store`).
//!
//! On Windows this is the available physical memory `GlobalMemoryStatusEx` reports, or
//! what the process may still commit if that is less (an allocation fails at the commit
//! limit, which a small page file can bring below the physical memory free); on Linux,
//! `MemAvailable` from `/proc/meminfo`. Elsewhere it is not known, and nothing is
//! refused for want of memory.

/// The memory work may take now, in bytes, if the system says: what is free, and what
/// images' tiles going to disk could give back (as far as the disk holds it).
pub fn free() -> Option<u64> {
    #[cfg(test)]
    if let Some(free) = testing::FREE.get() {
        return free;
    }
    let spill = opal_core::store::reclaimable().min(disk_free().unwrap_or(u64::MAX));
    Some(physical_free()?.saturating_add(spill))
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
fn physical_free() -> Option<u64> {
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
    free: Option<u64>,
    taken: u64,
}

impl Allowance {
    /// What is free now.
    pub fn now() -> Allowance {
        Allowance::of(free())
    }

    /// `free` bytes, or no limit if `None`.
    pub fn of(free: Option<u64>) -> Allowance {
        Allowance { free, taken: 0 }
    }

    /// Sets aside `bytes` more, or says why there is not that much: how much the work would
    /// need in all, and how much is free.
    pub fn take(&mut self, bytes: u64) -> Result<(), String> {
        let need = self.taken.saturating_add(bytes);
        match self.free {
            Some(free) if need > free => Err(format!(
                "This would need about {} of memory, but only {} is free.",
                opal_io::bytes(need),
                opal_io::bytes(free)
            )),
            _ => {
                self.taken = need;
                Ok(())
            }
        }
    }
}

/// For tests: what is free, made up.
#[cfg(test)]
pub mod testing {
    use std::cell::Cell;

    thread_local! {
        /// What [`free`](super::free) says on this thread, while pretended.
        pub(super) static FREE: Cell<Option<Option<u64>>> = const { Cell::new(None) };
    }

    /// Makes [`free`](super::free) say `free` on this thread (only, so tests running beside
    /// it are not affected) until what this returns is dropped.
    pub fn pretend_free(free: Option<u64>) -> Pretend {
        FREE.set(Some(free));
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
        assert!(free > 0);
    }

    #[test]
    fn an_allowance_refuses_what_would_pass_what_is_free() {
        let mut a = Allowance::of(Some(1_000_000_000));
        assert!(a.take(600_000_000).is_ok());
        let why = a.take(600_000_000).unwrap_err();
        assert!(why.contains("1.2 GB") && why.contains("1.0 GB"), "{why}");
        assert!(a.take(400_000_000).is_ok(), "a refusal set something aside");
        assert!(Allowance::of(None).take(u64::MAX).is_ok(), "no limit when not known");
    }
}
