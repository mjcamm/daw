//! A tiny VST3 plugin for this repo's tests, built from source so they
//! never depend on what is installed. "Test Gain" is a real effect: a
//! processor and a separate controller (as most plugins are built), one
//! parameter (id 0, gain = 2 × value, so the default 0.5 is unity) that
//! is saved in its state. "Test Synth" is listed but not yet creatable.
//! Adapted from the `vst3` crate's gain example (MIT OR Apache-2.0).
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

use std::cell::Cell;
use std::ffi::{c_char, c_void};
use std::sync::atomic::{AtomicU64, Ordering};
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{Class, ComRef, ComWrapper, uid};

pub const VENDOR: &str = "DAW Tests";
pub const EFFECT_NAME: &str = "Test Gain";
pub const INSTRUMENT_NAME: &str = "Test Synth";
const EFFECT_CID: TUID = uid(0x4441_5754, 0x4741_494E, 0x0000_0000, 0x0000_0001);
const CONTROLLER_CID: TUID = uid(0x4441_5754, 0x4741_494E, 0x0000_0000, 0x0000_00C1);
const INSTRUMENT_CID: TUID = uid(0x4441_5754, 0x5359_4E54, 0x0000_0000, 0x0000_0002);

/// The gain parameter's id and default (normalized).
const GAIN: ParamID = 0;
const DEFAULT_GAIN: f64 = 0.5;

/// (cid, category, name, subcategories) for each class.
const CLASSES: [(TUID, &str, &str, &str); 3] = [
    (EFFECT_CID, "Audio Module Class", EFFECT_NAME, "Fx|Dynamics"),
    (
        CONTROLLER_CID,
        "Component Controller Class",
        EFFECT_NAME,
        "",
    ),
    (
        INSTRUMENT_CID,
        "Audio Module Class",
        INSTRUMENT_NAME,
        "Instrument|Synth",
    ),
];

fn copy(text: &str, to: &mut [c_char]) {
    let n = text.len().min(to.len() - 1);
    for (dst, src) in to.iter_mut().zip(text.bytes().take(n)) {
        *dst = src as c_char;
    }
    to[n] = 0;
}

fn copy_wide(text: &str, to: &mut [TChar]) {
    let units: Vec<u16> = text.encode_utf16().take(to.len() - 1).collect();
    to[..units.len()].copy_from_slice(&units);
    to[units.len()] = 0;
}

/// The gain value as saved in a state stream: 8 bytes, little-endian.
unsafe fn read_gain(stream: *mut IBStream) -> Option<f64> {
    let stream = unsafe { ComRef::from_raw(stream) }?;
    let mut bytes = [0u8; 8];
    let mut read = 0;
    let ok = unsafe { stream.read(bytes.as_mut_ptr().cast(), 8, &mut read) } == kResultOk;
    (ok && read == 8).then(|| f64::from_le_bytes(bytes))
}

unsafe fn write_gain(stream: *mut IBStream, gain: f64) -> tresult {
    let Some(stream) = (unsafe { ComRef::from_raw(stream) }) else {
        return kInvalidArgument;
    };
    let mut bytes = gain.to_le_bytes();
    let mut written = 0;
    unsafe { stream.write(bytes.as_mut_ptr().cast(), 8, &mut written) }
}

struct Processor {
    gain: AtomicU64,
}

impl Class for Processor {
    type Interfaces = (IComponent, IAudioProcessor, IProcessContextRequirements);
}

impl IPluginBaseTrait for Processor {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }

    unsafe fn terminate(&self) -> tresult {
        kResultOk
    }
}

impl IComponentTrait for Processor {
    unsafe fn getControllerClassId(&self, class_id: *mut TUID) -> tresult {
        unsafe { *class_id = CONTROLLER_CID };
        kResultOk
    }

    unsafe fn setIoMode(&self, _mode: IoMode) -> tresult {
        kResultOk
    }

    unsafe fn getBusCount(&self, media: MediaType, _dir: BusDirection) -> int32 {
        i32::from(media as MediaTypes == MediaTypes_::kAudio)
    }

    unsafe fn getBusInfo(
        &self,
        media: MediaType,
        dir: BusDirection,
        index: int32,
        bus: *mut BusInfo,
    ) -> tresult {
        let Some(bus) = (unsafe { bus.as_mut() }) else {
            return kInvalidArgument;
        };
        if media as MediaTypes != MediaTypes_::kAudio || index != 0 {
            return kInvalidArgument;
        }
        bus.mediaType = media;
        bus.direction = dir;
        bus.channelCount = 2;
        copy_wide(
            if dir == BusDirections_::kInput as BusDirection {
                "Input"
            } else {
                "Output"
            },
            &mut bus.name,
        );
        bus.busType = BusTypes_::kMain as BusType;
        bus.flags = BusInfo_::BusFlags_::kDefaultActive as uint32;
        kResultOk
    }

