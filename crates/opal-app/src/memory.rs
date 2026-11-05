//! The memory the system has free, so that work needing more is refused with the reason
//! rather than started: an allocation that fails ends the program, losing unsaved work.
//!
//! On Windows this is the available physical memory `GlobalMemoryStatusEx` reports; on
//! Linux, `MemAvailable` from `/proc/meminfo`. Elsewhere it is not known, and nothing is
//! refused for want of memory.

/// Physical memory free now, in bytes, if the system says.
pub fn free() -> Option<u64> {
    #[cfg(windows)]
    {
        use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        let mut status =
            MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
        // SAFETY: `status` is a valid output struct whose `dwLength` is set, as the call requires.
        unsafe { GlobalMemoryStatusEx(&mut status) }.ok()?;
        Some(status.ullAvailPhys)
    }
    #[cfg(target_os = "linux")]
    {
        let info = std::fs::read_to_string("/proc/meminfo").ok()?;
        let line = info.lines().find(|l| l.starts_with("MemAvailable:"))?;
        let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
        Some(kib * 1024)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        None
    }
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
                crate::dialogs::bytes(need),
                crate::dialogs::bytes(free)
            )),
            _ => {
                self.taken = need;
                Ok(())
            }
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
