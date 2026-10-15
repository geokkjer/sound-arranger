# Agent Note: UI plugins in a compiled shell — views are compiled, composition is data

Status: proposed

## Problem

The composition thesis says **every capability is a plugin and a product is an assembled profile**
([profiles note](2026-09-27-profiles-and-the-umbrella-name.md),
[UI-as-plugin note](../../implemented/architecture/2026-08-18-ui-as-plugin-host-api-and-headless-reference.md),
[composition seams](2026-08-15-composition-seams-plugin-architecture.md)). The engine half is real:
crates, the closed carrier event, sidecar processes. The **UI half was never exercised**, and acceptance
criterion 4 of the [iced evaluation](2026-09-21-iced-shell-evaluation.md) now asks the sharp question —
*"how (or whether) runtime-contributed UI survives in a compiled Rust shell, and what that costs the
composition thesis."*

The evaluation's premise for that question is wrong, and correcting it changes what has to be built.
It says the Vue shell *"does that with a declarative loader + registry"*. What the retired shell
actually shipped is a **static registry**: [`crates/shell/src/plugins.ts`](../../../../crates/shell/src/plugins.ts)
imports its four view components at the top of the file, puts them in a `Record<string, UiPluginView>`,
and resolves a profile's `slots` through `viewFor()`. There is **no dynamic `import()` and no manifest
fetch anywhere in the shell's source** (grepped), and the file's own comment says the other five
contributions the plan describes — `Command`, `Overlay`, `State`, `Bridge`, `Preference` — are
*"declared in the plan but not yet wired for v1"*. The runtime manifest → registry → hot-swap lifecycle
in [the UI plan §1](../../../../docs/design/ui-plan.md) was a **design**, not a shipped behaviour, and
the shell's whole front end is 2,251 lines of hand-written Vue/TypeScript over plain markup —
`reka-ui` is declared in `package.json` and imported by no file
([inventory](../../../../research/architecture/2026-10-15-iced-widget-inventory.md) §1).

So the question is not "how do we port a runtime loader". It is: **what did the thesis actually promise
about UI, and what can a compiled shell honestly deliver?**

## Proposal

**Views are compiled Rust with a toolkit-free contract; composition is data; runtime *code*
contribution is deferred behind a named door instead of promised.**

