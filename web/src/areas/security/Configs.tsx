// Code security configurations (organization and enterprise): list, create / edit, delete,
// attach to repositories, defaults for new repositories, attached repositories.
import { useState, type FormEvent } from "react";
import { Link, useNavigate } from "react-router-dom";
import type { Repo } from "../../api";
import { Async } from "../../components/common";
import { useAsync } from "../../hooks";
import { humanize, sec, type CodeSecurityConfig } from "./api";
import { InlineError, PageHead, useAction } from "./ui";

const GROUPS: { title: string; keys: [string, string, string][] }[] = [
  { title: "Dependency graph and Dependabot", keys: [
    ["dependency_graph", "Dependency graph", "Understand your dependencies."],
    ["dependency_graph_autosubmit_action", "Automatic dependency submission", "Submit dependencies detected at build time."],
    ["dependabot_alerts", "Dependabot alerts", "Receive alerts for vulnerabilities that affect your dependencies."],
    ["dependabot_security_updates", "Security updates", "Open pull requests to resolve Dependabot alerts."],
    ["dependabot_delegated_alert_dismissal", "Dependabot dismissal requests", "Require review to dismiss Dependabot alerts."],
  ] },
  { title: "Code security", keys: [
    ["code_scanning_default_setup", "Code scanning default setup", "Store the CodeQL default setup for repositories."],
    ["code_scanning_delegated_alert_dismissal", "Code scanning dismissal requests", "Require review to dismiss code scanning alerts."],
  ] },
  { title: "Secret protection", keys: [
    ["secret_scanning", "Secret scanning", "Receive alerts for detected secrets, keys, or other tokens."],
    ["secret_scanning_push_protection", "Push protection", "Block commits that contain supported secrets."],
    ["secret_scanning_delegated_bypass", "Bypass privileges", "Contributors request bypasses that reviewers approve."],
    ["secret_scanning_validity_checks", "Validity checks", "Check whether detected tokens are active."],
    ["secret_scanning_non_provider_patterns", "Non-provider patterns", "Scan for connection strings and other generic secrets."],
    ["secret_scanning_delegated_alert_dismissal", "Secret scanning dismissal requests", "Require review to close secret scanning alerts."],
  ] },
  { title: "Reporting", keys: [
    ["private_vulnerability_reporting", "Private vulnerability reporting", "Allow your community to privately report vulnerabilities."],
  ] },
];

const triLabel = (v: unknown) => (v === "enabled" ? "Enabled" : v === "disabled" ? "Disabled" : v === "not_set" ? "Not set" : humanize(String(v)));

/** `scope`: "/orgs/x" or "/enterprises/forge"; `root`: settings URL of this section. */
export function ConfigList({ scope, root, level, orgLogin }: { scope: string; root: string; level: "org" | "enterprise"; orgLogin?: string }) {
  const list = useAsync(() => sec.configs(scope), [scope]);
  const defaults = useAsync(() => sec.configDefaults(scope).catch(() => []), [scope]);
  const [attachFor, setAttachFor] = useState<CodeSecurityConfig | null>(null);
  const act = useAction();
  const defaultOf = (id: number) => (defaults.data || []).find((d) => d.configuration.id === id)?.default_for_new_repos;
  return (
    <div data-testid="config-list">
      <PageHead title="Configurations" sub={level === "org"
        ? "Security configurations are collections of enablement settings for security features, applied to repositories in this organization."
        : "Enterprise security configurations can be applied to the repositories of every organization."}>
        <Link className="btn btn-sm btn-primary" to={`${root}/configurations/new`} data-testid="new-config">New configuration</Link>
      </PageHead>
      <InlineError error={act.error} />
      <Async state={list}>
        {(configs) => (
          <div className="box">
            <ul className="list">
              {configs.map((c) => (
                <li key={c.id} className="list-row sec-alert-row" data-testid="config-row">
                  <div className="grow sec-row-main">
                    {c.target_type === "global" ? <strong>{c.name}</strong> : <Link className="issue-title" to={`${root}/configurations/${c.id}/edit`}>{c.name}</Link>}
                    {c.target_type === "global" && <span className="badge">GitHub recommended</span>}
                    {c.target_type === "enterprise" && level === "org" && <span className="badge">Enterprise</span>}
                    {c.enforcement === "enforced" && <span className="badge badge-warn">Enforced</span>}
                    {defaultOf(c.id) && defaultOf(c.id) !== "none" && <span className="badge badge-added" data-testid="config-default">Default for {defaultOf(c.id) === "all" ? "all" : humanize(defaultOf(c.id))} new repositories</span>}
                    <div className="muted small">{c.description}</div>
                  </div>
                  <button type="button" className="btn btn-sm" onClick={() => setAttachFor(c)} data-testid="apply-config">Apply to…</button>
                  {c.target_type !== "global" && (
                    <button type="button" className="btn btn-sm btn-danger-outline" disabled={act.busy} data-testid="delete-config"
                      onClick={() => window.confirm(`Delete configuration ${c.name}?`) && act.run(async () => { await sec.deleteConfig(scope, c.id); list.reload(); defaults.reload(); })}>Delete</button>
                  )}
                </li>
              ))}
            </ul>
          </div>
        )}
      </Async>
      {attachFor && <AttachBox scope={scope} config={attachFor} level={level} orgLogin={orgLogin} onApplied={defaults.reload} onClose={() => { setAttachFor(null); defaults.reload(); }} />}
    </div>
  );
}

