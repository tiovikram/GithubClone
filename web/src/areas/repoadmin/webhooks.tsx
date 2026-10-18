// Webhooks (repository and organization): list, create/edit/delete, ping, recent deliveries with
// request/response, redeliver.
//   <root>                       list          (root = /{o}/{r}/settings/hooks or /organizations/{org}/settings/hooks)
//   <root>/new                   add webhook
//   <root>/<id>                  edit (Settings tab)
//   <root>/<id>/deliveries       Recent Deliveries tab
import { useEffect, useState, type FormEvent } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { Async, Blankslate, RelTime, Spinner } from "../../components/common";
import { useAsync, useTitle } from "../../hooks";
import { ra, type Delivery, type Hook } from "./api";
import { JsonBlock, Pill, SectionHead, Status, useAction } from "./ui";

export interface HookLevel {
  /** API base: /repos/{o}/{r} or /orgs/{org}. */
  api: string;
  root: string;
  kind: "repo" | "org";
}

export const HOOK_EVENTS: [string, string][] = [
  ["branch_protection_rule", "Branch protection rules"],
  ["check_run", "Check runs"],
  ["check_suite", "Check suites"],
  ["code_scanning_alert", "Code scanning alerts"],
  ["commit_comment", "Commit comments"],
  ["create", "Branch or tag creation"],
  ["delete", "Branch or tag deletion"],
  ["dependabot_alert", "Dependabot alerts"],
  ["deploy_key", "Deploy keys"],
  ["deployment", "Deployments"],
  ["deployment_status", "Deployment statuses"],
  ["discussion", "Discussions"],
  ["discussion_comment", "Discussion comments"],
  ["fork", "Forks"],
  ["gollum", "Wiki"],
  ["issue_comment", "Issue comments"],
  ["issues", "Issues"],
  ["label", "Labels"],
  ["member", "Collaborator add, remove, or changed"],
  ["meta", "Meta"],
  ["milestone", "Milestones"],
  ["package", "Packages"],
  ["page_build", "Page builds"],
  ["projects_v2_item", "Project items"],
  ["public", "Visibility changes"],
  ["pull_request", "Pull requests"],
  ["pull_request_review", "Pull request reviews"],
  ["pull_request_review_comment", "Pull request review comments"],
  ["pull_request_review_thread", "Pull request review threads"],
  ["push", "Pushes"],
  ["registry_package", "Registry packages"],
  ["release", "Releases"],
  ["repository", "Repositories"],
  ["repository_ruleset", "Repository rulesets"],
  ["repository_vulnerability_alert", "Repository vulnerability alerts"],
  ["secret_scanning_alert", "Secret scanning alerts"],
  ["security_and_analysis", "Security and analysis"],
  ["star", "Stars"],
  ["status", "Statuses"],
  ["team_add", "Team adds"],
  ["watch", "Watches"],
  ["workflow_dispatch", "Workflow dispatches"],
  ["workflow_job", "Workflow jobs"],
  ["workflow_run", "Workflow runs"],
];
const ORG_EVENTS: [string, string][] = [["membership", "Team membership"], ["organization", "Organizations"], ["team", "Teams"], ["repository", "Repositories"]];

export function HooksSection({ level, section = "hooks" }: { level: HookLevel; section?: string }) {
  const { "*": splat = "" } = useParams();
  const p = splat.replace(/^\/+|\/+$/g, "");
  const rest = p === section ? "" : p.slice(section.length + 1);
  if (rest === "") return <HookList level={level} />;
  if (rest === "new") return <HookForm level={level} />;
  const m = rest.match(/^(\d+)(\/deliveries)?$/);
  if (m) return <HookPage level={level} id={Number(m[1])} tab={m[2] ? "deliveries" : "settings"} />;
  return <HookList level={level} />;
}

const statusOf = (h: Hook) => {
  const c = h.last_response?.code;
  if (!h.active) return <Pill>Inactive</Pill>;
  if (c == null) return <Pill>Not delivered yet</Pill>;
  return c >= 200 && c < 300 ? <Pill kind="ok">Last delivery was successful</Pill> : <Pill kind="bad">Last delivery failed ({c || h.last_response?.status || "error"})</Pill>;
};

