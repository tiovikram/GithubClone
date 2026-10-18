// Site administration endpoints: GHES enterprise-admin REST API (/api/v3/admin, /enterprise,
// /enterprises/{e}, /manage/v1, /scim/v2) plus forge's admin listings under /api/forge/admin.
import { FORGE, request, requestPage, type Page } from "../../api";

const enc = encodeURIComponent;

// ---------- types ----------

export interface AdminInfo {
  enterprise_slug: string;
  forge_version: string;
  ghes_api_version: string;
}

export interface MaintenanceState {
  status: "off" | "scheduled" | "on";
  active: boolean;
  message: string | null;
  scheduled_time: string | null;
  bypass: boolean;
}

export interface AdminUser {
  id: number;
  login: string;
  name: string | null;
  email: string | null;
  avatar_url: string;
  html_url: string;
  site_admin: boolean;
  created_at: string;
  suspended_at: string | null;
  suspension_reason: string | null;
  has_password: boolean;
  repos: number;
  ldap_dn: string | null;
}

export interface AdminToken {
  id: number;
  name: string;
  token_last_eight: string;
  scopes: string[];
  impersonation: boolean;
  created_at: string;
  last_used_at: string | null;
  expires_at: string | null;
}

export interface AdminUserDetail extends AdminUser {
  organizations: { login: string; role: string }[];
  repositories: { name: string; full_name: string; private: boolean; archived: boolean; pushed_at: string | null }[];
  keys: { id: number; title: string; key: string; fingerprint: string; created_at: string }[];
  emails: string[];
  tokens: AdminToken[];
}

export interface AdminOrg {
  id: number;
  login: string;
  name: string | null;
  avatar_url: string;
  created_at: string;
  members: number;
  owners: number;
  repos: number;
  teams: number;
}

export interface AdminRepo {
  id: number;
  name: string;
  full_name: string;
  owner: { login: string; type: string };
  description: string | null;
  private: boolean;
  visibility: string;
  archived: boolean;
  fork: boolean;
  size: number;
  created_at: string;
  pushed_at: string | null;
}

export interface Listing<T> {
  total_count: number;
  items: T[];
}

export type Stats = Record<string, Record<string, number>>;

export interface GlobalHook {
  id: number;
  name: string;
  active: boolean;
  events: string[];
  config: { url?: string; content_type?: string; insecure_ssl?: string; secret?: string };
  created_at: string;
  updated_at: string;
}

export interface HookInput {
  config: { url: string; content_type: string; insecure_ssl: string; secret?: string };
  events: string[];
  active: boolean;
}

export interface PreReceiveEnv {
  id: number;
  name: string;
  image_url: string;
  default_environment: boolean;
  created_at: string;
  hooks_count: number;
  download: { state: string; downloaded_at: string | null; message: string | null };
}

export type Enforcement = "enabled" | "disabled" | "testing";

export interface PreReceiveHook {
  id: number;
  name: string;
  enforcement: Enforcement;
  script: string;
  script_repository: { id: number; full_name: string; html_url: string };
  environment: PreReceiveEnv;
  allow_downstream_configuration: boolean;
}

export interface PreReceiveHookInput {
  name?: string;
  script?: string;
  script_repository?: { full_name: string };
  environment?: { id: number };
  enforcement?: Enforcement;
  allow_downstream_configuration?: boolean;
}

/** Org- or repository-level view of a pre-receive hook. */
export interface ScopedPreReceiveHook {
  id: number;
  name: string;
  enforcement: Enforcement;
  configuration_url: string;
  allow_downstream_configuration?: boolean;
}

export interface Announcement {
  announcement: string | null;
  expires_at: string | null;
  user_dismissible: boolean;
}

export interface ManageMaintenance {
  hostname: string;
  uuid: string;
  status: string;
  ip_exception_list: string[];
  maintenance_mode_message: string;
  scheduled_time?: string;
}

export type ManageSettings = Record<string, unknown> & {
  private_mode?: boolean;
  public_pages?: boolean;
  subdomain_isolation?: boolean;
  signup_enabled?: boolean;
  github_hostname?: string;
  identicons_host?: string;
  auth_mode?: string;
  expire_sessions?: boolean;
  configuration_run_count?: number;
  smtp?: Record<string, unknown>;
  pages?: { enabled?: boolean };
  repository_invitations?: { enabled?: boolean };
};

