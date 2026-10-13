// Typed client for the GitHub-compatible REST API (/api/v3) and forge-only endpoints (/api/forge).
// Shapes follow api.github.com; only the fields the UI uses are typed.

export const API = "/api/v3";
export const FORGE = "/api/forge";

// ---------- types ----------

export interface User {
  login: string;
  id: number;
  avatar_url: string;
  html_url: string;
  type: "User" | "Organization" | string;
  name?: string | null;
  email?: string | null;
  bio?: string | null;
  company?: string | null;
  location?: string | null;
  blog?: string | null;
  public_repos?: number;
  followers?: number;
  following?: number;
  created_at?: string;
  site_admin?: boolean;
}

export interface Repo {
  id: number;
  name: string;
  full_name: string;
  owner: User;
  private: boolean;
  description: string | null;
  fork: boolean;
  parent?: Repo;
  source?: Repo;
  html_url: string;
  clone_url: string;
  ssh_url?: string;
  default_branch: string;
  language?: string | null;
  stargazers_count: number;
  forks_count: number;
  open_issues_count: number;
  size?: number;
  created_at: string;
  updated_at: string;
  pushed_at: string | null;
  homepage?: string | null;
  has_issues?: boolean;
  visibility?: string;
  permissions?: { admin: boolean; push: boolean; pull: boolean; maintain?: boolean; triage?: boolean };
}

export interface Branch {
  name: string;
  commit: { sha: string; url: string };
  protected?: boolean;
}

export interface ContentItem {
  type: "file" | "dir" | "symlink" | "submodule";
  name: string;
  path: string;
  sha: string;
  size: number;
  url: string;
  html_url: string | null;
  download_url: string | null;
  content?: string;
  encoding?: string;
}

export interface GitActor {
  name: string;
  email: string;
  date: string;
}

export interface CommitFile {
  sha?: string | null;
  filename: string;
  previous_filename?: string;
  status: "added" | "removed" | "modified" | "renamed" | "copied" | "changed" | "unchanged" | string;
  additions: number;
  deletions: number;
  changes: number;
  patch?: string;
  blob_url?: string;
}

export interface Commit {
  sha: string;
  html_url: string;
  commit: {
    message: string;
    author: GitActor | null;
    committer: GitActor | null;
    comment_count?: number;
  };
  author: User | null;
  committer: User | null;
  parents: { sha: string; url: string; html_url?: string }[];
  stats?: { additions: number; deletions: number; total: number };
  files?: CommitFile[];
}

export interface Label {
  id: number;
  name: string;
  color: string;
  description: string | null;
  default?: boolean;
}

export interface Issue {
  id: number;
  number: number;
  title: string;
  body: string | null;
  state: "open" | "closed";
  state_reason?: string | null;
  user: User;
  labels: Label[];
  assignees?: User[];
  comments: number;
  created_at: string;
  updated_at: string;
  closed_at: string | null;
  html_url: string;
  repository_url: string;
  repository?: Repo;
  pull_request?: { url: string; html_url: string; merged_at?: string | null };
  locked?: boolean;
  author_association?: string;
}

export interface IssueComment {
  id: number;
  body: string;
  user: User;
  created_at: string;
  updated_at: string;
  html_url: string;
  author_association?: string;
}

export interface PullRef {
  label: string;
  ref: string;
  sha: string;
  user: User | null;
  repo: Repo | null;
}

export interface Pull {
  id: number;
  number: number;
  title: string;
  body: string | null;
  state: "open" | "closed";
  draft?: boolean;
  user: User;
  labels: Label[];
  created_at: string;
  updated_at: string;
  closed_at: string | null;
  merged_at: string | null;
  merged?: boolean;
  mergeable?: boolean | null;
  mergeable_state?: string;
  merged_by?: User | null;
  merge_commit_sha?: string | null;
  comments?: number;
  commits?: number;
  additions?: number;
  deletions?: number;
  changed_files?: number;
  head: PullRef;
  base: PullRef;
  html_url: string;
}

export interface Compare {
  status: "ahead" | "behind" | "diverged" | "identical" | string;
  ahead_by: number;
  behind_by: number;
  total_commits: number;
  commits: Commit[];
  files?: CommitFile[];
}

export interface SearchResult<T> {
  total_count: number;
  incomplete_results: boolean;
  items: T[];
}

export interface Tag {
  name: string;
  commit: { sha: string; url: string };
  zipball_url: string;
  tarball_url: string;
}

export interface ReleaseAsset {
  id: number;
  name: string;
  label: string | null;
  state: string;
  content_type: string;
  size: number;
  download_count: number;
  browser_download_url: string;
  created_at: string;
  updated_at: string;
  uploader: User | null;
}

