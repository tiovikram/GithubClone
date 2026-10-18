// Repository security advisories: list, draft / report form, detail with edit, credits, CVE
// request and publish (/{o}/{r}/security/advisories[/new|/GHSA-…]).
import { useState, type FormEvent } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { Async, Avatar, RelTime, UserLink } from "../../components/common";
import { useRepo } from "../../components/RepoLayout";
import { useAuth } from "../../auth";
import { useAsync, useTitle } from "../../hooks";
import { Markdown } from "../../markdown";
import {
  CREDIT_TYPES, ECOSYSTEMS, ecoLabel, humanize, sec, type RepoAdvisory, type RepoAdvisoryBody, type RepoAdvisoryVuln,
} from "./api";
import { AlertState, InlineError, PageHead, SeverityLabel, StateTabs, useAction } from "./ui";

const TABS = [
  { key: "published", label: "Published" },
  { key: "draft", label: "Draft" },
  { key: "triage", label: "Triage" },
  { key: "closed", label: "Closed" },
];

export function AdvisoriesPage() {
  const ctx = useRepo();
  useTitle(`Security advisories · ${ctx.repo.full_name}`);
  const all = useAsync(() => sec.advisories(ctx.owner, ctx.name), [ctx.owner, ctx.name]);
  const pvr = useAsync(() => sec.pvr(ctx.owner, ctx.name).catch(() => ({ enabled: false })), [ctx.owner, ctx.name]);
  const [tab, setTab] = useState("published");
  const counts = (k: string) => (all.data || []).filter((a) => (k === "closed" ? a.state === "closed" || a.state === "withdrawn" : a.state === k)).length;
  const visibleTabs = TABS.filter((t) => t.key === "published" || ctx.isAdmin || counts(t.key) > 0);
  return (
    <div data-testid="advisories-page">
      <PageHead title="Security advisories" sub="Privately discuss, fix, and publish information about security vulnerabilities in your repository's code.">
        {ctx.isAdmin && <Link className="btn btn-sm btn-primary" to={`${ctx.base}/security/advisories/new`} data-testid="new-advisory">New draft security advisory</Link>}
        {!ctx.isAdmin && pvr.data?.enabled && <Link className="btn btn-sm btn-primary" to={`${ctx.base}/security/advisories/new`} data-testid="report-vuln">Report a vulnerability</Link>}
      </PageHead>
      <Async state={all}>
        {(list) => {
          const rows = list.filter((a) => (tab === "closed" ? a.state === "closed" || a.state === "withdrawn" : a.state === tab));
          return (
            <div className="box">
              <div className="box-header">
                <StateTabs value={tab} onChange={setTab} tabs={visibleTabs.map((t) => ({ ...t, count: counts(t.key) }))} />
              </div>
              {rows.length === 0 ? (
                <div className="blankslate">
                  <h3>{tab === "published" ? "There aren’t any published security advisories" : `No ${tab} security advisories`}</h3>
                  {tab === "published" && ctx.isAdmin && <p className="muted">Draft an advisory to privately discuss and fix a vulnerability, then publish it.</p>}
                </div>
              ) : (
                <ul className="list">
                  {rows.map((a) => (
                    <li key={a.ghsa_id} className="list-row sec-alert-row" data-testid="advisory-row">
                      <div className="grow sec-row-main">
                        <Link className="issue-title" to={`${ctx.base}/security/advisories/${a.ghsa_id}`}>{a.summary || "(no summary)"}</Link>
                        <div className="muted small sec-row-meta">
                          <span className="mono">{a.ghsa_id}</span>
                          {a.state === "published" ? <RelTime iso={a.published_at} prefix="published " /> : <RelTime iso={a.created_at} prefix="opened " />}
                          {a.author && <> by <UserLink user={a.author} /></>}
                          {a.submission && <span className="badge">Private report</span>}
                        </div>
                      </div>
                      <SeverityLabel severity={a.severity} />
                    </li>
                  ))}
                </ul>
              )}
            </div>
          );
        }}
      </Async>
      {pvr.data && (
        <p className="muted small" data-testid="pvr-status">
          Private vulnerability reporting is {pvr.data.enabled ? "enabled" : "disabled"} for this repository.
          {ctx.isAdmin && <> <Link to={`${ctx.base}/settings/security_analysis`}>Change in settings</Link>.</>}
        </p>
      )}
    </div>
  );
}

