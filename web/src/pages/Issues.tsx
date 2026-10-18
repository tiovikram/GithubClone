import { useState, type FormEvent } from "react";
import { Link, Navigate, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { api, errorMessage, type Issue, type Label } from "../api";
import { useAuth } from "../auth";
import { Async, Blankslate, ErrorBox, ItemIcon, LabelPill, Pagination, RelTime, StateBadge, UserLink } from "../components/common";
import { CommentBox, CommentForm, LabelsSidebar, MarkdownEditor, useComments } from "../components/Conversation";
import { CommentIcon, IssueClosedIcon, IssueOpenIcon } from "../components/icons";
import { useRepo } from "../components/RepoLayout";
import { useAsync, useTitle } from "../hooks";
import { Slot } from "../areas";

export function IssueRow({ item, base, kind }: { item: Issue; base: string; kind: "issues" | "pull" }) {
  const href = `${base}/${kind}/${item.number}`;
  return (
    <li className="list-row issue-row">
      <ItemIcon item={item} />
      <div className="issue-row-main">
        <Link to={href} className="issue-title">{item.title}</Link>{" "}
        {item.labels.map((l) => (
          <Link key={l.id} to={`${base}/issues?labels=${encodeURIComponent(l.name)}`} className="label-link">
            <LabelPill label={l} />
          </Link>
        ))}
        <div className="muted small">
          #{item.number}{" "}
          {item.state === "open" ? (
            <>opened <RelTime iso={item.created_at} /> by <UserLink user={item.user} /></>
          ) : (
            <>by <UserLink user={item.user} /> was closed <RelTime iso={item.closed_at || item.updated_at} /></>
          )}
        </div>
      </div>
      {item.comments > 0 && (
        <Link to={href} className="muted-link small issue-comments" aria-label={`${item.comments} comments`}>
          <CommentIcon /> {item.comments}
        </Link>
      )}
    </li>
  );
}

/** `/{o}/{r}/issues` */
export function IssuesPage() {
  const { repo, owner, name, base } = useRepo();
  const { user } = useAuth();
  const [sp, setSp] = useSearchParams();
  const state = sp.get("state") === "closed" ? "closed" : "open";
  const page = Number(sp.get("page") || 1);
  const labels = sp.get("labels") || undefined;
  useTitle(`Issues · ${repo.full_name}`);
  const st = useAsync(() => api.issues(owner, name, state, page, labels), [owner, name, state, page, labels]);
  const setParam = (k: string, v: string | null) => {
    const n = new URLSearchParams(sp);
    if (v === null) n.delete(k); else n.set(k, v);
    if (k !== "page") n.delete("page");
    setSp(n);
  };
  return (
    <div>
      <div className="toolbar">
        <h2 className="h3">Issues</h2>
        <span className="spacer" />
        {user && <Link to={`${base}/issues/new`} className="btn btn-primary">New issue</Link>}
      </div>
      {labels && (
        <p className="filter-note">
          Filtered by label <strong>{labels}</strong> ·{" "}
          <button type="button" className="btn-link" onClick={() => setParam("labels", null)}>Clear filter</button>
        </p>
      )}
      <div className="box">
        <div className="box-header list-tabs" role="tablist" aria-label="Issue state">
          <button type="button" role="tab" aria-selected={state === "open"} className={state === "open" ? "selected" : ""}
            onClick={() => setParam("state", null)}>
            <IssueOpenIcon /> Open
          </button>
          <button type="button" role="tab" aria-selected={state === "closed"} className={state === "closed" ? "selected" : ""}
            onClick={() => setParam("state", "closed")}>
            <IssueClosedIcon /> Closed
          </button>
        </div>
        <Async state={st}>
          {(p) => {
            // The issues endpoint also returns pull requests; hide them here like github.com does.
            const items = p.items.filter((i) => !i.pull_request);
            return items.length === 0 ? (
              <Blankslate title={state === "open" ? "No open issues" : "No closed issues"} icon={<IssueOpenIcon size={24} />}>
                {state === "open" && user ? <Link to={`${base}/issues/new`}>Open a new issue</Link> : null}
              </Blankslate>
            ) : (
              <ul className="list">{items.map((i) => <IssueRow key={i.id} item={i} base={base} kind="issues" />)}</ul>
            );
          }}
        </Async>
      </div>
      {st.data && (
        <Pagination page={page} next={st.data.next} prev={st.data.prev} last={st.data.last}
          onPage={(n) => setParam("page", String(n))} />
      )}
    </div>
  );
}

/** `/{o}/{r}/issues/new` */
export function NewIssuePage() {
  const { repo, owner, name, base, repo: { full_name } } = useRepo();
  const { user, loading } = useAuth();
  const nav = useNavigate();
  const [title, setTitle] = useState("");
  const [body, setBody] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>();
  useTitle(`New issue · ${repo.full_name}`);
  if (!loading && !user) return <Navigate to={`/login?return_to=${encodeURIComponent(`${base}/issues/new`)}`} replace />;
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(undefined);
    try {
      const issue = await api.createIssue(owner, name, { title: title.trim(), body });
      nav(`${base}/issues/${issue.number}`);
    } catch (err) {
      setError(err);
      setBusy(false);
    }
  };
  return (
    <form className="narrow-form" onSubmit={submit}>
      <h2 className="h3">New issue</h2>
      {error !== undefined && <ErrorBox error={error} />}
      <div className="form-group">
        <label htmlFor="issue-title">Title</label>
        <input id="issue-title" className="input input-lg" required autoFocus value={title}
          onChange={(e) => setTitle(e.target.value)} placeholder="Title" />
      </div>
      <div className="form-group">
        <span className="label-text">Description</span>
        <MarkdownEditor value={body} onChange={setBody} label="Description" placeholder="Add a description" rows={12} repoPath={full_name} />
      </div>
      <div className="form-actions">
        <button type="submit" className="btn btn-primary" disabled={busy || !title.trim()}>
          {busy ? "Submitting…" : "Submit new issue"}
        </button>
      </div>
    </form>
  );
}

