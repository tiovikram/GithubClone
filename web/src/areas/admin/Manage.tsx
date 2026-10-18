// Management console (GHES /manage/v1): settings + config apply, nodes and health, license,
// administrative SSH access.
import { Fragment, useEffect, useState, type FormEvent } from "react";
import { Async, Blankslate, ErrorBox, Flash } from "../../components/common";
import { useAsync } from "../../hooks";
import { fullDate } from "../../util";
import { adminApi, type ManageSettings } from "./api";
import { PageHead, Pill, Stat, useAction } from "./ui";

type Field =
  | { kind: "bool"; key: string; label: string; help?: string }
  | { kind: "text"; key: string; label: string; help?: string }
  | { kind: "select"; key: string; label: string; options: string[]; help?: string };

const SECTIONS: { title: string; fields: Field[] }[] = [
  {
    title: "Privacy and access",
    fields: [
      { kind: "bool", key: "signup_enabled", label: "Enable sign up", help: "Let anyone create an account on the sign-up page. Off: only site admins create users." },
      { kind: "bool", key: "private_mode", label: "Private mode", help: "Require authentication for every page (recorded; forge already hides private content)." },
      { kind: "bool", key: "repository_invitations.enabled", label: "Repository invitations", help: "Adding a collaborator sends an invitation the user must accept (github.com behavior)." },
      { kind: "bool", key: "expire_sessions", label: "Expire sessions on settings changes" },
    ],
  },
  {
    title: "Hostname and authentication",
    fields: [
      { kind: "text", key: "github_hostname", label: "Hostname" },
      { kind: "select", key: "auth_mode", label: "Authentication", options: ["default", "ldap", "saml", "cas"], help: "forge authenticates with passwords, tokens and SSH keys; LDAP/SAML/CAS are recorded." },
      { kind: "select", key: "identicons_host", label: "Identicons", options: ["dotcom", "local"] },
    ],
  },
  {
    title: "Pages",
    fields: [
      { kind: "bool", key: "pages.enabled", label: "Enable Pages" },
      { kind: "bool", key: "public_pages", label: "Public Pages", help: "Pages sites are visible without signing in." },
      { kind: "bool", key: "subdomain_isolation", label: "Subdomain isolation" },
    ],
  },
  {
    title: "Email (SMTP)",
    fields: [
      { kind: "bool", key: "smtp.enabled", label: "Enable email" },
      { kind: "text", key: "smtp.address", label: "Server address" },
      { kind: "text", key: "smtp.port", label: "Port" },
      { kind: "text", key: "smtp.domain", label: "Domain" },
      { kind: "text", key: "smtp.username", label: "Username" },
      { kind: "text", key: "smtp.noreply_address", label: "No-reply address" },
      { kind: "text", key: "smtp.support_address", label: "Support address" },
    ],
  },
];

function getPath(o: Record<string, unknown>, path: string): unknown {
  return path.split(".").reduce<unknown>((v, k) => (v && typeof v === "object" ? (v as Record<string, unknown>)[k] : undefined), o);
}

function setPath(o: Record<string, unknown>, path: string, value: unknown) {
  const keys = path.split(".");
  let cur = o;
  for (const k of keys.slice(0, -1)) {
    if (!cur[k] || typeof cur[k] !== "object") cur[k] = {};
    cur = cur[k] as Record<string, unknown>;
  }
  cur[keys[keys.length - 1]] = value;
}

export function SettingsSection() {
  const state = useAsync(() => adminApi.settings(), []);
  return (
    <div data-testid="sa-settings">
      <PageHead title="Settings" desc="Management console settings. They take effect when saved; “Save and apply” also records a configuration run." />
      <Async state={state}>{(s) => <SettingsForm s={s} reload={state.reload} />}</Async>
      <ApplyStatusBox />
    </div>
  );
}

