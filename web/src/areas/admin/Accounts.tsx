// Organizations, repositories, public keys and access tokens across the instance.
import { useState, type FormEvent } from "react";
import { Link } from "react-router-dom";
import { Async, Avatar, Blankslate, Pagination, RelTime } from "../../components/common";
import { useAsync } from "../../hooks";
import { humanSize } from "../../util";
import { adminApi, type AdminOrg, type AdminRepo } from "./api";
import { PageHead, Pager, Pill, useAction } from "./ui";

export function OrgsSection() {
  const [q, setQ] = useState("");
  const [query, setQuery] = useState("");
  const [page, setPage] = useState(1);
  const [creating, setCreating] = useState(false);
  const list = useAsync(() => adminApi.orgs(query, page), [query, page]);
  return (
    <div data-testid="sa-orgs">
      <PageHead title="Organizations" desc="Every organization on this instance.">
        <button type="button" className="btn btn-primary btn-sm" onClick={() => setCreating(!creating)} data-testid="sa-new-org">New organization</button>
      </PageHead>
      {creating && <CreateOrg onDone={() => { setCreating(false); list.reload(); }} />}
      <form className="sa-filter" role="search" onSubmit={(e) => { e.preventDefault(); setPage(1); setQuery(q.trim()); }}>
        <label htmlFor="sa-org-q" className="sr-only">Search organizations</label>
        <input id="sa-org-q" className="input" placeholder="Search organizations" value={q} onChange={(e) => setQ(e.target.value)} />
        <button type="submit" className="btn">Search</button>
      </form>
      <Async state={list}>
        {(l) => l.items.length === 0 ? <Blankslate title="No organizations found" /> : (
          <>
            <ul className="box list" aria-label="Organizations">
              {l.items.map((o) => <OrgRow key={o.id} o={o} onChange={list.reload} />)}
            </ul>
            <Pager page={page} total={l.total_count} per={30} onPage={setPage} />
          </>
        )}
      </Async>
    </div>
  );
}

function CreateOrg({ onDone }: { onDone: () => void }) {
  const [login, setLogin] = useState("");
  const [admin, setAdmin] = useState("");
  const [name, setName] = useState("");
  const act = useAction();
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (await act.run(() => adminApi.createOrg({ login: login.trim(), admin: admin.trim(), profile_name: name.trim() || undefined }))) onDone();
  };
  return (
    <form className="box box-body" onSubmit={submit} aria-label="Create organization">
      <h3 className="h5">Create an organization</h3>
      {act.status}
      <div className="form-row">
        <div className="form-group">
          <label htmlFor="sa-co-login">Organization name</label>
          <input id="sa-co-login" className="input" required value={login} onChange={(e) => setLogin(e.target.value)} />
        </div>
        <div className="form-group">
          <label htmlFor="sa-co-admin">Owner (username)</label>
          <input id="sa-co-admin" className="input" required value={admin} onChange={(e) => setAdmin(e.target.value)} />
        </div>
        <div className="form-group">
          <label htmlFor="sa-co-name">Display name</label>
          <input id="sa-co-name" className="input" value={name} onChange={(e) => setName(e.target.value)} />
        </div>
      </div>
      <button type="submit" className="btn btn-primary" disabled={act.busy}>{act.busy ? "Creating…" : "Create organization"}</button>
    </form>
  );
}

