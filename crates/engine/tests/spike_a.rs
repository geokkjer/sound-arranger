//! Spike A.5 acceptance tests (patch-bay note):
//! - a patched chain `euclidean.triggers → scale.trigger → scale.note →
//!   tone.note → tone.audio` renders audio whose pitch follows the scale;
//! - type-mismatched patches are refused at connect (fail-loud);
//! - patching is logged; replay reproduces the identical signal;
//! - the patch registry (`providers_of`) lists external sources among providers;
//! - Spike A invariants hold under the port model: sample-accurate lifecycle,
//!   determinism, no residual sound after unmount, zero-allocation render.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

use engine::*;

const SR: u32 = 48_000;

/// The master is now the mixer's stereo bus; de-interleave to the L channel for
/// frame-aligned mono assertions (a center-panned mono source appears on both
/// channels, so L carries the signal).
fn l(buf: &[f32]) -> Vec<f32> {
    buf.as_chunks::<2>().0.iter().map(|p| p[0]).collect()
}

fn engine() -> Engine {
    let mut e = Engine::new(SR, 120.0, 4);
    e.register_factory(
        "euclidean",
        plugins::euclidean_factory,
        plugins::euclidean::EUCLIDEAN_PORTS,
        &[],
    );
    e.register_factory(
        "scale",
        plugins::scale_factory,
        plugins::scale::SCALE_PORTS,
        &[],
    );
    e.register_factory(
        "tone",
        plugins::tone_factory,
        plugins::tone::TONE_PORTS,
        plugins::tone::TONE_PARAMS,
    );
    e.register_factory(
        "mixer",
        plugins::mixer_factory,
        plugins::mixer::MIXER_PORTS,
        plugins::mixer::MIXER_PARAMS,
    );
    e
}

/// The canonical patch: euclidean triggers → scale → tone → audio out.
/// Pure setup — no render: the mounts and patches are scheduled at frame 0 and
/// applied by the test's own first render (so every test renders from the same
/// absolute start as its replay counterpart).
fn mount_chain(e: &mut Engine) {
    e.mount(
        "euclidean",
        &[
            ("steps", 8.0),
            ("pulses", 3.0),
            ("rotation", 0.0),
            ("pulses_per_beat", 4.0),
        ],
    )
    .unwrap();
    e.mount("scale", &[("root", 0.0), ("note_len", 400.0)])
        .unwrap();
    e.mount("tone", &[("gain", 0.2), ("blip_len", 800.0)])
        .unwrap();
    e.patch(("euclidean", "triggers"), ("scale", "trigger"))
        .unwrap();
    e.patch(("scale", "note"), ("tone", "note")).unwrap();
    // Phase 1: the mixer owns the master bus — the chain's audio routes
    // through it (tone.audio → mixer.ch0); the tone no longer claims the out.
    e.mount("mixer", &[]).unwrap();
    e.patch(("tone", "audio"), ("mixer", "ch0")).unwrap();
}

/// The patch works end to end: a trigger lands sample-accurately at frame 6000
/// (E(8,1) rotated by 1 → pulse at step 1), and nothing sounds before it.
#[test]
fn chain_trigger_is_sample_accurate() {
    let mut e = engine();
    e.mount(
        "euclidean",
        &[
            ("steps", 8.0),
            ("pulses", 1.0),
            ("rotation", 1.0),
            ("pulses_per_beat", 4.0),
        ],
    )
    .unwrap();
    e.mount("scale", &[("root", 0.0)]).unwrap();
    e.mount("tone", &[("gain", 0.25), ("blip_len", 1200.0)])
        .unwrap();
    e.patch(("euclidean", "triggers"), ("scale", "trigger"))
        .unwrap();
    e.patch(("scale", "note"), ("tone", "note")).unwrap();
    e.mount("mixer", &[]).unwrap();
    e.patch(("tone", "audio"), ("mixer", "ch0")).unwrap();
    let out = l(&e.render(7000));

    assert_eq!(out[5999], 0.0);
    assert_eq!(out[6000], 0.0, "onset sample is sin(0)");
    assert!(
        out[6001] > 0.0,
        "blip must begin at frame 6001 (onset at 6000)"
    );
}

/// The pitch flows through the patch: changing the scale root changes the tone's
/// frequency (measured by zero crossings, robust to the decay envelope).
#[test]
fn scale_changes_pitch() {
    let crossings = |root: f32| -> usize {
        let mut e = engine();
        e.mount(
            "euclidean",
            &[
                ("steps", 8.0),
                ("pulses", 1.0),
                ("rotation", 1.0),
                ("pulses_per_beat", 4.0),
            ],
        )
        .unwrap();
        e.mount("scale", &[("root", root)]).unwrap();
        e.mount("tone", &[("gain", 0.25), ("blip_len", 800.0)])
            .unwrap();
        e.patch(("euclidean", "triggers"), ("scale", "trigger"))
            .unwrap();
        e.patch(("scale", "note"), ("tone", "note")).unwrap();
        e.mount("mixer", &[]).unwrap();
        e.patch(("tone", "audio"), ("mixer", "ch0")).unwrap();
        let out = l(&e.render(7000));
        // count sign changes in the blip window (6001..6800)
        let window = &out[6001..6800];
        let mut n = 0usize;
        for w in window.windows(2) {
            if (w[0] > 0.0) != (w[1] > 0.0) {
                n += 1;
            }
        }
        n
    };

    let low = crossings(0.0); // A4 → 440 Hz
    let high = crossings(12.0); // octave up → 880 Hz
    assert!(low > 0 && high > 0, "both must sound");
    assert!(
        high > low * 3 / 2 && high < low * 2 + 4,
        "an octave ≈ twice the crossings: {low} vs {high}"
    );
}

