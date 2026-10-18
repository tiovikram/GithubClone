// A workflow run: summary (trigger, status, duration, job graph, annotations, job summaries,
// artifacts, deployment reviews) and per-job step logs, polled while the run is in progress.
import { Fragment, useEffect, useMemo, useState, type FormEvent } from "react";
import { Link, NavLink, useNavigate, useParams } from "react-router-dom";
import { Async, ErrorBox, Flash, NotFoundView, RelTime, Spinner, UserLink } from "../../components/common";
import { useRepo } from "../../components/RepoLayout";
import { useAsync, useTitle } from "../../hooks";
import { TrashIcon } from "../../components/icons";
import { Markdown } from "../../markdown";
import { shortSha } from "../../util";
import { actionsApi as A, type Artifact, type Job, type PendingDeployment, type Step, type WorkflowRun } from "./api";
import { Dropdown, StatusIcon, bytes, duration, kindLabel, kindOf, usePoll } from "./ui";
import { workflowInfo } from "./yaml";

export function RunPage() {
  const { runId = "", attempt: attemptParam, jobId } = useParams();
  const { owner, name, repo } = useRepo();
  const id = Number(runId);
  const attemptN = attemptParam ? Number(attemptParam) : undefined;
  const run = useAsync(() => (attemptN ? A.attempt(owner, name, id, attemptN) : A.run(owner, name, id)), [owner, name, id, attemptN]);
  const jobs = useAsync(() => A.jobs(owner, name, id, attemptN), [owner, name, id, attemptN]);
  const live = !!run.data && (run.data.status !== "completed" || (jobs.data?.jobs || []).some((j) => j.status !== "completed"));
  usePoll(() => { run.reload(); jobs.reload(); }, 2500, live);
  useTitle(run.data ? `${run.data.display_title} · ${run.data.name} #${run.data.run_number} · ${repo.full_name}` : `Actions · ${repo.full_name}`);
  if (!Number.isFinite(id)) return <NotFoundView />;
  return (
    <Async state={run}>
      {(r) => <RunView run={r} jobs={jobs.data?.jobs} jobsError={jobs.error} reload={() => { run.reload(); jobs.reload(); }} jobId={jobId ? Number(jobId) : undefined} attemptN={attemptN} />}
    </Async>
  );
}

