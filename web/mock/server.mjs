// Tiny in-memory mock of the forge backend for UI development.
// Implements the subset of the GitHub REST API (/api/v3) and the forge-only endpoints (/api/forge)
// that the web UI uses, with GitHub-shaped JSON. Plain node:http, no dependencies.
//
//   node mock/server.mjs            # listens on MOCK_PORT (default 3001)
//
// Seed accounts: octocat / password  (owner of hello-world, empty-repo)
//                hubot   / password  (owner of private repo secret-project)

import http from "node:http";
import crypto from "node:crypto";

const PORT = Number(process.env.MOCK_PORT || 3001);
// Public base URL used in html_url/clone_url (the vite dev server, which proxies to us).
const BASE = (process.env.FORGE_BASE_URL || "http://localhost:5173").replace(/\/$/, "");
const API = `${BASE}/api/v3`;

// ---------------------------------------------------------------- helpers

const iso = (d = new Date()) => new Date(d).toISOString().replace(/\.\d{3}Z$/, "Z");
const daysAgo = (n) => new Date(Date.now() - n * 86400_000);
const hex = (n) => crypto.randomBytes(n).toString("hex");
const sha1 = (s) => crypto.createHash("sha1").update(s).digest("hex");
const sha256 = (s) => crypto.createHash("sha256").update(s).digest("hex");
const b62 = (n) => {
  const a = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
  return Array.from(crypto.randomBytes(n), (b) => a[b % 62]).join("");
};
let nextId = 1000;
const id = () => nextId++;

class HttpError extends Error {
  constructor(status, message, errors) {
    super(message);
    this.status = status;
    this.errors = errors;
  }
}
const notFound = () => new HttpError(404, "Not Found");
const validation = (resource, field, code) => new HttpError(422, "Validation Failed", [{ resource, field, code }]);

// ---------------------------------------------------------------- state

const users = new Map(); // login -> user
const sessions = new Map(); // sid -> login
const tokens = []; // {id, login, name, hash, last_eight, created_at, last_used_at}
const repos = new Map(); // "owner/name" (lowercase) -> repo

function addUser(login, password, extra = {}) {
  const u = {
    id: id(), login, password, email: `${login}@example.com`, name: null, bio: null, company: null,
    location: null, blog: null, created_at: iso(daysAgo(400)), updated_at: iso(daysAgo(1)), ...extra,
  };
  users.set(login.toLowerCase(), u);
  return u;
}
const findUser = (login) => users.get(String(login).toLowerCase());

function addRepo(owner, name, opts = {}) {
  const r = {
    id: id(), owner, name, description: opts.description ?? null, private: !!opts.private, homepage: null,
    default_branch: opts.default_branch || "main", created_at: iso(opts.created || daysAgo(30)),
    updated_at: iso(opts.created || daysAgo(30)), pushed_at: null,
    branches: new Map(), commits: new Map(), issues: [], next: 1, labels: defaultLabels(),
  };
  repos.set(`${owner}/${name}`.toLowerCase(), r);
  return r;
}
function defaultLabels() {
  return [
    ["bug", "d73a4a", "Something isn't working"], ["documentation", "0075ca", "Improvements or additions to documentation"],
    ["duplicate", "cfd3d7", "This issue or pull request already exists"], ["enhancement", "a2eeef", "New feature or request"],
    ["good first issue", "7057ff", "Good for newcomers"], ["help wanted", "008672", "Extra attention is needed"],
    ["invalid", "e4e669", "This doesn't seem right"], ["question", "d876e3", "Further information is requested"],
    ["wontfix", "ffffff", "This will not be worked on"],
  ].map(([name, color, description]) => ({ id: id(), name, color, description, default: true }));
}

/** Create a commit on `branch` applying `changes` ({path: content|null}). */
function commit(repo, branch, message, changes, login, date = new Date(), extraParents = []) {
  const parentSha = repo.branches.get(branch);
  const parent = parentSha ? repo.commits.get(parentSha) : null;
  const tree = { ...(parent ? parent.tree : {}) };
  for (const [p, c] of Object.entries(changes)) {
    if (c === null) delete tree[p];
    else tree[p] = c;
  }
  const u = findUser(login);
  const c = {
    sha: sha1(message + JSON.stringify(tree) + hex(8)), message,
    author: { name: u?.name || login, email: u?.email || `${login}@example.com`, date: iso(date) }, login,
    parents: [...(parentSha ? [parentSha] : []), ...extraParents], tree,
  };
  repo.commits.set(c.sha, c);
  repo.branches.set(branch, c.sha);
  repo.pushed_at = repo.updated_at = iso(date);
  return c;
}

function resolveRef(repo, ref) {
  if (!ref) ref = repo.default_branch;
  if (repo.branches.has(ref)) return repo.commits.get(repo.branches.get(ref));
  if (/^[0-9a-f]{4,40}$/.test(ref)) {
    for (const [s, c] of repo.commits) if (s.startsWith(ref)) return c;
  }
  return null;
}

/** All ancestors of `sha` (inclusive), newest first by date. */
function history(repo, sha) {
  const seen = new Set();
  const out = [];
  const stack = [sha];
  while (stack.length) {
    const s = stack.pop();
    if (!s || seen.has(s)) continue;
    seen.add(s);
    const c = repo.commits.get(s);
    if (!c) continue;
    out.push(c);
    stack.push(...c.parents);
  }
  return out.sort((a, b) => b.author.date.localeCompare(a.author.date));
}

function mergeBase(repo, a, b) {
  const anc = new Set(history(repo, a).map((c) => c.sha));
  return history(repo, b).find((c) => anc.has(c.sha)) || null;
}

// ---- unified diff (LCS) ----
function lineDiff(a, b) {
  const n = a.length, m = b.length;
  const dp = Array.from({ length: n + 1 }, () => new Uint32Array(m + 1));
  for (let i = n - 1; i >= 0; i--) for (let j = m - 1; j >= 0; j--)
    dp[i][j] = a[i] === b[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1]);
  const ops = [];
  let i = 0, j = 0;
  while (i < n && j < m) {
    if (a[i] === b[j]) { ops.push([" ", a[i], i, j]); i++; j++; }
    else if (dp[i + 1][j] >= dp[i][j + 1]) { ops.push(["-", a[i], i, j]); i++; }
    else { ops.push(["+", b[j], i, j]); j++; }
  }
  while (i < n) { ops.push(["-", a[i], i, j]); i++; }
  while (j < m) { ops.push(["+", b[j], i, j]); j++; }
  return ops;
}

function unifiedPatch(oldText, newText, ctx = 3) {
  const split = (t) => (t === null || t === undefined || t === "" ? [] : t.replace(/\n$/, "").split("\n"));
  const ops = lineDiff(split(oldText), split(newText));
  const changed = ops.map((o) => o[0] !== " ");
  const hunks = [];
  let k = 0;
  while (k < ops.length) {
    if (!changed[k]) { k++; continue; }
    let start = Math.max(0, k - ctx);
    let end = k;
    while (end < ops.length) {
      if (changed[end]) { end++; continue; }
      let run = 0;
      while (end + run < ops.length && !changed[end + run]) run++;
      if (end + run >= ops.length || run > ctx * 2) { end = Math.min(ops.length, end + ctx); break; }
      end += run;
    }
    hunks.push([start, end]);
    k = end;
  }
  let adds = 0, dels = 0;
  const out = [];
  for (const [s, e] of hunks) {
    const slice = ops.slice(s, e);
    const oldStart = slice[0][2] + 1, newStart = slice[0][3] + 1;
    const oldLen = slice.filter((o) => o[0] !== "+").length;
    const newLen = slice.filter((o) => o[0] !== "-").length;
    out.push(`@@ -${oldLen ? oldStart : oldStart - 1},${oldLen} +${newLen ? newStart : newStart - 1},${newLen} @@`);
    for (const [op, line] of slice) {
      if (op === "+") adds++;
      if (op === "-") dels++;
      out.push(op + line);
    }
  }
  return { patch: out.join("\n"), additions: adds, deletions: dels };
}

