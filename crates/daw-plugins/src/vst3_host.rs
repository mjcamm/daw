//! Running a VST3 plugin in the host process: the interfaces a plugin
//! calls back into (host application, messages, edit handler, state
//! streams, parameter changes), and [`Vst3Instance`], which follows
//! Steinberg's call order (`docs/vst3-notes.md`). Everything but
//! [`Vst3Instance::process`] runs on the host process's main thread;
//! `process` runs on the instance's audio thread and never allocates.

#![allow(
    unsafe_code,
    non_snake_case,
    reason = "plugins are reached through COM; the method names are the SDK's"
)]

use crate::block::{HostBlock, MAX_CHANNELS, MAX_PARAM_CHANGES, ParamChange};
use crate::vst3::{Module, parse_cid};
use daw_protocol::PluginData;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::{CStr, c_void};
use std::sync::Arc;
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::{Class, ComPtr, ComWrapper, Interface};

fn copy_wide(text: &str, to: &mut [TChar]) {
    let units: Vec<u16> = text.encode_utf16().take(to.len() - 1).collect();
    to[..units.len()].copy_from_slice(&units);
    to[units.len()] = 0;
}

/// Who the plugin is talking to.
struct HostApplication;

impl Class for HostApplication {
    type Interfaces = (IHostApplication,);
}

impl IHostApplicationTrait for HostApplication {
    unsafe fn getName(&self, name: *mut String128) -> tresult {
        match unsafe { name.as_mut() } {
            Some(name) => {
                copy_wide("DAW", name);
                kResultOk
            }
            None => kInvalidArgument,
        }
    }

    /// Plugins built on Steinberg's SDK ask the host for message objects
    /// to talk between their processor and controller.
    unsafe fn createInstance(
        &self,
        cid: *mut TUID,
        iid: *mut TUID,
        obj: *mut *mut c_void,
    ) -> tresult {
        let (Some(cid), Some(obj)) = (unsafe { cid.as_ref() }, unsafe { obj.as_mut() }) else {
            return kInvalidArgument;
        };
        *obj = std::ptr::null_mut();
        let wanted = cid.map(|b| b as u8);
        let instance = if wanted == IMessage::IID {
            ComWrapper::new(Message::default()).to_com_ptr::<FUnknown>()
        } else if wanted == IAttributeList::IID {
            ComWrapper::new(AttributeList::default()).to_com_ptr::<FUnknown>()
        } else {
            return kNoInterface;
        };
        let Some(instance) = instance else {
            return kNoInterface;
        };
        let ptr = instance.as_ptr();
        unsafe { ((*(*ptr).vtbl).queryInterface)(ptr, iid, obj) }
    }
}

#[derive(Default)]
struct Message {
    id: RefCell<Option<std::ffi::CString>>,
    attributes: RefCell<Option<ComPtr<IAttributeList>>>,
}

impl Class for Message {
    type Interfaces = (IMessage,);
}

impl IMessageTrait for Message {
    unsafe fn getMessageID(&self) -> FIDString {
        self.id
            .borrow()
            .as_ref()
            .map_or(std::ptr::null(), |id| id.as_ptr())
    }

    unsafe fn setMessageID(&self, id: FIDString) {
        let id = (!id.is_null()).then(|| unsafe { CStr::from_ptr(id) }.to_owned());
        *self.id.borrow_mut() = id;
    }

    unsafe fn getAttributes(&self) -> *mut IAttributeList {
        let mut attributes = self.attributes.borrow_mut();
        if attributes.is_none() {
            *attributes = ComWrapper::new(AttributeList::default()).to_com_ptr::<IAttributeList>();
        }
        attributes
            .as_ref()
            .map_or(std::ptr::null_mut(), ComPtr::as_ptr)
    }
}

enum Attribute {
    Int(i64),
    Float(f64),
    Text(Vec<u16>),
    Binary(Vec<u8>),
}

