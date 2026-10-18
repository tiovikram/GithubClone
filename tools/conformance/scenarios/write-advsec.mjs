// Area "advsec" scenarios (docs/BUILDOUT.md): global advisories, code security configurations,
// secret scanning (push protection, bypasses, alerts, delegated dismissal/bypass requests),
// Dependabot (alerts from a vulnerable package.json, dismissal requests, secrets, repository
// access) and code scanning (SARIF uploads, analyses, alerts, default setup).
//
// Needs --org (an org repo carries the configuration). The advisory fixture
// (crates/forge/tests/fixtures/advisories/osv-sample.json) is loaded through forge's
// site-admin endpoint POST /api/forge/advisories; on other servers the Dependabot alert steps
// find no alerts. --collaborator/--collaborator-token enable the delegated dismissal and bypass
// request steps (a requester who is not a reviewer). Enterprise ops use
// $CONFORMANCE_ENTERPRISE (default `forge`) and a site-admin token. Everything created here is
// removed here, except the imported advisories (global, idempotent).
import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';
import { fileURLToPath } from 'node:url';
import { sealedBox } from './write-actions-settings.mjs';

const HERE = path.dirname(fileURLToPath(import.meta.url));
const FIXTURE = path.join(HERE, '../../../crates/forge/tests/fixtures/advisories/osv-sample.json');
const ENT = { enterprise: process.env.CONFORMANCE_ENTERPRISE || 'forge' };
const b64 = (str) => Buffer.from(str, 'utf8').toString('base64');
const forgeUrl = (t, p) => t.base.replace(/\/api\/v3\/?$/, '') + p;

/** A random-looking (fake) AWS access key id, generated so no literal secret lives in git. */
function fakeAwsKey() {
  const A = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ234567';
  let k = 'AKIA';
  for (let i = 0; i < 16; i++) k += A[Math.floor(Math.random() * A.length)];
  return k;
}

function sarifDoc(results) {
  return {
    version: '2.1.0',
    $schema: 'https://json.schemastore.org/sarif-2.1.0.json',
    runs: [{
      tool: { driver: { name: 'forge-conf-lint', semanticVersion: '1.0.0', rules: [
        { id: 'js/eval', name: 'NoEval', shortDescription: { text: 'Avoid eval' }, fullDescription: { text: 'eval executes arbitrary code.' },
          defaultConfiguration: { level: 'error' }, help: { text: 'Do not use eval.' }, helpUri: 'https://example.com/rules/eval',
          properties: { tags: ['security', 'external/cwe/cwe-95'], 'security-severity': '8.8' } },
        { id: 'js/unused-var', name: 'UnusedVariable', shortDescription: { text: 'Unused variable' }, defaultConfiguration: { level: 'warning' } },
      ] } },
      automationDetails: { id: 'conf-lint/run-1' },
      results,
    }],
  };
}

const RESULT_EVAL = { ruleId: 'js/eval', ruleIndex: 0, level: 'error', message: { text: 'Call to eval with user input.' },
  locations: [{ physicalLocation: { artifactLocation: { uri: 'src/app.js' }, region: { startLine: 1, startColumn: 1, endColumn: 12 } } }],
  partialFingerprints: { primaryLocationLineHash: 'conf-eval-1:1' } };
const RESULT_UNUSED = { ruleId: 'js/unused-var', ruleIndex: 1, message: { text: 'Unused variable x.' },
  locations: [{ physicalLocation: { artifactLocation: { uri: 'src/app.js' }, region: { startLine: 2, startColumn: 5, endColumn: 6 } } }] };

const gzB64 = (doc) => zlib.gzipSync(Buffer.from(JSON.stringify(doc))).toString('base64');

