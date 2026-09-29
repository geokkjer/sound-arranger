//! The euclidean node's step walk from the API a user touches — the sibling of
//! `tests/clock_out.rs`. The grid a block walks is a pure function of the tempo,
//! and the walk had no bound: `mount euclidean` plus an absurd `set_tempo` was a
//! permanent render-thread wedge. The walk is now capped and what it could not
//! walk is **counted**, published under a context key the way the clock-out
//! plugin publishes its overflows.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use engine::*;

const SR: u32 = 48_000;

/// The euclidean generator on its own, plus a tone to own the bus (the
/// generator emits no audio, so without it the graph has no out node).
fn engine() -> Engine {
    let mut e = Engine::new(SR, 120.0, 4);
    e.register_factory(
        "euclidean",
        plugins::euclidean_factory,
        plugins::euclidean::EUCLIDEAN_PORTS,
        &[],
    );
    e.register_factory(
        "tone",
        plugins::tone_factory,
        plugins::tone::TONE_PORTS,
        plugins::tone::TONE_PARAMS,
    );
    e.mount("euclidean", &[]).unwrap();
    e.mount("tone", &[("gain", 0.2), ("blip_len", 800.0)])
        .unwrap();
    e
}

/// The euclidean generator with a pattern sparse enough that the **walk cap**,
/// not the trigger buffer, is what stops a block: one pulse in 1024 steps fills
/// no buffer, so the counts asserted below are about the walk alone.
fn engine_sparse() -> Engine {
    let mut e = Engine::new(SR, 120.0, 4);
    e.register_factory(
        "euclidean",
        plugins::euclidean_factory,
        plugins::euclidean::EUCLIDEAN_PORTS,
        &[],
    );
    e.register_factory(
        "tone",
        plugins::tone_factory,
        plugins::tone::TONE_PORTS,
        plugins::tone::TONE_PARAMS,
    );
    e.mount("euclidean", &[("steps", 1024.0), ("pulses", 1.0)])
        .unwrap();
    e.mount("tone", &[("gain", 0.0), ("blip_len", 8.0)])
        .unwrap();
    e
}

/// The live drop counter the plugin published, read back through the context
/// service — the accessor a host snapshot would use.
fn drops(e: &Engine) -> Arc<AtomicU64> {
    e.ctx
        .get::<Arc<AtomicU64>>(EUCLIDEAN_DROPS_KEY)
        .cloned()
        .expect("the euclidean plugin publishes its drop counter on apply")
}

fn count(e: &Engine) -> u64 {
    drops(e).load(Ordering::Relaxed)
}

/// **The defect, by name.** `set_tempo 1e300 4` is accepted (the floor in
/// `set_tempo` bounds the slow side only), and at that tempo the euclidean
/// node's block grid is 9.2e18 steps long on the *first* block at frame 0: the
/// walk never ended, `render` never returned, and the render thread wedged with
/// the shell frozen. Every block must return, and the steps it could not walk
/// must be visible rather than silently absent.
#[test]
fn an_absurd_tempo_does_not_wedge_the_step_walk() {
    // The script the verifier spelled out: a plain `mount euclidean`, then
    // `set_tempo` at a tempo whose block grid saturates.
    let mut e = engine();
    e.set_tempo(1e300, 4).expect("the fast side is not floored");
    // The counter is published by the mount that applies on the first render.
    let first = e.render(BLOCK);
    assert_eq!(first.len(), BLOCK, "the first block returned");
    let walked = i64::MAX as u64 - count(&e);
    assert!(walked > 0, "the block walked part of its grid");
    assert!(
        walked <= EUCLIDEAN_STEP_CAP,
        "and no more than the cap of it: {walked} of 9.2e18 steps"
    );
    for block in 1..4 {
        let rendered = e.render(BLOCK);
        assert_eq!(rendered.len(), BLOCK, "block {block} returned");
    }
    assert_eq!(
        count(&e),
        i64::MAX as u64 - walked,
        "the later blocks dropped nothing: `beat_at` saturates their whole grid at \\
         the same step index, so they had no steps to walk at all"
    );
}

/// The same defect one order of magnitude below the saturation, and the tempo a
/// script can actually spell: `set_tempo 1000000000 4`. 709 724 steps fit in one
/// 512-frame block at 48 kHz (511 frames is 177 430.556 beats, +1/4 of a beat, at
/// a quarter beat per step), and uncapped the block walked every one of them,
/// block after block. The walk is capped and the remainder counted, so the
/// accounting `walked + counted == the block's whole grid` holds exactly.
#[test]
fn a_dense_grid_does_not_run_away_in_one_block() {
    let mut e = engine_sparse();
    e.set_tempo(1_000_000_000.0, 4)
        .expect("a fast tempo is not floored");
    let rendered = e.render(BLOCK);
    assert_eq!(rendered.len(), BLOCK, "the block returned");
    assert_eq!(count(&e), 709_724 - EUCLIDEAN_STEP_CAP);
}

/// The counter is a *bound*, not a truncation the caller has to guess at: a
/// tempo whose grid fits the cap is walked whole, emits what it always did, and
/// counts nothing. A 120 bpm session is four steps per block.
#[test]
fn a_sane_tempo_walks_its_whole_grid_and_counts_nothing() {
    let mut e = engine_sparse();
    e.render(BLOCK);
    assert_eq!(count(&e), 0, "a 120 bpm grid fits the cap");
    // 240 bpm doubles it, still nothing counted.
    e.set_tempo(240.0, 4).unwrap();
    e.render(BLOCK);
    assert_eq!(count(&e), 0, "and so does 240 bpm");
}

/// The counter is the plugin's contribution, so it is withdrawn on unmount like
/// every other service a mount registers.
#[test]
fn unmounting_withdraws_the_drop_counter() {
    let mut e = engine_sparse();
    e.render(BLOCK);
    assert!(e.ctx.has(EUCLIDEAN_DROPS_KEY), "published on apply");
    e.unmount("euclidean").unwrap();
    e.render(BLOCK);
    assert!(
        !e.ctx.has(EUCLIDEAN_DROPS_KEY),
        "withdrawn by the disposer, like every other mount's service"
    );
}
