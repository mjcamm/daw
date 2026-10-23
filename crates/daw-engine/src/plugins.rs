//! Plugin effects on the control side: keeps one instance per plugin effect
//! running in a host process (`docs/design/vst3-hosting.md`, all plugins in
//! one process for now) and hands each instance's audio channel to the
//! audio thread.
//!
//! Calls to the host happen here, on the control thread; starting a host
//! takes a moment the first time a song needs one.

use crate::audio::{AudioMsg, CHUNK_FRAMES};
use crate::project::Project;
use daw_plugins::block::{BlockStats, DawBlock, Layout};
use daw_plugins::client::{HostCommand, HostProcess, next_instance};
use daw_plugins::control::HostEvent;
use daw_protocol::{
    DeviceId, EffectKind, InstrumentKind, PluginData, PluginParamInfo, PluginSpec, PluginState,
    PluginWindow,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

/// Blocks missed in a row before a plugin counts as hung (about a second
/// at small buffers, less at large ones).
const HUNG_AFTER: u32 = 64;

/// Every plugin effect gets stereo in and out, in chunks.
const LAYOUT: Layout = Layout {
    max_frames: CHUNK_FRAMES as u32,
    inputs: 2,
    outputs: 2,
};

#[derive(Debug, Default)]
pub(crate) struct Plugins {
    command: Option<HostCommand>,
    host: Option<HostProcess>,
    running: BTreeMap<DeviceId, Instance>,
    /// Plugin effects that could not start, and why.
    failed: BTreeMap<DeviceId, String>,
    /// Instances whose effects are gone, ended once the audio side hands
    /// back their channel (until then it may still be processing them).
    ending: BTreeSet<u64>,
    /// Each plugin effect's last known state: from the file, the last save,
    /// or when its effect went (so undo brings it back as it was). Kept for
    /// plugins that cannot run, so saving never loses theirs.
    saved: BTreeMap<DeviceId, PluginData>,
    /// Parameter lists shown for plugins that are not running, for front
    /// ends' tests (which run no host).
    shown_params: BTreeMap<DeviceId, Arc<Vec<PluginParamInfo>>>,
    /// How late each running plugin's sound comes out, in frames.
    latencies: BTreeMap<DeviceId, u32>,
    /// Plugin windows open, or why one couldn't open (absent: closed).
    windows: BTreeMap<DeviceId, PluginWindow>,
    /// Windows asked for and not answered yet, by call.
    opening: BTreeMap<u64, DeviceId>,
    /// Where each plugin effect's window was last, saved with the song.
    places: BTreeMap<DeviceId, (i32, i32)>,
    /// The title each window was given.
    titles: BTreeMap<DeviceId, String>,
}

/// Something the user did in a plugin's window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum WindowEdit {
    Begin(u32),
    Value(u32, f64),
    End(u32),
}

#[derive(Debug)]
struct Instance {
    id: u64,
    spec: PluginSpec,
    stats: Arc<BlockStats>,
    /// Its automatable parameters, asked once it starts.
    params: Arc<Vec<PluginParamInfo>>,
    /// How late its sound comes out, in frames.
    latency: u32,
}

impl Plugins {
    pub(crate) fn set_command(&mut self, command: HostCommand) {
        self.command = Some(command);
    }

    /// Starts instances for new plugin effects and ends those whose effects
    /// are gone. Each new instance's channel goes into `outbox`, after the
    /// render state that holds its effect.
    pub(crate) fn sync(
        &mut self,
        project: &Project,
        sample_rate: u32,
        outbox: &mut VecDeque<AudioMsg>,
    ) {
        let wanted = plugin_effects(project);

        let gone: Vec<DeviceId> = self
            .running
            .iter()
            .filter(|(id, instance)| wanted.get(id) != Some(&instance.spec))
            .map(|(id, _)| *id)
            .collect();
        for id in gone {
            self.windows.remove(&id);
            if let Some(instance) = self.running.remove(&id) {
                self.keep_state(id, instance.id);
                // An effect that now runs another plugin plays nothing until
                // its new instance attaches (or forever, if that fails).
                outbox.push_back(AudioMsg::DetachPlugin(id));
                self.ending.insert(instance.id);
            }
        }
        self.failed.retain(|id, _| wanted.contains_key(id));
        self.windows.retain(|id, _| wanted.contains_key(id));

        for (id, spec) in wanted {
            if self.running.contains_key(&id) || self.failed.contains_key(&id) {
                continue;
            }
            let state = self.saved.get(&id).cloned();
            match self.start(&spec, state, sample_rate) {
                Ok((instance, block)) => {
                    outbox.push_back(AudioMsg::AttachPlugin { effect: id, block });
                    self.running.insert(id, instance);
                }
                Err(why) => {
                    self.failed.insert(id, why);
                }
            }
        }
    }

