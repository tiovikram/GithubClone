// Teams: /orgs/:org/teams, /orgs/:org/new-team, /orgs/:org/teams/:slug[/members|/teams|/repositories|/settings].
import { useEffect, useMemo, useState, type FormEvent } from "react";
import { Link, NavLink, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { useAuth } from "../../auth";
import { Async, Avatar, Blankslate, ErrorBox, Flash, NotFoundView } from "../../components/common";
import { LockIcon } from "../../components/icons";
import { useAsync, useTitle } from "../../hooks";
import { orgApi, type Team } from "./api";
import { OrgPage, permLabel, TeamIcon, useAction, type OrgState } from "./common";

/** Teams sorted so children follow their parent, with depth for indentation. */
function tree(teams: Team[]): { t: Team; depth: number }[] {
  const out: { t: Team; depth: number }[] = [];
  const kids = (pid: number | null) => teams.filter((t) => (t.parent?.id ?? null) === pid).sort((a, b) => a.name.localeCompare(b.name));
  const walk = (pid: number | null, depth: number) => {
    for (const t of kids(pid)) {
      out.push({ t, depth });
      walk(t.id, depth + 1);
    }
  };
  walk(null, 0);
  // teams whose parent isn't visible
  for (const t of teams) if (!out.some((x) => x.t.id === t.id)) out.push({ t, depth: 0 });
  return out;
}

export function TeamsPage() {
  const { org = "" } = useParams();
  useTitle(`Teams · ${org}`);
  return <OrgPage login={org} active="teams">{(st) => (st.isMember ? <TeamList st={st} /> : <NotFoundView />)}</OrgPage>;
}

function TeamList({ st }: { st: OrgState }) {
  const { org } = st;
  const teams = useAsync(() => orgApi.teams(org.login), [org.login]);
  const [q, setQ] = useState("");
  return (
    <>
      <div className="toolbar">
        <label htmlFor="team-q" className="sr-only">Find a team</label>
        <input id="team-q" type="search" className="input grow" placeholder="Find a team…" value={q} onChange={(e) => setQ(e.target.value)} />
        <Link to={`/orgs/${org.login}/new-team`} className="btn btn-primary" data-testid="new-team">New team</Link>
      </div>
      <Async state={teams}>
        {(list) => {
          const rows = tree(list).filter(({ t }) => !q || t.name.toLowerCase().includes(q.toLowerCase()));
          return rows.length === 0 ? (
            <Blankslate title={q ? "No teams matched." : "No teams yet"} icon={<TeamIcon size={24} />}>
              {!q && "Teams are groups of organization members that reflect your company or group's structure, with cascading access permissions and mentions."}
            </Blankslate>
          ) : (
            <ul className="box list" data-testid="team-list">
              <li className="box-header small muted">{list.length} {list.length === 1 ? "team" : "teams"} in the {org.login} organization</li>
              {rows.map(({ t, depth }) => (
                <li key={t.id} className="list-row" style={{ paddingLeft: 16 + depth * 24 }} data-testid={`team-row-${t.slug}`}>
                  <div className="grow">
                    <Link to={`/orgs/${org.login}/teams/${t.slug}`}><strong>{t.name}</strong></Link>{" "}
                    {t.privacy === "secret" && <span className="badge"><LockIcon size={12} /> Secret</span>}
                    {t.description && <div className="muted small">{t.description}</div>}
                  </div>
                </li>
              ))}
            </ul>
          );
        }}
      </Async>
    </>
  );
}

function TeamFields({ org, team, onSubmit, submitLabel, busy }: {
  org: string;
  team?: Team;
  submitLabel: string;
  busy: boolean;
  onSubmit: (v: { name: string; description: string; privacy: string; parent_team_id: number | null; notification_setting: string }) => void;
}) {
  const [sp] = useSearchParams();
  const teams = useAsync(() => orgApi.teams(org), [org]);
  const [name, setName] = useState(team?.name || "");
  const [description, setDescription] = useState(team?.description || "");
  const [privacy, setPrivacy] = useState(team?.privacy || "closed");
  const [parent, setParent] = useState<string>(team?.parent ? String(team.parent.id) : "");
  const [notif, setNotif] = useState(team?.notification_setting || "notifications_enabled");
  useEffect(() => {
    const p = sp.get("parent");
    if (!team && p && teams.data) {
      const t = teams.data.find((x) => x.slug === p);
      if (t) setParent(String(t.id));
    }
  }, [teams.data, sp, team]);
  useEffect(() => {
    if (parent) setPrivacy("closed");
  }, [parent]);
  const parents = (teams.data || []).filter((t) => t.id !== team?.id && t.privacy !== "secret");
  return (
    <form className="box box-body" onSubmit={(e: FormEvent) => {
      e.preventDefault();
      onSubmit({ name, description, privacy, parent_team_id: parent ? Number(parent) : null, notification_setting: notif });
    }}>
      <div className="form-group">
        <label htmlFor="team-name">Team name</label>
        <input id="team-name" className="input" required value={name} onChange={(e) => setName(e.target.value)} />
        <p className="muted small">You'll use this name to mention this team in conversations.</p>
      </div>
      <div className="form-group">
        <label htmlFor="team-desc">Description</label>
        <input id="team-desc" className="input" value={description} onChange={(e) => setDescription(e.target.value)} placeholder="What is this team all about?" />
      </div>
      <div className="form-group">
        <label htmlFor="team-parent">Parent team</label>
        <select id="team-parent" className="input input-auto" value={parent} onChange={(e) => setParent(e.target.value)}>
          <option value="">No parent team</option>
          {parents.map((t) => <option key={t.id} value={t.id}>{t.name}</option>)}
        </select>
      </div>
      <fieldset className="form-group radio-group">
        <legend>Team visibility</legend>
        <label className="radio-row">
          <input type="radio" name="team-privacy" checked={privacy === "closed"} onChange={() => setPrivacy("closed")} />
          <span><strong>Visible</strong> <span className="muted small">(recommended)</span><br /><span className="muted small">A visible team can be seen and @mentioned by every member of this organization.</span></span>
        </label>
        <label className="radio-row">
          <input type="radio" name="team-privacy" checked={privacy === "secret"} disabled={!!parent} onChange={() => setPrivacy("secret")} data-testid="team-secret" />
          <span><strong>Secret</strong><br /><span className="muted small">A secret team can only be seen by its members and may not be nested.</span></span>
        </label>
      </fieldset>
      <fieldset className="form-group radio-group">
        <legend>Team notifications</legend>
        <label className="radio-row">
          <input type="radio" name="team-notif" checked={notif === "notifications_enabled"} onChange={() => setNotif("notifications_enabled")} />
          <span><strong>Enabled</strong><br /><span className="muted small">Everyone will be notified when the team is @mentioned.</span></span>
        </label>
        <label className="radio-row">
          <input type="radio" name="team-notif" checked={notif === "notifications_disabled"} onChange={() => setNotif("notifications_disabled")} data-testid="team-notif-off" />
          <span><strong>Disabled</strong><br /><span className="muted small">No one will receive notifications.</span></span>
        </label>
      </fieldset>
      <button type="submit" className="btn btn-primary" disabled={busy || !name.trim()}>{busy ? "Saving…" : submitLabel}</button>
    </form>
  );
}

export function NewTeamPage() {
  const { org = "" } = useParams();
  const nav = useNavigate();
  const act = useAction();
  useTitle(`Create new team · ${org}`);
  return (
    <OrgPage login={org} active="teams">
      {(st) => !st.isMember ? <NotFoundView /> : (
        <div className="narrow-form org-narrow">
          <h2 className="h3">Create new team</h2>
          {act.error !== undefined && <ErrorBox error={act.error} />}
          <TeamFields org={st.org.login} submitLabel="Create team" busy={act.busy} onSubmit={async (v) => {
            let slug = "";
            if (await act.run(async () => { slug = (await orgApi.createTeam(st.org.login, { ...v, parent_team_id: v.parent_team_id ?? undefined })).slug; })) {
              nav(`/orgs/${st.org.login}/teams/${slug}`);
            }
          }} />
        </div>
      )}
    </OrgPage>
  );
}

type TeamTab = "members" | "teams" | "repositories" | "settings";

export function TeamPage({ tab }: { tab: TeamTab }) {
  const { org = "", slug = "" } = useParams();
  useTitle(`${slug} · ${org}`);
  return <OrgPage login={org} active="teams">{(st) => (st.isMember ? <TeamView st={st} slug={slug} tab={tab} /> : <NotFoundView />)}</OrgPage>;
}

function TeamView({ st, slug, tab }: { st: OrgState; slug: string; tab: TeamTab }) {
  const { user: me } = useAuth();
  const team = useAsync(async () => {
    const [t, maintainers] = await Promise.all([orgApi.team(st.org.login, slug), orgApi.teamMembers(st.org.login, slug, "maintainer")]);
    return { t, maintainers };
  }, [st.org.login, slug]);
  return (
    <Async state={team}>
      {({ t, maintainers }) => {
        const canManage = st.isOwner || maintainers.some((m) => m.login === me?.login);
        const base = `/orgs/${st.org.login}/teams/${t.slug}`;
        const tabLink = (k: TeamTab, label: string, to: string, count?: number) => (
          <NavLink to={to} end className={() => "tabnav-tab" + (tab === k ? " selected" : "")} data-testid={`team-tab-${k}`}>
            {label} {count !== undefined && <span className="counter">{count}</span>}
          </NavLink>
        );
        return (
          <div className="org-team">
            <div className="org-team-head">
              <nav className="muted small" aria-label="Breadcrumb">
                <Link to={`/orgs/${st.org.login}/teams`}>Teams</Link>
                {t.parent && <> / <Link to={`/orgs/${st.org.login}/teams/${t.parent.slug}`}>{t.parent.name}</Link></>}
              </nav>
              <h2 className="h3">{t.name} {t.privacy === "secret" ? <span className="badge"><LockIcon size={12} /> Secret</span> : <span className="badge">Visible</span>}</h2>
              {t.description && <p className="muted">{t.description}</p>}
            </div>
            <nav className="tabnav" aria-label="Team">
              {tabLink("members", "Members", base, t.members_count)}
              {tabLink("teams", "Teams", `${base}/teams`)}
              {tabLink("repositories", "Repositories", `${base}/repositories`, t.repos_count)}
              {canManage && tabLink("settings", "Settings", `${base}/settings`)}
            </nav>
            {tab === "members" && <TeamMembers st={st} team={t} canManage={canManage} reload={team.reload} />}
            {tab === "teams" && <ChildTeams st={st} team={t} />}
            {tab === "repositories" && <TeamRepos st={st} team={t} canManage={canManage} reload={team.reload} />}
            {tab === "settings" && (canManage ? <TeamSettings st={st} team={t} /> : <NotFoundView />)}
          </div>
        );
      }}
    </Async>
  );
}

function TeamMembers({ st, team, canManage, reload }: { st: OrgState; team: Team; canManage: boolean; reload: () => void }) {
  const org = st.org.login;
  const data = useAsync(async () => {
    const [all, maint] = await Promise.all([orgApi.teamMembers(org, team.slug), orgApi.teamMembers(org, team.slug, "maintainer")]);
    const pending = st.isOwner ? await orgApi.teamInvitations(org, team.slug) : [];
    return { all, maint: new Set(maint.map((m) => m.login)), pending };
  }, [org, team.slug]);
  const [login, setLogin] = useState("");
  const [role, setRole] = useState<"member" | "maintainer">("member");
  const act = useAction();
  const [notice, setNotice] = useState("");
  const done = (msg: string) => { setNotice(msg); data.reload(); reload(); };
  return (
    <>
      {canManage && (
        <form className="toolbar" onSubmit={async (e) => {
          e.preventDefault();
          const l = login.trim();
          if (await act.run(() => orgApi.setTeamMembership(org, team.slug, l, role))) { setLogin(""); done(`Added ${l} to ${team.name}.`); }
        }}>
          <label htmlFor="team-add-member" className="sr-only">Username</label>
          <input id="team-add-member" className="input grow" placeholder="Add a member by username" value={login} onChange={(e) => setLogin(e.target.value)} />
          <label htmlFor="team-add-role" className="sr-only">Role</label>
          <select id="team-add-role" className="input input-auto" value={role} onChange={(e) => setRole(e.target.value as "member" | "maintainer")}>
            <option value="member">Member</option>
            <option value="maintainer">Maintainer</option>
          </select>
          <button type="submit" className="btn btn-primary" disabled={!login.trim() || act.busy}>Add member</button>
        </form>
      )}
      {notice && <Flash kind="success">{notice}</Flash>}
      {act.error !== undefined && <ErrorBox error={act.error} />}
      <Async state={data}>
        {(d) => (
          <>
            {d.all.length === 0 ? <Blankslate title="This team doesn't have any members yet." /> : (
              <ul className="box list" data-testid="team-members">
                {d.all.map((u) => (
                  <li key={u.id} className="list-row org-person" data-testid={`team-member-${u.login}`}>
                    <Avatar user={u} size={32} />
                    <Link to={`/${u.login}`} className="grow">{u.login}</Link>
                    {canManage ? (
                      <>
                        <label htmlFor={`tm-role-${u.login}`} className="sr-only">Role of {u.login}</label>
                        <select id={`tm-role-${u.login}`} className="input input-sm input-auto" value={d.maint.has(u.login) ? "maintainer" : "member"} disabled={act.busy}
                          onChange={async (e) => { const r = e.target.value as "member" | "maintainer"; if (await act.run(() => orgApi.setTeamMembership(org, team.slug, u.login, r))) done(`${u.login} is now a team ${r}.`); }}>
                          <option value="member">Member</option>
                          <option value="maintainer">Maintainer</option>
                        </select>
                        <button type="button" className="btn btn-sm btn-danger-outline" disabled={act.busy}
                          onClick={async () => { if (await act.run(() => orgApi.removeTeamMember(org, team.slug, u.login))) done(`Removed ${u.login} from ${team.name}.`); }}>
                          Remove
                        </button>
                      </>
                    ) : <span className="badge">{d.maint.has(u.login) ? "Maintainer" : "Member"}</span>}
                  </li>
                ))}
              </ul>
            )}
            {d.pending.length > 0 && (
              <>
                <h3 className="h5">Pending invitations</h3>
                <ul className="box list">
                  {d.pending.map((i) => <li key={i.id} className="list-row">{i.login || i.email} <span className="muted small">invited to the organization with this team</span></li>)}
                </ul>
              </>
            )}
          </>
        )}
      </Async>
    </>
  );
}

function ChildTeams({ st, team }: { st: OrgState; team: Team }) {
  const data = useAsync(() => orgApi.childTeams(st.org.login, team.slug), [st.org.login, team.slug]);
  return (
    <>
      <div className="toolbar">
        <span className="spacer" />
        <Link to={`/orgs/${st.org.login}/new-team?parent=${team.slug}`} className="btn btn-sm" data-testid="add-child-team">Add a team</Link>
      </div>
      <Async state={data}>
        {(list) => list.length === 0 ? <Blankslate title="This team has no child teams." /> : (
          <ul className="box list" data-testid="child-teams">
            {list.map((t) => (
              <li key={t.id} className="list-row"><Link to={`/orgs/${st.org.login}/teams/${t.slug}`}>{t.name}</Link>
                {t.description && <span className="muted small"> — {t.description}</span>}</li>
            ))}
          </ul>
        )}
      </Async>
    </>
  );
}

const BASE_PERMS = ["pull", "triage", "push", "maintain", "admin"];

function TeamRepos({ st, team, canManage, reload }: { st: OrgState; team: Team; canManage: boolean; reload: () => void }) {
  const org = st.org.login;
  const data = useAsync(async () => {
    const [repos, all, custom] = await Promise.all([
      orgApi.teamRepos(org, team.slug),
      canManage ? orgApi.allRepos(org) : Promise.resolve([]),
      st.isOwner ? orgApi.repoRoles(org).catch(() => []) : Promise.resolve([]),
    ]);
    return { repos, all, custom };
  }, [org, team.slug, canManage]);
  const [repo, setRepo] = useState("");
  const [perm, setPerm] = useState("pull");
  const act = useAction();
  const done = () => { data.reload(); reload(); };
  const options = (custom: { name: string }[]) => [
    ...BASE_PERMS.map((p) => <option key={p} value={p}>{permLabel[p]}</option>),
    ...custom.map((r) => <option key={`c-${r.name}`} value={r.name}>{r.name}</option>),
  ];
  return (
    <Async state={data}>
      {(d) => {
        const free = d.all.filter((r) => !d.repos.some((x) => x.id === r.id));
        const current = (r: (typeof d.repos)[number]) => {
          const n = r.role_name || "";
          const m: Record<string, string> = { read: "pull", write: "push" };
          return m[n] || n || (r.permissions?.admin ? "admin" : r.permissions?.maintain ? "maintain" : r.permissions?.push ? "push" : r.permissions?.triage ? "triage" : "pull");
        };
        return (
          <>
            {canManage && free.length > 0 && (
              <form className="toolbar" onSubmit={async (e) => {
                e.preventDefault();
                if (repo && await act.run(() => orgApi.setTeamRepo(org, team.slug, org, repo, perm))) { setRepo(""); done(); }
              }}>
                <label htmlFor="team-add-repo" className="sr-only">Repository</label>
                <select id="team-add-repo" className="input input-auto grow" value={repo} onChange={(e) => setRepo(e.target.value)}>
                  <option value="">Select a repository…</option>
                  {free.map((r) => <option key={r.id} value={r.name}>{r.name}</option>)}
                </select>
                <label htmlFor="team-add-perm" className="sr-only">Permission</label>
                <select id="team-add-perm" className="input input-auto" value={perm} onChange={(e) => setPerm(e.target.value)}>{options(d.custom)}</select>
                <button type="submit" className="btn btn-primary" disabled={!repo || act.busy}>Add repository</button>
              </form>
            )}
            {act.error !== undefined && <ErrorBox error={act.error} />}
            {d.repos.length === 0 ? <Blankslate title="This team doesn't have access to any repositories yet." /> : (
              <ul className="box list" data-testid="team-repos">
                {d.repos.map((r) => (
                  <li key={r.id} className="list-row" data-testid={`team-repo-${r.name}`}>
                    <Link to={`/${r.full_name}`} className="grow">{r.full_name}</Link>
                    {canManage ? (
                      <>
                        <label htmlFor={`tr-perm-${r.name}`} className="sr-only">Permission on {r.name}</label>
                        <select id={`tr-perm-${r.name}`} className="input input-sm input-auto" value={current(r)} disabled={act.busy}
                          onChange={async (e) => { const p = e.target.value; if (await act.run(() => orgApi.setTeamRepo(org, team.slug, r.owner.login, r.name, p))) done(); }}>
                          {options(d.custom)}
                        </select>
                        <button type="button" className="btn btn-sm btn-danger-outline" disabled={act.busy}
                          onClick={async () => { if (await act.run(() => orgApi.removeTeamRepo(org, team.slug, r.owner.login, r.name))) done(); }}>Remove</button>
                      </>
                    ) : <span className="badge">{permLabel[current(r)] || current(r)}</span>}
                  </li>
                ))}
              </ul>
            )}
          </>
        );
      }}
    </Async>
  );
}

function TeamSettings({ st, team }: { st: OrgState; team: Team }) {
  const org = st.org.login;
  const nav = useNavigate();
  const act = useAction();
  const del = useAction();
  const [saved, setSaved] = useState(false);
  const groups = useAsync(async () => {
    const [all, linked] = await Promise.all([orgApi.externalGroups(org).catch(() => []), orgApi.teamGroups(org, team.slug).catch(() => [])]);
    return { all, linked };
  }, [org, team.slug]);
  const [gid, setGid] = useState("");
  const gact = useAction();
  const linkedIds = useMemo(() => new Set((groups.data?.linked || []).map((g) => g.group_id)), [groups.data]);
  return (
    <div className="org-narrow">
      <h2 className="h4">Team settings</h2>
      {saved && <Flash kind="success">Team settings saved.</Flash>}
      {act.error !== undefined && <ErrorBox error={act.error} />}
      <TeamFields org={org} team={team} submitLabel="Save changes" busy={act.busy} onSubmit={async (v) => {
        setSaved(false);
        let slug = team.slug;
        if (await act.run(async () => { slug = (await orgApi.updateTeam(org, team.slug, v)).slug; })) {
          setSaved(true);
          if (slug !== team.slug) nav(`/orgs/${org}/teams/${slug}/settings`, { replace: true });
        }
      }} />

      <h2 className="h4">External groups</h2>
      <div className="box box-body">
        <p className="muted small">Synchronize this team's members with a group from your identity provider. Group members are added to the team.</p>
        {gact.error !== undefined && <ErrorBox error={gact.error} />}
        <Async state={groups}>
          {(g) => (
            <>
              {g.linked.length > 0 && (
                <ul className="list" data-testid="team-groups">
                  {g.linked.map((x) => (
                    <li key={x.group_id} className="list-row">
                      <span className="grow">{x.group_name}</span>
                      <button type="button" className="btn btn-sm btn-danger-outline" disabled={gact.busy}
                        onClick={async () => { if (await gact.run(() => orgApi.unlinkGroup(org, team.slug))) groups.reload(); }}>Remove</button>
                    </li>
                  ))}
                </ul>
              )}
              {g.all.length === 0 ? <p className="muted small" data-testid="no-external-groups">No external groups have been provisioned for this organization (via SCIM).</p> : (
                <form className="toolbar" onSubmit={async (e) => { e.preventDefault(); if (gid && await gact.run(() => orgApi.linkGroup(org, team.slug, Number(gid)))) { setGid(""); groups.reload(); } }}>
                  <label htmlFor="team-group" className="sr-only">Group</label>
                  <select id="team-group" className="input input-auto" value={gid} onChange={(e) => setGid(e.target.value)}>
                    <option value="">Select a group…</option>
                    {g.all.filter((x) => !linkedIds.has(x.group_id)).map((x) => <option key={x.group_id} value={x.group_id}>{x.group_name}</option>)}
                  </select>
                  <button type="submit" className="btn" disabled={!gid || gact.busy}>Connect group</button>
                </form>
              )}
            </>
          )}
        </Async>
      </div>

      <h2 className="h4 c-danger">Danger zone</h2>
      <div className="box danger-zone box-body">
        <h3 className="h5">Delete this team</h3>
        <p className="muted small">Once deleted, it will be gone forever. Child teams are deleted too.</p>
        {del.error !== undefined && <ErrorBox error={del.error} />}
        <button type="button" className="btn btn-danger" disabled={del.busy} data-testid="delete-team"
          onClick={async () => {
            if (!confirm(`Delete the ${team.name} team?`)) return;
            if (await del.run(() => orgApi.deleteTeam(org, team.slug))) nav(`/orgs/${org}/teams`);
          }}>
          Delete this team
        </button>
      </div>
    </div>
  );
}
