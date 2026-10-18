// Dependabot alerts: list with filters and alert detail with dismiss / reopen and dismissal
// requests (/{o}/{r}/security/dependabot[/N]).
import { useState, type FormEvent } from "react";
import { Link, useParams, useSearchParams } from "react-router-dom";
import { ApiError } from "../../api";
import { Async, Pagination, RelTime, UserLink } from "../../components/common";
import { useRepo } from "../../components/RepoLayout";
import { useAsync, useTitle } from "../../hooks";
import { Markdown } from "../../markdown";
import { DEPENDABOT_REASONS, ECOSYSTEMS, humanize, isOff, sec, type DependabotAlert, ecoLabel } from "./api";
import { AlertState, FeatureOff, FilterSelect, InlineError, PageHead, SeverityLabel, StateTabs, useAction } from "./ui";
import { RequestBox } from "./Requests";

export function DependabotPage() {
  const ctx = useRepo();
  useTitle(`Dependabot alerts · ${ctx.repo.full_name}`);
  const [sp, setSp] = useSearchParams();
  const state = sp.get("state") || "open";
  const severity = sp.get("severity") || "";
  const ecosystem = sp.get("ecosystem") || "";
  const scope = sp.get("scope") || "";
  const pkg = sp.get("package") || "";
  const manifest = sp.get("manifest") || "";
  const sort = sp.get("sort") || "created";
  const page = Number(sp.get("page") || 1);
  const set = (k: string, v: string) => {
    const n = new URLSearchParams(sp);
    if (v) n.set(k, v);
    else n.delete(k);
    if (k !== "page") n.delete("page");
    setSp(n, { replace: true });
  };
  const closed = "dismissed,fixed,auto_dismissed";
  const q = { severity, ecosystem, scope, package: pkg, manifest, sort, direction: "desc", page };
  const list = useAsync(() => sec.dependabotAlerts(ctx.owner, ctx.name, { ...q, state: state === "open" ? "open" : closed }),
    [ctx.owner, ctx.name, state, severity, ecosystem, scope, pkg, manifest, sort, page]);
  const counts = useAsync(async () => {
    const [o, c] = await Promise.all([
      sec.dependabotAlerts(ctx.owner, ctx.name, { state: "open", per_page: 100 }),
      sec.dependabotAlerts(ctx.owner, ctx.name, { state: closed, per_page: 100 }),
    ]);
    return { open: o.items.length, closed: c.items.length };
  }, [ctx.owner, ctx.name, list.data]);
  const [pkgInput, setPkgInput] = useState(pkg);
  if (list.error && isOff(list.error)) {
    return (
      <>
        <PageHead title="Dependabot alerts" />
        <FeatureOff title="Dependabot alerts are disabled" isAdmin={ctx.isAdmin} settings={`${ctx.base}/settings/security_analysis`}>
          Dependabot alerts tell you when one of your dependencies has a known vulnerability. Enable them in the repository's Advanced Security settings.
        </FeatureOff>
      </>
    );
  }
  return (
    <div data-testid="dependabot-page">
      <PageHead title="Dependabot alerts" sub="Dependabot alerts are generated from the dependency graph of the default branch and the advisory database.">
        <Link className="btn btn-sm" to={`${ctx.base}/network/dependencies`}>Dependency graph</Link>
      </PageHead>
      <div className="sec-filters" data-testid="dependabot-filters">
        <form onSubmit={(e) => { e.preventDefault(); set("package", pkgInput.trim()); }} className="sec-search">
          <input className="input input-sm" aria-label="Filter by package" placeholder="Filter by package" value={pkgInput}
            onChange={(e) => setPkgInput(e.target.value)} data-testid="package-filter" />
        </form>
        <FilterSelect id="db-sev" label="Severity" value={severity} onChange={(v) => set("severity", v)}
          options={[["critical", "Critical"], ["high", "High"], ["medium", "Moderate"], ["low", "Low"]]} />
        <FilterSelect id="db-eco" label="Ecosystem" value={ecosystem} onChange={(v) => set("ecosystem", v)} options={ECOSYSTEMS.filter(([k]) => k !== "other")} />
        <FilterSelect id="db-scope" label="Scope" value={scope} onChange={(v) => set("scope", v)} options={[["runtime", "Runtime"], ["development", "Development"]]} />
        <label className="sec-filter">
          <span className="sr-only">Sort</span>
          <select className="input input-sm input-auto" aria-label="Sort" value={sort} onChange={(e) => set("sort", e.target.value)}>
            <option value="created">Newest</option>
            <option value="updated">Recently updated</option>
            <option value="epss_percentage">EPSS score</option>
          </select>
        </label>
        {(severity || ecosystem || scope || pkg || manifest) && (
          <button type="button" className="btn btn-sm btn-link" onClick={() => { setPkgInput(""); setSp(new URLSearchParams(state !== "open" ? { state } : {}), { replace: true }); }}>Clear filters</button>
        )}
      </div>
      <div className="box">
        <div className="box-header">
          <StateTabs value={state} onChange={(v) => set("state", v === "open" ? "" : v)}
            tabs={[{ key: "open", label: "Open", count: counts.data?.open }, { key: "closed", label: "Closed", count: counts.data?.closed }]} />
        </div>
        <Async state={list}>
          {(p) => p.items.length === 0 ? (
            <div className="blankslate" data-testid="dependabot-empty">
              <h3>{state === "open" ? "No open alerts" : "No closed alerts"}</h3>
              <p className="muted">{state === "open" ? "Dependabot hasn't found vulnerable dependencies matching these filters." : "Dismissed and fixed alerts appear here."}</p>
            </div>
          ) : (
            <ul className="list">
              {p.items.map((a) => <DependabotRow key={a.number} a={a} base={ctx.base} onPick={set} />)}
            </ul>
          )}
        </Async>
      </div>
      {list.data && <Pagination page={page} next={list.data.next} prev={list.data.prev} last={list.data.last} onPage={(n) => set("page", String(n))} />}
    </div>
  );
}

