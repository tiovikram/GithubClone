// Endpoint helpers for the social area (notifications, watching, follows, events, gists,
// account settings). Shapes follow api.github.com; forge-only endpoints live under /api/forge.
import { ApiError, FORGE, request, requestAll, requestPage, type Repo, type User } from "../../api";

const enc = encodeURIComponent;
const rp = (o: string, r: string) => `/repos/${enc(o)}/${enc(r)}`;

export interface Me extends User {
  twitter_username?: string | null;
  hireable?: boolean | null;
  public_gists?: number;
  private_gists?: number;
  updated_at?: string;
}

export interface Thread {
  id: string;
  repository: Repo;
  subject: { title: string; url: string | null; latest_comment_url: string | null; type: string };
  reason: string;
  unread: boolean;
  updated_at: string;
  last_read_at: string | null;
  url: string;
}

export interface WatchState {
  state: "watching" | "not_watching" | "ignoring" | "custom";
  events: string[];
  subscribers_count: number;
}

export interface IssueSub {
  subscribed: boolean;
  reason: string;
}

export interface Event {
  id: string;
  type: string;
  actor: { login: string; avatar_url: string; display_login?: string };
  repo: { name: string; url: string } | null;
  payload: Record<string, any>; // eslint-disable-line @typescript-eslint/no-explicit-any
  public: boolean;
  created_at: string;
}

export interface GistFile {
  filename: string;
  type?: string;
  language: string | null;
  raw_url: string;
  size: number;
  truncated?: boolean;
  content?: string;
}

export interface GistHistory {
  version: string;
  committed_at: string;
  change_status: { total: number; additions: number; deletions: number };
  user: User | null;
  url: string;
}

export interface Gist {
  id: string;
  description: string | null;
  public: boolean;
  owner: User;
  files: Record<string, GistFile | null>;
  created_at: string;
  updated_at: string;
  comments: number;
  html_url: string;
  git_pull_url: string;
  history?: GistHistory[];
  forks?: { id: string; user: User; created_at: string; url: string }[];
  fork_of?: Gist | null;
}

export interface GistComment {
  id: number;
  body: string;
  user: User;
  created_at: string;
  updated_at: string;
}

export interface Email {
  email: string;
  primary: boolean;
  verified: boolean;
  visibility: string | null;
}

export interface GpgKey {
  id: number;
  name?: string | null;
  key_id: string;
  emails: { email: string; verified: boolean }[];
  created_at: string;
  expires_at: string | null;
  can_sign: boolean;
  subkeys: { key_id: string }[];
}

export interface SigningKey {
  id: number;
  key: string;
  title: string;
  created_at: string;
}

export interface SocialAccount {
  provider: string;
  url: string;
}

export interface Migration {
  id: number;
  state: string;
  created_at: string;
  updated_at: string;
  repositories: Repo[];
  url: string;
  archive_url?: string;
}

/** Resolve 404 to false (GitHub's "check" endpoints answer 204 / 404). */
async function check(path: string): Promise<boolean> {
  try {
    await request<void>(path);
    return true;
  } catch (e) {
    if (e instanceof ApiError && e.status === 404) return false;
    throw e;
  }
}

