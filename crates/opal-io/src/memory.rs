//! The memory a piece of work needs, and what it may have, so that work needing more is
//! refused with the reason rather than started: an allocation that fails ends the program,
//! losing unsaved work.
//!
//! Tiles past the memory budget go to disk (`opal_core::store`), so the tiles work makes
//! as it goes may take room on disk as well as in memory. What it takes in one go (a decoded
//! file, a bitmap for the clipboard) must be in memory all at once: tiles already there can
//! go to disk to make room for it, held there while the work runs ([`Free::hold`]).

use crate::bytes;

/// The memory a piece of work needs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Need {
    /// What it takes in one go, which must be in memory all at once.
    pub at_once: u64,
    /// The tiles it makes as it goes, which may go to disk past the memory budget.
    pub tiles: u64,
}

impl Need {
    pub const fn at_once(bytes: u64) -> Need {
        Need { at_once: bytes, tiles: 0 }
    }

    pub const fn tiles(bytes: u64) -> Need {
        Need { at_once: 0, tiles: bytes }
    }

    pub fn total(&self) -> u64 {
        self.at_once.saturating_add(self.tiles)
    }

    pub fn plus(self, other: Need) -> Need {
        Need { at_once: self.at_once.saturating_add(other.at_once), tiles: self.tiles.saturating_add(other.tiles) }
    }
}

/// What a piece of work may take, as the application finds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Free {
    /// Memory free now.
    pub memory: u64,
    /// How much of the tiles in memory could go to disk, to make room for what work takes
    /// in one go.
    pub reclaimable: u64,
    /// How much may go to disk in all: the room tiles have beyond `memory`.
    pub disk: u64,
    /// What tiles took in memory when `memory` was measured (see [`cap`](Self::cap)).
    pub resident: u64,
}

/// Why work needing more than it may have cannot be done: it would need `need` bytes, of
/// which there are `have`, in memory alone, or (`disk`) in memory and on disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Short {
    pub need: u64,
    pub have: u64,
    pub disk: bool,
}

impl std::fmt::Display for Short {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let room = if self.disk { "in memory and on disk" } else { "of memory" };
        write!(f, "would need about {} {room}, but only {} is free", bytes(self.need), bytes(self.have))
    }
}

impl Free {
    /// As measured before `bytes`, held now, were taken: for work whose need counts them
    /// (an image file decoded from bytes already read).
    pub fn counting(self, bytes: u64) -> Free {
        Free { memory: self.memory.saturating_add(bytes), ..self }
    }

    /// `bytes` of memory, and nothing going to disk.
    pub const fn memory(bytes: u64) -> Free {
        Free { memory: bytes, reclaimable: 0, disk: 0, resident: 0 }
    }

    /// Whether `need` fits: what it takes in one go, in memory with room made by tiles going
    /// to disk; and all of it, in memory and on disk.
    pub fn check(&self, need: Need) -> Result<(), Short> {
        let at_once = self.memory.saturating_add(self.reclaimable);
        if need.at_once > at_once {
            return Err(Short { need: need.at_once, have: at_once, disk: false });
        }
        let room = self.memory.saturating_add(self.disk);
        if need.total() > room {
            return Err(Short { need: need.total(), have: room, disk: self.disk > 0 });
        }
        Ok(())
    }

    /// The memory tiles may take while work taking `at_once` bytes in one go runs: what they
    /// took when this was measured and what was free then, less that. Tiles made since moved
    /// memory from free to taken, so the sum holds however long ago that was; what else was
    /// taken since must be in `at_once`.
    pub fn cap(&self, at_once: u64) -> u64 {
        self.resident.saturating_add(self.memory).saturating_sub(at_once)
    }

    /// Holds [`cap`](Self::cap) on tiles while what this returns is kept (those past it go to
    /// disk, the first at once), so that the work's memory and theirs fit in what was free;
    /// what it takes in one go set aside, so that work measuring meanwhile does not count it.
    pub fn hold(&self, at_once: u64) -> opal_core::store::Hold {
        opal_core::store::hold(self.cap(at_once), at_once)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_work_takes_in_one_go_must_fit_in_memory_and_its_tiles_may_go_to_disk() {
        let free = Free { memory: 100, reclaimable: 50, disk: 1000, resident: 0 };
        assert!(free.check(Need { at_once: 150, tiles: 950 }).is_ok());
        assert_eq!(
            free.check(Need::at_once(151)),
            Err(Short { need: 151, have: 150, disk: false }),
            "in one go, past memory and the tiles that could make room"
        );
        assert_eq!(free.check(Need { at_once: 10, tiles: 1091 }), Err(Short { need: 1101, have: 1100, disk: true }));
        // With nothing going to disk, memory alone.
        let only = Free::memory(100);
        assert!(only.check(Need::tiles(100)).is_ok());
        let short = only.check(Need { at_once: 1, tiles: 100 }).unwrap_err();
        assert_eq!(short, Short { need: 101, have: 100, disk: false });
        assert!(short.to_string().contains("of memory"), "{short}");
    }

    #[test]
    fn tiles_may_take_what_they_took_and_what_was_free_less_what_work_takes_in_one_go() {
        let free = Free { memory: 300, reclaimable: 0, disk: 0, resident: 1000 };
        assert_eq!(free.cap(0), 1300);
        assert_eq!(free.cap(200), 1100);
        assert_eq!(free.cap(5000), 0, "no more than nothing");
        // Measured with bytes held that the work's need counts: as if before they were taken.
        assert_eq!(free.counting(50).memory, 350);
    }
}