/// A type-mismatched patch (Trigger → Note) is refused at patch time, and so
/// are patches referencing unknown ports, wrong directions, or unknown plugins.
#[test]
fn patch_type_mismatch_refused() {
    let mut e = engine();
    e.mount("euclidean", &[]).unwrap();
    e.mount("tone", &[]).unwrap();
    // kinds differ: Trigger out vs Note in
    let err = e
        .patch(("euclidean", "triggers"), ("tone", "note"))
        .unwrap_err();
    assert!(err.contains("signal kind mismatch"), "got: {err}");
    // wrong direction: tone.audio is an Out port
    let err = e
        .patch(("euclidean", "triggers"), ("tone", "audio"))
        .unwrap_err();
    assert!(err.contains("must be an Out port"), "got: {err}");
    let err = e
        .patch(("euclidean", "nope"), ("tone", "note"))
        .unwrap_err();
    assert!(err.contains("no port 'nope'"), "got: {err}");
    let err = e
        .patch(("euclidean", "triggers"), ("ghost", "note"))
        .unwrap_err();
    assert!(err.contains("unknown plugin 'ghost'"), "got: {err}");
}

/// A cord the **graph** will refuse is refused here, before it is logged. `connect`
/// runs a cord from an earlier node to a later one, and a plugin's node lands in
/// the order its mount applied — so mounting the sink first and patching a source
/// into it is a document entry that can never become audio. `validate_patch` used
/// to check existence, direction, kind and channel count, and not order: the patch
/// was logged, `connect` returned `Err` at apply, and the only report was a
/// `debug_assert!` — a silent unfed channel in the build a user runs, a panic on
/// the audio thread in the other.
#[test]
fn a_backward_patch_is_refused_before_it_is_logged() {
    let mut e = engine();
    // Sink first: the mixer takes node 0, the tone node 1.
    e.mount("mixer", &[]).unwrap();
    e.mount("tone", &[("gain", 0.25)]).unwrap();
    let err = e
        .patch(("tone", "audio"), ("mixer", "ch0"))
        .expect_err("a cord must go forward in node order");
    assert!(err.contains("must go forward"), "got: {err}");
    assert!(
        err.contains("mount 'mixer' first"),
        "and the refusal names the fix: {err}"
    );

    // **Never logged, never scheduled.** The document must not carry a mutation the
    // engine would skip, or a replay of this log would diverge from this session.
    assert_eq!(
        e.log.len(),
        2,
        "the two mounts and nothing else: {:?}",
        e.log.events()
    );
    assert!(
        !e.log
            .events()
            .iter()
            .any(|ev| matches!(ev, Event::Patch { .. })),
        "no patch event was written"
    );
    assert!(
        !e.is_degraded(),
        "a refusal at call time is an Err, not a session fault"
    );

    // The same shape after a re-mount, which is where the hazard hides: the name
    // goes back and comes back at the *end*, so the source is behind the sink again.
    e.unmount("tone")
        .expect("the tone is scheduled, so it unmounts");
    let _ = e.render(1); // the unmount applies, and the name is free
    e.mount("tone", &[("gain", 0.25)])
        .expect("a re-mount is a first instance again");
    let err = e
        .patch(("tone", "audio"), ("mixer", "ch0"))
        .expect_err("a re-mounted source is still behind the sink");
    assert!(err.contains("must go forward"), "got: {err}");

    // Forward order is untouched: the rule is the graph's, not a blanket refusal of
    // mixer patches. (Queued or applied, the source's node precedes the sink's.)
    let mut e = engine();
    e.mount("tone", &[]).unwrap();
    e.mount("mixer", &[]).unwrap();
    e.patch(("tone", "audio"), ("mixer", "ch0"))
        .expect("a source mounted before its sink is forward");
}

/// A **recorded** log carrying a backward cord is refused at load, in the same words
/// the live path speaks. The engine cannot read back a document whose cord the
/// graph will refuse, and a load that dropped the cord quietly would render a
/// session the file does not describe.
#[test]
fn a_log_carrying_a_backward_cord_is_refused_at_load() {
    let mut log = SessionLog::new();
    for (plugin, params) in [("mixer", vec![]), ("tone", vec![("gain", 0.25)])] {
        log.push(Event::Mount {
            plugin,
            params,
            at_frame: 0,
        });
    }
    log.push(Event::Patch {
        from_plugin: "tone",
        from_port: "audio",
        to_plugin: "mixer",
        to_port: "ch0",
        at_frame: 0,
    });

    let mut e = engine();
    let err = e
        .replay_from(&log)
        .expect_err("a log whose cord the graph refuses is not a log to read");
    assert!(err.contains("must go forward"), "got: {err}");
    assert!(
        err.contains("mount 'mixer' first"),
        "and the refusal names the fix: {err}"
    );
}

