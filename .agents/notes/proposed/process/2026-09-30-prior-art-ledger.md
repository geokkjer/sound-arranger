# Agent Note: A prior-art ledger, kept separate from the licence record

Status: proposed

## Problem

The repo has accumulated substantial prior-art research — nine documents in
[research/architecture/](../../../../research/architecture/) covering Ardour and Audacity, Helix's
keyboard model, terminal audio tools, Ableton's Session/Arrangement split, FFmpeg, Faust, the
Synclavier and Autechre — plus a two-document Bol Processor study in
[research/prior-art/](../../../../research/prior-art/). Not one of them is indexed. Nothing in the
tree distinguishes *prior art* from the ~50 model reviews and design studies that share the same
directory, so the inventory cannot be read off the repo.

Three concrete costs follow:

- **Duplicate surveys.** Without an index, the next design question re-derives what the Ardour and
  Audacity pass already answered in August.
- **"Not yet researched" is indistinguishable from "researched and rejected."** The Ableton pass
  records "borrowed elements and why"; nothing records a search that found nothing, so a gap and a
  negative result look identical.
- **Licence facts are scattered in prose.** The [TUI prior-art doc](../../../../research/architecture/2026-09-21-tui-audio-prior-art.md)
  notes a peer as "Rust, MIT" inside a comparison table; elsewhere a peer is named with no licence
  at all. There is no place to answer "may we copy from this?" before copying.

