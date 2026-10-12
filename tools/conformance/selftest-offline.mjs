#!/usr/bin/env node
// Offline regression test for the validator (no network):
//  1. positive control: every recorded real api.github.com response
//     (fixtures/github-cli-cli.recording.json) must validate as conforming
//     against both the dotcom and the GHES description;
//  2. negative controls: deliberately broken copies of those responses must
//     be rejected (guards against a converter that accepts everything).
// Refresh the recording with:
//   node run.mjs --base https://api.github.com --readonly --spec dotcom --record fixtures/github-cli-cli.recording.json
import fs from 'node:fs';
import path from 'node:path';
import { HERE, loadSpec, indexOperations, makeValidatorFactory } from './lib/spec.mjs';
import { loadPriority } from './lib/priority.mjs';
import { Oracle } from './lib/oracle.mjs';

const REC = path.join(HERE, 'fixtures/github-cli-cli.recording.json');
const rec = JSON.parse(fs.readFileSync(REC, 'utf8'));
let failures = 0;
const fail = (m) => { failures++; console.log(`FAIL ${m}`); };

// Map each recorded URL to the operation the readonly scenarios used for it.
const ROUTES = [
  [/\/rate_limit$/, 'rate-limit/get'], [/\/meta$/, 'meta/get'], [/^https:\/\/api\.github\.com\/?$/, 'meta/root'],
  [/\/users\/[^/]+$/, 'users/get-by-username'], [/\/repos\/[^/]+\/[^/]+$/, 'repos/get'],
  [/\/branches\?/, 'repos/list-branches'], [/\/branches\/[^/?]+$/, 'repos/get-branch'], [/\/git\/ref\//, 'git/get-ref'],
  [/\/commits\?/, 'repos/list-commits'], [/\/repos\/[^/]+\/[^/]+\/commits\/[0-9a-f]{40}$/, 'repos/get-commit'],
  [/\/git\/commits\//, 'git/get-commit'], [/\/tags\?/, 'repos/list-tags'], [/\/contents\//, 'repos/get-content'],
  [/\/issues\?/, 'issues/list-for-repo'], [/\/issues\/\d+$/, 'issues/get'], [/\/issues\/\d+\/comments/, 'issues/list-comments'],
  [/\/labels\?/, 'issues/list-labels-for-repo'], [/\/milestones\?/, 'issues/list-milestones'],
  [/\/pulls\?/, 'pulls/list'], [/\/pulls\/\d+$/, 'pulls/get'], [/\/pulls\/\d+\/files/, 'pulls/list-files'],
  [/\/releases\?/, 'repos/list-releases'], [/\/releases\/latest$/, 'repos/get-latest-release'], [/\/releases\/tags\//, 'repos/get-release-by-tag'],
];
const opFor = (url) => ROUTES.find(([re]) => re.test(url))?.[1];

for (const which of ['dotcom', 'ghes']) {
  const { doc } = loadSpec(which);
  const o = new Oracle({ ops: indexOperations(doc), validators: makeValidatorFactory(doc), prio: loadPriority(), base: 'https://api.github.com', replay: null });
  o.replay = new Map(); // disables the Link-origin check (recorded links point at api.github.com anyway)
  let n = 0;
  for (const ex of rec) {
    const id = opFor(ex.url);
    if (!id) { fail(`${which}: no route mapping for ${ex.url}`); continue; }
    const r = o.validate(o.op(id), { status: ex.status, headers: new Headers(ex.headers), text: ex.text }, {});
    n++;
    if (r.verdict !== 'conforming') fail(`${which}: real response for ${id} (${ex.url}) rejected: ${r.errors.slice(0, 3).join(' | ')}`);
  }
  console.log(`${which}: ${n} recorded real responses checked`);

  // negative controls
  const find = (re) => rec.find((x) => re.test(x.url));
  const mutations = [
    ['repos/get', /\/repos\/cli\/cli$/, (b) => { delete b.full_name; }, 'missing required property'],
    ['repos/get', /\/repos\/cli\/cli$/, (b) => { b.id = String(b.id); }, 'integer as string'],
    ['repos/get', /\/repos\/cli\/cli$/, (b) => { b.owner = null; }, 'null for non-nullable object'],
    ['repos/get', /\/repos\/cli\/cli$/, (b) => { b.created_at = 'yesterday'; }, 'bad date-time format'],
    ['repos/get', /\/repos\/cli\/cli$/, (b) => { b.html_url = 'not a uri'; }, 'bad uri format'],
    ['pulls/list', /\/pulls\?/, (b) => { delete b[0].head.sha; }, 'nested required in array item'],
    ['pulls/get', /\/pulls\/\d+$/, (b) => { b.state = 'merged'; }, 'enum violation'],
    ['issues/get', /\/issues\/\d+$/, (b) => { b.user.login = 42; }, 'nested type through $ref'],
    ['repos/get-content', /\/contents\//, (b) => { delete b.content; delete b.sha; }, 'union (oneOf) with no matching branch'],
    ['repos/list-releases', /\/releases\?/, (b) => ({ releases: b }), 'object instead of array'],
  ];
  for (const [id, re, mut, label] of mutations) {
    const ex = find(re);
    if (!ex) { fail(`no recorded response for ${label}`); continue; }
    const body = JSON.parse(ex.text);
    const out = mut(body) ?? body;
    const r = o.validate(o.op(id), { status: 200, headers: new Headers(ex.headers), text: JSON.stringify(out) }, {});
    if (r.verdict === 'conforming') fail(`${which}: mutation "${label}" on ${id} was NOT detected`);
  }
  // nullable accepted, extra properties allowed
  const ex = find(/\/repos\/cli\/cli$/);
  const b = JSON.parse(ex.text); b.description = null; b.some_new_field = { x: 1 };
  if (o.validate(o.op('repos/get'), { status: 200, headers: new Headers(ex.headers), text: JSON.stringify(b) }, {}).verdict !== 'conforming') fail(`${which}: nullable/additionalProperties wrongly rejected`);
  // undocumented status / not implemented classification
  if (o.validate(o.op('repos/get'), { status: 404, headers: new Headers(), text: '{"message":"Not Found"}' }, {}).verdict !== 'not_implemented') fail('404 not classified as not_implemented');
  if (o.validate(o.op('repos/get'), { status: 418, headers: new Headers(), text: '' }, {}).verdict !== 'nonconforming') fail('undocumented status not flagged');
  console.log(`${which}: ${mutations.length} negative controls + 3 classification checks run`);
}
if (failures) { console.log(`\n${failures} failure(s)`); process.exit(1); }
console.log('\nselftest-offline: OK');