function HookList({ level }: { level: HookLevel }) {
  useTitle("Webhooks");
  const st = useAsync(() => ra.hooks(level.api), [level.api]);
  const act = useAction();
  return (
    <div className="settings ra-hooks">
      <SectionHead title="Webhooks" actions={<Link className="btn btn-sm btn-primary" to={`${level.root}/new`}>Add webhook</Link>}>
        Webhooks allow external services to be notified when certain events happen. When the specified events happen, we'll send a POST request to each of the URLs you provide.
      </SectionHead>
      <Status state={act.state} />
      <Async state={st}>
        {(list) =>
          list.length === 0 ? <div className="box"><Blankslate title="No webhooks yet" /></div> : (
            <ul className="box ra-list" data-testid="hooks">
              {list.map((h) => (
                <li key={h.id} className="ra-row" data-testid={`hook-${h.id}`}>
                  <div className="ra-row-main">
                    <Link className="ra-strong ra-break" to={`${level.root}/${h.id}`}>{h.config.url}</Link> <span className="muted small">({h.events.join(", ")})</span>
                    <div className="small">{statusOf(h)}</div>
                  </div>
                  <div className="ra-row-actions">
                    <Link className="btn btn-sm" to={`${level.root}/${h.id}`}>Edit</Link>
                    <button type="button" className="btn btn-sm btn-danger" disabled={act.busy} aria-label={`Delete webhook ${h.config.url}`}
                      onClick={async () => {
                        if (!confirm("Delete this webhook? This action cannot be undone. Future events will no longer be delivered to this webhook.")) return;
                        await act.run(() => ra.deleteHook(level.api, h.id), "Webhook deleted.");
                        st.reload();
                      }}>Delete</button>
                  </div>
                </li>
              ))}
            </ul>
          )
        }
      </Async>
    </div>
  );
}

function HookPage({ level, id, tab }: { level: HookLevel; id: number; tab: "settings" | "deliveries" }) {
  const st = useAsync(() => ra.hook(level.api, id), [level.api, id]);
  return (
    <div className="settings ra-hooks">
      <div className="ra-crumbs small"><Link to={level.root}>Webhooks</Link> / Manage webhook</div>
      <nav className="tabnav ra-tabnav" aria-label="Webhook">
        <Link className={`tabnav-tab${tab === "settings" ? " selected" : ""}`} to={`${level.root}/${id}`}>Settings</Link>
        <Link className={`tabnav-tab${tab === "deliveries" ? " selected" : ""}`} to={`${level.root}/${id}/deliveries`}>Recent Deliveries</Link>
      </nav>
      <Async state={st}>
        {(h) => (tab === "settings" ? <HookForm level={level} hook={h} onSaved={st.setData} /> : <Deliveries level={level} hook={h} />)}
      </Async>
    </div>
  );
}