/// Patching is logged; replay reproduces the identical signal (determinism
/// holds under the port model).
#[test]
fn patching_is_logged_and_replayable() {
    let mut e1 = engine();
    mount_chain(&mut e1);
    e1.schedule_unmount("euclidean", 1_200_000)
        .expect("scheduled unmount");
    e1.set_tempo(96.0, 4).unwrap();
    let a = e1.render(2 * 48_000);
    let log = e1.log.clone();
    assert!(
        log.events()
            .iter()
            .any(|ev| matches!(ev, Event::Patch { .. }))
    );

    let mut e2 = engine();
    e2.replay_from(&log).expect("replay");
    let b = e2.render(2 * 48_000);

    assert_eq!(a, b, "same log must render byte-identical audio");
}

/// Replay reproduces mid-session tempo changes under the patched chain.
#[test]
fn replay_is_exact_for_mid_session_tempo_change() {
    let mut e1 = engine();
    mount_chain(&mut e1);
    let first = l(&e1.render(2 * 48_000));
    e1.set_tempo(240.0, 4).unwrap();
    e1.schedule_unmount("euclidean", 3 * 48_000)
        .expect("scheduled unmount");
    let second = l(&e1.render(2 * 48_000));
    let log = e1.log.clone();

    let mut e2 = engine();
    e2.replay_from(&log).unwrap();
    let b = l(&e2.render(4 * 48_000));

    assert_eq!(
        &b[..2 * 48_000],
        &first[..],
        "pre-change segment must match"
    );
    assert_eq!(
        &b[2 * 48_000..],
        &second[..],
        "post-change segment must match"
    );
    assert!(
        b[3 * 48_000..].iter().all(|s| *s == 0.0),
        "silence after unmount"
    );
}

/// Replay reproduces mid-session mounts under the patched chain (silence
/// before the mount frame). The chain (including the stereo mixer, which must
/// be mounted last for the graph's forward-order rule) mounts at frame 48000,
/// so the master's channel count changes there. A fixed-size render buffer can
/// hold only one width per call, so the replay is rendered in two calls at the
/// mount frame — the channel change lands on a render-call boundary.
#[test]
fn replay_is_exact_for_mid_session_mount() {
    let mut e1 = engine();
    let _quiet = e1.render(48_000);
    mount_chain(&mut e1);
    let after = l(&e1.render(48_000));
    let log = e1.log.clone();

    let mut e2 = engine();
    e2.replay_from(&log).unwrap();
    let mut b = e2.render(48_000); // 0..48000: mono silence (no master yet)
    b.extend(l(&e2.render(48_000))); // 48000..96000: chain (stereo mixer) mounted

    assert!(
        b[..48_000].iter().all(|s| *s == 0.0),
        "silence before the mount frame"
    );
    assert_eq!(
        &b[48_000..],
        &after[..],
        "audio after the mount frame must match"
    );
}

/// Unmounting a provider is sample-accurate: a long-decay blip is cut exactly
/// at the scheduled frame.
#[test]
fn unmount_is_sample_accurate() {
    let mut e = engine();
    e.mount(
        "euclidean",
        &[
            ("steps", 8.0),
            ("pulses", 3.0),
            ("rotation", 0.0),
            ("pulses_per_beat", 4.0),
        ],
    )
    .unwrap();
    e.mount("scale", &[("root", 0.0), ("note_len", 400.0)])
        .unwrap();
    e.mount("tone", &[("gain", 0.2), ("blip_len", 20_000.0)])
        .unwrap();
    e.patch(("euclidean", "triggers"), ("scale", "trigger"))
        .unwrap();
    e.patch(("scale", "note"), ("tone", "note")).unwrap();
    e.mount("mixer", &[]).unwrap();
    e.patch(("tone", "audio"), ("mixer", "ch0")).unwrap();

    let at = 96_000 + 100;
    // Unmount the whole chain: stopping the *generator* would leave the tone's
    // running blip tail to ring out (the audio teardown protocol — ramp /
    // flush — is deferred; Spike B). Removing the chain is exact.
    e.schedule_unmount("euclidean", at)
        .expect("scheduled unmount");
    e.schedule_unmount("scale", at).expect("scheduled unmount");
    e.schedule_unmount("tone", at).expect("scheduled unmount");
    let out = l(&e.render((at + 64) as usize));

    assert!(
        out[..at as usize].iter().any(|s| *s != 0.0),
        "sound before the unmount"
    );
    assert!(
        out[(at - 1) as usize] != 0.0,
        "a blip is still decaying at the boundary"
    );
    assert!(
        out[at as usize..].iter().all(|s| *s == 0.0),
        "exact silence from frame {at}"
    );
}

/// Unmounting the whole chain leaves no residual sound.
#[test]
fn unmount_removes_contribution() {
    let mut e = engine();
    mount_chain(&mut e);
    let bar = 2 * 48_000;
    e.schedule_unmount("euclidean", bar)
        .expect("scheduled unmount");
    e.schedule_unmount("scale", bar).expect("scheduled unmount");
    e.schedule_unmount("tone", bar).expect("scheduled unmount");
    let out = l(&e.render(bar as usize + 4096));
    assert!(out[..bar as usize].iter().any(|s| *s != 0.0));
    assert!(out[bar as usize..].iter().all(|s| *s == 0.0));
}

