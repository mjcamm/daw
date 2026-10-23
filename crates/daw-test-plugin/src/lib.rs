//! A tiny VST3 plugin for this repo's tests, built from source so they
//! never depend on what is installed. "Test Gain" is a real effect: a
//! processor and a separate controller (as most plugins are built), one
//! parameter that does something (id 0, gain = 2 × value, so the default
//! 0.5 is unity) and is saved in its state, "Tilt" (id 7, in the "Tone"
//! group, does nothing) and a read-only "Meter" (id 9) a host must not
//! offer. It has a window (`view.rs`) that tests drive. "Test Synth" is an
//! instrument: each note plays a cosine at its pitch, `velocity × 0.2 ×
//! 2 × Gain` loud, from its exact sample until its end (no envelope), so
//! tests can check timing; it shares the controller.
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

#[cfg(windows)]
mod view;
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
/// A parameter in a group, and a read-only one.
const TILT: ParamID = 7;
const METER: ParamID = 9;
const TONE_UNIT: UnitID = 1;

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
    /// "Test Synth": notes in, sound out.
    synth: Option<Synth>,
}

/// A sounding note: its id, frequency, loudness and samples since it began.
#[derive(Clone, Copy)]
struct Voice {
    note: i32,
    hz: f64,
    amp: f64,
    age: u64,
}

struct Synth {
    voices: std::cell::UnsafeCell<[Option<Voice>; 32]>,
    rate: AtomicU64,
}

// The voices are touched only by `process`, on the host's audio thread.
unsafe impl Sync for Synth {}

impl Synth {
    fn new() -> Self {
        Self {
            voices: std::cell::UnsafeCell::new([None; 32]),
            rate: AtomicU64::new(48_000f64.to_bits()),
        }
    }

    fn start(voices: &mut [Option<Voice>; 32], note: &NoteOnEvent) {
        if let Some(free) = voices.iter_mut().find(|v| v.is_none()) {
            *free = Some(Voice {
                note: note.noteId,
                hz: 440.0 * 2f64.powf((f64::from(note.pitch) - 69.0) / 12.0),
                amp: f64::from(note.velocity) * 0.2,
                age: 0,
            });
        }
    }

    fn stop(voices: &mut [Option<Voice>; 32], note: &NoteOffEvent) {
        for voice in voices.iter_mut() {
            if voice.is_some_and(|v| v.note == note.noteId) {
                *voice = None;
            }
        }
    }

    unsafe fn process(&self, data: &ProcessData, gain: f64) {
        let voices = unsafe { &mut *self.voices.get() };
        let rate = f64::from_bits(self.rate.load(Ordering::Relaxed));
        let frames = data.numSamples.max(0) as usize;
        // Fixed room: a plugin never allocates while processing.
        let mut room: [Event; 64] = unsafe { std::mem::zeroed() };
        let mut count = 0;
        if let Some(list) = unsafe { ComRef::from_raw(data.inputEvents) } {
            for i in 0..unsafe { list.getEventCount() } {
                if count < room.len() && unsafe { list.getEvent(i, &mut room[count]) } == kResultOk
                {
                    count += 1;
                }
            }
        }
        let events = &room[..count];
        if data.numOutputs < 1 {
            return;
        }
        let output = unsafe { &*data.outputs };
        let channels = output.numChannels.max(0) as usize;
        let mut next = 0;
        for frame in 0..frames {
            while let Some(event) = events.get(next) {
                if event.sampleOffset as usize > frame {
                    break;
                }
                match i32::from(event.r#type) {
                    Event_::EventTypes_::kNoteOnEvent => {
                        Self::start(voices, unsafe { &event.__field0.noteOn });
                    }
                    Event_::EventTypes_::kNoteOffEvent => {
                        Self::stop(voices, unsafe { &event.__field0.noteOff });
                    }
                    _ => {}
                }
                next += 1;
            }
            let mut sum = 0.0;
            for voice in voices.iter_mut().flatten() {
                let t = voice.age as f64 / rate;
                sum += voice.amp * (std::f64::consts::TAU * voice.hz * t).cos();
                voice.age += 1;
            }
            let value = (sum * gain) as f32;
            for ch in 0..channels {
                unsafe { *(*output.__field0.channelBuffers32.add(ch)).add(frame) = value };
            }
        }
    }
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

