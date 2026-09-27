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

3. **Cordis is mid-migration and unstable.** v4 is `4.0.0-rc.7` (upstream is now `rc.10`; the harness's *own* `@deepseek-ai/cordis` manifest carries a separate Harness release version, `4.0.4`, which is **not** the upstream version — see the 2026-09-27 update below) with an explicit "may change without notice" warning; Koishi still runs v3 while the paper presents v4. The harness pays a visible price for reuse: it **vendors** nine packages with per-package upstream-commit pins, a scope rename (`cordis` → `@deepseek-ai/cordis`), a local-modification log, a `verify-vendored-links` hygiene gate, and an update procedure. That's real maintenance. Adopting churn before we know what we need violates the budget rule.

4. **The budget rule is binding.** Umbrella-first: "no composition machinery beyond what a phase-0 spike or a real second provider demands." Cordis *is* machinery — loader, reconciliation, HMR, realms, interception, service brokers. Its patterns cost nothing; its code costs ongoing sync.

5. **The FP angle favors traits over a runtime registry for our core.** The paper's own language guidance (§6.4): typeclasses (Haskell) / traits (Rust) are how a host extends the context type. Rust traits + generics give us **compile-time** dependency resolution and capability checking — stronger than runtime key lookup, no Proxy needed, and the realtime boundary stays value-based. For the TS host layer, Cordis's runtime model would fit — but that's precisely the part we don't need yet.

6. **The contract is ours regardless.** The graph value schema, log schema, and event types are *our* product contract (kimi's "the schema decisions Spike A will freeze"). Reusing Cordis doesn't give us these; it just adds a second vocabulary on top.

7. **It's conditional, not dogmatic.** The composition-seams note already states the gate: *adopt Cordis itself only when the frontend genuinely needs a plugin runtime.* When the TS side grows real third-party surface (external panels, scripts, device backends beyond our own), Cordis is battle-tested (Koishi 4 years, dsh) and the right thing to vendor — the harness shows exactly how (pin + scope-rename + sync manifest), and we keep the graph-value contract ours.

**Nix-flavored summary:** we're building our *own module system* because our "modules" (DSP nodes, RT services) carry requirements the generic evaluator can't honor — hard deadlines, allocation-freedom, no-inverse effects. That's the same reason you'd write a custom build system for a hard-realtime target rather than import one: the composition semantics are easy; the substrate is the product.

## 5. Bottom line

- **Reuse the paradigm** (free): seams as interfaces, `inject`-style dependencies, reversible effects, declarative rows with layered patches — already in our notes.
- **Borrow the code** (paid, with vendoring discipline) when a real seam demands it: the gate is written down.
- **Keep the realtime contract ours** (the graph value, log, events, teardown) — that's the part Cordis can't give us and the part the product lives on.

## Update 2026-09-27 — re-checked after a harness update: still an RC, and the design the vendored line gained

Verified against the harness checkout at `477b4f4205` (after a `dsh_update` from `0d1f50007f`) and upstream [`cordiverse/cordis`](https://github.com/cordiverse/cordis).

**Two version numbers, and conflating them misreads the situation.** `vendor/<pkg>/package.json` carries the *Harness release version* of the re-published `@deepseek-ai` package — Cordis reads `4.0.4` there, which looks like a stable 4.0 release. The **upstream** version lives only in `vendor/README.md`'s manifest table: `cordis 4.0.0-rc.7` at `56b3d4f`, and it did **not** move across the update (the `release(vendor)` commits touch only the manifests' version fields — nine files, one line each).

So §4.3 stands, and got sharper: upstream is now **`4.0.0-rc.10`** — the harness pin is three RCs behind a framework that is still a release candidate. A third-party repo exists purely to backport upstream Cordis fixes the vendored line misses ([dsh-cordis-backport](https://github.com/173787247/dsh-cordis-backport)); the vendoring cost §4.3 priced is now being paid by someone else too.

**What the line gained is design, not code volume** — Cordis core changed by four files (+13/−6) in that window. Three ideas worth carrying into our own row/patch model:

1. **Volatile config references** (`vendor/cosmokit/src/volatile.ts`, new). A schema field can be declared volatile; the plugin then holds a *stable reference* to an immutable snapshot the runtime updates in place (`get()` returns a frozen deep copy, functions and cycles are rejected, and ESM/CJS copies recognise each other through a `Symbol.for` protocol). Changing the value does **not** reconfigure the plugin.
2. **Schema-aware config diff** (`vendor/loader/src/config/diff.ts`, new). The loader compares raw configs through schema metadata: volatile fields compare equal, absent objects fall back to schema defaults, and expressions and unknown fields keep strict raw equality. This is what lets a reconciler read a change as "same row, new value" instead of "remount".
3. **Lazy config resolution** (ported from [cordiverse/cordis#41](https://github.com/cordiverse/cordis/pull/41)). Raw fiber config is retained and resolved only once declared injections are active, and only at the entry root — child plugins keep caller-owned config identity.

**Why this reaches us although we import none of it** (Cordis appears in this repo only in prose — research, notes, and one aside in the architecture explainer — and in no manifest or source file). Ideas 1+2 are the generic form of a problem we have in the realtime domain: *a parameter change must not tear down the thing it parameterizes*. Cordis's answer — keep identity stable, make the value a snapshot, and teach the **diff** to ignore what must not trigger a remount — is the same split our log already makes, where a logged `SetParam` carries the value while the node keeps its identity, and only an identity change pays the teardown protocol (ramp / voice-steal / tail-flush). Worth naming that distinction in the [composition-seams note](../../.agents/notes/proposed/architecture/2026-08-15-composition-seams-plugin-architecture.md) before any reconciliation machinery gets built.

The maintenance evidence grew with it: the vendored line's local-modification log now runs to **20 entries** — deep `fiber.ts` lifecycle hardening, durable Include writes, Node loader-shape detection. The harness does not consume Cordis as-is; it carries it. §4.4's budget rule reads the same, only more so.

*Corrections this re-check made to the document above: §4.3 said the harness vendors "eight packages" — the manifest lists nine; and the `4.0.4` in the package manifests is the Harness release version, not upstream.*

*Update authored with DeepSeek-V4.1-Flash · DeepSeek Harness, 2026-09-27.*
