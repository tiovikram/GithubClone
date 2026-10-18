// Actions → General settings for a repository, organization or the enterprise: which
// repositories/organizations may use Actions, allowed actions, artifact and log retention,
// fork PR approval, default GITHUB_TOKEN permissions, access from other repositories and the
// OIDC subject claim template.
import { useState, type FormEvent, type ReactNode } from "react";
import { ErrorBox, Flash, Spinner } from "../../components/common";
import { useAsync } from "../../hooks";
import { actionsApi as A, type ActionsPermissions, type OidcSub, type Scope, type SelectedActions, type WorkflowPermissions } from "./api";

function Card({ title, desc, children, onSave, testid, saving, saved, err }: {
  title: string; desc?: ReactNode; children: ReactNode; onSave: () => Promise<void>; testid: string; saving: boolean; saved: boolean; err: unknown;
}) {
  return (
    <form className="act-policy" data-testid={testid} onSubmit={(e: FormEvent) => { e.preventDefault(); onSave(); }}>
      <h3 className="h4">{title}</h3>
      {desc && <p className="muted small">{desc}</p>}
      <div className="act-policy-body">{children}</div>
      {err !== undefined && <ErrorBox error={err} />}
      {saved && <Flash kind="success">Saved.</Flash>}
      <button type="submit" className="btn btn-sm" disabled={saving} data-testid={`${testid}-save`}>{saving ? "Saving…" : "Save"}</button>
    </form>
  );
}

/** Load + save state for one settings card; `null` data = not available at this scope. */
function useCard<T>(load: () => Promise<T>, deps: unknown[]) {
  const st = useAsync(() => load().catch(() => null), deps);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [err, setErr] = useState<unknown>();
  const save = async (fn: () => Promise<unknown>) => {
    setSaving(true);
    setSaved(false);
    setErr(undefined);
    try {
      await fn();
      setSaved(true);
      st.reload();
    } catch (e) {
      setErr(e);
    } finally {
      setSaving(false);
    }
  };
  return { st, saving, saved, err, save };
}

export function PolicyPage({ scope, ownerName }: { scope: Scope; ownerName: string }) {
  const key = JSON.stringify(scope);
  return (
    <div className="act-policies">
      <h2 className="h3">Actions permissions</h2>
      <PermissionsCard scope={scope} ownerName={ownerName} k={key} />
      <RetentionCard scope={scope} k={key} />
      <ApprovalCard scope={scope} k={key} />
      <WorkflowCard scope={scope} k={key} />
      {scope.kind === "repo" && <AccessCard scope={scope} k={key} />}
      {(scope.kind === "repo" || scope.kind === "org") && <OidcCard scope={scope} k={key} />}
    </div>
  );
}

function PermissionsCard({ scope, ownerName, k }: { scope: Scope; ownerName: string; k: string }) {
  const c = useCard(() => Promise.all([A.perm<ActionsPermissions>(scope), A.perm<SelectedActions>(scope, "/selected-actions").catch(() => ({} as SelectedActions))]), [k]);
  if (c.st.data === undefined) return <Spinner />;
  if (c.st.data === null) return null;
  return <PermissionsForm key={JSON.stringify(c.st.data)} scope={scope} ownerName={ownerName} data={c.st.data} c={c} />;
}

