// Endpoint helpers for the packages area: REST (/api/v3) packages, apps, installations and
// private registries, plus the forge-only endpoints (/api/forge) behind the web-only pages.
import { FORGE, request, requestAll, type Repo, type User } from "../../api";

const enc = encodeURIComponent;

export const PACKAGE_TYPES = ["container", "npm", "maven", "rubygems", "nuget", "docker"] as const;
export type PackageType = (typeof PACKAGE_TYPES)[number];

export interface Package {
  id: number;
  name: string;
  package_type: PackageType | string;
  owner: User;
  version_count: number;
  visibility: "public" | "private" | string;
  url: string;
  html_url: string;
  repository: Repo | null;
  created_at: string;
  updated_at: string;
  deleted_at?: string | null;
  name_in_use?: boolean;
}

export interface PackageVersion {
  id: number;
  name: string;
  url: string;
  package_html_url: string;
  html_url: string;
  created_at: string;
  updated_at: string;
  deleted_at?: string;
  description?: string;
  license?: string;
  metadata: { package_type: string; container?: { tags: string[] } };
}

export interface VersionSummary {
  id: number;
  name: string;
  digest: string;
  size: number;
  media_type: string;
  tags: string[];
  description: string | null;
  license: string | null;
  created_at: string;
  deleted_at: string | null;
}

export interface VersionDetails extends VersionSummary {
  metadata: Record<string, unknown>;
  readme: string | null;
  publisher: string | null;
  viewer_permission: Perm;
}

export type Perm = "admin" | "write" | "read" | "none";

export interface PackageDetails {
  package: Package;
  visibility: "public" | "private" | "internal";
  inherit_access: boolean;
  viewer_permission: Perm;
  repository: string | null;
  creator: string | null;
  latest_version: VersionSummary | null;
  description: string | null;
  readme: string | null;
  npm_registry_url: string;
}

export interface AccessGrant {
  user: User;
  role: "read" | "write" | "admin";
}

/** `users` or `orgs`, as in GitHub's package URLs. */
export const ns = (owner: User | { type: string }) => (owner.type === "Organization" ? "orgs" : "users");

/** SPA path of a package page (GitHub's scheme: /{users|orgs}/{owner}/packages/{type}/package/{name}). */
export function packagePath(p: Pick<Package, "owner" | "package_type" | "name">, sub?: "settings" | "versions" | number | string): string {
  const base = `/${ns(p.owner)}/${p.owner.login}/packages/${p.package_type}`;
  const name = enc(p.name);
  if (sub === undefined) return `${base}/package/${name}`;
  return `${base}/${name}/${sub}`;
}

/** REST base for a package (`/users/{u}/packages/{type}/{name}` or `/orgs/...`). */
const restBase = (owner: User, type: string, name: string) => `/${ns(owner)}/${enc(owner.login)}/packages/${enc(type)}/${enc(name)}`;
const forgeBase = (owner: string, type: string, name: string) => `${FORGE}/packages/${enc(owner)}/${enc(type)}/${enc(name)}`;

export const pkgApi = {
  // lists (forge: every package type in one call)
  listAll: (owner: string, q?: { package_type?: string; q?: string }) => request<Package[]>(`${FORGE}/packages/${enc(owner)}`, { query: q }),
  listDeleted: (owner: string) => request<Package[]>(`${FORGE}/packages/${enc(owner)}/deleted`),
  repoPackages: (o: string, r: string) => request<Package[]>(`${FORGE}/repos/${enc(o)}/${enc(r)}/packages`),
  // REST list (one type)
  listType: (owner: User, type: string) =>
    requestAll<Package>(`/${ns(owner)}/${enc(owner.login)}/packages`, { query: { package_type: type } }, 3),

  details: (owner: string, type: string, name: string) => request<PackageDetails>(forgeBase(owner, type, name)),
  update: (owner: string, type: string, name: string, b: { visibility?: string; repository?: string; inherit_access?: boolean }) =>
    request<{ package: Package; visibility: string; inherit_access: boolean }>(forgeBase(owner, type, name), { method: "PATCH", body: b }),
  access: (owner: string, type: string, name: string) => request<AccessGrant[]>(`${forgeBase(owner, type, name)}/access`),
  grant: (owner: string, type: string, name: string, login: string, role: string) =>
    request<void>(`${forgeBase(owner, type, name)}/access/${enc(login)}`, { method: "PUT", body: { role } }),
  revoke: (owner: string, type: string, name: string, login: string) =>
    request<void>(`${forgeBase(owner, type, name)}/access/${enc(login)}`, { method: "DELETE" }),
  versionDetails: (owner: string, type: string, name: string, id: number) =>
    request<VersionDetails>(`${forgeBase(owner, type, name)}/versions/${id}`),

  // REST
  versions: (owner: User, type: string, name: string, state: "active" | "deleted" = "active") =>
    requestAll<PackageVersion>(`${restBase(owner, type, name)}/versions`, { query: { state } }, 5),
  deletePackage: (owner: User, type: string, name: string) => request<void>(restBase(owner, type, name), { method: "DELETE" }),
  restorePackage: (owner: User, type: string, name: string) => request<void>(`${restBase(owner, type, name)}/restore`, { method: "POST" }),
  deleteVersion: (owner: User, type: string, name: string, id: number) =>
    request<void>(`${restBase(owner, type, name)}/versions/${id}`, { method: "DELETE" }),
  restoreVersion: (owner: User, type: string, name: string, id: number) =>
    request<void>(`${restBase(owner, type, name)}/versions/${id}/restore`, { method: "POST" }),
};

