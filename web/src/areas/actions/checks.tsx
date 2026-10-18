// Checks and commit statuses on pull requests and commits, the PR "Checks" page, and
// deployments (Code page sidebar + /{o}/{r}/deployments).
import { useState } from "react";
import { Link, NavLink, useParams, useSearchParams } from "react-router-dom";
import { api, type Pull } from "../../api";
import { Async, Blankslate, ErrorBox, NotFoundView, Pagination, RelTime, StateBadge, UserLink } from "../../components/common";
import { useRepo, type RepoCtx } from "../../components/RepoLayout";
import { useAsync, useTitle } from "../../hooks";
import { Markdown } from "../../markdown";
import { shortSha } from "../../util";
import { actionsApi as A, type CheckRun, type CommitStatus, type Deployment, type DeploymentStatus } from "./api";
import { StatusIcon, duration, kindOf, statusKind, usePoll, type Kind } from "./ui";

/** A check run or a commit status, normalised for listing. */
interface CheckItem {
  key: string;
  kind: Kind;
  name: string;
  description: string;
  /** SPA path or absolute URL. */
  href?: string;
  external?: boolean;
}

/** Same-origin absolute URLs become SPA paths. */
function localPath(url?: string | null): { href?: string; external?: boolean } {
  if (!url) return {};
  try {
    const u = new URL(url, location.origin);
    if (u.origin === location.origin) return { href: u.pathname + u.search + u.hash };
    return { href: url, external: true };
  } catch {
    return {};
  }
}

function describe(c: CheckRun): string {
  const k = kindOf(c.status, c.conclusion);
  const d = duration(c.started_at, c.completed_at);
  switch (k) {
    case "success":
      return `Successful in ${d}`;
    case "failure":
      return `Failing after ${d}`;
    case "cancelled":
      return `Cancelled after ${d}`;
    case "skipped":
      return "Skipped";
    case "in_progress":
      return "In progress";
    default:
      return c.output?.title || c.status;
  }
}

export function checkItems(runs: CheckRun[], statuses: CommitStatus[], base: string, pull?: number): CheckItem[] {
  const out: CheckItem[] = runs.map((c) => {
    const l = localPath(c.details_url || c.html_url);
    return {
      key: `c${c.id}`,
      kind: kindOf(c.status, c.conclusion),
      name: c.app && c.app.slug !== "forge" && c.app.slug !== "github-actions" ? `${c.app.name} / ${c.name}` : c.name,
      description: describe(c),
      href: l.href && l.href.includes("/actions/runs/") ? l.href : pull ? `${base}/pull/${pull}/checks?check_run_id=${c.id}` : l.href,
      external: l.href && l.href.includes("/actions/runs/") ? false : pull ? false : l.external,
    };
  });
  for (const s of statuses) {
    const l = localPath(s.target_url);
    out.push({ key: `s${s.id}`, kind: statusKind(s.state), name: s.context, description: s.description || s.state, ...l });
  }
  return out;
}

function useChecks(owner: string, name: string, sha: string) {
  const st = useAsync(
    () => Promise.all([A.checkRuns(owner, name, sha).then((r) => r.check_runs), A.combinedStatus(owner, name, sha).then((s) => s.statuses).catch(() => [])]),
    [owner, name, sha],
  );
  const pending = (st.data?.[0] || []).some((c) => c.status !== "completed") || (st.data?.[1] || []).some((s) => s.state === "pending");
  usePoll(st.reload, 4000, pending);
  return st;
}

function overall(items: CheckItem[]): { kind: Kind; title: string } {
  if (items.some((i) => i.kind === "failure")) return { kind: "failure", title: "Some checks were not successful" };
  if (items.some((i) => ["in_progress", "queued", "pending", "waiting"].includes(i.kind))) return { kind: "pending", title: "Some checks haven't completed yet" };
  return { kind: "success", title: "All checks have passed" };
}

function counts(items: CheckItem[]): string {
  const c = new Map<string, number>();
  const word = (k: Kind) => (k === "success" ? "successful" : k === "failure" ? "failing" : k === "skipped" ? "skipped" : k === "cancelled" ? "cancelled" : k === "neutral" ? "neutral" : "pending");
  for (const i of items) c.set(word(i.kind), (c.get(word(i.kind)) || 0) + 1);
  return [...c.entries()].map(([w, n]) => `${n} ${w}`).join(", ") + ` check${items.length === 1 ? "" : "s"}`;
}

