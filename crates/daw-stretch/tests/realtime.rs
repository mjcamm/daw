//! Real-time guarantees of `StemStream`: no allocation in `process`, and output that doesn't
//! depend on how the host slices it into blocks.

use daw_stretch::{Stem, StemStream};

#[global_allocator]
static ALLOCATOR: daw_rt::GuardedAllocator = daw_rt::GuardedAllocator;

/// Runs `f` as the audio thread would: debug builds abort on any allocation or free.
fn realtime<R>(f: impl FnOnce() -> R) -> R {
    daw_rt::realtime(f)
}

/// A clip with hits and tones so every code path runs (onsets, locks, silence, PGHI).
fn clip(len: usize) -> Vec<Vec<f32>> {
    (0..2)
        .map(|c| {
            (0..len)
                .map(|i| {
                    let t = i as f32 / 44100.0;
                    let hit = if i % 11025 < 300 {
                        (i % 7) as f32 / 7.0 - 0.5
                    } else {
                        0.0
                    };
                    let tone = (t * 220.0 * std::f32::consts::TAU + c as f32).sin() * 0.3;
                    let gap = if (i / 44100) % 3 == 2 { 0.0 } else { 1.0 };
                    (hit + tone) * gap
                })
                .collect()
        })
        .collect()
}

#[test]
fn process_never_allocates() {
    let clip = clip(44100 * 8);
    let source: Vec<&[f32]> = clip.iter().map(Vec::as_slice).collect();
    let mut out = vec![vec![0.0f32; 512]; 2];
    let mut st = StemStream::new(&Stem::default(), 2, 512);
    st.set_stagger(3, 16);
    st.set_ratio(1.08);
    st.seek(1000.0);
    for b in 0..600 {
        st.set_ratio(1.0 + 0.1 * ((b as f64) * 0.05).sin());
        let n = [512, 128, 300, 1][b % 4];
        let mut o: Vec<&mut [f32]> = out.iter_mut().map(|c| &mut c[..n]).collect();
        realtime(|| {
            if b % 150 == 149 {
                st.seek((b * 977 % 300_000) as f64);
                st.prime(&source);
            }
            st.process(&source, &mut o);
        });
    }
}

#[test]
fn output_independent_of_block_size() {
    let clip = clip(44100 * 4);
    let source: Vec<&[f32]> = clip.iter().map(Vec::as_slice).collect();
    let total = 44100 * 3;
    let render = |sizes: &[usize]| {
        let mut st = StemStream::new(&Stem::default(), 2, 4096);
        st.set_stagger(0, 1);
        st.set_ratio(1.25);
        st.seek(0.0);
        let mut out = vec![vec![0.0f32; total]; 2];
        let (mut done, mut i) = (0, 0);
        while done < total {
            let n = sizes[i % sizes.len()].min(total - done);
            let mut o: Vec<&mut [f32]> = out.iter_mut().map(|c| &mut c[done..done + n]).collect();
            st.process(&source, &mut o);
            done += n;
            i += 1;
        }
        out
    };
    let a = render(&[4096]);
    let b = render(&[1, 127, 512, 33, 2000]);
    assert_eq!(a, b, "output depends on block slicing");
}

/// Measured 31.8 dB; a single zeroed window sample per hop gives 25.8 dB.
const SNR_MIN_DB: f64 = 29.0;

/// At ratio 1 a steady tone must come back essentially unchanged. Catches broken windows,
/// normalisation or overlap-add (each would buzz once per hop).
#[test]
fn unity_ratio_reconstructs_a_tone() {
    let len = 44100 * 2;
    let tone: Vec<f32> = (0..len)
        .map(|i| (i as f32 * 440.0 / 44100.0 * std::f32::consts::TAU).sin() * 0.5)
        .collect();
    let clip = [tone.clone(), tone.clone()];
    let source: Vec<&[f32]> = clip.iter().map(Vec::as_slice).collect();
    let mut st = StemStream::new(&Stem::default(), 2, 4096);
    st.set_ratio(1.0);
    st.seek(0.0);
    let mut out = vec![vec![0.0f32; len]; 2];
    for done in (0..len).step_by(4096) {
        let n = 4096.min(len - done);
        let mut o: Vec<&mut [f32]> = out.iter_mut().map(|c| &mut c[done..done + n]).collect();
        st.process(&source, &mut o);
    }
    // At ratio 1 a stationary tone's phases advance exactly as in the input, so the waveform
    // itself should match. Compare away from the edges.
    let (a, b) = (8192, len - 8192);
    let sig: f64 = tone[a..b].iter().map(|&v| (v as f64).powi(2)).sum();
    let err: f64 = (a..b).map(|i| ((out[0][i] - tone[i]) as f64).powi(2)).sum();
    let snr = 10.0 * (sig / err.max(1e-30)).log10();
    assert!(
        snr > SNR_MIN_DB,
        "tone reconstructed at only {snr:.1} dB SNR"
    );
}