export interface Release {
  id: number;
  tag_name: string;
  target_commitish: string;
  name: string | null;
  body: string | null;
  draft: boolean;
  prerelease: boolean;
  created_at: string;
  published_at: string | null;
  author: User;
  assets: ReleaseAsset[];
  html_url: string;
  upload_url: string;
  tarball_url: string | null;
  zipball_url: string | null;
}

export interface Review {
  id: number;
  user: User | null;
  body: string | null;
  state: "APPROVED" | "CHANGES_REQUESTED" | "COMMENTED" | "DISMISSED" | "PENDING" | string;
  submitted_at?: string | null;
  html_url: string;
  author_association?: string;
}

export interface SshKey {
  id: number;
  key: string;
  title?: string;
  created_at?: string;
  read_only?: boolean;
  verified?: boolean;
}

export interface CodeSearchItem {
  name: string;
  path: string;
  sha: string;
  html_url: string;
  repository: Repo;
  text_matches?: { fragment: string }[];
}

export interface Token {
  id: number;
  name: string;
  token?: string; // only present right after creation
  created_at?: string;
  last_used_at?: string | null;
  token_last_eight?: string | null;
}

// ---------- errors ----------

export interface GitHubErrorBody {
  message?: string;
  documentation_url?: string;
  errors?: ({ resource?: string; field?: string; code?: string; message?: string } | string)[];
}

export class ApiError extends Error {
  status: number;
  body: GitHubErrorBody | null;
  constructor(status: number, body: GitHubErrorBody | null, fallback: string) {
    super(formatError(body) || fallback);
    this.status = status;
    this.body = body;
  }
}

function formatError(body: GitHubErrorBody | null): string {
  if (!body || !body.message) return "";
  const details = (body.errors || [])
    .map((e) => {
      if (typeof e === "string") return e;
      if (e.message) return e.message;
      if (e.field && e.code) return `${e.field} ${e.code.replace(/_/g, " ")}`;
      return e.code || "";
    })
    .filter(Boolean);
  return details.length ? `${body.message.replace(/[.:]\s*$/, "")}: ${details.join("; ")}` : body.message;
}

export function errorMessage(e: unknown): string {
  if (e instanceof ApiError) return e.message;
  if (e instanceof Error) return e.message;
  return String(e);
}

export const isNotFound = (e: unknown) => e instanceof ApiError && e.status === 404;

// ---------- pagination ----------

export interface Links {
  next?: string;
  prev?: string;
  first?: string;
  last?: string;
}

/** Parse an RFC 5988 Link header as emitted by GitHub. */
export function parseLink(header: string | null): Links {
  const links: Links = {};
  if (!header) return links;
  for (const part of header.split(",")) {
    const m = part.match(/<([^>]+)>\s*;\s*rel="?([a-z]+)"?/i);
    if (m) (links as Record<string, string>)[m[2]] = m[1];
  }
  return links;
}

/** Extract the `page` query param from a Link URL. */
export function pageOf(url: string | undefined): number | undefined {
  if (!url) return undefined;
  try {
    const p = new URL(url, location.origin).searchParams.get("page");
    return p ? Number(p) : undefined;
  } catch {
    return undefined;
  }
}

export interface Page<T> {
  items: T;
  links: Links;
  /** Page numbers derived from the Link header (undefined = no such page). */
  next?: number;
  prev?: number;
  last?: number;
}

// ---------- core request ----------

type Query = Record<string, string | number | boolean | undefined | null>;

export interface RequestOpts {
  method?: string;
  query?: Query;
  body?: unknown;
  accept?: string;
  signal?: AbortSignal;
}

function buildUrl(path: string, query?: Query): string {
  const url = path.startsWith("/api/") ? path : API + path;
  if (!query) return url;
  const qs = new URLSearchParams();
  for (const [k, v] of Object.entries(query)) {
    if (v !== undefined && v !== null && v !== "") qs.set(k, String(v));
  }
  const s = qs.toString();
  return s ? `${url}?${s}` : url;
}

