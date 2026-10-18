// OAuth apps (developer settings: register, edit, client secret, delete), Settings →
// Applications (installed GitHub Apps, authorized GitHub Apps, authorized OAuth apps) and the
// device-flow verification page (/login/device).
import { useSettingsSplat } from "./splat";
import { useState, type FormEvent } from "react";
import { Link, Navigate, useLocation, useNavigate, useSearchParams } from "react-router-dom";
import { errorMessage, isNotFound } from "../../api";
import { useAuth } from "../../auth";
import { Async, Avatar, Blankslate, CodeLine, ErrorBox, Flash, NotFoundView, RelTime, Spinner } from "../../components/common";
import { useAsync, useTitle } from "../../hooks";
import { appsApi, type ManagedApp } from "./api";
import { InstallationPage, InstallationsList, type AppOwner } from "./apps";
import { AppsIcon, ShieldIcon } from "./icons";

// ---------------------------------------------------------------------------------------------
// Developer settings → OAuth Apps

/** Section element: `applications`, `applications/new`, `applications/{slug}`. */
export function OAuthAppsSection({ owner }: { owner: AppOwner }) {
  const splat = useSettingsSplat();
  const parts = splat.split("/").filter(Boolean);
  if (parts.length === 1) return <OAuthAppsList owner={owner} />;
  if (parts[1] === "new") return <NewOAuthApp owner={owner} />;
  return <OAuthAppPage owner={owner} slug={parts[1]} />;
}

function OAuthAppsList({ owner }: { owner: AppOwner }) {
  useTitle("OAuth Apps");
  const st = useAsync(() => appsApi.myApps(), []);
  return (
    <div data-testid="oauth-apps-list">
      <div className="toolbar">
        <h1 className="h2">OAuth Apps</h1>
        <span className="spacer" />
        <Link to={`${owner.root}/applications/new`} className="btn btn-primary btn-sm" data-testid="new-oauth-app">New OAuth App</Link>
      </div>
      <Async state={st}>
        {(all) => {
          const list = all.filter((a) => a.kind === "oauth" && a.owner?.login.toLowerCase() === owner.login.toLowerCase());
          return list.length === 0 ? <Blankslate title="No OAuth applications" icon={<AppsIcon size={24} />}>OAuth Apps sign users in and act on their behalf with the device flow.</Blankslate> : (
            <section className="box">
              <ul className="list">
                {list.map((a) => (
                  <li key={a.id} className="list-row" data-testid="oauth-app-row">
                    <AppsIcon className="icon muted" />
                    <div className="grow"><Link to={`${owner.root}/applications/${a.slug}`}><strong>{a.name}</strong></Link><div className="small muted mono">{a.client_id}</div></div>
                  </li>
                ))}
              </ul>
            </section>
          );
        }}
      </Async>
    </div>
  );
}

function NewOAuthApp({ owner }: { owner: AppOwner }) {
  useTitle("Register a new OAuth app");
  const nav = useNavigate();
  const [f, setF] = useState({ name: "", url: "", description: "", callback: "" });
  const [err, setErr] = useState<unknown>();
  const [busy, setBusy] = useState(false);
  const set = (k: keyof typeof f) => (e: { target: { value: string } }) => setF({ ...f, [k]: e.target.value });
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setErr(undefined);
    try {
      const app = await appsApi.createOauthApp({ name: f.name.trim(), url: f.url.trim(), callback_url: f.callback.trim() || undefined, owner: owner.isOrg ? owner.login : undefined });
      if (f.description.trim()) await appsApi.update(app.slug, { description: f.description.trim() });
      nav(`${owner.root}/applications/${app.slug}`, { state: { secret: app.client_secret } });
    } catch (e2) {
      setErr(e2);
    } finally {
      setBusy(false);
    }
  };
  return (
    <form onSubmit={submit} className="app-form" data-testid="new-oauth-form">
      <h1 className="h2">Register a new OAuth app</h1>
      {err !== undefined && <ErrorBox error={err} />}
      <div className="form-group"><label htmlFor="oa-name">Application name</label><input id="oa-name" className="input" required value={f.name} onChange={set("name")} />
        <p className="small muted">Something users will recognize and trust.</p></div>
      <div className="form-group"><label htmlFor="oa-url">Homepage URL</label><input id="oa-url" className="input" type="url" required value={f.url} onChange={set("url")} placeholder="https://example.com" /></div>
      <div className="form-group"><label htmlFor="oa-desc">Application description <span className="muted">(optional)</span></label><textarea id="oa-desc" className="textarea" rows={3} value={f.description} onChange={set("description")} /></div>
      <div className="form-group"><label htmlFor="oa-cb">Authorization callback URL</label><input id="oa-cb" className="input" type="url" value={f.callback} onChange={set("callback")} /></div>
      <button type="submit" className="btn btn-primary" disabled={busy || !f.name.trim() || !f.url.trim()} data-testid="register-oauth-app">{busy ? "Registering…" : "Register application"}</button>
    </form>
  );
}

