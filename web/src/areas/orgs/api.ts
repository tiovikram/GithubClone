// Endpoint helpers for the orgs area (organizations, people, teams, roles, org settings).
import { FORGE, request, requestAll, requestPage, type Repo, type User } from "../../api";

const enc = encodeURIComponent;
const o = (org: string) => `/orgs/${enc(org)}`;

export interface Org extends User {
  description?: string | null;
  twitter_username?: string | null;
  is_verified?: boolean;
  total_private_repos?: number;
  owned_private_repos?: number;
  // owner-only fields
  billing_email?: string | null;
  default_repository_permission?: "read" | "write" | "admin" | "none";
  members_can_create_repositories?: boolean;
  members_can_create_public_repositories?: boolean;
  members_can_create_private_repositories?: boolean;
  members_can_create_pages?: boolean;
  members_can_fork_private_repositories?: boolean;
  members_can_create_teams?: boolean;
  web_commit_signoff_required?: boolean;
  deploy_keys_enabled_for_repositories?: boolean;
  has_organization_projects?: boolean;
  has_repository_projects?: boolean;
  plan?: { name: string; filled_seats?: number };
}

export interface Membership {
  url: string;
  state: "active" | "pending";
  role: "admin" | "member" | string;
  organization: User & { description?: string | null };
  user: User;
  permissions?: { can_create_repository: boolean };
}

export interface Team {
  id: number;
  name: string;
  slug: string;
  description: string | null;
  privacy: "closed" | "secret" | string;
  notification_setting?: "notifications_enabled" | "notifications_disabled" | string;
  permission: string;
  parent: Team | null;
  html_url: string;
  members_count?: number;
  repos_count?: number;
  created_at?: string;
  updated_at?: string;
}

export interface TeamRepo extends Repo {
  role_name?: string;
}

export interface Invitation {
  id: number;
  login: string | null;
  email: string | null;
  role: string;
  created_at: string;
  failed_at: string | null;
  failed_reason: string | null;
  inviter: User | null;
  team_count: number;
}

export interface OrgRole {
  id: number;
  name: string;
  description: string | null;
  base_role: string | null;
  source: "Predefined" | "Organization" | string;
  permissions: string[];
}

export interface RepoRole {
  id: number;
  name: string;
  description: string | null;
  base_role: string;
  permissions: string[];
}

export interface FineGrainedPermission {
  name: string;
  description: string;
}

export interface ExternalGroup {
  group_id: number;
  group_name: string;
  updated_at?: string;
}

export interface AuditEvent {
  "@timestamp": number;
  _document_id: string;
  action: string;
  actor: string | null;
  user?: string;
  repo?: string;
  team?: string;
  data?: Record<string, unknown>;
}

export interface Migration {
  id: number;
  state: "pending" | "exporting" | "exported" | "failed" | string;
  lock_repositories: boolean;
  exclude_attachments?: boolean;
  repositories: Repo[];
  created_at: string;
  updated_at: string;
  archive_url?: string;
}

export interface PatRequest {
  id: number;
  owner: User;
  reason?: string | null;
  repository_selection?: string;
  created_at: string;
  token_name?: string;
}

export interface PatGrant {
  id: number;
  owner: User;
  repository_selection?: string;
  access_granted_at: string;
  token_name?: string;
  token_last_used_at?: string | null;
}

export type BasePermission = "none" | "read" | "write" | "admin";