function diffTrees(repo, oldTree, newTree, ref) {
  const paths = [...new Set([...Object.keys(oldTree || {}), ...Object.keys(newTree || {})])].sort();
  const files = [];
  for (const p of paths) {
    const a = oldTree?.[p], b = newTree?.[p];
    if (a === b) continue;
    const status = a === undefined ? "added" : b === undefined ? "removed" : "modified";
    const { patch, additions, deletions } = unifiedPatch(a ?? "", b ?? "");
    files.push({
      sha: b !== undefined ? sha1("blob " + b) : null, filename: p, status, additions, deletions, changes: additions + deletions,
      blob_url: `${BASE}/${repo.owner}/${repo.name}/blob/${ref}/${p}`,
      raw_url: `${BASE}/${repo.owner}/${repo.name}/raw/${ref}/${p}`,
      contents_url: `${API}/repos/${repo.owner}/${repo.name}/contents/${p}?ref=${ref}`,
      patch,
    });
  }
  return files;
}

// ---------------------------------------------------------------- JSON shapes

function userJSON(u, full = false) {
  if (!u) return null;
  const base = {
    login: u.login, id: u.id, node_id: Buffer.from(`User:${u.id}`).toString("base64url"),
    avatar_url: `${BASE}/api/forge/avatars/${u.login}`, gravatar_id: "", url: `${API}/users/${u.login}`,
    html_url: `${BASE}/${u.login}`, followers_url: `${API}/users/${u.login}/followers`,
    following_url: `${API}/users/${u.login}/following{/other_user}`, gists_url: `${API}/users/${u.login}/gists{/gist_id}`,
    starred_url: `${API}/users/${u.login}/starred{/owner}{/repo}`, subscriptions_url: `${API}/users/${u.login}/subscriptions`,
    organizations_url: `${API}/users/${u.login}/orgs`, repos_url: `${API}/users/${u.login}/repos`,
    events_url: `${API}/users/${u.login}/events{/privacy}`, received_events_url: `${API}/users/${u.login}/received_events`,
    type: "User", user_view_type: "public", site_admin: false,
  };
  if (!full) return base;
  const publicRepos = [...repos.values()].filter((r) => r.owner === u.login && !r.private).length;
  return {
    ...base, name: u.name, company: u.company, blog: u.blog || "", location: u.location, email: u.email, hireable: null,
    bio: u.bio, twitter_username: null, public_repos: publicRepos, public_gists: 0, followers: 0, following: 0,
    created_at: u.created_at, updated_at: u.updated_at,
  };
}

function repoJSON(r, viewer) {
  const full = `${r.owner}/${r.name}`;
  const owner = findUser(r.owner);
  const url = `${API}/repos/${full}`;
  const isOwner = viewer && viewer.login === r.owner;
  const open = r.issues.filter((i) => i.state === "open").length;
  const out = {
    id: r.id, node_id: Buffer.from(`Repository:${r.id}`).toString("base64url"), name: r.name, full_name: full,
    private: r.private, owner: userJSON(owner), html_url: `${BASE}/${full}`, description: r.description, fork: false, url,
    forks_url: `${url}/forks`, keys_url: `${url}/keys{/key_id}`, collaborators_url: `${url}/collaborators{/collaborator}`,
    branches_url: `${url}/branches{/branch}`, tags_url: `${url}/tags`, languages_url: `${url}/languages`,
    commits_url: `${url}/commits{/sha}`, contents_url: `${url}/contents/{+path}`, compare_url: `${url}/compare/{base}...{head}`,
    issues_url: `${url}/issues{/number}`, pulls_url: `${url}/pulls{/number}`, labels_url: `${url}/labels{/name}`,
    created_at: r.created_at, updated_at: r.updated_at, pushed_at: r.pushed_at,
    git_url: `${BASE.replace(/^https?/, "git")}/${full}.git`, ssh_url: `git@${new URL(BASE).host}:${full}.git`,
    clone_url: `${BASE}/${full}.git`, svn_url: `${BASE}/${full}`, homepage: r.homepage,
    size: r.commits.size ? 42 : 0, stargazers_count: 0, watchers_count: 0, language: r.commits.size ? "Rust" : null,
    has_issues: true, has_projects: false, has_downloads: true, has_wiki: false, has_pages: false, has_discussions: false,
    forks_count: 0, mirror_url: null, archived: false, disabled: false, open_issues_count: open, license: null,
    allow_forking: true, is_template: false, web_commit_signoff_required: false, topics: [],
    visibility: r.private ? "private" : "public", forks: 0, open_issues: open, watchers: 0, default_branch: r.default_branch,
  };
  if (viewer) {
    out.permissions = { admin: isOwner, maintain: isOwner, push: isOwner, triage: isOwner, pull: true };
  }
  return out;
}

function commitJSON(repo, c, withFiles = false) {
  const full = `${repo.owner}/${repo.name}`;
  const u = findUser(c.login);
  const out = {
    sha: c.sha, node_id: Buffer.from(`Commit:${c.sha}`).toString("base64url"),
    commit: {
      author: c.author, committer: c.author, message: c.message,
      tree: { sha: sha1(JSON.stringify(c.tree)), url: `${API}/repos/${full}/git/trees/${sha1(JSON.stringify(c.tree))}` },
      url: `${API}/repos/${full}/git/commits/${c.sha}`, comment_count: 0,
      verification: { verified: false, reason: "unsigned", signature: null, payload: null, verified_at: null },
    },
    url: `${API}/repos/${full}/commits/${c.sha}`, html_url: `${BASE}/${full}/commit/${c.sha}`,
    comments_url: `${API}/repos/${full}/commits/${c.sha}/comments`,
    author: userJSON(u), committer: userJSON(u),
    parents: c.parents.map((p) => ({ sha: p, url: `${API}/repos/${full}/commits/${p}`, html_url: `${BASE}/${full}/commit/${p}` })),
  };
  if (withFiles) {
    const parent = c.parents[0] ? repo.commits.get(c.parents[0]) : null;
    const files = diffTrees(repo, parent?.tree || {}, c.tree, c.sha);
    const additions = files.reduce((a, f) => a + f.additions, 0);
    const deletions = files.reduce((a, f) => a + f.deletions, 0);
    out.stats = { total: additions + deletions, additions, deletions };
    out.files = files;
  }
  return out;
}

function labelJSON(repo, l) {
  return { id: l.id, node_id: Buffer.from(`Label:${l.id}`).toString("base64url"),
    url: `${API}/repos/${repo.owner}/${repo.name}/labels/${encodeURIComponent(l.name)}`, name: l.name, color: l.color,
    default: !!l.default, description: l.description };
}

