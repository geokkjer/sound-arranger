# Prior-art ledger — where our ideas came from

> **Status:** living document. Ideas first, licence second. One row per *transferable idea*, not
> per project and not per file. This is the human, epistemic half of a deliberate two-document
> split — the machine-readable licence half is `THIRD_PARTY_NOTICES.md`, which only ever lists
> code we actually copied. Nothing in this ledger is copied code: every row is `read-only (concept)`.

## Why this exists, and why it is not their ledger

The n1m21n/Infinite ledger ([docs/prior-art](https://github.com/n1m21n/Infinite)) is the only
purpose-built prior-art ledger found in the field — probing 36 candidate filenames across
Ardour, LMMS, Tracktion, Carla, VCV Rack, Cardinal, BespokeSynth, SuperCollider, Csound,
Pure Data, Audacity, MuseScore, OBS, Blender, Krita, Zrythm and Inkscape returned **zero**
`IDEAS` / `INSPIRATION` / `related-work` / `design-rationale` files. The closest analogue is
Cardinal's hand-written
[`docs/DIFFERENCES.md`](https://github.com/DISTRHO/Cardinal/blob/main/docs/DIFFERENCES.md)
beside its *generated* `docs/LICENSES.md`.

We took their **shape** and inverted their **premise**. Their ledger is built on an MIT
clean-room rule — copyleft peers are "discussions only, never open the source". This project is
**GPL-3.0-or-later**, which makes copyleft peers *readable and adaptable*. So the ledger is not
a fence; it is a record of what we absorbed, and its licence column exists to tell us what we
may lawfully copy, not to keep us out.

## How to read the columns

| Column | Meaning |
|---|---|
| `ID` | Stable, never reused. Addressable from an Agent Note as `PA-0007`. |
| `Idea` | The transferable concept in one line — not the feature name. |
| `Source` | Project, document, or paper we actually read. |
| `Rev read` | The exact revision consulted (commit, tag, branch, or DOI). **Pin this** — an unpinned source is how a ledger starts lying. |
| `Licence (SPDX)` | From the **source file header**, not the forge tag, and including `-only`/`-or-later`. This is what decides copyability. |
| `Similarity — why` | What makes it comparable to our problem. |
| `What we took` | `read-only (concept)` \| `adapted` \| `verbatim`. Anything not `read-only` must also appear in `THIRD_PARTY_NOTICES.md`. |
| `Where it landed (ours)` | Repo path or Agent Note, or `—`. A row that never lands is a row that should be deleted. |
| `Status` | Ours: `adopted \| planned \| watching \| rejected`. Theirs: `solved \| open \| workaround`. |
| `Confidence` | `high` (read source/diff) \| `medium` (docs/thread) \| `low` (inference). |
| `Checked` | `YYYY-MM-DD · who`. Staleness should be visible, never assumed absent. |
| `Revisit if` | The concrete trigger that should reopen the decision. |

## Licence → copyability

Recorded per row, because the same project can carry different expressions per file.

| SPDX expression | May we copy code in? |
|---|---|
| `MIT`, `Apache-2.0`, `BSD-*`, `ISC`, `Zlib`, public domain | Yes, with attribution. |
| `GPL-2.0-or-later`, `GPL-3.0-or-later`, `LGPL-*` | Yes — compatible with our GPL-3.0-or-later. LGPL needs care if statically linked. |
| `GPL-2.0-only`, `GPL-3.0-only` | **No.** No "or later" means no upgrade path into GPLv3. |
| `AGPL-3.0-*` | **No.** Network copyleft would force the whole combined work to AGPL. |
| `LicenseRef-*` (non-SPDX) | **Stop.** Read the text: it is usually a standard licence *plus extra terms*. |
| Unknown / no licence file | **No.** Absence of a licence is not permission. |

## The ledger

### Data model and engine

| ID | Idea | Source | Rev read | Licence (SPDX) | Similarity — why | What we took | Where it landed (ours) | Status | Confidence | Checked | Revisit if |
|---|---|---|---|---|---|---|---|---|---|---|---|
| PA-0001 | Immutable sources; clips are `{path, offset, length}` ranges — splitting/trimming never touches audio files | [Ardour design knowledge](../architecture/2026-08-18-ardour-design-knowledge.md) | manual + ardour-dev archives, 2026-08-18 survey | `GPL-2.0-or-later` (verified `libs/ardour/session.cc:16-18`) | Our ACID model is exactly this shape, and it is 20 years shipped | `read-only (concept)` — the non-destructive model is independently ours | [minimal-core note](../../.agents/notes/proposed/architecture/2026-08-15-minimal-core-clock-graph-session-log.md) | theirs `solved` · ours `adopted` | high | 2026-09-30 · lead | Never — foundational |
| PA-0002 | Waveform peaks: min/max per 256-sample reduction, per-pixel pyramid on demand, background-computed with striped placeholders | [Audacity design knowledge](../architecture/2026-08-18-audacity-design-knowledge.md) | CMJ 2002 paper, AOSA chapter, archived wiki | `GPL-2.0-or-later` (Audacity) | Phase-1 "live waveform peaks" is the same problem | `read-only (concept)` — adopt the proven bin size and caching shape | [Ardour/Audacity prior art](../architecture/2026-08-18-ardour-audacity-prior-art.md) decision candidates | theirs `solved` · ours `planned` | high | 2026-09-30 · lead | If a binned RMS/peak sidecar proves insufficient at extreme zoom |
| PA-0003 | Layered regions with arbitrary overlap; **each region carries its own end crossfades** — a crossfade is not an entity *between* two regions | [Ardour design knowledge](../architecture/2026-08-18-ardour-design-knowledge.md) | Ardour manual §region fades | `GPL-2.0-or-later` | Generalises our sequential splice to a stack | `read-only (concept)` | [Ardour/Audacity prior art](../architecture/2026-08-18-ardour-audacity-prior-art.md) decision candidates | theirs `solved` · ours `planned` | high | 2026-09-30 · lead | When the mixer grows multi-clip stacking |
| PA-0004 | **PDC is a retrofit horror story**: designed 2005, still incomplete in 2018 ("full latency compensation, everywhere" as a 6.0 goal) | [Ardour design knowledge](../architecture/2026-08-18-ardour-design-knowledge.md) | ardour-dev 2005-07, Davis interview 2018 | `GPL-2.0-or-later` | The single strongest validation of PDC-from-day-one | `read-only (concept)` — a warning, not a mechanism | [Spike A core note](../../.agents/notes/implemented/architecture/2026-08-15-phase-0-spike-a-core.md) | theirs `open` · ours `adopted` | high | 2026-09-30 · lead | If we ever add a plugin with unreported latency |
| PA-0005 | **Never convert musical↔audio time repeatedly**; a duration needs an associated position; a high-resolution "superclock" does all tempo-map math | [Ardour design knowledge](../architecture/2026-08-18-ardour-design-knowledge.md) | Davis interview 2018 (nutempo) | `GPL-2.0-or-later` | Our absolute-frame clock is the same idea; the failure mode is sub-sample drift | `read-only (concept)` | [minimal-core note](../../.agents/notes/proposed/architecture/2026-08-15-minimal-core-clock-graph-session-log.md) | theirs `solved` · ours `adopted` | high | 2026-09-30 · lead | When ramped tempo / music-locked objects arrive |
| PA-0006 | Undo history persisted to disk, surviving restart; undo scope deliberately **split per view**; named snapshots as frozen versions | [Ardour design knowledge](../architecture/2026-08-18-ardour-design-knowledge.md) | Ardour manual §undo, §snapshots | `GPL-2.0-or-later` | We have log-based undo; they have the *semantics* worth matching | `read-only (concept)` | [minimal-core note](../../.agents/notes/proposed/architecture/2026-08-15-minimal-core-clock-graph-session-log.md) | theirs `solved` · ours `planned` | high | 2026-09-30 · lead | When scoped or per-view undo is designed |
| PA-0007 | **Reject the destructive block store.** Audacity's block files existed to make destructive edits cheap; their own main regret is that blocks were "exposed to end users" | [Audacity design knowledge](../architecture/2026-08-18-audacity-design-knowledge.md) | CMJ 2002, AOSA, 3.0.0 changelog | `GPL-2.0-or-later` | We are non-destructive by design, so the whole apparatus is unnecessary | `read-only (concept)` — a rejected path | [Ardour/Audacity prior art](../architecture/2026-08-18-ardour-audacity-prior-art.md) | theirs `workaround` · ours `rejected` | high | 2026-09-30 · lead | Never — the opposite of our model |
| PA-0008 | Two-tier undo: a full graph snapshot **or** a scoped snapshot that skips respawning the graph — because restoring the full document reset node state, dropped audio and rebuilt every FBO | n1m21n/Infinite `main.cpp` (`UndoEntry`, `arrangeOnly`) | `main` @ `c4a9c5810`, 2026-09-30 | `MIT` | Directly analogous to "compound gestures are one undo step", implemented by snapshot instead of log | `read-only (concept)` — we keep the log fold, not their mechanism | — | theirs `workaround` · ours `watching` | medium (read report, not the diff) | 2026-09-30 · lead | If log-replay undo ever makes a gesture cost O(document) |
| PA-0009 | Stable `uid` distinct from the reused array `index`, so long-lived references survive delete, undo and reload | n1m21n/Infinite `GraphNode.h:47-53` | `main` @ `c4a9c5810`, 2026-09-30 | `MIT` | Our takes/patchbay/rig are declared state referenced across log entries | `read-only (concept)` | — | theirs `solved` · ours `watching` | medium | 2026-09-30 · lead | If log entry identity proves ambiguous across compaction |

### Shells and interaction

| ID | Idea | Source | Rev read | Licence (SPDX) | Similarity — why | What we took | Where it landed (ours) | Status | Confidence | Checked | Revisit if |
|---|---|---|---|---|---|---|---|---|---|---|---|
| PA-0010 | Selection-first ("noun then verb"), keymap-as-labelled-data, and deliberate discoverability affordances | [TUI audio prior art](../architecture/2026-09-21-tui-audio-prior-art.md) | Helix docs + keymap source, 2026-09-21 survey | `MPL-2.0` (Helix) | Our shell is TUI-primary and keyboard-first | `read-only (concept)` — the interaction model, not the code | [modal-editing note](../../.agents/notes/proposed/architecture/2026-09-21-modal-editing-model.md) | theirs `solved` · ours `planned` | high | 2026-09-30 · lead | At the first TUI interaction slice |
| PA-0011 | `biomassa/tui-wave` — closest prior art to *any* of our shells: a Rust + ratatui audio editor with real audio behind it | [TUI audio prior art](../architecture/2026-09-21-tui-audio-prior-art.md) | `master` @ 2026-09-30 | `MIT` (`LICENSE:1`) | Nearly our shell stack in a peer project | `read-only (concept)` | — | theirs `solved` · ours `watching` | high | 2026-09-30 · lead | When the TUI shell slice starts |
| PA-0012 | Their `THIRD_PARTY_NOTICES.md` separates **bundled/recompiled** dependencies from merely *invoked* ones (Airwindows statically linked vs CDP/Praat not bundled), and states the obligation that follows | [tui-wave THIRD_PARTY_NOTICES](https://github.com/biomassa/tui-wave/blob/master/THIRD_PARTY_NOTICES.md) | `master` @ 2026-09-30 | `MIT` | We will bundle CDP as a sidecar and link Rust crates — the obligation differs per case | `read-only (concept)` — this is the *model* for our licence half | — | theirs `solved` · ours `planned` | high | 2026-09-30 · lead | When the first code is actually copied or a sidecar binary is distributed |

### Language and integration

| ID | Idea | Source | Rev read | Licence (SPDX) | Similarity — why | What we took | Where it landed (ours) | Status | Confidence | Checked | Revisit if |
|---|---|---|---|---|---|---|---|---|---|---|---|
| PA-0013 | Faust as a **block source behind our `AudioNode`**, never a second graph — because our patch is a loggable, diffable value while a Faust graph is compile-time and non-serializable | [Faust/HISE evaluation](../architecture/2026-09-20-faust-libfaust-and-hise-evaluation.md) | libfaust, measured 2026-09-20 | `LGPL-2.1-or-later` (libfaust); generated code is ours | Same "do not adopt their graph" conclusion we reached for fundsp | `read-only (concept)` | [Faust note](../../.agents/notes/proposed/architecture/2026-09-20-faust-as-optional-dsp-source.md) | theirs `solved` · ours `planned` | high | 2026-09-30 · lead | If the libfaust crate ecosystem changes materially |
| PA-0014 | FFmpeg: **steal the design lessons, not the weight** — decoupled send/receive I/O, flush/drain protocol, format negotiation as the hard part; keep the pool pure Rust and reach for ffmpeg as an `OfflineProcess` sidecar | [FFmpeg design knowledge](../architecture/2026-08-20-ffmpeg-design-knowledge.md) | FFmpeg docs + binding survey 2026-08-20 | `LGPL-2.1-or-later` / `GPL-2.0-or-later` (build-dependent) | Same media-graph problems; embedding cost measured and rejected | `read-only (concept)` | — | theirs `solved` · ours `adopted` | high | 2026-09-30 · lead | If a pure-Rust encoder reaches parity for our formats |
| PA-0015 | Bol Processor: sound-object placement semantics (pivot / cover / gap / relocation, pre/post-roll), rational integer-ratio time with LCM-aware quantization, auto-arrange | [BP4 overview](bp4-overview-document.md) | BP-2.9.8 source study; BP3 public | Not yet recorded — **verify before use** | A formal framework for placing sound-objects in time, aimed at exactly our arranger core | `read-only (concept)` | [BP4 porting plan](bp4-porting-plan.md) | theirs `solved` · ours `planned` | medium | 2026-09-30 · lead | At the arrangement-model slice; also **verify the licence** |

### Documentation cluster (indexed, not yet mined into rows)

The following were read for *design knowledge* and are cited from [RESEARCH.md](../../RESEARCH.md)
and Agent Notes, but have not yet been decomposed into one-row-per-idea entries. They are listed
here so that "not yet mined" is distinguishable from "not researched".

| Document | Subject | Status | Revisit if |
|---|---|---|---|
| [Ableton Live UI prior art](../architecture/2026-09-09-ableton-prior-art-ui.md) | Session vs Arrangement framing; borrowed elements; attribution section | mined as prose, not rows | When the clip/arrangement UI is designed |
| [Synclavier & modular hardware](../architecture/2026-09-01-synclavier-design-inspiration-and-modular-hardware.md) | Software instrument on a general-purpose computer; the patch as a structured value | mined as prose | When patch-as-value is revisited |
| [Autechre generative composition](../architecture/2026-09-01-autechre-generative-composition-inspiration.md) | "The generator replaces the band"; authored rule+process+steering | mined as prose | When Phase-2 generators land |
| [Softsynth integration routes](../architecture/2026-08-19-softsynth-integration-routes.md) | Route A/B/C for third-party synths (CLAP, SuperCollider, Csound) | mined as prose | When a provider route is chosen |

Not a research document but relevant prior-art *use*: the Cardinal-as-CLAP vertical-scaling decision
is an **Agent Note**, not a research pass — [software engine + hardware interface](../../.agents/notes/proposed/architecture/2026-09-03-software-engine-hardware-interface-and-scaling.md).
It is listed here so the ledger does not imply a survey that was never done.

## Deliberate gaps — searched, found nothing

Negative results are results. Each records the search, or it is not a gap.

| Gap | Search | Result | Date |
|---|---|---|---|
| Purpose-built prior-art ledger in a major FOSS audio/creative project | 36 candidate filenames (`CREDITS`, `ACKNOWLEDG*`, `IDEAS`, `INSPIRATION.md`, `docs/prior-art.md`, `docs/related-work.md`, `docs/design-rationale.md`, `CITATION.cff`, `REUSE.toml`, …) across 17 projects: Ardour, LMMS, Tracktion, Carla, VCV Rack, Cardinal, BespokeSynth, SuperCollider, Csound, Pure Data, Audacity, MuseScore, OBS, Blender, Krita, Zrythm, Inkscape | **None.** Only contributor credits, plus Cardinal's `DIFFERENCES.md` and Blender's generated Credits + hand-written History page | 2026-09-30 |
| A standard for recording *idea* provenance | CITATION.cff schema (no "influenced by" predicate — only `references:`), schema.org `isBasedOn` (metadata, not a ledger), SPDX 3.0 `RelationshipType` (`ancestorOf`/`hasVariant` — artifact relationships), CycloneDX `pedigree` (binaries, not concepts) | **No standard exists.** Closest load-bearing precedent is the Rust RFC template's required `## Prior art` section | 2026-09-30 |
| A DAW/TUI audio project we have not yet surveyed | — | **Not yet done.** The 17 above are not exhaustive; Zrythm, Qtractor, Non-DAW, MusE, Waveform, Reaper extension ecosystem remain unread | 2026-09-30 |

## Licence traps found while building this ledger

These are the reason the licence column exists.

| Source | Recorded elsewhere as | Actually is | Consequence |
|---|---|---|---|
| **Zrythm** | "AGPL-3.0" (our [TUI prior-art doc](../architecture/2026-09-21-tui-audio-prior-art.md) lists it as a peer) | `AGPL-3.0-or-later` **plus additional §7 terms**, published as the non-SPDX `LicenseRef-ZrythmLicense` (`LICENSES/LicenseRef-ZrythmLicense.txt`). The extra terms are trademark conditions on *modified* redistribution | **Never copy from Zrythm.** AGPL network copyleft would force the whole combined work to AGPL |
| **VCV Rack** | "GPL-3.0" | `GPL-3.0-or-later` **plus** the "VCV Rack Non-Commercial Plugin License Exception" under §7, plus a commercial option | Copying engine code drags in a non-commercial exception we do not want. Read for design only |
| **Ardour / LMMS / OBS** | forge tags say "GPL-2.0" | Source headers say *"either version 2 … or (at your option) any later version"* → `GPL-2.0-or-later` | Copyable into our GPL-3.0-or-later. A bare `GPL-2.0` forge tag is **not** sufficient evidence in either direction |

## Out of scope

**This ledger is software only.** It records engineering provenance — where mechanisms came from,
under what licence, and where they landed. Anything that is not a software decision is out of
scope here.

- **Musical and creative lineage** — Feldman, dub production, Teo Macero, musique concrète, the
  Composer's Onion framework. These influences are kept **for history, not for engineering**: they
  preserve *why this program exists and what it is trying to be*, not what any module does. They
  live in the music-theory corpus (`~/Projects/music/music-composition-theory`) and are linked from
  [RESEARCH.md](../../RESEARCH.md) §1. The ledger does not duplicate them, and a row here should
  never be opened merely because a source is musically interesting.
- **Gear, hardware and studio material** — per [AGENTS.md](../../AGENTS.md), that is the studio
  project's business, not this repo's.

## Open questions

1. **Who owns the row when a peer changes?** A new upstream release can invalidate an `adopted`
   verdict. `Revisit if` is the hook; nothing yet watches it.
2. **Do we want a `Prior-art: PA-0007` commit trailer**, mirroring the existing `Assisted-by:`
   convention? Attractive because it cannot go stale — it ships with the patch. Deferred: the
   Agent Note link covers it for now, and our AGENTS.md warns against over-building process.
3. **Should `research/architecture/` be split?** Nine of 59 documents are prior art; the rest are
   model reviews and design studies. The ledger indexes them today; a physical move is a larger
   change than the problem currently justifies.