function PermissionsForm({ scope, ownerName, data, c }: { scope: Scope; ownerName: string; data: [ActionsPermissions, SelectedActions]; c: Pick<ReturnType<typeof useCard>, "saving" | "saved" | "err" | "save"> }) {
  const [p, sel] = data;
  const enabledKey = scope.kind === "org" ? "enabled_repositories" : scope.kind === "enterprise" ? "enabled_organizations" : "enabled";
  const [enabled, setEnabled] = useState<string>(scope.kind === "repo" ? (p.enabled ? "all" : "none") : String(p[enabledKey as keyof ActionsPermissions] ?? "all"));
  const [allowed, setAllowed] = useState(p.allowed_actions || "all");
  const [gh, setGh] = useState(sel.github_owned_allowed ?? true);
  const [verified, setVerified] = useState(sel.verified_allowed ?? false);
  const [patterns, setPatterns] = useState((sel.patterns_allowed || []).join(", "));
  const [sha, setSha] = useState(!!p.sha_pinning_required);
  const save = () => c.save(async () => {
    const body: Record<string, unknown> = { allowed_actions: allowed, sha_pinning_required: sha };
    if (scope.kind === "repo") body.enabled = enabled !== "none";
    else body[enabledKey] = enabled;
    if (enabled === "none") delete body.allowed_actions;
    await A.putPerm(scope, "", body);
    if (enabled !== "none" && allowed === "selected") {
      await A.putPerm(scope, "/selected-actions", { github_owned_allowed: gh, verified_allowed: verified, patterns_allowed: patterns.split(/[,\n]/).map((x) => x.trim()).filter(Boolean) });
    }
  });
  const what = scope.kind === "org" ? "repositories" : "organizations";
  return (
    <Card title="Policies" testid="policy-permissions" onSave={save} saving={c.saving} saved={c.saved} err={c.err}
      desc={scope.kind === "repo" ? "Choose which actions and reusable workflows are allowed to run in this repository." : `Choose which ${what} are permitted to use GitHub Actions, and which actions they may run.`}>
      {scope.kind === "repo" ? (
        <label className="check-row"><input type="checkbox" checked={enabled !== "none"} onChange={(e) => setEnabled(e.target.checked ? "all" : "none")} data-testid="actions-enabled" /> <span>Enable GitHub Actions for this repository</span></label>
      ) : (
        <div className="form-group">
          <label htmlFor="enabled-scope">Enabled {what}</label>
          <select id="enabled-scope" className="input input-auto" value={enabled} onChange={(e) => setEnabled(e.target.value)}>
            <option value="all">All {what}</option>
            <option value="selected">Selected {what}</option>
            <option value="none">Disabled</option>
          </select>
          {enabled === "selected" && <p className="muted small">Pick the {what} through the API (<code>PUT …/actions/permissions/{what}</code>); all are listed as enabled until then.</p>}
        </div>
      )}
      {enabled !== "none" && (
        <fieldset className="form-group act-fieldset">
          <legend>Allowed actions and reusable workflows</legend>
          <label className="radio-row"><input type="radio" name="allowed" checked={allowed === "all"} onChange={() => setAllowed("all")} /> <span>Allow all actions and reusable workflows</span></label>
          <label className="radio-row"><input type="radio" name="allowed" checked={allowed === "local_only"} onChange={() => setAllowed("local_only")} data-testid="allowed-local" /> <span>Allow {ownerName} actions and reusable workflows</span></label>
          <label className="radio-row"><input type="radio" name="allowed" checked={allowed === "selected"} onChange={() => setAllowed("selected")} data-testid="allowed-selected" /> <span>Allow {ownerName}, and select non-{ownerName}, actions and reusable workflows</span></label>
          {allowed === "selected" && (
            <div className="act-indent">
              <label className="check-row"><input type="checkbox" checked={gh} onChange={(e) => setGh(e.target.checked)} /> <span>Allow actions created by GitHub</span></label>
              <label className="check-row"><input type="checkbox" checked={verified} onChange={(e) => setVerified(e.target.checked)} /> <span>Allow actions by Marketplace verified creators</span></label>
              <label htmlFor="patterns" className="small">Allow specified actions and reusable workflows (comma separated)</label>
              <textarea id="patterns" className="input textarea mono" rows={2} value={patterns} onChange={(e) => setPatterns(e.target.value)} placeholder="monalisa/octocat@*, docker/*" />
            </div>
          )}
          <label className="check-row"><input type="checkbox" checked={sha} onChange={(e) => setSha(e.target.checked)} /> <span>Require actions to be pinned to a full-length commit SHA</span></label>
        </fieldset>
      )}
    </Card>
  );
}

function RetentionCard({ scope, k }: { scope: Scope; k: string }) {
  const c = useCard(() => A.perm<{ days: number; maximum_allowed_days?: number }>(scope, "/artifact-and-log-retention"), [k]);
  const [days, setDays] = useState<number | null>(null);
  if (!c.st.data) return null;
  const v = days ?? c.st.data.days;
  return (
    <Card title="Artifact and log retention" testid="policy-retention" saving={c.saving} saved={c.saved} err={c.err}
      desc={`Choose how long artifacts and logs of workflow runs are kept.${c.st.data.maximum_allowed_days ? ` The maximum is ${c.st.data.maximum_allowed_days} days.` : ""}`}
      onSave={() => c.save(() => A.putPerm(scope, "/artifact-and-log-retention", { days: v }))}>
      <label className="act-inline-label"><input className="input input-auto" type="number" min={1} max={c.st.data.maximum_allowed_days || 400} value={v} onChange={(e) => setDays(Number(e.target.value))} aria-label="Retention days" data-testid="retention-days" /> days</label>
    </Card>
  );
}

const APPROVAL: [string, string, string][] = [
  ["first_time_contributors_new_to_github", "Require approval for first-time contributors who are new to GitHub", "Only first-time contributors who recently created an account will require approval to run workflows."],
  ["first_time_contributors", "Require approval for first-time contributors", "Only first-time contributors will require approval to run workflows."],
  ["all_external_contributors", "Require approval for all external contributors", "All users that are not a member or owner of this repository will require approval to run workflows."],
];

function ApprovalCard({ scope, k }: { scope: Scope; k: string }) {
  const c = useCard(() => A.perm<{ approval_policy: string }>(scope, "/fork-pr-contributor-approval"), [k]);
  const [v, setV] = useState<string | null>(null);
  if (!c.st.data) return null;
  const cur = v ?? c.st.data.approval_policy;
  return (
    <Card title="Approval for running fork pull request workflows from contributors" testid="policy-approval" saving={c.saving} saved={c.saved} err={c.err}
      desc="Choose which subset of outside collaborators will require approval to run workflows on their pull requests."
      onSave={() => c.save(() => A.putPerm(scope, "/fork-pr-contributor-approval", { approval_policy: cur }))}>
      {APPROVAL.map(([val, label, help]) => (
        <label key={val} className="radio-row"><input type="radio" name="approval" checked={cur === val} onChange={() => setV(val)} data-testid={`approval-${val}`} /> <span>{label}<br /><span className="muted small">{help}</span></span></label>
      ))}
    </Card>
  );
}