/// Identical channels take a shortcut (analysed once). It must match mono processing exactly.
#[test]
fn dual_mono_matches_mono() {
    let len = 44100 * 3;
    let mono = clip(len).remove(0);
    let render = |clip: Vec<Vec<f32>>| {
        let source: Vec<&[f32]> = clip.iter().map(Vec::as_slice).collect();
        let mut st = StemStream::new(&Stem::default(), clip.len(), 4096);
        st.set_stagger(0, 1);
        st.set_ratio(1.1);
        st.seek(0.0);
        let mut out = vec![vec![0.0f32; len]; clip.len()];
        for done in (0..len).step_by(4096) {
            let n = 4096.min(len - done);
            let mut o: Vec<&mut [f32]> = out.iter_mut().map(|c| &mut c[done..done + n]).collect();
            st.process(&source, &mut o);
        }
        out
    };
    let m = render(vec![mono.clone()]);
    let d = render(vec![mono.clone(), mono]);
    assert_eq!(d[0], d[1], "dual-mono channels differ");
    let worst = m[0]
        .iter()
        .zip(&d[0])
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(worst < 1e-5, "dual-mono differs from mono by {worst}");
}

/// Launching from a cue gives exactly what seek + prime gives, and allocates nothing.
#[test]
fn cue_launch_matches_seek_and_is_allocation_free() {
    let clip = clip(44100 * 4);
    let source: Vec<&[f32]> = clip.iter().map(Vec::as_slice).collect();
    let make = || {
        let mut st = StemStream::new(&Stem::default(), 2, 512);
        st.set_stagger(5, 16);
        st.set_ratio(1.1);
        st
    };
    let render = |st: &mut StemStream| {
        let mut out = vec![vec![0.0f32; 512 * 40]; 2];
        for b in 0..40 {
            let mut o: Vec<&mut [f32]> = out
                .iter_mut()
                .map(|c| &mut c[b * 512..(b + 1) * 512])
                .collect();
            st.process(&source, &mut o);
        }
        out
    };
    let mut a = make();
    a.seek(30_000.0);
    a.prime(&source);
    let expected = render(&mut a);

    let mut b = make();
    let cue = b.cue(&source, 30_000.0);
    // Play something else first, then launch.
    b.seek(5_000.0);
    render(&mut b);
    realtime(|| b.launch(&cue));
    assert_eq!(
        render(&mut b),
        expected,
        "cue launch differs from seek + prime"
    );
}

/// At exactly ratio 1 the input passes through untouched.
#[test]
fn bypass_is_bit_exact() {
    let clip = clip(44100 * 3);
    let source: Vec<&[f32]> = clip.iter().map(Vec::as_slice).collect();
    let mut st = StemStream::new(&Stem::default(), 2, 512);
    st.set_ratio(1.0);
    st.seek(1234.0);
    let mut out = vec![vec![0.0f32; 512]; 2];
    for b in 0..100 {
        let mut o: Vec<&mut [f32]> = out.iter_mut().map(|c| c.as_mut_slice()).collect();
        st.process(&source, &mut o);
        for c in 0..2 {
            assert_eq!(
                out[c][..],
                clip[c][1234 + b * 512..1234 + (b + 1) * 512],
                "block {b} channel {c}"
            );
        }
    }
}

/// Switching between ratio 1 (bypass) and stretching, repeatedly, on a steady tone: no clicks
/// (sample-to-sample jumps stay within the tone's own slope) and no level dips or bumps.
#[test]
fn bypass_transitions_are_seamless() {
    let len = 44100 * 12;
    let amp = 0.5f32;
    let w = 220.0 * std::f32::consts::TAU / 44100.0;
    let tone: Vec<f32> = (0..len).map(|i| (i as f32 * w).sin() * amp).collect();
    let clip = [tone.clone(), tone];
    let source: Vec<&[f32]> = clip.iter().map(Vec::as_slice).collect();
    let mut st = StemStream::new(&Stem::default(), 2, 512);
    st.set_ratio(1.03);
    st.seek(0.0);
    let blocks = 700;
    let mut out = vec![0.0f32; blocks * 512];
    let mut block = vec![vec![0.0f32; 512]; 2];
    for b in 0..blocks {
        // 1.03 → 1.0 → 0.97 → 1.0 → ... every 60 blocks (~0.7 s).
        st.set_ratio([1.03, 1.0, 0.97, 1.0][(b / 60) % 4]);
        let mut o: Vec<&mut [f32]> = block.iter_mut().map(|c| c.as_mut_slice()).collect();
        realtime(|| st.process(&source, &mut o));
        out[b * 512..(b + 1) * 512].copy_from_slice(&block[0]);
        if b % 60 == 59 {
            let unity = (b / 60) % 2 == 1;
            assert_eq!(
                st.is_bypassed(),
                unity,
                "bypass state wrong at end of section {}",
                b / 60
            );
        }
    }
    let skip = 8192; // start-up
    let max_step = out[skip..]
        .windows(2)
        .map(|p| (p[1] - p[0]).abs())
        .fold(0.0f32, f32::max);
    assert!(
        max_step < 1.5 * amp * w,
        "click: jump {max_step} vs tone slope {}",
        amp * w
    );
    let target = amp / 2f32.sqrt();
    for (i, win) in out[skip..].chunks(1024).enumerate() {
        let rms = (win.iter().map(|v| v * v).sum::<f32>() / win.len() as f32).sqrt();
        let db = 20.0 * (rms / target).log10();
        assert!(
            db.abs() < 1.0,
            "level {db:.2} dB at {:.2} s",
            (skip + i * 1024) as f32 / 44100.0
        );
    }
}
