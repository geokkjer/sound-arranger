# FaderFrame — architecture and UI study, and what it means for an MVU core

> Research input, 2026-10-14. Companion to the shells note (iced + ratatui; Tauri
> retired), the [minimal-core note](../../.agents/notes/proposed/architecture/2026-08-15-minimal-core-clock-graph-session-log.md),
> the [Ableton UI prior-art study](2026-09-09-ableton-prior-art-ui.md), and the
> [Cordis overview](2026-08-15-cordis-overview-and-reuse-analysis.md).
> Sources: upstream [github.com/BurningTreeC/faderframe](https://github.com/BurningTreeC/faderframe)
> (MIT), cloned to `~/Projects/faderframe` at rev `1f46123`; `docs/ARCHITECTURE.md` and the
> named crates' sources read in that checkout; the app built (release, 2m20s) and run
> (`--demo --backend dummy`) on this machine the same day. Ledger rows: [PA-0016 through
> PA-0022](../prior-art/ledger.md).

## 1. What it is

A complete multitrack DAW in Rust — recording, editing, mixing, mastering — by a single
author (BurningTreeC), GTK 4 native on Wayland, Linux/Windows/macOS from one codebase, MIT.
Version 0.21.0 at the rev read. Roughly 65 crates; stock devices include EQ/dynamics/reverb,
a plugin system (CLAP/VST3/LV2 hosted, per-plugin sandboxed processes with shared-memory
audio), surround to ADM BWF and IAMF, a clip launcher with record-to-arrangement, custom canvas-rendered editors with an optional vello/wgpu GPU painter. The engineering realism is
what makes it valuable prior art: this is shipped complexity, not a design sketch.

## 2. The two worlds, and the edit that crosses them

Their control-world/realtime-world split ([docs/ARCHITECTURE.md] in the clone) is our
problem statement solved to a late stage:

```text
 COMMAND WORLD (GTK main)                      REALTIME WORLD (audio thread)
 Session = Project + History + Workspace
   │ every edit is a Command
   │ impact() → None|Params|Timeline|Graph
   ├─ build_graph ─► compile ─► Mailbox ──────► CompiledGraph (AtomicPtr swap + adoption)
   ├─ TimelineSnapshot ─────────► Mailbox ────► clip / MIDI players
   ├─ ParamTable (atomic f32) ────────────────► strips read per block
   └─ TransportCommand ─► rtrb SPSC ──────────► TransportState
 meters, position ◄── atomics ◄─────────────── MeterBank, CallbackMetrics
 retired objects ◄── garbage queue (RT never frees)
```

Details worth naming, because each answers an objection we have raised ourselves:

- **Latest-value mailboxes.** A newer graph/snapshot *replaces* an unconsumed older one
  (AtomicPtr swap); the loser is dropped on the control thread. A burst of edits can never
  overflow a queue with stale structure. Our "rebuild to the current position" replay does
  the equivalent by construction; their mailbox is the live version of it.
- **State adoption.** On graph rebuild, processors whose node key matches the old graph are
  swapped in — a filter does not lose its cutoff when routing changes.
- **Command::impact()** ([project/edit.rs:858]) — every edit *declares* its flush cost, in
  the model layer, exhaustively matched over ~200 commands. Cost is a typed property of the
  edit, greppable and unit-testable, not an accident of where the edit happened to land.

## 3. The action chain — MVU's center built into an imperative shell

```text
 GTK signal ─► ViewEvent ─► CanvasView::event(model, cx)
                                 cx.emit(Vec<Action>)      ┐ actions
                                 cx.request(HostRequest)   ┘ toolkit-only hole
        ─► Session::dispatch(Action)                        [session/lib.rs:3394]
        ─► History::apply(cmd) ─► Impact ─► sync() ─► mailbox swap
```

`Session::dispatch` is a ~100-variant enum with monolithic match towers (~3,400 lines of
dispatcher). Around the edit sit three mechanisms whose *semantics* we lack and whose
*mechanism* we should not copy as-is:

1. **Gestures** — `BeginGesture(label) / EndGesture / CancelGesture`. A fader drag opens a
   gesture, accumulated edits close as one undo step, and a cancel *undoes what already
   happened* and restores the base. Our compound atomic messages have no cancel.
2. **Undo as history walk**, not replay — `HistoryTo(steps)` jumps to any point of the
   history. Cheap to seek; the price is that on-disk truth is *serialized final state*
   while the History (the thing you actually act on) is in-memory machinery. Our log is the
   truth; their History is the UX.
3. **`apply_amending`** ([history.rs]) — "Record to Arrangement" folds played runs into the
   still-open undo step, so "I jammed and captured it" is *one* undoable action, and a drag
   during capture amends the same step instead of stacking new ones.

What iced gives us for free that they approximate by convention: the compiler-enforced loop.
Their views emit into `EventCx` — exactly `(Model, Msg) -> (Model, Cmd)` in disguise — but
GTK callbacks still mutate `Rc<AppState>` directly at the edges, and nothing stops a side
effect inside a paint path. Our `update()` is the stronger contract; their `EventCx` is the
better *view-side API* shape.

## 4. The launcher: a model/runtime split we can transcribe almost 1:1

Two layers ([project/launcher.rs] — serialized; [session/launcher.rs] — runtime):

- **Model**: `Launcher { scenes, slots: BTreeMap<SlotKey, ClipId>, quantize, follow_actions }`.
  The slot's clip is an **ordinary clip in `Project::clips`** that no track lists — every
  clip editor, quantizer and warp tool works on it unchanged; only membership differs. No
  second clip type, no second editor, ever.
- **Runtime (engine-published, unserialized)**: `LauncherState { status: Vec<TrackStatus>,
  runs, open, amend }`, `TrackStatus { playing: Option<(slot_hash, start)>, queued }`.
  *The engine owns the quantized timing*; the session reads it back through
  `launch_status()`; the UI merely displays. Every launch press, releasing/stop,
  record-arm ends and follow-action chain resolves to whether the slot plays from its own
  start, loops over its length, or queues at the next bar — derivable from the tempo map and the slot's settings.

The settings themselves are small: `LaunchMode {Trigger, Gate, Toggle, Repeat}` + `legato`,
`LaunchQuantize {None, Beat, Bar, Bars(2|4|8)}`, and `tempo: Option<BPM>` (stretch-to-tempo).
Nothing about them needs a second clip model — which is exactly why our carrier can absorb
them as canonical-value fields.

## 5. Views: the `CanvasView` contract

Toolkit-free, generic over `(M, A)`, one instance re-hostable in any dock or window
([ui-canvas/view.rs]):

- `paint(Painter, size, model, theme)` / `event(ViewEvent, size, model, cx)`.
- **Repaint policy is declarative**: `wants_frames()` ("meters/playhead — keep drawing") and
  `dense()` ("geometry changes every frame — route to the GPU painter"). The *host* decides;
  views never schedule.
- **`HostRequest` hole** — context menus, inline text entry, file choosers, screen-reader
  announce. The only thing a view cannot do itself. This hole is what makes "17 GTK-free
  view crates" a true sentence.
- **Cross-view drag** as an opaque string payload with `drag_payload / hover_payload /
  drop_payload / cancel_drag` — launcher→arranger moves without either knowing the other.
- **A11y tree** built from what is shown, stable ids; `slot: Vec<AccessNode<A>>` regenerated
  per paint.
- **`kept()` layer cache** ([ui-canvas/painter.rs]) — draw a rarely-changing layer (waveform,
  grid) under a hash key; the painter replays the cached image while only the playhead
  moved. One function; the difference between re-rasterizing 40k path segments and not.
- Modifiers carry *meaning*, not raw state: `fine()`, `toggle()` — one home per semantics.

`Painter` itself is ~15 methods (rect/round/path/image/transform/clip/inset-shadow/text)
plus `hline`/`vline` helpers that snap hairlines to whole device pixels via `scale_factor()`.

## 6. Theming: a theme is a rendering style, not a palette

- Each skin = a **`Spec`** (≈50 lines: shell colours, LCD trio, knob/fader/meter/LED tables,
  a `ConsoleLook`) + one `build(Spec)` that **derives** the full `Theme` by formula
  ([ui-canvas/themes.rs]). Derived bits are formulae, so eight skins cannot drift. Derived
  values *cross-reference*: launcher colours reuse the meter's green/yellow, the piano roll's
  step cursor reuses the mute LED.
- **Theme is a rendering style, not a colour list** — `ConsoleLook { knob_skirt, knob_ring,
  sheen, brushed, wood, screws, meter: Ladder|Bar|Edgewise|Plasma, engrave, flat }`. "Fjord"
  and "Vintage Console" are different *objects*, and "High Contrast" (no bevels/gradients) is
  a different accessibility *mode* without a special code path.
- The theme carries layout constants (strip width, row heights, clip-header height) — no view
  ever hardcodes "72px".
- Device identity colours (`DEVICE_ACCENTS`, 11 categories: Dynamics, … Sampler, Midi) are a
  module constant **outside** any skin.

## 7. Side by side against our stack

| Axis | Ours (alpha) | FaderFrame 0.21 | Verdict |
|---|---|---|---|
| State authority | append-only session **log**, undo-by-replay (`replay_to`) | in-memory `Project` + structured `History`, on-disk = serialized final state | same start ("every edit is a command on one model"); different truth. Our log stays truth. |
| Realtime boundary | value contract, timestamped SPSC, teardown protocol; **no live multicore engine yet** | mailboxes + atomics + adoption + render-ahead + PDC + per-track threads | they are years ahead; the part to study, not to regret |
| Dispatcher | `HostCommand` + `HostSession`; profile ops ride the closed core (`Event::Arrangement {op, fields}`) | `Session::dispatch(Action)` — open ~100-variant enum, ~3.4k lines | sharp trade-off: theirs is Rustic (exhaustive, refactorable), ours is closed (profile ops are data). Steal ergonomics, keep closure. |
| Composition layer (Cordis test) | `ctx.rs` service registry, `inject`, disposers, declarative rows; machinery deliberately deferred | none — views modular, actions/state/engine monolithic | our plan is strictly more Cordis-shaped; FF shows the deferred 40% is payable for a long time |
| Shell | iced MVU: one Message enum, `update()`, pure `view()`, `Tick` polls snapshot; parameter fold read **from the log** | GTK callbacks → `EventCx::emit` → `dispatch` → repaint | they simulated MVU's center; we have it enforced. Their view-side contract is still the better pattern. |
| Launcher | `Play` / `Splice{at_frame, crossfade}` verbs | launch modes, quantize, legato, follow actions, runs→arrangement | complementary; transcription candidates in §8 |

Three verdicts. (1) **Where they win, we lose nothing by admitting it**: the live engine, the
canvas maturity, gesture semantics. (2) **Where we win, they prove the difference matters**:
the log makes a session reproducible; their History does not make a session
reconstructible the way our replay does. (3) **Where the trade is real** (dispatcher), both
halves are worth having at different boundaries — typed commands at the *profile plugin*
surface, the closed carrier at the core.

## 8. What to steal, mapped to our code

1. **`Impact` classification** → ours: classify every log event's flush cost
   (`None | Params | Timeline | Graph`) at the interpreter; the iced shell's `Tick` learns
   what to re-snapshot, the live runtime what to re-swap. ~30 lines.
2. **Gesture open / amend / cancel**, and **`apply_amending`'s runs→one-undo-step** → ours:
   a `gesture` field on arrangement events, a `gesture open/cancel` control event; undo
   drops the whole gesture, redo re-inserts it at its original history position (our existing
   redo position rule already has this shape).
3. **`EventCx`/`CanvasView` shape** for our iced views — views stay pure
   `(model, ViewEvent) -> Vec<Message>`; menus/text-entry routed through a request channel;
   `wants_frames`/`dense` as declarative repaint hints.
4. **`kept()` layer cache** for our waveform/grid beneath the playhead.
5. **Spec-derived theme** + `ConsoleLook` style enum, device accents outside the skin → an
   iced theme module. (MIT — code is copyable with attribution when the seam demands.)
6. **Slot = membership of an ordinary clip** — no launcher-specific clip type, no second
   editor; the arranger lists track members only.

## 9. The launcher transcribed into our session log (candidate events)

Our rules bind hard: one is *model-visible means logged*; positions are absolute frames; replay
reproduces an exact timeline; and the profile travels as `Event::Arrangement {op, fields,
at_frame}` with canonical values. FaderFrame's model/runtime split maps onto it exactly —
because it makes the same division for a different reason:

| FaderFrame | Ours (candidate carrier event) |
|---|---|
| `Launcher.slots` (a clip sits in a slot) | `Arrangement { op: "slot", fields: {track, scene, clip}, at_frame }` — slots are **state**, they shape what the timeline holds |
| `ClipLaunch {mode, quantize, legato}` / follow actions | fields of the same `"slot"` op (`mode`, `quantize`, `legato`, `follow`) — canonical values, so replay is exact |
| scenes (names/order of rows) | declarative rows in the host's table layer (like sources/takes), not per-frame events — scene layout, not audio |
| `LaunchQuantize` + engine-owned quantized start, published `TrackStatus` | **never logged.** The live host derives the queue/start from the launch op + tempo log, and publishes derived status the shell's `Tick` polls. Display state is derived — the volatile-value echo of the Cordis update: keep identity, derive display, never remount |
| `LauncherOp::Launch {..} / StopTrack(..)` (with the queue target in volatile state) | logged as **events**: `Arrangement { op: "launch", fields: {track}, at_frame }` / `Arrangement { op: "stop", fields: {track}, at_frame = now }`. What is logged is *that a clip started being heard at frame F* — the quantized start is derived at replay from `at_frame + quantize + tempo map`, and because the tempo lives in the same log, replay renders the identical audio |
| Runs / "Record to Arrangement" via `apply_amending` | an uncommitted run exists only in live-host volatile state (it dies with the session — the log records only what was committed). Committing a run writes its placements — our existing splice/place ops — all carrying one shared `gesture` id, so undo drops the entire jam-capture in one step and redo re-inserts it in place. **No new event kind is needed** |
| `HistoryTo(steps)` | deliberately not adopted; ours is undo-by-replay, and the log stays the truth |
| tempo-stretch-to-tempo (`ClipLaunch.tempo`) | rides our existing stretch/rate machinery (a rate-scoped conform in the pool), not a launcher concern |

Two consequences are worth stating. First, **the alpha's `Splice` gains a sibling verb**
(`launch/stop`) that is *smaller* than splice — it needs no crossfade machinery, just
membership triggers — so the carrier's surface grows by vocabulary, not by variant count.
Second, **retrofitting is the reason to carve these now while the log is young**: every
existing FaderFrame-free session is trivially forward-compatible (no slots), but a log that
gains `gesture` and `launch` years later must invent its own back-compat. The minimum viable
incision is `slot` + `launch` + `stop` + the `gesture` field — four additions, no new event
types, no new replay rules.

## 10. What not to copy

- **The open `Action` enum as core.** Every feature mints variants there; ours stays closed
  and 8 events strong. Their ergonomics live behind a profile plugin boundary.
- **In-memory-only truth.** Their History is brilliant UX and unreproducible; a 30-minute jam
  must be handable as text. Our log remains the thing you hand to a friend.
- **Imperative shell edges.** Nothing in a paint path may touch what `update()` owns.

## 11. Alternatives considered

- **Adopt FaderFrame as our shell** (MIT; it compiles and runs here). Rejected: we are
  committed to iced + ratatui on our own minimal core; their GTK shell drags GTK's gravity,
  `gio`/`cairo` idioms, and a 3.4k-line dispatch monolith that would sit *across* our log
  rather than beside it. Borrow code only when a seam demands, per the Cordis note's gate.
- **Port the theme engine verbatim now.** Deferred: worth doing when the iced theme work
  actually starts (PA-0022's revisit trigger), since the Spec form is trivial to re-express.
- **Extend `replay_to` to be history-walk instead.** Rejected after study: PA-0008 (Infinite's
  two-tier undo) shows snapshot-undo pays a complexity budget that log-replay avoids; seek
  cost is our known gap, solved better by point-in-time pool/caching than by abandoning replay.
- **Log the queue state** (`queued` display state as events). Rejected: it is derived from
  launch time + quantize + tempo — logging it would fork the truth and break "nothing
  audio-rate or derived is logged".

## 12. Sources

- Clone: `~/Projects/faderframe`, upstream [github.com/BurningTreeC/faderframe](https://github.com/BurningTreeC/faderframe), rev `1f46123`, MIT (`LICENSE`).
- Files read in that checkout: `docs/ARCHITECTURE.md`, `README.md`,
  `crates/faderframe-project/src/{edit,launcher}.rs`, `crates/faderframe-session/src/{lib,launcher}.rs`,
  `crates/faderframe-ui-canvas/src/{theme,themes,view,event,painter,controls}.rs`,
  `crates/faderframe-view-launcher/src/lib.rs`, `crates/faderframe-view-arranger/src/{lib,global}.rs`.
- Built and run on this host: `cargo build --release` (2m 20s, GTK 4.24.1 / GStreamer 1.28.8 /
  rustc 1.98.1), then `target/release/faderframe --version`, `--demo --backend dummy` (session
  loaded, local CLAP plugins scanned, ALSA MIDI ports connected).
- Reference screenshots (from upstream `docs/screenshots/`) at
  `.agents/reference/faderframe-screenshots/` in this repo.

*Authored with GLM-5.3-Flash · DeepSeek Harness, 2026-10-14.*