/// Full unmount → re-mount of the chain reproduces the identical signal
/// (reversibility is exact, not approximate — including plugin state like the
/// scale counter).
#[test]
fn remount_reproduces_identical_signal() {
    let mut e1 = engine();
    mount_chain(&mut e1);
    let first = e1.render(2 * 48_000);

    let mut e2 = engine();
    mount_chain(&mut e2);
    e2.unmount("euclidean").unwrap();
    e2.unmount("scale").unwrap();
    e2.unmount("tone").unwrap();
    e2.unmount("mixer").unwrap();
    let _gone = e2.render(2 * 48_000);
    mount_chain(&mut e2);
    let again = e2.render(2 * 48_000);

    assert_eq!(first, again, "re-mount must reproduce the exact signal");
}

/// A log the engine itself writes replays. The `remount_reproduces_identical_signal`
/// shape — `Mount p … ScheduleUnmount p … Mount p` — is what the live engine produces
/// (the name is released when the unmount *applies*), so refusing it would mean the
/// engine writes logs it cannot read back: the session fails to load at all, before
/// any audio is rendered.
///
/// Regression: `replay_from` validated each `Mount` against the *engine's* scheduling
/// state, which a replay never drains, so the second mount of a name was refused as a
/// second instance. Rendered here in the same call boundaries as the live run, because
/// the mixer unmount/remount changes the master width and such an event parks to a
/// render-call boundary.
#[test]
fn replay_accepts_a_log_that_re_mounts_a_plugin() {
    let mut e = engine();
    mount_chain(&mut e);
    let mut live = e.render(2 * 48_000);
    e.unmount("euclidean").unwrap();
    e.unmount("scale").unwrap();
    e.unmount("tone").unwrap();
    e.unmount("mixer").unwrap();
    live.extend(e.render(2 * 48_000)); // the unmounted window: no bus owner, so mono
    mount_chain(&mut e);
    live.extend(e.render(2 * 48_000));
    let log = e.log.clone();

    let mut replayed = engine();
    replayed
        .replay_from(&log)
        .expect("a log that re-mounts a plugin must replay");
    let mut out = replayed.render(2 * 48_000);
    out.extend(replayed.render(2 * 48_000));
    out.extend(replayed.render(2 * 48_000));

    assert_eq!(
        out.len(),
        live.len(),
        "the replay must land the same widths"
    );
    assert_eq!(
        out, live,
        "the replayed remount must render byte-identically"
    );
}

/// **`replay_from` refuses a target that already has plugins.** The doc's
/// precondition ("must be a *fresh* engine") was carried by the one-instance guard
/// alone, and the guard's lifecycle is the log's now — so the `disposers` half of
/// that check fell away with the rest of the engine-state check. A replay onto a
/// dirty engine is not refused: the log's `Mount p` would apply a *second* instance
/// over the first, and `apply_mount`'s `node_of`/`disposers` insert would overwrite
/// it — the first instance's node stays in the graph and its disposer is dropped with
/// its entry. A silent leak, so the replay says so instead.
///
/// The queued half is refused too: nothing has rendered, so a mount that is still
/// waiting to apply is pre-existing state just as much as an applied one.
#[test]
fn replay_onto_an_engine_that_already_has_plugins_is_refused() {
    // A log that mounts the whole chain — what a fresh replay is given.
    let mut e = engine();
    mount_chain(&mut e);
    let _ = e.render(48_000);
    let log = e.log.clone();

    // The target has already **applied** `tone`.
    let mut dirty = engine();
    mount_chain(&mut dirty);
    let _ = dirty.render(48_000);
    let err = dirty
        .replay_from(&log)
        .expect_err("a replay onto a mounted engine must be refused");
    assert!(
        err.contains("fresh engine")
            && ["euclidean", "scale", "tone", "mixer"]
                .iter()
                .any(|p| err.contains(&format!("'{p}'"))),
        "the refusal names the precondition and a plugin already on the engine: {err}"
    );

    // …and the target has merely **queued** it (nothing rendered, so the mount has
    // not applied): the same refusal, because the same second instance would be
    // applied over the first.
    let mut queued = engine();
    mount_chain(&mut queued);
    let err = queued
        .replay_from(&log)
        .expect_err("a replay onto an engine with a queued mount must be refused");
    assert!(err.contains("fresh engine"), "{err}");

    // The refused replay left nothing behind: no event was scheduled or logged, and
    // the engine still renders its own (unreplayed) session.
    assert_eq!(
        queued.render(4_096).len(),
        4_096 * 2,
        "the refused replay left the engine as it was"
    );

    // And the fresh case still replays — the precondition is the only new refusal.
    let mut fresh = engine();
    fresh.replay_from(&log).expect("a fresh engine replays");
}