// ---------- form ----------

interface FormState {
  summary: string;
  description: string;
  cve_id: string;
  vulns: { ecosystem: string; name: string; range: string; patched: string; functions: string }[];
  scoring: "severity" | "cvss";
  severity: string;
  cvss: string;
  cwes: string;
  credits: { login: string; type: string }[];
  collaborators: string;
}

function initial(a?: RepoAdvisory): FormState {
  return {
    summary: a?.summary || "",
    description: a?.description || "",
    cve_id: a?.cve_id || "",
    vulns: (a?.vulnerabilities?.length ? a.vulnerabilities : [{ package: { ecosystem: "npm", name: "" }, vulnerable_version_range: "", patched_versions: "", vulnerable_functions: [] } as RepoAdvisoryVuln])
      .map((v) => ({
        ecosystem: v.package.ecosystem, name: v.package.name || "", range: v.vulnerable_version_range || "",
        patched: v.patched_versions || "", functions: (v.vulnerable_functions || []).join(", "),
      })),
    scoring: a?.cvss?.vector_string ? "cvss" : "severity",
    severity: a?.severity || "high",
    cvss: a?.cvss?.vector_string || "",
    cwes: (a?.cwe_ids || []).join(", "),
    credits: (a?.credits || []).map((c) => ({ ...c })),
    collaborators: (a?.collaborating_users || []).map((u) => u.login).join(", "),
  };
}

function toBody(f: FormState, manager: boolean): RepoAdvisoryBody {
  const b: RepoAdvisoryBody = {
    summary: f.summary,
    description: f.description,
    vulnerabilities: f.vulns.map((v) => ({
      package: { ecosystem: v.ecosystem, name: v.name.trim() || null },
      vulnerable_version_range: v.range.trim() || null,
      patched_versions: v.patched.trim() || null,
      vulnerable_functions: v.functions.split(",").map((x) => x.trim()).filter(Boolean),
    })),
    cwe_ids: f.cwes.split(/[,\s]+/).map((x) => x.trim().toUpperCase()).filter(Boolean).map((x) => (x.startsWith("CWE-") ? x : `CWE-${x}`)),
  };
  if (f.scoring === "cvss") {
    b.cvss_vector_string = f.cvss.trim() || null;
    b.severity = null;
  } else {
    b.severity = f.severity;
    b.cvss_vector_string = null;
  }
  if (manager) {
    b.cve_id = f.cve_id.trim() || null;
    b.credits = f.credits.filter((c) => c.login.trim()).map((c) => ({ login: c.login.trim(), type: c.type }));
    b.collaborating_users = f.collaborators.split(/[,\s]+/).map((x) => x.trim()).filter(Boolean);
  }
  return b;
}

