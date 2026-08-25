# Tauri shell review — the Host API contract vs. a live GUI

> Research input, 2026-08-25. Co-worker: GLM-5.3 (`pi --provider zai --model glm-5.3 --thinking high`), read-only, over the intended next step (Tauri v2 as the first rich reference shell). Scope: the Host API contract (`crates/host/src/lib.rs`) + the media surface a shell reads, against what a *live, interactive* GUI actually needs. The reviewer was prompted to validate claims against the code it read. All critical findings in this note were independently re-verified in this repo before being recorded.
>
> Signals at review time: repo clean at `ab0de22`; 139 tests, 18 suites, clippy clean. No frontend or `src-tauri` exists yet.

## Verdict

**The UI-as-plugin framing holds up** — the seam is real (the host speaks `HostCommand` only; `engine_ref()` is explicitly documented scaffolding), the vocabulary is versioned and validated against a closed registry, and the headless host proves determinism. **But the contract as shipped is a batch contract (assemble a session, render it to a file), not a live contract (drive a session in time).** The implemented note honestly defers `peaks`/`transport` to "the first shell"; this review is the bill for that deferral, plus several things in the current code that would actively mislead a GUI. It also caught one factual error in the reviewer brief (cpal *is* already a dependency) and correctly reframed it: the real gap is not cpal, it's that **nothing drives `engine.render()` into a device** — there is no audio callback thread anywhere in the workspace.

## Findings

### F1 — Critical: no transport, and `at_frame` is catch-up, not scheduling

`HostCommand` (lib.rs:62) has no transport commands — no Stop/Pause, no seek, no "start the clock". `Play` means "mount a player node for one clip" (and refuses a second, lib.rs:276); `Bounce` is the only renderer. Three traps:

- **`at_frame` means "render up to it, then apply at the current position"** — `run_script` (lib.rs:488–508): `if frame > now { render(frame - now) }`. A frame at or behind the clock applies immediately. A GUI user's "mute ch2 at bar 5" while playing is *future scheduling*, which neither the host nor the engine's public API exposes (`mount`/`patch`/`set_param` all schedule at `clock.frame()`; only `schedule_unmount` takes an absolute frame). The field *name* promises scheduling; the semantics is catch-up.
- **The clock only moves when `render()` is called** — no seek, no pause, no wall-clock anchor. Transport position ≡ rendered-so-far.
- **`HostSession` has no position accessor.** `log()`, `meters()`, `providers()`, `arrangement()` exist; frame/seconds/beat do not. The sanctioned way to read the playhead today is `engine_ref()` — which the note itself forbids.

**Why it matters:** play/pause/stop/seek/edit-while-playing *is* the product, not a nicety. Every panel — playhead, meters, mixer — is downstream of a ticking transport.

**Fix:** add a host-owned transport tier (`TransportPlay`/`TransportStop`/`TransportSeek(u64)`), a `HostSession::position() -> (frame, bpm, playing)` value, and decide the `at_frame` semantics for live use. The minimal honest v1 for seek = seek-while-stopped (rebuild + replay + render-to-target), which the deterministic log makes correct by construction.

### F2 — Critical: `HostSession` is `!Send` as a *type* — the planned `tauri::State<Mutex<…>>` will not compile

`Engine` contains three non-`Send` fields (all re-verified):

- `disposers: HashMap<&'static str, Disposer>` with `Disposer = Box<dyn FnOnce(&mut DisposerCtx<'_>)>` (plugins/mod.rs:33) — no `+ Send`.
- `op_handlers: HashMap<&'static str, OpHandler>` with `OpHandler = Box<dyn FnMut(…)>` (render.rs:61) — no `+ Send`.
- `ctx: Context` with `HashMap<&'static str, Box<dyn Any>>` (ctx.rs:17) — no `+ Send + Sync`.

`tauri::App::manage` requires `T: Send + Sync + 'static`; `Mutex<HostSession>` is `Send + Sync` iff `HostSession: Send`. It isn't. **The default Tauri pattern is a compile error, not a runtime hazard.**