function issueJSON(repo, i, viewer) {
  const full = `${repo.owner}/${repo.name}`;
  const url = `${API}/repos/${full}/issues/${i.number}`;
  const out = {
    url, repository_url: `${API}/repos/${full}`, labels_url: `${url}/labels{/name}`, comments_url: `${url}/comments`,
    events_url: `${url}/events`, html_url: `${BASE}/${full}/${i.pull ? "pull" : "issues"}/${i.number}`,
    id: i.id, node_id: Buffer.from(`Issue:${i.id}`).toString("base64url"), number: i.number, title: i.title,
    user: userJSON(findUser(i.user)), labels: i.labels.map((n) => repo.labels.find((l) => l.name === n)).filter(Boolean).map((l) => labelJSON(repo, l)),
    state: i.state, locked: false, assignee: null, assignees: [], milestone: null, comments: i.comments.length,
    created_at: i.created_at, updated_at: i.updated_at, closed_at: i.closed_at,
    author_association: i.user === repo.owner ? "OWNER" : "NONE", active_lock_reason: null, body: i.body,
    closed_by: null, reactions: { url: `${url}/reactions`, total_count: 0 }, timeline_url: `${url}/timeline`,
    performed_via_github_app: null, state_reason: i.state_reason, repository: undefined,
  };
  if (i.pull) {
    out.draft = !!i.pull.draft;
    out.pull_request = {
      url: `${API}/repos/${full}/pulls/${i.number}`, html_url: `${BASE}/${full}/pull/${i.number}`,
      diff_url: `${BASE}/${full}/pull/${i.number}.diff`, patch_url: `${BASE}/${full}/pull/${i.number}.patch`,
      merged_at: i.pull.merged_at,
    };
  }
  if (viewer === "with-repo") out.repository = repoJSON(repo, null);
  else delete out.repository;
  return out;
}

function commentJSON(repo, i, c) {
  const full = `${repo.owner}/${repo.name}`;
  return {
    url: `${API}/repos/${full}/issues/comments/${c.id}`, html_url: `${BASE}/${full}/${i.pull ? "pull" : "issues"}/${i.number}#issuecomment-${c.id}`,
    issue_url: `${API}/repos/${full}/issues/${i.number}`, id: c.id, node_id: Buffer.from(`IssueComment:${c.id}`).toString("base64url"),
    user: userJSON(findUser(c.user)), created_at: c.created_at, updated_at: c.updated_at,
    author_association: c.user === repo.owner ? "OWNER" : "NONE", body: c.body,
    reactions: { total_count: 0 }, performed_via_github_app: null,
  };
}

function prState(repo, i) {
  const p = i.pull;
  const headC = repo.commits.get(repo.branches.get(p.head) || p.head_sha);
  const baseC = repo.commits.get(repo.branches.get(p.base) || p.base_sha);
  const mb = headC && baseC ? mergeBase(repo, baseC.sha, headC.sha) : null;
  const baseAnc = new Set(baseC ? history(repo, baseC.sha).map((c) => c.sha) : []);
  const commits = headC ? history(repo, headC.sha).filter((c) => !baseAnc.has(c.sha)).reverse() : [];
  const files = headC ? diffTrees(repo, mb?.tree || {}, headC.tree, headC.sha) : [];
  // Conflict if base also changed a file differently since the merge-base.
  let mergeable = true;
  if (mb && baseC && headC) {
    for (const f of files) {
      const b = baseC.tree[f.filename], m = mb.tree[f.filename], h = headC.tree[f.filename];
      if (b !== m && b !== h) mergeable = false;
    }
  }
  return { headC, baseC, mb, commits, files, mergeable };
}

function pullJSON(repo, i) {
  const full = `${repo.owner}/${repo.name}`;
  const s = prState(repo, i);
  const p = i.pull;
  const owner = findUser(repo.owner);
  const refJSON = (ref, c) => ({
    label: `${repo.owner}:${ref}`, ref, sha: c?.sha || p.head_sha || "", user: userJSON(owner), repo: repoJSON(repo, null),
  });
  const issue = issueJSON(repo, i);
  return {
    url: `${API}/repos/${full}/pulls/${i.number}`, id: i.id, node_id: Buffer.from(`PullRequest:${i.id}`).toString("base64url"),
    html_url: `${BASE}/${full}/pull/${i.number}`, diff_url: `${BASE}/${full}/pull/${i.number}.diff`,
    patch_url: `${BASE}/${full}/pull/${i.number}.patch`, issue_url: issue.url, number: i.number, state: i.state,
    locked: false, title: i.title, user: issue.user, body: i.body, labels: issue.labels, milestone: null,
    active_lock_reason: null, created_at: i.created_at, updated_at: i.updated_at, closed_at: i.closed_at,
    merged_at: p.merged_at, merge_commit_sha: p.merge_commit_sha, assignee: null, assignees: [],
    requested_reviewers: [], requested_teams: [], draft: !!p.draft,
    commits_url: `${API}/repos/${full}/pulls/${i.number}/commits`, review_comments_url: `${API}/repos/${full}/pulls/${i.number}/comments`,
    comments_url: issue.comments_url, statuses_url: `${API}/repos/${full}/statuses/${s.headC?.sha}`,
    head: refJSON(p.head, p.merged_at ? repo.commits.get(p.head_sha) : s.headC),
    base: refJSON(p.base, s.baseC), author_association: issue.author_association, auto_merge: null,
    merged: !!p.merged_at, mergeable: i.state === "open" ? s.mergeable : null, rebaseable: s.mergeable,
    mergeable_state: i.state !== "open" ? "unknown" : s.mergeable ? "clean" : "dirty",
    merged_by: p.merged_by ? userJSON(findUser(p.merged_by)) : null, comments: i.comments.length, review_comments: 0,
    maintainer_can_modify: false,
    commits: p.merged_at ? p.final_commits : s.commits.length,
    additions: p.merged_at ? p.final_add : s.files.reduce((a, f) => a + f.additions, 0),
    deletions: p.merged_at ? p.final_del : s.files.reduce((a, f) => a + f.deletions, 0),
    changed_files: p.merged_at ? p.final_files.length : s.files.length,
  };
}

// ---------------------------------------------------------------- avatars

function avatarSVG(login) {
  const h = sha1(login);
  const hue = parseInt(h.slice(0, 2), 16) * 360 / 256;
  let cells = "";
  for (let y = 0; y < 5; y++) for (let x = 0; x < 3; x++) {
    if (parseInt(h[2 + y * 3 + x], 16) % 2) {
      cells += `<rect x="${x + 1}" y="${y + 1}" width="1" height="1"/>`;
      if (x < 2) cells += `<rect x="${5 - x}" y="${y + 1}" width="1" height="1"/>`;
    }
  }
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 7 7" shape-rendering="crispEdges"><rect width="7" height="7" fill="#f0f0f0"/><g fill="hsl(${hue} 55% 55%)">${cells}</g></svg>`;
}

// ---------------------------------------------------------------- HTTP plumbing

function parseCookies(h = "") {
  return Object.fromEntries(h.split(";").map((p) => p.trim().split("=")).filter((p) => p[0]).map(([k, ...v]) => [k, v.join("=")]));
}

function authenticate(req) {
  const auth = req.headers.authorization || "";
  let tok = null;
  let m;
  if ((m = auth.match(/^(?:token|bearer)\s+(\S+)/i))) tok = m[1];
  else if ((m = auth.match(/^basic\s+(\S+)/i))) {
    const [login, pw] = Buffer.from(m[1], "base64").toString().split(":");
    const u = findUser(login);
    if (u && u.password === pw) return { user: u, via: "basic" };
    tok = pw;
  }
  if (tok) {
    const t = tokens.find((t) => t.hash === sha256(tok));
    if (!t) throw new HttpError(401, "Bad credentials");
    t.last_used_at = iso();
    return { user: findUser(t.login), via: "token" };
  }
  const sid = parseCookies(req.headers.cookie).forge_session;
  if (sid && sessions.has(sid)) return { user: findUser(sessions.get(sid)), via: "cookie" };
  return { user: null, via: null };
}

function paginate(req, res, items, url, def = 30) {
  const q = url.searchParams;
  const per = Math.min(100, Math.max(1, Number(q.get("per_page")) || def));
  const page = Math.max(1, Number(q.get("page")) || 1);
  const last = Math.max(1, Math.ceil(items.length / per));
  const link = (p, rel) => {
    const u = new URL(url);
    u.searchParams.set("page", String(p));
    u.searchParams.set("per_page", String(per));
    return `<${BASE}${u.pathname}${u.search}>; rel="${rel}"`;
  };
  const links = [];
  if (page < last) links.push(link(page + 1, "next"), link(last, "last"));
  if (page > 1) links.push(link(1, "first"), link(page - 1, "prev"));
  if (links.length) res.setHeader("Link", links.join(", "));
  return items.slice((page - 1) * per, page * per);
}

