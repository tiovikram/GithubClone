import { useState, type FormEvent } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import type { AdminCtx } from "../types";
import { Async, Avatar, Blankslate, CodeLine, Flash, RelTime } from "../../components/common";
import { useAsync } from "../../hooks";
import { adminApi, type AdminUser, type AdminUserDetail, type Authorization } from "./api";
import { PageHead, Pager, Pill, useAction } from "./ui";

/** `/site-admin/users` (search, create) and `/site-admin/users/{login}` (details and actions). */
export function UsersSection({ ctx }: { ctx: AdminCtx }) {
  const splat = useParams()["*"] || "";
  const login = splat.split("/")[1];
  return login ? <UserDetail ctx={ctx} login={decodeURIComponent(login)} /> : <UserList />;
}

function UserList() {
  const [q, setQ] = useState("");
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState("all");
  const [page, setPage] = useState(1);
  const [creating, setCreating] = useState(false);
  const list = useAsync(() => adminApi.users(query, filter, page), [query, filter, page]);
  return (
    <div data-testid="sa-users">
      <PageHead title="Users" desc="Search every account on this instance; open one to manage it.">
        <button type="button" className="btn btn-primary btn-sm" onClick={() => setCreating(!creating)} data-testid="sa-new-user">New user</button>
      </PageHead>
      {creating && <CreateUser onDone={list.reload} />}
      <form className="sa-filter" role="search" onSubmit={(e) => { e.preventDefault(); setPage(1); setQuery(q.trim()); }}>
        <label htmlFor="sa-user-q" className="sr-only">Search users</label>
        <input id="sa-user-q" className="input" placeholder="Search by login, name or email" value={q} onChange={(e) => setQ(e.target.value)} />
        <label htmlFor="sa-user-filter" className="sr-only">Filter</label>
        <select id="sa-user-filter" className="input input-auto" value={filter} onChange={(e) => { setPage(1); setFilter(e.target.value); }}>
          <option value="all">All users</option>
          <option value="admin">Site admins</option>
          <option value="suspended">Suspended</option>
          <option value="active">Active</option>
        </select>
        <button type="submit" className="btn">Search</button>
      </form>
      <Async state={list}>
        {(l) => l.items.length === 0 ? <Blankslate title="No users found" /> : (
          <>
            <p className="muted small">{l.total_count} user{l.total_count === 1 ? "" : "s"}</p>
            <ul className="box list" aria-label="Users">
              {l.items.map((u) => <UserRow key={u.id} u={u} />)}
            </ul>
            <Pager page={page} total={l.total_count} per={30} onPage={setPage} />
          </>
        )}
      </Async>
    </div>
  );
}

function UserRow({ u }: { u: AdminUser }) {
  return (
    <li className="list-row sa-row" data-testid={`sa-user-${u.login}`}>
      <Avatar user={u} size={32} />
      <div className="grow sa-row-main">
        <div className="sa-row-title">
          <Link to={`/site-admin/users/${encodeURIComponent(u.login)}`}><strong>{u.login}</strong></Link>
          {u.name && <span className="muted">{u.name}</span>}
          {u.site_admin && <Pill>site admin</Pill>}
          {u.suspended_at && <Pill>suspended</Pill>}
        </div>
        <div className="muted small">
          {u.email || "no email"} · {u.repos} repositor{u.repos === 1 ? "y" : "ies"} · joined <RelTime iso={u.created_at} />
        </div>
      </div>
    </li>
  );
}

function CreateUser({ onDone }: { onDone: () => void }) {
  const nav = useNavigate();
  const [login, setLogin] = useState("");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [suspended, setSuspended] = useState(false);
  const act = useAction();
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const ok = await act.run(async () => {
      const u = await adminApi.createUser({ login: login.trim(), email: email.trim() || undefined, suspended });
      if (password) await adminApi.setPassword(u.login, password);
    });
    if (ok) {
      onDone();
      nav(`/site-admin/users/${encodeURIComponent(login.trim())}`);
    }
  };
  return (
    <form className="box box-body" onSubmit={submit} aria-label="Create user">
      <h3 className="h5">Create a user</h3>
      {act.status}
      <div className="form-row">
        <div className="form-group">
          <label htmlFor="sa-cu-login">Username</label>
          <input id="sa-cu-login" className="input" required value={login} onChange={(e) => setLogin(e.target.value)} />
        </div>
        <div className="form-group">
          <label htmlFor="sa-cu-email">Email</label>
          <input id="sa-cu-email" type="email" className="input" value={email} onChange={(e) => setEmail(e.target.value)} />
        </div>
        <div className="form-group">
          <label htmlFor="sa-cu-pass">Password <span className="muted small">(optional, 8+ characters)</span></label>
          <input id="sa-cu-pass" type="password" className="input" minLength={8} autoComplete="new-password" value={password} onChange={(e) => setPassword(e.target.value)} />
        </div>
      </div>
      <label className="check-row"><input type="checkbox" checked={suspended} onChange={(e) => setSuspended(e.target.checked)} /> Create the account suspended</label>
      <p className="muted small">Without a password the user signs in after you set one, or through an impersonation token.</p>
      <button type="submit" className="btn btn-primary" disabled={act.busy}>{act.busy ? "Creating…" : "Create user"}</button>
    </form>
  );
}