function RunView({ run, jobs, jobsError, reload, jobId, attemptN }: {
  run: WorkflowRun; jobs?: Job[]; jobsError: unknown; reload: () => void; jobId?: number; attemptN?: number;
}) {
  const { owner, name, base, canPush } = useRepo();
  const nav = useNavigate();
  const [err, setErr] = useState<unknown>();
  const [flash, setFlash] = useState<string>();
  const k = kindOf(run.status, run.conclusion);
  const runBase = `${base}/actions/runs/${run.id}`;
  const wfFile = run.path.split("/").pop() || "";
  const latest = !attemptN || attemptN === run.run_attempt;
  const failed = (jobs || []).some((j) => j.conclusion === "failure" || j.conclusion === "cancelled" || j.conclusion === "timed_out");
  const act = async (what: string, fn: () => Promise<unknown>, after?: () => void) => {
    setErr(undefined);
    setFlash(undefined);
    try {
      await fn();
      setFlash(what);
      if (after) after();
      else reload();
    } catch (e) {
      setErr(e);
    }
  };
  const job = jobId ? jobs?.find((j) => j.id === jobId) : undefined;
  return (
    <div className="act-run">
      <div className="act-run-head">
        <div className="act-run-head-title">
          <Link className="muted small" to={`${base}/actions/workflows/${encodeURIComponent(wfFile)}`}>← {run.name}</Link>
          <h2 className="h3 act-run-h">
            <StatusIcon kind={k} size={20} /> <span>{run.display_title}</span> <span className="muted">#{run.run_number}</span>
          </h2>
        </div>
        <div className="act-run-actions">
          {run.run_attempt > 1 && (
            <Dropdown label={`Attempt #${attemptN || run.run_attempt}`} testid="attempt-menu">
              {(close) => Array.from({ length: run.run_attempt }, (_, i) => run.run_attempt - i).map((n) => (
                <Link key={n} role="menuitem" className="menu-item" onClick={close}
                  to={n === run.run_attempt ? runBase : `${runBase}/attempts/${n}`}>Attempt #{n}{n === run.run_attempt ? " (latest)" : ""}</Link>
              ))}
            </Dropdown>
          )}
          {canPush && latest && run.status === "completed" && (
            <Dropdown label="Re-run jobs ▾" className="btn btn-sm" testid="rerun-menu">
              {(close) => (
                <>
                  <button role="menuitem" type="button" className="menu-item" data-testid="rerun-all"
                    onClick={() => { close(); act("Re-run of all jobs requested.", () => A.rerun(owner, name, run.id)); }}>Re-run all jobs</button>
                  <button role="menuitem" type="button" className="menu-item" disabled={!failed} data-testid="rerun-failed"
                    onClick={() => { close(); act("Re-run of failed jobs requested.", () => A.rerunFailed(owner, name, run.id)); }}>Re-run failed jobs</button>
                </>
              )}
            </Dropdown>
          )}
          {canPush && run.status !== "completed" && (
            <button type="button" className="btn btn-sm" data-testid="cancel-run"
              onClick={() => act("Cancellation requested.", () => A.cancel(owner, name, run.id))}>Cancel workflow</button>
          )}
          <Dropdown label="…" ariaLabel="Show run options" testid="run-options">
            {(close) => (
              <>
                <a role="menuitem" className="menu-item" href={A.runLogsUrl(owner, name, run.id, attemptN)} onClick={close} data-testid="download-run-logs">Download log archive</a>
                <Link role="menuitem" className="menu-item" to={`${base}/blob/${run.head_sha}/${run.path}`} onClick={close}>View workflow file</Link>
                {canPush && run.status !== "completed" && (
                  <button role="menuitem" type="button" className="menu-item" onClick={() => { close(); act("Force cancellation requested.", () => A.forceCancel(owner, name, run.id)); }}>Force cancel workflow</button>
                )}
                {canPush && run.status === "completed" && (
                  <>
                    <button role="menuitem" type="button" className="menu-item" data-testid="delete-logs"
                      onClick={() => { close(); if (confirm("Are you sure you want to delete all logs of this run?")) act("The logs for this workflow run have been deleted.", () => A.deleteRunLogs(owner, name, run.id)); }}>Delete all logs</button>
                    <button role="menuitem" type="button" className="menu-item c-danger" data-testid="delete-run"
                      onClick={() => { close(); if (confirm("Are you sure you want to permanently delete this workflow run?")) act("Workflow run deleted.", () => A.deleteRun(owner, name, run.id), () => nav(`${base}/actions`)); }}>Delete workflow run</button>
                  </>
                )}
              </>
            )}
          </Dropdown>
        </div>
      </div>
      {flash && <Flash kind="success">{flash}</Flash>}
      {err !== undefined && <ErrorBox error={err} />}
      <div className="act-layout">
        <nav className="act-side" aria-label="Run">
          <NavLink end to={attemptN ? `${runBase}/attempts/${attemptN}` : runBase} className={() => "act-side-item" + (!jobId ? " selected" : "")} data-testid="run-summary-link">
            Summary
          </NavLink>
          <h3 className="act-side-heading">Jobs</h3>
          {jobsError !== undefined && <ErrorBox error={jobsError} />}
          {!jobs && jobsError === undefined && <Spinner />}
          {(jobs || []).map((j) => (
            <NavLink key={j.id} to={`${runBase}/job/${j.id}`} className={() => "act-side-item" + (j.id === jobId ? " selected" : "")} data-testid={`job-link-${j.name}`}>
              <StatusIcon kind={kindOf(j.status, j.conclusion)} /> <span className="act-truncate">{j.name}</span>
            </NavLink>
          ))}
          <h3 className="act-side-heading">Run details</h3>
          <Link className="act-side-item" to={`${base}/blob/${run.head_sha}/${run.path}`}>Workflow file</Link>
        </nav>
        <div className="act-main">
          {jobId ? (
            job ? <JobView run={run} job={job} onRerun={() => act("Re-run of the job requested.", () => A.rerunJob(owner, name, job.id))} /> : jobs ? <NotFoundView /> : <Spinner />
          ) : (
            <RunSummary run={run} jobs={jobs || []} reload={reload} />
          )}
        </div>
      </div>
    </div>
  );
}