#[derive(Default)]
struct AttributeList {
    values: RefCell<HashMap<Vec<u8>, Attribute>>,
}

impl Class for AttributeList {
    type Interfaces = (IAttributeList,);
}

fn key(id: IAttributeList_::AttrID) -> Option<Vec<u8>> {
    (!id.is_null()).then(|| unsafe { CStr::from_ptr(id) }.to_bytes().to_vec())
}

impl IAttributeListTrait for AttributeList {
    unsafe fn setInt(&self, id: IAttributeList_::AttrID, value: int64) -> tresult {
        let Some(key) = key(id) else {
            return kInvalidArgument;
        };
        self.values.borrow_mut().insert(key, Attribute::Int(value));
        kResultOk
    }

    unsafe fn getInt(&self, id: IAttributeList_::AttrID, value: *mut int64) -> tresult {
        match (
            key(id).and_then(|k| match self.values.borrow().get(&k) {
                Some(Attribute::Int(v)) => Some(*v),
                _ => None,
            }),
            unsafe { value.as_mut() },
        ) {
            (Some(v), Some(out)) => {
                *out = v;
                kResultOk
            }
            _ => kResultFalse,
        }
    }

    unsafe fn setFloat(&self, id: IAttributeList_::AttrID, value: f64) -> tresult {
        let Some(key) = key(id) else {
            return kInvalidArgument;
        };
        self.values
            .borrow_mut()
            .insert(key, Attribute::Float(value));
        kResultOk
    }

    unsafe fn getFloat(&self, id: IAttributeList_::AttrID, value: *mut f64) -> tresult {
        match (
            key(id).and_then(|k| match self.values.borrow().get(&k) {
                Some(Attribute::Float(v)) => Some(*v),
                _ => None,
            }),
            unsafe { value.as_mut() },
        ) {
            (Some(v), Some(out)) => {
                *out = v;
                kResultOk
            }
            _ => kResultFalse,
        }
    }

    unsafe fn setString(&self, id: IAttributeList_::AttrID, string: *const TChar) -> tresult {
        let Some(key) = key(id) else {
            return kInvalidArgument;
        };
        if string.is_null() {
            return kInvalidArgument;
        }
        let mut text = Vec::new();
        let mut i = 0;
        while unsafe { *string.add(i) } != 0 {
            text.push(unsafe { *string.add(i) });
            i += 1;
        }
        self.values.borrow_mut().insert(key, Attribute::Text(text));
        kResultOk
    }

    unsafe fn getString(
        &self,
        id: IAttributeList_::AttrID,
        string: *mut TChar,
        size_in_bytes: uint32,
    ) -> tresult {
        let values = self.values.borrow();
        let Some(Attribute::Text(text)) = key(id).and_then(|k| values.get(&k)) else {
            return kResultFalse;
        };
        let room = size_in_bytes as usize / size_of::<TChar>();
        if string.is_null() || room == 0 {
            return kInvalidArgument;
        }
        let n = text.len().min(room - 1);
        unsafe {
            std::ptr::copy_nonoverlapping(text.as_ptr(), string, n);
            *string.add(n) = 0;
        }
        kResultOk
    }

    unsafe fn setBinary(
        &self,
        id: IAttributeList_::AttrID,
        data: *const c_void,
        size_in_bytes: uint32,
    ) -> tresult {
        let Some(key) = key(id) else {
            return kInvalidArgument;
        };
        let bytes = if data.is_null() {
            Vec::new()
        } else {
            unsafe { std::slice::from_raw_parts(data.cast::<u8>(), size_in_bytes as usize) }
                .to_vec()
        };
        self.values
            .borrow_mut()
            .insert(key, Attribute::Binary(bytes));
        kResultOk
    }