// ---------- GitHub Apps / OAuth apps ----------

export interface AppSummary {
  id: number;
  slug: string;
  name: string;
  kind: "app" | "oauth";
  client_id: string;
  owner?: User;
  html_url?: string;
  description?: string | null;
  external_url?: string;
  url?: string;
}

export interface AppKey {
  id: number;
  fingerprint: string;
  created_at: string;
}

export interface ManagedApp {
  id: number;
  slug: string;
  name: string;
  kind: "app" | "oauth";
  description: string | null;
  external_url: string;
  html_url?: string;
  client_id: string;
  owner: User;
  permissions?: Record<string, string>;
  events?: string[];
  installations_count?: number;
  public: boolean;
  callback_urls: string[];
  setup_url: string | null;
  redirect_url: string | null;
  request_oauth_on_install: boolean;
  setup_on_update: boolean;
  webhook?: { url: string; active: boolean; has_secret: boolean };
  keys?: AppKey[];
  bot_login?: string | null;
  users_count: number;
  created_at: string;
  updated_at: string;
}

export interface Integration {
  id: number;
  slug: string;
  name: string;
  description: string | null;
  external_url: string;
  html_url: string;
  owner: User;
  permissions: Record<string, string>;
  events: string[];
  installations_count: number;
  client_id: string;
  created_at: string;
}

export interface Installation {
  id: number;
  account: User;
  repository_selection: "all" | "selected";
  app_id: number;
  app_slug: string;
  target_type: string;
  target_id: number;
  permissions: Record<string, string>;
  events: string[];
  created_at: string;
  updated_at: string;
  suspended_at: string | null;
  suspended_by: User | null;
  html_url: string;
}

export interface InstallationDetails extends Installation {
  app: Integration;
  repositories: Repo[];
  viewer_can_manage: boolean;
  pending_update: { permissions: Record<string, string>; events: string[] } | null;
}

export interface Grant {
  id: number;
  scopes: string[];
  created_at: string;
  updated_at: string;
  last_used_at: string | null;
  app: { name: string; slug: string; kind: string; url: string; client_id: string; owner: User };
}

export interface NewAppBody {
  owner?: string;
  name: string;
  url: string;
  description?: string;
  callback_urls?: string[];
  setup_url?: string;
  redirect_url?: string;
  hook_attributes?: { url?: string; active?: boolean };
  public?: boolean;
  default_permissions?: Record<string, string>;
  default_events?: string[];
  request_oauth_on_install?: boolean;
  setup_on_update?: boolean;
}

export interface CreatedApp extends Integration {
  client_secret: string;
  webhook_secret: string | null;
  pem: string;
}

