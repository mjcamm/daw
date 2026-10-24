//! Plugin windows: one native top-level window per open editor, made and
//! run on the host process's main thread (`docs/design/vst3-hosting.md`,
//! Editor windows).
//!
//! The plugin may resize its window (`IPlugFrame::resizeView`) and, if it
//! says it can, the user may too (its size limits come from
//! `checkSizeConstraint`). Content scale follows the screen. The windows
//! are never linked to the DAW's (that would freeze the DAW while a plugin
//! hangs): instead they lift themselves above the DAW's main window when it
//! comes forward ([`stacking`]). The window reports where the user moved it,
//! so the song can reopen it there.

#![cfg(windows)]
#![allow(
    unsafe_code,
    non_snake_case,
    reason = "Win32 windows and a COM interface the plugin calls"
)]

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use vst3::Steinberg::Vst::{IEditController, IEditControllerTrait};
use vst3::Steinberg::*;
use vst3::{Class, ComPtr, ComWrapper};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BLACK_BRUSH, GetMonitorInfoW, GetStockObject, HBRUSH, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MonitorFromPoint,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook};
use windows_sys::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow,
    EVENT_SYSTEM_FOREGROUND, GW_OWNER, GWL_EXSTYLE, GWL_STYLE, GWLP_USERDATA, GetClientRect,
    GetCursorPos, GetForegroundWindow, GetWindow, GetWindowLongPtrW, GetWindowLongW, GetWindowRect,
    GetWindowThreadProcessId, HWND_TOP, IDC_ARROW, LoadCursorW, RegisterClassW, SIZE_MINIMIZED,
    SW_HIDE, SW_SHOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetForegroundWindow,
    SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow, WINDOW_EX_STYLE, WINDOW_STYLE,
    WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS, WM_CLOSE, WM_DPICHANGED, WM_EXITSIZEMOVE,
    WM_SIZE, WM_SIZING, WMSZ_BOTTOMLEFT, WMSZ_LEFT, WMSZ_TOP, WMSZ_TOPLEFT, WMSZ_TOPRIGHT,
    WNDCLASSW, WS_CAPTION, WS_EX_TOOLWINDOW, WS_SYSMENU, WS_THICKFRAME,
};

const CLASS: &str = "DawPluginWindow";

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// What the window procedure and the plugin's frame share.
struct Shared {
    hwnd: Cell<HWND>,
    view: RefCell<Option<ComPtr<IPlugView>>>,
    resizable: bool,
    /// The user closed it: the host tears it down after handling messages.
    closed: Cell<bool>,
    /// Inside a resize the plugin asked for: it already knows its size.
    plugin_resizing: Cell<bool>,
    /// Placed or moved since the host last reported where it is.
    moved: Cell<bool>,
}

impl Shared {
    fn view(&self) -> Option<ComPtr<IPlugView>> {
        self.view.borrow().clone()
    }
}

/// The host side of the plugin's view: how it asks for another size.
struct PlugFrame {
    shared: Rc<Shared>,
}

impl Class for PlugFrame {
    type Interfaces = (IPlugFrame,);
}

impl IPlugFrameTrait for PlugFrame {
    unsafe fn resizeView(&self, view: *mut IPlugView, size: *mut ViewRect) -> tresult {
        let Some(rect) = (unsafe { size.as_ref() }) else {
            return kInvalidArgument;
        };
        let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
        if width <= 0 || height <= 0 {
            return kInvalidArgument;
        }
        self.shared.plugin_resizing.set(true);
        set_client_size(self.shared.hwnd.get(), width, height);
        self.shared.plugin_resizing.set(false);
        // Steinberg's order: the host resizes its window, then tells the view.
        if let Some(view) = unsafe { vst3::ComRef::from_raw(view) } {
            unsafe { view.onSize(size) };
        }
        kResultTrue
    }
}

/// An open plugin window. Dropping it closes it.
pub struct Editor {
    shared: Rc<Shared>,
    _frame: ComWrapper<PlugFrame>,
}

impl std::fmt::Debug for Editor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Editor").finish_non_exhaustive()
    }
}

