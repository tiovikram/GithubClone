import { useEffect, useMemo, useState, type FormEvent } from "react";
import { Link, NavLink, Navigate, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { api, errorMessage, isNotFound, type Pull } from "../api";
import { useAuth } from "../auth";
import { BranchSelect } from "../components/BranchSelect";
import { CommitList } from "../components/CommitList";
import {
  Async, Avatar, Blankslate, ErrorBox, Flash, ItemIcon, LabelPill, Pagination, RelTime, Spinner, StateBadge, UserLink,
} from "../components/common";
import { CommentBox, CommentForm, LabelsSidebar, MarkdownEditor } from "../components/Conversation";
import { DiffView, type DiffReview, type LineCommentDraft } from "../components/Diff";
import { CheckIcon, CommentIcon, LockIcon, MergeIcon, PullClosedIcon, PullIcon } from "../components/icons";
import { useRepo } from "../components/RepoLayout";
import { useAsync, useTitle } from "../hooks";
import { firstLine } from "../util";
import { EditableTitle } from "./Issues";
import { Slot } from "../areas";
import { gql, papi, type FullComment, type FullIssue, type FullPull, type FullReview, type NewReviewComment, type TimelineEvent } from "../areas/planning/api";
import { ThreadView, useThreads, type Thread } from "../areas/planning/ReviewThreads";
import { AssigneesSection, LockControl, MilestoneSection, ProjectsSection, ReviewersSection } from "../areas/planning/Sidebar";
import { CommentBody } from "../areas/planning/Suggestions";
import { eventTime, shownEvents, TimelineEventRow } from "../areas/planning/Timeline";
import { confirmAction, Dialog, KebabMenu, Popover, ReactionBar, SelectMenu, userOption } from "../areas/planning/ui";

function PullRow({ pr, base }: { pr: Pull | FullIssue; base: string }) {
  const href = `${base}/pull/${pr.number}`;
  const head = "head" in pr ? pr.head : null;
  const p = pr as FullPull;
  const merged = "merged_at" in pr ? pr.merged_at : (pr as FullIssue).pull_request?.merged_at;
  return (
    <li className="list-row issue-row" data-testid="pull-row">
      <ItemIcon item={pr} />
      <div className="issue-row-main">
        <Link to={href} className="issue-title">{pr.title}</Link>{" "}
        {"draft" in pr && pr.draft && <span className="badge">Draft</span>}{" "}
        {pr.labels.map((l) => <LabelPill key={l.id} label={l} />)}
        <div className="muted small">
          #{pr.number}{" "}
          {merged ? (
            <>by <UserLink user={pr.user} /> was merged <RelTime iso={merged} /></>
          ) : pr.state === "open" ? (
            <>opened <RelTime iso={pr.created_at} /> by <UserLink user={pr.user} /></>
          ) : (
            <>by <UserLink user={pr.user} /> was closed <RelTime iso={pr.closed_at} /></>
          )}
          {head && <> · <span className="mono">{head.ref}</span> → <span className="mono">{(pr as Pull).base.ref}</span></>}
          {p.requested_reviewers && p.requested_reviewers.length > 0 && <> · Review required</>}
          {p.milestone && <> · <Link to={`${base}/milestone/${p.milestone.number}`} className="muted-link">⚑ {p.milestone.title}</Link></>}
        </div>
      </div>
      {!!p.assignees?.length && (
        <span className="pl-row-assignees" aria-label="Assignees">
          {p.assignees.slice(0, 3).map((a) => <Avatar key={a.login} user={a} size={20} />)}
        </span>
      )}
      {!!pr.comments && (
        <Link to={href} className="muted-link small issue-comments" aria-label={`${pr.comments} comments`}>
          <CommentIcon /> {pr.comments}
        </Link>
      )}
    </li>
  );
}

/** `/{o}/{r}/pulls` */
export function PullsPage() {
  const { repo, owner, name, base } = useRepo();
  const { user } = useAuth();
  const [sp, setSp] = useSearchParams();
  const state = sp.get("state") === "closed" ? "closed" : "open";
  const page = Number(sp.get("page") || 1);
  const author = sp.get("author") || "";
  const label = sp.get("label") || "";
  const review = sp.get("review") || "";
  const assignee = sp.get("assignee") || "";
  const q = sp.get("q") || "";
  const [qInput, setQInput] = useState(q);
  useEffect(() => setQInput(q), [q]);
  const filtered = !!(author || label || review || assignee || q);
  useTitle(`Pull requests · ${repo.full_name}`);
  const st = useAsync(async () => {
    if (filtered) {
      const parts = [`repo:${repo.full_name}`, "is:pr", `is:${state}`];
      if (author) parts.push(`author:${author}`);
      if (label) parts.push(`label:"${label}"`);
      if (assignee) parts.push(`assignee:${assignee}`);
      if (review === "requested") parts.push(`review-requested:${user?.login || "@me"}`);
      else if (review) parts.push(`review:${review}`);
      if (q) parts.push(q);
      const r = await papi.search(parts.join(" "), page);
      return { ...r, items: r.items.items as (Pull | FullIssue)[] };
    }
    const r = await api.pulls(owner, name, state, page);
    return { ...r, items: r.items as (Pull | FullIssue)[] };
  }, [owner, name, state, page, author, label, review, assignee, q]);
  const counts = useAsync(async () => {
    const qs = (s: string) => `repo:${repo.full_name} is:pr is:${s}`;
    const [o, c] = await Promise.all([papi.search(qs("open")), papi.search(qs("closed"))]);
    return { open: o.items.total_count, closed: c.items.total_count };
  }, [repo.full_name]);
  const setParam = (k: string, v: string | null) => {
    const n = new URLSearchParams(sp);
    if (v === null || v === "") n.delete(k); else n.set(k, v);
    if (k !== "page") n.delete("page");
    setSp(n);
  };
  const [people, setPeople] = useState<ReturnType<typeof userOption>[] | null>(null);
  const loadPeople = async () => { if (!people) setPeople((await papi.assignees(owner, name)).map(userOption)); };
  const [labels, setLabels] = useState<{ key: string; text: string; label: React.ReactNode }[] | null>(null);
  return (
    <div>
      <div className="toolbar pl-issues-toolbar">
        <form className="pl-search" role="search" onSubmit={(e) => { e.preventDefault(); setParam("q", qInput.trim() || null); }}>
          <label htmlFor="pr-search" className="sr-only">Search all pull requests</label>
          <input id="pr-search" className="input" placeholder="Search all pull requests" value={qInput} onChange={(e) => setQInput(e.target.value)} />
        </form>
        <Link to={`${base}/labels`} className="btn">Labels</Link>
        <Link to={`${base}/milestones`} className="btn">Milestones</Link>
        {user && <Link to={`${base}/compare`} className="btn btn-primary">New pull request</Link>}
      </div>
      {filtered && (
        <p className="filter-note">
          <button type="button" className="btn-link" onClick={() => setSp(new URLSearchParams(state === "closed" ? { state } : {}))}>Clear current search query, filters, and sorts</button>
        </p>
      )}
      <div className="box">
        <div className="box-header list-tabs pl-list-head">
          <div role="tablist" aria-label="Pull request state" className="pl-state-tabs">
            <button type="button" role="tab" aria-selected={state === "open"} className={state === "open" ? "selected" : ""}
              onClick={() => setParam("state", null)}><PullIcon /> {counts.data ? `${counts.data.open} ` : ""}Open</button>
            <button type="button" role="tab" aria-selected={state === "closed"} className={state === "closed" ? "selected" : ""}
              onClick={() => setParam("state", "closed")}><CheckIcon /> {counts.data ? `${counts.data.closed} ` : ""}Closed</button>
          </div>
          <span className="spacer" />
          <div className="pl-filters">
            <Popover label="Filter by author" button={<>Author ▾</>} className="btn-link pl-filter-btn" testid="filter-author" onOpen={loadPeople}>
              {(close) => <SelectMenu title="Filter by author" loading={!people} options={people || []} selected={author ? [author] : []}
                onApply={(k) => { setParam("author", k[0] || null); close(); }} />}
            </Popover>
            <Popover label="Filter by label" button={<>Label ▾</>} className="btn-link pl-filter-btn" testid="filter-label"
              onOpen={async () => { if (!labels) setLabels((await papi.labels(owner, name)).map((l) => ({ key: l.name, text: l.name, label: <LabelPill label={l} /> }))); }}>
              {(close) => <SelectMenu title="Filter by label" loading={!labels} options={labels || []} selected={label ? [label] : []}
                onApply={(k) => { setParam("label", k[0] || null); close(); }} />}
            </Popover>
            <Popover label="Filter by reviews" button={<>Reviews ▾</>} className="btn-link pl-filter-btn" testid="filter-reviews">
              {(close) => <SelectMenu title="Filter by reviews" selected={review ? [review] : []}
                options={[
                  { key: "none", text: "No reviews", label: "No reviews" },
                  { key: "required", text: "Review required", label: "Review required" },
                  { key: "approved", text: "Approved review", label: "Approved review" },
                  { key: "changes_requested", text: "Changes requested", label: "Changes requested" },
                  ...(user ? [{ key: "requested", text: "Review requested from you", label: "Awaiting review from you" }] : []),
                ]}
                onApply={(k) => { setParam("review", k[0] || null); close(); }} />}
            </Popover>
            <Popover label="Filter by assignee" button={<>Assignee ▾</>} className="btn-link pl-filter-btn" testid="filter-assignee" onOpen={loadPeople}>
              {(close) => <SelectMenu title="Filter by who's assigned" loading={!people} options={people || []} selected={assignee ? [assignee] : []}
                onApply={(k) => { setParam("assignee", k[0] || null); close(); }} />}
            </Popover>
          </div>
        </div>
        <Async state={st}>
          {(p) =>
            p.items.length === 0 ? (
              <Blankslate title={filtered ? "No results matched your search." : `No ${state} pull requests`} icon={<PullIcon size={24} />}>
                {state === "open" && user && !filtered ? <Link to={`${base}/compare`}>Compare branches and open a pull request</Link> : null}
              </Blankslate>
            ) : (
              <ul className="list">{p.items.map((pr) => <PullRow key={pr.id} pr={pr} base={base} />)}</ul>
            )
          }
        </Async>
      </div>
      {st.data && <Pagination page={page} next={st.data.next} prev={st.data.prev} last={st.data.last}
        onPage={(n) => setParam("page", String(n))} />}
    </div>
  );
}

/** `/{o}/{r}/compare`, `/{o}/{r}/compare/{base}...{head}`, `/{o}/{r}/pulls/new` */
export function ComparePage() {
  const { repo, owner, name, base: repoBase } = useRepo();
  const { user, loading } = useAuth();
  const spec = useParams()["*"] || "";
  const nav = useNavigate();
  const branches = useAsync(() => api.branches(owner, name), [owner, name]);
  useTitle(`Comparing changes · ${repo.full_name}`);

  let baseRef = repo.default_branch;
  let headRef = "";
  if (spec.includes("...")) [baseRef, headRef] = spec.split("...");
  else if (spec) headRef = spec;
  const names = (branches.data || []).map((b) => b.name);
  if (!headRef && names.length) headRef = names.find((b) => b !== baseRef) || baseRef;

  const cmp = useAsync(
    () => (headRef && baseRef !== headRef ? api.compare(owner, name, baseRef, headRef) : Promise.resolve(null)),
    [owner, name, baseRef, headRef],
  );
  const [title, setTitle] = useState("");
  const [body, setBody] = useState("");
  const [draft, setDraft] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>();

  useEffect(() => {
    const c = cmp.data;
    if (c && c.commits.length === 1) setTitle(firstLine(c.commits[0].commit.message));
    else if (headRef) setTitle(headRef.replace(/[-_/]/g, " ").replace(/^./, (s) => s.toUpperCase()));
  }, [cmp.data, headRef]);

  if (!loading && !user) return <Navigate to={`/login?return_to=${encodeURIComponent(location.pathname)}`} replace />;
  const go = (b: string, h: string) => nav(`${repoBase}/compare/${b}...${h}`);
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(undefined);
    try {
      const pr = await api.createPull(owner, name, { title: title.trim(), body, head: headRef, base: baseRef, draft: draft || undefined });
      nav(`${repoBase}/pull/${pr.number}`);
    } catch (err) {
      setError(err);
      setBusy(false);
    }
  };

  return (
    <div>
      <h2 className="h3">Compare changes</h2>
      <p className="muted">Choose two branches to see what's changed or to start a new pull request.</p>
      {branches.error !== undefined ? <ErrorBox error={branches.error} onRetry={branches.reload} /> : branches.loading ? <Spinner /> : (
        <div className="box compare-bar">
          <BranchSelect branches={names} value={baseRef} label="Base branch" onChange={(b) => go(b, headRef)} />
          <span aria-hidden="true">←</span>
          <BranchSelect branches={names} value={headRef} label="Compare branch" onChange={(h) => go(baseRef, h)} />
          {cmp.data && (
            cmp.data.status === "identical" || cmp.data.ahead_by === 0 ? null :
              <span className="c-open small">✓ {cmp.data.ahead_by} commit{cmp.data.ahead_by === 1 ? "" : "s"} ahead</span>
          )}
        </div>
      )}
      {names.length < 2 && !branches.loading && (
        <Flash>This repository needs at least two branches to open a pull request. Push a branch first.</Flash>
      )}
      {baseRef === headRef && names.length >= 2 && <Flash>Choose different branches to compare.</Flash>}
      {cmp.error !== undefined && (isNotFound(cmp.error) ? <Flash kind="warn">Could not compare these branches.</Flash> : <ErrorBox error={cmp.error} />)}
      {cmp.loading && headRef && baseRef !== headRef && <Spinner label="Comparing…" />}
      {cmp.data && (cmp.data.ahead_by === 0 ? (
        <Blankslate title="There isn't anything to compare.">
          <code>{baseRef}</code> is up to date with all commits from <code>{headRef}</code>.
        </Blankslate>
      ) : (
        <>
          <form className="box pr-form" onSubmit={submit}>
            <div className="box-body">
              {error !== undefined && <ErrorBox error={error} />}
              <div className="form-group">
                <label htmlFor="pr-title">Title</label>
                <input id="pr-title" className="input input-lg" required value={title} onChange={(e) => setTitle(e.target.value)} />
              </div>
              <div className="form-group">
                <span className="label-text">Description</span>
                <MarkdownEditor value={body} onChange={setBody} label="Description" placeholder="Add a description" repoPath={repo.full_name} />
              </div>
              <div className="form-actions">
                <label className="check-row">
                  <input type="checkbox" checked={draft} onChange={(e) => setDraft(e.target.checked)} /> Create as draft
                </label>
                <button type="submit" className="btn btn-primary" disabled={busy || !title.trim()}>
                  {busy ? "Creating…" : "Create pull request"}
                </button>
              </div>
            </div>
          </form>
          <h3 className="h4">{cmp.data.total_commits} commit{cmp.data.total_commits === 1 ? "" : "s"}</h3>
          <CommitList commits={cmp.data.commits} base={repoBase} />
          {cmp.data.files && <DiffView files={cmp.data.files} />}
        </>
      ))}
    </div>
  );
}

