// Repository settings sections: "Advanced Security" (/settings/security_analysis) and
// "Secrets and variables → Dependabot" (/settings/secrets/dependabot).
import { useState, type FormEvent } from "react";
import { Link } from "react-router-dom";
import { Async, RelTime } from "../../components/common";
import type { RepoCtx } from "../../components/RepoLayout";
import { useAsync, useTitle } from "../../hooks";
import { sec, type DepSecret, type SecurityAndAnalysis } from "./api";
import { DefaultSetupView } from "./CodeScanning";
import { sealSecret } from "./sealedbox";
import { InlineError, PageHead, ToggleRow, useAction } from "./ui";

export function AdvancedSecuritySettings({ ctx }: { ctx: RepoCtx }) {
  useTitle(`Advanced Security · ${ctx.repo.full_name}`);
  const repo = useAsync(() => sec.repo(ctx.owner, ctx.name), [ctx.owner, ctx.name]);
  const vuln = useAsync(() => sec.vulnAlerts(ctx.owner, ctx.name), [ctx.owner, ctx.name]);
  const fixes = useAsync(() => sec.autoFixes(ctx.owner, ctx.name), [ctx.owner, ctx.name]);
  const pvr = useAsync(() => sec.pvr(ctx.owner, ctx.name), [ctx.owner, ctx.name]);
  const config = useAsync(() => sec.repoConfig(ctx.owner, ctx.name), [ctx.owner, ctx.name]);
  const setup = useAsync(() => sec.defaultSetup(ctx.owner, ctx.name), [ctx.owner, ctx.name]);
  const isOrg = ctx.repo.owner.type === "Organization";
  const teams = useAsync(() => (isOrg ? sec.orgTeams(ctx.owner).catch(() => []) : Promise.resolve([])), [ctx.owner, isOrg]);
  const act = useAction();
  const setupAct = useAction();
  const [suite, setSuite] = useState("default");
  const sa: SecurityAndAnalysis = repo.data?.security_and_analysis || {};
  const on = (k: keyof SecurityAndAnalysis) => (sa[k] as { status?: string } | undefined)?.status === "enabled";
  const enforced = config.data?.configuration?.enforcement === "enforced";
  const toggle = (k: string) => (next: boolean) =>
    act.run(async () => {
      const r = await sec.setSecurity(ctx.owner, ctx.name, { [k]: { status: next ? "enabled" : "disabled" } });
      repo.setData(r);
    });
  const reviewers = sa.secret_scanning_delegated_bypass_options?.reviewers || [];
  const setReviewers = (ids: number[]) =>
    act.run(async () => {
      const r = await sec.setSecurity(ctx.owner, ctx.name, {
        secret_scanning_delegated_bypass: { status: "enabled" },
        secret_scanning_delegated_bypass_options: { reviewers: ids.map((id) => ({ reviewer_id: id, reviewer_type: "TEAM" })) },
      });
      repo.setData(r);
    });
  return (
    <div data-testid="advanced-security-settings">
      <PageHead title="Advanced Security" sub="Security and analysis features help keep your repository secure and updated." />
      {config.data?.configuration && (
        <div className="flash" data-testid="attached-config">
          Code security configuration: <strong>{config.data.configuration.name}</strong>
          {enforced ? " (enforced: settings below are managed by your organization)" : ""}
        </div>
      )}
      <InlineError error={act.error} />
      <Async state={repo}>
        {() => (
          <>
            <div className="box">
              <div className="box-header"><h3>Private vulnerability reporting</h3></div>
              <div className="box-body">
                <ToggleRow testid="pvr" title="Private vulnerability reporting" on={!!pvr.data?.enabled} busy={act.busy}
                  description="Allow your community to privately report potential security vulnerabilities to maintainers and repository owners."
                  onToggle={(next) => act.run(async () => { await sec.setPvr(ctx.owner, ctx.name, next); pvr.reload(); })} />
              </div>
            </div>

            <div className="box">
              <div className="box-header"><h3>Dependency graph and Dependabot</h3></div>
              <div className="box-body">
                <div className="sec-toggle-row">
                  <div className="sec-toggle-text">
                    <h3 className="sec-toggle-title">Dependency graph <span className="badge badge-added">Enabled</span></h3>
                    <div className="muted small">Understand your dependencies. Always on: built from the manifests of the default branch. <Link to={`${ctx.base}/network/dependencies`}>View dependency graph</Link></div>
                  </div>
                </div>
                <ToggleRow testid="dependabot-alerts" title="Dependabot alerts" on={!!vuln.data} busy={act.busy} disabled={enforced}
                  description="Receive alerts for vulnerabilities that affect your dependencies."
                  onToggle={(next) => act.run(async () => { await sec.setVulnAlerts(ctx.owner, ctx.name, next); vuln.reload(); fixes.reload(); })} />
                <ToggleRow testid="dependabot-security-updates" title="Dependabot security updates" on={!!fixes.data?.enabled} busy={act.busy} disabled={enforced || !vuln.data}
                  description="Allow Dependabot to open pull requests automatically to resolve Dependabot alerts (requires Dependabot alerts)."
                  onToggle={(next) => act.run(async () => { await sec.setAutoFixes(ctx.owner, ctx.name, next); fixes.reload(); })} />
              </div>
            </div>

            <div className="box">
              <div className="box-header"><h3>Code scanning</h3></div>
              <div className="box-body">
                <ToggleRow testid="advanced-security" title="GitHub Advanced Security" on={on("advanced_security")} busy={act.busy} disabled={enforced}
                  description="Code scanning and secret scanning features for private repositories." onToggle={toggle("advanced_security")} />
                <p className="small"><Link to={`${ctx.base}/security/code-scanning/tools`}>Code scanning tools and SARIF uploads</Link></p>
              </div>
            </div>
            <DefaultSetupView st={setup} act={setupAct} suite={suite} setSuite={setSuite} owner={ctx.owner} name={ctx.name} />

            <div className="box">
              <div className="box-header"><h3>Secret Protection</h3></div>
              <div className="box-body">
                <ToggleRow testid="secret-scanning" title="Secret scanning" on={on("secret_scanning")} busy={act.busy} disabled={enforced}
                  description="Receive alerts on GitHub for detected secrets, keys, or other tokens. Enabling it scans the full history." onToggle={toggle("secret_scanning")} />
                <ToggleRow testid="push-protection" title="Push protection" on={on("secret_scanning_push_protection")} busy={act.busy} disabled={enforced || !on("secret_scanning")}
                  description="Block commits that contain supported secrets." onToggle={toggle("secret_scanning_push_protection")} />
                <ToggleRow testid="non-provider-patterns" title="Scan for non-provider patterns" on={on("secret_scanning_non_provider_patterns")} busy={act.busy} disabled={enforced || !on("secret_scanning")}
                  description="Scan for connection strings, private keys and HTTP authentication headers." onToggle={toggle("secret_scanning_non_provider_patterns")} />
                <ToggleRow testid="validity-checks" title="Validity checks" on={on("secret_scanning_validity_checks")} busy={act.busy} disabled={enforced || !on("secret_scanning")}
                  description="Check whether detected tokens are still active (forge tokens only)." onToggle={toggle("secret_scanning_validity_checks")} />
                <ToggleRow testid="delegated-dismissal" title="Delegated alert dismissal" on={on("secret_scanning_delegated_alert_dismissal")} busy={act.busy} disabled={enforced || !on("secret_scanning")}
                  description="Require review to close secret scanning alerts." onToggle={toggle("secret_scanning_delegated_alert_dismissal")} />
                <ToggleRow testid="delegated-bypass" title="Delegated bypass for push protection" on={on("secret_scanning_delegated_bypass")} busy={act.busy}
                  disabled={enforced || !on("secret_scanning_push_protection")}
                  description={isOrg ? "Contributors request a bypass; organization owners, security managers and the teams below review it." : "Contributors request a bypass; repository administrators review it."}
                  onToggle={toggle("secret_scanning_delegated_bypass")}>
                  {isOrg && on("secret_scanning_delegated_bypass") && (
                    <fieldset className="form-group sec-mt" data-testid="bypass-reviewers">
                      <legend className="small">Bypass reviewers (teams)</legend>
                      {(teams.data || []).length === 0 && <span className="muted small">No teams in this organization.</span>}
                      {(teams.data || []).map((t) => {
                        const checked = reviewers.some((r) => r.reviewer_type === "TEAM" && r.reviewer_id === t.id);
                        return (
                          <label key={t.id} className="check-row">
                            <input type="checkbox" checked={checked} disabled={act.busy}
                              onChange={() => setReviewers(checked ? reviewers.filter((r) => r.reviewer_id !== t.id).map((r) => r.reviewer_id) : [...reviewers.map((r) => r.reviewer_id), t.id])} />
                            {t.name}
                          </label>
                        );
                      })}
                    </fieldset>
                  )}
                </ToggleRow>
              </div>
            </div>
          </>
        )}
      </Async>
    </div>
  );
}