impl Editor {
    /// Opens the controller's editor in a new window titled `title`, with
    /// its top-left at `at` (screen pixels) or near the mouse.
    pub fn open(
        controller: &ComPtr<IEditController>,
        title: &str,
        at: Option<(i32, i32)>,
    ) -> Result<Self, String> {
        let raw = unsafe { controller.createView(c"editor".as_ptr()) };
        let view =
            unsafe { ComPtr::<IPlugView>::from_raw(raw) }.ok_or("this plugin has no window")?;
        if unsafe { view.isPlatformTypeSupported(kPlatformTypeHWND) } != kResultTrue {
            return Err("this plugin's window can't be shown on Windows".into());
        }
        let resizable = unsafe { view.canResize() } == kResultTrue;
        let mut style = WS_CAPTION | WS_SYSMENU;
        if resizable {
            style |= WS_THICKFRAME;
        }
        let hwnd = create_window(title, style, WS_EX_TOOLWINDOW)?;
        let shared = Rc::new(Shared {
            hwnd: Cell::new(hwnd),
            view: RefCell::new(Some(view.clone())),
            resizable,
            closed: Cell::new(false),
            plugin_resizing: Cell::new(false),
            // Where it first lands is worth remembering too.
            moved: Cell::new(true),
        });
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, Rc::as_ptr(&shared) as isize) };
        let frame = ComWrapper::new(PlugFrame {
            shared: shared.clone(),
        });
        let editor = Self {
            shared,
            _frame: frame,
        };
        if let Some(frame) = editor._frame.to_com_ptr::<IPlugFrame>() {
            unsafe { view.setFrame(frame.as_ptr()) };
        }
        set_content_scale(&view, scale(hwnd));
        if unsafe { view.attached(hwnd.cast(), kPlatformTypeHWND) } != kResultOk {
            // Dropping `editor` takes the window down.
            return Err("this plugin's window would not open".into());
        }
        let mut size = ViewRect {
            left: 0,
            top: 0,
            right: 400,
            bottom: 300,
        };
        unsafe { view.getSize(&mut size) };
        set_client_size(
            hwnd,
            (size.right - size.left).max(1),
            (size.bottom - size.top).max(1),
        );
        place(hwnd, at);
        unsafe {
            ShowWindow(hwnd, SW_SHOW);
            SetForegroundWindow(hwnd);
        }
        stacking::add(hwnd);
        Ok(editor)
    }

    /// The window's content size, in pixels.
    pub fn size(&self) -> (u32, u32) {
        let mut rect: RECT = unsafe { std::mem::zeroed() };
        unsafe { GetClientRect(self.shared.hwnd.get(), &mut rect) };
        (
            (rect.right - rect.left).max(0) as u32,
            (rect.bottom - rect.top).max(0) as u32,
        )
    }

    /// Where the window's top-left corner is, to open it there next time.
    pub fn position(&self) -> (i32, i32) {
        let mut rect: RECT = unsafe { std::mem::zeroed() };
        unsafe { GetWindowRect(self.shared.hwnd.get(), &mut rect) };
        (rect.left, rect.top)
    }

    /// Whether the user closed it.
    pub fn closed(&self) -> bool {
        self.shared.closed.get()
    }

    /// Brings it in front, and renames it.
    pub fn raise(&self, title: &str) {
        self.set_title(title);
        let hwnd = self.shared.hwnd.get();
        unsafe {
            ShowWindow(hwnd, SW_SHOW);
            SetForegroundWindow(hwnd);
        }
    }

    pub fn set_title(&self, title: &str) {
        unsafe { SetWindowTextW(self.shared.hwnd.get(), wide(title).as_ptr()) };
    }

    /// Whether it was placed or moved since last asked.
    pub fn take_moved(&self) -> bool {
        self.shared.moved.replace(false)
    }
}

impl Drop for Editor {
    fn drop(&mut self) {
        let hwnd = self.shared.hwnd.get();
        stacking::remove(hwnd);
        if let Some(view) = self.shared.view.borrow_mut().take() {
            unsafe {
                view.removed();
                view.setFrame(std::ptr::null_mut());
            }
        }
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            DestroyWindow(hwnd);
        }
    }
}

