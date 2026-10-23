//! "Test Gain"'s window: a plain child window that tests drive with posted
//! mouse messages. A left click sets Gain to 0.75 as one gesture (begin,
//! perform, end); a middle click sets Tilt to 0.3 with no gesture (as a
//! mouse wheel does); a right click asks the host for 600 × 450. It can be
//! resized between 200 × 150 and 1600 × 1200.

use std::cell::{Cell, RefCell};
use std::ffi::{CStr, c_void};
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{Class, ComPtr, ComWrapper};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetWindowLongPtrW, MoveWindow,
    RegisterClassW, SetWindowLongPtrW, WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_RBUTTONDOWN, WNDCLASSW,
    WS_CHILD, WS_VISIBLE,
};

use crate::{GAIN, TILT};

/// The view's class name, which tests look for.
const CLASS: &str = "DawTestPluginView";
const MIN: (i32, i32) = (200, 150);
const MAX: (i32, i32) = (1600, 1200);

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

pub struct View {
    handler: Option<ComPtr<IComponentHandler>>,
    frame: RefCell<Option<ComPtr<IPlugFrame>>>,
    hwnd: Cell<HWND>,
    size: Cell<(i32, i32)>,
    /// This view as the host sees it, for `resizeView` (no reference held).
    me: Cell<*mut IPlugView>,
}

impl Class for View {
    type Interfaces = (IPlugView,);
}

impl View {
    /// A new view, as `createView` hands it out (one reference).
    pub fn create(handler: Option<ComPtr<IComponentHandler>>) -> *mut IPlugView {
        let view = ComWrapper::new(Self {
            handler,
            frame: RefCell::new(None),
            hwnd: Cell::new(std::ptr::null_mut()),
            size: Cell::new((400, 300)),
            me: Cell::new(std::ptr::null_mut()),
        });
        match view.to_com_ptr::<IPlugView>() {
            Some(ptr) => {
                view.me.set(ptr.as_ptr());
                ptr.into_raw()
            }
            None => std::ptr::null_mut(),
        }
    }

    fn gesture(&self, id: ParamID, value: f64, begin_end: bool) {
        if let Some(handler) = &self.handler {
            unsafe {
                if begin_end {
                    handler.beginEdit(id);
                }
                handler.performEdit(id, value);
                if begin_end {
                    handler.endEdit(id);
                }
            }
        }
    }

    fn ask_for(&self, width: i32, height: i32) {
        let frame = self.frame.borrow().clone();
        if let Some(frame) = frame {
            let mut size = ViewRect {
                left: 0,
                top: 0,
                right: width,
                bottom: height,
            };
            unsafe { frame.resizeView(self.me.get(), &mut size) };
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let view = unsafe { (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const View).as_ref() };
    match (message, view) {
        (WM_LBUTTONDOWN, Some(view)) => view.gesture(GAIN, 0.75, true),
        (WM_MBUTTONDOWN, Some(view)) => view.gesture(TILT, 0.3, false),
        (WM_RBUTTONDOWN, Some(view)) => view.ask_for(600, 450),
        _ => return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
    0
}

impl IPlugViewTrait for View {
    unsafe fn isPlatformTypeSupported(&self, kind: FIDString) -> tresult {
        if !kind.is_null() && unsafe { CStr::from_ptr(kind) } == c"HWND" {
            kResultTrue
        } else {
            kResultFalse
        }
    }

    unsafe fn attached(&self, parent: *mut c_void, _kind: FIDString) -> tresult {
        let class = wide(CLASS);
        let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(window_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: std::ptr::null_mut(),
            hCursor: std::ptr::null_mut(),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class.as_ptr(),
        };
        // Fails harmlessly once registered.
        unsafe { RegisterClassW(&wc) };
        let (width, height) = self.size.get();
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                class.as_ptr(),
                WS_CHILD | WS_VISIBLE,
                0,
                0,
                width,
                height,
                parent.cast(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            )
        };
        if hwnd.is_null() {
            return kResultFalse;
        }
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, std::ptr::from_ref(self) as isize) };
        self.hwnd.set(hwnd);
        kResultOk
    }

    unsafe fn removed(&self) -> tresult {
        let hwnd = self.hwnd.replace(std::ptr::null_mut());
        if !hwnd.is_null() {
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                DestroyWindow(hwnd);
            }
        }
        kResultOk
    }

    unsafe fn onWheel(&self, _distance: f32) -> tresult {
        kResultFalse
    }

    unsafe fn onKeyDown(&self, _key: char16, _code: int16, _modifiers: int16) -> tresult {
        kResultFalse
    }

    unsafe fn onKeyUp(&self, _key: char16, _code: int16, _modifiers: int16) -> tresult {
        kResultFalse
    }

    unsafe fn getSize(&self, size: *mut ViewRect) -> tresult {
        let Some(size) = (unsafe { size.as_mut() }) else {
            return kInvalidArgument;
        };
        let (width, height) = self.size.get();
        *size = ViewRect {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        };
        kResultOk
    }

    unsafe fn onSize(&self, size: *mut ViewRect) -> tresult {
        let Some(size) = (unsafe { size.as_ref() }) else {
            return kInvalidArgument;
        };
        let (width, height) = (size.right - size.left, size.bottom - size.top);
        self.size.set((width, height));
        let hwnd = self.hwnd.get();
        if !hwnd.is_null() {
            unsafe { MoveWindow(hwnd, 0, 0, width, height, 1) };
        }
        kResultOk
    }

    unsafe fn onFocus(&self, _state: TBool) -> tresult {
        kResultOk
    }

    unsafe fn setFrame(&self, frame: *mut IPlugFrame) -> tresult {
        *self.frame.borrow_mut() = unsafe { vst3::ComRef::from_raw(frame) }.map(|f| f.to_com_ptr());
        kResultOk
    }

    unsafe fn canResize(&self) -> tresult {
        kResultTrue
    }

    unsafe fn checkSizeConstraint(&self, rect: *mut ViewRect) -> tresult {
        let Some(rect) = (unsafe { rect.as_mut() }) else {
            return kInvalidArgument;
        };
        rect.right = rect.left + (rect.right - rect.left).clamp(MIN.0, MAX.0);
        rect.bottom = rect.top + (rect.bottom - rect.top).clamp(MIN.1, MAX.1);
        kResultTrue
    }
}
