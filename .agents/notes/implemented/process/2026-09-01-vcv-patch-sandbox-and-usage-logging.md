# Agent Note: VCV Rack patch sandbox + CPU/RAM usage logging

Status: implemented

## Problem

The Eurorack GO build is planned, not owned yet — and Behringer modules have a 9–13 week lead time. We need to de-risk the purchase and get a feel for what the software can actually run before committing hardware, and to measure whether a given patch (or the intended hardwar voice) is within the CPU budget of the owner's "upper-midrange" machine. There was no place to keep good patches, and no quick way to log a running rack's resource cost.

## Decision

- **`~/Projects/music/vcv-rack/`** (extracted out of this repo) holds the VCV Rack patches used to prototype planned hardware and explore generative patching. A `README.md` documents the software↔hardware mapping (Vektor↔Victor, Venom↔Surges, Aestus/Tides↔Waves, ADSR↔140, VCA↔IDA, Audio↔CU1A), the reference monophonic voice patch, the naming convention, and the caveat about the Vektor embedding imported `.wav` into the patch JSON.
- **`scripts/log-vcv-usage.sh`** (moved with it) samples the CPU% and RSS of a running VCV Rack process (tree) and appends a CSV to `vcv-rack/.usage/` (gitignored). It reads `/proc/<pid>/stat` deltas so CPU% is a true per-interval rate (not a `ps` lifetime average), sums across the Rack process tree, handles the multi-threaded >100% case, and needs no deps beyond bash + pgrep + getconf/awk.
- The rack plan links the software sandbox so the two stay in sync.

## Alternatives considered

- **`ps -o %cpu`** — a lifetime/cumulative per-process figure that lags transients and isn't a good "current load" proxy; rejected for `/proc` jiffy deltas.
- **`pidstat` (sysstat)** — accurate, but an external dependency that may not be installed; fell back to a dependency-free `/proc` implementation.
- **VCV Rack's GUI CPU meter only** — authoritative for the audio engine, but not logged and not correlated to a patch; the script supplements it.
- **Committed sample logs** — the usage logs are transient measurement noise; gitignored and regenerated on demand instead of cluttering history.

## Consequences

- Patches live under version control (lean ones) and are easy to reproduce from the documented mapping; heavy waveform-embedded patches are kept out of the repo to avoid JSON bloat.
- Usage is measured as a whole-process CPU%/RSS proxy, so it's a guide for capacity planning, not a precise audio-engine metric — the Rack CPU bar remains the reference for the DSP thread.

*Authored with DeepSeek-V4-Flash · deepseek-harness, 2026-09-01.*
