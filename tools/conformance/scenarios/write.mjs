// Full read/write scenarios. They create their own fixtures in a fresh
// repository (`conf-<runId>` owned by the token's user), capture ids/numbers
// from responses, and delete everything at the end (unless --keep).
//
// Conventions:
//   t.call(opId, {params, query, body, expect, fixture})  -- validated call
//   fixture: true   => a failure throws Blocked and aborts the scenario
//   expect: [...]   => statuses the scenario considers correct (default: documented 2xx)
//   s               => shared state across scenarios (s.rp = {owner, repo})
// Scenarios run in order; `requires` lists state keys that must be present.
import { generateKeyPairSync } from 'node:crypto';
import { Blocked } from '../lib/oracle.mjs';

const b64 = (str) => Buffer.from(str, 'utf8').toString('base64');
const P = (s, extra = {}) => ({ ...s.rp, ...extra });

function sshPublicKey() {
  const { publicKey } = generateKeyPairSync('ed25519');
  const raw = publicKey.export({ format: 'der', type: 'spki' }).subarray(-32);
  const str = (b) => { const l = Buffer.alloc(4); l.writeUInt32BE(b.length); return Buffer.concat([l, b]); };
  return `ssh-ed25519 ${Buffer.concat([str(Buffer.from('ssh-ed25519')), str(raw)]).toString('base64')} conformance`;
}

