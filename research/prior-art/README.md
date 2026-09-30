# prior-art/ — what this repo's ideas came from

This directory holds the project's **prior-art record**: where our design ideas came from, what
we took, what we rejected, and what we searched for and did not find.

- **[ledger.md](ledger.md)** — the ledger. One row per transferable *idea* (not per project, not
  per file), with a licence column, a pinned revision, a confidence level, and a landing site.
  Read this before starting research on a design question: it exists so the same project is not
  surveyed twice, and so "not yet researched" is distinguishable from "researched and rejected".
- **[bp4-overview-document.md](bp4-overview-document.md)** — what Bol Processor is: polymetric
  structures, sound-objects, generative grammars.
- **[bp4-porting-plan.md](bp4-porting-plan.md)** — the plan to port those semantics into the
  arranger core (sound-object placement: pivot / cover / gap / relocation, pre/post-roll;
  rational integer-ratio time with LCM-aware quantization; auto-arrange).

The BP documents arrived from the former "Algorithmic composer" study (BP-2.9.8 source + a porting
plan) and are **prior art for the arranger core**; the ledger indexes them as `PA-0015`. The
archived `BP-2.9.8-source.tgz` was deliberately dropped — the source is public and not needed here.

## Where the analysis actually lives

Most prior-art *research* sits in [../architecture/](../architecture/) alongside model reviews and
design studies — nine of that directory's documents are prior art. The ledger indexes them rather
than copying them:

| Document | Subject |
|---|---|
| [Ardour & Audacity](../architecture/2026-08-18-ardour-audacity-prior-art.md) (+ two [design](../architecture/2026-08-18-ardour-design-knowledge.md) [knowledge](../architecture/2026-08-18-audacity-design-knowledge.md) reports) | Media pool, peaks, edit-during-playback, layered clips, click-free transport, undo gestures, PDC |
| [TUI audio](../architecture/2026-09-21-tui-audio-prior-art.md) | Helix's keyboard model, terminal drawing limits, terminal audio tools |
| [Ableton Live UI](../architecture/2026-09-09-ableton-prior-art-ui.md) | Session vs Arrangement framing |
| [FFmpeg](../architecture/2026-08-20-ffmpeg-design-knowledge.md) | Decoupled I/O, flush/drain, format negotiation |
| [Faust & HISE](../architecture/2026-09-20-faust-libfaust-and-hise-evaluation.md) | Block source behind our `AudioNode`, not a second graph |
| [Synclavier](../architecture/2026-09-01-synclavier-design-inspiration-and-modular-hardware.md), [Autechre](../architecture/2026-09-01-autechre-generative-composition-inspiration.md) | Musical lineage and the edit-as-composition thesis |
| [Softsynth integration routes](../architecture/2026-08-19-softsynth-integration-routes.md) | CLAP / SuperCollider / Csound routes |

A physical move of those nine into this directory is possible but rejected for now — see the open
questions in [ledger.md](ledger.md).

## Two documents, two audiences

The ledger deliberately splits the *ideas* from the *licences*, following Cardinal (hand-written
`DIFFERENCES.md` beside generated `LICENSES.md`), Blender (hand-written History beside generated
Credits), and `tui-wave` (`THIRD_PARTY_NOTICES.md` distinguishing bundled from invoked
dependencies):

- **This ledger** — ideas. Human, narrative, epistemic (confidence levels, negative results).
- **`THIRD_PARTY_NOTICES.md`** (repo root, when it exists) — licences. Machine-readable,
  CI-lintable, legally load-bearing, generated rather than typed, and only ever listing code we
  actually copied.

The coupling between them is **one column**: `What we took`. Rows marked `adapted` or `verbatim`
must also appear in the notices file. Rows marked `read-only (concept)` never touch it. Every row
in the ledger today is `read-only`.

## A note on our licence

This project is **GPL-3.0-or-later**. Unlike an MIT project, copyleft peers are *readable and
adaptable* — so this ledger contains no clean-room prohibition. Its licence column records what we
may lawfully copy (`-or-later` and permissive: yes; `-only` and AGPL: no), and the
[traps section](ledger.md#licence-traps-found-while-building-this-ledger) records the ones already
found — including that Zrythm is AGPL with extra §7 terms, and that a bare forge tag of `GPL-2.0`
is not evidence in either direction.