// ---------- Dependabot secrets ----------

export function SecretForm({ onSave, initialName, busy, error, extra }: {
  onSave: (name: string, value: string) => void;
  initialName?: string;
  busy: boolean;
  error: unknown;
  extra?: React.ReactNode;
}) {
  const [name, setName] = useState(initialName || "");
  const [value, setValue] = useState("");
  const submit = (e: FormEvent) => {
    e.preventDefault();
    onSave(name.trim().toUpperCase(), value);
  };
  return (
    <form className="box box-body" onSubmit={submit} data-testid="secret-form">
      <div className="form-group">
        <label htmlFor="dep-secret-name">Name</label>
        <input id="dep-secret-name" className="input mono" required pattern="[A-Za-z_][A-Za-z0-9_]*" value={name} disabled={!!initialName}
          onChange={(e) => setName(e.target.value)} placeholder="YOUR_SECRET_NAME" />
        <p className="muted small">Alphanumeric characters or underscores only; may not start with a number or GITHUB_.</p>
      </div>
      <div className="form-group">
        <label htmlFor="dep-secret-value">Secret</label>
        <textarea id="dep-secret-value" className="input textarea mono" required value={value} onChange={(e) => setValue(e.target.value)} />
        <p className="muted small">Encrypted in your browser with the repository's public key before it is sent.</p>
      </div>
      {extra}
      <InlineError error={error} />
      <div className="form-actions sec-left">
        <button type="submit" className="btn btn-primary" disabled={busy} data-testid="secret-save">{initialName ? "Update secret" : "Add secret"}</button>
      </div>
    </form>
  );
}