function HookForm({ level, hook, onSaved }: { level: HookLevel; hook?: Hook; onSaved?: (h: Hook) => void }) {
  useTitle(hook ? "Manage webhook" : "Add webhook");
  const nav = useNavigate();
  const [url, setUrl] = useState(hook?.config.url || "");
  const [ct, setCt] = useState(hook?.config.content_type || "json");
  const [secret, setSecret] = useState("");
  const [ssl, setSsl] = useState(String(hook?.config.insecure_ssl ?? "0") === "1" ? "1" : "0");
  const initialMode = !hook ? "push" : hook.events.includes("*") ? "all" : hook.events.length === 1 && hook.events[0] === "push" ? "push" : "pick";
  const [mode, setMode] = useState<"push" | "all" | "pick">(initialMode);
  const [events, setEvents] = useState<string[]>(hook?.events.filter((e) => e !== "*") || ["push"]);
  const [active, setActive] = useState(hook?.active ?? true);
  const act = useAction();
  const ping = useAction();
  useEffect(() => {
    if (!hook) return;
    setUrl(hook.config.url || "");
    setActive(hook.active);
  }, [hook]);
  const list = level.kind === "org" ? [...HOOK_EVENTS, ...ORG_EVENTS.filter((o) => !HOOK_EVENTS.some((h) => h[0] === o[0]))].sort((a, b) => a[1].localeCompare(b[1])) : HOOK_EVENTS;
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const evs = mode === "all" ? ["*"] : mode === "push" ? ["push"] : events;
    const config: Record<string, string> = { url, content_type: ct, insecure_ssl: ssl };
    if (secret) config.secret = secret;
    const body = { config, events: evs, active, ...(hook ? {} : { name: "web" }) };
    const out = await act.run(() => (hook ? ra.updateHook(level.api, hook.id, body) : ra.createHook(level.api, body)), hook ? "Okay, the hook was successfully updated." : "Okay, that hook was successfully created.");
    if (out && !hook) nav(`${level.root}/${out.id}`, { replace: true });
    if (out && hook) { setSecret(""); onSaved?.(out); }
  };
  return (
    <form className="ra-hook-form" onSubmit={submit} data-testid="hook-form">
      {!hook && <SectionHead title="Webhooks / Add webhook">We'll send a POST request to the URL below with details of any subscribed events.</SectionHead>}
      <Status state={act.state} />
      <div className="box box-body">
        <div className="form-group">
          <label htmlFor="hook-url">Payload URL</label>
          <input id="hook-url" className="input" type="url" required value={url} placeholder="https://example.com/postreceive" onChange={(e) => setUrl(e.target.value)} />
        </div>
        <div className="form-group">
          <label htmlFor="hook-ct">Content type</label>
          <select id="hook-ct" className="input input-auto" value={ct} onChange={(e) => setCt(e.target.value)}>
            <option value="json">application/json</option>
            <option value="form">application/x-www-form-urlencoded</option>
          </select>
        </div>
        <div className="form-group">
          <label htmlFor="hook-secret">Secret</label>
          <input id="hook-secret" className="input" type="password" autoComplete="new-password" value={secret}
            placeholder={hook?.config.secret ? "******** (unchanged)" : ""} onChange={(e) => setSecret(e.target.value)} />
        </div>
        <fieldset className="form-group">
          <legend>SSL verification</legend>
          <label className="radio-row"><input type="radio" name="ssl" checked={ssl === "0"} onChange={() => setSsl("0")} /> Enable SSL verification</label>
          <label className="radio-row"><input type="radio" name="ssl" checked={ssl === "1"} onChange={() => setSsl("1")} /> Disable (not recommended)</label>
        </fieldset>
        <fieldset className="form-group">
          <legend>Which events would you like to trigger this webhook?</legend>
          <label className="radio-row"><input type="radio" name="evmode" checked={mode === "push"} onChange={() => setMode("push")} /> Just the <code>push</code> event.</label>
          <label className="radio-row"><input type="radio" name="evmode" checked={mode === "all"} onChange={() => setMode("all")} /> Send me <strong>everything</strong>.</label>
          <label className="radio-row"><input type="radio" name="evmode" checked={mode === "pick"} onChange={() => setMode("pick")} /> Let me select individual events.</label>
          {mode === "pick" && (
            <div className="ra-events" data-testid="hook-events">
              {list.map(([k, l]) => (
                <label key={k} className="check-row">
                  <input type="checkbox" value={k} checked={events.includes(k)} onChange={(e) => setEvents(e.target.checked ? [...events, k] : events.filter((x) => x !== k))} />
                  <span>{l} <code className="small">{k}</code></span>
                </label>
              ))}
            </div>
          )}
        </fieldset>
        <label className="check-row"><input id="hook-active" type="checkbox" checked={active} onChange={(e) => setActive(e.target.checked)} />
          <span><strong>Active</strong><span className="ra-check-desc small muted">We will deliver event details when this hook is triggered.</span></span></label>
      </div>
      <div className="form-actions">
        <button type="submit" className="btn btn-primary" disabled={act.busy || (mode === "pick" && events.length === 0)}>{hook ? "Update webhook" : "Add webhook"}</button>
        {hook && (
          <button type="button" className="btn" disabled={ping.busy} onClick={() => ping.run(() => ra.pingHook(level.api, hook.id), "Ping sent. Check Recent Deliveries.")}>Ping</button>
        )}
        {hook && level.kind === "repo" && (
          <button type="button" className="btn" disabled={ping.busy} onClick={() => ping.run(() => ra.testHook(level.api, hook.id), "Test push event sent.")}>Test push</button>
        )}
        {hook && (
          <button type="button" className="btn btn-danger" disabled={act.busy} onClick={async () => {
            if (!confirm("Delete this webhook?")) return;
            const ok = await act.run(() => ra.deleteHook(level.api, hook.id).then(() => true));
            if (ok) nav(level.root);
          }}>Delete webhook</button>
        )}
      </div>
      <Status state={ping.state} />
    </form>
  );
}