function CheckList({ items }: { items: CheckItem[] }) {
  return (
    <ul className="list act-checklist">
      {items.map((i) => (
        <li key={i.key} className="list-row act-check-row" data-testid="check-item">
          <StatusIcon kind={i.kind} />
          <div className="grow act-check-text">
            <strong className="act-truncate">{i.name}</strong> <span className="muted small">— {i.description}</span>
          </div>
          {i.href && (i.external ? <a className="small" href={i.href} rel="noopener nofollow">Details</a> : <Link className="small" to={i.href}>Details</Link>)}
        </li>
      ))}
    </ul>
  );
}

/** Slot `pull.merge.top`: the checks box above the merge button. */
export function PullChecksBox({ ctx, pull }: { ctx: RepoCtx; pull: Pull }) {
  const st = useChecks(ctx.owner, ctx.name, pull.head.sha);
  const [open, setOpen] = useState(false);
  if (st.error !== undefined) return null;
  if (!st.data) return null;
  const items = checkItems(st.data[0], st.data[1], ctx.base, pull.number);
  if (!items.length) return null;
  const o = overall(items);
  return (
    <div className={`box act-checks-box act-checks-${o.kind}`} data-testid="pr-checks">
      <div className="box-header act-checks-head">
        <StatusIcon kind={o.kind} size={20} />
        <div className="grow">
          <strong>{o.title}</strong>
          <div className="muted small">{counts(items)}</div>
        </div>
        <button type="button" className="btn btn-sm" aria-expanded={open} onClick={() => setOpen(!open)}>{open ? "Hide all checks" : "Show all checks"}</button>
        <Link className="btn btn-sm" to={`${ctx.base}/pull/${pull.number}/checks`} data-testid="pr-checks-tab">Checks</Link>
      </div>
      {open && <CheckList items={items} />}
    </div>
  );
}

/** Slot `commit.header`: checks and statuses of a commit. */
export function CommitChecks({ ctx, sha }: { ctx: RepoCtx; sha: string }) {
  const st = useChecks(ctx.owner, ctx.name, sha);
  const [open, setOpen] = useState(false);
  if (!st.data) return null;
  const items = checkItems(st.data[0], st.data[1], ctx.base);
  if (!items.length) return null;
  const o = overall(items);
  return (
    <div className="box act-commit-checks" data-testid="commit-checks">
      <button type="button" className="box-header act-checks-head act-plain" aria-expanded={open} onClick={() => setOpen(!open)}>
        <StatusIcon kind={o.kind} />
        <span className="grow"><strong>{o.title}</strong> <span className="muted small">{counts(items)}</span></span>
        <span className="muted small">{open ? "▾" : "▸"}</span>
      </button>
      {open && <CheckList items={items} />}
    </div>
  );
}

/** `/{o}/{r}/pull/{n}/checks`: every check run of the head commit with its output and annotations. */
export function PullChecksPage() {
  const { number = "" } = useParams();
  const ctx = useRepo();
  const n = Number(number);
  const pr = useAsync(() => api.pull(ctx.owner, ctx.name, n), [ctx.owner, ctx.name, n]);
  useTitle(pr.data ? `Checks · ${pr.data.title} · Pull Request #${n}` : "Checks");
  if (!Number.isFinite(n)) return <NotFoundView />;
  return (
    <Async state={pr}>
      {(p) => (
        <div className="act-prchecks">
          <h1 className="h2 act-pr-title">{p.title} <span className="muted">#{p.number}</span></h1>
          <div className="act-pr-meta"><StateBadge item={p} /> <span className="muted small"><UserLink user={p.user} /> wants to merge <span className="branch-name">{p.head.ref}</span> into <span className="branch-name">{p.base.ref}</span></span></div>
          <nav className="tabnav" aria-label="Pull request">
            <NavLink end className="tabnav-tab" to={`${ctx.base}/pull/${p.number}`}>Conversation</NavLink>
            <NavLink className="tabnav-tab" to={`${ctx.base}/pull/${p.number}/commits`}>Commits</NavLink>
            <NavLink className="tabnav-tab selected" to={`${ctx.base}/pull/${p.number}/checks`} aria-current="page">Checks</NavLink>
            <NavLink className="tabnav-tab" to={`${ctx.base}/pull/${p.number}/files`}>Files changed</NavLink>
          </nav>
          <ChecksDetail sha={p.head.sha} pull={p.number} />
        </div>
      )}
    </Async>
  );
}

