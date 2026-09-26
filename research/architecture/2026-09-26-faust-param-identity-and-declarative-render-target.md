# Research: Faust parameter identity, and a declarative render target

> **Date:** 2026-09-26. **Scope:** two questions that came out of a hands-on
> Faust session (a personal course on `faust2vcvrack`, kept in a separate repo).
> **(a)** The [Faust-as-DSP-source note](../../.agents/notes/proposed/architecture/2026-09-20-faust-as-optional-dsp-source.md)
> leaves the parameter-identity convention to the spike; this measures it.
> **(b)** Can a eurorack patch be a *declarative value* we generate and render
> offline, instead of something a human wires by hand? **Method:** compiled
> real `.dsp` files with the locally installed **Faust 2.88.0** (`/usr/bin/faust`,
> LLVM 22.1.8) to C++ and to Rust, generated and loaded patches in a locally
> installed **VCV Rack Free 2.6.6**, and read the schema back out of a patch Rack
> itself had written. Numbers and quoted output below are from those runs.
> **Status:** research, not a decision. It feeds the Faust note's acceptance
> criteria and opens one question it does not currently ask.

## 0. Verdict on one screen

| Question | Answer |
|---|---|
| `-json` `address` or the UI-walk label as parameter identity? | **Either — they are the same string, in the same order.** `ParamIndex` *n* is the *n*-th control in `-json` order, and `address` is the path form of the UI-walk label. No convention to choose. |
| Is `[CV:N]` dead weight in the engine tier (no CV jacks)? | **No — it survives as data.** The Rust backend emits `declare(ParamIndex(n), "CV", "k")` on the UI interface, so the tag is readable at mount without a Rack host. |
| Do we store param values normalised or in engineering units? | **Engineering units, and we range-check them** (`render.rs` refuses out-of-`ParamDef` before logging). VCV Rack stores **normalised 0…1** and re-derives the range at load. Worth knowing before any bridge; ours is the better default. |
| Is a `.vcv` patch generatable? | **Yes, and it is small** — a tar archive, zstd compressed, containing one `patch.json`. ~100 lines of Python including validation. |
| Does Rack tell you when a generated patch is wrong? | **No. Nothing.** A bogus plugin slug, malformed `patch.json`, and `Rack -d` all load silently and render silence. See §4 — this is the load-bearing finding. |
| Should sound-arranger generate eurorack patches? | **Unresolved, and deliberately so.** The format is a *state snapshot*; our session log is an *event log*. §5 states the mapping problem rather than pretending it away. |

## 1. Parameter identity: measured, not chosen

The Faust note's acceptance criteria say the spike "settles and records: the
parameter-identity convention (`-json` `address` vs UI-walk path)". It does not
have to be settled, because the two agree.

Take `morph_voice.dsp` (10 controls: 2 with CV, 1 button, 7 sliders/entries).
Compile it twice — once with `faust -json`, once to Rust with the note's recipe —
and compare.

`faust -json` emits a `*.dsp.json` sidecar (**not** to `-o`; `-o` gets the C++).
Its `ui` tree, flattened, gives labels in widget order. The Rust backend's
`build_user_interface_static` gives the same controls with explicit indices:

```rust
ui_interface.declare(Some(ParamIndex(5)), "CV", "1");
ui_interface.add_horizontal_slider("Osc/freq", ParamIndex(5), 2.2e+02, 2e+01, 8e+03, 0.1);
ui_interface.declare(Some(ParamIndex(7)), "CV", "2");
ui_interface.add_button("Play/gate", ParamIndex(7));
```

Comparing index-by-index:

| `ParamIndex` | Rust UI-walk label | `-json` order | `address` |
|---|---|---|---|
| 0 | `FM/index` | `FM/index` | `/morph_voice/FM_index` |
| 1 | `FM/index CV` | `FM/index CV` | `/morph_voice/FM_index_CV` |
| 5 | `Osc/freq` | `Osc/freq` | `/morph_voice/Osc_freq` |
| 7 | `Play/gate` | `Play/gate` | `/morph_voice/Play_gate` |

**All ten agree, in order.** So:

- `address` is just the label with `/` → `_` and the root group prefixed. It is
  a *different spelling* of the same identity, not a different identity.
- The **UI walk is the cheaper source**: it is already in the generated Rust, it
  carries the index, and it needs no sidecar file or extra process at build
  time. `-json` remains the better *inspection* tool (it is readable, and it
  carries `min`/`max`/`step`/metadata as data).
- **Corollary for the log:** the identity is a compile-time constant, so the
  `path → ParamIndex` map is built once per class, not per instance, and
  `build_user_interface_static` is a `static` method precisely so it can be
  called without a live DSP. The note's "once at mount" is really "once per
  class, at registration".

