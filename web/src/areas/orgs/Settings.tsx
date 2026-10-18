// The organization settings shell (/organizations/:org/settings/*) and the orgs area's own
// sections. Other areas register sections through `orgSettings` (see docs/web/orgs.md).
import { useEffect, useMemo, useState, type FormEvent, type ReactNode } from "react";
import { Link, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { orgSettings } from "..";
import type { OrgCtx, SettingsSection } from "../types";
import { useAuth } from "../../auth";
import { Async, Avatar, Blankslate, ErrorBox, Flash, NotFoundView, Pagination, RelTime, Spinner } from "../../components/common";
import { SettingsShell } from "../../components/SettingsShell";
import { useAsync, useTitle } from "../../hooks";
import { orgApi, type BasePermission, type Org, type OrgRole, type RepoRole } from "./api";
import { OrgHeader, permLabel, useAction, WithOrg } from "./common";

/** `/organizations/:org/settings/*`: owners only (404 for everyone else). */
export function OrgSettingsPage() {
  const { org = "" } = useParams();
  const { user, loading } = useAuth();
  useTitle(`Settings · ${org}`);
  if (loading) return <Spinner />;
  if (!user) return <NotFoundView />;
  return (
    <WithOrg login={org}>
      {(st) => !st.isOwner ? <NotFoundView /> : (
        <>
          <OrgHeader st={st} active="settings" />
          <SettingsShell root={`/organizations/${st.org.login}/settings`} sections={orgSettings} label="Organization settings"
            ctx={{ org: st.org, login: st.org.login, isOwner: st.isOwner, reload: st.reload }} />
        </>
      )}
    </WithOrg>
  );
}

function Section({ title, children, desc }: { title: string; desc?: ReactNode; children: ReactNode }) {
  return (
    <div className="org-settings-section">
      <h2 className="h3 org-settings-title">{title}</h2>
      {desc && <p className="muted small">{desc}</p>}
      {children}
    </div>
  );
}

// ---------------------------------------------------------------------------------------------
// General

function General({ ctx }: { ctx: OrgCtx }) {
  const org = ctx.org as Org;
  const nav = useNavigate();
  const [f, setF] = useState({ name: "", email: "", description: "", blog: "", location: "", billing_email: "", twitter_username: "", company: "" });
  useEffect(() => {
    setF({
      name: org.name || "", email: org.email || "", description: org.description || "", blog: org.blog || "", location: org.location || "",
      billing_email: org.billing_email || "", twitter_username: org.twitter_username || "", company: org.company || "",
    });
  }, [org]);
  const act = useAction();
  const [saved, setSaved] = useState(false);
  const ren = useAction();
  const [newLogin, setNewLogin] = useState("");
  const del = useAction();
  const [confirmText, setConfirmText] = useState("");
  const field = (k: keyof typeof f, label: string, type = "text", hint?: string) => (
    <div className="form-group">
      <label htmlFor={`org-set-${k}`}>{label}</label>
      <input id={`org-set-${k}`} className="input" type={type} value={f[k]} onChange={(e) => setF({ ...f, [k]: e.target.value })} />
      {hint && <p className="muted small">{hint}</p>}
    </div>
  );
  return (
    <Section title="General">
      <form className="box box-body" onSubmit={async (e: FormEvent) => {
        e.preventDefault();
        setSaved(false);
        if (await act.run(() => orgApi.update(ctx.login, f))) { setSaved(true); ctx.reload(); }
      }}>
        {saved && <Flash kind="success">Profile updated successfully.</Flash>}
        {act.error !== undefined && <ErrorBox error={act.error} />}
        <div className="org-settings-profile">
          <div className="grow">
            {field("name", "Organization display name")}
            {field("email", "Public email", "email")}
            <div className="form-group">
              <label htmlFor="org-set-description">Description</label>
              <textarea id="org-set-description" className="input textarea" rows={3} value={f.description} onChange={(e) => setF({ ...f, description: e.target.value })} />
            </div>
            {field("blog", "URL", "text")}
            {field("location", "Location")}
            {field("twitter_username", "Social account (X/Twitter username)")}
            {field("billing_email", "Billing email (private)", "email", "Receipts and billing notices go here.")}
          </div>
          <div className="org-settings-avatar">
            <span className="label-text">Profile picture</span>
            <Avatar user={org} size={120} />
          </div>
        </div>
        <button type="submit" className="btn btn-primary" disabled={act.busy}>{act.busy ? "Saving…" : "Update profile"}</button>
      </form>

      <h2 className="h4 c-danger">Danger zone</h2>
      <div className="box danger-zone">
        <form className="box-body" onSubmit={async (e: FormEvent) => {
          e.preventDefault();
          let login = "";
          if (await ren.run(async () => { login = (await orgApi.rename(ctx.login, newLogin.trim())).login; })) {
            nav(`/organizations/${login}/settings/profile`, { replace: true });
          }
        }}>
          <h3 className="h5">Rename organization</h3>
          <p className="muted small">Renaming changes the URL of the organization and its repositories. Old links are not redirected.</p>
          {ren.error !== undefined && <ErrorBox error={ren.error} />}
          <div className="input-group">
            <label htmlFor="org-rename" className="sr-only">New name</label>
            <input id="org-rename" className="input" placeholder="New organization name" value={newLogin} onChange={(e) => setNewLogin(e.target.value)} />
            <button type="submit" className="btn btn-danger-outline" disabled={ren.busy || !newLogin.trim() || newLogin.trim() === ctx.login}>Rename organization</button>
          </div>
        </form>
        <form className="box-body org-danger-row" onSubmit={async (e: FormEvent) => {
          e.preventDefault();
          if (await del.run(() => orgApi.remove(ctx.login))) nav("/", { replace: true });
        }}>
          <h3 className="h5">Delete this organization</h3>
          <p className="muted small">Once deleted, it will be gone forever, together with all its repositories. Please be certain.</p>
          {del.error !== undefined && <ErrorBox error={del.error} />}
          <div className="form-group">
            <label htmlFor="org-del-confirm">To confirm, type <strong>{ctx.login}</strong> in the box below</label>
            <input id="org-del-confirm" className="input" autoComplete="off" value={confirmText} onChange={(e) => setConfirmText(e.target.value)} />
          </div>
          <button type="submit" className="btn btn-danger" disabled={del.busy || confirmText !== ctx.login}>{del.busy ? "Deleting…" : "Delete this organization"}</button>
        </form>
      </div>
    </Section>
  );
}

// ---------------------------------------------------------------------------------------------
// Member privileges

function MemberPrivileges({ ctx }: { ctx: OrgCtx }) {
  const org = ctx.org as Org;
  const init = () => ({
    default_repository_permission: (org.default_repository_permission || "read") as BasePermission,
    members_can_create_public_repositories: !!org.members_can_create_public_repositories && !!org.members_can_create_repositories,
    members_can_create_private_repositories: !!org.members_can_create_private_repositories && !!org.members_can_create_repositories,
    members_can_fork_private_repositories: !!org.members_can_fork_private_repositories,
    members_can_create_pages: !!org.members_can_create_pages,
    members_can_create_teams: !!org.members_can_create_teams,
    web_commit_signoff_required: !!org.web_commit_signoff_required,
    deploy_keys_enabled_for_repositories: org.deploy_keys_enabled_for_repositories !== false,
    has_organization_projects: org.has_organization_projects !== false,
    has_repository_projects: org.has_repository_projects !== false,
  });
  const [f, setF] = useState(init);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  useEffect(() => setF(init()), [org]);
  const act = useAction();
  const [saved, setSaved] = useState("");
  const save = async (patch: Partial<typeof f>, msg: string) => {
    setSaved("");
    const body: Record<string, unknown> = { ...patch };
    if ("members_can_create_public_repositories" in patch || "members_can_create_private_repositories" in patch) {
      const pub = patch.members_can_create_public_repositories ?? f.members_can_create_public_repositories;
      const pri = patch.members_can_create_private_repositories ?? f.members_can_create_private_repositories;
      body.members_can_create_repositories = pub || pri;
      body.members_can_create_public_repositories = pub;
      body.members_can_create_private_repositories = pri;
    }
    if (await act.run(() => orgApi.update(ctx.login, body))) { setSaved(msg); ctx.reload(); }
  };
  const check = (k: keyof typeof f, label: string, hint: string) => (
    <label className="check-row form-group">
      <input type="checkbox" checked={!!f[k]} onChange={(e) => setF({ ...f, [k]: e.target.checked })} data-testid={`priv-${k}`} />
      <span>{label}<br /><span className="muted small">{hint}</span></span>
    </label>
  );
  return (
    <Section title="Member privileges">
      {saved && <Flash kind="success">{saved}</Flash>}
      {act.error !== undefined && <ErrorBox error={act.error} />}
      <form className="box box-body" onSubmit={(e) => { e.preventDefault(); save({ default_repository_permission: f.default_repository_permission }, "Base permission updated."); }}>
        <h3 className="h5">Base permissions</h3>
        <p className="muted small">Base permissions to the organization's repositories apply to all members and exclude outside collaborators.</p>
        <div className="input-group">
          <label htmlFor="base-perm" className="sr-only">Base permission</label>
          <select id="base-perm" className="input input-auto" value={f.default_repository_permission}
            onChange={(e) => setF({ ...f, default_repository_permission: e.target.value as BasePermission })}>
            <option value="none">No permission</option>
            <option value="read">Read</option>
            <option value="write">Write</option>
            <option value="admin">Admin</option>
          </select>
          <button type="submit" className="btn" disabled={act.busy} data-testid="save-base-perm">Save</button>
        </div>
        <p className="muted small">{{
          none: "Members will only be able to clone and pull public repositories.",
          read: "Members will be able to clone and pull all repositories.",
          write: "Members will be able to clone, pull, and push all repositories.",
          admin: "Members will be able to clone, pull, push, and add new collaborators to all repositories.",
        }[f.default_repository_permission]}</p>
      </form>
      <form className="box box-body" onSubmit={(e) => {
        e.preventDefault();
        const { default_repository_permission: _ignored, ...rest } = f;
        void _ignored;
        save(rest, "Member privileges updated.");
      }}>
        <h3 className="h5">Repository creation</h3>
        <p className="muted small">Members will be able to create only the selected repository types. Outside collaborators can never create repositories.</p>
        {check("members_can_create_public_repositories", "Public", "Members will be able to create public repositories, visible to anyone.")}
        {check("members_can_create_private_repositories", "Private", "Members will be able to create private repositories, visible to organization members with permission.")}
        <h3 className="h5">Repository forking</h3>
        {check("members_can_fork_private_repositories", "Allow forking of private repositories", "If enabled, forking is allowed on private repositories.")}
        <h3 className="h5">Pages creation</h3>
        {check("members_can_create_pages", "Allow members to publish sites", "Members will be able to publish Pages sites for the repositories they administer.")}
        <h3 className="h5">Team creation rules</h3>
        {check("members_can_create_teams", "Allow members to create teams", "If disabled, only owners can create teams.")}
        <h3 className="h5">Commit signoff</h3>
        {check("web_commit_signoff_required", "Require contributors to sign off on web-based commits", "Applies to every repository of the organization.")}
        <h3 className="h5">Deploy keys</h3>
        {check("deploy_keys_enabled_for_repositories", "Allow deploy keys", "Repository administrators can add deploy keys.")}
        <h3 className="h5">Projects</h3>
        {check("has_organization_projects", "Enable projects for the organization", "Members can create projects owned by the organization.")}
        {check("has_repository_projects", "Enable projects for all repositories", "Repository projects can be enabled per repository.")}
        <button type="submit" className="btn btn-primary" disabled={act.busy} data-testid="save-privileges">Save privileges</button>
      </form>
    </Section>
  );
}

// ---------------------------------------------------------------------------------------------
// Organization roles

function OrgRoles({ ctx }: { ctx: OrgCtx }) {
  const data = useAsync(async () => {
    const [roles, perms, members, teams] = await Promise.all([
      orgApi.orgRoles(ctx.login), orgApi.orgPermissions(ctx.login), orgApi.members(ctx.login), orgApi.teams(ctx.login),
    ]);
    const assigned = await Promise.all(roles.map(async (r) => ({ id: r.id, users: await orgApi.roleUsers(ctx.login, r.id), teams: await orgApi.roleTeams(ctx.login, r.id) })));
    return { roles, perms, members, teams, assigned };
  }, [ctx.login]);
  const act = useAction();
  const [who, setWho] = useState("");
  const [roleId, setRoleId] = useState("");
  const [creating, setCreating] = useState(false);
  return (
    <Section title="Organization roles" desc="Assign predefined or custom roles to members and teams to grant them organization-wide permissions without making them owners.">
      {act.error !== undefined && <ErrorBox error={act.error} />}
      <Async state={data}>
        {(d) => (
          <>
            <form className="box box-body" aria-label="Assign role" onSubmit={async (e) => {
              e.preventDefault();
              const [kind, name] = who.split(":");
              const ok = await act.run(() => (kind === "team" ? orgApi.assignTeamRole(ctx.login, name, Number(roleId)) : orgApi.assignUserRole(ctx.login, name, Number(roleId))));
              if (ok) { setWho(""); data.reload(); }
            }}>
              <h3 className="h5">Role assignments</h3>
              <div className="toolbar">
                <label htmlFor="role-assignee" className="sr-only">Member or team</label>
                <select id="role-assignee" className="input input-auto grow" value={who} onChange={(e) => setWho(e.target.value)}>
                  <option value="">Select a member or team…</option>
                  <optgroup label="Members">{d.members.map((m) => <option key={m.id} value={`user:${m.login}`}>{m.login}</option>)}</optgroup>
                  <optgroup label="Teams">{d.teams.map((t) => <option key={t.id} value={`team:${t.slug}`}>{t.name}</option>)}</optgroup>
                </select>
                <label htmlFor="role-pick" className="sr-only">Role</label>
                <select id="role-pick" className="input input-auto" value={roleId} onChange={(e) => setRoleId(e.target.value)}>
                  <option value="">Select a role…</option>
                  {d.roles.map((r) => <option key={r.id} value={r.id}>{r.name}</option>)}
                </select>
                <button type="submit" className="btn btn-primary" disabled={!who || !roleId || act.busy}>Assign role</button>
              </div>
            </form>
            <div className="toolbar">
              <h3 className="h5">Roles</h3>
              <span className="spacer" />
              <button type="button" className="btn btn-sm" onClick={() => setCreating(!creating)} data-testid="new-org-role">Create a role</button>
            </div>
            {creating && <RoleForm kind="org" ctx={ctx} perms={d.perms} onDone={() => { setCreating(false); data.reload(); }} />}
            <ul className="box list" data-testid="org-roles">
              {d.roles.map((r: OrgRole) => {
                const a = d.assigned.find((x) => x.id === r.id);
                return (
                  <li key={r.id} className="list-row org-role-row" data-testid={`org-role-${r.name}`}>
                    <div className="grow">
                      <strong>{r.name}</strong> <span className="badge">{r.source === "Predefined" ? "Predefined" : "Custom"}</span>
                      {r.description && <div className="muted small">{r.description}</div>}
                      <div className="muted small">{r.permissions.length} permissions{r.base_role ? ` · base repository role: ${permLabel[r.base_role] || r.base_role}` : ""}</div>
                      {a && (a.users.length > 0 || a.teams.length > 0) && (
                        <ul className="org-assignments small">
                          {a.users.filter((u) => u.assignment !== "indirect").map((u) => (
                            <li key={`u${u.id}`} data-testid={`role-assignee-${u.login}`}>
                              <Avatar user={u} size={16} /> {u.login}
                              <button type="button" className="btn-link small" onClick={async () => { if (await act.run(() => orgApi.revokeUserRole(ctx.login, u.login, r.id))) data.reload(); }}>Remove</button>
                            </li>
                          ))}
                          {a.teams.map((t) => (
                            <li key={`t${t.id}`} data-testid={`role-assignee-team-${t.slug}`}>
                              Team {t.name}
                              <button type="button" className="btn-link small" onClick={async () => { if (await act.run(() => orgApi.revokeTeamRole(ctx.login, t.slug, r.id))) data.reload(); }}>Remove</button>
                            </li>
                          ))}
                        </ul>
                      )}
                    </div>
                    {r.source !== "Predefined" && (
                      <button type="button" className="btn btn-sm btn-danger-outline" disabled={act.busy}
                        onClick={async () => { if (confirm(`Delete the ${r.name} role?`) && await act.run(() => orgApi.deleteOrgRole(ctx.login, r.id))) data.reload(); }}>Delete</button>
                    )}
                  </li>
                );
              })}
            </ul>
          </>
        )}
      </Async>
    </Section>
  );
}

function RoleForm({ kind, ctx, perms, role, onDone }: {
  kind: "org" | "repo";
  ctx: OrgCtx;
  perms: { name: string; description: string }[];
  role?: RepoRole;
  onDone: () => void;
}) {
  const [name, setName] = useState(role?.name || "");
  const [description, setDescription] = useState(role?.description || "");
  const [base, setBase] = useState(role?.base_role || (kind === "repo" ? "read" : ""));
  const [sel, setSel] = useState<string[]>(role?.permissions || []);
  const [filter, setFilter] = useState("");
  const act = useAction();
  const shown = perms.filter((p) => !filter || p.name.includes(filter) || p.description.toLowerCase().includes(filter.toLowerCase()));
  return (
    <form className="box box-body" aria-label={role ? "Edit role" : "Create role"} onSubmit={async (e) => {
      e.preventDefault();
      const ok = await act.run(() => {
        if (kind === "org") return orgApi.createOrgRole(ctx.login, { name, description: description || undefined, permissions: sel, base_role: base || undefined });
        if (role) return orgApi.updateRepoRole(ctx.login, role.id, { name, description, base_role: base, permissions: sel });
        return orgApi.createRepoRole(ctx.login, { name, description: description || undefined, base_role: base, permissions: sel });
      });
      if (ok) onDone();
    }}>
      {act.error !== undefined && <ErrorBox error={act.error} />}
      <div className="form-group">
        <label htmlFor={`${kind}-role-name`}>Name</label>
        <input id={`${kind}-role-name`} className="input" required value={name} onChange={(e) => setName(e.target.value)} />
      </div>
      <div className="form-group">
        <label htmlFor={`${kind}-role-desc`}>Description</label>
        <input id={`${kind}-role-desc`} className="input" value={description} onChange={(e) => setDescription(e.target.value)} />
      </div>
      <div className="form-group">
        <label htmlFor={`${kind}-role-base`}>{kind === "repo" ? "Choose a role to inherit" : "Base repository role (optional)"}</label>
        <select id={`${kind}-role-base`} className="input input-auto" value={base} onChange={(e) => setBase(e.target.value)}>
          {kind === "org" && <option value="">None</option>}
          {["read", "triage", "write", "maintain", ...(kind === "org" ? ["admin"] : [])].map((b) => <option key={b} value={b}>{permLabel[b]}</option>)}
        </select>
      </div>
      <fieldset className="form-group">
        <legend>Add permissions</legend>
        <input className="input input-sm" type="search" placeholder="Filter permissions" aria-label="Filter permissions" value={filter} onChange={(e) => setFilter(e.target.value)} />
        <div className="org-perm-list">
          {shown.map((p) => (
            <label key={p.name} className="check-row small">
              <input type="checkbox" checked={sel.includes(p.name)} data-testid={`perm-${p.name}`}
                onChange={(e) => setSel(e.target.checked ? [...sel, p.name] : sel.filter((x) => x !== p.name))} />
              <span><code>{p.name}</code> <span className="muted">{p.description}</span></span>
            </label>
          ))}
        </div>
      </fieldset>
      <div className="form-actions">
        <button type="submit" className="btn btn-primary" disabled={act.busy || !name.trim() || (kind === "org" && sel.length === 0)}>{role ? "Update role" : "Create role"}</button>
        <button type="button" className="btn" onClick={onDone}>Cancel</button>
      </div>
    </form>
  );
}

// ---------------------------------------------------------------------------------------------
// Repository roles

function RepoRoles({ ctx }: { ctx: OrgCtx }) {
  const data = useAsync(async () => ({ roles: await orgApi.repoRoles(ctx.login), perms: await orgApi.repoPermissions(ctx.login) }), [ctx.login]);
  const [editing, setEditing] = useState<RepoRole | "new" | null>(null);
  const act = useAction();
  return (
    <Section title="Repository roles" desc="Custom repository roles add fine-grained permissions on top of a base role. Assign them to people and teams in a repository's collaborator settings or a team's repositories.">
      <ul className="box list">
        {["Read", "Triage", "Write", "Maintain", "Admin"].map((r) => (
          <li key={r} className="list-row"><strong className="grow">{r}</strong><span className="badge">Predefined</span></li>
        ))}
      </ul>
      {act.error !== undefined && <ErrorBox error={act.error} />}
      <Async state={data}>
        {(d) => (
          <>
            <div className="toolbar">
              <h3 className="h5">Custom roles</h3>
              <span className="spacer" />
              <button type="button" className="btn btn-sm btn-primary" onClick={() => setEditing("new")} data-testid="new-repo-role">Create a role</button>
            </div>
            {editing && <RoleForm kind="repo" ctx={ctx} perms={d.perms} role={editing === "new" ? undefined : editing}
              onDone={() => { setEditing(null); data.reload(); }} />}
            {d.roles.length === 0 ? <Blankslate title="No custom repository roles yet." /> : (
              <ul className="box list" data-testid="repo-roles">
                {d.roles.map((r) => (
                  <li key={r.id} className="list-row" data-testid={`repo-role-${r.name}`}>
                    <div className="grow">
                      <strong>{r.name}</strong> <span className="muted small">inherits {permLabel[r.base_role] || r.base_role}</span>
                      {r.description && <div className="muted small">{r.description}</div>}
                      {r.permissions.length > 0 && <div className="muted small">{r.permissions.join(", ")}</div>}
                    </div>
                    <button type="button" className="btn btn-sm" onClick={() => setEditing(r)}>Edit</button>
                    <button type="button" className="btn btn-sm btn-danger-outline" disabled={act.busy}
                      onClick={async () => { if (confirm(`Delete the ${r.name} role?`) && await act.run(() => orgApi.deleteRepoRole(ctx.login, r.id))) data.reload(); }}>Delete</button>
                  </li>
                ))}
              </ul>
            )}
          </>
        )}
      </Async>
    </Section>
  );
}

// ---------------------------------------------------------------------------------------------
// Blocked users

function BlockedUsers({ ctx }: { ctx: OrgCtx }) {
  const data = useAsync(() => orgApi.blocks(ctx.login), [ctx.login]);
  const [login, setLogin] = useState("");
  const act = useAction();
  return (
    <Section title="Blocked users" desc="Blocked users cannot be invited to the organization, and cannot open or comment on issues and pull requests in its repositories. Their pending invitations fail.">
      <form className="toolbar" onSubmit={async (e) => { e.preventDefault(); if (await act.run(() => orgApi.block(ctx.login, login.trim()))) { setLogin(""); data.reload(); } }}>
        <label htmlFor="block-login" className="sr-only">Username</label>
        <input id="block-login" className="input grow" placeholder="Search by username" value={login} onChange={(e) => setLogin(e.target.value)} />
        <button type="submit" className="btn btn-danger-outline" disabled={!login.trim() || act.busy}>Block user</button>
      </form>
      {act.error !== undefined && <ErrorBox error={act.error} />}
      <Async state={data}>
        {(list) => list.length === 0 ? <Blankslate title="You have not blocked any users." /> : (
          <ul className="box list" data-testid="blocked-users">
            {list.map((u) => (
              <li key={u.id} className="list-row org-person" data-testid={`blocked-${u.login}`}>
                <Avatar user={u} size={24} />
                <Link to={`/${u.login}`} className="grow">{u.login}</Link>
                <button type="button" className="btn btn-sm" disabled={act.busy}
                  onClick={async () => { if (await act.run(() => orgApi.unblock(ctx.login, u.login))) data.reload(); }}>Unblock</button>
              </li>
            ))}
          </ul>
        )}
      </Async>
    </Section>
  );
}

// ---------------------------------------------------------------------------------------------
// Personal access tokens

function PersonalAccessTokens({ ctx }: { ctx: OrgCtx }) {
  const data = useAsync(async () => ({ requests: await orgApi.patRequests(ctx.login), grants: await orgApi.patGrants(ctx.login) }), [ctx.login]);
  const act = useAction();
  return (
    <Section title="Personal access tokens" desc={<>Fine-grained personal access tokens need approval to access the organization's resources.
      forge issues classic tokens only (scoped to the user's own permissions, see <Link to="/settings/tokens">Access tokens</Link>),
      so there are no fine-grained requests or grants to review; this page lists them when they exist.</>}>
      {act.error !== undefined && <ErrorBox error={act.error} />}
      <Async state={data}>
        {(d) => (
          <>
            <h3 className="h5">Pending requests</h3>
            {d.requests.length === 0 ? <Blankslate title="No pending requests." /> : (
              <ul className="box list" data-testid="pat-requests">
                {d.requests.map((r) => (
                  <li key={r.id} className="list-row">
                    <span className="grow">{r.token_name || `Request ${r.id}`} by {r.owner.login} <RelTime iso={r.created_at} /></span>
                    <button type="button" className="btn btn-sm btn-primary" onClick={async () => { if (await act.run(() => orgApi.reviewPatRequest(ctx.login, r.id, "approve"))) data.reload(); }}>Approve</button>
                    <button type="button" className="btn btn-sm btn-danger-outline" onClick={async () => { if (await act.run(() => orgApi.reviewPatRequest(ctx.login, r.id, "deny"))) data.reload(); }}>Deny</button>
                  </li>
                ))}
              </ul>
            )}
            <h3 className="h5">Active tokens</h3>
            {d.grants.length === 0 ? <Blankslate title="No fine-grained tokens have access to this organization." /> : (
              <ul className="box list" data-testid="pat-grants">
                {d.grants.map((g) => (
                  <li key={g.id} className="list-row">
                    <span className="grow">{g.token_name || `Token ${g.id}`} by {g.owner.login}, granted <RelTime iso={g.access_granted_at} /></span>
                    <button type="button" className="btn btn-sm btn-danger-outline" onClick={async () => { if (await act.run(() => orgApi.revokePat(ctx.login, g.id))) data.reload(); }}>Revoke</button>
                  </li>
                ))}
              </ul>
            )}
          </>
        )}
      </Async>
    </Section>
  );
}

// ---------------------------------------------------------------------------------------------
// Audit log

function AuditLog({ ctx }: { ctx: OrgCtx }) {
  const [sp, setSp] = useSearchParams();
  const phrase = sp.get("q") || "";
  const page = Number(sp.get("page") || 1);
  const [q, setQ] = useState(phrase);
  const data = useAsync(() => orgApi.auditLog(ctx.login, phrase, page), [ctx.login, phrase, page]);
  return (
    <Section title="Audit log" desc="Events performed by members of the organization, newest first. Filter with words or qualifiers such as action:team.create, actor:login, user:login, repo:name.">
      <form className="toolbar" role="search" onSubmit={(e) => { e.preventDefault(); setSp(q ? { q } : {}); }}>
        <label htmlFor="audit-q" className="sr-only">Filter audit logs</label>
        <input id="audit-q" className="input grow" type="search" placeholder="Filter audit logs" value={q} onChange={(e) => setQ(e.target.value)} />
        <button type="submit" className="btn">Search</button>
      </form>
      <Async state={data}>
        {(p) => p.items.length === 0 ? <Blankslate title="No audit log events found." /> : (
          <ul className="box list" data-testid="audit-log">
            {p.items.map((ev) => (
              <li key={ev._document_id} className="list-row org-audit-row" data-testid="audit-event">
                <div className="grow">
                  <strong>{ev.actor || "system"}</strong> <code className="org-audit-action">{ev.action}</code>
                  {ev.user && <span className="muted"> user: {ev.user}</span>}
                  {ev.team && <span className="muted"> team: {ev.team}</span>}
                  {ev.repo && <span className="muted"> repo: {ev.repo}</span>}
                </div>
                <span className="muted small"><RelTime iso={new Date(ev["@timestamp"]).toISOString()} /></span>
              </li>
            ))}
          </ul>
        )}
      </Async>
      {data.data && (data.data.next || page > 1) && (
        <Pagination page={page} next={data.data.next} prev={page > 1 ? page - 1 : undefined}
          onPage={(n) => setSp(phrase ? { q: phrase, page: String(n) } : { page: String(n) })} />
      )}
    </Section>
  );
}

// ---------------------------------------------------------------------------------------------
// Migrations (organization exports)

function Migrations({ ctx }: { ctx: OrgCtx }) {
  const data = useAsync(async () => ({ list: await orgApi.migrations(ctx.login), repos: await orgApi.allRepos(ctx.login) }), [ctx.login]);
  const [sel, setSel] = useState<string[]>([]);
  const [lock, setLock] = useState(false);
  const [noAttach, setNoAttach] = useState(false);
  const act = useAction();
  const busyStates = useMemo(() => (data.data?.list || []).some((m) => m.state === "pending" || m.state === "exporting"), [data.data]);
  useEffect(() => {
    if (!busyStates) return;
    const t = setInterval(() => data.reload(), 2000);
    return () => clearInterval(t);
  }, [busyStates, data]);
  return (
    <Section title="Export" desc="Export repositories of the organization (with issues, pull requests, releases and more) as a migration archive.">
      {act.error !== undefined && <ErrorBox error={act.error} />}
      <Async state={data}>
        {(d) => (
          <>
            <form className="box box-body" onSubmit={async (e) => {
              e.preventDefault();
              if (await act.run(() => orgApi.startMigration(ctx.login, { repositories: sel, lock_repositories: lock, exclude_attachments: noAttach }))) { setSel([]); data.reload(); }
            }}>
              <h3 className="h5">Start an export</h3>
              {d.repos.length === 0 ? <p className="muted small">This organization has no repositories to export.</p> : (
                <fieldset className="form-group org-perm-list">
                  <legend className="sr-only">Repositories</legend>
                  {d.repos.map((r) => (
                    <label key={r.id} className="check-row small">
                      <input type="checkbox" checked={sel.includes(r.name)} data-testid={`export-repo-${r.name}`}
                        onChange={(e) => setSel(e.target.checked ? [...sel, r.name] : sel.filter((x) => x !== r.name))} /> {r.name}
                    </label>
                  ))}
                </fieldset>
              )}
              <label className="check-row form-group"><input type="checkbox" checked={lock} onChange={(e) => setLock(e.target.checked)} /> Lock repositories while exporting</label>
              <label className="check-row form-group"><input type="checkbox" checked={noAttach} onChange={(e) => setNoAttach(e.target.checked)} /> Exclude attachments</label>
              <button type="submit" className="btn btn-primary" disabled={sel.length === 0 || act.busy} data-testid="start-export">Start export</button>
            </form>
            <h3 className="h5">Recent exports</h3>
            {d.list.length === 0 ? <Blankslate title="No exports yet." /> : (
              <ul className="box list" data-testid="migrations">
                {d.list.map((m) => (
                  <li key={m.id} className="list-row" data-testid={`migration-${m.id}`}>
                    <div className="grow">
                      <strong>Export #{m.id}</strong> <span className="badge" data-testid="migration-state">{m.state}</span>
                      <div className="muted small">{m.repositories.map((r) => r.name).join(", ")} · started <RelTime iso={m.created_at} />{m.lock_repositories ? " · locked" : ""}</div>
                    </div>
                    {m.state === "exported" && (
                      <>
                        <a className="btn btn-sm" href={orgApi.migrationArchiveUrl(ctx.login, m.id)} data-testid="download-export">Download</a>
                        <button type="button" className="btn btn-sm btn-danger-outline" disabled={act.busy}
                          onClick={async () => { if (await act.run(() => orgApi.deleteMigrationArchive(ctx.login, m.id))) data.reload(); }}>Delete archive</button>
                      </>
                    )}
                  </li>
                ))}
              </ul>
            )}
          </>
        )}
      </Async>
    </Section>
  );
}

/** The orgs area's own org settings sections. */
export const ownOrgSettings: SettingsSection<OrgCtx>[] = [
  { key: "profile", path: "profile", label: "General", order: 0, element: General },
  { key: "org-roles", path: "organization_roles", label: "Organization roles", group: "Access", order: 100, element: OrgRoles },
  { key: "repo-roles", path: "repository_roles", label: "Repository roles", group: "Access", order: 110, element: RepoRoles },
  { key: "member-privileges", path: "member_privileges", label: "Member privileges", group: "Access", order: 120, element: MemberPrivileges },
  { key: "blocked-users", path: "blocked_users", label: "Blocked users", group: "Moderation", order: 300, element: BlockedUsers },
  { key: "pats", path: "personal-access-tokens", label: "Personal access tokens", group: "Third-party Access", order: 600, element: PersonalAccessTokens },
  { key: "audit-log", path: "audit-log", label: "Audit log", group: "Archive", order: 900, element: AuditLog },
  { key: "migrations", path: "migrations", label: "Export", group: "Archive", order: 910, element: Migrations },
];