/// **A frame-inverted document is refused, not applied.** The walk tracked the
/// lifecycle in *log* order while the apply queue drains in *frame* order, so a log
/// reading `Mount p@0, Unmount p@96_000, Mount p@0` was admitted: at apply both mounts
/// pop before the unmount, `apply_mount` runs twice for one name, and the first
/// instance's node stays in the graph while its disposer is dropped unrun.
///
/// The engine itself writes that log whenever a caller places the clock back before
/// a scheduled lifecycle frame (`Engine::seek`), so this is the real shape, not a
/// hand-built one. The rule is refusal — a loud `Err` naming the plugin and the two
/// frames — because the alternative (a self-healing apply that disposes the instance
/// it finds) would render something the log does not say and hide the log-order
/// error. No panic either way: a refusal is a `Result`, on every build.
#[test]
fn replay_refuses_a_log_whose_mounts_are_not_in_frame_order() {
    let mut e = engine();
    e.mount("tone", &[]).unwrap();
    let _ = e.render(1); // the first instance applies
    e.schedule_unmount("tone", 96_000)
        .expect("scheduled unmount");
    let _ = e.render(96_000); // …past 96_000, so the unmount is due
    let _ = e.render(1); // an event on a block's end frame applies in the next call
    e.seek(0); // …and the clock goes back *before* it
    e.mount("tone", &[])
        .expect("a re-mount, stamped at frame 0");
    let log = e.log.clone();

    // The log really is the inverted triple, so the refusal below is about the
    // engine's rule and not about an accident in the fixture.
    let frames: Vec<u64> = log
        .events()
        .iter()
        .filter_map(|ev| match ev {
            Event::Mount {
                plugin: "tone",
                at_frame,
                ..
            } => Some(*at_frame),
            Event::ScheduleUnmount {
                plugin: "tone",
                at_frame,
            } => Some(*at_frame),
            _ => None,
        })
        .collect();
    assert_eq!(
        frames,
        vec![0, 96_000, 0],
        "the live path wrote Mount@0, Unmount@96_000, Mount@0"
    );

    let mut fresh = engine();
    let err = fresh
        .replay_from(&log)
        .expect_err("a frame-inverted document must be refused");
    assert!(
        err.contains("'tone'") && err.contains("frame order"),
        "the refusal names the plugin and the rule: {err}"
    );
    // The refused event left no trace: the two events the replay had already
    // accepted are all that is scheduled, nothing was applied, and the engine
    // renders its own silence rather than an orphaned instance.
    assert_eq!(
        fresh.log.events().len(),
        2,
        "a refused event is never logged: {:?}",
        fresh.log.events()
    );
    assert!(
        fresh.graph.nodes().is_empty(),
        "nothing was applied: {} node(s)",
        fresh.graph.nodes().len()
    );
    assert!(
        fresh.render(4_096).iter().all(|s| *s == 0.0),
        "the refused replay left the engine rendering its own (empty) session"
    );

    // The **frame-ordered** triple is still accepted — the rule is order, not a ban
    // on re-mounts. `replay_accepts_a_log_that_re_mounts_a_plugin` is the audio
    // proof; this is the boundary.
    let mut ordered = engine();
    ordered.mount("tone", &[]).unwrap();
    ordered
        .schedule_unmount("tone", 48_000)
        .expect("scheduled unmount");
    let _ = ordered.render(48_001); // the unmount is due…
    // …and with no mixer mounted the tone owns the bus, so its unmount is a width
    // change and parks to the next control-side flush. The name is released when
    // the unmount *applies*, so the flush is what frees it.
    ordered.flush_scheduled();
    ordered
        .mount("tone", &[])
        .expect("a re-mount after the unmount's frame");
    let mut target = engine();
    target
        .replay_from(&ordered.log.clone())
        .expect("a frame-ordered re-mount replays");
    let _ = target.render(4_096);
    assert_eq!(
        target.graph.nodes().len(),
        1,
        "one instance, one node — the ordered triple is a re-mount, not a second mount"
    );
}

/// **A walk speaks only for the names the document spoke for.** The doc claimed a
/// walk "never loses the target's own state" while seeding itself from a snapshot
/// taken at entry — a snapshot that goes stale the moment the target's own state
/// moves, so the property was neither true nor checkable. The walk is now a *view of
/// the document*: for a name it has not mentioned, the target's own state answers.
///
/// That half is what keeps a walk from overwriting a live instance, and it is pinned
/// here so the doc cannot rot. The other half — a walk may not mount onto a target
/// from outside the document — is a caller obligation (the engine has no way to see
/// it), and `Engine::seek`'s doc says so.
#[test]
fn a_walk_answers_for_the_documents_names_only() {
    let mut target = engine();
    mount_chain(&mut target);
    let _ = target.render(48_000); // the chain is applied

    target.enter_walk();
    // A document that mounts a name the target holds is refused: the walk does not
    // speak for `tone`, so the target's applied instance does.
    let err = target
        .mount("tone", &[])
        .expect_err("a walk must not mount over an instance the target holds");
    assert!(err.contains("already mounted"), "{err}");

    // A document that unmounts it *first* is a replacement, not a second instance —
    // the frame rule puts the teardown before the re-mount (both at the current
    // frame here, and equal frames apply in the order they were scheduled).
    target.unmount("tone").expect("the document unmounts it");
    target
        .mount("tone", &[])
        .expect("a replacement is a re-mount, not a second instance");
    target.leave_walk();

    // Leaving the walk hands the question back to the engine: the name the document
    // left mounted is now the engine's own, so a live mount is refused again.
    let err = target
        .mount("tone", &[])
        .expect_err("off the walk the engine answers for itself");
    assert!(err.contains("already mounted"), "{err}");
}