function SettingsForm({ s, reload }: { s: ManageSettings; reload: () => void }) {
  const [draft, setDraft] = useState<Record<string, unknown>>({});
  const act = useAction();
  useEffect(() => setDraft({}), [s]);
  const value = (k: string) => (k in draft ? draft[k] : getPath(s, k));
  const save = async (apply: boolean) => {
    const body: Record<string, unknown> = {};
    for (const [k, v] of Object.entries(draft)) setPath(body, k, v);
    await act.run(async () => {
      if (Object.keys(body).length) await adminApi.putSettings(body);
      if (apply) await adminApi.apply();
    }, apply ? "Settings saved and applied." : "Settings saved.");
    reload();
  };
  const changed = Object.keys(draft).length > 0;
  return (
    <form onSubmit={(e: FormEvent) => { e.preventDefault(); void save(true); }} aria-label="Management console settings">
      {act.status}
      {SECTIONS.map((sec) => (
        <section key={sec.title} className="box">
          <div className="box-header"><h3 className="h5">{sec.title}</h3></div>
          <div className="box-body">
            {sec.fields.map((f) => {
              const id = `sa-set-${f.key.replace(/\W/g, "-")}`;
              if (f.kind === "bool") {
                return (
                  <div key={f.key} className="form-group">
                    <label className="check-row" htmlFor={id}>
                      <input id={id} type="checkbox" checked={!!value(f.key)} onChange={(e) => setDraft({ ...draft, [f.key]: e.target.checked })} />
                      <span><strong>{f.label}</strong>{f.help && <span className="muted small d-block">{f.help}</span>}</span>
                    </label>
                  </div>
                );
              }
              const v = value(f.key);
              return (
                <div key={f.key} className="form-group">
                  <label htmlFor={id}>{f.label}</label>
                  {f.kind === "select" ? (
                    <select id={id} className="input input-auto" value={String(v ?? "")} onChange={(e) => setDraft({ ...draft, [f.key]: e.target.value })}>
                      {f.options.map((o) => <option key={o} value={o}>{o}</option>)}
                    </select>
                  ) : (
                    <input id={id} className="input" value={v === null || v === undefined ? "" : String(v)} onChange={(e) => setDraft({ ...draft, [f.key]: e.target.value })} />
                  )}
                  {f.help && <p className="muted small">{f.help}</p>}
                </div>
              );
            })}
          </div>
        </section>
      ))}
      <details className="box box-body">
        <summary>All settings (JSON)</summary>
        <pre className="plain mono small sa-json">{JSON.stringify(s, null, 2)}</pre>
      </details>
      <div className="form-actions">
        <span className="muted small">{s.configuration_run_count ?? 0} configuration run{s.configuration_run_count === 1 ? "" : "s"}</span>
        <button type="button" className="btn" disabled={act.busy || !changed} onClick={() => save(false)} data-testid="sa-settings-save">Save settings</button>
        <button type="submit" className="btn btn-primary" disabled={act.busy} data-testid="sa-settings-apply">Save and apply</button>
      </div>
    </form>
  );
}

function ApplyStatusBox() {
  const status = useAsync(() => adminApi.applyStatus(), []);
  const events = useAsync(() => adminApi.applyEvents(), []);
  return (
    <section className="box" data-testid="sa-apply-status">
      <div className="box-header">
        <h3 className="h5">Last configuration run</h3>
        <button type="button" className="btn btn-sm" onClick={() => { status.reload(); events.reload(); }}>Refresh</button>
      </div>
      <div className="box-body">
        {status.data && (
          <p>{status.data.running ? <Pill kind="in_progress">running</Pill> : status.data.successful ? <Pill kind="success">successful</Pill> : <Pill kind="failed">failed</Pill>}
            {status.data.nodes[0] && <span className="muted small"> run {status.data.nodes[0].run_id} on {status.data.nodes[0].hostname}</span>}</p>
        )}
        {events.data?.nodes.flatMap((n) => n.events).map((ev, i) => (
          <div key={i} className="small"><span className="muted">{fullDate(ev.timestamp)}</span> <code>{ev.event_name}</code> {ev.body}</div>
        ))}
      </div>
    </section>
  );
}

export function NodesSection() {
  const nodes = useAsync(() => adminApi.nodes(), []);
  const cluster = useAsync(() => adminApi.clusterStatus(), []);
  const repl = useAsync(() => adminApi.replicationStatus(), []);
  const ver = useAsync(() => adminApi.version(), []);
  const req = useAsync(() => adminApi.systemRequirements(), []);
  const upg = useAsync(() => adminApi.upgradeStatus(), []);
  return (
    <div data-testid="sa-nodes">
      <PageHead title="Nodes and health" desc="forge runs as stateless processes over PostgreSQL and object storage; each process reports a heartbeat." />
      <Async state={nodes}>
        {(n) => (
          <section className="box">
            <div className="box-header"><h3 className="h5">Nodes</h3><Pill>{n.topology}</Pill></div>
            <ul className="list">
              {n.nodes.map((x) => {
                const v = ver.data?.find((y) => y.hostname === x.hostname);
                return (
                  <li key={x.uuid} className="list-row sa-row">
                    <div className="grow sa-row-main">
                      <strong>{x.hostname}</strong> <span className="muted small mono sa-break">{x.uuid}</span>
                      <div className="muted small">{x.cluster_roles.join(", ")}{v ? ` · ${v.version.version} (build ${v.version.build_id}, ${v.version.build_date})` : ""}</div>
                    </div>
                  </li>
                );
              })}
            </ul>
          </section>
        )}
      </Async>
      {[["Cluster status", cluster] as const, ["Replication", repl] as const].map(([title, st]) => (
        <section key={title} className="box">
          <div className="box-header"><h3 className="h5">{title}</h3>{st.data && <Pill>{st.data.status}</Pill>}</div>
          {st.error !== undefined ? <div className="box-body"><ErrorBox error={st.error} /></div> : (
            <ul className="list">
              {st.data?.nodes.flatMap((n) => n.services.map((sv) => (
                <li key={n.hostname + sv.name} className="list-row sa-row">
                  <div className="grow sa-row-main"><strong>{sv.name}</strong> <span className="muted small">{n.hostname}</span><div className="muted small sa-break">{sv.details}</div></div>
                  <Pill>{sv.status}</Pill>
                </li>
              )))}
            </ul>
          )}
        </section>
      ))}
      {req.data && (
        <section className="box">
          <div className="box-header"><h3 className="h5">System requirements</h3><Pill kind={req.data.status === "OK" ? "OK" : "failed"}>{req.data.status}</Pill></div>
          <div className="box-body small">
            {req.data.nodes.map((n) => <div key={n.hostname}>{n.hostname}: {n.roles_status.map((r) => `${r.role} ${r.status}`).join(", ")}</div>)}
          </div>
        </section>
      )}
      {upg.data && (
        <section className="box">
          <div className="box-header"><h3 className="h5">Upgrades</h3></div>
          <div className="box-body small">
            {upg.data.map((u) => <p key={u.hostname}><strong>{u.hostname}</strong> running {u.release_version} — {u.messages.join(" ")}</p>)}
          </div>
        </section>
      )}
    </div>
  );
}

