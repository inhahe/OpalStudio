//! The paste keys, Ctrl+V and Shift+Insert, seen on Windows before the windowing layer.
//!
//! egui-winit turns these keys into a paste event only when the clipboard holds text or an
//! image it can read itself, and swallows them otherwise. With only files on the clipboard
//! (an image file copied in Explorer), pressing Ctrl+V would then do nothing. So on Windows
//! a message hook counts the presses instead, and the app pastes once per press, ignoring
//! egui's paste events (which a text field still receives as usual).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

/// Paste-key presses not yet handled.
static PRESSES: AtomicU32 = AtomicU32::new(0);

/// The interface, woken when a press arrives so it is handled at once.
static CONTEXT: OnceLock<eframe::egui::Context> = OnceLock::new();

/// Whether presses are counted here (on Windows, outside tests) rather than taken from
/// egui's paste events.
pub fn counted() -> bool {
    cfg!(all(windows, not(test)))
}

/// Makes the hook wake `ctx` on a press.
pub fn wake(ctx: &eframe::egui::Context) {
    let _ = CONTEXT.set(ctx.clone());
}

/// The presses since the last call.
pub fn take() -> u32 {
    PRESSES.swap(0, Ordering::Relaxed)
}

/// The Windows message hook (see `winit`'s `EventLoopBuilderExtWindows::with_msg_hook`):
/// counts paste-key presses, and never keeps a message from the window.
#[cfg(windows)]
pub fn hook(msg: *const std::ffi::c_void) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_INSERT, VK_MENU, VK_SHIFT, VK_V};
    use windows::Win32::UI::WindowsAndMessaging::{MSG, WM_KEYDOWN};
    // SAFETY: winit hands the hook a pointer to the message it is about to dispatch.
    let msg = unsafe { &*(msg as *const MSG) };
    // Bit 30 of lParam: the key was already down (a repeat).
    if msg.message != WM_KEYDOWN || (msg.lParam.0 >> 30) & 1 == 1 {
        return false;
    }
    // SAFETY: GetKeyState only reads the thread's keyboard state.
    let down = |k: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY| unsafe { GetKeyState(i32::from(k.0)) } < 0;
    let (ctrl, shift, alt) = (down(VK_CONTROL), down(VK_SHIFT), down(VK_MENU));
    let key = msg.wParam.0 as u16;
    // The same keys egui-winit takes for paste: Ctrl alone with V, or Shift with Insert.
    let paste = (key == VK_V.0 && ctrl && !shift && !alt) || (key == VK_INSERT.0 && shift);
    if paste {
        PRESSES.fetch_add(1, Ordering::Relaxed);
        if let Some(ctx) = CONTEXT.get() {
            ctx.request_repaint();
        }
    }
    false
}