### The `[CV:N]` tag is data, not decoration

The lesson I set out to write asserted that in the engine tier — no CV jacks —
the tags are "dead weight". **That was wrong**, and the generated Rust says so:
the tag arrives as a `declare` on the UI interface, one line before the control
it belongs to:

```
declare ParamIndex(1) CV=4
declare ParamIndex(5) CV=1
declare ParamIndex(6) CV=3
declare ParamIndex(7) CV=2
```

The tag is stripped from the *label* and preserved as *metadata*. So a module
built for our engine can still declare "this control wants 1 V/oct", and an
adapter that later grows a CV surface can honour it without a `.dsp` change.
The cost of keeping the tags is a `_CV` suffix in the identity string; the cost
of dropping them is losing the declaration permanently.

**Decision-relevant consequence:** `ParamIndex` numbering is a function of
*widget declaration order in the `.dsp`*. Reordering two `hslider` lines
renumbers every later control. Since the session log stores the param *name*,
not the index, replay is safe — but any consumer that persists an index is not.
Worth an assertion in the spike.

## 2. We store engineering units; Rack stores normalised 0…1

A difference that will matter if we ever bridge, and is easy to get wrong.

Ours (`crates/engine/src/plugins/mod.rs`, `log.rs`, `render.rs`):

```rust
pub struct ParamDef { pub name: &'static str, pub min: f32, pub max: f32 }
// Event::SetParam { plugin, param, value: f32, at_frame }
```

and `set_param` **refuses** before logging:

```rust
if value < def.min || value > def.max {
    return Err(format!("parameter '{param}' out of range [{}, {}]: {value}", def.min, def.max));
}
```

`440.0` means 440 Hz. A bad value is an `Err`, never an event.

Rack's `.vcv`, from a patch Rack itself wrote:

```json
{"id": 0, "value": 0.021021021021021023}
```

`0.021` on a 20…20000 linear slider, because `configParam` is linear and the
patch stores the fraction. `440.0` there means *440 × the full range*, clamped to
the top — a 20 kHz sine at the correct volume, with no message.

Ours is the better default and I would not change it: absolute values are
legible in a log a human reads, and the range check is a real error path.
Recording it because the mapping is `(v - min) / (max - min)` and it is exactly
the kind of conversion that gets applied in the wrong direction once.

## 3. A patch file is data, and generating one is cheap

A `.vcv` is a **tar archive, zstd compressed**, containing one file named
`patch.json`. Not JSON — Rack 2 will not read a bare JSON file, despite a
persistent community claim that a "legacy path" accepts one.

```sh
tar -cf patch.tar patch.json
zstd -q -f patch.tar -o patch.tar.zst --long=27
mv patch.tar.zst patch.vcv
```

