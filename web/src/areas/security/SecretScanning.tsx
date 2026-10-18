// Secret scanning: alerts list, alert detail with locations and resolve / reopen, push
// protection bypasses (bypassed alerts and delegated bypass requests) and the unblock page that
// GH013 push rejections link to.
import { useState, type FormEvent } from "react";
import { Link, useParams, useSearchParams } from "react-router-dom";
import { ApiError } from "../../api";
import { Async, Pagination, RelTime, UserLink } from "../../components/common";
import { useRepo } from "../../components/RepoLayout";
import { useAsync, useTitle } from "../../hooks";
import { BYPASS_REASONS, humanize, isOff, SECRET_RESOLUTIONS, sec, type SecretAlert } from "./api";
import { RequestBox, RequestList } from "./Requests";
import { AlertState, FeatureOff, FilterSelect, InlineError, PageHead, StateTabs, useAction } from "./ui";

function Disabled() {
  const ctx = useRepo();
  return (
    <>
      <PageHead title="Secret scanning" />
      <FeatureOff title="Secret scanning is disabled" isAdmin={ctx.isAdmin} settings={`${ctx.base}/settings/security_analysis`}>
        Secret scanning finds tokens, keys and other secrets pushed to this repository, and push protection blocks them before they land.
      </FeatureOff>
    </>
  );
}

export function SecretScanningPage() {
  const ctx = useRepo();
  useTitle(`Secret scanning · ${ctx.repo.full_name}`);
  const [sp, setSp] = useSearchParams();
  const state = sp.get("state") || "open";
  const type = sp.get("secret_type") || "";
  const resolution = sp.get("resolution") || "";
  const validity = sp.get("validity") || "";
  const bypassed = sp.get("bypassed") || "";
  const page = Number(sp.get("page") || 1);
  const set = (k: string, v: string) => {
    const n = new URLSearchParams(sp);
    if (v) n.set(k, v);
    else n.delete(k);
    if (k !== "page") n.delete("page");
    setSp(n, { replace: true });
  };
  const q = { secret_type: type, resolution: state === "open" ? "" : resolution, validity, is_bypassed: bypassed ? "true" : undefined, hide_secret: "true" };
  const list = useAsync(() => sec.secretAlerts(ctx.owner, ctx.name, { ...q, state: state === "open" ? "open" : "resolved", page }),
    [ctx.owner, ctx.name, state, type, resolution, validity, bypassed, page]);
  const all = useAsync(() => sec.secretAlerts(ctx.owner, ctx.name, { per_page: 100, hide_secret: "true" }).then((p) => p.items).catch(() => [] as SecretAlert[]), [ctx.owner, ctx.name, list.data]);
  const history = useAsync(() => sec.scanHistory(ctx.owner, ctx.name).catch(() => undefined), [ctx.owner, ctx.name]);
  const types = [...new Map((all.data || []).map((a) => [a.secret_type, a.secret_type_display_name])).entries()];
  if (list.error && isOff(list.error)) return <Disabled />;
  const lastScan = [...(history.data?.incremental_scans || []), ...(history.data?.backfill_scans || [])]
    .map((s) => s.completed_at).filter(Boolean).sort().pop();
  return (
    <div data-testid="secret-scanning-page">
      <PageHead title="Secret scanning" sub="Secrets found in this repository's history and in new pushes.">
        <Link className="btn btn-sm" to={`${ctx.base}/security/secret-scanning/push-protection-bypasses`} data-testid="bypasses-link">Push protection bypasses</Link>
      </PageHead>
      <div className="sec-filters" data-testid="secret-filters">
        <FilterSelect id="ss-type" label="Secret type" value={type} onChange={(v) => set("secret_type", v)} options={types} />
        {state !== "open" && <FilterSelect id="ss-res" label="Resolution" value={resolution} onChange={(v) => set("resolution", v)} options={SECRET_RESOLUTIONS} />}
        <FilterSelect id="ss-val" label="Validity" value={validity} onChange={(v) => set("validity", v)} options={[["active", "Active"], ["inactive", "Inactive"], ["unknown", "Unknown"]]} />
        <label className="check-row sec-filter-check">
          <input type="checkbox" checked={!!bypassed} onChange={(e) => set("bypassed", e.target.checked ? "1" : "")} data-testid="bypassed-filter" /> Bypassed only
        </label>
      </div>
      <div className="box">
        <div className="box-header">
          <StateTabs value={state} onChange={(v) => set("state", v === "open" ? "" : v)} tabs={[
            { key: "open", label: "Open", count: (all.data || []).filter((a) => a.state === "open").length },
            { key: "closed", label: "Closed", count: (all.data || []).filter((a) => a.state === "resolved").length },
          ]} />
        </div>
        <Async state={list}>
          {(p) => p.items.length === 0 ? (
            <div className="blankslate" data-testid="secret-empty">
              <h3>{state === "open" ? "No open secrets" : "No closed secrets"}</h3>
              <p className="muted">Secret scanning has not found {state === "open" ? "any open" : "any resolved"} secrets matching these filters.</p>
            </div>
          ) : (
            <ul className="list">
              {p.items.map((a) => (
                <li key={a.number} className="list-row sec-alert-row" data-testid="secret-row">
                  <div className="grow sec-row-main">
                    <Link className="issue-title" to={`${ctx.base}/security/secret-scanning/${a.number}`}>{a.secret_type_display_name}</Link>
                    {a.validity === "active" && <span className="badge badge-removed">Active</span>}
                    {a.push_protection_bypassed && <span className="badge badge-warn">Bypassed</span>}
                    <div className="muted small sec-row-meta">
                      #{a.number} <RelTime iso={a.created_at} prefix="opened " />
                      {a.first_location_detected?.path && <> · <span className="mono">{a.first_location_detected.path}:{a.first_location_detected.start_line}</span></>}
                      {a.resolution && <> · {humanize(a.resolution)}</>}
                    </div>
                  </div>
                  {a.state !== "open" && <AlertState state="closed" />}
                </li>
              ))}
            </ul>
          )}
        </Async>
      </div>
      {list.data && <Pagination page={page} next={list.data.next} prev={list.data.prev} last={list.data.last} onPage={(n) => set("page", String(n))} />}
      {history.data && (
        <p className="muted small" data-testid="scan-history">
          {history.data.backfill_scans.length} backfill and {history.data.incremental_scans.length} incremental scan{history.data.incremental_scans.length === 1 ? "" : "s"}
          {lastScan && <> · last scan completed <RelTime iso={lastScan} /></>}
        </p>
      )}
    </div>
  );
}

