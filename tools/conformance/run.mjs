#!/usr/bin/env node
// Scenario runner + conformance scorer. See docs/conformance.md.
//
//   node run.mjs --base http://localhost:3000/api/v3 --token XYZ [--tier 1] [--report out.json]
//   node run.mjs --base https://api.github.com --readonly --spec dotcom      # self-test
import fs from 'node:fs';
import path from 'node:path';
import { HERE, loadSpec, indexOperations, makeValidatorFactory } from './lib/spec.mjs';
import { loadPriority } from './lib/priority.mjs';
import { Oracle, Blocked } from './lib/oracle.mjs';
import { buildReport, printSummary, renderMarkdown } from './lib/report.mjs';

const USAGE = `usage: node run.mjs --base URL [options]
  --base URL            API root, e.g. http://localhost:3000/api/v3, https://api.github.com, https://gitea.example/api/v1
  --token TOKEN         API token (or env CONFORMANCE_TOKEN)
  --auth-scheme S       Authorization scheme: token (default) | Bearer
  --spec NAME|FILE      ghes (default) | dotcom | path to an OpenAPI JSON file
  --tier N              only exercise operations of tier <= N (fixture setup always runs)
  --readonly            run read-only scenarios against an existing public repo (no token needed)
  --repo OWNER/NAME     repo for --readonly (default cli/cli)
  --org ORG             org the token can create repos in (enables org scenarios)
  --collaborator LOGIN  second existing user (enables collaborator / review-request scenarios)
  --collaborator-token T  API token of --collaborator (or env CONFORMANCE_COLLABORATOR_TOKEN);
                        enables scenarios that need a second actor (approve + pulls/dismiss-review)
  --only REGEX          only run scenarios whose name matches
  --keep                do not delete the fixture repository at the end
  --report FILE         write JSON report (default reports/latest.json)
  --md FILE             write Markdown summary (default reports/latest.md)
  --record FILE         record all HTTP exchanges to FILE
  --replay FILE         replay HTTP exchanges from FILE instead of the network
  --max-requests N      hard cap on HTTP requests (default: 30 with --readonly, unlimited otherwise)
  --strict-oneof        keep OpenAPI oneOf as JSON-Schema oneOf (default: relaxed to anyOf)
  --api-version V       X-GitHub-Api-Version header (default 2022-11-28)
  --verbose             print every call, not only failures`;

const argv = process.argv.slice(2);
const has = (n) => argv.includes(n);
const opt = (n, d) => { const i = argv.indexOf(n); return i >= 0 ? argv[i + 1] : d; };
if (has('--help') || has('-h') || !opt('--base')) { console.log(USAGE); process.exit(has('--base') ? 0 : 2); }

const base = opt('--base');
const readonly = has('--readonly');
const token = opt('--token', process.env.CONFORMANCE_TOKEN || '');
const maxTier = Number(opt('--tier', 3));
const reportFile = path.resolve(opt('--report', path.join(HERE, 'reports/latest.json')));
const mdFile = path.resolve(opt('--md', path.join(HERE, 'reports/latest.md')));
const specName = opt('--spec', 'ghes');
const onlyRe = opt('--only') ? new RegExp(opt('--only')) : null;

const { doc, lock } = loadSpec(specName);
const ops = indexOperations(doc);
const prio = loadPriority();
const validators = makeValidatorFactory(doc, { strictOneOf: has('--strict-oneof') });

const oracle = new Oracle({
  ops, validators, prio, base, token,
  authScheme: opt('--auth-scheme', 'token'),
  apiVersion: opt('--api-version', '2022-11-28'),
  maxTier,
  record: opt('--record') ? path.resolve(opt('--record')) : null,
  replay: opt('--replay') ? path.resolve(opt('--replay')) : null,
  maxRequests: Number(opt('--max-requests', readonly ? 30 : Infinity)),
  verbose: has('--verbose'),
});

