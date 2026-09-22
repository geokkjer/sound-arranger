//! A lock-free single-producer/single-consumer ring buffer, std only.
//!
//! Contract: exactly one thread calls [`Spsc::try_push`], exactly one other
//! thread calls [`Spsc::try_pop`]. Everything else is shared safely through
//! `&self` — the `count` atomic carries the happens-before edges (producer's
//! slot write → `fetch_add(Release)` → consumer's `load(Acquire)` → slot read,
//! and the mirror for the consumer's slot `take`). No allocation after
//! construction; the audio path touches only `try_push`/`try_pop`.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};

#[repr(C, align(64))]
pub struct Spsc<T> {
    /// Atomics first, each padded onto its own 64-byte cache line: `head` is
    /// producer-written, `tail` consumer-written, `count` written by both. The
    /// buffer (a fat pointer) is last, so the padding does not depend on its size.
    head: AtomicUsize,
    _pad0: [u8; 56],
    tail: AtomicUsize,
    _pad1: [u8; 56],
    count: AtomicUsize,
    _pad2: [u8; 56],
    slots: Box<[UnsafeCell<Option<T>>]>,
}

// SAFETY: the SPSC contract — one producer, one consumer, and the count
// protocol guaranteeing producer and consumer never touch the same slot — makes
// sharing the `UnsafeCell` slots sound.
unsafe impl<T: Send> Sync for Spsc<T> {}

// Compile-time check that the cache-line isolation actually holds (the hand-rolled
// pads are layout-dependent; this pins it for the targets we build on).
const _: () = {
    assert!(
        std::mem::offset_of!(Spsc<f32>, head) % 64 == 0,
        "Spsc head not cache-line-aligned"
    );
    assert!(
        std::mem::offset_of!(Spsc<f32>, tail) % 64 == 0,
        "Spsc tail not cache-line-aligned"
    );
    assert!(
        std::mem::offset_of!(Spsc<f32>, count) % 64 == 0,
        "Spsc count not cache-line-aligned"
    );
};

impl<T> Spsc<T> {
    pub fn new(capacity: usize) -> Self {
        assert!(
            capacity.is_power_of_two() && capacity > 0,
            "Spsc capacity must be a power of two"
        );
        Spsc {
            head: AtomicUsize::new(0),
            _pad0: [0; 56],
            tail: AtomicUsize::new(0),
            _pad1: [0; 56],
            count: AtomicUsize::new(0),
            _pad2: [0; 56],
            slots: (0..capacity).map(|_| UnsafeCell::new(None)).collect(),
        }
    }

    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// Producer side: push unless full. Returns false when full (caller decides
    /// the policy — the recorder counts it as an overrun, the player's reader
    /// thread waits).
    pub fn try_push(&self, value: T) -> bool {
        if self.count.load(Ordering::Acquire) >= self.slots.len() {
            return false;
        }
        let head = self.head.load(Ordering::Relaxed);
        let idx = head & (self.slots.len() - 1);
        // SAFETY: the producer owns slot `idx` — with count < capacity the slot
        // is free (occupied slots are tail..head), so the consumer cannot be
        // reading it.
        unsafe {
            *self.slots[idx].get() = Some(value);
        }
        self.head.store(head.wrapping_add(1), Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Release);
        true
    }

    /// Consumer side: pop unless empty.
    pub fn try_pop(&self) -> Option<T> {
        if self.count.load(Ordering::Acquire) == 0 {
            return None;
        }
        let tail = self.tail.load(Ordering::Relaxed);
        let idx = tail & (self.slots.len() - 1);
        // SAFETY: the consumer owns slot `idx` (occupied, count > 0); the
        // producer cannot be writing it.
        let value = unsafe { (*self.slots[idx].get()).take() };
        self.tail.store(tail.wrapping_add(1), Ordering::Relaxed);
        self.count.fetch_sub(1, Ordering::Release);
        value
    }

    pub fn len(&self) -> usize {
        self.count.load(Ordering::Acquire)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
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
}
