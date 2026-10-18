// People: /orgs/:org/people (members), /orgs/:org/outside-collaborators,
// /orgs/:org/people/pending_invitations, /orgs/:org/people/failed_invitations,
// /orgs/:org/people/security_managers.
import { useMemo, useState, type FormEvent } from "react";
import { Link, NavLink, useParams, useSearchParams } from "react-router-dom";
import type { User } from "../../api";
import { useAuth } from "../../auth";
import { Async, Avatar, Blankslate, ErrorBox, Flash, NotFoundView, RelTime } from "../../components/common";
import { useAsync, useTitle } from "../../hooks";
import { orgApi, type Team } from "./api";
import { OrgPage, roleLabel, useAction, type OrgState } from "./common";

type View = "members" | "outside" | "pending" | "failed" | "security";

export function PeoplePage({ view }: { view: View }) {
  const { org = "" } = useParams();
  useTitle(`People · ${org}`);
  return (
    <OrgPage login={org} active="people">
      {(st) => {
        if (view !== "members" && !st.isOwner) return <NotFoundView />;
        return (
          <div className="org-people">
            <PeopleNav st={st} />
            <div className="org-people-main">
              {view === "members" && <Members st={st} />}
              {view === "outside" && <Outside st={st} />}
              {view === "pending" && <Invitations st={st} failed={false} />}
              {view === "failed" && <Invitations st={st} failed />}
              {view === "security" && <SecurityManagers st={st} />}
            </div>
          </div>
        );
      }}
    </OrgPage>
  );
}

function PeopleNav({ st }: { st: OrgState }) {
  const b = `/orgs/${st.org.login}`;
  const item = (to: string, label: string, key: string) => (
    <NavLink to={to} end className={({ isActive }) => "menu-item" + (isActive ? " selected" : "")} data-testid={`people-nav-${key}`}>{label}</NavLink>
  );
  return (
    <nav className="settings-nav org-people-nav" aria-label="People">
      {item(`${b}/people`, "Members", "members")}
      {st.isOwner && item(`${b}/outside-collaborators`, "Outside collaborators", "outside")}
      {st.isOwner && item(`${b}/people/pending_invitations`, "Invitations", "pending")}
      {st.isOwner && item(`${b}/people/failed_invitations`, "Failed invitations", "failed")}
      {st.isOwner && item(`${b}/people/security_managers`, "Security managers", "security")}
    </nav>
  );
}