**Fix — and it's the fix the seam already wants:** run `HostSession` on a dedicated host thread as an **actor** — an `mpsc::Receiver<HostCommand>` (HostCommand *is* `Send`: `&'static str`, `f32`, `PathBuf`, `ClipRef`, `ArrangeOp` are all owned/Send) plus a render pump. This needs **zero engine changes**, keeps "UI thread never touches the render path" physical rather than voluntary, and matches the note's own "command channel / event stream" language. The alternative — adding `+ Send` to `Disposer`/`OpHandler` and `Box<dyn Any + Send + Sync>` in `Context` — is a small honest core change that also enables offline render on a rayon pool, but it is *not required for the first shell and shouldn't block it*.

### F3 — Critical (known, but the drift risk is unowned): the wire schema is a text grammar; IPC needs a serde surface nobody owns yet

`HostCommand` and every value a GUI reads (`Event`, `Timeline`/`Clip`/`Track`, `PoolIndex`/`PoolSource`, `MeterBank`) are non-serde. `engine` is std-only by design so derives can't land there; the DTOs shouldn't live in `media` either. The note says "serde arrives with the Tauri adapter" without saying *where* or *how drift is prevented*. Once both exist there are **two grammars** (text `parse_script` + JSON) over the same `HOST_PLUGINS`/`HOST_PORTS`/`HOST_PARAMS` vocabulary — a classic divergence surface: e.g. `parse_arrange`'s strict `arity()` can't be expressed by plain serde derive, so a hand-rolled mirror starts laxer.

**Why it matters:** once both exist, a shell bug becomes "which surface is wrong?" — and the "text format is the wire schema the adapter validates against" guarantee silently becomes a lie.

**Fix:** put the wire DTOs in `crates/host` (a `wire` module): a `WireCommand` with `String` keys, `TryFrom<WireCommand> for HostCommand` that runs the *same* `in_list` validation `parse_script` uses, and serde DTOs for `Timeline`/`PoolIndex`/`Event`. Add a parity test: a golden set of `(text, expected HostCommand)` vectors must parse identically from text and from JSON, and the refusal sets must match. Keep `HOST_API_VERSION` as the shared version gate.

### F4 — Major: edit-after-wire is not just silent — `RemoveTrack` ghosts audio, and mid-render edits can shift audio

`wire_arranger` (lib.rs:368–410) skips tracks already in `wired_tracks` (line 380): the mounted `ArrangerNode` holds a **copy** of the track's clips (`arranger.rs`, `clips: Vec<Clip>`), so any edit to a wired track never reaches audio — the documented P1.3.4 deferral. Two adjacent behaviors are worse than "silent":

- **`RemoveTrack` leaves the node mounted and playing.** The op applies to the `Timeline` value; nothing unmounts the node or drops it from `wired_tracks`. After `arrange remove_track t0` + re-render, t0's audio keeps playing while the UI shows no track. A *lying* host, today, in the reference-host path.
- **A `MoveClip`/`Trim` on a partially rendered clip violates the reader-alignment invariant** the arranger asserts (`arranger.rs`: `debug_assert_eq!(reader.popped, off)`; "an underrun cannot be recovered without shifting every later sample"). In release, an edit while playing produces shifted audio with no counter.
- Re-wiring is *structurally* blocked: `Graph::connect` requires array-forward order (graph.rs:693), so a re-added arranger node (appended at the end) can never connect to the earlier-indexed mixer — hence the deferred "mixer-last re-application".

**Why it matters:** for a GUI this is the primary loop (listen, razor, move, listen again). Edit-while-*stopped* is fixable cheaply; edit-while-*playing* is deferred design work.

**Fix (minimal, ordered):** (1) treat `RemoveTrack`/any dirty edit on a wired track as "retire that track's node" (remove node + cord, rebuild with warmed readers, reconnect) — accepting the mixer-remount dance for now, or better, add a `Graph` API that inserts/reorders; (2) document that v1 live edits apply at *next transport stop* or via node rebuild; (3) the real fix stays the shared-`Timeline` `ArrangerNode` + a control-side reader reconcile — but note that reconcile must also handle reader *re-positioning* for already-playing clips, which the SPSC design can't do without a rebuild.

### F5 — Major: the "values" half of the contract is thinner than the note implies

The contract advertises three values — graph value, providers, pool index. What ships:

- **Pool index: unreachable.** `set_pool` (lib.rs:192) closes over the dir inside the resolver closure; `HostSession` never keeps the `Pool`, and `Pool::list()` — precisely "the value a timeline UI reads" per its own doc — is not exposed. A shell that wants the pool must open its *own* `media::Pool`: a bespoke client, exactly what the seam forbids. Same for peaks: `PeakFile::read` exists but no host accessor.
- **`providers()` misses scheduled-but-unapplied mounts.** It reads `node_of`; a just-issued `Mount` sits in `scheduled` until `flush_scheduled()` (only called by `wire_pending`/`wire_arranger`, and only when non-empty/dirty). A patch bay that mounts the mixer then immediately reads `providers()` sees an empty list until the pump renders. In a script this never shows (at_frame forces renders); in a GUI it shows on every cold start.
- **`meters()` returns `Option<Arc<MeterBank>>`** — fine cross-thread (atomics), but an *engine type* crossing the boundary, frozen unless render is running (ties into F1).

**Fix:** add host-owned accessors `pool_index()`, `peaks(source_id, level)`, `position()`, and make `providers()` include scheduled mounts (or guarantee a flush after each applied command batch). These are the values the first slice's canvas timeline literally draws from — without them the slice can't be built through the seam.

### F6 — Major: sample rate is hardcoded on one side and defaulted on the other — a silent pitch/speed lie

`HostSession::new` hardcodes `Engine::new(48_000, 120.0, 4)`; no `HostCommand` can set the session rate. `media::devices::open_output` (devices.rs:59) opens the **device's default config** and returns whatever rate that is. On a 44.1 kHz-default device, the pump produces 48k frames/s of session audio, the callback drains at 44.1k, and the result is audio ~8% slow/flat — **with no counter** (the output path has silence-on-empty but no over/underrun accounting; only input counts, devices.rs:40–43). The arranger carefully refuses a 44.1k *source* in a 48k session (`ArrangerNode::new` rate check), but the session↔device mismatch — the loudest form of the same bug — is unguarded.

**Fix:** for the first slice, negotiate — query the device, set the `StreamConfig` sample rate to the session rate (add a config override to `open_output`), or construct the session from the device rate (needs a session-config command or `HostSession::with_rate`). Add underrun/overrun counters to the output ring and surface them — this codebase counts everything else; don't start lying at the device edge.

## Wrong or misleading relative to the plan

1. **`RemoveTrack` ghost playback** (F4) — a real bug today, not just a deferral. The P1.3.4 note's "an edit after a bounce is silent" *understates* it: removal keeps playing audio the value no longer shows.
2. **`at_frame`'s name promises scheduling; the code delivers catch-up-then-apply-now** (lib.rs:488). The doc is accurate; the *name* is the trap.
3. **"The text format is the wire schema"** — true today, but the note says "serde arrives with the Tauri adapter" without an owner or a parity obligation, permitting two schemas with no test tying them (F3).
4. **The reviewer brief's "cpal is not yet a dependency" was false** — `crates/media/Cargo.toml:10` has `cpal = "0.18.2"`, and `devices.rs` has enumerate/open-input/open-output (verified on a Scarlett 2i2 per the Spike B note). The real gap is that nothing drives `engine.render()` into a device; the reference host's "render" is a synchronous `Vec<f32>` bounce on the caller's thread (lib.rs:413).
5. **`HostCommand::Record`'s error message** ("the reference host bounces the master instead") is misleading — recording ≠ bouncing; the actual story is "the device path exists in `media` but isn't wired into the host." Minor, but it's the message every early GUI click will surface.
6. **`meters()`/`providers()` cross the boundary as engine types** (`Arc<MeterBank>`, `SignalKind`) while the note frames the seam as shell-agnostic. Acceptable for an in-process reference host; the Tauri adapter must convert, so the events/values half is currently Rust-shaped, not wire-shaped (consistent with F3).
7. **Verified honest:** determinism tests are real and non-vacuous (byte-identical bounces *and* identical logs; splice-effect-differs-from-control; refused-commands-never-logged — `tests/reference_host.rs`); the parser is panic-safe on truncated input; the bounce budget guard works. The note's test claims check out.

## Recommended ordering for the Tauri sub-steps

Steps 1–2 need **no Tauri at all** (consistent with the project's headless-host-first pattern):

1. **Decide the product name / bundle identifier.** Cheap now, painful after `src-tauri` exists (identifier baked into appdata paths, updater identity, deep links).
2. **Round out the contract, `crates/host` only:** `position()`, `pool_index()`, `peaks()`, providers-includes-scheduled; fix the `RemoveTrack` ghost (F4-1); add the `wire` module (serde DTOs + `TryFrom` through the same `in_list` validation + text/JSON parity tests). Pure host-crate work with tests — the drift risk dies here.
3. **The live runtime, still no Tauri:** host-thread actor (`mpsc<HostCommand>`, oneshot for values) + a render pump into an `Spsc` + `devices::open_output` with rate negotiation + output-ring counters (F6). Transport commands `Play`/`Stop`/`Seek` (= rebuild + replay + render-to-target) as host semantics over the deterministic log. Acceptance: a host test that plays an arrangement, issues an edit mid-playback (applies on stop/rebuild), seeks, and bounces byte-identically to the offline script run.
4. **Scaffold Tauri v2** with minimal capabilities: `core:default`, `dialog:default` (file pickers), `fs:read-*` scoped to the pool dir, `fs:write-file` scoped to the bounce/export dir, `core:event:default` for meters/log listens. No `shell`, no unscoped `fs`, no broad window permissions. The adapter is ~6 commands mirroring `WireCommand` + two events (meters @30 Hz, log-cursor) — nothing else.
5. **First Vue slice that proves the seam:** canvas timeline drawing `arrangement()` + `pool_index()` + base-level peaks as a typed array over a channel; transport bar with live frame/beat from `position()` (frontend-interpolated, ~10 Hz resync — as the app-shell note specifies); one razor/drag gesture issuing `Arrange` ops. **Do not** wire `Play`/`Splice` into the first slice — `warm_player` blocks up to 10 s on the actor thread and will stall the pump; the arranger path is the future anyway.
6. **Bounce as an offline command:** replay-the-log-on-a-fresh-session (which `run_script` already is), never a synchronous render on the live actor. Keeps "live session" and "render" cleanly separated forever.

## Questions for the maintainer

1. **Seek semantics** — is seek-via-rebuild acceptable for v1 (replay log + offline render to target; correct but O(target-frames) per seek)? Any gesture needing sample-accurate *backwards* seek changes the `ArrangerNode` reader design materially.
2. **`Send` bounds** — make the small core change (`Disposer`/`OpHandler + Send`, `Context: Box<dyn Any + Send + Sync>`) now, or keep the actor the only sanctioned threading model? The actor suffices; the bounds also unlock parallel offline bounces later.
3. **Future `at_frame`** — should engine commands grow true future scheduling (the scheduler supports it), or does the GUI only ever edit "now" and schedule via dedicated transport commands?
4. **Mono** — mixer, meters, bounce are all mono today (pan "arrives with stereo"). Is a mono v1 timeline an accepted product constraint for the first shell, or does stereo land first? It affects the canvas, the mixer panel, and `WavWriter` channels.
5. **Name/identifier** — decision date for the platform name (RESEARCH §14 risk 7)? Must precede ordering step 1.
6. **Bounce/export UX** — fixed pool-relative export dir (capability-scoped `fs:write-file`) vs. save-dialog everywhere? Decides the narrowest capability set.

## Dev-loop (correctly sized as minor)

pin the vite `server.port` + `strictPort` and mirror it in `tauri.conf.json`'s `devUrl` (rolldown-vite is a drop-in `vite`, so the CLI doesn't care); on Nix, build *and run* the Tauri binary inside the devenv shell (it dynamically links webkit2gtk-4.1/libsoup3/JACK-adjacent libs from the store — running it outside the shell fails), and add `alsa-lib` + `pkg-config` to the shell for cpal.
