//! The audio side of the engine.
//!
//! [`AudioProcessor`] runs on the audio thread. It never allocates, frees,
//! locks or blocks: it only reads from and writes to lock-free queues. State
//! arrives as whole replacement [`RenderState`]s, and each replaced state is
//! sent back to the control side to be freed there.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Everything the audio thread needs to render, compiled from the project by
/// the control side. Immutable once sent.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderState {
    pub tempo_bpm: f64,
}

/// Control-to-audio messages.
#[derive(Debug)]
pub(crate) enum AudioMsg {
    SetPlaying(bool),
    SwapState(Box<RenderState>),
}

/// Values the audio side reports back without locking.
#[derive(Debug, Default)]
pub(crate) struct SharedStatus {
    pub position_frames: AtomicU64,
}

#[derive(Debug)]
pub struct AudioProcessor {
    channels: usize,
    state: Box<RenderState>,
    playing: bool,
    position_frames: u64,
    from_control: rtrb::Consumer<AudioMsg>,
    garbage: rtrb::Producer<Box<RenderState>>,
    status: Arc<SharedStatus>,
}

impl AudioProcessor {
    pub(crate) fn new(
        channels: usize,
        state: Box<RenderState>,
        from_control: rtrb::Consumer<AudioMsg>,
        garbage: rtrb::Producer<Box<RenderState>>,
        status: Arc<SharedStatus>,
    ) -> Self {
        Self {
            channels,
            state,
            playing: false,
            position_frames: 0,
            from_control,
            garbage,
            status,
        }
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Renders one block of interleaved audio into `out`.
    ///
    /// `out.len()` must be a multiple of the channel count. Real-time safe.
    pub fn process(&mut self, out: &mut [f32]) {
        daw_rt::realtime(|| {
            debug_assert_eq!(out.len() % self.channels, 0, "partial frame in block");
            self.receive();
            out.fill(0.0);
            if self.playing {
                self.position_frames += (out.len() / self.channels) as u64;
            }
            self.status
                .position_frames
                .store(self.position_frames, Ordering::Relaxed);
        });
    }

    #[cfg(test)]
    pub(crate) fn state(&self) -> &RenderState {
        &self.state
    }

    fn receive(&mut self) {
        // Only take a message while the garbage queue has room, so a replaced
        // state can always be handed back instead of freed here.
        while self.garbage.slots() > 0 {
            let Ok(msg) = self.from_control.pop() else {
                break;
            };
            match msg {
                AudioMsg::SetPlaying(playing) => self.playing = playing,
                AudioMsg::SwapState(state) => {
                    let old = std::mem::replace(&mut self.state, state);
                    if self.garbage.push(old).is_err() {
                        unreachable!("garbage queue had a free slot");
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(tempo_bpm: f64) -> Box<RenderState> {
        Box::new(RenderState { tempo_bpm })
    }

    #[test]
    fn full_garbage_queue_defers_state_swaps_instead_of_freeing() {
        let (mut to_audio, from_control) = rtrb::RingBuffer::new(8);
        let (garbage_tx, mut garbage_rx) = rtrb::RingBuffer::new(1);
        let mut processor =
            AudioProcessor::new(2, state(100.0), from_control, garbage_tx, Arc::default());
        for bpm in [110.0, 120.0, 130.0] {
            to_audio.push(AudioMsg::SwapState(state(bpm))).unwrap();
        }
        let mut block = [0.0; 4];

        // Only one swap fits in the garbage queue; the rest wait. The realtime
        // guard aborts the test if the processor frees anything.
        processor.process(&mut block);
        processor.process(&mut block);
        assert_eq!(processor.state().tempo_bpm, 110.0);

        drop(garbage_rx.pop().unwrap());
        processor.process(&mut block);
        assert_eq!(processor.state().tempo_bpm, 120.0);

        drop(garbage_rx.pop().unwrap());
        processor.process(&mut block);
        assert_eq!(processor.state().tempo_bpm, 130.0);
    }
}
