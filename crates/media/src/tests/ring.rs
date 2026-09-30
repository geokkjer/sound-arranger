use super::*;

#[test]
fn ordered_transfer() {
    let ring = Spsc::new(4);
    // order preserved across wrap-around, in lockstep with the capacity
    let mut next = 0u32;
    for _ in 0..3 {
        for _ in 0..4 {
            assert!(ring.try_push(next), "push {next}");
            next += 1;
        }
        for i in 0..4 {
            assert_eq!(ring.try_pop(), Some(next - 4 + i), "pop {i}");
        }
    }
    assert!(ring.try_pop().is_none());
    assert!(ring.is_empty());
}

#[test]
fn empty_then_full() {
    let ring = Spsc::new(4);
    assert!(ring.try_pop().is_none());
    for i in 0..4 {
        assert!(ring.try_push(i));
    }
    assert!(!ring.try_push(99), "full ring refuses");
    assert_eq!(ring.len(), 4);
    for i in 0..4 {
        assert_eq!(ring.try_pop(), Some(i));
    }
    assert!(ring.try_push(7));
    assert_eq!(ring.try_pop(), Some(7));
}

/// Wrap-around: a full cycle of the head/tail indices must not corrupt
/// order or values.
#[test]
fn wraps_without_losing_order() {
    let ring = Spsc::new(8);
    for round in 0..3 {
        for i in 0..8 {
            assert!(ring.try_push(round * 8 + i));
        }
        for i in 0..8 {
            assert_eq!(ring.try_pop(), Some(round * 8 + i));
        }
    }
}

#[test]
fn head_tail_count_on_distinct_cache_lines() {
    // pins the false-sharing fix: the producer's `head`, the consumer's
    // `tail`, and the shared `count` must each be on their own 64-byte line.
    let ring = Spsc::<f32>::new(4);
    let line = |p: *const AtomicUsize| (p as usize) / 64;
    let h = line(&ring.head);
    let t = line(&ring.tail);
    let c = line(&ring.count);
    assert_ne!(h, t, "head and tail must not share a cache line");
    assert_ne!(t, c, "tail and count must not share a cache line");
    assert_ne!(h, c, "head and count must not share a cache line");
}

/// Cross-thread stress: 100k samples, one producer thread, one consumer
/// thread, must arrive in order with none lost.
#[test]
fn cross_thread_stress() {
    const N: u64 = 100_000;
    let ring = std::sync::Arc::new(Spsc::new(1024));
    let (pr, pc) = (ring.clone(), ring.clone());
    let producer = std::thread::spawn(move || {
        for i in 0..N {
            while !pr.try_push(i) {
                std::thread::yield_now();
            }
        }
    });
    let consumer = std::thread::spawn(move || {
        let mut got = 0u64;
        let mut next = 0u64;
        while got < N {
            if let Some(v) = pc.try_pop() {
                assert_eq!(v, next, "order violated");
                next += 1;
                got += 1;
            } else {
                std::thread::yield_now();
            }
        }
    });
    producer.join().unwrap();
    consumer.join().unwrap();
    assert!(ring.is_empty());
}