export function SecretAlertPage() {
  const ctx = useRepo();
  const { number = "0" } = useParams();
  const n = Number(number);
  const state = useAsync(() => sec.secretAlert(ctx.owner, ctx.name, n), [ctx.owner, ctx.name, n]);
  const locs = useAsync(() => sec.secretLocations(ctx.owner, ctx.name, n), [ctx.owner, ctx.name, n]);
  const req = useAsync(() => sec.dismissalRequest(ctx.owner, ctx.name, "secret-scanning", n).catch(() => undefined), [ctx.owner, ctx.name, n, state.data]);
  const [reveal, setReveal] = useState(false);
  useTitle(`Secret scanning alert #${n} · ${ctx.repo.full_name}`);
  if (state.error && isOff(state.error) && !(state.error instanceof ApiError && state.error.status === 404)) return <Disabled />;
  return (
    <Async state={state}>
      {(a) => (
        <div data-testid="secret-alert">
          <div className="sec-adv-head">
            <h2 className="sec-title" data-testid="alert-title">{a.secret_type_display_name}</h2>
            <div className="sec-row-meta">
              <AlertState state={a.state === "resolved" ? "closed" : "open"} />
              <span className="muted small">Secret scanning alert #{a.number} opened <RelTime iso={a.created_at} />{a.provider && <> · {a.provider}</>}</span>
            </div>
          </div>
          {a.state === "resolved" && (
            <div className="flash sec-note" data-testid="resolved-note">
              <UserLink user={a.resolved_by} /> closed this alert as <strong>{humanize(a.resolution)}</strong> <RelTime iso={a.resolved_at} />
              {a.resolution_comment && <div className="small sec-mt">“{a.resolution_comment}”</div>}
            </div>
          )}
          {a.push_protection_bypassed && (
            <div className="flash flash-warn sec-note" data-testid="bypassed-note">
              Push protection was bypassed by <UserLink user={a.push_protection_bypassed_by} /> <RelTime iso={a.push_protection_bypassed_at} />
              {a.push_protection_bypass_request_html_url && <> · <a href={a.push_protection_bypass_request_html_url.replace(/^https?:\/\/[^/]+/, "")}>bypass request</a></>}
            </div>
          )}
          {req.data && <RequestBox req={req.data} kind="secret-scanning" n={n} onChange={() => { req.reload(); state.reload(); }} />}
          <div className="box">
            <div className="box-header">
              <strong>Secret</strong>
              <span className={`badge ${a.validity === "active" ? "badge-removed" : ""}`}>{humanize(a.validity)}</span>
              <span className="grow" />
              <SecretActions a={a} onChange={(x) => { state.setData(x); req.reload(); }} pending={req.data?.status === "pending"} />
            </div>
            <div className="box-body">
              <div className="input-group">
                <input className="input mono" readOnly aria-label="Secret value" value={reveal ? a.secret || "" : "•".repeat(Math.min(40, (a.secret || "").length || 20))} data-testid="secret-value" />
                <button type="button" className="btn btn-sm" onClick={() => setReveal(!reveal)} data-testid="reveal-secret">{reveal ? "Hide" : "Reveal"}</button>
              </div>
              {a.validity === "active" && <p className="small">This secret is <strong>active</strong>. Revoke it with its provider before closing the alert as revoked.</p>}
            </div>
          </div>
          <div className="box">
            <div className="box-header"><strong>Locations</strong> <span className="counter">{(locs.data || []).length}</span></div>
            <Async state={locs}>
              {(ls) => (
                <ul className="list">
                  {ls.map((l, i) => (
                    <li key={i} className="list-row" data-testid="secret-location">
                      <div className="grow">
                        <Link className="mono" to={`${ctx.base}/blob/${l.details.commit_sha}/${l.details.path}#L${l.details.start_line}`}>
                          {l.details.path}:{l.details.start_line}
                        </Link>
                        <div className="muted small">commit <Link className="mono" to={`${ctx.base}/commit/${l.details.commit_sha}`}>{(l.details.commit_sha || "").slice(0, 7)}</Link> · columns {l.details.start_column}–{l.details.end_column}</div>
                      </div>
                    </li>
                  ))}
                </ul>
              )}
            </Async>
          </div>
        </div>
      )}
    </Async>
  );
}

