// Enterprise teams: CRUD, members and organization assignments (members of a team assigned to
// an organization become members of it).
import { useState, type FormEvent } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import type { AdminCtx } from "../types";
import { Async, Avatar, Blankslate, RelTime } from "../../components/common";
import { useAsync } from "../../hooks";
import { adminApi, type EntTeam } from "./api";
import { PageHead, Pill, useAction } from "./ui";

export function EnterpriseTeamsSection({ ctx }: { ctx: AdminCtx }) {
  const splat = useParams()["*"] || "";
  const slug = splat.split("/")[1];
  return slug ? <TeamPage ent={ctx.enterprise} slug={decodeURIComponent(slug)} /> : <TeamList ent={ctx.enterprise} />;
}

function TeamList({ ent }: { ent: string }) {
  const teams = useAsync(() => adminApi.teams(ent), [ent]);
  const nav = useNavigate();
  const [name, setName] = useState("");
  const [desc, setDesc] = useState("");
  const act = useAction();
  const create = async (e: FormEvent) => {
    e.preventDefault();
    let t: EntTeam | undefined;
    if (await act.run(async () => { t = await adminApi.createTeam(ent, { name: name.trim(), description: desc.trim() || undefined, organization_selection_type: "selected" }); })) {
      nav(`/site-admin/enterprise-teams/${encodeURIComponent(t!.slug)}`);
    }
  };
  return (
    <div data-testid="sa-ent-teams">
      <PageHead title="Enterprise teams" desc="Teams that span the enterprise. Assign a team to organizations to make its members members of those organizations." />
      <Async state={teams}>
        {(list) => list.length === 0 ? <Blankslate title="No enterprise teams" /> : (
          <ul className="box list" aria-label="Enterprise teams">
            {list.map((t) => (
              <li key={t.id} className="list-row sa-row" data-testid={`sa-ent-team-${t.slug}`}>
                <div className="grow sa-row-main">
                  <div className="sa-row-title"><Link to={`/site-admin/enterprise-teams/${encodeURIComponent(t.slug)}`}><strong>{t.name}</strong></Link><span className="muted small mono">{t.slug}</span>{t.group_name && <Pill>{`IdP group: ${t.group_name}`}</Pill>}</div>
                  <div className="muted small">{t.description || "No description"} · created <RelTime iso={t.created_at} /></div>
                </div>
              </li>
            ))}
          </ul>
        )}
      </Async>
      <form className="box box-body" onSubmit={create} aria-label="New enterprise team">
        <h3 className="h5">New enterprise team</h3>
        {act.status}
        <div className="form-row">
          <div className="form-group">
            <label htmlFor="sa-et-name">Team name</label>
            <input id="sa-et-name" className="input" required value={name} onChange={(e) => setName(e.target.value)} />
          </div>
          <div className="form-group grow">
            <label htmlFor="sa-et-desc">Description</label>
            <input id="sa-et-desc" className="input" value={desc} onChange={(e) => setDesc(e.target.value)} />
          </div>
        </div>
        <button type="submit" className="btn btn-primary" disabled={act.busy}>Create team</button>
      </form>
    </div>
  );
}

function TeamPage({ ent, slug }: { ent: string; slug: string }) {
  const team = useAsync(() => adminApi.team(ent, slug), [ent, slug]);
  return <Async state={team}>{(t) => <TeamAdmin ent={ent} t={t} reload={team.reload} />}</Async>;
}

