#!/usr/bin/env node
/**
 * SELFHOSTED (fork): decoupling guard.
 *
 * Keeps the selfhosted branch cheap to merge with the official upstream:
 *
 * 1. Hard fail — any shared translation.json under
 *    apps/readest-app/public/locales that differs from the merge-base with
 *    the upstream. Fork-only strings belong in the 'selfhost' namespace
 *    (locales/<lng>/selfhost.json), never in the shared translation files
 *    upstream keeps churning.
 *
 * 2. Budget warn — protected upstream-shared files whose changed-line count
 *    (additions + deletions vs merge-base) exceeds its budget. Every hunk
 *    above a budget is a future merge conflict against official/main. Warns
 *    by default; fails with --strict or DECOUPLING_STRICT=1 once budgets are
 *    trusted. Budgets are tightened, never loosened: each seam refactor
 *    lowers its file's budget to the new actual.
 *
 * 3. Unguarded report — upstream files modified by the fork but missing from
 *    the budget table, so new shared-file edits cannot slip in unnoticed.
 *
 * Run: node scripts/selfhost/check-decoupling.mjs [--strict]
 */

import { execSync } from 'node:child_process';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(fileURLToPath(new URL('../..', import.meta.url)));
const STRICT = process.argv.includes('--strict') || process.env.DECOUPLING_STRICT === '1';

// Upstream-shared files this fork is allowed to touch, with a ceiling on the
// total changed lines (add+del) vs merge-base. Lines here are the "seams".
const BUDGETS = {
  // root seam trio — the canonical SELFHOSTED hooks, kept but bounded
  'apps/readest-app/src/utils/supabase.ts': 50,
  'apps/readest-app/src/services/environment.ts': 24,
  'apps/readest-app/src/i18n/i18n.ts': 8,
  // seam files pending P2 convergence — budgets tighten as each lands
  'apps/readest-app/src/app/auth/components/EmailPasswordAuth.tsx': 16,
  'apps/readest-app/src/context/AuthContext.tsx': 56,
  'apps/readest-app/src/app/library/page.tsx': 16,
  'apps/readest-app/src/services/libraryService.ts': 18,
  'apps/readest-app/src/libs/storage.ts': 72,
  'apps/readest-app/src/helpers/auth.ts': 26,
  // build/CI/config — small, stable
  'apps/readest-app/package.json': 12,
  'package.json': 6,
  '.github/workflows/pull-request.yml': 22,
  '.github/workflows/codeql.yml': 6,
  'Dockerfile': 8,
  'apps/readest-app/scripts/tauri.mjs': 22,
  // upstream doc file: the ONLY allowed permanent diff is the FORK pointer
  'apps/readest-app/AGENTS.md': 2,
};

const sh = (cmd) => execSync(cmd, { cwd: ROOT, encoding: 'utf8' });

function resolveBase() {
  for (const ref of ['official/main', 'origin/main', 'main']) {
    try {
      return sh(`git merge-base ${ref} HEAD`).trim();
    } catch {
      // ref missing or unrelated — try the next candidate
    }
  }
  console.error('decoupling: no upstream ref (official/main, origin/main, main) found — cannot guard');
  process.exit(1);
}

const base = resolveBase();
let failed = false;

// 1. Shared translation files must be byte-identical to upstream.
const localeChanges = sh(`git diff --name-only ${base} HEAD -- apps/readest-app/public/locales`)
  .split('\n')
  .filter((f) => f.endsWith('/translation.json'));
if (localeChanges.length > 0) {
  failed = true;
  console.error('✛ FAIL: shared translation files diverge from upstream (use the selfhost namespace):');
  for (const f of localeChanges) console.error(`    ${f}`);
} else {
  console.log('✓ all locales/*/translation.json identical to upstream');
}

// 2. Budgets on protected upstream files.
const overBudget = [];
for (const [file, budget] of Object.entries(BUDGETS)) {
  let out = '';
  try {
    out = sh(`git diff --numstat ${base} HEAD -- ${file}`).trim();
  } catch {
    // path missing in one side — diff exits nonzero; treat as untracked
  }
  if (!out) continue;
  const [add, del] = out.split('\t');
  const changed = Number(add) + Number(del);
  const mark = changed > budget ? '✛ OVER' : '✓';
  if (changed > budget) overBudget.push(`${file}: ${changed} changed lines > budget ${budget}`);
  console.log(`${mark} ${file}: ${changed}/${budget}`);
}
if (overBudget.length > 0) {
  if (STRICT) {
    failed = true;
    console.error(`✛ FAIL (--strict): ${overBudget.length} protected file(s) over budget`);
  } else {
    console.warn(`⚠ WARN: ${overBudget.length} protected file(s) over budget (enable --strict to enforce)`);
  }
}

// 3. Upstream files modified but not in the budget table — report only.
const guarded = new Set(Object.keys(BUDGETS));
const modifiedUpstream = sh(`git diff --diff-filter=M --name-only ${base} HEAD`)
  .split('\n')
  .filter(Boolean)
  .filter((f) => !guarded.has(f) && !f.startsWith('apps/readest-app/src/__tests__'));
if (modifiedUpstream.length > 0) {
  console.warn(`⚠ INFO: ${modifiedUpstream.length} upstream file(s) modified outside the budget table:`);
  for (const f of modifiedUpstream) console.warn(`    ${f}`);
}

process.exit(failed ? 1 : 0);
