// Organization profile: Overview (/:org), Repositories (/orgs/:org/repositories) and the
// "Create a new repository" page for an organization (/organizations/:org/repositories/new).
import { useMemo, useState, type FormEvent } from "react";
import { Link, Navigate, useNavigate, useParams, useSearchParams } from "react-router-dom";
import type { Repo, User } from "../../api";
import { useAuth } from "../../auth";
import { Async, Avatar, Blankslate, ErrorBox, Pagination, RelTime, Spinner } from "../../components/common";
import { LockIcon, RepoIcon, StarIcon, ForkIcon } from "../../components/icons";
import { useAsync, useTitle } from "../../hooks";
import { orgApi } from "./api";
import { OrgPage, WithOrg, OrgHeader, type OrgState } from "./common";

function RepoRow({ r }: { r: Repo }) {
  return (
    <li className="list-row repo-row" data-testid="org-repo-row">
      <div className="grow">
        <h3 className="h4 repo-row-title">
          <Link to={`/${r.full_name}`}>{r.name}</Link>{" "}
          <span className="badge">{r.private ? <><LockIcon size={12} /> Private</> : "Public"}</span>
        </h3>
        {r.fork && <p className="muted small">{r.parent ? `Forked from ${r.parent.full_name}` : "Forked repository"}</p>}
        {r.description && <p className="muted">{r.description}</p>}
        <div className="muted small repo-row-meta">
          {r.language && <span><span className="lang-dot" /> {r.language}</span>}
          {r.stargazers_count > 0 && <span><StarIcon size={12} /> {r.stargazers_count}</span>}
          {r.forks_count > 0 && <span><ForkIcon size={12} /> {r.forks_count}</span>}
          <span>Updated <RelTime iso={r.pushed_at || r.updated_at} /></span>
        </div>
      </div>
    </li>
  );
}

function canCreateRepo(st: OrgState) {
  return st.isOwner || (st.membership?.state === "active" && st.membership.permissions?.can_create_repository !== false);
}

/** Registered as `orgProfile`: replaces the user profile at /:login for organizations. */
export function OrgProfile({ org }: { org: User }) {
  useTitle(org.name || org.login);
  return <OrgPage login={org.login} active="overview">{(st) => <Overview st={st} />}</OrgPage>;
}

function Overview({ st }: { st: OrgState }) {
  const { org } = st;
  const repos = useAsync(() => orgApi.allRepos(org.login), [org.login]);
  const people = useAsync(() => (st.isMember ? orgApi.members(org.login) : orgApi.publicMembers(org.login)), [org.login, st.isMember]);
  const popular = useMemo(
    () => [...(repos.data || [])].sort((a, b) => b.stargazers_count - a.stargazers_count || (b.pushed_at || b.updated_at).localeCompare(a.pushed_at || a.updated_at)).slice(0, 6),
    [repos.data],
  );
  const recent = useMemo(
    () => [...(repos.data || [])].sort((a, b) => (b.pushed_at || b.updated_at).localeCompare(a.pushed_at || a.updated_at)).slice(0, 10),
    [repos.data],
  );
  const langs = useMemo(() => {
    const m = new Map<string, number>();
    for (const r of repos.data || []) if (r.language) m.set(r.language, (m.get(r.language) || 0) + 1);
    return [...m.entries()].sort((a, b) => b[1] - a[1]).slice(0, 5);
  }, [repos.data]);
  return (
    <div className="org-overview">
      <div className="org-overview-main">
        <Async state={repos}>
          {(list) => list.length === 0 ? (
            <Blankslate title="This organization has no repositories yet." icon={<RepoIcon size={24} />}>
              {canCreateRepo(st) && <Link to={`/organizations/${org.login}/repositories/new`} className="btn btn-primary">Create a new repository</Link>}
            </Blankslate>
          ) : (
            <>
              <h2 className="h4">Popular repositories</h2>
              <ul className="org-pinned" data-testid="org-popular">
                {popular.map((r) => (
                  <li key={r.id} className="box box-body org-pinned-card">
                    <div className="org-pinned-title">
                      <RepoIcon /> <Link to={`/${r.full_name}`}>{r.name}</Link>
                      <span className="badge">{r.private ? "Private" : "Public"}</span>
                    </div>
                    <p className="muted small">{r.description || " "}</p>
                    <div className="muted small repo-row-meta">
                      {r.language && <span><span className="lang-dot" /> {r.language}</span>}
                      <span><StarIcon size={12} /> {r.stargazers_count}</span>
                      <span><ForkIcon size={12} /> {r.forks_count}</span>
                    </div>
                  </li>
                ))}
              </ul>
              <div className="toolbar org-overview-repos-head">
                <h2 className="h4">Repositories</h2>
                <span className="spacer" />
                {canCreateRepo(st) && <Link to={`/organizations/${org.login}/repositories/new`} className="btn btn-sm btn-primary">New</Link>}
              </div>
              <ul className="box list repo-list">{recent.map((r) => <RepoRow key={r.id} r={r} />)}</ul>
              <Link to={`/orgs/${org.login}/repositories`} className="btn btn-sm">View all repositories</Link>
            </>
          )}
        </Async>
      </div>
      <aside className="org-overview-side">
        <div className="sidebar-section">
          <h2 className="h5"><Link to={`/orgs/${org.login}/people`} className="muted-link">People</Link></h2>
          {people.data ? (
            people.data.length ? (
              <div className="org-avatars" data-testid="org-people-avatars">
                {people.data.slice(0, 24).map((u) => <Link key={u.id} to={`/${u.login}`} title={u.login} aria-label={u.login}><Avatar user={u} size={32} /></Link>)}
              </div>
            ) : <p className="muted small">This organization has no public members.</p>
          ) : <Spinner />}
        </div>
        {langs.length > 0 && (
          <div className="sidebar-section">
            <h2 className="h5">Top languages</h2>
            <ul className="org-langs small">
              {langs.map(([l]) => <li key={l}><span className="lang-dot" /> {l}</li>)}
            </ul>
          </div>
        )}
        {st.isMember && st.membership && (
          <p className="muted small">You are {st.membership.role === "admin" ? "an owner" : "a member"} of this organization.</p>
        )}
      </aside>
    </div>
  );
}