/// Spatial composability, static half: a consumer plugin declares `rhythm`,
/// resolves it at mount (fail-loud when the provider is absent).
#[test]
fn plugins_cooperate_via_service_key() {
    struct BassFollower;
    impl Plugin for BassFollower {
        fn id(&self) -> &'static str {
            "bass"
        }
        fn inject(&self) -> &'static [&'static str] {
            &["rhythm"]
        }
        fn ports(&self) -> &'static [Port] {
            &[]
        }
        fn apply(&mut self, api: &mut PluginApi) -> Result<(NodeId, Disposer), String> {
            let rhythm = api
                .ctx
                .get::<Rhythm>("rhythm")
                .ok_or("rhythm service missing at apply")?;
            assert_eq!(rhythm.pattern.len(), rhythm.steps as usize);
            Ok((NodeId(0), Box::new(|_| {})))
        }
    }
    let bass_factory: PluginFactory = |_| Ok(Box::new(BassFollower));

    let mut e = engine();
    e.register_factory("bass", bass_factory, &[], &[]);
    let err = e.mount("bass", &[]).unwrap_err();
    assert!(err.contains("rhythm"), "got: {err}");
    assert!(e.log.is_empty(), "a refused mount must not be logged");
    e.mount("euclidean", &[]).unwrap();
    e.render(1);
    e.mount("bass", &[])
        .expect("bass mounts once euclidean provides rhythm");
}

/// Fail-loud inject: a plugin requiring a missing service is refused at mount.
#[test]
fn inject_fails_loud() {
    struct NeedsMissing;
    impl Plugin for NeedsMissing {
        fn id(&self) -> &'static str {
            "needs-missing"
        }
        fn inject(&self) -> &'static [&'static str] {
            &["definitely-not-provided"]
        }
        fn ports(&self) -> &'static [Port] {
            &[]
        }
        fn apply(&mut self, _api: &mut PluginApi) -> Result<(NodeId, Disposer), String> {
            Ok((NodeId(0), Box::new(|_| {})))
        }
    }
    let factory: PluginFactory = |_| Ok(Box::new(NeedsMissing));
    let mut e = engine();
    e.register_factory("needs-missing", factory, &[], &[]);
    let err = e.mount("needs-missing", &[]).unwrap_err();
    assert!(err.contains("definitely-not-provided"), "got: {err}");
    assert!(e.log.is_empty(), "a refused mount must not be logged");
}

/// The euclidean generator honours tempo changes under the patched chain.
#[test]
fn tempo_change_moves_triggers() {
    let mut e = engine();
    e.mount(
        "euclidean",
        &[
            ("steps", 8.0),
            ("pulses", 3.0),
            ("rotation", 1.0),
            ("pulses_per_beat", 4.0),
        ],
    )
    .unwrap();
    e.mount("scale", &[("root", 0.0)]).unwrap();
    e.mount("tone", &[("gain", 0.25), ("blip_len", 1200.0)])
        .unwrap();
    e.patch(("euclidean", "triggers"), ("scale", "trigger"))
        .unwrap();
    e.patch(("scale", "note"), ("tone", "note")).unwrap();
    e.mount("mixer", &[]).unwrap();
    e.patch(("tone", "audio"), ("mixer", "ch0")).unwrap();
    e.set_tempo(240.0, 4).unwrap();
    let out = l(&e.render(5000));
    assert_eq!(out[2999], 0.0);
    assert!(
        out[3001] > 0.0,
        "pulse must land at frame 3000 under 240bpm"
    );
}

/// The patch-bay registry: an external source (here a fake OSC-style plugin)
/// appears among the providers of the matching signal kind — the dropdown's
/// data source.
#[test]
fn providers_of_lists_external_sources() {
    struct FakeOscNode;
    impl AudioNode for FakeOscNode {
        fn latency(&self) -> u32 {
            0
        }
        fn render(
            &mut self,
            _io: &NodeIO,
            _audio: &mut [f32],
            _control: &mut f32,
            out_triggers: &mut EventBuf<Trigger, CAP_EVENTS>,
            _notes: &mut EventBuf<NoteEvent, CAP_EVENTS>,
            _block: RenderBlock,
        ) {
            out_triggers.push(100);
        }
    }
    struct FakeOsc;
    impl Plugin for FakeOsc {
        fn id(&self) -> &'static str {
            "fakeosc"
        }
        fn inject(&self) -> &'static [&'static str] {
            &[]
        }
        fn ports(&self) -> &'static [Port] {
            &[Port {
                name: "triggers",
                direction: Direction::Out,
                kind: SignalKind::Trigger,
                channels: 1,
            }]
        }
        fn apply(&mut self, api: &mut PluginApi) -> Result<(NodeId, Disposer), String> {
            let node = api.graph.add_node(
                NodeKind::Opaque(Box::new(FakeOscNode)),
                self.ports().to_vec(),
            );
            Ok((
                node,
                Box::new(move |dis| {
                    dis.graph.remove_node(node);
                }),
            ))
        }
    }
    let osc_factory: PluginFactory = |_| Ok(Box::new(FakeOsc));
    let osc_ports: &'static [Port] = &[Port {
        name: "triggers",
        direction: Direction::Out,
        kind: SignalKind::Trigger,
        channels: 1,
    }];

    let mut e = engine();
    e.register_factory("fakeosc", osc_factory, osc_ports, &[]);
    e.mount("euclidean", &[]).unwrap();
    e.mount("fakeosc", &[]).unwrap();
    e.render(1); // apply

    let providers = e.providers_of(SignalKind::Trigger);
    assert_eq!(
        providers.len(),
        2,
        "euclidean + fakeosc both provide triggers"
    );
    assert!(providers.iter().all(|(_, name)| *name == "triggers"));
    let notes = e.providers_of(SignalKind::Note);
    assert!(
        notes.is_empty(),
        "no note providers yet — scale not mounted"
    );
    e.mount("scale", &[]).unwrap();
    e.render(1);
    assert_eq!(e.providers_of(SignalKind::Note).len(), 1);
}