function DependabotRow({ a, base, onPick }: { a: DependabotAlert; base: string; onPick: (k: string, v: string) => void }) {
  return (
    <li className="list-row sec-alert-row" data-testid="dependabot-row">
      <div className="grow sec-row-main">
        <Link className="issue-title" to={`${base}/security/dependabot/${a.number}`}>{a.security_advisory.summary}</Link>
        {" "}<SeverityLabel severity={a.security_vulnerability.severity} />
        {a.dependency.scope === "development" && <span className="badge">Development</span>}
        <div className="muted small sec-row-meta">
          #{a.number}
          {a.state === "open" ? <RelTime iso={a.created_at} prefix="opened " /> : a.state === "fixed" ? <RelTime iso={a.fixed_at} prefix="fixed " /> : <RelTime iso={a.dismissed_at || a.auto_dismissed_at} prefix={a.state === "auto_dismissed" ? "auto-dismissed " : "dismissed "} />}
          · <button type="button" className="btn-link sec-linkish" onClick={() => onPick("package", a.dependency.package.name)}>{a.dependency.package.name}</button>
          ({ecoLabel(a.dependency.package.ecosystem)})
          · <button type="button" className="btn-link sec-linkish mono" onClick={() => onPick("manifest", a.dependency.manifest_path)}>{a.dependency.manifest_path}</button>
        </div>
      </div>
      {a.state !== "open" && <AlertState state={a.state} />}
    </li>
  );
}