export function EditableTitle({ title, number, canEdit, onSave }: {
  title: string; number: number; canEdit: boolean; onSave: (t: string) => Promise<void>;
}) {
  const [editing, setEditing] = useState(false);
  const [val, setVal] = useState(title);
  const [err, setErr] = useState("");
  if (editing) {
    return (
      <form className="title-edit" onSubmit={async (e) => {
        e.preventDefault();
        try {
          await onSave(val.trim());
          setEditing(false);
        } catch (ex) {
          setErr(errorMessage(ex));
        }
      }}>
        <label htmlFor="title-edit" className="sr-only">Title</label>
        <input id="title-edit" className="input input-lg" value={val} onChange={(e) => setVal(e.target.value)} autoFocus required />
        <button type="submit" className="btn">Save</button>
        <button type="button" className="btn-link" onClick={() => setEditing(false)}>Cancel</button>
        {err && <span className="c-danger small" role="alert">{err}</span>}
      </form>
    );
  }
  return (
    <div className="title-row">
      <h1 className="issue-headline">
        {title} <span className="muted">#{number}</span>
      </h1>
      {canEdit && (
        <button type="button" className="btn btn-sm" onClick={() => { setVal(title); setEditing(true); }}>Edit</button>
      )}
    </div>
  );
}

/** `/{o}/{r}/issues/{n}` */
export function IssuePage() {
  const repoCtx = useRepo();
  const { repo, owner, name, base, canPush } = repoCtx;
  const { user } = useAuth();
  const n = Number(useParams().number);
  const st = useAsync(() => api.issue(owner, name, n), [owner, name, n]);
  const comments = useComments(owner, name, n);
  const [actionErr, setActionErr] = useState<unknown>();
  useTitle(st.data ? `${st.data.title} · Issue #${n} · ${repo.full_name}` : `Issue #${n}`);

  return (
    <Async state={st}>
      {(issue) => {
        if (issue.pull_request) return <Navigate to={`${base}/pull/${n}`} replace />;
        const canEdit = !!user && (canPush || user.login === issue.user.login);
        const setState = async (state: "open" | "closed", reason?: string) => {
          setActionErr(undefined);
          try {
            st.setData(await api.updateIssue(owner, name, n, { state, state_reason: reason }));
          } catch (e) {
            setActionErr(e);
          }
        };
        return (
          <div>
            <EditableTitle title={issue.title} number={issue.number} canEdit={canEdit}
              onSave={async (t) => st.setData(await api.updateIssue(owner, name, n, { title: t }))} />
            <div className="issue-meta">
              <StateBadge item={issue} />
              <span className="muted">
                <UserLink user={issue.user} /> opened this issue <RelTime iso={issue.created_at} /> ·{" "}
                {issue.comments} comment{issue.comments === 1 ? "" : "s"}
              </span>
            </div>
            <div className="layout-sidebar">
              <div className="layout-main timeline">
                <CommentBox user={issue.user} body={issue.body} created={issue.created_at}
                  association={issue.author_association} repoPath={repo.full_name} />
                {comments.loading && !comments.data ? null : comments.error ? (
                  <ErrorBox error={comments.error} onRetry={comments.reload} />
                ) : (
                  (comments.data || []).map((c) => (
                    <CommentBox key={c.id} user={c.user} body={c.body} created={c.created_at}
                      association={c.author_association} repoPath={repo.full_name} />
                  ))
                )}
                {issue.state === "closed" && (
                  <div className="timeline-event muted">
                    <IssueClosedIcon className="icon c-done" /> This issue was closed <RelTime iso={issue.closed_at} />
                    {issue.state_reason ? ` as ${issue.state_reason.replace(/_/g, " ")}` : ""}.
                  </div>
                )}
                {actionErr !== undefined && <ErrorBox error={actionErr} />}
                <CommentForm repoPath={repo.full_name}
                  onSubmit={async (body) => {
                    const c = await api.addComment(owner, name, n, body);
                    comments.setData([...(comments.data || []), c]);
                    st.setData({ ...issue, comments: issue.comments + 1 });
                  }}
                  extra={(body, reset) =>
                    canEdit ? (
                      issue.state === "open" ? (
                        <button type="button" className="btn" onClick={async () => {
                          if (body.trim()) {
                            const c = await api.addComment(owner, name, n, body);
                            comments.setData([...(comments.data || []), c]);
                            reset();
                          }
                          await setState("closed", "completed");
                        }}>
                          <IssueClosedIcon className="icon c-done" /> {body.trim() ? "Close with comment" : "Close issue"}
                        </button>
                      ) : (
                        <button type="button" className="btn" onClick={() => setState("open", "reopened")}>
                          Reopen issue
                        </button>
                      )
                    ) : null
                  }
                />
              </div>
              <aside className="layout-aside">
                <LabelsSidebar owner={owner} name={name} number={n} labels={issue.labels} canEdit={canPush}
                  onChange={(labels: Label[]) => st.setData({ ...issue, labels })} />
                <section className="sidebar-section">
                  <h3 className="h6">Assignees</h3>
                  {issue.assignees?.length ? issue.assignees.map((a) => <div key={a.id}><UserLink user={a} /></div>) :
                    <p className="muted small">No one assigned</p>}
                </section>
                <Slot name="issue.sidebar" ctx={repoCtx} issue={issue} setIssue={(i: Issue) => st.setData(i)} />
              </aside>
            </div>
          </div>
        );
      }}
    </Async>
  );
}