function AdvisoryForm({ start, manager, submitLabel, onSubmit, onCancel, report }: {
  start?: RepoAdvisory;
  manager: boolean;
  submitLabel: string;
  onSubmit: (b: RepoAdvisoryBody) => Promise<void>;
  onCancel?: () => void;
  report?: boolean;
}) {
  const [f, setF] = useState<FormState>(() => initial(start));
  const act = useAction();
  const set = <K extends keyof FormState>(k: K, v: FormState[K]) => setF((x) => ({ ...x, [k]: v }));
  const setVuln = (i: number, k: keyof FormState["vulns"][number], v: string) =>
    setF((x) => ({ ...x, vulns: x.vulns.map((y, j) => (j === i ? { ...y, [k]: v } : y)) }));
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    await act.run(() => onSubmit(toBody(f, manager)));
  };
  return (
    <form className="sec-form" onSubmit={submit} data-testid="advisory-form">
      <div className="box box-body">
        <h3 className="sec-form-h">Advisory details</h3>
        <div className="form-group">
          <label htmlFor="adv-title">Title</label>
          <input id="adv-title" className="input" required maxLength={1024} value={f.summary} onChange={(e) => set("summary", e.target.value)}
            placeholder="A short summary of the vulnerability" />
        </div>
        {manager && (
          <div className="form-group">
            <label htmlFor="adv-cve">CVE identifier <span className="muted small">(optional)</span></label>
            <input id="adv-cve" className="input input-auto mono" value={f.cve_id} onChange={(e) => set("cve_id", e.target.value)} placeholder="CVE-2024-12345" />
            <p className="muted small">Leave empty and request a CVE from the advisory page, or enter an existing one.</p>
          </div>
        )}
        <div className="form-group">
          <label htmlFor="adv-desc">Description</label>
          <textarea id="adv-desc" className="input textarea" rows={8} required value={f.description} onChange={(e) => set("description", e.target.value)}
            placeholder={"### Impact\nWhat kind of vulnerability is it? Who is impacted?\n\n### Patches\n\n### Workarounds\n\n### References"} />
        </div>
      </div>

      <div className="box box-body">
        <h3 className="sec-form-h">Affected products</h3>
        {f.vulns.map((v, i) => (
          <fieldset key={i} className="sec-vuln" data-testid="advisory-vuln">
            <div className="sec-grid">
              <div className="form-group">
                <label htmlFor={`adv-eco-${i}`}>Ecosystem</label>
                <select id={`adv-eco-${i}`} className="input" value={v.ecosystem} onChange={(e) => setVuln(i, "ecosystem", e.target.value)}>
                  {ECOSYSTEMS.map(([k, l]) => <option key={k} value={k}>{l}</option>)}
                </select>
              </div>
              <div className="form-group">
                <label htmlFor={`adv-pkg-${i}`}>Package name</label>
                <input id={`adv-pkg-${i}`} className="input" value={v.name} onChange={(e) => setVuln(i, "name", e.target.value)} placeholder="e.g. example.com/a/package" />
              </div>
              <div className="form-group">
                <label htmlFor={`adv-range-${i}`}>Affected versions</label>
                <input id={`adv-range-${i}`} className="input" value={v.range} onChange={(e) => setVuln(i, "range", e.target.value)} placeholder="< 1.2.3" />
              </div>
              <div className="form-group">
                <label htmlFor={`adv-patched-${i}`}>Patched versions</label>
                <input id={`adv-patched-${i}`} className="input" value={v.patched} onChange={(e) => setVuln(i, "patched", e.target.value)} placeholder="1.2.3" />
              </div>
            </div>
            <div className="form-group">
              <label htmlFor={`adv-fn-${i}`}>Vulnerable functions <span className="muted small">(comma separated, optional)</span></label>
              <input id={`adv-fn-${i}`} className="input" value={v.functions} onChange={(e) => setVuln(i, "functions", e.target.value)} />
            </div>
            {f.vulns.length > 1 && (
              <button type="button" className="btn btn-sm btn-danger-outline" onClick={() => set("vulns", f.vulns.filter((_, j) => j !== i))}>Remove product</button>
            )}
          </fieldset>
        ))}
        <button type="button" className="btn btn-sm" onClick={() => set("vulns", [...f.vulns, { ecosystem: "npm", name: "", range: "", patched: "", functions: "" }])}>
          + Add another affected product
        </button>
      </div>

      <div className="box box-body">
        <h3 className="sec-form-h">Severity</h3>
        <div className="segmented" role="group" aria-label="Scoring">
          <button type="button" aria-pressed={f.scoring === "severity"} onClick={() => set("scoring", "severity")}>Assess severity</button>
          <button type="button" aria-pressed={f.scoring === "cvss"} onClick={() => set("scoring", "cvss")} data-testid="use-cvss">CVSS v3 vector</button>
        </div>
        {f.scoring === "severity" ? (
          <div className="form-group sec-mt">
            <label htmlFor="adv-sev">Severity</label>
            <select id="adv-sev" className="input input-auto" value={f.severity} onChange={(e) => set("severity", e.target.value)}>
              <option value="critical">Critical</option>
              <option value="high">High</option>
              <option value="medium">Moderate</option>
              <option value="low">Low</option>
            </select>
          </div>
        ) : (
          <div className="form-group sec-mt">
            <label htmlFor="adv-cvss">CVSS vector string</label>
            <input id="adv-cvss" className="input mono" value={f.cvss} onChange={(e) => set("cvss", e.target.value)} placeholder="CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H" />
          </div>
        )}
        <div className="form-group">
          <label htmlFor="adv-cwe">Weaknesses (CWE)</label>
          <input id="adv-cwe" className="input" value={f.cwes} onChange={(e) => set("cwes", e.target.value)} placeholder="CWE-79, CWE-1321" />
        </div>
      </div>

      {manager && !report && (
        <div className="box box-body" data-testid="advisory-credits">
          <h3 className="sec-form-h">Credits</h3>
          <p className="muted small">Credit people who helped find, report or fix the vulnerability. Credits other than yours are pending until accepted.</p>
          {f.credits.map((c, i) => (
            <div key={i} className="form-row sec-credit-row">
              <div className="form-group">
                <label htmlFor={`credit-login-${i}`}>Username</label>
                <input id={`credit-login-${i}`} className="input" value={c.login}
                  onChange={(e) => set("credits", f.credits.map((x, j) => (j === i ? { ...x, login: e.target.value } : x)))} />
              </div>
              <div className="form-group">
                <label htmlFor={`credit-type-${i}`}>Credit type</label>
                <select id={`credit-type-${i}`} className="input" value={c.type}
                  onChange={(e) => set("credits", f.credits.map((x, j) => (j === i ? { ...x, type: e.target.value } : x)))}>
                  {CREDIT_TYPES.map(([k, l]) => <option key={k} value={k}>{l}</option>)}
                </select>
              </div>
              <button type="button" className="btn btn-sm btn-danger-outline" onClick={() => set("credits", f.credits.filter((_, j) => j !== i))}>Remove</button>
            </div>
          ))}
          <button type="button" className="btn btn-sm" data-testid="add-credit" onClick={() => set("credits", [...f.credits, { login: "", type: "finder" }])}>+ Add credit</button>
          <div className="form-group sec-mt">
            <label htmlFor="adv-collab">Collaborators <span className="muted small">(usernames, comma separated)</span></label>
            <input id="adv-collab" className="input" value={f.collaborators} onChange={(e) => set("collaborators", e.target.value)} />
          </div>
        </div>
      )}

      <InlineError error={act.error} />
      <div className="form-actions">
        {onCancel && <button type="button" className="btn" onClick={onCancel}>Cancel</button>}
        <button type="submit" className="btn btn-primary" disabled={act.busy} data-testid="advisory-submit">{act.busy ? "Saving…" : submitLabel}</button>
      </div>
    </form>
  );
}

