// Global webhooks (`/admin/hooks`): receive `user` and `organization` events for the whole instance.
import { useState, type FormEvent } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import { Async, Blankslate, RelTime } from "../../components/common";
import { useAsync } from "../../hooks";
import { adminApi, type GlobalHook, type HookInput } from "./api";
import { PageHead, Pill, useAction } from "./ui";

const EVENTS: [string, string][] = [
  ["user", "User — created, deleted, renamed, suspended, unsuspended, promoted, demoted"],
  ["organization", "Organization — created, deleted, renamed, member events"],
];

export function GlobalHooksSection() {
  const splat = useParams()["*"] || "";
  const sub = splat.split("/")[1];
  if (sub === "new") return <HookEditor />;
  if (sub) return <HookLoader id={Number(sub)} />;
  return <HookList />;
}

function HookList() {
  const hooks = useAsync(() => adminApi.hooks(), []);
  const act = useAction();
  return (
    <div data-testid="sa-hooks">
      <PageHead title="Global webhooks" desc="Webhooks that receive instance-wide user and organization events.">
        <Link className="btn btn-primary btn-sm" to="/site-admin/hooks/new" data-testid="sa-hook-new">Add webhook</Link>
      </PageHead>
      {act.status}
      <Async state={hooks}>
        {(list) => list.length === 0 ? <Blankslate title="No global webhooks">Add one to be notified about user and organization changes.</Blankslate> : (
          <ul className="box list" aria-label="Global webhooks">
            {list.map((h) => (
              <li key={h.id} className="list-row sa-row" data-testid={`sa-hook-${h.id}`}>
                <div className="grow sa-row-main">
                  <div className="sa-row-title">
                    <Link to={`/site-admin/hooks/${h.id}`} className="sa-break"><strong>{h.config.url}</strong></Link>
                    <Pill kind={h.active ? "enabled" : "disabled"}>{h.active ? "active" : "inactive"}</Pill>
                  </div>
                  <div className="muted small">{h.events.join(", ")} · {h.config.content_type || "form"} · updated <RelTime iso={h.updated_at} /></div>
                </div>
                <div className="sa-row-actions">
                  <button type="button" className="btn btn-sm" disabled={act.busy}
                    onClick={() => act.run(() => adminApi.pingHook(h.id), `Ping sent to ${h.config.url}.`)}>Ping</button>
                  <Link className="btn btn-sm" to={`/site-admin/hooks/${h.id}`}>Edit</Link>
                  <button type="button" className="btn btn-sm btn-danger-outline" disabled={act.busy}
                    onClick={async () => { if (window.confirm("Delete this webhook?") && await act.run(() => adminApi.deleteHook(h.id), "Webhook deleted.")) hooks.reload(); }}>Delete</button>
                </div>
              </li>
            ))}
          </ul>
        )}
      </Async>
    </div>
  );
}

function HookLoader({ id }: { id: number }) {
  const hook = useAsync(() => adminApi.hook(id), [id]);
  return <Async state={hook}>{(h) => <HookEditor hook={h} />}</Async>;
}

function HookEditor({ hook }: { hook?: GlobalHook }) {
  const nav = useNavigate();
  const [url, setUrl] = useState(hook?.config.url || "");
  const [ct, setCt] = useState(hook?.config.content_type || "json");
  const [secret, setSecret] = useState(hook?.config.secret ? "********" : "");
  const [insecure, setInsecure] = useState(hook?.config.insecure_ssl === "1");
  const [events, setEvents] = useState<string[]>(hook?.events || ["user", "organization"]);
  const [active, setActive] = useState(hook?.active ?? true);
  const act = useAction();
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const body: HookInput = { config: { url: url.trim(), content_type: ct, insecure_ssl: insecure ? "1" : "0" }, events, active };
    if (secret && secret !== "********") body.config.secret = secret;
    else if (secret === "********") body.config.secret = "********";
    else body.config.secret = "";
    if (await act.run(() => (hook ? adminApi.updateHook(hook.id, body) : adminApi.createHook(body)))) nav("/site-admin/hooks");
  };
  return (
    <div data-testid="sa-hook-editor">
      <div className="sa-crumbs small"><Link to="/site-admin/hooks">Global webhooks</Link> / {hook ? "Manage webhook" : "Add webhook"}</div>
      <PageHead title={hook ? "Manage webhook" : "Add webhook"} desc="We'll send a POST request to the URL below with details of subscribed events." />
      <form className="box box-body" onSubmit={submit} aria-label="Webhook">
        {act.status}
        <div className="form-group">
          <label htmlFor="sa-hook-url">Payload URL</label>
          <input id="sa-hook-url" className="input" type="url" required placeholder="https://example.com/postreceive" value={url} onChange={(e) => setUrl(e.target.value)} />
        </div>
        <div className="form-group">
          <label htmlFor="sa-hook-ct">Content type</label>
          <select id="sa-hook-ct" className="input input-auto" value={ct} onChange={(e) => setCt(e.target.value)}>
            <option value="json">application/json</option>
            <option value="form">application/x-www-form-urlencoded</option>
          </select>
        </div>
        <div className="form-group">
          <label htmlFor="sa-hook-secret">Secret</label>
          <input id="sa-hook-secret" className="input" type="password" autoComplete="off" value={secret} onChange={(e) => setSecret(e.target.value)} />
        </div>
        <label className="check-row"><input type="checkbox" checked={insecure} onChange={(e) => setInsecure(e.target.checked)} /> Disable SSL verification (not recommended)</label>
        <fieldset className="form-group">
          <legend>Events</legend>
          {EVENTS.map(([k, label]) => (
            <label key={k} className="check-row">
              <input type="checkbox" name="sa-hook-event" value={k} checked={events.includes(k)}
                onChange={(e) => setEvents(e.target.checked ? [...events, k] : events.filter((x) => x !== k))} /> {label}
            </label>
          ))}
        </fieldset>
        <label className="check-row"><input id="sa-hook-active" type="checkbox" checked={active} onChange={(e) => setActive(e.target.checked)} /> Active — deliver event details when this hook is triggered</label>
        <div className="form-actions">
          {hook && <button type="button" className="btn" disabled={act.busy} onClick={() => act.run(() => adminApi.pingHook(hook.id), "Ping sent.")}>Ping</button>}
          <button type="submit" className="btn btn-primary" disabled={act.busy || events.length === 0} data-testid="sa-hook-save">{hook ? "Update webhook" : "Add webhook"}</button>
        </div>
      </form>
    </div>
  );
}
