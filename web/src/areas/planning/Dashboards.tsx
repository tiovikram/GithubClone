// Global `/issues` and `/pulls` dashboards (github.com/issues, github.com/pulls): the signed-in
// user's issues / pull requests across repositories, by search.
import { useEffect, useState } from "react";
import { Link, Navigate, useSearchParams } from "react-router-dom";
import { useAuth } from "../../auth";
import { Async, Blankslate, Pagination } from "../../components/common";
import { IssueClosedIcon, IssueOpenIcon, PullIcon } from "../../components/icons";
import { useAsync, useTitle } from "../../hooks";
import { IssueRow } from "../../pages/Issues";
import { papi } from "./api";

type Kind = "issues" | "pulls";

const TABS: Record<Kind, { key: string; label: string; q: (me: string) => string }[]> = {
  issues: [
    { key: "created", label: "Created", q: (me) => `author:${me}` },
    { key: "assigned", label: "Assigned", q: (me) => `assignee:${me}` },
    { key: "mentioned", label: "Mentioned", q: (me) => `mentions:${me}` },
  ],
  pulls: [
    { key: "created", label: "Created", q: (me) => `author:${me}` },
    { key: "assigned", label: "Assigned", q: (me) => `assignee:${me}` },
    { key: "mentioned", label: "Mentioned", q: (me) => `mentions:${me}` },
    { key: "review-requested", label: "Review requests", q: (me) => `review-requested:${me}` },
  ],
};

export function Dashboard({ kind }: { kind: Kind }) {
  const { user, loading } = useAuth();
  const [sp, setSp] = useSearchParams();
  const tab = TABS[kind].find((t) => t.key === sp.get("tab")) || TABS[kind][0];
  const state = sp.get("state") === "closed" ? "closed" : "open";
  const page = Number(sp.get("page") || 1);
  const extra = sp.get("q") || "";
  const [input, setInput] = useState(extra);
  useEffect(() => setInput(extra), [extra]);
  const me = user?.login || "";
  const type = kind === "issues" ? "is:issue" : "is:pr";
  const query = `${type} is:${state} ${tab.q(me)} archived:false ${extra}`.trim();
  useTitle(kind === "issues" ? "Your issues" : "Your pull requests");
  const st = useAsync(() => (me ? papi.search(query, page) : Promise.resolve(null)), [query, page, me]);
  const counts = useAsync(async () => {
    if (!me) return null;
    const base = `${type} ${tab.q(me)} archived:false ${extra}`.trim();
    const [o, c] = await Promise.all([papi.search(`${base} is:open`), papi.search(`${base} is:closed`)]);
    return { open: o.items.total_count, closed: c.items.total_count };
  }, [me, type, tab.key, extra]);
  if (!loading && !user) return <Navigate to={`/login?return_to=/${kind}`} replace />;
  const set = (k: string, v: string | null) => {
    const n = new URLSearchParams(sp);
    if (v) n.set(k, v); else n.delete(k);
    if (k !== "page") n.delete("page");
    setSp(n);
  };
  return (
    <div className="container pl-dashboard">
      <div className="toolbar pl-dash-toolbar">
        <nav className="segmented pl-dash-tabs" aria-label={kind === "issues" ? "Issue filters" : "Pull request filters"}>
          {TABS[kind].map((t) => (
            <button key={t.key} type="button" className={"btn btn-sm" + (t.key === tab.key ? " selected" : "")} aria-pressed={t.key === tab.key}
              data-testid={`dash-tab-${t.key}`} onClick={() => set("tab", t.key === "created" ? null : t.key)}>
              {t.label}
            </button>
          ))}
        </nav>
        <form className="pl-search" role="search" onSubmit={(e) => { e.preventDefault(); set("q", input.trim() || null); }}>
          <label className="sr-only" htmlFor="dash-search">Search</label>
          <input id="dash-search" className="input" value={input} onChange={(e) => setInput(e.target.value)}
            placeholder={`Search ${kind === "issues" ? "issues" : "pull requests"}`} />
        </form>
      </div>
      <p className="muted small pl-dash-query" data-testid="dash-query"><code>{query}</code></p>
      <div className="box">
        <div className="box-header list-tabs" role="tablist" aria-label="State">
          <button type="button" role="tab" aria-selected={state === "open"} className={state === "open" ? "selected" : ""} onClick={() => set("state", null)}>
            {kind === "issues" ? <IssueOpenIcon /> : <PullIcon />} {counts.data ? `${counts.data.open} ` : ""}Open
          </button>
          <button type="button" role="tab" aria-selected={state === "closed"} className={state === "closed" ? "selected" : ""} onClick={() => set("state", "closed")}>
            <IssueClosedIcon /> {counts.data ? `${counts.data.closed} ` : ""}Closed
          </button>
        </div>
        <Async state={st}>
          {(r) => !r || r.items.items.length === 0 ? (
            <Blankslate title={`No results matched your search.`} icon={kind === "issues" ? <IssueOpenIcon size={24} /> : <PullIcon size={24} />}>
              {kind === "issues" ? <>Try <Link to="/issues?tab=assigned">issues assigned to you</Link>.</> : null}
            </Blankslate>
          ) : (
            <ul className="list">
              {r.items.items.map((i) => {
                const repo = i.repository_url.split("/repos/")[1] || "";
                return <IssueRow key={i.id} item={i} base={`/${repo}`} kind={i.pull_request ? "pull" : "issues"} showRepo />;
              })}
            </ul>
          )}
        </Async>
      </div>
      {st.data && <Pagination page={page} next={st.data.next} prev={st.data.prev} last={st.data.last} onPage={(x) => set("page", String(x))} />}
    </div>
  );
}