function readBody(req) {
  return new Promise((resolve, reject) => {
    let data = "";
    req.on("data", (c) => (data += c));
    req.on("end", () => {
      if (!data) return resolve({});
      try {
        resolve(JSON.parse(data));
      } catch {
        reject(new HttpError(400, "Problems parsing JSON"));
      }
    });
  });
}

function send(res, status, body, headers = {}) {
  for (const [k, v] of Object.entries(headers)) res.setHeader(k, v);
  if (body === undefined) {
    res.writeHead(status);
    return res.end();
  }
  if (typeof body === "string") {
    res.writeHead(status, { "Content-Type": res.getHeader("Content-Type") || "text/plain; charset=utf-8" });
    return res.end(body);
  }
  res.writeHead(status, { "Content-Type": "application/json; charset=utf-8" });
  res.end(JSON.stringify(body, null, 2));
}

// ---------------------------------------------------------------- routes

const routes = [];
const route = (method, pattern, handler) => {
  const keys = [];
  const re = new RegExp(
    "^" + pattern.replace(/\{(\w+)(\*?)\}/g, (_m, k, star) => (keys.push(k), star ? "(.*)" : "([^/]+)")) + "/?$",
  );
  routes.push({ method, re, keys, handler });
};

function getRepo(ctx, o, r, { write = false, admin = false } = {}) {
  const repo = repos.get(`${o}/${r}`.toLowerCase());
  if (!repo) throw notFound();
  const isOwner = ctx.user && ctx.user.login === repo.owner;
  if (repo.private && !isOwner) throw notFound();
  if ((write || admin) && !ctx.user) throw new HttpError(401, "Requires authentication");
  if ((write || admin) && !isOwner) throw new HttpError(403, "Must have admin rights to Repository.");
  return repo;
}
const requireUser = (ctx) => {
  if (!ctx.user) throw new HttpError(401, "Requires authentication");
  return ctx.user;
};
const findIssue = (repo, n) => {
  const i = repo.issues.find((x) => x.number === Number(n));
  if (!i) throw notFound();
  return i;
};
const visibleRepos = (ctx) => [...repos.values()].filter((r) => !r.private || (ctx.user && ctx.user.login === r.owner));

function startSession(res, login) {
  const sid = b62(32);
  sessions.set(sid, login);
  res.setHeader("Set-Cookie", `forge_session=${sid}; Path=/; HttpOnly; SameSite=Lax; Max-Age=2592000`);
}

// ---- forge endpoints
route("POST", "/api/forge/signup", async (ctx, res) => {
  const { login, email, password } = ctx.body;
  if (!login || !/^[a-zA-Z0-9](?:[a-zA-Z0-9]|-(?=[a-zA-Z0-9])){0,38}$/.test(login)) throw validation("User", "login", "invalid");
  if (!email || !email.includes("@")) throw validation("User", "email", "invalid");
  if (!password || password.length < 8) throw new HttpError(422, "Validation Failed", [{ resource: "User", field: "password", code: "invalid", message: "Password is too short (minimum is 8 characters)" }]);
  if (findUser(login) || ["login", "signup", "new", "settings", "api", "logout"].includes(login.toLowerCase()))
    throw new HttpError(422, "Validation Failed", [{ resource: "User", field: "login", code: "already_exists", message: "Login is already taken" }]);
  const u = addUser(login, password, { email, created_at: iso(), name: null });
  startSession(res, u.login);
  return [201, userJSON(u, true)];
});
route("POST", "/api/forge/session", async (ctx, res) => {
  const { login, password } = ctx.body;
  const u = findUser(login || "") || [...users.values()].find((x) => x.email === login);
  if (!u || u.password !== password) throw new HttpError(401, "Incorrect username or password.");
  startSession(res, u.login);
  return [200, userJSON(u, true)];
});
route("DELETE", "/api/forge/session", async (ctx, res) => {
  const sid = parseCookies(ctx.req.headers.cookie).forge_session;
  if (sid) sessions.delete(sid);
  res.setHeader("Set-Cookie", "forge_session=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0");
  return [204];
});
const tokenJSON = (t) => ({ id: t.id, name: t.name, token_last_eight: t.last_eight, created_at: t.created_at, last_used_at: t.last_used_at, scopes: ["repo", "read:org", "gist", "workflow"] });
route("GET", "/api/forge/tokens", async (ctx) => {
  const u = requireUser(ctx);
  return [200, tokens.filter((t) => t.login === u.login).map(tokenJSON)];
});
route("POST", "/api/forge/tokens", async (ctx) => {
  const u = requireUser(ctx);
  if (!ctx.body.name?.trim()) throw validation("Token", "name", "missing_field");
  const token = "gho_" + b62(36);
  const t = { id: id(), login: u.login, name: ctx.body.name.trim(), hash: sha256(token), last_eight: token.slice(-8), created_at: iso(), last_used_at: null };
  tokens.push(t);
  return [201, { ...tokenJSON(t), token }];
});
route("DELETE", "/api/forge/tokens/{id}", async (ctx, _res, p) => {
  const u = requireUser(ctx);
  const i = tokens.findIndex((t) => t.id === Number(p.id) && t.login === u.login);
  if (i < 0) throw notFound();
  tokens.splice(i, 1);
  return [204];
});
route("GET", "/api/forge/avatars/{login}", async (_ctx, res, p) => {
  res.setHeader("Content-Type", "image/svg+xml");
  res.setHeader("Cache-Control", "public, max-age=3600");
  return [200, avatarSVG(p.login)];
});

// ---- meta / users
route("GET", "/api/v3", async () => [200, { current_user_url: `${API}/user`, repository_url: `${API}/repos/{owner}/{repo}` }]);
route("GET", "/api/v3/meta", async () => [200, { verifiable_password_authentication: true, installed_version: "3.14.0" }]);
route("GET", "/api/v3/user", async (ctx) => [200, userJSON(requireUser(ctx), true)]);
route("GET", "/api/v3/users/{u}", async (_ctx, _res, p) => {
  const u = findUser(p.u);
  if (!u) throw notFound();
  return [200, userJSON(u, true)];
});
const sortRepos = (list, sort = "full_name") =>
  list.sort((a, b) => (sort === "full_name" ? `${a.owner}/${a.name}`.localeCompare(`${b.owner}/${b.name}`)
    : (b[sort === "pushed" ? "pushed_at" : sort === "created" ? "created_at" : "updated_at"] || "").localeCompare(a[sort === "pushed" ? "pushed_at" : sort === "created" ? "created_at" : "updated_at"] || "")));
route("GET", "/api/v3/users/{u}/repos", async (ctx, res, p) => {
  const u = findUser(p.u);
  if (!u) throw notFound();
  const list = sortRepos(visibleRepos(ctx).filter((r) => r.owner === u.login), ctx.url.searchParams.get("sort") || "full_name");
  return [200, paginate(ctx.req, res, list, ctx.url).map((r) => repoJSON(r, ctx.user))];
});
route("GET", "/api/v3/user/repos", async (ctx, res) => {
  const u = requireUser(ctx);
  const list = sortRepos([...repos.values()].filter((r) => r.owner === u.login), ctx.url.searchParams.get("sort") || "full_name");
  return [200, paginate(ctx.req, res, list, ctx.url).map((r) => repoJSON(r, u))];
});
route("POST", "/api/v3/user/repos", async (ctx) => {
  const u = requireUser(ctx);
  const { name, description, auto_init } = ctx.body;
  if (!name) throw validation("Repository", "name", "missing_field");
  const clean = String(name).replace(/[^A-Za-z0-9._-]+/g, "-");
  if (repos.has(`${u.login}/${clean}`.toLowerCase()))
    throw new HttpError(422, "Repository creation failed.", [{ resource: "Repository", code: "custom", field: "name", message: "name already exists on this account" }]);
  const r = addRepo(u.login, clean, { description: description || null, private: !!ctx.body.private, created: new Date() });
  if (auto_init) commit(r, r.default_branch, "Initial commit", { "README.md": `# ${clean}\n${description ? "\n" + description + "\n" : ""}` }, u.login);
  return [201, repoJSON(r, u)];
});

