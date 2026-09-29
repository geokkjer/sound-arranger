#!/usr/bin/env node
/**
 * Verify the Agent Note tree in .agents/notes/.
 * Zero dependencies — run with `node scripts/verify-agent-notes.mjs`.
 * The rules this enforces live in .agents/notes/README.md.
 */

import { existsSync, readFileSync, readdirSync } from 'node:fs'
import { dirname, resolve } from 'node:path'

const root = resolve(import.meta.dirname, '..', '.agents', 'notes')
const repoRoot = resolve(import.meta.dirname, '..')

const LIFECYCLES = ['proposed', 'implemented', 'rejected']
const CLASSES = ['feature', 'bug-fix', 'simplification', 'architecture', 'process']
const ARCHIVE = 'archived'
const ROOT_ALLOWLIST = new Set(['README.md', 'AGENTS.md', 'CLAUDE.md', '.gitkeep'])

const STATUS = {
  proposed: /^Status: proposed$/,
  implemented: /^Status: implemented$/,
  rejected: /^Status: rejected — .+$/,
}

const REQUIRED = {
  proposed: ['## Proposal', '## Alternatives considered', '## Acceptance criteria', '## Risks'],
  implemented: ['## Decision', '## Alternatives considered', '## Consequences'],
  rejected: ['## Proposal', '## Alternatives considered'],
}

const BANNED_IMPLEMENTED = /^## (?:Proposal\b|Plan\b|Migration plan\b|Acceptance criteria\b)/i
const FILENAME = /^(\d{4}-\d{2}-\d{2})-.+\.md$/

const errors = []
const fail = (rel, msg) => errors.push(`${rel} — ${msg}`)

// The lifecycle set is closed: a stray folder would hide notes from the walk.
for (const entry of readdirSync(root, { withFileTypes: true })) {
  if (entry.isDirectory() && !LIFECYCLES.includes(entry.name) && entry.name !== ARCHIVE) {
    fail(`${entry.name}/`, `unknown lifecycle folder (allowed: ${LIFECYCLES.join(', ')}, ${ARCHIVE})`)
  }
}

for (const lifecycle of LIFECYCLES) {
  let classes
  try {
    classes = readdirSync(resolve(root, lifecycle), { withFileTypes: true })
  } catch {
    continue // empty lifecycle — nothing to check
  }
  for (const cls of classes) {
    if (!cls.isDirectory()) {
      if (!ROOT_ALLOWLIST.has(cls.name)) fail(`${lifecycle}/${cls.name}`, 'unexpected file at the lifecycle root')
      continue
    }
    if (!CLASSES.includes(cls.name)) {
      fail(`${lifecycle}/${cls.name}/`, `unknown class folder (allowed: ${CLASSES.join(', ')})`)
      continue
    }
    const classDir = resolve(root, lifecycle, cls.name)
    for (const file of readdirSync(classDir)) {
      const rel = `${lifecycle}/${cls.name}/${file}`
      if (!file.endsWith('.md')) {
        if (file !== '.gitkeep') fail(rel, 'unexpected non-markdown file')
        continue
      }
      const match = FILENAME.exec(file)
      if (!match) {
        fail(rel, 'filename must be yyyy-mm-dd-topic.md')
        continue
      }
      const [year, month, day] = match[1].split('-').map(Number)
      const date = new Date(Date.UTC(year, month - 1, day))
      if (date.getUTCFullYear() !== year || date.getUTCMonth() !== month - 1 || date.getUTCDate() !== day) {
        fail(rel, `filename date ${match[1]} is not a real calendar date`)
      }

      const lines = readFileSync(resolve(classDir, file), 'utf8').split('\n')
      // Format tokens inside fenced examples are not document structure.
      let inFence = false
      const prose = lines.filter((l) => {
        if (l.startsWith('```')) {
          inFence = !inFence
          return false
        }
        return !inFence
      })

      if (!/^# Agent Note: \S/.test(lines[0] ?? '')) fail(rel, 'line 1 must be `# Agent Note: <title>`')
      if (lines[1] !== '') fail(rel, 'line 2 must be blank')
      if (!STATUS[lifecycle].test(lines[2] ?? '')) {
        fail(rel, `line 3 must match the ${lifecycle} status grammar (${String(STATUS[lifecycle])})`)
      }
      if (lines[3] !== '') fail(rel, 'line 4 must be blank')
      if (prose.filter((l) => l.startsWith('Status:')).length !== 1) fail(rel, 'exactly one `Status:` line is allowed')

      const h2s = prose.filter((l) => l.startsWith('## ')).map((l) => l.trimEnd())
      if (h2s[0] !== '## Problem') {
        fail(rel, `the first section must be \`## Problem\` (got ${JSON.stringify(h2s[0] ?? '<none>')})`)
      }
      for (const required of REQUIRED[lifecycle]) {
        if (!h2s.includes(required)) fail(rel, `missing required section \`${required}\``)
      }
      if (lifecycle === 'implemented') {
        for (const h2 of h2s.filter((h) => BANNED_IMPLEMENTED.test(h))) {
          fail(rel, `\`${h2}\` is proposal-era spec-speak; an implemented note states what is (fold it into Decision/Consequences)`)
        }
      }

      // Cross-references must resolve. A note pointing at a note that moved (or
      // never existed) is the rot this tree accumulates most quietly, and the
      // pre-commit gate is the only reader that never gets tired of noticing.
      // Fenced examples are already excluded (they are not document structure).
      // A leading `/` is repo-root-relative (how GitHub renders it).
      for (const link of prose.join('\n').matchAll(/\]\(([^)\s]+)\)/g)) {
        const target = link[1].split('#')[0]
        // External references are not ours to resolve (and a gate that needs the
        // network is a gate that fails on a train).
        if (!target || target.startsWith('//') || /^[a-z][a-z0-9+.-]*:/i.test(target)) continue
        const linkPath = target.startsWith('/')
          ? resolve(repoRoot, `.${target}`)
          : resolve(dirname(resolve(classDir, file)), target)
        // A path that leaves the repository is a **cross-project reference** — the
        // studio notes and sibling projects AGENTS.md tells us to link instead of
        // copying. It resolves on the machine that has those checkouts and can never
        // resolve in a clone, so it is out of scope for a gate that must mean the
        // same thing everywhere.
        if (!linkPath.startsWith(repoRoot + '/')) continue
        if (!existsSync(linkPath)) fail(rel, `broken link: ${link[1]}`)
      }
    }
  }
}

if (errors.length === 0) {
  console.log('verify-agent-notes: tree OK')
  process.exit(0)
}
console.error('verify-agent-notes: violations:')
for (const e of errors) console.error(`  ${e}`)
process.exit(1)
