//! The operating system's cross-process pieces: named shared memory, named
//! auto-reset events, a high-resolution timer and (on Windows) a job object
//! that ends host processes with the DAW.
//!
//! Windows only for now; elsewhere every constructor reports `Unsupported`.

#![allow(
    unsafe_code,
    reason = "operating system calls; each is wrapped so the rest of the crate is safe"
)]

/// Initialises COM on this thread (plugins with windows or system services
/// expect it on their host's main thread). No-op elsewhere.
pub fn init_com() {
    #[cfg(windows)]
    #[allow(unsafe_code, reason = "a plain system call")]
    unsafe {
        windows_sys::Win32::System::Ole::OleInitialize(std::ptr::null());
    }
}

/// Whether a wait ended because its object was signalled or the time ran out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wake {
    Signalled,
    TimedOut,
}

#[cfg(windows)]
pub use windows::*;

#[cfg(not(windows))]
pub use unsupported::*;

#[cfg(windows)]
mod windows {
    use super::Wake;
    use std::io;
    use std::ptr::null;
    use std::time::Duration;
    use windows_sys::Win32::Foundation::{
        CloseHandle, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    use windows_sys::Win32::System::Memory::{
        CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
        OpenFileMappingW, PAGE_READWRITE, UnmapViewOfFile,
    };
    use windows_sys::Win32::System::Threading::{
        CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateEventW, CreateWaitableTimerExW,
        EVENT_MODIFY_STATE, INFINITE, OpenEventW, SYNCHRONIZATION_SYNCHRONIZE, SetEvent,
        SetWaitableTimer, TIMER_ALL_ACCESS, WaitForMultipleObjects, WaitForSingleObject,
    };

    fn wide(name: &str) -> Vec<u16> {
        name.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// A handle closed on drop.
    #[derive(Debug)]
    struct Handle(HANDLE);

    // Kernel handles may be used from any thread.
    unsafe impl Send for Handle {}
    unsafe impl Sync for Handle {}

    impl Handle {
        fn checked(h: HANDLE) -> io::Result<Self> {
            if h.is_null() || h == INVALID_HANDLE_VALUE {
                Err(io::Error::last_os_error())
            } else {
                Ok(Self(h))
            }
        }
    }

    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }

    /// A named block of memory both processes map. Zeroed when created.
    #[derive(Debug)]
    pub struct SharedMemory {
        _mapping: Handle,
        view: *mut u8,
    }

    // The view is plain memory; who touches what is decided by the protocol
    // on top (`block.rs`), which hands each region to one side at a time.
    unsafe impl Send for SharedMemory {}
    unsafe impl Sync for SharedMemory {}

    impl SharedMemory {
        pub fn create(name: &str, len: usize) -> io::Result<Self> {
            let size = len as u64;
            let mapping = Handle::checked(unsafe {
                CreateFileMappingW(
                    INVALID_HANDLE_VALUE,
                    null(),
                    PAGE_READWRITE,
                    (size >> 32) as u32,
                    size as u32,
                    wide(name).as_ptr(),
                )
            })?;
            Self::map(mapping, len)
        }

        pub fn open(name: &str, len: usize) -> io::Result<Self> {
            let mapping = Handle::checked(unsafe {
                OpenFileMappingW(FILE_MAP_ALL_ACCESS, 0, wide(name).as_ptr())
            })?;
            Self::map(mapping, len)
        }

        fn map(mapping: Handle, len: usize) -> io::Result<Self> {
            let view = unsafe { MapViewOfFile(mapping.0, FILE_MAP_ALL_ACCESS, 0, 0, len) };
            if view.Value.is_null() {
                return Err(io::Error::last_os_error());
            }
            Ok(Self {
                _mapping: mapping,
                view: view.Value.cast(),
            })
        }

        pub fn as_ptr(&self) -> *mut u8 {
            self.view
        }
    }

    impl Drop for SharedMemory {
        fn drop(&mut self) {
            unsafe {
                UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                    Value: self.view.cast(),
                })
            };
        }
    }

    /// A named auto-reset event: `set` wakes one waiter, then it resets.
    #[derive(Debug)]
    pub struct Event(Handle);

    impl Event {
        pub fn create(name: &str) -> io::Result<Self> {
            Handle::checked(unsafe { CreateEventW(null(), 0, 0, wide(name).as_ptr()) }).map(Self)
        }

        /// An event only this process uses.
        pub fn local() -> io::Result<Self> {
            Handle::checked(unsafe { CreateEventW(null(), 0, 0, null()) }).map(Self)
        }

        pub fn open(name: &str) -> io::Result<Self> {
            Handle::checked(unsafe {
                OpenEventW(
                    EVENT_MODIFY_STATE | SYNCHRONIZATION_SYNCHRONIZE,
                    0,
                    wide(name).as_ptr(),
                )
            })
            .map(Self)
        }

        /// Never allocates or blocks.
        pub fn set(&self) {
            unsafe { SetEvent(self.0.0) };
        }

        /// Waits until set, or until `timeout` (`None`: forever). The timeout
        /// is coarse (the system tick); use [`Timer`] for audio deadlines.
        pub fn wait(&self, timeout: Option<Duration>) -> Wake {
            let ms = timeout.map_or(INFINITE, |t| {
                t.as_millis().min(u128::from(INFINITE - 1)) as u32
            });
            match unsafe { WaitForSingleObject(self.0.0, ms) } {
                WAIT_OBJECT_0 => Wake::Signalled,
                _ => Wake::TimedOut,
            }
        }

        /// Waits until set or until `timer` fires, without allocating.
        pub fn wait_or(&self, timer: &Timer) -> Wake {
            let handles = [self.0.0, timer.0.0];
            match unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) } {
                WAIT_OBJECT_0 => Wake::Signalled,
                _ => Wake::TimedOut,
            }
        }
    }

    /// Runs this thread's windows (plugin editors) until `wake` is set or
    /// `timeout` passes: waits for either, then handles every waiting
    /// window message.
    pub fn pump_until(wake: &Event, timeout: Duration) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, MSG, MWMO_INPUTAVAILABLE, MsgWaitForMultipleObjectsEx, PM_REMOVE,
            PeekMessageW, QS_ALLINPUT, TranslateMessage,
        };
        let ms = timeout.as_millis().min(u128::from(INFINITE - 1)) as u32;
        unsafe {
            MsgWaitForMultipleObjectsEx(1, &wake.0.0, ms, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
            let mut msg: MSG = std::mem::zeroed();
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    /// Lets process `pid` bring its windows in front of ours (we are the
    /// active app and it is about to open one for the user).
    pub fn allow_foreground(pid: u32) {
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(pid) };
    }

    /// Windows in this process get real pixels on every screen and are told
    /// when a screen's scale changes (plugin editors scale themselves).
    pub fn dpi_aware() {
        use windows_sys::Win32::UI::HiDpi::{
            DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
        };
        unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    }

    /// A high-resolution one-shot timer (sub-millisecond, unlike wait
    /// timeouts, which round to the system tick of up to ~15.6 ms).
    #[derive(Debug)]
    pub struct Timer(Handle);

    impl Timer {
        pub fn new() -> io::Result<Self> {
            Handle::checked(unsafe {
                CreateWaitableTimerExW(
                    null(),
                    null(),
                    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                    TIMER_ALL_ACCESS,
                )
            })
            .map(Self)
        }

        /// Fires once, `after` from now. Never allocates.
        pub fn arm(&self, after: Duration) {
            // Negative: relative, in 100 ns units.
            let due = -((after.as_nanos() / 100).clamp(1, i64::MAX as u128) as i64);
            unsafe { SetWaitableTimer(self.0.0, &due, 0, None, null(), 0) };
        }
    }

    /// Ends every process assigned to it when the DAW exits, even if it
    /// crashes (the system closes the job's last handle).
    #[derive(Debug)]
    pub struct Job(Handle);

    impl Job {
        pub fn new() -> io::Result<Self> {
            let job = Handle::checked(unsafe { CreateJobObjectW(null(), null()) })?;
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = unsafe {
                SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    (&raw const info).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self(job))
        }

        pub fn assign(&self, child: &std::process::Child) -> io::Result<()> {
            use std::os::windows::io::AsRawHandle;
            if unsafe { AssignProcessToJobObject(self.0.0, child.as_raw_handle()) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
    }
}

#[cfg(not(windows))]
mod unsupported {
    use super::Wake;
    use std::io;
    use std::time::Duration;

    fn unsupported<T>() -> io::Result<T> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "plugin hosting is Windows-only for now",
        ))
    }

    #[derive(Debug)]
    pub struct SharedMemory(());
    impl SharedMemory {
        pub fn create(_: &str, _: usize) -> io::Result<Self> {
            unsupported()
        }
        pub fn open(_: &str, _: usize) -> io::Result<Self> {
            unsupported()
        }
        pub fn as_ptr(&self) -> *mut u8 {
            std::ptr::null_mut()
        }
    }

    #[derive(Debug)]
    pub struct Event(());
    impl Event {
        pub fn create(_: &str) -> io::Result<Self> {
            unsupported()
        }
        pub fn open(_: &str) -> io::Result<Self> {
            unsupported()
        }
        pub fn local() -> io::Result<Self> {
            unsupported()
        }
        pub fn set(&self) {}
        pub fn wait(&self, _: Option<Duration>) -> Wake {
            Wake::TimedOut
        }
        pub fn wait_or(&self, _: &Timer) -> Wake {
            Wake::TimedOut
        }
    }

    #[allow(
        clippy::disallowed_methods,
        reason = "the host's main thread, never an audio thread"
    )]
    pub fn pump_until(_: &Event, timeout: Duration) {
        std::thread::sleep(timeout);
    }

    pub fn allow_foreground(_: u32) {}

    pub fn dpi_aware() {}

    #[derive(Debug)]
    pub struct Timer(());
    impl Timer {
        pub fn new() -> io::Result<Self> {
            unsupported()
        }
        pub fn arm(&self, _: Duration) {}
    }

    #[derive(Debug)]
    pub struct Job(());
    impl Job {
        pub fn new() -> io::Result<Self> {
            unsupported()
        }
        pub fn assign(&self, _: &std::process::Child) -> io::Result<()> {
            unsupported()
        }
    }
}
