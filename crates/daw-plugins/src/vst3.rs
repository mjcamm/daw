//! Loading a VST3 module and reading its classes. Runs only in a scanner
//! or host process, never in the DAW (`docs/vst3-notes.md`, Loading).

#![allow(
    unsafe_code,
    reason = "a plugin module is foreign code reached through raw exports and COM"
)]

use std::ffi::c_char;
use std::path::Path;
use vst3::ComPtr;
use vst3::Steinberg::*;

/// A class a module offers, as its factory describes it.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassInfo {
    pub cid: TUID,
    pub category: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
    /// `|`-separated, e.g. "Fx|Delay" or "Instrument|Synth".
    pub subcategories: String,
}

/// The class id as Steinberg writes it (`moduleinfo.json`, `INLINE_UID`):
/// four 32-bit numbers in hex. Their byte layout in a `TUID` differs on
/// Windows (COM order), which `vst3::uid` and this inverse take care of.
pub fn cid_string(cid: &TUID) -> String {
    let b = cid.map(|x| x as u8);
    let be = |i: usize| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    let (first, second) = if cfg!(windows) {
        (
            u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            u32::from_le_bytes([b[6], b[7], b[4], b[5]]),
        )
    } else {
        (be(0), be(4))
    };
    format!("{first:08X}{second:08X}{:08X}{:08X}", be(8), be(12))
}

/// The inverse of [`cid_string`].
pub fn parse_cid(text: &str) -> Option<TUID> {
    let text = text.trim();
    if text.len() != 32 || !text.is_ascii() {
        return None;
    }
    let part = |i: usize| u32::from_str_radix(&text[i * 8..i * 8 + 8], 16).ok();
    Some(vst3::uid(part(0)?, part(1)?, part(2)?, part(3)?))
}

fn text(chars: &[c_char]) -> String {
    let bytes: Vec<u8> = chars
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn wide(chars: &[char16]) -> String {
    let units: Vec<u16> = chars.iter().take_while(|&&c| c != 0).copied().collect();
    String::from_utf16_lossy(&units)
}

/// A loaded module and its factory. Dropping it releases the factory, then
/// calls the module's exit function and unloads it.
pub struct Module {
    factory: Option<ComPtr<IPluginFactory>>,
    library: sys::Library,
}

impl std::fmt::Debug for Module {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Module")
            .field("library", &self.library)
            .finish()
    }
}

impl Module {
    /// Loads the module's binary (`binary`, inside the bundle or a legacy
    /// single file) and gets its factory.
    pub fn load(binary: &Path) -> Result<Self, String> {
        let library = sys::Library::open(binary)?;
        if !library.enter() {
            return Err("the plugin's entry function failed".into());
        }
        let factory = library.factory()?;
        Ok(Self {
            factory: Some(factory),
            library,
        })
    }

    fn factory(&self) -> &ComPtr<IPluginFactory> {
        // Present until drop.
        self.factory.as_ref().unwrap_or_else(|| unreachable!())
    }

    /// Every class the factory lists, with the richest description it
    /// offers (`IPluginFactory3` names in UTF-16, else 2, else the basics).
    pub fn classes(&self) -> Vec<ClassInfo> {
        let factory = self.factory();
        let second = factory.cast::<IPluginFactory2>();
        let third = factory.cast::<IPluginFactory3>();
        let factory_vendor = unsafe {
            let mut info: PFactoryInfo = std::mem::zeroed();
            if factory.getFactoryInfo(&mut info) == kResultOk {
                text(&info.vendor)
            } else {
                String::new()
            }
        };
        let unicode = |index: int32| -> Option<ClassInfo> {
            let mut info: PClassInfoW = unsafe { std::mem::zeroed() };
            let ok = unsafe { third.as_ref()?.getClassInfoUnicode(index, &mut info) } == kResultOk;
            ok.then(|| ClassInfo {
                cid: info.cid,
                category: text(&info.category),
                name: wide(&info.name),
                vendor: wide(&info.vendor),
                version: wide(&info.version),
                subcategories: text(&info.subCategories),
            })
        };
        let described = |index: int32| -> Option<ClassInfo> {
            let mut info: PClassInfo2 = unsafe { std::mem::zeroed() };
            let ok = unsafe { second.as_ref()?.getClassInfo2(index, &mut info) } == kResultOk;
            ok.then(|| ClassInfo {
                cid: info.cid,
                category: text(&info.category),
                name: text(&info.name),
                vendor: text(&info.vendor),
                version: text(&info.version),
                subcategories: text(&info.subCategories),
            })
        };
        let basic = |index: int32| -> Option<ClassInfo> {
            let mut info: PClassInfo = unsafe { std::mem::zeroed() };
            let ok = unsafe { factory.getClassInfo(index, &mut info) } == kResultOk;
            ok.then(|| ClassInfo {
                cid: info.cid,
                category: text(&info.category),
                name: text(&info.name),
                vendor: String::new(),
                version: String::new(),
                subcategories: String::new(),
            })
        };
        let count = unsafe { factory.countClasses() }.max(0);
        let mut classes = Vec::new();
        for index in 0..count {
            let class = unicode(index)
                .or_else(|| described(index))
                .or_else(|| basic(index));
            if let Some(mut class) = class {
                if class.vendor.is_empty() {
                    class.vendor.clone_from(&factory_vendor);
                }
                classes.push(class);
            }
        }
        classes
    }
}

