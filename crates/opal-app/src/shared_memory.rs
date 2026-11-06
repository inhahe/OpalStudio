//! Named shared memory, through which pixels pass between the editor and a script without
//! going through the connection (see design.md, *Scripting*): a section the editor makes
//! for the pixels a script asks for, or one a script made for the pixels it gives.
//!
//! Windows only, for now: elsewhere, pixels go in the connection's lines instead.

/// A view of a named section of shared memory, unmapped and closed when dropped (the
/// section itself lasts while anyone else has it open).
pub struct Mapping {
    #[cfg(windows)]
    handle: windows::Win32::Foundation::HANDLE,
    #[cfg(windows)]
    view: windows::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS,
    len: usize,
}

// SAFETY: the view is plain memory and the handle a kernel object's, both usable from any
// thread; `Mapping` hands out the bytes only through `&self`/`&mut self`.
unsafe impl Send for Mapping {}

/// A name for a new section: `Local\` (this session's), made unguessable (so none could have
/// taken it first); an error if the system gives no random bytes.
pub fn new_name() -> std::io::Result<String> {
    Ok(format!("Local\\opal-pixels-{}-{}", std::process::id(), crate::server::random_hex(12)?))
}

impl Mapping {
    /// The bytes, `len` of them.
    pub fn bytes(&self) -> &[u8] {
        #[cfg(windows)]
        // SAFETY: the view maps at least `len` bytes (checked when made or opened), for as
        // long as `self` lives.
        unsafe {
            std::slice::from_raw_parts(self.view.Value.cast::<u8>(), self.len)
        }
        #[cfg(not(windows))]
        unreachable!("no mapping is made elsewhere")
    }

    /// The bytes, to write (a section this editor made).
    pub fn bytes_mut(&mut self) -> &mut [u8] {
        #[cfg(windows)]
        // SAFETY: as in `bytes`; the section was made for writing, and `&mut self` keeps
        // this the only slice of it here.
        unsafe {
            std::slice::from_raw_parts_mut(self.view.Value.cast::<u8>(), self.len)
        }
        #[cfg(not(windows))]
        unreachable!("no mapping is made elsewhere")
    }

    /// Makes a section of `len` bytes called `name`, for writing. Fails if there is one by
    /// that name already (someone else's: it is not written to).
    #[cfg(windows)]
    pub fn create(name: &str, len: usize) -> Result<Mapping, String> {
        use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, INVALID_HANDLE_VALUE};
        use windows::Win32::System::Memory::{CreateFileMappingW, FILE_MAP_ALL_ACCESS, MapViewOfFile, PAGE_READWRITE};
        let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
        let size = len.max(1) as u64;
        // SAFETY: a section backed by the paging file, of `size` bytes, with the default
        // security (the user's own); the name is a NUL-terminated wide string.
        let handle = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                None,
                PAGE_READWRITE,
                (size >> 32) as u32,
                size as u32,
                windows::core::PCWSTR(wide.as_ptr()),
            )
        }
        .map_err(|e| format!("The shared memory could not be made: {e}"))?;
        // SAFETY: read right after the call that set it.
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            // SAFETY: the handle was just opened, and is not used again.
            unsafe {
                let _ = CloseHandle(handle);
            }
            return Err("The shared memory's name was taken.".to_owned());
        }
        // SAFETY: the whole of the section just made, which is at least `len` bytes.
        let view = unsafe { MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, len.max(1)) };
        if view.Value.is_null() {
            // SAFETY: as above.
            unsafe {
                let _ = CloseHandle(handle);
            }
            return Err("The shared memory could not be mapped.".to_owned());
        }
        Ok(Mapping { handle, view, len })
    }

    /// Opens the section called `name` that someone else made, to read `len` bytes of it.
    /// Fails if there is none, or it is smaller.
    #[cfg(windows)]
    pub fn open(name: &str, len: usize) -> Result<Mapping, String> {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Memory::{
            FILE_MAP_READ, MEMORY_BASIC_INFORMATION, MapViewOfFile, OpenFileMappingW, VirtualQuery,
        };
        let wide: Vec<u16> = name.encode_utf16().chain([0]).collect();
        // SAFETY: the name is a NUL-terminated wide string; read access only.
        let handle = unsafe { OpenFileMappingW(FILE_MAP_READ.0, false, windows::core::PCWSTR(wide.as_ptr())) }
            .map_err(|_| format!("There is no shared memory called {name}."))?;
        let close = || {
            // SAFETY: the handle was opened above, and is not used again.
            unsafe {
                let _ = CloseHandle(handle);
            }
        };
        // SAFETY: the whole section, for reading.
        let view = unsafe { MapViewOfFile(handle, FILE_MAP_READ, 0, 0, 0) };
        if view.Value.is_null() {
            close();
            return Err(format!("The shared memory called {name} could not be mapped."));
        }
        let mut info = MEMORY_BASIC_INFORMATION::default();
        // SAFETY: asks about the view just mapped, into `info`, which is its size.
        let told = unsafe { VirtualQuery(Some(view.Value), &mut info, size_of::<MEMORY_BASIC_INFORMATION>()) };
        let mapping = Mapping { handle, view, len };
        // To the page: a section a few bytes short of `len` passes, its last page read as
        // the zeros it holds past its end, which is no harm.
        if told == 0 || info.RegionSize < len {
            return Err(format!("The shared memory called {name} holds less than the {len} bytes said."));
        }
        Ok(mapping)
    }

    #[cfg(not(windows))]
    pub fn create(_: &str, _: usize) -> Result<Mapping, String> {
        Err("Shared memory is not used on this system yet: ask for the pixels in the answer.".to_owned())
    }

    #[cfg(not(windows))]
    pub fn open(_: &str, _: usize) -> Result<Mapping, String> {
        Err("Shared memory is not used on this system yet: give the pixels as data.".to_owned())
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        #[cfg(windows)]
        // SAFETY: the view and handle are this mapping's own, and go with it.
        unsafe {
            let _ = windows::Win32::System::Memory::UnmapViewOfFile(self.view);
            let _ = windows::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn a_section_made_is_read_through_another_view_of_it() {
        let name = new_name().unwrap();
        let mut made = Mapping::create(&name, 10).unwrap();
        made.bytes_mut().copy_from_slice(b"0123456789");
        let opened = Mapping::open(&name, 10).unwrap();
        assert_eq!(opened.bytes(), b"0123456789");
        assert!(Mapping::create(&name, 10).is_err(), "made twice");
        assert!(Mapping::open(&name, 1 << 20).is_err(), "read past its end");
        drop((made, opened));
        assert!(Mapping::open(&name, 10).is_err(), "outlived everyone who had it");
    }
}
