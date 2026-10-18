// Area "apps" (docs/BUILDOUT.md): GitHub Apps, OAuth token endpoints, the legacy OAuth
// Authorizations API and migrations. Apps are registered through forge's own endpoint
// (`POST /api/forge/apps`, or the manifest flow `POST /api/forge/app-manifests` +
// `apps/create-from-manifest`), so against GitHub these scenarios stop at their first fixture step.
// JWTs are signed with node:crypto. Everything created here is removed again.
import { createSign } from 'node:crypto';

const P = (s, extra = {}) => ({ ...s.rp, ...extra });
const b64url = (b) => Buffer.from(b).toString('base64').replace(/=+$/, '').replace(/\+/g, '-').replace(/\//g, '_');
const basic = (a, b) => `Basic ${Buffer.from(`${a}:${b}`).toString('base64')}`;

/** RS256 JWT for a GitHub App (iss = app id, 9 minutes). */
export function appJwt(appId, pem) {
  const now = Math.floor(Date.now() / 1000);
  const input = `${b64url(JSON.stringify({ alg: 'RS256', typ: 'JWT' }))}.${b64url(JSON.stringify({ iat: now - 30, exp: now + 540, iss: appId }))}`;
  const sig = createSign('RSA-SHA256').update(input).sign(pem);
  return `${input}.${b64url(sig)}`;
}

const origin = (t) => new URL(t.base).origin;

/** Forge-only JSON request (not validated, not recorded). */
async function forge(t, method, path, body, opts = {}) {
  const res = await t.http(method, `${origin(t)}${path}`, { body, ...opts });
  let json = null;
  try { json = JSON.parse(res.text); } catch { /* empty */ }
  return { status: res.status, body: json, text: res.text };
}

function fixture(res, what, codes = [200, 201]) {
  if (!codes.includes(res.status)) throw new Error(`${what}: ${res.status} ${res.text.slice(0, 200)}`);
  return res.body;
}

/** A user-to-server token for `app` through the device flow (approved by the token user). */
async function deviceToken(t, app) {
  const form = (o) => ({ rawBody: new URLSearchParams(o).toString(), contentType: 'application/x-www-form-urlencoded', headers: { accept: 'application/json' }, token: '' });
  const code = JSON.parse((await t.http('POST', `${origin(t)}/login/device/code`, form({ client_id: app.client_id }))).text);
  fixture(await forge(t, 'POST', '/api/forge/device/approve', { user_code: code.user_code }), 'device approve', [204]);
  const tok = JSON.parse((await t.http('POST', `${origin(t)}/login/oauth/access_token`, form({
    client_id: app.client_id, device_code: code.device_code, grant_type: 'urn:ietf:params:oauth:grant-type:device_code',
  }))).text);
  if (!tok.access_token) throw new Error(`device flow: ${JSON.stringify(tok)}`);
  return tok.access_token;
}

export const scenarios = [
  {
    name: 'apps: GitHub App, JWT, installations, installation tokens, webhook',
    requires: ['login', 'rp', 'repoId'],
    run: async (t, s) => {
      const app = fixture(await forge(t, 'POST', '/api/forge/apps', {
        name: `conf-${s.runId}`, url: 'https://example.com/conformance', public: true, description: 'conformance app',
        hook_attributes: { url: 'https://example.com/conformance-hook' },
        default_permissions: { issues: 'write', contents: 'write', checks: 'write', deployments: 'write', administration: 'read' },
        default_events: ['issues', 'push'],
      }), 'register app');
      s.app = app;
      const J = () => ({ token: '', headers: { authorization: `Bearer ${appJwt(app.id, app.pem)}` } });
      const CC = { token: '', headers: { authorization: basic(app.client_id, app.client_secret) } };
      try {
        await t.call('apps/get-authenticated', J());
        await t.call('apps/get-by-slug', { params: { app_slug: app.slug } });
        const inst = fixture(await forge(t, 'POST', `/api/forge/apps/${app.slug}/installations`, {
          repository_selection: 'selected', repository_ids: [s.repoId],
        }), 'install app');
        const iid = inst.id;
        await t.call('apps/list-installations', { ...J(), minItems: 1 });
        await t.call('apps/get-installation', { ...J(), params: { installation_id: iid } });
        await t.call('apps/get-user-installation', { ...J(), params: { username: s.login } });
        await t.call('apps/get-repo-installation', { ...J(), params: s.rp });
        await t.call('apps/list-installation-requests-for-authenticated-app', J());

        // installation access token, then calls made with it
        const tok = await t.call('apps/create-installation-access-token', {
          ...J(), params: { installation_id: iid }, expect: [201], fixture: true,
          body: { repository_ids: [s.repoId], permissions: { issues: 'write', contents: 'read', checks: 'write' } },
        });
        const ghs = tok.body.token;
        await t.call('apps/list-repos-accessible-to-installation', { token: ghs, minItems: 1, itemsKey: 'repositories' });
        await t.call('issues/create', { params: s.rp, token: ghs, body: { title: 'Opened by the conformance app' }, expect: [201] });

        // the app's webhook (an installation event was queued above)
        await t.call('apps/get-webhook-config-for-app', J());
        await t.call('apps/update-webhook-config-for-app', { ...J(), body: { content_type: 'json', insecure_ssl: '0' } });
        const dl = await t.call('apps/list-webhook-deliveries', { ...J(), minItems: 1 });
        const did = dl?.body?.[0]?.id;
        if (did) {
          await t.call('apps/get-webhook-delivery', { ...J(), params: { delivery_id: did } });
          await t.call('apps/redeliver-webhook-delivery', { ...J(), params: { delivery_id: did } });
        }

        // the user's view: installations and their repositories, adding/removing one
        await t.call('apps/list-installations-for-authenticated-user', { minItems: 1, itemsKey: 'installations' });
        await t.call('apps/list-installation-repos-for-authenticated-user', { params: { installation_id: iid }, minItems: 1, itemsKey: 'repositories' });
        const extra = await t.call('repos/create-for-authenticated-user', { body: { name: `conf-${s.runId}-app`, auto_init: true }, expect: [201], fixture: true });
        (s.cleanup ||= []).push({ owner: extra.body.owner.login, repo: extra.body.name });
        await t.call('apps/add-repo-to-installation-for-authenticated-user', { params: { installation_id: iid, repository_id: extra.body.id } });
        await t.call('apps/remove-repo-from-installation-for-authenticated-user', { params: { installation_id: iid, repository_id: extra.body.id } });

        // custom deployment protection rules backed by the installed app
        const env = P(s, { environment_name: `conf-app-${s.runId}` });
        await t.call('repos/create-or-update-environment', { params: env, body: {}, fixture: true });
        await t.call('repos/list-custom-deployment-rule-integrations', { params: env, minItems: 1, itemsKey: 'available_custom_deployment_protection_rule_integrations' });
        const rule = await t.call('repos/create-deployment-protection-rule', { params: env, body: { integration_id: app.id }, expect: [201] });
        await t.call('repos/get-all-deployment-protection-rules', { params: env, minItems: 1, itemsKey: 'custom_deployment_protection_rules' });
        if (rule?.body?.id) {
          await t.call('repos/get-custom-deployment-protection-rule', { params: { ...env, protection_rule_id: rule.body.id } });
          await t.call('repos/disable-deployment-protection-rule', { params: { ...env, protection_rule_id: rule.body.id } });
        }
        await t.call('repos/delete-an-environment', { params: env });

        await t.call('apps/suspend-installation', { ...J(), params: { installation_id: iid } });
        await t.call('apps/unsuspend-installation', { ...J(), params: { installation_id: iid } });
        const tok2 = await t.call('apps/create-installation-access-token', { ...J(), params: { installation_id: iid }, expect: [201] });
        if (tok2?.body?.token) await t.call('apps/revoke-installation-access-token', { token: tok2.body.token });

        // user-to-server tokens (device flow) and the OAuth token endpoints
        const ghu = await deviceToken(t, app);
        await t.call('apps/check-token', { ...CC, params: { client_id: app.client_id }, body: { access_token: ghu } });
        await t.call('apps/scope-token', { ...CC, params: { client_id: app.client_id }, body: { access_token: ghu, target: s.login, permissions: { issues: 'read' } } });
        const reset = await t.call('apps/reset-token', { ...CC, params: { client_id: app.client_id }, body: { access_token: ghu } });
        if (reset?.body?.token) await t.call('apps/delete-token', { ...CC, params: { client_id: app.client_id }, body: { access_token: reset.body.token } });
        const ghu2 = await deviceToken(t, app);
        await t.call('apps/delete-authorization', { ...CC, params: { client_id: app.client_id }, body: { access_token: ghu2 } });

        await t.call('apps/delete-installation', { ...J(), params: { installation_id: iid } });

        // enterprise installation (site admin)
        const ent = await forge(t, 'POST', `/api/forge/apps/${app.slug}/installations`, { enterprise: true });
        if (ent.status === 201) {
          await t.call('apps/get-enterprise-installation', { ...J(), params: { enterprise: 'forge' } });
          await t.call('apps/delete-installation', { ...J(), params: { installation_id: ent.body.id } });
        }
      } finally {
        await forge(t, 'DELETE', `/api/forge/apps/${app.slug}`);
      }
    },
  },
  {
    name: 'apps: enterprise app installations on an organization',
    requires: ['login', 'org'],
    run: async (t, s) => {
      const app = fixture(await forge(t, 'POST', '/api/forge/apps', {
        name: `conf-${s.runId}-ent`, url: 'https://example.com/conformance', public: true,
        default_permissions: { contents: 'read', issues: 'read' },
      }), 'register app');
      const J = () => ({ token: '', headers: { authorization: `Bearer ${appJwt(app.id, app.pem)}` } });
      const E = { enterprise: 'forge', org: s.org };
      try {
        const r = await t.call('repos/create-in-org', { params: { org: s.org }, body: { name: `conf-${s.runId}-oapp`, auto_init: true }, expect: [201], fixture: true });
        (s.cleanup ||= []).push({ owner: s.org, repo: r.body.name });
        await t.call('enterprise-apps/installable-organizations', { params: { enterprise: 'forge' }, minItems: 1 });
        await t.call('enterprise-apps/installable-organization-accessible-repositories', { params: E, minItems: 1 });
        const inst = await t.call('enterprise-apps/create-installation', {
          params: E, body: { client_id: app.client_id, repository_selection: 'selected', repositories: [r.body.name] }, expect: [201], fixture: true,
        });
        const id = inst.body.id;
        const I = { ...E, installation_id: id };
        await t.call('enterprise-apps/organization-installations', { params: E, minItems: 1 });
        await t.call('apps/get-org-installation', { ...J(), params: { org: s.org } });
        await t.call('enterprise-apps/organization-installation-repositories', { params: I, minItems: 1 });
        await t.call('enterprise-apps/change-installation-repository-access-selection', { params: I, body: { repository_selection: 'selected', repositories: [] } });
        await t.call('enterprise-apps/grant-repository-access-to-installation', { params: I, body: { repositories: [r.body.name] }, minItems: 1 });
        await t.call('enterprise-apps/remove-repository-access-to-installation', { params: I, body: { repositories: [r.body.name] } });
        await t.call('enterprise-apps/delete-installation', { params: I });
      } finally {
        await forge(t, 'DELETE', `/api/forge/apps/${app.slug}`);
      }
    },
  },
  {
    name: 'apps: manifest flow',
    requires: ['login'],
    run: async (t, s) => {
      const m = fixture(await forge(t, 'POST', '/api/forge/app-manifests', {
        manifest: { name: `conf-${s.runId}-mf`, url: 'https://example.com/manifest', default_permissions: { issues: 'read' }, default_events: ['issues'] },
      }), 'manifest code');
      const res = await t.call('apps/create-from-manifest', { params: { code: m.code }, token: '', expect: [201] });
      if (res?.body?.slug) await forge(t, 'DELETE', `/api/forge/apps/${res.body.slug}`);
    },
  },
  {
    // Basic auth only: the token user's login with its token as the password.
    name: 'oauth-authorizations: authorizations and grants',
    requires: ['login'],
    run: async (t, s) => {
      const B = { token: '', headers: { authorization: basic(s.login, t.token) } };
      const oa = fixture(await forge(t, 'POST', '/api/forge/oauth-apps', { name: `conf-${s.runId}-oauth`, url: 'https://example.com/oauth' }), 'oauth app');
      try {
        const a = await t.call('oauth-authorizations/create-authorization', { ...B, body: { scopes: ['repo'], note: `conf-${s.runId}` }, expect: [201], fixture: true });
        const id = a.body.id;
        await t.call('oauth-authorizations/get-authorization', { ...B, params: { authorization_id: id } });
        await t.call('oauth-authorizations/update-authorization', { ...B, params: { authorization_id: id }, body: { add_scopes: ['gist'], note_url: 'https://example.com/note' } });
        await t.call('oauth-authorizations/list-authorizations', { ...B, minItems: 1 });
        await t.call('oauth-authorizations/get-or-create-authorization-for-app', { ...B, params: { client_id: oa.client_id }, body: { client_secret: oa.client_secret, scopes: ['repo'] }, expect: [201] });
        await t.call('oauth-authorizations/get-or-create-authorization-for-app', { ...B, params: { client_id: oa.client_id }, body: { client_secret: oa.client_secret, scopes: ['repo'] }, expect: [200] });
        await t.call('oauth-authorizations/get-or-create-authorization-for-app-and-fingerprint', {
          ...B, params: { client_id: oa.client_id, fingerprint: `fp-${s.runId}` }, body: { client_secret: oa.client_secret, scopes: ['repo'] }, expect: [201],
        });
        const grants = await t.call('oauth-authorizations/list-grants', { ...B, minItems: 1 });
        const g = (grants?.body || []).find((x) => x.app?.client_id === oa.client_id);
        if (g) {
          await t.call('oauth-authorizations/get-grant', { ...B, params: { grant_id: g.id } });
          await t.call('oauth-authorizations/delete-grant', { ...B, params: { grant_id: g.id } });
        }
        await t.call('oauth-authorizations/delete-authorization', { ...B, params: { authorization_id: id } });
      } finally {
        await forge(t, 'DELETE', `/api/forge/apps/${oa.slug}`);
      }
    },
  },
  {
    name: 'migrations: user export',
    requires: ['login', 'rp'],
    run: async (t, s) => {
      const m = await t.call('migrations/start-for-authenticated-user', { body: { repositories: [s.rp.repo], lock_repositories: false, exclude_attachments: true }, expect: [201], fixture: true });
      await t.call('migrations/list-for-authenticated-user', { minItems: 1 });
      await t.call('migrations/list-repos-for-authenticated-user', { params: { migration_id: m.body.id }, minItems: 1 });
      await t.waitFor('migrations/list-for-authenticated-user', {}, (x) => (x.body || []).some((y) => y.id === m.body.id && y.state === 'exported'), { tries: 12 });
      await t.call('migrations/get-archive-for-authenticated-user', { params: { migration_id: m.body.id }, expect: [302] });
    },
  },
  {
    name: 'migrations: organization export with locked repositories',
    requires: ['login', 'org'],
    run: async (t, s) => {
      const r = await t.call('repos/create-in-org', { params: { org: s.org }, body: { name: `conf-${s.runId}-omig`, auto_init: true }, expect: [201], fixture: true });
      (s.cleanup ||= []).push({ owner: s.org, repo: r.body.name });
      const m = await t.call('migrations/start-for-org', { params: { org: s.org }, body: { repositories: [r.body.name], lock_repositories: true }, expect: [201], fixture: true });
      const M = { org: s.org, migration_id: m.body.id };
      try {
        await t.call('migrations/list-for-org', { params: { org: s.org }, minItems: 1 });
        await t.call('migrations/list-repos-for-org', { params: M, minItems: 1 });
        await t.waitFor('migrations/get-status-for-org', { params: M }, (x) => x.body?.state === 'exported', { tries: 12 });
        await t.call('migrations/get-status-for-org', { params: M });
        await t.call('migrations/download-archive-for-org', { params: M, expect: [302] });
        await t.call('migrations/delete-archive-for-org', { params: M });
      } finally {
        await t.call('migrations/unlock-repo-for-org', { params: { ...M, repo_name: r.body.name } });
      }
    },
  },
];