// ---------- summary ----------

function RunSummary({ run, jobs, reload }: { run: WorkflowRun; jobs: Job[]; reload: () => void }) {
  const { owner, name, base } = useRepo();
  const k = kindOf(run.status, run.conclusion);
  const arts = useAsync(() => A.runArtifacts(owner, name, run.id), [owner, name, run.id, run.status]);
  const actor = run.triggering_actor || run.actor;
  return (
    <>
      <div className="box act-summary-box">
        <div className="act-summary-grid">
          <div>
            <div className="muted small">Triggered via {run.event} <RelTime iso={run.created_at} /></div>
            <div className="act-summary-who">
              {actor && <UserLink user={actor} />}{" "}
              {run.event === "push" ? "pushed" : "ran"}{" "}
              <Link className="mono" to={`${base}/commit/${run.head_sha}`}>{shortSha(run.head_sha)}</Link>{" "}
              {run.head_branch && <span className="branch-name">{run.head_branch}</span>}
            </div>
          </div>
          <div>
            <div className="muted small">Status</div>
            <div className="act-summary-val" data-testid="run-status">{kindLabel(k)}</div>
          </div>
          <div>
            <div className="muted small">Total duration</div>
            <div className="act-summary-val">{duration(run.run_started_at || run.created_at, run.status === "completed" ? run.updated_at : undefined) || "–"}</div>
          </div>
          <div>
            <div className="muted small">Artifacts</div>
            <div className="act-summary-val">{arts.data ? arts.data.total_count : "–"}</div>
          </div>
        </div>
      </div>
      {run.status === "waiting" && <PendingReviews run={run} onDone={reload} />}
      <JobGraph run={run} jobs={jobs} />
      <Annotations jobs={jobs} />
      <JobSummaries jobs={jobs} />
      <ArtifactsBox run={run} state={arts} />
    </>
  );
}

