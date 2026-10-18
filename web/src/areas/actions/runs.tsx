// The Actions tab: workflows sidebar, runs list with filters, workflow_dispatch form,
// enable/disable, caches and the repository's runners.
import { useMemo, useState, type FormEvent } from "react";
import { Link, NavLink, Route, Routes, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { api } from "../../api";
import { Async, Blankslate, ErrorBox, Flash, NotFoundView, Pagination, RelTime, Spinner } from "../../components/common";
import { useRepo } from "../../components/RepoLayout";
import { useAsync, useTitle } from "../../hooks";
import { actionsApi as A, type Runner, type Workflow, type WorkflowRun } from "./api";
import { RunPage } from "./run";
import { Dropdown, EVENTS, STATUSES, StatusIcon, bytes, duration, eventVerb, kindOf, usePoll } from "./ui";
import { workflowInfo, type DispatchInput } from "./yaml";

const fileOf = (w: Workflow) => w.path.split("/").pop() || w.path;

/** `/{o}/{r}/actions/*` */
export function ActionsRoot() {
  return (
    <Routes>
      <Route index element={<ActionsList />} />
      <Route path="workflows/:file" element={<ActionsList />} />
      <Route path="runs/:runId" element={<RunPage />} />
      <Route path="runs/:runId/attempts/:attempt" element={<RunPage />} />
      <Route path="runs/:runId/job/:jobId" element={<RunPage />} />
      <Route path="caches" element={<ActionsShell active="caches"><CachesPage /></ActionsShell>} />
      <Route path="runners" element={<ActionsShell active="runners"><RepoRunnersPage /></ActionsShell>} />
      <Route path="*" element={<NotFoundView />} />
    </Routes>
  );
}

function ActionsShell({ active, children }: { active: string; children: React.ReactNode }) {
  const { owner, name, base } = useRepo();
  const wfs = useAsync(() => A.workflows(owner, name), [owner, name]);
  const [open, setOpen] = useState(false);
  return (
    <div className="act-layout">
      <nav className="act-side" aria-label="Workflows">
        <div className="act-side-head">
          <h2 className="h4">Actions</h2>
          <button type="button" className="btn btn-sm act-side-toggle" aria-expanded={open} onClick={() => setOpen(!open)}>
            {open ? "Hide workflows" : "Show workflows"}
          </button>
        </div>
        <div className={"act-side-body" + (open ? " open" : "")}>
          <NavLink end to={`${base}/actions`} className={() => "act-side-item" + (active === "all" ? " selected" : "")} data-testid="wf-all">
            All workflows
          </NavLink>
          {wfs.error !== undefined && <ErrorBox error={wfs.error} />}
          {(wfs.data?.workflows || []).map((w) => (
            <NavLink key={w.id} to={`${base}/actions/workflows/${encodeURIComponent(fileOf(w))}`}
              className={() => "act-side-item" + (active === `wf:${fileOf(w)}` ? " selected" : "")} data-testid={`wf-${fileOf(w)}`}>
              <span className="act-truncate">{w.name}</span>
              {w.state !== "active" && <span className="badge">Disabled</span>}
            </NavLink>
          ))}
          <h3 className="act-side-heading">Management</h3>
          <NavLink to={`${base}/actions/caches`} className={() => "act-side-item" + (active === "caches" ? " selected" : "")}>Caches</NavLink>
          <NavLink to={`${base}/actions/runners`} className={() => "act-side-item" + (active === "runners" ? " selected" : "")}>Runners</NavLink>
        </div>
      </nav>
      <div className="act-main">{children}</div>
    </div>
  );
}

function ActionsList() {
  const { file } = useParams();
  const { owner, name, repo } = useRepo();
  const wfs = useAsync(() => A.workflows(owner, name), [owner, name]);
  useTitle(`Actions · ${repo.full_name}`);
  const wf = file ? wfs.data?.workflows.find((w) => fileOf(w) === file || String(w.id) === file) : undefined;
  return (
    <ActionsShell active={file ? `wf:${file}` : "all"}>
      {file && wfs.data && !wf ? <NotFoundView /> : file && !wfs.data ? <Spinner /> : <RunsPanel workflow={wf} reloadWorkflows={wfs.reload} hasWorkflows={!!wfs.data?.workflows.length} />}
    </ActionsShell>
  );
}

function RunsPanel({ workflow, reloadWorkflows, hasWorkflows }: { workflow?: Workflow; reloadWorkflows: () => void; hasWorkflows: boolean }) {
  const { owner, name, base, canPush, repo } = useRepo();
  const [sp, setSp] = useSearchParams();
  const page = Number(sp.get("page") || 1);
  const q = { event: sp.get("event") || "", status: sp.get("status") || "", branch: sp.get("branch") || "", actor: sp.get("actor") || "" };
  const runs = useAsync(() => A.runs(owner, name, { workflow: workflow?.id, ...q, page }), [owner, name, workflow?.id, q.event, q.status, q.branch, q.actor, page]);
  const active = (runs.data?.items.workflow_runs || []).some((r) => r.status !== "completed");
  usePoll(runs.reload, 3000, active);
  const [flash, setFlash] = useState<string>();
  const [err, setErr] = useState<unknown>();
  const setFilter = (k: string, v: string) => {
    const n = new URLSearchParams(sp);
    if (v) n.set(k, v);
    else n.delete(k);
    n.delete("page");
    setSp(n);
  };
  const toggle = async () => {
    if (!workflow) return;
    setErr(undefined);
    try {
      if (workflow.state === "active") await A.disableWorkflow(owner, name, workflow.id);
      else await A.enableWorkflow(owner, name, workflow.id);
      setFlash(workflow.state === "active" ? "Workflow disabled successfully." : "Workflow enabled successfully.");
      reloadWorkflows();
    } catch (e) {
      setErr(e);
    }
  };
  const total = runs.data?.items.total_count ?? 0;
  const filtered = Object.values(q).some(Boolean);
  return (
    <>
      <div className="act-head">
        <div className="act-head-title">
          <h2 className="h3">{workflow ? workflow.name : "All workflows"}</h2>
          {workflow && (
            <Link className="muted small mono" to={`${base}/blob/${repo.default_branch}/${workflow.path}`}>{fileOf(workflow)}</Link>
          )}
        </div>
        {workflow && canPush && (
          <Dropdown label="…" ariaLabel="Show workflow options" testid="workflow-menu">
            {(close) => (
              <>
                <Link role="menuitem" className="menu-item" to={`${base}/blob/${repo.default_branch}/${workflow.path}`}>View workflow file</Link>
                <button role="menuitem" type="button" className="menu-item" onClick={() => { close(); toggle(); }} data-testid="workflow-toggle">
                  {workflow.state === "active" ? "Disable workflow" : "Enable workflow"}
                </button>
              </>
            )}
          </Dropdown>
        )}
      </div>
      {flash && <Flash kind="success">{flash}</Flash>}
      {err !== undefined && <ErrorBox error={err} />}
      {workflow && workflow.state !== "active" && <Flash kind="warn">This workflow was disabled manually.</Flash>}
      {workflow && <DispatchBox workflow={workflow} onDone={(id) => { setFlash(id ? `Workflow run was successfully requested.` : "Workflow run was successfully requested."); runs.reload(); }} />}
      <div className="box">
        <div className="box-header act-runs-header">
          <strong data-testid="runs-count">{total} workflow run{total === 1 ? "" : "s"}</strong>
          <div className="act-filters">
            <FilterSelect label="Event" value={q.event} options={EVENTS} onChange={(v) => setFilter("event", v)} />
            <FilterSelect label="Status" value={q.status} options={STATUSES} onChange={(v) => setFilter("status", v)} />
            <FilterText label="Branch" value={q.branch} onChange={(v) => setFilter("branch", v)} />
            <FilterText label="Actor" value={q.actor} onChange={(v) => setFilter("actor", v)} />
          </div>
        </div>
        <Async state={runs}>
          {(p) =>
            p.items.workflow_runs.length === 0 ? (
              filtered ? (
                <Blankslate title="No workflow runs match your filters">
                  <button type="button" className="btn-link" onClick={() => setSp(new URLSearchParams())}>Clear filters</button>
                </Blankslate>
              ) : (
                <Blankslate title={hasWorkflows ? "There are no workflow runs yet." : "Get started with GitHub Actions"}>
                  {hasWorkflows ? "Runs appear here when a workflow is triggered." : (
                    <>Add a workflow file under <code>.github/workflows/</code> (for example <code>ci.yml</code>) and push it to the default branch.</>
                  )}
                </Blankslate>
              )
            ) : (
              <ul className="list act-runs" data-testid="runs-list">
                {p.items.workflow_runs.map((r) => (
                  <RunRow key={r.id} run={r} showWorkflow={!workflow} onDeleted={runs.reload} onError={setErr} />
                ))}
              </ul>
            )
          }
        </Async>
      </div>
      {runs.data && (
        <Pagination page={page} next={runs.data.next} prev={runs.data.prev} last={runs.data.last}
          onPage={(n) => { const s = new URLSearchParams(sp); s.set("page", String(n)); setSp(s); }} />
      )}
    </>
  );
}

function FilterSelect({ label, value, options, onChange }: { label: string; value: string; options: string[]; onChange: (v: string) => void }) {
  return (
    <label className="act-filter">
      <span className="sr-only">Filter by {label.toLowerCase()}</span>
      <select className="input input-sm" value={value} onChange={(e) => onChange(e.target.value)} aria-label={`Filter by ${label.toLowerCase()}`}
        data-testid={`filter-${label.toLowerCase()}`}>
        <option value="">{label}</option>
        {options.map((o) => <option key={o} value={o}>{o}</option>)}
      </select>
    </label>
  );
}

function FilterText({ label, value, onChange }: { label: string; value: string; onChange: (v: string) => void }) {
  const [v, setV] = useState(value);
  return (
    <form className="act-filter" onSubmit={(e) => { e.preventDefault(); onChange(v.trim()); }}>
      <input className="input input-sm" placeholder={label} value={v} onChange={(e) => setV(e.target.value)}
        onBlur={() => v.trim() !== value && onChange(v.trim())} aria-label={`Filter by ${label.toLowerCase()}`} data-testid={`filter-${label.toLowerCase()}`} />
    </form>
  );
}

function RunRow({ run, showWorkflow, onDeleted, onError }: { run: WorkflowRun; showWorkflow: boolean; onDeleted: () => void; onError: (e: unknown) => void }) {
  const { owner, name, base, canPush } = useRepo();
  const k = kindOf(run.status, run.conclusion);
  const del = async () => {
    if (!confirm("Are you sure you want to permanently delete this workflow run?")) return;
    try {
      await A.deleteRun(owner, name, run.id);
      onDeleted();
    } catch (e) {
      onError(e);
    }
  };
  return (
    <li className="list-row act-run-row" data-testid={`run-${run.id}`}>
      <StatusIcon kind={k} />
      <div className="act-run-main">
        <Link className="act-run-title" to={`${base}/actions/runs/${run.id}`}>{run.display_title || run.name}</Link>
        <div className="muted small">
          {showWorkflow && <strong>{run.name}</strong>} #{run.run_number}: {eventVerb(run.event)}{" "}
          {run.event !== "schedule" && (run.triggering_actor || run.actor) ? <Link className="muted-link" to={`/${(run.triggering_actor || run.actor)!.login}`}>{(run.triggering_actor || run.actor)!.login}</Link> : null}
          {run.run_attempt > 1 && <> · attempt {run.run_attempt}</>}
        </div>
      </div>
      <div className="act-run-meta small muted">
        {run.head_branch && <span className="branch-name act-truncate">{run.head_branch}</span>}
        <span><RelTime iso={run.run_started_at || run.created_at} /></span>
        <span>{run.status === "completed" ? duration(run.run_started_at || run.created_at, run.updated_at) : kindOf(run.status) === "in_progress" ? "In progress" : run.status}</span>
      </div>
      {canPush && (
        <Dropdown label="…" ariaLabel="Show options" testid={`run-menu-${run.id}`}>
          {(close) => (
            <>
              <Link role="menuitem" className="menu-item" to={`${base}/actions/runs/${run.id}`}>View workflow run</Link>
              <button role="menuitem" type="button" className="menu-item c-danger" disabled={run.status !== "completed"}
                onClick={() => { close(); del(); }}>Delete workflow run</button>
            </>
          )}
        </Dropdown>
      )}
    </li>
  );
}

/** "This workflow has a workflow_dispatch event trigger." + the Run workflow form. */
function DispatchBox({ workflow, onDone }: { workflow: Workflow; onDone: (runId?: number) => void }) {
  const { owner, name, repo, canPush } = useRepo();
  const [ref, setRef] = useState(repo.default_branch);
  const src = useAsync(() => A.workflowFile(owner, name, workflow.path, ref).catch(() => ""), [owner, name, workflow.path, ref]);
  const info = useMemo(() => workflowInfo(src.data || ""), [src.data]);
  const [open, setOpen] = useState(false);
  if (!src.data || !info.dispatch) return null;
  return (
    <div className="box act-dispatch">
      <div className="box-body act-dispatch-row">
        <span>This workflow has a <code>workflow_dispatch</code> event trigger.</span>
        {canPush && workflow.state === "active" && (
          <button type="button" className="btn btn-sm" aria-expanded={open} onClick={() => setOpen(!open)} data-testid="run-workflow">
            Run workflow ▾
          </button>
        )}
      </div>
      {open && <DispatchForm workflow={workflow} inputs={info.inputs} refName={ref} setRef={setRef} onDone={(id) => { setOpen(false); onDone(id); }} />}
    </div>
  );
}

function DispatchForm({ workflow, inputs, refName, setRef, onDone }: {
  workflow: Workflow; inputs: DispatchInput[]; refName: string; setRef: (r: string) => void; onDone: (runId?: number) => void;
}) {
  const { owner, name } = useRepo();
  const branches = useAsync(() => api.branches(owner, name), [owner, name]);
  const envs = useAsync(() => (inputs.some((i) => i.type === "environment") ? A.environments(owner, name).then((e) => e.environments.map((x) => x.name)) : Promise.resolve([])), [owner, name, inputs.length]);
  const [vals, setVals] = useState<Record<string, string | boolean>>(() =>
    Object.fromEntries(inputs.map((i) => [i.name, i.type === "choice" && !i.default ? i.options[0] || "" : i.default])),
  );
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<unknown>();
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setErr(undefined);
    try {
      const r = await A.dispatch(owner, name, workflow.id, refName, vals);
      onDone(r?.workflow_run_id);
    } catch (e2) {
      setErr(e2);
    } finally {
      setBusy(false);
    }
  };
  return (
    <form className="box-body act-dispatch-form" onSubmit={submit} data-testid="dispatch-form">
      <div className="form-group">
        <label htmlFor="dispatch-ref">Use workflow from</label>
        <select id="dispatch-ref" className="input" value={refName} onChange={(e) => setRef(e.target.value)}>
          {(branches.data || [{ name: refName }]).map((b) => <option key={b.name} value={b.name}>Branch: {b.name}</option>)}
        </select>
      </div>
      {inputs.map((i) => (
        <div className="form-group" key={i.name}>
          {i.type === "boolean" ? (
            <label className="check-row">
              <input type="checkbox" checked={!!vals[i.name]} onChange={(e) => setVals({ ...vals, [i.name]: e.target.checked })} data-testid={`input-${i.name}`} />
              <span>{i.description || i.name}</span>
            </label>
          ) : (
            <>
              <label htmlFor={`input-${i.name}`}>{i.description || i.name}{i.required && <span className="c-danger"> *</span>}</label>
              {i.type === "choice" || i.type === "environment" ? (
                <select id={`input-${i.name}`} className="input" value={String(vals[i.name] ?? "")} data-testid={`input-${i.name}`}
                  onChange={(e) => setVals({ ...vals, [i.name]: e.target.value })}>
                  {i.type === "environment" && <option value="">Select an environment</option>}
                  {(i.type === "choice" ? i.options : envs.data || []).map((o) => <option key={o} value={o}>{o}</option>)}
                </select>
              ) : (
                <input id={`input-${i.name}`} className="input" type={i.type === "number" ? "number" : "text"} required={i.required}
                  value={String(vals[i.name] ?? "")} onChange={(e) => setVals({ ...vals, [i.name]: e.target.value })} data-testid={`input-${i.name}`} />
              )}
            </>
          )}
        </div>
      ))}
      {err !== undefined && <ErrorBox error={err} />}
      <button type="submit" className="btn btn-primary btn-sm" disabled={busy} data-testid="dispatch-submit">{busy ? "Requesting…" : "Run workflow"}</button>
    </form>
  );
}