    unsafe fn getBinary(
        &self,
        id: IAttributeList_::AttrID,
        data: *mut *const c_void,
        size_in_bytes: *mut uint32,
    ) -> tresult {
        let values = self.values.borrow();
        let Some(Attribute::Binary(bytes)) = key(id).and_then(|k| values.get(&k)) else {
            return kResultFalse;
        };
        let (Some(data), Some(size)) =
            (unsafe { data.as_mut() }, unsafe { size_in_bytes.as_mut() })
        else {
            return kInvalidArgument;
        };
        // Stays valid while the list lives and this value is not replaced.
        *data = bytes.as_ptr().cast();
        *size = bytes.len() as uint32;
        kResultOk
    }
}

/// Receives edits from the plugin's own controls. Accepted for now;
/// knob moves and learn come with plugin windows (`vst3-hosting.md`).
struct ComponentHandler;

impl Class for ComponentHandler {
    type Interfaces = (IComponentHandler,);
}

impl IComponentHandlerTrait for ComponentHandler {
    unsafe fn beginEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }

    unsafe fn performEdit(&self, _id: ParamID, _value: ParamValue) -> tresult {
        kResultOk
    }

    unsafe fn endEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }

    unsafe fn restartComponent(&self, _flags: int32) -> tresult {
        kResultOk
    }
}

/// A plugin's state in memory (`IBStream`).
#[derive(Debug, Default)]
pub struct MemoryStream {
    data: RefCell<Vec<u8>>,
    position: Cell<usize>,
}

impl Class for MemoryStream {
    type Interfaces = (IBStream,);
}

impl MemoryStream {
    pub fn with(data: Vec<u8>) -> Self {
        Self {
            data: RefCell::new(data),
            position: Cell::new(0),
        }
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.data.into_inner()
    }
}

impl IBStreamTrait for MemoryStream {
    unsafe fn read(&self, buffer: *mut c_void, bytes: int32, read: *mut int32) -> tresult {
        let data = self.data.borrow();
        let at = self.position.get().min(data.len());
        let n = (bytes.max(0) as usize).min(data.len() - at);
        if n > 0 {
            if buffer.is_null() {
                return kInvalidArgument;
            }
            unsafe { std::ptr::copy_nonoverlapping(data[at..].as_ptr(), buffer.cast(), n) };
        }
        self.position.set(at + n);
        if let Some(read) = unsafe { read.as_mut() } {
            *read = n as int32;
        }
        kResultOk
    }

    unsafe fn write(&self, buffer: *mut c_void, bytes: int32, written: *mut int32) -> tresult {
        let n = bytes.max(0) as usize;
        if n > 0 && buffer.is_null() {
            return kInvalidArgument;
        }
        let mut data = self.data.borrow_mut();
        let at = self.position.get();
        if data.len() < at + n {
            data.resize(at + n, 0);
        }
        if n > 0 {
            unsafe {
                std::ptr::copy_nonoverlapping(buffer.cast::<u8>(), data[at..].as_mut_ptr(), n)
            };
        }
        self.position.set(at + n);
        if let Some(written) = unsafe { written.as_mut() } {
            *written = n as int32;
        }
        kResultOk
    }

    unsafe fn seek(&self, pos: int64, mode: int32, result: *mut int64) -> tresult {
        let base = match mode as u32 {
            m if m == IBStream_::IStreamSeekMode_::kIBSeekSet as u32 => 0,
            m if m == IBStream_::IStreamSeekMode_::kIBSeekCur as u32 => self.position.get() as i64,
            m if m == IBStream_::IStreamSeekMode_::kIBSeekEnd as u32 => {
                self.data.borrow().len() as i64
            }
            _ => return kInvalidArgument,
        };
        let to = (base + pos).max(0) as usize;
        self.position.set(to);
        if let Some(result) = unsafe { result.as_mut() } {
            *result = to as int64;
        }
        kResultOk
    }