function AttachBox({ scope, config, level, orgLogin, onClose, onApplied }: { scope: string; config: CodeSecurityConfig; level: "org" | "enterprise"; orgLogin?: string; onClose: () => void; onApplied: () => void }) {
  const [target, setTarget] = useState(level === "org" ? "selected" : "all_without_configurations");
  const [picked, setPicked] = useState<number[]>([]);
  const [defaultFor, setDefaultFor] = useState("");
  const [done, setDone] = useState(false);
  const repos = useAsync(() => (orgLogin ? sec.orgRepos(orgLogin) : Promise.resolve([] as Repo[])), [orgLogin]);
  const attached = useAsync(() => sec.configRepos(scope, config.id).catch(() => []), [scope, config.id, done]);
  const act = useAction();
  const scopes: [string, string][] = level === "org"
    ? [["selected", "Selected repositories"], ["all", "All repositories"], ["all_without_configurations", "All repositories without configurations"], ["public", "Public repositories"], ["private_or_internal", "Private and internal repositories"]]
    : [["all", "All repositories"], ["all_without_configurations", "All repositories without configurations"]];
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const ok = await act.run(async () => {
      await sec.attachConfig(scope, config.id, target === "selected" ? { scope: target, selected_repository_ids: picked } : { scope: target });
      if (defaultFor) await sec.setConfigDefault(scope, config.id, defaultFor);
    });
    if (ok) {
      setDone(true);
      onApplied();
    }
  };
  return (
    <form className="box" onSubmit={submit} data-testid="attach-config">
      <div className="box-header"><h3>Apply “{config.name}”</h3><span className="grow" /><button type="button" className="btn btn-sm" onClick={onClose}>Close</button></div>
      <div className="box-body">
        <div className="form-group">
          <label htmlFor="attach-scope">Repositories</label>
          <select id="attach-scope" className="input input-auto" value={target} onChange={(e) => setTarget(e.target.value)}>
            {scopes.map(([k, l]) => <option key={k} value={k}>{l}</option>)}
          </select>
        </div>
        {target === "selected" && (
          <fieldset className="form-group sec-repo-picker" data-testid="attach-repos">
            <legend>Select repositories</legend>
            {(repos.data || []).map((r) => (
              <label key={r.id} className="check-row">
                <input type="checkbox" checked={picked.includes(r.id)} onChange={() => setPicked(picked.includes(r.id) ? picked.filter((x) => x !== r.id) : [...picked, r.id])} />
                {r.name}
              </label>
            ))}
            {repos.data?.length === 0 && <span className="muted small">No repositories.</span>}
          </fieldset>
        )}
        <div className="form-group">
          <label htmlFor="attach-default">Use as default for new repositories</label>
          <select id="attach-default" className="input input-auto" value={defaultFor} onChange={(e) => setDefaultFor(e.target.value)} data-testid="default-select">
            <option value="">Don't change</option>
            <option value="all">All repositories</option>
            <option value="public">Public repositories</option>
            <option value="private_and_internal">Private and internal repositories</option>
            <option value="none">None</option>
          </select>
        </div>
        <InlineError error={act.error} />
        {done && <div className="flash flash-success" data-testid="attach-done">Configuration applied.</div>}
        <div className="form-actions sec-left">
          <button type="submit" className="btn btn-primary btn-sm" disabled={act.busy || (target === "selected" && picked.length === 0 && !defaultFor)} data-testid="attach-submit">Apply</button>
        </div>
        <h4 className="sec-mt">Attached repositories</h4>
        {(attached.data || []).length === 0 ? <p className="muted small">None yet.</p> : (
          <ul className="list" data-testid="attached-repos">
            {(attached.data || []).map((x) => <li key={x.repository.id} className="list-row small"><span className="grow">{x.repository.full_name}</span><span className="badge">{humanize(x.status)}</span></li>)}
          </ul>
        )}
      </div>
    </form>
  );
}