export interface NodeHealth {
  status: string;
  nodes: { hostname: string; status: string; services: { status: string; name: string; details: string }[] }[];
}

export interface ManageNodes {
  topology: string;
  nodes: { hostname: string; uuid: string; replica: boolean; cluster_roles: string[] }[];
}

export interface ManageVersion {
  hostname: string;
  version: { version: string; platform: string; build_id: string; build_date: string };
}

export interface ApplyStatus {
  running: boolean;
  successful: boolean;
  nodes: { run_id: string; hostname: string; running: boolean; successful: boolean }[];
}

export interface ApplyEvents {
  nodes: { node: string; last_request_id: string; events: { timestamp: string; severity_text: string; body: string; event_name: string }[] }[];
}

export interface EnterpriseLicense {
  seats: number | string;
  seats_used: number;
  seats_available: number | string;
  kind: string;
  days_until_expiration: number;
  expire_at: string;
}

export type ManageLicense = Record<string, string | number | boolean>;

export interface AuditEvent {
  "@timestamp": number;
  _document_id: string;
  action: string;
  actor?: string;
  user?: string;
  org?: string;
  repo?: string;
  team?: string;
  actor_ip?: string;
  created_at: number;
  data?: Record<string, unknown>;
}

export interface PublicKey {
  id: number;
  key: string;
  user_id: number | null;
  repository_id: number | null;
  title: string;
  read_only: boolean;
  created_at: string;
  added_by: string | null;
  last_used: string | null;
}

export interface Authorization {
  id: number;
  token_last_eight: string;
  scopes: string[];
  note: string;
  token: string;
  created_at: string;
  updated_at: string;
  expires_at: string | null;
  user: { login: string };
}

export interface LiveMigration {
  migration_id: string;
  status: string;
  source_organization_login: string;
  source_repository_name: string;
  target_organization_login: string;
  target_repository_name: string;
  target_visibility: string | null;
  target_api_endpoint: string;
  created_at: string;
  started_at: string | null;
  completed_at: string | null;
  expires_at: string | null;
}

export interface EntTeam {
  id: number;
  name: string;
  slug: string;
  description: string;
  sync_to_organizations: string;
  organization_selection_type: string;
  group_id: string | null;
  group_name: string | null;
  notification_setting: string;
  created_at: string;
  updated_at: string;
}

export interface MiniUser {
  login: string;
  id: number;
  avatar_url: string;
  site_admin?: boolean;
}

export interface ScimList<T> {
  totalResults: number;
  itemsPerPage?: number;
  startIndex?: number;
  Resources: T[];
}

export interface ScimUser {
  id: string;
  externalId: string | null;
  active: boolean;
  userName: string;
  displayName: string | null;
  name?: { givenName?: string; familyName?: string; formatted?: string };
  emails: { value: string; primary?: boolean; type?: string }[];
  roles: { value: string; primary?: boolean }[];
  groups?: { value: string; display: string }[];
  meta?: { created?: string; lastModified?: string; resourceType?: string };
}

export interface ScimGroup {
  id: string;
  externalId: string | null;
  displayName: string;
  members: { value: string; display?: string }[];
  meta?: { created?: string; lastModified?: string };
}

const SCIM = "application/scim+json";

// ---------- endpoints ----------

const e = (ent: string) => `/enterprises/${enc(ent)}`;
const scim = (ent: string) => `/scim/v2/enterprises/${enc(ent)}`;