// ---------- caches ----------

function CachesPage() {
  const { owner, name, repo, canPush } = useRepo();
  useTitle(`Caches · ${repo.full_name}`);
  const [sp, setSp] = useSearchParams();
  const page = Number(sp.get("page") || 1);
  const sort = sp.get("sort") || "last_accessed_at";
  const [key, setKey] = useState(sp.get("key") || "");
  const st = useAsync(() => A.caches(owner, name, { page, sort, direction: "desc", key: sp.get("key") || undefined }), [owner, name, page, sort, sp.get("key")]);
  const usage = useAsync(() => A.cacheUsage(owner, name).catch(() => undefined), [owner, name]);
  const [err, setErr] = useState<unknown>();
  const del = async (id: number) => {
    if (!confirm("Are you sure you want to delete this cache?")) return;
    try {
      await A.deleteCache(owner, name, id);
      st.reload();
      usage.reload();
    } catch (e) {
      setErr(e);
    }
  };
  return (
    <>
      <div className="act-head"><h2 className="h3">Caches</h2></div>
      <p className="muted">
        Showing caches from all workflows.
        {usage.data && <> {usage.data.active_caches_count} cache{usage.data.active_caches_count === 1 ? "" : "s"} using {bytes(usage.data.active_caches_size_in_bytes)}.</>}
      </p>
      {err !== undefined && <ErrorBox error={err} />}
      <div className="box">
        <div className="box-header act-runs-header">
          <strong>{st.data?.items.total_count ?? 0} caches</strong>
          <div className="act-filters">
            <form onSubmit={(e) => { e.preventDefault(); const n = new URLSearchParams(sp); if (key) n.set("key", key); else n.delete("key"); n.delete("page"); setSp(n); }}>
              <input className="input input-sm" placeholder="Filter caches by key" value={key} onChange={(e) => setKey(e.target.value)} aria-label="Filter caches by key" />
            </form>
            <select className="input input-sm" value={sort} aria-label="Sort" onChange={(e) => { const n = new URLSearchParams(sp); n.set("sort", e.target.value); setSp(n); }}>
              <option value="last_accessed_at">Last used</option>
              <option value="created_at">Created</option>
              <option value="size_in_bytes">Size</option>
            </select>
          </div>
        </div>
        <Async state={st}>
          {(p) => p.items.actions_caches.length === 0 ? <Blankslate title="No caches">Caches appear here when a workflow uses <code>actions/cache</code>.</Blankslate> : (
            <ul className="list" data-testid="caches-list">
              {p.items.actions_caches.map((c) => (
                <li key={c.id} className="list-row act-cache-row">
                  <div className="grow">
                    <div className="mono act-break">{c.key}</div>
                    <div className="muted small">
                      {bytes(c.size_in_bytes)} cached <RelTime iso={c.created_at} /> · last used <RelTime iso={c.last_accessed_at} /> · <span className="branch-name">{c.ref.replace(/^refs\/heads\//, "")}</span>
                    </div>
                  </div>
                  {canPush && <button type="button" className="btn btn-sm btn-danger-outline" onClick={() => del(c.id)} aria-label={`Delete cache ${c.key}`}>Delete</button>}
                </li>
              ))}
            </ul>
          )}
        </Async>
      </div>
      {st.data && <Pagination page={page} next={st.data.next} prev={st.data.prev} last={st.data.last} onPage={(n) => { const s = new URLSearchParams(sp); s.set("page", String(n)); setSp(s); }} />}
    </>
  );
}

// ---------- runners (read-only view; management lives in Settings → Actions → Runners) ----------

function RepoRunnersPage() {
  const { owner, name, base, isAdmin, repo } = useRepo();
  useTitle(`Runners · ${repo.full_name}`);
  const st = useAsync(() => (isAdmin ? A.runners({ kind: "repo", owner, repo: name }).then((d) => d.runners) : Promise.resolve([] as Runner[])), [owner, name, isAdmin]);
  const nav = useNavigate();
  return (
    <>
      <div className="act-head">
        <h2 className="h3">Runners</h2>
        {isAdmin && <button type="button" className="btn btn-sm" onClick={() => nav(`${base}/settings/actions/runners`)}>Manage runners</button>}
      </div>
      {!isAdmin ? (
        <Flash>Only repository administrators can see this repository's self-hosted runners.</Flash>
      ) : (
        <Async state={st}>
          {(runners) =>
            runners.length === 0 ? (
              <Blankslate title="No runners configured">
                Self-hosted runners run your jobs. <Link to={`${base}/settings/actions/runners/new`}>New self-hosted runner</Link>
              </Blankslate>
            ) : (
              <ul className="box list">
                {runners.map((r) => (
                  <li key={r.id} className="list-row">
                    <span className={`act-dot act-dot-${r.status === "online" ? (r.busy ? "busy" : "online") : "offline"}`} aria-hidden="true" />
                    <div className="grow">
                      <strong>{r.name}</strong>
                      <div className="act-labels">{r.labels.map((l) => <span key={l.name} className="badge">{l.name}</span>)}</div>
                    </div>
                    <span className="muted small">{r.status === "online" ? (r.busy ? "Active" : "Idle") : "Offline"}</span>
                  </li>
                ))}
              </ul>
            )
          }
        </Async>
      )}
    </>
  );
}
