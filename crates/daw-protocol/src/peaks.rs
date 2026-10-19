//! Waveform overviews: an audio file's minimum and maximum in buckets of
//! frames, at several sizes, so a front end can draw any zoom without the
//! audio itself.

/// The lowest and highest sample in a stretch of audio.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MinMax {
    pub min: f32,
    pub max: f32,
}

impl MinMax {
    /// Nothing seen yet; [`MinMax::union`] widens it.
    pub const EMPTY: Self = Self {
        min: f32::INFINITY,
        max: f32::NEG_INFINITY,
    };

    pub fn union(self, other: Self) -> Self {
        Self {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
        }
    }

    pub fn is_empty(self) -> bool {
        self.min > self.max
    }
}

/// One bucket size: each channel's `MinMax` per `span` frames.
#[derive(Debug, Clone, PartialEq)]
pub struct PeakLevel {
    pub span: usize,
    pub channels: Vec<Vec<MinMax>>,
}

/// A file's overview, finest level first. Each level's span is four times
/// the one before.
#[derive(Debug, Clone, PartialEq)]
pub struct Peaks {
    /// The file's length and rate, which frame positions are counted in.
    pub frames: usize,
    pub sample_rate: u32,
    pub levels: Vec<PeakLevel>,
}

/// Frames per bucket on the finest level.
pub const FINEST_SPAN: usize = 32;
/// Levels stop once they would have fewer buckets than this.
const MIN_BUCKETS: usize = 64;

impl Peaks {
    /// The overview of audio given as one slice per channel (equal lengths).
    pub fn compute(channels: &[&[f32]], sample_rate: u32) -> Self {
        let frames = channels.first().map_or(0, |c| c.len());
        let finest = PeakLevel {
            span: FINEST_SPAN,
            channels: channels
                .iter()
                .map(|c| {
                    c.chunks(FINEST_SPAN)
                        .map(|chunk| {
                            chunk
                                .iter()
                                .fold(MinMax::EMPTY, |m, &s| m.union(MinMax { min: s, max: s }))
                        })
                        .collect()
                })
                .collect(),
        };
        let mut levels = vec![finest];
        while let Some(last) = levels.last() {
            let buckets = last.channels.first().map_or(0, Vec::len);
            if buckets / 4 < MIN_BUCKETS {
                break;
            }
            let next = PeakLevel {
                span: last.span * 4,
                channels: last
                    .channels
                    .iter()
                    .map(|c| {
                        c.chunks(4)
                            .map(|four| four.iter().fold(MinMax::EMPTY, |m, &b| m.union(b)))
                            .collect()
                    })
                    .collect(),
            };
            levels.push(next);
        }
        Self {
            frames,
            sample_rate,
            levels,
        }
    }

    pub fn channels(&self) -> usize {
        self.levels.first().map_or(0, |l| l.channels.len())
    }

    /// The lowest and highest sample of `channel` over frames `from..to`,
    /// from the coarsest level whose buckets still fit the range, so a
    /// pixel's worth of audio reads a handful of buckets. Buckets are read
    /// whole, so the answer can reach up to a bucket past either end.
    /// Empty outside the audio.
    pub fn range(&self, channel: usize, from: f64, to: f64) -> MinMax {
        let from = from.max(0.0);
        let to = to.min(self.frames as f64);
        if to <= from {
            return MinMax::EMPTY;
        }
        let width = to - from;
        let level = self
            .levels
            .iter()
            .rev()
            .find(|l| l.span as f64 <= width)
            .or(self.levels.first());
        let Some(level) = level else {
            return MinMax::EMPTY;
        };
        let Some(buckets) = level.channels.get(channel) else {
            return MinMax::EMPTY;
        };
        let first = (from / level.span as f64) as usize;
        let last = ((to / level.span as f64).ceil() as usize).min(buckets.len());
        buckets[first.min(last)..last]
            .iter()
            .fold(MinMax::EMPTY, |m, &b| m.union(b))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp(frames: usize) -> Vec<f32> {
        (0..frames).map(|i| i as f32 / frames as f32).collect()
    }

    #[test]
    fn levels_get_coarser_by_four_until_few_buckets_are_left() {
        let audio = ramp(100_000);
        let peaks = Peaks::compute(&[&audio], 48_000);
        let spans: Vec<usize> = peaks.levels.iter().map(|l| l.span).collect();
        assert_eq!(spans, [32, 128, 512]);
        assert_eq!(peaks.levels[0].channels[0].len(), 3125);
        assert_eq!(peaks.levels[2].channels[0].len(), 196);
        assert_eq!(peaks.frames, 100_000);
    }

    #[test]
    fn a_range_spans_the_lowest_and_highest_samples_in_it() {
        let mut left = vec![0.0f32; 4096];
        left[1000] = 0.8;
        left[3000] = -0.6;
        let right = vec![0.1f32; 4096];
        let peaks = Peaks::compute(&[&left, &right], 44_100);
        assert_eq!(peaks.channels(), 2);
        assert_eq!(
            peaks.range(0, 0.0, 4096.0),
            MinMax {
                min: -0.6,
                max: 0.8
            }
        );
        assert_eq!(
            peaks.range(0, 2048.0, 4096.0),
            MinMax {
                min: -0.6,
                max: 0.0
            }
        );
        assert_eq!(peaks.range(1, 10.0, 11.0), MinMax { min: 0.1, max: 0.1 });
        assert!(peaks.range(0, 5000.0, 6000.0).is_empty(), "past the end");
        assert!(peaks.range(2, 0.0, 100.0).is_empty(), "no such channel");
    }

    #[test]
    fn silence_and_empty_audio_have_overviews() {
        let peaks = Peaks::compute(&[&[]], 48_000);
        assert!(peaks.range(0, 0.0, 10.0).is_empty());
        let quiet = vec![0.0f32; 10];
        let peaks = Peaks::compute(&[&quiet], 48_000);
        assert_eq!(peaks.range(0, 0.0, 10.0), MinMax { min: 0.0, max: 0.0 });
    }
}
