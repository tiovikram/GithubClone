// Repository settings → Environments: list, create, configure protection rules (required
// reviewers, wait timer, deployment branches and tags), environment secrets and variables.
import { useState, type FormEvent } from "react";
import { Link, useNavigate } from "react-router-dom";
import { api, errorMessage, type User } from "../../api";
import { Async, Blankslate, ErrorBox, Flash, NotFoundView, RelTime } from "../../components/common";
import type { RepoCtx } from "../../components/RepoLayout";
import { useAsync, useTitle } from "../../hooks";
import { actionsApi as A, type Environment, type Reviewer } from "./api";
import { SecretForm, SecretsList, VariableForm, VariablesList } from "./secrets";
import { useSubRoute } from "./ui";

export function EnvironmentsSection({ ctx }: { ctx: RepoCtx }) {
  useTitle(`Environments · ${ctx.repo.full_name}`);
  const m = useSubRoute(`${ctx.base}/settings`, [
    "/environments", "/environments/new", "/environments/:id/edit",
    "/environments/:id/edit/secrets/new", "/environments/:id/edit/secrets/:item",
    "/environments/:id/edit/variables/new", "/environments/:id/edit/variables/:item",
  ]);
  if (!m) return <NotFoundView />;
  if (m.pattern === "/environments") return <EnvList ctx={ctx} />;
  if (m.pattern === "/environments/new") return <NewEnv ctx={ctx} />;
  const sub = m.pattern.includes("/secrets/") ? "secret" : m.pattern.includes("/variables/") ? "variable" : undefined;
  return <EnvById key={m.pattern + (m.params.item || "")} ctx={ctx} id={m.params.id || ""} item={m.params.item} sub={sub} />;
}

function protectionSummary(e: Environment): string {
  const parts: string[] = [];
  for (const r of e.protection_rules || []) {
    if (r.type === "required_reviewers") parts.push(`${r.reviewers?.length || 0} required reviewer${r.reviewers?.length === 1 ? "" : "s"}`);
    if (r.type === "wait_timer" && r.wait_timer) parts.push(`${r.wait_timer} minute wait timer`);
    if (r.type === "branch_policy") parts.push(e.deployment_branch_policy?.protected_branches ? "Protected branches only" : "Selected branches and tags");
  }
  return parts.length ? parts.join(" · ") : "No protection rules";
}

function EnvList({ ctx }: { ctx: RepoCtx }) {
  const st = useAsync(() => A.environments(ctx.owner, ctx.name), [ctx.owner, ctx.name]);
  const root = `${ctx.base}/settings/environments`;
  return (
    <>
      <div className="act-head">
        <h2 className="h3">Environments</h2>
        <Link className="btn btn-sm btn-primary" to={`${root}/new`} data-testid="new-environment">New environment</Link>
      </div>
      <p className="muted">You can configure environments with protection rules and secrets. A workflow job that references an environment must follow any protection rules for the environment before running or accessing the environment's secrets.</p>
      <Async state={st}>
        {(d) => d.environments.length === 0 ? <Blankslate title="There are no environments for this repository" /> : (
          <ul className="box list" data-testid="environments-list">
            {d.environments.map((e) => (
              <li key={e.id} className="list-row">
                <div className="grow">
                  <Link to={`${root}/${e.id}/edit`}><strong>{e.name}</strong></Link>
                  <div className="muted small">{protectionSummary(e)}</div>
                </div>
                <span className="muted small">Updated <RelTime iso={e.updated_at} /></span>
              </li>
            ))}
          </ul>
        )}
      </Async>
    </>
  );
}

function NewEnv({ ctx }: { ctx: RepoCtx }) {
  const nav = useNavigate();
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<unknown>();
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!name.trim()) return;
    setBusy(true);
    setErr(undefined);
    try {
      const env = await A.putEnvironment(ctx.owner, ctx.name, name.trim(), {});
      nav(`${ctx.base}/settings/environments/${env.id}/edit`);
    } catch (e2) {
      setErr(e2);
    } finally {
      setBusy(false);
    }
  };
  return (
    <form className="act-form" onSubmit={submit}>
      <p><Link to={`${ctx.base}/settings/environments`}>Environments</Link> / Add</p>
      <h2 className="h3">Add environment</h2>
      <div className="form-group">
        <label htmlFor="env-name">Name <span className="c-danger">*</span></label>
        <input id="env-name" className="input" value={name} onChange={(e) => setName(e.target.value)} maxLength={255} />
      </div>
      {err !== undefined && <ErrorBox error={err} />}
      <button type="submit" className="btn btn-primary" disabled={busy || !name.trim()} data-testid="configure-environment">Configure environment</button>
    </form>
  );
}

function EnvById({ ctx, id, item, sub }: { ctx: RepoCtx; id: string; item?: string; sub?: "secret" | "variable" }) {
  const st = useAsync(() => A.environments(ctx.owner, ctx.name).then((d) => d.environments.find((e) => String(e.id) === id) ?? null), [ctx.owner, ctx.name, id]);
  return (
    <Async state={st}>
      {(env) => {
        if (!env) return <NotFoundView />;
        const back = `${ctx.base}/settings/environments/${env.id}/edit`;
        const scope = { kind: "env" as const, owner: ctx.owner, repo: ctx.name, env: env.name };
        if (sub === "secret") return <SecretForm scope={scope} existing={item} back={back} />;
        if (sub === "variable") return <VariableForm scope={scope} existing={item} back={back} />;
        return <EnvEdit ctx={ctx} env={env} reload={st.reload} />;
      }}
    </Async>
  );
}

