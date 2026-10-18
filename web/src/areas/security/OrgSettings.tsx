// Organization settings sections (rendered by the orgs area's settings shell) and site admin
// sections (admin area shell): code security configurations, security overview, Dependabot
// secrets, push protection pattern settings, enterprise code security.
import { useState } from "react";
import { Link, useParams } from "react-router-dom";
import type { Repo } from "../../api";
import { Async, RelTime } from "../../components/common";
import { useAsync, useTitle } from "../../hooks";
import type { AdminCtx, OrgCtx } from "../types";
import { ENTERPRISE, humanize, sec, type EntCodeSecurity, type PatternConfig } from "./api";
import { ConfigEditor, ConfigList } from "./Configs";
import { RequestList } from "./Requests";
import { SecretForm, SecretTable } from "./Settings";
import { sealSecret } from "./sealedbox";
import { InlineError, PageHead, SeverityLabel, StateTabs, ToggleRow, useAction } from "./ui";

const enc = encodeURIComponent;

/** Sub-route of a section: the splat after the section path, e.g. "configurations/12/edit". */
function useSub(prefix: string) {
  const { "*": splat = "" } = useParams();
  const p = splat.replace(/^\/+|\/+$/g, "");
  return p === prefix ? "" : p.startsWith(`${prefix}/`) ? p.slice(prefix.length + 1) : "";
}

// ---------- org: code security ----------

export function OrgCodeSecurity({ ctx }: { ctx: OrgCtx }) {
  useTitle(`Code security · ${ctx.login}`);
  const sub = useSub("security_products");
  const scope = `/orgs/${enc(ctx.login)}`;
  const root = `/organizations/${ctx.login}/settings/security_products`;
  const m = sub.match(/^configurations\/(\d+)\/edit$/);
  if (sub === "configurations/new") return <ConfigEditor scope={scope} root={root} />;
  if (m) return <ConfigEditor scope={scope} root={root} id={Number(m[1])} />;
  return (
    <div data-testid="org-code-security">
      <ConfigList scope={scope} root={root} level="org" orgLogin={ctx.login} />
      <OrgPatterns org={ctx.login} />
    </div>
  );
}