export function DependabotAlertPage() {
  const ctx = useRepo();
  const { number = "0" } = useParams();
  const n = Number(number);
  const state = useAsync(() => sec.dependabotAlert(ctx.owner, ctx.name, n), [ctx.owner, ctx.name, n]);
  const req = useAsync(() => sec.dismissalRequest(ctx.owner, ctx.name, "dependabot", n).catch(() => undefined), [ctx.owner, ctx.name, n, state.data]);
  useTitle(`Dependabot alert #${n} · ${ctx.repo.full_name}`);
  return (
    <Async state={state}>
      {(a) => {
        const adv = a.security_advisory;
        const vuln = a.security_vulnerability;
        return (
          <div data-testid="dependabot-alert">
            <div className="sec-adv-head">
              <h2 className="sec-title" data-testid="alert-title">{adv.summary}</h2>
              <div className="sec-row-meta">
                <AlertState state={a.state} />
                <span className="muted small">Dependabot alert #{a.number} opened <RelTime iso={a.created_at} /> · <span className="mono">{a.dependency.manifest_path}</span></span>
              </div>
            </div>
            {a.state === "dismissed" && (
              <div className="flash sec-note" data-testid="dismissed-note">
                <UserLink user={a.dismissed_by} /> dismissed this alert as <strong>{humanize(a.dismissed_reason)}</strong> <RelTime iso={a.dismissed_at} />
                {a.dismissed_comment && <div className="small sec-mt">“{a.dismissed_comment}”</div>}
              </div>
            )}
            {a.state === "fixed" && <div className="flash flash-success">This alert was fixed <RelTime iso={a.fixed_at} />: the manifest no longer has a vulnerable version.</div>}
            {a.state === "auto_dismissed" && <div className="flash">Auto-dismissed <RelTime iso={a.auto_dismissed_at} /> by a Dependabot auto-triage rule (development dependency, resource-exhaustion advisory).</div>}
            {req.data && <RequestBox req={req.data} kind="dependabot" n={n} onChange={() => { req.reload(); state.reload(); }} />}
            <div className="layout-sidebar">
              <div className="layout-main">
                <div className="box">
                  <div className="box-header">
                    <strong className="mono">{vuln.package.name}</strong>
                    <span className="muted small">({ecoLabel(vuln.package.ecosystem)}) · {a.dependency.scope || "unknown"} dependency</span>
                    <span className="grow" />
                    <AlertActions a={a} onChange={(x) => { state.setData(x); req.reload(); }} hasPendingRequest={req.data?.status === "pending"} />
                  </div>
                  <div className="box-body sec-kv">
                    <div><span className="muted">Affected versions</span> <code>{vuln.vulnerable_version_range}</code></div>
                    <div><span className="muted">Patched version</span> <code>{vuln.first_patched_version?.identifier || "No fix available"}</code></div>
                    {vuln.first_patched_version && (
                      <p className="small muted sec-mt">Upgrade <strong>{vuln.package.name}</strong> to version {vuln.first_patched_version.identifier} or later in <span className="mono">{a.dependency.manifest_path}</span>.</p>
                    )}
                  </div>
                </div>
                <div className="box">
                  <div className="box-header"><strong>Advisory details</strong></div>
                  <div className="box-body"><Markdown source={adv.description || adv.summary} /></div>
                </div>
              </div>
              <aside className="layout-aside sec-aside">
                <div className="sidebar-section">
                  <h3 className="sidebar-heading">Severity</h3>
                  <SeverityLabel severity={adv.severity} />
                  {adv.cvss?.score ? <div className="small sec-mt"><strong>{adv.cvss.score.toFixed(1)}</strong> / 10 <div className="mono small sec-wrap">{adv.cvss.vector_string}</div></div> : null}
                </div>
                <div className="sidebar-section">
                  <h3 className="sidebar-heading">Identifiers</h3>
                  <div className="small"><Link to={`/advisories/${adv.ghsa_id}`} data-testid="advisory-link">{adv.ghsa_id}</Link></div>
                  {adv.cve_id && <div className="small mono">{adv.cve_id}</div>}
                </div>
                {adv.cwes.length > 0 && (
                  <div className="sidebar-section">
                    <h3 className="sidebar-heading">Weaknesses</h3>
                    {adv.cwes.map((c) => <div key={c.cwe_id} className="small"><span className="badge">{c.cwe_id}</span> {c.name}</div>)}
                  </div>
                )}
                {adv.references.length > 0 && (
                  <div className="sidebar-section">
                    <h3 className="sidebar-heading">References</h3>
                    {adv.references.slice(0, 6).map((r) => <div key={r.url} className="small sec-wrap"><a href={r.url} rel="noreferrer noopener" target="_blank">{r.url}</a></div>)}
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

function AlertActions({ a, onChange, hasPendingRequest }: { a: DependabotAlert; onChange: (a: DependabotAlert) => void; hasPendingRequest: boolean }) {
  const ctx = useRepo();
  const [open, setOpen] = useState(false);
  const [reason, setReason] = useState("");
  const [comment, setComment] = useState("");
  const act = useAction();
  if (!ctx.canPush) return null;
  if (a.state === "dismissed") {
    return (
      <span>
        <button type="button" className="btn btn-sm" disabled={act.busy} data-testid="reopen-alert"
          onClick={() => act.run(async () => onChange(await sec.updateDependabot(ctx.owner, ctx.name, a.number, { state: "open" })))}>Reopen</button>
        <InlineError error={act.error} />
      </span>
    );
  }
  if (a.state !== "open") return null;
  if (hasPendingRequest) return <span className="badge badge-warn">Dismissal requested</span>;
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    await act.run(async () => {
      try {
        onChange(await sec.updateDependabot(ctx.owner, ctx.name, a.number, { state: "dismissed", dismissed_reason: reason, dismissed_comment: comment || undefined }));
      } catch (err) {
        // Delegated alert dismissal: non-reviewers request a dismissal instead.
        if (err instanceof ApiError && err.status === 403 && /delegated/i.test(err.message)) {
          await sec.createDependabotRequest(ctx.owner, ctx.name, a.number, { dismissed_reason: reason, dismissed_comment: comment || undefined });
          onChange({ ...a });
        } else throw err;
      }
      setOpen(false);
    });
  };
  return (
    <div className="menu-wrap sec-dismiss">
      <button type="button" className="btn btn-sm" aria-expanded={open} onClick={() => setOpen(!open)} data-testid="dismiss-alert">Dismiss alert ▾</button>
      {open && (
        <form className="menu sec-dismiss-menu" onSubmit={submit} data-testid="dismiss-form">
          <div className="menu-header">Select a reason to dismiss</div>
          {DEPENDABOT_REASONS.map(([k, l]) => (
            <label key={k} className="radio-row sec-radio">
              <input type="radio" name="dismiss-reason" value={k} checked={reason === k} onChange={() => setReason(k)} /> {l}
            </label>
          ))}
          <div className="sec-pad">
            <textarea className="input textarea" rows={3} aria-label="Dismissal comment" placeholder="Add a comment (optional)" value={comment}
              onChange={(e) => setComment(e.target.value)} maxLength={280} />
            <InlineError error={act.error} />
            <button type="submit" className="btn btn-sm btn-danger sec-mt" disabled={!reason || act.busy} data-testid="dismiss-submit">Dismiss alert</button>
          </div>
        </form>
      )}
    </div>
  );
}
