// Area "runner" scenarios (docs/BUILDOUT.md): GitHub Actions execution. A fixture repository
// gets workflows; a real `forge runner` process (spawned from target/debug/forge, or
// $FORGE_BIN) registers with a repository registration token and runs the jobs, so runs,
// attempts, jobs, logs, artifacts and caches carry real data. Also: self-hosted runner
// management (repository, organization with --org, enterprise), JIT configs, cache usage and
// policy, organization OIDC subject template, pending deployments (environment reviewers)
// and custom deployment protection rule reviews.
//
// The runner process is killed and every fixture deleted at the end.
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { Blocked } from '../lib/oracle.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const FORGE_BIN = process.env.FORGE_BIN || path.resolve(HERE, '../../../target/debug/forge');
const ENT = process.env.FORGE_ENTERPRISE_SLUG || 'forge';
const b64 = (str) => Buffer.from(str, 'utf8').toString('base64');
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const CI = `name: Conf CI
on:
  push:
  workflow_dispatch:
jobs:
  build:
    runs-on: ubuntu-latest
    strategy:
      matrix:
        n: [1, 2]
    outputs:
      val: \${{ steps.o.outputs.val }}
    steps:
      - uses: actions/checkout@v4
      - id: o
        run: echo "val=v\${{ matrix.n }}" >> "$GITHUB_OUTPUT"
      - run: mkdir -p dist && echo "artifact \${{ matrix.n }}" > dist/a.txt
      - uses: actions/upload-artifact@v4
        with:
          name: dist-\${{ matrix.n }}
          path: dist/
      - uses: actions/cache@v4
        with:
          path: cached
          key: conf-\${{ matrix.n }}-\${{ github.run_id }}-\${{ github.run_attempt }}
      - run: mkdir -p cached && echo x > cached/f
  flaky:
    needs: build
    runs-on: ubuntu-latest
    steps:
      - run: |
          echo "needs=\${{ needs.build.outputs.val }}"
          if [ "\${{ github.run_attempt }}" = 1 ]; then exit 1; fi
`;

const SLOW = `name: Conf Slow
on: workflow_dispatch
jobs:
  sleepy:
    runs-on: ubuntu-latest
    steps:
      - run: echo started && sleep 300
`;

const DEPLOY = `name: Conf Deploy
on: workflow_dispatch
jobs:
  ship:
    runs-on: ubuntu-latest
    environment: conf-review
    steps:
      - run: echo shipped
`;

const GATED = `name: Conf Gated
on: workflow_dispatch
jobs:
  ship:
    runs-on: ubuntu-latest
    environment: conf-gate
    steps:
      - run: echo gated
`;

/** Polls a run until pred(run) (unrecorded). */
async function waitRun(t, rp, runId, pred, timeoutMs = 180000) {
  const end = Date.now() + timeoutMs;
  let r;
  while (Date.now() < end) {
    r = await t.probe('actions/get-workflow-run', { params: { ...rp, run_id: runId } });
    if (r.status === 200 && pred(r.body)) {
      if (r.body.status !== 'completed') return r.body;
      const j = await t.probe('actions/list-jobs-for-workflow-run', { params: { ...rp, run_id: runId } });
      if ((j.body?.jobs || []).every((x) => x.status === 'completed')) return r.body;
    }
    await sleep(1000);
  }
  throw new Blocked(`run ${runId} did not reach the expected state (last: ${r?.body?.status}/${r?.body?.conclusion})`);
}

async function waitJob(t, rp, runId, pred, timeoutMs = 120000) {
  const end = Date.now() + timeoutMs;
  while (Date.now() < end) {
    const j = await t.probe('actions/list-jobs-for-workflow-run', { params: { ...rp, run_id: runId } });
    const hit = (j.body?.jobs || []).find(pred);
    if (hit) return hit;
    await sleep(500);
  }
  throw new Blocked(`run ${runId}: no job reached the expected state`);
}

async function latestRun(t, rp, query) {
  for (let i = 0; i < 60; i++) {
    const r = await t.probe('actions/list-workflow-runs-for-repo', { params: rp, query });
    if (r.body?.workflow_runs?.length) return r.body.workflow_runs[0];
    await sleep(500);
  }
  throw new Blocked(`no workflow run for ${JSON.stringify(query)}`);
}

async function putFile(t, rp, filePath, content) {
  await t.call('repos/create-or-update-file-contents', { params: { ...rp, path: filePath }, body: { message: `add ${filePath}`, content: b64(content) }, expect: [201], fixture: true });
}