function Deliveries({ level, hook }: { level: HookLevel; hook: Hook }) {
  useTitle("Recent Deliveries");
  const st = useAsync(() => ra.deliveries(level.api, hook.id), [level.api, hook.id]);
  const [open, setOpen] = useState<number | null>(null);
  return (
    <div className="ra-deliveries">
      <div className="toolbar">
        <span className="muted small ra-break">{hook.config.url}</span>
        <span className="spacer" />
        <button type="button" className="btn btn-sm" onClick={st.reload}>Refresh</button>
      </div>
      <Async state={st}>
        {(list) =>
          list.length === 0 ? <div className="box"><Blankslate title="No deliveries yet">Deliveries appear here once an event triggers this webhook. Try “Ping”.</Blankslate></div> : (
            <ul className="box ra-list" data-testid="deliveries">
              {list.map((d) => (
                <li key={d.id} className="ra-delivery">
                  <button type="button" className="ra-delivery-head" aria-expanded={open === d.id} onClick={() => setOpen(open === d.id ? null : d.id)} data-testid="delivery">
                    <span className={d.status_code >= 200 && d.status_code < 300 ? "c-open" : "c-danger"} aria-hidden="true">{d.status_code >= 200 && d.status_code < 300 ? "✔" : "✖"}</span>
                    <span className="mono small ra-guid">{d.guid}</span>
                    <span className="ra-event">{d.event}{d.action ? `.${d.action}` : ""}</span>
                    {d.redelivery && <Pill>redelivery</Pill>}
                    <span className="spacer" />
                    <span className="small muted"><RelTime iso={d.delivered_at} /></span>
                  </button>
                  {open === d.id && <DeliveryDetail level={level} hookId={hook.id} id={d.id} onRedelivered={st.reload} />}
                </li>
              ))}
            </ul>
          )
        }
      </Async>
    </div>
  );
}

function DeliveryDetail({ level, hookId, id, onRedelivered }: { level: HookLevel; hookId: number; id: number; onRedelivered: () => void }) {
  const st = useAsync(() => ra.delivery(level.api, hookId, id), [level.api, hookId, id]);
  const [tab, setTab] = useState<"request" | "response">("request");
  const act = useAction();
  if (st.loading) return <Spinner />;
  return (
    <Async state={st}>
      {(d: Delivery) => (
        <div className="ra-delivery-body" data-testid="delivery-detail">
          <div className="toolbar">
            <div className="segmented" role="tablist">
              <button type="button" role="tab" aria-pressed={tab === "request"} aria-selected={tab === "request"} onClick={() => setTab("request")}>Request</button>
              <button type="button" role="tab" aria-pressed={tab === "response"} aria-selected={tab === "response"} onClick={() => setTab("response")}>
                Response <span className="counter">{d.status_code || "—"}</span>
              </button>
            </div>
            <span className="spacer" />
            <span className="small muted">Completed in {d.duration.toFixed(2)} seconds.</span>
            <button type="button" className="btn btn-sm" disabled={act.busy} onClick={async () => {
              await act.run(() => ra.redeliver(level.api, hookId, id), "Redelivery requested.");
              setTimeout(onRedelivered, 800);
            }}>Redeliver</button>
          </div>
          <Status state={act.state} />
          {tab === "request" ? (
            <>
              <h4 className="h6">Headers</h4>
              <Headers h={d.request?.headers} />
              <h4 className="h6">Payload</h4>
              <JsonBlock value={d.request?.payload ?? null} />
            </>
          ) : (
            <>
              <p className="small">{d.status}</p>
              <h4 className="h6">Headers</h4>
              <Headers h={d.response?.headers} />
              <h4 className="h6">Body</h4>
              <JsonBlock value={d.response?.payload ?? ""} />
            </>
          )}
        </div>
      )}
    </Async>
  );
}

function Headers({ h }: { h: Record<string, string> | null | undefined }) {
  if (!h || !Object.keys(h).length) return <p className="muted small">No headers.</p>;
  return <pre className="ra-pre mono">{Object.entries(h).map(([k, v]) => `${k}: ${v}`).join("\n")}</pre>;
}
