//! The engine's apply path: a mutation the log validated and the plugin or the graph
//! then refused — a mount whose `apply` returns `Err`, a cord whose endpoint is gone.
//!
//! `Euclidean::apply` is the first `apply` in the tree that can return `Err` — it
//! re-checks the `steps` bound because the struct's fields are public, so an
//! instance can be hand-assembled without the factory's door. Two things had to be
//! true of the engine when it did, and neither was:
//!
//! - the refusal **wedged the name**. `apply_mount` returned on the `?` from
//!   `plugin.apply` *before* `scheduled.remove(name)`, so the name stayed reserved
//!   for the rest of the session: every later mount of it was refused as a second
//!   instance, and a log the engine itself had written could not be replayed onto
//!   it.
//! - the only report was a `debug_assert!`, which release compiles away — so in
//!   the build a user runs, a plugin that silently never mounted was not reported
//!   at all.
//!
//! So `apply_mount` is a transaction (nothing is written until the apply has
//! succeeded, the graph is put back on a refusal) and the refusal is *recorded* in
//! [`ApplyFault`] rather than asserted — the same answer the cord path gets, where
//! `apply_patch` used to `debug_assert!` and release said nothing at all.

use std::panic::{AssertUnwindSafe, catch_unwind};

use engine::*;

/// The euclidean instance the **factory** would refuse, assembled straight from the
/// plugin's public fields. `Euclidean::apply` exists to refuse exactly this, so
/// the factory here is the euclidean factory with its door removed — which is the
/// only way a scheduled mount reaches an `apply` that refuses.
///
/// It reads the params exactly as `euclidean_factory` does, so a second mount with
/// `steps=8` below is an ordinary euclidean and the retry is not a fiction.
fn euclidean_without_a_door(params: &[(&'static str, f32)]) -> Result<Box<dyn Plugin>, String> {
    let get = |key: &str, default: f32| {
        params
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| *value)
            .unwrap_or(default)
    };
    Ok(Box::new(Euclidean {
        steps: get("steps", 8.0) as u32,
        pulses: get("pulses", 3.0) as u32,
        rotation: get("rotation", 0.0) as u32,
        pulses_per_beat: get("pulses_per_beat", 4.0) as u32,
    }))
}

const SR: u32 = 48_000;

/// One oversize instance: a `steps` the plugin's own factory refuses and its
/// `apply` refuses again, which is the disagreement the engine has to survive.
fn oversize() -> [(&'static str, f32); 1] {
    [("steps", (EUCLIDEAN_MAX_STEPS + 1) as f32)]
}

fn engine_with_a_doored_euclidean() -> Engine {
    let mut e = Engine::new(SR, 120.0, 4);
    e.register_factory(
        "euclidean",
        euclidean_without_a_door,
        plugins::euclidean::EUCLIDEAN_PORTS,
        &[],
    );
    e
}

/// A tone and a mixer — the two the cord tests wire (`tone.audio` → `mixer.ch0`).
fn engine_with_a_tone_and_a_mixer() -> Engine {
    let mut e = Engine::new(SR, 120.0, 4);
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

/// Render through the scheduled apply, tolerating the **unfixed** engine's tripwire
/// (the `parked_arrangements.rs` shape, for the same reason): there, a refused
/// mount tripped `debug_assert!(false, …)` inside `render_block`, so a debug
/// build unwound out of the render and the test would abort on a panic instead of
/// failing on the assertion that names the wedge. With the fix there is nothing to
/// tolerate — no build panics on a refusal — so the helper exists to keep one test
/// honest in both profiles.
fn render_tolerating_an_unfixed_engine(e: &mut Engine, frames: usize) -> Vec<f32> {
    match catch_unwind(AssertUnwindSafe(|| e.render(frames))) {
        Ok(out) => out,
        #[cfg(debug_assertions)]
        Err(payload) => {
            drop(payload); // the unfixed engine's tripwire — tolerate, assert below
            Vec::new()
        }
        #[cfg(not(debug_assertions))]
        Err(_) => panic!("a refused mount must never panic, in any build"),
    }
}

/// **The defect, by name, through the scheduled path.** The mount is *accepted*
/// (the factory has no door), logged, scheduled, and refused when the render loop
/// applies it. The refusal must be visible, must leave the engine describing only
/// what is mounted, and must not wedge the name: a later mount of the same name
/// has to succeed and apply.
#[test]
fn a_scheduled_mount_the_plugin_refuses_is_reported_and_frees_the_name() {
    let mut e = engine_with_a_doored_euclidean();
    e.mount("euclidean", &oversize())
        .expect("a factory without a door lets it through");
    assert_eq!(
        e.log.len(),
        1,
        "the mount is in the document before it applies"
    );

    // The refusal happens where it must: on the apply path.
    let rendered = render_tolerating_an_unfixed_engine(&mut e, BLOCK);
    assert_eq!(
        rendered.len(),
        BLOCK,
        "a refused apply does not wedge the loop"
    );

    // Nothing is mounted, so every map describes nothing: no instance, no node on
    // the bay, no published service, no provider offered to a patch.
    assert_eq!(e.node_of("euclidean"), None, "no instance was created");
    assert!(
        e.providers_of(SignalKind::Trigger).is_empty(),
        "and no node was left in the graph"
    );
    assert!(
        !e.ctx.has("rhythm"),
        "and the instance's service was never published"
    );
    assert!(
        !e.provider_names_of(SignalKind::Trigger)
            .contains(&"euclidean"),
        "nor is it offered as a trigger provider"
    );

    // **The report.** Loud in every build, naming the plugin, the frame the *log*
    // stamped (not the frame the refusal was noticed at), and the plugin's own
    // reason. The session is degraded, so a host can say the audio is not what the
    // log says.
    let faults = e.apply_faults();
    assert_eq!(faults.len(), 1, "exactly the refused mount: {faults:?}");
    assert_eq!(faults[0].plugin, "euclidean");
    assert_eq!(faults[0].at_frame, 0, "the frame the log stamped");
    assert!(
        faults[0].reason.contains("steps"),
        "and the plugin's own refusal, verbatim: {}",
        faults[0].reason
    );
    assert_eq!(e.apply_faults_dropped(), 0, "one fault, none dropped");
    assert!(e.is_degraded(), "the session is not what its log says");

    // **The wedge.** The name is free: the next mount of it is a first instance,
    // not a second one, and it applies.
    e.mount("euclidean", &[("steps", 8.0)])
        .expect("a refused mount must not keep the name reserved");
    assert_eq!(e.render(BLOCK).len(), BLOCK);
    assert!(
        e.node_of("euclidean").is_some(),
        "and the re-mount really applies"
    );
    assert!(
        e.ctx.has("rhythm"),
        "the instance provides its service as usual"
    );
    assert_eq!(
        e.apply_faults().len(),
        1,
        "a mount that applied is not a fault"
    );
}

/// The same refusal from a plugin that gets *further* than the euclidean: it
/// registers its node — stealing the master bus on the way — and only then
/// refuses. A node nothing holds a disposer for is an orphan that renders (or not)
/// forever, so the rollback has to reach the graph and give the bus back, not just
/// leave the engine's own maps clean.
#[test]
fn a_refused_apply_leaves_no_node_and_no_bus_claim_behind() {
    /// A node with an audio Out, so `add_node` would claim the bus and the
    /// rollback has a claim to undo.
    const PORTS: &[Port] = &[Port {
        name: "audio",
        direction: Direction::Out,
        kind: SignalKind::Audio,
        channels: 1,
    }];
    struct AddsANodeThenRefuses;
    impl Plugin for AddsANodeThenRefuses {
        fn id(&self) -> &'static str {
            "half_applied"
        }
        fn inject(&self) -> &'static [&'static str] {
            &[]
        }
        fn ports(&self) -> &'static [Port] {
            PORTS
        }
        fn apply(&mut self, api: &mut PluginApi) -> Result<(NodeId, Disposer), String> {
            // The realistic shape of a fallible apply: register, then discover the
            // instance cannot be built and refuse.
            let node = api
                .graph
                .add_node(NodeKind::Sine(Sine::new(440.0)), PORTS.to_vec());
            api.graph.set_out(node);
            Err("this instance cannot be built after all".to_string())
        }
    }

    let mut e = engine_with_a_doored_euclidean();
    // A tone owns the bus first, so the failed mount's claim is a *steal* and the
    // rollback has something to give back.
    e.register_factory(
        "tone",
        plugins::tone_factory,
        plugins::tone::TONE_PORTS,
        plugins::tone::TONE_PARAMS,
    );
    e.mount("tone", &[]).expect("the tone mounts");
    render_tolerating_an_unfixed_engine(&mut e, BLOCK);
    let tone = e.node_of("tone").expect("the tone is mounted");
    assert_eq!(e.graph.out_node, Some(tone), "the tone owns the master bus");

    e.register_factory(
        "half_applied",
        |_| Ok(Box::new(AddsANodeThenRefuses)),
        PORTS,
        &[],
    );
    e.mount("half_applied", &[])
        .expect("the factory accepts; the refusal is at apply");
    render_tolerating_an_unfixed_engine(&mut e, BLOCK);

    assert_eq!(e.node_of("half_applied"), None, "no instance was recorded");
    assert_eq!(
        e.graph.nodes().len(),
        1,
        "and the node it added is gone: {:?}",
        e.providers_of(SignalKind::Audio)
    );
    assert_eq!(
        e.graph.out_node,
        Some(tone),
        "the bus went back to the tone that held it"
    );
    assert_eq!(e.apply_faults().len(), 1);
    assert_eq!(e.apply_faults()[0].plugin, "half_applied");
    e.mount("half_applied", &[])
        .expect("and the name is free again after a half-applied refusal");
}

