use daw_stretch::StemStream;

#[test]
fn resident_windows_match_whole_clip_across_seeks_tempo_and_loops() {
    let clip: Vec<f32> = (0..9137)
        .map(|i| (i as f32 * 0.03).sin() * (i % 997) as f32 / 997.0)
        .collect();
    for rate in [44100.0, 96000.0, 192000.0] {
        let mut full = StemStream::for_sample_rate(1, 512, rate).unwrap();
        let mut win = StemStream::for_sample_rate(1, 512, rate).unwrap();
        full.set_looping(true);
        win.set_looping(true);
        full.set_stagger(0, 1);
        win.set_stagger(0, 1);
        let mut a = [0.0; 512];
        let mut b = [0.0; 512];
        for k in 0..120 {
            let ratio = [0.25, 0.8, 1.0, 1.25, 4.0][(k / 24) % 5];
            full.set_ratio(ratio);
            win.set_ratio(ratio);
            if k % 37 == 0 {
                full.seek((k * 823) as f64);
                win.seek((k * 823) as f64);
            }
            let n = if k % 2 == 0 { 127 } else { 512 };
            let (lo, hi) = win.source_bounds(n).unwrap();
            assert!(hi - lo <= 524288);
            let window: Vec<f32> = (lo..hi)
                .map(|i| clip[i.rem_euclid(clip.len() as i64) as usize])
                .collect();
            full.process(&[&clip], &mut [&mut a[..n]]);
            win.process_window(&[&window], lo, &mut [&mut b[..n]])
                .unwrap();
            for i in 0..n {
                assert!(
                    (a[i] - b[i]).abs() < 1e-5,
                    "rate={rate}, block={k}, sample={i}"
                );
            }
        }
    }
}