function allIssuesFor(ctx, filterFn) {
  const out = [];
  for (const r of visibleRepos(ctx)) for (const i of r.issues) if (filterFn(r, i)) out.push([r, i]);
  return out.sort((a, b) => b[1].updated_at.localeCompare(a[1].updated_at));
}
route("GET", "/api/v3/issues", async (ctx, res) => {
  const u = requireUser(ctx);
  const q = ctx.url.searchParams;
  const state = q.get("state") || "open";
  const list = allIssuesFor(ctx, (r, i) =>
    (state === "all" || i.state === state) && (r.owner === u.login || i.user === u.login || i.comments.some((c) => c.user === u.login)));
  return [200, paginate(ctx.req, res, list, ctx.url).map(([r, i]) => issueJSON(r, i, "with-repo"))];
});
route("GET", "/api/v3/search/issues", async (ctx, res) => {
  const q = ctx.url.searchParams.get("q") || "";
  const terms = q.split(/\s+/).filter(Boolean);
  const list = allIssuesFor(ctx, (r, i) => terms.every((t) => {
    const [k, v] = t.includes(":") ? t.split(":") : [null, t];
    if (k === "is") return v === "issue" ? !i.pull : v === "pr" ? !!i.pull : v === "open" || v === "closed" ? i.state === v : true;
    if (k === "repo") return `${r.owner}/${r.name}`.toLowerCase() === v.toLowerCase();
    if (k === "involves" || k === "author") return i.user === v || r.owner === v;
    if (k === "state") return i.state === v;
    return (i.title + " " + (i.body || "")).toLowerCase().includes(t.toLowerCase());
  }));
  return [200, { total_count: list.length, incomplete_results: false, items: paginate(ctx.req, res, list, ctx.url).map(([r, i]) => issueJSON(r, i, "with-repo")) }];
});

// ---- repos
route("GET", "/api/v3/repos/{o}/{r}", async (ctx, _res, p) => [200, repoJSON(getRepo(ctx, p.o, p.r), ctx.user)]);
route("PATCH", "/api/v3/repos/{o}/{r}", async (ctx, _res, p) => {
  const repo = getRepo(ctx, p.o, p.r, { admin: true });
  const b = ctx.body;
  if (b.default_branch !== undefined && !repo.branches.has(b.default_branch)) throw validation("Repository", "default_branch", "invalid");
  if (b.name !== undefined && b.name !== repo.name) {
    if (!/^[A-Za-z0-9._-]+$/.test(b.name)) throw validation("Repository", "name", "invalid");
    if (repos.has(`${repo.owner}/${b.name}`.toLowerCase())) throw validation("Repository", "name", "already_exists");
    repos.delete(`${repo.owner}/${repo.name}`.toLowerCase());
    repo.name = b.name;
    repos.set(`${repo.owner}/${repo.name}`.toLowerCase(), repo);
  }
  for (const k of ["description", "homepage", "default_branch", "private"]) if (b[k] !== undefined) repo[k] = b[k];
  repo.updated_at = iso();
  return [200, repoJSON(repo, ctx.user)];
});
route("DELETE", "/api/v3/repos/{o}/{r}", async (ctx, _res, p) => {
  const repo = getRepo(ctx, p.o, p.r, { admin: true });
  repos.delete(`${repo.owner}/${repo.name}`.toLowerCase());
  return [204];
});
route("GET", "/api/v3/repos/{o}/{r}/branches", async (ctx, res, p) => {
  const repo = getRepo(ctx, p.o, p.r);
  const list = [...repo.branches.entries()].sort((a, b) => a[0].localeCompare(b[0]));
  return [200, paginate(ctx.req, res, list, ctx.url).map(([name, sha]) => ({
    name, commit: { sha, url: `${API}/repos/${repo.owner}/${repo.name}/commits/${sha}` }, protected: false,
  }))];
});

function contentItem(repo, path, type, ref, content) {
  const full = `${repo.owner}/${repo.name}`;
  const name = path.split("/").pop();
  const sha = type === "file" ? sha1("blob " + content) : sha1("tree " + path + ref);
  const url = `${API}/repos/${full}/contents/${path}?ref=${ref}`;
  const html = `${BASE}/${full}/${type === "file" ? "blob" : "tree"}/${ref}/${path}`;
  return {
    type, name, path, sha, size: type === "file" ? Buffer.byteLength(content) : 0, url, html_url: html,
    git_url: `${API}/repos/${full}/git/${type === "file" ? "blobs" : "trees"}/${sha}`,
    download_url: type === "file" ? `${BASE}/${full}/raw/${ref}/${path}` : null,
    _links: { self: url, git: `${API}/repos/${full}/git/blobs/${sha}`, html },
  };
}
function contents(ctx, res, repo, path, rawWanted) {
  const ref = ctx.url.searchParams.get("ref") || repo.default_branch;
  if (repo.branches.size === 0) throw new HttpError(404, "This repository is empty.");
  const c = resolveRef(repo, ref);
  if (!c) throw new HttpError(404, `No commit found for the ref ${ref}`);
  path = path.replace(/^\/+|\/+$/g, "");
  if (path && c.tree[path] !== undefined) {
    const content = c.tree[path];
    if (rawWanted) {
      res.setHeader("Content-Type", "text/plain; charset=utf-8");
      return [200, content];
    }
    return [200, { ...contentItem(repo, path, "file", ref, content), content: Buffer.from(content).toString("base64").replace(/(.{60})/g, "$1\n"), encoding: "base64" }];
  }
  const prefix = path ? path + "/" : "";
  const entries = new Map();
  for (const p of Object.keys(c.tree)) {
    if (!p.startsWith(prefix)) continue;
    const rest = p.slice(prefix.length);
    const [head, ...tail] = rest.split("/");
    entries.set(head, tail.length ? "dir" : "file");
  }
  if (!entries.size) throw notFound();
  return [200, [...entries].map(([n, t]) => contentItem(repo, prefix + n, t, ref, t === "file" ? c.tree[prefix + n] : ""))];
}
const wantsRaw = (req) => /vnd\.github(\.v3)?\.raw/.test(req.headers.accept || "");
route("GET", "/api/v3/repos/{o}/{r}/contents", async (ctx, res, p) => contents(ctx, res, getRepo(ctx, p.o, p.r), "", wantsRaw(ctx.req)));
route("GET", "/api/v3/repos/{o}/{r}/contents/{path*}", async (ctx, res, p) =>
  contents(ctx, res, getRepo(ctx, p.o, p.r), decodeURIComponent(p.path), wantsRaw(ctx.req)));
function readme(ctx, res, repo, dir) {
  const ref = ctx.url.searchParams.get("ref") || repo.default_branch;
  const c = resolveRef(repo, ref);
  if (!c) throw notFound();
  const prefix = dir ? dir.replace(/\/+$/, "") + "/" : "";
  const path = Object.keys(c.tree).find((p) => p.startsWith(prefix) && /^readme(\.\w+)?$/i.test(p.slice(prefix.length)));
  if (!path) throw notFound();
  if (wantsRaw(ctx.req)) {
    res.setHeader("Content-Type", "text/plain; charset=utf-8");
    return [200, c.tree[path]];
  }
  return [200, { ...contentItem(repo, path, "file", ref, c.tree[path]), content: Buffer.from(c.tree[path]).toString("base64"), encoding: "base64" }];
}
route("GET", "/api/v3/repos/{o}/{r}/readme", async (ctx, res, p) => readme(ctx, res, getRepo(ctx, p.o, p.r), ""));
route("GET", "/api/v3/repos/{o}/{r}/readme/{dir*}", async (ctx, res, p) => readme(ctx, res, getRepo(ctx, p.o, p.r), decodeURIComponent(p.dir)));

