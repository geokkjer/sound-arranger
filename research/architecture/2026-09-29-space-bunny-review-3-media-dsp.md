# Space-bunny external review — media crate — DSP and arrangement

> **Disposition.** External deep review, 2026-09-29, against `49e8774` (clean tree). Reviewer:
> Space Bunny (`opencode/space-bunny-free`) driven through OpenCode by DeepSeek Harness; the
> supervisor (DeepSeek-V4.1-Flash) wrote the scope, then handed every CRITICAL/MAJOR claim to an
> **independent adversarial verifier** that had to falsify it before it counted.
>
> This file is the **media crate — DSP and arrangement** scope, verbatim: the reviewer's report, then its verifier's
> verdicts. What was done about each finding — fixed, deferred or refuted — is in
> [the index](2026-09-29-space-bunny-review.md). Findings are the reviewer's, not the
> supervisor's: read the verdicts before acting on any single claim.

---

## 1. Reviewer's report (verbatim)

## media DSP + arrangement review

`crates/media/{resample,dither,stretch,drift,arranger,pool,timeline,clip_editor}.rs` + `tests/{clip_editor_engine,pool}.rs` at `49e8774`. The previous review's criticals 1–4 and majors 5–6 are genuinely fixed (verified below); minors 48 and 49 are not. Everything below is new or an incomplete fix.

### CRITICAL

1. **`fade_in + fade_out` overflows `u64` on a user-reachable path — panic in debug (poisoning the timeline mutex), silent clip mute in release**

- **Severity** — CRITICAL (invariant 1 by proxy: a panic path reachable from the `host v1` text format; and a logged op that renders a clip silent)
- **Location** — `crates/media/src/timeline.rs:349` and `crates/media/src/timeline.rs:759`
- **Trigger** — one line of a script or session file: `arrange set_clip_fade t0 c0 18446744073709551615 1` (equivalently `add_clip t0 c0 s1 0 4000 0 18446744073709551615 1 1.0`). The host's parser takes both fade operands as raw `u64` with no bound — `crates/host/src/lib.rs:3900-3907`, and `add_clip` words 7/8 at `crates/host/src/lib.rs:3820-3821`; neither is a `frame_operand`, so `snap=` cannot even be used to clamp them.
- **Wrong behaviour** — `u64::MAX + 1` wraps to `0`, so `0 > src_len` is false and the check passes.
  - *debug*: `attempt to add with overflow` **panics inside `ClipEditor::apply`'s lock critical section** (`clip_editor.rs:549-556` holds `self.timeline.lock()`), so the `Mutex` is poisoned: every later `snapshot()`/`host.arrangement()` returns `Err("timeline poisoned")` and every later arrange op is refused. The panic is not caught in production.
  - *release*: the op is **accepted and logged**, and `arranger.rs:93-95` then computes `gin = min(off as f32 / 1.8446744e19, 1.0) = 0.0` for every sample — the clip renders **completely silent** while the log, the panel and the clip's gain all claim it is audible. Any later `MoveClip`/`Trim`/`Stretch` on it re-validates and is refused, so the clip cannot even be nudged.
- **Evidence**
  ```rust
  // timeline.rs:349  (validate_clip)
      if c.fade_in + c.fade_out > c.src_len {
          return Err(format!("clip '{}' fades exceed the clip length", c.id));
      }
  // timeline.rs:759  (SetClipFade)
                  if fade_in + fade_out > self.tracks[ti].clips[ci].src_len {
                      return Err("fades exceed the clip length".into());
                  }
  ```
  The project already *documents* this as a debug-only panic: `crates/host/src/lib.rs:4106-4112` uses `fade_in: u64::MAX, fade_out: 1` to poison the editor on purpose, gated `#[cfg(debug_assertions)]` with the comment "release builds have no reachable poison path through the public API" — i.e. in release the same input is *accepted*.
- **Fix** — refuse instead of wrapping, in both places: `if c.fade_in.checked_add(c.fade_out).is_none_or(|s| s > c.src_len)`, and the same in `SetClipFade`. Belt and braces: cap fades at parse (`fade_in.min(src_len)`) in the host's `set_clip_fade`/`add_clip` arms so the wire schema cannot carry a value the value model rejects.
- **Verification** — read `validate_clip` and every caller; read `encode_op`/`decode_op` for `SetClipFade` (round-trips `u64` verbatim, so a poisoned log replays the same); read `fade_gain`. No test uses a fade near `u64::MAX` except the poison test, which asserts the *panic* and therefore cannot catch the release wrap. `gain_and_fade_validate` only tests `fade_in 80 + fade_out 80 > 100`.

2. **Razor-split and chop emit clips that violate the timeline's own fade invariant — the renderer then refuses the whole track, so the session will not play**

