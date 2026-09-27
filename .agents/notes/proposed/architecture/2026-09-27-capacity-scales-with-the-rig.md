# Agent Note: Capacity scales with the rig — demand-sized tracks and an equipment profile

Status: proposed

## Problem

The platform knows how wide a session is, at compile time, and it is wrong about it.

- `MIXER_CHANNELS_MAX = 8`, `engine::MAX_AUDIO_INS = 8` ([`graph.rs`](../../../../crates/engine/src/graph.rs)) and a third `1..=8` clamp in `media/capture.rs` all assert the same ceiling — and `MAX_AUDIO_INS` is not a validation bound but the **array dimension in the render path** (`audio_ins: [&[f32]; MAX_AUDIO_INS]`, in `graph.rs`, `stream.rs` and `arranger.rs`).
- The surface that implements it is hand-written: `MIXER_PORTS` is eight literal `ch0…ch7` ports and `MIXER_PARAMS` four params per channel, all `&'static`, behind `Plugin::ports() -> &'static [Port]` / `params() -> &'static [ParamDef]` with `ParamDef.name: &'static str`
  ([`mod.rs`](../../../../crates/engine/src/plugins/mod.rs), [`mixer.rs`](../../../../crates/engine/src/plugins/mixer.rs)). Raising the number therefore means more literal surface, not less; and *removing* the number means a different seam.

The recorder's actual workflow is **additive dubbing**: play one part, layer the next, "only two hands". Lanes grow with passes, not with instruments, so eight is reached early — and the ceiling stops being a tuning knob and becomes the product's limit. It is also a **device-shaped** assumption: the platform's other half is software sources (VCV, SuperDirt, Tidal, our own second instance), which have no device to report a channel count.

The layout half of this is already open, and already argued: the
[channel-layout note](../feature/2026-08-20-channel-layout-typed-value.md) shows a count cannot express
roles (the Notepad-12FX is **2 mono + 1 stereo pair**), cites FFmpeg's 281-patch `AVChannelLayout`
retrofit as the precedent, and warns the schema hardens now. Its diagnosis stands; only its
comfortable ceiling does not.

## Proposal

**Delete the fixed ceiling. Size everything from the rig, declared by an equipment profile.**

- **Capacity scales with demand.** Node input capacity is **preallocated when a node is added** and read as a slice by the render path — so the realtime invariant (no allocation in render) is unchanged, while the compile-time constant disappears. Tracks, mixer channels and lanes all follow the mounted rig.
- **The plugin surface belongs to the instance.** `ports()`/`params()` return borrows from the plugin (`&[Port]` / `&[ParamDef]`, owned or interned names) so a plugin can declare a surface *derived from its mount params*. `SetParam` still refuses an undeclared name — the fail-loud rule is preserved, and it becomes the mechanism that replaces today's silent path, where a patch beyond the mounted count is "accepted but ignored".
- **The layout lives in an equipment profile.** One artifact declares a rig: its sources, each source's **channel roles** (an ordered `Mono`/`StereoL`/`StereoR`/… list, per the channel-layout note), its clock role and its binding rule. The mixer's mount carries the layout; `channels` becomes a derived count. This is the same shape the [capture-topology note](2026-09-25-capture-topology-aligned-stems.md) gives a source (*a logged declaration, not UI state*) and the same owner as the [input/jam-layer registry](2026-09-01-input-jam-layer-device-registry-io.md).
- **Device data is not ours to hold.** Which box is 2 mono + 1 stereo pair is gear research: it lives in the studio project
  (`../../../../../music/music-composition-theory/studio/instruments/`), **linked, not copied**, per the separation-of-concerns rule. This repo owns the *profile type*, not the catalog.
- **Never invent a layout.** A source that cannot report roles is marked `unattributed` and confirmed explicitly; a patch to an undeclared role is refused and never logged.

### What this means for the recorder's next slice

Additive placement assigns fresh lanes per pass; **strict overdub** reuses the source's lane (the punch-in case). Nothing in the code bounds how many passes a session holds except memory and the UI's ability to show them — which is a rendering question to measure when a real session exceeds a screenful, not a model question.

## Alternatives considered

- **Raise the fixed ceiling (8 → 32).** The choice made earlier and reversed on better information: it buys headroom without removing the limit, grows a hand-written surface (or needs a macro to fake one), and answers "how wide can a session be?" with a second guess. Rejected.
- **Keep the count and defer the layout** (the status quo the channel-layout note rejects). Rejected again, for its own reason: the schema hardens now, and a count cannot express roles at any number.
- **A device-shaped model** (a fixed catalog of interfaces and their layouts). Rejected: our own software sources have no device, and the same layout concept already describes them — a SuperDirt output is a stereo pair, a second instance of ourselves declares its own.
- **Let lanes exceed the mixer by summing off-mixer.** Rejected: it hides per-pass balance, which is precisely what additive dubbing is for.
- **One multi-channel port instead of per-channel ports.** Compatible with this direction and cheaper in surface, but it decides the *port* model (interleaved vs per-channel) well beyond what capacity needs. Recorded as the likely follow-on, for when roles must carry through the graph rather than being flattened into mono lanes.
- **Cap the profile instead of the code** (a "max 8 sources" rule in the profile format). Rejected: it moves the arbitrary limit into user data, where it is harder to raise and still a guess.

## Acceptance criteria

1. No compile-time channel ceiling: `MAX_AUDIO_INS` and `MIXER_CHANNELS_MAX` are gone, or kept only as a named sanity bound with a stated reason — never as the array dimension in the render path.
2. The mixer's port and param surface is derived from the mounted layout, with no hand-written channel list; `SetParam` still refuses an undeclared name.
3. The render path allocates nothing: per-node capacity is preallocated when the node is added, and the counting-allocator test still passes.
4. An equipment profile declares a rig — sources, per-source roles, clock role, binding rule — and a session logs it and replays it byte-identically.
5. A patch to an undeclared role is refused and never logged; a source that cannot report roles is `unattributed`, never guessed.
6. Additive placement of *N* passes × *M* channels mounts and renders for every *N* the profile declared — measured on a session that would have exceeded the old ceiling, not assumed.
7. Device-specific layout data appears in the studio project and is linked here, never copied.

## Risks

- **Scope: this is a core-seam change.** It touches the plugin trait, the graph's node capacity and the mount-log shape. Mitigation: change the *surface and capacity* only — no plugin-ABI redesign, no plugin discovery, nothing the recorder does not need.
- **The UI meets the ceiling next.** *N* strips is a rendering question the TUI and iced shells will hit; measure when a real session exceeds a screenful rather than designing for 64 now.
- **Format timing.** The mount event gains a layout while `host v1` is young; after the first tag this is a migration — the channel-layout note's warning, now with a deadline it did not have.
- **Profiles becoming a device database.** The profile is a *rig declaration*; per-device data is studio content. Keep the file thin.
- **Silent regressions in the loud path.** Replacing "accepted but ignored" with a refusal will break any existing script that patched past the mounted count; that is the point, but it belongs in the same change as the loud rule so the breakage is explained rather than discovered.
