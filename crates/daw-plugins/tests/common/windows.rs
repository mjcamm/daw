//! Finding and poking a host process's plugin windows from a test.

#![allow(unsafe_code, reason = "Win32 calls")]
#![allow(dead_code, reason = "each test binary uses some of these")]

use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowExW, GetClassNameW, GetClientRect, GetWindowRect, GetWindowTextW,
    GetWindowThreadProcessId, IsWindowVisible, PostMessageW,
};

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn text(f: impl FnOnce(&mut [u16]) -> i32) -> String {
    let mut buffer = [0u16; 256];
    let n = f(&mut buffer).max(0) as usize;
    String::from_utf16_lossy(&buffer[..n])
}

/// Every visible plugin window, in any process: (window, owner, title).
fn all_plugin_windows() -> Vec<(HWND, u32, String)> {
    unsafe extern "system" fn each(hwnd: HWND, found: LPARAM) -> i32 {
        let found = unsafe { &mut *(found as *mut Vec<HWND>) };
        found.push(hwnd);
        1
    }
    let mut all: Vec<HWND> = Vec::new();
    unsafe { EnumWindows(Some(each), (&raw mut all) as LPARAM) };
    all.into_iter()
        .filter(|&hwnd| {
            let class = text(|b| unsafe { GetClassNameW(hwnd, b.as_mut_ptr(), b.len() as i32) });
            class == "DawPluginWindow" && unsafe { IsWindowVisible(hwnd) } != 0
        })
        .map(|hwnd| {
            let mut owner = 0;
            unsafe { GetWindowThreadProcessId(hwnd, &mut owner) };
            let title = text(|b| unsafe { GetWindowTextW(hwnd, b.as_mut_ptr(), b.len() as i32) });
            (hwnd, owner, title)
        })
        .collect()
}

/// A host's visible plugin windows: (window, title).
pub fn windows_of(pid: u32) -> Vec<(HWND, String)> {
    all_plugin_windows()
        .into_iter()
        .filter(|(_, owner, _)| *owner == pid)
        .map(|(hwnd, _, title)| (hwnd, title))
        .collect()
}

/// Every visible plugin window: (window, title).
pub fn plugin_windows() -> Vec<(HWND, String)> {
    all_plugin_windows()
        .into_iter()
        .map(|(hwnd, _, title)| (hwnd, title))
        .collect()
}

pub fn view_of(window: HWND) -> HWND {
    let class = wide("DawTestPluginView");
    unsafe {
        FindWindowExW(
            window,
            std::ptr::null_mut(),
            class.as_ptr(),
            std::ptr::null(),
        )
    }
}

pub fn client_size(hwnd: HWND) -> (i32, i32) {
    let mut rect: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(hwnd, &mut rect) };
    (rect.right - rect.left, rect.bottom - rect.top)
}

pub fn window_rect(hwnd: HWND) -> RECT {
    let mut rect: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetWindowRect(hwnd, &mut rect) };
    rect
}

/// Waits (up to 5 s) until `ready` holds.
pub fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(5);
    while !ready() {
        assert!(Instant::now() < until, "timed out waiting for {what}");
        std::thread::yield_now();
    }
}

pub fn click(hwnd: HWND, message: u32) {
    unsafe { PostMessageW(hwnd, message, 0, 0) };
}
