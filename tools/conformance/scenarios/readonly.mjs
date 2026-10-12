// Read-only scenarios against an existing public repository (default cli/cli).
// Used for the self-test against api.github.com (calibrates the validator) and
// usable against any server that already has data. ~25 requests in total.
export const scenarios = [
  {
    name: 'ro: meta',
    run: async (t) => {
      await t.call('meta/root');
      await t.call('meta/get');
    },
  },
  {
    name: 'ro: repository',
    run: async (t, s) => {
      const repo = await t.call('repos/get', { params: s.rp, fixture: true });
      s.defaultBranch = repo.body.default_branch;
      await t.call('users/get-by-username', { params: { username: s.rp.owner } });
      await t.call('repos/list-branches', { params: s.rp, query: { per_page: 5 } });
      await t.call('repos/get-branch', { params: { ...s.rp, branch: s.defaultBranch } });
      await t.call('git/get-ref', { params: { ...s.rp, ref: `heads/${s.defaultBranch}` } });
      const commits = await t.call('repos/list-commits', { params: s.rp, query: { per_page: 2 } });
      const sha = commits?.body?.[0]?.sha;
      if (sha) {
        await t.call('repos/get-commit', { params: { ...s.rp, ref: sha } });
        await t.call('git/get-commit', { params: { ...s.rp, commit_sha: sha } });
      }
      await t.call('repos/list-tags', { params: s.rp, query: { per_page: 3 } });
      await t.call('repos/get-content', { params: { ...s.rp, path: 'README.md' } });
    },
  },
  {
    name: 'ro: issues',
    run: async (t, s) => {
      await t.checkPagination('issues/list-for-repo', { params: s.rp, query: { state: 'all' } });
      const list = await t.call('issues/list-for-repo', { params: s.rp, query: { state: 'all', per_page: 5 } });
      const issue = list.body.find((i) => !i.pull_request && i.comments > 0) || list.body[0];
      await t.call('issues/get', { params: { ...s.rp, issue_number: issue.number } });
      await t.call('issues/list-comments', { params: { ...s.rp, issue_number: issue.number }, query: { per_page: 5 } });
      await t.checkPagination('issues/list-labels-for-repo', { params: s.rp });
      await t.call('issues/list-milestones', { params: s.rp, query: { state: 'all', per_page: 3 } });
    },
  },
  {
    name: 'ro: pulls',
    run: async (t, s) => {
      const list = await t.call('pulls/list', { params: s.rp, query: { state: 'all', per_page: 3 } });
      const n = list.body[0].number;
      await t.call('pulls/get', { params: { ...s.rp, pull_number: n } });
      await t.call('pulls/list-files', { params: { ...s.rp, pull_number: n }, query: { per_page: 3 } });
    },
  },
  {
    name: 'ro: releases',
    run: async (t, s) => {
      await t.call('repos/list-releases', { params: s.rp, query: { per_page: 2 } });
      const latest = await t.call('repos/get-latest-release', { params: s.rp });
      if (latest?.body?.tag_name) await t.call('repos/get-release-by-tag', { params: { ...s.rp, tag: latest.body.tag_name } });
    },
  },
];
