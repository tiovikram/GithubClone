// Delegated alert dismissal requests and push protection bypass requests: a status box with the
// reviewer's approve / deny form.
import { useState, type FormEvent } from "react";
import { Link } from "react-router-dom";
import { useAuth } from "../../auth";
import { RelTime } from "../../components/common";
import { useRepo } from "../../components/RepoLayout";
import { humanize, sec, type SecurityRequest } from "./api";
import { AlertState, InlineError, useAction } from "./ui";

/** kind: "dependabot" | "code-scanning" | "secret-scanning" (dismissal) or "bypass". */
export function RequestBox({ req, kind, n, onChange }: { req: SecurityRequest; kind: string; n: number; onChange: () => void }) {
  const ctx = useRepo();
  const { user } = useAuth();
  const [message, setMessage] = useState("");
  const act = useAction();
  const mine = !!user && req.requester?.actor_name === user.login;
  const pending = req.status === "pending";
  const what = kind === "bypass" ? "bypass push protection" : "dismiss this alert";
  const review = async (e: FormEvent, approve: boolean) => {
    e.preventDefault();
    const ok = await act.run(() =>
      kind === "bypass"
        ? sec.reviewBypass(ctx.owner, ctx.name, n, approve ? "approve" : "reject", message)
        : sec.reviewDismissal(ctx.owner, ctx.name, kind, n, approve ? "approve" : "deny", message));
    if (ok) onChange();
  };
  const data = (req.data || [])[0] as Record<string, unknown> | undefined;
  return (
    <div className={`box sec-request ${pending ? "sec-request-pending" : ""}`} data-testid="request-box">
      <div className="box-header">
        <strong>{kind === "bypass" ? `Bypass request #${req.number}` : "Dismissal request"}</strong>
        <AlertState state={req.status} />
        <span className="muted small">
          {req.requester?.actor_name || "ghost"} requested to {what} <RelTime iso={req.created_at} />
          {pending && <> · expires <RelTime iso={req.expires_at} /></>}
        </span>
      </div>
      <div className="box-body">
        {data && (
          <div className="small sec-kv">
            {Object.entries(data).filter(([, v]) => v !== null && v !== "").map(([k, v]) => (
              <div key={k}><span className="muted">{humanize(k)}</span> <code>{String(v)}</code></div>
            ))}
          </div>
        )}
        {req.requester_comment && <p className="small">“{req.requester_comment}”</p>}
        {(req.responses || []).map((r) => (
          <div key={r.id} className="small sec-response" data-testid="request-response">
            <strong>{r.reviewer?.actor_name}</strong> {r.status} this request <RelTime iso={r.created_at} />{r.message && <>: “{r.message}”</>}
          </div>
        ))}
        {pending && !mine && (
          <form className="sec-review-form" onSubmit={(e) => review(e, true)} data-testid="review-form">
            <label htmlFor={`review-msg-${req.id}`} className="label-text">Review comment</label>
            <textarea id={`review-msg-${req.id}`} className="input textarea" rows={2} required value={message} onChange={(e) => setMessage(e.target.value)} />
            <InlineError error={act.error} />
            <div className="form-actions sec-left">
              <button type="submit" className="btn btn-sm btn-primary" disabled={act.busy || !message} data-testid="approve-request">Approve</button>
              <button type="button" className="btn btn-sm btn-danger-outline" disabled={act.busy || !message} onClick={(e) => review(e, false)} data-testid="deny-request">
                {kind === "bypass" ? "Reject" : "Deny"}
              </button>
            </div>
          </form>
        )}
        {pending && mine && kind === "dependabot" && (
          <div className="form-actions sec-left">
            <button type="button" className="btn btn-sm" disabled={act.busy} data-testid="cancel-request"
              onClick={async () => (await act.run(() => sec.cancelDependabotRequest(ctx.owner, ctx.name, n))) && onChange()}>Cancel request</button>
            <InlineError error={act.error} />
          </div>
        )}
        {pending && mine && kind !== "dependabot" && <p className="muted small">Waiting for a reviewer.</p>}
      </div>
    </div>
  );
}

/** A compact list of requests (used on the bypass requests page and the org overview). */
export function RequestList({ reqs, link }: { reqs: SecurityRequest[]; link: (r: SecurityRequest) => string }) {
  if (!reqs.length) return <div className="blankslate"><h3>No requests</h3></div>;
  return (
    <ul className="list">
      {reqs.map((r) => (
        <li key={r.id} className="list-row sec-alert-row" data-testid="request-row">
          <div className="grow sec-row-main">
            <Link className="issue-title" to={link(r)}>
              {r.request_type === "push_protection_bypass" ? `Bypass request #${r.number}` : `Dismissal request for alert #${r.resource_identifier}`}
            </Link>
            <div className="muted small sec-row-meta">
              {r.repository.full_name} · requested by {r.requester?.actor_name || "ghost"} <RelTime iso={r.created_at} />
            </div>
          </div>
          <AlertState state={r.status} />
        </li>
      ))}
    </ul>
  );
}