    unsafe fn tell(&self, pos: *mut int64) -> tresult {
        match unsafe { pos.as_mut() } {
            Some(pos) => {
                *pos = self.position.get() as int64;
                kResultOk
            }
            None => kInvalidArgument,
        }
    }
}

/// Points a queue can hold in one block.
const MAX_POINTS: usize = 64;
/// Parameters that can change in one block.
const MAX_QUEUES: usize = 128;

/// One parameter's changes in a block, read by the plugin. Filled on the
/// audio thread from preallocated room.
struct ParamValueQueue {
    id: Cell<ParamID>,
    points: [Cell<(int32, f64)>; MAX_POINTS],
    count: Cell<usize>,
}

impl Class for ParamValueQueue {
    type Interfaces = (IParamValueQueue,);
}

impl IParamValueQueueTrait for ParamValueQueue {
    unsafe fn getParameterId(&self) -> ParamID {
        self.id.get()
    }

    unsafe fn getPointCount(&self) -> int32 {
        self.count.get() as int32
    }

    unsafe fn getPoint(&self, index: int32, offset: *mut int32, value: *mut ParamValue) -> tresult {
        let Some(point) = usize::try_from(index)
            .ok()
            .filter(|&i| i < self.count.get())
            .map(|i| self.points[i].get())
        else {
            return kInvalidArgument;
        };
        match (unsafe { offset.as_mut() }, unsafe { value.as_mut() }) {
            (Some(o), Some(v)) => {
                (*o, *v) = point;
                kResultOk
            }
            _ => kInvalidArgument,
        }
    }

    unsafe fn addPoint(&self, _offset: int32, _value: ParamValue, _index: *mut int32) -> tresult {
        kResultFalse
    }
}

/// A block's parameter changes, one queue per parameter (VST3 asks each
/// id to appear once), points in order.
struct ParameterChanges {
    queues: Vec<ComWrapper<ParamValueQueue>>,
    pointers: Vec<ComPtr<IParamValueQueue>>,
    count: Cell<usize>,
}

impl Class for ParameterChanges {
    type Interfaces = (IParameterChanges,);
}

impl IParameterChangesTrait for ParameterChanges {
    unsafe fn getParameterCount(&self) -> int32 {
        self.count.get() as int32
    }

    unsafe fn getParameterData(&self, index: int32) -> *mut IParamValueQueue {
        usize::try_from(index)
            .ok()
            .filter(|&i| i < self.count.get())
            .and_then(|i| self.pointers.get(i))
            .map_or(std::ptr::null_mut(), ComPtr::as_ptr)
    }

    unsafe fn addParameterData(
        &self,
        _id: *const ParamID,
        _index: *mut int32,
    ) -> *mut IParamValueQueue {
        std::ptr::null_mut()
    }
}

impl ParameterChanges {
    fn new() -> Self {
        let queues: Vec<ComWrapper<ParamValueQueue>> = (0..MAX_QUEUES)
            .map(|_| {
                ComWrapper::new(ParamValueQueue {
                    id: Cell::new(0),
                    points: std::array::from_fn(|_| Cell::new((0, 0.0))),
                    count: Cell::new(0),
                })
            })
            .collect();
        let pointers = queues
            .iter()
            .filter_map(|q| q.to_com_ptr::<IParamValueQueue>())
            .collect();
        Self {
            queues,
            pointers,
            count: Cell::new(0),
        }
    }

    /// Takes a block's changes (in offset order per id). Allocation-free.
    fn fill(&self, changes: &[ParamChange]) {
        self.count.set(0);
        for change in changes {
            let used = self.count.get();
            let found = self.queues[..used]
                .iter()
                .position(|q| q.id.get() == change.id);
            let index = match found {
                Some(i) => i,
                None if used < self.queues.len() => {
                    self.queues[used].id.set(change.id);
                    self.queues[used].count.set(0);
                    self.count.set(used + 1);
                    used
                }
                None => continue,
            };
            let queue = &self.queues[index];
            let n = queue.count.get();
            if n < MAX_POINTS {
                queue.points[n].set((change.offset as int32, change.value));
                queue.count.set(n + 1);
            }
        }
    }
}

