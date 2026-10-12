// Scoring + report rendering (JSON and Markdown).
import fs from 'node:fs';
import path from 'node:path';
import { HERE } from './spec.mjs';

const STATUSES = ['conforming', 'nonconforming', 'not_implemented', 'untested'];

/** operationIds mentioned as string literals in scenario sources = ops that have a scenario. */
export function scenarioCoverage(ids) {
  const dir = path.join(HERE, 'scenarios');
  const found = new Set();
  for (const f of fs.readdirSync(dir).filter((x) => x.endsWith('.mjs'))) {
    const src = fs.readFileSync(path.join(dir, f), 'utf8');
    for (const m of src.matchAll(/['"`]([a-z0-9-]+\/[a-z0-9-]+)['"`]/g)) if (ids.has(m[1])) found.add(m[1]);
  }
  return found;
}

const empty = () => ({ total: 0, conforming: 0, nonconforming: 0, not_implemented: 0, untested: 0 });
const pct = (a, b) => (b ? Math.round((1000 * a) / b) / 10 : 0);
function finish(b) {
  const tested = b.total - b.untested;
  return { ...b, tested, pct_conforming: pct(b.conforming, b.total), pct_conforming_of_tested: tested ? pct(b.conforming, tested) : null };
}

export function buildReport({ ops, oracle, prio, meta, scenarioResults }) {
  const ids = new Set(ops.map((o) => o.operationId));
  const covered = scenarioCoverage(ids);
  const operations = ops.map((o) => {
    const r = oracle.results.get(o.operationId);
    const tier = prio.tierOf(o.operationId);
    const base = { operationId: o.operationId, method: o.method, path: o.path, tag: o.tag, tier };
    if (!r) return { ...base, status: 'untested', reason: covered.has(o.operationId) ? 'scenario not reached (blocked, skipped by --tier/--readonly, or optional fixture missing)' : 'no scenario' };
    const errors = [...new Set(r.calls.flatMap((c) => c.errors))].slice(0, 8);
    return { ...base, status: r.verdict, errors, calls: r.calls.map(({ method, url, status, verdict, note }) => ({ method, url, status, verdict, note })) };
  });
  const overall = empty(), tiers = { 1: empty(), 2: empty(), 3: empty() }, byTag = {};
  for (const o of operations) {
    for (const b of [overall, tiers[o.tier], (byTag[o.tag] ||= empty())]) { b.total++; b[o.status]++; }
  }
  return {
    meta,
    summary: {
      overall: finish(overall),
      tier1: finish(tiers[1]), tier2: finish(tiers[2]), tier3: finish(tiers[3]),
      byTag: Object.fromEntries(Object.entries(byTag).sort((a, b) => b[1].total - a[1].total).map(([k, v]) => [k, finish(v)])),
      scenariosWithCoverage: covered.size,
    },
    scenarios: scenarioResults,
    harnessWarnings: oracle.harnessWarnings,
    operations,
  };
}

const row = (name, b) => `| ${name} | ${b.total} | ${b.conforming} | ${b.nonconforming} | ${b.not_implemented} | ${b.untested} | **${b.pct_conforming}%** | ${b.pct_conforming_of_tested === null ? '–' : `${b.pct_conforming_of_tested}%`} |`;
const HEAD = '| scope | ops | conforming | nonconforming | not impl. | untested | % conforming | % of tested |\n|---|---:|---:|---:|---:|---:|---:|---:|';

export function printSummary(rep) {
  const s = rep.summary;
  const line = (n, b) => `${n.padEnd(8)} ${String(b.conforming).padStart(4)}/${String(b.total).padEnd(5)} conforming = ${String(b.pct_conforming).padStart(5)}%   (tested ${b.tested}: ${b.nonconforming} nonconforming, ${b.not_implemented} not implemented${b.tested ? `; ${b.pct_conforming_of_tested}% of tested conform` : ''})`;
  console.log('\n== Conformance summary ==');
  console.log(line('overall', s.overall));
  console.log(line('tier 1', s.tier1));
  console.log(line('tier 2', s.tier2));
  console.log(line('tier 3', s.tier3));
  const tags = Object.entries(s.byTag).filter(([, b]) => b.tested);
  if (tags.length) {
    console.log('\nper tag (tested tags only):');
    for (const [t, b] of tags) console.log(`  ${t.padEnd(18)} ${b.conforming}/${b.total} (${b.pct_conforming}%), tested ${b.tested}, nonconf ${b.nonconforming}, n/i ${b.not_implemented}`);
  }
  const failed = rep.scenarios.filter((x) => x.status !== 'passed');
  if (failed.length) {
    console.log('\nscenarios not completed:');
    for (const f of failed) console.log(`  [${f.status}] ${f.name}: ${f.error}`);
  }
  if (rep.harnessWarnings.length) console.log(`\n${rep.harnessWarnings.length} harness warnings (see report)`);
}

export function renderMarkdown(rep) {
  const s = rep.summary, m = rep.meta;
  const out = [];
  out.push(`# REST conformance report`, '');
  out.push(`- **Target:** \`${m.base}\`${m.readonly ? ' (read-only scenarios)' : ''}`);
  out.push(`- **Spec:** \`${m.spec.file}\` @ [\`${m.spec.sha.slice(0, 10)}\`](https://github.com/${m.spec.repo}/tree/${m.spec.sha}) (API ${m.spec.apiVersion})`);
  out.push(`- **Run:** ${m.startedAt} · ${m.requests} HTTP requests · max tier ${m.maxTier}${m.replay ? ' · replayed from recording' : ''}`);
  out.push('', '## Score', '', HEAD);
  out.push(row('**all operations**', s.overall), row('tier 1 (gh CLI core)', s.tier1), row('tier 2 (octokit common)', s.tier2), row('tier 3 (rest)', s.tier3));
  out.push('', '## Per tag', '', HEAD.replace('scope', 'tag'));
  for (const [t, b] of Object.entries(s.byTag)) out.push(row(t, b));
  const bad = rep.operations.filter((o) => o.status === 'nonconforming' || o.status === 'not_implemented').sort((a, b) => a.tier - b.tier);
  out.push('', `## Failing operations (${bad.length})`, '');
  if (!bad.length) out.push('_none_');
  for (const o of bad) {
    out.push(`- **${o.operationId}** (tier ${o.tier}, \`${o.method} ${o.path}\`): ${o.status}`);
    for (const e of o.errors.slice(0, 4)) out.push(`  - \`${e.replace(/`/g, "'").slice(0, 300)}\``);
  }
  const conf = rep.operations.filter((o) => o.status === 'conforming');
  out.push('', `## Conforming operations (${conf.length})`, '');
  out.push(conf.map((o) => `\`${o.operationId}\``).join(', ') || '_none_');
  const untestedT12 = rep.operations.filter((o) => o.status === 'untested' && o.tier <= 2);
  out.push('', `## Untested tier 1/2 operations (${untestedT12.length})`, '');
  for (const o of untestedT12) out.push(`- \`${o.operationId}\` (tier ${o.tier}): ${o.reason}`);
  out.push('', '## Scenarios', '');
  for (const x of rep.scenarios) out.push(`- ${x.status === 'passed' ? 'completed' : x.status}: ${x.name}${x.error ? ` — ${x.error.slice(0, 200)}` : ''}`);
  if (rep.harnessWarnings.length) {
    out.push('', '## Harness warnings', '', ...[...new Set(rep.harnessWarnings)].map((w) => `- ${w}`));
  }
  return out.join('\n') + '\n';
}