function WorkflowCard({ scope, k }: { scope: Scope; k: string }) {
  const c = useCard(() => A.perm<WorkflowPermissions>(scope, "/workflow"), [k]);
  const [perm, setPerm] = useState<string | null>(null);
  const [approve, setApprove] = useState<boolean | null>(null);
  if (!c.st.data) return null;
  const p = perm ?? c.st.data.default_workflow_permissions;
  const a = approve ?? c.st.data.can_approve_pull_request_reviews;
  return (
    <Card title="Workflow permissions" testid="policy-workflow" saving={c.saving} saved={c.saved} err={c.err}
      desc="Choose the default permissions granted to the GITHUB_TOKEN when running workflows. You can specify more granular permissions in the workflow using YAML."
      onSave={() => c.save(() => A.putPerm(scope, "/workflow", { default_workflow_permissions: p, can_approve_pull_request_reviews: a }))}>
      <label className="radio-row"><input type="radio" name="wfperm" checked={p === "write"} onChange={() => setPerm("write")} data-testid="wfperm-write" /> <span>Read and write permissions<br /><span className="muted small">Workflows have read and write permissions in the repository for all scopes.</span></span></label>
      <label className="radio-row"><input type="radio" name="wfperm" checked={p === "read"} onChange={() => setPerm("read")} data-testid="wfperm-read" /> <span>Read repository contents and packages permissions<br /><span className="muted small">Workflows have read permissions in the repository for the contents and packages scopes only.</span></span></label>
      <label className="check-row"><input type="checkbox" checked={a} onChange={(e) => setApprove(e.target.checked)} /> <span>Allow GitHub Actions to create and approve pull requests</span></label>
    </Card>
  );
}

function AccessCard({ scope, k }: { scope: Scope; k: string }) {
  const c = useCard(() => A.perm<{ access_level: string }>(scope, "/access"), [k]);
  const [v, setV] = useState<string | null>(null);
  if (!c.st.data) return null;
  const cur = v ?? c.st.data.access_level;
  return (
    <Card title="Access" testid="policy-access" saving={c.saving} saved={c.saved} err={c.err}
      desc="Control how this repository is used by GitHub Actions workflows in other repositories (private and internal repositories only)."
      onSave={() => c.save(() => A.putPerm(scope, "/access", { access_level: cur }))}>
      <select className="input input-auto" value={cur} onChange={(e) => setV(e.target.value)} aria-label="Access level">
        <option value="none">Not accessible</option>
        <option value="user">Accessible from repositories owned by the same user</option>
        <option value="organization">Accessible from repositories in the organization</option>
        <option value="enterprise">Accessible from repositories in the enterprise</option>
      </select>
    </Card>
  );
}

function OidcCard({ scope, k }: { scope: Scope; k: string }) {
  const c = useCard(() => A.oidc(scope), [k]);
  const [useDefault, setUseDefault] = useState<boolean | null>(null);
  const [keys, setKeys] = useState<string | null>(null);
  if (!c.st.data) return null;
  const d: OidcSub = c.st.data;
  const def = useDefault ?? (scope.kind === "repo" ? d.use_default !== false : false);
  const ks = keys ?? (d.include_claim_keys || []).join(", ");
  return (
    <Card title="OpenID Connect" testid="policy-oidc" saving={c.saving} saved={c.saved} err={c.err}
      desc={<>Customize the subject (<code>sub</code>) claim of the OIDC tokens issued to workflows, e.g. <code>repo, context, job_workflow_ref</code>.</>}
      onSave={() => c.save(() => A.putOidc(scope, scope.kind === "repo"
        ? (def ? { use_default: true } : { use_default: false, include_claim_keys: ks.split(",").map((x) => x.trim()).filter(Boolean) })
        : { include_claim_keys: ks.split(",").map((x) => x.trim()).filter(Boolean) }))}>
      {scope.kind === "repo" && (
        <>
          <label className="radio-row"><input type="radio" name="oidc" checked={def} onChange={() => setUseDefault(true)} /> <span>Use default subject claim template</span></label>
          <label className="radio-row"><input type="radio" name="oidc" checked={!def} onChange={() => setUseDefault(false)} data-testid="oidc-custom" /> <span>Use custom template</span></label>
        </>
      )}
      {(!def || scope.kind !== "repo") && (
        <input className="input mono" value={ks} onChange={(e) => setKeys(e.target.value)} placeholder="repo, context" aria-label="Claim keys" data-testid="oidc-keys" />
      )}
    </Card>
  );
}
