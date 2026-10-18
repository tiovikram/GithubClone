// Area "enterprise" scenarios (docs/BUILDOUT.md): GHES enterprise administration — users
// (create, rename, impersonation, promote/demote, suspend), global webhooks, public keys, LDAP
// mapping, statistics, license, announcement, pre-receive environments and hooks, the
// enterprise Actions policy and runner groups, audit log and streams, code security,
// custom properties, billing, enterprise teams, SCIM, live migrations and the Manage API.
//
// The harness token must belong to a site administrator (forge: `forge admin create-user
// --admin`). Everything created here is deleted again; a second (non-admin) user is created
// through POST /admin/users and acts through an impersonation token.
import { generateKeyPairSync } from 'node:crypto';

const b64 = (str) => Buffer.from(str, 'utf8').toString('base64');
const ENT = 'forge'; // FORGE_ENTERPRISE_SLUG of the server under test
const E = (extra = {}) => ({ enterprise: ENT, ...extra });

function sshPublicKey(comment = 'enterprise-conformance') {
  const { publicKey } = generateKeyPairSync('ed25519');
  const raw = publicKey.export({ format: 'der', type: 'spki' }).subarray(-32);
  const str = (b) => { const l = Buffer.alloc(4); l.writeUInt32BE(b.length); return Buffer.concat([l, b]); };
  return `ssh-ed25519 ${Buffer.concat([str(Buffer.from('ssh-ed25519')), str(raw)]).toString('base64')} ${comment}`;
}

/** Unrecorded check that `token` gets `status` (e.g. 403 for a non-admin); throws otherwise. */
async function expectStatus(t, opId, opts, status, what) {
  const r = await t.probe(opId, opts);
  if (r.status !== status) throw new Error(`${what}: expected ${status}, got ${r.status} ${r.text?.slice(0, 200)}`);
  return r;
}