This became urgent after surveying [n1m21n/Infinite](https://github.com/n1m21n/Infinite), the only
purpose-built prior-art ledger found in the field. Building this ledger surfaced three live licence
traps in projects we already cite — see [Risks](#risks).

## Proposal

**Adopt a thin, index-shaped prior-art ledger at [research/prior-art/ledger.md](../../../../research/prior-art/ledger.md),
one row per transferable idea, and keep it deliberately separate from the licence record.**

1. **Ideas get a ledger; licences get a notices file.** Two audiences, two artefacts — the field's
   own practice separates them (Cardinal's hand-written `DIFFERENCES.md` beside its generated
   `LICENSES.md`; Blender's hand-written History beside its generated Credits; `tui-wave`'s
   `THIRD_PARTY_NOTICES.md`). The coupling is exactly **one column**, `What we took`: rows marked
   `adapted` or `verbatim` must also appear in `THIRD_PARTY_NOTICES.md`; rows marked
   `read-only (concept)` never touch it. Merging them would require a linter to parse prose and
   would put unverified speculation into a legally load-bearing document.

2. **The ledger is an index, not a second copy of the research.** Each row links to the analysis
   that already exists. Transcribing nine good documents into a table would duplicate them and rot
   immediately. This also means the ledger points at *shipped* reality — an Agent Note or a source
   path — rather than at a plan.

3. **Every row carries a pinned revision and a licence expression read from the source header.**
   `Rev read` (commit/tag/DOI) and `Licence (SPDX)` including `-only`/`-or-later`. A bare forge tag
   is not evidence: Ardour, LMMS and OBS are all tagged `GPL-2.0` but are `GPL-2.0-or-later` in
   their headers, which is the difference between copyable and not. This is not hypothetical — while
   writing this note a licence probe returned a false "no licence found" for `tui-wave` purely
   because the wrong default branch was guessed.

4. **Negative results are entries.** A `## Deliberate gaps` table records the search, the scope and
   the result, so an unresearched area is visibly a gap rather than silently absent. The ledger's
   first gap is substantial: 36 candidate filenames probed across 17 FOSS audio/creative projects
   found **no** purpose-built prior-art ledger anywhere.

5. **Our GPL-3.0-or-later licence inverts their clean-room premise.** Infinite's ledger exists to
   keep its authors *out* of copyleft source. Ours exists to record what we absorbed and to tell us
   what we may lawfully copy. The ledger therefore contains no prohibition — only a per-row
   copyability rule.

6. **The ledger is triggered by the workflow we already have, not by an audit.** Our standing order
   is that every non-trivial change ships an Agent Note in the same commit, gated by
   `scripts/verify-agent-notes.mjs` from the pre-commit hook. That gate already resolves every
   markdown cross-reference, so a ledger of links is structurally verified for free. A periodic
   audit would rot; a step we already take will not.

## Alternatives considered

- **Copy n1m21n/Infinite's ledger shape verbatim** — rejected: their columns are good (one row per
  idea, `Similarity — why`, `Confidence`, a documented "searched, found nothing"), but the artefact
  is shaped by an MIT clean-room rule we do not have — it has no licence column at all, which is the
  single gap their own skill file exposes ("Always note each source's licence" with nowhere to put
  it). We take the shape and invert the premise.
- **One document covering ideas and licences** — rejected: it forces a linter to parse narrative
  prose and puts confidence levels inside a document that decides redistribution duties. Cardinal,
  the closest audio peer, already made this split; merging it re-litigates a settled question.
- **Transcribe all nine prior-art documents into ledger rows** — rejected as the wrong first move:
  it duplicates existing analysis, and the rows would be written from summaries rather than from the
  sources. Rows are backfilled as each document is actually re-read; the nine are indexed today.
- **Move the nine prior-art documents into `research/prior-art/`** — rejected for now: it is a
  larger change (nine file moves plus every inbound link in `RESEARCH.md` and the notes tree) than
  the legibility problem justifies. The ledger indexes them where they are; recorded as an open
  question instead.
- **Generate the ledger from a machine-readable source** — rejected: the field's lesson is to derive
  what a tool can derive and never hand-maintain it, but idea provenance has no derivation source.
  There is no standard for it — `CITATION.cff` has no "influenced by" predicate, SPDX's
  `RelationshipType` describes artifacts rather than concepts, and CycloneDX `pedigree` traces
  binaries. The licence half *is* derivable and should be generated; the ideas half is not.
- **Add a `Prior-art: PA-0007` commit trailer now** — deferred: attractive because a trailer cannot
  go stale (it ships with the patch, like our existing `Assisted-by:`), but our AGENTS.md warns
  against over-building process, and the Agent Note link covers the need today.
- **Add a `verify-prior-art.mjs` gate now** — deferred: the checks worth having (ids unique, landing
  paths exist, `Checked` and `Revisit if` present, copied rows appear in the notices file) mostly
  apply to a ledger with copied rows in it, and every row today is `read-only`. Revisit when the
  first `adapted` row appears.

## Acceptance criteria

- `research/prior-art/ledger.md` exists, and `research/prior-art/README.md` states what the directory
  is for, where the nine analysis documents live, and how the two-document split works.
- Every ledger row carries `ID`, `Idea`, `Source`, `Rev read`, `Licence (SPDX)`, `What we took`,
  `Status`, `Confidence`, `Checked` and `Revisit if`.
- The ledger contains at least one `## Deliberate gaps` entry with the search actually performed.
- Every relative link in the ledger and the README resolves — verified by
  `node scripts/verify-agent-notes.mjs` for the notes tree, and by hand for `research/` (which the
  gate does not currently walk).
- The three licence traps found are recorded in the ledger's traps table.
- `RESEARCH.md` links the ledger as the prior-art entry point, so a reader starting from the research
  draft finds it.

## Risks

- **The ledger rots into a bookmark dump.** Mitigated by the `Where it landed (ours)` column: a row
  that never lands is a row to delete. The `Checked` and `Revisit if` columns make staleness
  visible rather than assumed absent.
- **Three licence traps were found in projects we already cite, which means our existing research
  assumed licences it had not verified.** [Zrythm](../../../../research/architecture/2026-09-21-tui-audio-prior-art.md)
  is not plain AGPL: it is `AGPL-3.0-or-later` **plus additional §7 terms** under the non-SPDX
  `LicenseRef-ZrythmLicense`, and copying from it would force the whole combined work to AGPL.
  VCV Rack is `GPL-3.0-or-later` **plus** a non-commercial plugin exception. Ardour, LMMS and OBS
  are `-or-later` and therefore copyable. None of this was written down before; the ledger is the
  fix, but the finding is that our prior-art passes have been recording licences informally.
- **The `Rev read` column will go stale faster than the rest.** Pinning a branch rather than a
  commit is the failure mode; the checks recorded were verified on 2026-09-30 against the listed
  refs. A future re-read should prefer a commit SHA or tag.
- **Scope creep into non-software material.** Musical lineage (Feldman, dub, Macero, musique
  concrète) and gear/hardware belong to the music-theory and studio projects per
  [AGENTS.md](../../../../AGENTS.md). The ledger's `## Out of scope` section states this, because a
  provenance ledger is exactly the artefact that attracts everything with a citation.
- **The two-document split has a real trigger that has not fired.** `THIRD_PARTY_NOTICES.md` does
  not exist yet. The split is proven in the field but unexercised here; if the first copied code
  arrives without a notices file, the coupling column becomes a dead field.
