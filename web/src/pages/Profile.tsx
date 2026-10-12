import { Link, useParams, useSearchParams } from "react-router-dom";
import { api } from "../api";
import { useAuth } from "../auth";
import { Async, Avatar, Blankslate, Pagination, RelTime } from "../components/common";
import { LockIcon, RepoIcon, StarIcon } from "../components/icons";
import { useAsync, useTitle } from "../hooks";

export function ProfilePage() {
  const { owner = "" } = useParams();
  const { user: me } = useAuth();
  const [sp, setSp] = useSearchParams();
  const page = Number(sp.get("page") || 1);
  const tab = sp.get("tab") === "stars" ? "stars" : "repositories";
  const user = useAsync(() => api.user(owner), [owner]);
  const repos = useAsync(() => (tab === "stars" ? api.starred(owner, page) : api.userRepos(owner, page)), [owner, page, tab]);
  useTitle(user.data ? `${user.data.login}${user.data.name ? ` (${user.data.name})` : ""}` : owner);
  return (
    <Async state={user}>
      {(u) => (
        <div className="container profile">
          <aside className="profile-side">
            <Avatar user={u} size={260} />
            <h1 className="profile-names">
              {u.name && <span className="profile-name">{u.name}</span>}
              <span className="profile-login muted">{u.login}</span>
            </h1>
            {u.bio && <p>{u.bio}</p>}
            <ul className="profile-details muted small">
              {u.company && <li>{u.company}</li>}
              {u.location && <li>{u.location}</li>}
              {u.blog && <li><a href={u.blog.startsWith("http") ? u.blog : `https://${u.blog}`} rel="nofollow noopener">{u.blog}</a></li>}
              {u.created_at && <li>Joined <RelTime iso={u.created_at} /></li>}
            </ul>
            {me?.login === u.login && <Link to="/settings/tokens" className="btn btn-block">Access tokens</Link>}
          </aside>
          <section className="profile-main">
            <nav className="tabnav" aria-label="Profile">
              <Link to={`/${u.login}`} className={"tabnav-tab" + (tab === "repositories" ? " selected" : "")}>
                <RepoIcon /> Repositories {u.public_repos !== undefined && <span className="counter">{u.public_repos}</span>}
              </Link>
              <Link to={`/${u.login}?tab=stars`} className={"tabnav-tab" + (tab === "stars" ? " selected" : "")} data-testid="stars-tab">
                <StarIcon /> Stars
              </Link>
            </nav>
            <div className="toolbar">
              <span className="spacer" />
              {me?.login === u.login && <Link to="/new" className="btn btn-primary btn-sm">New</Link>}
            </div>
            <Async state={repos}>
              {(p) =>
                p.items.length === 0 ? (
                  <Blankslate title={tab === "stars" ? `${u.login} hasn't starred any repositories yet.` : `${u.login} doesn't have any public repositories yet.`} />
                ) : (
                  <ul className="list repo-list">
                    {p.items.map((r) => (
                      <li key={r.id} className="list-row repo-row">
                        <div>
                          <h3 className="h4 repo-row-title">
                            <Link to={`/${r.full_name}`}>{tab === "stars" || r.owner.login !== u.login ? r.full_name : r.name}</Link>{" "}
                            <span className="badge">{r.private ? <><LockIcon size={12} /> Private</> : "Public"}</span>
                          </h3>
                          {r.fork && r.parent && <p className="muted small">Forked from {r.parent.full_name}</p>}
                          {r.description && <p className="muted">{r.description}</p>}
                          <div className="muted small repo-row-meta">
                            {r.language && <span><span className="lang-dot" /> {r.language}</span>}
                            {r.stargazers_count > 0 && <span><StarIcon size={12} /> {r.stargazers_count}</span>}
                            <span>Updated <RelTime iso={r.pushed_at || r.updated_at} /></span>
                          </div>
                        </div>
                      </li>
                    ))}
                  </ul>
                )
              }
            </Async>
            {repos.data && <Pagination page={page} next={repos.data.next} prev={repos.data.prev} last={repos.data.last}
              onPage={(n) => setSp(tab === "stars" ? { tab, page: String(n) } : { page: String(n) })} />}
          </section>
        </div>
      )}
    </Async>
  );
}
