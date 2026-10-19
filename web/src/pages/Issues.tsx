import { useEffect, useMemo, useState, type FormEvent } from "react";
import { Link, Navigate, useLocation, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { errorMessage, type Issue, type Label, type User } from "../api";
import { useAuth } from "../auth";
import { Async, Avatar, Blankslate, ErrorBox, Flash, ItemIcon, LabelPill, Pagination, RelTime, Spinner, StateBadge, UserLink } from "../components/common";
import { CommentBox, CommentForm, LabelsSidebar, MarkdownEditor } from "../components/Conversation";
import { CommentIcon, IssueClosedIcon, IssueOpenIcon, LockIcon } from "../components/icons";
import { useRepo } from "../components/RepoLayout";
import { useAsync, useTitle } from "../hooks";
import { Slot } from "../areas";
import { gql, papi, type FullComment, type FullIssue, type Milestone, type TimelineEvent } from "../areas/planning/api";
import { CommentBody } from "../areas/planning/Suggestions";
import {
  AssigneesSection, DeleteIssueControl, LockControl, MilestoneSection, PinControl, ProjectsSection, RelationshipsSection,
  SubIssuesBox, TransferControl,
} from "../areas/planning/Sidebar";
import { eventTime, shownEvents, TimelineEventRow } from "../areas/planning/Timeline";
import { confirmAction, KebabMenu, Popover, ReactionBar, SelectMenu, userOption } from "../areas/planning/ui";

export function IssueRow({ item, base, kind, showRepo }: { item: Issue; base: string; kind: "issues" | "pull"; showRepo?: boolean }) {
  const href = `${base}/${kind}/${item.number}`;
  const full = item as FullIssue;
  const repoName = item.repository_url?.split("/repos/")[1] || "";
  return (
    <li className="list-row issue-row" data-testid="issue-row">
      <ItemIcon item={item} />
      <div className="issue-row-main">
        {showRepo && <Link to={`/${repoName}`} className="muted-link pl-row-repo">{repoName}</Link>}{" "}
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
          {full.milestone && (
            <> · <Link to={`${base}/milestone/${full.milestone.number}`} className="muted-link">⚑ {full.milestone.title}</Link></>
          )}
        </div>
      </div>
      {!!item.assignees?.length && (
        <span className="pl-row-assignees" aria-label="Assignees">
          {item.assignees.slice(0, 3).map((a) => <Link key={a.login} to={`/${a.login}`} title={`Assigned to ${a.login}`}><Avatar user={a} size={20} /></Link>)}
        </span>
      )}
      {item.comments > 0 && (
        <Link to={href} className="muted-link small issue-comments" aria-label={`${item.comments} comments`}>
          <CommentIcon /> {item.comments}
        </Link>
      )}
    </li>
  );
}

/** Filter dropdown in the issue list header (Author / Label / Milestone / Assignee / Sort). */
function FilterMenu({ label, title, options, selected, onPick, multi, loadOptions }: {
  label: string;
  title: string;
  options?: { key: string; label: React.ReactNode; text: string }[];
  selected: string[];
  onPick: (keys: string[]) => void;
  multi?: boolean;
  loadOptions?: () => Promise<{ key: string; label: React.ReactNode; text: string }[]>;
}) {
  const [opts, setOpts] = useState(options);
  const [loading, setLoading] = useState(false);
  return (
    <Popover label={`Filter by ${label.toLowerCase()}`} button={<>{label} ▾</>} className="btn-link pl-filter-btn" testid={`filter-${label.toLowerCase()}`}
      onOpen={async () => {
        if (loadOptions && !opts) {
          setLoading(true);
          try { setOpts(await loadOptions()); } finally { setLoading(false); }
        }
      }}>
      {(close) => (
        <SelectMenu title={title} options={opts || options || []} loading={loading} selected={selected} multi={multi}
          onApply={(k) => { onPick(k); close(); }} />
      )}
    </Popover>
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
  const milestone = sp.get("milestone") || undefined;
  const assignee = sp.get("assignee") || undefined;
  const creator = sp.get("creator") || undefined;
  const sort = sp.get("sort") || "created";
  const q = sp.get("q") || "";
  const [qInput, setQInput] = useState(q);
  useEffect(() => setQInput(q), [q]);
  useTitle(`Issues · ${repo.full_name}`);
  const st = useAsync(async () => {
    if (q.trim()) {
      const r = await papi.search(`repo:${repo.full_name} is:issue is:${state} ${q}`, page);
      return { ...r, items: r.items.items, total: r.items.total_count };
    }
    const [field, direction] = sort.split("-");
    const r = await papi.issues(owner, name, { state, page, labels, milestone, assignee, creator, sort: field, direction: direction || "desc" });
    return { ...r, total: undefined as number | undefined };
  }, [owner, name, state, page, labels, milestone, assignee, creator, sort, q]);
  const counts = useAsync(async () => {
    const qs = (s: string) => `repo:${repo.full_name} is:issue is:${s}`;
    const [o, c] = await Promise.all([papi.search(qs("open")), papi.search(qs("closed"))]);
    return { open: o.items.total_count, closed: c.items.total_count };
  }, [repo.full_name]);
  const setParam = (k: string, v: string | null) => {
    const n = new URLSearchParams(sp);
    if (v === null || v === "") n.delete(k); else n.set(k, v);
    if (k !== "page") n.delete("page");
    setSp(n);
  };
  const people = async () => (await papi.assignees(owner, name)).map(userOption);
  const filtered = labels || milestone || assignee || creator || q;
  return (
    <div>
      <div className="toolbar pl-issues-toolbar">
        <form className="pl-search" role="search" onSubmit={(e) => { e.preventDefault(); setParam("q", qInput.trim() || null); }}>
          <label htmlFor="issue-search" className="sr-only">Search all issues</label>
          <input id="issue-search" className="input" placeholder="Search all issues" value={qInput} onChange={(e) => setQInput(e.target.value)} />
        </form>
        <Link to={`${base}/labels`} className="btn" data-testid="labels-link">Labels</Link>
        <Link to={`${base}/milestones`} className="btn" data-testid="milestones-link">Milestones</Link>
        {user && <Link to={`${base}/issues/new/choose`} className="btn btn-primary">New issue</Link>}
      </div>
      {filtered && (
        <p className="filter-note">
          {labels && <>Label <strong>{labels}</strong> · </>}
          {milestone && <>Milestone <strong>{milestone === "none" ? "none" : `#${milestone}`}</strong> · </>}
          {assignee && <>Assignee <strong>{assignee}</strong> · </>}
          {creator && <>Author <strong>{creator}</strong> · </>}
          {q && <>Matching <strong>{q}</strong> · </>}
          <button type="button" className="btn-link" onClick={() => setSp(new URLSearchParams(state === "closed" ? { state } : {}))}>Clear current search query, filters, and sorts</button>
        </p>
      )}
      <div className="box">
        <div className="box-header list-tabs pl-list-head">
          <div role="tablist" aria-label="Issue state" className="pl-state-tabs">
            <button type="button" role="tab" aria-selected={state === "open"} className={state === "open" ? "selected" : ""}
              onClick={() => setParam("state", null)}>
              <IssueOpenIcon /> {counts.data ? `${counts.data.open} ` : ""}Open
            </button>
            <button type="button" role="tab" aria-selected={state === "closed"} className={state === "closed" ? "selected" : ""}
              onClick={() => setParam("state", "closed")}>
              <IssueClosedIcon /> {counts.data ? `${counts.data.closed} ` : ""}Closed
            </button>
          </div>
          <span className="spacer" />
          <div className="pl-filters">
            <FilterMenu label="Author" title="Filter by author" selected={creator ? [creator] : []} loadOptions={people}
              onPick={(k) => setParam("creator", k[0] || null)} />
            <FilterMenu label="Labels" title="Filter by label" selected={labels ? labels.split(",") : []}
              loadOptions={async () => (await papi.labels(owner, name)).map((l) => ({ key: l.name, text: l.name, label: <LabelPill label={l} /> }))}
              onPick={(k) => setParam("labels", k.join(",") || null)} />
            <FilterMenu label="Milestones" title="Filter by milestone" selected={milestone ? [milestone] : []}
              loadOptions={async () => [{ key: "none", text: "Issues with no milestone", label: <em>Issues with no milestone</em> },
                ...(await papi.milestones(owner, name, "open")).map((m) => ({ key: String(m.number), text: m.title, label: m.title }))]}
              onPick={(k) => setParam("milestone", k[0] || null)} />
            <FilterMenu label="Assignee" title="Filter by who's assigned" selected={assignee ? [assignee] : []}
              loadOptions={async () => [{ key: "none", text: "Assigned to nobody", label: <em>Assigned to nobody</em> }, ...(await people())]}
              onPick={(k) => setParam("assignee", k[0] || null)} />
            <FilterMenu label="Sort" title="Sort by" selected={[sort]}
              options={[
                { key: "created", text: "Newest", label: "Newest" },
                { key: "created-asc", text: "Oldest", label: "Oldest" },
                { key: "comments", text: "Most commented", label: "Most commented" },
                { key: "comments-asc", text: "Least commented", label: "Least commented" },
                { key: "updated", text: "Recently updated", label: "Recently updated" },
                { key: "updated-asc", text: "Least recently updated", label: "Least recently updated" },
              ]}
              onPick={(k) => setParam("sort", k[0] && k[0] !== "created" ? k[0] : null)} />
          </div>
        </div>
        <Async state={st}>
          {(p) => {
            // The issues endpoint also returns pull requests; hide them here like github.com does.
            const items = p.items.filter((i) => !i.pull_request);
            return items.length === 0 ? (
              <Blankslate title={filtered ? "No results matched your search." : state === "open" ? "No open issues" : "No closed issues"} icon={<IssueOpenIcon size={24} />}>
                {state === "open" && user && !filtered ? <Link to={`${base}/issues/new`}>Open a new issue</Link> : null}
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

interface IssueTemplate { name: string; title: string | null; body: string | null; about: string | null; filename: string | null }

function useTemplates(owner: string, name: string) {
  return useAsync(async () => {
    const d = await gql<{ repository: { issueTemplates: IssueTemplate[] | null } | null }>(
      `query($o:String!,$r:String!){repository(owner:$o,name:$r,followRenames:true){issueTemplates{name title body about filename}}}`, { o: owner, r: name });
    return d.repository?.issueTemplates || [];
  }, [owner, name]);
}

/** `/{o}/{r}/issues/new/choose` — issue template chooser (redirects when there are none). */
export function ChooseIssuePage() {
  const { repo, owner, name, base } = useRepo();
  const tpl = useTemplates(owner, name);
  useTitle(`New issue · ${repo.full_name}`);
  return (
    <Async state={tpl}>
      {(list) => list.length === 0 ? <Navigate to={`${base}/issues/new`} replace /> : (
        <div className="narrow-form">
          <h2 className="h3">Create new issue in {repo.full_name}</h2>
          <div className="box">
            <ul className="list">
              {list.map((t) => (
                <li key={t.filename || t.name} className="list-row pl-template-row">
                  <div className="grow">
                    <strong>{t.name}</strong>
                    {t.about && <div className="muted small">{t.about}</div>}
                  </div>
                  <Link className="btn btn-primary btn-sm" to={`${base}/issues/new?template=${encodeURIComponent(t.filename || t.name)}`}>Get started</Link>
                </li>
              ))}
              <li className="list-row">
                <div className="grow"><strong>Blank issue</strong><div className="muted small">Create a new issue from scratch</div></div>
                <Link className="btn btn-sm" to={`${base}/issues/new`} data-testid="blank-issue">Open a blank issue</Link>
              </li>
            </ul>
          </div>
        </div>
      )}
    </Async>
  );
}

/** `/{o}/{r}/issues/new` (optionally `?template=…&title=…&body=…&labels=a,b`) */
export function NewIssuePage() {
  const { repo, owner, name, base, canPush, repo: { full_name } } = useRepo();
  const { user, loading } = useAuth();
  const nav = useNavigate();
  const [sp] = useSearchParams();
  const tpl = useTemplates(owner, name);
  const [title, setTitle] = useState(sp.get("title") || "");
  const [body, setBody] = useState(sp.get("body") || "");
  const [labels, setLabels] = useState<string[]>(sp.get("labels") ? sp.get("labels")!.split(",") : []);
  const [assignees, setAssignees] = useState<string[]>([]);
  const [milestone, setMilestone] = useState<Milestone | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>();
  const tname = sp.get("template");
  const template = tname ? (tpl.data || []).find((t) => t.filename === tname || t.name === tname) : undefined;
  useEffect(() => {
    if (template) {
      if (!title && template.title) setTitle(template.title);
      if (!body && template.body) setBody(template.body);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [template]);
  useTitle(`New issue · ${repo.full_name}`);
  if (!loading && !user) return <Navigate to={`/login?return_to=${encodeURIComponent(`${base}/issues/new`)}`} replace />;
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(undefined);
    try {
      const issue = await papi.createIssue(owner, name, {
        title: title.trim(), body,
        ...(canPush ? { labels, assignees, milestone: milestone?.number ?? undefined } : {}),
      });
      nav(`${base}/issues/${issue.number}`);
    } catch (err) {
      setError(err);
      setBusy(false);
    }
  };
  return (
    <form className="layout-sidebar" onSubmit={submit}>
      <div className="layout-main">
        <h2 className="h3">New issue{template ? `: ${template.name}` : ""}</h2>
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
      </div>
      {canPush && (
        <aside className="layout-aside">
          <NewIssueMeta owner={owner} name={name} labels={labels} setLabels={setLabels} assignees={assignees}
            setAssignees={setAssignees} milestone={milestone} setMilestone={setMilestone} />
        </aside>
      )}
    </form>
  );
}

function NewIssueMeta({ owner, name, labels, setLabels, assignees, setAssignees, milestone, setMilestone }: {
  owner: string; name: string; labels: string[]; setLabels: (l: string[]) => void; assignees: string[];
  setAssignees: (a: string[]) => void; milestone: Milestone | null; setMilestone: (m: Milestone | null) => void;
}) {
  const allLabels = useAsync(() => papi.labels(owner, name), [owner, name]);
  const people = useAsync(() => papi.assignees(owner, name), [owner, name]);
  const ms = useAsync(() => papi.milestones(owner, name, "open"), [owner, name]);
  return (
    <>
      <section className="sidebar-section">
        <div className="sidebar-heading"><h3 className="h6">Assignees</h3>
          <Popover label="Edit assignees" button={<span aria-hidden="true">⚙</span>} className="btn-link btn-icon" testid="new-assignees">
            {(close) => <SelectMenu title="Assign people" multi options={(people.data || []).map(userOption)} selected={assignees}
              onApply={(k) => { setAssignees(k); close(); }} />}
          </Popover>
        </div>
        {assignees.length ? assignees.map((a) => <div key={a}><UserLink user={{ login: a }} /></div>) : <p className="muted small">No one assigned</p>}
      </section>
      <section className="sidebar-section">
        <div className="sidebar-heading"><h3 className="h6">Labels</h3>
          <Popover label="Edit labels" button={<span aria-hidden="true">⚙</span>} className="btn-link btn-icon" testid="new-labels">
            {(close) => <SelectMenu title="Apply labels" multi selected={labels}
              options={(allLabels.data || []).map((l) => ({ key: l.name, text: l.name, label: <LabelPill label={l} /> }))}
              onApply={(k) => { setLabels(k); close(); }} />}
          </Popover>
        </div>
        {labels.length ? <div className="label-list">{labels.map((l) => {
          const full = (allLabels.data || []).find((x) => x.name === l);
          return <LabelPill key={l} label={full || { name: l, color: "ededed", description: null }} />;
        })}</div> : <p className="muted small">None yet</p>}
      </section>
      <section className="sidebar-section">
        <div className="sidebar-heading"><h3 className="h6">Milestone</h3>
          <Popover label="Edit milestone" button={<span aria-hidden="true">⚙</span>} className="btn-link btn-icon" testid="new-milestone">
            {(close) => <SelectMenu title="Set milestone" selected={milestone ? [String(milestone.number)] : []}
              options={(ms.data || []).map((m) => ({ key: String(m.number), text: m.title, label: m.title }))}
              onApply={(k) => { setMilestone((ms.data || []).find((m) => String(m.number) === k[0]) || null); close(); }} />}
          </Popover>
        </div>
        {milestone ? <strong>{milestone.title}</strong> : <p className="muted small">No milestone</p>}
      </section>
    </>
  );
}

export function EditableTitle({ title, number, canEdit, onSave, extra }: {
  title: string; number: number; canEdit: boolean; onSave: (t: string) => Promise<void>; extra?: React.ReactNode;
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
      <div className="pl-title-actions">
        {canEdit && (
          <button type="button" className="btn btn-sm" onClick={() => { setVal(title); setEditing(true); }}>Edit</button>
        )}
        {extra}
      </div>
    </div>
  );
}

/** Close button with a reason menu (completed / not planned / duplicate). */
function CloseIssueButton({ body, onClose }: { body: string; onClose: (reason: string) => Promise<void> }) {
  const [reason, setReason] = useState("completed");
  const labels: Record<string, string> = { completed: "Close as completed", not_planned: "Close as not planned", duplicate: "Close as duplicate" };
  return (
    <span className="btn-group pl-close-group">
      <button type="button" className="btn" data-testid="close-issue" onClick={() => onClose(reason)}>
        <IssueClosedIcon className={"icon " + (reason === "completed" ? "c-done" : "muted")} />{" "}
        {body.trim() ? labels[reason].replace("Close", "Close with comment") : reason === "completed" ? "Close issue" : labels[reason]}
      </button>
      <Popover label="Choose a close reason" button={<span aria-hidden="true">▾</span>} className="btn" testid="close-reason">
        {(close) => (
          <div className="pl-menu" role="menu">
            {Object.entries(labels).map(([k, v]) => (
              <button key={k} type="button" role="menuitemradio" aria-checked={reason === k} className="menu-item"
                onClick={() => { setReason(k); close(); }}>
                {reason === k ? "✓ " : ""}{v}
              </button>
            ))}
          </div>
        )}
      </Popover>
    </span>
  );
}

type ConvEntry = { at: string; c?: FullComment; e?: TimelineEvent };

/** `/{o}/{r}/issues/{n}` */
export function IssuePage() {
  const repoCtx = useRepo();
  const { repo, owner, name, base, canPush, isAdmin } = repoCtx;
  const { user } = useAuth();
  const loc = useLocation();
  const n = Number(useParams().number);
  const st = useAsync(() => papi.issue(owner, name, n), [owner, name, n]);
  const comments = useAsync(() => papi.comments(owner, name, n), [owner, name, n]);
  const timeline = useAsync(() => papi.timeline(owner, name, n), [owner, name, n]);
  const [actionErr, setActionErr] = useState<unknown>();
  const [quote, setQuote] = useState("");
  useTitle(st.data ? `${st.data.title} · Issue #${n} · ${repo.full_name}` : `Issue #${n}`);
  const entries = useMemo<ConvEntry[]>(() => [
    ...(comments.data || []).map((c) => ({ at: c.created_at, c })),
    ...shownEvents(timeline.data || []).map((e) => ({ at: eventTime(e), e })),
  ].sort((a, b) => a.at.localeCompare(b.at)), [comments.data, timeline.data]);
  useEffect(() => {
    if (loc.hash && comments.data) document.getElementById(loc.hash.slice(1))?.scrollIntoView();
  }, [loc.hash, comments.data]);

  return (
    <Async state={st}>
      {(issue) => {
        if (issue.pull_request && issue.repository_url.toLowerCase().endsWith(`/repos/${owner}/${name}`.toLowerCase())) return <Navigate to={`${base}/pull/${n}`} replace />;
        const canEdit = !!user && (canPush || user.login === issue.user.login);
        const locked = !!issue.locked;
        const refresh = async () => {
          st.setData(await papi.issue(owner, name, n));
          timeline.reload();
        };
        const setState = async (state: "open" | "closed", reason?: string) => {
          setActionErr(undefined);
          try {
            st.setData(await papi.updateIssue(owner, name, n, { state, state_reason: reason }));
            timeline.reload();
          } catch (e) {
            setActionErr(e);
          }
        };
        const update = async (b: Record<string, unknown>) => {
          st.setData(await papi.updateIssue(owner, name, n, b));
          timeline.reload();
        };
        const commentMenu = (c: FullComment) => (startEdit: () => void) => {
          const mine = user?.login === c.user.login;
          return (
            <KebabMenu label="Comment actions" items={[
              { label: "Copy link", onClick: () => navigator.clipboard?.writeText(`${location.origin}${base}/issues/${n}#issuecomment-${c.id}`).catch(() => {}) },
              { label: "Quote reply", hidden: !user, onClick: () => setQuote(c.body.split("\n").map((l) => `> ${l}`).join("\n") + "\n\n") },
              { label: "Edit", hidden: !(mine || canPush), onClick: startEdit },
              { label: c.pin ? "Unpin comment" : "Pin comment", hidden: !canPush, onClick: async () => {
                try {
                  if (c.pin) await papi.unpinComment(owner, name, c.id); else await papi.pinComment(owner, name, c.id);
                  comments.reload();
                } catch (e) { setActionErr(e); }
              } },
              { label: "Delete", danger: true, hidden: !(mine || canPush), onClick: async () => {
                if (!confirmAction("Are you sure you want to delete this comment?")) return;
                try {
                  await papi.deleteComment(owner, name, c.id);
                  comments.setData((comments.data || []).filter((x) => x.id !== c.id));
                  st.setData({ ...issue, comments: Math.max(0, issue.comments - 1) });
                } catch (e) { setActionErr(e); }
              } },
            ]} />
          );
        };
        const pinnedComment = (comments.data || []).find((c) => c.pin);
        return (
          <div>
            <EditableTitle title={issue.title} number={issue.number} canEdit={canEdit}
              onSave={(t) => update({ title: t })}
              extra={user && <Link to={`${base}/issues/new/choose`} className="btn btn-sm btn-primary">New issue</Link>} />
            <div className="issue-meta">
              <StateBadge item={issue} />
              {locked && <span className="badge" data-testid="locked-badge"><LockIcon /> Locked</span>}
              <span className="muted">
                <UserLink user={issue.user} /> opened this issue <RelTime iso={issue.created_at} /> ·{" "}
                {issue.comments} comment{issue.comments === 1 ? "" : "s"}
              </span>
            </div>
            <div className="layout-sidebar">
              <div className="layout-main timeline">
                {pinnedComment && (
                  <a href={`#issuecomment-${pinnedComment.id}`} className="pl-pinned-note small" data-testid="pinned-comment-note">
                    📌 Pinned comment by {pinnedComment.user.login}
                  </a>
                )}
                <CommentBox user={issue.user} body={issue.body} created={issue.created_at} verb="opened"
                  association={issue.author_association} repoPath={repo.full_name} id="issue-body"
                  onSave={canEdit ? (b) => update({ body: b }) : undefined}
                  renderBody={(b) => <CommentBody body={b} repoPath={repo.full_name} />}
                  menu={(startEdit) => <KebabMenu label="Issue body actions" items={[
                    { label: "Copy link", onClick: () => navigator.clipboard?.writeText(`${location.origin}${base}/issues/${n}`).catch(() => {}) },
                    { label: "Quote reply", hidden: !user, onClick: () => setQuote((issue.body || "").split("\n").map((l) => `> ${l}`).join("\n") + "\n\n") },
                    { label: "Edit", hidden: !canEdit, onClick: startEdit },
                  ]} />}
                  footer={<ReactionBar owner={owner} name={name} subject={`issues/${n}`} summary={issue.reactions} readOnly={locked && !canPush} />} />
                <SubIssuesBox owner={owner} name={name} issue={issue} canEdit={canPush} />
                {comments.error ? <ErrorBox error={comments.error} onRetry={comments.reload} /> :
                  timeline.error ? <ErrorBox error={timeline.error} onRetry={timeline.reload} /> :
                  comments.loading && !comments.data ? <Spinner /> :
                  entries.map((en) => en.c ? (
                    <CommentBox key={`c${en.c.id}`} id={`issuecomment-${en.c.id}`} user={en.c.user} body={en.c.body} created={en.c.created_at}
                      association={en.c.author_association} repoPath={repo.full_name} className={en.c.pin ? "pl-pinned" : undefined}
                      header={en.c.pin ? <span className="badge">📌 Pinned</span> : undefined}
                      renderBody={(b) => <CommentBody body={b} repoPath={repo.full_name} />}
                      onSave={async (b) => {
                        const u = await papi.updateComment(owner, name, en.c!.id, b);
                        comments.setData((comments.data || []).map((x) => (x.id === u.id ? { ...x, ...u } : x)));
                      }}
                      menu={commentMenu(en.c)}
                      footer={<ReactionBar owner={owner} name={name} subject={`issues/comments/${en.c.id}`} summary={en.c.reactions} readOnly={locked && !canPush} />} />
                  ) : en.e ? <TimelineEventRow key={`e${en.e.id ?? en.at}${en.e.event}`} e={en.e} base={base} /> : null)}
                {issue.state === "closed" && (
                  <div className="timeline-event muted">
                    <IssueClosedIcon className="icon c-done" /> This issue was closed <RelTime iso={issue.closed_at} />
                    {issue.state_reason ? ` as ${issue.state_reason.replace(/_/g, " ")}` : ""}.
                  </div>
                )}
                {actionErr !== undefined && <ErrorBox error={actionErr} />}
                {locked && (
                  <Flash kind="warn">
                    <LockIcon /> This conversation has been locked{issue.active_lock_reason ? <> as <strong>{issue.active_lock_reason}</strong></> : null} and limited to collaborators.
                  </Flash>
                )}
                {(!locked || canPush) && (
                  <CommentForm repoPath={repo.full_name} key={quote}
                    initial={quote}
                    onSubmit={async (body) => {
                      const c = await papi.addComment(owner, name, n, body);
                      comments.setData([...(comments.data || []), c]);
                      st.setData({ ...issue, comments: issue.comments + 1 });
                      setQuote("");
                    }}
                    extra={(body, reset) =>
                      canEdit ? (
                        issue.state === "open" ? (
                          <CloseIssueButton body={body} onClose={async (reason) => {
                            if (body.trim()) {
                              const c = await papi.addComment(owner, name, n, body);
                              comments.setData([...(comments.data || []), c]);
                              reset();
                            }
                            await setState("closed", reason);
                          }} />
                        ) : (
                          <button type="button" className="btn" data-testid="reopen-issue" onClick={() => setState("open", "reopened")}>
                            Reopen issue
                          </button>
                        )
                      ) : null
                    }
                  />
                )}
              </div>
              <aside className="layout-aside">
                <AssigneesSection owner={owner} name={name} number={n} assignees={issue.assignees || []} canEdit={canPush}
                  onChange={(a: User[]) => { st.setData({ ...issue, assignees: a }); timeline.reload(); }} />
                <LabelsSidebar owner={owner} name={name} number={n} labels={issue.labels} canEdit={canPush}
                  onChange={(labels: Label[]) => { st.setData({ ...issue, labels }); timeline.reload(); }} />
                {issue.node_id && <ProjectsSection nodeId={issue.node_id} owner={owner} canEdit={canPush} />}
                <MilestoneSection owner={owner} name={name} milestone={issue.milestone} canEdit={canPush}
                  onSet={(m) => update({ milestone: m })} />
                <RelationshipsSection owner={owner} name={name} issue={issue} canEdit={canPush} />
                <Slot name="issue.sidebar" ctx={repoCtx} issue={issue} setIssue={(i: Issue) => st.setData(i)} />
                {canPush && (
                  <section className="sidebar-section pl-side-actions">
                    <LockControl owner={owner} name={name} number={n} locked={locked} kind="issue" onChange={refresh} />
                    {issue.node_id && <PinControl nodeId={issue.node_id} />}
                    <TransferControl issue={issue} repo={repo} />
                    {isAdmin && <DeleteIssueControl issue={issue} base={base} />}
                  </section>
                )}
              </aside>
            </div>
          </div>
        );
      }}
    </Async>
  );
}