export function NewAdvisoryPage() {
  const ctx = useRepo();
  const nav = useNavigate();
  const pvr = useAsync(() => sec.pvr(ctx.owner, ctx.name).catch(() => ({ enabled: false })), [ctx.owner, ctx.name]);
  const manager = ctx.isAdmin;
  useTitle(manager ? `New security advisory · ${ctx.repo.full_name}` : `Report a vulnerability · ${ctx.repo.full_name}`);
  if (!manager && pvr.loading) return null;
  if (!manager && !pvr.data?.enabled) {
    return (
      <div className="blankslate" data-testid="pvr-disabled">
        <h3>Private vulnerability reporting is disabled</h3>
        <p className="muted">The maintainers of this repository have not enabled private vulnerability reporting. See the <Link to={`${ctx.base}/security/policy`}>security policy</Link> for how to report a vulnerability.</p>
      </div>
    );
  }
  return (
    <div data-testid={manager ? "new-advisory-page" : "report-page"}>
      <PageHead title={manager ? "Open a draft security advisory" : "Report a vulnerability"}
        sub={manager
          ? "After the draft security advisory is open, you can privately discuss it with collaborators and fix the vulnerability, then publish it."
          : "Privately report a security vulnerability to the maintainers of this repository. Only you and the maintainers can see this report."} />
      <AdvisoryForm manager={manager} report={!manager} submitLabel={manager ? "Create draft security advisory" : "Submit report"}
        onCancel={() => nav(`${ctx.base}/security/advisories`)}
        onSubmit={async (b) => {
          const a = manager ? await sec.createAdvisory(ctx.owner, ctx.name, b) : await sec.reportVuln(ctx.owner, ctx.name, b);
          nav(`${ctx.base}/security/advisories/${a.ghsa_id}`);
        }} />
    </div>
  );
}