/// The bound is **reported, not hidden** — the `DrainOutcome::capped` discipline.
/// A session whose every mount is refused would otherwise grow the engine's state
/// by a string per mount; past the bound the refusals are counted instead of held.
#[test]
fn the_fault_list_is_bounded_and_says_what_it_could_not_hold() {
    let mut e = engine_with_a_doored_euclidean();
    let over = MAX_APPLY_FAULTS + 3;
    for frame in 0..over {
        // Every refusal gives the name back, which is what lets the very same
        // name be mounted again — the wedge, over and over.
        e.mount("euclidean", &oversize())
            .unwrap_or_else(|err| panic!("refusal {frame} kept the name reserved: {err}"));
        let _ = e.render(1);
    }
    assert_eq!(
        e.apply_faults().len(),
        MAX_APPLY_FAULTS,
        "the list is bounded"
    );
    assert_eq!(
        e.apply_faults_dropped(),
        3,
        "and the refusals past the bound are counted, not dropped silently"
    );
    assert!(e.is_degraded());
}

/// A fault belongs to the audio path's determinism: the same log refuses at the
/// same frame with the same message, so a host shows the same text whether the
/// session was played or loaded — and the log stays readable, which is what the
/// wedge used to prevent.
#[test]
fn a_replayed_log_refuses_at_the_same_frame_with_the_same_fault() {
    let mut live = engine_with_a_doored_euclidean();
    live.mount("euclidean", &oversize()).unwrap();
    let audio = render_tolerating_an_unfixed_engine(&mut live, BLOCK);
    let log = live.log.clone();

    let mut replayed = engine_with_a_doored_euclidean();
    replayed
        .replay_from(&log)
        .expect("a log whose mount refuses is still a log the engine can read");
    let replayed_audio = render_tolerating_an_unfixed_engine(&mut replayed, BLOCK);

    assert_eq!(replayed_audio, audio, "the same log renders the same audio");
    assert_eq!(
        replayed.apply_faults(),
        live.apply_faults(),
        "and reports the same fault, at the same frame"
    );
    assert!(replayed.is_degraded(), "a loaded session is degraded too");
}

