# Agent Note: A clip source is a pool id, not a path

Status: implemented

## Problem

A clip's `source` is a **pool id** — the stem of a file in the media pool — but nothing
enforced that, and the one resolver in production joined the id straight onto the pool
directory. `HostSession::set_pool` built it by hand:

```rust
let resolver_dir = dir.clone();
let resolver: media::PoolResolver = std::sync::Arc::new(move |id| {
    let p = resolver_dir.join(format!("{id}.wav"));
    p.is_file().then_some(p)
});
```

`Pool::path_for` — the guarded lookup, with the same signature as the resolver — refuses
an id that is not a plain stem (`valid_id`: no path separator, no `..`, no whitespace).
Nothing called it. It was referenced from two pool tests and one host test, and
`valid_id` was **private**, so the host could not have applied it: the hardening the
[pool note](../feature/2026-08-24-p1-3-3-media-pool.md) records was unreachable from
where it mattered.

The id is user-craftable from two directions. `add_clip` takes it as an operand —
`arrange add_clip t0 c0 ../../../../tmp/other 0 4800 0 0 0 1.0` — and `validate_clip`
(which bounds every other reference a clip holds) never looked at it. And a `Clip` is
`Deserialize`, so a session file is a document a person can edit, while a session is
*replayed* op for op. Either way the escape was silent: `ArrangerNode::new` calls
`resolve(&c.source)` and trusts the answer, because a resolver it did not build is a
thing it cannot validate (`crates/media/src/arranger.rs`). The file that answered was
opened, streamed through the arranger, and written into bounces and exports. A pool is a
**working** directory — pointing it at a library of originals rewrites them — so the
sibling directories a real pool sits next to are exactly the kind of thing a `../` id
would name.

## Decision

**The seam a host hands the arranger is the pool's own guarded lookup.**
`Pool::resolver() -> PoolResolver` closes over the `Pool` and delegates to `path_for`,
and `set_pool` builds the production resolver with it. A `PoolResolver` built by hand
can still be unguarded, but the platform no longer builds one: the guard sits where the
pool is, so a resolver *is* a guarded lookup unless a caller goes out of its way.

**A `source` that is not a plain pool id is refused by `validate_clip`**, beside the
name check that already refuses a clip name the log cannot spell. Every op that sets a
source goes through it (`AddClip`, and `Stretch`, the one op that repoints a clip at
rendered material), so the value can never hold a source that would address a file
outside the pool — and, since a refused op is never logged, a saved session cannot carry
one either. `valid_id` is `pub` for this: the rule belongs to the pool and a caller
cannot re-derive it.

The two ends are complementary and each is worth having alone. `validate_clip` is the
guard at the **writing** end: a bad value is refused where the user typed it, by name,
and never reaches the log. `path_for` is the guard at the **reading** end, and it is the
one that matters for the untrusted-session case: a `Timeline` read from anywhere else
(a snapshot, a future binary format, a hand-built `Track` — all legal `&self` to
`Timeline::apply`) never passed `AddClip`, and `ArrangerNode::new` validates every clip
it holds, so a clip that reaches the render path without passing the value check is
refused by the resolver instead.

## Alternatives considered

- **Make `valid_id` `pub` and call it inside the host's closure**, leaving
  `Pool::resolver` out. Rejected: it works, and it is the smaller diff, but it keeps
  the pool's own rule out of the pool — the next host surface that resolves a source
  has to remember to call it. `Pool::path_for` already *is* the guarded lookup, and it
  was written; the bug was that nothing used it.
- **Canonicalize and check containment** (`path.starts_with(canonical_dir)`) in the
  closure instead of validating the id. Rejected: it accepts ids the pool's naming
  conventions refuse (a source with a space, which the `host v1` log cannot name back,
  would resolve and then be unsaveable), and it needs a filesystem hit per lookup plus a
  symlink story. The id rule is the rule the rest of the pool already uses.
- **The value check alone** (report's "belt and braces", taken as the whole fix).
  Rejected: it closes the *writing* end only. Every read of a clip's source still trusts
  the resolver it was handed, which is the property the report is actually about.
- **Sanitize the source** (`../../x` → something inside the pool) instead of refusing.
  Rejected for the reason `Pool::import` sanitizes a stem but refuses a separator: a
  mangled path is a surprise, and it would silently play the wrong material.
- **A `Snapshot`/`Deserialize` normalisation** for clips, the way markers are normalised
  in the same module. Deferred, for the reason the [geometry-op
  fix](2026-09-29-geometry-ops-leave-a-renderable-clip.md) gives: a clip cannot be
  normalised without silently rewriting a value, and the workspace has no snapshot route
  yet. If one lands, it belongs in that note's open question.
- **Pin this with a pool test only.** Rejected: `Pool::path_for`'s guard was already
  tested, and the test passed on the code that had the bug. A test that cannot fail on
  the unfixed code is not a regression test; the host test reaches the closure the
  platform actually builds.

## Consequences

- `arrange add_clip t0 c0 ../secret 0 4800 0 0 0 1.0` is refused by name — *"clip 'c0'
  source '../secret' is not a pool id (a source is a file stem, not a path)"* — nothing
  is logged, and a plain id is unaffected.
- A clip carrying such a source, if it reaches the value by another route, is refused at
  the **resolver**: `arranger: pool has no source '../secret'`. A `stretch` on one says
  `pool source '../secret' is missing`. Neither opens a file.
- `Pool::resolver` is the way a host builds a `PoolResolver`; `Pool::path_for` is the
  lookup it delegates to and is unchanged.
- **The guard is now reachable from where it matters**, which is the whole defect: the
  pool's id rule is enforced on the platform's only source-reading path instead of only in
  the pool's own tests.
- **Three regression tests, each verified to fail on the unfixed code** (the two host ones
  reverted one fix at a time to confirm which defect they pin):
  `the_pool_resolver_only_resolves_a_plain_pool_id` (host, the production closure — fails
  with `'../secret' is not a plain pool id, so it resolves to nothing`),
  `a_clip_source_that_would_escape_the_pool_is_refused` (host, the reported trigger as a
  script — fails because the gesture is *not* refused), and
  `a_clip_source_that_is_not_a_pool_id_is_refused` (value model — `AddClip` accepts
  `../../elsewhere` on the unfixed value). Plus
  `resolver_refuses_an_id_that_is_not_a_plain_pool_id` in `crates/media/tests/pool.rs`,
  which pins the new seam against a pool that has a readable WAV one level above it.
- Nothing on the render path changed: the refusal is control-side (`validate_clip` in
  `ArrangerNode::new`, the value check on `AddClip`), and `fade_gain`, the readers and
  the ring are untouched.
- The [P1.3.3 pool note](../feature/2026-08-24-p1-3-3-media-pool.md) is updated with
  `Pool::resolver` beside `path_for`, and the [P1.3.0
  timeline-value note](../feature/2026-08-24-p1-3-0-timeline-value.md) with the source
  invariant among the ones every mutating op enforces.

*Authored with Space Bunny · OpenCode, 2026-09-29.*
