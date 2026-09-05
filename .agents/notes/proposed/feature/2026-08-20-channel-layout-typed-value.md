# Agent Note: Channel layout as a typed value — before `channels: 1..=8` hardens in the log

Status: proposed

## Problem

The mixer's channel model is a bare **count**: a `channels` mount param, `1..=MIXER_CHANNELS_MAX` (8), default 4, that "the profile sets from the input device's layout" ([`mixer.rs`](../../../../crates/engine/src/plugins/mixer.rs)) — e.g. the Notepad-12FX's 4 USB capture channels or the Scarlett 2i2's 2. A count cannot express the actual target layout: the Notepad-12FX's four channels are **2 mono + 1 stereo pair** — a mixed layout with roles, not "four equal channels." The mixer already shows the symptom: *"Patches to channels beyond the mounted `channels` count are accepted but ignored (documented)"* — a silent behavior, the opposite of the patch bay's fail-loud rule.

FFmpeg's history is the warning: the `uint64_t` bitmask + count model conflated "how many channels" with "what the channels are," and replacing it was a **281-patch retrofit** (2022 `AVChannelLayout` rework) whose stated principle is *"for decoders that do not set a channel layout, lavc will not make one up for them"* — inventing format facts was a decade of wrongness ([ffmpeg research](../../../../research/architecture/2026-08-20-ffmpeg-design-knowledge.md)). The log schema freezes in Phase 1; a channel model decided later is a schema migration.

## Proposal

Introduce a **`ChannelLayout` value** for the mixer (and any future multichannel node) *before* the mount-log schema hardens:

- A small ordered list of **named roles**: `Mono`, `Left`, `Right`, `StereoL`, `StereoR`, … — the shape of `AVChannelLayout`'s idea, not its API surface. A count stays available as a derived convenience (`layout.len()`).
- The mixer's mount event carries the layout; the profile sets it from the capture device's reported roles. The Soundcraft 4-capture becomes `[Mono1, Mono2, StereoL, StereoR]`; the Scarlett 2i2 becomes `[Left, Right]` — both currently indistinguishable as "4" / "2".
- **Never invent a layout:** a source that doesn't declare one is refused loudly at patch time (fail-loud, never logged — the patch-bay rule), or explicitly marked `unattributed` where a real device cannot report roles. This replaces the "accepted but ignored" silent path.
- Roles index the mixer's channel strip; meter banks and gain/mute/solo key off the same roles so a layout change re-labels, not re-numbers, the UI.

## Alternatives considered

- **Keep the count, document the order (status quo)** — cheapest today, but it is already leaking (silent ignore of out-of-range patches) and cannot express the mixed 2-mono+stereo-pair layout that is our actual Phase-1 capture target. Rejected: the count is a schema field hardening right now.
- **Adopt ffmpeg's `AVChannelLayout` wholesale** — the full API (custom names, opaque pointers, describe/parse) is far more than we need and would drag a C-shaped API into the log model. Rejected: take the *shape* (named, ordered, never-invented), not the surface.
- **Per-channel metadata table (role + name + routing per channel)** — heavier than needed; the ordered-role list covers capture and mixing, and richer metadata can be layered later without a schema break. Rejected for now.
- **Defer until a node needs it** — the risk is precisely that Phase 1 hardens `channels` as a bare count and the retrofit cost lands later (FFmpeg's 281 patches). Rejected: this is the cheapest moment.

## Acceptance criteria

- The mixer mount log event carries a `ChannelLayout`; `channels` remains a derived count.
- The Notepad-12FX 4-capture renders as 2 mono + 1 stereo pair; the Scarlett 2i2 as a stereo pair — both logged with roles, not just counts.
- A source patched to an undeclared role is refused loudly and never logged; no "accepted but ignored" path remains.
- A device that cannot report roles is marked `unattributed` explicitly — never a guessed layout.
- Existing mixer behavior (gain/mute/solo/meters, `MIXER_CHANNELS_MAX`) is preserved; the change is additive to the log schema, landed before Phase-1 schema freeze.

## Risks

- **Device drivers reporting conflicting or absent layouts** — validate at capture time and fall back to `unattributed` + explicit user confirmation; never guess.
- **Scope creep toward full multichannel/surround** — the ordered-role list is the floor, not the ceiling; surround and custom speaker mappings are deferred, not designed here.
- **Schema churn if it slips past the freeze** — that is the point of doing it now; after freeze, this becomes a migration, which is the cost the note exists to avoid.