export function ConfigEditor({ scope, root, id }: { scope: string; root: string; id?: number }) {
  const nav = useNavigate();
  const existing = useAsync(() => (id ? sec.config(scope, id) : Promise.resolve(undefined)), [scope, id]);
  return (
    <Async state={{ ...existing, data: id ? existing.data : ({} as CodeSecurityConfig) }}>
      {(c) => <ConfigForm key={id || "new"} start={id ? c : undefined} scope={scope} onDone={() => nav(root)} />}
    </Async>
  );
}

function ConfigForm({ start, scope, onDone }: { start?: CodeSecurityConfig; scope: string; onDone: () => void }) {
  const [name, setName] = useState(start?.name || "");
  const [description, setDescription] = useState(start?.description || "");
  const [enforcement, setEnforcement] = useState(start?.enforcement || "unenforced");
  const [values, setValues] = useState<Record<string, string>>(() => {
    const v: Record<string, string> = { advanced_security: String(start?.advanced_security || "enabled") };
    for (const g of GROUPS) for (const [k] of g.keys) v[k] = String((start as Record<string, unknown> | undefined)?.[k] ?? (k === "dependency_graph" ? "enabled" : "disabled"));
    return v;
  });
  const act = useAction();
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const ok = await act.run(async () => {
      const body = { name, description, enforcement, ...values } as Partial<CodeSecurityConfig>;
      if (start) await sec.updateConfig(scope, start.id, body);
      else await sec.createConfig(scope, body);
    });
    if (ok) onDone();
  };
  return (
    <form onSubmit={submit} data-testid="config-form">
      <PageHead title={start ? `Edit ${start.name}` : "New configuration"} />
      <div className="box box-body">
        <div className="form-group">
          <label htmlFor="cfg-name">Name</label>
          <input id="cfg-name" className="input" required maxLength={255} value={name} onChange={(e) => setName(e.target.value)} />
        </div>
        <div className="form-group">
          <label htmlFor="cfg-desc">Description</label>
          <input id="cfg-desc" className="input" maxLength={255} value={description} onChange={(e) => setDescription(e.target.value)} />
        </div>
        <div className="form-group">
          <label htmlFor="cfg-ghas">GitHub Advanced Security features</label>
          <select id="cfg-ghas" className="input input-auto" value={values.advanced_security} onChange={(e) => setValues({ ...values, advanced_security: e.target.value })}>
            <option value="enabled">Include</option>
            <option value="disabled">Exclude</option>
            <option value="code_security">Code Security only</option>
            <option value="secret_protection">Secret Protection only</option>
          </select>
        </div>
      </div>
      {GROUPS.map((g) => (
        <div key={g.title} className="box">
          <div className="box-header"><h3>{g.title}</h3></div>
          <ul className="list">
            {g.keys.map(([k, label, desc]) => (
              <li key={k} className="list-row sec-cfg-row">
                <div className="grow">
                  <label htmlFor={`cfg-${k}`}><strong>{label}</strong></label>
                  <div className="muted small">{desc}</div>
                </div>
                <select id={`cfg-${k}`} className="input input-sm input-auto" value={values[k]} onChange={(e) => setValues({ ...values, [k]: e.target.value })} data-testid={`cfg-${k}`}>
                  {["enabled", "disabled", "not_set"].map((v) => <option key={v} value={v}>{triLabel(v)}</option>)}
                </select>
              </li>
            ))}
          </ul>
        </div>
      ))}
      <div className="box box-body">
        <div className="form-group">
          <label htmlFor="cfg-enforce">Policy</label>
          <select id="cfg-enforce" className="input input-auto" value={enforcement} onChange={(e) => setEnforcement(e.target.value as "enforced" | "unenforced")}>
            <option value="unenforced">Don't enforce</option>
            <option value="enforced">Enforce configuration (repositories can't change these settings)</option>
          </select>
        </div>
      </div>
      <InlineError error={act.error} />
      <div className="form-actions">
        <button type="button" className="btn" onClick={onDone}>Cancel</button>
        <button type="submit" className="btn btn-primary" disabled={act.busy} data-testid="config-save">{start ? "Update configuration" : "Save configuration"}</button>
      </div>
    </form>
  );
}
