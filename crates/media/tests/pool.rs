//! P1.3.3 — the media pool: enumeration + crash recovery (finalize un-finalized
//! takes, rebuild missing `.peaks`). The pool is take-id-addressed.

use std::mem;
use std::path::PathBuf;

use media::Pool;
use media::peaks::PeakFile;
use media::wav::{WavReader, WavWriter};

fn tmp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("media-pool-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write_finalized(dir: &std::path::Path, stem: &str, frames: u64, sample_rate: u32, value: f32) {
    let mut w = WavWriter::create_float(&dir.join(format!("{stem}.wav")), sample_rate, 1).unwrap();
    let mut buf = vec![0.0f32; 4096];
    let mut i = 0u64;
    while i < frames {
        let n = buf.len().min((frames - i) as usize);
        buf[..n].fill(value);
        w.write(&buf[..n]).unwrap();
        i += n as u64;
    }
    w.finalize().unwrap();
}

/// Write a "crashed" take: header with placeholder sizes + audio, but *no*
/// finalize (Drop is suppressed so best-effort finalize doesn't run).
fn write_crashed(dir: &std::path::Path, stem: &str, frames: u64, sample_rate: u32, value: f32) {
    let mut w = WavWriter::create_float(&dir.join(format!("{stem}.wav")), sample_rate, 1).unwrap();
    let mut buf = vec![0.0f32; 4096];
    let mut i = 0u64;
    while i < frames {
        let n = buf.len().min((frames - i) as usize);
        buf[..n].fill(value);
        w.write(&buf[..n]).unwrap();
        i += n as u64;
    }
    w.flush().unwrap();
    mem::forget(w); // suppress Drop's best-effort finalize — the "crash"
}

