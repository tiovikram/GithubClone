// LDAP mappings and SCIM provisioning (read-only view of what the identity provider pushed).
import { useState, type FormEvent } from "react";
import { Link, useParams } from "react-router-dom";
import type { AdminCtx } from "../types";
import { Async, Blankslate, Flash, RelTime } from "../../components/common";
import { useAsync } from "../../hooks";
import { adminApi, type ScimGroup, type ScimUser } from "./api";
import { PageHead, Pill, useAction } from "./ui";

export function LdapSection() {
  const [login, setLogin] = useState("");
  const [udn, setUdn] = useState("");
  const [org, setOrg] = useState("");
  const [team, setTeam] = useState("");
  const [tdn, setTdn] = useState("");
  const act = useAction();
  const teamId = async () => (await adminApi.orgTeam(org.trim(), team.trim())).id;
  return (
    <div data-testid="sa-ldap">
      <PageHead title="LDAP" desc="Map users and teams to directory entries and request a sync. forge stores the mappings and records sync requests; it does not contact an LDAP server." />
      {act.status}
      <form className="box box-body" aria-label="LDAP user mapping" onSubmit={(e: FormEvent) => { e.preventDefault(); void act.run(() => adminApi.ldapUserMapping(login.trim(), udn.trim()), `LDAP mapping of ${login.trim()} updated.`); }}>
        <h3 className="h5">User mapping</h3>
        <div className="form-row">
          <div className="form-group">
            <label htmlFor="sa-ldap-user">Username</label>
            <input id="sa-ldap-user" className="input" required value={login} onChange={(e) => setLogin(e.target.value)} />
          </div>
          <div className="form-group grow">
            <label htmlFor="sa-ldap-udn">Distinguished name</label>
            <input id="sa-ldap-udn" className="input" placeholder="uid=jdoe,ou=people,dc=example,dc=com" value={udn} onChange={(e) => setUdn(e.target.value)} />
          </div>
        </div>
        <div className="form-actions">
          <button type="button" className="btn" disabled={act.busy || !login.trim()} onClick={() => act.run(() => adminApi.ldapUserSync(login.trim()), `Sync of ${login.trim()} queued.`)}>Sync user</button>
          <button type="submit" className="btn btn-primary" disabled={act.busy || !udn.trim()}>Update mapping</button>
        </div>
      </form>
      <form className="box box-body" aria-label="LDAP team mapping" onSubmit={(e: FormEvent) => { e.preventDefault(); void act.run(async () => adminApi.ldapTeamMapping(await teamId(), tdn.trim()), `LDAP mapping of ${org.trim()}/${team.trim()} updated.`); }}>
        <h3 className="h5">Team mapping</h3>
        <div className="form-row">
          <div className="form-group">
            <label htmlFor="sa-ldap-org">Organization</label>
            <input id="sa-ldap-org" className="input" required value={org} onChange={(e) => setOrg(e.target.value)} />
          </div>
          <div className="form-group">
            <label htmlFor="sa-ldap-team">Team slug</label>
            <input id="sa-ldap-team" className="input" required value={team} onChange={(e) => setTeam(e.target.value)} />
          </div>
          <div className="form-group grow">
            <label htmlFor="sa-ldap-tdn">Group distinguished name</label>
            <input id="sa-ldap-tdn" className="input" placeholder="cn=engineering,ou=groups,dc=example,dc=com" value={tdn} onChange={(e) => setTdn(e.target.value)} />
          </div>
        </div>
        <div className="form-actions">
          <button type="button" className="btn" disabled={act.busy || !org.trim() || !team.trim()} onClick={() => act.run(async () => adminApi.ldapTeamSync(await teamId()), `Sync of ${org.trim()}/${team.trim()} queued.`)}>Sync team</button>
          <button type="submit" className="btn btn-primary" disabled={act.busy || !tdn.trim()}>Update mapping</button>
        </div>
      </form>
      <p className="muted small">A user's current mapping is shown on their page under <Link to="/site-admin/users">Users</Link>.</p>
    </div>
  );
}

export function ScimSection({ ctx }: { ctx: AdminCtx }) {
  const splat = useParams()["*"] || "";
  const [, kind, id] = splat.split("/");
  if (kind === "users" && id) return <ScimUserView ent={ctx.enterprise} id={decodeURIComponent(id)} />;
  if (kind === "groups" && id) return <ScimGroupView ent={ctx.enterprise} id={decodeURIComponent(id)} />;
  return <ScimLists ent={ctx.enterprise} />;
}