export function LicenseSection() {
  const lic = useAsync(() => adminApi.license(), []);
  const manage = useAsync(() => adminApi.manageLicense(), []);
  const check = useAsync(() => adminApi.licenseCheck(), []);
  const [text, setText] = useState("");
  const act = useAction();
  return (
    <div data-testid="sa-license">
      <PageHead title="License" desc="forge has no license limits; it reports an unlimited, perpetual license." />
      <Async state={lic}>
        {(l) => (
          <section className="box">
            <div className="box-header"><h3 className="h5">Seats</h3>{check.data && <Pill>{check.data.status}</Pill>}</div>
            <div className="sa-stat-row">
              <Stat label="Seats" value={String(l.seats)} testid="sa-license-seats" />
              <Stat label="Used" value={l.seats_used} />
              <Stat label="Available" value={String(l.seats_available)} />
              <Stat label="Days until expiration" value={l.days_until_expiration} />
            </div>
            <p className="box-body small muted">Kind: {l.kind} · expires {l.expire_at}</p>
          </section>
        )}
      </Async>
      {manage.data && (
        <section className="box">
          <div className="box-header"><h3 className="h5">License details</h3></div>
          <dl className="sa-dl box-body">
            {Object.entries(manage.data).map(([k, v]) => <Fragment key={k}><dt>{k}</dt><dd>{String(v)}</dd></Fragment>)}
          </dl>
        </section>
      )}
      <form className="box box-body" aria-label="Upload license" onSubmit={async (e) => { e.preventDefault(); if (await act.run(() => adminApi.uploadLicense(text.trim()), "License uploaded.")) { setText(""); manage.reload(); } }}>
        <h3 className="h5">Upload a license</h3>
        {act.status}
        <label htmlFor="sa-lic" className="sr-only">License file contents</label>
        <textarea id="sa-lic" className="input textarea mono" rows={3} placeholder="Paste the license file contents (base64)" value={text} onChange={(e) => setText(e.target.value)} />
        <div className="form-actions"><button type="submit" className="btn" disabled={act.busy || !text.trim()}>Upload</button></div>
      </form>
    </div>
  );
}

export function SshAccessSection() {
  const keys = useAsync(() => adminApi.sshAccess(), []);
  const [key, setKey] = useState("");
  const act = useAction();
  return (
    <div data-testid="sa-ssh-access">
      <PageHead title="Administrative SSH access" desc="Keys authorized for administrative shell access to the nodes (recorded; forge processes have no admin shell)." />
      {act.status}
      <Async state={keys}>
        {(list) => list.length === 0 ? <Blankslate title="No administrative SSH keys" /> : (
          <ul className="box list">
            {list.map((k) => (
              <li key={k.fingerprint} className="list-row sa-row">
                <div className="grow sa-row-main mono small sa-break">{k.fingerprint}</div>
                <button type="button" className="btn btn-sm btn-danger-outline" disabled={act.busy}
                  onClick={async () => { if (await act.run(() => adminApi.removeSshAccess(k.key), "Key removed.")) keys.reload(); }}>Remove</button>
              </li>
            ))}
          </ul>
        )}
      </Async>
      <form className="box box-body" aria-label="Add SSH key" onSubmit={async (e) => { e.preventDefault(); if (await act.run(() => adminApi.addSshAccess(key.trim()), "Key added.")) { setKey(""); keys.reload(); } }}>
        <label htmlFor="sa-ssh-key"><strong>Add a key</strong></label>
        <textarea id="sa-ssh-key" className="input textarea mono" rows={2} placeholder="ssh-ed25519 AAAA…" value={key} onChange={(e) => setKey(e.target.value)} />
        <div className="form-actions"><button type="submit" className="btn" disabled={act.busy || !key.trim()}>Add key</button></div>
      </form>
      <Flash kind="info">Keys here do not grant git access; users add those in their own settings.</Flash>
    </div>
  );
}