export const appsApi = {
  myApps: () => request<AppSummary[]>(`${FORGE}/apps`),
  createApp: (b: NewAppBody) => request<CreatedApp>(`${FORGE}/apps`, { method: "POST", body: b }),
  createOauthApp: (b: { name: string; url: string; callback_url?: string; owner?: string }) =>
    request<{ id: number; slug: string; name: string; client_id: string; client_secret: string }>(`${FORGE}/oauth-apps`, { method: "POST", body: b }),
  manage: (slug: string) => request<ManagedApp>(`${FORGE}/apps/${enc(slug)}`),
  update: (slug: string, b: Partial<NewAppBody> & { webhook_secret?: string }) =>
    request<ManagedApp>(`${FORGE}/apps/${enc(slug)}`, { method: "PATCH", body: b }),
  remove: (slug: string) => request<void>(`${FORGE}/apps/${enc(slug)}`, { method: "DELETE" }),
  addKey: (slug: string) => request<{ id: number; pem: string }>(`${FORGE}/apps/${enc(slug)}/keys`, { method: "POST" }),
  deleteKey: (slug: string, id: number) => request<void>(`${FORGE}/apps/${enc(slug)}/keys/${id}`, { method: "DELETE" }),
  newSecret: (slug: string) => request<{ client_secret: string }>(`${FORGE}/apps/${enc(slug)}/client-secret`, { method: "POST" }),

  publicApp: (slug: string) => request<Integration>(`/apps/${enc(slug)}`),
  install: (slug: string, b: { account?: string; repository_selection: string; repository_ids?: number[] }) =>
    request<Installation>(`${FORGE}/apps/${enc(slug)}/installations`, { method: "POST", body: b }),

  myInstallations: () => request<{ total_count: number; installations: Installation[] }>("/user/installations", { query: { per_page: 100 } }),
  orgInstallations: (org: string) => request<{ total_count: number; installations: Installation[] }>(`/orgs/${enc(org)}/installations`, { query: { per_page: 100 } }),
  installation: (id: number) => request<InstallationDetails>(`${FORGE}/installations/${id}`),
  updateInstallation: (id: number, b: { repository_selection: string; repository_ids?: number[] }) =>
    request<InstallationDetails>(`${FORGE}/installations/${id}`, { method: "PATCH", body: b }),
  uninstall: (id: number) => request<void>(`${FORGE}/installations/${id}`, { method: "DELETE" }),
  suspend: (id: number) => request<void>(`${FORGE}/installations/${id}/suspended`, { method: "PUT" }),
  unsuspend: (id: number) => request<void>(`${FORGE}/installations/${id}/suspended`, { method: "DELETE" }),
  acceptPermissions: (id: number) => request<InstallationDetails>(`${FORGE}/installations/${id}/permissions`, { method: "POST" }),

  authorizations: (kind: "app" | "oauth") => request<Grant[]>(`${FORGE}/authorizations`, { query: { kind } }),
  revokeAuthorization: (id: number) => request<void>(`${FORGE}/authorizations/${id}`, { method: "DELETE" }),

  device: (code: string) =>
    request<{ user_code: string; client_id: string; scopes: string[]; expires_at: string; app: { name: string; kind: string; url: string; owner: string | null } | null }>(
      `${FORGE}/device/${enc(code)}`,
    ),
  approveDevice: (user_code: string) => request<void>(`${FORGE}/device/approve`, { method: "POST", body: { user_code } }),
  denyDevice: (user_code: string) => request<void>(`${FORGE}/device/deny`, { method: "POST", body: { user_code } }),

  // accounts the viewer can install on / register apps for
  myOrgs: () => request<User[]>("/user/orgs", { query: { per_page: 100 } }),
  membership: (org: string) => request<{ role: string; state: string }>(`/user/memberships/orgs/${enc(org)}`),
  accountRepos: (account: User) =>
    account.type === "Organization"
      ? requestAll<Repo>(`/orgs/${enc(account.login)}/repos`, { query: { per_page: 100 } }, 5)
      : requestAll<Repo>("/user/repos", { query: { affiliation: "owner", per_page: 100 } }, 5),
};

// ---------- private registries ----------

export interface PrivateRegistry {
  name: string;
  registry_type: string;
  url: string;
  username?: string | null;
  replaces_base?: boolean;
  visibility: "all" | "private" | "selected";
  selected_repository_ids?: number[];
  auth_type?: string | null;
  created_at: string;
  updated_at: string;
  [k: string]: unknown;
}

export const registriesApi = {
  list: (org: string) => request<{ total_count: number; configurations: PrivateRegistry[] }>(`/orgs/${enc(org)}/private-registries`, { query: { per_page: 100 } }),
  get: (org: string, name: string) => request<PrivateRegistry>(`/orgs/${enc(org)}/private-registries/${enc(name)}`),
  publicKey: (org: string) => request<{ key_id: string; key: string }>(`/orgs/${enc(org)}/private-registries/public-key`),
  create: (org: string, b: Record<string, unknown>) => request<PrivateRegistry>(`/orgs/${enc(org)}/private-registries`, { method: "POST", body: b }),
  update: (org: string, name: string, b: Record<string, unknown>) =>
    request<void>(`/orgs/${enc(org)}/private-registries/${enc(name)}`, { method: "PATCH", body: b }),
  remove: (org: string, name: string) => request<void>(`/orgs/${enc(org)}/private-registries/${enc(name)}`, { method: "DELETE" }),
};
