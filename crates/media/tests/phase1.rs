//! Phase 1 profile-level tests: bounce. The mixer plugin (engine) owns the
//! master bus; `media::bounce` renders it to a 16-bit WAV that round-trips
//! the rendered output.


use engine::*;
use media::*;

const SR: u32 = 48_000;

fn engine() -> Engine {
    let mut e = Engine::new(SR, 120.0, 4);
    e.register_factory("euclidean", plugins::euclidean_factory, plugins::euclidean::EUCLIDEAN_PORTS, &[]);
    e.register_factory("scale", plugins::scale_factory, plugins::scale::SCALE_PORTS, &[]);
    e.register_factory("tone", plugins::tone_factory, plugins::tone::TONE_PORTS, plugins::tone::TONE_PARAMS);
    e.register_factory("mixer", plugins::mixer_factory, plugins::mixer::MIXER_PORTS, plugins::mixer::MIXER_PARAMS);
    e
}

fn chain(e: &mut Engine) {
    e.mount(
        "euclidean",
        &[("steps", 8.0), ("pulses", 1.0), ("rotation", 1.0), ("pulses_per_beat", 4.0)],
    )
    .unwrap();
    e.mount("scale", &[("root", 0.0)]).unwrap();
    e.mount("tone", &[("gain", 0.25), ("blip_len", 1200.0)]).unwrap();
    e.mount("mixer", &[]).unwrap();
    e.patch(("euclidean", "triggers"), ("scale", "trigger")).unwrap();
    e.patch(("scale", "note"), ("tone", "note")).unwrap();
    e.patch(("tone", "audio"), ("mixer", "ch0")).unwrap();
}

/// Bounce writes the master out to a readable WAV that round-trips the render
/// (16-bit quantization tolerance). The expected master comes from replaying
/// the same log on a fresh engine — the log is the source of truth.
#[test]
fn bounce_to_wav_roundtrips_the_master() {
    let path = std::env::temp_dir().join(format!("phase1-bounce-{}.wav", std::process::id()));
    let frames = 10_000usize;

    let mut e = engine();
    chain(&mut e);
    media::bounce(&mut e, frames, &path).unwrap();

    let mut expected = engine();
    expected.replay_from(&e.log).unwrap();
    let out = expected.render(frames);
    assert!(out[6001..].iter().any(|s| *s != 0.0), "the chain sounds through the mixer");

    let recorded = read_all(&path); // channel 0 (L) of the stereo bounce
    assert_eq!(recorded.len(), frames, "the bounce holds every frame");
    // `out` is the interleaved stereo master; its channel 0 (L) must match the
    // bounced L channel (the chain is a single center-panned channel on ch0).
    for (a, b) in out.as_chunks::<2>().0.iter().map(|p| p[0]).zip(&recorded) {
        assert!((a - b).abs() < 2e-4, "master {a} vs bounce {b}");
    }
    let _ = std::fs::remove_file(&path);
}

fn read_all(path: &std::path::Path) -> Vec<f32> {
    let mut r = WavReader::open(path).unwrap();
    let mut out = vec![0.0f32; r.total_frames() as usize];
    assert_eq!(r.read_into(&mut out), out.len());
    out
}