function startRunner(t, s, token, name) {
  if (!fs.existsSync(FORGE_BIN)) throw new Blocked(`forge binary not found at ${FORGE_BIN} (set FORGE_BIN)`);
  const server = t.base.replace(/\/api\/v3$/, '');
  const work = fs.mkdtempSync(path.join(os.tmpdir(), 'forge-conf-runner-'));
  const log = fs.openSync(path.join(work, 'runner.log'), 'a');
  const child = spawn(FORGE_BIN, ['runner', '--url', server, '--token', token, '--name', name, '--work-dir', work, '--slots', '2'], {
    stdio: ['ignore', log, log],
    env: { PATH: process.env.PATH, HOME: process.env.HOME, RUST_LOG: 'forge=info' },
  });
  s.runnerProcs = s.runnerProcs || [];
  s.runnerProcs.push({ child, work });
  return child;
}

export const scenarios = [
  {
    name: 'runner: fixture repository and runner process',
    requires: ['login'],
    run: async (t, s) => {
      const r = await t.call('repos/create-for-authenticated-user', { body: { name: `conf-runner-${s.runId}`, auto_init: true }, expect: [201], fixture: true });
      const rp = { owner: r.body.owner.login, repo: r.body.name };
      s.runnerRepo = rp;
      await t.waitFor('git/get-ref', { params: { ...rp, ref: 'heads/main' } }, (x) => x.status === 200);
      // Runner management on a JIT runner (never started).
      await t.call('actions/list-runner-applications-for-repo', { params: rp, minItems: 1 });
      const jit = await t.call('actions/generate-runner-jitconfig-for-repo', { params: rp, body: { name: `conf-jit-${s.runId}`, runner_group_id: 1, labels: ['self-hosted', 'linux'] }, expect: [201], fixture: true });
      const rid = { ...rp, runner_id: jit.body.runner.id };
      await t.call('actions/get-self-hosted-runner-for-repo', { params: rid });
      await t.call('actions/list-labels-for-self-hosted-runner-for-repo', { params: rid, minItems: 1, itemsKey: 'labels' });
      await t.call('actions/add-custom-labels-to-self-hosted-runner-for-repo', { params: rid, body: { labels: ['conf-a', 'conf-b'] } });
      await t.call('actions/set-custom-labels-for-self-hosted-runner-for-repo', { params: rid, body: { labels: ['conf-c', 'conf-d'] } });
      await t.call('actions/remove-custom-label-from-self-hosted-runner-for-repo', { params: { ...rid, name: 'conf-d' } });
      await t.call('actions/remove-all-custom-labels-from-self-hosted-runner-for-repo', { params: rid });
      await t.call('actions/delete-self-hosted-runner-from-repo', { params: rid, expect: [204] });
      await t.call('actions/create-remove-token-for-repo', { params: rp, expect: [201] });
      // The real runner.
      const tok = await t.call('actions/create-registration-token-for-repo', { params: rp, expect: [201], fixture: true });
      startRunner(t, s, tok.body.token, `conf-runner-${s.runId}`);
      for (let i = 0; i < 60; i++) {
        const l = await t.probe('actions/list-self-hosted-runners-for-repo', { params: rp });
        if (l.body?.runners?.some((x) => x.status === 'online')) break;
        await sleep(500);
      }
      await t.call('actions/list-self-hosted-runners-for-repo', { params: rp, minItems: 1, itemsKey: 'runners' });
      s.runnerReady = true;
    },
  },
  {
    name: 'runner: push workflow, runs, jobs, logs, artifacts, caches, re-runs',
    requires: ['runnerRepo', 'runnerReady'],
    run: async (t, s) => {
      const rp = s.runnerRepo;
      await putFile(t, rp, '.github/workflows/ci.yml', CI);
      const first = await latestRun(t, rp, { event: 'push' });
      const runId = first.id;
      let run = await waitRun(t, rp, runId, (r) => r.status === 'completed');
      if (run.conclusion !== 'failure') throw new Blocked(`expected the flaky job to fail attempt 1, got ${run.conclusion}`);
      const P = { ...rp, run_id: runId };
      await t.call('actions/list-repo-workflows', { params: rp, minItems: 1, itemsKey: 'workflows' });
      await t.call('actions/get-workflow', { params: { ...rp, workflow_id: 'ci.yml' } });
      await t.call('actions/list-workflow-runs', { params: { ...rp, workflow_id: 'ci.yml' }, minItems: 1, itemsKey: 'workflow_runs' });
      await t.call('actions/list-workflow-runs-for-repo', { params: rp, query: { status: 'failure' }, minItems: 1, itemsKey: 'workflow_runs' });
      await t.call('actions/get-workflow-run', { params: P });
      await t.call('actions/get-workflow-run-attempt', { params: { ...P, attempt_number: 1 } });
      const jobs = await t.call('actions/list-jobs-for-workflow-run', { params: P, minItems: 3, itemsKey: 'jobs' });
      await t.call('actions/list-jobs-for-workflow-run-attempt', { params: { ...P, attempt_number: 1 }, minItems: 3, itemsKey: 'jobs' });
      const job = jobs.body.jobs[0];
      await t.call('actions/get-job-for-workflow-run', { params: { ...rp, job_id: job.id } });
      await t.call('actions/download-job-logs-for-workflow-run', { params: { ...rp, job_id: job.id }, expect: [302] });
      await t.call('actions/download-workflow-run-logs', { params: P, expect: [302] });
      await t.call('actions/download-workflow-run-attempt-logs', { params: { ...P, attempt_number: 1 }, expect: [302] });
      const arts = await t.call('actions/list-workflow-run-artifacts', { params: P, minItems: 2, itemsKey: 'artifacts' });
      await t.call('actions/list-artifacts-for-repo', { params: rp, minItems: 2, itemsKey: 'artifacts' });
      const art = arts.body.artifacts[0];
      await t.call('actions/get-artifact', { params: { ...rp, artifact_id: art.id } });
      await t.call('actions/download-artifact', { params: { ...rp, artifact_id: art.id, archive_format: 'zip' }, expect: [302] });
      const caches = await t.call('actions/get-actions-cache-list', { params: rp, minItems: 2, itemsKey: 'actions_caches' });
      await t.call('actions/get-actions-cache-usage', { params: rp });
      await t.call('actions/get-actions-cache-usage-policy', { params: rp });
      await t.call('actions/set-actions-cache-usage-policy', { params: rp, body: { repo_cache_size_limit_in_gb: 5 }, expect: [204] });
      await t.call('actions/get-reviews-for-run', { params: P });
      await t.call('actions/get-pending-deployments-for-run', { params: P });
      // Re-run the failed job (attempt 2 succeeds), then one job, then everything.
      await t.call('actions/re-run-workflow-failed-jobs', { params: P, body: {}, expect: [201] });
      run = await waitRun(t, rp, runId, (r) => r.run_attempt === 2 && r.status === 'completed');
      const j2 = await t.call('actions/list-jobs-for-workflow-run', { params: P, minItems: 3, itemsKey: 'jobs' });
      const flaky = j2.body.jobs.find((x) => x.name === 'flaky');
      await t.call('actions/re-run-job-for-workflow-run', { params: { ...rp, job_id: flaky.id }, body: {}, expect: [201] });
      await waitRun(t, rp, runId, (r) => r.run_attempt === 3 && r.status === 'completed');
      await t.call('actions/re-run-workflow', { params: P, body: {}, expect: [201] });
      await waitRun(t, rp, runId, (r) => r.run_attempt === 4 && r.status === 'completed');
      // Deletions.
      const cache = caches.body.actions_caches;
      await t.call('actions/delete-actions-cache-by-id', { params: { ...rp, cache_id: cache[0].id }, expect: [204] });
      await t.call('actions/delete-actions-cache-by-key', { params: rp, query: { key: cache[1].key }, expect: [200] });
      // re-run attempts replaced the artifacts of attempt 1: delete a current one
      const now = await t.probe('actions/list-workflow-run-artifacts', { params: P });
      const cur = now.body?.artifacts?.[0];
      if (cur) await t.call('actions/delete-artifact', { params: { ...rp, artifact_id: cur.id }, expect: [204] });
      await t.call('actions/delete-workflow-run-logs', { params: P, expect: [204] });
      await t.call('actions/disable-workflow', { params: { ...rp, workflow_id: 'ci.yml' }, expect: [204] });
      await t.call('actions/enable-workflow', { params: { ...rp, workflow_id: 'ci.yml' }, expect: [204] });
      await t.call('actions/delete-workflow-run', { params: P, expect: [204] });
    },
  },
  {
    name: 'runner: dispatch, cancel, force-cancel',
    requires: ['runnerRepo', 'runnerReady'],
    run: async (t, s) => {
      const rp = s.runnerRepo;
      await putFile(t, rp, '.github/workflows/slow.yml', SLOW);
      for (const op of ['actions/cancel-workflow-run', 'actions/force-cancel-workflow-run']) {
        const before = await t.probe('actions/list-workflow-runs-for-repo', { params: rp, query: { event: 'workflow_dispatch' } });
        await t.call('actions/create-workflow-dispatch', { params: { ...rp, workflow_id: 'slow.yml' }, body: { ref: 'main' }, expect: [204] });
        let run;
        for (let i = 0; i < 60; i++) {
          run = await latestRun(t, rp, { event: 'workflow_dispatch' });
          if (run.id !== before.body?.workflow_runs?.[0]?.id) break;
          await sleep(300);
        }
        await waitJob(t, rp, run.id, (j) => j.status === 'in_progress');
        await t.call(op, { params: { ...rp, run_id: run.id }, expect: [202] });
        await waitRun(t, rp, run.id, (r) => r.status === 'completed' && r.conclusion === 'cancelled');
      }
    },
  },
  {
    name: 'runner: environment reviews and deployment protection rules',
    requires: ['runnerRepo', 'runnerReady', 'me'],
    run: async (t, s) => {
      const rp = s.runnerRepo;
      await t.call('repos/create-or-update-environment', { params: { ...rp, environment_name: 'conf-review' }, body: { reviewers: [{ type: 'User', id: s.me.id }] }, fixture: true });
      await putFile(t, rp, '.github/workflows/deploy.yml', DEPLOY);
      await t.call('actions/create-workflow-dispatch', { params: { ...rp, workflow_id: 'deploy.yml' }, body: { ref: 'main' }, expect: [204] });
      const run = await latestRun(t, rp, { event: 'workflow_dispatch', status: 'waiting' }).catch(() => null);
      if (!run) throw new Blocked('the environment job did not wait for review');
      const P = { ...rp, run_id: run.id };
      const pend = await t.call('actions/get-pending-deployments-for-run', { params: P, minItems: 1 });
      const envId = pend.body?.[0]?.environment?.id;
      await t.call('actions/review-pending-deployments-for-run', { params: P, body: { environment_ids: [envId], state: 'approved', comment: 'conformance' }, expect: [200] });
      await waitRun(t, rp, run.id, (r) => r.status === 'completed');
      await t.call('actions/get-reviews-for-run', { params: P, minItems: 1 });
      // Custom deployment protection rules need an installed integration (apps area).
      const apps = await t.probe('repos/list-custom-deployment-rule-integrations', { params: { ...rp, environment_name: 'conf-review' } });
      const app = apps.body?.available_custom_deployment_protection_rule_integrations?.[0];
      if (!app) throw new Blocked('no custom deployment protection rule integration is installed (apps area): review-custom-gates-for-run not exercised');
      await t.call('repos/create-or-update-environment', { params: { ...rp, environment_name: 'conf-gate' }, body: {}, fixture: true });
      await t.call('repos/create-deployment-protection-rule', { params: { ...rp, environment_name: 'conf-gate' }, body: { integration_id: app.id }, expect: [201], fixture: true });
      await putFile(t, rp, '.github/workflows/gated.yml', GATED);
      await t.call('actions/create-workflow-dispatch', { params: { ...rp, workflow_id: 'gated.yml' }, body: { ref: 'main' }, expect: [204] });
      const g = await latestRun(t, rp, { event: 'workflow_dispatch', status: 'waiting' });
      await t.call('actions/review-custom-gates-for-run', { params: { ...rp, run_id: g.id }, body: { environment_name: 'conf-gate', state: 'approved', comment: 'gate ok' }, expect: [204] });
      await waitRun(t, rp, g.id, (r) => r.status === 'completed');
    },
  },
  {
    name: 'runner: organization runners, OIDC, cache usage',
    requires: ['org'],
    run: async (t, s) => {
      const org = { org: s.org };
      await t.call('oidc/get-oidc-custom-sub-template-for-org', { params: org });
      await t.call('oidc/update-oidc-custom-sub-template-for-org', { params: org, body: { include_claim_keys: ['repo', 'context', 'job_workflow_ref'] }, expect: [201] });
      await t.call('oidc/update-oidc-custom-sub-template-for-org', { params: org, body: { include_claim_keys: ['repo', 'context'] }, expect: [201] });
      await t.call('actions/get-actions-cache-usage-for-org', { params: org });
      await t.call('actions/get-actions-cache-usage-by-repo-for-org', { params: org });
      await t.call('actions/list-runner-applications-for-org', { params: org, minItems: 1 });
      await t.call('actions/create-registration-token-for-org', { params: org, expect: [201] });
      await t.call('actions/create-remove-token-for-org', { params: org, expect: [201] });
      const jit = await t.call('actions/generate-runner-jitconfig-for-org', { params: org, body: { name: `conf-org-jit-${s.runId}`, runner_group_id: 1, labels: ['self-hosted', 'linux'] }, expect: [201], fixture: true });
      const rid = { ...org, runner_id: jit.body.runner.id };
      await t.call('actions/list-self-hosted-runners-for-org', { params: org, minItems: 1, itemsKey: 'runners' });
      await t.call('actions/get-self-hosted-runner-for-org', { params: rid });
      await t.call('actions/list-labels-for-self-hosted-runner-for-org', { params: rid, minItems: 1, itemsKey: 'labels' });
      await t.call('actions/add-custom-labels-to-self-hosted-runner-for-org', { params: rid, body: { labels: ['conf-a'] } });
      await t.call('actions/set-custom-labels-for-self-hosted-runner-for-org', { params: rid, body: { labels: ['conf-a', 'conf-b'] } });
      await t.call('actions/remove-custom-label-from-self-hosted-runner-for-org', { params: { ...rid, name: 'conf-b' } });
      await t.call('actions/remove-all-custom-labels-from-self-hosted-runner-for-org', { params: rid });
      // Runner group membership (actions-settings ops) with a registered org runner.
      const g = await t.probe('actions/create-self-hosted-runner-group-for-org', { params: org, body: { name: `conf-runner-grp-${s.runId}`, visibility: 'all' } });
      if (g.status === 201) {
        const gp = { ...org, runner_group_id: g.body.id };
        await t.call('actions/add-self-hosted-runner-to-group-for-org', { params: { ...gp, runner_id: rid.runner_id }, expect: [204] });
        await t.call('actions/list-self-hosted-runners-in-group-for-org', { params: gp, minItems: 1, itemsKey: 'runners' });
        await t.call('actions/remove-self-hosted-runner-from-group-for-org', { params: { ...gp, runner_id: rid.runner_id }, expect: [204] });
        await t.call('actions/set-self-hosted-runners-in-group-for-org', { params: gp, body: { runners: [rid.runner_id] }, expect: [204] });
        await t.probe('actions/delete-self-hosted-runner-group-from-org', { params: gp });
      }
      await t.call('actions/delete-self-hosted-runner-from-org', { params: rid, expect: [204] });
    },
  },
  {
    name: 'runner: enterprise runners and cache policy',
    requires: ['login'],
    run: async (t, s) => {
      const e = { enterprise: ENT };
      await t.call('actions/get-actions-cache-usage-for-enterprise', { params: e });
      const pol = await t.call('actions/get-actions-cache-usage-policy-for-enterprise', { params: e });
      await t.call('actions/set-actions-cache-usage-policy-for-enterprise', { params: e, body: { repo_cache_size_limit_in_gb: 10, max_repo_cache_size_limit_in_gb: pol?.body?.max_repo_cache_size_limit_in_gb || 10000 }, expect: [204] });
      const jit = await t.call('actions/generate-runner-jitconfig-for-enterprise', { params: e, body: { name: `conf-ent-jit-${s.runId}`, runner_group_id: 1, labels: ['self-hosted', 'linux'] }, expect: [201], fixture: true });
      // The enterprise runner API (enterprise area) on a real enterprise runner.
      const rp = { ...e, runner_id: jit.body.runner.id };
      await t.call('enterprise-admin/get-self-hosted-runner-for-enterprise', { params: rp });
      await t.call('enterprise-admin/list-labels-for-self-hosted-runner-for-enterprise', { params: rp });
      await t.call('enterprise-admin/add-custom-labels-to-self-hosted-runner-for-enterprise', { params: rp, body: { labels: ['conf-a'] } });
      await t.call('enterprise-admin/set-custom-labels-for-self-hosted-runner-for-enterprise', { params: rp, body: { labels: ['conf-a', 'conf-b'] } });
      await t.call('enterprise-admin/remove-custom-label-from-self-hosted-runner-for-enterprise', { params: { ...rp, name: 'conf-b' } });
      await t.call('enterprise-admin/remove-all-custom-labels-from-self-hosted-runner-for-enterprise', { params: rp });
      await t.call('enterprise-admin/delete-self-hosted-runner-from-enterprise', { params: rp, expect: [204] });
    },
  },
  {
    name: 'runner: cleanup',
    always: true,
    run: async (t, s) => {
      for (const p of s.runnerProcs || []) {
        p.child.kill('SIGKILL');
        try { fs.rmSync(p.work, { recursive: true, force: true }); } catch { /* ignore */ }
      }
      if (s.runnerRepo && !s.keep) await t.call('repos/delete', { params: s.runnerRepo, expect: [204] });
    },
  },
];
