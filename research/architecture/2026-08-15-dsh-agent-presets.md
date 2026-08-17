# dsh agent presets — the four modes (standard · code · minimal · cordis)

> Research input, 2026-08-15. Source: local checkout of `deepseek-ai/deepseek-harness` (vendor of Cordis 4.0.0-rc.7). The presets are YAML compositions under `apps/cli/config/agent-presets/<id>/`: an `agent.cordis.yml` (the composition) plus a `preset.yml` (name/description/order). The mechanism is documented in `packages/preset/README.md` and `packages/core/agent-tool-presentation/README.md`.

## The four shipped presets

| id | Name (zh) | order | One-liner |
|---|---|---|---|
| `standard` | 标准模式 | 1 | The full coding agent — the baseline every other preset extends |
| `code` | PTC 模式 | 2 | Standard **plus Code Mode**: tools presented through a generated TypeScript SDK |
| `minimal` | 极简模式 | 3 | Fixed-prompt, two-tool agent (persistent bash + str_replace_editor) |
| `cordis` | 创造模式 | 4 | Standard **plus the self-referential Cordis toolset** — an agent that authors agents |

## The mechanism

- **A preset is a per-session composition.** `ctx.agentPresets.compose()` mounts one `agent.cordis.yml` under an agent's scope context; each session gets its own tools/persona/prompt sections while other live sessions keep theirs — one process runs several differently-composed agents at once.
- **Host-plane vs agent-plane.** Registries and cross-session facilities (the `tools` registry, sandbox + approval stack, persistence, model route, subagent registry and backends, `tokenMeter`) live in the **host** composition (`base.cordis.yml` + `web.cordis.yml`). A preset only contributes what one agent adds to those registries. Every preset file's header explains which rows must stay host-plane and why (e.g. `tool-jobs`: "the task REGISTRY stays on the host plane… the registry is keyed by owning agent anyway, so one host instance serves every session").
- **Realm discipline is enforced.** Any service row a preset publishes must sit inside a group with an `isolate` realm (`isolate: {planMode: true}`, `{compaction: true, toolResultPruner: true}`, `{workflowEngine: true}`…). Without one it publishes process-global and the second session mounting the preset collides; `dsh-agent-presets` **rejects the mount**, naming the row — fail-loud, not corrupt.
- **Discovery is unmemoized**: `list()`/`resolve()` re-read the roots on every call, so a preset authored while the process runs is visible immediately. User presets live at `${DSH_HOME}/.agent-presets/<id>/`; the roster is the directory listing of `apps/cli/config/agent-presets`.
- **Broken presets resolve but refuse to mount**: delete/read/report need the row; mounting validates through `resolveMountable` and a rejection rolls the agent creation back — "a broken preset never yields a half-composed session."

## Code Mode — one row changes the interaction paradigm

`code` = `standard` verbatim **plus**:

```yaml
- id: tool-presentation
  name: '@deepseek-ai/dsh-agent-tool-presentation'
  config:
    mode: code
```

`dsh-agent-tool-presentation` is a *presentation* of the existing tool registry: `native` (every schema), `code` (only `run_code` plus a generated TypeScript SDK), `both`. "Instead of one tool call per action, the model writes a TypeScript program against a generated SDK and `run_code` executes it, so a sequence that would be five round trips becomes one." Key properties:

- The **catalog is unchanged** — the request prefix stays stable for the session's life (KV-cache stability).
- Under `code`, a model-direct call naming any other tool resolves to `UNKNOWN_TOOL` — the announced surface and the callable surface stay identical (executor-collapse).
- The TypeScript runtime is host-plane (`dsh-code-runtime-worker-thread`); a preset selecting Code Mode against a deployment composing none **holds the row pending and refuses the mount**, naming this id — failure at composition, not at first request.
- One agent declares one presentation; a second declaration is refused rather than merged ("two answers to 'which form does the model see' is a contradiction").

## Creator Mode — the agent that authors agents

`cordis` = `standard` **plus** the self-referential toolset. Its persona:

> "You can read and modify the harness you run on. Its composition is Cordis: every capability is a plugin row in a `cordis.yml`, and an agent preset is one such file mounted for a single session. Two planes decide where an edit belongs… Presets you author live one directory per preset under `${DSH_HOME}/.agent-presets/<id>/`… NEVER edit or delete the shipped preset install… corrupting the `cordis` preset would disable this very mode. Load the `editing-cordis-compositions` skill before writing or changing a composition."

It adds `cordis_mount` (evaluates **model-written JavaScript against the live runtime**), the `editing-cordis-compositions` skill, and an explicit trust warning in the file header: *"Treat a session on this preset as shell access."*

## Patterns worth stealing for sound-arranger

1. **The host-plane/agent-plane rule is our core/plugin boundary in production.** Registries are process singletons; presets contribute registrations; a preset that would publish a global service is rejected at mount. Our privileged-realtime-kernel rule is the audio-flavored version.
2. **Presentation is a first-class concept.** `code` mode adds no tools — it changes how the model sees the same registry. Maps directly onto "views edit the log+graph": one clip engine, many presentations (timeline / pattern-cell grid / notation), each a layer over one substrate. Worth a line in the musical-event-model note.
3. **Creator Mode is the blueprint for our improv plugin**: an agent that inspects and re-composes its own runtime, with the audit trail (our session log) and the trust boundary stated in the persona itself.
4. **Fail-loud composition**: broken presets resolve but refuse to mount, with the offending row named — matches our "invalid config fails the load with actionable errors" instinct.