const SCOPES = ["repo", "read:org", "admin:org", "user", "gist", "workflow", "delete_repo", "admin:public_key", "admin:repo_hook", "site_admin"];

function UserDetail({ ctx, login }: { ctx: AdminCtx; login: string }) {
  const state = useAsync(() => adminApi.user(login), [login]);
  return (
    <Async state={state}>
      {(u) => <UserAdmin ctx={ctx} u={u} reload={state.reload} />}
    </Async>
  );
}

function UserAdmin({ ctx, u, reload }: { ctx: AdminCtx; u: AdminUserDetail; reload: () => void }) {
  const nav = useNavigate();
  const self = ctx.user.login.toLowerCase() === u.login.toLowerCase();
  const act = useAction();
  const [reason, setReason] = useState("");
  const [newLogin, setNewLogin] = useState(u.login);
  const [password, setPassword] = useState("");
  const [dn, setDn] = useState(u.ldap_dn || "");
  const [scopes, setScopes] = useState<string[]>(["repo", "read:org"]);
  const [imp, setImp] = useState<Authorization>();
  const [confirm, setConfirm] = useState("");
  const doRun = async (fn: () => Promise<unknown>, msg: string) => {
    const ok = await act.run(fn, msg);
    if (ok) reload();
    return ok;
  };
  return (
    <div data-testid="sa-user-detail">
      <div className="sa-crumbs small"><Link to="/site-admin/users">Users</Link> / {u.login}</div>
      <div className="sa-user-head">
        <Avatar user={u} size={64} />
        <div>
          <h2 className="h3">{u.login} {u.site_admin && <Pill>site admin</Pill>} {u.suspended_at && <Pill>suspended</Pill>}</h2>
          <div className="muted">{[u.name, u.email].filter(Boolean).join(" · ")}</div>
          <div className="small"><Link to={`/${u.login}`}>View profile</Link> · joined <RelTime iso={u.created_at} /></div>
        </div>
      </div>
      {act.status}
      {imp && (
        <div className="box box-body" data-testid="sa-impersonation-token">
          <Flash kind="success">Impersonation token created for {u.login}. Copy it now; it will not be shown again.</Flash>
          <CodeLine text={imp.token} label="Impersonation token" />
        </div>
      )}

      <section className="box">
        <div className="box-header"><h3 className="h5">Account</h3></div>
        <dl className="sa-dl box-body">
          <dt>User ID</dt><dd>{u.id}</dd>
          <dt>Emails</dt><dd>{u.emails.length ? u.emails.join(", ") : <span className="muted">none</span>}</dd>
          <dt>Password</dt><dd>{u.has_password ? "set" : <span className="muted">not set</span>}</dd>
          <dt>Suspension</dt>
          <dd data-testid="sa-suspension">{u.suspended_at ? <>Suspended <RelTime iso={u.suspended_at} />{u.suspension_reason ? ` — “${u.suspension_reason}”` : ""}</> : "Active"}</dd>
          <dt>LDAP DN</dt><dd>{u.ldap_dn || <span className="muted">not mapped</span>}</dd>
        </dl>
      </section>

      <section className="box">
        <div className="box-header"><h3 className="h5">Organizations</h3><span className="counter">{u.organizations.length}</span></div>
        {u.organizations.length === 0 ? <p className="box-body muted">Not a member of any organization.</p> : (
          <ul className="list">
            {u.organizations.map((o) => (
              <li key={o.login} className="list-row"><Link className="grow" to={`/${o.login}`}>{o.login}</Link><Pill kind={o.role === "admin" ? "owner" : ""}>{o.role === "admin" ? "owner" : "member"}</Pill></li>
            ))}
          </ul>
        )}
      </section>

      <section className="box">
        <div className="box-header"><h3 className="h5">Repositories</h3><span className="counter">{u.repositories.length}</span></div>
        {u.repositories.length === 0 ? <p className="box-body muted">No repositories.</p> : (
          <ul className="list">
            {u.repositories.map((r) => (
              <li key={r.name} className="list-row">
                <Link className="grow" to={`/${r.full_name}`}>{r.full_name}</Link>
                {r.private && <Pill>private</Pill>}{r.archived && <Pill>archived</Pill>}
              </li>
            ))}
          </ul>
        )}
      </section>

      <section className="box">
        <div className="box-header"><h3 className="h5">SSH keys</h3><span className="counter">{u.keys.length}</span></div>
        {u.keys.length === 0 ? <p className="box-body muted">No SSH keys.</p> : (
          <ul className="list">
            {u.keys.map((k) => (
              <li key={k.id} className="list-row sa-row">
                <div className="grow sa-row-main"><strong>{k.title}</strong><div className="mono small sa-break">{k.fingerprint}</div></div>
                <button type="button" className="btn btn-sm btn-danger-outline" disabled={act.busy}
                  onClick={() => confirmed(`Delete SSH key “${k.title}”?`) && doRun(() => adminApi.deleteKey(k.id), "SSH key deleted.")}>Delete</button>
              </li>
            ))}
          </ul>
        )}
      </section>

      <section className="box" data-testid="sa-user-tokens">
        <div className="box-header"><h3 className="h5">Access tokens</h3><span className="counter">{u.tokens.length}</span></div>
        {u.tokens.length === 0 ? <p className="box-body muted">No tokens.</p> : (
          <ul className="list">
            {u.tokens.map((t) => (
              <li key={t.id} className="list-row sa-row">
                <div className="grow sa-row-main">
                  <strong>{t.name}</strong> {t.impersonation && <Pill kind="scheduled">impersonation</Pill>}
                  <div className="muted small">…{t.token_last_eight} · {t.scopes.join(", ") || "no scopes"} · created <RelTime iso={t.created_at} />{t.last_used_at ? <> · used <RelTime iso={t.last_used_at} /></> : " · never used"}</div>
                </div>
                <button type="button" className="btn btn-sm btn-danger-outline" disabled={act.busy}
                  onClick={() => confirmed(`Revoke token “${t.name}”?`) && doRun(() => adminApi.deleteToken(t.id), "Token revoked.")}>Revoke</button>
              </li>
            ))}
          </ul>
        )}
      </section>

      <h3 className="h4 sa-subhead">Administrative actions</h3>
      <div className="box">
        <div className="sa-action">
          <div><strong>{u.site_admin ? "Demote from site admin" : "Promote to site admin"}</strong>
            <p className="muted small">Site admins can manage every account, organization and repository on this instance.</p></div>
          {u.site_admin ? (
            <button type="button" className="btn btn-sm" disabled={act.busy || self} data-testid="sa-demote"
              onClick={() => doRun(() => adminApi.demote(u.login), `${u.login} is no longer a site admin.`)}>Demote</button>
          ) : (
            <button type="button" className="btn btn-sm" disabled={act.busy} data-testid="sa-promote"
              onClick={() => doRun(() => adminApi.promote(u.login), `${u.login} is now a site admin.`)}>Promote</button>
          )}
        </div>

        <form className="sa-action" onSubmit={(e) => { e.preventDefault(); void doRun(() => adminApi.renameUser(u.login, newLogin.trim()), "User renamed.").then((ok) => ok && nav(`/site-admin/users/${encodeURIComponent(newLogin.trim())}`, { replace: true })); }}>
          <div className="grow"><label htmlFor="sa-rename"><strong>Rename user</strong></label>
            <p className="muted small">Repositories follow the new name; old URLs stop working.</p>
            <input id="sa-rename" className="input" value={newLogin} onChange={(e) => setNewLogin(e.target.value)} /></div>
          <button type="submit" className="btn btn-sm" disabled={act.busy || !newLogin.trim() || newLogin.trim() === u.login}>Rename</button>
        </form>

        <form className="sa-action" onSubmit={(e) => { e.preventDefault(); void doRun(() => adminApi.setPassword(u.login, password), "Password updated; the user's web sessions were signed out.").then(() => setPassword("")); }}>
          <div className="grow"><label htmlFor="sa-password"><strong>Reset password</strong></label>
            <p className="muted small">Sets a new password (8+ characters) and signs the user out of the web UI.</p>
            <input id="sa-password" type="password" className="input" minLength={8} autoComplete="new-password" value={password} onChange={(e) => setPassword(e.target.value)} /></div>
          <button type="submit" className="btn btn-sm" disabled={act.busy || password.length < 8}>Set password</button>
        </form>

        <form className="sa-action" onSubmit={(e) => { e.preventDefault(); void doRun(() => adminApi.ldapUserMapping(u.login, dn.trim()), "LDAP mapping updated."); }}>
          <div className="grow"><label htmlFor="sa-ldap-dn"><strong>LDAP mapping</strong></label>
            <p className="muted small">The distinguished name this account maps to in the directory.</p>
            <input id="sa-ldap-dn" className="input" placeholder="uid=jdoe,ou=people,dc=example,dc=com" value={dn} onChange={(e) => setDn(e.target.value)} /></div>
          <div className="btn-group-col">
            <button type="submit" className="btn btn-sm" disabled={act.busy || !dn.trim()}>Update mapping</button>
            <button type="button" className="btn btn-sm" disabled={act.busy} onClick={() => doRun(() => adminApi.ldapUserSync(u.login), "LDAP sync queued.")}>Sync now</button>
          </div>
        </form>

        <form className="sa-action" onSubmit={async (e) => {
          e.preventDefault();
          let tok: Authorization | undefined;
          if (await act.run(async () => { tok = await adminApi.impersonate(u.login, scopes); })) { setImp(tok); reload(); }
        }}>
          <div className="grow"><strong>Impersonation token</strong>
            <p className="muted small">An OAuth token that acts as {u.login} (recorded in the audit log).</p>
            <fieldset className="sa-scopes" aria-label="Scopes">
              {SCOPES.map((s) => (
                <label key={s} className="check-row"><input type="checkbox" checked={scopes.includes(s)}
                  onChange={(e) => setScopes(e.target.checked ? [...scopes, s] : scopes.filter((x) => x !== s))} /> <code>{s}</code></label>
              ))}
            </fieldset></div>
          <div className="btn-group-col">
            <button type="submit" className="btn btn-sm" disabled={act.busy || u.suspended_at !== null} data-testid="sa-impersonate">Create token</button>
            <button type="button" className="btn btn-sm btn-danger-outline" disabled={act.busy || !u.tokens.some((t) => t.impersonation)}
              onClick={() => doRun(() => adminApi.deleteImpersonation(u.login), "Impersonation tokens revoked.")}>Revoke all</button>
          </div>
        </form>
      </div>

      <h3 className="h4 sa-subhead c-danger">Danger zone</h3>
      <div className="box danger-zone">
        {u.suspended_at ? (
          <div className="sa-action">
            <div><strong>Unsuspend user</strong><p className="muted small">The user can sign in, push and use tokens again.</p></div>
            <button type="button" className="btn btn-sm" disabled={act.busy} data-testid="sa-unsuspend"
              onClick={() => doRun(() => adminApi.unsuspend(u.login), `${u.login} was unsuspended.`)}>Unsuspend</button>
          </div>
        ) : (
          <form className="sa-action" onSubmit={(e) => { e.preventDefault(); void doRun(() => adminApi.suspend(u.login, reason.trim()), `${u.login} was suspended.`); }}>
            <div className="grow"><label htmlFor="sa-suspend-reason"><strong>Suspend user</strong></label>
              <p className="muted small">Suspended users cannot sign in, push, or use tokens or SSH keys. {u.site_admin && "Demote the user first."}</p>
              <input id="sa-suspend-reason" className="input" placeholder="Reason (shown in the audit log)" value={reason} onChange={(e) => setReason(e.target.value)} /></div>
            <button type="submit" className="btn btn-sm btn-danger" disabled={act.busy || self || u.site_admin} data-testid="sa-suspend">Suspend</button>
          </form>
        )}
        <form className="sa-action" onSubmit={async (e) => {
          e.preventDefault();
          if (await act.run(() => adminApi.deleteUser(u.login))) nav("/site-admin/users", { replace: true });
        }}>
          <div className="grow"><label htmlFor="sa-del-user"><strong>Delete user</strong></label>
            <p className="muted small">Deletes the account and every repository it owns. Type <strong>{u.login}</strong> to confirm. {u.site_admin && "Demote the user first."}</p>
            <input id="sa-del-user" className="input" autoComplete="off" value={confirm} onChange={(e) => setConfirm(e.target.value)} /></div>
          <button type="submit" className="btn btn-sm btn-danger" disabled={act.busy || self || u.site_admin || confirm !== u.login}>Delete user</button>
        </form>
      </div>
    </div>
  );
}

function confirmed(msg: string) {
  return window.confirm(msg);
}