#[test]
fn list_returns_sources_with_frames_and_peaks_status() {
    let dir = tmp_dir("list");
    write_finalized(&dir, "take-1.ch0", 5000, 48_000, 0.5);
    write_finalized(&dir, "take-1.ch1", 3000, 48_000, -0.25); // no peaks written

    let pool = Pool::open(&dir).unwrap();
    let index = pool.list().unwrap();
    assert_eq!(index.sources.len(), 2);

    let c0 = index.sources.iter().find(|s| s.id == "take-1.ch0").unwrap();
    assert_eq!(c0.frames, 5000);
    assert_eq!(c0.sample_rate, 48_000);
    assert!(c0.peaks_missing, "ch0 has no sidecar in this test");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn recover_finalizes_a_crashed_take() {
    let dir = tmp_dir("crash");
    let frames = 1234u64;
    write_crashed(&dir, "take-2.ch0", frames, 48_000, 0.7);

    let pool = Pool::open(&dir).unwrap();
    // before recover, the take is not finalized
    assert!(!media::wav::WavWriter::is_finalized(&dir.join("take-2.ch0.wav")).unwrap());

    let report = pool.recover().unwrap();
    assert_eq!(report.finalized.len(), 1, "one crashed take recovered");
    assert_eq!(report.finalized[0].0, "take-2.ch0");
    assert_eq!(report.finalized[0].1, frames);

    // recovered file is now well-formed and readable to its full length
    assert!(media::wav::WavWriter::is_finalized(&dir.join("take-2.ch0.wav")).unwrap());
    let mut r = WavReader::open(&dir.join("take-2.ch0.wav")).unwrap();
    assert_eq!(r.total_frames(), frames);
    let mut back = vec![0.0f32; frames as usize];
    assert_eq!(r.read_into(&mut back), frames as usize);
    assert!(back.iter().all(|s| (*s - 0.7).abs() < 1e-4));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn recover_rebuilds_missing_peaks() {
    let dir = tmp_dir("peaks");
    write_finalized(&dir, "take-3.ch0", 4000, 44_100, 0.3);
    let peaks_path = dir.join("take-3.ch0.peaks");
    assert!(!peaks_path.exists());

    let pool = Pool::open(&dir).unwrap();
    let report = pool.recover().unwrap();
    assert_eq!(report.rebuilt_peaks, vec!["take-3.ch0".to_string()]);
    assert!(peaks_path.exists(), "peaks sidecar rebuilt");

    // the rebuilt sidecar is readable (PeakData = (base_bin, levels, frames, sample_rate, per-level))
    let (_, _, frames, sample_rate, _) = PeakFile::read(&peaks_path).unwrap();
    assert_eq!(frames, 4000);
    assert_eq!(sample_rate, 44_100);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn path_for_maps_stem_to_wav() {
    let dir = tmp_dir("path");
    write_finalized(&dir, "take-4.ch1", 1000, 48_000, 0.1);
    let pool = Pool::open(&dir).unwrap();
    assert_eq!(
        pool.path_for("take-4.ch1"),
        Some(dir.join("take-4.ch1.wav"))
    );
    assert!(pool.path_for("nope").is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

/// **A file whose name the log could not name still imports.** A space in a stem is
/// common (`My Take.wav`), but a pool id is a clip source id and `host v1` splits on
/// whitespace — so the stem is sanitized (`_`) instead of refused, and the caller reads
/// the real id back from the import. A path separator is still refused (a mangled path
/// would be a surprise).
#[test]
fn import_sanitizes_a_stem_the_log_could_not_name() {
    let dir = tmp_dir("stem-space");
    let outside = tmp_dir("stem-space-src");
    write_tone(&outside, "My Take", 1_000, 48_000, 440.0);
    let pool = Pool::open(&dir).unwrap();

    let imported = pool
        .import(&outside.join("My Take.wav"), 48_000)
        .expect("a space is sanitized, not fatal");
    assert_eq!(imported.id, "My_Take");
    assert_eq!(imported.ids(), vec!["My_Take"]);
    assert!(dir.join("My_Take.wav").is_file());
    assert_eq!(
        pool.path_for("My_Take"),
        Some(dir.join("My_Take.wav")),
        "and the sanitized id is the one the log can name"
    );
    assert!(
        pool.path_for("My Take").is_none(),
        "while the raw stem is not an id"
    );

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&outside);
}

/// A 440 Hz sine, so pitch survives as a countable property.
fn write_tone(dir: &std::path::Path, stem: &str, frames: u64, sample_rate: u32, freq: f64) {
    let mut w = WavWriter::create_float(&dir.join(format!("{stem}.wav")), sample_rate, 1).unwrap();
    let samples: Vec<f32> = (0..frames)
        .map(|i| (std::f64::consts::TAU * freq * i as f64 / sample_rate as f64).sin() as f32 * 0.5)
        .collect();
    w.write(&samples).unwrap();
    w.finalize().unwrap();
}

/// Read one channel of a WAV back in full, with its header facts.
fn read_all_ch(path: &std::path::Path, channel: u16) -> (Vec<f32>, u32, u64) {
    let mut r = WavReader::open(path)
        .unwrap()
        .with_channel(channel)
        .unwrap();
    let rate = r.sample_rate();
    let frames = r.total_frames();
    let mut buf = vec![0.0f32; frames as usize];
    assert_eq!(r.read_into(&mut buf), frames as usize);
    (buf, rate, frames)
}

/// Read channel 0 (the pool's own convention for a source).
fn read_all(path: &std::path::Path) -> (Vec<f32>, u32, u64) {
    read_all_ch(path, 0)
}

/// A stereo float file the *writer* can produce: distinct per-channel levels, so
/// a channel swap or a dropped channel is visible in the samples.
fn write_stereo(dir: &std::path::Path, stem: &str, frames: u64, sample_rate: u32, l: f32, r: f32) {
    let path = dir.join(format!("{stem}.wav"));
    let mut w = WavWriter::create_float(&path, sample_rate, 2).unwrap();
    let mut interleaved = Vec::with_capacity(frames as usize * 2);
    for _ in 0..frames {
        interleaved.push(l);
        interleaved.push(r);
    }
    w.write(&interleaved).unwrap();
    w.finalize().unwrap();
}

/// A float WAV with an arbitrary channel count, each channel a distinct constant
/// (`ch + i/1000`), so a mis-routed or stale channel is visible in the samples.
/// The writer is mono/stereo on purpose, so this hand-builds the header.
fn write_channels(dir: &std::path::Path, stem: &str, frames: u64, sample_rate: u32, channels: u16) {
    let path = dir.join(format!("{stem}.wav"));
    let ba = channels * 4;
    let data_bytes = (frames as usize * ba as usize * 4 / 4) as u32;
    let mut v = Vec::with_capacity(44 + data_bytes as usize);
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&3u16.to_le_bytes()); // IEEE float
    v.extend_from_slice(&channels.to_le_bytes());
    v.extend_from_slice(&sample_rate.to_le_bytes());
    v.extend_from_slice(&(sample_rate * ba as u32).to_le_bytes());
    v.extend_from_slice(&ba.to_le_bytes());
    v.extend_from_slice(&32u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data_bytes.to_le_bytes());
    for i in 0..frames {
        for ch in 0..channels {
            let value = ch as f32 + (i % 10) as f32 / 10.0;
            v.extend_from_slice(&value.to_le_bytes());
        }
    }
    std::fs::write(&path, v).unwrap();
}

/// Importing a stereo file splits it **at the boundary**: two mono pool sources
/// named the way capture names them, each holding one channel, so a clip is a
/// straight mono read and the mixer decides where the channel goes.
#[test]
fn a_stereo_file_imports_as_two_mono_sources() {
    let src_dir = tmp_dir("stereo-src");
    let pool_dir = tmp_dir("stereo-pool");
    write_stereo(&src_dir, "jam", 4800, 48_000, 0.25, -0.5);

    let pool = Pool::open(&pool_dir).unwrap();
    let done = pool.import(&src_dir.join("jam.wav"), 48_000).unwrap();

    assert_eq!(done.channels, 2);
    assert_eq!(done.ids(), vec!["jam.ch0", "jam.ch1"]);
    assert!(!done.converted(), "already at the session rate");
    assert_eq!(done.frames_out(), 4800);
    assert!(
        !pool_dir.join("jam.wav").exists(),
        "a split file must not also land as a whole-file source"
    );

    for (id, want) in [("jam.ch0", 0.25f32), ("jam.ch1", -0.5f32)] {
        let path = pool_dir.join(format!("{id}.wav"));
        let (audio, rate, frames) = read_all(&path);
        assert_eq!((rate, frames), (48_000, 4800), "{id}");
        assert!(
            audio.iter().all(|s| (s - want).abs() < 1e-6),
            "{id} must hold its own channel"
        );
        assert!(
            path.with_extension("peaks").is_file(),
            "{id} needs its peaks"
        );
    }

    // The index agrees: two mono sources at the session rate.
    let index = pool.list().unwrap();
    assert_eq!(index.errors, Vec::new());
    assert_eq!(index.sources.len(), 2);
    assert!(index.sources.iter().all(|s| s.channels == 1));

    let _ = std::fs::remove_dir_all(&src_dir);
    let _ = std::fs::remove_dir_all(&pool_dir);
}

/// A stereo file that is *already* in the pool (hand-filled, or written by an
/// older build) is split by `conform`: `{id}` keeps meaning channel 0 — clips
/// that referenced it still play what they played — and channel 1 becomes
/// addressable instead of silently dropped.
/// Importing over an existing id **replaces** it: a stereo file whose stem is
/// already a mono pool source leaves the split channels and no stale source
/// behind (otherwise a clip on that id keeps playing the old material).
#[test]
fn a_stereo_import_replaces_the_mono_source_holding_that_id() {
    let src_dir = tmp_dir("replace-src");
    let pool_dir = tmp_dir("replace-pool");
    write_finalized(&pool_dir, "jam", 4800, 48_000, 0.25); // the existing, mono source
    write_stereo(&src_dir, "jam", 4800, 48_000, 0.5, -0.5);

    let pool = Pool::open(&pool_dir).unwrap();
    let done = pool.import(&src_dir.join("jam.wav"), 48_000).unwrap();
    assert_eq!(done.ids(), vec!["jam.ch0", "jam.ch1"]);

    let index = pool.list().unwrap();
    assert_eq!(index.sources.len(), 2, "the old mono source is gone");
    assert!(
        !pool_dir.join("jam.wav").exists(),
        "a replaced id leaves no stale source"
    );
    let (left, _, _) = read_all(&pool_dir.join("jam.ch0.wav"));
    assert!(left.iter().all(|s| (s - 0.5).abs() < 1e-6));
    let (right, _, _) = read_all(&pool_dir.join("jam.ch1.wav"));
    assert!(right.iter().all(|s| (s + 0.5).abs() < 1e-6));

    let _ = std::fs::remove_dir_all(&src_dir);
    let _ = std::fs::remove_dir_all(&pool_dir);
}

/// Importing **replaces the id**: a narrower file imported under a stem that
/// held a wider one leaves no channel of the old material behind. (The first
/// draft removed only `{id}.wav`, so `jam.ch2`… of a former 4-channel take
/// stayed addressable and a clip on them played audio the user had replaced.)
#[test]
fn a_narrower_import_removes_the_older_channels() {
    let src_dir = tmp_dir("narrow-src");
    let pool_dir = tmp_dir("narrow-pool");
    write_channels(&pool_dir, "jam", 4800, 48_000, 4); // the existing 4-channel take
    write_stereo(&src_dir, "jam", 4800, 48_000, 0.25, -0.5);

    let pool = Pool::open(&pool_dir).unwrap();
    let done = pool.import(&src_dir.join("jam.wav"), 48_000).unwrap();
    assert_eq!(done.ids(), vec!["jam.ch0", "jam.ch1"]);

    let mut left: Vec<String> = pool
        .list()
        .unwrap()
        .sources
        .iter()
        .map(|s| s.id.clone())
        .collect();
    left.sort();
    assert_eq!(
        left,
        vec!["jam.ch0", "jam.ch1"],
        "the replaced take's extra channels must not survive"
    );
    for stale in [
        "jam.wav",
        "jam.ch2.wav",
        "jam.ch3.wav",
        "jam.ch2.peaks",
        "jam.ch3.peaks",
    ] {
        assert!(
            !pool_dir.join(stale).exists(),
            "{stale} outlived the import that replaced it"
        );
    }

    let _ = std::fs::remove_dir_all(&src_dir);
    let _ = std::fs::remove_dir_all(&pool_dir);
}

/// A **torn sibling** (an interrupted split, or a hand-placed truncated file) is
/// re-derived by the next pass instead of being trusted: `{id}.ch{k}` means
/// "channel k of `{id}`", so the pass overwrites it through a temporary name.
#[test]
fn a_torn_sibling_is_re_derived_not_trusted() {
    let dir = tmp_dir("torn-sibling");
    write_stereo(&dir, "jam", 4800, 48_000, 0.25, -0.5);

    // A sibling that died mid-write: placeholder header, never finalized, and it
    // holds the wrong channel's audio (0.75), so trusting it is visible.
    {
        let mut w = WavWriter::create_float(&dir.join("jam.ch1.wav"), 48_000, 1).unwrap();
        w.write(&vec![0.75f32; 100]).unwrap();
        w.flush().unwrap();
        // No finalize, and Drop suppressed: the "crash" (Drop would patch the
        // header best-effort, which is exactly what must not have happened).
        mem::forget(w);
    }
    assert!(
        !WavWriter::is_finalized(&dir.join("jam.ch1.wav")).unwrap(),
        "the fixture must look like a crashed take"
    );

    let pool = Pool::open(&dir).unwrap();
    let report = pool.conform(48_000).unwrap();
    assert_eq!(report.errors, Vec::new());
    assert_eq!(
        report
            .converted
            .iter()
            .map(|c| c.id.as_str())
            .collect::<Vec<_>>(),
        vec!["jam", "jam.ch1"]
    );

    let sibling = dir.join("jam.ch1.wav");
    assert!(
        WavWriter::is_finalized(&sibling).unwrap(),
        "the re-derived sibling is a complete take"
    );
    let (right, _, frames) = read_all(&sibling);
    assert_eq!(frames, 4800, "the full channel, not the torn 100 frames");
    assert!(
        right.iter().all(|s| (s + 0.5).abs() < 1e-6),
        "channel 1's own audio, not the torn file's"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Preserving an original never overwrites an earlier preservation: the backup
/// name is numbered when it is taken. (The first draft's `hard_link` failed on an
/// existing name and the `fs::copy` fallback silently truncated it.)
#[test]
fn preserving_an_original_does_not_clobber_an_earlier_backup() {
    let dir = tmp_dir("backup-clash");
    write_stereo(&dir, "jam", 4800, 48_000, 0.25, -0.5);
    // An older preservation of the same file already sits beside it.
    let older = dir.join("jam.wav.pre2ch");
    std::fs::write(&older, b"an older preservation").unwrap();

    let pool = Pool::open(&dir).unwrap();
    let report = pool.conform(48_000).unwrap();
    let done = &report.converted[0];
    let backup = done.preserved.clone().expect("the original is kept");
    assert_ne!(backup, older, "the older backup keeps its name");
    assert_eq!(std::fs::read(&older).unwrap(), b"an older preservation");
    let (audio, _, _) = read_all(&backup);
    assert!(audio.iter().all(|s| (s - 0.25).abs() < 1e-6));

    let _ = std::fs::remove_dir_all(&dir);
}

/// Importing a file that **is already the pool's source** must report what it
/// found, not an empty result: the shell's `--wave` uses `import` for every
/// path, including one inside the session pool.
#[test]
fn importing_a_pool_source_over_itself_reports_it() {
    let dir = tmp_dir("self-import");
    write_finalized(&dir, "jam", 4800, 48_000, 0.25);
    write_stereo(&dir, "two", 4800, 48_000, 0.25, -0.5);

    let pool = Pool::open(&dir).unwrap();

    // A mono source at the session rate: nothing to do, and no backup appears.
    let mono = pool.import(&dir.join("jam.wav"), 48_000).unwrap();
    assert_eq!(mono.ids(), vec!["jam"]);
    assert_eq!(mono.frames_out(), 4800, "the source's length is reported");
    assert!(!mono.converted());
    assert!(
        !dir.join("jam.wav.pre48000").exists(),
        "a no-op leaves no backup"
    );

    // A stereo source in the pool: importing it over itself splits it in place,
    // and the result lists **every** source the material now has — a shell
    // places tracks from this list, so `two` alone would drop the right channel.
    let stereo = pool.import(&dir.join("two.wav"), 48_000).unwrap();
    assert_eq!(stereo.channels, 2);
    assert_eq!(stereo.ids(), vec!["two", "two.ch1"]);
    assert_eq!(stereo.sources[0].extracted, vec!["two.ch1"]);
    assert_eq!(stereo.frames_out(), 4800);
    assert!(dir.join("two.ch1.wav").is_file());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn conform_splits_a_stereo_source_already_in_the_pool() {
    let dir = tmp_dir("conform-stereo");
    write_stereo(&dir, "jam", 4800, 48_000, 0.25, -0.5);

    let pool = Pool::open(&dir).unwrap();
    let report = pool.conform(48_000).unwrap();
    assert_eq!(report.errors, Vec::new());
    // One entry per pool source the pass wrote: the file's own `{id}` first
    // (listing the siblings it produced), then each sibling.
    assert_eq!(report.converted.len(), 2);
    assert_eq!(
        report
            .converted
            .iter()
            .map(|c| c.id.as_str())
            .collect::<Vec<_>>(),
        vec!["jam", "jam.ch1"]
    );
    let done = &report.converted[0];
    assert_eq!(done.id, "jam");
    assert_eq!(done.channels, 2);
    assert_eq!(done.extracted, vec!["jam.ch1"]);
    assert_eq!(report.converted[1].frames_out, 4800);
    assert!(!done.converted, "the rate was already right");
    let preserved = done.preserved.clone().expect("the original is kept");
    assert_eq!(preserved, dir.join("jam.wav.pre2ch"));

    // The original survives intact — both channels — under a name the pool does
    // not index.
    let (audio, rate, frames) = read_all(&preserved);
    assert_eq!((rate, frames), (48_000, 4800));
    assert!(audio.iter().all(|s| (s - 0.25).abs() < 1e-6));
    let (right, _, _) = read_all_ch(&preserved, 1);
    assert!(right.iter().all(|s| (s + 0.5).abs() < 1e-6));

    // `jam` is now channel 0, mono and at the session rate.
    let (audio, rate, frames) = read_all(&dir.join("jam.wav"));
    assert_eq!((rate, frames), (48_000, 4800));
    assert!(audio.iter().all(|s| (s - 0.25).abs() < 1e-6));
    let (right, _, _) = read_all(&dir.join("jam.ch1.wav"));
    assert!(right.iter().all(|s| (s + 0.5).abs() < 1e-6));

    let index = pool.list().unwrap();
    assert_eq!(index.sources.len(), 2, "the backup is not a source");
    assert!(
        index.sources.iter().all(|s| s.channels == 1),
        "the pool is mono after a split: {:?}",
        index.sources.iter().map(|s| &s.id).collect::<Vec<_>>()
    );

    // Idempotent: a second pass finds nothing to do and extracts nothing twice.
    let again = pool.conform(48_000).unwrap();
    assert!(again.converted.is_empty());
    assert!(again.errors.is_empty());

    let _ = std::fs::remove_dir_all(&dir);
}

/// The two halves compose: a foreign-rate stereo file is resampled *and* split,
/// both channels landing at the session rate with the original preserved.
#[test]
fn conform_resamples_and_splits_a_stereo_source() {
    let dir = tmp_dir("conform-stereo-rate");
    write_stereo(&dir, "jam", 44_100, 44_100, 0.25, -0.5);

    let pool = Pool::open(&dir).unwrap();
    let report = pool.conform(48_000).unwrap();
    assert_eq!(report.errors, Vec::new());
    let done = &report.converted[0];
    assert!(done.converted, "44.1 kHz was resampled");
    assert_eq!(done.channels, 2);
    assert_eq!(done.extracted, vec!["jam.ch1"]);
    assert_eq!(done.preserved, Some(dir.join("jam.wav.pre44100")));

    let index = pool.list().unwrap();
    assert_eq!(index.sources.len(), 2);
    assert!(index.sources.iter().all(|s| s.sample_rate == 48_000));
    for (id, want) in [("jam", 0.25f32), ("jam.ch1", -0.5f32)] {
        let (audio, rate, frames) = read_all(&dir.join(format!("{id}.wav")));
        assert_eq!((rate, frames), (48_000, 48_000), "{id}");
        let middle = &audio[256..audio.len() - 256];
        assert!(
            middle.iter().all(|s| (s - want).abs() < 0.01),
            "{id} holds its own channel at the right level"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn import_converts_a_foreign_rate_once() {
    // The reported bug: a 44.1 kHz file in a 48 kHz session was refused outright.
    let src_dir = tmp_dir("import-src");
    let pool_dir = tmp_dir("import-pool");
    write_tone(&src_dir, "jam", 44_100, 44_100, 440.0);

    let pool = Pool::open(&pool_dir).unwrap();
    let done = pool.import(&src_dir.join("jam.wav"), 48_000).unwrap();
    assert_eq!(done.channels, 1, "a mono file is one source");
    assert_eq!(done.ids(), vec!["jam"]);
    let done = &done.sources[0];

    assert!(done.converted, "44.1 kHz must be resampled");
    assert_eq!((done.from_rate, done.to_rate), (44_100, 48_000));
    assert_eq!((done.frames_in, done.frames_out), (44_100, 48_000));
    assert_eq!(done.preserved, None, "imported material stays where it was");

    // The pool now holds a session-rate source…
    let index = pool.list().unwrap();
    assert_eq!(index.sources.len(), 1);
    assert_eq!(index.sources[0].id, "jam");
    assert_eq!(index.sources[0].sample_rate, 48_000);
    assert_eq!(index.sources[0].frames, 48_000);
    assert!(!index.sources[0].peaks_missing, "import derives the peaks");

    // …whose audio is the same tone at full level, not a pitch-shifted copy.
    let (audio, rate, frames) = read_all(&pool_dir.join("jam.wav"));
    assert_eq!((rate, frames), (48_000, 48_000));
    let crossings = audio
        .windows(2)
        .filter(|w| (w[0] > 0.0) != (w[1] > 0.0))
        .count();
    assert!(
        (crossings as i64 - 880).abs() <= 2,
        "440 Hz over 1 s should cross zero ~880 times, got {crossings}"
    );
    let middle = &audio[256..audio.len() - 256];
    let rms = (middle
        .iter()
        .map(|s| (*s as f64) * (*s as f64))
        .sum::<f64>()
        / middle.len() as f64)
        .sqrt();
    assert!(
        (rms - 0.5 / std::f64::consts::SQRT_2).abs() < 0.01,
        "level changed on import: rms {rms:.4}"
    );

    let _ = std::fs::remove_dir_all(&src_dir);
    let _ = std::fs::remove_dir_all(&pool_dir);
}

#[test]
fn import_copies_a_matching_rate_bit_for_bit() {
    let src_dir = tmp_dir("import-copy-src");
    let pool_dir = tmp_dir("import-copy-pool");
    // 16-bit PCM: a re-quantizing import would show up as different bytes.
    let mut w = WavWriter::create(&src_dir.join("take.wav"), 48_000, 1).unwrap();
    w.write(&vec![0.25f32; 4800]).unwrap();
    w.finalize().unwrap();

    let pool = Pool::open(&pool_dir).unwrap();
    let done = pool.import(&src_dir.join("take.wav"), 48_000).unwrap();
    assert!(!done.converted(), "a matching rate must not be resampled");
    let done = &done.sources[0];
    assert!(!done.converted, "a matching rate must not be resampled");
    assert_eq!((done.frames_in, done.frames_out), (4800, 4800));
    assert_eq!(
        std::fs::read(src_dir.join("take.wav")).unwrap(),
        std::fs::read(pool_dir.join("take.wav")).unwrap(),
        "the copy must be byte-identical"
    );

    let _ = std::fs::remove_dir_all(&src_dir);
    let _ = std::fs::remove_dir_all(&pool_dir);
}

/// **A rendered source can be written into the pool** — the seam an offline transform
/// (the time-stretch) uses to materialise new material. The conventions stay in the
/// pool: mono float on disk, derived peaks, a temporary file then a rename, and the
/// frames actually written returned (the log records a measurement, not a guess).
#[test]
fn a_rendered_source_is_written_into_the_pool() {
    let dir = tmp_dir("write-source");
    let pool = Pool::open(&dir).unwrap();

    let samples: Vec<f32> = (0..4_800).map(|i| (i as f32 / 4_800.0) - 0.5).collect();
    let frames = pool
        .write_source("jam.stretch.3_2", &samples, 48_000)
        .unwrap();
    assert_eq!(
        frames, 4_800,
        "the frame count is the file's, not an estimate"
    );

    let index = pool.list().unwrap();
    assert_eq!(index.errors, Vec::new());
    assert_eq!(index.sources.len(), 1);
    let source = &index.sources[0];
    assert_eq!(source.id, "jam.stretch.3_2");
    assert_eq!(
        (source.frames, source.sample_rate, source.channels),
        (4_800, 48_000, 1)
    );
    assert!(
        !source.peaks_missing && source.finalized,
        "peaks derived, take complete"
    );

    // The samples come back bit-for-bit (float on disk, mono, one channel).
    let (back, _, _) = read_all(&dir.join("jam.stretch.3_2.wav"));
    assert_eq!(back.len(), samples.len());
    assert!(
        back.iter()
            .zip(&samples)
            .all(|(a, b)| a.to_bits() == b.to_bits())
    );
    // …and no temporary file is left behind.
    assert!(
        !dir.join("jam.stretch.3_2.converting").exists(),
        "the staged write is renamed, never left"
    );

    // Writing the **same id again replaces it** — the deliberate behaviour behind a
    // deterministic render id (stretch, undo, stretch renders to the same name), and the
    // one path that needs a remove-then-rename fallback on a platform whose `rename`
    // refuses an existing destination.
    let fewer = &samples[..2_400];
    assert_eq!(
        pool.write_source("jam.stretch.3_2", fewer, 48_000).unwrap(),
        2_400
    );
    let index = pool.list().unwrap();
    assert_eq!(index.sources.len(), 1, "replaced, not duplicated");
    assert_eq!(index.sources[0].frames, 2_400, "the index follows the file");

    // The guards: an unusable id, an empty render, and a zero rate are refused — and so
    // is a **whitespace-bearing** id, which no `host v1` line could name back (the gate
    // found `valid_id` accepting one while the doc promised it would not).
    assert!(pool.write_source("a/b", &samples, 48_000).is_err());
    assert!(pool.write_source("", &samples, 48_000).is_err());
    assert!(pool.write_source("two words", &samples, 48_000).is_err());
    assert!(pool.write_source("tab\tid", &samples, 48_000).is_err());
    assert!(pool.write_source("ok", &[], 48_000).is_err());
    assert!(pool.write_source("ok", &samples, 0).is_err());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn conform_brings_a_hand_filled_pool_to_the_session_rate() {
    let dir = tmp_dir("conform");
    write_tone(&dir, "old.ch0", 44_100, 44_100, 440.0); // foreign rate
    write_finalized(&dir, "old.ch1", 48_000, 48_000, 0.1); // already fine

    let pool = Pool::open(&dir).unwrap();
    let report = pool.conform(48_000).unwrap();

    assert_eq!(
        report.converted.len(),
        1,
        "only the foreign source is touched"
    );
    let done = &report.converted[0];
    assert_eq!(done.id, "old.ch0");
    assert_eq!((done.from_rate, done.to_rate), (44_100, 48_000));
    assert_eq!((done.frames_in, done.frames_out), (44_100, 48_000));

    // The original is preserved under a name the pool does not index.
    let preserved = done.preserved.clone().expect("the original is kept");
    assert_eq!(preserved, dir.join("old.ch0.wav.pre44100"));
    let (_, rate, frames) = read_all(&preserved);
    assert_eq!((rate, frames), (44_100, 44_100));

    // The pool now reads as one rate, with peaks rebuilt for the new length.
    let index = pool.list().unwrap();
    assert_eq!(
        index.sources.len(),
        2,
        "the preserved original is not a source"
    );
    assert!(index.sources.iter().all(|s| s.sample_rate == 48_000));
    let (_, _, frames, rate, _) = PeakFile::read(&dir.join("old.ch0.peaks")).unwrap();
    assert_eq!((frames, rate), (48_000, 48_000));

    // Idempotent: a second pass has nothing to do.
    let again = pool.conform(48_000).unwrap();
    assert!(again.converted.is_empty());
    assert!(again.errors.is_empty());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn import_and_conform_refuse_what_they_cannot_do() {
    let dir = tmp_dir("conform-guards");
    write_finalized(&dir, "s1", 1000, 44_100, 0.1); // foreign, so conform has work
    let pool = Pool::open(&dir).unwrap();

    assert!(
        pool.import(&dir.join("s1.wav"), 0).is_err(),
        "zero session rate"
    );
    assert!(
        pool.import(&dir.join("missing.wav"), 48_000).is_err(),
        "missing source"
    );
    assert!(pool.conform(0).is_err(), "zero session rate");

    // An unreadable file is reported, not fatal: the readable one still conforms.
    std::fs::write(dir.join("broken.wav"), b"not a wav").unwrap();
    let report = pool.conform(48_000).unwrap();
    assert_eq!(report.errors.len(), 1, "the broken file is reported");
    assert!(report.errors[0].0.ends_with("broken.wav"));
    assert!(
        report.converted.iter().any(|c| c.id == "s1"),
        "the readable source is still imported"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