fn create_window(title: &str, style: WINDOW_STYLE, ex: WINDOW_EX_STYLE) -> Result<HWND, String> {
    let class = wide(CLASS);
    let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
    thread_local! { static REGISTERED: Cell<bool> = const { Cell::new(false) }; }
    if !REGISTERED.get() {
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(window_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: std::ptr::null_mut(),
            hCursor: unsafe { LoadCursorW(std::ptr::null_mut(), IDC_ARROW) },
            hbrBackground: unsafe { GetStockObject(BLACK_BRUSH) } as HBRUSH,
            lpszMenuName: std::ptr::null(),
            lpszClassName: class.as_ptr(),
        };
        if unsafe { RegisterClassW(&wc) } == 0 {
            return Err("could not make a window".into());
        }
        REGISTERED.set(true);
    }
    let hwnd = unsafe {
        CreateWindowExW(
            ex,
            class.as_ptr(),
            wide(title).as_ptr(),
            style,
            0,
            0,
            400,
            300,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        )
    };
    if hwnd.is_null() {
        Err("could not make a window".into())
    } else {
        Ok(hwnd)
    }
}

/// The screen's scale where the window is (1.0 at 96 dpi).
fn scale(hwnd: HWND) -> f32 {
    unsafe { GetDpiForWindow(hwnd) }.max(96) as f32 / 96.0
}

fn set_content_scale(view: &ComPtr<IPlugView>, factor: f32) {
    if let Some(scaling) = view.cast::<IPlugViewContentScaleSupport>() {
        unsafe { scaling.setContentScaleFactor(factor) };
    }
}

/// The window's border and title around content of `width` × `height`.
fn frame_rect(hwnd: HWND, width: i32, height: i32) -> RECT {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: width,
        bottom: height,
    };
    unsafe {
        AdjustWindowRectExForDpi(
            &mut rect,
            GetWindowLongW(hwnd, GWL_STYLE) as u32,
            0,
            GetWindowLongW(hwnd, GWL_EXSTYLE) as u32,
            GetDpiForWindow(hwnd),
        )
    };
    rect
}

fn set_client_size(hwnd: HWND, width: i32, height: i32) {
    let rect = frame_rect(hwnd, width, height);
    unsafe {
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            0,
            0,
            rect.right - rect.left,
            rect.bottom - rect.top,
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
    };
}

/// Puts the window at `at`, or just below and right of the mouse, kept on
/// the screen it lands on.
fn place(hwnd: HWND, at: Option<(i32, i32)>) {
    let (x, y) = at.unwrap_or_else(|| {
        let mut cursor = POINT { x: 0, y: 0 };
        unsafe { GetCursorPos(&mut cursor) };
        (cursor.x + 24, cursor.y + 24)
    });
    let mut window: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetWindowRect(hwnd, &mut window) };
    let (width, height) = (window.right - window.left, window.bottom - window.top);
    let monitor = unsafe { MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST) };
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = size_of::<MONITORINFO>() as u32;
    let (x, y) = if unsafe { GetMonitorInfoW(monitor, &mut info) } != 0 {
        let work = info.rcWork;
        (
            x.min(work.right - width).max(work.left),
            y.min(work.bottom - height).max(work.top),
        )
    } else {
        (x, y)
    };
    unsafe {
        SetWindowPos(
            hwnd,
            std::ptr::null_mut(),
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        )
    };
}