function EnvEdit({ ctx, env, reload }: { ctx: RepoCtx; env: Environment; reload: () => void }) {
  const nav = useNavigate();
  const rules = env.protection_rules || [];
  const rr = rules.find((r) => r.type === "required_reviewers");
  const wt = rules.find((r) => r.type === "wait_timer");
  const [reviewers, setReviewers] = useState<Reviewer[]>(rr?.reviewers || []);
  const [selfReview, setSelfReview] = useState(!!rr?.prevent_self_review);
  const [useReviewers, setUseReviewers] = useState(!!rr);
  const [useTimer, setUseTimer] = useState(!!wt?.wait_timer);
  const [timer, setTimer] = useState(wt?.wait_timer || 0);
  const [bypass, setBypass] = useState(env.can_admins_bypass !== false);
  const policy = env.deployment_branch_policy;
  const [branchMode, setBranchMode] = useState<"none" | "protected" | "custom">(!policy ? "none" : policy.protected_branches ? "protected" : "custom");
  const [who, setWho] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<unknown>();
  const [saved, setSaved] = useState(false);
  const root = `${ctx.base}/settings/environments/${env.id}/edit`;
  const scope = { kind: "env" as const, owner: ctx.owner, repo: ctx.name, env: env.name };

  const addReviewer = async () => {
    const login = who.trim().replace(/^@/, "");
    if (!login) return;
    setErr(undefined);
    try {
      const u: User = await api.user(login);
      if (!reviewers.some((r) => r.reviewer.id === u.id)) setReviewers([...reviewers, { type: "User", reviewer: { id: u.id, login: u.login, avatar_url: u.avatar_url } }]);
      setWho("");
    } catch (e) {
      setErr(new Error(`Could not find user ${login}: ${errorMessage(e)}`));
    }
  };
  const save = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setErr(undefined);
    setSaved(false);
    try {
      await A.putEnvironment(ctx.owner, ctx.name, env.name, {
        wait_timer: useTimer ? timer : 0,
        prevent_self_review: useReviewers ? selfReview : false,
        reviewers: useReviewers ? reviewers.map((r) => ({ type: r.type, id: r.reviewer.id })) : [],
        deployment_branch_policy: branchMode === "none" ? null : { protected_branches: branchMode === "protected", custom_branch_policies: branchMode === "custom" },
        can_admins_bypass: bypass,
      });
      setSaved(true);
      reload();
    } catch (e2) {
      setErr(e2);
    } finally {
      setBusy(false);
    }
  };
  const del = async () => {
    if (!confirm(`Are you sure you want to delete the ${env.name} environment? This also deletes its secrets and protection rules.`)) return;
    try {
      await A.deleteEnvironment(ctx.owner, ctx.name, env.name);
      nav(`${ctx.base}/settings/environments`);
    } catch (e) {
      setErr(e);
    }
  };
  return (
    <div className="act-env-edit">
      <p><Link to={`${ctx.base}/settings/environments`}>Environments</Link> / Configure <strong>{env.name}</strong></p>
      <form onSubmit={save} data-testid="environment-form">
        <h3 className="h4">Deployment protection rules</h3>
        <p className="muted small">Configure reviewers, timers, and custom rules that must pass before deployments to this environment can proceed.</p>
        <div className="box box-body">
          <label className="check-row"><input type="checkbox" checked={useReviewers} onChange={(e) => setUseReviewers(e.target.checked)} data-testid="env-required-reviewers" />
            <span><strong>Required reviewers</strong><br /><span className="muted small">Specify people or teams that may approve workflow runs when they access this environment.</span></span></label>
          {useReviewers && (
            <div className="act-indent">
              <div className="input-group">
                <input className="input" placeholder="Search for people by username" value={who} onChange={(e) => setWho(e.target.value)} aria-label="Add reviewer"
                  onKeyDown={(e) => { if (e.key === "Enter") { e.preventDefault(); addReviewer(); } }} data-testid="env-reviewer-input" />
                <button type="button" className="btn" onClick={addReviewer} data-testid="env-add-reviewer">Add</button>
              </div>
              <ul className="act-chips">
                {reviewers.map((r) => (
                  <li key={`${r.type}${r.reviewer.id}`} className="badge act-chip">
                    {r.reviewer.login || r.reviewer.slug || r.reviewer.name}
                    <button type="button" className="btn-link" aria-label={`Remove ${r.reviewer.login}`} onClick={() => setReviewers(reviewers.filter((x) => x !== r))}>×</button>
                  </li>
                ))}
              </ul>
              <label className="check-row"><input type="checkbox" checked={selfReview} onChange={(e) => setSelfReview(e.target.checked)} /> <span>Prevent self-review</span></label>
            </div>
          )}
          <label className="check-row"><input type="checkbox" checked={useTimer} onChange={(e) => setUseTimer(e.target.checked)} data-testid="env-wait-timer" />
            <span><strong>Wait timer</strong><br /><span className="muted small">Set an amount of time to wait before allowing deployments to proceed.</span></span></label>
          {useTimer && (
            <div className="act-indent">
              <input className="input input-auto" type="number" min={0} max={43200} value={timer} onChange={(e) => setTimer(Number(e.target.value))} aria-label="Wait timer minutes" /> <span className="muted">minutes</span>
            </div>
          )}
          <label className="check-row"><input type="checkbox" checked={bypass} onChange={(e) => setBypass(e.target.checked)} />
            <span><strong>Allow administrators to bypass configured protection rules</strong></span></label>
        </div>
        <h3 className="h4">Deployment branches and tags</h3>
        <p className="muted small">Limit which branches and tags can deploy to this environment based on rules or naming patterns.</p>
        <div className="box box-body">
          <select className="input input-auto" value={branchMode} onChange={(e) => setBranchMode(e.target.value as typeof branchMode)} aria-label="Deployment branches" data-testid="env-branch-mode">
            <option value="none">No restriction</option>
            <option value="protected">Protected branches only</option>
            <option value="custom">Selected branches and tags</option>
          </select>
          {branchMode === "custom" && policy?.custom_branch_policies && <BranchPolicies ctx={ctx} env={env.name} />}
          {branchMode === "custom" && !policy?.custom_branch_policies && <p className="muted small">Save the protection rules to add branch and tag rules.</p>}
        </div>
        {err !== undefined && <ErrorBox error={err} />}
        {saved && <Flash kind="success">Environment protection rules saved.</Flash>}
        <div className="form-actions act-left">
          <button type="submit" className="btn btn-primary" disabled={busy} data-testid="env-save">{busy ? "Saving…" : "Save protection rules"}</button>
        </div>
      </form>
      <h3 className="h4">Environment secrets</h3>
      <p className="muted small">Secrets are encrypted environment variables. They are accessible only by GitHub Actions in the context of this environment.</p>
      <SecretsList scope={scope} newPath={`${root}/secrets/new`} editPath={(n) => `${root}/secrets/${encodeURIComponent(n)}`} />
      <h3 className="h4">Environment variables</h3>
      <p className="muted small">Variables are used for non-sensitive configuration data. They are accessible only by GitHub Actions in the context of this environment.</p>
      <VariablesList scope={scope} newPath={`${root}/variables/new`} editPath={(n) => `${root}/variables/${encodeURIComponent(n)}`} />
      <div className="box danger-zone">
        <div className="box-body act-box-head">
          <span><strong>Delete environment</strong><br /><span className="muted small">Deleting an environment deletes its secrets, variables and protection rules.</span></span>
          <button type="button" className="btn btn-danger-outline" onClick={del} data-testid="env-delete">Delete environment</button>
        </div>
      </div>
    </div>
  );
}