    /// A channel the audio side let go of: freed here, and its instance
    /// ended if its effect is gone.
    pub(crate) fn retired(&mut self, block: Box<DawBlock>) {
        let instance = block.instance();
        drop(block);
        if self.ending.remove(&instance)
            && let Some(host) = &mut self.host
        {
            let _ = host.destroy(instance);
        }
    }

    fn start(
        &mut self,
        spec: &PluginSpec,
        state: Option<PluginData>,
        sample_rate: u32,
    ) -> Result<(Instance, Box<DawBlock>), String> {
        let host = self.host()?;
        let id = next_instance();
        host.create(id, spec.clone(), state)
            .map_err(|e| e.to_string())?;
        // A plugin that cannot list its parameters still plays.
        let params = host.params(id).unwrap_or_default();
        let block = host
            .activate(id, LAYOUT, f64::from(sample_rate))
            .map_err(|e| e.to_string())?;
        let latency = host.latency(id).unwrap_or(0);
        let instance = Instance {
            id,
            spec: spec.clone(),
            stats: block.stats(),
            params: Arc::new(params),
            latency,
        };
        Ok((instance, Box::new(block)))
    }

    /// Asks a running instance for its state and remembers it. A host that
    /// is gone or hung keeps the last known state.
    fn keep_state(&mut self, effect: DeviceId, instance: u64) {
        if let Some(host) = self.host.as_mut().filter(|h| h.is_alive())
            && let Ok(Some(state)) = host.state(instance)
        {
            self.saved.insert(effect, state);
        }
    }

    /// Every plugin effect's state, to save with the song: asked of those
    /// running, the last known state of the rest.
    pub(crate) fn states(&mut self, project: &Project) -> BTreeMap<DeviceId, PluginData> {
        // A hung plugin may not answer: it keeps its last known state.
        let running: Vec<(DeviceId, u64)> = self
            .running
            .iter()
            .filter(|(_, i)| i.stats.missed_in_a_row() < HUNG_AFTER)
            .map(|(id, i)| (*id, i.id))
            .collect();
        for (effect, instance) in running {
            self.keep_state(effect, instance);
        }
        let wanted = plugin_effects(project);
        self.saved
            .iter()
            .filter(|(id, state)| wanted.contains_key(id) && !state.is_empty())
            .map(|(id, state)| (*id, state.clone()))
            .collect()
    }

    /// A new song is coming: every instance ends (once its channel is
    /// back), and its plugins start from `states` (read from its file) at
    /// the next [`Plugins::sync`].
    pub(crate) fn replace_song(
        &mut self,
        states: BTreeMap<DeviceId, PluginData>,
        places: BTreeMap<DeviceId, (i32, i32)>,
        outbox: &mut VecDeque<AudioMsg>,
    ) {
        self.places = places;
        self.titles.clear();
        self.opening.clear();
        for (effect, instance) in std::mem::take(&mut self.running) {
            outbox.push_back(AudioMsg::DetachPlugin(effect));
            self.ending.insert(instance.id);
        }
        self.failed.clear();
        self.windows.clear();
        self.saved = states;
    }

    /// The host process, started if there is none yet. A host that died
    /// stays dead until [`Plugins::reload`], so its plugins show as crashed.
    fn host(&mut self) -> Result<&mut HostProcess, String> {
        if self.host.is_none() {
            let command = self.command.as_ref().ok_or("no plugin host is set up")?;
            self.host = Some(HostProcess::spawn(command).map_err(|e| e.to_string())?);
        }
        self.host
            .as_mut()
            .ok_or_else(|| "no plugin host".to_owned())
    }