const _: () = assert!(MAX_QUEUES * MAX_POINTS >= MAX_PARAM_CHANGES);

/// The speaker channels in an arrangement.
fn channels(arrangement: SpeakerArrangement) -> usize {
    arrangement.count_ones() as usize
}

/// An audio bus's buffers as the plugin sees them: the main bus points into
/// the shared block, others into silent scratch.
struct Bus {
    channels: usize,
    scratch: Vec<Vec<f32>>,
    pointers: [*mut f32; MAX_CHANNELS],
}

/// A plugin instance (processor, and its controller).
pub struct Vst3Instance {
    component: ComPtr<IComponent>,
    processor: ComPtr<IAudioProcessor>,
    controller: Option<ComPtr<IEditController>>,
    /// Processor and controller linked both ways, when they are separate.
    connection: Option<(ComPtr<IConnectionPoint>, ComPtr<IConnectionPoint>)>,
    changes: ComWrapper<ParameterChanges>,
    changes_ptr: ComPtr<IParameterChanges>,
    inputs: Vec<Bus>,
    outputs: Vec<Bus>,
    in_buses: Vec<AudioBusBuffers>,
    out_buses: Vec<AudioBusBuffers>,
    context: ProcessContext,
    sample_rate: f64,
    active: bool,
    processing: bool,
    _handler: ComWrapper<ComponentHandler>,
    _host: ComWrapper<HostApplication>,
    /// Dropped last: the plugin's code lives in it.
    _module: Arc<Module>,
}

impl std::fmt::Debug for Vst3Instance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vst3Instance")
            .field("active", &self.active)
            .field("processing", &self.processing)
            .finish_non_exhaustive()
    }
}

// Created and set up on the host's main thread, then handed whole to its
// audio thread for processing and back again; never used by two threads
// at once.
unsafe impl Send for Vst3Instance {}

impl Vst3Instance {
    /// Creates the processor and its controller, connects them and gives
    /// the controller the processor's state (Steinberg's order).
    pub fn create(module: Arc<Module>, cid: &str) -> Result<Self, String> {
        let cid = parse_cid(cid).ok_or("bad class id")?;
        let host = ComWrapper::new(HostApplication);
        let host_ptr = host.to_com_ptr::<FUnknown>().ok_or("no host context")?;
        let component: ComPtr<IComponent> =
            module.create(&cid).ok_or("the plugin would not start")?;
        if unsafe { component.initialize(host_ptr.as_ptr()) } != kResultOk {
            return Err("the plugin would not initialize".into());
        }
        let processor = component
            .cast::<IAudioProcessor>()
            .ok_or("not an audio processor")?;
        let (controller, separate) = match component.cast::<IEditController>() {
            Some(single) => (Some(single), false),
            None => {
                let mut ccid: TUID = [0; 16];
                let controller = (unsafe { component.getControllerClassId(&mut ccid) }
                    == kResultOk)
                    .then(|| module.create::<IEditController>(&ccid))
                    .flatten()
                    .filter(|c| unsafe { c.initialize(host_ptr.as_ptr()) } == kResultOk);
                (controller, true)
            }
        };
        let handler = ComWrapper::new(ComponentHandler);
        let mut connection = None;
        if let Some(controller) = &controller {
            if let Some(h) = handler.to_com_ptr::<IComponentHandler>() {
                unsafe { controller.setComponentHandler(h.as_ptr()) };
            }
            if separate
                && let (Some(a), Some(b)) = (
                    component.cast::<IConnectionPoint>(),
                    controller.cast::<IConnectionPoint>(),
                )
            {
                unsafe {
                    a.connect(b.as_ptr());
                    b.connect(a.as_ptr());
                }
                connection = Some((a, b));
            }
            // The controller starts from the processor's state.
            let stream = ComWrapper::new(MemoryStream::default());
            if let Some(s) = stream.to_com_ptr::<IBStream>()
                && unsafe { component.getState(s.as_ptr()) } == kResultOk
            {
                stream.position.set(0);
                unsafe { controller.setComponentState(s.as_ptr()) };
            }
        }
        let changes = ComWrapper::new(ParameterChanges::new());
        let changes_ptr = changes
            .to_com_ptr::<IParameterChanges>()
            .ok_or("no parameter changes")?;
        Ok(Self {
            component,
            processor,
            controller,
            connection,
            changes,
            changes_ptr,
            inputs: Vec::new(),
            outputs: Vec::new(),
            in_buses: Vec::new(),
            out_buses: Vec::new(),
            context: unsafe { std::mem::zeroed() },
            sample_rate: 48_000.,
            active: false,
            processing: false,
            _handler: handler,
            _host: host,
            _module: module,
        })
    }