function Members({ st }: { st: OrgState }) {
  const { org } = st;
  const { user: me } = useAuth();
  const [sp, setSp] = useSearchParams();
  const [q, setQ] = useState(sp.get("query") || "");
  const role = (sp.get("role") || "all") as "all" | "admin" | "member";
  const [inviting, setInviting] = useState(sp.get("invite") === "1");
  const data = useAsync(async () => {
    if (!st.isMember) return { all: await orgApi.publicMembers(org.login), owners: new Set<string>() };
    const [all, owners] = await Promise.all([orgApi.members(org.login), orgApi.members(org.login, "admin")]);
    return { all, owners: new Set(owners.map((u) => u.login)) };
  }, [org.login, st.isMember]);
  const act = useAction();
  const [notice, setNotice] = useState("");
  const list = useMemo(() => {
    const d = data.data;
    if (!d) return [];
    return d.all.filter((u) =>
      (!q || u.login.toLowerCase().includes(q.toLowerCase()) || (u.name || "").toLowerCase().includes(q.toLowerCase())) &&
      (role === "all" || (role === "admin") === d.owners.has(u.login)));
  }, [data.data, q, role]);
  const doAct = async (fn: () => Promise<unknown>, msg: string) => {
    setNotice("");
    if (await act.run(fn)) {
      setNotice(msg);
      data.reload();
      st.reload();
    }
  };
  return (
    <>
      <div className="toolbar">
        <label htmlFor="people-q" className="sr-only">Find a member</label>
        <input id="people-q" className="input grow" type="search" placeholder="Find a member…" value={q} onChange={(e) => setQ(e.target.value)} />
        <label htmlFor="people-role" className="sr-only">Role</label>
        <select id="people-role" className="input input-auto" value={role}
          onChange={(e) => setSp(e.target.value === "all" ? {} : { role: e.target.value }, { replace: true })}>
          <option value="all">Role: Everyone</option>
          <option value="admin">Owners</option>
          <option value="member">Members</option>
        </select>
        {st.isOwner && <button type="button" className="btn btn-primary" onClick={() => setInviting(!inviting)} data-testid="invite-member">Invite member</button>}
      </div>
      {inviting && st.isOwner && <InviteForm st={st} onDone={(msg) => { setInviting(false); setNotice(msg); }} />}
      {notice && <Flash kind="success">{notice}</Flash>}
      {act.error !== undefined && <ErrorBox error={act.error} />}
      <Async state={data}>
        {(d) => list.length === 0 ? <Blankslate title={st.isMember ? "No members matched." : "This organization has no public members."} /> : (
          <ul className="box list" data-testid="member-list">
            <li className="box-header small muted">{list.length} {list.length === 1 ? "member" : "members"} in the {org.login} organization</li>
            {list.map((u) => (
              <li key={u.id} className="list-row org-person" data-testid={`member-${u.login}`}>
                <Avatar user={u} size={40} />
                <div className="grow">
                  <Link to={`/${u.login}`} className="org-person-name"><strong>{u.name || u.login}</strong></Link>
                  {u.name && <span className="muted"> {u.login}</span>}
                </div>
                {st.isMember && <span className="badge" data-testid="member-role">{d.owners.has(u.login) ? "Owner" : "Member"}</span>}
                {st.isOwner && me?.login !== u.login && (
                  <div className="btn-group org-person-actions">
                    <label className="sr-only" htmlFor={`role-${u.login}`}>Role of {u.login}</label>
                    <select id={`role-${u.login}`} className="input input-sm input-auto" value={d.owners.has(u.login) ? "admin" : "member"} disabled={act.busy}
                      onChange={(e) => doAct(() => orgApi.setRole(org.login, u.login, e.target.value as "admin" | "member"), `${u.login} is now ${e.target.value === "admin" ? "an owner" : "a member"}.`)}>
                      <option value="member">Member</option>
                      <option value="admin">Owner</option>
                    </select>
                    <button type="button" className="btn btn-sm" disabled={act.busy}
                      onClick={() => confirm(`Convert ${u.login} to an outside collaborator?`) && doAct(() => orgApi.convertToOutside(org.login, u.login), `${u.login} is now an outside collaborator.`)}>
                      Convert to outside collaborator
                    </button>
                    <button type="button" className="btn btn-sm btn-danger-outline" disabled={act.busy}
                      onClick={() => confirm(`Remove ${u.login} from ${org.login}?`) && doAct(() => orgApi.removeMember(org.login, u.login), `${u.login} was removed from ${org.login}.`)}>
                      Remove from organization
                    </button>
                  </div>
                )}
                {me?.login === u.login && <LeaveButton org={org.login} />}
              </li>
            ))}
          </ul>
        )}
      </Async>
    </>
  );
}

function LeaveButton({ org }: { org: string }) {
  const { user } = useAuth();
  const act = useAction();
  if (!user) return null;
  return (
    <>
      <button type="button" className="btn btn-sm btn-danger-outline" disabled={act.busy} data-testid="leave-org"
        onClick={async () => {
          if (!confirm(`Leave ${org}? You will lose access to its private repositories.`)) return;
          if (await act.run(() => orgApi.removeMember(org, user.login))) location.assign("/settings/organizations");
        }}>
        Leave
      </button>
      {act.error !== undefined && <ErrorBox error={act.error} />}
    </>
  );
}