function SecretActions({ a, onChange, pending }: { a: SecretAlert; onChange: (a: SecretAlert) => void; pending: boolean }) {
  const ctx = useRepo();
  const [open, setOpen] = useState(false);
  const [res, setRes] = useState("");
  const [comment, setComment] = useState("");
  const act = useAction();
  if (a.state === "resolved") {
    return (
      <span>
        <button type="button" className="btn btn-sm" disabled={act.busy} data-testid="reopen-alert"
          onClick={() => act.run(async () => onChange(await sec.updateSecret(ctx.owner, ctx.name, a.number, { state: "open" })))}>Reopen alert</button>
        <InlineError error={act.error} />
      </span>
    );
  }
  if (pending) return <span className="badge badge-warn">Closure requested</span>;
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const ok = await act.run(async () => onChange(await sec.updateSecret(ctx.owner, ctx.name, a.number, { state: "resolved", resolution: res, resolution_comment: comment || undefined })));
    if (ok) setOpen(false);
  };
  return (
    <div className="menu-wrap sec-dismiss">
      <button type="button" className="btn btn-sm" aria-expanded={open} onClick={() => setOpen(!open)} data-testid="close-alert">Close as ▾</button>
      {open && (
        <form className="menu sec-dismiss-menu" onSubmit={submit} data-testid="resolve-form">
          <div className="menu-header">Close alert as</div>
          {SECRET_RESOLUTIONS.map(([k, l]) => (
            <label key={k} className="radio-row sec-radio">
              <input type="radio" name="secret-res" value={k} checked={res === k} onChange={() => setRes(k)} /> {l}
            </label>
          ))}
          <div className="sec-pad">
            <textarea className="input textarea" rows={3} aria-label="Resolution comment" placeholder="Comment (optional)" value={comment} onChange={(e) => setComment(e.target.value)} maxLength={270} />
            <InlineError error={act.error} />
            <button type="submit" className="btn btn-sm btn-danger sec-mt" disabled={!res || act.busy} data-testid="resolve-submit">Close alert</button>
          </div>
        </form>
      )}
    </div>
  );
}