route("GET", "/api/v3/repos/{o}/{r}/commits", async (ctx, res, p) => {
  const repo = getRepo(ctx, p.o, p.r);
  if (repo.branches.size === 0) throw new HttpError(409, "Git Repository is empty.");
  const q = ctx.url.searchParams;
  const c = resolveRef(repo, q.get("sha"));
  if (!c) throw notFound();
  let list = history(repo, c.sha);
  const path = q.get("path");
  if (path) {
    list = list.filter((x) => {
      const parent = x.parents[0] ? repo.commits.get(x.parents[0]) : null;
      const changed = (t) => Object.keys(t || {}).filter((k) => k === path || k.startsWith(path + "/"));
      const keys = new Set([...changed(x.tree), ...changed(parent?.tree)]);
      return [...keys].some((k) => x.tree[k] !== parent?.tree?.[k]);
    });
  }
  return [200, paginate(ctx.req, res, list, ctx.url).map((x) => commitJSON(repo, x))];
});
route("GET", "/api/v3/repos/{o}/{r}/commits/{ref*}", async (ctx, _res, p) => {
  const repo = getRepo(ctx, p.o, p.r);
  const c = resolveRef(repo, decodeURIComponent(p.ref));
  if (!c) throw new HttpError(422, `No commit found for SHA: ${p.ref}`);
  return [200, commitJSON(repo, c, true)];
});
route("GET", "/api/v3/repos/{o}/{r}/compare/{spec*}", async (ctx, _res, p) => {
  const repo = getRepo(ctx, p.o, p.r);
  const [b, h] = decodeURIComponent(p.spec).split("...");
  const bc = resolveRef(repo, b), hc = resolveRef(repo, h);
  if (!bc || !hc) throw notFound();
  const mb = mergeBase(repo, bc.sha, hc.sha);
  const baseAnc = new Set(history(repo, bc.sha).map((c) => c.sha));
  const headAnc = new Set(history(repo, hc.sha).map((c) => c.sha));
  const ahead = history(repo, hc.sha).filter((c) => !baseAnc.has(c.sha)).reverse();
  const behind = history(repo, bc.sha).filter((c) => !headAnc.has(c.sha));
  const full = `${repo.owner}/${repo.name}`;
  return [200, {
    url: `${API}/repos/${full}/compare/${b}...${h}`, html_url: `${BASE}/${full}/compare/${b}...${h}`,
    base_commit: commitJSON(repo, bc), merge_base_commit: mb ? commitJSON(repo, mb) : null,
    status: bc.sha === hc.sha ? "identical" : ahead.length && behind.length ? "diverged" : ahead.length ? "ahead" : "behind",
    ahead_by: ahead.length, behind_by: behind.length, total_commits: ahead.length,
    commits: ahead.map((c) => commitJSON(repo, c)), files: diffTrees(repo, mb?.tree || {}, hc.tree, hc.sha),
  }];
});

// ---- issues
route("GET", "/api/v3/repos/{o}/{r}/labels", async (ctx, res, p) => {
  const repo = getRepo(ctx, p.o, p.r);
  return [200, paginate(ctx.req, res, repo.labels, ctx.url).map((l) => labelJSON(repo, l))];
});
function filterIssues(ctx, repo, pullsOnly) {
  const q = ctx.url.searchParams;
  const state = q.get("state") || "open";
  const labels = (q.get("labels") || "").split(",").filter(Boolean);
  return repo.issues
    .filter((i) => (pullsOnly ? !!i.pull : true))
    .filter((i) => state === "all" || i.state === state)
    .filter((i) => labels.every((l) => i.labels.includes(l)))
    .sort((a, b) => b.number - a.number);
}
route("GET", "/api/v3/repos/{o}/{r}/issues", async (ctx, res, p) => {
  const repo = getRepo(ctx, p.o, p.r);
  return [200, paginate(ctx.req, res, filterIssues(ctx, repo, false), ctx.url).map((i) => issueJSON(repo, i))];
});
route("POST", "/api/v3/repos/{o}/{r}/issues", async (ctx, _res, p) => {
  const u = requireUser(ctx);
  const repo = getRepo(ctx, p.o, p.r);
  if (!ctx.body.title?.trim()) throw validation("Issue", "title", "missing_field");
  const i = {
    id: id(), number: repo.next++, title: ctx.body.title.trim(), body: ctx.body.body || null, state: "open", state_reason: null,
    user: u.login, labels: (ctx.body.labels || []).filter((n) => repo.labels.some((l) => l.name === n)),
    created_at: iso(), updated_at: iso(), closed_at: null, comments: [],
  };
  repo.issues.push(i);
  return [201, issueJSON(repo, i)];
});
route("GET", "/api/v3/repos/{o}/{r}/issues/{n}", async (ctx, _res, p) => {
  const repo = getRepo(ctx, p.o, p.r);
  return [200, issueJSON(repo, findIssue(repo, p.n))];
});
function canEditIssue(ctx, repo, i) {
  const u = requireUser(ctx);
  if (u.login !== repo.owner && u.login !== i.user) throw new HttpError(403, "Must have push access to edit this issue.");
}
function applyIssuePatch(i, b) {
  if (b.title !== undefined) {
    if (!String(b.title).trim()) throw validation("Issue", "title", "missing_field");
    i.title = b.title;
  }
  if (b.body !== undefined) i.body = b.body;
  if (b.state && b.state !== i.state) {
    if (i.pull?.merged_at) throw new HttpError(422, "Cannot change the state of a merged pull request.");
    i.state = b.state;
    i.closed_at = b.state === "closed" ? iso() : null;
    i.state_reason = b.state === "closed" ? b.state_reason || "completed" : b.state_reason || "reopened";
  }
  i.updated_at = iso();
}
route("PATCH", "/api/v3/repos/{o}/{r}/issues/{n}", async (ctx, _res, p) => {
  const repo = getRepo(ctx, p.o, p.r);
  const i = findIssue(repo, p.n);
  canEditIssue(ctx, repo, i);
  applyIssuePatch(i, ctx.body);
  return [200, issueJSON(repo, i)];
});
route("GET", "/api/v3/repos/{o}/{r}/issues/{n}/comments", async (ctx, res, p) => {
  const repo = getRepo(ctx, p.o, p.r);
  const i = findIssue(repo, p.n);
  return [200, paginate(ctx.req, res, i.comments, ctx.url).map((c) => commentJSON(repo, i, c))];
});
route("POST", "/api/v3/repos/{o}/{r}/issues/{n}/comments", async (ctx, _res, p) => {
  const u = requireUser(ctx);
  const repo = getRepo(ctx, p.o, p.r);
  const i = findIssue(repo, p.n);
  if (!ctx.body.body?.trim()) throw validation("IssueComment", "body", "missing_field");
  const c = { id: id(), user: u.login, body: ctx.body.body, created_at: iso(), updated_at: iso() };
  i.comments.push(c);
  i.updated_at = iso();
  return [201, commentJSON(repo, i, c)];
});
route("PUT", "/api/v3/repos/{o}/{r}/issues/{n}/labels", async (ctx, _res, p) => {
  const repo = getRepo(ctx, p.o, p.r, { write: true });
  const i = findIssue(repo, p.n);
  const names = (Array.isArray(ctx.body) ? ctx.body : ctx.body.labels || []).map((l) => (typeof l === "string" ? l : l.name));
  for (const n of names) if (!repo.labels.some((l) => l.name === n)) repo.labels.push({ id: id(), name: n, color: "ededed", description: null });
  i.labels = names;
  i.updated_at = iso();
  return [200, issueJSON(repo, i).labels];
});

