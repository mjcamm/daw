//! Envelope generators.

/// Level below which a releasing envelope counts as finished (-80 dB).
const SILENCE: f32 = 1e-4;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AdsrParams {
    pub attack_s: f32,
    pub decay_s: f32,
    /// Sustain level, 0..=1.
    pub sustain: f32,
    pub release_s: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Idle,
    Attack,
    Decay,
    Release,
}

/// Attack-decay-sustain-release envelope. The attack is linear; decay and
/// release are exponential, as they sound natural that way.
#[derive(Debug, Clone, Copy)]
pub struct Adsr {
    stage: Stage,
    level: f32,
}

impl Default for Adsr {
    fn default() -> Self {
        Self {
            stage: Stage::Idle,
            level: 0.0,
        }
    }
}

impl Adsr {
    /// Starts the attack from the current level, so retriggering never clicks.
    pub fn gate_on(&mut self) {
        self.stage = Stage::Attack;
    }

    pub fn gate_off(&mut self) {
        if self.stage != Stage::Idle {
            self.stage = Stage::Release;
        }
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn is_idle(&self) -> bool {
        self.stage == Stage::Idle
    }

    pub fn is_releasing(&self) -> bool {
        self.stage == Stage::Release
    }

    pub fn level(&self) -> f32 {
        self.level
    }

    /// Advances one sample and returns the new level.
    #[inline]
    pub fn next_level(&mut self, params: &AdsrParams, sample_rate: f32) -> f32 {
        match self.stage {
            Stage::Idle => {}
            Stage::Attack => {
                self.level += 1.0 / (params.attack_s * sample_rate).max(1.0);
                if self.level >= 1.0 {
                    self.level = 1.0;
                    self.stage = Stage::Decay;
                }
            }
            Stage::Decay => {
                let sustain = params.sustain.clamp(0.0, 1.0);
                self.level = sustain
                    + (self.level - sustain) * decay_coefficient(params.decay_s, sample_rate);
            }
            Stage::Release => {
                self.level *= decay_coefficient(params.release_s, sample_rate);
                if self.level < SILENCE {
                    self.level = 0.0;
                    self.stage = Stage::Idle;
                }
            }
        }
        self.level
    }
}

/// Per-sample multiplier that decays by 60 dB over `seconds`.
#[inline]
fn decay_coefficient(seconds: f32, sample_rate: f32) -> f32 {
    // ln(1000) = 6.9078: the time constant for a 60 dB fall.
    (-6.907_755 / (seconds * sample_rate).max(1.0)).exp()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 1_000.0;
    const PARAMS: AdsrParams = AdsrParams {
        attack_s: 0.01,
        decay_s: 0.1,
        sustain: 0.5,
        release_s: 0.1,
    };

    fn run(env: &mut Adsr, samples: usize) -> f32 {
        let mut level = 0.0;
        for _ in 0..samples {
            level = env.next_level(&PARAMS, SR);
        }
        level
    }

    #[test]
    fn idle_until_gated() {
        let mut env = Adsr::default();
        assert_eq!(run(&mut env, 10), 0.0);
        assert!(env.is_idle());
    }

    #[test]
    fn attack_rises_linearly_to_full() {
        let mut env = Adsr::default();
        env.gate_on();
        assert!((env.next_level(&PARAMS, SR) - 0.1).abs() < 1e-6);
        assert_eq!(run(&mut env, 9), 1.0);
    }

    #[test]
    fn decays_to_sustain() {
        let mut env = Adsr::default();
        env.gate_on();
        let level = run(&mut env, 1_000);
        assert!((level - 0.5).abs() < 1e-3, "{level}");
    }

    #[test]
    fn release_falls_sixty_db_over_release_time_then_goes_idle() {
        let mut env = Adsr::default();
        env.gate_on();
        run(&mut env, 1_000);
        env.gate_off();
        let level = run(&mut env, 100);
        assert!((level / 0.5 - 0.001).abs() < 1e-4, "{level}");
        run(&mut env, 1_000);
        assert!(env.is_idle());
        assert_eq!(env.level(), 0.0);
    }

    #[test]
    fn retrigger_attacks_from_current_level() {
        let mut env = Adsr::default();
        env.gate_on();
        run(&mut env, 1_000);
        env.gate_off();
        run(&mut env, 10);
        let before = env.level();
        env.gate_on();
        assert!(env.next_level(&PARAMS, SR) > before);
    }
}