/// Fan-in: two trigger producers patch into one input; the merged stream must
/// stay sorted by offset or the tone's pending queue would drop/stall notes
/// (kimi A.5 review, bug 2).
#[test]
fn fan_in_merges_events_sorted() {
    struct FakeOscNode;
    impl AudioNode for FakeOscNode {
        fn latency(&self) -> u32 {
            0
        }
        fn render(
            &mut self,
            _io: &NodeIO,
            _audio: &mut [f32],
            _control: &mut f32,
            out_triggers: &mut EventBuf<Trigger, CAP_EVENTS>,
            _notes: &mut EventBuf<NoteEvent, CAP_EVENTS>,
            block: RenderBlock,
        ) {
            if block.frame == 0 {
                out_triggers.push(100); // fires once, early in block 0
            }
        }
    }
    struct FakeOsc;
    impl Plugin for FakeOsc {
        fn id(&self) -> &'static str {
            "fakeosc"
        }
        fn inject(&self) -> &'static [&'static str] {
            &[]
        }
        fn ports(&self) -> &'static [Port] {
            &[Port {
                name: "triggers",
                direction: Direction::Out,
                kind: SignalKind::Trigger,
                channels: 1,
            }]
        }
        fn apply(&mut self, api: &mut PluginApi) -> Result<(NodeId, Disposer), String> {
            let node = api.graph.add_node(
                NodeKind::Opaque(Box::new(FakeOscNode)),
                self.ports().to_vec(),
            );
            Ok((
                node,
                Box::new(move |dis| {
                    dis.graph.remove_node(node);
                }),
            ))
        }
    }
    let osc_factory: PluginFactory = |_| Ok(Box::new(FakeOsc));
    let osc_ports: &'static [Port] = &[Port {
        name: "triggers",
        direction: Direction::Out,
        kind: SignalKind::Trigger,
        channels: 1,
    }];

    let mut e = engine();
    e.register_factory("fakeosc", osc_factory, osc_ports, &[]);
    // euclidean triggers at 6000; fakeosc at 100. Patch order puts the *later*
    // offset first into the merge, exercising the sorted insert.
    e.mount(
        "euclidean",
        &[
            ("steps", 8.0),
            ("pulses", 1.0),
            ("rotation", 1.0),
            ("pulses_per_beat", 4.0),
        ],
    )
    .unwrap();
    e.mount("fakeosc", &[]).unwrap();
    e.mount("scale", &[("root", 0.0)]).unwrap();
    e.mount("tone", &[("gain", 0.25), ("blip_len", 1200.0)])
        .unwrap();
    e.patch(("euclidean", "triggers"), ("scale", "trigger"))
        .unwrap();
    e.patch(("fakeosc", "triggers"), ("scale", "trigger"))
        .unwrap();
    e.patch(("scale", "note"), ("tone", "note")).unwrap();
    e.mount("mixer", &[]).unwrap();
    e.patch(("tone", "audio"), ("mixer", "ch0")).unwrap();

    let out = l(&e.render(7000));
    assert!(
        out[101] > 0.0,
        "fakeosc blip must fire at frame 101 (offset 100)"
    );
    assert_eq!(out[5999], 0.0);
    assert!(
        out[6001] > 0.0,
        "euclidean blip must still fire at 6001 — sorted merge"
    );
}

/// A patch for a plugin that is neither scheduled nor mounted is refused at
/// patch time (fail-loud) and never logged.
#[test]
fn patch_before_mount_refused() {
    let mut e = engine();
    let err = e
        .patch(("euclidean", "triggers"), ("scale", "trigger"))
        .unwrap_err();
    assert!(err.contains("neither scheduled nor mounted"), "got: {err}");
    assert!(e.log.is_empty(), "a refused patch must not be logged");
}

/// PDC delay lines are preallocated: rendering a latency-bearing graph through
/// the counting allocator must still allocate nothing.
#[test]
fn render_path_does_not_allocate_with_latency() {
    struct TestDelay;
    impl AudioNode for TestDelay {
        fn latency(&self) -> u32 {
            3
        }
        fn render(
            &mut self,
            io: &NodeIO,
            out: &mut [f32],
            _control: &mut f32,
            _triggers: &mut EventBuf<Trigger, CAP_EVENTS>,
            _notes: &mut EventBuf<NoteEvent, CAP_EVENTS>,
            _block: RenderBlock,
        ) {
            let n = out.len().min(io.audio_in.len());
            out[..n].copy_from_slice(&io.audio_in[..n]);
        }
    }
    let mut e = Engine::new(SR, 120.0, 4);
    let sine = e.graph.add_node(
        NodeKind::Sine(Sine::new(440.0)),
        vec![Port {
            name: "audio",
            direction: Direction::Out,
            kind: SignalKind::Audio,
            channels: 1,
        }],
    );
    let delay = e.graph.add_node(
        NodeKind::Opaque(Box::new(TestDelay)),
        vec![
            Port {
                name: "audio",
                direction: Direction::In,
                kind: SignalKind::Audio,
                channels: 1,
            },
            Port {
                name: "audio",
                direction: Direction::Out,
                kind: SignalKind::Audio,
                channels: 1,
            },
        ],
    );
    e.graph.connect(sine, "audio", delay, "audio").unwrap();
    e.graph.set_out(delay);
    let mut out = vec![0.0f32; 4096];

    ALLOCS.store(0, Ordering::Relaxed);
    MEASURING.with(|m| m.set(true));
    e.render_into(&mut out);
    MEASURING.with(|m| m.set(false));

    assert_eq!(
        ALLOCS.load(Ordering::Relaxed),
        0,
        "PDC must not allocate, even with a latency-bearing node"
    );
}

