//! How much GPU memory the process uses and may use, asked of the operating system (wgpu
//! does not report it).
//!
//! On Windows this is DXGI's `QueryVideoMemoryInfo` for the adapter wgpu chose, matched by
//! vendor and device identifiers. Elsewhere it is not reported yet (Vulkan's
//! `VK_EXT_memory_budget` is the planned source).

/// A handle for querying one adapter's memory.
pub struct GpuMemory {
    #[cfg(windows)]
    adapter: Option<windows::Win32::Graphics::Dxgi::IDXGIAdapter3>,
}

impl GpuMemory {
    /// Finds the adapter with these PCI vendor and device identifiers.
    pub fn new(vendor: u32, device: u32) -> GpuMemory {
        #[cfg(windows)]
        {
            GpuMemory { adapter: find_adapter(vendor, device) }
        }
        #[cfg(not(windows))]
        {
            let _ = (vendor, device);
            GpuMemory {}
        }
    }

    /// Bytes in use by this process, and the budget the system currently allows it, in the
    /// adapter's own (local) memory.
    pub fn query(&self) -> Option<(u64, u64)> {
        #[cfg(windows)]
        {
            use windows::Win32::Graphics::Dxgi::{DXGI_MEMORY_SEGMENT_GROUP_LOCAL, DXGI_QUERY_VIDEO_MEMORY_INFO};
            let adapter = self.adapter.as_ref()?;
            let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
            // SAFETY: `adapter` is a live COM interface and `info` a valid output struct.
            unsafe { adapter.QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut info) }.ok()?;
            Some((info.CurrentUsage, info.Budget))
        }
        #[cfg(not(windows))]
        {
            None
        }
    }
}

#[cfg(windows)]
fn find_adapter(vendor: u32, device: u32) -> Option<windows::Win32::Graphics::Dxgi::IDXGIAdapter3> {
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIAdapter3, IDXGIFactory1};
    use windows::core::Interface;
    // SAFETY: plain DXGI factory creation and adapter enumeration; every returned interface
    // is reference counted by the `windows` crate.
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
        for i in 0.. {
            let Ok(adapter) = factory.EnumAdapters1(i) else { break };
            let Ok(desc) = adapter.GetDesc1() else { continue };
            if desc.VendorId == vendor && desc.DeviceId == device {
                return adapter.cast::<IDXGIAdapter3>().ok();
            }
        }
    }
    None
}
