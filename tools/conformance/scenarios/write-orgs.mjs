// Organizations area (orgs, teams, private-registries). Needs --org (owned by the token user);
// --collaborator enables the member / role-assignment / outside-collaborator steps.
// Every scenario creates its own fixtures (repos `conf-<runId>-o*`, teams `conf-<runId>-*`)
// and removes them again.
const O = (s, extra = {}) => ({ org: s.org, ...extra });

export const scenarios = [
  {
    name: 'orgs: settings, announcement, immutable releases, security products',
    requires: ['login', 'org'],
    run: async (t, s) => {
      await t.call('orgs/get', { params: O(s) });
      await t.call('orgs/update', { params: O(s), body: { description: 'forge conformance org', default_repository_permission: 'read', members_can_create_repositories: true } });
      await t.call('announcement-banners/set-announcement-banner-for-org', { params: O(s), body: { announcement: 'Conformance maintenance', expires_at: null, user_dismissible: true } });
      await t.call('announcement-banners/get-announcement-banner-for-org', { params: O(s) });
      await t.call('announcement-banners/remove-announcement-banner-for-org', { params: O(s) });
      const r = await t.call('repos/create-in-org', { params: O(s), body: { name: `conf-${s.runId}-oset`, private: true, auto_init: true }, expect: [201] });
      const repoId = r?.body?.id;
      if (r?.body?.name) (s.cleanup ||= []).push({ owner: s.org, repo: r.body.name });
      await t.call('orgs/set-immutable-releases-settings', { params: O(s), body: { enforced_repositories: 'selected', selected_repository_ids: repoId ? [repoId] : [] } });
      await t.call('orgs/get-immutable-releases-settings', { params: O(s) });
      if (repoId) {
        await t.call('orgs/set-immutable-releases-settings-repositories', { params: O(s), body: { selected_repository_ids: [repoId] } });
        await t.call('orgs/disable-selected-repository-immutable-releases-organization', { params: O(s, { repository_id: repoId }) });
        await t.call('orgs/enable-selected-repository-immutable-releases-organization', { params: O(s, { repository_id: repoId }) });
        await t.call('orgs/get-immutable-releases-settings-repositories', { params: O(s), minItems: 1, itemsKey: 'repositories' });
      }
      await t.call('orgs/set-immutable-releases-settings', { params: O(s), body: { enforced_repositories: 'none' } });
      await t.call('orgs/enable-or-disable-security-product-on-all-org-repos', { params: O(s, { security_product: 'dependency_graph', enablement: 'enable_all' }) });
      await t.call('orgs/enable-or-disable-security-product-on-all-org-repos', { params: O(s, { security_product: 'dependency_graph', enablement: 'disable_all' }) });
      await t.call('orgs/list-app-installations', { params: O(s) });
      await t.call('orgs/list-push-bypass-requests', { params: O(s), query: { time_period: 'day' } });
    },
  },
  {
    name: 'orgs: public membership',
    requires: ['login', 'org'],
    run: async (t, s) => {
      const p = O(s, { username: s.login });
      await t.call('orgs/set-public-membership-for-authenticated-user', { params: p });
      await t.call('orgs/check-public-membership-for-user', { params: p, expect: [204] });
      await t.call('orgs/list-public-members', { params: O(s), minItems: 1 });
      await t.call('orgs/remove-public-membership-for-authenticated-user', { params: p });
      await t.call('orgs/check-public-membership-for-user', { params: p, expect: [404] });
    },
  },
  {
    name: 'teams: nested teams, members, repositories (slug and legacy routes)',
    requires: ['login', 'org'],
    run: async (t, s) => {
      const parent = await t.call('teams/create', { params: O(s), body: { name: `conf-${s.runId}-parent`, privacy: 'closed', description: 'parent' }, expect: [201] });
      if (!parent?.body?.id) return;
      const child = await t.call('teams/create', { params: O(s), body: { name: `conf-${s.runId}-child`, parent_team_id: parent.body.id }, expect: [201] });
      const ps = O(s, { team_slug: parent.body.slug });
      await t.call('teams/get-by-name', { params: ps });
      await t.call('teams/list-child-in-org', { params: ps, minItems: 1 });
      await t.call('teams/get-legacy', { params: { team_id: parent.body.id } });
      await t.call('teams/update-legacy', { params: { team_id: parent.body.id }, body: { name: parent.body.name, description: 'parent (edited)', privacy: 'closed' } });
      await t.call('teams/list-child-legacy', { params: { team_id: parent.body.id }, minItems: 1 });
      await t.call('teams/list-members-legacy', { params: { team_id: parent.body.id }, minItems: 1 });
      await t.call('teams/get-member-legacy', { params: { team_id: parent.body.id, username: s.login }, expect: [204] });
      await t.call('teams/get-membership-for-user-legacy', { params: { team_id: parent.body.id, username: s.login } });
      if (s.collaborator) {
        await t.call('orgs/set-membership-for-user', { params: O(s, { username: s.collaborator }), body: { role: 'member' } });
        await t.call('teams/add-or-update-membership-for-user-legacy', { params: { team_id: parent.body.id, username: s.collaborator }, body: { role: 'member' } });
        await t.call('teams/remove-membership-for-user-legacy', { params: { team_id: parent.body.id, username: s.collaborator } });
        await t.call('teams/add-member-legacy', { params: { team_id: parent.body.id, username: s.collaborator } });
        await t.call('teams/remove-member-legacy', { params: { team_id: parent.body.id, username: s.collaborator } });
      }
      const r = await t.call('repos/create-in-org', { params: O(s), body: { name: `conf-${s.runId}-oteam`, private: true, auto_init: true }, expect: [201] });
      if (r?.body?.name) {
        (s.cleanup ||= []).push({ owner: s.org, repo: r.body.name });
        const rp = { owner: s.org, repo: r.body.name };
        await t.call('teams/add-or-update-repo-permissions-in-org', { params: { ...ps, ...rp }, body: { permission: 'push' } });
        await t.call('teams/check-permissions-for-repo-in-org', { params: { ...ps, ...rp }, expect: [204] });
        await t.call('teams/check-permissions-for-repo-in-org', { params: { ...ps, ...rp }, headers: { accept: 'application/vnd.github.v3.repository+json' }, expect: [200] });
        await t.call('teams/list-repos-in-org', { params: ps, minItems: 1 });
        if (child?.body?.id) {
          // child teams inherit the parent's repository access
          await t.call('teams/check-permissions-for-repo-legacy', { params: { team_id: child.body.id, ...rp }, expect: [204] });
          await t.call('teams/add-or-update-repo-permissions-legacy', { params: { team_id: child.body.id, ...rp }, body: { permission: 'admin' } });
          await t.call('teams/check-permissions-for-repo-legacy', { params: { team_id: child.body.id, ...rp }, headers: { accept: 'application/vnd.github.v3.repository+json' }, expect: [200] });
          await t.call('teams/list-repos-legacy', { params: { team_id: child.body.id }, minItems: 1 });
          await t.call('teams/remove-repo-legacy', { params: { team_id: child.body.id, ...rp } });
        }
        await t.call('teams/remove-repo-in-org', { params: { ...ps, ...rp } });
        await t.call('teams/check-permissions-for-repo-in-org', { params: { ...ps, ...rp }, expect: [404] });
      }
      const groups = await t.call('teams/list-external-idp-groups-for-org', { params: O(s) });
      await t.call('teams/list-linked-external-idp-groups-to-team-for-org', { params: ps });
      const g = groups?.body?.groups?.[0];
      if (g) { // groups exist only when an identity provider provisioned them (SCIM)
        await t.call('teams/external-idp-group-info-for-org', { params: O(s, { group_id: g.group_id }) });
        await t.call('teams/link-external-idp-group-to-team-for-org', { params: ps, body: { group_id: g.group_id } });
        await t.call('teams/unlink-external-idp-group-from-team-for-org', { params: ps });
      }
      if (child?.body?.id) await t.call('teams/delete-legacy', { params: { team_id: child.body.id } });
      await t.call('teams/delete-in-org', { params: ps });
    },
  },
  {
    name: 'orgs: custom repository roles',
    requires: ['login', 'org'],
    run: async (t, s) => {
      await t.call('orgs/list-repo-fine-grained-permissions', { params: O(s), minItems: 1 });
      const role = await t.call('orgs/create-custom-repo-role', { params: O(s), body: { name: `conf-${s.runId}-role`, description: 'triage plus labels', base_role: 'triage', permissions: ['add_label', 'remove_label'] }, expect: [201] });
      if (!role?.body?.id) return;
      const p = O(s, { role_id: role.body.id });
      await t.call('orgs/get-custom-repo-role', { params: p });
      await t.call('orgs/update-custom-repo-role', { params: p, body: { description: 'edited', permissions: ['add_label'] } });
      await t.call('orgs/list-custom-repo-roles', { params: O(s), minItems: 1, itemsKey: 'custom_roles' });
      const org = await t.probe('orgs/get', { params: O(s) });
      if (org.body?.id) await t.call('orgs/list-custom-roles', { params: { organization_id: org.body.id }, minItems: 1, itemsKey: 'custom_roles' });
      await t.call('orgs/delete-custom-repo-role', { params: p });
    },
  },
  {
    name: 'orgs: organization roles and security managers',
    requires: ['login', 'org'],
    run: async (t, s) => {
      await t.call('orgs/list-organization-fine-grained-permissions', { params: O(s), minItems: 1 });
      await t.call('orgs/list-org-roles', { params: O(s), minItems: 1, itemsKey: 'roles' });
      const role = await t.call('orgs/create-custom-organization-role', { params: O(s), body: { name: `conf-${s.runId}-orgrole`, description: 'audit readers', permissions: ['read_audit_logs'], base_role: 'read' }, expect: [201] });
      const team = await t.call('teams/create', { params: O(s), body: { name: `conf-${s.runId}-sec`, privacy: 'closed' }, expect: [201] });
      const ts = team?.body?.slug;
      if (role?.body?.id) {
        const p = O(s, { role_id: role.body.id });
        await t.call('orgs/get-org-role', { params: p });
        await t.call('orgs/patch-custom-organization-role', { params: p, body: { description: 'audit readers (edited)' } });
        if (ts) {
          await t.call('orgs/assign-team-to-org-role', { params: { ...p, team_slug: ts } });
          await t.call('orgs/list-org-role-teams', { params: p, minItems: 1 });
          await t.call('orgs/list-org-role-users', { params: p, minItems: 1 });
          await t.call('orgs/revoke-org-role-team', { params: { ...p, team_slug: ts } });
          await t.call('orgs/assign-team-to-org-role', { params: { ...p, team_slug: ts } });
          await t.call('orgs/revoke-all-org-roles-team', { params: O(s, { team_slug: ts }) });
        }
        const who = s.collaborator || s.login;
        if (s.collaborator) await t.call('orgs/set-membership-for-user', { params: O(s, { username: s.collaborator }), body: { role: 'member' } });
        await t.call('orgs/assign-user-to-org-role', { params: { ...p, username: who } });
        await t.call('orgs/revoke-org-role-user', { params: { ...p, username: who } });
        await t.call('orgs/assign-user-to-org-role', { params: { ...p, username: who } });
        await t.call('orgs/revoke-all-org-roles-user', { params: O(s, { username: who }) });
        await t.call('orgs/delete-custom-organization-role', { params: p });
      }
      if (ts) {
        await t.call('orgs/add-security-manager-team', { params: O(s, { team_slug: ts }) });
        await t.call('orgs/list-security-manager-teams', { params: O(s), minItems: 1 });
        await t.call('orgs/remove-security-manager-team', { params: O(s, { team_slug: ts }) });
        await t.call('teams/delete-in-org', { params: O(s, { team_slug: ts }) });
      }
    },
  },
  {
    name: 'orgs: outside collaborators',
    requires: ['login', 'org', 'collaborator'],
    run: async (t, s) => {
      const u = O(s, { username: s.collaborator });
      await t.call('orgs/set-membership-for-user', { params: u, body: { role: 'member' } });
      const team = await t.call('teams/create', { params: O(s), body: { name: `conf-${s.runId}-oc`, privacy: 'closed' }, expect: [201] });
      const r = await t.call('repos/create-in-org', { params: O(s), body: { name: `conf-${s.runId}-ooc`, private: true, auto_init: true }, expect: [201] });
      if (r?.body?.name) (s.cleanup ||= []).push({ owner: s.org, repo: r.body.name });
      if (team?.body?.slug && r?.body?.name) {
        const ts = O(s, { team_slug: team.body.slug });
        await t.call('teams/add-or-update-membership-for-user-in-org', { params: { ...ts, username: s.collaborator }, body: { role: 'member' } });
        await t.call('teams/add-or-update-repo-permissions-in-org', { params: { ...ts, owner: s.org, repo: r.body.name }, body: { permission: 'pull' } });
      }
      // the member keeps the team's repository access as a direct collaborator
      await t.call('orgs/convert-member-to-outside-collaborator', { params: u, body: { async: false }, expect: [204] });
      await t.call('orgs/list-outside-collaborators', { params: O(s), query: { filter: 'all' }, minItems: 1 });
      await t.call('orgs/remove-outside-collaborator', { params: u });
      if (team?.body?.slug) await t.call('teams/delete-in-org', { params: O(s, { team_slug: team.body.slug }) });
    },
  },
  {
    name: 'orgs: custom properties',
    requires: ['login', 'org'],
    run: async (t, s) => {
      const env = `conf_env_${s.runId}`;
      const team = `conf_team_${s.runId}`;
      await t.call('orgs/custom-properties-for-repos-create-or-update-organization-definitions', { params: O(s),
        body: { properties: [{ property_name: env, value_type: 'single_select', allowed_values: ['prod', 'dev'], required: true, default_value: 'dev', description: 'environment' }] } });
      await t.call('orgs/custom-properties-for-repos-create-or-update-organization-definition', { params: O(s, { custom_property_name: team }), body: { value_type: 'string', description: 'owning team' } });
      await t.call('orgs/custom-properties-for-repos-get-organization-definition', { params: O(s, { custom_property_name: env }) });
      await t.call('orgs/custom-properties-for-repos-get-organization-definitions', { params: O(s), minItems: 2 });
      const r = await t.call('repos/create-in-org', { params: O(s), body: { name: `conf-${s.runId}-oprop`, private: false, auto_init: true }, expect: [201] });
      if (r?.body?.name) {
        (s.cleanup ||= []).push({ owner: s.org, repo: r.body.name });
        await t.call('orgs/custom-properties-for-repos-create-or-update-organization-values', { params: O(s),
          body: { repository_names: [r.body.name], properties: [{ property_name: env, value: 'prod' }, { property_name: team, value: 'platform' }] } });
        await t.call('orgs/custom-properties-for-repos-get-organization-values', { params: O(s), query: { repository_query: r.body.name }, minItems: 1 });
      }
      await t.call('orgs/custom-properties-for-repos-delete-organization-definition', { params: O(s, { custom_property_name: team }) });
      await t.call('orgs/custom-properties-for-repos-delete-organization-definition', { params: O(s, { custom_property_name: env }) });
      await t.call('orgs/custom-properties-for-orgs-create-or-update-organization-values', { params: O(s), body: { properties: [{ property_name: 'conf_tier', value: 'gold' }] } });
      await t.call('orgs/custom-properties-for-orgs-get-organization-values', { params: O(s), minItems: 1 });
      await t.call('orgs/custom-properties-for-orgs-create-or-update-organization-values', { params: O(s), body: { properties: [{ property_name: 'conf_tier', value: null }] } });
    },
  },
  {
    name: 'private registries',
    requires: ['login', 'org'],
    run: async (t, s) => {
      await t.call('private-registries/get-org-public-key', { params: O(s) });
      // OIDC auth needs no secret (secret-based configurations are covered by tests/orgs_extra.rs,
      // which seals the token with libsodium's crypto_box_seal against this public key)
      const c = await t.call('private-registries/create-org-private-registry', { params: O(s),
        body: { registry_type: 'maven_repository', url: 'https://maven.example.com/repo', visibility: 'private', auth_type: 'oidc_azure', tenant_id: 'tenant', client_id: 'client' }, expect: [201] });
      await t.call('private-registries/list-org-private-registries', { params: O(s), minItems: 1, itemsKey: 'configurations' });
      const name = c?.body?.name;
      if (name) {
        await t.call('private-registries/get-org-private-registry', { params: O(s, { secret_name: name }) });
        await t.call('private-registries/update-org-private-registry', { params: O(s, { secret_name: name }), body: { url: 'https://maven2.example.com/repo', visibility: 'all' } });
        await t.call('private-registries/delete-org-private-registry', { params: O(s, { secret_name: name }) });
      }
    },
  },
  {
    name: 'orgs: ruleset history, fine-grained PAT policy, audit log',
    requires: ['login', 'org'],
    run: async (t, s) => {
      const rs = await t.call('repos/create-org-ruleset', { params: O(s), body: { name: `conf-${s.runId}-rs`, enforcement: 'disabled', target: 'branch',
        conditions: { ref_name: { include: ['~DEFAULT_BRANCH'], exclude: [] }, repository_name: { include: ['~ALL'], exclude: [] } }, rules: [{ type: 'deletion' }] }, expect: [201] });
      if (rs?.body?.id) {
        const p = O(s, { ruleset_id: rs.body.id });
        await t.call('repos/update-org-ruleset', { params: p, body: { enforcement: 'evaluate' } });
        const h = await t.call('orgs/get-org-ruleset-history', { params: p, minItems: 2 });
        const v = h?.body?.[h.body.length - 1]?.version_id;
        if (v) await t.call('orgs/get-org-ruleset-version', { params: { ...p, version_id: v } });
        await t.call('repos/delete-org-ruleset', { params: p });
      }
      // forge issues classic tokens only: no fine-grained requests or grants exist
      await t.call('orgs/list-pat-grant-requests', { params: O(s) });
      await t.call('orgs/review-pat-grant-requests-in-bulk', { params: O(s), body: { action: 'deny', reason: 'conformance' }, expect: [202] });
      await t.call('orgs/review-pat-grant-request', { params: O(s, { pat_request_id: 999999999 }), body: { action: 'approve' }, expect: [404] });
      await t.call('orgs/list-pat-grant-request-repositories', { params: O(s, { pat_request_id: 999999999 }), expect: [404] });
      await t.call('orgs/list-pat-grants', { params: O(s) });
      await t.call('orgs/update-pat-accesses', { params: O(s), body: { action: 'revoke', pat_ids: [999999999] }, expect: [404] });
      await t.call('orgs/update-pat-access', { params: O(s, { pat_id: 999999999 }), body: { action: 'revoke' }, expect: [404] });
      await t.call('orgs/list-pat-grant-repositories', { params: O(s, { pat_id: 999999999 }), expect: [404] });
      await t.call('orgs/get-audit-log', { params: O(s), query: { phrase: 'action:team', per_page: 10 }, minItems: 1 });
    },
  },
  {
    name: 'orgs: webhooks, memberships, team membership',
    requires: ['login', 'org'],
    run: async (t, s) => {
      const h = await t.call('orgs/create-webhook', { params: O(s), body: { name: 'web', active: true, events: ['organization', 'team', 'membership'],
        config: { url: 'https://example.com/forge-conformance-hook', content_type: 'json', secret: 'conf-secret' } }, expect: [201] });
      await t.call('orgs/list-webhooks', { params: O(s), minItems: h?.body?.id ? 1 : 0 });
      if (h?.body?.id) {
        const hp = O(s, { hook_id: h.body.id });
        await t.call('orgs/get-webhook', { params: hp });
        await t.call('orgs/update-webhook', { params: hp, body: { events: ['organization', 'team', 'membership', 'repository'] } });
        await t.call('orgs/get-webhook-config-for-org', { params: hp });
        await t.call('orgs/update-webhook-config-for-org', { params: hp, body: { content_type: 'json' } });
        await t.call('orgs/ping-webhook', { params: hp });
      }
      await t.call('orgs/list-memberships-for-authenticated-user', { minItems: 1 });
      await t.call('orgs/update-membership-for-authenticated-user', { params: O(s), body: { state: 'active' } });
      await t.call('teams/list-for-authenticated-user');
      const team = await t.call('teams/create', { params: O(s), body: { name: `conf-${s.runId}-mem`, privacy: 'secret' }, expect: [201] });
      const ts = team?.body?.slug && O(s, { team_slug: team.body.slug });
      if (ts) {
        await t.call('teams/update-in-org', { params: ts, body: { name: team.body.name, description: 'membership team', privacy: 'closed' } });
        await t.call('teams/list-members-in-org', { params: ts, query: { role: 'maintainer' }, minItems: 1 });
        await t.call('teams/get-membership-for-user-in-org', { params: { ...ts, username: s.login } });
      }
      if (s.collaborator) {
        const u = O(s, { username: s.collaborator });
        await t.call('orgs/set-membership-for-user', { params: u, body: { role: 'member' } });
        await t.call('orgs/get-membership-for-user', { params: u });
        if (ts) {
          await t.call('teams/add-or-update-membership-for-user-in-org', { params: { ...ts, username: s.collaborator }, body: { role: 'maintainer' } });
          await t.call('teams/remove-membership-for-user-in-org', { params: { ...ts, username: s.collaborator } });
        }
        await t.call('orgs/remove-membership-for-user', { params: u });
        await t.call('orgs/set-membership-for-user', { params: u, body: { role: 'member' } });
        await t.call('orgs/remove-member', { params: u });
      }
      if (ts) await t.call('teams/delete-in-org', { params: ts });
      if (h?.body?.id) {
        const hp = O(s, { hook_id: h.body.id });
        const d = await t.waitFor('orgs/list-webhook-deliveries', { params: hp }, (x) => Array.isArray(x.body) && x.body.length > 0);
        await t.call('orgs/list-webhook-deliveries', { params: hp, minItems: d?.body?.length ? 1 : 0 });
        const did = d?.body?.[0]?.id;
        if (did) {
          await t.call('orgs/get-webhook-delivery', { params: { ...hp, delivery_id: did } });
          await t.call('orgs/redeliver-webhook-delivery', { params: { ...hp, delivery_id: did } });
        }
        await t.call('orgs/delete-webhook', { params: hp });
      }
    },
  },
  {
    name: 'orgs: delete organization',
    requires: ['login'],
    run: async (t, s) => {
      const login = `conf-${s.runId}-del`;
      const o = await t.call('enterprise-admin/create-org', { body: { login, admin: s.login }, expect: [201] });
      if (o?.body?.login) await t.call('orgs/delete', { params: { org: o.body.login }, expect: [202] });
    },
  },
];
