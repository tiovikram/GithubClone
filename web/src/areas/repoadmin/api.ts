// REST helpers for the repoadmin area (repository settings, rulesets, webhooks, insights, …).
import { ApiError, request, requestAll, requestPage, type Repo, type User } from "../../api";

const enc = encodeURIComponent;
const encPath = (p: string) => p.split("/").filter(Boolean).map(enc).join("/");
export const rp = (o: string, r: string) => `/repos/${enc(o)}/${enc(r)}`;
const op = (org: string) => `/orgs/${enc(org)}`;

// ---------- types ----------

export interface RepoFull extends Repo {
  has_projects?: boolean;
  has_wiki?: boolean;
  has_discussions?: boolean;
  is_template?: boolean;
  archived?: boolean;
  allow_merge_commit?: boolean;
  allow_squash_merge?: boolean;
  allow_rebase_merge?: boolean;
  allow_auto_merge?: boolean;
  allow_update_branch?: boolean;
  allow_forking?: boolean;
  delete_branch_on_merge?: boolean;
  web_commit_signoff_required?: boolean;
  squash_merge_commit_title?: string;
  squash_merge_commit_message?: string;
  merge_commit_title?: string;
  merge_commit_message?: string;
  use_squash_pr_title_as_default?: boolean;
  has_pages?: boolean;
  topics?: string[];
  template_repository?: Repo | null;
  organization?: User;
}

export interface Collaborator extends User {
  permissions?: { admin: boolean; maintain?: boolean; push: boolean; triage?: boolean; pull: boolean };
  role_name?: string;
}

export interface Invitation {
  id: number;
  repository: Repo;
  invitee: User | null;
  inviter: User | null;
  permissions: string;
  created_at: string;
  expired?: boolean;
  html_url: string;
}

export interface Team {
  id: number;
  name: string;
  slug: string;
  description?: string | null;
  permission?: string;
  role_name?: string;
  html_url?: string;
  privacy?: string;
}

export interface CustomRole {
  id: number;
  name: string;
  description?: string | null;
  base_role: string;
  permissions: string[];
}

export interface Hook {
  id: number;
  type?: string;
  name: string;
  active: boolean;
  events: string[];
  config: { url?: string; content_type?: string; secret?: string; insecure_ssl?: string | number };
  updated_at: string;
  created_at: string;
  last_response?: { code: number | null; status: string | null; message: string | null };
  ping_url?: string;
  deliveries_url?: string;
}

export interface Delivery {
  id: number;
  guid: string;
  delivered_at: string;
  redelivery: boolean;
  duration: number;
  status: string;
  status_code: number;
  event: string;
  action: string | null;
  installation_id?: number | null;
  repository_id?: number | null;
  url?: string;
  request?: { headers: Record<string, string> | null; payload: unknown };
  response?: { headers: Record<string, string> | null; payload: string | null };
}

export interface DeployKey {
  id: number;
  key: string;
  title: string;
  read_only: boolean;
  verified?: boolean;
  created_at: string;
  added_by?: string | null;
  last_used?: string | null;
}

export interface Autolink {
  id: number;
  key_prefix: string;
  url_template: string;
  is_alphanumeric: boolean;
}

export interface PagesSite {
  url: string;
  status: string | null;
  cname: string | null;
  custom_404?: boolean;
  html_url?: string;
  build_type?: "legacy" | "workflow" | null;
  source?: { branch: string; path: string };
  public?: boolean;
  https_enforced?: boolean;
}

export interface PagesBuild {
  url: string;
  status: string;
  error: { message: string | null };
  pusher: User | null;
  commit: string;
  duration: number;
  created_at: string;
  updated_at: string;
}

export interface PropertyDef {
  property_name: string;
  value_type: "string" | "single_select" | "multi_select" | "true_false" | "url" | string;
  required?: boolean;
  default_value?: string | string[] | null;
  description?: string | null;
  allowed_values?: string[] | null;
  values_editable_by?: "org_actors" | "org_and_repo_actors" | null;
  source_type?: string;
  url?: string;
}

export interface PropertyValue {
  property_name: string;
  value: string | string[] | null;
}