export function DependabotSecretsSettings({ ctx }: { ctx: RepoCtx }) {
  useTitle(`Dependabot secrets · ${ctx.repo.full_name}`);
  const list = useAsync(() => sec.repoSecrets(ctx.owner, ctx.name), [ctx.owner, ctx.name]);
  const [mode, setMode] = useState<"" | "new" | string>("");
  const act = useAction();
  const save = (name: string, value: string) =>
    act.run(async () => {
      const k = await sec.repoSecretKey(ctx.owner, ctx.name);
      await sec.putRepoSecret(ctx.owner, ctx.name, name, { encrypted_value: sealSecret(k.key, value), key_id: k.key_id });
      setMode("");
      list.reload();
    });
  return (
    <div data-testid="dependabot-secrets">
      <PageHead title="Dependabot secrets" sub="Secrets are credentials that are encrypted. Dependabot can use them to access private registries; they are not passed to Actions workflows.">
        {!mode && <button type="button" className="btn btn-sm btn-primary" onClick={() => setMode("new")} data-testid="new-dep-secret">New repository secret</button>}
      </PageHead>
      {mode && <SecretForm key={mode} initialName={mode === "new" ? undefined : mode} onSave={save} busy={act.busy} error={act.error} />}
      <SecretTable secrets={list.data?.secrets} loading={list.loading} onEdit={setMode}
        onDelete={(n) => act.run(async () => { await sec.deleteRepoSecret(ctx.owner, ctx.name, n); list.reload(); })} busy={act.busy} />
    </div>
  );
}

export function SecretTable({ secrets, loading, onEdit, onDelete, busy, extra, title = "Repository secrets" }: {
  title?: string;
  secrets: DepSecret[] | undefined;
  loading: boolean;
  onEdit: (name: string) => void;
  onDelete: (name: string) => void;
  busy: boolean;
  extra?: (s: DepSecret) => React.ReactNode;
}) {
  return (
    <div className="box">
      <div className="box-header"><strong>{title}</strong></div>
      {!secrets ? (loading ? <div className="box-body muted">Loading…</div> : null) : secrets.length === 0 ? (
        <div className="blankslate"><h3>There are no secrets</h3></div>
      ) : (
        <ul className="list">
          {secrets.map((s) => (
            <li key={s.name} className="list-row sec-alert-row" data-testid="dep-secret-row">
              <span className="grow"><strong className="mono">{s.name}</strong> <span className="muted small">updated <RelTime iso={s.updated_at} /></span>{extra?.(s)}</span>
              <button type="button" className="btn btn-sm" onClick={() => onEdit(s.name)} data-testid="edit-dep-secret">Update</button>
              <button type="button" className="btn btn-sm btn-danger-outline" disabled={busy} data-testid="delete-dep-secret"
                onClick={() => window.confirm(`Delete ${s.name}?`) && onDelete(s.name)}>Remove</button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