async function raw(path: string, opts: RequestOpts = {}): Promise<Response> {
  const method = (opts.method || "GET").toUpperCase();
  const headers: Record<string, string> = {
    Accept: opts.accept || "application/vnd.github+json",
    "X-GitHub-Api-Version": "2022-11-28",
  };
  if (method !== "GET" && method !== "HEAD") headers["X-Forge-CSRF"] = "1";
  let body: BodyInit | undefined;
  if (opts.body !== undefined) {
    headers["Content-Type"] = "application/json";
    body = JSON.stringify(opts.body);
  }
  let res: Response;
  try {
    res = await fetch(buildUrl(path, opts.query), {
      method,
      headers,
      body,
      credentials: "same-origin",
      // The API sends GitHub's `Cache-Control: private, max-age=60`; without this the browser would
      // serve stale lists (branches, comments, …) for a minute after every change.
      cache: "no-store",
      signal: opts.signal,
    });
  } catch (e) {
    if ((e as Error).name === "AbortError") throw e;
    throw new ApiError(0, null, "Network error: could not reach the server");
  }
  if (!res.ok) {
    let errBody: GitHubErrorBody | null = null;
    try {
      const text = await res.text();
      errBody = text ? JSON.parse(text) : null;
    } catch {
      /* non-JSON error */
    }
    throw new ApiError(res.status, errBody, `${res.status} ${res.statusText || "Request failed"}`);
  }
  return res;
}

export async function request<T>(path: string, opts: RequestOpts = {}): Promise<T> {
  const res = await raw(path, opts);
  if (res.status === 204 || res.status === 205) return undefined as T;
  const text = await res.text();
  return (text ? JSON.parse(text) : undefined) as T;
}

export async function requestPage<T>(path: string, opts: RequestOpts = {}): Promise<Page<T>> {
  const res = await raw(path, opts);
  const items = (await res.json()) as T;
  const links = parseLink(res.headers.get("Link"));
  return { items, links, next: pageOf(links.next), prev: pageOf(links.prev), last: pageOf(links.last) };
}

export async function requestText(path: string, opts: RequestOpts = {}): Promise<string> {
  const res = await raw(path, opts);
  return res.text();
}

/** Follow `rel="next"` links until exhausted (bounded by maxPages). */
export async function requestAll<T>(path: string, opts: RequestOpts = {}, maxPages = 10): Promise<T[]> {
  const out: T[] = [];
  let page = 1;
  for (let i = 0; i < maxPages; i++) {
    const p = await requestPage<T[]>(path, { ...opts, query: { per_page: 100, ...opts.query, page } });
    out.push(...p.items);
    if (!p.next) break;
    page = p.next;
  }
  return out;
}

const enc = encodeURIComponent;
const encPath = (p: string) => p.split("/").filter(Boolean).map(enc).join("/");
const rp = (o: string, r: string) => `/repos/${enc(o)}/${enc(r)}`;
const RAW = "application/vnd.github.raw";

// ---------- endpoints ----------