    /// Ends the host process and starts every plugin again.
    pub(crate) fn reload(
        &mut self,
        project: &Project,
        sample_rate: u32,
        outbox: &mut VecDeque<AudioMsg>,
    ) {
        if let Some(mut host) = self.host.take() {
            host.kill();
        }
        self.running.clear();
        self.failed.clear();
        self.ending.clear();
        self.windows.clear();
        self.opening.clear();
        self.sync(project, sample_rate, outbox);
    }

    /// After the audio device changed: every instance gets a new channel at
    /// the new rate (the old channels went with the old processor).
    pub(crate) fn reactivate(
        &mut self,
        project: &Project,
        sample_rate: u32,
        outbox: &mut VecDeque<AudioMsg>,
    ) {
        if let Some(host) = &mut self.host {
            for (id, instance) in &mut self.running {
                if let Ok(block) = host.activate(instance.id, LAYOUT, f64::from(sample_rate)) {
                    instance.stats = block.stats();
                    instance.latency = host.latency(instance.id).unwrap_or(instance.latency);
                    outbox.push_back(AudioMsg::AttachPlugin {
                        effect: *id,
                        block: Box::new(block),
                    });
                }
            }
        }
        self.sync(project, sample_rate, outbox);
    }

    /// Asks for a plugin effect's window titled `title` (or for it to come
    /// to the front) without waiting: some plugins take a second or two.
    /// The answer arrives in [`Plugins::window_edits`]; why it can't open
    /// shows in [`Plugins::window`].
    pub(crate) fn open_window(&mut self, effect: DeviceId, title: &str) {
        let started = match self.running.get(&effect) {
            Some(instance) => {
                let id = instance.id;
                let at = self.places.get(&effect).copied();
                self.host().and_then(|host| {
                    host.start_open_editor(id, title, at)
                        .map_err(|e| e.to_string())
                })
            }
            None => Err(self
                .failed
                .get(&effect)
                .cloned()
                .unwrap_or_else(|| "the plugin isn't running".into())),
        };
        match started {
            Ok(call) => {
                self.opening.insert(call, effect);
                self.titles.insert(effect, title.to_owned());
            }
            Err(why) => {
                self.windows.insert(effect, PluginWindow::Failed(why));
            }
        }
    }

    /// Renames a plugin effect's window if it is open and its title
    /// changed (its node was renamed).
    pub(crate) fn retitle(&mut self, effect: DeviceId, title: &str) {
        let open = matches!(self.windows.get(&effect), Some(PluginWindow::Open))
            || self.opening.values().any(|e| *e == effect);
        if !open || self.titles.get(&effect).is_some_and(|t| t == title) {
            return;
        }
        if let Some(instance) = self.running.get(&effect).map(|i| i.id)
            && let Some(host) = self.host.as_mut().filter(|h| h.is_alive())
            && host.retitle_editor(instance, title).is_ok()
        {
            self.titles.insert(effect, title.to_owned());
        }
    }

    /// Plugin effects whose windows are open or opening.
    pub(crate) fn open_windows(&self) -> Vec<DeviceId> {
        let mut open: Vec<DeviceId> = self
            .windows
            .iter()
            .filter(|(_, w)| matches!(w, PluginWindow::Open))
            .map(|(e, _)| *e)
            .chain(self.opening.values().copied())
            .collect();
        open.sort();
        open.dedup();
        open
    }

    /// Where the project's plugin windows were last, to save with it.
    pub(crate) fn places(&self, project: &Project) -> BTreeMap<DeviceId, (i32, i32)> {
        let wanted = plugin_effects(project);
        self.places
            .iter()
            .filter(|(id, _)| wanted.contains_key(id))
            .map(|(id, at)| (*id, *at))
            .collect()
    }

    pub(crate) fn window(&self, effect: DeviceId) -> PluginWindow {
        self.windows.get(&effect).cloned().unwrap_or_default()
    }