export const sapi = {
  me: () => request<Me>("/user"),
  updateMe: (b: Partial<Pick<Me, "name" | "email" | "bio" | "company" | "location" | "blog" | "twitter_username" | "hireable">>) =>
    request<Me>("/user", { method: "PATCH", body: b }),

  // notifications
  notifications: (q: { all?: boolean; participating?: boolean; page?: number }) =>
    requestPage<Thread[]>("/notifications", { query: { all: q.all, participating: q.participating, page: q.page, per_page: 50 } }),
  repoNotifications: (o: string, r: string, q: { all?: boolean; participating?: boolean; page?: number }) =>
    requestPage<Thread[]>(`${rp(o, r)}/notifications`, { query: { all: q.all, participating: q.participating, page: q.page, per_page: 50 } }),
  unreadCount: async () => (await request<Thread[]>("/notifications", { query: { per_page: 50 } })).length,
  markThreadRead: (id: string) => request<void>(`/notifications/threads/${enc(id)}`, { method: "PATCH" }),
  markThreadDone: (id: string) => request<void>(`/notifications/threads/${enc(id)}`, { method: "DELETE" }),
  markAllRead: () => request<void>("/notifications", { method: "PUT", body: { read: true } }),
  markRepoRead: (o: string, r: string) => request<void>(`${rp(o, r)}/notifications`, { method: "PUT", body: { read: true } }),
  unsubscribeThread: (id: string) => request<unknown>(`/notifications/threads/${enc(id)}/subscription`, { method: "PUT", body: { ignored: true } }),

  // watching
  watchState: (o: string, r: string) => request<WatchState>(`${FORGE}${rp(o, r)}/subscription`),
  setWatch: (o: string, r: string, state: WatchState["state"], events: string[] = []) =>
    request<WatchState>(`${FORGE}${rp(o, r)}/subscription`, { method: "PUT", body: { state, events } }),
  watched: (page = 1) => requestPage<Repo[]>("/user/subscriptions", { query: { per_page: 30, page } }),
  unwatch: (o: string, r: string) => request<void>(`${rp(o, r)}/subscription`, { method: "DELETE" }),
  stargazers: (o: string, r: string, page = 1) => requestPage<User[]>(`${rp(o, r)}/stargazers`, { query: { per_page: 30, page } }),
  watchers: (o: string, r: string, page = 1) => requestPage<User[]>(`${rp(o, r)}/subscribers`, { query: { per_page: 30, page } }),

  // issue / PR subscriptions
  issueSub: (o: string, r: string, n: number) => request<IssueSub>(`${FORGE}${rp(o, r)}/issues/${n}/subscription`),
  setIssueSub: (o: string, r: string, n: number, on: boolean) =>
    request<IssueSub>(`${FORGE}${rp(o, r)}/issues/${n}/subscription`, { method: on ? "PUT" : "DELETE" }),

  // follows
  isFollowing: (u: string) => check(`/user/following/${enc(u)}`),
  follow: (u: string) => request<void>(`/user/following/${enc(u)}`, { method: "PUT" }),
  unfollow: (u: string) => request<void>(`/user/following/${enc(u)}`, { method: "DELETE" }),
  followers: (u: string, page = 1) => requestPage<User[]>(`/users/${enc(u)}/followers`, { query: { per_page: 30, page } }),
  following: (u: string, page = 1) => requestPage<User[]>(`/users/${enc(u)}/following`, { query: { per_page: 30, page } }),

  // events
  receivedEvents: (u: string, page = 1) => requestPage<Event[]>(`/users/${enc(u)}/received_events`, { query: { per_page: 30, page } }),
  userEvents: (u: string, page = 1) => requestPage<Event[]>(`/users/${enc(u)}/events`, { query: { per_page: 30, page } }),
  publicEvents: () => request<Event[]>("/events", { query: { per_page: 30 } }),

  // gists
  myGists: (page = 1) => requestPage<Gist[]>("/gists", { query: { per_page: 30, page } }),
  userGists: (u: string, page = 1) => requestPage<Gist[]>(`/users/${enc(u)}/gists`, { query: { per_page: 30, page } }),
  starredGists: (page = 1) => requestPage<Gist[]>("/gists/starred", { query: { per_page: 30, page } }),
  publicGists: (page = 1) => requestPage<Gist[]>("/gists/public", { query: { per_page: 30, page } }),
  gist: (id: string) => request<Gist>(`/gists/${enc(id)}`),
  gistRevision: (id: string, sha: string) => request<Gist>(`/gists/${enc(id)}/${enc(sha)}`),
  createGist: (b: { description: string; public: boolean; files: Record<string, { content: string }> }) =>
    request<Gist>("/gists", { method: "POST", body: b }),
  updateGist: (id: string, b: { description?: string; files?: Record<string, { content?: string; filename?: string } | null> }) =>
    request<Gist>(`/gists/${enc(id)}`, { method: "PATCH", body: b }),
  deleteGist: (id: string) => request<void>(`/gists/${enc(id)}`, { method: "DELETE" }),
  forkGist: (id: string) => request<Gist>(`/gists/${enc(id)}/forks`, { method: "POST" }),
  gistForks: (id: string) => request<Gist[]>(`/gists/${enc(id)}/forks`, { query: { per_page: 100 } }),
  gistStarred: (id: string) => check(`/gists/${enc(id)}/star`),
  starGist: (id: string) => request<void>(`/gists/${enc(id)}/star`, { method: "PUT" }),
  unstarGist: (id: string) => request<void>(`/gists/${enc(id)}/star`, { method: "DELETE" }),
  gistCommits: (id: string) => request<GistHistory[]>(`/gists/${enc(id)}/commits`, { query: { per_page: 100 } }),
  gistComments: (id: string) => requestAll<GistComment>(`/gists/${enc(id)}/comments`, {}, 5),
  addGistComment: (id: string, body: string) => request<GistComment>(`/gists/${enc(id)}/comments`, { method: "POST", body: { body } }),
  editGistComment: (id: string, cid: number, body: string) =>
    request<GistComment>(`/gists/${enc(id)}/comments/${cid}`, { method: "PATCH", body: { body } }),
  deleteGistComment: (id: string, cid: number) => request<void>(`/gists/${enc(id)}/comments/${cid}`, { method: "DELETE" }),

  // emails
  emails: () => request<Email[]>("/user/emails"),
  addEmail: (email: string) => request<Email[]>("/user/emails", { method: "POST", body: { emails: [email] } }),
  deleteEmail: (email: string) => request<void>("/user/emails", { method: "DELETE", body: { emails: [email] } }),

  // keys
  gpgKeys: () => requestAll<GpgKey>("/user/gpg_keys", {}, 3),
  addGpgKey: (b: { name?: string; armored_public_key: string }) => request<GpgKey>("/user/gpg_keys", { method: "POST", body: b }),
  deleteGpgKey: (id: number) => request<void>(`/user/gpg_keys/${id}`, { method: "DELETE" }),
  signingKeys: () => requestAll<SigningKey>("/user/ssh_signing_keys", {}, 3),
  addSigningKey: (b: { title: string; key: string }) => request<SigningKey>("/user/ssh_signing_keys", { method: "POST", body: b }),
  deleteSigningKey: (id: number) => request<void>(`/user/ssh_signing_keys/${id}`, { method: "DELETE" }),

  // social accounts
  socialAccounts: () => request<SocialAccount[]>("/user/social_accounts"),
  userSocialAccounts: (u: string) => request<SocialAccount[]>(`/users/${enc(u)}/social_accounts`),
  addSocialAccount: (url: string) => request<SocialAccount[]>("/user/social_accounts", { method: "POST", body: { account_urls: [url] } }),
  deleteSocialAccount: (url: string) => request<void>("/user/social_accounts", { method: "DELETE", body: { account_urls: [url] } }),

  // blocks
  blocks: () => requestAll<User>("/user/blocks", {}, 3),
  isBlocked: (u: string) => check(`/user/blocks/${enc(u)}`),
  block: (u: string) => request<void>(`/user/blocks/${enc(u)}`, { method: "PUT" }),
  unblock: (u: string) => request<void>(`/user/blocks/${enc(u)}`, { method: "DELETE" }),

  // account
  changePassword: (current_password: string, password: string) =>
    request<void>(`${FORGE}/user/password`, { method: "PUT", body: { current_password, password } }),
  rename: (login: string) => request<Me>(`${FORGE}/user/login`, { method: "PATCH", body: { login } }),
  deleteAccount: (login: string, password: string) => request<void>(`${FORGE}/user`, { method: "DELETE", body: { login, password } }),
  migrations: () => request<Migration[]>("/user/migrations", { query: { per_page: 20 } }),
  startMigration: (repositories: string[]) => request<Migration>("/user/migrations", { method: "POST", body: { repositories } }),
  myRepos: () => requestAll<Repo>("/user/repos", { query: { affiliation: "owner", per_page: 100 } }, 3),

  // explore / search
  searchUsers: (q: string, page = 1) =>
    requestPage<{ total_count: number; items: User[] }>("/search/users", { query: { q, per_page: 20, page } }),
  searchRepos: (q: string, sort?: string) =>
    request<{ total_count: number; items: Repo[] }>("/search/repositories", { query: { q, sort, order: "desc", per_page: 20 } }),
  user: (u: string) => request<Me>(`/users/${enc(u)}`),
};
