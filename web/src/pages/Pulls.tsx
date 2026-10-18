import { useEffect, useState, type FormEvent } from "react";
import { Link, NavLink, Navigate, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { api, errorMessage, isNotFound, type IssueComment, type Pull, type Review } from "../api";
import { useAuth } from "../auth";
import { BranchSelect } from "../components/BranchSelect";
import { CommitList } from "../components/CommitList";
import {
  Async, Blankslate, ErrorBox, Flash, ItemIcon, LabelPill, Pagination, RelTime, Spinner, StateBadge, UserLink,
} from "../components/common";
import { CommentBox, CommentForm, LabelsSidebar, MarkdownEditor, useComments } from "../components/Conversation";
import { DiffView } from "../components/Diff";
import { CheckIcon, CommentIcon, MergeIcon, PullClosedIcon, PullIcon } from "../components/icons";
import { useRepo } from "../components/RepoLayout";
import { useAsync, useTitle } from "../hooks";
import { firstLine } from "../util";
import { EditableTitle } from "./Issues";
import { Slot } from "../areas";

function PullRow({ pr, base }: { pr: Pull; base: string }) {
  const href = `${base}/pull/${pr.number}`;
  return (
    <li className="list-row issue-row">
      <ItemIcon item={pr} />
      <div className="issue-row-main">
        <Link to={href} className="issue-title">{pr.title}</Link>{" "}
        {pr.draft && <span className="badge">Draft</span>}{" "}
        {pr.labels.map((l) => <LabelPill key={l.id} label={l} />)}
        <div className="muted small">
          #{pr.number}{" "}
          {pr.merged_at ? (
            <>by <UserLink user={pr.user} /> was merged <RelTime iso={pr.merged_at} /></>
          ) : pr.state === "open" ? (
            <>opened <RelTime iso={pr.created_at} /> by <UserLink user={pr.user} /></>
          ) : (
            <>by <UserLink user={pr.user} /> was closed <RelTime iso={pr.closed_at} /></>
          )}{" "}
          · <span className="mono">{pr.head.ref}</span> → <span className="mono">{pr.base.ref}</span>
        </div>
      </div>
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
  useTitle(`Pull requests · ${repo.full_name}`);
  const st = useAsync(() => api.pulls(owner, name, state, page), [owner, name, state, page]);
  const set = (s: string | null, p?: number) => {
    const n = new URLSearchParams();
    if (s) n.set("state", s);
    if (p && p > 1) n.set("page", String(p));
    setSp(n);
  };
  return (
    <div>
      <div className="toolbar">
        <h2 className="h3">Pull requests</h2>
        <span className="spacer" />
        {user && <Link to={`${base}/compare`} className="btn btn-primary">New pull request</Link>}
      </div>
      <div className="box">
        <div className="box-header list-tabs" role="tablist" aria-label="Pull request state">
          <button type="button" role="tab" aria-selected={state === "open"} className={state === "open" ? "selected" : ""}
            onClick={() => set(null)}><PullIcon /> Open</button>
          <button type="button" role="tab" aria-selected={state === "closed"} className={state === "closed" ? "selected" : ""}
            onClick={() => set("closed")}><PullClosedIcon /> Closed</button>
        </div>
        <Async state={st}>
          {(p) =>
            p.items.length === 0 ? (
              <Blankslate title={`No ${state} pull requests`} icon={<PullIcon size={24} />}>
                {state === "open" && user ? <Link to={`${base}/compare`}>Compare branches and open a pull request</Link> : null}
              </Blankslate>
            ) : (
              <ul className="list">{p.items.map((pr) => <PullRow key={pr.id} pr={pr} base={base} />)}</ul>
            )
          }
        </Async>
      </div>
      {st.data && <Pagination page={page} next={st.data.next} prev={st.data.prev} last={st.data.last}
        onPage={(n) => set(state === "closed" ? "closed" : null, n)} />}
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

function MergeBox({ pr, onMerged, canPush }: { pr: Pull; onMerged: () => void; canPush: boolean }) {
  const { owner, name } = useRepo();
  const [method, setMethod] = useState<"merge" | "squash" | "rebase">("merge");
  const [confirm, setConfirm] = useState(false);
  const [title, setTitle] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState("");
  const labels = { merge: "Create a merge commit", squash: "Squash and merge", rebase: "Rebase and merge" };
  const conflict = pr.mergeable === false;
  const defaultTitle = method === "squash" ? `${pr.title} (#${pr.number})` : `Merge pull request #${pr.number} from ${pr.head.label || pr.head.ref}`;
  return (
    <div className={`box merge-box ${conflict ? "merge-conflict" : "merge-ok"}`}>
      <div className="box-body">
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
            <button type="button" className="btn btn-primary" disabled={conflict || pr.draft} onClick={() => setConfirm(true)}>
              <MergeIcon /> {method === "merge" ? "Merge pull request" : labels[method]}
            </button>
            {pr.draft && <span className="muted small">This pull request is still a draft.</span>}
          </div>
        )}
      </div>
    </div>
  );
}

/** `/{o}/{r}/pull/{n}`, `/pull/{n}/commits`, `/pull/{n}/files` */
export function PullPage({ tab }: { tab: "conversation" | "commits" | "files" }) {
  const { repo, owner, name, base, canPush } = useRepo();
  const { user } = useAuth();
  const n = Number(useParams().number);
  const st = useAsync(() => api.pull(owner, name, n), [owner, name, n]);
  useTitle(st.data ? `${st.data.title} · Pull Request #${n} · ${repo.full_name}` : `Pull Request #${n}`);
  const prBase = `${base}/pull/${n}`;
  return (
    <Async state={st}>
      {(pr) => {
        const canEdit = !!user && (canPush || user.login === pr.user.login);
        return (
          <div>
            <EditableTitle title={pr.title} number={pr.number} canEdit={canEdit}
              onSave={async (t) => st.setData(await api.updatePull(owner, name, n, { title: t }))} />
            <div className="issue-meta">
              <StateBadge item={pr} />
              <span className="muted">
                <UserLink user={pr.merged_at && pr.merged_by ? pr.merged_by : pr.user} />{" "}
                {pr.merged_at ? "merged " : "wants to merge "}
                {pr.commits ?? ""} commit{pr.commits === 1 ? "" : "s"} into <span className="branch-name">{pr.base.ref}</span> from{" "}
                <span className="branch-name">{pr.head.label || pr.head.ref}</span>
              </span>
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
              setPr={(p) => st.setData(p)} />}
            {tab === "commits" && <PullCommits n={n} />}
            {tab === "files" && <PullFiles n={n} />}
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
  DISMISSED: "reviewed (dismissed)",
};

function ReviewEntry({ review, repoPath }: { review: Review; repoPath: string }) {
  const cls = review.state === "APPROVED" ? "review-approved" : review.state === "CHANGES_REQUESTED" ? "review-changes" : "";
  return (
    <>
      <div className="timeline-event" data-testid="review-event">
        {review.state === "APPROVED" ? <CheckIcon className="icon c-open" /> : <CommentIcon />}{" "}
        <UserLink user={review.user} />{" "}
        <span className={`review-state ${cls}`}>{REVIEW_TEXT[review.state] || review.state.toLowerCase()}</span>{" "}
        <RelTime iso={review.submitted_at} />
      </div>
      {review.body && <CommentBox user={review.user} body={review.body} created={review.submitted_at || ""}
        association={review.author_association} repoPath={repoPath} />}
    </>
  );
}

function ReviewForm({ pr, onDone }: { pr: Pull; onDone: (r: Review) => void }) {
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
      <div className="timeline-event">
        <button type="button" className="btn btn-sm" onClick={() => setOpen(true)} data-testid="review-open">Review changes</button>
      </div>
    );
  }
  return (
    <form className="box box-body review-form" aria-label="Submit review" onSubmit={async (e) => {
      e.preventDefault();
      setBusy(true);
      setErr("");
      try {
        const r = await api.createReview(owner, name, pr.number, { event, body: body.trim() || undefined });
        onDone(r);
        setOpen(false);
        setBody("");
      } catch (ex) {
        setErr(errorMessage(ex));
      } finally {
        setBusy(false);
      }
    }}>
      <h3 className="h5">Finish your review</h3>
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
        <button type="button" className="btn" onClick={() => setOpen(false)}>Cancel</button>
        <button type="submit" className="btn btn-primary" disabled={busy || (event !== "APPROVE" && !body.trim())}>
          {busy ? "Submitting…" : "Submit review"}
        </button>
      </div>
    </form>
  );
}

function Conversation({ pr, canEdit, canPush, reload, setPr }: {
  pr: Pull; canEdit: boolean; canPush: boolean; reload: () => void; setPr: (p: Pull) => void;
}) {
  const { user } = useAuth();
  const repoCtx = useRepo();
  const { repo, owner, name, base } = repoCtx;
  const comments = useComments(owner, name, pr.number);
  const reviews = useAsync(() => api.reviews(owner, name, pr.number), [owner, name, pr.number]);
  const [err, setErr] = useState<unknown>();
  type Entry = { at: string; c?: IssueComment; r?: Review };
  const timeline: Entry[] = [
    ...(comments.data || []).map((c) => ({ at: c.created_at, c })),
    ...(reviews.data || []).filter((r) => r.state !== "PENDING").map((r) => ({ at: r.submitted_at || "", r })),
  ].sort((a, b) => a.at.localeCompare(b.at));
  const setState = async (state: "open" | "closed") => {
    setErr(undefined);
    try {
      setPr(await api.updatePull(owner, name, pr.number, { state }));
    } catch (e) {
      setErr(e);
    }
  };
  return (
    <div className="layout-sidebar">
      <div className="layout-main timeline">
        <CommentBox user={pr.user} body={pr.body} created={pr.created_at} repoPath={repo.full_name} />
        {comments.error !== undefined ? <ErrorBox error={comments.error} onRetry={comments.reload} /> :
          reviews.error !== undefined ? <ErrorBox error={reviews.error} onRetry={reviews.reload} /> :
          timeline.map((e) => e.c ? (
            <CommentBox key={`c${e.c.id}`} user={e.c.user} body={e.c.body} created={e.c.created_at}
              association={e.c.author_association} repoPath={repo.full_name} />
          ) : e.r ? <ReviewEntry key={`r${e.r.id}`} review={e.r} repoPath={repo.full_name} /> : null)}
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
            <MergeBox pr={pr} canPush={canPush} onMerged={reload} />
          </>
        )}
        {err !== undefined && <ErrorBox error={err} />}
        {user && !pr.merged_at && pr.state === "open" && (
          <ReviewForm pr={pr} onDone={(r) => reviews.setData([...(reviews.data || []), r])} />
        )}
        <CommentForm repoPath={repo.full_name}
          onSubmit={async (body) => {
            const c = await api.addComment(owner, name, pr.number, body);
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
      </div>
      <aside className="layout-aside">
        <LabelsSidebar owner={owner} name={name} number={pr.number} labels={pr.labels} canEdit={canPush}
          onChange={(labels) => setPr({ ...pr, labels })} />
        <Slot name="pull.sidebar" ctx={repoCtx} pull={pr} setPull={setPr} />
      </aside>
    </div>
  );
}

function PullCommits({ n }: { n: number }) {
  const { owner, name, base } = useRepo();
  const st = useAsync(() => api.pullCommits(owner, name, n), [owner, name, n]);
  return <Async state={st}>{(c) => (c.length ? <CommitList commits={c} base={base} /> : <Blankslate title="No commits" />)}</Async>;
}

function PullFiles({ n }: { n: number }) {
  const { owner, name } = useRepo();
  const st = useAsync(() => api.pullFiles(owner, name, n), [owner, name, n]);
  return <Async state={st}>{(f) => <DiffView files={f} />}</Async>;
}