- **Severity** — CRITICAL (a two-keypress, fully legal, *logged* edit sequence produces a value the render path rejects; the offending clip cannot then be moved or trimmed)
- **Location** — `crates/media/src/timeline.rs:584-593` (`RazorSplit`) and `crates/media/src/timeline.rs:841-855` (`ChopClip`); consumed at `crates/media/src/arranger.rs:128`
- **Trigger** — the TUI's `f` / `F` set a fade to the playhead (`spikes/tui-shell/src/main.rs:2270-2300`, cap `src_len - fade_out`, so a **full-length** fade-in is a one-keypress reachable state — the shell's own test issues `set_clip_fade t0 c0 {src_len} 0`, `main.rs:5376`). Then `x` razor-splits at any earlier frame. Concretely: clip `c0` at frame 0, `src_len = 48000`, `fade_in = 47999, fade_out = 0` (legal: `47999 + 0 ≤ 48000`), then `razor_split t0 c0 L R 12000`.
- **Wrong behaviour** — `left` inherits `fade_in = 47999` and shrinks to `src_len = 12000`; `left.fade_out = 0` is zeroed but `left.fade_in` is not touched, and the arm never calls `validate_clip`. The half violates `fade_in + fade_out ≤ src_len`. `ArrangerNode::new` (added by the previous review's fix) then returns `Err("arranger: clip 'L' fades exceed the clip length")` for the *whole track*, `wire_arranger` propagates it, and `HostSession::render` returns `Err` — **nothing on that track plays, and every subsequent edit keeps failing until the fade is re-set by hand.** `ChopClip` has the same hole: `fade_in: if i == 0 { c.fade_in }` on a piece of `src_len / times` frames (chop a 100-frame clip with `fade_in = 50` into 10 slices and 2 of the 10 pieces are invalid).
- **Evidence**
  ```rust
  // timeline.rs:585-593 — nothing validates the halves
              let mut left = c.clone();
              left.id = new_left.clone();
              left.src_len = split_in;
              left.fade_out = 0; // the split seam is hard (a crossfade is a later SetClipFade)
              let mut right = c.clone();
              right.id = new_right.clone();
              right.at_frame = *at_frame;
              right.src_len = c.src_len - split_in;
              right.fade_in = 0;
  // timeline.rs:850-851 — same, per piece
                          fade_in: if i == 0 { c.fade_in } else { 0 },
                          fade_out: if i + 1 == times_f { c.fade_out } else { 0 },
  // arranger.rs:128 — the consumer that now refuses
              crate::timeline::validate_clip(c).map_err(|e| format!("arranger: {e}"))?;
  ```
  `validate_clip` is called in exactly six places (`timeline.rs:542, 647, 669, 685, 704, 899`) — **never** in `RazorSplit` or `ChopClip`, which are the only two ops that *shrink* `src_len` without re-validating. (`LoopRegion` only grows it, `Duplicate` clones a valid clip, so those two are sound.)
- **Fix** — in `RazorSplit`, after computing the halves: `left.fade_in = left.fade_in.min(left.src_len); right.fade_out = right.fade_out.min(right.src_len);` (or `validate_clip` each half and refuse the split with a message naming the fade). In `ChopClip`, apply the same `.min(piece.src_len)` to `i == 0`'s `fade_in` and `times-1`'s `fade_out`. Add a regression test: split a clip with a full-length fade, then assert `validate_clip` accepts both halves *and* that `ArrangerNode::new` builds.
- **Verification** — traced the whole op chain into `wire_arranger` (`crates/host/src/lib.rs:1805-1811`) and `render` (`:1857-1858`), both of which return `Result`, so this surfaces as a hard error rather than a panic. Tests: `razor_split_inside_produces_two_sorted_halves`, `razor_split_keeps_sorted_with_overlapping_neighbor`, `a_reversed_clip_reads_backwards_and_the_ops_mirror` and `chop_splits_a_clip_into_contiguous_pieces` **all use `fade_in: 0, fade_out: 0`**; `clip_editor_engine.rs:54-59` sets `64/128` on a 2000-frame half. Nothing covers a fade longer than a resulting half, which is why this survived.

### MAJOR

3. **The production `PoolResolver` bypasses `valid_id`, so a clip's `source` escapes the pool directory**

- **Severity** — MAJOR
- **Location** — `crates/host/src/lib.rs:746-749` (the resolver) vs `crates/media/src/pool.rs:148-160` / `:183-191` (the guard); consumed at `crates/media/src/arranger.rs:129-132`
- **Trigger** — `arrange add_clip t0 c0 ../../../../tmp/other 0 100 0 0 0 1.0`. The `source` operand is `words[3]`, an arbitrary whitespace-free token; `validate_clip` (`timeline.rs:324-353`) never inspects `source`, and `Pool::import`/`write_source` validate only the ids *they* derive.
- **Wrong behaviour** — `resolver_dir.join("../../../../tmp/other.wav")` escapes the pool; if that file exists it is opened, streamed through the arranger and written into bounces and exports. Replay of a saved session does the same, so an untrusted session file reads arbitrary readable WAVs.
- **Evidence**
  ```rust
  // crates/host/src/lib.rs:745-749 — the only resolver used in production
          let resolver_dir = dir.clone();
          let resolver: media::PoolResolver = std::sync::Arc::new(move |id| {
              let p = resolver_dir.join(format!("{id}.wav"));
              p.is_file().then_some(p)
          });
  // crates/media/src/pool.rs:148-152 — the guard it skips, and it is *private*
  /// Whether `id` is a plain file stem (no path separators, no `..`, non-empty) —
  /// a clip id is user-craftable, so it must not escape the pool dir (kimi pool
  /// should-fix 5).
  fn valid_id(id: &str) -> bool {
  ```
  `Pool::path_for` (the only public guarded lookup) is referenced **only** from `crates/media/tests/pool.rs:114,140,145` and one host test (`crates/host/src/lib.rs:6486`); no production path calls it. And because `valid_id` is private, a host *cannot* apply it — the hardening the previous review credited is unreachable from where it matters.
- **Fix** — have the host build its resolver from `Pool::path_for` (e.g. add `pub fn resolver(&self) -> PoolResolver` on `Pool` that closes over the `Pool` and delegates), or make `valid_id` `pub` and call it in the closure. Belt and braces: reject a clip whose `source` fails `valid_id` in `validate_clip`, so a bad value can never be logged.
- **Verification** — grepped every `PoolResolver` construction (`arranger.rs` tests, `host:746`) and every `path_for` call; read `arranger.rs:129-132`, which trusts `resolve()` unconditionally because it *cannot* validate. No test constructs a clip with a traversing `source`.

4. **`replace_sources` deletes the previous take *before* the new material is committed, contradicting its own doc and losing the take on any commit failure**

- **Severity** — MAJOR
- **Location** — `crates/media/src/pool.rs:572-575` (the doc), `:423` and `:486` (the call sites)
- **Trigger** — any import whose `fs::rename` fails at commit time (a read-only directory, `ENOSPC` on the directory entry, Windows destination-exists, or a crash/power loss inside the window). The window is small but it is exactly the window the design's staging exists to close.
- **Wrong behaviour** — the old `{id}.wav` **and its `.peaks`** are already unlinked, the staged temporaries are then removed by the error path, and the new material never lands: the id is simply gone from the pool. The same arm also `?`-propagates `rebuild_peaks` *after* the renames (`:440`), so a peaks failure leaves a partially-committed import — the exact outcome `:401-404` promises cannot happen.
- **Evidence**
  ```rust
  // pool.rs:573-575 — the contract
      /// Called only once the new material is safely written, so a failed import
      /// changes nothing. A filesystem error here is ignored: `list` reports what
      /// remains, and the next import retries.
  // pool.rs:419-426 — the multi-channel commit
              // count — is replaced (a clip on `jam` must not keep playing an old
              // take, or a channel that no longer exists).
              self.replace_sources(&id, channels);
              let mut sources = Vec::with_capacity(channels as usize);
              for (tmp, ch_dest, frames_out) in &staged {
                  if let Err(e) = fs::rename(tmp, ch_dest) {
  // pool.rs:486-487 — the mono commit, same shape
          self.replace_sources(&id, 1);
          if let Err(e) = fs::rename(&tmp, &dest) {
  ```
  (The delete-first order is *load-bearing* for the Windows fallback noted at `:305-308`, so the fix must keep the destination absent at rename time while preserving the old bytes — e.g. rename the old file to a `.bak` sidecar first and restore it if the commit fails, then sweep.)
- **Fix** — stage-then-sweep: keep the old sources until every `rename` *and* every `rebuild_peaks` has succeeded, then sweep the stale stems; on any failure, restore. Alternatively `fs::rename(&old, &old.with_extension("wav.prev"))` before the commit and roll back on error.
- **Verification** — read `import` (both branches) and `expand_one` end to end; `expand_one` gets the order *right* (it stages, preserves, commits siblings, then renames the primary last, `:708-797`), which is the proof that `import` is the outlier. No test injects a rename failure — the pool tests only exercise the success paths.

5. **A mono import over a previously-split id leaves the old `{id}.ch0` addressable, so a clip keeps playing the take the user replaced**

- **Severity** — MAJOR
- **Location** — `crates/media/src/pool.rs:588-596` (the `stale` predicate), called with `keep_channels = 1` from `:486`
- **Trigger** — `pool.import("stereo_jam.wav", 48000)` → the pool holds `jam.ch0.wav` and `jam.ch1.wav`, and the returned `Import::ids()` is `["jam.ch0","jam.ch1"]` (the shell places clips on both). Then the user imports a **new mono** `jam.wav` over the same id.
- **Wrong behaviour** — `replace_sources("jam", 1)` deletes `jam.wav` and `jam.ch1.wav` (`k = 1 >= 1`) but **keeps `jam.ch0.wav`** (`k = 0 < 1`), which still holds the *previous stereo file's left channel*. The new mono `jam` is unrelated material, so a clip on `jam.ch0` — an id the pool itself handed out — silently plays the old take. This is precisely the failure the function was written for.
- **Evidence**
  ```rust
  // pool.rs:588-596
              // `{id}` itself, or `{id}.ch{k}` with k beyond what this import keeps.
              let stale = match stem.strip_prefix(id) {
                  Some("") => true,
                  Some(rest) => rest
                      .strip_prefix(".ch")
                      .and_then(|k| k.parse::<u16>().ok())
                      .is_some_and(|k| k >= keep_channels),
                  None => false,
              };
  ```
  against the doc it implements (`:566-571`): "**Importing replaces the id** — without this, a stereo file imported over a stem that held a 5.1 take (or a mono source) would leave the old material addressable, and a clip on that id would keep playing audio the user just replaced."
- **Fix** — for the mono path pass `keep_channels = 0` (so `{id}.ch0` is stale too), or change the predicate to `k >= keep_channels || keep_channels == 1`. The multi-channel call site is correct as written (`ch0..channels-1` are about to be overwritten).
- **Verification** — read `import`'s mono branch (`:465-512`) and `replace_sources`; the reverse order (split → mono) is **not** covered. `a_narrower_import_removes_the_older_channels` tests 4-channel → stereo (`keep_channels = 2`, where `ch0` *is* overwritten), and `a_stereo_import_replaces_the_mono_source_holding_that_id` tests mono → stereo. No test does stereo → mono.

6. **`ChopClip` has no bound on the piece count: one logged op is quadratic before it is fatal**

- **Severity** — MAJOR
- **Location** — `crates/media/src/timeline.rs:795-813` (the only guards) and `:829-833` (the per-piece work)
- **Trigger** — `arrange chop t0 c0 4294967295 pre` on a clip with a large `src_len`. `times` is a `u32` straight off the wire (`crates/host/src/lib.rs:3919-3928`, no cap) and the only bound is `times > c.src_len` (`:808-813`), i.e. up to `i64::MAX`.
- **Wrong behaviour** — the loop runs `times` times, and each iteration does a full `clip_id_exists` scan over every track and every clip, so it is `O(times × clips)` string comparisons: `chop … 10000000` is ~10⁷ × N comparisons and hangs long before anything is allocated. If it ever completes, `pieces` holds `times` `Clip`s each with **three heap `String`s** (`id`, plus `name: c.name.clone()` and `source: c.source.clone()` at `:845-847`) — ~250 bytes/piece, so 10⁷ pieces ≈ 2.5 GiB and 3×10⁷ allocations inside a `Timeline` that `apply` already **cloned** (`timeline.rs:422`), then a 10⁷-element `sort_by_key`. There is no `try_reserve` anywhere in the value model (the host is careful about this in `stretch` and `Pool::write_source`), so it ends in an allocation abort.
- **Evidence**
  ```rust
  // timeline.rs:795-813
                      if *times == 0 {
                          return Err("chop times must be >= 1".into());
                      }
                  ...
                  let times_f = *times as Frame;
                  if times_f > c.src_len {
                      return Err(format!(
                          "chop {} times exceeds src_len {}",
                          times_f, c.src_len
                      ));
                  }
  // timeline.rs:829-833 — the per-piece quadratic scan
                  for i in 0..times_f {
                      let pid = format!("{prefix}.{i}");
                      if self.clip_id_exists(&pid) || !seen.insert(pid.clone()) {
                          return Err(format!("chop derived id '{pid}' already exists or repeats"));
                      }
  ```
- **Fix** — refuse `times` above a named bound (a chop into more than a few thousand slices is a typo, and the op's own grain is one slice per bar at most): `if times_f > MAX_CHOP_SLICES { return Err(...) }` with something like 4096. Make the duplicate check linear: build one `HashSet` of the track's existing ids before the loop and test membership (which also retires the dead `seen` set — see NIT 11).
- **Verification** — read `apply`/`apply_mut`/`ChopClip` in full and the host's `chop` arm; `chop_refuses_bad_inputs_and_a_looped_clip` only tests `times = 0` and `times = src_len + 1`.

7. **A reader thread and a 512 KiB ring are mounted and warmed for *every* clip on *every* track, on every edit — including value-only edits**

- **Severity** — MAJOR
- **Location** — `crates/media/src/arranger.rs:124-182` (the unconditional loop), `crates/media/src/stream.rs:115` + `crates/media/src/ring.rs:50-53` (the cost), driven by `crates/host/src/lib.rs:846` and `:1857`
- **Trigger** — any arrangement with N clips. The host's own seek test uses 120 clips on one track; a 30-minute jam is ~900. Every `Arrange` command sets `arrange_dirty = true`, and `render()` → `wire_arranger()` rebuilds every track's node from scratch.
- **Wrong behaviour** — per clip: one `std::thread` plus `Spsc::new(1<<16)`, whose slots are `Box<[UnsafeCell<Option<f32>>]>`. `Option<f32>` has no niche, so it is 8 bytes/slot — **512 KiB per clip**, not the 256 KiB the comment at `stream.rs:25-26` implies. At 900 clips that is 900 threads and ~450 MiB of ring buffers, plus `warm()` reading 65 536 frames (256 KiB) per clip ≈ 230 MiB of disk reads, **allocated and freed on every single edit** — including `RenameClip` and `SetMarker`, which touch no audio at all. `warm` also polls with a 2 ms sleep up to 10 s **per clip** (`arranger.rs:59-69`), so the worst-case control-side stall is 10 s × clips before the build fails.
- **Evidence**
  ```rust
  // arranger.rs:124 and 171-173 — no windowing, no laziness
          for c in &track.clips {
          // …
              let player =
                  FilePlayer::start_looped_anchored(clip_ref, c.loop_len, ring_capacity, off0)?;
              warm(&player, c.src_len.saturating_sub(off0), ring_capacity)?;
  // stream.rs:115 — one ring per player, at the full 64 Ki
          let ring = Arc::new(Spsc::new(ring_capacity.max(1)));
  // ring.rs:50-53
          pub fn new(capacity: usize) -> Self {
              assert!(
                  capacity.is_power_of_two() && capacity > 0,
                  "Spsc capacity must be a power of two"
              );
  ```
  `docs/capabilities.md:114` claims "30-minute, 4-track backward seek, warm-up path | **0.10 s**" — measured on the 120-clip, 1-track, 2-minute fixture at `crates/host/src/lib.rs:6019`, not on a 30-minute 4-track arrangement. The cost is linear in total clips, so the documented figure does not scale to the arrangement size the product is for.
- **Fix** — window the mount: a clip whose span ends at or before `from_frame` needs no reader (its span is over), so skip it; and make the `warm` target the *playable* remainder rather than the ring capacity. Skip `wire_arranger` entirely for ops that cannot change audio (`RenameClip`, `SetMarker`, `RemoveMarker`) by classifying ops rather than blanket-dirtying.
- **Verification** — read `wire_arranger` end to end, `ArrangerNode::new`, `FilePlayer::start_looped_anchored`, `Spsc::new`, and `warm`. The claim that this is control-side is **correct** — no cpal callback reaches `wire_arranger`; both shells drive `HostSession` from a pull loop — so this is resource exhaustion, not a realtime-invariant breach.

### MINOR

8. **`fade_out` never reaches zero inside the clip, so a one-frame micro-fade is a no-op**

- **Severity** — MINOR
- **Location** — `crates/media/src/arranger.rs:98-101`; mirrored in the envelope at `spikes/tui-shell/src/timeline.rs:945-948`
- **Trigger** — any `fade_out = N`; most audible at `N = 1`, which `SetClipFade` accepts (`1 + 0 ≤ src_len`).
- **Wrong behaviour** — the numerator is `src_len - off`, so at the clip's **last** sample (`off = src_len - 1`) the gain is `1/N`, not `0`. The ramp only reaches zero at the sample *after* the clip. Consequences: a 1-frame declick fade-out (the classic micro-fade) multiplies by `1.0` and does nothing; two butt-joined clips with `fade_out = N` / `fade_in = N` step by `1/N` (a full-scale step at `N = 1`) instead of meeting. `fade_in` is asymmetric: it *does* start at 0, so the two ramps are one frame out of phase with each other.
- **Evidence**
  ```rust
  // arranger.rs:98-101
      let gout = if c.fade_out > 0 {
          let remaining = c.src_len.saturating_sub(off);
          (remaining as f32 / c.fade_out as f32).min(1.0)
      } else {
          1.0
      };
  ```
- **Fix** — count the fade from the frame *after* the last one: `let remaining = c.src_len.saturating_sub(off + 1);` (equivalently `(src_len - off - 1) as f32 / fade_out as f32`), which makes the last emitted sample exactly 0. Move the TUI's `tail` with it, or the drawn envelope and the audio will disagree at the tail.
- **Verification** — read `fade_gain` and its caller; the only test, `per_clip_fade_ramps` (`arranger.rs:673-714`), asserts *relative* magnitudes (`out[0] < out[10] < out[100]`, `out[999] < out[950]`) and never the terminal gain, so it passes with either convention. The TUI's `Action::Fade` path and `spikes/tui-shell/src/main.rs:2270-2300` confirm short fades are a first-class, reachable state.

9. **`Stretch::for_len` accepts `num = 0` or `den = 0` although it returns `Result` — the two constructors disagree about the same argument**

- **Severity** — MINOR
- **Location** — `crates/media/src/stretch.rs:117-133` vs `:117-122`
- **Trigger** — a direct `Stretch::for_len(0, 1, n)` or `Stretch::for_len(1, 0, n)`. The host happens to guard it first (`crates/host/src/lib.rs:1026-1028`), so this is a library-contract hole, not a live crash.
- **Wrong behaviour** — `for_len` routes straight to `with_hops`, skipping `new`'s check, so its `Err` arm is unreachable. `ratio = 0.0` → `hop_in = ((hop_out / 0.0).round() as usize).max(1).min(window.len())` saturates to `window.len()` and `target_len()` returns `hop_out`, i.e. a silently degenerate transform. `ratio = inf` (from `den = 0`) → `hop_in = 1` and `target_len()`'s `round(fed * inf) as usize` saturates to `usize::MAX`, so the flush loop walks one block per input frame and `acc` grows to `fed × hop_out` — hundreds of MB for a modest region, with no refusal.
- **Evidence**
  ```rust
  // stretch.rs:117-122 — new() checks
      pub fn new(num: u32, den: u32) -> Result<Self, String> {
          if num == 0 || den == 0 {
              return Err(format!("stretch ratio must be non-zero (got {num}/{den})"));
          }
          Ok(Self::with_hops(num, den, Self::DEFAULT_HOP))
      }
  // stretch.rs:127-133 — for_len() does not
      pub fn for_len(num: u32, den: u32, len: u64) -> Result<Self, String> {
          let hop = ((len / 4).clamp(4, Self::DEFAULT_HOP as u64)) as usize;
          Ok(Self::with_hops(num, den, hop))
      }
  ```
- **Fix** — move the `num == 0 || den == 0` check into `with_hops` (or have `for_len` call `new` and then rebuild the hops), so both constructors share one rule. Also consider refusing a `ratio` outside a sane band, the way the host already does with `MAX_STRETCH_RATIO`.
- **Verification** — read `with_hops`, `run`, `target_len`, `hand_out` and the host's `stretch`; `guards_and_edge_inputs` (`stretch.rs:598-611`) tests only `Stretch::new(0, 1)` and `new(1, 0)`, never `for_len`.

10. **`list()` still yields an unaddressable id for a non-UTF8 stem, and `conform` then writes dot-prefixed phantom sources into the pool** *(the previous review's minor #49, still open)*

- **Severity** — MINOR
- **Location** — `crates/media/src/pool.rs:217-221` and `:713`
- **Trigger** — a pool directory containing a WAV whose *name* is not valid UTF-8 but whose header parses (a camera/radio dump, a mangled copy). `: pool <dir>` → `HostSession::set_pool` → `Pool::conform` (`crates/host/src/lib.rs:744`) picks it up because `sample_rate != session_rate` or `channels > 1`.
- **Wrong behaviour** — `list()` assigns `id: ""` with **no** entry in `PoolIndex::errors` (the previous review asked for exactly that and it was not done), so the failure is silent. `conform` then calls `expand_one(&src.wav, rate, "")`, whose sibling naming at `:713` is `path.with_file_name(format!("{id}.ch{ch}.wav"))` → the literal file `.ch1.wav`. That name *passes* `valid_id` (a leading dot is a plain stem), so the pool **gains a new, addressable, log-spellable source named `.ch1`**, and the `Conform` report carries `id: ""` with `extracted: [".ch1"]`.
- **Evidence**
  ```rust
  // pool.rs:217-221
              let id = wav
                  .file_name()
                  .and_then(|n| n.to_str())
                  .map(|n| n.strip_suffix(".wav").unwrap_or(n).to_string())
                  .unwrap_or_default();
  // pool.rs:713
                  let sibling = path.with_file_name(format!("{id}.ch{ch}.wav"));
  ```
- **Fix** — as the previous review specified: in `list()`, `None => { index.errors.push((wav.clone(), "non-UTF8 file name is not a usable pool id".into())); continue; }`. A whitespace-bearing stem gets the same treatment (or at least a flag), because `import` sanitizes such stems (`sanitize_stem`, `:168-172`) while `list` does not — so a hand-placed `My Take.wav` is listed under an id `path_for` will forever refuse.
- **Verification** — read `list`, `conform`, `expand_one`, `valid_id` and `sanitize_stem`; the pool tests cover the space-in-a-stem *import* path (`import_sanitizes_a_stem_the_log_could_not_name`) but there is no fixture with a non-UTF8 name, which is why it is still open.

### NIT

11. **`ChopClip`'s `seen` set can never fire** — `crates/media/src/timeline.rs:828-833`. `pid = format!("{prefix}.{i}")` with `i` unique over `0..times_f`, so `seen.insert` always returns `true` and the `!seen.insert(..)` arm is dead. It costs a `String` clone + hash per piece and hides the fact that the *real* check on the same line, `clip_id_exists`, is a full O(tracks × clips) scan. Delete `seen` and hoist the id set (see finding 6).

12. **`ArrangerNode::new` takes `ring_capacity` unchecked, so a non-power-of-two panics instead of erroring** — `crates/media/src/ring.rs:50-53` asserts `capacity.is_power_of_two() && capacity > 0`, reached from `crates/media/src/stream.rs:115` (`Spsc::new(ring_capacity.max(1))`). `ArrangerNode::new`'s signature (`arranger.rs:116-122`) is public and its `ring_capacity` argument is never validated, so a host passing `1000` (or `0` meaning "auto") gets a panic rather than an `Err`. Round the capacity up to the next power of two inside `Spsc::new`, or validate in `ArrangerNode::new`.

13. **`decode_op` truncates the `MoveTrack` index with `as usize`** — `crates/media/src/clip_editor.rs:344`: `index: u64_field(fields, "index")? as usize`. On a 32-bit target a hand-edited log carrying `index = 2³²+3` silently becomes `3` and `MoveTrack` accepts it. `u32_field` (`clip_editor.rs:310-318`) already does the checked `u32::try_from` for exactly this reason; the index should use it (or `usize::try_from`). Harmless on the 64-bit targets built today, hence NIT.

14. **`ArrangerNode::new` stores `track.clips` unsorted** — `crates/media/src/arranger.rs:183-187`. The render loop's `break` at `:237-239` relies on the `Timeline`'s sort-by-`at_frame` invariant, which `validate_clip` does not establish. A hand-built `Track` (which `new` explicitly invites, having just validated it) that is out of order silently drops clips from the render. Sorting in `new` is one line, or add a `debug_assert!(is_sorted_by_key)` so the contract is at least checked.

### Test gaps

- **Fade length vs. clip length after `RazorSplit` / `ChopClip`** — every split/chop test in `timeline.rs` and `clip_editor_engine.rs` uses zero fades, or fades far shorter than the resulting halves. This is the whole of CRITICAL 2.
- **Fades near `u64::MAX`** — the only test that uses one (`crates/host/src/lib.rs:4106-4112`) asserts the *debug panic*, so the release wrap (CRITICAL 1) is untested by construction. Needs a non-`cfg(debug_assertions)` case asserting `Err`.
- **Reverse split → mono import ordering** (MAJOR 5) — `tests/pool.rs` covers 4ch→stereo and mono→stereo; stereo→mono is the untested direction.
- **Injected commit failure in `Pool::import`** (MAJOR 4) — no test can make `fs::rename` fail, so the delete-before-commit window is untested. A test could exercise the recovery path directly instead.
- **`Spsc` capacity and `Resampler` phase at the final output sample** — the resampler's `ready()`/`trim()` bounds are right by inspection and the 7-frame-chunk test, but nothing pins the *last* output sample against a hand-computed expectation, nor `trim()`'s `base` bookkeeping across a 1-frame-at-a-time feed.
- **`Stretch::for_len` with a zero ratio** (MINOR 9) and **`Stretch` streaming with a 1-frame feed** — `guards_and_edge_inputs` covers `Stretch::new`, not `for_len`, and `streaming_in_chunks_matches_one_shot` uses 37-frame chunks, never 1.
- **A `ChopClip` with `times` in the thousands** — would pin both the quadratic scan and the fade-invariant hole in one test.

### Design risks

- **`drift.rs` is no longer dead code, and the previous review's major #5 is closed on both halves — with two residuals.** `crates/media/src/capture.rs:113-114` builds one `DriftCompensator` per channel and the demux drives `push_input`/`pull_output` per batch, with identical input lengths and identical `f64` op order per channel (`capture.rs:106-112`) so channels cannot diverge — that part is sound. For the specific question asked: a 44.1 kHz source in a 48 kHz session is now handled twice over — `Pool::import`/`Pool::conform` convert once at the boundary and `HostSession::set_pool` calls `conform` (`crates/host/src/lib.rs:744`), and `ArrangerNode::new` **refuses** a mismatch outright (`arranger.rs:141-146`, pinned by `rate_mismatched_source_is_refused`). *Residual 1*: the ratio is **handed in**, not measured (`drift.rs:9-10` says so), so a device whose true clock differs from its declared rate still drifts — the compensator reconciles a *declared* rate, not an observed one. *Residual 2*: `DriftCompensator::new` validates neither argument, so `in_rate == 0` yields `ratio = 0.0` (every output frame repeats `pending[0]`) and `out_rate == 0` yields `inf` (zero output frames) — a silent, empty take rather than an `Err`. Both are one `if in_rate == 0 || out_rate == 0` away from fixed.
- **The value model clones the whole arrangement on every op.** `Timeline::apply` (`:421-425`) clones every track, clip and `String`, then `ClipEditor::apply` moves the result in, then `wire_arranger` snapshots it *again* (`crates/host/src/lib.rs:1773`), and `host.arrangement()` snapshots again for the shell. At ~900 clips that is thousands of allocations per keystroke-level edit, on top of finding 7. The correctness argument for the clone (never partially apply) is sound; the cost is not, and it is the same axis as finding 7.
- **One reader thread + one ring per clip is a scalability ceiling, not a tuning knob** (finding 7). The stated product is "record long live jams" — the number of readers grows with the arrangement, and so does the memory, on every edit, with no windowing in sight. This is the shape that needs deciding before the first real 30-minute arrangement, not after.
- **The `Interner` leaks by design** (`clip_editor.rs:37-44`, "spike scale — a serialized log would use a string table instead"). Fine at edit scale; it means a long-lived session's memory is monotonically non-decreasing, and it is only reached from the live encode path, never from replay.
- **`ArrangerNode`'s module doc is stale on two points**: "one reader per active clip *instance* (a source read at two `src_start` offsets is two readers)" (`:8-9`) — the code keys one reader per *clip id* (`readers: HashMap<Id, ClipReader>`, `:55`), and a clip has exactly one `src_start`; and the determinism scope note (`:16-24`) predates the rate-mismatch refusal and the `from_frame` anchoring, both of which narrow it further.
- **`Stretch`'s `acc` is the whole render, held in memory** (`:93-97`, honestly documented). Bounded today by `MAX_STRETCH_FRAMES` in the host; if the *sculptor* profile ever streams a stretch, this is the field that has to become a ring.

### Checked and clean

- **`resample.rs`** — read in full. The kernel maths is right: `fc = 0.5 · min(out/in, 1)` is exactly the output Nyquist expressed in input-sample cycles for every direction tested (44.1→48 anti-images at 22.05 kHz, 96→48 anti-aliases at 24 kHz, 48→44.1 anti-aliases at 22.05 kHz); the tap centring (`x = f − tap + h − 1`, `first = floor(pos) − 63`) matches between `Kernel::new` and `sample`, and each row is normalised to unity DC so the `f = 1` blend also sums to 1. `ready()`'s bound (`floor(pos) + 64 < fed`) is exactly the maximum index `sample` touches (`first + 127 = floor(pos) + 64`), so the `break` at `:254` is genuinely unreachable and `index < base` can only fire at t = 0. `trim()`'s `keep_from` is computed from the *already-advanced* `pos`, so it never drops a frame the next sample needs, and `hist.len() == fed − base` holds inductively (so the `hist` index at `:257` is in range by construction). `hist` stays bounded by the feed block + 64 + step. Block-size independence, the 44.1→48 exact length, the empty-input case, zero rates, DC preservation, 20 kHz flatness, 96→48 alias rejection, and the passthrough bit-exactness are all pinned by real assertions. No ratio-change API exists, so "ratio changing mid-stream" cannot happen; a new `Resampler` would restart the filter at t = 0, which is inherent.
- **`dither.rs`** — read in full. `SplitMix64` is seeded from a fixed constant and the state is per-instance, so two exports of the same session are byte-identical (invariant 2 holds for the dither path). `quantize_s16`'s pre-clamp to `±32766` is what makes the "no sample is ever clamped" claim true, and the arithmetic is overflow-proof for any finite input: `x * 32767.0` can reach `inf` for a huge finite `x`, but `f32::clamp` folds `inf` to the rail. Non-finite input is zeroed before scaling, and the writer's re-multiply by 32767 recovers `q` exactly (the grid test holds it to 1e-2, which is a quarter of an LSB). One instance per export over the interleaved buffer means L and R get different dither. TPDF mean/variance, full-scale behaviour and the bias test all hold.
- **`stretch.rs`** — read in full apart from MINOR 9. `hop_in` capped at one window is the right guard against gaps under compression, and the cap is what makes the tail-hold (`read = nominal.min(fed − window)`) coherent. `best_offset`'s `at + j` reads stay inside `nominal + search + window` because the gate at `:259` demands `fed ≥ nominal + search + window` and `overlap = min(hop_out, corr) ≤ window`. `acc.get(base + j)` is always in range after the `resize`, and `produced` is bounded by `acc.len()` in both assignment sites, so `hand_out`'s slice at `:324` cannot panic. `trim()` keeps from `nominal − search − 1`, which is strictly below the next block's minimum read (`nominal + hop_in − search` with `hop_in ≥ 1`). `target_len`'s `fed >= window` branch is what stops a short region from trailing into zero padding, and the 0-length input correctly yields 0 frames rather than the `max(1)` it computes. Streaming-equals-one-shot and byte-reproducibility are pinned.
- **`arranger.rs`** — the two-phase discipline holds: `ArrangerNode::new` validates every clip and resolves the source *before* touching `self`, and the reader set is committed only after all of them succeed, so a failed build leaves nothing half-wired. The render path allocates nothing (`acc` is a fixed `[f32; BLOCK]` stack array), takes no lock, and issues no syscall; `pop_sample` correctly distinguishes a legitimate end (EOF or `popped >= expected`) from a starvation, and the `debug_assert_eq!(popped + off0, off)` alignment invariant is guarded by the same `at_end` predicate so it cannot fire at a real end-of-clip. `j0`/`j1` are provably within `0..=out.len()` from the two `continue`/`break` guards above them, so `acc[j0..j1]` cannot go out of bounds. The `off0` clamp is measured from `src_start` and deliberately excluded for looped clips, and the `reversed` field is a clip property the loop/trim/split/chop ops all mirror. `warm`'s 10 s deadline is a control-side bound that fails loudly. The rate-mismatch refusal (previous major #5, first half) is real and tested.
- **`pool.rs`** — the staging discipline is right where it matters: every channel is written to a `.converting` sidecar first, so an interrupted split never leaves a torn `.wav` where a source is expected, and peaks are derived after the wav (a crash in that window leaves a source `list` reports as `peaks_missing` and `recover` heals). `write_channel` streams the resampler in 16 KiB blocks and its reported `frames_out` is the writer's own count, so `Conform.frames_out` is a measurement rather than an estimate — and because `Resampler::flush` tops up to `round(fed · ratio)`, the total is exact for a 44.1→48 import (pinned by `import_converts_a_foreign_rate_once`). The equal-rate path is a byte-exact `fs::copy`, and the `.pre{rate}` / `.pre{N}ch` backups are never `.wav`, so `list` cannot index them. `free_backup` numbers rather than clobbers, `expand_one` commits the primary last (so every read happened against the original), `recover` finalizes before rebuilding peaks, and `rebuild_peaks` is shared by import, conform, recover and `write_source`. `valid_id` correctly refuses empty, `.`, `..`, both separators and any whitespace, and `sanitize_stem` mangles only whitespace, which is the right asymmetry.
- **`timeline.rs`** — the invariants that *are* enforced are enforced consistently and fail-loud: `src_len > 0`, `src_len ≤ i64::MAX`, finite gain, `loop_len != Some(0)`, checked `at_frame + src_len`, and the log-spellability of track ids / clip names / marker names (`valid_name` refusing a leading `@`, a leading `snap=`, and `#`). `add_signed` uses `i128` so signed trim arithmetic cannot wrap, `MoveTrack` range-checks its index, `RazorSplit` refuses a looped clip and requires the cut strictly inside the span, `ChopClip` distributes the remainder so the pieces tile the original span exactly (pinned), `MoveClipToTrack` re-sorts both tracks, `SetMarker`'s "set = add or rename" removes the duplicate failure mode, `RemoveMarker` refuses a no-op delete, and the reversed-clip mirroring of split/trim/chop is pinned by exact `(src_start, src_len)` assertions. `markers_from_json` normalises on deserialize, so `marker_at`'s binary search cannot be broken by a hand-written snapshot. `end_frame` is clip-derived, so a marker past the last clip does not make an export render silence.
- **`clip_editor.rs`** — `encode_op`/`decode_op` are exact inverses for all 20 ops (pinned field-by-field, including `loop_len`'s `0`-means-none, the `reversed` flag, and the `name` `""`-means-none convention), every op name is in `ALL_OPS`, the type decoders are strict (`v_u64` refuses an `I64`/`U32` where a `U64` is required rather than coercing), `u32_field` does the checked narrowing, an unknown extra field is a documented forward-compat drop, `snapshot()` maps poison to `Err` (the previous review's inconsistent-`expect` minor is genuinely fixed), and `ClipEditor::apply` applies-and-validates **before** calling `arrange_logged`, so a refused op never reaches the log (invariant 3). `arrange_logged` does not schedule, so the eager apply and the replay-time handler apply cannot double-apply an op.
- **`drift.rs`** — `pull_output`'s `consumed = (pos.floor() as usize).min(pending.len())` clamp is what keeps the `drain(..floor(pos))` from panicking when a fractional `pos` carries into a short final batch, the tail hold (`b = a` at the last pending sample) consumes every delivered sample, and the pitch-preservation and both-ratio-directions tests are real zero-crossing counts, not "did it not crash". `pending` stays bounded in both directions, which is the property that matters over a 20-minute take. The one gap is the unvalidated constructor (see Design risks).
- **`tests/clip_editor_engine.rs`, `tests/pool.rs`** — read both in full. They pin what they claim: replay reconstructs the identical `Timeline` value, a refused op leaves zero `Arrangement` events *and* an empty timeline, and the pool suite covers the crash-recovery finalize, the missing-peaks rebuild, the space-in-a-stem sanitization, stereo split at the boundary, the three import-replacement orderings it does test, the torn-sibling re-derivation, backup non-clobbering, self-import, rate conversion with a pitch assertion, the bit-exact copy, `write_source`'s replace-and-id-guards, hand-filled conform, and idempotence.

---

## 2. Adversarial verification (verbatim)

I'll verify each claim by reading the actual code.

Now let me check the docs claim (capabilities.md:114) and the seek test with 120 clips, plus the shell's rename/marker ops.

Let me verify several remaining details.

Let me verify a few remaining specifics.

Now let me verify remaining specifics.

Let me verify several remaining specifics.

Let me do a final check on the audio-callback path for claim 7.

I've read every function, caller, and test named by the claims. Verdicts:

---

`PARTLY 3-media-dsp#1` — the overflow is real and the trigger is constructible, but one stated consequence is false.

The code is exactly as quoted. `Frame = u64` (`timeline.rs:30`), and both checks are plain `+`:
- `crates/media/src/timeline.rs:349` — `if c.fade_in + c.fade_out > c.src_len`
- `crates/media/src/timeline.rs:759` — `if fade_in + fade_out > self.tracks[ti].clips[ci].src_len`

Trigger I used: `arrange set_clip_fade t0 c0 18446744073709551615 1`. The parser at `crates/host/src/lib.rs:3905-3906` uses `u(3)?`/`u(4)?`, which is a bare `str::parse::<u64>` (`:3680-3688`) with no bound. `set_clip_fade` is absent from the `frame_operand` match (`:3658-3664`), so `snap=` is *refused outright* on this op (`:3665-3669`) rather than able to clamp — the reviewer's point holds, though snap would quantize, not clamp, so it would not have helped anyway.

Debug: `ClipEditor::apply` holds the guard across the apply — `{ let mut tl = self.timeline.lock()...?; *tl = tl.apply(op)?; }` (`crates/media/src/clip_editor.rs:551-556`) — so the overflow check at `timeline.rs:759` fires under the lock and poisons it. `Cargo.toml` `[profile.release]` sets only `lto = true`, so `overflow-checks` is off in release.

Release: `u64::MAX + 1` wraps to `0`; both the op check and the later `validate_clip` at `arranger.rs:128` pass, the node builds, and `fade_gain` (`arranger.rs:92-105`) computes `(off as f32 / 1.8446744e19).min(1.0) = 0.0` for every sample. Silent. Confirmed.

**What is wrong:** *"Any later `MoveClip`/`Trim`/`Stretch` on it re-validates and is refused, so the clip cannot even be nudged."* In release it is **accepted**, not refused — `MoveClip` calls `validate_clip` at `timeline.rs:685`, which performs the *same* wrapping add, so `0 > src_len` is false and the move succeeds. The clip is permanently movable-but-silent, not stuck. The core fix (`checked_add` at both sites) is correct regardless.

---

`CONFIRMED 3-media-dsp#2` — mechanism, trigger, and consequence all hold.

- `RazorSplit` (`timeline.rs:585-593`) zeroes `left.fade_out` and `right.fade_in` but never touches `left.fade_in` or `right.fade_out`, and never calls `validate_clip`. The right half is *also* reachable: splitting a clip with a large `fade_out` leaves `right.src_len = c.src_len - split_in < c.fade_out`.
- `ChopClip` (`timeline.rs:850-851`) puts `c.fade_in` on piece 0 of length `src_len / times` and `c.fade_out` on the last piece of the same length.
- `validate_clip` has exactly six call sites — `timeline.rs:542, 647, 669, 685, 704, 899` — and is in neither arm. `LoopRegion` only grows `src_len`; `Duplicate` clones a valid clip. Correct.

Trigger: clip `c0` at frame 0, `src_len = 48000`, `fade_in = 47999`, `fade_out = 0` (legal: 47999 ≤ 48000), then `razor_split t0 c0 L R 12000` → `left.src_len = 12000`, `left.fade_in = 47999` → `47999 + 0 > 12000`. The fade is one `f` press: `fade_to_playhead` (`spikes/tui-shell/src/main.rs:2264-2300`) computes `wanted = playhead - at_frame`, capped at `src_len - fade_out`, so the playhead at the clip's end gives a full-length fade-in; the shell's own test at `main.rs:5376-5385` drives exactly that and asserts `fade_in == 6000 == src_len`.

Consequence: `ArrangerNode::new` → `validate_clip` (`arranger.rs:128`) → `Err`; `wire_arranger` propagates at `crates/host/src/lib.rs:1811`; `render` propagates at `:1857`. The live path handles it honestly (`crates/host/src/live.rs:444-452` stops the transport and surfaces `last_error`).

Test coverage is genuinely absent, as claimed: the `clip()` helper at `timeline.rs:911-925` builds `fade_in: 0, fade_out: 0`, and `clip_editor_engine.rs:54-59` sets 64/128 on a 2000-frame half.

Two imprecisions, neither load-bearing: it takes three keypresses (`f` at the end, move the playhead back, `x` — the split frame must be strictly inside, `timeline.rs:577`), not two; and the chop example needs *both* fades at 50 to make 2 of 10 pieces invalid, not `fade_in = 50` alone.

---

`CONFIRMED 3-media-dsp#3` — the guard exists and is genuinely unreachable from production.

`crates/host/src/lib.rs:746-749` is the only production `PoolResolver` construction (the others are in `arranger.rs` tests at `:305, :432, :501`):

```rust
let resolver: media::PoolResolver = std::sync::Arc::new(move |id| {
    let p = resolver_dir.join(format!("{id}.wav"));
    p.is_file().then_some(p)
});
```

`valid_id` is private (`crates/media/src/pool.rs:153`, no `pub`) and rejects `/`, `\`, `..`, and whitespace. `Pool::path_for` (`:185-191`) is the only `pub` guarded lookup, and grep confirms it is referenced solely from `crates/media/tests/pool.rs:114,117,140,145` and one host **test** at `lib.rs:6486` — no production caller.

Trigger: `arrange add_clip t0 c0 ../../../../tmp/other 0 100 0 0 0 1.0`. `source: s(3)?` (`lib.rs:3816`) is an arbitrary whitespace-free token, and `validate_clip` (`timeline.rs:324-353`) inspects only `src_len`, `gain`, `loop_len`, `at_frame + src_len`, `name`, and the fades — never `source`. `Path::join` concatenates, and the OS resolves the `..` at open time, so `is_file()` follows it out of the pool and `WavReader::open` (`arranger.rs:131`) reads the file. `arranger.rs:129-132` cannot validate because it holds only a `PoolResolver` closure.

One caveat on framing, not on the code: the "untrusted session file" threat model is thin for a single-user local desktop app whose user chooses which file to open. The structural defect — a hardening that exists but is dead on the production path — is real and worth fixing.

---

`PARTLY 3-media-dsp#4` — the delete-before-commit contradiction and the `?`-after-rename are real; the stated trigger is not one I can construct, and the "load-bearing on Windows" citation points at the wrong function.

Confirmed in code:
- `pool.rs:486` `self.replace_sources(&id, 1);` runs **before** `fs::rename(&tmp, &dest)` at `:487` — directly contradicting its own doc at `:573-575` ("Called only once the new material is safely written, so a failed import changes nothing"). Same shape at `:423` before the multi-channel rename loop at `:426`.
- `Self::rebuild_peaks(ch_dest, ...).map_err(...)?` at `:440-441` does propagate after the renames. If it fails on `ch0`, then `ch1..` are never renamed, and since `replace_sources(&id, channels)` deliberately *kept* `ch0..ch{channels-1}` (`k >= keep_channels` is false for them), the pool is left holding the **new** `ch0` beside the **old** `ch1`. That is the mixed state `:401-404` promises cannot happen. The mono arm at `:495-496` is milder (wav present, sidecar missing, which `list`/`recover` handle).
- `expand_one` really is the contrast: it hard-links or copies the original to a backup at `:744-745` *before* renaming, so the old bytes survive.

Where the claim overreaches:
- **Trigger.** "A read-only directory" fails earlier — the staging write at `stage_channel`/`fs::copy` (`pool.rs:556`, `:480`) would fail first, so `replace_sources` is never reached. A rename failure at commit needs ENOSPC/EIO on a directory entry in a directory that just accepted a write. That is a real durability/crash window, not something I can construct deterministically, and I will not call it CONFIRMED as a demonstrated fault.
- **The Windows parenthetical.** The delete-first order is *not* load-bearing for the reason given. The comment at `:305-308` is in `write_source`, a different function. In `import` the delete exists so the *mono* dest `{id}.wav` is absent for `fs::rename`, which Windows refuses when occupied. (And as noted under New findings, the delete is incomplete for the multi-channel case.)
- The claim's own "no test injects a rename failure" is accurate — there is no fault injection anywhere in `crates/media/tests/pool.rs`.

The suggested fix direction (preserve-then-sweep, or rename-to-`.bak` and roll back) is the right shape.

---

`CONFIRMED 3-media-dsp#5` — the predicate leaves exactly the hole described.

`pool.rs:588-596`, with `keep_channels = 1` from the mono call site at `:486`:

```rust
let stale = match stem.strip_prefix(id) {
    Some("") => true,
    Some(rest) => rest.strip_prefix(".ch")
        .and_then(|k| k.parse::<u16>().ok())
        .is_some_and(|k| k >= keep_channels),
    None => false,
};
```

For `{id}.ch0`: `0 >= 1` is false → **kept**. For `{id}.ch1`: `1 >= 1` → removed. So a mono import over a previously-split id leaves the prior stereo's left channel addressable under an id the pool itself handed out via `Import::ids()` (`pool.rs:117-119`, returns `["S.ch0", "S.ch1"]` — asserted at `crates/media/tests/pool.rs:237`).

Trigger is constructible: import a stereo `S.wav` from dir A, then a *different* mono `S.wav` from dir B. It must not be the same file, or `same_file` (`pool.rs:382-390`) short-circuits into `expand_one`, which takes a different path.

Coverage is exactly as claimed. Reading the full test list in `crates/media/tests/pool.rs`: `a_narrower_import_removes_the_older_channels` is 4ch→2ch (`keep_channels = 2`, `ch0` overwritten by the rename), `a_stereo_import_replaces_the_mono_source_holding_that_id` is mono→2ch, `importing_a_pool_source_over_itself_reports_it` is the same-file path. No test does stereo→mono. The suggested fix — pass `keep_channels = 0` on the mono path, or special-case `keep_channels == 1` — is correct.

---

`CONFIRMED 3-media-dsp#6` — no bound exists and the trigger is constructible; two parentheticals are wrong.

Guards at `timeline.rs:795-813` are `times != 0` and `times_f > c.src_len`. `times` is a `u32` off the wire with no cap (`crates/host/src/lib.rs:3924`), and `validate_clip` permits `src_len` up to `i64::MAX` (`timeline.rs:328`). So:

1. `arrange add_clip t0 c0 s1 0 4294967295 0 0 0 1.0` — legal.
2. `arrange chop t0 c0 4294967295 pre` — `4294967295 <= 4294967295`, so the only guard passes.

The loop at `timeline.rs:829-833` then runs 4.3×10⁹ iterations, each calling `self.clip_id_exists(&pid)` — a full scan of every track and every clip (`timeline.rs:413-417`) — plus `format!` and `seen.insert(pid.clone())`, i.e. two fresh `String` allocations per iteration on top of the scan. Quadratic in clips, and memory grows concurrently.

The project bounds its other wire-driven allocations (`MAX_BOUNCE_BYTES` at `host/lib.rs:1849`, `MAX_STRETCH_FRAMES` at `:1100`, `try_reserve_exact` at `:1071` and `:1108`) and `word()` (`:3974`) is explicitly hardened "never an index-out-of-bounds panic — the wire schema's public parser must not panic." An unbounded `times` is inconsistent with that standard. Note the TUI has no `chop` keybinding (grep for "chop" in `spikes/tui-shell/src/main.rs` returns nothing), so reachability is via script/session file only — which is still a public wire schema by the project's own framing.

Two errors: the allocations happen *during* the loop, not after it completes (so "hangs long before anything is allocated" is wrong), and `Pool::write_source` has no `try_reserve` — the only two in the repo are both in `stretch` (`host/lib.rs:1071, 1108`).

---

`PARTLY 3-media-dsp#7` — the mechanism is real and correctly identified as control-side; the stall arithmetic and the docs provenance are both wrong.

Confirmed:
- `arranger.rs:124` loops `for c in &track.clips` with no windowing and no laziness; each clip gets `FilePlayer::start_looped_anchored` (`:171`) → one `std::thread` (`stream.rs:125`) and one `Spsc::new(1<<16)` (`stream.rs:115`, `DEFAULT_RING_CAPACITY` at `stream.rs:26`), then `warm` (`:173`).
- Slot size: `slots: Box<[UnsafeCell<Option<f32>>]>` (`ring.rs:24`), built at `ring.rs:62`. `f32` has no niche, so `Option<f32>` is 8 bytes → **65536 × 8 = 512 KiB per clip**. Correct.
- `warm`'s target is `min(cap, want)` (`arranger.rs:60`) = up to 65536 frames = 256 KiB of samples per clip.
- Every `HostCommand::Arrange` sets `arrange_dirty = true` unconditionally (`host/lib.rs:1666`), and `wire_arranger` returns early *only* if the flag is clear (`:1763-1766`). `RenameClip`, `SetMarker`, and `RemoveMarker` are all `ArrangeOp`s, so yes — a marker edit rebuilds every reader in the arrangement.
- No cpal callback reaches `wire_arranger`; the device callback only pops the output ring. The reviewer's concession is correct.

Two errors:

1. **"the worst-case control-side stall is 10 s × clips."** No. `warm` returns `Err` on the first timeout, and `arranger.rs:173` is `warm(...)?` inside the loop — the first `Err` returns from `ArrangerNode::new` and the build aborts. Worst case is **10 s, once**.

2. **The docs provenance.** `docs/capabilities.md:114` is *not* "measured on the 120-clip, 1-track, 2-minute fixture at `crates/host/src/lib.rs:6019`." That test (`a_warm_seek_equals_a_replay`) asserts byte-equality, arrangement equality, and clock equality — I read it end to end and it contains no timing assertion. The note at `.agents/notes/implemented/feature/2026-09-24-seek-at-scale-warm-up.md:48-53` documents the measurement as a separate `time host seek30.txt` on a real 30-minute, four-track script (`real 0m0.097s`). So the documented figure stands as measured, and the "does not scale" inference is unsupported speculation. If the cost is genuinely a problem, it needs its own measurement, not a re-reading of an unrelated test.

The structural point survives independently and is worth acting on: an O(total clips) rebuild with ~512 KiB and one thread per clip, on every edit including marker and rename gestures, is real overhead. The suggested windowing (skip clips whose span ended before `from_frame`) is sound.

---

### New findings

**`crates/media/src/pool.rs:407-426` — the multi-channel commit deletes nothing it is about to overwrite, so a second stereo import over the same id fails outright on Windows.**

`stage_channel` writes each channel to `{id}.ch{ch}.converting` (`:556`), then `replace_sources(&id, channels)` runs at `:423`, then the loop renames each staged file over `{id}.ch{ch}.wav` at `:426`. The `stale` predicate (`:588-596`) marks `{id}.ch{k}` stale only when `k >= keep_channels` — so with `keep_channels == channels`, the actual rename destinations `ch0..ch{channels-1}` are **deliberately kept**, not deleted.

Trigger: import `jam.wav` (stereo) into a pool, then import a *different* stereo `jam.wav`. On the second import `jam.ch0.wav` and `jam.ch1.wav` exist, `replace_sources` leaves them alone, and `fs::rename` is called onto occupied paths. POSIX replaces silently; **Windows refuses** — the exact behaviour `write_source` documents and works around at `:305-318`:

> On the Windows `rename` refuses an existing destination, and replacing is *intended* here

`write_source` has the `remove_file` + retry fallback at `:313`. `Pool::import` has no equivalent. The error path at `:427-436` then removes every staged temporary and returns `Err`, so the import fails and the pool keeps the old material — not data loss, but a stereo re-import over an existing stereo id is broken on Windows. `Pool::import` needs the same remove-then-rename fallback, or `replace_sources` must also clear `ch0..ch{keep-1}` on the multi-channel path.

I could not execute this (Linux, and the instruction forbids building), so it is code-reading plus the repo's own Windows note — the reasoning is unambiguous but the repro is unrun.

**`crates/media/src/timeline.rs:829-860` — `ChopClip`'s per-iteration `HashSet` is dead weight and its memory cost is understated.**

`seen` is `let mut seen = std::collections::HashSet::new();` (`:828`), and the guard is `if self.clip_id_exists(&pid) || !seen.insert(pid.clone())` (`:831`). Because `pid = format!("{prefix}.{i}")` is a pure function of the loop counter, `i` never repeats, so `seen.insert` always returns `true` and the `!seen.insert(...)` disjunct is never true. The set is a pure leak: it grows to `times` cloned `String`s for no benefit. This compounds claim 6's memory figure — the real per-iteration allocation count is three `String`s (the `format!`, the `seen` clone, and the `Clip`'s `id` + `name` + `source` clones at `:842-846`), not the three the reviewer counted. Replacing the whole guard with a pre-built `HashSet` of the track's existing ids both removes the dead set and linearises the duplicate check.

**`crates/host/src/lib.rs:819-837` — `execute_group`'s fold-then-apply probe inherits claim 1's wrap, so a gesture containing an overflowing `SetClipFade` passes the probe and then behaves differently on the real apply.**

The comment at `:820-824` is explicit that the probe and the real apply agree "**as long as the real apply cannot fail after the fold**", and records the precondition. That precondition is currently violated in release: the probe's `probe.apply(op)` and the real `editor.apply` both wrap `u64::MAX + 1` to `0` and both succeed, so they agree — but they agree on a value that violates the documented fade invariant, and the *group* is then logged. This is not a new bug so much as a second consumer of the same defect, and it means fixing claim 1 at `timeline.rs` fixes both. Worth noting because the group path bypasses the single-op arm at `:1657-1668` that the claim's trigger uses, so a regression test on `set_clip_fade` alone would not cover a grouped gesture.
