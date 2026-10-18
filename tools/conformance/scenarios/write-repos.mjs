// Build-out area `repos`: scenarios for the repos, users, issues, gists, activity,
// codes-of-conduct and dependency-graph tags (docs/BUILDOUT.md). They run after the core
// scenarios (write.mjs) and before `cleanup`, using its shared state (s.rp, s.login, s.def,
// s.org, s.collaborator, s.me). Everything created here is deleted here.
//
// Optional: --collaborator-token (s.collaboratorToken). With it the scenarios also act as the
// collaborator (notifications need someone else's activity; gist forks need a non-owner).
import { generateKeyPairSync } from 'node:crypto';

const b64 = (str) => Buffer.from(str, 'utf8').toString('base64');
const P = (s, extra = {}) => ({ ...s.rp, ...extra });

function sshPublicKey(comment = 'conformance') {
  const { publicKey } = generateKeyPairSync('ed25519');
  const raw = publicKey.export({ format: 'der', type: 'spki' }).subarray(-32);
  const str = (b) => { const l = Buffer.alloc(4); l.writeUInt32BE(b.length); return Buffer.concat([l, b]); };
  return `ssh-ed25519 ${Buffer.concat([str(Buffer.from('ssh-ed25519')), str(raw)]).toString('base64')} ${comment}`;
}

/** A one-directory ustar archive with the given files ({name: content}). */
function tarball(files) {
  const blocks = [];
  for (const [name, content] of Object.entries(files)) {
    const body = Buffer.from(content);
    const h = Buffer.alloc(512);
    h.write(name, 0, 100, 'utf8');
    h.write('0000644\0', 100); h.write('0000000\0', 108); h.write('0000000\0', 116);
    h.write(body.length.toString(8).padStart(11, '0') + '\0', 124);
    h.write(Math.floor(Date.now() / 1000).toString(8).padStart(11, '0') + '\0', 136);
    h.write('        ', 148); h.write('0', 156); h.write('ustar\x0000', 257);
    let sum = 0; for (const b of h) sum += b;
    h.write(sum.toString(8).padStart(6, '0') + '\0 ', 148);
    blocks.push(h, body, Buffer.alloc((512 - (body.length % 512)) % 512));
  }
  blocks.push(Buffer.alloc(1024));
  return Buffer.concat(blocks);
}

/** Runs `fn` with another user's token. */
async function as(t, token, fn) {
  const saved = t.token;
  t.token = token;
  try { return await fn(); } finally { t.token = saved; }
}
const collabToken = (s) => s.collaboratorToken || '';

async function putFile(t, s, path, content, branch, rp = s.rp) {
  const cur = await t.probe('repos/get-content', { params: { ...rp, path }, query: branch ? { ref: branch } : {} });
  return t.call('repos/create-or-update-file-contents', {
    params: { ...rp, path },
    body: { message: `conformance: ${path}`, content: b64(content), ...(branch ? { branch } : {}), ...(cur.status === 200 && cur.body?.sha ? { sha: cur.body.sha } : {}) },
    expect: cur.status === 200 ? [200] : [201],
  });
}

async function orgRepo(t, s) {
  if (!s.org) return null;
  if (s.repoOrg) return s.repoOrg;
  const r = await t.call('repos/create-in-org', { params: { org: s.org }, body: { name: `conf-${s.runId}-repos`, private: false, auto_init: true }, expect: [201] });
  if (!r?.body?.name) return null;
  (s.cleanup ||= []).push({ owner: s.org, repo: r.body.name });
  await t.waitFor('repos/get-branch', { params: { owner: s.org, repo: r.body.name, branch: r.body.default_branch || 'main' } }, (x) => x.status === 200);
  s.repoOrg = { owner: s.org, repo: r.body.name, def: r.body.default_branch || 'main', id: r.body.id };
  return s.repoOrg;
}