export const adminApi = {
  // forge-only
  info: () => request<AdminInfo>(`${FORGE}/admin/info`),
  maintenanceState: () => request<MaintenanceState>(`${FORGE}/maintenance`),
  users: (q: string, filter: string, page: number) =>
    request<Listing<AdminUser>>(`${FORGE}/admin/users`, { query: { q, filter, page, per_page: 30 } }),
  user: (login: string) => request<AdminUserDetail>(`${FORGE}/admin/users/${enc(login)}`),
  setPassword: (login: string, password: string) =>
    request<void>(`${FORGE}/admin/users/${enc(login)}/password`, { method: "PUT", body: { password } }),
  orgs: (q: string, page: number) => request<Listing<AdminOrg>>(`${FORGE}/admin/orgs`, { query: { q, page, per_page: 30 } }),
  repos: (q: string, filter: string, page: number) =>
    request<Listing<AdminRepo>>(`${FORGE}/admin/repos`, { query: { q, filter, page, per_page: 30 } }),

  // stats, license
  stats: () => request<Stats>("/enterprise/stats/all"),
  license: () => request<EnterpriseLicense>("/enterprise/settings/license"),

  // users
  createUser: (b: { login: string; email?: string; suspended?: boolean }) => request<MiniUser>("/admin/users", { method: "POST", body: b }),
  renameUser: (login: string, newLogin: string) => request<{ message: string }>(`/admin/users/${enc(login)}`, { method: "PATCH", body: { login: newLogin } }),
  deleteUser: (login: string) => request<void>(`/admin/users/${enc(login)}`, { method: "DELETE" }),
  promote: (login: string) => request<void>(`/users/${enc(login)}/site_admin`, { method: "PUT" }),
  demote: (login: string) => request<void>(`/users/${enc(login)}/site_admin`, { method: "DELETE" }),
  suspend: (login: string, reason: string) => request<void>(`/users/${enc(login)}/suspended`, { method: "PUT", body: { reason } }),
  unsuspend: (login: string) => request<void>(`/users/${enc(login)}/suspended`, { method: "DELETE", body: {} }),
  impersonate: (login: string, scopes: string[]) =>
    request<Authorization>(`/admin/users/${enc(login)}/authorizations`, { method: "POST", body: { scopes } }),
  deleteImpersonation: (login: string) => request<void>(`/admin/users/${enc(login)}/authorizations`, { method: "DELETE" }),
  tokens: (page: number) => requestPage<Authorization[]>("/admin/tokens", { query: { page, per_page: 50 } }),
  deleteToken: (id: number) => request<void>(`/admin/tokens/${id}`, { method: "DELETE" }),
  keys: (page: number) => requestPage<PublicKey[]>("/admin/keys", { query: { page, per_page: 50 } }),
  deleteKey: (id: number) => request<void>(`/admin/keys/${id}`, { method: "DELETE" }),

  // ldap
  ldapUserMapping: (login: string, ldap_dn: string) =>
    request<unknown>(`/admin/ldap/users/${enc(login)}/mapping`, { method: "PATCH", body: { ldap_dn } }),
  ldapUserSync: (login: string) => request<{ status: string }>(`/admin/ldap/users/${enc(login)}/sync`, { method: "POST" }),
  ldapTeamMapping: (id: number, ldap_dn: string) =>
    request<{ ldap_dn: string; name: string }>(`/admin/ldap/teams/${id}/mapping`, { method: "PATCH", body: { ldap_dn } }),
  ldapTeamSync: (id: number) => request<{ status: string }>(`/admin/ldap/teams/${id}/sync`, { method: "POST" }),
  orgTeam: (org: string, slug: string) => request<{ id: number; name: string; slug: string }>(`/orgs/${enc(org)}/teams/${enc(slug)}`),

  // organizations
  createOrg: (b: { login: string; admin: string; profile_name?: string }) => request<MiniUser>("/admin/organizations", { method: "POST", body: b }),
  renameOrg: (org: string, login: string) => request<{ message: string }>(`/admin/organizations/${enc(org)}`, { method: "PATCH", body: { login } }),

  // repositories
  updateRepo: (full: string, b: { archived?: boolean; private?: boolean }) => request<unknown>(`/repos/${full}`, { method: "PATCH", body: b }),
  deleteRepo: (full: string) => request<void>(`/repos/${full}`, { method: "DELETE" }),

  // announcement
  announcement: () => request<Announcement>("/enterprise/announcement"),
  setAnnouncement: (b: { announcement: string; expires_at: string | null; user_dismissible: boolean }) =>
    request<Announcement>("/enterprise/announcement", { method: "PATCH", body: b }),
  removeAnnouncement: () => request<void>("/enterprise/announcement", { method: "DELETE" }),

  // management console
  maintenance: () => request<ManageMaintenance[]>("/manage/v1/maintenance"),
  setMaintenance: (b: { enabled: boolean; when?: string; maintenance_mode_message?: string; ip_exception_list?: string[] }) =>
    request<unknown>("/manage/v1/maintenance", { method: "POST", body: b }),
  settings: () => request<ManageSettings>("/manage/v1/config/settings"),
  putSettings: (b: Record<string, unknown>) => request<void>("/manage/v1/config/settings", { method: "PUT", body: b }),
  apply: () => request<{ run_id: string }>("/manage/v1/config/apply", { method: "POST", body: {} }),
  applyStatus: () => request<ApplyStatus>("/manage/v1/config/apply"),
  applyEvents: () => request<ApplyEvents>("/manage/v1/config/apply/events"),
  nodes: () => request<ManageNodes>("/manage/v1/config/nodes"),
  clusterStatus: () => request<NodeHealth>("/manage/v1/cluster/status"),
  replicationStatus: () => request<NodeHealth>("/manage/v1/replication/status"),
  version: () => request<ManageVersion[]>("/manage/v1/version"),
  systemRequirements: () => request<{ status: string; nodes: { hostname: string; status: string; roles_status: { status: string; role: string }[] }[] }>("/manage/v1/checks/system-requirements"),
  upgradeStatus: () => request<{ hostname: string; status: string; messages: string[]; release_version: string }[]>("/manage/v1/upgrade/status"),
  manageLicense: () => request<ManageLicense>("/manage/v1/config/license"),
  licenseCheck: () => request<{ status: string }>("/manage/v1/config/license/check"),
  uploadLicense: (license: string) => request<void>("/manage/v1/config/license", { method: "PUT", body: { license } }),
  sshAccess: () => request<{ key: string; fingerprint: string }[]>("/manage/v1/access/ssh"),
  addSshAccess: (key: string) => request<{ message: string; modified: boolean }[]>("/manage/v1/access/ssh", { method: "POST", body: { key } }),
  removeSshAccess: (key: string) => request<{ message: string }[]>("/manage/v1/access/ssh", { method: "DELETE", body: { key } }),

  // global webhooks
  hooks: () => request<GlobalHook[]>("/admin/hooks", { query: { per_page: 100 } }),
  hook: (id: number) => request<GlobalHook>(`/admin/hooks/${id}`),
  createHook: (b: HookInput) => request<GlobalHook>("/admin/hooks", { method: "POST", body: { name: "web", ...b } }),
  updateHook: (id: number, b: HookInput) => request<GlobalHook>(`/admin/hooks/${id}`, { method: "PATCH", body: b }),
  deleteHook: (id: number) => request<void>(`/admin/hooks/${id}`, { method: "DELETE" }),
  pingHook: (id: number) => request<void>(`/admin/hooks/${id}/pings`, { method: "POST" }),

  // pre-receive
  envs: () => request<PreReceiveEnv[]>("/admin/pre-receive-environments", { query: { per_page: 100, sort: "name" } }),
  createEnv: (b: { name: string; image_url: string }) => request<PreReceiveEnv>("/admin/pre-receive-environments", { method: "POST", body: b }),
  updateEnv: (id: number, b: { name?: string; image_url?: string }) =>
    request<PreReceiveEnv>(`/admin/pre-receive-environments/${id}`, { method: "PATCH", body: b }),
  deleteEnv: (id: number) => request<void>(`/admin/pre-receive-environments/${id}`, { method: "DELETE" }),
  downloadEnv: (id: number) => request<PreReceiveEnv["download"]>(`/admin/pre-receive-environments/${id}/downloads`, { method: "POST" }),
  envDownload: (id: number) => request<PreReceiveEnv["download"]>(`/admin/pre-receive-environments/${id}/downloads/latest`),
  preHooks: () => request<PreReceiveHook[]>("/admin/pre-receive-hooks", { query: { per_page: 100, sort: "name" } }),
  createPreHook: (b: PreReceiveHookInput) => request<PreReceiveHook>("/admin/pre-receive-hooks", { method: "POST", body: b }),
  updatePreHook: (id: number, b: PreReceiveHookInput) => request<PreReceiveHook>(`/admin/pre-receive-hooks/${id}`, { method: "PATCH", body: b }),
  deletePreHook: (id: number) => request<void>(`/admin/pre-receive-hooks/${id}`, { method: "DELETE" }),
  orgPreHooks: (org: string) => request<ScopedPreReceiveHook[]>(`/orgs/${enc(org)}/pre-receive-hooks`, { query: { per_page: 100, sort: "name" } }),
  updateOrgPreHook: (org: string, id: number, b: { enforcement?: Enforcement; allow_downstream_configuration?: boolean }) =>
    request<ScopedPreReceiveHook>(`/orgs/${enc(org)}/pre-receive-hooks/${id}`, { method: "PATCH", body: b }),
  resetOrgPreHook: (org: string, id: number) => request<ScopedPreReceiveHook>(`/orgs/${enc(org)}/pre-receive-hooks/${id}`, { method: "DELETE" }),
  repoPreHooks: (o: string, r: string) => request<ScopedPreReceiveHook[]>(`/repos/${enc(o)}/${enc(r)}/pre-receive-hooks`, { query: { per_page: 100, sort: "name" } }),
  updateRepoPreHook: (o: string, r: string, id: number, enforcement: Enforcement) =>
    request<ScopedPreReceiveHook>(`/repos/${enc(o)}/${enc(r)}/pre-receive-hooks/${id}`, { method: "PATCH", body: { enforcement } }),
  resetRepoPreHook: (o: string, r: string, id: number) =>
    request<ScopedPreReceiveHook>(`/repos/${enc(o)}/${enc(r)}/pre-receive-hooks/${id}`, { method: "DELETE" }),

  // audit log
  auditLog: (ent: string, phrase: string, page: number, per_page = 50) =>
    requestPage<AuditEvent[]>(`${e(ent)}/audit-log`, { query: { phrase, page, per_page, include: "all" } }),

  // live migrations
  migrations: (status?: string) => request<{ migrations: LiveMigration[]; total_count: number }>("/enterprise/live-migrations", { query: { status, page_size: 100 } }),
  migration: (id: string) => request<LiveMigration>(`/enterprise/live-migrations/${enc(id)}`),
  createMigration: (b: Record<string, string>) => request<LiveMigration>("/enterprise/live-migrations", { method: "POST", body: b }),
  migrationAction: (id: string, action: string) => request<unknown>(`/enterprise/live-migrations/${enc(id)}/${action}`, { method: "POST", body: {} }),

  // enterprise teams
  teams: (ent: string) => request<EntTeam[]>(`${e(ent)}/teams`, { query: { per_page: 100 } }),
  team: (ent: string, slug: string) => request<EntTeam>(`${e(ent)}/teams/${enc(slug)}`),
  createTeam: (ent: string, b: Partial<EntTeam>) => request<EntTeam>(`${e(ent)}/teams`, { method: "POST", body: b }),
  updateTeam: (ent: string, slug: string, b: Partial<EntTeam>) => request<EntTeam>(`${e(ent)}/teams/${enc(slug)}`, { method: "PATCH", body: b }),
  deleteTeam: (ent: string, slug: string) => request<void>(`${e(ent)}/teams/${enc(slug)}`, { method: "DELETE" }),
  teamMembers: (ent: string, slug: string) => request<MiniUser[]>(`${e(ent)}/teams/${enc(slug)}/memberships`, { query: { per_page: 100 } }),
  addTeamMember: (ent: string, slug: string, login: string) =>
    request<MiniUser>(`${e(ent)}/teams/${enc(slug)}/memberships/${enc(login)}`, { method: "PUT" }),
  removeTeamMember: (ent: string, slug: string, login: string) =>
    request<void>(`${e(ent)}/teams/${enc(slug)}/memberships/${enc(login)}`, { method: "DELETE" }),
  teamOrgs: (ent: string, slug: string) => request<MiniUser[]>(`${e(ent)}/teams/${enc(slug)}/organizations`, { query: { per_page: 100 } }),
  addTeamOrg: (ent: string, slug: string, org: string) =>
    request<MiniUser>(`${e(ent)}/teams/${enc(slug)}/organizations/${enc(org)}`, { method: "PUT" }),
  removeTeamOrg: (ent: string, slug: string, org: string) =>
    request<void>(`${e(ent)}/teams/${enc(slug)}/organizations/${enc(org)}`, { method: "DELETE" }),

  // SCIM
  scimUsers: (ent: string, filter?: string) =>
    request<ScimList<ScimUser>>(`${scim(ent)}/Users`, { query: { filter, count: 100 }, accept: SCIM }),
  scimUser: (ent: string, id: string) => request<ScimUser>(`${scim(ent)}/Users/${enc(id)}`, { accept: SCIM }),
  scimGroups: (ent: string) => request<ScimList<ScimGroup>>(`${scim(ent)}/Groups`, { query: { count: 100 }, accept: SCIM }),
  scimGroup: (ent: string, id: string) => request<ScimGroup>(`${scim(ent)}/Groups/${enc(id)}`, { accept: SCIM }),
};

export type { Page };
