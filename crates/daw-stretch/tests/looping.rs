use daw_stretch::StemStream;

#[test]
fn looping_matches_a_physically_repeated_source() {
    let clip: Vec<f32> = (0..4096)
        .map(|i| (i as f32 * std::f32::consts::TAU / 128.0).sin())
        .collect();
    let repeated = clip.repeat(32);
    for ratio in [0.8, 1.0, 1.25] {
        let mut looping = StemStream::for_sample_rate(1, 512, 44100.0).unwrap();
        let mut linear = StemStream::for_sample_rate(1, 512, 44100.0).unwrap();
        looping.set_looping(true);
        looping.set_stagger(0, 1);
        linear.set_stagger(0, 1);
        looping.set_ratio(ratio);
        linear.set_ratio(ratio);
        looping.seek(16384.0);
        linear.seek(16384.0);
        let mut a = [0.0; 512];
        let mut b = [0.0; 512];
        for _ in 0..40 {
            looping.process(&[&clip], &mut [&mut a]);
            linear.process(&[&repeated], &mut [&mut b]);
            for (x, y) in a.iter().zip(b.iter()) {
                assert!((x - y).abs() < 1e-5);
            }
        }
    }
}

#[test]
fn periodic_bypass_reads_both_sides_of_the_seam() {
    let mut stream = StemStream::for_sample_rate(1, 8, 48000.0).unwrap();
    stream.set_looping(true);
    stream.seek(-2.0);
    let mut out = [0.0; 8];
    stream.process(&[&[1.0, 2.0, 3.0]], &mut [&mut out]);
    assert_eq!(out, [2.0, 3.0, 1.0, 2.0, 3.0, 1.0, 2.0, 3.0]);
}

#[test]
fn sample_rate_constructor_validates_host_configuration() {
    for rate in [44100.0, 48000.0, 96000.0, 192000.0] {
        let mut s = StemStream::for_sample_rate(2, 128, rate).unwrap();
        s.set_ratio(1.2);
        s.set_looping(true);
        s.seek(0.0);
        let source = [0.25; 4096];
        let mut l = [0.0; 128];
        let mut r = [0.0; 128];
        s.process(&[&source, &source], &mut [&mut l, &mut r]);
        assert!(l.iter().chain(r.iter()).all(|v| v.is_finite()));
    }
    assert!(StemStream::for_sample_rate(0, 128, 44100.0).is_err());
    assert!(StemStream::for_sample_rate(2, 0, 44100.0).is_err());
    assert!(StemStream::for_sample_rate(2, 128, f64::NAN).is_err());
}
