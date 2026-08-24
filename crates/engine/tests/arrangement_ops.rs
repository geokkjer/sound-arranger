//! The close-core plugin-message dispatch (P1.3.2): `Event::Arrangement` ops are
//! logged with `at_frame`, applied by the render path at that frame, and replay
//! reproduces them deterministically — without the core growing a per-plugin-op
//! variant (kimi design-review must-fix 9: single envelope + dispatch).

use std::cell::RefCell;
use std::rc::Rc;

use engine::{Engine, Event, SchedEvent, Value};

/// A handler that records the (op, u64 arg) pairs it applied, for determinism checks.
fn recorder(apply: Rc<RefCell<Vec<(String, u64)>>>) -> engine::OpHandler {
    Box::new(move |fields: &[(&'static str, Value)]| {
        let n = fields.iter().find(|(k, _)| *k == "n").and_then(|(_, v)| match v {
            Value::U64(n) => Some(*n),
            _ => None,
        });
        if let Some(n) = n {
            apply.borrow_mut().push(("bump".into(), n));
        }
        Ok(())
    })
}

#[test]
fn arranges_and_applies_at_frame() {
    let mut e = Engine::new(48_000, 120.0, 4);
    let seen = Rc::new(RefCell::new(Vec::new()));
    e.register_op_handler("demo.bump", recorder(seen.clone())).unwrap();

    // advance the clock, then arrange at that frame
    e.render(1024);
    let at = e.clock.frame();
    e.arrange("demo.bump", vec![("n", Value::U64(7))]).unwrap();

    // the event is logged with its frame
    let ev = e.log.events().last().unwrap();
    match ev {
        Event::Arrangement { op, fields, at_frame } => {
            assert_eq!(*op, "demo.bump");
            assert_eq!(*at_frame, at);
            assert_eq!(fields.len(), 1);
        }
        other => panic!("expected an Arrangement event, got {other:?}"),
    }

    // the handler applies on the CONTROL side when the frame is flushed
    e.flush_scheduled();
    let applied = seen.borrow();
    assert_eq!(*applied, vec![("bump".into(), 7)]);
}

#[test]
fn refuses_unregistered_op_and_never_logs() {
    let mut e = Engine::new(48_000, 120.0, 4);
    assert!(e.arrange("demo.nope", vec![]).is_err());
    assert!(e.log.events().iter().all(|ev| !matches!(ev, Event::Arrangement { .. })));
}

#[test]
fn refuses_non_finite_f32_field() {
    let mut e = Engine::new(48_000, 120.0, 4);
    e.register_op_handler("demo.bump", recorder(Rc::new(RefCell::new(Vec::new())))).unwrap();
    assert!(e.arrange("demo.bump", vec![("n", Value::F32(f32::NAN))]).is_err());
    assert!(e.log.events().iter().all(|ev| !matches!(ev, Event::Arrangement { .. })));
}

#[test]
fn replay_reproduces_applied_ops_deterministically() {
    let mut a = Engine::new(48_000, 120.0, 4);
    let seen_a = Rc::new(RefCell::new(Vec::new()));
    a.register_op_handler("demo.bump", recorder(seen_a.clone())).unwrap();

    // a sequence of ops issued at increasing frames, flushed on the control side
    for i in 1..=3u64 {
        a.render(512);
        assert!(a.arrange("demo.bump", vec![("n", Value::U64(i))]).is_ok());
        a.flush_scheduled();
    }

    // a fresh engine, same log, replayed
    let mut b = Engine::new(48_000, 120.0, 4);
    let seen_b = Rc::new(RefCell::new(Vec::new()));
    b.register_op_handler("demo.bump", recorder(seen_b.clone())).unwrap();
    b.replay_from(&a.log).unwrap();
    // reach each op frame and flush (control side), as a host does
    for _ in 0..3u64 {
        b.render(512);
        b.flush_scheduled();
    }

    assert_eq!(seen_a.borrow().as_slice(), seen_b.borrow().as_slice(), "replay must apply the same ops");
    assert!(!seen_a.borrow().is_empty());
}

#[test]
fn replay_repopulates_the_log_for_continuation() {
    // replay_from must push the replayed events into the engine's own log, so a
    // continued session's log still describes the audio produced (kimi must-fix 2).
    let mut a = Engine::new(48_000, 120.0, 4);
    a.register_op_handler("demo.bump", recorder(Rc::new(RefCell::new(Vec::new())))).unwrap();
    a.render(1);
    a.arrange("demo.bump", vec![("n", Value::U64(5))]).unwrap();
    a.flush_scheduled();

    let mut b = Engine::new(48_000, 120.0, 4);
    let seen_b = Rc::new(RefCell::new(Vec::new()));
    b.register_op_handler("demo.bump", recorder(seen_b.clone())).unwrap();
    b.replay_from(&a.log).unwrap();

    // after replay, b's log carries the whole replayed prefix
    assert_eq!(b.log.len(), a.log.len());
    assert!(b.log.events().iter().any(|ev| matches!(ev, Event::Arrangement { .. })));

    // continuing to edit appends to a log that still describes the session
    b.render(1);
    b.arrange("demo.bump", vec![("n", Value::U64(6))]).unwrap();
    b.flush_scheduled();
    assert_eq!(b.log.len(), a.log.len() + 1);
    assert_eq!(seen_b.borrow().as_slice(), &[("bump".into(), 5), ("bump".into(), 6)]);
}

#[test]
fn sched_event_is_public_and_constructible() {
    // Exercising that SchedEvent::Arrangement carries the op + fields (the public
    // shape a host reads when driving the engine).
    let ev = SchedEvent::Arrangement {
        op: "demo.bump",
        fields: vec![("n", Value::U64(1))],
    };
    match ev {
        SchedEvent::Arrangement { op, .. } => assert_eq!(op, "demo.bump"),
        _ => unreachable!(),
    }
}