export const orgApi = {
  // organizations
  org: (org: string) => request<Org>(o(org)),
  create: (login: string, profile_name?: string) =>
    request<User>(`${FORGE}/orgs`, { method: "POST", body: { login, profile_name } }),
  update: (org: string, body: Partial<Org> & Record<string, unknown>) => request<Org>(o(org), { method: "PATCH", body }),
  rename: (org: string, login: string) => request<User>(`${FORGE}/orgs/${enc(org)}`, { method: "PATCH", body: { login } }),
  remove: (org: string) => request<void>(o(org), { method: "DELETE" }),
  repos: (org: string, q: { type?: string; sort?: string; page?: number; per_page?: number }) =>
    requestPage<Repo[]>(`${o(org)}/repos`, { query: q }),
  allRepos: (org: string, type = "all") => requestAll<Repo>(`${o(org)}/repos`, { query: { type, sort: "full_name" } }),
  createRepo: (org: string, body: { name: string; description?: string; private?: boolean; auto_init?: boolean }) =>
    request<Repo>(`${o(org)}/repos`, { method: "POST", body }),

  // the viewer's memberships
  myMemberships: (state?: "active" | "pending") => requestAll<Membership>("/user/memberships/orgs", { query: { state } }),
  myMembership: (org: string) => request<Membership>(`/user/memberships/orgs/${enc(org)}`),
  acceptInvitation: (org: string) =>
    request<Membership>(`/user/memberships/orgs/${enc(org)}`, { method: "PATCH", body: { state: "active" } }),
  declineInvitation: (org: string) => request<void>(`${FORGE}/user/memberships/orgs/${enc(org)}`, { method: "DELETE" }),
  isPublicMember: (org: string, login: string) =>
    request<void>(`${o(org)}/public_members/${enc(login)}`).then(() => true, () => false),
  publicize: (org: string, login: string) => request<void>(`${o(org)}/public_members/${enc(login)}`, { method: "PUT" }),
  conceal: (org: string, login: string) => request<void>(`${o(org)}/public_members/${enc(login)}`, { method: "DELETE" }),

  // people
  members: (org: string, role: "all" | "admin" | "member" = "all") => requestAll<User>(`${o(org)}/members`, { query: { role } }),
  publicMembers: (org: string) => requestAll<User>(`${o(org)}/public_members`),
  setRole: (org: string, login: string, role: "admin" | "member") =>
    request<Membership>(`${o(org)}/memberships/${enc(login)}`, { method: "PUT", body: { role } }),
  removeMember: (org: string, login: string) => request<void>(`${o(org)}/memberships/${enc(login)}`, { method: "DELETE" }),
  outsideCollaborators: (org: string) => requestAll<User>(`${o(org)}/outside_collaborators`),
  convertToOutside: (org: string, login: string) =>
    request<void>(`${o(org)}/outside_collaborators/${enc(login)}`, { method: "PUT", body: {} }),
  removeOutside: (org: string, login: string) => request<void>(`${o(org)}/outside_collaborators/${enc(login)}`, { method: "DELETE" }),
  invitations: (org: string) => requestAll<Invitation>(`${o(org)}/invitations`),
  failedInvitations: (org: string) => requestAll<Invitation>(`${o(org)}/failed_invitations`),
  invitationTeams: (org: string, id: number) => requestAll<Team>(`${o(org)}/invitations/${id}/teams`),
  invite: (org: string, body: { invitee_id?: number; email?: string; role: string; team_ids: number[] }) =>
    request<Invitation>(`${o(org)}/invitations`, { method: "POST", body }),
  cancelInvitation: (org: string, id: number) => request<void>(`${o(org)}/invitations/${id}`, { method: "DELETE" }),
  securityManagers: (org: string) => request<Team[]>(`${o(org)}/security-managers`),
  addSecurityManager: (org: string, slug: string) => request<void>(`${o(org)}/security-managers/teams/${enc(slug)}`, { method: "PUT" }),
  removeSecurityManager: (org: string, slug: string) =>
    request<void>(`${o(org)}/security-managers/teams/${enc(slug)}`, { method: "DELETE" }),

  // teams
  teams: (org: string) => requestAll<Team>(`${o(org)}/teams`),
  team: (org: string, slug: string) => request<Team>(`${o(org)}/teams/${enc(slug)}`),
  createTeam: (org: string, body: { name: string; description?: string; privacy?: string; parent_team_id?: number | null; notification_setting?: string }) =>
    request<Team>(`${o(org)}/teams`, { method: "POST", body }),
  updateTeam: (org: string, slug: string, body: { name?: string; description?: string; privacy?: string; parent_team_id?: number | null; notification_setting?: string }) =>
    request<Team>(`${o(org)}/teams/${enc(slug)}`, { method: "PATCH", body }),
  deleteTeam: (org: string, slug: string) => request<void>(`${o(org)}/teams/${enc(slug)}`, { method: "DELETE" }),
  teamMembers: (org: string, slug: string, role: "all" | "maintainer" | "member" = "all") =>
    requestAll<User>(`${o(org)}/teams/${enc(slug)}/members`, { query: { role } }),
  setTeamMembership: (org: string, slug: string, login: string, role: "member" | "maintainer") =>
    request<{ role: string; state: string }>(`${o(org)}/teams/${enc(slug)}/memberships/${enc(login)}`, { method: "PUT", body: { role } }),
  removeTeamMember: (org: string, slug: string, login: string) =>
    request<void>(`${o(org)}/teams/${enc(slug)}/memberships/${enc(login)}`, { method: "DELETE" }),
  teamRepos: (org: string, slug: string) => requestAll<TeamRepo>(`${o(org)}/teams/${enc(slug)}/repos`),
  setTeamRepo: (org: string, slug: string, owner: string, repo: string, permission: string) =>
    request<void>(`${o(org)}/teams/${enc(slug)}/repos/${enc(owner)}/${enc(repo)}`, { method: "PUT", body: { permission } }),
  removeTeamRepo: (org: string, slug: string, owner: string, repo: string) =>
    request<void>(`${o(org)}/teams/${enc(slug)}/repos/${enc(owner)}/${enc(repo)}`, { method: "DELETE" }),
  childTeams: (org: string, slug: string) => requestAll<Team>(`${o(org)}/teams/${enc(slug)}/teams`),
  externalGroups: (org: string) => request<{ groups: ExternalGroup[] }>(`${o(org)}/external-groups`).then((r) => r.groups || []),
  teamGroups: (org: string, slug: string) =>
    request<{ groups: ExternalGroup[] }>(`${o(org)}/teams/${enc(slug)}/external-groups`).then((r) => r.groups || []),
  linkGroup: (org: string, slug: string, group_id: number) =>
    request<unknown>(`${o(org)}/teams/${enc(slug)}/external-groups`, { method: "PATCH", body: { group_id } }),
  unlinkGroup: (org: string, slug: string) => request<void>(`${o(org)}/teams/${enc(slug)}/external-groups`, { method: "DELETE" }),
  teamInvitations: (org: string, slug: string) => requestAll<Invitation>(`${o(org)}/teams/${enc(slug)}/invitations`),

  // organization roles
  orgRoles: (org: string) => request<{ total_count: number; roles: OrgRole[] }>(`${o(org)}/organization-roles`).then((r) => r.roles),
  orgPermissions: (org: string) => request<FineGrainedPermission[]>(`${o(org)}/organization-fine-grained-permissions`),
  createOrgRole: (org: string, body: { name: string; description?: string; permissions: string[]; base_role?: string }) =>
    request<OrgRole>(`${o(org)}/organization-roles`, { method: "POST", body }),
  updateOrgRole: (org: string, id: number, body: { name?: string; description?: string; permissions?: string[]; base_role?: string }) =>
    request<OrgRole>(`${o(org)}/organization-roles/${id}`, { method: "PATCH", body }),
  deleteOrgRole: (org: string, id: number) => request<void>(`${o(org)}/organization-roles/${id}`, { method: "DELETE" }),
  roleUsers: (org: string, id: number) => requestAll<User & { assignment?: string }>(`${o(org)}/organization-roles/${id}/users`),
  roleTeams: (org: string, id: number) => requestAll<Team>(`${o(org)}/organization-roles/${id}/teams`),
  assignUserRole: (org: string, login: string, id: number) =>
    request<void>(`${o(org)}/organization-roles/users/${enc(login)}/${id}`, { method: "PUT" }),
  revokeUserRole: (org: string, login: string, id: number) =>
    request<void>(`${o(org)}/organization-roles/users/${enc(login)}/${id}`, { method: "DELETE" }),
  assignTeamRole: (org: string, slug: string, id: number) =>
    request<void>(`${o(org)}/organization-roles/teams/${enc(slug)}/${id}`, { method: "PUT" }),
  revokeTeamRole: (org: string, slug: string, id: number) =>
    request<void>(`${o(org)}/organization-roles/teams/${enc(slug)}/${id}`, { method: "DELETE" }),

  // custom repository roles
  repoRoles: (org: string) =>
    request<{ total_count: number; custom_roles: RepoRole[] }>(`${o(org)}/custom-repository-roles`).then((r) => r.custom_roles),
  repoPermissions: (org: string) => request<FineGrainedPermission[]>(`${o(org)}/repository-fine-grained-permissions`),
  createRepoRole: (org: string, body: { name: string; description?: string; base_role: string; permissions: string[] }) =>
    request<RepoRole>(`${o(org)}/custom-repository-roles`, { method: "POST", body }),
  updateRepoRole: (org: string, id: number, body: { name?: string; description?: string; base_role?: string; permissions?: string[] }) =>
    request<RepoRole>(`${o(org)}/custom-repository-roles/${id}`, { method: "PATCH", body }),
  deleteRepoRole: (org: string, id: number) => request<void>(`${o(org)}/custom-repository-roles/${id}`, { method: "DELETE" }),

  // blocked users
  blocks: (org: string) => requestAll<User>(`${o(org)}/blocks`),
  block: (org: string, login: string) => request<void>(`${o(org)}/blocks/${enc(login)}`, { method: "PUT" }),
  unblock: (org: string, login: string) => request<void>(`${o(org)}/blocks/${enc(login)}`, { method: "DELETE" }),

  // fine-grained personal access tokens
  patRequests: (org: string) => request<PatRequest[]>(`${o(org)}/personal-access-token-requests`),
  reviewPatRequest: (org: string, id: number, action: "approve" | "deny") =>
    request<void>(`${o(org)}/personal-access-token-requests/${id}`, { method: "POST", body: { action } }),
  patGrants: (org: string) => request<PatGrant[]>(`${o(org)}/personal-access-tokens`),
  revokePat: (org: string, id: number) => request<void>(`${o(org)}/personal-access-tokens/${id}`, { method: "POST", body: { action: "revoke" } }),

  // audit log
  auditLog: (org: string, phrase: string, page: number) =>
    requestPage<AuditEvent[]>(`${o(org)}/audit-log`, { query: { phrase: phrase || undefined, page, per_page: 50 } }),

  // migrations (exports)
  migrations: (org: string) => request<Migration[]>(`${o(org)}/migrations`),
  startMigration: (org: string, body: { repositories: string[]; lock_repositories: boolean; exclude_attachments: boolean }) =>
    request<Migration>(`${o(org)}/migrations`, { method: "POST", body }),
  deleteMigrationArchive: (org: string, id: number) => request<void>(`${o(org)}/migrations/${id}/archive`, { method: "DELETE" }),
  migrationArchiveUrl: (org: string, id: number) => `/api/v3${o(org)}/migrations/${id}/archive`,

  // users
  userByLogin: (login: string) => request<User>(`/users/${enc(login)}`),
};
