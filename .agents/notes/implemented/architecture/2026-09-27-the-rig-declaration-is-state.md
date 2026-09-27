# Agent Note: The rig declaration is state; the binding is not

Status: implemented

## Problem

A session could record a take but had no way to say **what it was recording from**. The recorder
needs a declared rig — which sources exist, how wide each is, and what each does about the clock —
because capture, alignment and the coming clock-out all need addresses to work with. The
[capture-topology note](../proposed/architecture/2026-09-25-capture-topology-aligned-stems.md) had
already fixed the *shape* of a source (identity, channels, clock role, binding rule) and the *form*
(`source add` is a logged `host v1` command); what did not exist was the type, the command and the
log/replay contract.

The contract is the whole point, and it is the takes slice's
([takes are declared state](2026-09-27-takes-are-declared-state.md)) rule applied to the rig: **a
declaration is state; touching a device is a side effect.** Get that backwards and a session stops
being replayable on a machine that has different gear plugged in — or none.

## Decision

`crates/host/src/rig.rs` holds the shape, and `HostCommand::SourceAdd` carries it:

```text
source add synth kind=alsa match=hw:USB channels=8 clock=master
```

- **It is state.** `SourceAdd` is in `HostCommand::is_state()`, so `process` commits it through the
  same `commit_state` every other state change uses: it reaches the history and the autosave
  journal, `session.txt` carries it, and a rebuild re-declares the rig. No arm on that path opens,
  binds or probes a device.
- **Identity is a name plus a rule, never an index.** `SourceDecl { name, kind, matcher, channels,
  clock }` — an index renumbers across reboots and replugs, so a session that named one would
  replay against the wrong device or none.
- **`kind` is closed** (`SOURCE_KINDS = alsa | jack | osc`), looked up like the host's own name
  registry: an unknown kind names the registry instead of becoming a silently inert declaration.
- **The clock role is declared here** (`master` / `follower` / `free`, parsed loud and with no
  default), which is what the clock-out slice binds to: a `Follower` is a source we send clock to.
- **Refusals are loud and never logged.** A name that is not spellable in the whitespace log
  (reusing `media::valid_name`, the discipline a take id gets), a duplicate name (an address is an
  address; a second declaration of one is a bug, not a merge), an empty matcher, and a width outside
  `1..=CAPTURE_CHANNELS_SANITY` all `return Err` from the arm — so `process` never reaches its
  commit and the refusal stays out of the document.
- **`HostSession::sources()`** exposes the declarations, the way `mixer_channels()` and
  `source_tempos()` are exposed, so a shell can render the rig without knowing the log's spelling.

## Alternatives considered

- **Make the declaration an action that re-binds on every replay.** Rejected: replay would then
  need the hardware, which is exactly what the purity rule forbids — and a seek rebuilds the
  session, so it would fail on any machine without the rig attached.
- **Keep the rig in a separate profile file (TOML/JSON) beside `session.txt`.** Rejected: the
  topology note already decided the log is the document, and two formats for one truth drift apart.
  A profile *file* can still be a convenience that emits these lines later; it must not be a second
  source of truth.
- **Address sources by device index.** Rejected by the topology note, and by reality: indices
  renumber.
- **Accept any `kind` string.** Rejected: an open list turns a typo into a declaration that looks
  fine and binds nothing — the failure mode this project keeps choosing to make loud.
- **Extend the tokenizer to quoted matchers in this slice.** Deferred, not rejected: a matcher
  containing a space cannot be expressed yet, and growing the tokenizer is a parser change that
  deserves its own decision rather than a ride along.

## Consequences

- A session now carries its rig in the document: `session.txt` gets one `source add` line per
  source, a seek rebuilds it, and the autosave journal has it per gesture.
- The clock-out slice has a destination and does not need to invent one: `clock=follower` is the
  set of sources to drive, and `clock=master` is the one the session follows.
- The binding slice can resolve `matcher` to a real device without touching the declaration —
  which is why `matcher` is a *rule* rather than a resolved handle.
- **Honest limitation:** a matcher is one token, so a device name containing a space cannot be
  declared yet; such a line is refused loudly rather than truncated, and the comment on the parse
  arm says where quoted matchers would come from.
- The known flake `a_take_records_into_the_pool_and_plays` (already in the capabilities list) was
  re-observed during this work, including once on an unmodified tree — pre-existing, unrelated, and
  left to its own note.
- The implementation of this slice came back from a scoped coding handoff (GLM-5.3-Flash) and was
  reviewed against the code rather than the report: the state path was verified by tracing
  `process`'s central `commit_state` and confirming that a refusing arm returns before it.

*Authored with DeepSeek-V4.1-Flash · DeepSeek Harness, 2026-09-27.*