export const scenarios = [
  {
    name: 'identity & meta',
    always: true,
    run: async (t, s) => {
      const me = await t.call('users/get-authenticated', { fixture: true });
      s.me = me.body;
      s.login = me.body.login;
      await t.call('users/get-by-username', { params: { username: s.login } });
      await t.call('meta/root');
      await t.call('meta/get');
      await t.call('rate-limit/get');
    },
  },
  {
    name: 'repository: create (fixture)',
    always: true,
    requires: ['login'],
    run: async (t, s) => {
      const name = `conf-${s.runId}`;
      const r = await t.call('repos/create-for-authenticated-user', {
        body: { name, description: 'forge conformance fixture', homepage: 'https://example.com', private: false,
          has_issues: true, has_projects: false, has_wiki: false, auto_init: true, license_template: 'mit', gitignore_template: 'Node' },
        expect: [201], fixture: true,
      });
      s.rp = { owner: r.body.owner.login, repo: r.body.name };
      s.repoId = r.body.id;
      s.def = r.body.default_branch || 'main';
      // auto_init may complete asynchronously
      const ref = await t.waitFor('git/get-ref', { params: P(s, { ref: `heads/${s.def}` }) }, (x) => x.status === 200);
      if (ref.status !== 200) {
        const repo = await t.probe('repos/get', { params: s.rp });
        s.def = repo.body?.default_branch || s.def;
      }
      const got = await t.call('git/get-ref', { params: P(s, { ref: `heads/${s.def}` }) });
      s.baseSha = got?.body?.object?.sha;
      if (!s.baseSha) { // fallback so that servers without the refs API can still be measured
        const br = await t.probe('repos/get-branch', { params: P(s, { branch: s.def }) });
        s.baseSha = br.body?.commit?.sha || br.body?.commit?.id;
      }
      if (!s.baseSha) throw new Blocked('could not determine the head commit of the default branch');
      await t.call('repos/get', { params: s.rp, fixture: true });
    },
  },
  {
    name: 'repository: metadata',
    requires: ['rp'],
    run: async (t, s) => {
      await t.call('repos/update', { params: s.rp, body: { description: 'forge conformance fixture (updated)', has_wiki: false } });
      await t.call('repos/list-for-authenticated-user', { query: { per_page: 100, sort: 'created', direction: 'desc' }, minItems: 1 });
      await t.call('repos/list-for-user', { params: { username: s.login }, query: { per_page: 10 }, minItems: 1 });
      await t.call('repos/replace-all-topics', { params: s.rp, body: { names: ['conformance', 'forge'] } });
      await t.call('repos/get-all-topics', { params: s.rp });
      await t.call('repos/list-languages', { params: s.rp });
      await t.call('repos/list-contributors', { params: s.rp });
      await t.call('licenses/get-for-repo', { params: s.rp });
      await t.call('repos/list-tags', { params: s.rp });
      await t.call('repos/list-forks', { params: s.rp });
    },
  },
  {
    name: 'contents API',
    requires: ['rp'],
    run: async (t, s) => {
      await t.call('repos/get-readme', { params: s.rp });
      await t.call('repos/get-content', { params: P(s, { path: 'README.md' }) });
      const c = await t.call('repos/create-or-update-file-contents', {
        params: P(s, { path: 'docs/hello.md' }),
        body: { message: 'docs: add hello', content: b64('# hello\n'), branch: s.def }, expect: [201], fixture: true,
      });
      const up = await t.call('repos/create-or-update-file-contents', {
        params: P(s, { path: 'docs/hello.md' }),
        body: { message: 'docs: update hello', content: b64('# hello again\n'), sha: c.body.content.sha, branch: s.def }, expect: [200],
      });
      await t.call('repos/get-content', { params: P(s, { path: 'docs' }) }); // directory listing (array)
      await t.call('repos/get-content', { params: P(s, { path: 'docs/hello.md' }), query: { ref: s.def } });
      await t.call('repos/create-or-update-file-contents', {
        params: P(s, { path: 'src/app.txt' }), body: { message: 'add app', content: b64('app\n') }, expect: [201],
      });
      await t.call('repos/delete-file', {
        params: P(s, { path: 'docs/hello.md' }),
        body: { message: 'docs: remove hello', sha: up?.body?.content?.sha || c.body.content.sha },
      });
    },
  },
  {
    name: 'git data & refs',
    requires: ['rp', 'baseSha'],
    run: async (t, s) => {
      await t.call('git/get-ref', { params: P(s, { ref: `heads/${s.def}` }) });
      await t.call('git/list-matching-refs', { params: P(s, { ref: 'heads' }) });
      // Build the `feature` branch (2 commits ahead of base) through the git data API ...
      const ok = (r, code) => r && r.status === code && r.body;
      const commit = await t.call('git/get-commit', { params: P(s, { commit_sha: s.baseSha }) });
      const baseTree = commit?.body?.tree?.sha;
      if (baseTree) await t.call('git/get-tree', { params: P(s, { tree_sha: baseTree }), query: { recursive: 1 } });
      const blob = await t.call('git/create-blob', { params: s.rp, body: { content: 'feature line 1\nfeature line 2\n', encoding: 'utf-8' }, expect: [201] });
      if (ok(blob, 201)) await t.call('git/get-blob', { params: P(s, { file_sha: blob.body.sha }) });
      const tree = baseTree && ok(blob, 201) && await t.call('git/create-tree', {
        params: s.rp, expect: [201],
        body: { base_tree: baseTree, tree: [{ path: 'feature.txt', mode: '100644', type: 'blob', sha: blob.body.sha }] },
      });
      const c1 = ok(tree, 201) && await t.call('git/create-commit', {
        params: s.rp, expect: [201],
        body: { message: 'feature: add feature.txt', tree: tree.body.sha, parents: [s.baseSha],
          author: { name: 'Conformance Bot', email: 'conformance@example.com', date: '2024-01-01T00:00:00Z' } },
      });
      const ref = ok(c1, 201) && await t.call('git/create-ref', { params: s.rp, body: { ref: 'refs/heads/feature', sha: c1.body.sha }, expect: [201] });
      const tree2 = ok(ref, 201) && await t.call('git/create-tree', {
        params: s.rp, expect: [201],
        body: { base_tree: tree.body.sha, tree: [{ path: 'feature2.txt', mode: '100644', type: 'blob', content: 'second\n' }] },
      });
      const c2 = ok(tree2, 201) && await t.call('git/create-commit', { params: s.rp, body: { message: 'feature: add feature2.txt', tree: tree2.body.sha, parents: [c1.body.sha] }, expect: [201] });
      const upd = ok(c2, 201) && await t.call('git/update-ref', { params: P(s, { ref: 'heads/feature' }), body: { sha: c2.body.sha, force: false } });
      if (ok(upd, 200)) s.featureSha = c2.body.sha;
      // ... or, if the git data API is missing, via refs + contents API (still GitHub-compatible calls).
      if (!s.featureSha) {
        console.log('   (git data API unavailable; building feature branch via the contents API)');
        if (!ok(ref, 201)) await t.call('git/create-ref', { params: s.rp, body: { ref: 'refs/heads/feature', sha: s.baseSha }, expect: [201], fixture: true });
        let last;
        for (const [f, txt] of [['feature.txt', 'feature line 1\nfeature line 2\n'], ['feature2.txt', 'second\n']]) {
          last = await t.call('repos/create-or-update-file-contents', { params: P(s, { path: f }), body: { message: `feature: add ${f}`, content: b64(txt), branch: 'feature' }, expect: [201], fixture: true });
        }
        s.featureSha = last.body.commit.sha;
      }
      // throwaway ref
      await t.call('git/create-ref', { params: s.rp, body: { ref: 'refs/heads/tmp-delete-me', sha: s.baseSha }, expect: [201] });
      await t.call('git/delete-ref', { params: P(s, { ref: 'heads/tmp-delete-me' }) });
      // annotated tag
      const tag = await t.call('git/create-tag', {
        params: s.rp, expect: [201],
        body: { tag: 'v0.1.0', message: 'conformance tag', object: s.baseSha, type: 'commit',
          tagger: { name: 'Conformance Bot', email: 'conformance@example.com', date: '2024-01-01T00:00:00Z' } },
      });
      if (tag?.body?.sha) {
        await t.call('git/get-tag', { params: P(s, { tag_sha: tag.body.sha }) });
        await t.call('git/create-ref', { params: s.rp, body: { ref: 'refs/tags/v0.1.0', sha: tag.body.sha }, expect: [201] });
      }
    },
  },
  {
    name: 'branches, merges, protection, archives',
    requires: ['rp', 'featureSha'],
    run: async (t, s) => {
      await t.call('repos/list-branches', { params: s.rp, minItems: 2 });
      await t.call('repos/get-branch', { params: P(s, { branch: 'feature' }) });
      await t.call('git/create-ref', { params: s.rp, body: { ref: 'refs/heads/to-rename', sha: s.baseSha }, expect: [201], fixture: true });
      await t.call('repos/rename-branch', { params: P(s, { branch: 'to-rename' }), body: { new_name: 'renamed' } });
      await t.call('repos/merge', { params: s.rp, body: { base: 'renamed', head: 'feature', commit_message: 'merge feature into renamed' }, expect: [201, 204] });
      await t.call('repos/update-branch-protection', {
        params: P(s, { branch: 'renamed' }),
        body: { required_status_checks: null, enforce_admins: false, required_pull_request_reviews: null, restrictions: null },
      });
      await t.call('repos/get-branch-protection', { params: P(s, { branch: 'renamed' }) });
      await t.call('repos/delete-branch-protection', { params: P(s, { branch: 'renamed' }) });
      await t.call('repos/download-tarball-archive', { params: P(s, { ref: s.def }) });
      await t.call('repos/download-zipball-archive', { params: P(s, { ref: s.def }) });
    },
  },
  {
    name: 'commits, compare, statuses, checks, commit comments',
    requires: ['rp', 'featureSha'],
    run: async (t, s) => {
      await t.call('repos/list-commits', { params: s.rp, query: { sha: 'feature' }, minItems: 3 });
      await t.checkPagination('repos/list-commits', { params: s.rp, query: { sha: 'feature' } });
      await t.call('repos/get-commit', { params: P(s, { ref: s.featureSha }) });
      await t.call('repos/compare-commits', { params: P(s, { basehead: `${s.def}...feature` }) });
      await t.call('repos/list-branches-for-head-commit', { params: P(s, { commit_sha: s.featureSha }) });
      const st = await t.call('repos/create-commit-status', {
        params: P(s, { sha: s.featureSha }), expect: [201],
        body: { state: 'success', target_url: 'https://example.com/ci/1', description: 'conformance', context: 'conformance/ci' },
      });
      await t.call('repos/list-commit-statuses-for-ref', { params: P(s, { ref: 'feature' }), minItems: st?.status === 201 ? 1 : 0 });
      await t.call('repos/get-combined-status-for-ref', { params: P(s, { ref: 'feature' }) });
      await t.call('checks/list-for-ref', { params: P(s, { ref: 'feature' }) });
      await t.call('checks/list-suites-for-ref', { params: P(s, { ref: 'feature' }) });
      const cc = await t.call('repos/create-commit-comment', { params: P(s, { commit_sha: s.featureSha }), body: { body: 'commit comment', path: 'feature2.txt', position: 1 }, expect: [201] });
      await t.call('repos/list-comments-for-commit', { params: P(s, { commit_sha: s.featureSha }), minItems: cc?.status === 201 ? 1 : 0 });
      await t.call('repos/list-commit-comments-for-repo', { params: s.rp, minItems: cc?.status === 201 ? 1 : 0 });
      if (cc?.body?.id) {
        const id = cc.body.id;
        await t.call('repos/get-commit-comment', { params: P(s, { comment_id: id }) });
        await t.call('repos/update-commit-comment', { params: P(s, { comment_id: id }), body: { body: 'commit comment (edited)' } });
        const r = await t.call('reactions/create-for-commit-comment', { params: P(s, { comment_id: id }), body: { content: '+1' }, expect: [200, 201] });
        await t.call('reactions/list-for-commit-comment', { params: P(s, { comment_id: id }), minItems: r?.body?.id ? 1 : 0 });
        if (r?.body?.id) await t.call('reactions/delete-for-commit-comment', { params: P(s, { comment_id: id, reaction_id: r.body.id }) });
        await t.call('repos/delete-commit-comment', { params: P(s, { comment_id: id }) });
      }
    },
  },
  {
    name: 'labels & milestones',
    requires: ['rp'],
    run: async (t, s) => {
      await t.call('issues/create-label', { params: s.rp, body: { name: 'conf-bug', color: 'd73a4a', description: 'conformance bug label' }, expect: [201], fixture: true });
      await t.call('issues/create-label', { params: s.rp, body: { name: 'conf-triage', color: 'fbca04' }, expect: [201] });
      await t.call('issues/create-label', { params: s.rp, body: { name: 'conf-temp', color: 'ededed' }, expect: [201] });
      await t.call('issues/list-labels-for-repo', { params: s.rp, minItems: 2 });
      await t.checkPagination('issues/list-labels-for-repo', { params: s.rp });
      await t.call('issues/get-label', { params: P(s, { name: 'conf-bug' }) });
      const u = await t.call('issues/update-label', { params: P(s, { name: 'conf-bug' }), body: { new_name: 'conf-bug-renamed', color: 'b60205', description: 'renamed' } });
      s.label = u?.status === 200 ? 'conf-bug-renamed' : 'conf-bug';
      await t.call('issues/delete-label', { params: P(s, { name: 'conf-temp' }) });
      const m = await t.call('issues/create-milestone', { params: s.rp, body: { title: 'v1.0', state: 'open', description: 'first', due_on: '2030-01-01T00:00:00Z' }, expect: [201], fixture: true });
      s.milestone = m.body.number ?? m.body.id;
      await t.call('issues/list-milestones', { params: s.rp, minItems: 1 });
      await t.call('issues/get-milestone', { params: P(s, { milestone_number: s.milestone }) });
      await t.call('issues/update-milestone', { params: P(s, { milestone_number: s.milestone }), body: { description: 'first (edited)' } });
      const m2 = await t.call('issues/create-milestone', { params: s.rp, body: { title: 'throwaway' }, expect: [201] });
      if (m2?.body?.number) await t.call('issues/delete-milestone', { params: P(s, { milestone_number: m2.body.number }) });
    },
  },
  {
    name: 'issues',
    requires: ['rp', 'login', 'label', 'milestone'],
    run: async (t, s) => {
      let i1 = await t.call('issues/create', {
        params: s.rp, expect: [201],
        body: { title: 'Conformance issue', body: 'Body with **markdown**', labels: [s.label], milestone: s.milestone, assignees: [s.login] },
      });
      if (i1.status !== 201) i1 = await t.call('issues/create', { params: s.rp, body: { title: 'Conformance issue', body: 'minimal' }, expect: [201], fixture: true, note: 'fallback: minimal body' });
      s.issue = i1.body.number;
      const i2 = await t.call('issues/create', { params: s.rp, body: { title: 'Second issue' }, expect: [201], fixture: true });
      s.issue2 = i2.body.number;
      await t.call('issues/list-for-repo', { params: s.rp, minItems: 1 });
      await t.checkPagination('issues/list-for-repo', { params: s.rp });
      await t.call('issues/get', { params: P(s, { issue_number: s.issue }) });
      await t.call('issues/update', { params: P(s, { issue_number: s.issue }), body: { title: 'Conformance issue (edited)', body: 'edited' } });
      await t.call('issues/update', { params: P(s, { issue_number: s.issue2 }), body: { state: 'closed', state_reason: 'completed' } });
      await t.call('issues/list', { query: { filter: 'all', state: 'all' } });
      await t.call('issues/list-for-authenticated-user', { query: { filter: 'all' } });
      await t.call('issues/list-assignees', { params: s.rp });
      await t.call('issues/check-user-can-be-assigned', { params: P(s, { assignee: s.login }) });
      await t.call('issues/remove-assignees', { params: P(s, { issue_number: s.issue }), body: { assignees: [s.login] } });
      await t.call('issues/add-assignees', { params: P(s, { issue_number: s.issue }), body: { assignees: [s.login] } });
      await t.call('issues/list-labels-for-milestone', { params: P(s, { milestone_number: s.milestone }) });
      await t.call('issues/list-labels-on-issue', { params: P(s, { issue_number: s.issue }), minItems: i1.body.labels?.length ? 1 : 0 });
      await t.call('issues/add-labels', { params: P(s, { issue_number: s.issue }), body: { labels: ['conf-triage'] } });
      await t.call('issues/remove-label', { params: P(s, { issue_number: s.issue, name: 'conf-triage' }) });
      await t.call('issues/set-labels', { params: P(s, { issue_number: s.issue }), body: { labels: [s.label, 'conf-triage'] } });
      await t.call('issues/remove-all-labels', { params: P(s, { issue_number: s.issue }) });
      await t.call('issues/add-labels', { params: P(s, { issue_number: s.issue }), body: { labels: [s.label] } });
      await t.call('issues/lock', { params: P(s, { issue_number: s.issue2 }), body: { lock_reason: 'resolved' } });
      await t.call('issues/unlock', { params: P(s, { issue_number: s.issue2 }) });
      await t.call('issues/list-events', { params: P(s, { issue_number: s.issue }) });
      const ev = await t.call('issues/list-events-for-repo', { params: s.rp });
      if (ev?.body?.[0]?.id) await t.call('issues/get-event', { params: P(s, { event_id: ev.body[0].id }) });
      await t.call('issues/list-events-for-timeline', { params: P(s, { issue_number: s.issue }), minItems: 1 });
    },
  },
  {
    name: 'issue comments & reactions',
    requires: ['rp', 'issue'],
    run: async (t, s) => {
      const c1 = await t.call('issues/create-comment', { params: P(s, { issue_number: s.issue }), body: { body: 'first comment' }, expect: [201], fixture: true });
      const c2 = await t.call('issues/create-comment', { params: P(s, { issue_number: s.issue }), body: { body: 'second comment' }, expect: [201] });
      await t.call('issues/list-comments', { params: P(s, { issue_number: s.issue }), minItems: 1 });
      await t.checkPagination('issues/list-comments', { params: P(s, { issue_number: s.issue }) });
      await t.call('issues/list-comments-for-repo', { params: s.rp, minItems: 1 });
      await t.call('issues/get-comment', { params: P(s, { comment_id: c1.body.id }) });
      await t.call('issues/update-comment', { params: P(s, { comment_id: c1.body.id }), body: { body: 'first comment (edited)' } });
      const r1 = await t.call('reactions/create-for-issue', { params: P(s, { issue_number: s.issue }), body: { content: 'heart' }, expect: [200, 201] });
      await t.call('reactions/list-for-issue', { params: P(s, { issue_number: s.issue }), minItems: r1?.body?.id ? 1 : 0 });
      if (r1?.body?.id) await t.call('reactions/delete-for-issue', { params: P(s, { issue_number: s.issue, reaction_id: r1.body.id }) });
      const r2 = await t.call('reactions/create-for-issue-comment', { params: P(s, { comment_id: c1.body.id }), body: { content: 'rocket' }, expect: [200, 201] });
      await t.call('reactions/list-for-issue-comment', { params: P(s, { comment_id: c1.body.id }), minItems: r2?.body?.id ? 1 : 0 });
      if (r2?.body?.id) await t.call('reactions/delete-for-issue-comment', { params: P(s, { comment_id: c1.body.id, reaction_id: r2.body.id }) });
      if (c2?.body?.id) await t.call('issues/delete-comment', { params: P(s, { comment_id: c2.body.id }) });
    },
  },
  {
    name: 'pull requests: create, review comments, reviews',
    requires: ['rp', 'featureSha'],
    run: async (t, s) => {
      const pr = await t.call('pulls/create', {
        params: s.rp, expect: [201], fixture: true,
        body: { title: 'Add feature files', head: 'feature', base: s.def, body: 'PR body', draft: false },
      });
      s.pr = pr.body.number;
      const headSha = pr.body.head.sha;
      await t.call('pulls/list', { params: s.rp, minItems: 1 });
      await t.call('pulls/get', { params: P(s, { pull_number: s.pr }) });
      await t.call('pulls/update', { params: P(s, { pull_number: s.pr }), body: { body: 'PR body (edited)' } });
      await t.call('pulls/list-commits', { params: P(s, { pull_number: s.pr }), minItems: 2 });
      await t.call('pulls/list-files', { params: P(s, { pull_number: s.pr }), minItems: 2 });
      await t.call('pulls/check-if-merged', { params: P(s, { pull_number: s.pr }), expect: [404], note: 'not merged yet' });
      await t.call('pulls/list-requested-reviewers', { params: P(s, { pull_number: s.pr }) });
      await t.call('repos/list-pull-requests-associated-with-commit', { params: P(s, { commit_sha: headSha }) });
      if (s.collaborator) {
        await t.call('pulls/request-reviewers', { params: P(s, { pull_number: s.pr }), body: { reviewers: [s.collaborator] }, expect: [201] });
        await t.call('pulls/remove-requested-reviewers', { params: P(s, { pull_number: s.pr }), body: { reviewers: [s.collaborator] } });
      }
      // review comments
      const rc = await t.call('pulls/create-review-comment', {
        params: P(s, { pull_number: s.pr }), expect: [201],
        body: { body: 'line comment', commit_id: headSha, path: 'feature.txt', line: 1, side: 'RIGHT' },
      });
      await t.call('pulls/list-review-comments', { params: P(s, { pull_number: s.pr }), minItems: rc?.body?.id ? 1 : 0 });
      await t.call('pulls/list-review-comments-for-repo', { params: s.rp, minItems: rc?.body?.id ? 1 : 0 });
      if (rc?.body?.id) {
        const id = rc.body.id;
        await t.call('pulls/get-review-comment', { params: P(s, { comment_id: id }) });
        await t.call('pulls/update-review-comment', { params: P(s, { comment_id: id }), body: { body: 'line comment (edited)' } });
        const reply = await t.call('pulls/create-reply-for-review-comment', { params: P(s, { pull_number: s.pr, comment_id: id }), body: { body: 'a reply' }, expect: [201] });
        const re = await t.call('reactions/create-for-pull-request-review-comment', { params: P(s, { comment_id: id }), body: { content: 'eyes' }, expect: [200, 201] });
        await t.call('reactions/list-for-pull-request-review-comment', { params: P(s, { comment_id: id }), minItems: re?.body?.id ? 1 : 0 });
        if (re?.body?.id) await t.call('reactions/delete-for-pull-request-comment', { params: P(s, { comment_id: id, reaction_id: re.body.id }) });
        if (reply?.body?.id) await t.call('pulls/delete-review-comment', { params: P(s, { comment_id: reply.body.id }) });
      }
      // reviews (the author cannot APPROVE their own PR, so use COMMENT)
      const rv = await t.call('pulls/create-review', {
        params: P(s, { pull_number: s.pr }),
        body: { commit_id: headSha, body: 'Review summary', event: 'COMMENT', comments: [{ path: 'feature2.txt', line: 1, body: 'nit' }] },
      });
      await t.call('pulls/list-reviews', { params: P(s, { pull_number: s.pr }), minItems: rv?.body?.id ? 1 : 0 });
      if (rv?.body?.id) {
        await t.call('pulls/get-review', { params: P(s, { pull_number: s.pr, review_id: rv.body.id }) });
        await t.call('pulls/update-review', { params: P(s, { pull_number: s.pr, review_id: rv.body.id }), body: { body: 'Review summary (edited)' } });
        await t.call('pulls/list-comments-for-review', { params: P(s, { pull_number: s.pr, review_id: rv.body.id }) });
      }
      const pending = await t.call('pulls/create-review', { params: P(s, { pull_number: s.pr }), body: { body: 'pending review' } });
      if (pending?.body?.id) await t.call('pulls/delete-pending-review', { params: P(s, { pull_number: s.pr, review_id: pending.body.id }) });
      const pending2 = await t.call('pulls/create-review', { params: P(s, { pull_number: s.pr }), body: { body: 'to be submitted' } });
      if (pending2?.body?.id) await t.call('pulls/submit-review', { params: P(s, { pull_number: s.pr, review_id: pending2.body.id }), body: { event: 'COMMENT', body: 'submitted' } });
    },
  },
  {
    name: 'pull requests: update-branch & close',
    requires: ['rp', 'baseSha'],
    run: async (t, s) => {
      // a branch based on the initial commit is behind the default branch (contents scenario added commits)
      await t.call('git/create-ref', { params: s.rp, body: { ref: 'refs/heads/behind', sha: s.baseSha }, expect: [201], fixture: true });
      await t.call('repos/create-or-update-file-contents', { params: P(s, { path: 'behind.txt' }), body: { message: 'behind change', content: b64('behind\n'), branch: 'behind' }, expect: [201], fixture: true });
      const pr = await t.call('pulls/create', { params: s.rp, body: { title: 'Behind PR', head: 'behind', base: s.def }, expect: [201], fixture: true });
      await t.call('pulls/update-branch', { params: P(s, { pull_number: pr.body.number }), body: { expected_head_sha: pr.body.head.sha } });
      await t.call('pulls/update', { params: P(s, { pull_number: pr.body.number }), body: { state: 'closed' } });
    },
  },
  {
    name: 'pull requests: merge',
    requires: ['rp', 'pr'],
    run: async (t, s) => {
      await t.waitFor('pulls/get', { params: P(s, { pull_number: s.pr }) }, (x) => x.body?.mergeable !== null && x.body?.mergeable !== undefined);
      await t.call('pulls/merge', { params: P(s, { pull_number: s.pr }), body: { commit_title: 'Merge feature', merge_method: 'merge' } });
      await t.call('pulls/check-if-merged', { params: P(s, { pull_number: s.pr }), expect: [204] });
    },
  },
  {
    name: 'releases & assets',
    requires: ['rp'],
    run: async (t, s) => {
      const rel = await t.call('repos/create-release', {
        params: s.rp, expect: [201], fixture: true,
        body: { tag_name: 'v1.0.0', target_commitish: s.def, name: 'v1.0.0', body: 'First release', draft: false, prerelease: false },
      });
      const id = rel.body.id;
      await t.call('repos/list-releases', { params: s.rp, minItems: 1 });
      await t.call('repos/get-release', { params: P(s, { release_id: id }) });
      await t.call('repos/get-latest-release', { params: s.rp });
      await t.call('repos/get-release-by-tag', { params: P(s, { tag: 'v1.0.0' }) });
      await t.call('repos/update-release', { params: P(s, { release_id: id }), body: { body: 'First release (edited)' } });
      await t.call('repos/generate-release-notes', { params: s.rp, body: { tag_name: 'v1.1.0', target_commitish: s.def } });
      const up = await t.call('repos/upload-release-asset', {
        params: P(s, { release_id: id }), url: rel.body.upload_url, query: { name: 'asset.txt', label: 'An asset' },
        rawBody: Buffer.from('hello asset\n'), contentType: 'text/plain', expect: [201],
      });
      await t.call('repos/list-release-assets', { params: P(s, { release_id: id }), minItems: up?.body?.id ? 1 : 0 });
      if (up?.body?.id) {
        await t.call('repos/get-release-asset', { params: P(s, { asset_id: up.body.id }) });
        await t.call('repos/update-release-asset', { params: P(s, { asset_id: up.body.id }), body: { name: 'asset-renamed.txt', label: 'Renamed' } });
        await t.call('repos/delete-release-asset', { params: P(s, { asset_id: up.body.id }) });
      }
      const r = await t.call('reactions/create-for-release', { params: P(s, { release_id: id }), body: { content: 'hooray' }, expect: [200, 201] });
      await t.call('reactions/list-for-release', { params: P(s, { release_id: id }), minItems: r?.body?.id ? 1 : 0 });
      if (r?.body?.id) await t.call('reactions/delete-for-release', { params: P(s, { release_id: id, reaction_id: r.body.id }) });
      await t.call('repos/delete-release', { params: P(s, { release_id: id }) });
    },
  },
  {
    name: 'deployments & webhooks',
    requires: ['rp'],
    run: async (t, s) => {
      const d = await t.call('repos/create-deployment', {
        params: s.rp, expect: [201],
        body: { ref: s.def, environment: 'conformance', auto_merge: false, required_contexts: [], description: 'conformance deploy' },
      });
      await t.call('repos/list-deployments', { params: s.rp, minItems: d?.body?.id ? 1 : 0 });
      if (d?.body?.id) {
        const did = d.body.id;
        await t.call('repos/get-deployment', { params: P(s, { deployment_id: did }) });
        await t.call('repos/create-deployment-status', { params: P(s, { deployment_id: did }), body: { state: 'success', description: 'deployed', environment_url: 'https://example.com' }, expect: [201] });
        await t.call('repos/list-deployment-statuses', { params: P(s, { deployment_id: did }), minItems: 1 });
        await t.call('repos/create-deployment-status', { params: P(s, { deployment_id: did }), body: { state: 'inactive' }, expect: [201] });
        await t.call('repos/delete-deployment', { params: P(s, { deployment_id: did }) });
      }
      const h = await t.call('repos/create-webhook', {
        params: s.rp, expect: [201],
        body: { name: 'web', active: true, events: ['push', 'pull_request'], config: { url: 'https://example.com/conformance-hook', content_type: 'json', insecure_ssl: '0' } },
      });
      await t.call('repos/list-webhooks', { params: s.rp, minItems: h?.body?.id ? 1 : 0 });
      if (h?.body?.id) {
        await t.call('repos/get-webhook', { params: P(s, { hook_id: h.body.id }) });
        await t.call('repos/update-webhook', { params: P(s, { hook_id: h.body.id }), body: { active: false, events: ['push'] } });
        await t.call('repos/ping-webhook', { params: P(s, { hook_id: h.body.id }) });
        await t.call('repos/delete-webhook', { params: P(s, { hook_id: h.body.id }) });
      }
    },
  },
  {
    name: 'stars, watching, notifications, events',
    requires: ['rp', 'login'],
    run: async (t, s) => {
      const star = await t.call('activity/star-repo-for-authenticated-user', { params: s.rp });
      const n = star?.status === 204 ? 1 : 0;
      await t.call('activity/check-repo-is-starred-by-authenticated-user', { params: s.rp });
      await t.call('activity/list-repos-starred-by-authenticated-user', { minItems: n });
      await t.call('activity/list-repos-starred-by-user', { params: { username: s.login }, minItems: n });
      await t.call('activity/list-stargazers-for-repo', { params: s.rp, minItems: n });
      await t.call('activity/unstar-repo-for-authenticated-user', { params: s.rp });
      await t.call('activity/check-repo-is-starred-by-authenticated-user', { params: s.rp, expect: [404], note: 'after unstar' });
      const sub = await t.call('activity/set-repo-subscription', { params: s.rp, body: { subscribed: true, ignored: false } });
      const w = sub?.status === 200 ? 1 : 0;
      await t.call('activity/get-repo-subscription', { params: s.rp });
      await t.call('activity/list-watchers-for-repo', { params: s.rp, minItems: w });
      await t.call('activity/list-watched-repos-for-authenticated-user', { minItems: w });
      await t.call('activity/delete-repo-subscription', { params: s.rp });
      await t.call('activity/list-notifications-for-authenticated-user', { query: { all: true } });
      await t.call('activity/list-repo-events', { params: s.rp });
    },
  },
  {
    name: 'collaborators',
    requires: ['rp', 'login'],
    run: async (t, s) => {
      await t.call('repos/list-collaborators', { params: s.rp, minItems: 1 }); // the owner is listed
      await t.call('repos/check-collaborator', { params: P(s, { username: s.login }) });
      await t.call('repos/get-collaborator-permission-level', { params: P(s, { username: s.login }) });
      await t.call('repos/list-invitations', { params: s.rp });
      if (s.collaborator) {
        await t.call('repos/add-collaborator', { params: P(s, { username: s.collaborator }), body: { permission: 'push' } });
        await t.call('repos/remove-collaborator', { params: P(s, { username: s.collaborator }) });
      }
    },
  },
  {
    name: 'template & fork',
    requires: ['rp', 'login'],
    run: async (t, s) => {
      await t.call('repos/update', { params: s.rp, body: { is_template: true } });
      const gen = await t.call('repos/create-using-template', {
        params: { template_owner: s.rp.owner, template_repo: s.rp.repo },
        body: { owner: s.login, name: `${s.rp.repo}-gen`, description: 'generated from template', private: false }, expect: [201],
      });
      if (gen?.body?.name) (s.cleanup ||= []).push({ owner: gen.body.owner.login, repo: gen.body.name });
      if (s.org) {
        const fork = await t.call('repos/create-fork', { params: s.rp, body: { organization: s.org, name: `${s.rp.repo}-fork`, default_branch_only: true } });
        if (fork?.body?.name) {
          (s.cleanup ||= []).push({ owner: fork.body.owner.login, repo: fork.body.name });
          await t.waitFor('repos/get', { params: { owner: fork.body.owner.login, repo: fork.body.name } }, (x) => x.status === 200);
          await t.call('repos/merge-upstream', { params: { owner: fork.body.owner.login, repo: fork.body.name }, body: { branch: s.def } });
        }
        await t.call('repos/list-forks', { params: s.rp });
      }
    },
  },
  {
    name: 'users & ssh keys',
    requires: ['login'],
    run: async (t, s) => {
      await t.call('users/list', { query: { per_page: 2 } });
      await t.call('users/update-authenticated', { body: { bio: s.me?.bio ?? '' } }); // no-op update
      await t.call('users/list-emails-for-authenticated-user');
      await t.call('users/list-followers-for-user', { params: { username: s.login } });
      await t.call('users/list-following-for-user', { params: { username: s.login } });
      await t.call('users/list-public-keys-for-user', { params: { username: s.login } });
      await t.call('users/list-public-ssh-keys-for-authenticated-user');
      await t.call('users/list-gpg-keys-for-authenticated-user');
      const k = await t.call('users/create-public-ssh-key-for-authenticated-user', { body: { title: `conformance-${s.runId}`, key: sshPublicKey() }, expect: [201] });
      if (k?.body?.id) {
        await t.call('users/get-public-ssh-key-for-authenticated-user', { params: { key_id: k.body.id } });
        await t.call('users/delete-public-ssh-key-for-authenticated-user', { params: { key_id: k.body.id } });
      }
    },
  },
  {
    name: 'organizations',
    requires: ['login'],
    run: async (t, s) => {
      await t.call('orgs/list-for-authenticated-user');
      await t.call('orgs/list-for-user', { params: { username: s.login } });
      await t.call('orgs/list', { query: { per_page: 5 } });
      if (!s.org) return;
      const org = { org: s.org };
      await t.call('orgs/get', { params: org });
      await t.call('orgs/list-members', { params: org });
      await t.call('orgs/check-membership-for-user', { params: { ...org, username: s.login }, expect: [204] });
      await t.call('orgs/get-membership-for-authenticated-user', { params: org });
      await t.call('teams/list', { params: org });
      await t.call('repos/list-for-org', { params: org });
      const r = await t.call('repos/create-in-org', { params: org, body: { name: `conf-${s.runId}-org`, private: false, auto_init: true }, expect: [201] });
      if (r?.body?.name) (s.cleanup ||= []).push({ owner: s.org, repo: r.body.name });
    },
  },
  {
    name: 'gists',
    requires: ['login'],
    run: async (t, s) => {
      const g = await t.call('gists/create', { body: { description: 'conformance gist', public: false, files: { 'conf.txt': { content: 'hello gist' } } }, expect: [201] });
      await t.call('gists/list', { minItems: g?.body?.id ? 1 : 0 });
      if (g?.body?.id) {
        await t.call('gists/get', { params: { gist_id: g.body.id } });
        await t.call('gists/update', { params: { gist_id: g.body.id }, body: { description: 'conformance gist (edited)', files: { 'conf.txt': { content: 'hello again' } } } });
        await t.call('gists/delete', { params: { gist_id: g.body.id } });
      }
    },
  },
  {
    name: 'actions (read)',
    requires: ['rp'],
    run: async (t, s) => {
      await t.call('actions/list-repo-workflows', { params: s.rp });
      const runs = await t.call('actions/list-workflow-runs-for-repo', { params: s.rp });
      const run = runs?.body?.workflow_runs?.[0];
      if (run) {
        await t.call('actions/get-workflow-run', { params: P(s, { run_id: run.id }) });
        await t.call('actions/list-jobs-for-workflow-run', { params: P(s, { run_id: run.id }) });
      }
    },
  },
  {
    name: 'search',
    requires: ['rp', 'login'],
    run: async (t, s) => {
      // Results may lag behind indexing; only the response shape is validated.
      const repoQ = `repo:${s.rp.owner}/${s.rp.repo}`;
      await t.call('search/repos', { query: { q: `${s.rp.repo} in:name`, per_page: 5 } });
      await t.call('search/issues-and-pull-requests', { query: { q: `${repoQ} is:issue`, per_page: 5 } });
      await t.call('search/code', { query: { q: `feature ${repoQ}`, per_page: 5 } });
      await t.call('search/commits', { query: { q: `${repoQ} feature`, per_page: 5 } });
      await t.call('search/users', { query: { q: s.login, per_page: 5 } });
      if (s.repoId) await t.call('search/labels', { query: { repository_id: s.repoId, q: 'conf' } });
      await t.call('search/topics', { query: { q: 'conformance', per_page: 5 } });
    },
  },
  {
    name: 'misc endpoints',
    run: async (t, s) => {
      await t.call('markdown/render', { body: { text: 'Hello **world** #1', mode: 'gfm', ...(s.rp ? { context: `${s.rp.owner}/${s.rp.repo}` } : {}) } });
      await t.call('emojis/get');
      await t.call('licenses/get-all-commonly-used');
      await t.call('gitignore/get-all-templates');
      await t.call('meta/get-octocat');
      await t.call('meta/get-zen');
    },
  },
  {
    name: 'cleanup',
    always: true,
    requires: ['rp'],
    run: async (t, s) => {
      if (s.keep) { console.log(`   keeping fixture repo ${s.rp.owner}/${s.rp.repo}`); return; }
      for (const r of s.cleanup || []) await t.call('repos/delete', { params: r });
      await t.call('repos/delete', { params: s.rp });
    },
  },
];

