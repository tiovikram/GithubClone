// Area "actions-settings" scenarios (docs/BUILDOUT.md): Actions secrets (sealed with the
// owner's public key, libsodium crypto_box_seal), variables, selected repositories of org
// secrets/variables, permission policies at repository / organization / enterprise level, runner
// groups and the repository OIDC sub-claim template. They run after the core scenarios
// (`s.rp`, `s.repoId`, `s.org`, `s.runId`) and restore or delete everything they change.
//
// Sealing needs the dev dependencies `tweetnacl` and `blakejs` (`npm install`); without them the
// secret writes are skipped. Enterprise ops use $CONFORMANCE_ENTERPRISE (default `forge`; forge
// has one enterprise, any slug names it) and need a site-admin token.

let nacl = null;
let blake2b = null;
try {
  nacl = (await import('tweetnacl')).default;
  blake2b = (await import('blakejs')).blake2b;
} catch { /* optional */ }

/** libsodium crypto_box_seal: epk || box(m, nonce = blake2b-24(epk || pk), pk, esk). */
export function sealedBox(publicKeyB64, plaintext) {
  if (!nacl) return null;
  const pk = Buffer.from(publicKeyB64, 'base64');
  const eph = nacl.box.keyPair();
  const nonce = blake2b(Buffer.concat([Buffer.from(eph.publicKey), pk]), undefined, 24);
  const boxed = nacl.box(Buffer.from(plaintext, 'utf8'), nonce, pk, eph.secretKey);
  return Buffer.concat([Buffer.from(eph.publicKey), Buffer.from(boxed)]).toString('base64');
}

const ENT = { enterprise: process.env.CONFORMANCE_ENTERPRISE || 'forge' };

/** GET a setting, PUT a new value, PUT the original back (all validated). */
async function roundTrip(t, getOp, putOp, params, change, restore = (b) => b) {
  const cur = await t.call(getOp, { params });
  await t.call(putOp, { params, body: change(cur?.body || {}), expect: [204] });
  await t.call(getOp, { params });
  if (cur?.body) await t.call(putOp, { params, body: restore(cur.body), expect: [204] });
  return cur?.body;
}

/** The four policy sections every level has. */
async function commonSections(t, params, suffix, ops) {
  await roundTrip(t, ops.selGet, ops.selPut, params,
    () => ({ github_owned_allowed: true, verified_allowed: false, patterns_allowed: ['octo-org/*', 'monalisa/setup@v1'] }));
  await roundTrip(t, ops.wfGet, ops.wfPut, params,
    () => ({ default_workflow_permissions: 'read', can_approve_pull_request_reviews: false }));
  await roundTrip(t, ops.retGet, ops.retPut, params, () => ({ days: 30 }), (b) => ({ days: b.days }));
  await roundTrip(t, ops.apGet, ops.apPut, params, () => ({ approval_policy: 'all_external_contributors' }));
  await roundTrip(t, ops.fpGet, ops.fpPut, params,
    () => ({ run_workflows_from_fork_pull_requests: false, send_write_tokens_to_workflows: false, send_secrets_and_variables: false, require_approval_for_fork_pr_workflows: true }));
  void suffix;
}