const PER = 30;

/** /orgs/:org/repositories */
export function OrgRepositoriesPage() {
  const { org = "" } = useParams();
  useTitle(`${org} repositories`);
  return <OrgPage login={org} active="repositories">{(st) => <RepoList st={st} />}</OrgPage>;
}

function RepoList({ st }: { st: OrgState }) {
  const { org } = st;
  const [sp, setSp] = useSearchParams();
  const q = sp.get("q") || "";
  const type = sp.get("type") || "all";
  const sort = sp.get("sort") || "updated";
  const page = Number(sp.get("page") || 1);
  const repos = useAsync(() => orgApi.allRepos(org.login, type === "public" || type === "private" || type === "forks" || type === "sources" ? type : "all"), [org.login, type]);
  const set = (k: string, v: string) => {
    const n = new URLSearchParams(sp);
    if (v) n.set(k, v);
    else n.delete(k);
    n.delete("page");
    setSp(n, { replace: true });
  };
  const filtered = useMemo(() => {
    let l = repos.data || [];
    if (q) l = l.filter((r) => r.name.toLowerCase().includes(q.toLowerCase()) || (r.description || "").toLowerCase().includes(q.toLowerCase()));
    if (type === "archived") l = l.filter((r) => (r as Repo & { archived?: boolean }).archived);
    const key = (r: Repo) => r.pushed_at || r.updated_at;
    return [...l].sort((a, b) =>
      sort === "name" ? a.name.localeCompare(b.name) : sort === "stars" ? b.stargazers_count - a.stargazers_count : key(b).localeCompare(key(a)));
  }, [repos.data, q, sort, type]);
  const last = Math.max(1, Math.ceil(filtered.length / PER));
  const shown = filtered.slice((page - 1) * PER, page * PER);
  return (
    <>
      <div className="toolbar org-repo-filters">
        <label htmlFor="org-repo-q" className="sr-only">Find a repository</label>
        <input id="org-repo-q" className="input grow" type="search" placeholder="Find a repository…" defaultValue={q}
          onChange={(e) => set("q", e.target.value)} />
        <label className="sr-only" htmlFor="org-repo-type">Type</label>
        <select id="org-repo-type" className="input input-auto" value={type} onChange={(e) => set("type", e.target.value === "all" ? "" : e.target.value)}>
          <option value="all">Type: All</option>
          <option value="public">Public</option>
          <option value="private">Private</option>
          <option value="sources">Sources</option>
          <option value="forks">Forks</option>
          <option value="archived">Archived</option>
        </select>
        <label className="sr-only" htmlFor="org-repo-sort">Sort</label>
        <select id="org-repo-sort" className="input input-auto" value={sort} onChange={(e) => set("sort", e.target.value === "updated" ? "" : e.target.value)}>
          <option value="updated">Sort: Last updated</option>
          <option value="name">Name</option>
          <option value="stars">Stars</option>
        </select>
        {canCreateRepo(st) && <Link to={`/organizations/${org.login}/repositories/new`} className="btn btn-primary" data-testid="org-new-repo">New repository</Link>}
      </div>
      <Async state={repos}>
        {() => shown.length === 0 ? (
          <Blankslate title={q || type !== "all" ? "No repositories matched your search." : "This organization has no repositories yet."} />
        ) : (
          <ul className="box list repo-list">{shown.map((r) => <RepoRow key={r.id} r={r} />)}</ul>
        )}
      </Async>
      {last > 1 && <Pagination page={page} last={last} next={page < last ? page + 1 : undefined} prev={page > 1 ? page - 1 : undefined}
        onPage={(n) => { const x = new URLSearchParams(sp); x.set("page", String(n)); setSp(x); }} />}
    </>
  );
}