    unsafe fn getRoutingInfo(&self, _in: *mut RoutingInfo, _out: *mut RoutingInfo) -> tresult {
        kNotImplemented
    }

    unsafe fn activateBus(
        &self,
        _media: MediaType,
        _dir: BusDirection,
        _index: int32,
        _state: TBool,
    ) -> tresult {
        kResultOk
    }

    unsafe fn setActive(&self, _state: TBool) -> tresult {
        kResultOk
    }

    unsafe fn setState(&self, state: *mut IBStream) -> tresult {
        match unsafe { read_gain(state) } {
            Some(gain) => {
                self.gain.store(gain.to_bits(), Ordering::Relaxed);
                kResultOk
            }
            None => kResultFalse,
        }
    }

    unsafe fn getState(&self, state: *mut IBStream) -> tresult {
        unsafe { write_gain(state, f64::from_bits(self.gain.load(Ordering::Relaxed))) }
    }
}

impl IAudioProcessorTrait for Processor {
    unsafe fn setBusArrangements(
        &self,
        inputs: *mut SpeakerArrangement,
        num_ins: int32,
        outputs: *mut SpeakerArrangement,
        num_outs: int32,
    ) -> tresult {
        let stereo = num_ins == 1
            && num_outs == 1
            && unsafe { *inputs == SpeakerArr::kStereo && *outputs == SpeakerArr::kStereo };
        if stereo { kResultTrue } else { kResultFalse }
    }

    unsafe fn getBusArrangement(
        &self,
        _dir: BusDirection,
        index: int32,
        arr: *mut SpeakerArrangement,
    ) -> tresult {
        if index != 0 {
            return kInvalidArgument;
        }
        unsafe { *arr = SpeakerArr::kStereo };
        kResultOk
    }

    unsafe fn canProcessSampleSize(&self, size: int32) -> tresult {
        if size as SymbolicSampleSizes == SymbolicSampleSizes_::kSample32 {
            kResultOk
        } else {
            kNotImplemented
        }
    }

    unsafe fn getLatencySamples(&self) -> uint32 {
        0
    }

    unsafe fn setupProcessing(&self, _setup: *mut ProcessSetup) -> tresult {
        kResultOk
    }

    unsafe fn setProcessing(&self, _state: TBool) -> tresult {
        kResultOk
    }

    unsafe fn process(&self, data: *mut ProcessData) -> tresult {
        let Some(data) = (unsafe { data.as_ref() }) else {
            return kInvalidArgument;
        };
        // The last value of each block's gain changes wins (a test plugin
        // need not ramp).
        if let Some(changes) = unsafe { ComRef::from_raw(data.inputParameterChanges) } {
            for i in 0..unsafe { changes.getParameterCount() } {
                let Some(queue) = (unsafe { ComRef::from_raw(changes.getParameterData(i)) }) else {
                    continue;
                };
                let points = unsafe { queue.getPointCount() };
                if unsafe { queue.getParameterId() } == GAIN && points > 0 {
                    let (mut offset, mut value) = (0, 0.0);
                    if unsafe { queue.getPoint(points - 1, &mut offset, &mut value) } == kResultOk {
                        self.gain.store(value.to_bits(), Ordering::Relaxed);
                    }
                }
            }
        }
        if data.numInputs < 1 || data.numOutputs < 1 {
            return kResultOk;
        }
        let gain = (2.0 * f64::from_bits(self.gain.load(Ordering::Relaxed))) as f32;
        let frames = data.numSamples as usize;
        let (input, output) = unsafe { (&*data.inputs, &*data.outputs) };
        let channels = input.numChannels.min(output.numChannels) as usize;
        for ch in 0..channels {
            let (src, dst) = unsafe {
                (
                    std::slice::from_raw_parts(*input.__field0.channelBuffers32.add(ch), frames),
                    std::slice::from_raw_parts_mut(
                        *output.__field0.channelBuffers32.add(ch),
                        frames,
                    ),
                )
            };
            for (d, s) in dst.iter_mut().zip(src) {
                *d = s * gain;
            }
        }
        kResultOk
    }

    unsafe fn getTailSamples(&self) -> uint32 {
        0
    }
}

impl IProcessContextRequirementsTrait for Processor {
    unsafe fn getProcessContextRequirements(&self) -> uint32 {
        0
    }
}

struct Controller {
    gain: Cell<f64>,
}

impl Class for Controller {
    type Interfaces = (IEditController,);
}