    /// What the user did in plugin windows since last asked, by effect,
    /// and whether any window opened, failed to or closed.
    pub(crate) fn window_edits(&mut self) -> (Vec<(DeviceId, WindowEdit)>, bool) {
        let Some(host) = self.host.as_mut().filter(|h| h.is_alive()) else {
            return (Vec::new(), false);
        };
        let events = host.take_events();
        let mut changed = false;
        for (call, result) in host.take_replies() {
            if let Some(effect) = self.opening.remove(&call) {
                let window = match result {
                    Ok(_) => PluginWindow::Open,
                    Err(why) => PluginWindow::Failed(why),
                };
                self.windows.insert(effect, window);
                changed = true;
            }
        }
        let effect_of = |instance: u64| {
            self.running
                .iter()
                .find(|(_, i)| i.id == instance)
                .map(|(effect, _)| *effect)
        };
        let mut edits = Vec::new();
        let mut closed = Vec::new();
        for event in events {
            match event {
                HostEvent::EditBegin { instance, id } => {
                    edits.extend(effect_of(instance).map(|e| (e, WindowEdit::Begin(id))));
                }
                HostEvent::EditValue {
                    instance,
                    id,
                    value,
                } => edits.extend(effect_of(instance).map(|e| (e, WindowEdit::Value(id, value)))),
                HostEvent::EditEnd { instance, id } => {
                    edits.extend(effect_of(instance).map(|e| (e, WindowEdit::End(id))));
                }
                HostEvent::EditorClosed { instance } => closed.extend(effect_of(instance)),
                HostEvent::EditorMoved { instance, at } => {
                    if let Some(effect) = effect_of(instance) {
                        self.places.insert(effect, at);
                    }
                }
            }
        }
        changed |= !closed.is_empty();
        for effect in closed {
            self.windows.remove(&effect);
        }
        (edits, changed)
    }

    /// How late a plugin's sound comes out, in frames: 0 unless it runs
    /// (one that can't run is silent).
    pub(crate) fn latency(&self, device: DeviceId) -> u32 {
        self.running.get(&device).map_or(0, |i| i.latency)
    }

    /// Whether any plugin's latency changed since last asked (one started,
    /// ended or was reactivated).
    pub(crate) fn take_latency_changed(&mut self) -> bool {
        let now: BTreeMap<DeviceId, u32> = self
            .running
            .iter()
            .filter(|(_, i)| i.latency > 0)
            .map(|(id, i)| (*id, i.latency))
            .collect();
        let changed = now != self.latencies;
        self.latencies = now;
        changed
    }

    /// A running plugin's automatable parameters; none for anything else.
    pub(crate) fn params(&self, effect: DeviceId) -> Arc<Vec<PluginParamInfo>> {
        self.running
            .get(&effect)
            .map(|i| i.params.clone())
            .or_else(|| self.shown_params.get(&effect).cloned())
            .unwrap_or_default()
    }

    pub(crate) fn show_params(&mut self, effect: DeviceId, params: Vec<PluginParamInfo>) {
        self.shown_params.insert(effect, Arc::new(params));
    }

    /// How a plugin effect is doing; `None` for anything else.
    pub(crate) fn state(&self, effect: DeviceId) -> Option<PluginState> {
        if let Some(why) = self.failed.get(&effect) {
            return Some(PluginState::Failed(why.clone()));
        }
        let instance = self.running.get(&effect)?;
        let alive = self.host.as_ref().is_some_and(HostProcess::is_alive);
        Some(if !alive {
            PluginState::Crashed
        } else if instance.stats.missed_in_a_row() >= HUNG_AFTER {
            PluginState::Hung
        } else {
            PluginState::Running
        })
    }
}

/// The project's plugin effects and instruments (device ids are
/// project-wide) and which plugin each runs.
fn plugin_effects(project: &Project) -> BTreeMap<DeviceId, PluginSpec> {
    let nodes = project.nodes();
    let effects = nodes
        .iter()
        .flat_map(|entry| entry.node.effects.iter())
        .filter(|(_, effect)| effect.kind == EffectKind::Plugin)
        .filter_map(|(id, effect)| Some((*id, effect.plugin.clone()?)));
    let instruments = nodes
        .iter()
        .flat_map(|entry| entry.node.instruments.iter())
        .filter(|(_, instrument)| instrument.kind == InstrumentKind::Plugin)
        .filter_map(|(id, instrument)| Some((*id, instrument.plugin.clone()?)));
    effects.chain(instruments).collect()
}