/// The patch-bay registry names providers (the dropdown's data), and the OSC
/// seam traits are exercised, not just declared.
#[test]
fn provider_names_and_osc_seam() {
    let mut e = engine();
    e.register_factory(
        "fakeosc",
        |_| Ok(Box::new(ProviderNamesFakeOsc)),
        &[Port {
            name: "triggers",
            direction: Direction::Out,
            kind: SignalKind::Trigger,
            channels: 1,
        }],
        &[],
    );
    e.mount("euclidean", &[]).unwrap();
    e.mount("fakeosc", &[]).unwrap();
    e.mount("scale", &[]).unwrap();
    e.render(1);

    let names = e.provider_names_of(SignalKind::Trigger);
    assert!(
        names.contains(&"euclidean") && names.contains(&"fakeosc"),
        "got {names:?}"
    );
    assert_eq!(e.provider_names_of(SignalKind::Note), vec!["scale"]);
}

struct ProviderNamesFakeOsc;
impl Plugin for ProviderNamesFakeOsc {
    fn id(&self) -> &'static str {
        "fakeosc"
    }
    fn inject(&self) -> &'static [&'static str] {
        &[]
    }
    fn ports(&self) -> &'static [Port] {
        &[Port {
            name: "triggers",
            direction: Direction::Out,
            kind: SignalKind::Trigger,
            channels: 1,
        }]
    }
    fn apply(&mut self, api: &mut PluginApi) -> Result<(NodeId, Disposer), String> {
        let node = api.graph.add_node(
            NodeKind::Opaque(Box::new(SeamOscNode)),
            self.ports().to_vec(),
        );
        Ok((
            node,
            Box::new(move |dis| {
                dis.graph.remove_node(node);
            }),
        ))
    }
}

struct SeamOscNode;
impl AudioNode for SeamOscNode {
    fn latency(&self) -> u32 {
        0
    }
    fn render(
        &mut self,
        _io: &NodeIO,
        _audio: &mut [f32],
        _control: &mut f32,
        out_triggers: &mut EventBuf<Trigger, CAP_EVENTS>,
        _notes: &mut EventBuf<NoteEvent, CAP_EVENTS>,
        _block: RenderBlock,
    ) {
        out_triggers.push(0);
    }
}

/// The OSC seam traits are exercised (not just declared): a source object can
/// emit typed external events.
impl EventSource for SeamOscNode {
    fn id(&self) -> &'static str {
        "fakeosc"
    }
    fn for_each_event(&self, _block: std::ops::Range<u64>, emit: &mut dyn FnMut(ExternalEvent)) {
        emit(ExternalEvent::Trigger { offset: 0 });
    }
}
impl OscSource for SeamOscNode {}

#[test]
fn osc_seam_is_usable() {
    let src = SeamOscNode;
    let mut got = Vec::new();
    src.for_each_event(0..512, &mut |ev| got.push(ev));
    assert_eq!(got, vec![ExternalEvent::Trigger { offset: 0 }]);
    // and it can be held as a trait object (the seam's contract)
    let _: &dyn OscSource = &src;
}

// The render path must not allocate (engine invariant, now under the patch
// bay): a counting allocator asserts zero allocations inside `render_into`.
thread_local! {
    static MEASURING: Cell<bool> = const { Cell::new(false) };
}
static ALLOCS: AtomicUsize = AtomicUsize::new(0);

struct CountingAllocator;
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if MEASURING.with(|m| m.get()) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if MEASURING.with(|m| m.get()) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}
#[global_allocator]
static GLOBAL_ALLOC: CountingAllocator = CountingAllocator;

#[test]
fn render_path_does_not_allocate() {
    let mut e = engine();
    mount_chain(&mut e);
    // The clock generator is portless and sinkless here — its per-block
    // scratch is preallocated, so steady-state clocking must also allocate
    // nothing (midi-clock-out note, acceptance 5).
    e.register_factory(
        "clock_out",
        plugins::clock_out_factory,
        plugins::clock_out::CLOCK_OUT_PORTS,
        &[],
    );
    e.mount("clock_out", &[]).unwrap();
    e.schedule_unmount("euclidean", 100_000)
        .expect("scheduled unmount");
    // Prime: applying the mounts/patches allocates on the control side
    // (factories, boxes, service table). The measured region must be free.
    let _prime = e.render(512);
    let mut out = vec![0.0f32; 8192]; // allocated before measuring

    ALLOCS.store(0, Ordering::Relaxed);
    MEASURING.with(|m| m.set(true));
    e.render_into(&mut out);
    MEASURING.with(|m| m.set(false));

    assert_eq!(
        ALLOCS.load(Ordering::Relaxed),
        0,
        "the render path must not allocate (engine invariant)"
    );
}
