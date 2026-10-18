import { useState } from "react";
import { Link } from "react-router-dom";
import { api, type Issue, type User } from "../api";
import { useAuth } from "../auth";
import { Async, Avatar, Blankslate, ErrorBox, ItemIcon, RelTime, Spinner } from "../components/common";
import { LockIcon, LogoIcon, RepoIcon } from "../components/icons";
import { useAsync, useTitle } from "../hooks";
import { Slot } from "../areas";
import { DashboardFeed } from "../areas/social/dashboard";

function Landing() {
  return (
    <div className="landing">
      <LogoIcon size={64} />
      <h1>Where code gets forged.</h1>
      <p className="lead muted">
        A self-hosted, GitHub-compatible code forge. Works with stock <code>git</code> and the <code>gh</code> CLI.
      </p>
      <div className="landing-actions">
        <Link to="/signup" className="btn btn-primary btn-lg">Sign up</Link>
        <Link to="/login" className="btn btn-lg">Sign in</Link>
      </div>
    </div>
  );
}

function repoFromIssue(i: Issue): string {
  if (i.repository?.full_name) return i.repository.full_name;
  const m = i.repository_url?.match(/repos\/([^/]+\/[^/]+)$/);
  return m ? m[1] : "";
}

/** Recent open issues/PRs involving the user: GET /issues, falling back to /search/issues. */
async function loadRecent(user: User): Promise<Issue[]> {
  try {
    return await api.myIssues();
  } catch {
    const r = await api.searchIssues(`involves:${user.login} is:open`);
    return r.items;
  }
}

function Dashboard({ user }: { user: User }) {
  const repos = useAsync(() => api.myRepos(), [user.login]);
  const recent = useAsync(() => loadRecent(user), [user.login]);
  const [filter, setFilter] = useState("");
  const list = (repos.data || []).filter((r) => r.full_name.toLowerCase().includes(filter.toLowerCase()));
  return (
    <div className="container dashboard">
      <aside className="dashboard-side">
        <div className="side-head">
          <h2 className="h5">Top repositories</h2>
          <Link to="/new" className="btn btn-sm btn-primary"><RepoIcon /> New</Link>
        </div>
        <label htmlFor="repo-filter" className="sr-only">Find a repository</label>
        <input id="repo-filter" className="input input-sm" placeholder="Find a repository…" value={filter}
          onChange={(e) => setFilter(e.target.value)} />
        {repos.loading ? <Spinner /> : repos.error !== undefined ? <ErrorBox error={repos.error} onRetry={repos.reload} /> : (
          <ul className="side-repos">
            {list.map((r) => (
              <li key={r.id}>
                <Avatar user={r.owner} size={16} />
                <Link to={`/${r.full_name}`}>{r.full_name}</Link>
                {r.private && <LockIcon className="icon muted" size={12} />}
              </li>
            ))}
            {list.length === 0 && <li className="muted small">{filter ? "No matching repositories." : "You don't have any repositories yet."}</li>}
          </ul>
        )}
        <Slot name="home.sidebar" user={user} />
      </aside>
      <section className="dashboard-main">
        <h1 className="h3">Welcome, {user.name || user.login}</h1>
        {repos.data && repos.data.length === 0 && (
          <div className="box box-body getting-started">
            <h2 className="h4">Create your first repository</h2>
            <p className="muted">Repositories hold your project's files and history.</p>
            <Link to="/new" className="btn btn-primary">Create repository</Link>{" "}
            <Link to="/settings/tokens" className="btn">Set up git &amp; gh</Link>
          </div>
        )}
        <h2 className="h4">Recent issues and pull requests</h2>
        <Async state={recent} notFound={<Blankslate title="Nothing here yet" />}>
          {(items) =>
            items.length === 0 ? (
              <Blankslate title="No open issues or pull requests">Issues and pull requests you're involved in will show up here.</Blankslate>
            ) : (
              <ul className="box list">
                {items.map((i) => {
                  const full = repoFromIssue(i);
                  return (
                    <li key={i.id} className="list-row issue-row">
                      <ItemIcon item={i} />
                      <div className="issue-row-main">
                        <span className="muted small">{full}</span>{" "}
                        <Link className="issue-title" to={`/${full}/${i.pull_request ? "pull" : "issues"}/${i.number}`}>{i.title}</Link>
                        <div className="muted small">#{i.number} updated <RelTime iso={i.updated_at} /></div>
                      </div>
                    </li>
                  );
                })}
              </ul>
            )
          }
        </Async>
        <DashboardFeed user={user} />
      </section>
    </div>
  );
}

export function HomePage() {
  const { user, loading } = useAuth();
  useTitle(user ? "Dashboard" : "");
  if (loading) return <Spinner />;
  return user ? <Dashboard user={user} /> : <Landing />;
}