export const scenarios = [
  {
    name: 'actions-settings: repository secrets and variables',
    requires: ['rp'],
    run: async (t, s) => {
      const pk = await t.call('actions/get-repo-public-key', { params: s.rp, fixture: true });
      const sealed = sealedBox(pk.body.key, `conf-secret-${s.runId}`);
      if (sealed) {
        const name = 'CONF_SECRET';
        await t.call('actions/create-or-update-repo-secret', { params: { ...s.rp, secret_name: name }, body: { encrypted_value: sealed, key_id: pk.body.key_id }, expect: [201] });
        await t.call('actions/create-or-update-repo-secret', { params: { ...s.rp, secret_name: name }, body: { encrypted_value: sealedBox(pk.body.key, 'v2'), key_id: pk.body.key_id }, expect: [204] });
        await t.call('actions/get-repo-secret', { params: { ...s.rp, secret_name: name } });
        await t.call('actions/list-repo-secrets', { params: s.rp, minItems: 1 });
        await t.call('actions/delete-repo-secret', { params: { ...s.rp, secret_name: name }, expect: [204] });

        const env = { ...s.rp, environment_name: 'conf-env' };
        const epk = await t.call('actions/get-environment-public-key', { params: env, fixture: true });
        await t.call('actions/create-or-update-environment-secret', { params: { ...env, secret_name: 'ENV_SECRET' }, body: { encrypted_value: sealedBox(epk.body.key, 'env'), key_id: epk.body.key_id }, expect: [201] });
        await t.call('actions/get-environment-secret', { params: { ...env, secret_name: 'ENV_SECRET' } });
        await t.call('actions/list-environment-secrets', { params: env, minItems: 1 });
        await t.call('actions/delete-environment-secret', { params: { ...env, secret_name: 'ENV_SECRET' }, expect: [204] });
      }
      const v = { ...s.rp, name: 'CONF_VAR' };
      await t.call('actions/create-repo-variable', { params: s.rp, body: { name: 'CONF_VAR', value: 'one' }, expect: [201] });
      await t.call('actions/update-repo-variable', { params: v, body: { value: 'two' }, expect: [204] });
      await t.call('actions/get-repo-variable', { params: v });
      await t.call('actions/list-repo-variables', { params: s.rp, minItems: 1 });
      await t.call('actions/delete-repo-variable', { params: v, expect: [204] });

      const env = { ...s.rp, environment_name: 'conf-env' };
      await t.call('actions/create-environment-variable', { params: env, body: { name: 'ENV_VAR', value: 'e' }, expect: [201] });
      await t.call('actions/update-environment-variable', { params: { ...env, name: 'ENV_VAR' }, body: { value: 'f' }, expect: [204] });
      await t.call('actions/get-environment-variable', { params: { ...env, name: 'ENV_VAR' } });
      await t.call('actions/list-environment-variables', { params: env, minItems: 1 });
      await t.call('actions/delete-environment-variable', { params: { ...env, name: 'ENV_VAR' }, expect: [204] });
    },
  },
  {
    name: 'actions-settings: org secrets and variables with selected repositories',
    requires: ['org', 'runId'],
    run: async (t, s) => {
      const org = { org: s.org };
      const r = await t.call('repos/create-in-org', { params: org, body: { name: `conf-${s.runId}-actset`, private: true, auto_init: true }, expect: [201], fixture: true });
      (s.cleanup ||= []).push({ owner: s.org, repo: r.body.name });
      s.actsetRepo = { owner: s.org, repo: r.body.name, id: r.body.id };
      const rp = { owner: s.org, repo: r.body.name };
      const pk = await t.call('actions/get-org-public-key', { params: org, fixture: true });
      const sealed = sealedBox(pk.body.key, 'org-secret');
      if (sealed) {
        const sp = { ...org, secret_name: 'CONF_ORG_SECRET' };
        await t.call('actions/create-or-update-org-secret', { params: sp, body: { encrypted_value: sealed, key_id: pk.body.key_id, visibility: 'selected', selected_repository_ids: [r.body.id] }, expect: [201] });
        await t.call('actions/get-org-secret', { params: sp });
        await t.call('actions/list-org-secrets', { params: org, minItems: 1 });
        await t.call('actions/list-selected-repos-for-org-secret', { params: sp, minItems: 1 });
        await t.call('actions/list-repo-organization-secrets', { params: rp, minItems: 1 });
        await t.call('actions/remove-selected-repo-from-org-secret', { params: { ...sp, repository_id: r.body.id }, expect: [204] });
        await t.call('actions/add-selected-repo-to-org-secret', { params: { ...sp, repository_id: r.body.id }, expect: [204] });
        await t.call('actions/set-selected-repos-for-org-secret', { params: sp, body: { selected_repository_ids: [r.body.id] }, expect: [204] });
        await t.call('actions/delete-org-secret', { params: sp, expect: [204] });
      }
      const vp = { ...org, name: 'CONF_ORG_VAR' };
      await t.call('actions/create-org-variable', { params: org, body: { name: 'CONF_ORG_VAR', value: 'x', visibility: 'selected', selected_repository_ids: [r.body.id] }, expect: [201] });
      await t.call('actions/update-org-variable', { params: vp, body: { value: 'y' }, expect: [204] });
      await t.call('actions/get-org-variable', { params: vp });
      await t.call('actions/list-org-variables', { params: org, minItems: 1 });
      await t.call('actions/list-selected-repos-for-org-variable', { params: vp, minItems: 1 });
      await t.call('actions/list-repo-organization-variables', { params: rp, minItems: 1 });
      await t.call('actions/remove-selected-repo-from-org-variable', { params: { ...vp, repository_id: r.body.id }, expect: [204] });
      await t.call('actions/add-selected-repo-to-org-variable', { params: { ...vp, repository_id: r.body.id }, expect: [204] });
      await t.call('actions/set-selected-repos-for-org-variable', { params: vp, body: { selected_repository_ids: [r.body.id] }, expect: [204] });
      await t.call('actions/delete-org-variable', { params: vp, expect: [204] });
    },
  },
  {
    name: 'actions-settings: repository permissions and OIDC',
    requires: ['rp'],
    run: async (t, s) => {
      await roundTrip(t, 'actions/get-github-actions-permissions-repository', 'actions/set-github-actions-permissions-repository', s.rp,
        () => ({ enabled: true, allowed_actions: 'selected', sha_pinning_required: false }),
        (b) => ({ enabled: b.enabled, allowed_actions: b.allowed_actions || 'all', sha_pinning_required: !!b.sha_pinning_required }));
      await commonSections(t, s.rp, 'repository', {
        selGet: 'actions/get-allowed-actions-repository', selPut: 'actions/set-allowed-actions-repository',
        wfGet: 'actions/get-github-actions-default-workflow-permissions-repository', wfPut: 'actions/set-github-actions-default-workflow-permissions-repository',
        retGet: 'actions/get-artifact-and-log-retention-settings-repository', retPut: 'actions/set-artifact-and-log-retention-settings-repository',
        apGet: 'actions/get-fork-pr-contributor-approval-permissions-repository', apPut: 'actions/set-fork-pr-contributor-approval-permissions-repository',
        fpGet: 'actions/get-private-repo-fork-pr-workflows-settings-repository', fpPut: 'actions/set-private-repo-fork-pr-workflows-settings-repository',
      });
      await roundTrip(t, 'actions/get-workflow-access-to-repository', 'actions/set-workflow-access-to-repository', s.rp,
        () => ({ access_level: 'none' }));
      await t.call('actions/get-custom-oidc-sub-claim-for-repo', { params: s.rp });
      await t.call('actions/set-custom-oidc-sub-claim-for-repo', { params: s.rp, body: { use_default: false, include_claim_keys: ['repo', 'context'] }, expect: [201] });
      await t.call('actions/get-custom-oidc-sub-claim-for-repo', { params: s.rp });
      await t.call('actions/set-custom-oidc-sub-claim-for-repo', { params: s.rp, body: { use_default: true }, expect: [201] });
    },
  },
  {
    name: 'actions-settings: organization permissions',
    requires: ['org', 'actsetRepo'],
    run: async (t, s) => {
      const org = { org: s.org };
      const id = s.actsetRepo.id;
      const cur = await t.call('actions/get-github-actions-permissions-organization', { params: org });
      await t.call('actions/set-github-actions-permissions-organization', { params: org, body: { enabled_repositories: 'selected', allowed_actions: 'all' }, expect: [204] });
      await t.call('actions/set-selected-repositories-enabled-github-actions-organization', { params: org, body: { selected_repository_ids: [id] }, expect: [204] });
      await t.call('actions/list-selected-repositories-enabled-github-actions-organization', { params: org, minItems: 1 });
      await t.call('actions/disable-selected-repository-github-actions-organization', { params: { ...org, repository_id: id }, expect: [204] });
      await t.call('actions/enable-selected-repository-github-actions-organization', { params: { ...org, repository_id: id }, expect: [204] });
      await t.call('actions/get-github-actions-permissions-organization', { params: org });
      await t.call('actions/set-github-actions-permissions-organization', { params: org,
        body: { enabled_repositories: cur?.body?.enabled_repositories || 'all', allowed_actions: cur?.body?.allowed_actions || 'all' }, expect: [204] });
      await commonSections(t, org, 'organization', {
        selGet: 'actions/get-allowed-actions-organization', selPut: 'actions/set-allowed-actions-organization',
        wfGet: 'actions/get-github-actions-default-workflow-permissions-organization', wfPut: 'actions/set-github-actions-default-workflow-permissions-organization',
        retGet: 'actions/get-artifact-and-log-retention-settings-organization', retPut: 'actions/set-artifact-and-log-retention-settings-organization',
        apGet: 'actions/get-fork-pr-contributor-approval-permissions-organization', apPut: 'actions/set-fork-pr-contributor-approval-permissions-organization',
        fpGet: 'actions/get-private-repo-fork-pr-workflows-settings-organization', fpPut: 'actions/set-private-repo-fork-pr-workflows-settings-organization',
      });
      const sh = await t.call('actions/get-self-hosted-runners-permissions-organization', { params: org });
      await t.call('actions/set-self-hosted-runners-permissions-organization', { params: org, body: { enabled_repositories: 'selected' }, expect: [204] });
      await t.call('actions/set-selected-repositories-self-hosted-runners-organization', { params: org, body: { selected_repository_ids: [id] }, expect: [204] });
      await t.call('actions/list-selected-repositories-self-hosted-runners-organization', { params: org, minItems: 1 });
      await t.call('actions/disable-selected-repository-self-hosted-runners-organization', { params: { ...org, repository_id: id }, expect: [204] });
      await t.call('actions/enable-selected-repository-self-hosted-runners-organization', { params: { ...org, repository_id: id }, expect: [204] });
      await t.call('actions/set-self-hosted-runners-permissions-organization', { params: org, body: { enabled_repositories: sh?.body?.enabled_repositories || 'all' }, expect: [204] });
    },
  },
  {
    name: 'actions-settings: organization runner groups',
    requires: ['org', 'actsetRepo'],
    run: async (t, s) => {
      const org = { org: s.org };
      const id = s.actsetRepo.id;
      await t.call('actions/list-self-hosted-runner-groups-for-org', { params: org, minItems: 1 }); // Default group
      const g = await t.call('actions/create-self-hosted-runner-group-for-org', { params: org, fixture: true, expect: [201],
        body: { name: `conf-group-${s.runId}`, visibility: 'selected', selected_repository_ids: [id], runners: [], allows_public_repositories: false } });
      const gp = { ...org, runner_group_id: g.body.id };
      try {
        await t.call('actions/get-self-hosted-runner-group-for-org', { params: gp });
        await t.call('actions/update-self-hosted-runner-group-for-org', { params: gp, body: { name: `conf-group-${s.runId}-b`, visibility: 'selected', restricted_to_workflows: true, selected_workflows: [`${s.org}/${s.actsetRepo.repo}/.github/workflows/ci.yml@main`] } });
        await t.call('actions/list-repo-access-to-self-hosted-runner-group-in-org', { params: gp, minItems: 1 });
        await t.call('actions/remove-repo-access-to-self-hosted-runner-group-in-org', { params: { ...gp, repository_id: id }, expect: [204] });
        await t.call('actions/add-repo-access-to-self-hosted-runner-group-in-org', { params: { ...gp, repository_id: id }, expect: [204] });
        await t.call('actions/set-repo-access-to-self-hosted-runner-group-in-org', { params: gp, body: { selected_repository_ids: [id] }, expect: [204] });
        await t.call('actions/list-self-hosted-runners-in-group-for-org', { params: gp });
        await t.call('actions/set-self-hosted-runners-in-group-for-org', { params: gp, body: { runners: [] }, expect: [204] });
      } finally {
        await t.call('actions/delete-self-hosted-runner-group-from-org', { params: gp, expect: [204] });
      }
    },
  },
  {
    name: 'actions-settings: enterprise permissions and runner groups',
    requires: ['org', 'me'],
    run: async (t, s) => {
      const orgBody = await t.probe('orgs/get', { params: { org: s.org } });
      const orgId = orgBody.body?.id;
      const cur = await t.call('enterprise-admin/get-github-actions-permissions-enterprise', { params: ENT, fixture: true });
      await t.call('enterprise-admin/set-github-actions-permissions-enterprise', { params: ENT, body: { enabled_organizations: 'selected', allowed_actions: 'all' }, expect: [204] });
      await t.call('enterprise-admin/set-selected-organizations-enabled-github-actions-enterprise', { params: ENT, body: { selected_organization_ids: [orgId] }, expect: [204] });
      await t.call('enterprise-admin/list-selected-organizations-enabled-github-actions-enterprise', { params: ENT, minItems: 1 });
      await t.call('enterprise-admin/disable-selected-organization-github-actions-enterprise', { params: { ...ENT, org_id: orgId }, expect: [204] });
      await t.call('enterprise-admin/enable-selected-organization-github-actions-enterprise', { params: { ...ENT, org_id: orgId }, expect: [204] });
      await t.call('enterprise-admin/set-github-actions-permissions-enterprise', { params: ENT,
        body: { enabled_organizations: cur.body.enabled_organizations || 'all', allowed_actions: cur.body.allowed_actions || 'all' }, expect: [204] });
      await commonSections(t, ENT, 'enterprise', {
        selGet: 'enterprise-admin/get-allowed-actions-enterprise', selPut: 'enterprise-admin/set-allowed-actions-enterprise',
        wfGet: 'actions/get-github-actions-default-workflow-permissions-enterprise', wfPut: 'actions/set-github-actions-default-workflow-permissions-enterprise',
        retGet: 'enterprise-admin/get-artifact-and-log-retention-settings', retPut: 'enterprise-admin/set-artifact-and-log-retention-settings',
        apGet: 'enterprise-admin/get-fork-pr-contributor-approval-permissions', apPut: 'enterprise-admin/set-fork-pr-contributor-approval-permissions',
        fpGet: 'enterprise-admin/get-private-repo-fork-pr-workflows-settings', fpPut: 'enterprise-admin/set-private-repo-fork-pr-workflows-settings',
      });
      await roundTrip(t, 'enterprise-admin/get-self-hosted-runners-permissions', 'enterprise-admin/set-self-hosted-runners-permissions', ENT,
        () => ({ disable_self_hosted_runners_for_all_orgs: false }));

      await t.call('enterprise-admin/list-self-hosted-runner-groups-for-enterprise', { params: ENT, minItems: 1 });
      const g = await t.call('enterprise-admin/create-self-hosted-runner-group-for-enterprise', { params: ENT, fixture: true, expect: [201],
        body: { name: `conf-egroup-${s.runId}`, visibility: 'selected', selected_organization_ids: [orgId], runners: [] } });
      const gp = { ...ENT, runner_group_id: g.body.id };
      try {
        await t.call('enterprise-admin/get-self-hosted-runner-group-for-enterprise', { params: gp });
        await t.call('enterprise-admin/update-self-hosted-runner-group-for-enterprise', { params: gp, body: { name: `conf-egroup-${s.runId}-b`, allows_public_repositories: true } });
        await t.call('enterprise-admin/list-org-access-to-self-hosted-runner-group-in-enterprise', { params: gp, minItems: 1 });
        await t.call('enterprise-admin/remove-org-access-to-self-hosted-runner-group-in-enterprise', { params: { ...gp, org_id: orgId }, expect: [204] });
        await t.call('enterprise-admin/add-org-access-to-self-hosted-runner-group-in-enterprise', { params: { ...gp, org_id: orgId }, expect: [204] });
        await t.call('enterprise-admin/set-org-access-to-self-hosted-runner-group-in-enterprise', { params: gp, body: { selected_organization_ids: [orgId] }, expect: [204] });
        await t.call('enterprise-admin/list-self-hosted-runners-in-group-for-enterprise', { params: gp });
        await t.call('enterprise-admin/set-self-hosted-runners-in-group-for-enterprise', { params: gp, body: { runners: [] }, expect: [204] });
        // The org sees the enterprise group as inherited.
        await t.call('actions/list-self-hosted-runner-groups-for-org', { params: { org: s.org }, minItems: 2 });
      } finally {
        await t.call('enterprise-admin/delete-self-hosted-runner-group-from-enterprise', { params: gp, expect: [204] });
      }
    },
  },
];