function OAuthAppPage({ owner, slug }: { owner: AppOwner; slug: string }) {
  const st = useAsync(() => appsApi.manage(slug), [slug]);
  useTitle(st.data?.name || "OAuth app");
  return <Async state={st}>{(app) => app.kind !== "oauth" ? <NotFoundView /> : <OAuthAppEdit app={app} owner={owner} setApp={st.setData} />}</Async>;
}

function OAuthAppEdit({ app, owner, setApp }: { app: ManagedApp; owner: AppOwner; setApp: (a: ManagedApp) => void }) {
  const nav = useNavigate();
  const loc = useLocation();
  const initialSecret = (loc.state as { secret?: string } | null)?.secret || "";
  const [secret, setSecret] = useState(initialSecret);
  const [f, setF] = useState({ name: app.name, url: app.external_url, description: app.description || "", callback: app.callback_urls[0] || "" });
  const [err, setErr] = useState<unknown>();
  const [msg, setMsg] = useState("");
  const set = (k: keyof typeof f) => (e: { target: { value: string } }) => setF({ ...f, [k]: e.target.value });
  const run = async (fn: () => Promise<void>) => {
    setErr(undefined);
    setMsg("");
    try { await fn(); } catch (e) { setErr(e); }
  };
  return (
    <div data-testid="oauth-app-page">
      <div className="toolbar"><h1 className="h2 grow">{app.name}</h1><Link to={`${owner.root}/applications`} className="btn btn-sm">All OAuth Apps</Link></div>
      {msg && <Flash kind="success">{msg}</Flash>}
      {err !== undefined && <ErrorBox error={err} />}
      <p className="muted small">{app.users_count} user{app.users_count === 1 ? "" : "s"} · Owned by <Link to={`/${app.owner.login}`}>{app.owner.login}</Link></p>
      <section className="box">
        <div className="box-header"><h2 className="h5">Client credentials</h2></div>
        <div className="box-body">
          <div className="form-group"><label>Client ID</label><CodeLine text={app.client_id} label="Client ID" /></div>
          {secret && <Flash kind="success"><p>Make sure to copy your new client secret now. You won't be able to see it again.</p><CodeLine text={secret} label="Client secret" /></Flash>}
          <button type="button" className="btn btn-sm" data-testid="new-client-secret" onClick={() => run(async () => {
            if (!confirm("Generate a new client secret? The current secret stops working.")) return;
            setSecret((await appsApi.newSecret(app.slug)).client_secret);
          })}>Generate a new client secret</button>
        </div>
      </section>
      <form className="box" data-testid="oauth-edit-form" onSubmit={(e) => {
        e.preventDefault();
        run(async () => {
          setApp(await appsApi.update(app.slug, { name: f.name.trim(), url: f.url.trim(), description: f.description, callback_urls: f.callback.trim() ? [f.callback.trim()] : [] }));
          setMsg("Application updated successfully.");
        });
      }}>
        <div className="box-header"><h2 className="h5">Application settings</h2></div>
        <div className="box-body">
          <div className="form-group"><label htmlFor="oe-name">Application name</label><input id="oe-name" className="input" required value={f.name} onChange={set("name")} /></div>
          <div className="form-group"><label htmlFor="oe-url">Homepage URL</label><input id="oe-url" className="input" type="url" required value={f.url} onChange={set("url")} /></div>
          <div className="form-group"><label htmlFor="oe-desc">Application description</label><textarea id="oe-desc" className="textarea" rows={3} value={f.description} onChange={set("description")} /></div>
          <div className="form-group"><label htmlFor="oe-cb">Authorization callback URL</label><input id="oe-cb" className="input" type="url" value={f.callback} onChange={set("callback")} /></div>
          <button type="submit" className="btn btn-primary" data-testid="update-oauth-app">Update application</button>
        </div>
      </form>
      <h2 className="h3 c-danger">Danger zone</h2>
      <section className="box danger-zone">
        <div className="box-body pkg-danger-row">
          <div className="grow"><h3 className="h5">Delete application</h3><p className="small muted">This revokes every token issued to it and cannot be undone.</p></div>
          <button type="button" className="btn btn-danger" data-testid="delete-oauth-app" onClick={() => run(async () => {
            if (!confirm(`Delete ${app.name}? This cannot be undone.`)) return;
            await appsApi.remove(app.slug);
            nav(`${owner.root}/applications`);
          })}>Delete application</button>
        </div>
      </section>
    </div>
  );
}

// ---------------------------------------------------------------------------------------------
// Settings → Applications

/** Section element for `installations`, `installations/{id}`, `installations/authorized`, `installations/oauth`. */
export function ApplicationsSection() {
  const splat = useSettingsSplat();
  const parts = splat.split("/").filter(Boolean);
  const sub = parts[1] || "";
  if (/^\d+$/.test(sub)) return <InstallationPage id={Number(sub)} back="/settings/installations" />;
  const tab = (key: string, to: string, label: string) => (
    <Link to={to} className={"tabnav-tab" + (sub === key ? " selected" : "")} data-testid={`apps-tab-${key || "installed"}`}>{label}</Link>
  );
  return (
    <div data-testid="applications-settings">
      <h1 className="h2">Applications</h1>
      <nav className="tabnav" aria-label="Applications">
        {tab("", "/settings/installations", "Installed GitHub Apps")}
        {tab("authorized", "/settings/installations/authorized", "Authorized GitHub Apps")}
        {tab("oauth", "/settings/installations/oauth", "Authorized OAuth Apps")}
      </nav>
      {sub === "" && (
        <>
          <p className="muted small">GitHub Apps installed on your account. Apps installed on organizations are managed in each organization's settings.</p>
          <InstallationsList base="/settings/installations" empty={<>Install apps from their public page (<code>/apps/NAME</code>).</>}
            load={async () => (await appsApi.myInstallations()).installations.filter((i) => i.target_type === "User")} />
        </>
      )}
      {sub === "authorized" && <Authorizations kind="app" />}
      {sub === "oauth" && <Authorizations kind="oauth" />}
      {sub && !["authorized", "oauth"].includes(sub) && <NotFoundView />}
    </div>
  );
}

function Authorizations({ kind }: { kind: "app" | "oauth" }) {
  const st = useAsync(() => appsApi.authorizations(kind), [kind]);
  const [err, setErr] = useState<unknown>();
  const [msg, setMsg] = useState("");
  const what = kind === "app" ? "GitHub Apps" : "OAuth Apps";
  return (
    <div data-testid={`authorized-${kind}`}>
      <p className="muted small">You have granted these {what} access to your account. Revoking access invalidates their tokens.</p>
      {msg && <Flash kind="success">{msg}</Flash>}
      {err !== undefined && <ErrorBox error={err} />}
      <Async state={st}>
        {(list) => list.length === 0 ? <Blankslate title={`No authorized ${what}`} icon={<ShieldIcon size={24} />} /> : (
          <section className="box">
            <ul className="list">
              {list.map((g) => (
                <li key={g.id} className="list-row" data-testid="authorization-row">
                  <Avatar user={g.app.owner} size={24} />
                  <div className="grow">
                    <strong>{g.app.name}</strong> <span className="small muted">by {g.app.owner.login}</span>
                    <div className="small muted">
                      {g.scopes.length > 0 && <>Scopes: {g.scopes.join(", ")} · </>}
                      {g.last_used_at ? <>Last used <RelTime iso={g.last_used_at} /></> : "Never used"} · Authorized <RelTime iso={g.created_at} />
                    </div>
                  </div>
                  <button type="button" className="btn btn-sm btn-danger-outline" data-testid="revoke-authorization" onClick={async () => {
                    if (!confirm(`Revoke ${g.app.name}'s access? Its tokens stop working.`)) return;
                    setErr(undefined);
                    try {
                      await appsApi.revokeAuthorization(g.id);
                      setMsg(`Access for ${g.app.name} revoked.`);
                      st.reload();
                    } catch (e) {
                      setErr(e);
                    }
                  }}>Revoke</button>
                </li>
              ))}
            </ul>
          </section>
        )}
      </Async>
    </div>
  );
}

/** Org settings → GitHub Apps (installations on the organization). */
export function OrgInstallationsSection({ org }: { org: string }) {
  const splat = useSettingsSplat();
  const parts = splat.split("/").filter(Boolean);
  const base = `/organizations/${org}/settings/installations`;
  if (parts[1] && /^\d+$/.test(parts[1])) return <InstallationPage id={Number(parts[1])} back={base} />;
  return (
    <div data-testid="org-installations">
      <h1 className="h2">Installed GitHub Apps</h1>
      <p className="muted small">GitHub Apps augment and extend your workflows on forge. These apps are installed on {org}.</p>
      <InstallationsList base={base} empty={<>Install apps from their public page (<code>/apps/NAME</code>).</>}
        load={async () => (await appsApi.orgInstallations(org)).installations} />
    </div>
  );
}

// ---------------------------------------------------------------------------------------------
// Device flow: /login/device

export function DevicePage() {
  const { user, loading } = useAuth();
  const [sp] = useSearchParams();
  const [code, setCode] = useState(sp.get("user_code") || "");
  const [req, setReq] = useState<Awaited<ReturnType<typeof appsApi.device>> | null>(null);
  const [err, setErr] = useState("");
  const [done, setDone] = useState<"" | "approved" | "denied">("");
  useTitle("Device activation");
  if (loading) return <Spinner />;
  if (!user) return <Navigate to={`/login?return_to=${encodeURIComponent("/login/device")}`} replace />;
  const look = async (e: FormEvent) => {
    e.preventDefault();
    setErr("");
    try {
      setReq(await appsApi.device(code.trim()));
    } catch (e2) {
      setErr(isNotFound(e2) ? "That code is invalid or has expired. Check the code shown on your device and try again." : errorMessage(e2));
    }
  };
  const decide = async (approve: boolean) => {
    if (!req) return;
    setErr("");
    try {
      if (approve) await appsApi.approveDevice(req.user_code);
      else await appsApi.denyDevice(req.user_code);
      setDone(approve ? "approved" : "denied");
    } catch (e2) {
      setErr(errorMessage(e2));
    }
  };
  return (
    <div className="container narrow device-page" data-testid="device-page">
      {done === "approved" ? (
        <section className="box box-body device-done" data-testid="device-approved">
          <h1 className="h2">Congratulations, you're all set!</h1>
          <p className="muted">Your device is now connected. You can close this window and go back to it.</p>
        </section>
      ) : done === "denied" ? (
        <section className="box box-body" data-testid="device-denied">
          <h1 className="h2">Access denied</h1>
          <p className="muted">The device was not authorized.</p>
        </section>
      ) : !req ? (
        <form className="box box-body" onSubmit={look}>
          <h1 className="h2">Device activation</h1>
          <p className="muted">Enter the code displayed on your device.</p>
          {err && <Flash kind="error">{err}</Flash>}
          <div className="form-group">
            <label htmlFor="device-code">Code</label>
            <input id="device-code" className="input input-lg mono" placeholder="XXXX-XXXX" autoComplete="off" value={code} onChange={(e) => setCode(e.target.value.toUpperCase())} />
          </div>
          <button type="submit" className="btn btn-primary" disabled={code.replace(/[^A-Z0-9]/gi, "").length < 8} data-testid="device-continue">Continue</button>
        </form>
      ) : (
        <section className="box box-body" data-testid="device-authorize">
          <h1 className="h2">Authorize {req.app?.name || "this device"}</h1>
          {err && <Flash kind="error">{err}</Flash>}
          <p>{req.app ? <><strong>{req.app.name}</strong>{req.app.owner && <> by <strong>{req.app.owner}</strong></>} wants to access your <strong>{user.login}</strong> account.</> : <>A device wants to access your <strong>{user.login}</strong> account.</>}</p>
          {req.scopes.length > 0 ? <ul className="small">{req.scopes.map((s) => <li key={s}><code>{s}</code></li>)}</ul> : <p className="small muted">{req.app?.kind === "app" ? "It will act on your behalf within the permissions of its installations." : "It requests public, read-only access."}</p>}
          <p className="small muted">Code <span className="mono">{req.user_code}</span> · expires <RelTime iso={req.expires_at} /></p>
          <div className="form-actions">
            <button type="button" className="btn" onClick={() => decide(false)} data-testid="device-deny">Cancel</button>
            <button type="button" className="btn btn-primary" onClick={() => decide(true)} data-testid="device-authorize-btn">Authorize {req.app?.name || ""}</button>
          </div>
        </section>
      )}
    </div>
  );
}