/// The low and high 16 bits of a message parameter.
fn words(value: usize) -> (i32, i32) {
    ((value & 0xFFFF) as i32, ((value >> 16) & 0xFFFF) as i32)
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let shared = unsafe { (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Shared).as_ref() };
    let Some(shared) = shared else {
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    };
    match message {
        // The user let go after moving it.
        WM_EXITSIZEMOVE => {
            shared.moved.set(true);
            0
        }
        WM_CLOSE => {
            // Taken down by the host after this round of messages, so
            // nothing is freed while the plugin may still be on the stack.
            shared.closed.set(true);
            unsafe { ShowWindow(hwnd, SW_HIDE) };
            0
        }
        // The user drags an edge: keep to the sizes the plugin allows.
        WM_SIZING if shared.resizable => {
            let Some(rect) = (unsafe { (lparam as *mut RECT).as_mut() }) else {
                return 0;
            };
            let border = frame_rect(hwnd, 0, 0);
            let extra = (border.right - border.left, border.bottom - border.top);
            let mut want = ViewRect {
                left: 0,
                top: 0,
                right: (rect.right - rect.left - extra.0).max(1),
                bottom: (rect.bottom - rect.top - extra.1).max(1),
            };
            if let Some(view) = shared.view() {
                unsafe { view.checkSizeConstraint(&mut want) };
            }
            let (width, height) = (
                want.right - want.left + extra.0,
                want.bottom - want.top + extra.1,
            );
            let edge = wparam as u32;
            if matches!(edge, WMSZ_LEFT | WMSZ_TOPLEFT | WMSZ_BOTTOMLEFT) {
                rect.left = rect.right - width;
            } else {
                rect.right = rect.left + width;
            }
            if matches!(edge, WMSZ_TOP | WMSZ_TOPLEFT | WMSZ_TOPRIGHT) {
                rect.top = rect.bottom - height;
            } else {
                rect.bottom = rect.top + height;
            }
            1
        }
        WM_SIZE => {
            if !shared.plugin_resizing.get()
                && wparam as u32 != SIZE_MINIMIZED
                && let Some(view) = shared.view()
            {
                let (width, height) = words(lparam as usize);
                let mut size = ViewRect {
                    left: 0,
                    top: 0,
                    right: width,
                    bottom: height,
                };
                unsafe { view.onSize(&mut size) };
            }
            0
        }
        // Moved to a screen with another scale: the plugin rescales, and
        // the window takes the size Windows suggests.
        WM_DPICHANGED => {
            shared.moved.set(true);
            if let Some(view) = shared.view() {
                set_content_scale(&view, words(wparam).1 as f32 / 96.0);
            }
            if let Some(rect) = unsafe { (lparam as *const RECT).as_ref() } {
                unsafe {
                    SetWindowPos(
                        hwnd,
                        std::ptr::null_mut(),
                        rect.left,
                        rect.top,
                        rect.right - rect.left,
                        rect.bottom - rect.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    )
                };
            }
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

/// Plugin windows stay above the DAW's main window: when one of the DAW's
/// own unowned windows comes forward (the user clicked the DAW), they are
/// lifted just above it, without taking focus. The DAW's dialogs (a file
/// picker, owned by its window) stay above them, and other apps cover them
/// as they would any window. Told by the system when the active window
/// changes; never touches the DAW's windows.
pub mod stacking {
    use super::*;

    thread_local! {
        static WINDOWS: RefCell<Vec<HWND>> = const { RefCell::new(Vec::new()) };
        static DAW: Cell<u32> = const { Cell::new(0) };
        static HOOK: Cell<HWINEVENTHOOK> = const { Cell::new(std::ptr::null_mut()) };
    }

    /// Which process is the DAW; starts listening for app switches.
    pub fn set_daw(pid: u32) {
        DAW.set(pid);
        if HOOK.get().is_null() {
            let hook = unsafe {
                SetWinEventHook(
                    EVENT_SYSTEM_FOREGROUND,
                    EVENT_SYSTEM_FOREGROUND,
                    std::ptr::null_mut(),
                    Some(on_foreground),
                    0,
                    0,
                    WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
                )
            };
            HOOK.set(hook);
        }
    }

    pub(super) fn add(hwnd: HWND) {
        WINDOWS.with_borrow_mut(|w| w.push(hwnd));
    }

    pub(super) fn remove(hwnd: HWND) {
        WINDOWS.with_borrow_mut(|w| w.retain(|&h| h != hwnd));
    }

    /// Whether plugin windows go above a window that came forward: one of
    /// the DAW's own, not owned by another (a dialog).
    pub(crate) fn lift_above(front_pid: u32, front_owned: bool, daw_pid: u32) -> bool {
        front_pid == daw_pid && !front_owned
    }

    unsafe extern "system" fn on_foreground(
        _hook: HWINEVENTHOOK,
        _event: u32,
        front: HWND,
        _object: i32,
        _child: i32,
        _thread: u32,
        _time: u32,
    ) {
        let front = if front.is_null() {
            unsafe { GetForegroundWindow() }
        } else {
            front
        };
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(front, &mut pid) };
        let owned = !unsafe { GetWindow(front, GW_OWNER) }.is_null();
        if !lift_above(pid, owned, DAW.get()) {
            return;
        }
        // The most recently opened ends up on top.
        let windows = WINDOWS.with_borrow(Vec::clone);
        for hwnd in windows {
            unsafe {
                SetWindowPos(
                    hwnd,
                    HWND_TOP,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                )
            };
        }
    }

    #[cfg(test)]
    mod tests {
        use super::lift_above;

        #[test]
        fn plugin_windows_go_above_the_daws_main_window_only() {
            assert!(lift_above(10, false, 10), "the DAW's main window");
            assert!(!lift_above(10, true, 10), "a DAW dialog stays on top");
            assert!(!lift_above(11, false, 10), "another app covers them");
        }
    }
}