    /// Buses (main ones stereo), processing setup, then active. Main thread.
    pub fn activate(
        &mut self,
        sample_rate: f64,
        max_frames: u32,
        offline: bool,
    ) -> Result<(), String> {
        self.deactivate();
        let audio = MediaTypes_::kAudio as MediaType;
        let (input, output) = (
            BusDirections_::kInput as BusDirection,
            BusDirections_::kOutput as BusDirection,
        );
        let counts = unsafe {
            (
                self.component.getBusCount(audio, input).clamp(0, 8) as usize,
                self.component.getBusCount(audio, output).clamp(0, 8) as usize,
            )
        };
        let current = |dir, index: usize| {
            let mut arrangement = 0;
            unsafe {
                self.processor
                    .getBusArrangement(dir, index as int32, &mut arrangement)
            };
            arrangement
        };
        let mut ins: Vec<SpeakerArrangement> = (0..counts.0).map(|i| current(input, i)).collect();
        let mut outs: Vec<SpeakerArrangement> = (0..counts.1).map(|i| current(output, i)).collect();
        if let Some(main) = ins.first_mut() {
            *main = SpeakerArr::kStereo;
        }
        if let Some(main) = outs.first_mut() {
            *main = SpeakerArr::kStereo;
        }
        let accepted = unsafe {
            self.processor.setBusArrangements(
                ins.as_mut_ptr(),
                ins.len() as int32,
                outs.as_mut_ptr(),
                outs.len() as int32,
            )
        } == kResultTrue;
        if !accepted {
            // Use what the plugin has instead.
            ins = (0..counts.0).map(|i| current(input, i)).collect();
            outs = (0..counts.1).map(|i| current(output, i)).collect();
        }
        unsafe {
            if counts.0 > 0 {
                self.component.activateBus(audio, input, 0, 1);
            }
            if counts.1 > 0 {
                self.component.activateBus(audio, output, 0, 1);
            }
            let events = MediaTypes_::kEvent as MediaType;
            if self.component.getBusCount(events, input) > 0 {
                self.component.activateBus(events, input, 0, 1);
            }
        }
        let bus = |arrangement: SpeakerArrangement| {
            let channels = channels(arrangement).min(MAX_CHANNELS);
            let mut scratch: Vec<Vec<f32>> = (0..channels)
                .map(|_| vec![0.; max_frames as usize])
                .collect();
            let mut pointers = [std::ptr::null_mut(); MAX_CHANNELS];
            for (p, s) in pointers.iter_mut().zip(&mut scratch) {
                *p = s.as_mut_ptr();
            }
            Bus {
                channels,
                scratch,
                pointers,
            }
        };
        self.inputs = ins.into_iter().map(bus).collect();
        self.outputs = outs.into_iter().map(bus).collect();
        let empty = |b: &Bus| AudioBusBuffers {
            numChannels: b.channels as int32,
            silenceFlags: 0,
            __field0: AudioBusBuffers__type0 {
                channelBuffers32: std::ptr::null_mut(),
            },
        };
        self.in_buses = self.inputs.iter().map(empty).collect();
        self.out_buses = self.outputs.iter().map(empty).collect();

        let mut setup = ProcessSetup {
            processMode: if offline {
                ProcessModes_::kOffline
            } else {
                ProcessModes_::kRealtime
            } as int32,
            symbolicSampleSize: SymbolicSampleSizes_::kSample32 as int32,
            maxSamplesPerBlock: max_frames as int32,
            sampleRate: sample_rate,
        };
        if unsafe { self.processor.setupProcessing(&mut setup) } != kResultOk {
            return Err("the plugin refused the processing setup".into());
        }
        if unsafe { self.component.setActive(1) } != kResultOk {
            return Err("the plugin would not activate".into());
        }
        self.sample_rate = sample_rate;
        self.active = true;
        Ok(())
    }