export const api = {
  // auth / forge
  currentUser: () => request<User>("/user"),
  signup: (b: { login: string; email: string; password: string }) =>
    request<User>(`${FORGE}/signup`, { method: "POST", body: b }),
  login: (b: { login: string; password: string }) =>
    request<User>(`${FORGE}/session`, { method: "POST", body: b }),
  logout: () => request<void>(`${FORGE}/session`, { method: "DELETE" }),
  listTokens: () => request<Token[]>(`${FORGE}/tokens`),
  createToken: (name: string) => request<Token>(`${FORGE}/tokens`, { method: "POST", body: { name } }),
  deleteToken: (id: number) => request<void>(`${FORGE}/tokens/${id}`, { method: "DELETE" }),

  // users
  user: (u: string) => request<User>(`/users/${enc(u)}`),
  userRepos: (u: string, page = 1) =>
    requestPage<Repo[]>(`/users/${enc(u)}/repos`, { query: { sort: "updated", per_page: 30, page } }),
  myRepos: () => requestAll<Repo>("/user/repos", { query: { sort: "updated", per_page: 100 } }, 3),
  myIssues: () =>
    request<Issue[]>("/issues", { query: { filter: "all", state: "open", sort: "updated", per_page: 10 } }),
  searchIssues: (q: string, per_page = 10) =>
    request<SearchResult<Issue>>("/search/issues", { query: { q, sort: "updated", order: "desc", per_page } }),

  // repos
  repo: (o: string, r: string) => request<Repo>(rp(o, r)),
  forkRepo: (o: string, r: string) => request<Repo>(`${rp(o, r)}/forks`, { method: "POST", body: {} }),
  forks: (o: string, r: string) => request<Repo[]>(`${rp(o, r)}/forks`, { query: { per_page: 100 } }),
  isStarred: async (o: string, r: string) => {
    try {
      await request<void>(`/user/starred/${enc(o)}/${enc(r)}`);
      return true;
    } catch (e) {
      if (e instanceof ApiError && e.status === 404) return false;
      throw e;
    }
  },
  star: (o: string, r: string) => request<void>(`/user/starred/${enc(o)}/${enc(r)}`, { method: "PUT" }),
  unstar: (o: string, r: string) => request<void>(`/user/starred/${enc(o)}/${enc(r)}`, { method: "DELETE" }),
  starred: (u: string, page = 1) => requestPage<Repo[]>(`/users/${enc(u)}/starred`, { query: { per_page: 30, page } }),
  tags: (o: string, r: string, page = 1) => requestPage<Tag[]>(`${rp(o, r)}/tags`, { query: { per_page: 30, page } }),
  branchesPage: (o: string, r: string, page = 1) => requestPage<Branch[]>(`${rp(o, r)}/branches`, { query: { per_page: 30, page } }),
  deleteBranch: (o: string, r: string, b: string) =>
    request<void>(`${rp(o, r)}/git/refs/heads/${encPath(b)}`, { method: "DELETE" }),

  // releases
  releases: (o: string, r: string, page = 1) => requestPage<Release[]>(`${rp(o, r)}/releases`, { query: { per_page: 10, page } }),
  releaseByTag: (o: string, r: string, tag: string) => request<Release>(`${rp(o, r)}/releases/tags/${encPath(tag)}`),
  latestRelease: (o: string, r: string) => request<Release>(`${rp(o, r)}/releases/latest`),
  createRelease: (o: string, r: string, b: { tag_name: string; target_commitish?: string; name?: string; body?: string; draft?: boolean; prerelease?: boolean }) =>
    request<Release>(`${rp(o, r)}/releases`, { method: "POST", body: b }),
  updateRelease: (o: string, r: string, id: number, b: { tag_name?: string; name?: string; body?: string; draft?: boolean; prerelease?: boolean }) =>
    request<Release>(`${rp(o, r)}/releases/${id}`, { method: "PATCH", body: b }),
  deleteRelease: (o: string, r: string, id: number) => request<void>(`${rp(o, r)}/releases/${id}`, { method: "DELETE" }),
  deleteAsset: (o: string, r: string, id: number) => request<void>(`${rp(o, r)}/releases/assets/${id}`, { method: "DELETE" }),
  /** Upload a release asset through the uploads endpoint (`upload_url` template from the release JSON). */
  uploadAsset: async (release: Release, file: File) => {
    const tmpl = release.upload_url.replace(/\{\?[^}]*\}$/, "");
    const u = new URL(tmpl, location.origin);
    // Always upload to this origin (the server may advertise a different FORGE_BASE_URL host).
    const path = u.pathname + "?name=" + encodeURIComponent(file.name);
    const res = await fetch(path, {
      method: "POST",
      headers: {
        Accept: "application/vnd.github+json",
        "Content-Type": file.type || "application/octet-stream",
        "X-Forge-CSRF": "1",
      },
      body: file,
      credentials: "same-origin",
    });
    if (!res.ok) {
      let b: GitHubErrorBody | null = null;
      try {
        b = await res.json();
      } catch {
        /* ignore */
      }
      throw new ApiError(res.status, b, `${res.status} upload failed`);
    }
    return (await res.json()) as ReleaseAsset;
  },

  // reviews
  reviews: (o: string, r: string, n: number) => requestAll<Review>(`${rp(o, r)}/pulls/${n}/reviews`, {}, 5),
  createReview: (o: string, r: string, n: number, b: { event: "APPROVE" | "REQUEST_CHANGES" | "COMMENT"; body?: string }) =>
    request<Review>(`${rp(o, r)}/pulls/${n}/reviews`, { method: "POST", body: b }),

  // ssh keys
  sshKeys: () => requestAll<SshKey>("/user/keys", {}, 3),
  addSshKey: (b: { title: string; key: string }) => request<SshKey>("/user/keys", { method: "POST", body: b }),
  deleteSshKey: (id: number) => request<void>(`/user/keys/${id}`, { method: "DELETE" }),

  // search
  searchRepos: (q: string, page = 1) =>
    requestPage<SearchResult<Repo>>("/search/repositories", { query: { q, per_page: 20, page } }),
  searchIssuesPage: (q: string, page = 1) =>
    requestPage<SearchResult<Issue>>("/search/issues", { query: { q, per_page: 20, page } }),
  searchCode: (q: string, page = 1) =>
    requestPage<SearchResult<CodeSearchItem>>("/search/code", { query: { q, per_page: 20, page }, accept: "application/vnd.github.text-match+json" }),
  createRepo: (b: { name: string; description?: string; private: boolean; auto_init: boolean }) =>
    request<Repo>("/user/repos", { method: "POST", body: b }),
  updateRepo: (o: string, r: string, b: Partial<Pick<Repo, "name" | "description" | "default_branch" | "private" | "homepage">>) =>
    request<Repo>(rp(o, r), { method: "PATCH", body: b }),
  deleteRepo: (o: string, r: string) => request<void>(rp(o, r), { method: "DELETE" }),
  branches: (o: string, r: string) => requestAll<Branch>(`${rp(o, r)}/branches`, {}, 5),

  // contents
  contents: (o: string, r: string, path: string, ref?: string) =>
    request<ContentItem | ContentItem[]>(`${rp(o, r)}/contents${path ? "/" + encPath(path) : ""}`, { query: { ref } }),
  rawFile: (o: string, r: string, path: string, ref?: string) =>
    requestText(`${rp(o, r)}/contents${path ? "/" + encPath(path) : ""}`, { query: { ref }, accept: RAW }),
  readme: (o: string, r: string, ref?: string, dir?: string) =>
    requestText(`${rp(o, r)}/readme${dir ? "/" + encPath(dir) : ""}`, { query: { ref }, accept: RAW }),

  // commits
  commits: (o: string, r: string, sha: string, page = 1, path?: string) =>
    requestPage<Commit[]>(`${rp(o, r)}/commits`, { query: { sha, path, per_page: 30, page } }),
  commit: (o: string, r: string, sha: string) => request<Commit>(`${rp(o, r)}/commits/${enc(sha)}`),
  compare: (o: string, r: string, base: string, head: string) =>
    request<Compare>(`${rp(o, r)}/compare/${enc(base)}...${enc(head)}`),

  // issues
  issues: (o: string, r: string, state: string, page = 1, labels?: string) =>
    requestPage<Issue[]>(`${rp(o, r)}/issues`, { query: { state, labels, per_page: 25, page } }),
  issue: (o: string, r: string, n: number) => request<Issue>(`${rp(o, r)}/issues/${n}`),
  createIssue: (o: string, r: string, b: { title: string; body?: string; labels?: string[] }) =>
    request<Issue>(`${rp(o, r)}/issues`, { method: "POST", body: b }),
  updateIssue: (o: string, r: string, n: number, b: { title?: string; body?: string; state?: string; state_reason?: string }) =>
    request<Issue>(`${rp(o, r)}/issues/${n}`, { method: "PATCH", body: b }),
  issueComments: (o: string, r: string, n: number) =>
    requestAll<IssueComment>(`${rp(o, r)}/issues/${n}/comments`, {}, 10),
  addComment: (o: string, r: string, n: number, body: string) =>
    request<IssueComment>(`${rp(o, r)}/issues/${n}/comments`, { method: "POST", body: { body } }),
  repoLabels: (o: string, r: string) => requestAll<Label>(`${rp(o, r)}/labels`, {}, 3),
  setIssueLabels: (o: string, r: string, n: number, labels: string[]) =>
    request<Label[]>(`${rp(o, r)}/issues/${n}/labels`, { method: "PUT", body: { labels } }),

  // pulls
  pulls: (o: string, r: string, state: string, page = 1) =>
    requestPage<Pull[]>(`${rp(o, r)}/pulls`, { query: { state, per_page: 25, page } }),
  pull: (o: string, r: string, n: number) => request<Pull>(`${rp(o, r)}/pulls/${n}`),
  createPull: (o: string, r: string, b: { title: string; body?: string; head: string; base: string; draft?: boolean }) =>
    request<Pull>(`${rp(o, r)}/pulls`, { method: "POST", body: b }),
  updatePull: (o: string, r: string, n: number, b: { title?: string; body?: string; state?: string }) =>
    request<Pull>(`${rp(o, r)}/pulls/${n}`, { method: "PATCH", body: b }),
  pullFiles: (o: string, r: string, n: number) => requestAll<CommitFile>(`${rp(o, r)}/pulls/${n}/files`, {}, 30),
  pullCommits: (o: string, r: string, n: number) => requestAll<Commit>(`${rp(o, r)}/pulls/${n}/commits`, {}, 3),
  mergePull: (o: string, r: string, n: number, b: { merge_method: "merge" | "squash" | "rebase"; commit_title?: string; commit_message?: string }) =>
    request<{ sha: string; merged: boolean; message: string }>(`${rp(o, r)}/pulls/${n}/merge`, { method: "PUT", body: b }),
};

/** Decode base64 file content from the contents API into a UTF-8 string. */
export function decodeBase64(b64: string): string {
  const bin = atob(b64.replace(/\s/g, ""));
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return new TextDecoder("utf-8", { fatal: false }).decode(bytes);
}

export function looksBinary(s: string): boolean {
  return s.slice(0, 8000).includes("\u0000");
}