export const scenarios = [
  {
    name: 'repos: environments, branch policies, deployment statuses',
    requires: ['rp', 'me'],
    run: async (t, s) => {
      const env = { ...s.rp, environment_name: 'conf-env' };
      await t.call('repos/create-or-update-environment', {
        params: env,
        body: { wait_timer: 5, prevent_self_review: false, reviewers: [{ type: 'User', id: s.me.id }], deployment_branch_policy: { protected_branches: false, custom_branch_policies: true } },
      });
      await t.call('repos/get-environment', { params: env });
      await t.call('repos/get-all-environments', { params: s.rp, minItems: 1 });
      const bp = await t.call('repos/create-deployment-branch-policy', { params: env, body: { name: 'release/*', type: 'branch' }, expect: [200] });
      await t.call('repos/list-deployment-branch-policies', { params: env, minItems: 1 });
      if (bp?.body?.id) {
        const pid = { ...env, branch_policy_id: bp.body.id };
        await t.call('repos/get-deployment-branch-policy', { params: pid });
        await t.call('repos/update-deployment-branch-policy', { params: pid, body: { name: 'release/**' } });
        await t.call('repos/delete-deployment-branch-policy', { params: pid });
      }
      await t.call('repos/get-all-deployment-protection-rules', { params: env });
      await t.call('repos/list-custom-deployment-rule-integrations', { params: env });
      // A deployment status, read back by id.
      const d = await t.call('repos/create-deployment', { params: s.rp, body: { ref: s.def, environment: 'conf-env', required_contexts: [], auto_merge: false }, expect: [201] });
      if (d?.body?.id) {
        const st = await t.call('repos/create-deployment-status', { params: P(s, { deployment_id: d.body.id }), body: { state: 'inactive', description: 'conformance' }, expect: [201] });
        if (st?.body?.id) await t.call('repos/get-deployment-status', { params: P(s, { deployment_id: d.body.id, status_id: st.body.id }) });
        await t.call('repos/delete-deployment', { params: P(s, { deployment_id: d.body.id }) });
      }
      await t.call('repos/delete-an-environment', { params: env });
    },
  },
  {
    name: 'repos: security & feature toggles',
    requires: ['rp'],
    run: async (t, s) => {
      await t.call('repos/enable-vulnerability-alerts', { params: s.rp });
      await t.call('repos/check-vulnerability-alerts', { params: s.rp, expect: [204] });
      await t.call('repos/check-automated-security-fixes', { params: s.rp });
      await t.call('repos/disable-vulnerability-alerts', { params: s.rp });
      await t.call('repos/check-vulnerability-alerts', { params: s.rp, expect: [404], note: 'after disable' });
      await t.call('repos/enable-immutable-releases', { params: s.rp });
      await t.call('repos/check-immutable-releases', { params: s.rp });
      await t.call('repos/disable-immutable-releases', { params: s.rp });
      await t.call('repos/enable-lfs-for-repo', { params: s.rp, expect: [202] });
      await t.call('repos/disable-lfs-for-repo', { params: s.rp });
    },
  },
  {
    name: 'repos: properties, dispatch, CODEOWNERS, teams, caches, public list',
    requires: ['rp', 'login'],
    run: async (t, s) => {
      await t.call('repos/custom-properties-for-repos-get-repository-values', { params: s.rp });
      const or = await orgRepo(t, s);
      if (or) {
        const rp = { owner: or.owner, repo: or.repo };
        // Values must match an organization property definition (orgs area).
        const prop = `conf_${s.runId}`;
        await t.probe('orgs/custom-properties-for-repos-create-or-update-organization-definition', { params: { org: s.org, custom_property_name: prop }, body: { value_type: 'string' } });
        await t.call('repos/custom-properties-for-repos-create-or-update-repository-values', { params: rp, body: { properties: [{ property_name: prop, value: 'forge' }] } });
        await t.call('repos/custom-properties-for-repos-get-repository-values', { params: rp, minItems: 1 });
        await t.probe('orgs/custom-properties-for-repos-delete-organization-definition', { params: { org: s.org, custom_property_name: prop } });
        await t.call('repos/list-teams', { params: rp });
      }
      await t.call('repos/create-dispatch-event', { params: s.rp, body: { event_type: 'conformance', client_payload: { run: s.runId } } });
      await putFile(t, s, '.github/CODEOWNERS', `* @${s.login}\n/docs/ @no-such-user-${s.runId}\n`);
      await t.call('repos/codeowners-errors', { params: s.rp });
      await t.call('repos/list-teams', { params: s.rp });
      await t.call('repos/list-cache-info', { params: s.rp, minItems: 1 });
      await t.call('repos/list-public', { query: { since: Math.max(0, (s.repoId || 1) - 1) }, minItems: 1 });
      await t.call('repos/list-activities', { params: s.rp, minItems: 1 });
      await putFile(t, s, 'docs/sub/README.md', '# sub readme\n');
      await t.call('repos/get-readme-in-directory', { params: P(s, { dir: 'docs/sub' }) });
    },
  },
  {
    name: 'repos: statistics',
    requires: ['rp'],
    run: async (t, s) => {
      await t.call('repos/get-code-frequency-stats', { params: s.rp });
      await t.call('repos/get-commit-activity-stats', { params: s.rp, minItems: 1 });
      await t.call('repos/get-contributors-stats', { params: s.rp });
      await t.call('repos/get-participation-stats', { params: s.rp });
      await t.call('repos/get-punch-card-stats', { params: s.rp, minItems: 1 });
    },
  },
  {
    name: 'repos: branch protection sub-resources',
    requires: ['rp', 'baseSha', 'login'],
    run: async (t, s) => {
      const branch = `conf-prot-${s.runId}`;
      await t.call('git/create-ref', { params: s.rp, body: { ref: `refs/heads/${branch}`, sha: s.baseSha }, expect: [201] });
      const bp = P(s, { branch });
      await t.call('repos/update-branch-protection', { params: bp, body: { required_status_checks: { strict: false, contexts: ['ci/a'] }, enforce_admins: false, required_pull_request_reviews: null, restrictions: null } });
      await t.call('repos/get-branch-protection', { params: bp });
      await t.call('repos/get-all-status-check-contexts', { params: bp, minItems: 1 });
      await t.call('repos/add-status-check-contexts', { params: bp, body: { contexts: ['ci/b'] } });
      await t.call('repos/set-status-check-contexts', { params: bp, body: { contexts: ['ci/a', 'ci/c'] } });
      await t.call('repos/remove-status-check-contexts', { params: bp, body: { contexts: ['ci/c'] } });
      await t.call('repos/get-status-checks-protection', { params: bp });
      await t.call('repos/update-status-check-protection', { params: bp, body: { strict: true } });
      await t.call('repos/remove-status-check-protection', { params: bp });
      await t.call('repos/get-admin-branch-protection', { params: bp });
      await t.call('repos/set-admin-branch-protection', { params: bp });
      await t.call('repos/delete-admin-branch-protection', { params: bp });
      await t.call('repos/get-commit-signature-protection', { params: bp });
      await t.call('repos/create-commit-signature-protection', { params: bp });
      await t.call('repos/delete-commit-signature-protection', { params: bp });
      await t.call('repos/update-pull-request-review-protection', { params: bp, body: { required_approving_review_count: 1, dismiss_stale_reviews: true } });
      await t.call('repos/get-pull-request-review-protection', { params: bp });
      await t.call('repos/delete-pull-request-review-protection', { params: bp });
      await t.call('repos/get-branch-rules', { params: bp });
      await t.call('repos/delete-branch-protection', { params: bp });
      await t.call('git/delete-ref', { params: P(s, { ref: `heads/${branch}` }) });
      // Push restrictions are for organization repositories.
      const or = await orgRepo(t, s);
      if (!or) return;
      const ob = { owner: or.owner, repo: or.repo, branch: or.def };
      const team = await t.call('teams/create', { params: { org: s.org }, body: { name: `conf-${s.runId}-team`, privacy: 'closed' }, expect: [201] });
      await t.call('repos/update-branch-protection', { params: ob, body: { required_status_checks: null, enforce_admins: false, required_pull_request_reviews: null, restrictions: { users: [], teams: [] } } });
      await t.call('repos/get-access-restrictions', { params: ob });
      await t.call('repos/add-user-access-restrictions', { params: ob, body: { users: [s.login] } });
      await t.call('repos/get-users-with-access-to-protected-branch', { params: ob, minItems: 1 });
      await t.call('repos/set-user-access-restrictions', { params: ob, body: { users: [s.login] } });
      await t.call('repos/remove-user-access-restrictions', { params: ob, body: { users: [s.login] } });
      if (team?.body?.slug) {
        await t.call('repos/add-team-access-restrictions', { params: ob, body: { teams: [team.body.slug] } });
        await t.call('repos/get-teams-with-access-to-protected-branch', { params: ob, minItems: 1 });
        await t.call('repos/set-team-access-restrictions', { params: ob, body: { teams: [team.body.slug] } });
        await t.call('repos/remove-team-access-restrictions', { params: ob, body: { teams: [team.body.slug] } });
      }
      await t.call('repos/set-app-access-restrictions', { params: ob, body: { apps: [] } });
      await t.call('repos/add-app-access-restrictions', { params: ob, body: { apps: [] } });
      await t.call('repos/get-apps-with-access-to-protected-branch', { params: ob });
      await t.call('repos/remove-app-access-restrictions', { params: ob, body: { apps: [] } });
      await t.call('repos/delete-access-restrictions', { params: ob });
      await t.call('repos/delete-branch-protection', { params: ob });
      if (team?.body?.slug) await t.call('teams/delete-in-org', { params: { org: s.org, team_slug: team.body.slug } });
    },
  },
  {
    name: 'repos: rulesets, history, rule suites',
    requires: ['rp'],
    run: async (t, s) => {
      const rs = await t.call('repos/create-repo-ruleset', {
        params: s.rp, expect: [201],
        body: { name: `conf-${s.runId}`, target: 'branch', enforcement: 'evaluate', conditions: { ref_name: { include: ['~DEFAULT_BRANCH'], exclude: [] } }, rules: [{ type: 'deletion' }, { type: 'non_fast_forward' }] },
      });
      await t.call('repos/get-repo-rulesets', { params: s.rp, minItems: 1 });
      if (rs?.body?.id) {
        const id = { ...s.rp, ruleset_id: rs.body.id };
        await t.call('repos/get-repo-ruleset', { params: id });
        await t.call('repos/update-repo-ruleset', { params: id, body: { rules: [{ type: 'deletion' }, { type: 'non_fast_forward' }, { type: 'creation' }] } });
        const hist = await t.call('repos/get-repo-ruleset-history', { params: id, minItems: 2 });
        const vid = hist?.body?.[hist.body.length - 1]?.version_id;
        if (vid) await t.call('repos/get-repo-ruleset-version', { params: { ...id, version_id: vid } });
        await putFile(t, s, `rules-${s.runId}.txt`, 'evaluated by a ruleset\n');
        const suites = await t.call('repos/get-repo-rule-suites', { params: s.rp, minItems: 1 });
        const sid = suites?.body?.[0]?.id;
        if (sid) await t.call('repos/get-repo-rule-suite', { params: P(s, { rule_suite_id: sid }) });
        await t.call('repos/delete-repo-ruleset', { params: id });
      }
      await t.call('repos/list-repo-push-bypass-requests', { params: s.rp });
      if (!s.org) return;
      const org = { org: s.org };
      const ors = await t.call('repos/create-org-ruleset', {
        params: org, expect: [201],
        body: { name: `conf-${s.runId}-org`, target: 'branch', enforcement: 'evaluate', conditions: { ref_name: { include: ['~ALL'], exclude: [] }, repository_name: { include: ['~ALL'], exclude: [] } }, rules: [{ type: 'deletion' }] },
      });
      await t.call('repos/get-org-rulesets', { params: org, minItems: 1 });
      if (ors?.body?.id) {
        const id = { ...org, ruleset_id: ors.body.id };
        await t.call('repos/get-org-ruleset', { params: id });
        await t.call('repos/update-org-ruleset', { params: id, body: { enforcement: 'evaluate' } });
        const or = await orgRepo(t, s);
        if (or) await putFile(t, s, `org-rules-${s.runId}.txt`, 'org ruleset\n', undefined, { owner: or.owner, repo: or.repo });
        const suites = await t.call('repos/get-org-rule-suites', { params: org, minItems: or ? 1 : 0 });
        const sid = suites?.body?.[0]?.id;
        if (sid) await t.call('repos/get-org-rule-suite', { params: { ...org, rule_suite_id: sid } });
        await t.call('repos/delete-org-ruleset', { params: id });
      }
    },
  },
  {
    name: 'repos: pages (legacy builds and artifact deployments)',
    requires: ['rp', 'def'],
    run: async (t, s) => {
      await putFile(t, s, 'index.html', `<h1>conformance ${s.runId}</h1>\n`);
      const site = await t.call('repos/create-pages-site', { params: s.rp, body: { build_type: 'legacy', source: { branch: s.def, path: '/' } }, expect: [201] });
      if (!site?.body) return;
      await t.call('repos/get-pages', { params: s.rp });
      await t.waitFor('repos/get-latest-pages-build', { params: s.rp }, (x) => x.status === 200 && ['built', 'errored'].includes(x.body?.status));
      const latest = await t.call('repos/get-latest-pages-build', { params: s.rp });
      await t.call('repos/list-pages-builds', { params: s.rp, minItems: 1 });
      const bid = latest?.body?.url?.split('/').pop();
      if (bid) await t.call('repos/get-pages-build', { params: P(s, { build_id: Number(bid) }) });
      await t.call('repos/request-pages-build', { params: s.rp, expect: [201] });
      await t.call('repos/update-information-about-pages-site', { params: s.rp, body: { build_type: 'workflow' } });
      // Publish an artifact (a tarball uploaded as a release asset of the same server).
      const rel = await t.call('repos/create-release', { params: s.rp, body: { tag_name: `pages-${s.runId}`, target_commitish: s.def, name: 'pages artifact' }, expect: [201] });
      if (rel?.body?.upload_url) {
        const up = await t.call('repos/upload-release-asset', {
          params: P(s, { release_id: rel.body.id }), url: rel.body.upload_url, query: { name: 'artifact.tar' },
          rawBody: tarball({ 'index.html': '<h1>deployed</h1>\n', '404.html': 'not here\n' }), contentType: 'application/x-tar', expect: [201],
        });
        if (up?.body?.browser_download_url) {
          const dep = await t.call('repos/create-pages-deployment', {
            params: s.rp, expect: [200],
            body: { artifact_url: up.body.browser_download_url, environment: 'github-pages', pages_build_version: s.baseSha || 'conformance', oidc_token: 'conformance' },
          });
          if (dep?.body?.id !== undefined) {
            const did = { ...s.rp, pages_deployment_id: dep.body.id };
            await t.waitFor('repos/get-pages-deployment', { params: did }, (x) => x.status === 200 && ['succeed', 'deployment_failed'].includes(x.body?.status));
            await t.call('repos/get-pages-deployment', { params: did });
            await t.call('repos/cancel-pages-deployment', { params: did });
          }
        }
        await t.call('repos/delete-release', { params: P(s, { release_id: rel.body.id }) });
        await t.probe('git/delete-ref', { params: P(s, { ref: `tags/pages-${s.runId}` }) });
      }
      await t.call('repos/delete-pages-site', { params: s.rp });
    },
  },
  {
    name: 'repos: webhooks extras, deploy keys, autolinks, transfer',
    requires: ['rp', 'login'],
    run: async (t, s) => {
      const h = await t.call('repos/create-webhook', { params: s.rp, expect: [201], body: { name: 'web', active: true, events: ['push'], config: { url: 'https://example.com/conformance-hook', content_type: 'json' } } });
      if (h?.body?.id) {
        const hid = P(s, { hook_id: h.body.id });
        await t.call('repos/get-webhook-config-for-repo', { params: hid });
        await t.call('repos/update-webhook-config-for-repo', { params: hid, body: { content_type: 'form', insecure_ssl: '0' } });
        await t.call('repos/test-push-webhook', { params: hid });
        await t.call('repos/ping-webhook', { params: hid });
        const dl = await t.waitFor('repos/list-webhook-deliveries', { params: hid }, (x) => x.status === 200 && x.body?.length > 0);
        await t.call('repos/list-webhook-deliveries', { params: hid, minItems: dl?.body?.length ? 1 : 0 });
        const did = dl?.body?.[0]?.id;
        if (did) {
          await t.call('repos/get-webhook-delivery', { params: { ...hid, delivery_id: did } });
          await t.call('repos/redeliver-webhook-delivery', { params: { ...hid, delivery_id: did }, expect: [202] });
        }
        await t.call('repos/delete-webhook', { params: hid });
      }
      const k = await t.call('repos/create-deploy-key', { params: s.rp, body: { title: `conf-${s.runId}`, key: sshPublicKey('deploy'), read_only: true }, expect: [201] });
      await t.call('repos/list-deploy-keys', { params: s.rp, minItems: k?.body?.id ? 1 : 0 });
      if (k?.body?.id) {
        await t.call('repos/get-deploy-key', { params: P(s, { key_id: k.body.id }) });
        await t.call('repos/delete-deploy-key', { params: P(s, { key_id: k.body.id }) });
      }
      const a = await t.call('repos/create-autolink', { params: s.rp, body: { key_prefix: 'CONF-', url_template: 'https://example.com/conf/<num>', is_alphanumeric: false }, expect: [201] });
      await t.call('repos/list-autolinks', { params: s.rp, minItems: a?.body?.id ? 1 : 0 });
      if (a?.body?.id) {
        await t.call('repos/get-autolink', { params: P(s, { autolink_id: a.body.id }) });
        await t.call('repos/delete-autolink', { params: P(s, { autolink_id: a.body.id }) });
      }
      await t.call('repos/list-invitations-for-authenticated-user');
      if (!s.org) return;
      const name = `conf-${s.runId}-xfer`;
      const r = await t.call('repos/create-for-authenticated-user', { body: { name, auto_init: true }, expect: [201] });
      if (!r?.body?.name) return;
      const x = await t.call('repos/transfer', { params: { owner: s.login, repo: name }, body: { new_owner: s.org, new_name: name }, expect: [202] });
      if (x?.status === 202) {
        (s.cleanup ||= []).push({ owner: s.org, repo: name });
        await t.call('repos/get', { params: { owner: s.login, repo: name }, note: 'old path redirects after transfer' });
      } else {
        (s.cleanup ||= []).push({ owner: s.login, repo: name });
      }
    },
  },
  {
    name: 'users: emails, social accounts, follows, signing keys, hovercard',
    requires: ['login', 'me'],
    run: async (t, s) => {
      const email = `conf-${s.runId}@example.com`;
      await t.call('users/add-email-for-authenticated-user', { body: { emails: [email] }, expect: [201] });
      await t.call('users/list-emails-for-authenticated-user', { minItems: 2 });
      await t.call('users/list-public-emails-for-authenticated-user');
      await t.call('users/delete-email-for-authenticated-user', { body: { emails: [email] } });
      const url = `https://example.com/conf-${s.runId}`;
      await t.call('users/add-social-account-for-authenticated-user', { body: { account_urls: [url] }, expect: [201] });
      await t.call('users/list-social-accounts-for-authenticated-user', { minItems: 1 });
      await t.call('users/list-social-accounts-for-user', { params: { username: s.login }, minItems: 1 });
      await t.call('users/delete-social-account-for-authenticated-user', { body: { account_urls: [url] } });
      await t.call('users/get-by-id', { params: { account_id: s.me.id } });
      await t.call('users/get-context-for-user', { params: { username: s.login } });
      if (s.repoId) await t.call('users/get-context-for-user', { params: { username: s.login }, query: { subject_type: 'repository', subject_id: String(s.repoId) } });
      if (s.collaborator) {
        await t.call('users/follow', { params: { username: s.collaborator } });
        await t.call('users/check-person-is-followed-by-authenticated', { params: { username: s.collaborator } });
        await t.call('users/check-following-for-user', { params: { username: s.login, target_user: s.collaborator } });
        await t.call('users/list-followed-by-authenticated-user', { minItems: 1 });
        await t.call('users/list-followers-for-authenticated-user');
        await t.call('users/unfollow', { params: { username: s.collaborator } });
      }
      const k = await t.call('users/create-ssh-signing-key-for-authenticated-user', { body: { title: `conf-${s.runId}`, key: sshPublicKey('signing') }, expect: [201] });
      await t.call('users/list-ssh-signing-keys-for-authenticated-user', { minItems: k?.body?.id ? 1 : 0 });
      await t.call('users/list-ssh-signing-keys-for-user', { params: { username: s.login } });
      if (k?.body?.id) {
        await t.call('users/get-ssh-signing-key-for-authenticated-user', { params: { ssh_signing_key_id: k.body.id } });
        await t.call('users/delete-ssh-signing-key-for-authenticated-user', { params: { ssh_signing_key_id: k.body.id } });
      }
      await t.call('users/list-gpg-keys-for-user', { params: { username: s.login } });
    },
  },
  {
    name: 'issues: pinned comments, dependencies, field values',
    requires: ['rp', 'login'],
    run: async (t, s) => {
      const a = await t.call('issues/create', { params: s.rp, body: { title: 'conformance: blocked issue' }, expect: [201] });
      const b = await t.call('issues/create', { params: s.rp, body: { title: 'conformance: blocking issue' }, expect: [201] });
      if (!a?.body?.number || !b?.body?.id) return;
      const A = P(s, { issue_number: a.body.number });
      const c = await t.call('issues/create-comment', { params: A, body: { body: 'pin me' }, expect: [201] });
      if (c?.body?.id) {
        await t.call('issues/pin-comment', { params: P(s, { comment_id: c.body.id }) });
        await t.call('issues/unpin-comment', { params: P(s, { comment_id: c.body.id }) });
      }
      await t.call('issues/add-blocked-by-dependency', { params: A, body: { issue_id: b.body.id }, expect: [201] });
      await t.call('issues/list-dependencies-blocked-by', { params: A, minItems: 1 });
      await t.call('issues/list-dependencies-blocking', { params: P(s, { issue_number: b.body.number }), minItems: 1 });
      await t.call('issues/remove-dependency-blocked-by', { params: { ...A, issue_id: b.body.id } });
      await t.call('issues/list-issue-field-values-for-issue', { params: A });
      await t.call('issues/check-user-can-be-assigned-to-issue', { params: { ...A, assignee: s.login }, expect: [204] });
      if (s.org) await t.call('issues/list-for-org', { params: { org: s.org }, query: { filter: 'all' } });
      for (const n of [a.body.number, b.body.number]) await t.call('issues/update', { params: P(s, { issue_number: n }), body: { state: 'closed' } });
    },
  },
  {
    name: 'gists: comments, stars, forks, revisions',
    requires: ['login'],
    run: async (t, s) => {
      const g = await t.call('gists/create', { body: { description: 'conformance gist (repos)', public: true, files: { 'a.txt': { content: 'one' } } }, expect: [201] });
      if (!g?.body?.id) return;
      const gid = { gist_id: g.body.id };
      await t.call('gists/update', { params: gid, body: { files: { 'a.txt': { content: 'two' } } } });
      const c = await t.call('gists/create-comment', { params: gid, body: { body: 'a gist comment' }, expect: [201] });
      await t.call('gists/list-comments', { params: gid, minItems: c?.body?.id ? 1 : 0 });
      if (c?.body?.id) {
        const cid = { ...gid, comment_id: c.body.id };
        await t.call('gists/get-comment', { params: cid });
        await t.call('gists/update-comment', { params: cid, body: { body: 'edited' } });
        await t.call('gists/delete-comment', { params: cid });
      }
      const commits = await t.call('gists/list-commits', { params: gid, minItems: 1 });
      const sha = commits?.body?.[0]?.version;
      if (sha) await t.call('gists/get-revision', { params: { ...gid, sha } });
      await t.call('gists/star', { params: gid });
      await t.call('gists/check-is-starred', { params: gid });
      await t.call('gists/list-starred', { minItems: 1 });
      await t.call('gists/unstar', { params: gid });
      await t.call('gists/list-public', { query: { per_page: 5 } });
      await t.call('gists/list-for-user', { params: { username: s.login }, minItems: 1 });
      if (collabToken(s)) {
        const f = await as(t, collabToken(s), () => t.call('gists/fork', { params: gid, expect: [201] }));
        await t.call('gists/list-forks', { params: gid, minItems: f?.body?.id ? 1 : 0 });
        if (f?.body?.id) await as(t, collabToken(s), () => t.call('gists/delete', { params: { gist_id: f.body.id } }));
      } else {
        await t.call('gists/list-forks', { params: gid });
      }
      await t.call('gists/delete', { params: gid });
    },
  },
  {
    name: 'activity: notifications, feeds, events, codes of conduct',
    requires: ['rp', 'login'],
    run: async (t, s) => {
      if (collabToken(s) && s.collaborator) {
        await t.call('repos/add-collaborator', { params: P(s, { username: s.collaborator }), body: { permission: 'push' } });
        const i = await t.call('issues/create', { params: s.rp, body: { title: 'conformance: notify me' }, expect: [201] });
        if (i?.body?.number) {
          await as(t, collabToken(s), () => t.call('issues/create-comment', { params: P(s, { issue_number: i.body.number }), body: { body: `ping @${s.login}` }, expect: [201] }));
          const n = await t.call('activity/list-notifications-for-authenticated-user', { minItems: 1 });
          await t.call('activity/list-repo-notifications-for-authenticated-user', { params: s.rp, minItems: 1 });
          const th = n?.body?.find((x) => x.repository?.full_name === `${s.rp.owner}/${s.rp.repo}`) || n?.body?.[0];
          if (th?.id) {
            const tid = { thread_id: Number(th.id) };
            await t.call('activity/get-thread', { params: tid });
            await t.call('activity/get-thread-subscription-for-authenticated-user', { params: tid });
            await t.call('activity/set-thread-subscription', { params: tid, body: { ignored: false } });
            await t.call('activity/delete-thread-subscription', { params: tid });
            await t.call('activity/mark-thread-as-read', { params: tid });
            await t.call('activity/mark-thread-as-done', { params: tid });
          }
          await t.call('issues/update', { params: P(s, { issue_number: i.body.number }), body: { state: 'closed' } });
        }
        await t.call('repos/remove-collaborator', { params: P(s, { username: s.collaborator }) });
      }
      await t.call('activity/mark-repo-notifications-as-read', { params: s.rp, body: {}, expect: [202, 205] });
      await t.call('activity/mark-notifications-as-read', { body: { read: true }, expect: [202, 205] });
      await t.call('activity/get-feeds');
      await t.call('activity/list-public-events', { query: { per_page: 5 } });
      await t.call('activity/list-public-events-for-repo-network', { params: s.rp });
      await t.call('activity/list-events-for-authenticated-user', { params: { username: s.login }, minItems: 1 });
      await t.call('activity/list-public-events-for-user', { params: { username: s.login } });
      await t.call('activity/list-received-events-for-user', { params: { username: s.login } });
      await t.call('activity/list-received-public-events-for-user', { params: { username: s.login } });
      await t.call('activity/list-repos-watched-by-user', { params: { username: s.login } });
      if (s.org) {
        await t.call('activity/list-public-org-events', { params: { org: s.org } });
        await t.call('activity/list-org-events-for-authenticated-user', { params: { username: s.login, org: s.org } });
      }
      await t.call('codes-of-conduct/get-all-codes-of-conduct', { minItems: 1 });
      await t.call('codes-of-conduct/get-conduct-code', { params: { key: 'contributor_covenant' } });
    },
  },
  {
    name: 'dependency graph: SBOM, compare, snapshots',
    requires: ['rp', 'def', 'baseSha'],
    run: async (t, s) => {
      await putFile(t, s, 'package.json', JSON.stringify({ name: 'conf', version: '1.0.0', dependencies: { 'left-pad': '^1.3.0' }, devDependencies: { jest: '^29.0.0' } }, null, 2));
      const branch = `conf-deps-${s.runId}`;
      const head = await t.probe('git/get-ref', { params: P(s, { ref: `heads/${s.def}` }) });
      await t.call('git/create-ref', { params: s.rp, body: { ref: `refs/heads/${branch}`, sha: head.body?.object?.sha || s.baseSha }, expect: [201] });
      await putFile(t, s, 'requirements.txt', 'requests==2.31.0\n', branch);
      await t.call('dependency-graph/export-sbom', { params: s.rp });
      const tip = await t.probe('git/get-ref', { params: P(s, { ref: `heads/${branch}` }) });
      await t.call('dependency-graph/create-repository-snapshot', {
        params: s.rp, expect: [201],
        body: {
          version: 0, sha: tip.body?.object?.sha || s.baseSha, ref: `refs/heads/${branch}`,
          job: { id: `conf-${s.runId}`, correlator: 'conformance' },
          detector: { name: 'conformance', version: '1.0.0', url: 'https://example.com/detector' },
          scanned: new Date().toISOString().replace(/\.\d+Z$/, 'Z'),
          manifests: { 'go.mod': { name: 'go.mod', file: { source_location: 'go.mod' }, resolved: { 'github.com/pkg/errors': { package_url: 'pkg:golang/github.com/pkg/errors@v0.9.1', relationship: 'direct', scope: 'runtime', dependencies: [] } } } },
        },
      });
      await t.call('dependency-graph/diff-range', { params: P(s, { basehead: `${s.def}...${branch}` }), minItems: 1 });
      await t.call('git/delete-ref', { params: P(s, { ref: `heads/${branch}` }) });
    },
  },
];