impl Drop for Module {
    fn drop(&mut self) {
        drop(self.factory.take());
        self.library.exit();
    }
}

/// Stops Windows showing dialogs (a missing DLL, a bad drive) for code a
/// plugin loads: they would wait for a click no one makes.
pub fn quiet_errors() {
    sys::quiet_errors();
}

#[cfg(windows)]
mod sys {
    use super::{ComPtr, IPluginFactory};
    use std::path::Path;
    use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE};
    use windows_sys::Win32::System::Diagnostics::Debug::{
        SEM_FAILCRITICALERRORS, SEM_NOOPENFILEERRORBOX, SetErrorMode,
    };
    use windows_sys::Win32::System::LibraryLoader::{
        GetProcAddress, LOAD_WITH_ALTERED_SEARCH_PATH, LoadLibraryExW,
    };

    #[derive(Debug)]
    pub struct Library(HMODULE);

    /// What `GetProcAddress` returns, before it is given its real type.
    type Symbol = unsafe extern "system" fn() -> isize;
    type Entry = unsafe extern "system" fn() -> bool;
    type GetFactory = unsafe extern "system" fn() -> *mut IPluginFactory;

    impl Library {
        pub fn open(path: &Path) -> Result<Self, String> {
            use std::os::windows::ffi::OsStrExt;
            let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
            // Its own folder first, so the plugin's DLLs beside it load.
            let module = unsafe {
                LoadLibraryExW(
                    wide.as_ptr(),
                    std::ptr::null_mut(),
                    LOAD_WITH_ALTERED_SEARCH_PATH,
                )
            };
            if module.is_null() {
                return Err(format!(
                    "could not load it: {}",
                    std::io::Error::last_os_error()
                ));
            }
            Ok(Self(module))
        }

        fn symbol(&self, name: &[u8]) -> Option<Symbol> {
            unsafe { GetProcAddress(self.0, name.as_ptr()) }
        }

        /// `InitDll`, if the module has one (optional on Windows).
        pub fn enter(&self) -> bool {
            match self.symbol(b"InitDll\0") {
                Some(f) => unsafe { std::mem::transmute::<Symbol, Entry>(f)() },
                None => true,
            }
        }

        pub fn exit(&self) {
            if let Some(f) = self.symbol(b"ExitDll\0") {
                unsafe { std::mem::transmute::<Symbol, Entry>(f)() };
            }
        }

        pub fn factory(&self) -> Result<ComPtr<IPluginFactory>, String> {
            let f = self
                .symbol(b"GetPluginFactory\0")
                .ok_or("not a VST3 plugin (no GetPluginFactory)")?;
            let raw = unsafe { std::mem::transmute::<Symbol, GetFactory>(f)() };
            // The factory comes with a reference for the caller.
            unsafe { ComPtr::from_raw(raw) }.ok_or_else(|| "the plugin gave no factory".into())
        }
    }

    impl Drop for Library {
        fn drop(&mut self) {
            unsafe { FreeLibrary(self.0) };
        }
    }

    pub fn quiet_errors() {
        unsafe { SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOOPENFILEERRORBOX) };
    }
}

#[cfg(not(windows))]
mod sys {
    use super::{ComPtr, IPluginFactory};
    use std::path::Path;

    #[derive(Debug)]
    pub struct Library(());

    impl Library {
        pub fn open(_: &Path) -> Result<Self, String> {
            Err("VST3 loading is Windows-only for now".into())
        }
        pub fn enter(&self) -> bool {
            false
        }
        pub fn exit(&self) {}
        pub fn factory(&self) -> Result<ComPtr<IPluginFactory>, String> {
            Err("VST3 loading is Windows-only for now".into())
        }
    }

    pub fn quiet_errors() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_ids_round_trip_through_steinbergs_text_form() {
        let cid = vst3::uid(0x4441_5754, 0x4741_494E, 0x0102_0304, 0xA0B0_C0D0);
        let text = cid_string(&cid);
        assert_eq!(text, "444157544741494E01020304A0B0C0D0");
        assert_eq!(parse_cid(&text), Some(cid));
        assert_eq!(parse_cid("not a class id"), None);
        assert_eq!(
            parse_cid(&text.to_lowercase()),
            Some(cid),
            "hex in either case"
        );
    }
}