    unsafe fn getBusCount(&self, media: MediaType, dir: BusDirection) -> int32 {
        let audio = media as MediaTypes == MediaTypes_::kAudio;
        if self.synth.is_none() {
            return i32::from(audio);
        }
        // An instrument: notes in, sound out.
        let input = dir == BusDirections_::kInput as BusDirection;
        i32::from(if audio { !input } else { input })
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
        if index != 0 || unsafe { self.getBusCount(media, dir) } == 0 {
            return kInvalidArgument;
        }
        if media as MediaTypes == MediaTypes_::kEvent {
            bus.mediaType = media;
            bus.direction = dir;
            bus.channelCount = 1;
            copy_wide("Notes", &mut bus.name);
            bus.busType = BusTypes_::kMain as BusType;
            bus.flags = BusInfo_::BusFlags_::kDefaultActive as uint32;
            return kResultOk;
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
        let stereo = if self.synth.is_some() {
            num_ins == 0 && num_outs == 1 && unsafe { *outputs == SpeakerArr::kStereo }
        } else {
            num_ins == 1
                && num_outs == 1
                && unsafe { *inputs == SpeakerArr::kStereo && *outputs == SpeakerArr::kStereo }
        };
        let _ = inputs;
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

    unsafe fn setupProcessing(&self, setup: *mut ProcessSetup) -> tresult {
        if let (Some(synth), Some(setup)) = (&self.synth, unsafe { setup.as_ref() }) {
            synth
                .rate
                .store(setup.sampleRate.to_bits(), Ordering::Relaxed);
        }
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
        if let Some(synth) = &self.synth {
            let gain = 2.0 * f64::from_bits(self.gain.load(Ordering::Relaxed));
            unsafe { synth.process(data, gain) };
            return kResultOk;
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
    handler: std::cell::RefCell<Option<vst3::ComPtr<IComponentHandler>>>,
}

impl Class for Controller {
    type Interfaces = (IEditController, IUnitInfo);
}

impl IUnitInfoTrait for Controller {
    unsafe fn getUnitCount(&self) -> int32 {
        2
    }

    unsafe fn getUnitInfo(&self, index: int32, info: *mut UnitInfo) -> tresult {
        let Some(info) = (unsafe { info.as_mut() }) else {
            return kInvalidArgument;
        };
        let (id, parent, name) = match index {
            0 => (kRootUnitId, kNoParentUnitId, "Root"),
            1 => (TONE_UNIT, kRootUnitId, "Tone"),
            _ => return kInvalidArgument,
        };
        info.id = id;
        info.parentUnitId = parent;
        copy_wide(name, &mut info.name);
        info.programListId = kNoProgramListId;
        kResultOk
    }

    unsafe fn getProgramListCount(&self) -> int32 {
        0
    }

    unsafe fn getProgramListInfo(&self, _index: int32, _info: *mut ProgramListInfo) -> tresult {
        kInvalidArgument
    }

    unsafe fn getProgramName(
        &self,
        _list: ProgramListID,
        _index: int32,
        _name: *mut String128,
    ) -> tresult {
        kInvalidArgument
    }

    unsafe fn getProgramInfo(
        &self,
        _list: ProgramListID,
        _index: int32,
        _attribute: *const std::ffi::c_char,
        _value: *mut String128,
    ) -> tresult {
        kInvalidArgument
    }

    unsafe fn hasProgramPitchNames(&self, _list: ProgramListID, _index: int32) -> tresult {
        kResultFalse
    }

    unsafe fn getProgramPitchName(
        &self,
        _list: ProgramListID,
        _index: int32,
        _pitch: int16,
        _name: *mut String128,
    ) -> tresult {
        kResultFalse
    }

    unsafe fn getSelectedUnit(&self) -> UnitID {
        kRootUnitId
    }

    unsafe fn selectUnit(&self, _unit: UnitID) -> tresult {
        kResultOk
    }

    unsafe fn getUnitByBus(
        &self,
        _kind: MediaType,
        _dir: BusDirection,
        _bus: int32,
        _channel: int32,
        _unit: *mut UnitID,
    ) -> tresult {
        kResultFalse
    }

    unsafe fn setUnitProgramData(
        &self,
        _list_or_unit: int32,
        _index: int32,
        _data: *mut IBStream,
    ) -> tresult {
        kNotImplemented
    }
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
        3
    }

    unsafe fn getParameterInfo(&self, index: int32, info: *mut ParameterInfo) -> tresult {
        let Some(info) = (unsafe { info.as_mut() }) else {
            return kInvalidArgument;
        };
        use ParameterInfo_::ParameterFlags_::{kCanAutomate, kIsReadOnly};
        let (id, name, units, unit, flags) = match index {
            0 => (GAIN, "Gain", "x", kRootUnitId, kCanAutomate),
            1 => (TILT, "Tilt", "dB", TONE_UNIT, kCanAutomate),
            2 => (METER, "Meter", "", kRootUnitId, kIsReadOnly),
            _ => return kInvalidArgument,
        };
        info.id = id;
        copy_wide(name, &mut info.title);
        copy_wide(name, &mut info.shortTitle);
        copy_wide(units, &mut info.units);
        info.stepCount = 0;
        info.defaultNormalizedValue = if id == GAIN { DEFAULT_GAIN } else { 0.0 };
        info.unitId = unit;
        info.flags = flags as int32;
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

    unsafe fn setComponentHandler(&self, handler: *mut IComponentHandler) -> tresult {
        *self.handler.borrow_mut() = unsafe { ComRef::from_raw(handler) }.map(|h| h.to_com_ptr());
        kResultOk
    }

    #[cfg(windows)]
    unsafe fn createView(&self, name: FIDString) -> *mut IPlugView {
        if name.is_null() || unsafe { std::ffi::CStr::from_ptr(name) } != c"editor" {
            return std::ptr::null_mut();
        }
        view::View::create(self.handler.borrow().clone())
    }

    #[cfg(not(windows))]
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
                synth: None,
            })
            .to_com_ptr::<FUnknown>(),
            INSTRUMENT_CID => ComWrapper::new(Processor {
                gain: AtomicU64::new(DEFAULT_GAIN.to_bits()),
                synth: Some(Synth::new()),
            })
            .to_com_ptr::<FUnknown>(),
            CONTROLLER_CID => ComWrapper::new(Controller {
                gain: Cell::new(DEFAULT_GAIN),
                handler: std::cell::RefCell::new(None),
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