function OrgPatterns({ org }: { org: string }) {
  const st = useAsync(() => sec.orgPatterns(org), [org]);
  const act = useAction();
  const [filter, setFilter] = useState("");
  const set = (p: PatternConfig, token: string, setting: string) =>
    act.run(async () => {
      await sec.setOrgPatterns(org, { pattern_config_version: p.pattern_config_version, provider_pattern_settings: [{ token_type: token, push_protection_setting: setting }] });
      st.reload();
    });
  return (
    <div className="box" data-testid="org-patterns">
      <div className="box-header">
        <h3>Push protection patterns</h3>
        <span className="grow" />
        <input className="input input-sm input-auto" placeholder="Filter patterns" aria-label="Filter patterns" value={filter} onChange={(e) => setFilter(e.target.value)} />
      </div>
      <InlineError error={act.error} />
      <Async state={st}>
        {(p) => (
          <div className="sec-table-wrap">
            <table className="sec-table">
              <thead><tr><th>Pattern</th><th>Alerts</th><th>Default</th><th>Push protection</th></tr></thead>
              <tbody>
                {p.provider_pattern_overrides.filter((o) => !filter || o.display_name.toLowerCase().includes(filter.toLowerCase())).map((o) => (
                  <tr key={o.token_type} data-testid="pattern-row">
                    <td>{o.display_name}<div className="muted small mono">{o.token_type}</div></td>
                    <td>{o.alert_total}</td>
                    <td>{humanize(o.default_setting)}</td>
                    <td>
                      <select className="input input-sm input-auto" aria-label={`Push protection for ${o.display_name}`} value={o.setting} disabled={act.busy}
                        onChange={(e) => set(p, o.token_type, e.target.value)}>
                        <option value="not-set">Default</option>
                        <option value="enabled">Enabled</option>
                        <option value="disabled">Disabled</option>
                      </select>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Async>
    </div>
  );
}

// ---------- org: security overview ----------

export function OrgSecurityOverview({ ctx }: { ctx: OrgCtx }) {
  useTitle(`Security overview · ${ctx.login}`);
  const [tab, setTab] = useState("dependabot");
  const org = ctx.login;
  const dep = useAsync(() => sec.orgDependabot(org, { state: "open" }).then((p) => p.items).catch(() => []), [org]);
  const code = useAsync(() => sec.orgCode(org, { state: "open" }).then((p) => p.items).catch(() => []), [org]);
  const secrets = useAsync(() => sec.orgSecrets(org, { state: "open", hide_secret: "true" }).then((p) => p.items).catch(() => []), [org]);
  const advisories = useAsync(() => sec.orgAdvisories(org).catch(() => []), [org]);
  const requests = useAsync(async () => {
    const [b, d, c, s] = await Promise.all([
      sec.orgBypassRequests(org).catch(() => []),
      sec.orgDismissalRequests(org, "dependabot").catch(() => []),
      sec.orgDismissalRequests(org, "code-scanning").catch(() => []),
      sec.orgDismissalRequests(org, "secret-scanning").catch(() => []),
    ]);
    return [...b, ...d, ...c, ...s].sort((x, y) => y.created_at.localeCompare(x.created_at));
  }, [org]);
  const counts = { dependabot: dep.data?.length, code: code.data?.length, secrets: secrets.data?.length, advisories: advisories.data?.length, requests: requests.data?.length };
  const repoOf = (a: { repository?: { full_name: string } }) => a.repository?.full_name || "";
  return (
    <div data-testid="org-security-overview">
      <PageHead title="Security overview" sub="Open alerts and pending requests across the repositories of this organization." />
      <div className="sec-cards">
        {([["Dependabot alerts", counts.dependabot], ["Code scanning alerts", counts.code], ["Secret scanning alerts", counts.secrets], ["Requests", counts.requests]] as [string, number | undefined][]).map(([l, n]) => (
          <div key={l} className="box box-body sec-card"><div className="sec-card-n">{n ?? "…"}</div><div className="muted small">{l}</div></div>
        ))}
      </div>
      <div className="box">
        <div className="box-header">
          <StateTabs value={tab} onChange={setTab} tabs={[
            { key: "dependabot", label: "Dependabot", count: counts.dependabot },
            { key: "code", label: "Code scanning", count: counts.code },
            { key: "secrets", label: "Secret scanning", count: counts.secrets },
            { key: "advisories", label: "Advisories", count: counts.advisories },
            { key: "requests", label: "Requests", count: counts.requests },
          ]} />
        </div>
        {tab === "dependabot" && (
          <Async state={dep}>
            {(l) => l.length === 0 ? <div className="blankslate"><h3>No open Dependabot alerts</h3></div> : (
              <ul className="list">
                {l.map((a) => (
                  <li key={repoOf(a) + a.number} className="list-row sec-alert-row" data-testid="org-alert-row">
                    <div className="grow sec-row-main">
                      <Link className="issue-title" to={`/${repoOf(a)}/security/dependabot/${a.number}`}>{a.security_advisory.summary}</Link>
                      <div className="muted small">{repoOf(a)} · {a.dependency.package.name} · <RelTime iso={a.created_at} /></div>
                    </div>
                    <SeverityLabel severity={a.security_vulnerability.severity} />
                  </li>
                ))}
              </ul>
            )}
          </Async>
        )}
        {tab === "code" && (
          <Async state={code}>
            {(l) => l.length === 0 ? <div className="blankslate"><h3>No open code scanning alerts</h3></div> : (
              <ul className="list">
                {l.map((a) => (
                  <li key={repoOf(a) + a.number} className="list-row sec-alert-row" data-testid="org-alert-row">
                    <div className="grow sec-row-main">
                      <Link className="issue-title" to={`/${repoOf(a)}/security/code-scanning/${a.number}`}>{a.rule.description || a.rule.id}</Link>
                      <div className="muted small">{repoOf(a)} · {a.tool.name} · <RelTime iso={a.created_at} /></div>
                    </div>
                    {a.rule.security_severity_level && <SeverityLabel severity={a.rule.security_severity_level} />}
                  </li>
                ))}
              </ul>
            )}
          </Async>
        )}
        {tab === "secrets" && (
          <Async state={secrets}>
            {(l) => l.length === 0 ? <div className="blankslate"><h3>No open secret scanning alerts</h3></div> : (
              <ul className="list">
                {l.map((a) => (
                  <li key={repoOf(a) + a.number} className="list-row sec-alert-row" data-testid="org-alert-row">
                    <div className="grow sec-row-main">
                      <Link className="issue-title" to={`/${repoOf(a)}/security/secret-scanning/${a.number}`}>{a.secret_type_display_name}</Link>
                      <div className="muted small">{repoOf(a)} · <RelTime iso={a.created_at} /></div>
                    </div>
                  </li>
                ))}
              </ul>
            )}
          </Async>
        )}
        {tab === "advisories" && (
          <Async state={advisories}>
            {(l) => l.length === 0 ? <div className="blankslate"><h3>No repository security advisories</h3></div> : (
              <ul className="list">
                {l.map((a) => (
                  <li key={a.ghsa_id} className="list-row sec-alert-row" data-testid="org-advisory-row">
                    <div className="grow sec-row-main">
                      <Link className="issue-title" to={new URL(a.html_url, location.origin).pathname}>{a.summary}</Link>
                      <div className="muted small"><span className="mono">{a.ghsa_id}</span> · {humanize(a.state)}</div>
                    </div>
                    <SeverityLabel severity={a.severity} />
                  </li>
                ))}
              </ul>
            )}
          </Async>
        )}
        {tab === "requests" && (
          <Async state={requests}>
            {(l) => <RequestList reqs={l} link={(r) => new URL(r.html_url, location.origin).pathname} />}
          </Async>
        )}
      </div>
    </div>
  );
}

// ---------- org: Dependabot secrets ----------

export function OrgDependabotSecrets({ ctx }: { ctx: OrgCtx }) {
  useTitle(`Dependabot secrets · ${ctx.login}`);
  const org = ctx.login;
  const list = useAsync(() => sec.orgDepSecrets(org), [org]);
  const repos = useAsync(() => sec.orgRepos(org).catch(() => [] as Repo[]), [org]);
  const [mode, setMode] = useState("");
  const [visibility, setVisibility] = useState("all");
  const [picked, setPicked] = useState<number[]>([]);
  const act = useAction();
  const startEdit = async (name: string) => {
    setMode(name);
    const s = list.data?.secrets.find((x) => x.name === name);
    setVisibility(s?.visibility || "all");
    if (s?.visibility === "selected") {
      const r = await sec.orgSecretRepos(org, name).catch(() => ({ repositories: [] as Repo[] }));
      setPicked(r.repositories.map((x) => x.id));
    } else setPicked([]);
  };
  const save = (name: string, value: string) =>
    act.run(async () => {
      const k = await sec.orgSecretKey(org);
      await sec.putOrgSecret(org, name, {
        encrypted_value: sealSecret(k.key, value), key_id: k.key_id, visibility,
        selected_repository_ids: visibility === "selected" ? picked : undefined,
      });
      setMode("");
      list.reload();
    });
  const extra = (
    <fieldset className="form-group" data-testid="secret-visibility">
      <legend>Repository access</legend>
      {[["all", "All repositories"], ["private", "Private repositories"], ["selected", "Selected repositories"]].map(([k, l]) => (
        <label key={k} className="radio-row"><input type="radio" name="dep-vis" value={k} checked={visibility === k} onChange={() => setVisibility(k)} /> {l}</label>
      ))}
      {visibility === "selected" && (
        <div className="sec-repo-picker">
          {(repos.data || []).map((r) => (
            <label key={r.id} className="check-row">
              <input type="checkbox" checked={picked.includes(r.id)} onChange={() => setPicked(picked.includes(r.id) ? picked.filter((x) => x !== r.id) : [...picked, r.id])} /> {r.name}
            </label>
          ))}
        </div>
      )}
    </fieldset>
  );
  return (
    <div data-testid="org-dependabot-secrets">
      <PageHead title="Dependabot secrets" sub="Organization secrets that Dependabot can use in the repositories they are shared with.">
        {!mode && <button type="button" className="btn btn-sm btn-primary" onClick={() => { setMode("new"); setVisibility("all"); setPicked([]); }} data-testid="new-dep-secret">New organization secret</button>}
      </PageHead>
      {mode && <SecretForm key={mode} initialName={mode === "new" ? undefined : mode} onSave={save} busy={act.busy} error={act.error} extra={extra} />}
      <SecretTable title="Organization secrets" secrets={list.data?.secrets} loading={list.loading} onEdit={startEdit} busy={act.busy}
        extra={(s) => <span className="badge">{s.visibility === "all" ? "All repositories" : s.visibility === "private" ? "Private repositories" : "Selected repositories"}</span>}
        onDelete={(n) => act.run(async () => { await sec.deleteOrgSecret(org, n); list.reload(); })} />
      <DependabotAccess org={org} repos={repos.data || []} />
    </div>
  );
}

function DependabotAccess({ org, repos }: { org: string; repos: Repo[] }) {
  const st = useAsync(() => sec.depAccess(org), [org]);
  const [add, setAdd] = useState("");
  const act = useAction();
  return (
    <div className="box" data-testid="dependabot-access">
      <div className="box-header"><h3>Grant Dependabot access to private repositories</h3></div>
      <div className="box-body">
        <InlineError error={act.error} />
        <Async state={st}>
          {(a) => (
            <>
              <div className="form-row">
                <div className="form-group">
                  <label htmlFor="dep-default-level">Default access level</label>
                  <select id="dep-default-level" className="input input-auto" value={a.default_level || "public"} disabled={act.busy}
                    onChange={(e) => act.run(async () => { await sec.setDepAccessDefault(org, e.target.value); st.reload(); })}>
                    <option value="public">Public repositories</option>
                    <option value="internal">Internal repositories</option>
                  </select>
                </div>
              </div>
              <ul className="list">
                {a.accessible_repositories.map((r) => (
                  <li key={r.id} className="list-row" data-testid="dep-access-row">
                    <span className="grow">{r.full_name}</span>
                    <button type="button" className="btn btn-sm btn-danger-outline" disabled={act.busy}
                      onClick={() => act.run(async () => { await sec.setDepAccess(org, { repository_ids_to_remove: [r.id] }); st.reload(); })}>Remove</button>
                  </li>
                ))}
              </ul>
              <div className="form-row sec-mt">
                <select className="input input-sm input-auto" aria-label="Repository to grant access to" value={add} onChange={(e) => setAdd(e.target.value)}>
                  <option value="">Select repository…</option>
                  {repos.filter((r) => !a.accessible_repositories.some((x) => x.id === r.id)).map((r) => <option key={r.id} value={r.id}>{r.name}</option>)}
                </select>
                <button type="button" className="btn btn-sm" disabled={!add || act.busy} data-testid="dep-access-add"
                  onClick={() => act.run(async () => { await sec.setDepAccess(org, { repository_ids_to_add: [Number(add)] }); setAdd(""); st.reload(); })}>Add repository</button>
              </div>
            </>
          )}
        </Async>
      </div>
    </div>
  );
}

// ---------- site admin: enterprise code security ----------

export function EnterpriseCodeSecurity({ ctx }: { ctx: AdminCtx }) {
  useTitle("Code security · Site admin");
  const sub = useSub("code-security");
  // The admin shell passes the enterprise slug (AdminCtx.enterprise); default "forge".
  const ent = (ctx as AdminCtx & { enterprise?: string }).enterprise || ENTERPRISE;
  const scope = `/enterprises/${enc(ent)}`;
  const root = "/site-admin/code-security";
  const m = sub.match(/^configurations\/(\d+)\/edit$/);
  if (sub === "configurations/new") return <ConfigEditor scope={scope} root={root} />;
  if (m) return <ConfigEditor scope={scope} root={root} id={Number(m[1])} />;
  return (
    <div data-testid="enterprise-code-security">
      <EnterpriseDefaults ent={ent} />
      <ConfigList scope={scope} root={root} level="enterprise" />
    </div>
  );
}

function EnterpriseDefaults({ ent }: { ent: string }) {
  const st = useAsync(() => sec.entSettings(ent), [ent]);
  const act = useAction();
  const [link, setLink] = useState<string | null>(null);
  const rows: [keyof EntCodeSecurity, string, string][] = [
    ["advanced_security_enabled_for_new_repositories", "GitHub Advanced Security", "Enable for new repositories."],
    ["dependabot_alerts_enabled_for_new_repositories", "Dependabot alerts", "Enable for new repositories."],
    ["secret_scanning_enabled_for_new_repositories", "Secret scanning", "Enable for new repositories."],
    ["secret_scanning_push_protection_enabled_for_new_repositories", "Push protection", "Enable for new repositories."],
    ["secret_scanning_non_provider_patterns_enabled_for_new_repositories", "Non-provider patterns", "Scan for generic secrets in new repositories."],
  ];
  return (
    <div className="box" data-testid="enterprise-defaults">
      <div className="box-header"><h3>Enterprise code security and analysis</h3></div>
      <div className="box-body">
        <InlineError error={act.error} />
        <Async state={st}>
          {(s) => (
            <>
              {rows.map(([k, t, d]) => (
                <ToggleRow key={k} testid={`ent-${k}`} title={t} description={d} on={!!s[k]} busy={act.busy}
                  onToggle={(next) => act.run(async () => { await sec.setEntSettings(ent, { [k]: next }); st.reload(); })} />
              ))}
              <div className="form-group sec-mt">
                <label htmlFor="pp-link">Custom link shown when a push is blocked</label>
                <div className="input-group">
                  <input id="pp-link" className="input" value={link ?? s.secret_scanning_push_protection_custom_link ?? ""} onChange={(e) => setLink(e.target.value)} placeholder="https://example.com/secrets-policy" />
                  <button type="button" className="btn btn-sm" disabled={act.busy || link === null}
                    onClick={() => act.run(async () => { await sec.setEntSettings(ent, { secret_scanning_push_protection_custom_link: link || null }); setLink(null); st.reload(); })}>Save</button>
                </div>
              </div>
            </>
          )}
        </Async>
      </div>
    </div>
  );
}