1. **A view is a Rust crate, and it does not know it is iced.** The contract is the shape
   [FaderFrame's `CanvasView`](../../../../research/architecture/2026-10-14-faderframe-architecture-and-ui-study.md)
   proved with 17 GTK-free view crates: `paint(&mut Painter, size, &Model, &Theme)` and
   `event(ViewEvent, size, &mut Model, &mut EventCx) -> Vec<Message>`, with declarative repaint hints
   (`wants_frames()`, `dense()`), an opaque drag payload with an explicit `cancel_drag`, and a
   `HostRequest` hole for the things a view cannot do itself (menus, text entry, file choosers,
   screen-reader announce). The iced shell owns the translation: `Model`/`Message` in, iced
   `canvas::Program`/`Element` out, theme and repaint policy at the shell's edge. This is also what keeps
   timings and the theme honest — a view cannot reach for `iced::Theme` and hardcode a colour.
2. **A profile is data, and the registry is compiled.** A profile names which view id occupies which
   slot (`fill`/`left`/`right`/`bottom`), its keymap, and its default parameter values — exactly the
   table `plugins.ts` already is, and the shape the composition seams' **planned** `ctx.rs` service
   registry describes for services (deferred there, not shipped — see
   [composition seams](2026-08-15-composition-seams-plugin-architecture.md)). The registry maps ids to
   compiled constructors. **Hot-swap means swapping the table**: a
   new profile assembles a different view set and reflows the Surface without a rebuild and without a
   restart, which is the visible behaviour the plan wanted; adding a *new* view is a rebuild.
3. **The `Command` half is already solved, and shared — use it as the model.** The
   [`workflow` crate](../../../../crates/workflow/src/lib.rs) is a compiled, data-driven table of
   actions, keys and help rows that *two different shells* read; the plan's `Command` contribution
   (action + binding + help) is that crate. A profile's keymap is an overlay on it, not a second table.
   So one of the six contributions is already in the tree, and it is the one that proves the pattern.
4. **Runtime code contribution is deferred, with the door named.** The only true "a third party adds a
   view without a rebuild" mechanism for a compiled shell is a **WASM view module against a paint/event
   ABI** (a second runtime, an ABI to version, a sandbox, and a serialization boundary — most of what
   retiring the webview removed), or an **external process** on the
   [external-programs seam](2026-09-21-external-programs-not-sidecars.md), which is the right home for a
   *tool* and the wrong home for a panel a user drags. Neither is built; this note fixes the order
   (compiled contract first, then data composition, then — only if a second contributor exists — the
   WASM door) and says plainly that **until then the thesis is true of the assembly, not of the code**.
5. **The promised contributions that still have no home get named now.** `Overlay` and `Preference` are
   data the shell can interpret (an overlay is a view + a lifetime; a preference is a declared row).
   `State`/`Bridge` are the Host API's job already (the snapshot and the command bridge). `Command` is
   `workflow`. So the table is complete under compiled views — which is why the honest gap is narrower
   than the plan's six-way contract suggested.
6. **Accessibility is a stated cost, not a solved problem.** iced 0.14 has **no accessibility tree**
   (verified: zero `accesskit` references in `iced`, `iced_winit`, `iced_runtime`, `iced_core`), while
   the webview inherited the DOM's. The plan chose `reka-ui` precisely for behaviour + a11y. A compiled
   shell must either build a tree into its painter contract (`AccessNode` per view, as FaderFrame does)
   or **declare the gap** in [`docs/capabilities.md`](../../../../docs/capabilities.md). Either way it
   is a first-class item of the compiled-view contract, not an afterthought — the contract in item 1
   should carry the `slot: Vec<AccessNode<A>>` that makes the second option a choice rather than an
   excuse.

**What this costs the composition thesis, stated plainly:** a third party can contribute an *engine*
plugin, a *profile*, a *keymap* and *data* without touching our shell; they cannot contribute a *view*
without building against our crates. That was already true of the shipped Vue shell in practice (its
registry was compiled in), and it is true of the TUI — the primary shell — which loads nothing at
runtime either. The thesis keeps its load-bearing half (capabilities compose, products assemble) and
loses a half that was aspirational.

## Alternatives considered

- **Keep a webview for the UI-plugin half** (a compiled shell for hot paths, a webview for contributed
  views). Rejected: that is the three-runtimes-for-one-program shape the owner retired on 2026-09-22
  ([shells note](../../implemented/architecture/2026-09-22-shells-are-iced-and-ratatui-tauri-retired.md)),
  and it reinstates the IPC + serde wire for the *least* hot path while keeping it out of the hot one —
  the complexity, minus the justification.
- **A general data-driven view schema** (a declarative tree/DSL the shell interprets, so a plugin
  contributes JSON and gets a panel). Rejected for now: it is a second view language, which is exactly
  why Slint was dismissed and why iced's philosophy is in the running; and a schema general enough for
  the timeline would be a bad drawing API. Worth revisiting narrowly for *forms* (the detail view's
  fields are a legitimately data-shaped surface).
- **An embedded script engine (rhai/lua) for view construction.** Rejected: it reaches into widget
  internals through a scripting boundary, cannot be statically checked against the contract, and buys
  hot-reload for our own views — a developer convenience, not a plug-in story.
- **WASM now.** Rejected as sequencing, not as an option: it is the real door for runtime code and it
  should be opened when there is a second contributor to walk through it. Building an ABI, a sandbox
  and a paint protocol to serve one author's own views is the "effect-plugin framework before any
  effect" mistake this project has already refused once.
- **Ship the plan's runtime manifest loader by fetching a manifest and resolving views by name.**
  Rejected: names still have to resolve to compiled code, so it buys configurability (which item 2 gives
  with a table) while looking like extensibility.
- **Drop the composition thesis for the UI entirely and say so.** Considered and rejected as the opening
  position: it over-corrects by giving up profile assembly, which is real, shipped and cheap (the
  `workflow` table, the `Surface` slots, and the engine profile chain already compose).
- **Do nothing until the alpha lands.** Rejected only in the sense that this note exists: criterion 4
  asks for an answer in writing, and the answer is cheap to write and expensive to discover later (a
  view-contract retrofit after a dozen views exist is the "reshaping the log with users' sessions in it"
  mistake, applied to the UI).

## Acceptance criteria

1. **A toolkit-free view contract exists in the tree**, with no `iced` type in its signature, and at
   least one shipped view (the timeline) implemented against it *and* still driven by the shell's
   existing tests.
2. **A profile table assembles the surface**: two profiles over the same compiled view set produce
   different slot layouts and keymaps, switched without a restart, from data.
3. **The `Command` contribution is the `workflow` table**: a profile's keymap is an overlay on it, and
   a test proves a profile cannot invent a key the shared table does not have.
4. **The evaluation's criterion 4 is answered by construction**: the note states, in one sentence, what
   a third party can contribute without rebuilding (engine plugin, profile, keymap, data) and what they
   cannot (a view), with the WASM door named as the deferred mechanism.
5. **The accessibility decision is explicit**: either the contract carries an `AccessNode` tree and the
   shell publishes it, or `docs/capabilities.md` states that this shell has no screen-reader tree and
   why — no third option where it is merely unmentioned.
6. **The repaint policy is declarative** (`wants_frames()`/`dense()` per view, the host deciding frames
   and routing), so a contributed view cannot couple itself to the shell's clock — the FaderFrame
   lesson, and the thing that keeps the measured 60 fps honest as views multiply.

## Risks

- **The contract freezes too early.** A `paint`/`event`/`AccessNode` contract designed against one view
  (the timeline) may not fit the detail view or a plugin editor. Mitigation: the contract is proposed
  here, and item 1's acceptance requires a *second* view (the console or the detail view) converted
  before the note becomes implemented.
- **`Model`/`Message` generics leak iced types anyway.** A view that returns `Vec<Message>` where
  `Message` is the shell's enum is already coupled to the shell's vocabulary. Mitigation: the contract
  is generic over the message type (as FaderFrame's `CanvasView<M, A>` is) and the shell owns the
  mapping; a test can assert a view crate does not depend on `iced` in its `Cargo.toml`.
- **Data-driven composition hides the real cost of a view.** Assembling profiles from a table makes
  "adding a view" look like configuration; it is still a rebuild and a review. Mitigation: the note's
  wording ("thesis is true of the assembly, not of the code") is the plain statement that must survive
  into the docs.
- **A11y is expensive and easy to defer forever.** If the contract carries `AccessNode` and nobody
  publishes it, the field is decoration. Mitigation: acceptance item 5 forces a decision in writing at
  the same time as the contract, and the fallback (declare the gap) is explicitly permitted.
- **Themes and repaint hints drift between views.** Every view deciding its own colours or its own
  redraw cadence is how a compiled shell grows the inconsistency the theme engine exists to prevent.
  Mitigation: the theme is passed *into* `paint` and the repaint hints are declared, per FaderFrame
  §5–§6 (ledger [PA-0020/PA-0021/PA-0022](../../../../research/prior-art/ledger.md)).
- **The TUI cannot follow.** A contract shaped for a pixel painter may not fit a cell-based shell, so
  the two shells could drift on the very thing the `workflow` crate keeps shared. Mitigation: the
  contract is for *views*, not for commands; the shared half stays `workflow`. This is the same split
  the viewport made when it was deliberately kept per-shell.

Authored with DeepSeek-V4.1-Flash · DeepSeek Harness, 2026-10-15.