// ---- pulls
route("GET", "/api/v3/repos/{o}/{r}/pulls", async (ctx, res, p) => {
  const repo = getRepo(ctx, p.o, p.r);
  return [200, paginate(ctx.req, res, filterIssues(ctx, repo, true), ctx.url).map((i) => pullJSON(repo, i))];
});
route("POST", "/api/v3/repos/{o}/{r}/pulls", async (ctx, _res, p) => {
  const u = requireUser(ctx);
  const repo = getRepo(ctx, p.o, p.r);
  const { title, body, head, base, draft } = ctx.body;
  if (!title?.trim()) throw validation("PullRequest", "title", "missing_field");
  const headName = String(head || "").replace(/^[^:]+:/, "");
  if (!repo.branches.has(headName)) throw validation("PullRequest", "head", "invalid");
  if (!repo.branches.has(base)) throw validation("PullRequest", "base", "invalid");
  if (repo.issues.some((i) => i.pull && i.state === "open" && i.pull.head === headName && i.pull.base === base))
    throw new HttpError(422, "Validation Failed", [{ resource: "PullRequest", code: "custom", message: `A pull request already exists for ${repo.owner}:${headName}.` }]);
  const ahead = history(repo, repo.branches.get(headName)).filter((c) => !new Set(history(repo, repo.branches.get(base)).map((x) => x.sha)).has(c.sha));
  if (!ahead.length) throw new HttpError(422, "Validation Failed", [{ resource: "PullRequest", code: "custom", message: `No commits between ${base} and ${headName}` }]);
  const i = {
    id: id(), number: repo.next++, title: title.trim(), body: body || null, state: "open", state_reason: null, user: u.login,
    labels: [], created_at: iso(), updated_at: iso(), closed_at: null, comments: [],
    pull: { head: headName, base, draft: !!draft, merged_at: null, merged_by: null, merge_commit_sha: null },
  };
  repo.issues.push(i);
  return [201, pullJSON(repo, i)];
});
const findPull = (repo, n) => {
  const i = findIssue(repo, n);
  if (!i.pull) throw notFound();
  return i;
};
route("GET", "/api/v3/repos/{o}/{r}/pulls/{n}", async (ctx, _res, p) => {
  const repo = getRepo(ctx, p.o, p.r);
  return [200, pullJSON(repo, findPull(repo, p.n))];
});
route("PATCH", "/api/v3/repos/{o}/{r}/pulls/{n}", async (ctx, _res, p) => {
  const repo = getRepo(ctx, p.o, p.r);
  const i = findPull(repo, p.n);
  canEditIssue(ctx, repo, i);
  applyIssuePatch(i, ctx.body);
  return [200, pullJSON(repo, i)];
});
route("GET", "/api/v3/repos/{o}/{r}/pulls/{n}/files", async (ctx, res, p) => {
  const repo = getRepo(ctx, p.o, p.r);
  const i = findPull(repo, p.n);
  const files = i.pull.merged_at ? i.pull.final_files : prState(repo, i).files;
  return [200, paginate(ctx.req, res, files, ctx.url)];
});
route("GET", "/api/v3/repos/{o}/{r}/pulls/{n}/commits", async (ctx, res, p) => {
  const repo = getRepo(ctx, p.o, p.r);
  const i = findPull(repo, p.n);
  const commits = i.pull.merged_at ? i.pull.final_commit_list.map((s) => repo.commits.get(s)) : prState(repo, i).commits;
  return [200, paginate(ctx.req, res, commits, ctx.url).map((c) => commitJSON(repo, c))];
});
route("GET", "/api/v3/repos/{o}/{r}/pulls/{n}/merge", async (ctx, _res, p) => {
  const repo = getRepo(ctx, p.o, p.r);
  if (!findPull(repo, p.n).pull.merged_at) throw notFound();
  return [204];
});
route("PUT", "/api/v3/repos/{o}/{r}/pulls/{n}/merge", async (ctx, _res, p) => {
  const repo = getRepo(ctx, p.o, p.r, { write: true });
  const i = findPull(repo, p.n);
  if (i.state !== "open") throw new HttpError(405, "Pull Request is not mergeable");
  const s = prState(repo, i);
  if (!s.mergeable) throw new HttpError(405, "Pull Request is not mergeable");
  const method = ctx.body.merge_method || "merge";
  const u = ctx.user;
  const changes = {};
  for (const f of s.files) changes[f.filename] = f.status === "removed" ? null : s.headC.tree[f.filename];
  i.pull.final_files = s.files;
  i.pull.final_commits = s.commits.length;
  i.pull.final_commit_list = s.commits.map((c) => c.sha);
  i.pull.final_add = s.files.reduce((a, f) => a + f.additions, 0);
  i.pull.final_del = s.files.reduce((a, f) => a + f.deletions, 0);
  i.pull.head_sha = s.headC.sha;
  let c;
  if (method === "merge") {
    const msg = (ctx.body.commit_title || `Merge pull request #${i.number} from ${repo.owner}/${i.pull.head}`) + "\n\n" + (ctx.body.commit_message ?? i.title);
    c = commit(repo, i.pull.base, msg, changes, u.login, new Date(), [s.headC.sha]);
  } else if (method === "squash") {
    const msg = (ctx.body.commit_title || `${i.title} (#${i.number})`) + "\n\n" + (ctx.body.commit_message ?? s.commits.map((x) => "* " + x.message.split("\n")[0]).join("\n"));
    c = commit(repo, i.pull.base, msg, changes, u.login);
  } else if (method === "rebase") {
    for (const orig of s.commits) {
      const parent = orig.parents[0] ? repo.commits.get(orig.parents[0]) : null;
      const ch = {};
      for (const f of diffTrees(repo, parent?.tree || {}, orig.tree, orig.sha)) ch[f.filename] = f.status === "removed" ? null : orig.tree[f.filename];
      c = commit(repo, i.pull.base, orig.message, ch, orig.login);
    }
  } else throw validation("PullRequest", "merge_method", "invalid");
  i.state = "closed";
  i.closed_at = i.updated_at = iso();
  i.pull.merged_at = iso();
  i.pull.merged_by = u.login;
  i.pull.merge_commit_sha = c.sha;
  return [200, { sha: c.sha, merged: true, message: "Pull Request successfully merged" }];
});

// ---------------------------------------------------------------- server

