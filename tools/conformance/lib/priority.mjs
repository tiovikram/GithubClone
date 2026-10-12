import fs from 'node:fs';
import path from 'node:path';
import { HERE } from './spec.mjs';

/** Returns { tierOf(opId) -> 1|2|3, tier1: Set, tier2: Set, groupOf: Map } */
export function loadPriority(file = path.join(HERE, 'priority.json')) {
  const raw = JSON.parse(fs.readFileSync(file, 'utf8'));
  const tiers = { 1: new Set(), 2: new Set() };
  const groupOf = new Map();
  for (const [t, key] of [[1, 'tier1'], [2, 'tier2']]) {
    for (const [group, ids] of Object.entries(raw[key])) {
      if (group.startsWith('$')) continue;
      for (const id of ids) {
        if (groupOf.has(id)) throw new Error(`priority.json: ${id} listed twice (${groupOf.get(id)} and ${key}.${group})`);
        tiers[t].add(id);
        groupOf.set(id, `${key}.${group}`);
      }
    }
  }
  return {
    tier1: tiers[1], tier2: tiers[2], groupOf,
    tierOf: (id) => (tiers[1].has(id) ? 1 : tiers[2].has(id) ? 2 : 3),
  };
}
