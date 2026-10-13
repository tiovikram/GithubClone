import { useEffect, useState, type FormEvent, type ReactNode } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { api, type CodeSearchItem, type Issue, type Page, type Repo, type SearchResult } from "../api";
import { Async, Blankslate, ItemIcon, Pagination, RelTime } from "../components/common";
import { CodeIcon, IssueOpenIcon, LockIcon, RepoIcon, SearchIcon, StarIcon } from "../components/icons";
import { useAsync, useTitle } from "../hooks";
import { encPath } from "../util";

type Kind = "repositories" | "issues" | "code";
const KINDS: { kind: Kind; label: string; icon: ReactNode }[] = [
  { kind: "repositories", label: "Repositories", icon: <RepoIcon /> },
  { kind: "issues", label: "Issues & pull requests", icon: <IssueOpenIcon /> },
  { kind: "code", label: "Code", icon: <CodeIcon /> },
];

function repoOfIssue(i: Issue): string {
  const m = i.repository_url?.match(/repos\/([^/]+\/[^/]+)$/);
  return i.repository?.full_name || (m ? m[1] : "");
}

/** Path to a repo-relative html_url: strip the scheme/host so SPA links stay on this origin. */
function localPath(url: string): string {
  return url.replace(/^https?:\/\/[^/]+/, "");
}

/** `/search?q=…&type=repositories|issues|code` */
export function SearchPage() {
  const [sp, setSp] = useSearchParams();
  const q = sp.get("q") || "";
  const type = (KINDS.find((k) => k.kind === sp.get("type"))?.kind || "repositories") as Kind;
  const page = Number(sp.get("page") || 1);
  const [input, setInput] = useState(q);
  useEffect(() => setInput(q), [q]);
  useTitle(q ? `Search · ${q}` : "Search");
  const st = useAsync<Page<SearchResult<unknown>> | null>(async () => {
    if (!q.trim()) return null;
    if (type === "issues") return api.searchIssuesPage(q, page);
    if (type === "code") return api.searchCode(q, page);
    return api.searchRepos(q, page);
  }, [q, type, page]);
  const go = (next: { q?: string; type?: Kind; page?: number }) => {
    const n = new URLSearchParams();
    n.set("q", next.q ?? q);
    n.set("type", next.type ?? type);
    if (next.page && next.page > 1) n.set("page", String(next.page));
    setSp(n);
  };
  const submit = (e: FormEvent) => {
    e.preventDefault();
    go({ q: input.trim(), page: 1 });
  };
  return (
    <div className="container search-page">
      <form className="search-form" onSubmit={submit} role="search">
        <label htmlFor="search-q" className="sr-only">Search</label>
        <input id="search-q" className="input input-lg" value={input} placeholder="Search repositories, issues, code…"
          onChange={(e) => setInput(e.target.value)} autoFocus={!q} />
        <button type="submit" className="btn btn-primary"><SearchIcon /> Search</button>
      </form>
      <div className="search-layout">
        <nav className="search-kinds" aria-label="Result type">
          {KINDS.map((k) => (
            <button key={k.kind} type="button" className={"menu-item" + (k.kind === type ? " selected" : "")}
              aria-current={k.kind === type ? "page" : undefined} onClick={() => go({ type: k.kind, page: 1 })}>
              {k.icon} {k.label}
              {k.kind === type && st.data && <span className="counter">{st.data.items.total_count}</span>}
            </button>
          ))}
        </nav>
        <section className="search-results">
          {!q.trim() ? (
            <Blankslate title="Search forge" icon={<SearchIcon size={24} />}>
              Try <code>is:open</code>, <code>repo:owner/name</code>, <code>user:login</code>, or plain words.
            </Blankslate>
          ) : (
            <Async state={st}>
              {(p) => {
                if (!p) return null;
                const r = p.items;
                if (r.items.length === 0) return <Blankslate title={`No ${type} matched your search`} />;
                return (
                  <>
                    <h2 className="h4" data-testid="search-count">
                      {r.total_count.toLocaleString()} result{r.total_count === 1 ? "" : "s"}
                    </h2>
                    {type === "repositories" && <RepoResults items={r.items as Repo[]} />}
                    {type === "issues" && <IssueResults items={r.items as Issue[]} />}
                    {type === "code" && <CodeResults items={r.items as CodeSearchItem[]} />}
                    <Pagination page={page} next={p.next} prev={p.prev} last={p.last} onPage={(n) => go({ page: n })} />
                  </>
                );
              }}
            </Async>
          )}
        </section>
      </div>
    </div>
  );
}

function RepoResults({ items }: { items: Repo[] }) {
  return (
    <ul className="box list" data-testid="search-results">
      {items.map((r) => (
        <li key={r.id} className="list-row repo-row">
          <div>
            <h3 className="h5 repo-row-title">
              {r.private ? <LockIcon /> : <RepoIcon />} <Link to={`/${r.full_name}`}>{r.full_name}</Link>
            </h3>
            {r.description && <p className="muted">{r.description}</p>}
            <div className="muted small repo-row-meta">
              {r.language && <span>{r.language}</span>}
              <span><StarIcon size={12} /> {r.stargazers_count}</span>
              <span>Updated <RelTime iso={r.pushed_at || r.updated_at} /></span>
            </div>
          </div>
        </li>
      ))}
    </ul>
  );
}

function IssueResults({ items }: { items: Issue[] }) {
  return (
    <ul className="box list" data-testid="search-results">
      {items.map((i) => {
        const full = repoOfIssue(i);
        return (
          <li key={i.id} className="list-row issue-row">
            <ItemIcon item={i} />
            <div className="issue-row-main">
              <span className="muted small">{full}</span>{" "}
              <Link className="issue-title" to={`/${full}/${i.pull_request ? "pull" : "issues"}/${i.number}`}>{i.title}</Link>
              <div className="muted small">#{i.number} · updated <RelTime iso={i.updated_at} /></div>
            </div>
          </li>
        );
      })}
    </ul>
  );
}

function CodeResults({ items }: { items: CodeSearchItem[] }) {
  return (
    <ul className="box list" data-testid="search-results">
      {items.map((c) => {
        // html_url is {base}/{o}/{r}/blob/{sha}/{path}; fall back to the default branch if absent.
        const href = c.html_url ? localPath(c.html_url) : `/${c.repository.full_name}/blob/${encPath(c.repository.default_branch)}/${encPath(c.path)}`;
        return (
          <li key={`${c.repository.full_name}/${c.path}`} className="list-row code-result">
            <div className="grow">
              <div className="small">
                <Link to={`/${c.repository.full_name}`} className="muted-link">{c.repository.full_name}</Link>
                {" · "}
                <Link to={href} className="mono">{c.path}</Link>
              </div>
              {c.text_matches?.slice(0, 2).map((m, i) => <pre key={i} className="code-fragment">{m.fragment}</pre>)}
            </div>
          </li>
        );
      })}
    </ul>
  );
}