impl IPluginBaseTrait for Controller {
    unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
        kResultOk
    }

    unsafe fn terminate(&self) -> tresult {
        kResultOk
    }
}

impl IEditControllerTrait for Controller {
    unsafe fn setComponentState(&self, state: *mut IBStream) -> tresult {
        match unsafe { read_gain(state) } {
            Some(gain) => {
                self.gain.set(gain);
                kResultOk
            }
            None => kResultFalse,
        }
    }

    unsafe fn setState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }

    unsafe fn getState(&self, _state: *mut IBStream) -> tresult {
        kResultOk
    }

    unsafe fn getParameterCount(&self) -> int32 {
        1
    }

    unsafe fn getParameterInfo(&self, index: int32, info: *mut ParameterInfo) -> tresult {
        let Some(info) = (unsafe { info.as_mut() }) else {
            return kInvalidArgument;
        };
        if index != 0 {
            return kInvalidArgument;
        }
        info.id = GAIN;
        copy_wide("Gain", &mut info.title);
        copy_wide("Gain", &mut info.shortTitle);
        copy_wide("x", &mut info.units);
        info.stepCount = 0;
        info.defaultNormalizedValue = DEFAULT_GAIN;
        info.unitId = 0;
        info.flags = ParameterInfo_::ParameterFlags_::kCanAutomate as int32;
        kResultOk
    }

    unsafe fn getParamStringByValue(
        &self,
        id: ParamID,
        value: ParamValue,
        string: *mut String128,
    ) -> tresult {
        let Some(string) = (unsafe { string.as_mut() }) else {
            return kInvalidArgument;
        };
        if id != GAIN {
            return kInvalidArgument;
        }
        copy_wide(&format!("{:.2}", 2.0 * value), string);
        kResultOk
    }

    unsafe fn getParamValueByString(
        &self,
        _id: ParamID,
        _string: *mut TChar,
        _value: *mut ParamValue,
    ) -> tresult {
        kNotImplemented
    }

    unsafe fn normalizedParamToPlain(&self, _id: ParamID, value: ParamValue) -> ParamValue {
        2.0 * value
    }

    unsafe fn plainParamToNormalized(&self, _id: ParamID, plain: ParamValue) -> ParamValue {
        plain / 2.0
    }

    unsafe fn getParamNormalized(&self, id: ParamID) -> ParamValue {
        if id == GAIN { self.gain.get() } else { 0.0 }
    }

    unsafe fn setParamNormalized(&self, id: ParamID, value: ParamValue) -> tresult {
        if id != GAIN {
            return kInvalidArgument;
        }
        self.gain.set(value);
        kResultOk
    }

    unsafe fn setComponentHandler(&self, _handler: *mut IComponentHandler) -> tresult {
        kResultOk
    }

    unsafe fn createView(&self, _name: FIDString) -> *mut IPlugView {
        std::ptr::null_mut()
    }
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
        let (Some((cid, category, name, _)), Some(info)) =
            (CLASSES.get(index as usize), unsafe { info.as_mut() })
        else {
            return kInvalidArgument;
        };
        info.cid = *cid;
        info.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as int32;
        copy(category, &mut info.category);
        copy(name, &mut info.name);
        kResultOk
    }

    unsafe fn createInstance(
        &self,
        cid: FIDString,
        iid: FIDString,
        obj: *mut *mut c_void,
    ) -> tresult {
        let Some(obj) = (unsafe { obj.as_mut() }) else {
            return kInvalidArgument;
        };
        *obj = std::ptr::null_mut();
        let instance = match unsafe { *cid.cast::<TUID>() } {
            EFFECT_CID => ComWrapper::new(Processor {
                gain: AtomicU64::new(DEFAULT_GAIN.to_bits()),
            })
            .to_com_ptr::<FUnknown>(),
            CONTROLLER_CID => ComWrapper::new(Controller {
                gain: Cell::new(DEFAULT_GAIN),
            })
            .to_com_ptr::<FUnknown>(),
            _ => return kNotImplemented,
        };
        let Some(instance) = instance else {
            return kNoInterface;
        };
        let ptr = instance.as_ptr();
        unsafe { ((*(*ptr).vtbl).queryInterface)(ptr, iid.cast::<TUID>().cast_mut(), obj) }
    }
}

impl IPluginFactory2Trait for Factory {
    unsafe fn getClassInfo2(&self, index: int32, info: *mut PClassInfo2) -> tresult {
        let (Some((cid, category, name, subcategories)), Some(info)) =
            (CLASSES.get(index as usize), unsafe { info.as_mut() })
        else {
            return kInvalidArgument;
        };
        info.cid = *cid;
        info.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as int32;
        copy(category, &mut info.category);
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