/** /organizations/:org/repositories/new */
export function NewOrgRepoPage() {
  const { org = "" } = useParams();
  const { user, loading } = useAuth();
  useTitle("New repository");
  if (loading) return <Spinner />;
  if (!user) return <Navigate to={`/login?return_to=${encodeURIComponent(location.pathname)}`} replace />;
  return (
    <WithOrg login={org}>
      {(st) => <><OrgHeader st={st} active="repositories" /><NewOrgRepoForm st={st} /></>}
    </WithOrg>
  );
}

function NewOrgRepoForm({ st }: { st: OrgState }) {
  const nav = useNavigate();
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [priv, setPriv] = useState(false);
  const [init, setInit] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>();
  if (!st.isMember) return <Blankslate title="You must be a member of this organization to create repositories in it." />;
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(undefined);
    try {
      const r = await orgApi.createRepo(st.org.login, { name, description: description || undefined, private: priv, auto_init: init });
      nav(`/${r.full_name}`);
    } catch (err) {
      setError(err);
      setBusy(false);
    }
  };
  return (
    <div className="container narrow">
      <h1 className="h2">Create a new repository</h1>
      <form onSubmit={submit} className="new-repo-form">
        {error !== undefined && <ErrorBox error={error} />}
        <div className="form-row">
          <div className="form-group">
            <span className="label-text">Owner</span>
            <div className="owner-pill"><Avatar user={st.org} size={20} /> {st.org.login}</div>
          </div>
          <span className="slash" aria-hidden="true">/</span>
          <div className="form-group grow">
            <label htmlFor="org-repo-name">Repository name</label>
            <input id="org-repo-name" className="input" required autoFocus value={name} onChange={(e) => setName(e.target.value)} />
          </div>
        </div>
        <div className="form-group">
          <label htmlFor="org-repo-desc">Description <span className="muted">(optional)</span></label>
          <input id="org-repo-desc" className="input" value={description} onChange={(e) => setDescription(e.target.value)} />
        </div>
        <fieldset className="form-group radio-group">
          <legend>Visibility</legend>
          <label className="radio-row">
            <input type="radio" name="org-vis" checked={!priv} onChange={() => setPriv(false)} />
            <span><strong>Public</strong><br /><span className="muted small">Anyone can see this repository.</span></span>
          </label>
          <label className="radio-row">
            <input type="radio" name="org-vis" checked={priv} onChange={() => setPriv(true)} data-testid="org-repo-private" />
            <span><strong>Private</strong><br /><span className="muted small">Only people with access through the organization can see it.</span></span>
          </label>
        </fieldset>
        <label className="check-row form-group">
          <input type="checkbox" checked={init} onChange={(e) => setInit(e.target.checked)} /> Add a README file
        </label>
        <div className="form-actions">
          <button type="submit" className="btn btn-primary" disabled={busy || !name}>{busy ? "Creating…" : "Create repository"}</button>
        </div>
      </form>
    </div>
  );
}
