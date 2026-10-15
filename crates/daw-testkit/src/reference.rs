//! Comparing renders against reference recordings stored as 32-bit float WAV.

use crate::Rendered;
use std::path::Path;

/// Environment variable that writes references instead of checking them.
pub const BLESS_ENV: &str = "DAW_BLESS";

/// Allowed per-sample difference. Leaves room for floating-point differences
/// between platforms while catching any audible change.
pub const DEFAULT_TOLERANCE: f32 = 1e-5;

/// Checks `rendered` against the reference at `path`, or writes it there when
/// `DAW_BLESS=1`. Returns a human-readable failure message on mismatch.
pub fn check_reference(path: &Path, rendered: &Rendered, tolerance: f32) -> Result<(), String> {
    if std::env::var(BLESS_ENV).is_ok_and(|v| v == "1") {
        return write(path, rendered).map_err(|e| format!("writing {}: {e}", path.display()));
    }
    let reference = read(path).map_err(|e| {
        format!(
            "reading reference {}: {e}\nRun with {BLESS_ENV}=1 to create it.",
            path.display()
        )
    })?;
    compare(&reference, rendered, tolerance)
        .map_err(|e| format!("{} does not match: {e}", path.display()))
}

fn compare(reference: &Rendered, actual: &Rendered, tolerance: f32) -> Result<(), String> {
    if (reference.sample_rate, reference.channels) != (actual.sample_rate, actual.channels) {
        return Err(format!(
            "format differs: reference is {} Hz x {} ch, render is {} Hz x {} ch",
            reference.sample_rate, reference.channels, actual.sample_rate, actual.channels
        ));
    }
    if reference.samples.len() != actual.samples.len() {
        return Err(format!(
            "length differs: reference has {} frames, render has {}",
            reference.frames(),
            actual.frames()
        ));
    }
    let channels = usize::from(actual.channels);
    let mut worst: Option<(usize, f32)> = None;
    for (i, (r, a)) in reference.samples.iter().zip(&actual.samples).enumerate() {
        // A NaN on either side never matches.
        let diff = match (r - a).abs() {
            d if d.is_nan() => f32::INFINITY,
            d => d,
        };
        if diff > tolerance && worst.is_none_or(|(_, w)| diff > w) {
            worst = Some((i, diff));
        }
    }
    match worst {
        None => Ok(()),
        Some((i, diff)) => Err(format!(
            "largest difference {diff} (tolerance {tolerance}) at frame {} channel {}: \
             reference {}, render {}",
            i / channels,
            i % channels,
            reference.samples[i],
            actual.samples[i]
        )),
    }
}

fn read(path: &Path) -> Result<Rendered, hound::Error> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    if spec.sample_format != hound::SampleFormat::Float || spec.bits_per_sample != 32 {
        return Err(hound::Error::Unsupported);
    }
    let samples = reader.samples::<f32>().collect::<Result<_, _>>()?;
    Ok(Rendered {
        sample_rate: spec.sample_rate,
        channels: spec.channels,
        samples,
    })
}

fn write(path: &Path, rendered: &Rendered) -> Result<(), hound::Error> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let spec = hound::WavSpec {
        channels: rendered.channels,
        sample_rate: rendered.sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    for &sample in &rendered.samples {
        writer.write_sample(sample)?;
    }
    writer.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rendered(samples: Vec<f32>) -> Rendered {
        Rendered {
            sample_rate: 48_000,
            channels: 2,
            samples,
        }
    }

    #[test]
    fn identical_renders_match() {
        let a = rendered(vec![0.0, 0.5, -0.5, 1.0]);
        assert!(compare(&a, &a.clone(), DEFAULT_TOLERANCE).is_ok());
    }

    #[test]
    fn differences_within_tolerance_match() {
        let a = rendered(vec![0.0, 0.5]);
        let b = rendered(vec![0.000_001, 0.5]);
        assert!(compare(&a, &b, DEFAULT_TOLERANCE).is_ok());
    }

    #[test]
    fn reports_the_largest_difference_with_its_position() {
        let a = rendered(vec![0.0, 0.0, 0.0, 0.0]);
        let b = rendered(vec![0.0, 0.1, 0.0, 0.3]);
        let err = compare(&a, &b, DEFAULT_TOLERANCE).unwrap_err();
        assert!(err.contains("frame 1 channel 1"), "{err}");
    }

    #[test]
    fn nan_never_matches() {
        let a = rendered(vec![0.0, 0.0]);
        let b = rendered(vec![0.0, f32::NAN]);
        assert!(compare(&a, &b, DEFAULT_TOLERANCE).is_err());
    }

    #[test]
    fn length_and_format_differences_fail() {
        let a = rendered(vec![0.0, 0.0]);
        assert!(compare(&a, &rendered(vec![0.0; 4]), DEFAULT_TOLERANCE).is_err());
        let mono = Rendered {
            channels: 1,
            ..a.clone()
        };
        assert!(compare(&a, &mono, DEFAULT_TOLERANCE).is_err());
    }

    #[test]
    fn wav_round_trip_is_lossless() {
        let dir = std::env::temp_dir().join(format!("daw-testkit-{}", std::process::id()));
        let path = dir.join("round-trip.wav");
        let original = rendered(vec![0.0, 0.25, -1.0, 0.123_456_7]);
        write(&path, &original).unwrap();
        let read_back = read(&path).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(read_back, original);
    }
}
