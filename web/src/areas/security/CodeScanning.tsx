// Code scanning: alerts list with tool / branch / severity filters, alert detail with code snippet,
// dismiss / reopen and dismissal requests; tools page with analyses, SARIF uploads and default setup.
import { useEffect, useState, type FormEvent } from "react";
import { Link, useParams, useSearchParams } from "react-router-dom";
import { api, ApiError } from "../../api";
import { Async, Pagination, RelTime, UserLink } from "../../components/common";
import { useRepo } from "../../components/RepoLayout";
import { useAsync, useTitle, type AsyncState } from "../../hooks";
import { Markdown } from "../../markdown";
import { CODE_REASONS, humanize, isOff, sec, type CodeScanningAlert, type DefaultSetup } from "./api";
import { AlertState, FeatureOff, FilterSelect, InlineError, PageHead, SeverityLabel, StateTabs, useAction } from "./ui";
import { RequestBox } from "./Requests";

const shortRef = (r: string) => r.replace(/^refs\/heads\//, "").replace(/^refs\/pull\/(\d+)\/(head|merge)$/, "#$1");

function RuleSeverity({ a }: { a: CodeScanningAlert }) {
  if (a.rule.security_severity_level) return <SeverityLabel severity={a.rule.security_severity_level} />;
  const s = a.rule.severity || "warning";
  return <span className={`sec-sev sec-sev-${s === "error" ? "high" : s === "warning" ? "medium" : "low"}`}>{humanize(s)}</span>;
}

export function CodeScanningPage() {
  const ctx = useRepo();
  useTitle(`Code scanning · ${ctx.repo.full_name}`);
  const [sp, setSp] = useSearchParams();
  const state = sp.get("state") || "open";
  const tool = sp.get("tool") || "";
  const ref = sp.get("ref") || "";
  const severity = sp.get("severity") || "";
  const page = Number(sp.get("page") || 1);
  const set = (k: string, v: string) => {
    const n = new URLSearchParams(sp);
    if (v) n.set(k, v);
    else n.delete(k);
    if (k !== "page") n.delete("page");
    setSp(n, { replace: true });
  };
  const analyses = useAsync(() => sec.analyses(ctx.owner, ctx.name).catch(() => []), [ctx.owner, ctx.name]);
  const tools = [...new Set((analyses.data || []).map((a) => a.tool.name))];
  const refs = [...new Set((analyses.data || []).map((a) => a.ref))];
  const list = useAsync(() => sec.codeAlerts(ctx.owner, ctx.name, { state: state === "open" ? "open" : "closed", tool_name: tool, ref, severity, page }),
    [ctx.owner, ctx.name, state, tool, ref, severity, page]);
  const counts = useAsync(async () => {
    const [o, c] = await Promise.all([
      sec.codeAlerts(ctx.owner, ctx.name, { state: "open", per_page: 100, tool_name: tool, ref }),
      sec.codeAlerts(ctx.owner, ctx.name, { state: "closed", per_page: 100, tool_name: tool, ref }),
    ]);
    return { open: o.items.length, closed: c.items.length };
  }, [ctx.owner, ctx.name, tool, ref, list.data]);
  if (list.error && isOff(list.error)) {
    return (
      <>
        <PageHead title="Code scanning" />
        <FeatureOff title="You don't have access to code scanning alerts" isAdmin={false} settings="">Code scanning alerts are visible to people with write access.</FeatureOff>
      </>
    );
  }
  return (
    <div data-testid="code-scanning-page">
      <PageHead title="Code scanning" sub="Automatically detect vulnerabilities and coding errors. Results are uploaded as SARIF by your analysis tools.">
        <Link className="btn btn-sm" to={`${ctx.base}/security/code-scanning/tools`} data-testid="code-tools-link">Tools</Link>
      </PageHead>
      <div className="sec-filters" data-testid="code-filters">
        <FilterSelect id="cs-tool" label="Tool" value={tool} onChange={(v) => set("tool", v)} options={tools.map((t) => [t, t])} />
        <FilterSelect id="cs-ref" label="Branch" value={ref} onChange={(v) => set("ref", v)} options={refs.map((r) => [r, shortRef(r)])} />
        <FilterSelect id="cs-sev" label="Severity" value={severity} onChange={(v) => set("severity", v)}
          options={[["critical", "Critical"], ["high", "High"], ["medium", "Medium"], ["low", "Low"], ["error", "Error"], ["warning", "Warning"], ["note", "Note"]]} />
      </div>
      <div className="box">
        <div className="box-header">
          <StateTabs value={state} onChange={(v) => set("state", v === "open" ? "" : v)}
            tabs={[{ key: "open", label: "Open", count: counts.data?.open }, { key: "closed", label: "Closed", count: counts.data?.closed }]} />
        </div>
        <Async state={list}>
          {(p) => p.items.length === 0 ? (
            <div className="blankslate" data-testid="code-empty">
              <h3>{(analyses.data || []).length === 0 ? "No code scanning results" : state === "open" ? "No open alerts" : "No closed alerts"}</h3>
              <p className="muted">
                {(analyses.data || []).length === 0
                  ? <>Upload a SARIF file from your analysis tool on the <Link to={`${ctx.base}/security/code-scanning/tools`}>tools page</Link> or with the code scanning API.</>
                  : "Nothing matches these filters."}
              </p>
            </div>
          ) : (
            <ul className="list">
              {p.items.map((a) => (
                <li key={a.number} className="list-row sec-alert-row" data-testid="code-row">
                  <div className="grow sec-row-main">
                    <Link className="issue-title" to={`${ctx.base}/security/code-scanning/${a.number}`}>{a.rule.description || a.rule.id}</Link>
                    {" "}<RuleSeverity a={a} />
                    <div className="muted small sec-row-meta">
                      #{a.number} <RelTime iso={a.created_at} prefix="opened " /> · {a.tool.name}
                      · <span className="mono">{a.most_recent_instance.location.path}:{a.most_recent_instance.location.start_line}</span>
                      · {shortRef(a.most_recent_instance.ref)}
                    </div>
                  </div>
                  {a.state !== "open" && <AlertState state={a.state} />}
                </li>
              ))}
            </ul>
          )}
        </Async>
      </div>
      {list.data && <Pagination page={page} next={list.data.next} prev={list.data.prev} last={list.data.last} onPage={(n) => set("page", String(n))} />}
    </div>
  );
}

/** Lines around an alert location at the analysed commit. */
function Snippet({ path, sha, start, end }: { path: string; sha: string; start: number; end: number }) {
  const ctx = useRepo();
  const text = useAsync(() => api.rawFile(ctx.owner, ctx.name, path, sha), [ctx.owner, ctx.name, path, sha]);
  if (!text.data) return text.error ? <p className="muted small">The source file is not available at {sha.slice(0, 7)}.</p> : null;
  const lines = text.data.split("\n");
  const from = Math.max(1, start - 2);
  const to = Math.min(lines.length, Math.max(end, start) + 2);
  return (
    <div className="sec-snippet" data-testid="code-snippet">
      <div className="sec-snippet-head mono small">
        <Link to={`${ctx.base}/blob/${sha}/${path}#L${start}`}>{path}</Link>
      </div>
      <table className="code-table">
        <tbody>
          {lines.slice(from - 1, to).map((l, i) => {
            const no = from + i;
            const hl = no >= start && no <= Math.max(end, start);
            return (
              <tr key={no} className={hl ? "sec-hl" : ""}>
                <td className="sec-ln mono">{no}</td>
                <td className="mono sec-code">{l || " "}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

export function CodeScanningAlertPage() {
  const ctx = useRepo();
  const { number = "0" } = useParams();
  const n = Number(number);
  const state = useAsync(() => sec.codeAlert(ctx.owner, ctx.name, n), [ctx.owner, ctx.name, n]);
  const instances = useAsync(() => sec.codeInstances(ctx.owner, ctx.name, n).catch(() => []), [ctx.owner, ctx.name, n]);
  const req = useAsync(() => sec.dismissalRequest(ctx.owner, ctx.name, "code-scanning", n).catch(() => undefined), [ctx.owner, ctx.name, n, state.data]);
  useTitle(`Code scanning alert #${n} · ${ctx.repo.full_name}`);
  return (
    <Async state={state}>
      {(a) => {
        const inst = a.most_recent_instance;
        return (
          <div data-testid="code-alert">
            <div className="sec-adv-head">
              <h2 className="sec-title" data-testid="alert-title">{a.rule.description || a.rule.id}</h2>
              <div className="sec-row-meta">
                <AlertState state={a.state} />
                <span className="muted small">Code scanning alert #{a.number} · {a.tool.name}{a.tool.version ? ` ${a.tool.version}` : ""} · opened <RelTime iso={a.created_at} /> in {shortRef(inst.ref)}</span>
              </div>
            </div>
            {a.state === "dismissed" && (
              <div className="flash sec-note" data-testid="dismissed-note">
                <UserLink user={a.dismissed_by} /> dismissed this alert as <strong>{a.dismissed_reason}</strong> <RelTime iso={a.dismissed_at} />
                {a.dismissed_comment && <div className="small sec-mt">“{a.dismissed_comment}”</div>}
              </div>
            )}
            {a.state === "fixed" && <div className="flash flash-success">Fixed <RelTime iso={a.fixed_at} />: the latest analysis no longer reports this alert.</div>}
            {req.data && <RequestBox req={req.data} kind="code-scanning" n={n} onChange={() => { req.reload(); state.reload(); }} />}
            <div className="layout-sidebar">
              <div className="layout-main">
                <div className="box">
                  <div className="box-header">
                    <strong className="mono sec-wrap">{inst.location.path}:{inst.location.start_line}</strong>
                    <span className="grow" />
                    <CodeActions a={a} onChange={(x) => { state.setData(x); req.reload(); }} pending={req.data?.status === "pending"} />
                  </div>
                  <div className="box-body">
                    <Snippet path={inst.location.path} sha={inst.commit_sha} start={inst.location.start_line} end={inst.location.end_line} />
                    <div className="sec-message" data-testid="alert-message">{inst.message.text}</div>
                  </div>
                </div>
                {(a.rule.help || a.rule.full_description) && (
                  <div className="box">
                    <div className="box-header"><strong>Rule help</strong></div>
                    <div className="box-body">
                      {a.rule.full_description && <p>{a.rule.full_description}</p>}
                      {a.rule.help && <Markdown source={a.rule.help} />}
                      {a.rule.help_uri && <a href={a.rule.help_uri} target="_blank" rel="noreferrer noopener" className="small">More about this rule</a>}
                    </div>
                  </div>
                )}
                <div className="box">
                  <div className="box-header"><strong>Affected branches</strong></div>
                  <ul className="list">
                    {(instances.data || []).map((i) => (
                      <li key={i.ref + i.analysis_key + i.category} className="list-row" data-testid="code-instance">
                        <span className="grow"><span className="mono">{shortRef(i.ref)}</span> <span className="muted small">{i.category || i.analysis_key}</span></span>
                        <AlertState state={i.state} />
                      </li>
                    ))}
                  </ul>
                </div>
              </div>
              <aside className="layout-aside sec-aside">
                <div className="sidebar-section">
                  <h3 className="sidebar-heading">Severity</h3>
                  <RuleSeverity a={a} />
                </div>
                <div className="sidebar-section">
                  <h3 className="sidebar-heading">Rule</h3>
                  <div className="mono small">{a.rule.id}</div>
                  {a.rule.name && <div className="small muted">{a.rule.name}</div>}
                </div>
                {(a.rule.tags || []).length > 0 && (
                  <div className="sidebar-section">
                    <h3 className="sidebar-heading">Tags</h3>
                    {(a.rule.tags || []).map((t) => <span key={t} className="badge sec-tag">{t}</span>)}
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

function CodeActions({ a, onChange, pending }: { a: CodeScanningAlert; onChange: (a: CodeScanningAlert) => void; pending: boolean }) {
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
          onClick={() => act.run(async () => onChange(await sec.updateCode(ctx.owner, ctx.name, a.number, { state: "open" })))}>Reopen alert</button>
        <InlineError error={act.error} />
      </span>
    );
  }
  if (a.state !== "open") return null;
  if (pending) return <span className="badge badge-warn">Dismissal requested</span>;
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    await act.run(async () => {
      const b = { state: "dismissed", dismissed_reason: reason, dismissed_comment: comment || undefined };
      try {
        onChange(await sec.updateCode(ctx.owner, ctx.name, a.number, b));
      } catch (err) {
        if (err instanceof ApiError && err.status === 403 && /delegated/i.test(err.message)) {
          onChange(await sec.updateCode(ctx.owner, ctx.name, a.number, { ...b, create_request: true }));
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
          {CODE_REASONS.map(([k, l]) => (
            <label key={k} className="radio-row sec-radio">
              <input type="radio" name="code-reason" value={k} checked={reason === k} onChange={() => setReason(k)} /> {l}
            </label>
          ))}
          <div className="sec-pad">
            <textarea className="input textarea" rows={3} aria-label="Dismissal comment" placeholder="Dismissal comment (optional)" value={comment}
              onChange={(e) => setComment(e.target.value)} maxLength={280} />
            <InlineError error={act.error} />
            <button type="submit" className="btn btn-sm btn-danger sec-mt" disabled={!reason || act.busy} data-testid="dismiss-submit">Dismiss alert</button>
          </div>
        </form>
      )}
    </div>
  );
}

// ---------- tools: analyses, SARIF upload, default setup ----------

async function gzipBase64(text: string): Promise<string> {
  const stream = new Blob([text]).stream().pipeThrough(new CompressionStream("gzip"));
  const buf = new Uint8Array(await new Response(stream).arrayBuffer());
  let bin = "";
  for (let i = 0; i < buf.length; i += 0x8000) bin += String.fromCharCode(...buf.subarray(i, i + 0x8000));
  return btoa(bin);
}

export function CodeScanningToolsPage() {
  const ctx = useRepo();
  useTitle(`Code scanning tools · ${ctx.repo.full_name}`);
  const analyses = useAsync(() => sec.analyses(ctx.owner, ctx.name), [ctx.owner, ctx.name]);
  const act = useAction();
  const [tool, setTool] = useState("");
  return (
    <div data-testid="code-tools-page">
      <PageHead title="Code scanning tools" sub="Analyses uploaded for this repository, by tool.">
        <Link className="btn btn-sm" to={`${ctx.base}/security/code-scanning`}>Back to alerts</Link>
      </PageHead>
      {ctx.isAdmin && <DefaultSetupBox />}
      {ctx.canPush && <SarifUpload onDone={analyses.reload} />}
      <div className="box">
        <div className="box-header">
          <h3>Analyses</h3>
          <span className="grow" />
          <FilterSelect id="tools-tool" label="Tool" value={tool} onChange={setTool}
            options={[...new Set((analyses.data || []).map((a) => a.tool.name))].map((t) => [t, t])} />
        </div>
        <InlineError error={act.error} />
        <Async state={analyses}>
          {(list) => {
            const rows = list.filter((a) => !tool || a.tool.name === tool);
            return rows.length === 0 ? <div className="blankslate"><h3>No analyses yet</h3></div> : (
              <ul className="list">
                {rows.map((a) => (
                  <li key={a.id} className="list-row sec-alert-row" data-testid="analysis-row">
                    <div className="grow sec-row-main">
                      <strong>{a.tool.name}</strong> {a.tool.version && <span className="muted small">{a.tool.version}</span>}
                      {a.category && <span className="badge">{a.category}</span>}
                      <div className="muted small sec-row-meta">
                        {shortRef(a.ref)} · <span className="mono">{a.commit_sha.slice(0, 7)}</span> · {a.results_count} result{a.results_count === 1 ? "" : "s"} · {a.rules_count} rule{a.rules_count === 1 ? "" : "s"} · <RelTime iso={a.created_at} />
                        · SARIF <span className="mono">{a.sarif_id.slice(0, 8)}</span>
                      </div>
                      {a.error && <div className="flash flash-error small">{a.error}</div>}
                      {a.warning && <div className="muted small">{a.warning}</div>}
                    </div>
                    {ctx.canPush && a.deletable && (
                      <button type="button" className="btn btn-sm btn-danger-outline" disabled={act.busy} data-testid="delete-analysis"
                        onClick={() => {
                          const last = list.filter((x) => x.tool.name === a.tool.name && x.ref === a.ref && x.category === a.category).length === 1;
                          if (!window.confirm(last ? "This is the last analysis for this tool and branch: deleting it removes its alerts. Continue?" : "Delete this analysis?")) return;
                          act.run(async () => { await sec.deleteAnalysis(ctx.owner, ctx.name, a.id, last); analyses.reload(); });
                        }}>Delete</button>
                    )}
                  </li>
                ))}
              </ul>
            );
          }}
        </Async>
      </div>
    </div>
  );
}

function SarifUpload({ onDone }: { onDone: () => void }) {
  const ctx = useRepo();
  const [file, setFile] = useState<File | null>(null);
  const [ref, setRef] = useState(`refs/heads/${ctx.repo.default_branch}`);
  const [sha, setSha] = useState("");
  const [sarifId, setSarifId] = useState("");
  const [status, setStatus] = useState<{ processing_status: string; errors: string[] | null } | null>(null);
  const act = useAction();
  useEffect(() => {
    api.commits(ctx.owner, ctx.name, ref.replace(/^refs\/heads\//, ""), 1).then((p) => p.items[0] && setSha(p.items[0].sha)).catch(() => {});
  }, [ctx.owner, ctx.name, ref]);
  const check = async (id: string) => {
    const s = await sec.sarifStatus(ctx.owner, ctx.name, id);
    setStatus(s);
    return s.processing_status;
  };
  useEffect(() => {
    if (!sarifId || status?.processing_status !== "pending") return;
    const t = setTimeout(() => check(sarifId).then((st) => st !== "pending" && onDone()).catch(() => {}), 1000);
    return () => clearTimeout(t);
  });
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!file) return;
    await act.run(async () => {
      const sarif = await gzipBase64(await file.text());
      const r = await sec.uploadSarif(ctx.owner, ctx.name, { commit_sha: sha, ref, sarif });
      setSarifId(r.id);
      setStatus({ processing_status: "pending", errors: null });
      await check(r.id);
    });
  };
  return (
    <form className="box" onSubmit={submit} data-testid="sarif-upload">
      <div className="box-header"><h3>Upload SARIF</h3></div>
      <div className="box-body">
        <p className="muted small">Upload results from any SARIF 2.1.0 producing tool. Alerts appear once processing completes.</p>
        <div className="sec-grid">
          <div className="form-group">
            <label htmlFor="sarif-file">SARIF file</label>
            <input id="sarif-file" type="file" accept=".sarif,.json,application/json" onChange={(e) => setFile(e.target.files?.[0] || null)} />
          </div>
          <div className="form-group">
            <label htmlFor="sarif-ref">Ref</label>
            <input id="sarif-ref" className="input mono" value={ref} onChange={(e) => setRef(e.target.value)} />
          </div>
          <div className="form-group">
            <label htmlFor="sarif-sha">Commit SHA</label>
            <input id="sarif-sha" className="input mono" value={sha} onChange={(e) => setSha(e.target.value)} pattern="[0-9a-f]{40}" required />
          </div>
        </div>
        <InlineError error={act.error} />
        <div className="form-actions sec-left">
          <button type="submit" className="btn btn-primary btn-sm" disabled={!file || !sha || act.busy} data-testid="sarif-submit">{act.busy ? "Uploading…" : "Upload"}</button>
          {sarifId && status && (
            <span className="small" data-testid="sarif-status">
              SARIF <span className="mono">{sarifId.slice(0, 8)}</span>: <strong>{status.processing_status}</strong>
              {status.errors?.length ? <> — {status.errors.join("; ")}</> : null}
            </span>
          )}
        </div>
      </div>
    </form>
  );
}

function DefaultSetupBox() {
  const ctx = useRepo();
  const st = useAsync(() => sec.defaultSetup(ctx.owner, ctx.name), [ctx.owner, ctx.name]);
  const act = useAction();
  const [suite, setSuite] = useState("default");
  useEffect(() => { if (st.data) setSuite(st.data.query_suite); }, [st.data]);
  return <DefaultSetupView st={st} act={act} suite={suite} setSuite={setSuite} owner={ctx.owner} name={ctx.name} />;
}

export function DefaultSetupView({ st, act, suite, setSuite, owner, name }: {
  st: AsyncState<DefaultSetup>;
  act: ReturnType<typeof useAction>;
  suite: string;
  setSuite: (s: string) => void;
  owner: string;
  name: string;
}) {
  return (
    <div className="box" data-testid="default-setup">
      <div className="box-header"><h3>CodeQL analysis: default setup</h3></div>
      <div className="box-body">
        <Async state={st}>
          {(d) => (
            <>
              <p className="small">
                Status: <strong data-testid="default-setup-state">{d.state === "configured" ? "Configured" : "Not configured"}</strong>
                {d.languages.length > 0 && <> · Languages: {d.languages.join(", ")}</>}
                {d.updated_at && <> · updated <RelTime iso={d.updated_at} /></>}
              </p>
              <p className="muted small">This server stores the default setup configuration but does not run CodeQL itself; upload SARIF from your own analysis to get alerts.</p>
              <div className="form-row">
                <div className="form-group">
                  <label htmlFor={`suite-${name}`}>Query suite</label>
                  <select id={`suite-${name}`} className="input input-auto" value={suite} onChange={(e) => setSuite(e.target.value)}>
                    <option value="default">Default</option>
                    <option value="extended">Extended</option>
                  </select>
                </div>
                <button type="button" className="btn btn-sm" disabled={act.busy} data-testid="default-setup-toggle"
                  onClick={() => act.run(async () => {
                    await sec.setDefaultSetup(owner, name, d.state === "configured" ? { state: "not-configured" } : { state: "configured", query_suite: suite });
                    st.reload();
                  })}>
                  {d.state === "configured" ? "Disable CodeQL" : "Enable CodeQL"}
                </button>
                {d.state === "configured" && suite !== d.query_suite && (
                  <button type="button" className="btn btn-sm" disabled={act.busy}
                    onClick={() => act.run(async () => { await sec.setDefaultSetup(owner, name, { state: "configured", query_suite: suite }); st.reload(); })}>Save</button>
                )}
              </div>
              <InlineError error={act.error} />
            </>
          )}
        </Async>
      </div>
    </div>
  );
}