/// The same discipline on the **cord** path: a scheduled patch whose endpoint is gone
/// when the cord applies. It is reachable — the destination's unmount is scheduled at
/// the same frame the cord is, and the queue is FIFO within a frame, so the unmount
/// applies first — and it is *not* reachable by validation, which sees the mixer still
/// mounted and the cord forward. The only report used to be a `debug_assert!` inside
/// `render_block`: a panic on the audio thread in a debug build, and a destination
/// channel nothing feeds — with nothing said — in the build a user runs.
#[test]
fn a_patch_the_engine_cannot_make_at_apply_is_reported_and_does_not_panic() {
    let mut e = engine_with_a_tone_and_a_mixer();
    e.mount("tone", &[]).expect("the tone mounts");
    e.mount("mixer", &[]).expect("the mixer mounts");
    e.schedule_unmount("mixer", 0)
        .expect("the mixin's go-away is scheduled");
    e.patch(("tone", "audio"), ("mixer", "ch0"))
        .expect("at call time the mixer is mounted and the cord is forward");
    assert_eq!(
        e.log.len(),
        4,
        "so the cord is in the document: {:?}",
        e.log.events()
    );

    let rendered = render_tolerating_an_unfixed_engine(&mut e, BLOCK);
    assert_eq!(
        rendered.len(),
        BLOCK,
        "a refused cord does not wedge the loop"
    );

    // **The report.** A fault like any other: named after the cord's source plugin,
    // stamped with the frame the *log* carried, and carrying the cord as the log
    // spells it plus the reason. The session is degraded — the audio is not what the
    // document says.
    let faults = e.apply_faults();
    assert_eq!(faults.len(), 1, "exactly the cord: {faults:?}");
    assert_eq!(faults[0].plugin, "tone", "the cord's source plugin");
    assert_eq!(faults[0].at_frame, 0, "the frame the log stamped");
    assert!(
        faults[0].reason.contains("tone.audio → mixer.ch0"),
        "the cord, as the log spells it: {}",
        faults[0].reason
    );
    assert!(
        faults[0].reason.contains("destination is not mounted"),
        "and why: {}",
        faults[0].reason
    );
    assert_eq!(e.apply_faults_dropped(), 0, "one fault, none dropped");
    assert!(e.is_degraded());

    // And a **loaded** session says the same thing: the fault is a function of the
    // log, so a host shows one report whether the session was played or opened.
    let mut loaded = engine_with_a_tone_and_a_mixer();
    loaded
        .replay_from(&e.log)
        .expect("a log whose cord is refused is still a log the engine can read");
    let loaded_audio = render_tolerating_an_unfixed_engine(&mut loaded, BLOCK);
    assert_eq!(loaded_audio, rendered, "and the same audio");
    assert_eq!(
        loaded.apply_faults(),
        e.apply_faults(),
        "and the same fault, at the same frame"
    );
    assert!(loaded.is_degraded(), "a loaded session is degraded too");
}
