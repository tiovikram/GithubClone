#!/usr/bin/env node
// Writes catalogue.json (every operation in the pinned spec with its tier) and
// checks that every operationId curated in priority.json exists.
//   node catalogue.mjs [--spec ghes|dotcom] [--print]
import fs from 'node:fs';
import path from 'node:path';
import { HERE, loadSpec, indexOperations } from './lib/spec.mjs';
import { loadPriority } from './lib/priority.mjs';

const args = process.argv.slice(2);
const opt = (n, d) => { const i = args.indexOf(n); return i >= 0 ? args[i + 1] : d; };
const which = opt('--spec', 'ghes');

const prio = loadPriority();
let bad = 0;
const results = {};
for (const w of ['ghes', 'dotcom']) {
  const { doc } = loadSpec(w);
  const ops = indexOperations(doc);
  const ids = new Set(ops.map((o) => o.operationId));
  for (const id of [...prio.tier1, ...prio.tier2]) {
    if (!ids.has(id)) {
      console.log(`${w === which ? 'ERROR' : 'note '}  ${id} (${prio.groupOf.get(id)}) not in ${w} spec`);
      if (w === which) bad++;
    }
  }
  results[w] = { doc, ops };
}

const { doc, ops } = results[which];
const lock = JSON.parse(fs.readFileSync(path.join(HERE, 'spec.lock.json'), 'utf8'));
const catalogue = {
  source: { repo: lock.repo, sha: lock.sha, spec: which, file: lock.files[which].upstream, apiVersion: doc.info.version },
  counts: { total: ops.length, tier1: 0, tier2: 0, tier3: 0 },
  operations: ops.map((o) => ({ operationId: o.operationId, method: o.method, path: o.path, tag: o.tag,
    tier: prio.tierOf(o.operationId), summary: o.summary })),
};
for (const o of catalogue.operations) catalogue.counts[`tier${o.tier}`]++;
const out = path.join(HERE, `catalogue.${which}.json`);
fs.writeFileSync(out, JSON.stringify(catalogue, null, 1) + '\n');
console.log(`${which}: ${ops.length} operations; tier1=${catalogue.counts.tier1} tier2=${catalogue.counts.tier2} tier3=${catalogue.counts.tier3} -> ${path.relative(process.cwd(), out)}`);
if (args.includes('--print')) for (const o of catalogue.operations) console.log(`${o.tier} ${o.method.padEnd(6)} ${o.path.padEnd(70)} ${o.operationId}`);
if (bad) { console.error(`${bad} curated operationIds missing from ${which} spec`); process.exit(1); }
