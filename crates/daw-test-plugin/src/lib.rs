//! A tiny VST3 plugin for this repo's tests, built from source so they
//! never depend on what is installed. It describes an effect and an
//! instrument class; creating instances comes with the audio step.
//!
//! `DAW_TEST_PLUGIN_SCAN=crash` or `hang` makes loading it crash or hang,
//! to test that scanning survives both.

#![allow(
    unsafe_code,
    non_snake_case,
    reason = "a VST3 module's exports and COM methods are unsafe and named by the SDK"
)]
#![allow(
    clippy::disallowed_methods,
    reason = "the hang switch sleeps forever on purpose"
)]

use std::ffi::{c_char, c_void};
use vst3::Steinberg::*;
use vst3::{Class, ComWrapper, uid};

/// Shown in hosts and asserted by tests.
pub const VENDOR: &str = "DAW Tests";
pub const EFFECT_NAME: &str = "Test Gain";
pub const INSTRUMENT_NAME: &str = "Test Synth";
const EFFECT_CID: TUID = uid(0x4441_5754, 0x4741_494E, 0x0000_0000, 0x0000_0001);
const INSTRUMENT_CID: TUID = uid(0x4441_5754, 0x5359_4E54, 0x0000_0000, 0x0000_0002);

/// (cid, name, subcategories) for each class.
const CLASSES: [(TUID, &str, &str); 2] = [
    (EFFECT_CID, EFFECT_NAME, "Fx|Dynamics"),
    (INSTRUMENT_CID, INSTRUMENT_NAME, "Instrument|Synth"),
];

fn copy(text: &str, to: &mut [c_char]) {
    let n = text.len().min(to.len() - 1);
    for (dst, src) in to.iter_mut().zip(text.bytes().take(n)) {
        *dst = src as c_char;
    }
    to[n] = 0;
}

struct Factory;

impl Class for Factory {
    type Interfaces = (IPluginFactory2,);
}

impl IPluginFactoryTrait for Factory {
    unsafe fn getFactoryInfo(&self, info: *mut PFactoryInfo) -> tresult {
        let Some(info) = (unsafe { info.as_mut() }) else {
            return kInvalidArgument;
        };
        copy(VENDOR, &mut info.vendor);
        copy("", &mut info.url);
        copy("", &mut info.email);
        info.flags = 0;
        kResultOk
    }

    unsafe fn countClasses(&self) -> int32 {
        CLASSES.len() as int32
    }

    unsafe fn getClassInfo(&self, index: int32, info: *mut PClassInfo) -> tresult {
        let (Some((cid, name, _)), Some(info)) =
            (CLASSES.get(index as usize), unsafe { info.as_mut() })
        else {
            return kInvalidArgument;
        };
        info.cid = *cid;
        info.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as int32;
        copy("Audio Module Class", &mut info.category);
        copy(name, &mut info.name);
        kResultOk
    }

    unsafe fn createInstance(
        &self,
        _cid: FIDString,
        _iid: FIDString,
        obj: *mut *mut c_void,
    ) -> tresult {
        if let Some(obj) = unsafe { obj.as_mut() } {
            *obj = std::ptr::null_mut();
        }
        kNotImplemented
    }
}

impl IPluginFactory2Trait for Factory {
    unsafe fn getClassInfo2(&self, index: int32, info: *mut PClassInfo2) -> tresult {
        let (Some((cid, name, subcategories)), Some(info)) =
            (CLASSES.get(index as usize), unsafe { info.as_mut() })
        else {
            return kInvalidArgument;
        };
        info.cid = *cid;
        info.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as int32;
        copy("Audio Module Class", &mut info.category);
        copy(name, &mut info.name);
        info.classFlags = 0;
        copy(subcategories, &mut info.subCategories);
        copy(VENDOR, &mut info.vendor);
        copy("1.0.0", &mut info.version);
        copy("VST 3.8.0", &mut info.sdkVersion);
        kResultOk
    }
}

#[unsafe(no_mangle)]
extern "system" fn GetPluginFactory() -> *mut IPluginFactory {
    match std::env::var("DAW_TEST_PLUGIN_SCAN").as_deref() {
        Ok("crash") => std::process::abort(),
        Ok("hang") => loop {
            std::thread::sleep(std::time::Duration::from_secs(60));
        },
        _ => {}
    }
    ComWrapper::new(Factory)
        .to_com_ptr::<IPluginFactory>()
        .map_or(std::ptr::null_mut(), |p| p.into_raw())
}

#[cfg(windows)]
#[unsafe(no_mangle)]
extern "system" fn InitDll() -> bool {
    true
}

#[cfg(windows)]
#[unsafe(no_mangle)]
extern "system" fn ExitDll() -> bool {
    true
}

#[cfg(target_os = "macos")]
#[unsafe(no_mangle)]
extern "system" fn bundleEntry(_bundle: *mut c_void) -> bool {
    true
}

#[cfg(target_os = "macos")]
#[unsafe(no_mangle)]
extern "system" fn bundleExit() -> bool {
    true
}

#[cfg(target_os = "linux")]
#[unsafe(no_mangle)]
extern "system" fn ModuleEntry(_library: *mut c_void) -> bool {
    true
}

#[cfg(target_os = "linux")]
#[unsafe(no_mangle)]
extern "system" fn ModuleExit() -> bool {
    true
}