function OrgRow({ o, onChange }: { o: AdminOrg; onChange: () => void }) {
  const [renaming, setRenaming] = useState(false);
  const [login, setLogin] = useState(o.login);
  const act = useAction();
  return (
    <li className="list-row sa-row" data-testid={`sa-org-${o.login}`}>
      <Avatar user={o} size={32} />
      <div className="grow sa-row-main">
        <div className="sa-row-title"><Link to={`/${o.login}`}><strong>{o.login}</strong></Link>{o.name && <span className="muted">{o.name}</span>}</div>
        <div className="muted small">{o.members} member{o.members === 1 ? "" : "s"} · {o.owners} owner{o.owners === 1 ? "" : "s"} · {o.repos} repositor{o.repos === 1 ? "y" : "ies"} · {o.teams} team{o.teams === 1 ? "" : "s"} · created <RelTime iso={o.created_at} /></div>
        {act.status}
        {renaming && (
          <form className="sa-inline-form" onSubmit={async (e) => { e.preventDefault(); if (await act.run(() => adminApi.renameOrg(o.login, login.trim()))) onChange(); }}>
            <label htmlFor={`sa-ro-${o.id}`} className="sr-only">New name</label>
            <input id={`sa-ro-${o.id}`} className="input input-sm" value={login} onChange={(e) => setLogin(e.target.value)} />
            <button type="submit" className="btn btn-sm" disabled={act.busy || !login.trim() || login.trim() === o.login}>Rename</button>
          </form>
        )}
      </div>
      <div className="sa-row-actions">
        <Link className="btn btn-sm" to={`/organizations/${o.login}/settings`}>Settings</Link>
        <button type="button" className="btn btn-sm" onClick={() => setRenaming(!renaming)}>Rename…</button>
      </div>
    </li>
  );
}

export function ReposSection() {
  const [q, setQ] = useState("");
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState("all");
  const [page, setPage] = useState(1);
  const list = useAsync(() => adminApi.repos(query, filter, page), [query, filter, page]);
  return (
    <div data-testid="sa-repos">
      <PageHead title="Repositories" desc="Every repository on this instance, including private ones." />
      <form className="sa-filter" role="search" onSubmit={(e) => { e.preventDefault(); setPage(1); setQuery(q.trim()); }}>
        <label htmlFor="sa-repo-q" className="sr-only">Search repositories</label>
        <input id="sa-repo-q" className="input" placeholder="owner/name or description" value={q} onChange={(e) => setQ(e.target.value)} />
        <label htmlFor="sa-repo-filter" className="sr-only">Filter</label>
        <select id="sa-repo-filter" className="input input-auto" value={filter} onChange={(e) => { setPage(1); setFilter(e.target.value); }}>
          <option value="all">All</option>
          <option value="public">Public</option>
          <option value="private">Private</option>
          <option value="archived">Archived</option>
          <option value="fork">Forks</option>
        </select>
        <button type="submit" className="btn">Search</button>
      </form>
      <Async state={list}>
        {(l) => l.items.length === 0 ? <Blankslate title="No repositories found" /> : (
          <>
            <p className="muted small">{l.total_count} repositor{l.total_count === 1 ? "y" : "ies"}</p>
            <ul className="box list" aria-label="Repositories">
              {l.items.map((r) => <RepoRow key={r.id} r={r} onChange={list.reload} />)}
            </ul>
            <Pager page={page} total={l.total_count} per={30} onPage={setPage} />
          </>
        )}
      </Async>
    </div>
  );
}

function RepoRow({ r, onChange }: { r: AdminRepo; onChange: () => void }) {
  const act = useAction();
  const [confirm, setConfirm] = useState(false);
  const [typed, setTyped] = useState("");
  return (
    <li className="list-row sa-row" data-testid={`sa-repo-${r.full_name}`}>
      <div className="grow sa-row-main">
        <div className="sa-row-title">
          <Link to={`/${r.full_name}`}><strong>{r.full_name}</strong></Link>
          <Pill>{r.visibility}</Pill>{r.archived && <Pill kind="scheduled">archived</Pill>}{r.fork && <Pill>fork</Pill>}
        </div>
        {r.description && <div className="small">{r.description}</div>}
        <div className="muted small">{humanSize(r.size * 1024)} · created <RelTime iso={r.created_at} />{r.pushed_at ? <> · pushed <RelTime iso={r.pushed_at} /></> : ""}</div>
        {act.status}
        {confirm && (
          <form className="sa-inline-form" onSubmit={async (e) => { e.preventDefault(); if (await act.run(() => adminApi.deleteRepo(r.full_name))) onChange(); }}>
            <label htmlFor={`sa-dr-${r.id}`} className="small">Type <strong>{r.full_name}</strong> to delete</label>
            <input id={`sa-dr-${r.id}`} className="input input-sm" autoComplete="off" value={typed} onChange={(e) => setTyped(e.target.value)} />
            <button type="submit" className="btn btn-sm btn-danger" disabled={act.busy || typed !== r.full_name}>Delete</button>
          </form>
        )}
      </div>
      <div className="sa-row-actions">
        <Link className="btn btn-sm" to={`/${r.full_name}/settings`}>Settings</Link>
        <button type="button" className="btn btn-sm" disabled={act.busy}
          onClick={async () => { if (await act.run(() => adminApi.updateRepo(r.full_name, { archived: !r.archived }))) onChange(); }}>
          {r.archived ? "Unarchive" : "Archive"}
        </button>
        <button type="button" className="btn btn-sm btn-danger-outline" onClick={() => setConfirm(!confirm)}>Delete…</button>
      </div>
    </li>
  );
}