/** Columns by `needs` depth with connectors, like GitHub's run graph. */
function JobGraph({ run, jobs }: { run: WorkflowRun; jobs: Job[] }) {
  const { owner, name, base } = useRepo();
  const src = useAsync(() => A.workflowFile(owner, name, run.path, run.head_sha).catch(() => ""), [owner, name, run.path, run.head_sha]);
  const layout = useMemo(() => {
    const info = workflowInfo(src.data || "");
    // map each run job to its workflow job id (matrix jobs are "name (a, b)")
    const idOf = (j: Job) =>
      info.jobs.find((w) => j.name === w.name || j.name === w.id)?.id ??
      info.jobs.find((w) => j.name.startsWith(`${w.name} (`) || j.name.startsWith(`${w.id} (`))?.id ?? j.name;
    const needs = new Map(info.jobs.map((w) => [w.id, w.needs]));
    const depth = new Map<string, number>();
    const d = (id: string, seen = new Set<string>()): number => {
      if (depth.has(id)) return depth.get(id)!;
      if (seen.has(id)) return 0;
      seen.add(id);
      const v = Math.max(-1, ...(needs.get(id) || []).map((n) => d(n, seen))) + 1;
      depth.set(id, v);
      return v;
    };
    const cols: { id: string; jobs: Job[] }[][] = [];
    const groups = new Map<string, Job[]>();
    for (const j of jobs) {
      const id = idOf(j);
      if (!groups.has(id)) groups.set(id, []);
      groups.get(id)!.push(j);
    }
    for (const [id, js] of groups) {
      const c = d(id);
      (cols[c] ||= []).push({ id, jobs: js });
    }
    return { cols: cols.filter(Boolean), needs };
  }, [src.data, jobs]);
  if (!jobs.length) return null;
  const W = 220, GAP = 56, H = 38, VG = 12;
  const pos = new Map<string, { x: number; y: number; h: number }>();
  layout.cols.forEach((col, ci) => {
    let y = 0;
    for (const g of col) {
      const h = g.jobs.length * H + (g.jobs.length - 1) * 4;
      pos.set(g.id, { x: ci * (W + GAP), y, h });
      y += h + VG;
    }
  });
  const height = Math.max(...[...pos.values()].map((p) => p.y + p.h), H);
  const width = layout.cols.length * W + (layout.cols.length - 1) * GAP;
  const edges: string[] = [];
  for (const [id, p] of pos) {
    for (const n of layout.needs.get(id) || []) {
      const q = pos.get(n);
      if (!q) continue;
      const x1 = q.x + W, y1 = q.y + q.h / 2, x2 = p.x, y2 = p.y + p.h / 2, mx = (x1 + x2) / 2;
      edges.push(`M${x1} ${y1} C${mx} ${y1} ${mx} ${y2} ${x2} ${y2}`);
    }
  }
  return (
    <div className="box act-graph-box">
      <div className="box-header"><strong>{run.path.split("/").pop()}</strong> <span className="muted small">on: {run.event}</span></div>
      <div className="act-graph-scroll">
        <div className="act-graph" style={{ width, height }} data-testid="job-graph">
          <svg className="act-graph-edges" width={width} height={height} aria-hidden="true">
            {edges.map((e, i) => <path key={i} d={e} />)}
          </svg>
          {[...pos.entries()].map(([id, p]) => {
            const g = layout.cols.flat().find((x) => x.id === id)!;
            return (
              <div key={id} className="act-graph-group" style={{ left: p.x, top: p.y, width: W }}>
                {g.jobs.map((j) => (
                  <Link key={j.id} className="act-graph-job" to={`${base}/actions/runs/${run.id}/job/${j.id}`} style={{ height: H }}>
                    <StatusIcon kind={kindOf(j.status, j.conclusion)} />
                    <span className="act-truncate grow">{j.name}</span>
                    <span className="muted small">{duration(j.started_at, j.completed_at)}</span>
                  </Link>
                ))}
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}

function Annotations({ jobs }: { jobs: Job[] }) {
  const { owner, name } = useRepo();
  const done = jobs.filter((j) => j.status === "completed");
  const key = done.map((j) => j.id).join(",");
  const st = useAsync(
    () => Promise.all(done.map((j) => A.annotations(owner, name, j.id).then((a) => a.map((x) => ({ job: j, a: x }))).catch(() => []))).then((x) => x.flat()),
    [owner, name, key],
  );
  const list = st.data || [];
  if (!list.length) return null;
  const counts = { failure: 0, warning: 0, notice: 0 } as Record<string, number>;
  for (const x of list) counts[x.a.annotation_level] = (counts[x.a.annotation_level] || 0) + 1;
  return (
    <div className="box" data-testid="annotations">
      <div className="box-header">
        <strong>Annotations</strong>{" "}
        <span className="muted small">
          {[["failure", "error"], ["warning", "warning"], ["notice", "notice"]].filter(([l]) => counts[l]).map(([l, word]) => `${counts[l]} ${word}${counts[l] === 1 ? "" : "s"}`).join(" and ")}
        </span>
      </div>
      <ul className="list">
        {list.map(({ job, a }, i) => (
          <li key={i} className="list-row act-annotation">
            <span className={`act-ann act-ann-${a.annotation_level}`} aria-label={a.annotation_level}>{a.annotation_level === "failure" ? "✕" : a.annotation_level === "warning" ? "!" : "i"}</span>
            <div className="grow">
              <strong>{job.name}</strong>{a.title ? `: ${a.title}` : ""}
              <div>{a.message}</div>
              {a.path && <div className="muted small mono">{a.path}{a.start_line ? `#L${a.start_line}` : ""}</div>}
            </div>
          </li>
        ))}
      </ul>
    </div>
  );
}

/** GITHUB_STEP_SUMMARY of each finished job (stored as its check run's output text). */
function JobSummaries({ jobs }: { jobs: Job[] }) {
  const { owner, name, repo } = useRepo();
  const done = jobs.filter((j) => j.status === "completed");
  const key = done.map((j) => j.id).join(",");
  const st = useAsync(
    () => Promise.all(done.map((j) => A.checkRun(owner, name, j.id).then((c) => ({ job: j, text: c.output?.text || "" })).catch(() => ({ job: j, text: "" })))),
    [owner, name, key],
  );
  return (
    <>
      {(st.data || []).filter((x) => x.text.trim()).map(({ job, text }) => (
        <div key={job.id} className="box" data-testid={`job-summary-${job.name}`}>
          <div className="box-header"><strong>{job.name} summary</strong></div>
          <div className="box-body"><Markdown source={text} repoPath={repo.full_name} /></div>
        </div>
      ))}
    </>
  );
}

function ArtifactsBox({ run, state }: { run: WorkflowRun; state: { data?: { artifacts: Artifact[] }; error: unknown; reload: () => void } }) {
  const { owner, name, canPush } = useRepo();
  const [err, setErr] = useState<unknown>();
  const list = state.data?.artifacts || [];
  return (
    <div className="box" data-testid="artifacts">
      <div className="box-header">
        <strong>Artifacts</strong>
        <div className="muted small">Produced during runtime</div>
      </div>
      {state.error !== undefined && <ErrorBox error={state.error} />}
      {err !== undefined && <ErrorBox error={err} />}
      {list.length === 0 ? (
        <div className="box-body muted small">{run.status === "completed" ? "No artifacts were produced by this run." : "Artifacts appear here when jobs upload them."}</div>
      ) : (
        <table className="act-table">
          <thead><tr><th>Name</th><th>Size</th><th><span className="sr-only">Actions</span></th></tr></thead>
          <tbody>
            {list.map((a) => (
              <tr key={a.id}>
                <td>
                  {a.expired ? <span className="muted">{a.name}</span> : <a href={A.artifactZipUrl(owner, name, a.id)} download={`${a.name}.zip`} data-testid={`artifact-${a.name}`}>{a.name}</a>}
                  {a.expired && <span className="badge">Expired</span>}
                </td>
                <td className="muted">{bytes(a.size_in_bytes)}</td>
                <td className="act-right">
                  {canPush && (
                    <button type="button" className="btn btn-sm btn-icon" aria-label={`Delete artifact ${a.name}`} data-testid={`delete-artifact-${a.name}`}
                      onClick={async () => {
                        if (!confirm(`Delete artifact ${a.name}?`)) return;
                        try {
                          await A.deleteArtifact(owner, name, a.id);
                          state.reload();
                        } catch (e) {
                          setErr(e);
                        }
                      }}><TrashIcon /></button>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}

function PendingReviews({ run, onDone }: { run: WorkflowRun; onDone: () => void }) {
  const { owner, name } = useRepo();
  const st = useAsync(() => A.pendingDeployments(owner, name, run.id), [owner, name, run.id, run.updated_at]);
  const [open, setOpen] = useState(false);
  const list = st.data || [];
  if (!list.length) return null;
  const canApprove = list.some((p) => p.current_user_can_approve);
  return (
    <div className="box act-review" data-testid="pending-deployments">
      <div className="box-body act-review-row">
        <div>
          <strong>{list.length === 1 ? `${list[0].environment.name} needs review` : `${list.length} environments need review`}</strong>
          <div className="muted small">
            {list.map((p) => (
              <Fragment key={p.environment.id}>
                {p.environment.name}: reviewers {p.reviewers.map((r) => r.reviewer.login || r.reviewer.slug || r.reviewer.name).join(", ") || "—"}
                {p.wait_timer ? ` · wait timer ${p.wait_timer} min` : ""}
                <br />
              </Fragment>
            ))}
          </div>
        </div>
        {canApprove && <button type="button" className="btn btn-sm" onClick={() => setOpen(!open)} data-testid="review-deployments">Review deployments</button>}
      </div>
      {open && <ReviewForm run={run} list={list} onDone={() => { setOpen(false); st.reload(); onDone(); }} />}
    </div>
  );
}

function ReviewForm({ run, list, onDone }: { run: WorkflowRun; list: PendingDeployment[]; onDone: () => void }) {
  const { owner, name } = useRepo();
  const mine = list.filter((p) => p.current_user_can_approve);
  const [sel, setSel] = useState<number[]>(mine.map((p) => p.environment.id));
  const [comment, setComment] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<unknown>();
  const submit = async (state: "approved" | "rejected", e?: FormEvent) => {
    e?.preventDefault();
    setBusy(true);
    setErr(undefined);
    try {
      await A.reviewDeployments(owner, name, run.id, { environment_ids: sel, state, comment });
      onDone();
    } catch (e2) {
      setErr(e2);
    } finally {
      setBusy(false);
    }
  };
  return (
    <form className="box-body act-review-form" onSubmit={(e) => submit("approved", e)}>
      <p className="muted small">Review pending deployments for this run.</p>
      {mine.map((p) => (
        <label key={p.environment.id} className="check-row">
          <input type="checkbox" checked={sel.includes(p.environment.id)}
            onChange={(e) => setSel(e.target.checked ? [...sel, p.environment.id] : sel.filter((x) => x !== p.environment.id))} />
          <span><strong>{p.environment.name}</strong></span>
        </label>
      ))}
      <div className="form-group">
        <label htmlFor="review-comment">Leave a comment</label>
        <textarea id="review-comment" className="input textarea" rows={3} value={comment} onChange={(e) => setComment(e.target.value)} />
      </div>
      {err !== undefined && <ErrorBox error={err} />}
      <div className="form-actions">
        <button type="button" className="btn" disabled={busy || !sel.length} onClick={() => submit("rejected")} data-testid="reject-deployments">Reject</button>
        <button type="submit" className="btn btn-primary" disabled={busy || !sel.length} data-testid="approve-deployments">Approve and deploy</button>
      </div>
    </form>
  );
}

// ---------- job logs ----------

function JobView({ run, job, onRerun }: { run: WorkflowRun; job: Job; onRerun: () => void }) {
  const { owner, name, canPush } = useRepo();
  const k = kindOf(job.status, job.conclusion);
  const logs = useAsync(() => (job.started_at ? A.stepLogs(owner, name, job.id) : Promise.resolve({ job_id: job.id, status: job.status, steps: [] })), [owner, name, job.id, !!job.started_at]);
  usePoll(logs.reload, 2000, job.status !== "completed" && !!job.started_at);
  // one more fetch once the job completes (its log is compacted)
  useEffect(() => {
    if (job.status === "completed") logs.reload();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [job.status]);
  const [search, setSearch] = useState("");
  const byStep = new Map((logs.data?.steps || []).map((s) => [s.number, s.log]));
  const verb = k === "success" ? "succeeded" : k === "failure" ? "failed" : k === "cancelled" ? "was cancelled" : k === "skipped" ? "was skipped" : k === "in_progress" ? "started" : "is queued";
  return (
    <div className="box act-job" data-testid="job-view">
      <div className="box-header act-job-head">
        <div>
          <h3 className="h4 act-job-title">{job.name}</h3>
          <div className="muted small">
            {verb} {job.status === "completed" ? <RelTime iso={job.completed_at} /> : job.started_at ? <RelTime iso={job.started_at} /> : null}
            {job.started_at && <> in {duration(job.started_at, job.completed_at)}</>}
            {job.runner_name && <> · runner {job.runner_name}</>}
          </div>
        </div>
        <div className="act-job-tools">
          <input className="input input-sm" placeholder="Search logs" value={search} onChange={(e) => setSearch(e.target.value)} aria-label="Search logs" />
          {canPush && run.status === "completed" && run.run_attempt === job.run_attempt && (
            <button type="button" className="btn btn-sm" onClick={onRerun} data-testid="rerun-job">Re-run this job</button>
          )}
          <Dropdown label="⚙" ariaLabel="Job options">
            {(close) => (
              <>
                <a role="menuitem" className="menu-item" href={A.jobLogsUrl(owner, name, job.id)} target="_blank" rel="noopener" onClick={close} data-testid="raw-logs">View raw logs</a>
                <a role="menuitem" className="menu-item" href={A.jobLogsUrl(owner, name, job.id)} download={`${job.name}.txt`} onClick={close}>Download log archive</a>
              </>
            )}
          </Dropdown>
        </div>
      </div>
      {logs.error !== undefined && <ErrorBox error={logs.error} onRetry={logs.reload} />}
      {!job.started_at && <div className="box-body muted">{job.status === "completed" ? "This job did not run." : "Waiting for a runner to pick up this job…"}</div>}
      <div className="act-steps">
        {(job.steps || []).map((s) => <StepRow key={s.number} step={s} log={byStep.get(s.number)} search={search} />)}
      </div>
    </div>
  );
}

function StepRow({ step, log, search }: { step: Step; log?: string; search: string }) {
  const k = kindOf(step.status, step.conclusion);
  const [open, setOpen] = useState<boolean | null>(null);
  const auto = k === "in_progress" || k === "failure";
  const isOpen = open ?? (auto || (!!search && !!log && log.toLowerCase().includes(search.toLowerCase())));
  return (
    <div className={"act-step" + (isOpen ? " open" : "")} data-testid={`step-${step.number}`}>
      <button type="button" className="act-step-head" aria-expanded={isOpen} onClick={() => setOpen(!isOpen)}>
        <span className="act-chevron" aria-hidden="true">{isOpen ? "▾" : "▸"}</span>
        <StatusIcon kind={k} />
        <span className="grow act-truncate">{step.name}</span>
        <span className="muted small">{duration(step.started_at, step.completed_at)}</span>
      </button>
      {isOpen && <LogView text={log || ""} search={search} empty={k === "skipped" ? "This step was skipped." : k === "queued" || k === "pending" ? "Waiting…" : ""} />}
    </div>
  );
}

interface LogLine {
  n: number;
  text: string;
  kind?: "group" | "warning" | "error" | "notice" | "command" | "debug";
  children?: LogLine[];
}

// eslint-disable-next-line no-control-regex
const ANSI = /\x1b\[[0-9;]*[A-Za-z]/g;

function parseLog(text: string): LogLine[] {
  const out: LogLine[] = [];
  let group: LogLine | null = null;
  let n = 0;
  for (let raw of text.replace(/\r\n?/g, "\n").split("\n")) {
    raw = raw.replace(/^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(\.\d+)?Z ?/, "").replace(ANSI, "");
    const m = raw.match(/^##\[(group|endgroup|warning|error|notice|command|debug)\](.*)$/);
    if (m && m[1] === "endgroup") {
      group = null;
      continue;
    }
    n++;
    const line: LogLine = { n, text: m ? m[2] : raw.replace(/^\[command\]/, ""), kind: m ? (m[1] as LogLine["kind"]) : raw.startsWith("[command]") ? "command" : undefined };
    if (line.kind === "group") {
      line.children = [];
      out.push(line);
      group = line;
    } else if (group) group.children!.push(line);
    else out.push(line);
  }
  while (out.length && out[out.length - 1].text === "" && !out[out.length - 1].children) out.pop();
  return out;
}

function LogView({ text, search, empty }: { text: string; search: string; empty: string }) {
  const lines = useMemo(() => parseLog(text), [text]);
  if (!lines.length) return <div className="act-log act-log-empty muted small">{empty || "No output."}</div>;
  return (
    <div className="act-log" role="log">
      {lines.map((l) => (l.children ? <LogGroup key={l.n} line={l} search={search} /> : <LogRow key={l.n} line={l} search={search} />))}
    </div>
  );
}

function LogGroup({ line, search }: { line: LogLine; search: string }) {
  const [open, setOpen] = useState(false);
  const hit = !!search && line.children!.some((c) => c.text.toLowerCase().includes(search.toLowerCase()));
  const isOpen = open || hit;
  return (
    <>
      <div className="act-log-line act-log-group" onClick={() => setOpen(!isOpen)} role="button" aria-expanded={isOpen} tabIndex={0}
        onKeyDown={(e) => (e.key === "Enter" || e.key === " ") && setOpen(!isOpen)}>
        <span className="act-log-n">{line.n}</span>
        <span className="act-log-t"><span aria-hidden="true">{isOpen ? "▾ " : "▸ "}</span>{line.text}</span>
      </div>
      {isOpen && line.children!.map((c) => <LogRow key={c.n} line={c} search={search} />)}
    </>
  );
}

function LogRow({ line, search }: { line: LogLine; search: string }) {
  const t = line.text;
  let body: React.ReactNode = t;
  if (search) {
    const i = t.toLowerCase().indexOf(search.toLowerCase());
    if (i >= 0) body = <>{t.slice(0, i)}<mark>{t.slice(i, i + search.length)}</mark>{t.slice(i + search.length)}</>;
  }
  return (
    <div className={"act-log-line" + (line.kind ? ` act-log-${line.kind}` : "")}>
      <span className="act-log-n">{line.n}</span>
      <span className="act-log-t">
        {line.kind === "warning" && <strong>Warning: </strong>}
        {line.kind === "error" && <strong>Error: </strong>}
        {line.kind === "notice" && <strong>Notice: </strong>}
        {body}
      </span>
    </div>
  );
}
