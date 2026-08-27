//! The parked-arrangement safety net (control→render handoff, 2026-08-27):
//! an arrangement op that reaches the render stack (a host that rendered
//! without flushing) is parked for the control side instead of dropped, so a
//! logged op is never lost and live/replay cannot diverge. In debug builds the
//! contract violation additionally trips `debug_assert!` — these tests capture
//! that panic to exercise both profiles' behavior through one code path.

use std::cell::RefCell;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::rc::Rc;

use engine::{Engine, Value};

fn recorder(seen: Rc<RefCell<Vec<u64>>>) -> engine::OpHandler {
    Box::new(move |fields: &[(&'static str, Value)]| {
        if let Some(Value::U64(n)) = fields.iter().find(|(k, _)| *k == "n").map(|(_, v)| v) {
            seen.borrow_mut().push(*n);
        }
        Ok(())
    })
}

/// Render `e`, tolerating the debug-build tripwire: in debug builds the
/// contract violation panics right after parking; in release it recovers
/// silently. Both engines in a determinism comparison take the same branch.
fn render_tolerating_violation(e: &mut Engine) -> Vec<f32> {
    let result = catch_unwind(AssertUnwindSafe(|| e.render(512)));
    match result {
        Ok(out) => out,
        #[cfg(debug_assertions)]
        Err(_payload) => {
            drop(_payload); // the expected tripwire unwind — tolerate it
            Vec::new()
        }
        #[cfg(not(debug_assertions))]
        Err(_) => panic!("the parking tripwire must never panic in a release build"),
    }
}

#[test]
fn render_stack_arrangement_parks_not_drops() {
    let mut e = Engine::new(48_000, 120.0, 4);
    let seen = Rc::new(RefCell::new(Vec::new()));
    e.register_op_handler("demo.bump", recorder(seen.clone())).unwrap();

    e.render(1024); // clock advances; contract-abiding so far
    e.arrange("demo.bump", vec![("n", Value::U64(7))]).unwrap();
    // No flush before rendering — the old behavior dropped the op here in
    // release while keeping it in the log.
    let _ = render_tolerating_violation(&mut e);

    assert!(e.scheduler.is_empty(), "the reached op left the queue");
    e.flush_scheduled();
    assert_eq!(*seen.borrow(), vec![7], "the parked op applies on flush");
    e.render(256); // the engine keeps rendering normally afterwards
}

#[test]
fn parked_misuse_is_deterministic_live_and_replayed() {
    // Engine A commits the misuse (render past an unflushed op); engine B
    // replays the SAME log with the SAME call rhythm — same renders, same
    // flush points, same parking. Byte-identical output + identical handler
    // history is the invariant (in debug builds both engines also trip and
    // unwind at the same point, which the shared branch below tolerates).
    let mut a = Engine::new(48_000, 120.0, 4);
    let seen_a = Rc::new(RefCell::new(Vec::new()));
    a.register_op_handler("demo.bump", recorder(seen_a.clone())).unwrap();
    let mut outs_a: Vec<Vec<f32>> = Vec::new();
    for i in 1..=3u64 {
        outs_a.push(a.render(512));
        a.arrange("demo.bump", vec![("n", Value::U64(i))]).unwrap();
        if i == 2 {
            // the violation: this flush is skipped and the next render reaches
            // the op on the render stack — parked, applied at the NEXT flush,
            // never lost (in debug, unwind skips the rest of that block)
            outs_a.push(render_tolerating_violation(&mut a));
        } else {
            a.flush_scheduled();
        }
    }
    a.flush_scheduled(); // drain whatever remains

    let mut b = Engine::new(48_000, 120.0, 4);
    let seen_b = Rc::new(RefCell::new(Vec::new()));
    b.register_op_handler("demo.bump", recorder(seen_b.clone())).unwrap();
    b.replay_from(&a.log).unwrap();
    let mut outs_b: Vec<Vec<f32>> = Vec::new();
    for i in 1..=3u64 {
        outs_b.push(b.render(512));
        if i == 2 {
            outs_b.push(render_tolerating_violation(&mut b)); // identical misuse
        } else {
            b.flush_scheduled();
        }
    }
    b.flush_scheduled();

    assert_eq!(seen_a.borrow().as_slice(), &[1, 2, 3], "misused op applies late, not never");
    assert_eq!(seen_a.borrow().as_slice(), seen_b.borrow().as_slice());
    for (i, (x, y)) in outs_a.iter().zip(&outs_b).enumerate() {
        assert_eq!(x, y, "render {i} diverged between live and replay");
    }
}
