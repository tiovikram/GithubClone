// /explore: most starred and recently updated public repositories, people search.
import { useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { Async, Avatar, Blankslate, RelTime } from "../../components/common";
import { StarIcon } from "../../components/icons";
import { useAsync, useTitle } from "../../hooks";
import { sapi } from "./api";

export function ExplorePage() {
  const [sp, setSp] = useSearchParams();
  const tab = sp.get("tab") || "stars";
  const q = sp.get("q") || "";
  const [text, setText] = useState(q);
  useTitle("Explore");
  const repos = useAsync(() => (tab === "people" ? Promise.resolve(null) : sapi.searchRepos("is:public", tab === "updated" ? "updated" : "stars")), [tab]);
  const people = useAsync(() => (tab === "people" ? sapi.searchUsers(q ? `${q} type:user` : "type:user") : Promise.resolve(null)), [tab, q]);
  const cls = (k: string) => "tabnav-tab" + (tab === k ? " selected" : "");
  return (
    <div className="container explore">
      <h1 className="h3">Explore</h1>
      <nav className="tabnav" aria-label="Explore">
        <Link to="/explore" className={cls("stars")} data-testid="explore-tab-stars">Most starred</Link>
        <Link to="/explore?tab=updated" className={cls("updated")} data-testid="explore-tab-updated">Recently updated</Link>
        <Link to="/explore?tab=people" className={cls("people")} data-testid="explore-tab-people">People</Link>
        <Link to="/gists/discover" className="tabnav-tab">Gists</Link>
      </nav>
      {tab === "people" ? (
        <>
          <form className="input-group" role="search" onSubmit={(e) => { e.preventDefault(); setSp({ tab: "people", q: text.trim() }); }}>
            <label htmlFor="explore-q" className="sr-only">Search people</label>
            <input id="explore-q" className="input" placeholder="Search people by username or name" value={text} onChange={(e) => setText(e.target.value)} data-testid="explore-people-q" />
            <button type="submit" className="btn">Search</button>
          </form>
          <Async state={people}>
            {(r) => !r || r.items.items.length === 0 ? <Blankslate title="No users found" /> : (
              <ul className="box list" data-testid="explore-people">
                {r.items.items.map((u) => (
                  <li key={u.id} className="list-row people-row">
                    <Avatar user={u} size={32} />
                    <Link to={`/${u.login}`}><strong>{u.login}</strong></Link>
                  </li>
                ))}
              </ul>
            )}
          </Async>
        </>
      ) : (
        <Async state={repos}>
          {(r) => !r || r.items.length === 0 ? <Blankslate title="No public repositories yet" /> : (
            <ul className="box list" data-testid="explore-repos">
              {r.items.map((x) => (
                <li key={x.id} className="list-row explore-repo">
                  <Avatar user={x.owner} size={32} />
                  <div className="grow">
                    <Link to={`/${x.full_name}`}><strong>{x.full_name}</strong></Link>
                    {x.description && <p className="muted small">{x.description}</p>}
                    <div className="muted small repo-row-meta">
                      {x.language && <span>{x.language}</span>}
                      <span><StarIcon size={12} /> {x.stargazers_count}</span>
                      <span>Updated <RelTime iso={x.pushed_at || x.updated_at} /></span>
                    </div>
                  </div>
                </li>
              ))}
            </ul>
          )}
        </Async>
      )}
    </div>
  );
}