function TeamAdmin({ ent, t, reload }: { ent: string; t: EntTeam; reload: () => void }) {
  const nav = useNavigate();
  const members = useAsync(() => adminApi.teamMembers(ent, t.slug), [ent, t.slug]);
  const orgs = useAsync(() => adminApi.teamOrgs(ent, t.slug), [ent, t.slug]);
  const [name, setName] = useState(t.name);
  const [desc, setDesc] = useState(t.description);
  const [sel, setSel] = useState(t.organization_selection_type);
  const [member, setMember] = useState("");
  const [org, setOrg] = useState("");
  const act = useAction();
  return (
    <div data-testid="sa-ent-team">
      <div className="sa-crumbs small"><Link to="/site-admin/enterprise-teams">Enterprise teams</Link> / {t.name}</div>
      <PageHead title={t.name} desc={<span className="mono">{t.slug}</span>} />
      {act.status}
      <form className="box box-body" aria-label="Team settings" onSubmit={async (e) => {
        e.preventDefault();
        let nt: EntTeam | undefined;
        if (await act.run(async () => { nt = await adminApi.updateTeam(ent, t.slug, { name: name.trim(), description: desc, organization_selection_type: sel }); }, "Team updated.")) {
          if (nt && nt.slug !== t.slug) nav(`/site-admin/enterprise-teams/${encodeURIComponent(nt.slug)}`, { replace: true });
          else reload();
        }
      }}>
        <div className="form-row">
          <div className="form-group">
            <label htmlFor="sa-et-ename">Name</label>
            <input id="sa-et-ename" className="input" value={name} onChange={(e) => setName(e.target.value)} />
          </div>
          <div className="form-group grow">
            <label htmlFor="sa-et-edesc">Description</label>
            <input id="sa-et-edesc" className="input" value={desc} onChange={(e) => setDesc(e.target.value)} />
          </div>
          <div className="form-group">
            <label htmlFor="sa-et-sel">Organizations</label>
            <select id="sa-et-sel" className="input input-auto" value={sel} onChange={(e) => setSel(e.target.value)}>
              <option value="selected">Selected organizations</option>
              <option value="all">All organizations</option>
              <option value="disabled">None</option>
            </select>
          </div>
        </div>
        <button type="submit" className="btn">Save</button>
      </form>

      <section className="box" data-testid="sa-ent-team-members">
        <div className="box-header"><h3 className="h5">Members</h3>{members.data && <span className="counter">{members.data.length}</span>}</div>
        <Async state={members}>
          {(list) => list.length === 0 ? <p className="box-body muted">No members.</p> : (
            <ul className="list">
              {list.map((m) => (
                <li key={m.id} className="list-row sa-row">
                  <Avatar user={m} size={20} /><Link className="grow" to={`/${m.login}`}>{m.login}</Link>
                  <button type="button" className="btn btn-sm btn-danger-outline" disabled={act.busy}
                    onClick={async () => { if (await act.run(() => adminApi.removeTeamMember(ent, t.slug, m.login), `${m.login} removed.`)) members.reload(); }}>Remove</button>
                </li>
              ))}
            </ul>
          )}
        </Async>
        <form className="box-footer" onSubmit={async (e) => { e.preventDefault(); if (await act.run(() => adminApi.addTeamMember(ent, t.slug, member.trim()), `${member.trim()} added.`)) { setMember(""); members.reload(); } }}>
          <label htmlFor="sa-et-member" className="sr-only">Username</label>
          <input id="sa-et-member" className="input input-sm" placeholder="Username" value={member} onChange={(e) => setMember(e.target.value)} />
          <button type="submit" className="btn btn-sm" disabled={act.busy || !member.trim()}>Add member</button>
        </form>
      </section>

      <section className="box" data-testid="sa-ent-team-orgs">
        <div className="box-header"><h3 className="h5">Organization assignments</h3>{orgs.data && <span className="counter">{orgs.data.length}</span>}</div>
        <Async state={orgs}>
          {(list) => list.length === 0 ? <p className="box-body muted">Not assigned to any organization.</p> : (
            <ul className="list">
              {list.map((o) => (
                <li key={o.id} className="list-row sa-row">
                  <Avatar user={o} size={20} /><Link className="grow" to={`/${o.login}`}>{o.login}</Link>
                  <button type="button" className="btn btn-sm btn-danger-outline" disabled={act.busy}
                    onClick={async () => { if (await act.run(() => adminApi.removeTeamOrg(ent, t.slug, o.login), `Unassigned from ${o.login}.`)) orgs.reload(); }}>Unassign</button>
                </li>
              ))}
            </ul>
          )}
        </Async>
        <form className="box-footer" onSubmit={async (e) => { e.preventDefault(); if (await act.run(() => adminApi.addTeamOrg(ent, t.slug, org.trim()), `Assigned to ${org.trim()}.`)) { setOrg(""); orgs.reload(); } }}>
          <label htmlFor="sa-et-org" className="sr-only">Organization</label>
          <input id="sa-et-org" className="input input-sm" placeholder="Organization" value={org} onChange={(e) => setOrg(e.target.value)} />
          <button type="submit" className="btn btn-sm" disabled={act.busy || !org.trim()}>Assign</button>
        </form>
      </section>

      <div className="box danger-zone sa-action">
        <div><strong>Delete this team</strong><p className="muted small">Members keep organization memberships they had before the assignment.</p></div>
        <button type="button" className="btn btn-sm btn-danger" disabled={act.busy}
          onClick={async () => { if (window.confirm(`Delete enterprise team “${t.name}”?`) && await act.run(() => adminApi.deleteTeam(ent, t.slug))) nav("/site-admin/enterprise-teams", { replace: true }); }}>Delete team</button>
      </div>
    </div>
  );
}