function MergeBox({ pr, onMerged, canPush, onChange }: { pr: FullPull; onMerged: () => void; canPush: boolean; onChange: (p: FullPull) => void }) {
  const { owner, name } = useRepo();
  const [method, setMethod] = useState<"merge" | "squash" | "rebase">("merge");
  const [confirm, setConfirm] = useState(false);
  const [title, setTitle] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const [note, setNote] = useState("");
  const behind = useAsync(
    () => (pr.state === "open" ? api.compare(owner, name, pr.head.sha, pr.base.ref).then((c) => c.ahead_by).catch(() => 0) : Promise.resolve(0)),
    [owner, name, pr.head.sha, pr.base.ref, pr.state],
  );
  const labels = { merge: "Create a merge commit", squash: "Squash and merge", rebase: "Rebase and merge" };
  const conflict = pr.mergeable === false;
  const defaultTitle = method === "squash" ? `${pr.title} (#${pr.number})` : `Merge pull request #${pr.number} from ${pr.head.label || pr.head.ref}`;
  const ready = async () => {
    setErr("");
    try {
      await gql(`mutation($id:ID!){markPullRequestReadyForReview(input:{pullRequestId:$id}){pullRequest{id}}}`, { id: pr.node_id });
      onChange(await papi.pull(owner, name, pr.number));
    } catch (e) {
      setErr(errorMessage(e));
    }
  };
  const updateBranch = async () => {
    setErr("");
    setNote("");
    setBusy(true);
    try {
      await papi.updateBranch(owner, name, pr.number);
      setNote("Branch updated.");
      // update-branch is asynchronous on GitHub; reload after a moment.
      setTimeout(async () => { onChange(await papi.pull(owner, name, pr.number)); behind.reload(); }, 800);
    } catch (e) {
      setErr(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };
  if (pr.draft) {
    return (
      <div className="box merge-box merge-conflict" data-testid="draft-box">
        <div className="box-body">
          <h3 className="h5">This pull request is still a work in progress</h3>
          <p className="muted small">Draft pull requests cannot be merged.</p>
          {canPush || pr.user.login ? (
            <button type="button" className="btn" onClick={ready} data-testid="ready-for-review">Ready for review</button>
          ) : null}
          {err && <p className="c-danger small" role="alert">{err}</p>}
        </div>
      </div>
    );
  }
  return (
    <div className={`box merge-box ${conflict ? "merge-conflict" : "merge-ok"}`}>
      <div className="box-body">
        {!!behind.data && behind.data > 0 && (
          <div className="pl-behind" data-testid="behind-box">
            <div>
              <strong>This branch is out-of-date with the base branch</strong>
              <div className="muted small">Merge the latest changes from <span className="branch-name">{pr.base.ref}</span> into this branch.</div>
            </div>
            {canPush && <button type="button" className="btn" disabled={busy} onClick={updateBranch} data-testid="update-branch">Update branch</button>}
          </div>
        )}
        {note && <p className="c-open small" role="status">{note}</p>}
        <h3 className="h5">
          {pr.mergeable === null || pr.mergeable === undefined
            ? "Checking mergeability…"
            : conflict
              ? "This branch has conflicts that must be resolved"
              : "This branch has no conflicts with the base branch"}
        </h3>
        <p className="muted small">
          {conflict ? "Resolve conflicts locally and push to update this pull request." : "Merging can be performed automatically."}
        </p>
        {!canPush ? (
          <p className="muted small">Only those with write access to this repository can merge pull requests.</p>
        ) : confirm ? (
          <form onSubmit={async (e) => {
            e.preventDefault();
            setBusy(true);
            setErr("");
            try {
              await api.mergePull(owner, name, pr.number, { merge_method: method, commit_title: method === "rebase" ? undefined : (title || defaultTitle) });
              onMerged();
            } catch (ex) {
              setErr(errorMessage(ex));
              setBusy(false);
            }
          }}>
            {method !== "rebase" && (
              <div className="form-group">
                <label htmlFor="merge-title">Commit message</label>
                <input id="merge-title" className="input" value={title || defaultTitle} onChange={(e) => setTitle(e.target.value)} />
              </div>
            )}
            {err && <p className="c-danger" role="alert">{err}</p>}
            <div className="form-actions">
              <button type="button" className="btn" onClick={() => setConfirm(false)}>Cancel</button>
              <button type="submit" className="btn btn-primary" disabled={busy}>{busy ? "Merging…" : `Confirm ${labels[method].toLowerCase()}`}</button>
            </div>
          </form>
        ) : (
          <div className="merge-actions">
            <label htmlFor="merge-method" className="sr-only">Merge method</label>
            <select id="merge-method" className="input input-auto" value={method}
              onChange={(e) => setMethod(e.target.value as typeof method)}>
              <option value="merge">Create a merge commit</option>
              <option value="squash">Squash and merge</option>
              <option value="rebase">Rebase and merge</option>
            </select>
            <button type="button" className="btn btn-primary" disabled={conflict} onClick={() => setConfirm(true)}>
              <MergeIcon /> {method === "merge" ? "Merge pull request" : labels[method]}
            </button>
          </div>
        )}
        {err && !confirm && <p className="c-danger small" role="alert">{err}</p>}
      </div>
    </div>
  );
}

/** Base branch switcher shown next to the PR meta line. */
function BaseEditor({ pr, onChange }: { pr: FullPull; onChange: (p: FullPull) => void }) {
  const { owner, name } = useRepo();
  const [open, setOpen] = useState(false);
  const branches = useAsync(() => (open ? api.branches(owner, name) : Promise.resolve([])), [owner, name, open]);
  const [err, setErr] = useState("");
  return (
    <>
      <button type="button" className="btn btn-sm" onClick={() => setOpen(true)} data-testid="edit-base">Edit base</button>
      {open && (
        <Dialog title="Change base branch" onClose={() => setOpen(false)}>
          {branches.loading ? <Spinner /> : (
            <div className="form-group">
              <label htmlFor="base-branch">Base branch</label>
              <select id="base-branch" className="input" defaultValue={pr.base.ref} onChange={async (e) => {
                setErr("");
                try {
                  onChange(await papi.updatePull(owner, name, pr.number, { base: e.target.value }));
                  setOpen(false);
                } catch (ex) { setErr(errorMessage(ex)); }
              }}>
                {(branches.data || []).filter((b) => b.name !== pr.head.ref || pr.head.repo?.full_name !== pr.base.repo?.full_name).map((b) => (
                  <option key={b.name} value={b.name}>{b.name}</option>
                ))}
              </select>
              <p className="muted small">Changing the base branch updates the commits and files compared in this pull request.</p>
            </div>
          )}
          {err && <p className="c-danger small" role="alert">{err}</p>}
        </Dialog>
      )}
    </>
  );
}

const PENDING_KEY = (o: string, r: string, n: number) => `forge:pending-review:${o}/${r}#${n}`;

/** Pending review comments (client-side until the review is submitted), kept in sessionStorage. */
function usePending(owner: string, name: string, n: number) {
  const key = PENDING_KEY(owner, name, n);
  const [pending, setPendingState] = useState<NewReviewComment[]>(() => {
    try { return JSON.parse(sessionStorage.getItem(key) || "[]"); } catch { return []; }
  });
  const setPending = (p: NewReviewComment[]) => {
    setPendingState(p);
    try { if (p.length) sessionStorage.setItem(key, JSON.stringify(p)); else sessionStorage.removeItem(key); } catch { /* ignore */ }
  };
  return [pending, setPending] as const;
}

/** `/{o}/{r}/pull/{n}`, `/pull/{n}/commits`, `/pull/{n}/files` */
export function PullPage({ tab }: { tab: "conversation" | "commits" | "files" }) {
  const { repo, owner, name, base, canPush } = useRepo();
  const { user } = useAuth();
  const n = Number(useParams().number);
  const st = useAsync(() => papi.pull(owner, name, n), [owner, name, n]);
  const reviews = useAsync(() => papi.reviews(owner, name, n), [owner, name, n]);
  const threads = useThreads(owner, name, n);
  const [pending, setPending] = usePending(owner, name, n);
  useTitle(st.data ? `${st.data.title} · Pull Request #${n} · ${repo.full_name}` : `Pull Request #${n}`);
  const prBase = `${base}/pull/${n}`;
  return (
    <Async state={st}>
      {(pr) => {
        const canEdit = !!user && (canPush || user.login === pr.user.login);
        const setPr = (p: FullPull) => st.setData(p);
        return (
          <div>
            <EditableTitle title={pr.title} number={pr.number} canEdit={canEdit}
              onSave={async (t) => st.setData(await papi.updatePull(owner, name, n, { title: t }))}
              extra={user && <Link to={`${base}/compare`} className="btn btn-sm">New pull request</Link>} />
            <div className="issue-meta">
              <StateBadge item={pr} />
              {pr.locked && <span className="badge"><LockIcon /> Locked</span>}
              <span className="muted">
                <UserLink user={pr.merged_at && pr.merged_by ? pr.merged_by : pr.user} />{" "}
                {pr.merged_at ? "merged " : "wants to merge "}
                {pr.commits ?? ""} commit{pr.commits === 1 ? "" : "s"} into <span className="branch-name" data-testid="base-ref">{pr.base.ref}</span> from{" "}
                <span className="branch-name">{pr.head.label || pr.head.ref}</span>
              </span>
              {canEdit && pr.state === "open" && <BaseEditor pr={pr} onChange={setPr} />}
            </div>
            <nav className="tabnav" aria-label="Pull request">
              <NavLink end to={prBase} className={({ isActive }) => "tabnav-tab" + (isActive ? " selected" : "")}>
                <CommentIcon /> Conversation <span className="counter">{pr.comments ?? 0}</span>
              </NavLink>
              <NavLink to={`${prBase}/commits`} className={({ isActive }) => "tabnav-tab" + (isActive ? " selected" : "")}>
                Commits <span className="counter">{pr.commits ?? "–"}</span>
              </NavLink>
              <NavLink to={`${prBase}/files`} className={({ isActive }) => "tabnav-tab" + (isActive ? " selected" : "")}>
                Files changed <span className="counter">{pr.changed_files ?? "–"}</span>
              </NavLink>
              {pr.additions !== undefined && (
                <span className="tabnav-extra small">
                  <span className="c-add">+{pr.additions}</span> <span className="c-del">−{pr.deletions}</span>
                </span>
              )}
            </nav>
            {tab === "conversation" && <Conversation pr={pr} canEdit={canEdit} canPush={canPush} reload={st.reload}
              setPr={setPr} reviews={reviews.data || []} reloadReviews={reviews.reload} threads={threads.data?.threads || []}
              reloadThreads={threads.reload} pending={pending} setPending={setPending} />}
            {tab === "commits" && <PullCommits n={n} />}
            {tab === "files" && <PullFiles pr={pr} canPush={canPush} threads={threads.data?.threads || []} reloadThreads={threads.reload}
              pending={pending} setPending={setPending} onReviewed={() => { reviews.reload(); threads.reload(); }} />}
          </div>
        );
      }}
    </Async>
  );
}

const REVIEW_TEXT: Record<string, string> = {
  APPROVED: "approved these changes",
  CHANGES_REQUESTED: "requested changes",
  COMMENTED: "reviewed",
  DISMISSED: "reviewed",
};

function ReviewEntry({ review, repoPath, threads, pr, canPush, reloadThreads, onDismiss }: {
  review: FullReview; repoPath: string; threads: Thread[]; pr: FullPull; canPush: boolean; reloadThreads: () => void;
  onDismiss?: () => void;
}) {
  const { owner, name } = useRepo();
  const cls = review.state === "APPROVED" ? "review-approved" : review.state === "CHANGES_REQUESTED" ? "review-changes" : "";
  const mine = threads.filter((t) => t.root.pull_request_review_id === review.id);
  // A review that is only a single line comment renders as the thread alone, like github.com.
  return (
    <div className="pl-review" data-testid="review-entry" data-state={review.state}>
      <div className="timeline-event" data-testid="review-event">
        {review.state === "APPROVED" ? <CheckIcon className="icon c-open" /> : review.state === "CHANGES_REQUESTED" ? <span className="c-danger" aria-hidden="true">±</span> : <CommentIcon />}{" "}
        <UserLink user={review.user} />{" "}
        <span className={`review-state ${cls}`}>{REVIEW_TEXT[review.state] || review.state.toLowerCase()}</span>{" "}
        <RelTime iso={review.submitted_at} />
        {review.state === "DISMISSED" && <span className="badge">Dismissed</span>}
        {onDismiss && <button type="button" className="btn-link small" onClick={onDismiss} data-testid="dismiss-review">Dismiss review</button>}
      </div>
      {review.body && <CommentBox user={review.user} body={review.body} created={review.submitted_at || ""}
        association={review.author_association} repoPath={repoPath} />}
      {mine.length > 0 && (
        <div className="pl-review-threads">
          {mine.map((t) => <ThreadView key={t.root.id} thread={t} owner={owner} name={name} pr={pr} canPush={canPush} onChange={reloadThreads} showContext repoPath={repoPath} />)}
        </div>
      )}
    </div>
  );
}

function DismissDialog({ review, onClose, onDone }: { review: FullReview; onClose: () => void; onDone: () => void }) {
  const { owner, name } = useRepo();
  const n = Number(useParams().number);
  const [msg, setMsg] = useState("");
  const [err, setErr] = useState("");
  return (
    <Dialog title="Dismiss review" onClose={onClose}>
      <p className="small">Are you sure you want to dismiss {review.user?.login}'s review?</p>
      <label htmlFor="dismiss-msg">Message</label>
      <textarea id="dismiss-msg" className="input textarea" rows={3} value={msg} onChange={(e) => setMsg(e.target.value)} />
      {err && <p className="c-danger small" role="alert">{err}</p>}
      <div className="form-actions">
        <button type="button" className="btn" onClick={onClose}>Cancel</button>
        <button type="button" className="btn btn-danger" disabled={!msg.trim()} onClick={async () => {
          try { await papi.dismissReview(owner, name, n, review.id, msg.trim()); onDone(); } catch (e) { setErr(errorMessage(e)); }
        }}>Dismiss review</button>
      </div>
    </Dialog>
  );
}

/** "Review changes" / "Finish your review" form; submits pending line comments too. */
function ReviewForm({ pr, pending, setPending, onDone, compact }: {
  pr: FullPull; pending: NewReviewComment[]; setPending: (p: NewReviewComment[]) => void; onDone: () => void; compact?: boolean;
}) {
  const { owner, name, repo } = useRepo();
  const { user } = useAuth();
  const [open, setOpen] = useState(false);
  const [body, setBody] = useState("");
  const own = user?.login === pr.user.login;
  const [event, setEvent] = useState<"COMMENT" | "APPROVE" | "REQUEST_CHANGES">("COMMENT");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  if (!open) {
    return (
      <div className={compact ? "" : "timeline-event"}>
        <button type="button" className={"btn btn-sm" + (pending.length ? " btn-primary" : "")} onClick={() => setOpen(true)} data-testid="review-open">
          {pending.length ? <>Finish your review <span className="counter">{pending.length}</span></> : "Review changes"}
        </button>
      </div>
    );
  }
  return (
    <form className="box box-body review-form" aria-label="Submit review" onSubmit={async (e) => {
      e.preventDefault();
      setBusy(true);
      setErr("");
      try {
        await papi.createReview(owner, name, pr.number, { event, body: body.trim() || undefined, commit_id: pr.head.sha, comments: pending.length ? pending : undefined });
        setPending([]);
        onDone();
        setOpen(false);
        setBody("");
      } catch (ex) {
        setErr(errorMessage(ex));
      } finally {
        setBusy(false);
      }
    }}>
      <h3 className="h5">Finish your review</h3>
      {pending.length > 0 && <p className="muted small">{pending.length} pending comment{pending.length === 1 ? "" : "s"} will be submitted with this review.</p>}
      <MarkdownEditor value={body} onChange={setBody} label="Review summary" placeholder="Leave a comment" rows={4} repoPath={repo.full_name} />
      <fieldset>
        <legend className="sr-only">Review type</legend>
        <label className="radio-row"><input type="radio" name="review-event" checked={event === "COMMENT"} onChange={() => setEvent("COMMENT")} />
          <span><strong>Comment</strong><br /><span className="muted small">Submit general feedback without explicit approval.</span></span></label>
        <label className="radio-row"><input type="radio" name="review-event" disabled={own} checked={event === "APPROVE"} onChange={() => setEvent("APPROVE")} />
          <span><strong>Approve</strong><br /><span className="muted small">{own ? "Pull request authors can't approve their own pull request." : "Submit feedback and approve merging these changes."}</span></span></label>
        <label className="radio-row"><input type="radio" name="review-event" disabled={own} checked={event === "REQUEST_CHANGES"} onChange={() => setEvent("REQUEST_CHANGES")} />
          <span><strong>Request changes</strong><br /><span className="muted small">{own ? "Pull request authors can't request changes on their own pull request." : "Submit feedback that must be addressed before merging."}</span></span></label>
      </fieldset>
      {err && <p className="c-danger small" role="alert">{err}</p>}
      <div className="form-actions">
        {pending.length > 0 && (
          <button type="button" className="btn btn-danger-outline" onClick={() => { if (confirmAction("Discard all pending comments?")) { setPending([]); setOpen(false); } }}>
            Abandon review
          </button>
        )}
        <button type="button" className="btn" onClick={() => setOpen(false)}>Cancel</button>
        <button type="submit" className="btn btn-primary" disabled={busy || (event === "REQUEST_CHANGES" && !body.trim() && !pending.length) || (event === "COMMENT" && !body.trim() && !pending.length)}>
          {busy ? "Submitting…" : "Submit review"}
        </button>
      </div>
    </form>
  );
}

type Entry = { at: string; c?: FullComment; r?: FullReview; e?: TimelineEvent; t?: Thread };

function Conversation({ pr, canEdit, canPush, reload, setPr, reviews, reloadReviews, threads, reloadThreads, pending, setPending }: {
  pr: FullPull; canEdit: boolean; canPush: boolean; reload: () => void; setPr: (p: FullPull) => void;
  reviews: FullReview[]; reloadReviews: () => void; threads: Thread[]; reloadThreads: () => void;
  pending: NewReviewComment[]; setPending: (p: NewReviewComment[]) => void;
}) {
  const { user } = useAuth();
  const repoCtx = useRepo();
  const { repo, owner, name, base } = repoCtx;
  const comments = useAsync(() => papi.comments(owner, name, pr.number), [owner, name, pr.number]);
  const timeline = useAsync(() => papi.timeline(owner, name, pr.number), [owner, name, pr.number, pr.updated_at]);
  const issueNode = useAsync(() => papi.issue(owner, name, pr.number), [owner, name, pr.number]);
  const [err, setErr] = useState<unknown>();
  const [dismissing, setDismissing] = useState<FullReview | null>(null);
  const reviewIds = new Set(reviews.map((r) => r.id));
  const timelineEntries = useMemo<Entry[]>(() => [
    ...(comments.data || []).map((c) => ({ at: c.created_at, c })),
    ...reviews.filter((r) => r.state !== "PENDING").map((r) => ({ at: r.submitted_at || "", r })),
    // Threads whose review is missing (e.g. pending reviews of others) are shown on their own.
    ...threads.filter((t) => !t.root.pull_request_review_id || !reviewIds.has(t.root.pull_request_review_id)).map((t) => ({ at: t.root.created_at, t })),
    ...shownEvents(timeline.data || []).map((e) => ({ at: eventTime(e), e })),
  ].sort((a, b) => a.at.localeCompare(b.at)), [comments.data, reviews, threads, timeline.data]);
  const setState = async (state: "open" | "closed") => {
    setErr(undefined);
    try {
      setPr(await papi.updatePull(owner, name, pr.number, { state }));
      timeline.reload();
    } catch (e) {
      setErr(e);
    }
  };
  const locked = !!pr.locked;
  const commentMenu = (c: FullComment) => (startEdit: () => void) => {
    const mine = user?.login === c.user.login;
    return (
      <KebabMenu label="Comment actions" items={[
        { label: "Edit", hidden: !(mine || canPush), onClick: startEdit },
        { label: "Delete", danger: true, hidden: !(mine || canPush), onClick: async () => {
          if (!confirmAction("Are you sure you want to delete this comment?")) return;
          try { await papi.deleteComment(owner, name, c.id); comments.setData((comments.data || []).filter((x) => x.id !== c.id)); } catch (e) { setErr(e); }
        } },
      ]} />
    );
  };
  return (
    <div className="layout-sidebar">
      <div className="layout-main timeline">
        <CommentBox user={pr.user} body={pr.body} created={pr.created_at} repoPath={repo.full_name} verb="opened"
          onSave={canEdit ? async (b) => setPr(await papi.updatePull(owner, name, pr.number, { body: b })) : undefined}
          menu={(startEdit) => <KebabMenu label="Pull request body actions" items={[{ label: "Edit", hidden: !canEdit, onClick: startEdit }]} />}
          renderBody={(b) => <CommentBody body={b} repoPath={repo.full_name} />}
          footer={<ReactionBar owner={owner} name={name} subject={`issues/${pr.number}`} summary={issueNode.data?.reactions} readOnly={locked && !canPush} />} />
        {comments.error !== undefined ? <ErrorBox error={comments.error} onRetry={comments.reload} /> :
          timelineEntries.map((e) => e.c ? (
            <CommentBox key={`c${e.c.id}`} id={`issuecomment-${e.c.id}`} user={e.c.user} body={e.c.body} created={e.c.created_at}
              association={e.c.author_association} repoPath={repo.full_name}
              renderBody={(b) => <CommentBody body={b} repoPath={repo.full_name} />}
              onSave={async (b) => {
                const u = await papi.updateComment(owner, name, e.c!.id, b);
                comments.setData((comments.data || []).map((x) => (x.id === u.id ? { ...x, ...u } : x)));
              }}
              menu={commentMenu(e.c)}
              footer={<ReactionBar owner={owner} name={name} subject={`issues/comments/${e.c.id}`} summary={e.c.reactions} readOnly={locked && !canPush} />} />
          ) : e.r ? (
            <ReviewEntry key={`r${e.r.id}`} review={e.r} repoPath={repo.full_name} threads={threads} pr={pr} canPush={canPush}
              reloadThreads={reloadThreads}
              onDismiss={canPush && (e.r.state === "APPROVED" || e.r.state === "CHANGES_REQUESTED") ? () => setDismissing(e.r!) : undefined} />
          ) : e.t ? (
            <div key={`t${e.t.root.id}`} className="pl-review-threads">
              <ThreadView thread={e.t} owner={owner} name={name} pr={pr} canPush={canPush} onChange={reloadThreads} showContext repoPath={repo.full_name} />
            </div>
          ) : e.e ? <TimelineEventRow key={`e${e.e.id ?? e.at}${e.e.event}`} e={e.e} base={base} /> : null)}
        {pr.merged_at ? (
          <div className="timeline-event">
            <MergeIcon className="icon c-merged" /> <UserLink user={pr.merged_by} /> merged commit{" "}
            {pr.merge_commit_sha && <Link className="mono" to={`${base}/commit/${pr.merge_commit_sha}`}>{pr.merge_commit_sha.slice(0, 7)}</Link>}{" "}
            into <span className="branch-name">{pr.base.ref}</span> <RelTime iso={pr.merged_at} />
          </div>
        ) : pr.state === "closed" ? (
          <div className="timeline-event muted">
            <PullClosedIcon className="icon c-closed" /> This pull request was closed <RelTime iso={pr.closed_at} />.
          </div>
        ) : (
          <>
            <Slot name="pull.merge.top" ctx={repoCtx} pull={pr} />
            <MergeBox pr={pr} canPush={canPush} onMerged={reload} onChange={(p) => { setPr(p); timeline.reload(); }} />
          </>
        )}
        {err !== undefined && <ErrorBox error={err} />}
        {user && !pr.merged_at && pr.state === "open" && (
          <ReviewForm pr={pr} pending={pending} setPending={setPending} onDone={() => { reloadReviews(); reloadThreads(); }} />
        )}
        {locked && (
          <Flash kind="warn"><LockIcon /> This conversation has been locked{pr.active_lock_reason ? <> as <strong>{pr.active_lock_reason}</strong></> : null} and limited to collaborators.</Flash>
        )}
        {(!locked || canPush) && (
          <CommentForm repoPath={repo.full_name}
            onSubmit={async (body) => {
              const c = await papi.addComment(owner, name, pr.number, body);
              comments.setData([...(comments.data || []), c]);
            }}
            extra={() =>
              canEdit && !pr.merged_at ? (
                pr.state === "open" ? (
                  <button type="button" className="btn" onClick={() => setState("closed")}>Close pull request</button>
                ) : (
                  <button type="button" className="btn" onClick={() => setState("open")}>Reopen pull request</button>
                )
              ) : null
            }
          />
        )}
      </div>
      <aside className="layout-aside">
        <ReviewersSection owner={owner} name={name} pr={pr} reviews={reviews} canEdit={canPush} onChange={(p) => { setPr(p); timeline.reload(); }} />
        <AssigneesSection owner={owner} name={name} number={pr.number} assignees={pr.assignees || []} canEdit={canPush}
          onChange={(a) => { setPr({ ...pr, assignees: a }); timeline.reload(); }} />
        <LabelsSidebar owner={owner} name={name} number={pr.number} labels={pr.labels} canEdit={canPush}
          onChange={(labels) => { setPr({ ...pr, labels }); timeline.reload(); }} />
        {pr.node_id && <ProjectsSection nodeId={pr.node_id} owner={owner} canEdit={canPush} />}
        <MilestoneSection owner={owner} name={name} milestone={pr.milestone} canEdit={canPush}
          onSet={async (m) => { await papi.updateIssue(owner, name, pr.number, { milestone: m }); setPr(await papi.pull(owner, name, pr.number)); timeline.reload(); }} />
        <Slot name="pull.sidebar" ctx={repoCtx} pull={pr} setPull={setPr} />
        {canEdit && pr.state === "open" && !pr.draft && (
          <section className="sidebar-section">
            <p className="muted small">Still in progress?{" "}
              <button type="button" className="btn-link" data-testid="convert-to-draft" onClick={async () => {
                try {
                  await gql(`mutation($id:ID!){convertPullRequestToDraft(input:{pullRequestId:$id}){pullRequest{id}}}`, { id: pr.node_id });
                  setPr(await papi.pull(owner, name, pr.number));
                  timeline.reload();
                } catch (e) { setErr(e); }
              }}>Convert to draft</button>
            </p>
          </section>
        )}
        {canPush && (
          <section className="sidebar-section pl-side-actions">
            <LockControl owner={owner} name={name} number={pr.number} locked={locked} kind="pull request"
              onChange={async () => { setPr(await papi.pull(owner, name, pr.number)); timeline.reload(); }} />
          </section>
        )}
      </aside>
      {dismissing && <DismissDialog review={dismissing} onClose={() => setDismissing(null)} onDone={() => { setDismissing(null); reloadReviews(); timeline.reload(); }} />}
    </div>
  );
}

function PullCommits({ n }: { n: number }) {
  const { owner, name, base } = useRepo();
  const st = useAsync(() => api.pullCommits(owner, name, n), [owner, name, n]);
  return <Async state={st}>{(c) => (c.length ? <CommitList commits={c} base={base} /> : <Blankslate title="No commits" />)}</Async>;
}

function PendingComment({ c, onRemove }: { c: NewReviewComment; onRemove: () => void }) {
  return (
    <div className="box pl-thread pl-pending" data-testid="pending-comment">
      <div className="pl-thread-comment">
        <div className="pl-thread-comment-head"><span className="badge badge-warn">Pending</span><span className="spacer" />
          <button type="button" className="btn-link small c-danger" onClick={onRemove}>Delete</button></div>
        <div className="pl-thread-comment-body"><CommentBody body={c.body} repoPath="" /></div>
      </div>
    </div>
  );
}

function PullFiles({ pr, canPush, threads, reloadThreads, pending, setPending, onReviewed }: {
  pr: FullPull; canPush: boolean; threads: Thread[]; reloadThreads: () => void;
  pending: NewReviewComment[]; setPending: (p: NewReviewComment[]) => void; onReviewed: () => void;
}) {
  const { owner, name, repo } = useRepo();
  const { user } = useAuth();
  const st = useAsync(() => api.pullFiles(owner, name, pr.number), [owner, name, pr.number]);
  const review: DiffReview = {
    canComment: !!user && pr.state === "open" && (!pr.locked || canPush),
    reviewing: pending.length > 0,
    repoPath: repo.full_name,
    threadsAt: (path, side, line) => threads
      .filter((t) => t.root.path === path && t.root.side === side && t.root.line === line)
      .map((t) => <ThreadView key={t.root.id} thread={t} owner={owner} name={name} pr={pr} canPush={canPush} onChange={reloadThreads} repoPath={repo.full_name} />),
    pendingAt: (path, side, line) => pending
      .map((c, i) => ({ c, i }))
      .filter(({ c }) => c.path === path && c.side === side && c.line === line)
      .map(({ c, i }) => <PendingComment key={`p${i}`} c={c} onRemove={() => setPending(pending.filter((_, j) => j !== i))} />),
    onComment: async (c: LineCommentDraft, mode) => {
      if (mode === "single") {
        await papi.addReviewComment(owner, name, pr.number, { ...c, commit_id: pr.head.sha });
        reloadThreads();
        onReviewed();
      } else {
        setPending([...pending, c]);
      }
    },
  };
  return (
    <Async state={st}>
      {(f) => (
        <DiffView files={f} review={review}
          toolbar={user && pr.state === "open" ? (
            <div className="pl-files-review"><ReviewForm pr={pr} pending={pending} setPending={setPending} onDone={onReviewed} compact /></div>
          ) : undefined} />
      )}
    </Async>
  );
}