function ChecksDetail({ sha, pull }: { sha: string; pull: number }) {
  const { owner, name, base, canPush } = useRepo();
  const st = useChecks(owner, name, sha);
  const [sp, setSp] = useSearchParams();
  const [err, setErr] = useState<unknown>();
  return (
    <Async state={st}>
      {([runs, statuses]) => {
        if (!runs.length && !statuses.length) return <Blankslate title="No checks">There are no checks or statuses for commit {shortSha(sha)}.</Blankslate>;
        const selId = Number(sp.get("check_run_id")) || runs[0]?.id;
        const sel = runs.find((r) => r.id === selId);
        return (
          <div className="act-layout">
            <nav className="act-side" aria-label="Checks">
              {runs.map((r) => (
                <button key={r.id} type="button" className={"act-side-item act-plain" + (r.id === sel?.id ? " selected" : "")}
                  onClick={() => setSp(new URLSearchParams({ check_run_id: String(r.id) }))} data-testid={`check-${r.name}`}>
                  <StatusIcon kind={kindOf(r.status, r.conclusion)} /> <span className="act-truncate">{r.name}</span>
                </button>
              ))}
              {statuses.length > 0 && <h3 className="act-side-heading">Commit statuses</h3>}
              {statuses.map((s) => (
                <div key={s.id} className="act-side-item">
                  <StatusIcon kind={statusKind(s.state)} /> <span className="act-truncate" title={s.description || ""}>{s.context}</span>
                </div>
              ))}
            </nav>
            <div className="act-main">
              {err !== undefined && <ErrorBox error={err} />}
              {sel && <CheckRunDetail run={sel} canPush={canPush} base={base} onRerun={async () => {
                setErr(undefined);
                try {
                  const path = localPath(sel.details_url).href || "";
                  const job = path.match(/\/actions\/runs\/\d+\/job\/(\d+)/);
                  if (job) await A.rerunJob(owner, name, Number(job[1]));
                  else await A.rerequestCheckRun(owner, name, sel.id);
                  st.reload();
                } catch (e) {
                  setErr(e);
                }
              }} />}
              {statuses.length > 0 && (
                <div className="box">
                  <div className="box-header"><strong>Commit statuses</strong></div>
                  <CheckList items={checkItems([], statuses, base, pull)} />
                </div>
              )}
            </div>
          </div>
        );
      }}
    </Async>
  );
}

function CheckRunDetail({ run, canPush, base, onRerun }: { run: CheckRun; canPush: boolean; base: string; onRerun: () => void }) {
  const { owner, name, repo } = useRepo();
  const ann = useAsync(() => (run.output?.annotations_count ? A.annotations(owner, name, run.id) : Promise.resolve([])), [owner, name, run.id, run.output?.annotations_count]);
  const l = localPath(run.details_url);
  return (
    <div className="box" data-testid="check-detail">
      <div className="box-header act-checks-head">
        <StatusIcon kind={kindOf(run.status, run.conclusion)} size={20} />
        <div className="grow">
          <strong>{run.name}</strong>
          <div className="muted small">{describe(run)}{run.app && <> · {run.app.name}</>}</div>
        </div>
        {canPush && run.status === "completed" && <button type="button" className="btn btn-sm" onClick={onRerun} data-testid="check-rerun">Re-run</button>}
        {l.href && (l.external ? <a className="btn btn-sm" href={l.href} rel="noopener nofollow">Details</a> : <Link className="btn btn-sm" to={l.href}>View details</Link>)}
      </div>
      <div className="box-body">
        {run.output?.title && <h3 className="h4">{run.output.title}</h3>}
        {run.output?.summary && <Markdown source={run.output.summary} repoPath={repo.full_name} />}
        {run.output?.text && <Markdown source={run.output.text} repoPath={repo.full_name} />}
        {!run.output?.summary && !run.output?.text && <p className="muted">No output.</p>}
        {(ann.data || []).length > 0 && (
          <>
            <h4 className="h5">Annotations</h4>
            <ul className="list box">
              {ann.data!.map((a, i) => (
                <li key={i} className="list-row">
                  <span className={`act-ann act-ann-${a.annotation_level}`}>{a.annotation_level === "failure" ? "✕" : a.annotation_level === "warning" ? "!" : "i"}</span>
                  <div className="grow">
                    {a.title && <strong>{a.title}: </strong>}{a.message}
                    <div className="muted small mono"><Link to={`${base}/blob/${run.head_sha}/${a.path}#L${a.start_line}`}>{a.path}#L{a.start_line}</Link></div>
                  </div>
                </li>
              ))}
            </ul>
          </>
        )}
      </div>
    </div>
  );
}

// ---------- deployments ----------

const DEP_KIND: Record<string, Kind> = { success: "success", failure: "failure", error: "failure", inactive: "neutral", in_progress: "in_progress", queued: "queued", pending: "pending" };