    /// Audio thread: one block from the shared memory. Allocation-free.
    pub fn process(&mut self, block: &mut HostBlock<'_>) {
        if !self.processing {
            unsafe { self.processor.setProcessing(1) };
            self.processing = true;
        }
        let frames = block.frames();
        self.changes.fill(block.params());

        // The main buses read and write the shared block; others scratch.
        let (shared_in, shared_out) = block.channel_pointers();
        for (index, (bus, buffers)) in self.inputs.iter_mut().zip(&mut self.in_buses).enumerate() {
            if index == 0 {
                for ch in 0..bus.channels {
                    bus.pointers[ch] = shared_in[ch.min(block.inputs().saturating_sub(1))];
                }
            } else {
                for s in &mut bus.scratch {
                    s[..frames].fill(0.);
                }
            }
            buffers.__field0.channelBuffers32 = bus.pointers.as_mut_ptr();
        }
        for (index, (bus, buffers)) in self.outputs.iter_mut().zip(&mut self.out_buses).enumerate()
        {
            if index == 0 {
                let n = bus.channels.min(block.outputs());
                bus.pointers[..n].copy_from_slice(&shared_out[..n]);
            }
            buffers.__field0.channelBuffers32 = bus.pointers.as_mut_ptr();
        }

        let c = &block.context;
        let flags = ProcessContext_::StatesAndFlags_::kTempoValid
            | ProcessContext_::StatesAndFlags_::kTimeSigValid
            | ProcessContext_::StatesAndFlags_::kProjectTimeMusicValid
            | if c.playing {
                ProcessContext_::StatesAndFlags_::kPlaying
            } else {
                0
            };
        self.context.state = flags as uint32;
        self.context.sampleRate = self.sample_rate;
        self.context.tempo = c.tempo_bpm;
        self.context.timeSigNumerator = 4;
        self.context.timeSigDenominator = 4;
        self.context.projectTimeMusic = c.position_beats;
        self.context.projectTimeSamples =
            (c.position_beats * 60. / c.tempo_bpm.max(1.) * self.sample_rate) as TSamples;

        let mut data = ProcessData {
            processMode: if c.offline {
                ProcessModes_::kOffline
            } else {
                ProcessModes_::kRealtime
            } as int32,
            symbolicSampleSize: SymbolicSampleSizes_::kSample32 as int32,
            numSamples: frames as int32,
            numInputs: self.in_buses.len() as int32,
            numOutputs: self.out_buses.len() as int32,
            inputs: self.in_buses.as_mut_ptr(),
            outputs: self.out_buses.as_mut_ptr(),
            inputParameterChanges: self.changes_ptr.as_ptr(),
            outputParameterChanges: std::ptr::null_mut(),
            inputEvents: std::ptr::null_mut(),
            outputEvents: std::ptr::null_mut(),
            processContext: &mut self.context,
        };
        let ok = unsafe { self.processor.process(&mut data) } == kResultOk;

        // A mono main output feeds both sides; no output, silence.
        let main_out = self.outputs.first().map_or(0, |b| b.channels);
        if !ok || main_out == 0 {
            for ch in 0..block.outputs() {
                block.output(ch).fill(0.);
            }
        } else {
            for ch in main_out..block.outputs() {
                let (src, dst) = (shared_out[main_out - 1], shared_out[ch]);
                unsafe { std::ptr::copy_nonoverlapping(src, dst, frames) };
            }
        }
    }