export type BypassMode = "always" | "pull_request" | "exempt";
export interface BypassActor {
  actor_id: number | null;
  actor_type: "Integration" | "OrganizationAdmin" | "RepositoryRole" | "Team" | "DeployKey" | "EnterpriseOwner" | "User" | string;
  bypass_mode: BypassMode | string;
}

export interface Rule {
  type: string;
  parameters?: Record<string, unknown>;
}

export interface RefCondition {
  include: string[];
  exclude: string[];
}

export interface RulesetConditions {
  ref_name?: RefCondition;
  repository_name?: { include: string[]; exclude: string[]; protected?: boolean };
  repository_id?: { repository_ids: number[] };
  repository_property?: {
    include: { name: string; property_values: string[]; source?: string }[];
    exclude: { name: string; property_values: string[]; source?: string }[];
  };
  organization_name?: { include: string[]; exclude: string[] };
}

export interface Ruleset {
  id: number;
  name: string;
  target?: "branch" | "tag" | "push" | string;
  source_type?: string;
  source: string;
  enforcement: "active" | "evaluate" | "disabled" | string;
  bypass_actors?: BypassActor[];
  current_user_can_bypass?: string;
  conditions?: RulesetConditions | null;
  rules?: Rule[];
  created_at?: string;
  updated_at?: string;
  _links?: { html?: { href: string } };
}

export interface RulesetInput {
  name: string;
  target: string;
  enforcement: string;
  bypass_actors: BypassActor[];
  conditions: RulesetConditions | null;
  rules: Rule[];
}

export interface RulesetVersion {
  version_id: number;
  actor: { id: number; type: string } | null;
  updated_at: string;
  state?: unknown;
}

export interface RuleSuite {
  id: number;
  actor_id: number | null;
  actor_name: string | null;
  before_sha: string;
  after_sha: string;
  ref: string;
  repository_id: number;
  repository_name: string;
  pushed_at: string;
  result: "pass" | "fail" | "bypass" | string;
  evaluation_result: "pass" | "fail" | "bypass" | null | string;
  rule_evaluations?: {
    rule_source: { type: string; id: number | null; name: string | null };
    enforcement: string;
    result: string;
    rule_type: string;
    details: string | null;
  }[];
}

export interface BypassRequest {
  id: number;
  number: number;
  repository: { id: number; name: string; full_name: string };
  organization?: { id: number; name: string } | null;
  requester: { actor_id: number; actor_name: string } | null;
  request_type: string;
  data: { ruleset_id: number; ruleset_name: string; total_violations: number; rule_type: string }[] | null;
  resource_identifier: string;
  status: "pending" | "denied" | "approved" | "cancelled" | "completed" | "expired" | string;
  requester_comment: string | null;
  expires_at: string;
  created_at: string;
  responses?: { id: number; reviewer: { actor_id: number; actor_name: string } | null; status: string; created_at: string }[];
  url?: string;
  html_url?: string;
}

export interface Protection {
  url?: string;
  required_status_checks?: { strict: boolean; contexts: string[]; checks?: { context: string; app_id: number | null }[] } | null;
  enforce_admins?: { enabled: boolean };
  required_pull_request_reviews?: {
    dismiss_stale_reviews: boolean;
    require_code_owner_reviews: boolean;
    required_approving_review_count?: number;
    require_last_push_approval?: boolean;
    dismissal_restrictions?: { users: User[]; teams: Team[]; apps?: unknown[] };
    bypass_pull_request_allowances?: { users: User[]; teams: Team[]; apps?: unknown[] };
  } | null;
  restrictions?: { users: User[]; teams: Team[]; apps: { id: number; slug: string; name: string }[] } | null;
  required_linear_history?: { enabled: boolean };
  allow_force_pushes?: { enabled: boolean };
  allow_deletions?: { enabled: boolean };
  block_creations?: { enabled: boolean };
  required_conversation_resolution?: { enabled: boolean };
  lock_branch?: { enabled: boolean };
  allow_fork_syncing?: { enabled: boolean };
  required_signatures?: { enabled: boolean };
}