/** Slot `repo.code.sidebar`: "Deployments" with the latest deployment per environment. */
export function DeploymentsSidebar({ ctx }: { ctx: RepoCtx }) {
  const st = useAsync(() => A.deployments(ctx.owner, ctx.name, { page: 1 }).then((p) => p.items).catch(() => [] as Deployment[]), [ctx.owner, ctx.name]);
  const list = st.data || [];
  if (!list.length) return null;
  const latest = new Map<string, Deployment>();
  for (const d of list) if (!latest.has(d.environment)) latest.set(d.environment, d);
  return (
    <section className="act-deploy-side" data-testid="deployments-sidebar">
      <h2 className="h4"><Link className="muted-link" to={`${ctx.base}/deployments`}>Deployments <span className="counter">{list.length}{list.length >= 30 ? "+" : ""}</span></Link></h2>
      <ul className="act-deploy-envs">
        {[...latest.values()].slice(0, 5).map((d) => (
          <li key={d.environment}><DeployStatusIcon owner={ctx.owner} name={ctx.name} d={d} /> <Link to={`${ctx.base}/deployments?environment=${encodeURIComponent(d.environment)}`}>{d.environment}</Link> <span className="muted small"><RelTime iso={d.created_at} /></span></li>
        ))}
      </ul>
    </section>
  );
}

function DeployStatusIcon({ owner, name, d }: { owner: string; name: string; d: Deployment }) {
  const st = useAsync(() => A.deploymentStatuses(owner, name, d.id).catch(() => [] as DeploymentStatus[]), [owner, name, d.id]);
  const s = st.data?.[0];
  return <StatusIcon kind={s ? DEP_KIND[s.state] || "neutral" : "queued"} title={s ? s.state : "No status"} />;
}

/** `/{o}/{r}/deployments` */
export function DeploymentsPage() {
  const { owner, name, base, repo } = useRepo();
  useTitle(`Deployments · ${repo.full_name}`);
  const [sp, setSp] = useSearchParams();
  const env = sp.get("environment") || sp.get("environments_filter") || "";
  const page = Number(sp.get("page") || 1);
  const envs = useAsync(() => A.environments(owner, name).then((e) => e.environments).catch(() => []), [owner, name]);
  const deps = useAsync(() => A.deployments(owner, name, { environment: env || undefined, page }), [owner, name, env, page]);
  return (
    <div className="act-layout">
      <nav className="act-side" aria-label="Environments">
        <h2 className="h4">Deployments</h2>
        <NavLink end to={`${base}/deployments`} className={() => "act-side-item" + (!env ? " selected" : "")}>All environments</NavLink>
        <h3 className="act-side-heading">Environments</h3>
        {(envs.data || []).map((e) => (
          <Link key={e.id} to={`${base}/deployments?environment=${encodeURIComponent(e.name)}`} className={"act-side-item" + (env === e.name ? " selected" : "")}>{e.name}</Link>
        ))}
      </nav>
      <div className="act-main">
        <div className="act-head"><h2 className="h3">{env ? `${env} deployments` : "Deployment history"}</h2></div>
        <Async state={deps}>
          {(p) => p.items.length === 0 ? <Blankslate title="There aren't any deployments yet.">Deployments are created by workflow jobs that target an environment, or through the Deployments API.</Blankslate> : (
            <ul className="box list" data-testid="deployments-list">
              {p.items.map((d) => <DeploymentRow key={d.id} d={d} />)}
            </ul>
          )}
        </Async>
        {deps.data && <Pagination page={page} next={deps.data.next} prev={deps.data.prev} last={deps.data.last} onPage={(n) => { const s = new URLSearchParams(sp); s.set("page", String(n)); setSp(s); }} />}
      </div>
    </div>
  );
}

function DeploymentRow({ d }: { d: Deployment }) {
  const { owner, name, base } = useRepo();
  const st = useAsync(() => A.deploymentStatuses(owner, name, d.id).catch(() => [] as DeploymentStatus[]), [owner, name, d.id]);
  const s = st.data?.[0];
  const log = localPath(s?.log_url || s?.target_url);
  return (
    <li className="list-row act-deploy-row">
      <StatusIcon kind={s ? DEP_KIND[s.state] || "neutral" : "queued"} title={s?.state} />
      <div className="grow">
        <div><strong>{d.environment}</strong> <span className="badge">{s ? s.state : "created"}</span></div>
        <div className="muted small">
          <UserLink user={d.creator} /> deployed <Link className="mono" to={`${base}/commit/${d.sha}`}>{shortSha(d.sha)}</Link> from <span className="branch-name">{d.ref}</span> <RelTime iso={d.created_at} />
          {d.description && <> · {d.description}</>}
        </div>
      </div>
      {s?.environment_url && <a className="btn btn-sm" href={s.environment_url} rel="noopener nofollow">View deployment</a>}
      {log.href && (log.external ? <a className="btn btn-sm" href={log.href} rel="noopener nofollow">Logs</a> : <Link className="btn btn-sm" to={log.href}>Logs</Link>)}
    </li>
  );
}