    /// Audio thread, after the last block.
    pub fn stop_processing(&mut self) {
        if self.processing {
            unsafe { self.processor.setProcessing(0) };
            self.processing = false;
        }
    }

    /// Main thread.
    pub fn deactivate(&mut self) {
        if self.active {
            unsafe { self.component.setActive(0) };
            self.active = false;
        }
    }

    /// A handle for reading and restoring this instance's state from the
    /// main thread, kept while the instance processes on its audio thread.
    pub fn state_handle(&self) -> Vst3State {
        Vst3State {
            component: self.component.clone(),
            controller: self.controller.clone(),
            _module: self._module.clone(),
        }
    }
}

/// An instance's state, reached from the host's main thread. Hosts save
/// while playing: `getState` runs on the UI thread while `process` runs on
/// the audio thread, as the API intends.
pub struct Vst3State {
    component: ComPtr<IComponent>,
    controller: Option<ComPtr<IEditController>>,
    /// Dropped last: the plugin's code lives in it.
    _module: Arc<Module>,
}

impl std::fmt::Debug for Vst3State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vst3State").finish_non_exhaustive()
    }
}

// Used only on the host's main thread, where the instance was made.
unsafe impl Send for Vst3State {}

impl Vst3State {
    /// The processor's and the controller's state, to save with the song.
    /// Either is empty if the plugin gave none.
    pub fn get(&self) -> PluginData {
        let read = |f: &dyn Fn(*mut IBStream) -> tresult| {
            let stream = ComWrapper::new(MemoryStream::default());
            let ok = stream
                .to_com_ptr::<IBStream>()
                .is_some_and(|s| f(s.as_ptr()) == kResultOk);
            if ok {
                stream.data.borrow().clone()
            } else {
                Vec::new()
            }
        };
        PluginData {
            component: read(&|s| unsafe { self.component.getState(s) }),
            controller: self
                .controller
                .as_ref()
                .map(|c| read(&|s| unsafe { c.getState(s) }))
                .unwrap_or_default(),
        }
    }

    /// Restores saved state: the processor's, then the controller's view of
    /// it, then the controller's own. Before activation.
    pub fn set(&self, state: &PluginData) {
        let stream = |bytes: &[u8]| {
            let wrapper = ComWrapper::new(MemoryStream::with(bytes.to_vec()));
            let pointer = wrapper.to_com_ptr::<IBStream>();
            (wrapper, pointer)
        };
        if !state.component.is_empty()
            && let (w, Some(s)) = stream(&state.component)
        {
            unsafe { self.component.setState(s.as_ptr()) };
            if let Some(c) = &self.controller {
                w.position.set(0);
                unsafe { c.setComponentState(s.as_ptr()) };
            }
        }
        if !state.controller.is_empty()
            && let (Some(c), (_w, Some(s))) = (&self.controller, stream(&state.controller))
        {
            unsafe { c.setState(s.as_ptr()) };
        }
    }
}

impl Drop for Vst3Instance {
    fn drop(&mut self) {
        self.deactivate();
        if let Some((a, b)) = self.connection.take() {
            unsafe {
                a.disconnect(b.as_ptr());
                b.disconnect(a.as_ptr());
            }
        }
        if let Some(controller) = self.controller.take() {
            unsafe { controller.terminate() };
        }
        unsafe { self.component.terminate() };
    }
}