export interface ProtectionInput {
  required_status_checks: { strict: boolean; contexts: string[] } | null;
  enforce_admins: boolean | null;
  required_pull_request_reviews: {
    dismiss_stale_reviews?: boolean;
    require_code_owner_reviews?: boolean;
    required_approving_review_count?: number;
    require_last_push_approval?: boolean;
    dismissal_restrictions?: { users: string[]; teams: string[] };
    bypass_pull_request_allowances?: { users: string[]; teams: string[] };
  } | null;
  restrictions: { users: string[]; teams: string[]; apps?: string[] } | null;
  required_linear_history?: boolean;
  allow_force_pushes?: boolean | null;
  allow_deletions?: boolean;
  block_creations?: boolean;
  required_conversation_resolution?: boolean;
  lock_branch?: boolean;
  allow_fork_syncing?: boolean;
}

export interface ContributorStat {
  author: User | null;
  total: number;
  weeks: { w: number; a: number; d: number; c: number }[];
}

export interface TrafficCount {
  count: number;
  uniques: number;
  views?: { timestamp: string; count: number; uniques: number }[];
  clones?: { timestamp: string; count: number; uniques: number }[];
}

export interface Referrer {
  referrer: string;
  count: number;
  uniques: number;
}

export interface PopularPath {
  path: string;
  title: string;
  count: number;
  uniques: number;
}

export interface CommunityProfile {
  health_percentage: number;
  description: string | null;
  documentation: string | null;
  files: Record<string, { name?: string; key?: string; url?: string; html_url: string | null } | null>;
  updated_at: string | null;
  content_reports_enabled?: boolean;
}

export interface Org {
  login: string;
  id: number;
  avatar_url: string;
  description?: string | null;
}

// ---------- helpers ----------

/** GET that maps 404 to `null`. */
async function orNull<T>(p: Promise<T>): Promise<T | null> {
  try {
    return await p;
  } catch (e) {
    if (e instanceof ApiError && e.status === 404) return null;
    throw e;
  }
}

/** Stats endpoints answer 202 (computing) with an empty body; retry a few times. */
async function stats<T>(path: string): Promise<T> {
  for (let i = 0; i < 5; i++) {
    const v = await request<T | undefined>(path);
    if (v !== undefined && !(typeof v === "object" && v !== null && !Array.isArray(v) && Object.keys(v).length === 0)) return v as T;
    await new Promise((r) => setTimeout(r, 400 * (i + 1)));
  }
  return [] as unknown as T;
}

// ---------- endpoints ----------