export function AdvisoryPage() {
  const ctx = useRepo();
  const { ghsa = "" } = useParams();
  const state = useAsync(() => sec.advisory(ctx.owner, ctx.name, ghsa), [ctx.owner, ctx.name, ghsa]);
  const [editing, setEditing] = useState(false);
  const act = useAction();
  const { user: me } = useAuth();
  useTitle(`${ghsa} · ${ctx.repo.full_name}`);
  const manager = ctx.isAdmin;
  return (
    <Async state={state}>
      {(a) => {
        const setState = (st: string) => act.run(async () => state.setData(await sec.updateAdvisory(ctx.owner, ctx.name, a.ghsa_id, { state: st })));
        if (editing) {
          return (
            <div data-testid="advisory-edit">
              <PageHead title={`Edit ${a.ghsa_id}`} />
              <AdvisoryForm start={a} manager={manager} submitLabel="Update advisory" onCancel={() => setEditing(false)}
                onSubmit={async (b) => {
                  state.setData(await sec.updateAdvisory(ctx.owner, ctx.name, a.ghsa_id, b));
                  setEditing(false);
                }} />
            </div>
          );
        }
        const canEdit = manager || (!!me && (a.author?.login === me.login || a.collaborating_users.some((u) => u.login === me.login)));
        return (
          <div data-testid="advisory-page">
            <div className="sec-adv-head">
              <h2 className="sec-title" data-testid="advisory-title">{a.summary}</h2>
              <div className="sec-row-meta">
                <AlertState state={a.state} />
                <span className="mono small">{a.ghsa_id}</span>
                {a.author && <span className="muted small"><UserLink user={a.author} /> {a.submission ? "reported" : "opened"} this advisory <RelTime iso={a.created_at} /></span>}
                {a.published_at && <span className="muted small">· published <RelTime iso={a.published_at} />{a.publisher && <> by <UserLink user={a.publisher} /></>}</span>}
              </div>
            </div>
            {a.state === "triage" && (
              <div className="flash flash-warn" data-testid="triage-banner">
                This advisory was privately reported{a.author ? ` by ${a.author.login}` : ""} and needs triage.
                {manager && (
                  <span className="sec-inline-actions">
                    <button type="button" className="btn btn-sm btn-primary" disabled={act.busy} onClick={() => setState("draft")} data-testid="accept-report">Accept and open as draft</button>
                    <button type="button" className="btn btn-sm" disabled={act.busy} onClick={() => setState("closed")} data-testid="reject-report">Close report</button>
                  </span>
                )}
              </div>
            )}
            <InlineError error={act.error} />
            <div className="layout-sidebar">
              <div className="layout-main">
                <div className="box">
                  <div className="box-header">
                    <strong>Description</strong>
                    <span className="grow" />
                    {(manager || canEdit) && a.state !== "withdrawn" && <button type="button" className="btn btn-sm" onClick={() => setEditing(true)} data-testid="edit-advisory">Edit advisory</button>}
                  </div>
                  <div className="box-body">
                    {a.description ? <Markdown source={a.description} repoPath={ctx.base} /> : <p className="muted">No description provided.</p>}
                  </div>
                </div>
                <div className="box">
                  <div className="box-header"><strong>Affected products</strong></div>
                  <ul className="list">
                    {a.vulnerabilities.map((v, i) => (
                      <li key={i} className="list-row sec-vuln-row" data-testid="advisory-product">
                        <div className="grow">
                          <div><span className="muted small">{ecoLabel(v.package.ecosystem)}</span> <strong className="mono">{v.package.name || "(any package)"}</strong></div>
                          <div className="small">Affected versions: <code>{v.vulnerable_version_range || "unknown"}</code> · Patched versions: <code>{v.patched_versions || "none"}</code></div>
                        </div>
                      </li>
                    ))}
                  </ul>
                </div>
                {manager && a.state === "draft" && (
                  <div className="box box-body" data-testid="publish-box">
                    <h3 className="sec-form-h">Publish advisory</h3>
                    <p className="muted small">Publishing makes this advisory public and adds it to the advisory database, so Dependabot alerts repositories that depend on the affected versions.</p>
                    <div className="form-actions sec-left">
                      <button type="button" className="btn btn-primary" disabled={act.busy} onClick={() => setState("published")} data-testid="publish-advisory">Publish advisory</button>
                      <button type="button" className="btn btn-danger-outline" disabled={act.busy} onClick={() => setState("closed")} data-testid="close-advisory">Close draft</button>
                    </div>
                  </div>
                )}
                {manager && a.state === "closed" && (
                  <div className="box box-body">
                    <button type="button" className="btn" disabled={act.busy} onClick={() => setState("draft")} data-testid="reopen-advisory">Reopen as draft</button>
                  </div>
                )}
              </div>
              <aside className="layout-aside sec-aside">
                <div className="sidebar-section">
                  <h3 className="sidebar-heading">Severity</h3>
                  <SeverityLabel severity={a.severity} />
                  {a.cvss?.score != null && <div className="small sec-mt"><strong>{a.cvss.score.toFixed(1)}</strong> / 10 <div className="mono small sec-wrap">{a.cvss.vector_string}</div></div>}
                </div>
                <div className="sidebar-section" data-testid="cve-section">
                  <h3 className="sidebar-heading">CVE ID</h3>
                  {a.cve_id ? <span className="mono">{a.cve_id}</span> : a.cve_requested_at ? (
                    <span className="muted small" data-testid="cve-requested">CVE requested <RelTime iso={a.cve_requested_at} /></span>
                  ) : (
                    <>
                      <span className="muted small">No CVE</span>
                      {manager && a.state === "draft" && (
                        <div><button type="button" className="btn btn-sm sec-mt" disabled={act.busy} data-testid="request-cve"
                          onClick={() => act.run(async () => { await sec.requestCve(ctx.owner, ctx.name, a.ghsa_id); state.reload(); })}>Request CVE</button></div>
                      )}
                    </>
                  )}
                </div>
                <div className="sidebar-section">
                  <h3 className="sidebar-heading">Weaknesses</h3>
                  {a.cwes.length ? a.cwes.map((c) => <div key={c.cwe_id} className="small"><span className="badge">{c.cwe_id}</span> {c.name}</div>) : <span className="muted small">No CWEs</span>}
                </div>
                <div className="sidebar-section" data-testid="credits-section">
                  <h3 className="sidebar-heading">Credits</h3>
                  {a.credits_detailed.length ? a.credits_detailed.map((c) => (
                    <div key={c.user.login} className="sec-credit small" data-testid="credit">
                      <Avatar user={c.user} size={20} /> <UserLink user={c.user} /> <span className="badge">{humanize(c.type)}</span>
                      {c.state !== "accepted" && <span className="muted"> ({c.state})</span>}
                    </div>
                  )) : <span className="muted small">No credits</span>}
                </div>
                {a.collaborating_users.length > 0 && (
                  <div className="sidebar-section">
                    <h3 className="sidebar-heading">Collaborators</h3>
                    {a.collaborating_users.map((u) => <div key={u.login} className="small"><Avatar user={u} size={20} /> <UserLink user={u} /></div>)}
                  </div>
                )}
                {a.state === "published" && (
                  <div className="sidebar-section">
                    <Link to={`/advisories/${a.ghsa_id}`} className="small" data-testid="global-advisory-link">View in the advisory database</Link>
                  </div>
                )}
              </aside>
            </div>
          </div>
        );
      }}
    </Async>
  );
}