function BranchPolicies({ ctx, env }: { ctx: RepoCtx; env: string }) {
  const st = useAsync(() => A.branchPolicies(ctx.owner, ctx.name, env), [ctx.owner, ctx.name, env]);
  const [name, setName] = useState("");
  const [type, setType] = useState<"branch" | "tag">("branch");
  const [err, setErr] = useState<unknown>();
  const add = async () => {
    if (!name.trim()) return;
    setErr(undefined);
    try {
      await A.addBranchPolicy(ctx.owner, ctx.name, env, { name: name.trim(), type });
      setName("");
      st.reload();
    } catch (e) {
      setErr(e);
    }
  };
  return (
    <div className="act-indent" data-testid="branch-policies">
      <ul className="list box">
        {(st.data?.branch_policies || []).map((p) => (
          <li key={p.id} className="list-row">
            <span className="grow mono">{p.name}</span>
            <span className="badge">{p.type || "branch"}</span>
            <button type="button" className="btn btn-sm btn-danger-outline" aria-label={`Remove rule ${p.name}`}
              onClick={async () => { try { await A.deleteBranchPolicy(ctx.owner, ctx.name, env, p.id); st.reload(); } catch (e) { setErr(e); } }}>Remove</button>
          </li>
        ))}
        {st.data && !st.data.branch_policies.length && <li className="list-row muted small">No rules yet: no branch or tag can deploy.</li>}
      </ul>
      <div className="input-group">
        <select className="input input-auto" value={type} onChange={(e) => setType(e.target.value as "branch" | "tag")} aria-label="Rule type">
          <option value="branch">Branch</option>
          <option value="tag">Tag</option>
        </select>
        <input className="input" placeholder="Name pattern, e.g. main or release/*" value={name} onChange={(e) => setName(e.target.value)} aria-label="Name pattern"
          onKeyDown={(e) => { if (e.key === "Enter") { e.preventDefault(); add(); } }} data-testid="branch-policy-input" />
        <button type="button" className="btn" onClick={add} data-testid="branch-policy-add">Add rule</button>
      </div>
      {err !== undefined && <ErrorBox error={err} />}
    </div>
  );
}