export function InviteForm({ st, onDone }: { st: OrgState; onDone: (msg: string) => void }) {
  const { org } = st;
  const teams = useAsync(() => orgApi.teams(org.login), [org.login]);
  const [who, setWho] = useState("");
  const [role, setRole] = useState("direct_member");
  const [teamIds, setTeamIds] = useState<number[]>([]);
  const act = useAction();
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const v = who.trim();
    const ok = await act.run(async () => {
      if (v.includes("@")) await orgApi.invite(org.login, { email: v, role, team_ids: teamIds });
      else {
        const u = await orgApi.userByLogin(v);
        await orgApi.invite(org.login, { invitee_id: u.id, role, team_ids: teamIds });
      }
    });
    if (ok) onDone(`Invitation sent to ${v}.`);
  };
  return (
    <form className="box box-body org-invite" onSubmit={submit} aria-label="Invite member">
      <h2 className="h4">Invite a member to {org.login}</h2>
      {act.error !== undefined && <ErrorBox error={act.error} />}
      <div className="form-group">
        <label htmlFor="invite-who">Username or email</label>
        <input id="invite-who" className="input" required value={who} onChange={(e) => setWho(e.target.value)} autoFocus />
      </div>
      <fieldset className="form-group radio-group">
        <legend>Role in the organization</legend>
        <label className="radio-row">
          <input type="radio" name="invite-role" checked={role === "direct_member"} onChange={() => setRole("direct_member")} />
          <span><strong>Member</strong><br /><span className="muted small">Members can see all other members, and can be granted access to repositories.</span></span>
        </label>
        <label className="radio-row">
          <input type="radio" name="invite-role" checked={role === "admin"} onChange={() => setRole("admin")} data-testid="invite-role-owner" />
          <span><strong>Owner</strong><br /><span className="muted small">Owners have full administrative rights to the organization.</span></span>
        </label>
      </fieldset>
      {teams.data && teams.data.length > 0 && (
        <fieldset className="form-group">
          <legend>Teams</legend>
          {teams.data.map((t: Team) => (
            <label key={t.id} className="check-row">
              <input type="checkbox" checked={teamIds.includes(t.id)} data-testid={`invite-team-${t.slug}`}
                onChange={(e) => setTeamIds(e.target.checked ? [...teamIds, t.id] : teamIds.filter((x) => x !== t.id))} /> {t.name}
            </label>
          ))}
        </fieldset>
      )}
      <div className="form-actions">
        <button type="submit" className="btn btn-primary" disabled={act.busy || !who.trim()}>{act.busy ? "Sending…" : "Send invitation"}</button>
        <button type="button" className="btn" onClick={() => onDone("")}>Cancel</button>
      </div>
    </form>
  );
}

function Outside({ st }: { st: OrgState }) {
  const { org } = st;
  const data = useAsync(() => orgApi.outsideCollaborators(org.login), [org.login]);
  const act = useAction();
  return (
    <>
      <h2 className="h3">Outside collaborators</h2>
      <p className="muted small">People with access to one or more of the organization's repositories who are not members.</p>
      {act.error !== undefined && <ErrorBox error={act.error} />}
      <Async state={data}>
        {(list: User[]) => list.length === 0 ? <Blankslate title="This organization doesn't have any outside collaborators." /> : (
          <ul className="box list" data-testid="outside-list">
            {list.map((u) => (
              <li key={u.id} className="list-row org-person" data-testid={`outside-${u.login}`}>
                <Avatar user={u} size={32} />
                <Link to={`/${u.login}`} className="grow">{u.login}</Link>
                <button type="button" className="btn btn-sm btn-danger-outline" disabled={act.busy}
                  onClick={async () => { if (confirm(`Remove ${u.login} from all repositories?`) && await act.run(() => orgApi.removeOutside(org.login, u.login))) data.reload(); }}>
                  Remove from all repositories
                </button>
              </li>
            ))}
          </ul>
        )}
      </Async>
    </>
  );
}