const state = oracle.state;
Object.assign(state, {
  org: opt('--org'), collaborator: opt('--collaborator'), keep: has('--keep'),
  collaboratorToken: opt('--collaborator-token', process.env.CONFORMANCE_COLLABORATOR_TOKEN || '') || null,
  runId: `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`,
});

const startedAt = new Date().toISOString();
const scenarioResults = [];

async function main() {
  let scenarios;
  if (readonly) {
    const [owner, repo] = opt('--repo', 'cli/cli').split('/');
    state.rp = { owner, repo };
    // GET /rate_limit does not count against the quota; check budget first.
    const rl = await oracle.call('rate-limit/get', { fixture: true });
    const remaining = rl.body?.resources?.core?.remaining ?? rl.body?.rate?.remaining;
    console.log(`rate limit: ${remaining} core requests remaining`);
    if (remaining !== undefined && remaining < 28 && !oracle.replay) throw new Error(`only ${remaining} requests left; wait for reset (${new Date((rl.body.resources.core.reset) * 1000).toISOString()})`);
    scenarios = (await import('./scenarios/readonly.mjs')).scenarios;
  } else {
    if (!token) throw new Error('--token is required for write scenarios (or use --readonly)');
    scenarios = (await import('./scenarios/write.mjs')).scenarios;
    // Area files scenarios/write-<area>.mjs (one per feature owner, so edits don't conflict) run
    // after the core scenarios and before 'cleanup', in file-name order.
    const extra = [];
    for (const f of fs.readdirSync(path.join(HERE, 'scenarios')).filter((f) => /^write-.+\.mjs$/.test(f)).sort()) {
      extra.push(...(await import(`./scenarios/${f}`)).scenarios);
    }
    const at = scenarios.findIndex((sc) => sc.name === 'cleanup');
    scenarios = at < 0 ? [...scenarios, ...extra] : [...scenarios.slice(0, at), ...extra, ...scenarios.slice(at)];
  }
  for (const sc of scenarios) {
    if (onlyRe && !onlyRe.test(sc.name) && !sc.always) { scenarioResults.push({ name: sc.name, status: 'filtered' }); continue; }
    const missing = (sc.requires || []).filter((k) => !state[k]);
    if (missing.length) { scenarioResults.push({ name: sc.name, status: 'skipped', error: `requires ${missing.join(', ')}` }); console.log(`-- ${sc.name}: skipped (requires ${missing.join(', ')})`); continue; }
    console.log(`-- ${sc.name}`);
    try {
      await sc.run(oracle, state);
      scenarioResults.push({ name: sc.name, status: 'passed' });
    } catch (e) {
      const status = e instanceof Blocked ? 'blocked' : 'errored';
      console.log(`   !! ${status}: ${e.message}`);
      scenarioResults.push({ name: sc.name, status, error: e instanceof Blocked ? e.message : `${e.message} @ ${e.stack?.split('\n')[1]?.trim()}` });
    }
  }
}

let fatal = null;
try { await main(); } catch (e) { fatal = e; console.error(`fatal: ${e.message}`); }
oracle.saveRecording();

const report = buildReport({
  ops, oracle, prio, scenarioResults,
  meta: {
    base, readonly, maxTier, startedAt, finishedAt: new Date().toISOString(), requests: oracle.requests,
    replay: !!oracle.replay, fatal: fatal?.message || null,
    spec: { repo: lock.repo, sha: lock.sha, file: lock.files[specName]?.upstream || specName, apiVersion: doc.info.version },
  },
});
fs.mkdirSync(path.dirname(reportFile), { recursive: true });
fs.mkdirSync(path.dirname(mdFile), { recursive: true });
fs.writeFileSync(reportFile, JSON.stringify(report, null, 1) + '\n');
fs.writeFileSync(mdFile, renderMarkdown(report));
printSummary(report);
console.log(`\nHTTP requests: ${oracle.requests}\nreport: ${path.relative(process.cwd(), reportFile)}\nsummary: ${path.relative(process.cwd(), mdFile)}`);
process.exit(fatal ? 1 : 0);
