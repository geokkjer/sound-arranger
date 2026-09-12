# Agent Note: Hub-and-spokes — this repo is the hub, sibling software plugs in on demand, and the LLM collaborates through the Host API text format

Status: proposed

## Problem

Three things are true at once and none of them is written down.

**First, the topology is unstated.** The `umbrella-first` note locks *platform vs. profile* — the
platform is the goal, the clip arranger is its first profile. It says nothing about the
**sibling software**. Tidal (`tidal-lsp`), VCV (`vcv-rack`), CDP and the eventual offline
processors are neither peers nor dependencies nor declared optional, so "does this belong in
the core, in a spoke, or in another repo entirely?" has to be re-argued each time. The
`~/Projects/music` view says where things *live*; nothing says what they *plug into*.

**Second, the practice has been redefined and the docs have not caught up.** The work is
authorial: composition, sonic exploration, sound sculpting — iteration, bursts of writing,
heavy editing. It is explicitly **not** performance; in this framing "live performance" is
hitting play on an audio player. That is a real design constraint being lifted. Latency stops
being a requirement, real-time paths stop being where the value is, and the entire
co-performer framing of the AI literature becomes irrelevant rather than wrong. Nothing in the
repo currently records this, so the next agent reading `audio-latency.md` and the realtime-path
emphasis will reasonably assume latency is a first-class constraint.

**Third, the LLM seam already exists and is documented as something else.** The Host API is a
typed, versioned **text command language** (`host v{N}` first line, refusals on mismatch) whose
commands *are* the session log — "the log is the command list". It is documented as the
conformance seam for interchangeable shells and as the headless smoke binary's input. It is
also, unremarked, the natural interface for a conversational LLM: a text format the model can
read, write, and be corrected on, driving the same deterministic engine the UI drives. That
identity is the whole answer to "how do we hook an LLM in", and it is currently incidental.

## Proposal

### 1. Topology: hub and spokes

- **This repo is the hub.** The engine, media engine, host, shell and the profile definitions
  live here. The hub owns the clock, the graph, the session log, the media pool, and the
  assembly of profiles.
- **Everything else is a spoke, and spokes are optional.** A spoke is any external program or
  repo that either *feeds* the hub (a generator producing material into the pool) or *extends*
  it (an `OfflineProcess` sidecar, a synthesis engine, an instrument). Spokes are pulled in
  **on demand, when a profile needs them** — never as baseline requirements of the core.
- **Spokes are reached across a process boundary, not linked into the core.** VCV Rack, Tidal,
  Csound and CDP stay in their own repos and are driven as external processes or plugin
  formats. The `other-software` zone rule already implies this; this note makes it the
  topology rather than a filing convention.
- **The core stays runnable with zero spokes.** A clean checkout must build, test and bounce
  audio with nothing else installed. This is the property that keeps the hub a hub rather than
  a distribution.

### 2. The collaboration model: author and editor, not performer

- **Latency is not a design constraint.** No part of the creative loop requires a real-time
  response. Batching, thinking, revising and re-running are the normal mode.
- **The unit of collaboration is the session, not the gesture.** What the LLM produces is a
  *command list* — a revisable description of an edit — not a stream of notes.
- **Iteration is the point.** The authorial loop is: describe → generate a command list →
  run it → listen → revise the list. Because a command list is text and the engine is
  deterministic and byte-identical across runs, **revision is cheap and a result is
  reproducible.** That combination is what makes conversational editing viable here and it is
  not available in a DAW.

### 3. The LLM seam is the Host API text format

The interface is the existing versioned text format. No new protocol is required and none
should be invented.

- The LLM writes the same script the CLI and the Tauri bridge already speak. The engine cannot
  tell the difference, which is the point: **the LLM is just another host**, and it inherits
  the determinism, the refusal semantics and the log for free.
