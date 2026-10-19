// Milestones: `/{o}/{r}/milestones` (list with progress), `/milestones/new`, `/milestones/{n}/edit`
// and `/milestone/{n}` (the milestone's issues and pull requests).
import { useState } from "react";
import { Link, Navigate, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { errorMessage } from "../../api";
import { Async, Blankslate, ErrorBox, Pagination, RelTime } from "../../components/common";
import { MarkdownEditor } from "../../components/Conversation";
import { IssueClosedIcon, IssueOpenIcon } from "../../components/icons";
import { useRepo } from "../../components/RepoLayout";
import { useAsync, useTitle } from "../../hooks";
import { Markdown } from "../../markdown";
import { IssueRow } from "../../pages/Issues";
import { papi, type Milestone } from "./api";
import { confirmAction, Progress } from "./ui";

function dueText(m: Milestone) {
  if (m.state === "closed") return <>Closed <RelTime iso={m.closed_at} /></>;
  if (!m.due_on) return <>No due date</>;
  const d = new Date(m.due_on);
  const past = d.getTime() < Date.now();
  return <span className={past ? "c-danger" : undefined}>{past ? "Past due by " : "Due by "}{d.toLocaleDateString(undefined, { year: "numeric", month: "long", day: "numeric", timeZone: "UTC" })}</span>;
}

export function MilestonesPage() {
  const { repo, owner, name, base, canPush } = useRepo();
  const [sp, setSp] = useSearchParams();
  const state = sp.get("state") === "closed" ? "closed" : "open";
  const sort = sp.get("sort") || "due_on-asc";
  const [field, dir] = sort.split("-");
  useTitle(`Milestones · ${repo.full_name}`);
  const st = useAsync(() => papi.milestones(owner, name, state, field, dir), [owner, name, state, sort]);
  const counts = useAsync(async () => {
    const [o, c] = await Promise.all([papi.milestones(owner, name, "open"), papi.milestones(owner, name, "closed")]);
    return { open: o.length, closed: c.length };
  }, [owner, name, st.data]);
  const [err, setErr] = useState("");
  const set = (k: string, v: string | null) => {
    const n = new URLSearchParams(sp);
    if (v) n.set(k, v); else n.delete(k);
    setSp(n);
  };
  const act = async (fn: () => Promise<unknown>) => {
    setErr("");
    try { await fn(); st.reload(); } catch (e) { setErr(errorMessage(e)); }
  };
  return (
    <div>
      <div className="toolbar">
        <Link to={`${base}/labels`} className="btn btn-sm">Labels</Link>
        <Link to={`${base}/milestones`} className="btn btn-sm selected">Milestones</Link>
        <span className="spacer" />
        {canPush && <Link to={`${base}/milestones/new`} className="btn btn-primary" data-testid="new-milestone">New milestone</Link>}
      </div>
      {err && <ErrorBox error={err} />}
      <div className="box">
        <div className="box-header list-tabs pl-list-head">
          <div role="tablist" aria-label="Milestone state" className="pl-state-tabs">
            <button type="button" role="tab" aria-selected={state === "open"} className={state === "open" ? "selected" : ""} onClick={() => set("state", null)}>
              {counts.data ? `${counts.data.open} ` : ""}Open
            </button>
            <button type="button" role="tab" aria-selected={state === "closed"} className={state === "closed" ? "selected" : ""} onClick={() => set("state", "closed")}>
              ✓ {counts.data ? `${counts.data.closed} ` : ""}Closed
            </button>
          </div>
          <span className="spacer" />
          <label className="sr-only" htmlFor="ms-sort">Sort</label>
          <select id="ms-sort" className="input input-sm input-auto" value={sort} onChange={(e) => set("sort", e.target.value)}>
            <option value="due_on-asc">Closest due date</option>
            <option value="due_on-desc">Furthest due date</option>
            <option value="completeness-desc">Most complete</option>
            <option value="completeness-asc">Least complete</option>
          </select>
        </div>
        <Async state={st}>
          {(list) => list.length === 0 ? (
            <Blankslate title={state === "open" ? "You haven't created any Milestones." : "No closed milestones."}>
              Use Milestones to create collections of Issues and Pull Requests for a particular release or project.
            </Blankslate>
          ) : (
            <ul className="list">
              {list.map((m) => {
                const total = m.open_issues + m.closed_issues;
                const pct = total ? Math.round((m.closed_issues / total) * 100) : 0;
                return (
                  <li key={m.id} className="list-row pl-ms-row" data-testid="milestone-row" data-title={m.title}>
                    <div className="pl-ms-main">
                      <Link to={`${base}/milestone/${m.number}`} className="pl-ms-title">{m.title}</Link>
                      <div className="muted small">{dueText(m)} · Last updated <RelTime iso={m.updated_at} /></div>
                      {m.description && <div className="pl-ms-desc small"><Markdown source={m.description} comment repoPath={repo.full_name} /></div>}
                    </div>
                    <div className="pl-ms-side">
                      <Progress done={m.closed_issues} total={total} />
                      <div className="small pl-ms-stats">
                        <strong>{pct}%</strong> complete · <strong>{m.open_issues}</strong> open · <strong>{m.closed_issues}</strong> closed
                      </div>
                      {canPush && (
                        <div className="small pl-ms-actions">
                          <Link to={`${base}/milestones/${m.number}/edit`}>Edit</Link>
                          <button type="button" className="btn-link" onClick={() => act(() => papi.updateMilestone(owner, name, m.number, { state: m.state === "open" ? "closed" : "open" }))}>
                            {m.state === "open" ? "Close" : "Reopen"}
                          </button>
                          <button type="button" className="btn-link c-danger" onClick={() => {
                            if (confirmAction("Are you sure? Deleting a milestone removes it from all issues and pull requests.")) act(() => papi.deleteMilestone(owner, name, m.number));
                          }}>Delete</button>
                        </div>
                      )}
                    </div>
                  </li>
                );
              })}
            </ul>
          )}
        </Async>
      </div>
    </div>
  );
}

function MilestoneForm({ initial, onSubmit, submitText, extra }: {
  initial?: Milestone;
  onSubmit: (b: { title: string; description: string; due_on: string | null }) => Promise<void>;
  submitText: string;
  extra?: React.ReactNode;
}) {
  const [title, setTitle] = useState(initial?.title || "");
  const [due, setDue] = useState(initial?.due_on ? initial.due_on.slice(0, 10) : "");
  const [description, setDescription] = useState(initial?.description || "");
  const [err, setErr] = useState<unknown>();
  const [busy, setBusy] = useState(false);
  return (
    <form className="narrow-form" onSubmit={async (e) => {
      e.preventDefault();
      setBusy(true);
      setErr(undefined);
      try {
        await onSubmit({ title: title.trim(), description, due_on: due ? `${due}T00:00:00Z` : null });
      } catch (ex) {
        setErr(ex);
        setBusy(false);
      }
    }}>
      {err !== undefined && <ErrorBox error={err} />}
      <div className="form-group">
        <label htmlFor="ms-title">Title</label>
        <input id="ms-title" className="input" required value={title} onChange={(e) => setTitle(e.target.value)} placeholder="Title" />
      </div>
      <div className="form-group">
        <label htmlFor="ms-due">Due date (optional)</label>
        <input id="ms-due" type="date" className="input input-auto" value={due} onChange={(e) => setDue(e.target.value)} />
      </div>
      <div className="form-group">
        <span className="label-text">Description</span>
        <MarkdownEditor value={description} onChange={setDescription} label="Description" placeholder="Description" rows={6} />
      </div>
      <div className="form-actions">
        {extra}
        <button type="submit" className="btn btn-primary" disabled={busy || !title.trim()}>{busy ? "Saving…" : submitText}</button>
      </div>
    </form>
  );
}

export function NewMilestonePage() {
  const { repo, owner, name, base, canPush } = useRepo();
  const nav = useNavigate();
  useTitle(`New milestone · ${repo.full_name}`);
  if (!canPush) return <Navigate to={`${base}/milestones`} replace />;
  return (
    <div>
      <h2 className="h3">New milestone</h2>
      <p className="muted">Create a new milestone to help organize your issues and pull requests.</p>
      <MilestoneForm submitText="Create milestone" onSubmit={async (b) => {
        await papi.createMilestone(owner, name, b);
        nav(`${base}/milestones`);
      }} />
    </div>
  );
}

export function EditMilestonePage() {
  const { repo, owner, name, base, canPush } = useRepo();
  const n = Number(useParams().number);
  const nav = useNavigate();
  const st = useAsync(() => papi.milestone(owner, name, n), [owner, name, n]);
  useTitle(`Edit milestone · ${repo.full_name}`);
  if (!canPush) return <Navigate to={`${base}/milestone/${n}`} replace />;
  return (
    <Async state={st}>
      {(m) => (
        <div>
          <h2 className="h3">Edit milestone</h2>
          <MilestoneForm initial={m} submitText="Save changes" onSubmit={async (b) => {
            await papi.updateMilestone(owner, name, n, b);
            nav(`${base}/milestones`);
          }} extra={<>
            <Link to={`${base}/milestones`} className="btn">Cancel</Link>
            <button type="button" className="btn" onClick={async () => {
              await papi.updateMilestone(owner, name, n, { state: m.state === "open" ? "closed" : "open" });
              nav(`${base}/milestones${m.state === "open" ? "?state=closed" : ""}`);
            }}>{m.state === "open" ? "Close milestone" : "Reopen milestone"}</button>
          </>} />
        </div>
      )}
    </Async>
  );
}

export function MilestonePage() {
  const { repo, owner, name, base, canPush } = useRepo();
  const n = Number(useParams().number);
  const [sp, setSp] = useSearchParams();
  const state = sp.get("state") === "closed" ? "closed" : "open";
  const page = Number(sp.get("page") || 1);
  const st = useAsync(() => papi.milestone(owner, name, n), [owner, name, n]);
  const items = useAsync(() => papi.issues(owner, name, { milestone: String(n), state, page }), [owner, name, n, state, page]);
  useTitle(st.data ? `${st.data.title} Milestone · ${repo.full_name}` : "Milestone");
  return (
    <Async state={st}>
      {(m) => {
        const total = m.open_issues + m.closed_issues;
        return (
          <div>
            <div className="toolbar">
              <h2 className="h3 grow" data-testid="milestone-title">{m.title}</h2>
              {canPush && <Link to={`${base}/milestones/${n}/edit`} className="btn btn-sm">Edit milestone</Link>}
              <Link to={`${base}/issues/new`} className="btn btn-sm btn-primary">New issue</Link>
            </div>
            <div className="pl-ms-summary">
              <span className="muted small">{dueText(m)}</span>
              <Progress done={m.closed_issues} total={total} />
              <span className="small"><strong>{total ? Math.round((m.closed_issues / total) * 100) : 0}%</strong> complete</span>
            </div>
            {m.description && <div className="markdown-wrap"><Markdown source={m.description} comment repoPath={repo.full_name} /></div>}
            <div className="box">
              <div className="box-header list-tabs" role="tablist" aria-label="State">
                <button type="button" role="tab" aria-selected={state === "open"} className={state === "open" ? "selected" : ""} onClick={() => setSp({})}>
                  <IssueOpenIcon /> {m.open_issues} Open
                </button>
                <button type="button" role="tab" aria-selected={state === "closed"} className={state === "closed" ? "selected" : ""} onClick={() => setSp({ state: "closed" })}>
                  <IssueClosedIcon /> {m.closed_issues} Closed
                </button>
              </div>
              <Async state={items}>
                {(p) => p.items.length === 0 ? <Blankslate title="No issues in this milestone">Add issues to this milestone to see them here.</Blankslate> : (
                  <ul className="list">{p.items.map((i) => <IssueRow key={i.id} item={i} base={base} kind={i.pull_request ? "pull" : "issues"} />)}</ul>
                )}
              </Async>
            </div>
            {items.data && <Pagination page={page} next={items.data.next} prev={items.data.prev} last={items.data.last}
              onPage={(x) => setSp({ ...(state === "closed" ? { state } : {}), page: String(x) })} />}
          </div>
        );
      }}
    </Async>
  );
}