export const ra = {
  repo: (o: string, r: string) => request<RepoFull>(rp(o, r)),
  updateRepo: (o: string, r: string, b: Record<string, unknown>) => request<RepoFull>(rp(o, r), { method: "PATCH", body: b }),
  transfer: (o: string, r: string, b: { new_owner: string; new_name?: string }) =>
    request<RepoFull>(`${rp(o, r)}/transfer`, { method: "POST", body: b }),
  generate: (o: string, r: string, b: { owner?: string; name: string; description?: string; include_all_branches?: boolean; private?: boolean }) =>
    request<RepoFull>(`${rp(o, r)}/generate`, { method: "POST", body: b }),
  enableLfs: (o: string, r: string) => request<void>(`${rp(o, r)}/lfs`, { method: "PUT" }),
  disableLfs: (o: string, r: string) => request<void>(`${rp(o, r)}/lfs`, { method: "DELETE" }),
  topics: (o: string, r: string) => request<{ names: string[] }>(`${rp(o, r)}/topics`),
  setTopics: (o: string, r: string, names: string[]) => request<{ names: string[] }>(`${rp(o, r)}/topics`, { method: "PUT", body: { names } }),
  myOrgs: () => requestAll<Org>("/user/orgs", {}, 3),
  user: (u: string) => request<User>(`/users/${enc(u)}`),
  searchUsers: (q: string) => request<{ items: User[] }>("/search/users", { query: { q, per_page: 8 } }),

  // access
  collaborators: (o: string, r: string, affiliation = "all") =>
    requestAll<Collaborator>(`${rp(o, r)}/collaborators`, { query: { affiliation } }, 5),
  addCollaborator: (o: string, r: string, u: string, permission?: string) =>
    request<Invitation | undefined>(`${rp(o, r)}/collaborators/${enc(u)}`, { method: "PUT", body: permission ? { permission } : {} }),
  removeCollaborator: (o: string, r: string, u: string) => request<void>(`${rp(o, r)}/collaborators/${enc(u)}`, { method: "DELETE" }),
  collaboratorPermission: (o: string, r: string, u: string) =>
    request<{ permission: string; role_name: string; user: Collaborator }>(`${rp(o, r)}/collaborators/${enc(u)}/permission`),
  repoInvitations: (o: string, r: string) => requestAll<Invitation>(`${rp(o, r)}/invitations`, {}, 5),
  updateInvitation: (o: string, r: string, id: number, permissions: string) =>
    request<Invitation>(`${rp(o, r)}/invitations/${id}`, { method: "PATCH", body: { permissions } }),
  deleteInvitation: (o: string, r: string, id: number) => request<void>(`${rp(o, r)}/invitations/${id}`, { method: "DELETE" }),
  myInvitations: () => requestAll<Invitation>("/user/repository_invitations", {}, 5),
  acceptInvitation: (id: number) => request<void>(`/user/repository_invitations/${id}`, { method: "PATCH" }),
  declineInvitation: (id: number) => request<void>(`/user/repository_invitations/${id}`, { method: "DELETE" }),
  repoTeams: (o: string, r: string) => requestAll<Team>(`${rp(o, r)}/teams`, {}, 5),
  orgTeams: (org: string) => requestAll<Team>(`${op(org)}/teams`, {}, 5),
  setTeamRepo: (org: string, slug: string, o: string, r: string, permission: string) =>
    request<void>(`${op(org)}/teams/${enc(slug)}/repos/${enc(o)}/${enc(r)}`, { method: "PUT", body: { permission } }),
  removeTeamRepo: (org: string, slug: string, o: string, r: string) =>
    request<void>(`${op(org)}/teams/${enc(slug)}/repos/${enc(o)}/${enc(r)}`, { method: "DELETE" }),
  customRoles: async (org: string) => {
    const r = await orNull(request<{ total_count: number; custom_roles: CustomRole[] }>(`${op(org)}/custom-repository-roles`));
    return r?.custom_roles || [];
  },

  // branches + protection
  branches: (o: string, r: string) => requestAll<{ name: string; protected: boolean; commit: { sha: string } }>(`${rp(o, r)}/branches`, {}, 5),
  protectedBranches: (o: string, r: string) =>
    requestAll<{ name: string; protected: boolean }>(`${rp(o, r)}/branches`, { query: { protected: true } }, 5),
  protection: (o: string, r: string, b: string) => orNull(request<Protection>(`${rp(o, r)}/branches/${encPath(b)}/protection`)),
  putProtection: (o: string, r: string, b: string, body: ProtectionInput) =>
    request<Protection>(`${rp(o, r)}/branches/${encPath(b)}/protection`, { method: "PUT", body }),
  deleteProtection: (o: string, r: string, b: string) => request<void>(`${rp(o, r)}/branches/${encPath(b)}/protection`, { method: "DELETE" }),
  setSignatures: (o: string, r: string, b: string, on: boolean) =>
    request<unknown>(`${rp(o, r)}/branches/${encPath(b)}/protection/required_signatures`, { method: on ? "POST" : "DELETE" }),
  branchRules: (o: string, r: string, b: string) =>
    request<(Rule & { ruleset_source_type: string; ruleset_source: string; ruleset_id: number })[]>(`${rp(o, r)}/rules/branches/${encPath(b)}`),
  renameBranch: (o: string, r: string, b: string, new_name: string) =>
    request<unknown>(`${rp(o, r)}/branches/${encPath(b)}/rename`, { method: "POST", body: { new_name } }),

  // rulesets
  rulesets: (base: string, includes = true) =>
    requestAll<Ruleset>(`${base}/rulesets`, { query: includes && base.startsWith("/repos/") ? { includes_parents: true } : {} }, 5),
  ruleset: (base: string, id: number) => request<Ruleset>(`${base}/rulesets/${id}`),
  createRuleset: (base: string, b: RulesetInput) => request<Ruleset>(`${base}/rulesets`, { method: "POST", body: b }),
  updateRuleset: (base: string, id: number, b: RulesetInput) => request<Ruleset>(`${base}/rulesets/${id}`, { method: "PUT", body: b }),
  deleteRuleset: (base: string, id: number) => request<void>(`${base}/rulesets/${id}`, { method: "DELETE" }),
  rulesetHistory: (base: string, id: number) => orNull(request<RulesetVersion[]>(`${base}/rulesets/${id}/history`)),
  ruleSuites: (base: string, q: Record<string, string | number | undefined>) =>
    requestPage<RuleSuite[]>(`${base}/rulesets/rule-suites`, { query: { per_page: 30, ...q } }),
  ruleSuite: (base: string, id: number) => request<RuleSuite>(`${base}/rulesets/rule-suites/${id}`),
  pushBypassRequests: (base: string, status = "all") =>
    requestAll<BypassRequest>(`${base}/bypass-requests/push-rules`, { query: { request_status: status, time_period: "month" } }, 3),
  pushBypassRequest: (o: string, r: string, n: number) => request<BypassRequest>(`${rp(o, r)}/bypass-requests/push-rules/${n}`),
  reviewPushBypass: (o: string, r: string, n: number, status: "approve" | "deny", message?: string) =>
    request<BypassRequest>(`${rp(o, r)}/bypass-requests/push-rules/${n}`, { method: "PATCH", body: { status, message } }),
  cancelPushBypass: (o: string, r: string, n: number) => request<void>(`${rp(o, r)}/bypass-requests/push-rules/${n}`, { method: "DELETE" }),
  deployKeysForBypass: (o: string, r: string) => requestAll<DeployKey>(`${rp(o, r)}/keys`, {}, 3),
  installations: (org: string) =>
    orNull(request<{ installations: { id: number; app_id: number; app_slug: string }[] }>(`${op(org)}/installations`)),

  // webhooks (base = /repos/o/r or /orgs/org)
  hooks: (base: string) => requestAll<Hook>(`${base}/hooks`, {}, 5),
  hook: (base: string, id: number) => request<Hook>(`${base}/hooks/${id}`),
  createHook: (base: string, b: Record<string, unknown>) => request<Hook>(`${base}/hooks`, { method: "POST", body: b }),
  updateHook: (base: string, id: number, b: Record<string, unknown>) => request<Hook>(`${base}/hooks/${id}`, { method: "PATCH", body: b }),
  deleteHook: (base: string, id: number) => request<void>(`${base}/hooks/${id}`, { method: "DELETE" }),
  pingHook: (base: string, id: number) => request<void>(`${base}/hooks/${id}/pings`, { method: "POST" }),
  testHook: (base: string, id: number) => request<void>(`${base}/hooks/${id}/tests`, { method: "POST" }),
  deliveries: (base: string, id: number) => request<Delivery[]>(`${base}/hooks/${id}/deliveries`, { query: { per_page: 50 } }),
  delivery: (base: string, id: number, d: number) => request<Delivery>(`${base}/hooks/${id}/deliveries/${d}`),
  redeliver: (base: string, id: number, d: number) => request<unknown>(`${base}/hooks/${id}/deliveries/${d}/attempts`, { method: "POST" }),

  // deploy keys + autolinks
  deployKeys: (o: string, r: string) => requestAll<DeployKey>(`${rp(o, r)}/keys`, {}, 3),
  addDeployKey: (o: string, r: string, b: { title: string; key: string; read_only: boolean }) =>
    request<DeployKey>(`${rp(o, r)}/keys`, { method: "POST", body: b }),
  deleteDeployKey: (o: string, r: string, id: number) => request<void>(`${rp(o, r)}/keys/${id}`, { method: "DELETE" }),
  autolinks: (o: string, r: string) => requestAll<Autolink>(`${rp(o, r)}/autolinks`, {}, 3),
  addAutolink: (o: string, r: string, b: { key_prefix: string; url_template: string; is_alphanumeric: boolean }) =>
    request<Autolink>(`${rp(o, r)}/autolinks`, { method: "POST", body: b }),
  deleteAutolink: (o: string, r: string, id: number) => request<void>(`${rp(o, r)}/autolinks/${id}`, { method: "DELETE" }),

  // pages
  pages: (o: string, r: string) => orNull(request<PagesSite>(`${rp(o, r)}/pages`)),
  createPages: (o: string, r: string, b: { build_type: string; source?: { branch: string; path: string } }) =>
    request<PagesSite>(`${rp(o, r)}/pages`, { method: "POST", body: b }),
  updatePages: (o: string, r: string, b: Record<string, unknown>) => request<void>(`${rp(o, r)}/pages`, { method: "PUT", body: b }),
  deletePages: (o: string, r: string) => request<void>(`${rp(o, r)}/pages`, { method: "DELETE" }),
  pagesBuilds: (o: string, r: string) => request<PagesBuild[]>(`${rp(o, r)}/pages/builds`, { query: { per_page: 10 } }),
  requestPagesBuild: (o: string, r: string) => request<{ url: string; status: string }>(`${rp(o, r)}/pages/builds`, { method: "POST" }),

  // custom properties
  repoProperties: (o: string, r: string) => request<PropertyValue[]>(`${rp(o, r)}/properties/values`),
  setRepoProperties: (o: string, r: string, properties: PropertyValue[]) =>
    request<void>(`${rp(o, r)}/properties/values`, { method: "PATCH", body: { properties } }),
  /** base = /orgs/{org} or /enterprises/{e} */
  propertySchema: (base: string) => request<PropertyDef[]>(`${base}/properties/schema`),
  putProperty: (base: string, name: string, b: Omit<PropertyDef, "property_name">) =>
    request<PropertyDef>(`${base}/properties/schema/${enc(name)}`, { method: "PUT", body: b }),
  deleteProperty: (base: string, name: string) => request<void>(`${base}/properties/schema/${enc(name)}`, { method: "DELETE" }),
  orgRepoValues: (org: string) =>
    requestAll<{ repository_id: number; repository_name: string; repository_full_name: string; properties: PropertyValue[] }>(`${op(org)}/properties/values`, {}, 3),
  setOrgRepoValues: (org: string, names: string[], properties: PropertyValue[]) =>
    request<void>(`${op(org)}/properties/values`, { method: "PATCH", body: { repository_names: names, properties } }),

  // insights
  contributors: (o: string, r: string) => stats<ContributorStat[]>(`${rp(o, r)}/stats/contributors`),
  commitActivity: (o: string, r: string) => stats<{ days: number[]; total: number; week: number }[]>(`${rp(o, r)}/stats/commit_activity`),
  codeFrequency: (o: string, r: string) => stats<[number, number, number][]>(`${rp(o, r)}/stats/code_frequency`),
  participation: (o: string, r: string) => stats<{ all: number[]; owner: number[] }>(`${rp(o, r)}/stats/participation`),
  punchCard: (o: string, r: string) => stats<[number, number, number][]>(`${rp(o, r)}/stats/punch_card`),
  views: (o: string, r: string, per = "day") => request<TrafficCount>(`${rp(o, r)}/traffic/views`, { query: { per } }),
  clones: (o: string, r: string, per = "day") => request<TrafficCount>(`${rp(o, r)}/traffic/clones`, { query: { per } }),
  referrers: (o: string, r: string) => request<Referrer[]>(`${rp(o, r)}/traffic/popular/referrers`),
  paths: (o: string, r: string) => request<PopularPath[]>(`${rp(o, r)}/traffic/popular/paths`),
  forks: (o: string, r: string, sort = "newest") => requestAll<Repo>(`${rp(o, r)}/forks`, { query: { sort } }, 5),
  community: (o: string, r: string) => request<CommunityProfile>(`${rp(o, r)}/community/profile`),
  pulls: (o: string, r: string, state: string) =>
    request<{ number: number; title: string; state: string; merged_at: string | null; created_at: string; closed_at: string | null; user: User }[]>(
      `${rp(o, r)}/pulls`, { query: { state, sort: "updated", direction: "desc", per_page: 100 } }),
  issues: (o: string, r: string, state: string, since: string) =>
    request<{ number: number; title: string; state: string; created_at: string; closed_at: string | null; pull_request?: unknown; user: User }[]>(
      `${rp(o, r)}/issues`, { query: { state, since, per_page: 100 } }),
  commitsSince: (o: string, r: string, since: string) =>
    requestAll<{ sha: string; author: User | null; commit: { author: { name: string; date: string } | null; message: string } }>(
      `${rp(o, r)}/commits`, { query: { since } }, 3),
};
