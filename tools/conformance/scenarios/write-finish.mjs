// Area "finish" scenarios (docs/BUILDOUT.md): the last operations not exercised elsewhere.
// - gitignore/get-template, licenses/get, markdown/render-raw (text/plain body);
// - repository invitations: the site setting `repository_invitations.enabled` (Manage API
//   settings; forge's default is GHES's direct collaborator add) is switched on for the
//   duration of the scenario, three invitations to --collaborator are created on a fixture
//   repository, accepted / declined with --collaborator-token, updated and deleted; the setting
//   and the fixture repository are restored / deleted afterwards;
// - enterprise runner group membership with a JIT-registered enterprise runner.
//
// The harness token must belong to a site administrator.
import { Blocked } from '../lib/oracle.mjs';

const ENT = process.env.FORGE_ENTERPRISE_SLUG || 'forge';

export const scenarios = [
  {
    name: 'finish: gitignore template, license, raw markdown',
    always: true,
    run: async (t) => {
      const all = await t.call('gitignore/get-all-templates');
      const name = all?.body?.includes?.('Node') ? 'Node' : all?.body?.[0] || 'Node';
      await t.call('gitignore/get-template', { params: { name } });
      const lic = await t.call('licenses/get-all-commonly-used');
      await t.call('licenses/get', { params: { license: lic?.body?.[0]?.key || 'mit' } });
      await t.call('markdown/render-raw', { rawBody: 'Hello **world** _raw_', contentType: 'text/plain' });
    },
  },
  {
    name: 'finish: repository invitations',
    requires: ['login', 'collaborator', 'collaboratorToken'],
    run: async (t, s) => {
      const as = s.collaboratorToken;
      const settings = await t.call('enterprise-admin/get-manage-settings', { fixture: true });
      const before = settings.body?.repository_invitations?.enabled ?? false;
      const repos = [];
      try {
        await t.call('enterprise-admin/set-manage-settings', { body: { repository_invitations: { enabled: true } }, expect: [204], fixture: true });
        // One fixture repository per invitation: an invitee has at most one pending
        // invitation per repository (re-inviting refreshes it).
        for (const tag of ['accept', 'decline', 'edit']) {
          const r = await t.call('repos/create-for-authenticated-user', {
            body: { name: `conf-inv-${tag}-${s.runId}`, private: true, auto_init: true }, expect: [201], fixture: true,
          });
          repos.push({ owner: r.body.owner.login, repo: r.body.name });
        }
        // GHES documents only 204 for add-collaborator (direct add), so the github.com-style 201
        // with an invitation is an unrecorded probe here.
        const invs = [];
        for (const rp of repos) {
          const inv = await t.probe('repos/add-collaborator', { params: { ...rp, username: s.collaborator }, body: { permission: 'push' } });
          if (inv.status !== 201 || !inv.body?.id) throw new Blocked(`add-collaborator with invitations enabled returned ${inv.status}: ${inv.text.slice(0, 200)}`);
          invs.push(inv.body.id);
        }
        await t.call('repos/list-invitations-for-authenticated-user', { token: as, minItems: 3 });
        await t.call('repos/list-invitations', { params: repos[2], minItems: 1 });
        await t.call('repos/accept-invitation-for-authenticated-user', { token: as, params: { invitation_id: invs[0] }, expect: [204] });
        await t.call('repos/decline-invitation-for-authenticated-user', { token: as, params: { invitation_id: invs[1] }, expect: [204] });
        await t.call('repos/update-invitation', { params: { ...repos[2], invitation_id: invs[2] }, body: { permissions: 'read' } });
        await t.call('repos/delete-invitation', { params: { ...repos[2], invitation_id: invs[2] }, expect: [204] });
        // Accepting made the collaborator a collaborator; declining did not.
        await t.call('repos/check-collaborator', { params: { ...repos[0], username: s.collaborator }, expect: [204] });
        await t.call('repos/check-collaborator', { params: { ...repos[1], username: s.collaborator }, expect: [404] });
      } finally {
        await t.probe('enterprise-admin/set-manage-settings', { body: { repository_invitations: { enabled: before } } });
        if (!s.keep) for (const rp of repos) await t.probe('repos/delete', { params: rp });
      }
    },
  },
  {
    name: 'finish: enterprise runner group membership',
    requires: ['login'],
    run: async (t, s) => {
      const e = { enterprise: ENT };
      const jit = await t.call('actions/generate-runner-jitconfig-for-enterprise', {
        params: e, body: { name: `conf-ent-grp-runner-${s.runId}`, runner_group_id: 1, labels: ['self-hosted', 'linux'] }, expect: [201], fixture: true,
      });
      const runner_id = jit.body.runner.id;
      let group = null;
      try {
        const g = await t.call('enterprise-admin/create-self-hosted-runner-group-for-enterprise', {
          params: e, body: { name: `conf-ent-grp-${s.runId}`, visibility: 'all' }, expect: [201], fixture: true,
        });
        group = g.body.id;
        const gp = { ...e, runner_group_id: group };
        await t.call('enterprise-admin/add-self-hosted-runner-to-group-for-enterprise', { params: { ...gp, runner_id }, expect: [204] });
        await t.call('enterprise-admin/remove-self-hosted-runner-from-group-for-enterprise', { params: { ...gp, runner_id }, expect: [204] });
      } finally {
        if (group) await t.probe('enterprise-admin/delete-self-hosted-runner-group-from-enterprise', { params: { ...e, runner_group_id: group } });
        await t.probe('enterprise-admin/delete-self-hosted-runner-from-enterprise', { params: { ...e, runner_id } });
      }
    },
  },
  // push bypass requests: added by parent
];