const server = http.createServer(async (req, res) => {
  const url = new URL(req.url, "http://mock");
  res.setHeader("X-GitHub-Request-Id", hex(8).toUpperCase());
  res.setHeader("X-GitHub-Enterprise-Version", "3.14.0");
  const t0 = Date.now();
  try {
    // git smart HTTP is not mocked
    if (/\/(info\/refs|git-upload-pack|git-receive-pack)$/.test(url.pathname)) {
      throw new HttpError(501, "git smart HTTP is not implemented by the mock server");
    }
    const ctx = { req, url, ...authenticate(req) };
    const mutating = !["GET", "HEAD", "OPTIONS"].includes(req.method);
    if (mutating && ctx.via === "cookie" && req.headers["x-forge-csrf"] !== "1") throw new HttpError(403, "CSRF header X-Forge-CSRF missing");
    if (ctx.user) res.setHeader("X-OAuth-Scopes", "repo, read:org, gist, workflow, admin:org, user, delete_repo");
    let matched = false;
    for (const r of routes) {
      const m = url.pathname.match(r.re);
      if (!m) continue;
      matched = true;
      if (r.method !== req.method) continue;
      const params = Object.fromEntries(r.keys.map((k, i) => [k, m[i + 1]]));
      ctx.body = mutating ? await readBody(req) : {};
      // a little latency so loading states are visible
      await new Promise((ok) => setTimeout(ok, Number(process.env.MOCK_LATENCY ?? 120)));
      const [status, body] = await r.handler(ctx, res, params);
      send(res, status, body);
      log(req, status, t0);
      return;
    }
    throw matched ? new HttpError(405, "Method Not Allowed") : notFound();
  } catch (e) {
    const status = e instanceof HttpError ? e.status : 500;
    if (!(e instanceof HttpError)) console.error(e);
    send(res, status, {
      message: e instanceof HttpError ? e.message : "Internal Server Error",
      ...(e.errors ? { errors: e.errors } : {}),
      documentation_url: "https://docs.github.com/rest",
      status: String(status),
    });
    log(req, status, t0);
  }
});
function log(req, status, t0) {
  if (process.env.MOCK_QUIET) return;
  console.log(`${new Date().toISOString().slice(11, 19)} ${status} ${req.method} ${req.url} ${Date.now() - t0}ms`);
}

// ---------------------------------------------------------------- seed

function seed() {
  addUser("octocat", "password", { name: "The Octocat", bio: "Forging code since 2026.", company: "@forge", location: "San Francisco", blog: "forge.example.com" });
  addUser("hubot", "password", { name: "Hubot", bio: "I am a robot." });

  const r = addRepo("octocat", "hello-world", { description: "My first repository on forge — a tiny Rust greeter.", created: daysAgo(30) });
  commit(r, "main", "Initial commit", {
    "README.md": `# hello-world

A tiny **Rust** greeter used to exercise the forge web UI.

## Usage

\`\`\`sh
cargo run -- Octocat
\`\`\`

| Feature | Status |
|---|---|
| Greeting | ✅ |
| Farewell | 🚧 |

- [x] Initial skeleton
- [ ] Localization

> Note: this README is rendered client-side with *marked* and sanitized with DOMPurify.
<script>alert("this must not run")</script>
`,
    "Cargo.toml": `[package]\nname = "hello-world"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\n`,
    ".gitignore": "/target\n",
    "src/main.rs": `fn main() {\n    let name = std::env::args().nth(1).unwrap_or_else(|| "world".to_string());\n    println!("Hello, {}!", name);\n}\n`,
  }, "octocat", daysAgo(20));
  commit(r, "main", "Extract greeting into a library\n\nThis makes the greeting testable.", {
    "src/lib.rs": `/// Build a greeting for \`name\`.\npub fn greet(name: &str) -> String {\n    format!("Hello, {}!", name)\n}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn greets() {\n        assert_eq!(greet("forge"), "Hello, forge!");\n    }\n}\n`,
    "src/main.rs": `use hello_world::greet;\n\nfn main() {\n    let name = std::env::args().nth(1).unwrap_or_else(|| "world".to_string());\n    println!("{}", greet(&name));\n}\n`,
  }, "octocat", daysAgo(12));
  commit(r, "main", "Add docs", {
    "docs/guide.md": "# Guide\n\nRun `cargo test` to run the tests.\n",
    "docs/README.md": "# Docs\n\nThis directory has its own README.\n",
  }, "hubot", daysAgo(6));
  // feature branch from main
  r.branches.set("feature/farewell", r.branches.get("main"));
  commit(r, "feature/farewell", "Add farewell()", {
    "src/lib.rs": `/// Build a greeting for \`name\`.\npub fn greet(name: &str) -> String {\n    format!("Hello, {}!", name)\n}\n\n/// Say goodbye to \`name\`.\npub fn farewell(name: &str) -> String {\n    format!("Goodbye, {}!", name)\n}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn greets() {\n        assert_eq!(greet("forge"), "Hello, forge!");\n    }\n\n    #[test]\n    fn farewells() {\n        assert_eq!(farewell("forge"), "Goodbye, forge!");\n    }\n}\n`,
  }, "hubot", daysAgo(3));
  commit(r, "feature/farewell", "Document farewell in README", {
    "README.md": r.commits.get(r.branches.get("main")).tree["README.md"].replace("| Farewell | 🚧 |", "| Farewell | ✅ |"),
  }, "hubot", daysAgo(2));
  r.branches.set("experiment", r.branches.get("main"));
  commit(r, "experiment", "Try a different edition", { "Cargo.toml": `[package]\nname = "hello-world"\nversion = "0.2.0"\nedition = "2024"\n\n[dependencies]\n` }, "octocat", daysAgo(1));
  // more history so the commits page paginates
  for (let k = 0; k < 32; k++) {
    commit(r, "main", `Tweak changelog (${k + 1})`, { "CHANGELOG.md": `# Changelog\n\n${Array.from({ length: k + 1 }, (_, j) => `- entry ${j + 1}`).join("\n")}\n` }, k % 3 ? "octocat" : "hubot", new Date(Date.now() - (5 - k / 8) * 86400_000));
  }

  const mkIssue = (title, body, user, labels = [], extra = {}) => {
    const i = { id: id(), number: r.next++, title, body, state: "open", state_reason: null, user, labels, created_at: iso(daysAgo(10)), updated_at: iso(daysAgo(1)), closed_at: null, comments: [], ...extra };
    r.issues.push(i);
    return i;
  };
  const i1 = mkIssue("Crash when name contains an emoji", "Steps to reproduce:\n\n1. `cargo run -- 🦀`\n2. Observe the panic\n\n```rust\nthread 'main' panicked at src/main.rs:3\n```", "hubot", ["bug"]);
  i1.comments.push({ id: id(), user: "octocat", body: "Thanks for the report! I can reproduce this. See #3 for a related change.", created_at: iso(daysAgo(9)), updated_at: iso(daysAgo(9)) });
  i1.comments.push({ id: id(), user: "hubot", body: "Happy to help **debug**.", created_at: iso(daysAgo(8)), updated_at: iso(daysAgo(8)) });
  mkIssue("Write a contributing guide", "We should have a `CONTRIBUTING.md`.", "octocat", ["documentation", "good first issue"], { state: "closed", state_reason: "completed", closed_at: iso(daysAgo(4)) });
  mkIssue("Add a farewell function", "Implemented in the `feature/farewell` branch.", "hubot", ["enhancement"], {
    pull: { head: "feature/farewell", base: "main", draft: false, merged_at: null, merged_by: null, merge_commit_sha: null },
  });
  mkIssue("Bump edition to 2024", "", "octocat", [], {
    pull: { head: "experiment", base: "main", draft: true, merged_at: null, merged_by: null, merge_commit_sha: null },
  });
  for (let k = 0; k < 28; k++) mkIssue(`Seeded issue ${k + 1}`, `Body of seeded issue ${k + 1}.`, k % 2 ? "octocat" : "hubot", k % 4 === 0 ? ["question"] : []);

  addRepo("octocat", "empty-repo", { description: "Nothing pushed yet", created: daysAgo(2) });
  const s = addRepo("hubot", "secret-project", { description: "Private things", private: true, created: daysAgo(15) });
  commit(s, "main", "Initial commit", { "README.md": "# secret-project\n\nShh.\n" }, "hubot", daysAgo(15));

  // A known token for curl testing: gho_mocktokenmocktokenmocktokenmocktok
  const known = "gho_" + "mocktoken".repeat(4);
  tokens.push({ id: id(), login: "octocat", name: "seeded token", hash: sha256(known), last_eight: known.slice(-8), created_at: iso(daysAgo(3)), last_used_at: null });
}

seed();
server.listen(PORT, () => {
  console.log(`forge mock API listening on http://localhost:${PORT} (public base ${BASE})`);
  console.log(`  sign in as octocat / password  (or hubot / password)`);
});