export function KeysSection() {
  const [page, setPage] = useState(1);
  const list = useAsync(() => adminApi.keys(page), [page]);
  const act = useAction();
  return (
    <div data-testid="sa-keys">
      <PageHead title="SSH keys" desc="Every user SSH key and repository deploy key; delete keys that should no longer have access." />
      {act.status}
      <Async state={list}>
        {(p) => p.items.length === 0 ? <Blankslate title="No SSH keys" /> : (
          <>
            <ul className="box list" aria-label="SSH keys">
              {p.items.map((k) => (
                <li key={`${k.repository_id ? "d" : "u"}${k.id}`} className="list-row sa-row">
                  <div className="grow sa-row-main">
                    <div className="sa-row-title"><strong>{k.title}</strong> <Pill>{k.repository_id ? "deploy key" : "user key"}</Pill>{k.read_only && <Pill>read-only</Pill>}</div>
                    <div className="mono small sa-break">{k.key.slice(0, 60)}…</div>
                    <div className="muted small">added <RelTime iso={k.created_at} />{k.added_by ? ` by ${k.added_by}` : ""}{k.last_used ? <> · last used <RelTime iso={k.last_used} /></> : " · never used"}</div>
                  </div>
                  <button type="button" className="btn btn-sm btn-danger-outline" disabled={act.busy}
                    onClick={async () => { if (window.confirm(`Delete key “${k.title}”?`) && await act.run(() => adminApi.deleteKey(k.id), "Key deleted.")) list.reload(); }}>Delete</button>
                </li>
              ))}
            </ul>
            <Pagination page={page} next={p.next} prev={p.prev} last={p.last} onPage={setPage} />
          </>
        )}
      </Async>
    </div>
  );
}

export function TokensSection() {
  const [page, setPage] = useState(1);
  const list = useAsync(() => adminApi.tokens(page), [page]);
  const act = useAction();
  return (
    <div data-testid="sa-tokens">
      <PageHead title="Access tokens" desc="Personal access and impersonation tokens of every user." />
      {act.status}
      <Async state={list}>
        {(p) => p.items.length === 0 ? <Blankslate title="No tokens" /> : (
          <>
            <ul className="box list" aria-label="Access tokens">
              {p.items.map((t) => (
                <li key={t.id} className="list-row sa-row">
                  <div className="grow sa-row-main">
                    <div className="sa-row-title"><strong>{t.note}</strong> <span className="muted">…{t.token_last_eight}</span></div>
                    <div className="muted small"><Link to={`/site-admin/users/${encodeURIComponent(t.user.login)}`}>{t.user.login}</Link> · {t.scopes.join(", ") || "no scopes"} · created <RelTime iso={t.created_at} /></div>
                  </div>
                  <button type="button" className="btn btn-sm btn-danger-outline" disabled={act.busy}
                    onClick={async () => { if (window.confirm(`Revoke token “${t.note}” of ${t.user.login}?`) && await act.run(() => adminApi.deleteToken(t.id), "Token revoked.")) list.reload(); }}>Revoke</button>
                </li>
              ))}
            </ul>
            <Pagination page={page} next={p.next} prev={p.prev} last={p.last} onPage={setPage} />
          </>
        )}
      </Async>
    </div>
  );
}
