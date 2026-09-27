# Agent Note: Capture topology — aligned stems, not a summed mixdown

Status: proposed

## Problem

The platform's stated job is to *record long generative runs, then arrange them*. The runs do not
come from one instrument. They come from a **rig**: a modular patch, a SuperDirt/SuperCollider
voice, a semi-modular hardware synth, and possibly MIDI hardware — sounding at once, each on its
own device, each with its own clock.

Two problems follow, and they are usually conflated:

1. **Patching.** Getting N sources into the box on Linux means JACK/PipeWire routing. It works, but
   WirePlumber is a config daemon rather than a patchbay, and the graphical patchbays (`qpwgraph`,
   `helvum`) present a *graph* — the wrong abstraction for "record my rig".
2. **Clocking.** Several devices means several clocks, which means drift over a long take.

Both of the obvious simplifications are traps:

- **Patching everything into one stereo bus at capture time** is cheap to wire and destroys the
  stems. The later mixing and mastering stage then has nothing to work on, per-source editing is
  gone, and a source that was mis-levelled cannot be recovered.
- **Requiring a shared clock for every source** converts a *measurement* problem into a
  *distribution* problem, and buys hardware (DC-coupled interfaces, clock boxes) that may not be
  needed.

There is no artifact model for "a recording of a rig" today. `record <take_id>` takes one input
device; the pool holds sources; nothing states what a multi-source capture *is*, how its pieces
relate to one session timeline, or where summing is allowed to happen.

### Where this sits in the workflow

Per the Autechre methodology this project is built around — record a system running, then compose
by selecting and assembling — sound-arranger owns **record → align → mix → master**, and the
**mixdown is the export, the last step, not the capture step**. That ordering is the whole
argument for this note: capture must preserve exactly what mixing and mastering will later need.

## Proposal

**The capture artifact model is N parallel per-source takes, aligned to the session timeline.
Summing is a later, explicit, reversible mix step — never a capture side effect.**

Five parts.

### 1. A source is a logged declaration, not UI state

`source add` / `source set` / `source arm` are **logged commands** in the `host v1` vocabulary, so
the rig declaration is diffable, replayable and travels inside the session directory beside
`session.txt` and `pool/`. This is the same invariant as the clip model: the log is the truth, and
a session opened on another machine declares the same rig intent.

The **`Device`/`Rig` registry that owns this** is
[the input/jam-layer note](2026-09-01-input-jam-layer-device-registry-io.md); this note fixes only
the *capture-facing shape* of a source (identity, channels, clock role, binding rule) and does not
restate the registry's scope, characterization tooling or per-device adapters.

Identity is by **stable name plus a matching rule**, never by a port index — ALSA/JACK indices
renumber across reboots and replugs, and a session that breaks because a USB device enumerated
differently is not a session. Names are the address; the binding rule is how the name is resolved
at run time.

### 2. Alignment before clocking

"Clocking" is three separate questions, and only one of them matters to a passive recorder:

| Question | Who answers it | Needed for capture? |
|---|---|---|
| **Transport / position** ("we are at bar 17") | JACK transport, MIDI SPP, or nobody | Only if a source must follow a shared position |
| **Rate / tempo** | The session's own tempo map | Yes — and it is the *session's* job, not the sources' |
| **Alignment** (are the takes sample-aligned?) | **Post-hoc measurement** | **This is the one that matters** |

So every source declares a **clock role** — `master` (defines time), `follower` (we send it clock),
or `free` (record it, align it afterwards). The ordering follows:

- **Alignment is the requirement; clock distribution is an optimisation.** A source may be `free`
  and still land perfectly aligned, because alignment is measured, not wired.
- **Short takes**: record `free`, align on detected onset.
- **Long takes**: drift accumulates — and `crates/media` already carries **device-clock drift
  compensation wired into the capture**, which is precisely the capability this model needs. That
  asset is why the alignment-first ordering is affordable.
- **Clock distribution is opt-in per source**, for the sources that genuinely need to follow
  (a sequencer that must stay in phase, a drum machine that must not wander).

### 3. Capture writes parallel takes, never a mix

Each source becomes its own `CaptureNode` writing its own pool source
(`{take_id}.{source}.ch{N}`), at the session rate, all anchored to the same transport origin. The
capture path has **no mix bus**. The result is a *set of stems plus one timeline*, which is exactly
the input that mixing and mastering consume — and it is what makes a 65-minute multi-source take
editable per source later.

Multichannel sources are already split one pool source per channel at the import boundary; capture
follows the same convention, so a captured take and an imported file behave identically downstream.

### 4. Intent is compiled and verified, not hosted

**Do not build a patchbay.** PipeWire is scriptable (`pw-link`, `pw-dump`, `pw-cli`) and
WirePlumber accepts declarative routing rules. Following
[the external-programs rule](2026-09-21-external-programs-not-sidecars.md), the platform **invokes
the existing tool and logs the command**:

1. **Declare** the rig (`source add …`) as logged intent.
2. **Compile** that intent to `pw-link` calls and/or a generated WirePlumber fragment.
3. **Report** what actually resolved.

The third step is the feature. A patchbay UI is fiddly because it shows a **graph**; what a
recorder needs is a **diff between intent and reality** — which is a text command, trivially
scriptable, and the natural TUI citizen beside the existing pool panel:

```text
:source add vcv     --kind jack  --match "VCV Rack:out_1"  --channels 2 --clock free
:source add tidal   --kind osc   --label "SuperDirt"       --channels 2 --clock master
:source add volca   --kind alsa  --match "Notepad" --ch 3  --clock free
:source check
  vcv     ok    bound VCV Rack:out_1,out_2   (2 ch, free)
  tidal   ok    bound SuperDirt orbit 0      (2 ch, master)
  volca   MISS  no input matching "Notepad" ch 3
```

`source check` (intent vs. binding, with a non-zero exit for scripts) is the deliverable. It
replaces the fiddly UI with a diagnosis, and it is the affordance that turns the
pool-before-arrange error string into an answer.

### 5. Prefer fewer devices — a guidance, not a feature

One multichannel interface is **one clock, one driver, zero drift and no patchbay**. Aggregating
several USB devices is the origin of every problem above. The first recommendation to the owner is
therefore structural, not software: *prefer one interface with more inputs over a pile of devices*,
and treat multi-device as the fallback case the drift compensator exists for.

## What this note does not own

- **Device characterization, per-device adapters, firmware/update paths, BLE-MIDI bridging** —
  [the input/jam-layer note](2026-09-01-input-jam-layer-device-registry-io.md).
- **The clock/transport *contract* and the process-boundary integration rule** (MIDI clock / SPP
  out, audio-encoded sync as a pulse train, "a partner is something we can start or address, sync
  and record") — [the external-programs note](2026-09-21-external-programs-not-sidecars.md). This
  note decides only *what a capture produces* and *in what order the problems are solved*.
- **The device matrix** — which box takes MIDI clock vs. an analog pulse vs. DC-coupled CV — is
  gear research and belongs in the studio project
  (`../../../../../music/music-composition-theory/studio/instruments/`), linked not copied, per the
  separation-of-concerns rule.

## Alternatives considered

- **Sum the rig into one stereo bus at capture.** Cheapest routing, least code — and it destroys
  the stamps that make the platform an *arranger*. It also makes a mis-levelled source
  unrecoverable and forecloses per-source mixing and mastering, which the owner's stated workflow
  needs. Rejected: summing is a *mixing decision* and must be available later and reversibly, not
  forced at capture.
- **Require a shared clock for every source.** Makes alignment trivial by assumption and demands
  DC-coupled interfaces or clock distribution hardware; it also fails for gear that has no clock
  input at all (a free-running semi-modular). Rejected: it solves by hardware what a measurement
  plus the existing drift compensation already solves in software.
- **Build a patchbay/graph UI in the app.** The most obvious answer to "WirePlumber's UI is
  fiddly", and the largest surface: a live graph editor, plus device hot-plug semantics, plus the
  maintenance of a matrix of routes. It also duplicates a solved problem in the OS. Rejected in
  favour of compiling intent to `pw-link`/WirePlumber rules and *verifying* the result — the same
  posture the platform already takes toward every external program.
- **Make generated WirePlumber configuration the source of truth.** Persists routing across
  reboots for free, but puts the session's semantics in system config: not portable with the
  session directory, not diffable as part of the work, and the log would no longer reproduce the
  session. Rejected: config is an *output* of the registry, never the registry.
- **One device per source, aggregated in software.** Maximally flexible and maximally fragile —
  N clocks, N drivers, N hot-plug behaviours. Rejected as the default; retained as the fallback
  the drift compensator covers.
- **Record the rig with an external tool and import afterwards.** Sidesteps the whole problem and
  gives up the one thing the platform owns: the record → arrange loop stays inside the tool, and
  the pool-before-arrange requirement is met by import
  (`import`/`conform` already work). Rejected as the *primary* path, kept as the escape hatch.

## Acceptance criteria

1. A session can declare ≥ 3 sources by name, with per-source channel counts and clock roles, and
   the declaration survives save/load and replays identically with the rest of the log.
2. `source check` reports, per declared source, one of bound / missing / ambiguous, with the
   resolved port or device named — and returns non-zero when any source is unbound.
3. A single `record` against a declared rig produces **one pool source per channel per source**,
   anchored to one transport origin, with no mixed bus in the pool.
4. Recording three free-running sources for ≥ 10 minutes yields takes whose relative offset is
   stable (alignment within a defined tolerance) and whose drift is corrected by the existing
   compensation — verified by measurement on captured material, not by assumption.
5. Removing a device and re-running `source check` names the missing source rather than silently
   recording fewer channels.
6. A source that was summed by an explicit, logged mix step is recoverable by undo; a source that
   was never captured is not silently substituted.

## Risks

- **Scope trap — the real risk.** The input/jam layer is unbounded if allowed to be (its own note
  says so). This note stays inside the artifact model and the ordering, and defers device coverage
  entirely. The seam is the deliverable; per-device completeness is not.
- **Alignment heuristics can be wrong.** Onset detection fails on a sustained, quiet or silent
  source; a source that starts late or never sounds could be aligned to noise. Mitigation: alignment
  is *proposed with a confidence and a reported offset*, never applied invisibly, and a `master`
  source can pin the origin instead.
- **Indices and names drift.** A matching rule can bind the wrong node once new devices appear.
  Mitigation: bind by name-plus-stable-property, report ambiguity as a failure rather than picking,
  and never persist an index.
- **Hardware reality on this rig.** CV clocking needs a DC-coupled interface (a clock *pulse* does
  not), so `follower` roles may be unavailable for some sources until the interface changes — a
  gear constraint that must not be discovered halfway through a session.
- **Two records of truth for the rig.** The registry (session) and WirePlumber (system) can
  disagree. Mitigation: the registry is authoritative and `source check` reports the divergence
  rather than reconciling it silently.