function ScimLists({ ent }: { ent: string }) {
  const users = useAsync(() => adminApi.scimUsers(ent), [ent]);
  const groups = useAsync(() => adminApi.scimGroups(ent), [ent]);
  return (
    <div data-testid="sa-scim">
      <PageHead title="SCIM provisioning" desc={<>Users and groups provisioned by your identity provider through <code>/scim/v2/enterprises/{ent}</code>.</>} />
      <section className="box">
        <div className="box-header"><h3 className="h5">Provisioned users</h3>{users.data && <span className="counter">{users.data.totalResults}</span>}</div>
        <Async state={users}>
          {(l) => l.Resources.length === 0 ? <p className="box-body muted">No SCIM users have been provisioned.</p> : (
            <ul className="list">
              {l.Resources.map((u) => (
                <li key={u.id} className="list-row sa-row">
                  <div className="grow sa-row-main">
                    <div className="sa-row-title"><Link to={`/site-admin/scim/users/${encodeURIComponent(u.id)}`}><strong>{u.userName}</strong></Link>{u.displayName && <span className="muted">{u.displayName}</span>}<Pill kind={u.active ? "enabled" : "suspended"}>{u.active ? "active" : "inactive"}</Pill></div>
                    <div className="muted small">{u.emails.map((e) => e.value).join(", ")}{u.externalId ? ` · external id ${u.externalId}` : ""}</div>
                  </div>
                </li>
              ))}
            </ul>
          )}
        </Async>
      </section>
      <section className="box">
        <div className="box-header"><h3 className="h5">Provisioned groups</h3>{groups.data && <span className="counter">{groups.data.totalResults}</span>}</div>
        <Async state={groups}>
          {(l) => l.Resources.length === 0 ? <p className="box-body muted">No SCIM groups have been provisioned.</p> : (
            <ul className="list">
              {l.Resources.map((g) => (
                <li key={g.id} className="list-row sa-row">
                  <div className="grow sa-row-main"><Link to={`/site-admin/scim/groups/${encodeURIComponent(g.id)}`}><strong>{g.displayName}</strong></Link>
                    <div className="muted small">{g.members.length} member{g.members.length === 1 ? "" : "s"}</div></div>
                </li>
              ))}
            </ul>
          )}
        </Async>
      </section>
    </div>
  );
}

function ScimUserView({ ent, id }: { ent: string; id: string }) {
  const u = useAsync(() => adminApi.scimUser(ent, id), [ent, id]);
  return <Async state={u}>{(x: ScimUser) => (
    <div data-testid="sa-scim-user">
      <div className="sa-crumbs small"><Link to="/site-admin/scim">SCIM</Link> / {x.userName}</div>
      <PageHead title={x.userName} />
      <dl className="box box-body sa-dl">
        <dt>SCIM id</dt><dd className="mono">{x.id}</dd>
        <dt>External id</dt><dd>{x.externalId || "—"}</dd>
        <dt>Active</dt><dd>{x.active ? "yes" : "no (suspended)"}</dd>
        <dt>Display name</dt><dd>{x.displayName || "—"}</dd>
        <dt>Emails</dt><dd>{x.emails.map((e) => e.value + (e.primary ? " (primary)" : "")).join(", ") || "—"}</dd>
        <dt>Roles</dt><dd>{x.roles.map((r) => r.value).join(", ") || "—"}</dd>
        <dt>Groups</dt><dd>{x.groups?.map((g) => g.display).join(", ") || "—"}</dd>
        <dt>Created</dt><dd><RelTime iso={x.meta?.created} /></dd>
        <dt>Account</dt><dd><Link to={`/site-admin/users/${encodeURIComponent(x.userName)}`}>Manage {x.userName}</Link></dd>
      </dl>
    </div>
  )}</Async>;
}

function ScimGroupView({ ent, id }: { ent: string; id: string }) {
  const g = useAsync(() => adminApi.scimGroup(ent, id), [ent, id]);
  return <Async state={g}>{(x: ScimGroup) => (
    <div data-testid="sa-scim-group">
      <div className="sa-crumbs small"><Link to="/site-admin/scim">SCIM</Link> / {x.displayName}</div>
      <PageHead title={x.displayName} desc={x.externalId ? `External id ${x.externalId}` : undefined} />
      {x.members.length === 0 ? <Blankslate title="No members" /> : (
        <ul className="box list">{x.members.map((m) => <li key={m.value} className="list-row">{m.display || m.value}</li>)}</ul>
      )}
      <Flash kind="info">Teams link to this group from their settings (organization teams) or from <Link to="/site-admin/enterprise-teams">Enterprise teams</Link>.</Flash>
    </div>
  )}</Async>;
}