function Invitations({ st, failed }: { st: OrgState; failed: boolean }) {
  const { org } = st;
  const data = useAsync(() => (failed ? orgApi.failedInvitations(org.login) : orgApi.invitations(org.login)), [org.login, failed]);
  const act = useAction();
  return (
    <>
      <div className="toolbar">
        <h2 className="h3">{failed ? "Failed invitations" : "Pending invitations"}</h2>
        <span className="spacer" />
        {!failed && <Link to={`/orgs/${org.login}/people?invite=1`} className="btn btn-sm">Invite member</Link>}
      </div>
      {act.error !== undefined && <ErrorBox error={act.error} />}
      <Async state={data}>
        {(list) => list.length === 0 ? (
          <Blankslate title={failed ? "No failed invitations." : "No pending invitations."}>
            {failed ? "Invitations fail when they expire after 7 days or the invitee is blocked." : null}
          </Blankslate>
        ) : (
          <ul className="box list" data-testid={failed ? "failed-invitations" : "pending-invitations"}>
            {list.map((i) => (
              <li key={i.id} className="list-row org-person" data-testid={`invitation-${i.login || i.email}`}>
                <div className="grow">
                  <strong>{i.login ? <Link to={`/${i.login}`}>{i.login}</Link> : i.email}</strong>{" "}
                  <span className="badge">{roleLabel(i.role)}</span>
                  <div className="muted small">
                    Invited <RelTime iso={i.created_at} />{i.inviter && <> by {i.inviter.login}</>}
                    {i.team_count > 0 && <> · {i.team_count} {i.team_count === 1 ? "team" : "teams"}</>}
                    {failed && i.failed_reason && <> · <span className="c-danger">{i.failed_reason}</span> <RelTime iso={i.failed_at} /></>}
                  </div>
                </div>
                {!failed && (
                  <button type="button" className="btn btn-sm btn-danger-outline" disabled={act.busy}
                    onClick={async () => { if (await act.run(() => orgApi.cancelInvitation(org.login, i.id))) data.reload(); }}>
                    Cancel invitation
                  </button>
                )}
              </li>
            ))}
          </ul>
        )}
      </Async>
    </>
  );
}

function SecurityManagers({ st }: { st: OrgState }) {
  const { org } = st;
  const data = useAsync(async () => ({ managers: await orgApi.securityManagers(org.login), teams: await orgApi.teams(org.login) }), [org.login]);
  const [slug, setSlug] = useState("");
  const act = useAction();
  return (
    <>
      <h2 className="h3">Security managers</h2>
      <p className="muted small">Members of security manager teams can manage security alerts and settings across the organization,
        and have read access to every repository.</p>
      {act.error !== undefined && <ErrorBox error={act.error} />}
      <Async state={data}>
        {(d) => {
          const free = d.teams.filter((t) => !d.managers.some((m) => m.id === t.id));
          return (
            <>
              {free.length > 0 && (
                <form className="toolbar" onSubmit={async (e) => { e.preventDefault(); if (slug && await act.run(() => orgApi.addSecurityManager(org.login, slug))) { setSlug(""); data.reload(); } }}>
                  <label htmlFor="secmgr-team" className="sr-only">Team</label>
                  <select id="secmgr-team" className="input input-auto" value={slug} onChange={(e) => setSlug(e.target.value)}>
                    <option value="">Select a team…</option>
                    {free.map((t) => <option key={t.id} value={t.slug}>{t.name}</option>)}
                  </select>
                  <button type="submit" className="btn" disabled={!slug || act.busy}>Add security manager team</button>
                </form>
              )}
              {d.managers.length === 0 ? <Blankslate title="No security manager teams yet." /> : (
                <ul className="box list" data-testid="security-managers">
                  {d.managers.map((t) => (
                    <li key={t.id} className="list-row" data-testid={`secmgr-${t.slug}`}>
                      <Link to={`/orgs/${org.login}/teams/${t.slug}`} className="grow">{t.name}</Link>
                      <button type="button" className="btn btn-sm btn-danger-outline" disabled={act.busy}
                        onClick={async () => { if (await act.run(() => orgApi.removeSecurityManager(org.login, t.slug))) data.reload(); }}>Remove</button>
                    </li>
                  ))}
                </ul>
              )}
            </>
          );
        }}
      </Async>
    </>
  );
}
