# Agent Note: Frontend toolchain — Vite + pnpm, Node via devenv, single-package shell

Status: proposed

## Problem

The app is Rust (Tauri v2) + Vue 3, and the repo so far is a pure Cargo workspace (`crates/engine`, `crates/media`, `crates/host`) with no JavaScript at all. The `devenv.nix` already enables `languages.javascript.pnpm` and pins `nodejs_22`, but the frontend bundler and package layout were never fixed, and building a Tauri shell needs them decided up front so the layout is stable and reproducible. The plugin-as-a-package philosophy also means the JS side may eventually split into packages, so the choice should not paint us into a corner.

A concrete wrinkle: the devenv pins Node 22, but the shell exposes Node 24 from a `.vite-plus` runtime — the JS toolchain is not reproducible yet in the same way the Rust toolchain is (via the Nix/devenv flake).

## Proposal

1. **Bundler: Vite** — the Tauri v2 default (its `beforeDevCommand`/`dist` flow), first-class `@vitejs/plugin-vue` for SFCs, fast HMR. Start on a stable Vite major now; treat the Vite 8 (Rolldown) migration as a deliberate later bump, not a scaffold concern.
2. **Package manager: pnpm** — already enabled in devenv; strict isolation, workspaces (the eventual home for split `ui-plugin`s), speed, disk efficiency. The `@tauri-apps/cli` is installed as a dev dependency and driven by `pnpm tauri dev`.
3. **Node standardized on the devenv pin (Node 22)** — the devenv shell is the canonical JS environment, matching how the Rust toolchain comes from the flake. Don't rely on the `.vite-plus` Node 24 for builds; if we need newer, bump the devenv pin deliberately. Both satisfy Vite/Tailwind v4/Tauri v2.
4. **Single package for now, no workspace yet.** The frontend is one package today; move to a pnpm workspace only when we actually split `ui-plugin`s into separate packages (a workspace now is ceremony for a single frontend).
5. **Layout — the canonical Tauri app in a workspace crate.** The shell is the new crate `crates/shell`, a workspace member via `crates/shell/src-tauri`. The frontend lives at the crate root (app root) with `src-tauri/` inside it, so `beforeDevCommand: "pnpm dev"` and `frontendDist: "../dist"` stay simple and match `create-tauri-app`. `engine`/`media`/`host` stay untouched; the Tauri Rust side is the thin transport adapter over the Host API (the [UI-as-plugin note](../../implemented/architecture/2026-08-18-ui-as-plugin-host-api-and-headless-reference.md) and the [UI shell note](../architecture/2026-08-25-ui-shell-profiles-and-views.md) define the shape).

## Alternatives considered

- **npm** — universally available, symlink-free `node_modules`, fewest surprises with native modules. Rejected as the default: slower, weaker workspace ergonomics, no strict isolation; the plugin-as-package future favors pnpm.
- **bun** — fast, but less mature with some toolchains, not in the devenv, and Tauri examples lean npm/pnpm. Rejected for now.
- **webpack / rollup by hand** — overkill for a single Vue frontend; Vite covers it.
- **bare esbuild** — too low-level for `.vue` SFCs. Rejected.
- **pnpm workspace now** — rejected: ceremony for a currently single frontend; adopt it when plugins split.
- **Use the live Node 24 as the target** — both work, but it makes builds depend on a non-flake runtime and breaks reproducibility. Rejected in favor of the devenv pin.

## Acceptance criteria

- `pnpm install` in `crates/shell` produces a `pnpm-lock.yaml`.
- `pnpm build` in `crates/shell` produces `dist/` with no errors.
- `pnpm tauri dev` boots a window serving the Vue app (frontend dev server on port 1420).
- `cargo build -p sound-arranger-shell` compiles; the Rust side is a thin adapter, not a UI substrate.
- `node`/`pnpm` in the devenv shell resolve from the pinned flake (Node 22).

## Risks

- **pnpm's strict, symlinked `node_modules`** can misbehave with certain native/CJS packages (e.g. Tailwind v4's `@tailwindcss/oxide`). Expected to be fine (it ships platform-specific optional deps, not postinstall), but if a package breaks, fix it at the config level (`node-linker=hoisted` or `shamefully-hoist=true` in `.npmrc`), not by rewriting deps.
- **Node version drift** between the `.vite-plus` runtime and the devenv pin — mitigate by standardizing on the devenv shell and pinning.
- **`tauri build` bundling** requires icons/targets; scaffold sets `bundle.active=false` for dev, and real packaging is deferred.