export function BypassRequestsPage() {
  const ctx = useRepo();
  useTitle(`Push protection bypasses · ${ctx.repo.full_name}`);
  const [tab, setTab] = useState("requests");
  const reqs = useAsync(() => sec.bypassRequests(ctx.owner, ctx.name), [ctx.owner, ctx.name]);
  const bypassed = useAsync(() => sec.secretAlerts(ctx.owner, ctx.name, { is_bypassed: "true", per_page: 100, hide_secret: "true", state: "" }), [ctx.owner, ctx.name]);
  if (bypassed.error && isOff(bypassed.error) && reqs.error) return <Disabled />;
  return (
    <div data-testid="bypasses-page">
      <PageHead title="Push protection bypasses" sub="Secrets that were pushed past push protection, and bypass requests waiting for review when delegated bypass is on.">
        <Link className="btn btn-sm" to={`${ctx.base}/security/secret-scanning`}>Back to alerts</Link>
      </PageHead>
      <div className="box">
        <div className="box-header">
          <StateTabs value={tab} onChange={setTab} tabs={[
            { key: "requests", label: "Bypass requests", count: reqs.data?.length },
            { key: "bypassed", label: "Bypassed secrets", count: bypassed.data?.items.length },
          ]} />
        </div>
        {tab === "requests" ? (
          <Async state={reqs}>
            {(list) => <RequestList reqs={list} link={(r) => `${ctx.base}/security/secret-scanning/push-protection-bypasses/${r.number}`} />}
          </Async>
        ) : (
          <Async state={bypassed}>
            {(p) => p.items.length === 0 ? <div className="blankslate"><h3>No bypassed secrets</h3></div> : (
              <ul className="list">
                {p.items.map((a) => (
                  <li key={a.number} className="list-row sec-alert-row" data-testid="bypassed-row">
                    <div className="grow sec-row-main">
                      <Link className="issue-title" to={`${ctx.base}/security/secret-scanning/${a.number}`}>{a.secret_type_display_name}</Link>
                      <div className="muted small sec-row-meta">
                        #{a.number} bypassed by <UserLink user={a.push_protection_bypassed_by} /> <RelTime iso={a.push_protection_bypassed_at} />
                      </div>
                    </div>
                    <AlertState state={a.state === "resolved" ? "closed" : "open"} />
                  </li>
                ))}
              </ul>
            )}
          </Async>
        )}
      </div>
    </div>
  );
}

export function BypassRequestPage() {
  const ctx = useRepo();
  const { number = "0" } = useParams();
  const n = Number(number);
  const st = useAsync(() => sec.bypassRequest(ctx.owner, ctx.name, n), [ctx.owner, ctx.name, n]);
  useTitle(`Bypass request #${n} · ${ctx.repo.full_name}`);
  return (
    <div data-testid="bypass-request-page">
      <PageHead title={`Push protection bypass request #${n}`}>
        <Link className="btn btn-sm" to={`${ctx.base}/security/secret-scanning/push-protection-bypasses`}>All bypass requests</Link>
      </PageHead>
      <Async state={st}>{(r) => <RequestBox req={r} kind="bypass" n={n} onChange={st.reload} />}</Async>
    </div>
  );
}

export function UnblockSecretPage() {
  const ctx = useRepo();
  const { placeholder = "" } = useParams();
  const [reason, setReason] = useState("");
  const [done, setDone] = useState<{ expire_at: string; token_type: string } | null>(null);
  const [requested, setRequested] = useState<string | null>(null);
  const act = useAction();
  useTitle(`Allow secret · ${ctx.repo.full_name}`);
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    await act.run(async () => {
      try {
        setDone(await sec.bypass(ctx.owner, ctx.name, { reason, placeholder_id: placeholder }));
      } catch (err) {
        if (err instanceof ApiError && err.status === 403 && /bypass request/i.test(err.message)) {
          const m = err.message.match(/push-protection-bypasses\/(\d+)/);
          setRequested(m ? m[1] : "");
        } else throw err;
      }
    });
  };
  return (
    <div data-testid="unblock-page">
      <PageHead title="Allow this secret to be pushed" sub={<>Secret <span className="mono">{placeholder.slice(0, 12)}…</span> was blocked by push protection. Tell us why it is safe to push.</>} />
      {done ? (
        <div className="flash flash-success" data-testid="unblock-done">
          You can now push this {done.token_type} secret. The bypass expires <RelTime iso={done.expire_at} />; push again before then.
        </div>
      ) : requested !== null ? (
        <div className="flash flash-warn" data-testid="unblock-requested">
          Delegated bypass is enabled for this repository: your request needs a reviewer's approval before you can push.{" "}
          {requested && <Link to={`${ctx.base}/security/secret-scanning/push-protection-bypasses/${requested}`}>View bypass request #{requested}</Link>}
        </div>
      ) : (
        <form className="box box-body" onSubmit={submit} data-testid="unblock-form">
          {BYPASS_REASONS.map(([k, l]) => (
            <label key={k} className="radio-row">
              <input type="radio" name="bypass-reason" value={k} checked={reason === k} onChange={() => setReason(k)} /> {l}
            </label>
          ))}
          <InlineError error={act.error} />
          <div className="form-actions sec-left">
            <button type="submit" className="btn btn-primary" disabled={!reason || act.busy} data-testid="unblock-submit">Allow me to push this secret</button>
          </div>
        </form>
      )}
    </div>
  );
}
