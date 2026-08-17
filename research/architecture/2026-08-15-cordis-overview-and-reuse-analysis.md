# Cordis overview — and how close we are implementing it (and why we're not just reusing it)

> Research input, 2026-08-15. Companion to `2026-08-15-dsh-agent-presets.md` (the harness's use of Cordis) and the kimi review (`2026-08-15-kimi-review-minimal-core.md`). Sources: the paper (`cordiverse/paper`, full text at `.research/paper/paper.txt`), the Cordis repo README + core package, the official primer (`deepseek-harness.github.io/deepseek-harness/reference/cordis-primer`), the vendored copy + `vendor/README.md` in the harness checkout.

## 1. Cordis at a glance

Cordis is a **TypeScript meta-framework of spatiotemporal composability** — "a framework for building frameworks." It fixes how effects and coeffects compose and leaves domain vocabulary to the application; Koishi (chatbot framework, ~4 years, 4000+ plugins) is its production case study, and DeepSeek Harness is its second, larger consumer. Version 4 is the paper's implementation; the repo itself warns the API "is not yet stable and may change without notice," and the harness vendors **4.0.0-rc.7**.

Repo shape: a workspace of `core`, `loader`, `group`, `include`, `hmr`, `create`, `timer`, `logger-console`, `utils` (+ ecosystem packages for database, webui, sso, registry, …).

## 2. The five core ideas (primer)

1. **A plugin is an object implementing Service** — a function with optional `inject` + `apply(ctx)`, or a `Service` subclass whose lifecycle Cordis mounts into the context.
2. **A context is a repository of services** — a service claims a stable `ctx.<key>`; consumers find it by key, never by importing an implementation.
3. **Declare dependencies via `inject`** — load order is expressed through service requirements, not manual boot sequencing.
4. **Typed events** — names via TypeScript declaration merging; dispatched `emit` (observe) / `waterfall` (wrap; `next()` delegates) / `parallel` (fan out) / `serial` (ordered, returns value); the dispatch mode is part of the event's public contract.
5. **Registrations are reversible effects** — installed via `ctx.effect()` / `ctx.on()`, unwound on reload and teardown, LIFO; the author writes an inverse only per atomic effect, and the inverse of any composite is derived by composition.

**On top of the five:** a declarative **loader** (entries `{id, url, config, disabled, isolate, intercept}`; incremental keyed reconciliation; layered patches; whole-row replacement; transactional HMR with rollback; path independence — final state is a function of the config alone), **isolation realms** (same key resolves differently per context), **interception** (mergeable metadata changing how a dependency is used), and service-broker patterns (exclusive binding vs broker: load balancing, rolling updates, cross-process).

## 3. How close are we implementing it?

Our documented plan (composition-seams + minimal-core + musical-event + umbrella-first notes) mapped against Cordis's surface:

| Cordis concept | Our plan | Closeness |
|---|---|---|
| Context as service repository (`ctx.<key>`) | `ctx.clock` / `ctx.audio` / `ctx.session` / `ctx.parts` / `ctx.progression` + Rust trait seams (`Recorder`, `OfflineProcess`, `EffectHost`, `Codec`) | **High in spirit** — same pattern, our own vocabulary. Structurally different: Rust traits (compile-time) vs TS Proxy (runtime) |
| Plugin = Service with `inject` + `apply(ctx)` | Config row + trait impl; generators declare what they consume (euclid injects `ctx.clock`) | **High** |
| `inject` = dependencies order loading | Consumers depend on trait definitions, never providers; runtime orders via service requirements | **High in spirit** (our Rust side gets it at compile time) |
| Typed events (emit/waterfall/parallel/serial, `@mode`) | **Not specified yet** — the musical-event note defines event *types* (ClipEvent/NoteEvent/ControlEvent/MarkerEvent) but not dispatch modes | **Gap** (deliberate — no cross-plugin listeners yet) |
| Reversible effects (`ctx.effect` + disposer, derived teardown) | "Every registration returns a disposer; teardown derived LIFO" + our **audio teardown protocol** (ramp/voice-steal/tail-flush — the RT-correct version of reversal) | **High**, plus an extension Cordis doesn't have |
| Declarative loader (entries, reconciliation, patches, path independence) | Rows `{id, name, config, disabled}`, layered patches, whole-row replacement, later-layers-win | **Medium-high** — subset; reconciliation machinery deferred |
| Isolation (realms) | Only in research (§16); not yet in any note | **Gap** |
| Interception (metadata) | Not planned | **Gap** |
| Fiber lifecycle / inertial state machine | Mount/unmount discipline + teardown protocol; no full lifecycle state machine | **Partial** |
| HMR / hot module replacement | Explicitly deferred (YAGNI, budget rule) | **Deliberate gap** |
| Service broker (multiplexing, rolling updates) | Not planned (no need yet) | **Deliberate gap** |

**Honest summary: we are implementing the *discipline* at high fidelity where it matters — seams, dependency declaration, reversible effects, declarative rows — and deliberately skipping the *machinery*: loader reconciliation, realms, interception, the lifecycle state machine, HMR. By concept surface we're at roughly 60%; by effort, the skipped 40% is the most expensive-to-maintain and cheapest-to-add-later part.** The umbrella-first budget rule is the governor: no machinery beyond what a spike or a real second provider demands.

## 4. Why we are not just reusing the code

1. **The language boundary is the real wall.** Cordis is TypeScript/npm and runs in a JS host. Our product's difficulty is Rust-side: the clock, the graph interpreter, PDC, disk streaming. Reuse could only cover the TS composition layer; the engine would remain a foreign body behind the value boundary, and every plugin that touches audio would split across it — the split-brain kimi flagged. We would still have to build the graph-value contract, the log schema, the event types, the teardown protocol, and the RT swap machinery ourselves. Cordis doesn't know audio, and no amount of reuse removes that work.

2. **Cordis's guarantees are control-plane; audio has no inverses.** Revertible effects assume environment state that can be restored. You cannot un-ring a reverb. Our teardown protocol (ramp / voice-steal / tail-flush), the atomic `Arc` graph swap, the timestamped SPSC parameter queues, PDC — none of these exist in Cordis, and they're the hard 80% of the product. The composition layer is the easy 20%.

3. **Cordis is mid-migration and unstable.** v4 is `4.0.0-rc.7` with an explicit "may change without notice" warning; Koishi still runs v3 while the paper presents v4. The harness pays a visible price for reuse: it **vendors** eight packages with per-package upstream-commit pins, a scope rename (`cordis` → `@deepseek-ai/cordis`), a local-modification log, a `verify-vendored-links` hygiene gate, and an update procedure. That's real maintenance. Adopting churn before we know what we need violates the budget rule.

4. **The budget rule is binding.** Umbrella-first: "no composition machinery beyond what a phase-0 spike or a real second provider demands." Cordis *is* machinery — loader, reconciliation, HMR, realms, interception, service brokers. Its patterns cost nothing; its code costs ongoing sync.

5. **The FP angle favors traits over a runtime registry for our core.** The paper's own language guidance (§6.4): typeclasses (Haskell) / traits (Rust) are how a host extends the context type. Rust traits + generics give us **compile-time** dependency resolution and capability checking — stronger than runtime key lookup, no Proxy needed, and the realtime boundary stays value-based. For the TS host layer, Cordis's runtime model would fit — but that's precisely the part we don't need yet.

6. **The contract is ours regardless.** The graph value schema, log schema, and event types are *our* product contract (kimi's "the schema decisions Spike A will freeze"). Reusing Cordis doesn't give us these; it just adds a second vocabulary on top.

7. **It's conditional, not dogmatic.** The composition-seams note already states the gate: *adopt Cordis itself only when the frontend genuinely needs a plugin runtime.* When the TS side grows real third-party surface (external panels, scripts, device backends beyond our own), Cordis is battle-tested (Koishi 4 years, dsh) and the right thing to vendor — the harness shows exactly how (pin + scope-rename + sync manifest), and we keep the graph-value contract ours.

**Nix-flavored summary:** we're building our *own module system* because our "modules" (DSP nodes, RT services) carry requirements the generic evaluator can't honor — hard deadlines, allocation-freedom, no-inverse effects. That's the same reason you'd write a custom build system for a hard-realtime target rather than import one: the composition semantics are easy; the substrate is the product.

## 5. Bottom line

- **Reuse the paradigm** (free): seams as interfaces, `inject`-style dependencies, reversible effects, declarative rows with layered patches — already in our notes.
- **Borrow the code** (paid, with vendoring discipline) when a real seam demands it: the gate is written down.
- **Keep the realtime contract ours** (the graph value, log, events, teardown) — that's the part Cordis can't give us and the part the product lives on.