The schema is best learned from a patch Rack wrote, not from the manual (there
isn't one) and not from forum threads (they disagree). My first attempt,
written from memory, got four things wrong:

| | guessed | actual |
|---|---|---|
| module slug field | `module` | **`model`** |
| param id field | `paramId` | **`id`** |
| cable endpoints | `leftModuleId` / `rightPortId` | **`outputModuleId`/`outputId`** + **`inputModuleId`/`inputId`** |
| param values | `440.0` | `0.021` (§2) |

Two more details that are not optional and are not checked: the file inside the
archive **must** be named `patch.json` (Rack looks for that name and silently
loads an empty patch otherwise — I lost a test cycle to this), and module/cable
ids are **random 64-bit integers below 2<sup>53</sup>**, not 1, 2, 3.

A generated patch turned out to be a strict *subset* of the keys Rack writes
(Rack adds `zoom`, `gridOffset`, `unsaved`, `masterModuleId`, and per-module
`version`, `data`, `leftModuleId`, `rightModuleId` — all view state or optional
metadata). Worth sitting with: **an invented key is ignored, and a typo is the
only way this format fails.** Both directions are silent.

## 4. Rack reports nothing — the load-bearing finding

Running the generated patch headlessly (`Rack -h patch.vcv`, Rack 2.6.6):

| patch | result |
|---|---|
| valid | runs |
| **plugin slug that does not exist** | runs, no warning |
| **`patch.json` malformed — not JSON at all** | runs, no error |
| the same, with `Rack -d` | runs, no error |

Nothing in `log.txt` (which records startup, settings and translations, and
never the patch). No `RACK_LOG_LEVEL`, no `logLevel` setting; `Rack -d` does not
surface it. `libRack.so` contains
`"Failed to load patch. JSON parsing error at %s %d:%d %s"` — the diagnostic
exists and headless never reaches it.

One gotcha worth recording: on a machine with a display, Rack blocks forever on
a tip-of-the-day dialog, which on Wayland renders as **an empty window with an OK
button** and shows up in a stack trace as `osdialog_message` → `wait4`. Fix:
`"showTipsOnLaunch": false` in `settings.json`. Required for any scripted launch.

**The generalisable part.** The response is not "be careful" — it is that a
generator which *cannot fail* is not a generator. The working version validates
against the manifests Rack unpacked into its own plugin folder, and refuses to
write:

```
error: plugin 'sinee' is not installed. Installed: delay, sine
```

Four classes of mistake are caught before a byte is written: unknown plugin
slug, unknown model within a plugin, value outside the declared range, empty
patch. This is the same shape as our own `set_param` range check in §2, and it
is the argument for keeping that check: *the boundary where values enter from
outside is the boundary that must validate.*

## 5. The unresolved part: snapshot vs event log — and the clock

**A `.vcv` is a state snapshot. Our session log is an event log.** They are not
the same shape, and the mapping is the actual cost of this idea.

- Generating snapshots is free. We never had events, so nothing is lost.
- *Recording* a Rack performance and replaying it is not: going snapshot →
  events needs to know what changed, and a `.vcv` does not say. Order is gone;
  two patches differing in one param are indistinguishable from two differing
  in one param plus a reordering of everything else.
- Our [`shell-state-is-a-fold-of-the-log`](../../.agents/notes/implemented/architecture/2026-09-21-shell-state-is-a-fold-of-the-log.md)
  already knows the forward direction (log → state). This would need the
  inverse, and the inverse is lossy.

**The clock is the part that decides it.** A patch gives us param values and
cables, and nothing else — we cannot call a module's method. So any process
living *inside* Rack is opaque to us: we can only set values over time. We own
the clock (minimal-core note), so the clock has to *reach* Rack over CV/MIDI,
which Rack stores in the patch as a `moduleMapping` section. That means a "live"
patch is still a declarative artifact — a bigger snapshot, not a new integration.

The genuinely irreducible limit: **snapshots cannot carry sub-frame-continuous
automation.** Rack's mapping updates at control rate. So notes and events are
fine via mapping, and anything needing sample-accurate modulation has to arrive
as CV on an input jack. Which loops back to §1: the `[CV:N]` declaration is what
makes such a jack interpretable.

**Therefore, undecided, deliberately.** Offline render first (it is free and
deterministic), live only via mappings. But the honest summary of this section
is that the format question is settled and cheap, and the *value* question is
not: we would be generating patches for ~2000 modules we do not control, to
obtain timbres our own Faust tier can already produce with a text file we *do*
control and a licence we already clear. That is the trade to name before
building anything.

## 6. What I could not verify

- **That a generated patch produces audio.** The local PipeWire config exposes
  no `support.null-audio-sink` factory, and the alternative was rendering
  through the real ALSA output. Untested, and it is the check that matters:
  §4 means a structurally valid patch and a working one are indistinguishable
  without listening.
- **That Rack even opened an audio device.** It logs nothing about the device.
  Absence of an audio error is not evidence of a device; it is evidence of
  silence, which is this document's subject.
- **Autosave does not fire headlessly**, so the obvious round trip (load, let
  it autosave, read the result back, confirm the modules survived) does not work.

All three are the same gap: §4's failure mode has no signal, and the signal I
wanted was the audio. Closing it needs a null sink — `pw-cli create-node adapter
'{ factory.name=support.null-audio-sink … }'` and record from its monitor — on a
machine whose PipeWire cooperates.

## 7. Prior art

- [`sandraschi/vcv-rack-mcp`](https://github.com/sandraschi/vcv-rack-mcp) — an
  MCP server built on exactly this insight (`.vcv` as generated structured
  data). Read before building anything here; it is the "do not reinvent" answer
  for the generation half.
- *Creating VCV Rack patch files from software* (VCV community) — the working
  Python that motivated this. Confirms the approach is ordinary, not exotic.

## 8. Consequences for the Faust note

The note's acceptance criteria ask the spike to settle the parameter-identity
convention. **§1 settles it by measurement** — the spike should assert the
index↔label agreement rather than choose between the two spellings, and should
add the two things this session found that the note does not mention:

- the `[CV:N]` tag arrives as a `declare` and is therefore worth keeping even in
  the no-CV engine tier (§1);
- a consumer that persists a `ParamIndex` across a `.dsp` edit is fragile, and
  the log's use of *names* is what protects replay (§1).

Everything else here is a **new** question the note does not ask, and it should
not be folded into it: whether a declarative eurorack patch is a sound-arranger
capability at all (§5). That is a direction question, not a DSP-source question,
and it stays open.

*Authored with Claude (OpenCode) · OpenCode, 2026-09-26.*