export const scenarios = [
  {
    name: 'advsec: global advisories',
    requires: ['login'],
    run: async (t, s) => {
      if (fs.existsSync(FIXTURE)) {
        const r = await t.http('POST', forgeUrl(t, '/api/forge/advisories'), { body: JSON.parse(fs.readFileSync(FIXTURE, 'utf8')) });
        s.advisoriesLoaded = r.status === 200;
      }
      await t.call('security-advisories/list-global-advisories', { minItems: s.advisoriesLoaded ? 1 : 0 });
      await t.call('security-advisories/list-global-advisories', { query: { ecosystem: 'npm', affects: 'lodash@4.17.15', per_page: 5 }, minItems: s.advisoriesLoaded ? 1 : 0 });
      const list = await t.probe('security-advisories/list-global-advisories', { query: { per_page: 1 } });
      const ghsa = list.body?.[0]?.ghsa_id || 'GHSA-p6mc-m468-83gw';
      await t.call('security-advisories/get-global-advisory', { params: { ghsa_id: ghsa }, expect: list.body?.length ? [200] : [200, 404] });
    },
  },
  {
    name: 'advsec: code security configuration on an org repository',
    requires: ['org', 'runId'],
    run: async (t, s) => {
      const org = { org: s.org };
      const r = await t.call('repos/create-in-org', { params: org, body: { name: `conf-${s.runId}-advsec`, private: true, auto_init: true }, expect: [201], fixture: true });
      (s.cleanup ||= []).push({ owner: s.org, repo: r.body.name });
      const rp = { owner: s.org, repo: r.body.name };
      await t.waitFor('git/get-ref', { params: { ...rp, ref: `heads/${r.body.default_branch || 'main'}` } }, (x) => x.status === 200);
      const cfg = await t.call('code-security/create-configuration', { params: org, expect: [201], fixture: true, body: {
        name: `conf-${s.runId}-config`, description: 'conformance configuration', advanced_security: 'enabled',
        dependency_graph: 'enabled', dependabot_alerts: 'enabled', dependabot_security_updates: 'not_set', dependabot_delegated_alert_dismissal: 'enabled',
        code_scanning_default_setup: 'disabled', code_scanning_delegated_alert_dismissal: 'enabled',
        secret_scanning: 'enabled', secret_scanning_push_protection: 'enabled', secret_scanning_delegated_bypass: 'enabled',
        secret_scanning_delegated_bypass_options: { reviewers: [{ reviewer_id: 138, reviewer_type: 'ROLE' }] },
        secret_scanning_validity_checks: 'disabled', secret_scanning_non_provider_patterns: 'disabled', secret_scanning_delegated_alert_dismissal: 'enabled',
        private_vulnerability_reporting: 'disabled', enforcement: 'unenforced',
      } });
      const cp = { ...org, configuration_id: cfg.body.id };
      s.advsec = { rp, id: r.body.id, configId: cfg.body.id, def: r.body.default_branch || 'main' };
      await t.call('code-security/get-configurations-for-org', { params: org, minItems: 1 });
      await t.call('code-security/get-configurations-for-org', { params: org, query: { target_type: 'global' } });
      await t.call('code-security/get-configuration', { params: cp });
      await t.call('code-security/update-configuration', { params: cp, body: { description: 'conformance configuration (updated)' }, expect: [200] });
      await t.call('code-security/attach-configuration', { params: cp, body: { scope: 'selected', selected_repository_ids: [r.body.id] }, expect: [202] });
      await t.call('code-security/get-repositories-for-configuration', { params: cp, minItems: 1 });
      await t.call('code-security/get-configuration-for-repository', { params: rp, expect: [200] });
      await t.call('code-security/set-configuration-as-default', { params: cp, body: { default_for_new_repos: 'private_and_internal' } });
      await t.call('code-security/get-default-configurations', { params: org, minItems: 1 });
      await t.call('code-security/set-configuration-as-default', { params: cp, body: { default_for_new_repos: 'none' } });
    },
  },
  {
    name: 'advsec: secret scanning, push protection and bypasses',
    requires: ['advsec'],
    run: async (t, s) => {
      const { rp, def } = s.advsec;
      const key = fakeAwsKey();
      const put = (p, content, token) => t.call('repos/create-or-update-file-contents', { params: { ...rp, path: p }, token,
        body: { message: `add ${p}`, content: b64(content), branch: def }, expect: [201, 409] });
      // Push protection blocks the secret and names a placeholder id.
      const blocked = await put('config/aws.txt', `aws_access_key_id = ${key}\n`);
      const placeholder = blocked?.body?.errors?.[0]?.placeholder_id;
      if (blocked?.status === 409 && placeholder) {
        await t.call('secret-scanning/create-push-protection-bypass', { params: rp, body: { reason: 'used_in_tests', placeholder_id: placeholder } });
        await put('config/aws.txt', `aws_access_key_id = ${key}\n`);
      }
      await t.waitFor('secret-scanning/list-alerts-for-repo', { params: rp }, (x) => Array.isArray(x.body) && x.body.length > 0, { tries: 10 });
      const alerts = await t.call('secret-scanning/list-alerts-for-repo', { params: rp, minItems: 1 });
      const n = alerts?.body?.[0]?.number;
      if (!n) return;
      const ap = { ...rp, alert_number: n };
      await t.call('secret-scanning/get-alert', { params: ap });
      await t.call('secret-scanning/list-locations-for-alert', { params: ap, minItems: 1 });
      await t.call('secret-scanning/get-scan-history', { params: rp });
      await t.call('secret-scanning/list-alerts-for-org', { params: { org: s.org }, minItems: 1 });
      await t.call('secret-scanning/list-alerts-for-enterprise', { params: ENT, minItems: 1 });
      // The org owner is a reviewer, so it resolves directly.
      await t.call('secret-scanning/update-alert', { params: ap, body: { state: 'resolved', resolution: 'used_in_tests', resolution_comment: 'conformance' } });
      await t.call('secret-scanning/update-alert', { params: ap, body: { state: 'open' } });

      if (s.collaborator && s.collaboratorToken) {
        const as = s.collaboratorToken;
        await t.call('repos/add-collaborator', { params: { ...rp, username: s.collaborator }, body: { permission: 'admin' }, expect: [201, 204], fixture: true });
        // A non-reviewer's resolution becomes a dismissal request.
        await t.call('secret-scanning/update-alert', { params: ap, token: as, body: { state: 'resolved', resolution: 'false_positive', resolution_comment: 'please close' } });
        await t.call('secret-scanning/get-dismissal-request', { params: ap });
        await t.call('secret-scanning/list-repo-dismissal-requests', { params: rp, minItems: 1 });
        await t.call('secret-scanning/list-org-dismissal-requests', { params: { org: s.org }, minItems: 1 });
        await t.call('secret-scanning/list-enterprise-dismissal-requests', { params: ENT, minItems: 1 });
        await t.call('secret-scanning/review-dismissal-request', { params: ap, body: { status: 'approve', message: 'ok' } });
        // A non-reviewer's bypass becomes a bypass request.
        const key2 = fakeAwsKey();
        const b2 = await put('config/aws2.txt', `aws_access_key_id = ${key2}\n`, as);
        const ph2 = b2?.body?.errors?.[0]?.placeholder_id;
        if (ph2) {
          await t.call('secret-scanning/create-push-protection-bypass', { params: rp, token: as, body: { reason: 'false_positive', placeholder_id: ph2 }, expect: [403] });
          const reqs = await t.call('secret-scanning/list-repo-bypass-requests', { params: rp, minItems: 1 });
          const num = reqs?.body?.[0]?.number;
          if (num) {
            const bp = { ...rp, bypass_request_number: num };
            await t.call('secret-scanning/get-bypass-request', { params: bp });
            const rev = await t.call('secret-scanning/review-bypass-request', { params: bp, body: { status: 'approve', message: 'test key' } });
            if (rev?.body?.bypass_review_id) {
              await t.call('secret-scanning/dismiss-bypass-response', { params: { ...rp, bypass_response_id: rev.body.bypass_review_id }, expect: [204] });
            }
          }
          await t.call('secret-scanning/list-org-bypass-requests', { params: { org: s.org }, minItems: 1 });
          await t.call('secret-scanning/list-enterprise-bypass-requests', { params: ENT, minItems: 1 });
        }
        await t.call('repos/remove-collaborator', { params: { ...rp, username: s.collaborator } });
      } else {
        await t.call('secret-scanning/list-repo-dismissal-requests', { params: rp });
        await t.call('secret-scanning/list-org-dismissal-requests', { params: { org: s.org } });
        await t.call('secret-scanning/list-enterprise-dismissal-requests', { params: ENT });
        await t.call('secret-scanning/list-repo-bypass-requests', { params: rp });
        await t.call('secret-scanning/list-org-bypass-requests', { params: { org: s.org } });
        await t.call('secret-scanning/list-enterprise-bypass-requests', { params: ENT });
      }
    },
  },
  {
    name: 'advsec: secret scanning pattern configurations',
    requires: ['org'],
    run: async (t, s) => {
      for (const [list, update, params] of [
        ['secret-scanning/list-org-pattern-configs', 'secret-scanning/update-org-pattern-configs', { org: s.org }],
        ['secret-scanning/list-enterprise-pattern-configs', 'secret-scanning/update-enterprise-pattern-configs', ENT],
      ]) {
        const cur = await t.call(list, { params });
        const v1 = await t.call(update, { params, body: { pattern_config_version: cur?.body?.pattern_config_version ?? null,
          provider_pattern_settings: [{ token_type: 'slack_api_token', push_protection_setting: 'disabled' }] } });
        await t.call(list, { params });
        await t.call(update, { params, body: { pattern_config_version: v1?.body?.pattern_config_version ?? null,
          provider_pattern_settings: [{ token_type: 'slack_api_token', push_protection_setting: 'not-set' }] } });
      }
    },
  },
  {
    name: 'advsec: Dependabot alerts and dismissal requests',
    requires: ['advsec'],
    run: async (t, s) => {
      const { rp, def } = s.advsec;
      const pkg = { name: 'conf-app', version: '1.0.0', dependencies: { lodash: '4.17.15', minimist: '1.2.5' }, devDependencies: { semver: '7.3.0' } };
      await t.call('repos/create-or-update-file-contents', { params: { ...rp, path: 'package.json' }, body: { message: 'add package.json', content: b64(JSON.stringify(pkg, null, 2)), branch: def }, expect: [201] });
      await t.waitFor('dependabot/list-alerts-for-repo', { params: rp }, (x) => Array.isArray(x.body) && x.body.length >= 2, { tries: 10 });
      const want = s.advisoriesLoaded ? 1 : 0;
      const alerts = await t.call('dependabot/list-alerts-for-repo', { params: rp, minItems: want });
      await t.call('dependabot/list-alerts-for-repo', { params: rp, query: { state: 'open,auto_dismissed', severity: 'high,critical', ecosystem: 'npm', scope: 'runtime', sort: 'updated' } });
      await t.call('dependabot/list-alerts-for-org', { params: { org: s.org }, minItems: want });
      await t.call('dependabot/list-alerts-for-enterprise', { params: ENT, minItems: want });
      const open = (alerts?.body || []).filter((a) => a.state === 'open');
      if (open.length) {
        const ap = { ...rp, alert_number: open[0].number };
        await t.call('dependabot/get-alert', { params: ap });
        await t.call('dependabot/update-alert', { params: ap, body: { state: 'dismissed', dismissed_reason: 'tolerable_risk', dismissed_comment: 'conformance' } });
        await t.call('dependabot/update-alert', { params: ap, body: { state: 'open' } });
        const dp = ap;
        await t.call('dependabot/create-dismissal-request-for-repo', { params: dp, body: { dismissed_reason: 'not_used', dismissed_comment: 'not reachable' }, expect: [201] });
        await t.call('dependabot/get-dismissal-request-for-repo', { params: dp });
        await t.call('dependabot/list-dismissal-requests-for-repo', { params: rp, minItems: 1 });
        await t.call('dependabot/list-dismissal-requests-for-org', { params: { org: s.org }, minItems: 1 });
        await t.call('dependabot/review-dismissal-request-for-repo', { params: dp, body: { status: 'deny', message: 'still used' } });
        await t.call('dependabot/create-dismissal-request-for-repo', { params: dp, body: { dismissed_reason: 'inaccurate' }, expect: [201] });
        await t.call('dependabot/cancel-dismissal-request-for-repo', { params: dp, expect: [204] });
      } else {
        await t.call('dependabot/list-dismissal-requests-for-repo', { params: rp });
        await t.call('dependabot/list-dismissal-requests-for-org', { params: { org: s.org } });
      }
    },
  },
  {
    name: 'advsec: Dependabot secrets and repository access',
    requires: ['advsec'],
    run: async (t, s) => {
      const { rp, id } = s.advsec;
      const org = { org: s.org };
      const pk = await t.call('dependabot/get-repo-public-key', { params: rp, fixture: true });
      const sealed = sealedBox(pk.body.key, 'dependabot-secret');
      if (sealed) {
        const sp = { ...rp, secret_name: 'CONF_DEPENDABOT' };
        await t.call('dependabot/create-or-update-repo-secret', { params: sp, body: { encrypted_value: sealed, key_id: pk.body.key_id }, expect: [201] });
        await t.call('dependabot/create-or-update-repo-secret', { params: sp, body: { encrypted_value: sealedBox(pk.body.key, 'v2'), key_id: pk.body.key_id }, expect: [204] });
        await t.call('dependabot/get-repo-secret', { params: sp });
        await t.call('dependabot/list-repo-secrets', { params: rp, minItems: 1 });
        await t.call('dependabot/delete-repo-secret', { params: sp, expect: [204] });
      }
      const opk = await t.call('dependabot/get-org-public-key', { params: org, fixture: true });
      const osealed = sealedBox(opk.body.key, 'org-dependabot-secret');
      if (osealed) {
        const sp = { ...org, secret_name: 'CONF_ORG_DEPENDABOT' };
        await t.call('dependabot/create-or-update-org-secret', { params: sp, body: { encrypted_value: osealed, key_id: opk.body.key_id, visibility: 'selected', selected_repository_ids: [id] }, expect: [201] });
        await t.call('dependabot/get-org-secret', { params: sp });
        await t.call('dependabot/list-org-secrets', { params: org, minItems: 1 });
        await t.call('dependabot/list-selected-repos-for-org-secret', { params: sp, minItems: 1 });
        await t.call('dependabot/remove-selected-repo-from-org-secret', { params: { ...sp, repository_id: id }, expect: [204] });
        await t.call('dependabot/add-selected-repo-to-org-secret', { params: { ...sp, repository_id: id }, expect: [204] });
        await t.call('dependabot/set-selected-repos-for-org-secret', { params: sp, body: { selected_repository_ids: [id] }, expect: [204] });
        await t.call('dependabot/delete-org-secret', { params: sp, expect: [204] });
      }
      await t.call('dependabot/repository-access-for-org', { params: org });
      await t.call('dependabot/update-repository-access-for-org', { params: org, body: { repository_ids_to_add: [id] }, expect: [204] });
      await t.call('dependabot/repository-access-for-org', { params: org });
      await t.call('dependabot/update-repository-access-for-org', { params: org, body: { repository_ids_to_remove: [id] }, expect: [204] });
      await t.call('dependabot/set-repository-access-default-level', { params: org, body: { default_level: 'public' }, expect: [204] });
    },
  },
  {
    name: 'advsec: code scanning (SARIF upload, analyses, alerts)',
    requires: ['advsec'],
    run: async (t, s) => {
      const { rp, def } = s.advsec;
      await t.call('repos/create-or-update-file-contents', { params: { ...rp, path: 'src/app.js' }, body: { message: 'add app', content: b64('eval(input);\nlet x = 1;\n'), branch: def }, expect: [201] });
      const ref = await t.probe('git/get-ref', { params: { ...rp, ref: `heads/${def}` } });
      const sha = ref.body?.object?.sha;
      if (!sha) return;
      const upload = async (results) => {
        const up = await t.call('code-scanning/upload-sarif', { params: rp, expect: [202], body: { commit_sha: sha, ref: `refs/heads/${def}`, sarif: gzB64(sarifDoc(results)), tool_name: 'forge-conf-lint' } });
        const id = up?.body?.id;
        if (id) await t.waitFor('code-scanning/get-sarif', { params: { ...rp, sarif_id: id } }, (x) => x.body?.processing_status && x.body.processing_status !== 'pending', { tries: 10 });
        return id;
      };
      const sarif1 = await upload([RESULT_EVAL, RESULT_UNUSED]);
      if (!sarif1) return;
      await t.call('code-scanning/get-sarif', { params: { ...rp, sarif_id: sarif1 } });
      const an = await t.call('code-scanning/list-recent-analyses', { params: rp, minItems: 1 });
      const analysisId = an?.body?.[0]?.id;
      if (analysisId) {
        await t.call('code-scanning/get-analysis', { params: { ...rp, analysis_id: analysisId } });
        await t.call('code-scanning/get-analysis', { params: { ...rp, analysis_id: analysisId }, headers: { accept: 'application/sarif+json' }, note: 'SARIF media type' });
      }
      const alerts = await t.call('code-scanning/list-alerts-for-repo', { params: rp, minItems: 1 });
      await t.call('code-scanning/list-alerts-for-repo', { params: rp, query: { tool_name: 'forge-conf-lint', ref: def, state: 'open', severity: 'high' } });
      await t.call('code-scanning/list-alerts-for-org', { params: { org: s.org }, minItems: 1 });
      await t.call('code-scanning/list-alerts-for-enterprise', { params: ENT, minItems: 1 });
      const evalAlert = (alerts?.body || []).find((a) => a.rule?.id === 'js/eval') || alerts?.body?.[0];
      const unused = (alerts?.body || []).find((a) => a.rule?.id === 'js/unused-var');
      if (evalAlert) {
        const ap = { ...rp, alert_number: evalAlert.number };
        await t.call('code-scanning/get-alert', { params: ap });
        await t.call('code-scanning/list-alert-instances', { params: ap, minItems: 1 });
        await t.call('code-scanning/update-alert', { params: ap, body: { state: 'dismissed', dismissed_reason: 'false positive', dismissed_comment: 'conformance' } });
        await t.call('code-scanning/update-alert', { params: ap, body: { state: 'open' } });
        // Delegated dismissal: an explicit request, reviewed by the org owner.
        await t.call('code-scanning/update-alert', { params: ap, body: { state: 'dismissed', dismissed_reason: "won't fix", create_request: true } });
        await t.call('code-scanning/get-dismissal-request-for-repo', { params: ap });
        await t.call('code-scanning/list-dismissal-requests-for-repo', { params: rp, minItems: 1 });
        await t.call('code-scanning/list-org-dismissal-requests', { params: { org: s.org }, minItems: 1 });
        await t.call('code-scanning/review-dismissal-request-for-repo', { params: ap, body: { status: 'approve', message: 'accepted risk' }, expect: [204] });
      }
      // A second analysis without the unused-variable result fixes that alert.
      await upload([RESULT_EVAL]);
      if (unused) await t.call('code-scanning/get-alert', { params: { ...rp, alert_number: unused.number }, note: 'fixed by the second analysis' });
      const an2 = await t.probe('code-scanning/list-recent-analyses', { params: rp });
      const [latest, older] = an2.body || [];
      if (latest) await t.call('code-scanning/delete-analysis', { params: { ...rp, analysis_id: latest.id } });
      if (older) await t.call('code-scanning/delete-analysis', { params: { ...rp, analysis_id: older.id }, query: { confirm_delete: 'true' } });
      await t.call('code-scanning/get-default-setup', { params: rp });
      await t.call('code-scanning/update-default-setup', { params: rp, body: { state: 'configured', query_suite: 'extended', languages: ['javascript-typescript'] } });
      await t.call('code-scanning/get-default-setup', { params: rp });
      await t.call('code-scanning/update-default-setup', { params: rp, body: { state: 'not-configured' } });
    },
  },
  {
    name: 'advsec: detach and delete the org configuration',
    requires: ['advsec'],
    run: async (t, s) => {
      const org = { org: s.org };
      await t.call('code-security/detach-configuration', { params: org, body: { selected_repository_ids: [s.advsec.id] }, expect: [204] });
      await t.call('code-security/delete-configuration', { params: { ...org, configuration_id: s.advsec.configId }, expect: [204] });
    },
  },
  {
    name: 'advsec: enterprise code security configurations',
    requires: ['me', 'runId'],
    run: async (t, s) => {
      // Every setting `not_set`, so attaching it to the enterprise's repositories changes nothing.
      const notSet = Object.fromEntries(['dependency_graph', 'dependabot_alerts', 'dependabot_security_updates', 'code_scanning_default_setup',
        'secret_scanning', 'secret_scanning_push_protection', 'secret_scanning_validity_checks', 'secret_scanning_non_provider_patterns',
        'private_vulnerability_reporting'].map((k) => [k, 'not_set']));
      const c = await t.call('code-security/create-configuration-for-enterprise', { params: ENT, expect: [201], fixture: true,
        body: { name: `conf-${s.runId}-ent`, description: 'conformance enterprise configuration', advanced_security: 'disabled', enforcement: 'unenforced', ...notSet } });
      const cp = { ...ENT, configuration_id: c.body.id };
      try {
        await t.call('code-security/get-configurations-for-enterprise', { params: ENT, minItems: 1 });
        await t.call('code-security/get-single-configuration-for-enterprise', { params: cp });
        await t.call('code-security/update-enterprise-configuration', { params: cp, body: { description: 'updated' } });
        await t.call('code-security/set-configuration-as-default-for-enterprise', { params: cp, body: { default_for_new_repos: 'public' } });
        await t.call('code-security/get-default-configurations-for-enterprise', { params: ENT, minItems: 1 });
        await t.call('code-security/set-configuration-as-default-for-enterprise', { params: cp, body: { default_for_new_repos: 'none' } });
        await t.call('code-security/attach-enterprise-configuration', { params: cp, body: { scope: 'all_without_configurations' }, expect: [202] });
        await t.call('code-security/get-repositories-for-enterprise-configuration', { params: cp });
      } finally {
        await t.call('code-security/delete-configuration-for-enterprise', { params: cp, expect: [204] });
      }
    },
  },
];
