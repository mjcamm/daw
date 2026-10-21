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
use daw_protocol::{DeviceId, EffectKind, PluginSpec, PluginState};
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
}

#[derive(Debug)]
struct Instance {
    id: u64,
    spec: PluginSpec,
    stats: Arc<BlockStats>,
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
        let wanted: BTreeMap<DeviceId, PluginSpec> = project
            .nodes()
            .into_iter()
            .flat_map(|entry| entry.node.effects.iter())
            .filter(|(_, effect)| effect.kind == EffectKind::Plugin)
            .filter_map(|(id, effect)| Some((*id, effect.plugin.clone()?)))
            .collect();

        let gone: Vec<DeviceId> = self
            .running
            .iter()
            .filter(|(id, instance)| wanted.get(id) != Some(&instance.spec))
            .map(|(id, _)| *id)
            .collect();
        for id in gone {
            if let Some(instance) = self.running.remove(&id) {
                self.ending.insert(instance.id);
            }
        }
        self.failed.retain(|id, _| wanted.contains_key(id));

        for (id, spec) in wanted {
            if self.running.contains_key(&id) || self.failed.contains_key(&id) {
                continue;
            }
            match self.start(&spec, sample_rate) {
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
        sample_rate: u32,
    ) -> Result<(Instance, Box<DawBlock>), String> {
        let host = self.host()?;
        let id = next_instance();
        host.create(id, spec.clone()).map_err(|e| e.to_string())?;
        let block = host
            .activate(id, LAYOUT, f64::from(sample_rate))
            .map_err(|e| e.to_string())?;
        let instance = Instance {
            id,
            spec: spec.clone(),
            stats: block.stats(),
        };
        Ok((instance, Box::new(block)))
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
                    outbox.push_back(AudioMsg::AttachPlugin {
                        effect: *id,
                        block: Box::new(block),
                    });
                }
            }
        }
        self.sync(project, sample_rate, outbox);
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
