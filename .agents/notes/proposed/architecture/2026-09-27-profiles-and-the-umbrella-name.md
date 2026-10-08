# Agent Note: Profiles and the umbrella name — recorder leads, arranger and sculptor follow

Status: proposed

## Problem

Three things share one name. "sound-arranger" is the repository, the platform, and the ACID clip
editor — and the platform never got a name of its own. The debt was recorded, not paid: RESEARCH
§14 risk 7 says *"resolve before the first tag"*, and
[umbrella-first](2026-08-15-umbrella-first-product-direction.md) made it an acceptance criterion
("the name question is decided … before the first tag/release"). Neither happened, and the
ambiguity has been doing quiet work ever since: RESEARCH §7 bolds **Capture** and **Arrange**
alike as "Phase 1 (first profile)", the [alpha finish line](2026-09-23-alpha-finish-line.md) is
titled *"the arrange loop, end to end"* and wires recording in as its prerequisite, and
[`docs/capabilities.md`](../../../../docs/capabilities.md) presents recording and arranging as one
product. The owner's current need — send a clock, receive audio, align it, master it, export it —
has no owner in that structure.

The cost is measurable, and it is not the UI. The arranger owns **~4.6k of `crates/media`'s 9.0k
source lines (~51%)** (timeline, arranger node, clip editor, snap, stretch), **33 of
`workflow::Action`'s 50 variants**, 28 `timeline_key` handlers against 2 `pool_key`, and a
1,350-line timeline module — while the recorder's two defining capabilities are stubs: MIDI clock
output is empty marker traits (`MidiSink: EventSink {}`), and there is no alignment model at all
(`record`'s own doc says the take "is not aligned to the playhead"). The recorder has no gesture
in the TUI; it is reachable only by typing `:record <id>`.

Worse than unrouted, the recorder is a **subset of the arranger's data model** rather than a
sibling:

| Coupling | Where | Consequence |
|---|---|---|
| Export length is the timeline's | `crates/host/src/lib.rs:1776` → `self.arrangement()?.end_frame()`; `Err("nothing to export: the arrangement has no clips")` at 1778 | A session with a take and no clips **cannot export** |
| Every render wires the arranger | `render()` 1627 and `render_with_drain()` 1646 both call `wire_arranger()` (1533–1615) | The arranger is on the recorder's render path unconditionally |
| Recordings are not logged | `HostCommand::Record` absent from `is_state()` (398–413) | A session's recording is not in the document; `load` would re-open the device |
| The core's vocabulary is B's | `Event::Arrangement` documented as "the clip-editor's ACID ops" | The mechanism is generic; the naming and one graph affordance (`Graph::insert_before`) are not |

## Proposal

**Name the platform `audio`, and assemble it from three profiles: `recorder` (focus), `arranger`,
`sculptor` (deferred).**

- **The umbrella is `audio`** — repo and remote slug included; the owner's call, made rather than
  deferred. The register is right: the platform is *infrastructure*, not a product, and the
  profiles carry product identity, so `audio recorder` / `audio arranger` / `audio sculptor` reads
  as a family without the umbrella claiming a personality. The acknowledged cost is that
  `~/Projects/audio` is the least distinguishing name among its audio siblings (`music/*`,
  `vcv-rack`, `tidal-lsp`, `pi5-daisy-synth-rig`); accepted, because the profiles are what anyone
  actually names. Mechanically the rename is **prose-only** — the crates are `engine`, `media`,
  `host`, `workflow`, `[workspace.package]` carries no name, and no code identifier contains
  "sound-arranger" — so the only real cost is ~20 doc mentions.
- **One active front.** `recorder` is the focus and the architecture's proof; `arranger` is
  committed and sequenced after it; `sculptor` stays deferred per
  [external-programs-not-sidecars](2026-09-21-external-programs-not-sidecars.md).
- **The recorder is the proof of the architecture — with one named gap.** It proves the
  capture → align → mix → master → export spine, the seams (device registry, external programs,
  the log), determinism, and the realtime path. It does **not** prove *structural live editing* —
  graph swap under load — which is the substrate's hardest invariant. That corner stays the
  arranger's, and stays on the list below, so "the recorder is the proof" remains a measurement
  rather than a hope.
- **A profile becomes a declared thing, minimally**: a name, the ops it exposes, the plugins it
  mounts, and the UI surface it shows. Not a loader, not a framework. Two products is exactly the
  "real second provider" the budget rule waits for; a profile *selection* (`--profile`, or the
  shell's own choice) is what earns it now.
- **Profiles are runtime instances, and they compose across the process boundary.** Two instances
  on one machine are legitimate: an **arranger instance's audio output routed into a recorder
  instance's input**, where the recorder declares it as a source by name and captures it alongside
  the hardware sources — one take per source, no summing, per the
  [capture-topology note](2026-09-25-capture-topology-aligned-stems.md). The recorder owns the
  clock (it is the tape), so the arranger takes the `follower` clock role that note already
  defines; and because both instances sit in one audio-device graph, that source is the *easy*
  one — same clock, no drift, alignment by construction. The mechanism is the
  [external-programs rule](2026-09-21-external-programs-not-sidecars.md)'s, applied to our own
  binary: a partner is something we can start or address, sync and record.
- **The recorder overdubs.** "Just hit record again" over playback is cheap and expected: the
  player path already exists for it (`Play`/`Splice` "remain for the recorder player path"), and
  an overdub is simply another aligned take.
- **Length stays derived from the value.** `Timeline::end_frame()` measures the export length from
  the material on purpose — "measured from the value, not handed in, so a mix cannot be exported
  short by a stale frame count" — and that invariant is worth more than a configurable number. So
  the recorder adds **no** session length: a capture produces a **placement**, the placement
  extends the timeline, and the existing derivation does the rest. An explicit length stays a
  separate, optional affordance for rendering *past* the material (a decay tail, silence), which
  is a limitation the capabilities page already names — not the mechanism.
- **The recorder's first slices are the under-the-hood changes, not the UI**: (1) recordings as
  logged state; (2) a capture produces a **placement** (a clip at its origin), so the
  already-derived render length sees the take and the timeline extends itself — no handed-in
  length; (3) MIDI clock out; (4) an alignment/offset model. A record gesture in the shell is a
  small, separate follow-on, and a render path that does not wire the arranger becomes *optional*
  — worth doing only if a timeline-free recorder build is ever actually wanted.

### The levels: core, plugin, profile, rig

Four levels, each assembled from the one below — **composition, not containment**:

| Level | What it is | Does it run? |
|---|---|---|
| **Core** | clock · graph interpreter · session log · context · media engine — model-free, shared | never alone |
| **Plugin** | one capability as a node/op/service (`euclidean`, `scale`, `tone`, `mixer`, `master`, a CLAP-host adapter) | inside a profile |
| **Profile** | an assembly *value*: name + op set + mount set + default surface | **yes — this is the process** |
| **Rig / session** | one or more profile **instances** composed across a process/audio boundary | yes, as a chain |

Two rules follow, and they are the point:

- **The platform is never the process; a profile instance is.** The core is a library that
  `run_script` assembles a profile from; every running binary is some profile's assembly. That is
  why the umbrella is infrastructure and the profile carries the product name.
- **Profiles stay coarse; the small units are plugins.** The Unix/JACK impulse to give the mixer or
  a generator its own profile is right about small units and wrong about *which* unit: a process
  boundary around something that must share the sample clock and the graph is exactly what the
  [external-programs rule](2026-09-21-external-programs-not-sidecars.md) says a boundary is not for.
  A partner is something you start or address, sync and record — not your own internals.

**The end goal is the rig/studio**: a set of small programs wired into one session that can be
played, recorded and arranged, with a session declaring its own sources, channels and clock roles
([the rig declaration is state](../../implemented/architecture/2026-09-27-the-rig-declaration-is-state.md)).
`recorder` stays the focus because it is the tape that makes a rig usable — capture, alignment,
clock, mix, master, export. The levels name the goal; they do not re-order the work.

### What this phase does not prove (the arranger's corner, owned and written down)

Structural live editing (graph swap under load) · clip/region semantics in the value schema · the
33 document gestures · time-stretch and tempo match · whatever session-format pressure heavy
editing applies. The arranger **resumes** when the recorder's acceptance criteria below pass and a
real session has been captured, aligned, mastered and exported end to end — a trigger, not a
sentiment, because "viable but not focus" decays into "never" without one.

## Alternatives considered

- **Leave the arranger as the first profile (status quo).** Rejected: measured above, it built the
  half that is not the current need and left the recorder's defining capabilities stubbed, with
  the recorder modelled as the arranger's input rather than a product.
- **Name the umbrella `sound`.** Rejected: the other pre-registered candidate, and it has both of
  `audio`'s weaknesses — generic where it needs to disambiguate, and it reads as a *product*
  rather than a platform.
- **A distinctive umbrella name (`algedonic`).** Rejected in favour of `audio` (the owner's call).
  It is the better *identity* — it disambiguates among the owner's audio projects, and because no
  code carries the name the switch would be cheap. But `audio recorder/arranger/sculptor` is the
  better family reading, and in Beer's VSM an *algedonic signal* is the affect/emergency channel
  that **bypasses** the normal reporting hierarchy — close to the opposite of this platform's
  logged-flow thesis. Worth remembering if `audio` ever grates.
- **A fourth profile for dub / live console mixing.** Rejected: it is a *usage pattern*, not a
  product — run the arranger instance, route its output into the recorder instance, and perform
  the mix while the tape runs. The vocabulary already exists (sources by name, clock roles,
  fixed-offset alignment), so a fourth profile would restate what two instances express.
- **Make the recorder a profile in name only.** Rejected: leaving A a subset of B's data model
  keeps all four couplings, and they would surface later as bugs (an unexportable recording; a
  device re-opened on load) instead of being decided now.
- **Build a real profile/loader abstraction.** Rejected: two profiles justify a name, an op set
  and a mount set — not machinery. The budget rule is binding.
- **Rename the crates to `audio-*`.** Rejected for now: no code carries the project name, so the
  prefix buys nothing and costs churn across every `use`. Revisit if the crates are ever published.
- **Split the mixer, a generator, or the clock out as its own profile (`mixer` profile, `clock`
  profile).** Rejected: it moves a plugin up a level and buys a process boundary and a clock bridge
  for something that must share the sample clock and the graph. The small unit is the plugin; a
  profile is a program; a rig wires programs. Revisit only if one of them must run on another
  machine or outlive the session's process.

## Acceptance criteria

1. The repo and remote are renamed to `audio` — prose-only (~20 doc mentions; no crate, package or
   source identifier changes), discharging RESEARCH §14 risk 7 before the first tag.
2. The conflation is gone from the artifacts: RESEARCH §7's feature groups, `capabilities.md` and
   the clip-arranger explainer each name the profile they describe, and nothing calls the arranger
   "the first profile" without qualification.
3. A profile is selectable between `recorder` and `arranger` (`sculptor` absent while deferred):
   the recorder exposes no document ops and no timeline surface; the arranger exposes them.
4. A recorded take's identity — id, frame count, dropped frames, channel count, origin — survives
   save/load **without re-opening a device**, and a saved session's log describes its own material.
   (Shipped: [takes are declared state](../../implemented/architecture/2026-09-27-takes-are-declared-state.md).
   Device identity is the rig's, declared by `source add` per the capture note — not a property of
   every take.)
5. A session whose only material is a recorded take masters and exports, with the render length
   still **derived from the value** — `end_frame()`'s invariant — extended to see the session's
   material and not only its clips.
6. `render`/`render_with_drain` work with the arranger plugin unmounted. *(Optional: this is the
   timeline-free-build goal, not a recorder requirement.)*
7. MIDI clock out and an alignment/offset model exist and are measured on captured material, or
   are listed as the recorder's remaining gaps — never assumed done.
8. Overdub works: recording again over playback yields another take, and every take has a
   placement (origin + length) the render length can see.
9. The two-instance pattern is exercised once end to end: an arranger instance's output declared
   as a recorder source by name, captured alongside the hardware sources with no summing.

## Risks

- **The recorder is not the small scope.** The [capture-topology note](2026-09-25-capture-topology-aligned-stems.md)
  is unbounded if allowed to be, by its own admission. Mitigation: the seam is the deliverable —
  `source` declaration plus `source check` — and per-device coverage is not.
- **The unproven corner hardens.** If structural live editing goes unexercised for a long stretch,
  the substrate's hardest invariant rots behind a green build. Mitigation: the honesty list above,
  plus keeping the arranger's tests in the always-on set as the canary even while its product is
  deferred.
- **Profile-abstraction creep.** "Profile" quietly becomes a loader/config framework. Mitigation:
  the budget rule — name, op set, mount set, and nothing else until a third provider demands more.
- **Rename churn at the wrong moment.** Prose-only today; the longer it waits, the more prose
  there is. Mitigation: decide here, execute at the tag.
- **Recorder-first starves the arranger.** Mitigation: the written trigger above, and the
  acceptance criteria naming the arranger's corner explicitly.
- **The timeline is the take registry, chosen deliberately.** Giving each take a placement re-uses
  the arranger's model, so the recorder renders through the arranger node and takes and clips are
  one representation in the document. Accepted: it buys the derived length, record → arrange
  continuity, and zero new concepts — and it is why criterion 6 is optional rather than required.
  The cost: overdub stacks takes as clips rather than as a distinct take list. If that ever grates,
  it is the signal to split the representations, not a bug to patch.

*Authored with DeepSeek-V4.1-Flash · DeepSeek Harness, 2026-09-27.*

*Amended with DeepSeek-V4.1-Flash · DeepSeek Harness, 2026-10-08: the four levels (core, plugin,
profile, rig), the "the platform is never the process" rule, "profiles stay coarse — the small
units are plugins", and the rig/studio as the end goal. The owner's decisions; no prior decision
is rewritten.*