export const scenarios = [
  {
    name: 'enterprise: users, impersonation, suspension',
    requires: ['login'],
    run: async (t, s) => {
      const login = `ent-u-${s.runId}`;
      const u = await t.call('enterprise-admin/create-user', { body: { login, email: `${login}@example.com` }, expect: [201], fixture: true });
      s.entUser = u.body.login;
      const renamed = `${login}-r`;
      await t.call('enterprise-admin/update-username-for-user', { params: { username: login }, body: { login: renamed }, expect: [202] });
      s.entUser = renamed;
      const imp = await t.call('enterprise-admin/create-impersonation-o-auth-token', {
        params: { username: renamed }, body: { scopes: ['repo', 'read:org'] }, expect: [201], fixture: true,
      });
      s.entUserToken = imp.body.token;
      // The impersonation token acts as the user; a non-admin gets 403 on admin endpoints.
      const me = await expectStatus(t, 'users/get-authenticated', { token: s.entUserToken }, 200, 'impersonation token');
      if (me.body?.login !== renamed) throw new Error(`impersonation token authenticated as ${me.body?.login}`);
      await expectStatus(t, 'enterprise-admin/get-all-stats', { token: s.entUserToken }, 403, 'non-admin stats');
      await t.call('enterprise-admin/promote-user-to-be-site-administrator', { params: { username: renamed } });
      await expectStatus(t, 'enterprise-admin/get-user-stats', { token: s.entUserToken }, 200, 'promoted user stats');
      await t.call('enterprise-admin/demote-site-administrator', { params: { username: renamed } });
      await t.call('enterprise-admin/suspend-user', { params: { username: renamed }, body: { reason: 'conformance' } });
      // Suspended users cannot authenticate.
      await expectStatus(t, 'users/get-authenticated', { token: s.entUserToken }, 401, 'suspended user');
      await t.call('enterprise-admin/get-user-stats');
      await t.call('enterprise-admin/unsuspend-user', { params: { username: renamed }, body: { reason: 'conformance done' } });
      await expectStatus(t, 'users/get-authenticated', { token: s.entUserToken }, 200, 'unsuspended user');
      await t.call('enterprise-admin/update-ldap-mapping-for-user', { params: { username: renamed }, body: { ldap_dn: `uid=${renamed},ou=users,dc=example,dc=com` } });
      await t.call('enterprise-admin/sync-ldap-mapping-for-user', { params: { username: renamed }, expect: [201] });
      const toks = await t.call('enterprise-admin/list-personal-access-tokens', { minItems: 1 });
      // A second impersonation token, deleted through /admin/tokens/{id}.
      const imp2 = await t.call('enterprise-admin/create-impersonation-o-auth-token', { params: { username: renamed }, body: { scopes: ['repo'] }, expect: [201] });
      if (imp2?.body?.id) await t.call('enterprise-admin/delete-personal-access-token', { params: { token_id: imp2.body.id } });
      void toks;
    },
  },
  {
    name: 'enterprise: global webhooks',
    requires: ['login'],
    run: async (t) => {
      const h = await t.call('enterprise-admin/create-global-webhook', {
        body: { name: 'web', events: ['user', 'organization'], active: true, config: { url: 'https://example.com/global-hook', content_type: 'json', secret: 's3cret', insecure_ssl: '0' } },
        expect: [201], fixture: true,
      });
      const id = h.body.id;
      await t.call('enterprise-admin/list-global-webhooks', { minItems: 1 });
      await t.call('enterprise-admin/get-global-webhook', { params: { hook_id: id } });
      await t.call('enterprise-admin/update-global-webhook', { params: { hook_id: id }, body: { events: ['user'], config: { url: 'https://example.com/global-hook-2', content_type: 'form' } } });
      await t.call('enterprise-admin/ping-global-webhook', { params: { hook_id: id } });
      await t.call('enterprise-admin/delete-global-webhook', { params: { hook_id: id } });
    },
  },
  {
    name: 'enterprise: stats, license, announcement, keys',
    requires: ['login'],
    run: async (t, s) => {
      for (const op of ['all', 'comment', 'gist', 'hooks', 'issue', 'milestone', 'org', 'pages', 'pull-request', 'repo', 'user']) {
        await t.call(op === 'all' ? 'enterprise-admin/get-all-stats' : op === 'hooks' ? 'enterprise-admin/get-hooks-stats' : op === 'pages' ? 'enterprise-admin/get-pages-stats' : `enterprise-admin/get-${op}-stats`);
      }
      await t.call('enterprise-admin/get-security-products');
      await t.call('enterprise-admin/get-license-information');
      await t.call('enterprise-admin/set-announcement', { body: { announcement: `Conformance banner ${s.runId}`, expires_at: '2099-01-01T00:00:00Z', user_dismissible: true } });
      const a = await t.call('enterprise-admin/get-announcement');
      if (a?.body?.announcement !== `Conformance banner ${s.runId}`) throw new Error('announcement not stored');
      await t.call('enterprise-admin/remove-announcement');
      const k = await t.call('users/create-public-ssh-key-for-authenticated-user', { body: { title: `ent-${s.runId}`, key: sshPublicKey() }, expect: [201], fixture: true });
      await t.call('enterprise-admin/list-public-keys', { query: { sort: 'created', direction: 'desc' }, minItems: 1 });
      await t.call('enterprise-admin/delete-public-key', { params: { key_ids: String(k.body.id) } });
    },
  },
  {
    // Own organization (create-org, rename, teams, LDAP team mapping); deleted at the end of
    // the enterprise scenarios.
    name: 'enterprise: organizations, LDAP team mapping',
    requires: ['login'],
    run: async (t, s) => {
      const o = await t.call('enterprise-admin/create-org', { body: { login: `ent-org-${s.runId}`, admin: s.login, profile_name: 'Enterprise conformance' }, expect: [201], fixture: true });
      const renamed = `ent-org2-${s.runId}`;
      await t.call('enterprise-admin/update-org-name', { params: { org: o.body.login }, body: { login: renamed }, expect: [202] });
      s.entOrg = renamed;
      s.entOrgId = o.body.id;
      const team = await t.call('teams/create', { params: { org: renamed }, body: { name: `ent-team-${s.runId}`, privacy: 'closed' }, expect: [201], fixture: true });
      s.entOrgTeam = team.body.slug;
      await t.call('enterprise-admin/update-ldap-mapping-for-team', { params: { team_id: team.body.id }, body: { ldap_dn: 'cn=engineering,ou=groups,dc=example,dc=com' } });
      await t.call('enterprise-admin/sync-ldap-mapping-for-team', { params: { team_id: team.body.id }, expect: [201] });
    },
  },
  {
    name: 'enterprise: pre-receive environments and hooks',
    requires: ['login', 'rp', 'entOrg'],
    run: async (t, s) => {
      // The download of an unresolvable URL fails fast (state "failed"); the API is what is measured.
      const env = await t.call('enterprise-admin/create-pre-receive-environment', {
        body: { name: `ent-env-${s.runId}`, image_url: 'https://pre-receive.invalid/env.tar.gz' }, expect: [201], fixture: true,
      });
      const envId = env.body.id;
      await t.call('enterprise-admin/list-pre-receive-environments', { minItems: 2 });
      await t.call('enterprise-admin/get-pre-receive-environment', { params: { pre_receive_environment_id: envId } });
      await t.waitFor('enterprise-admin/get-download-status-for-pre-receive-environment', { params: { pre_receive_environment_id: envId } }, (r) => r.body?.state && r.body.state !== 'in_progress');
      await t.call('enterprise-admin/update-pre-receive-environment', { params: { pre_receive_environment_id: envId }, body: { name: `ent-env2-${s.runId}` } });
      await t.call('enterprise-admin/start-pre-receive-environment-download', { params: { pre_receive_environment_id: envId }, expect: [202] });
      await t.waitFor('enterprise-admin/get-download-status-for-pre-receive-environment', { params: { pre_receive_environment_id: envId } }, (r) => r.body?.state && r.body.state !== 'in_progress');
      await t.call('enterprise-admin/get-download-status-for-pre-receive-environment', { params: { pre_receive_environment_id: envId } });

      await t.call('repos/create-or-update-file-contents', {
        params: { ...s.rp, path: 'hooks/conformance-check.sh' }, expect: [201], fixture: true,
        body: { message: 'Add pre-receive hook script', content: b64('#!/bin/sh\n# reject nothing\nexit 0\n') },
      });
      const full = `${s.rp.owner}/${s.rp.repo}`;
      const h = await t.call('enterprise-admin/create-pre-receive-hook', {
        body: { name: `ent-hook-${s.runId}`, script: 'hooks/conformance-check.sh', script_repository: { full_name: full }, environment: { id: 1 }, enforcement: 'disabled', allow_downstream_configuration: true },
        expect: [201], fixture: true,
      });
      const hid = h.body.id;
      await t.call('enterprise-admin/list-pre-receive-hooks', { minItems: 1 });
      await t.call('enterprise-admin/get-pre-receive-hook', { params: { pre_receive_hook_id: hid } });
      await t.call('enterprise-admin/update-pre-receive-hook', { params: { pre_receive_hook_id: hid }, body: { enforcement: 'testing' } });
      const op = { org: s.entOrg, pre_receive_hook_id: hid };
      await t.call('enterprise-admin/list-pre-receive-hooks-for-org', { params: { org: s.entOrg }, minItems: 1 });
      await t.call('enterprise-admin/get-pre-receive-hook-for-org', { params: op });
      await t.call('enterprise-admin/update-pre-receive-hook-enforcement-for-org', { params: op, body: { enforcement: 'disabled', allow_downstream_configuration: true } });
      await t.call('enterprise-admin/remove-pre-receive-hook-enforcement-for-org', { params: op });
      const rp = { ...s.rp, pre_receive_hook_id: hid };
      await t.call('enterprise-admin/list-pre-receive-hooks-for-repo', { params: s.rp, minItems: 1 });
      await t.call('enterprise-admin/get-pre-receive-hook-for-repo', { params: rp });
      await t.call('enterprise-admin/update-pre-receive-hook-enforcement-for-repo', { params: rp, body: { enforcement: 'disabled' } });
      await t.call('enterprise-admin/remove-pre-receive-hook-enforcement-for-repo', { params: rp });
      await t.call('enterprise-admin/delete-pre-receive-hook', { params: { pre_receive_hook_id: hid } });
      await t.call('enterprise-admin/delete-pre-receive-environment', { params: { pre_receive_environment_id: envId } });
    },
  },
  {
    name: 'enterprise: Actions policy, runner groups, runners',
    requires: ['login', 'entOrgId'],
    run: async (t, s) => {
      await t.call('enterprise-admin/get-github-actions-permissions-enterprise', { params: E() });
      await t.call('enterprise-admin/set-github-actions-permissions-enterprise', { params: E(), body: { enabled_organizations: 'selected', allowed_actions: 'selected' } });
      await t.call('enterprise-admin/set-selected-organizations-enabled-github-actions-enterprise', { params: E(), body: { selected_organization_ids: [s.entOrgId] } });
      await t.call('enterprise-admin/list-selected-organizations-enabled-github-actions-enterprise', { params: E(), minItems: 1, itemsKey: 'organizations' });
      await t.call('enterprise-admin/disable-selected-organization-github-actions-enterprise', { params: E({ org_id: s.entOrgId }) });
      await t.call('enterprise-admin/enable-selected-organization-github-actions-enterprise', { params: E({ org_id: s.entOrgId }) });
      await t.call('enterprise-admin/get-github-actions-permissions-enterprise', { params: E() });
      await t.call('enterprise-admin/set-allowed-actions-enterprise', { params: E(), body: { github_owned_allowed: true, verified_allowed: false, patterns_allowed: ['octo-org/*'] } });
      await t.call('enterprise-admin/get-allowed-actions-enterprise', { params: E() });
      await t.call('enterprise-admin/set-github-actions-permissions-enterprise', { params: E(), body: { enabled_organizations: 'all', allowed_actions: 'all' } });
      await t.call('enterprise-admin/set-artifact-and-log-retention-settings', { params: E(), body: { days: 60 } });
      await t.call('enterprise-admin/get-artifact-and-log-retention-settings', { params: E() });
      await t.call('enterprise-admin/set-artifact-and-log-retention-settings', { params: E(), body: { days: 90 } });
      await t.call('enterprise-admin/set-fork-pr-contributor-approval-permissions', { params: E(), body: { approval_policy: 'all_external_contributors' } });
      await t.call('enterprise-admin/get-fork-pr-contributor-approval-permissions', { params: E() });
      await t.call('enterprise-admin/set-fork-pr-contributor-approval-permissions', { params: E(), body: { approval_policy: 'first_time_contributors' } });
      await t.call('enterprise-admin/set-private-repo-fork-pr-workflows-settings', { params: E(), body: { run_workflows_from_fork_pull_requests: true, require_approval_for_fork_pr_workflows: true } });
      await t.call('enterprise-admin/get-private-repo-fork-pr-workflows-settings', { params: E() });
      await t.call('enterprise-admin/set-private-repo-fork-pr-workflows-settings', { params: E(), body: { run_workflows_from_fork_pull_requests: false } });
      await t.call('enterprise-admin/set-self-hosted-runners-permissions', { params: E(), body: { disable_self_hosted_runners_for_all_orgs: false } });
      await t.call('enterprise-admin/get-self-hosted-runners-permissions', { params: E() });

      const g = await t.call('enterprise-admin/create-self-hosted-runner-group-for-enterprise', {
        params: E(), body: { name: `ent-rg-${s.runId}`, visibility: 'selected', selected_organization_ids: [s.entOrgId], runners: [] }, expect: [201], fixture: true,
      });
      const gp = E({ runner_group_id: g.body.id });
      await t.call('enterprise-admin/list-self-hosted-runner-groups-for-enterprise', { params: E(), minItems: 2, itemsKey: 'runner_groups' });
      await t.call('enterprise-admin/get-self-hosted-runner-group-for-enterprise', { params: gp });
      await t.call('enterprise-admin/update-self-hosted-runner-group-for-enterprise', { params: gp, body: { allows_public_repositories: true } });
      await t.call('enterprise-admin/list-org-access-to-self-hosted-runner-group-in-enterprise', { params: gp, minItems: 1, itemsKey: 'organizations' });
      await t.call('enterprise-admin/remove-org-access-to-self-hosted-runner-group-in-enterprise', { params: { ...gp, org_id: s.entOrgId } });
      await t.call('enterprise-admin/add-org-access-to-self-hosted-runner-group-in-enterprise', { params: { ...gp, org_id: s.entOrgId } });
      await t.call('enterprise-admin/set-org-access-to-self-hosted-runner-group-in-enterprise', { params: gp, body: { selected_organization_ids: [] } });
      await t.call('enterprise-admin/set-self-hosted-runners-in-group-for-enterprise', { params: gp, body: { runners: [] } });
      await t.call('enterprise-admin/list-self-hosted-runners-in-group-for-enterprise', { params: gp, itemsKey: 'runners' });

      const runners = await t.call('enterprise-admin/list-self-hosted-runners-for-enterprise', { params: E() });
      await t.call('enterprise-admin/list-runner-applications-for-enterprise', { params: E(), minItems: 1 });
      await t.call('enterprise-admin/create-registration-token-for-enterprise', { params: E(), expect: [201] });
      await t.call('enterprise-admin/create-remove-token-for-enterprise', { params: E(), expect: [201] });
      // Runner-specific operations need a registered enterprise runner (a runner process).
      const r = runners?.body?.runners?.[0];
      if (r) {
        const rp = E({ runner_id: r.id });
        await t.call('enterprise-admin/get-self-hosted-runner-for-enterprise', { params: rp });
        await t.call('enterprise-admin/list-labels-for-self-hosted-runner-for-enterprise', { params: rp });
        await t.call('enterprise-admin/add-custom-labels-to-self-hosted-runner-for-enterprise', { params: rp, body: { labels: ['conf-a'] } });
        await t.call('enterprise-admin/set-custom-labels-for-self-hosted-runner-for-enterprise', { params: rp, body: { labels: ['conf-a', 'conf-b'] } });
        await t.call('enterprise-admin/remove-custom-label-from-self-hosted-runner-for-enterprise', { params: { ...rp, name: 'conf-b' } });
        await t.call('enterprise-admin/remove-all-custom-labels-from-self-hosted-runner-for-enterprise', { params: rp });
        await t.call('enterprise-admin/add-self-hosted-runner-to-group-for-enterprise', { params: { ...gp, runner_id: r.id } });
        await t.call('enterprise-admin/remove-self-hosted-runner-from-group-for-enterprise', { params: { ...gp, runner_id: r.id } });
      }
      await t.call('enterprise-admin/delete-self-hosted-runner-group-from-enterprise', { params: gp });
    },
  },
  {
    name: 'enterprise: audit log, streams, code security, billing, rulesets, bypass requests',
    requires: ['login', 'entOrg'],
    run: async (t, s) => {
      await t.call('enterprise-admin/get-audit-log', { params: E(), minItems: 1 });
      await t.call('enterprise-admin/get-audit-log', { params: E(), query: { phrase: 'action:user', order: 'asc', per_page: 5 }, minItems: 1 });
      const key = await t.call('enterprise-admin/get-audit-log-stream-key', { params: E(), fixture: true });
      const st = await t.call('enterprise-admin/create-audit-log-stream', {
        params: E(),
        body: { enabled: true, stream_type: 'Amazon S3', vendor_specific: { bucket: `audit-${s.runId}`, region: 'us-east-1', key_id: key.body.key_id, authentication_type: 'oidc', arn_role: 'arn:aws:iam::123456789012:role/audit' } },
      });
      await t.call('enterprise-admin/get-audit-log-streams', { params: E(), minItems: 1 });
      if (st?.body?.id) {
        const sp = E({ stream_id: st.body.id });
        await t.call('enterprise-admin/get-one-audit-log-stream', { params: sp });
        await t.call('enterprise-admin/update-audit-log-stream', {
          params: sp,
          body: { enabled: false, stream_type: 'Amazon S3', vendor_specific: { bucket: `audit-${s.runId}`, region: 'eu-west-1', key_id: key.body.key_id, authentication_type: 'oidc', arn_role: 'arn:aws:iam::123456789012:role/audit' } },
        });
        await t.call('enterprise-admin/delete-audit-log-stream', { params: sp });
      }
      await t.call('enterprise-admin/list-push-bypass-requests', { params: E() });
      await t.call('secret-scanning/get-security-analysis-settings-for-enterprise', { params: E() });
      await t.call('secret-scanning/patch-security-analysis-settings-for-enterprise', { params: E(), body: { secret_scanning_enabled_for_new_repositories: true, secret_scanning_push_protection_custom_link: 'https://example.com/push-protection' } });
      await t.call('secret-scanning/patch-security-analysis-settings-for-enterprise', { params: E(), body: { secret_scanning_enabled_for_new_repositories: false, secret_scanning_push_protection_custom_link: null } });
      await t.call('secret-scanning/post-security-product-enablement-for-enterprise', { params: E({ security_product: 'dependabot_alerts', enablement: 'enable_all' }) });
      await t.call('secret-scanning/post-security-product-enablement-for-enterprise', { params: E({ security_product: 'dependabot_alerts', enablement: 'disable_all' }) });
      await t.call('billing/get-github-advanced-security-billing-ghe', { params: E() });
      await t.call('billing/get-github-advanced-security-billing-org', { params: { org: s.entOrg } });
      // forge has no enterprise-level rulesets (no REST API creates them): unknown ids are 404.
      await t.call('enterprise-admin/get-enterprise-ruleset-history', { params: E({ ruleset_id: 987654321 }), expect: [404] });
      await t.call('enterprise-admin/get-enterprise-ruleset-version', { params: E({ ruleset_id: 987654321, version_id: 1 }), expect: [404] });
    },
  },
  {
    name: 'enterprise: custom properties',
    requires: ['login', 'entOrg'],
    run: async (t, s) => {
      const op = `ent_org_prop_${s.runId}`;
      await t.call('enterprise-admin/custom-properties-for-orgs-create-or-update-enterprise-definitions', {
        params: E(), body: { properties: [{ property_name: op, value_type: 'single_select', allowed_values: ['gold', 'silver'], description: 'tier', values_editable_by: 'enterprise_actors' }] },
      });
      await t.call('enterprise-admin/custom-properties-for-orgs-get-enterprise-definitions', { params: E(), minItems: 1 });
      await t.call('enterprise-admin/custom-properties-for-orgs-get-enterprise-definition', { params: E({ custom_property_name: op }) });
      await t.call('enterprise-admin/custom-properties-for-orgs-create-or-update-enterprise-definition', { params: E({ custom_property_name: op }), body: { value_type: 'single_select', allowed_values: ['gold', 'silver', 'bronze'] } });
      await t.call('enterprise-admin/custom-properties-for-orgs-create-or-update-enterprise-values', { params: E(), body: { organization_logins: [s.entOrg], properties: [{ property_name: op, value: 'gold' }] } });
      await t.call('enterprise-admin/custom-properties-for-orgs-get-enterprise-values', { params: E(), minItems: 1 });
      await t.call('enterprise-admin/custom-properties-for-orgs-delete-enterprise-definition', { params: E({ custom_property_name: op }) });

      const rp = `ent_repo_prop_${s.runId}`;
      await t.call('enterprise-admin/custom-properties-for-repos-create-or-update-enterprise-definitions', {
        params: E(), body: { properties: [{ property_name: rp, value_type: 'string', required: false, description: 'owner team' }] },
      });
      await t.call('enterprise-admin/custom-properties-for-repos-get-enterprise-definitions', { params: E(), minItems: 1 });
      await t.call('enterprise-admin/custom-properties-for-repos-get-enterprise-definition', { params: E({ custom_property_name: rp }) });
      await t.call('enterprise-admin/custom-properties-for-repos-create-or-update-enterprise-definition', { params: E({ custom_property_name: rp }), body: { value_type: 'true_false', default_value: 'false', required: true } });
      await t.call('enterprise-admin/custom-properties-for-repos-delete-enterprise-definition', { params: E({ custom_property_name: rp }) });
      // promote an organization's repository property to the enterprise
      const pp = `ent_promo_${s.runId}`;
      await t.call('orgs/custom-properties-for-repos-create-or-update-organization-definition', { params: { org: s.entOrg, custom_property_name: pp }, body: { value_type: 'string' }, fixture: true });
      await t.call('enterprise-admin/custom-properties-for-repos-promote-definition-to-enterprise', { params: E({ org: s.entOrg, custom_property_name: pp }) });
      await t.call('enterprise-admin/custom-properties-for-repos-delete-enterprise-definition', { params: E({ custom_property_name: pp }) });
    },
  },
  {
    name: 'enterprise: SCIM provisioning, enterprise teams, external groups',
    requires: ['login', 'entOrg', 'entOrgTeam', 'entUser'],
    run: async (t, s) => {
      const scimUser = await t.call('enterprise-admin/provision-enterprise-user', {
        params: E(), expect: [201], fixture: true,
        body: {
          schemas: ['urn:ietf:params:scim:schemas:core:2.0:User'], externalId: `ext-${s.runId}`, active: true, userName: `ent-scim-${s.runId}`,
          name: { givenName: 'Scim', familyName: 'User' }, displayName: 'Scim User',
          emails: [{ value: `ent-scim-${s.runId}@example.com`, type: 'work', primary: true }], roles: [{ value: 'user', primary: true }],
        },
      });
      const sid = scimUser.body.id;
      const su = E({ scim_user_id: sid });
      await t.call('enterprise-admin/list-provisioned-identities-enterprise', { params: E(), query: { filter: `userName eq "ent-scim-${s.runId}"` }, minItems: 1, itemsKey: 'Resources' });
      await t.call('enterprise-admin/get-provisioning-information-for-enterprise-user', { params: su });
      await t.call('enterprise-admin/set-information-for-provisioned-enterprise-user', {
        params: su,
        body: {
          schemas: ['urn:ietf:params:scim:schemas:core:2.0:User'], externalId: `ext-${s.runId}`, active: true, userName: `ent-scim-${s.runId}`,
          name: { givenName: 'Scim', familyName: 'Person' }, displayName: 'Scim Person',
          emails: [{ value: `ent-scim-${s.runId}@example.com`, type: 'work', primary: true }],
        },
      });
      await t.call('enterprise-admin/update-attribute-for-enterprise-user', {
        params: su, body: { schemas: ['urn:ietf:params:scim:api:messages:2.0:PatchOp'], Operations: [{ op: 'replace', path: 'displayName', value: 'Scim Patched' }] },
      });

      const grp = await t.call('enterprise-admin/provision-enterprise-group', {
        params: E(), expect: [201], fixture: true,
        body: { schemas: ['urn:ietf:params:scim:schemas:core:2.0:Group'], externalId: `grp-${s.runId}`, displayName: `ent-group-${s.runId}`, members: [{ value: sid, displayName: 'Scim Patched' }] },
      });
      const gid = grp.body.id;
      const gp = E({ scim_group_id: gid });
      await t.call('enterprise-admin/list-provisioned-groups-enterprise', { params: E(), minItems: 1, itemsKey: 'Resources' });
      await t.call('enterprise-admin/get-provisioning-information-for-enterprise-group', { params: gp });
      await t.call('enterprise-admin/set-information-for-provisioned-enterprise-group', {
        params: gp, body: { schemas: ['urn:ietf:params:scim:schemas:core:2.0:Group'], externalId: `grp-${s.runId}`, displayName: `ent-group-${s.runId}`, members: [{ value: sid, displayName: 'Scim Patched' }] },
      });
      await t.call('enterprise-admin/update-attribute-for-enterprise-group', {
        params: gp, body: { schemas: ['urn:ietf:params:scim:api:messages:2.0:PatchOp'], Operations: [{ op: 'replace', path: 'displayName', value: `ent-group2-${s.runId}` }] },
      });

      // The SCIM group is an identity-provider group organization teams can link to.
      const groups = await t.call('teams/list-external-idp-groups-for-org', { params: { org: s.entOrg }, minItems: 1, itemsKey: 'groups' });
      const eg = groups?.body?.groups?.find((g) => g.group_name === `ent-group2-${s.runId}`);
      if (eg) {
        await t.call('teams/external-idp-group-info-for-org', { params: { org: s.entOrg, group_id: eg.group_id } });
        await t.call('teams/link-external-idp-group-to-team-for-org', { params: { org: s.entOrg, team_slug: s.entOrgTeam }, body: { group_id: eg.group_id } });
        await t.call('teams/list-linked-external-idp-groups-to-team-for-org', { params: { org: s.entOrg, team_slug: s.entOrgTeam }, minItems: 1, itemsKey: 'groups' });
        await t.call('teams/unlink-external-idp-group-from-team-for-org', { params: { org: s.entOrg, team_slug: s.entOrgTeam } });
      }

      // Enterprise teams: members of a team assigned to an organization are its members.
      const team = await t.call('enterprise-teams/create', { params: E(), body: { name: `ent-eteam-${s.runId}`, description: 'conformance', organization_selection_type: 'selected' }, expect: [201], fixture: true });
      const tp = E({ 'enterprise-team': team.body.slug });
      await t.call('enterprise-teams/list', { params: E(), minItems: 1 });
      await t.call('enterprise-teams/get', { params: E({ team_slug: team.body.slug }) });
      await t.call('enterprise-teams/update', { params: E({ team_slug: team.body.slug }), body: { description: 'conformance (updated)', notification_setting: 'notifications_disabled' } });
      await t.call('enterprise-team-memberships/add', { params: { ...tp, username: s.entUser }, expect: [201] });
      await t.call('enterprise-team-memberships/list', { params: tp, minItems: 1 });
      await t.call('enterprise-team-organizations/add', { params: { ...tp, org: s.entOrg }, expect: [201] });
      await expectStatus(t, 'orgs/check-membership-for-user', { params: { org: s.entOrg, username: s.entUser } }, 204, 'enterprise team member joins the organization');
      await t.call('enterprise-team-organizations/get-assignment', { params: { ...tp, org: s.entOrg } });
      await t.call('enterprise-team-organizations/get-assignments', { params: tp, minItems: 1 });
      await t.call('enterprise-team-organizations/bulk-remove', { params: tp, body: { organization_slugs: [s.entOrg] } });
      await expectStatus(t, 'orgs/check-membership-for-user', { params: { org: s.entOrg, username: s.entUser } }, 404, 'membership granted by the team is revoked');
      await t.call('enterprise-team-organizations/bulk-add', { params: tp, body: { organization_slugs: [s.entOrg] } });
      await t.call('enterprise-team-organizations/delete', { params: { ...tp, org: s.entOrg } });
      await t.call('enterprise-team-memberships/bulk-remove', { params: tp, body: { usernames: [s.entUser] } });
      await t.call('enterprise-team-memberships/bulk-add', { params: tp, body: { usernames: [s.entUser] } });
      await t.call('enterprise-team-memberships/remove', { params: { ...tp, username: s.entUser } });
      await t.call('enterprise-teams/delete', { params: E({ team_slug: team.body.slug }) });

      await t.call('enterprise-admin/delete-scim-group-from-enterprise', { params: gp });
      await t.call('enterprise-admin/delete-user-from-enterprise', { params: su });
      // the deprovisioned forge user is suspended; remove it
      await t.probe('enterprise-admin/delete-user', { params: { username: `ent-scim-${s.runId}` } });
    },
  },
  {
    name: 'enterprise: live migrations',
    requires: ['login', 'rp'],
    run: async (t, s) => {
      const body = {
        source_organization_login: s.rp.owner, source_repository_name: s.rp.repo, target_organization_login: 'target-org',
        target_repository_name: s.rp.repo, target_api_endpoint: 'https://api.github.example.com', pat_name: 'MIGRATION_PAT', target_visibility: 'private',
      };
      const m1 = await t.call('enterprise-admin/live-migration-create', { body, expect: [201], fixture: true });
      const p1 = { migration_id: m1.body.migration_id };
      await t.call('enterprise-admin/live-migration-list', { minItems: 1, itemsKey: 'migrations' });
      await t.call('enterprise-admin/live-migration-get', { params: p1 });
      await t.call('enterprise-admin/live-migration-pause', { params: p1 });
      await t.call('enterprise-admin/live-migration-resume', { params: p1 });
      // forge never becomes ready for cutover (it transfers no data): 422
      await t.call('enterprise-admin/live-migration-cutover', { params: p1, body: { force: false }, expect: [422] });
      await t.call('enterprise-admin/live-migration-revert-cutover', { params: p1 });
      const m2 = await t.call('enterprise-admin/live-migration-create', { body, expect: [201] });
      if (m2?.body?.migration_id) await t.call('enterprise-admin/live-migration-start', { params: { migration_id: m2.body.migration_id } });
      const m3 = await t.call('enterprise-admin/live-migration-create', { body, expect: [201] });
      if (m3?.body?.migration_id) await t.call('enterprise-admin/live-migration-cancel', { params: { migration_id: m3.body.migration_id } });
    },
  },
  {
    name: 'enterprise: Manage API (nodes, settings, maintenance, license, ssh, upgrades)',
    requires: ['login'],
    run: async (t, s) => {
      await t.call('enterprise-admin/get-checks-system-requirements');
      await t.call('enterprise-admin/get-cluster-status');
      await t.call('enterprise-admin/get-replication-status');
      await t.call('enterprise-admin/get-config-nodes');
      await t.call('enterprise-admin/get-version');
      await t.call('enterprise-admin/get-manage-settings');
      await t.call('enterprise-admin/set-manage-settings', { body: { private_mode: false } });
      const run = await t.call('enterprise-admin/post-manage-run-config-apply', { body: { run_id: `conf${s.runId}` } });
      await t.call('enterprise-admin/get-manage-config-apply-status', { query: { run_id: run?.body?.run_id } });
      await t.call('enterprise-admin/get-manage-config-apply-events');
      await t.call('enterprise-admin/initialize-instance-configuration', { body: { license: 'forge-license', password: `pw-${s.runId}-long` }, expect: [202] });
      await t.call('enterprise-admin/license-info');
      await t.call('enterprise-admin/license-upload', { body: { license: 'forge-license' }, expect: [201] });
      await t.call('enterprise-admin/license-check');
      const key = sshPublicKey('manage-conformance');
      await t.call('enterprise-admin/set-manage-ssh', { body: { key } });
      await t.call('enterprise-admin/get-manage-ssh', { minItems: 1 });
      await t.call('enterprise-admin/delete-manage-ssh', { body: { key } });
      await t.call('enterprise-admin/get-manage-upgrade-status');
      await t.call('enterprise-admin/get-manage-upgrade-download-status');
      await t.call('enterprise-admin/download-manage-upgrade', { body: { version: '9.9.9' }, expect: [200] });
      await t.call('enterprise-admin/apply-manage-upgrade', { body: { version: '9.9.9' } });
      // Maintenance mode really blocks non-admin requests (503) and is lifted again.
      await t.call('enterprise-admin/set-manage-maintenance', { body: { enabled: true, maintenance_mode_message: 'conformance maintenance' } });
      await t.call('enterprise-admin/get-manage-maintenance', { minItems: 1 });
      try {
        if (s.entUserToken) await expectStatus(t, 'users/get-authenticated', { token: s.entUserToken }, 503, 'maintenance mode');
        await expectStatus(t, 'users/get-authenticated', {}, 200, 'site admin during maintenance');
      } finally {
        await t.call('enterprise-admin/set-manage-maintenance', { body: { enabled: false } });
      }
    },
  },
  {
    name: 'enterprise: cleanup',
    requires: ['login'],
    run: async (t, s) => {
      if (s.entOrg) await t.probe('orgs/delete', { params: { org: s.entOrg } });
      if (s.entUser) {
        await t.call('enterprise-admin/delete-impersonation-o-auth-token', { params: { username: s.entUser } });
        await t.call('enterprise-admin/delete-user', { params: { username: s.entUser } });
      }
    },
  },
];