- A worked example of the vocabulary as it stands today — **and this exact script was run
  through `target/debug/host` while writing this note, producing audible audio** (see
  [The seam, exercised](#the-seam-exercised) below):

  ```text
  host v1
  mount euclidean steps=8 pulses=1 rotation=1 pulses_per_beat=4 @0
  mount scale root=0 @0
  mount tone gain=0.25 blip_len=1200 @0
  mount mixer channels=4 @0
  patch euclidean.triggers scale.trigger @0
  patch scale.note tone.note @0
  patch tone.audio mixer.ch0 @0
  bounce 96000 /tmp/out.wav
  ```

  The full verb set is small: `mount`, `patch`, `play`, `splice`, `record`, `bounce`,
  `pool`, `arrange` (`add_track` / `add_clip` / `move_clip` / `trim`), `set_param`,
  `set_tempo`, `unmount`. That is a vocabulary well inside a model's comfortable working set.

- **Refusals are the model's feedback channel.** A version mismatch, an unknown name, a
  type-mismatched patch or a splice-without-play is refused, loudly, and never logged. A
  conversational partner needs exactly that: a machine that says *no* with a reason, so the
  next attempt is better rather than silently wrong.
- **The session log is the conversation's memory.** Because the log is the command list, the
  history of how a piece reached its current state is a readable artifact — which is what an
  editing collaborator needs in order to revise rather than restart.
- **Agent Notes are the durable half of the collaboration.** Decisions graduate into notes;
  notes are the project memory. The LLM's contribution is therefore not a chat transcript but
  a command list plus, where a decision was made, a note.

### 4. Spokes named explicitly

| Spoke | Direction | Status |
|---|---|---|
| VCV Rack (`vcv-rack`) | Feeds material (renders source audio into the pool) | exists, separate repo |
| Tidal / SuperCollider (`tidal-lsp`) | Feeds material (generative runs) | exists, separate repo |
| CDP8, PaulStretch, phase vocoder | Extends (`OfflineProcess` plugins) | deferred — see below |
| CLAP/VST hosting | Extends (effect/instrument plugins) | declared seam, not implemented |
| Music theory corpus | Context only — never a runtime dependency | exists, separate repo |

## The sound-sculptor question this reopens

The `umbrella-first` note deliberately **deferred** offline/non-realtime processing (CDP8,
PaulStretch, phase-vocoder) into a separate "sound sculptor" profile at Phase 3+, and removed
it from the arranger to stop the profile re-coupling to CMake sidecar concerns. The renewed
emphasis on **sonic exploration and sound sculpting** points the other way, and this note does
not resolve it. Two readings:

- **Sculpting is a spoke, not a profile** — offline processing enters the arranger as
  `OfflineProcess` plugins driven by the same command list (`arrange`-style ops over rendered
  sources). Cheaper, keeps the authorial loop intact, contradicts the deferral.
- **Sculpting is its own profile, as decided** — the arranger stays pure realtime-work, and
  sculpting gets its own profile and workflow later, as the note says.

This needs an explicit call, and it should be a separate note. Recording it here so the next
agent does not silently drift either way.

## The seam, exercised

The central claim above is not speculative. While writing this note, a script in exactly the
format shown was composed by an LLM and run through the **existing** headless host binary — no
code changes, no new interface, no adapter:

```text
$ ./target/debug/host /tmp/llm-tone.txt
host: bounced 384044 bytes to /tmp/llm-tone.wav
engine log events: 7
media commands: 1
underruns: 0
deferred splices: 0
master out node: Some(NodeId(3))
```

The bounce was verified as real audio rather than silence: **2.000 s at 48 kHz stereo, peak
5572 of 32767, 3408 loud samples in two bursts, first onset at 0.125 s.** Seven engine commands
were logged — which is the session-log-as-memory property in practice, not just in principle.

Two things this establishes:

- **The LLM needs no new seam.** It drove the same parser, the same closed registry
  (`HOST_PLUGINS` / `HOST_PORTS` / `HOST_PARAMS`), the same refusals, and the same log as the
  Tauri shell and the CLI. It is not a special case; it is another host.
- **The authorial loop is already closed.** Write a command list → run it → inspect the bounce
  and the log → revise the list. That is precisely the loop this note proposes, available today
  with no new code.

It also surfaced a piece of friction worth recording, because it argues *for* the design rather
than against it: the registry is closed and hand-maintained, so an unknown name is refused with
its own documentation — `unknown plugin 'x' (registry: euclidean, scale, tone, mixer)`. **The
refusal is the teaching signal.** A conversational partner learns this vocabulary from the
failures, which is exactly the feedback channel the Proposal depends on and the reason the seam
does not need a bespoke "LLM mode".

## Alternatives considered

- **An MCP server wrapping the Host API** — a fashionable default, and rejected as premature:
  MCP adds a transport, a schema layer and a process to maintain in order to reach a format
  that is *already* text, versioned, deterministic and refusal-rich. The CLI can be driven
  today with stdin. Revisit only if a second consumer needs tool-discovery or if the format
  grows past what a model can hold comfortably.
- **The LLM as an in-process plugin (`Plugin` trait)** — tighter coupling and access to
  render-time state, but it would put a probabilistic component inside the deterministic core
  and forfeit byte-identical replay, which is the property that makes conversational revision
  safe. Rejected.
- **Fold the spokes into the core as crates** — the `other-software` zone rule already forbids
  it, and it would make the hub build depend on VCV/Tidal/CDP toolchains. Rejected.
- **Treat latency as still-relevant and keep pursuing real-time paths** — rejected as
  misaligned with the actual practice; it would optimize the inner loop of a workflow that is
  not being used.
- **A bespoke "LLM mode" or agent-specific command surface** — rejected: a parallel vocabulary
  would drift from the real one, and the value of the seam is precisely that the LLM drives
  *the same* commands as every other host.

## Acceptance criteria

- `README.md` states the hub-and-spokes topology and that the core runs with zero spokes
  installed.
- `README.md` (or the theory-of-the-program doc) states that latency is not a design
  constraint, and names the authorial loop as the primary workflow.
- The Host API text format is documented as the LLM/agent seam, with at least one worked
  script example, in `docs/` rather than only in a code comment or test.
- A script written by an LLM and a script written by hand are indistinguishable to the engine:
  same parser, same refusals, same log, byte-identical bounces.
- The sound-sculptor question above is resolved by a separate note before any `OfflineProcess`
  work starts.

## Risks

- **Hub becomes gatekeeper.** If the spokes' needs leak back into the core (a VCV-specific
  port, a CDP-specific job type), the "core runs with zero spokes" property dies and the hub
  becomes a distribution. Mitigation: the zero-spoke build stays a tested invariant.
- **The text format grows past conversational size.** The vocabulary is currently small enough
  to hold in context. If it expands toward a full DAW surface, hand-written scripts and LLM
  scripts both degrade. Mitigation: keep profile-level ops logged as commands the core does not
  need to understand (the existing `arrange_logged` pattern) rather than widening the core
  grammar.
- **Text-only editing under-serves sculpting.** Sound sculpting is unusually parameter-heavy and
  visual — waveforms, spectra, transients. A command list may be a poor handle for it, and the
  author may end up wanting a GUI for exactly the work named as the new emphasis. Flagged: this
  is the strongest argument for the "sculpting is its own profile" reading.
- **Silent determinism regressions.** The whole argument rests on byte-identical replay. If a
  future change makes a bounce depend on timing or thread order, conversational revision stops
  being safe and the seam becomes untrustworthy without any obvious symptom. Mitigation: the
  existing determinism tests are load-bearing and should be treated as such.
- **Latency-doc drift.** `docs/audio-latency.md` and the realtime-path emphasis remain, and
  nothing currently marks them as tuning rather than requirements. A future agent may
  re-optimize for a constraint that was withdrawn.
