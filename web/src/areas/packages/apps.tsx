// GitHub Apps: developer settings (register, edit, permissions & events, private keys, client
// secret, visibility, delete), the public app page with the install flow, and the installation
// settings page (repository access, permission updates, suspend, uninstall).
import { useSettingsSplat } from "./splat";
import { useEffect, useMemo, useState, type FormEvent, type ReactNode } from "react";
import { Link, useLocation, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { type Repo, type User } from "../../api";
import { useAuth } from "../../auth";
import { Async, Avatar, Blankslate, CodeLine, ErrorBox, Flash, NotFoundView, RelTime, Spinner } from "../../components/common";
import { useAsync, useTitle } from "../../hooks";
import { appsApi, type CreatedApp, type Installation, type InstallationDetails, type ManagedApp, type NewAppBody } from "./api";
import { AppsIcon } from "./icons";

/** Whose apps a developer-settings page manages, and where its settings live. */
export interface AppOwner {
  login: string;
  isOrg: boolean;
  /** Settings root: `/settings` or `/organizations/{org}/settings`. */
  root: string;
}

// ---------------------------------------------------------------------------------------------
// Permission and event catalog (subset of GitHub's, grouped like the registration form)

type Level = "read" | "write";
interface PermDef { key: string; label: string; levels: Level[]; hint: string }

export const PERMISSION_GROUPS: { title: string; perms: PermDef[] }[] = [
  {
    title: "Repository permissions",
    perms: [
      { key: "actions", label: "Actions", levels: ["read", "write"], hint: "Workflows, workflow runs and artifacts." },
      { key: "administration", label: "Administration", levels: ["read", "write"], hint: "Repository creation, deletion, settings, teams, and collaborators." },
      { key: "checks", label: "Checks", levels: ["read", "write"], hint: "Checks on code." },
      { key: "contents", label: "Contents", levels: ["read", "write"], hint: "Repository contents, commits, branches, downloads, releases, and merges." },
      { key: "deployments", label: "Deployments", levels: ["read", "write"], hint: "Deployments and deployment statuses." },
      { key: "environments", label: "Environments", levels: ["read", "write"], hint: "Manage repository environments." },
      { key: "issues", label: "Issues", levels: ["read", "write"], hint: "Issues and related comments, assignees, labels, and milestones." },
      { key: "metadata", label: "Metadata", levels: ["read"], hint: "Search repositories, list collaborators, and access repository metadata. Mandatory." },
      { key: "packages", label: "Packages", levels: ["read", "write"], hint: "Packages published to the forge package registry." },
      { key: "pages", label: "Pages", levels: ["read", "write"], hint: "Retrieve Pages statuses, configuration, and builds." },
      { key: "pull_requests", label: "Pull requests", levels: ["read", "write"], hint: "Pull requests and related comments, assignees, labels, milestones, and merges." },
      { key: "repository_hooks", label: "Webhooks", levels: ["read", "write"], hint: "Manage the post-receive hooks for a repository." },
      { key: "secrets", label: "Secrets", levels: ["read", "write"], hint: "Manage Actions repository secrets." },
      { key: "security_events", label: "Code scanning alerts", levels: ["read", "write"], hint: "View and manage code scanning alerts." },
      { key: "statuses", label: "Commit statuses", levels: ["read", "write"], hint: "Commit statuses." },
      { key: "workflows", label: "Workflows", levels: ["write"], hint: "Update GitHub Actions workflow files." },
    ],
  },
  {
    title: "Organization permissions",
    perms: [
      { key: "members", label: "Members", levels: ["read", "write"], hint: "Organization members and teams." },
      { key: "organization_administration", label: "Administration", levels: ["read", "write"], hint: "Manage access to an organization." },
      { key: "organization_hooks", label: "Webhooks", levels: ["read", "write"], hint: "Manage the post-receive hooks for an organization." },
      { key: "organization_packages", label: "Packages", levels: ["read", "write"], hint: "Manage organization packages." },
      { key: "organization_secrets", label: "Secrets", levels: ["read", "write"], hint: "Manage Actions organization secrets." },
      { key: "organization_self_hosted_runners", label: "Self-hosted runners", levels: ["read", "write"], hint: "View and manage Actions self-hosted runners." },
    ],
  },
  {
    title: "Account permissions",
    perms: [
      { key: "email_addresses", label: "Email addresses", levels: ["read", "write"], hint: "Manage a user's email addresses." },
      { key: "followers", label: "Followers", levels: ["read", "write"], hint: "A user's followers." },
      { key: "git_ssh_keys", label: "Git SSH keys", levels: ["read", "write"], hint: "Git SSH keys." },
      { key: "gpg_keys", label: "GPG keys", levels: ["read", "write"], hint: "View and manage a user's GPG keys." },
      { key: "starring", label: "Starring", levels: ["read", "write"], hint: "List and manage repositories a user is starring." },
    ],
  },
];

const PERM_LABEL: Record<string, string> = Object.fromEntries(PERMISSION_GROUPS.flatMap((g) => g.perms.map((p) => [p.key, p.label])));
export const permLabel = (k: string) => PERM_LABEL[k] || k.replace(/_/g, " ").replace(/^./, (c) => c.toUpperCase());
export const levelLabel = (l: string) => (l === "read" ? "Read-only" : l === "write" ? "Read and write" : l === "admin" ? "Admin" : l);

export const EVENTS = [
  "check_run", "check_suite", "create", "delete", "deployment", "deployment_status", "fork", "issue_comment", "issues", "label",
  "member", "milestone", "public", "pull_request", "pull_request_review", "pull_request_review_comment", "push", "release",
  "repository", "star", "status", "watch", "workflow_dispatch", "workflow_job", "workflow_run",
];

/** Read-only list of granted permissions. */
export function PermissionList({ perms, events }: { perms: Record<string, string>; events?: string[] }) {
  const entries = Object.entries(perms).sort(([a], [b]) => a.localeCompare(b));
  return (
    <div className="app-perms" data-testid="permission-list">
      <ul className="app-perm-list">
        {entries.map(([k, v]) => <li key={k}><strong>{levelLabel(v)}</strong> access to {permLabel(k).toLowerCase()}</li>)}
      </ul>
      {events && events.length > 0 && <p className="small muted">Subscribed to events: {events.join(", ")}</p>}
    </div>
  );
}

function PermissionsEditor({ perms, setPerms, events, setEvents }: {
  perms: Record<string, string>; setPerms: (p: Record<string, string>) => void; events: string[]; setEvents: (e: string[]) => void;
}) {
  return (
    <>
      {PERMISSION_GROUPS.map((g) => (
        <fieldset key={g.title} className="form-group app-perm-group">
          <legend>{g.title}</legend>
          <ul className="list app-perm-rows">
            {g.perms.map((p) => (
              <li key={p.key} className="list-row app-perm-row">
                <div className="grow"><strong>{p.label}</strong><div className="small muted">{p.hint}</div></div>
                <select className="input input-sm input-auto" aria-label={`${g.title}: ${p.label}`} data-testid={`perm-${p.key}`}
                  value={perms[p.key] || ""} disabled={p.key === "metadata"}
                  onChange={(e) => {
                    const n = { ...perms };
                    if (e.target.value) n[p.key] = e.target.value;
                    else delete n[p.key];
                    setPerms(n);
                  }}>
                  {p.key !== "metadata" && <option value="">No access</option>}
                  {p.levels.map((l) => <option key={l} value={l}>{levelLabel(l)}</option>)}
                </select>
              </li>
            ))}
          </ul>
        </fieldset>
      ))}
      <fieldset className="form-group">
        <legend>Subscribe to events</legend>
        <p className="small muted">Your webhook receives these events for the repositories the app is installed on (installation events are always sent).</p>
        <div className="app-events">
          {EVENTS.map((ev) => (
            <label key={ev} className="checkbox app-event">
              <input type="checkbox" checked={events.includes(ev)} data-testid={`event-${ev}`}
                onChange={(e) => setEvents(e.target.checked ? [...events, ev] : events.filter((x) => x !== ev))} /> {ev}
            </label>
          ))}
        </div>
      </fieldset>
    </>
  );
}

/** Offer a PEM file to save, and show it (it can't be downloaded again). */
export function PemDownload({ pem, slug }: { pem: string; slug: string }) {
  const url = useMemo(() => URL.createObjectURL(new Blob([pem], { type: "application/x-pem-file" })), [pem]);
  useEffect(() => () => URL.revokeObjectURL(url), [url]);
  const file = `${slug}.${new Date().toISOString().slice(0, 10)}.private-key.pem`;
  useEffect(() => {
    // Start the download right away, like GitHub.
    const a = document.createElement("a");
    a.href = url;
    a.download = file;
    a.click();
  }, [url, file]);
  return (
    <Flash kind="success">
      <p><strong>Private key generated.</strong> Your browser downloaded <code>{file}</code>. Store it safely: forge keeps only the public half.</p>
      <p><a href={url} download={file} className="btn btn-sm" data-testid="pem-download">Download again</a></p>
      <details><summary className="small">Show private key</summary><pre className="pem" data-testid="pem">{pem}</pre></details>
    </Flash>
  );
}

// ---------------------------------------------------------------------------------------------
// Developer settings → GitHub Apps (user or organization)

/** Section element: handles `apps`, `apps/new`, `apps/{slug}[/permissions|/advanced|/installations]`. */
export function AppsSection({ owner }: { owner: AppOwner }) {
  const splat = useSettingsSplat();
  const parts = splat.split("/").filter(Boolean);
  // parts[0] === "apps"
  if (parts.length === 1) return <AppsList owner={owner} />;
  if (parts[1] === "new") return <NewApp owner={owner} />;
  return <AppSettings owner={owner} slug={parts[1]} tab={parts[2] || "general"} />;
}

function AppsList({ owner }: { owner: AppOwner }) {
  useTitle("GitHub Apps");
  const st = useAsync(() => appsApi.myApps(), []);
  return (
    <div data-testid="apps-list">
      <div className="toolbar">
        <h1 className="h2">GitHub Apps</h1>
        <span className="spacer" />
        <Link to={`${owner.root}/apps/new`} className="btn btn-primary btn-sm" data-testid="new-app">New GitHub App</Link>
      </div>
      <p className="muted">Want to build something that integrates with and extends forge? Register a new GitHub App to get started.</p>
      <Async state={st}>
        {(all) => {
          const list = all.filter((a) => a.kind === "app" && a.owner?.login.toLowerCase() === owner.login.toLowerCase());
          return list.length === 0 ? <Blankslate title="No GitHub Apps" icon={<AppsIcon size={24} />}>Register one to act on repositories with its own identity and permissions.</Blankslate> : (
            <section className="box">
              <ul className="list">
                {list.map((a) => (
                  <li key={a.id} className="list-row" data-testid="app-row">
                    <AppsIcon className="icon muted" />
                    <div className="grow"><Link to={`${owner.root}/apps/${a.slug}`}><strong>{a.name}</strong></Link>{a.description && <div className="small muted">{a.description}</div>}</div>
                    <Link to={`${owner.root}/apps/${a.slug}`} className="btn btn-sm">Edit</Link>
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

const lines = (s: string) => s.split(/\s*\n\s*/).map((x) => x.trim()).filter(Boolean);

function NewApp({ owner }: { owner: AppOwner }) {
  useTitle("Register new GitHub App");
  const nav = useNavigate();
  const [f, setF] = useState({ name: "", description: "", url: "", callbacks: "", setup_url: "", hook_url: "", hook_active: true, public: false, request_oauth: false });
  const [perms, setPerms] = useState<Record<string, string>>({ metadata: "read" });
  const [events, setEvents] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<unknown>();
  const set = (k: keyof typeof f) => (e: { target: { value: string; checked?: boolean; type?: string } }) =>
    setF({ ...f, [k]: e.target.type === "checkbox" || e.target.type === "radio" ? (k === "public" ? e.target.value === "any" : !!e.target.checked) : e.target.value });
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setErr(undefined);
    const body: NewAppBody = {
      name: f.name.trim(),
      url: f.url.trim(),
      description: f.description.trim() || undefined,
      callback_urls: lines(f.callbacks),
      setup_url: f.setup_url.trim() || undefined,
      hook_attributes: f.hook_active && f.hook_url.trim() ? { url: f.hook_url.trim(), active: true } : undefined,
      public: f.public,
      default_permissions: perms,
      default_events: f.hook_active ? events : [],
      request_oauth_on_install: f.request_oauth,
    };
    if (owner.isOrg) body.owner = owner.login;
    try {
      const app = await appsApi.createApp(body);
      nav(`${owner.root}/apps/${app.slug}`, { state: { created: app } });
    } catch (e2) {
      setErr(e2);
    } finally {
      setBusy(false);
    }
  };
  return (
    <form onSubmit={submit} className="app-form" data-testid="new-app-form">
      <h1 className="h2">Register new GitHub App</h1>
      {err !== undefined && <ErrorBox error={err} />}
      <div className="form-group">
        <label htmlFor="app-name">GitHub App name</label>
        <input id="app-name" className="input" required maxLength={34} value={f.name} onChange={set("name")} />
        <p className="small muted">The name of your GitHub App (at most 34 characters); its slug and bot account are derived from it.</p>
      </div>
      <div className="form-group">
        <label htmlFor="app-desc">Description <span className="muted">(optional)</span></label>
        <textarea id="app-desc" className="textarea" rows={3} value={f.description} onChange={set("description")} />
      </div>
      <div className="form-group">
        <label htmlFor="app-url">Homepage URL</label>
        <input id="app-url" className="input" type="url" required placeholder="https://example.com" value={f.url} onChange={set("url")} />
      </div>
      <h2 className="h4">Identifying and authorizing users</h2>
      <div className="form-group">
        <label htmlFor="app-callbacks">Callback URLs <span className="muted">(one per line)</span></label>
        <textarea id="app-callbacks" className="textarea" rows={2} value={f.callbacks} onChange={set("callbacks")} />
      </div>
      <label className="checkbox form-group"><input type="checkbox" checked={f.request_oauth} onChange={set("request_oauth")} /> Request user authorization (OAuth) during installation</label>
      <h2 className="h4">Post installation</h2>
      <div className="form-group">
        <label htmlFor="app-setup">Setup URL <span className="muted">(optional)</span></label>
        <input id="app-setup" className="input" type="url" value={f.setup_url} onChange={set("setup_url")} />
      </div>
      <h2 className="h4">Webhook</h2>
      <label className="checkbox form-group"><input type="checkbox" checked={f.hook_active} onChange={set("hook_active")} data-testid="hook-active" /> Active</label>
      {f.hook_active && (
        <div className="form-group">
          <label htmlFor="app-hook">Webhook URL</label>
          <input id="app-hook" className="input" type="url" value={f.hook_url} onChange={set("hook_url")} placeholder="https://example.com/webhook" />
          <p className="small muted">Events will POST to this URL. A webhook secret is generated for you; change it on the app's settings page.</p>
        </div>
      )}
      <h2 className="h4">Permissions</h2>
      <PermissionsEditor perms={perms} setPerms={setPerms} events={events} setEvents={setEvents} />
      <fieldset className="form-group">
        <legend>Where can this GitHub App be installed?</legend>
        <label className="radio"><input type="radio" name="app-public" value="only" checked={!f.public} onChange={set("public")} /> Only on this account (<strong>@{owner.login}</strong>)</label>
        <label className="radio"><input type="radio" name="app-public" value="any" checked={f.public} onChange={set("public")} data-testid="app-public-any" /> Any account</label>
      </fieldset>
      <button type="submit" className="btn btn-primary" disabled={busy || !f.name.trim() || !f.url.trim()} data-testid="create-app">
        {busy ? "Creating…" : "Create GitHub App"}
      </button>
    </form>
  );
}

function AppSettings({ owner, slug, tab }: { owner: AppOwner; slug: string; tab: string }) {
  const st = useAsync(() => appsApi.manage(slug), [slug]);
  const loc = useLocation();
  const created = (loc.state as { created?: CreatedApp } | null)?.created;
  useTitle(st.data ? st.data.name : "GitHub App");
  const base = `${owner.root}/apps/${slug}`;
  const t = (key: string, to: string, label: string) => (
    <Link to={to} className={"tabnav-tab" + (tab === key ? " selected" : "")} data-testid={`app-tab-${key}`}>{label}</Link>
  );
  return (
    <Async state={st}>
      {(app) => app.kind !== "app" ? <NotFoundView /> : (
        <div data-testid="app-settings">
          <div className="toolbar">
            <h1 className="h2 grow">{app.name}</h1>
            <Link to={`/apps/${app.slug}`} className="btn btn-sm">Public page</Link>
          </div>
          <nav className="tabnav" aria-label="App settings">
            {t("general", base, "General")}
            {t("permissions", `${base}/permissions`, "Permissions & events")}
            {t("installations", `${base}/installations`, "Install App")}
            {t("advanced", `${base}/advanced`, "Advanced")}
          </nav>
          {created && tab === "general" && <CreatedFlash app={created} />}
          {tab === "general" && <AppGeneral app={app} setApp={st.setData} />}
          {tab === "permissions" && <AppPermissions app={app} setApp={st.setData} />}
          {tab === "installations" && <AppInstallTab app={app} />}
          {tab === "advanced" && <AppAdvanced app={app} owner={owner} setApp={st.setData} />}
        </div>
      )}
    </Async>
  );
}

function CreatedFlash({ app }: { app: CreatedApp }) {
  return (
    <div data-testid="app-created">
      <Flash kind="success"><strong>Registration successful.</strong> Save the credentials below: they are shown only once.</Flash>
      <PemDownload pem={app.pem} slug={app.slug} />
      <div className="form-group"><label>Client secret</label><CodeLine text={app.client_secret} label="Client secret" /></div>
      {app.webhook_secret && <div className="form-group"><label>Webhook secret</label><CodeLine text={app.webhook_secret} label="Webhook secret" /></div>}
    </div>
  );
}

function AppGeneral({ app, setApp }: { app: ManagedApp; setApp: (a: ManagedApp) => void }) {
  const [f, setF] = useState({
    name: app.name, description: app.description || "", url: app.external_url, callbacks: app.callback_urls.join("\n"),
    setup_url: app.setup_url || "", setup_on_update: app.setup_on_update, request_oauth: app.request_oauth_on_install,
    hook_url: app.webhook?.url || "", hook_active: !!app.webhook?.active, hook_secret: "",
  });
  const [err, setErr] = useState<unknown>();
  const [msg, setMsg] = useState("");
  const [secret, setSecret] = useState("");
  const [pem, setPem] = useState("");
  const [busy, setBusy] = useState(false);
  const set = (k: keyof typeof f) => (e: { target: { value: string; checked?: boolean; type?: string } }) =>
    setF({ ...f, [k]: e.target.type === "checkbox" ? !!e.target.checked : e.target.value });
  const run = async (fn: () => Promise<void>) => {
    setErr(undefined);
    setMsg("");
    try { await fn(); } catch (e) { setErr(e); }
  };
  const save = (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    run(async () => {
      const updated = await appsApi.update(app.slug, {
        name: f.name.trim(), description: f.description, url: f.url.trim(), callback_urls: lines(f.callbacks),
        setup_url: f.setup_url.trim(), setup_on_update: f.setup_on_update, request_oauth_on_install: f.request_oauth,
        hook_attributes: { url: f.hook_url.trim(), active: f.hook_active },
        ...(f.hook_secret ? { webhook_secret: f.hook_secret } : {}),
      });
      setApp(updated);
      setF({ ...f, hook_secret: "" });
      setMsg("Your changes have been saved.");
    }).finally(() => setBusy(false));
  };
  return (
    <div className="app-general">
      {msg && <Flash kind="success">{msg}</Flash>}
      {err !== undefined && <ErrorBox error={err} />}
      <section className="box">
        <div className="box-header"><h2 className="h5">About</h2></div>
        <dl className="box-body pkg-dl">
          <dt>Owned by</dt><dd><Avatar user={app.owner} size={16} /> <Link to={`/${app.owner.login}`}>@{app.owner.login}</Link></dd>
          <dt>App ID</dt><dd className="mono" data-testid="app-id">{app.id}</dd>
          <dt>Client ID</dt><dd className="mono" data-testid="app-client-id">{app.client_id}</dd>
          <dt>Public link</dt><dd><Link to={`/apps/${app.slug}`}>{location.origin}/apps/{app.slug}</Link></dd>
          {app.bot_login && <><dt>Bot account</dt><dd className="mono">{app.bot_login}</dd></>}
          <dt>Installations</dt><dd>{app.installations_count ?? 0}</dd>
        </dl>
      </section>

      <section className="box">
        <div className="box-header"><h2 className="h5">Client secrets</h2></div>
        <div className="box-body">
          <p className="small muted">The client secret authenticates the app to the OAuth endpoints (device flow refresh, token checks). Generating a new secret replaces the old one.</p>
          {secret && <Flash kind="success"><p>Copy your new client secret now. You won't be able to see it again.</p><CodeLine text={secret} label="New client secret" /></Flash>}
          <button type="button" className="btn btn-sm" data-testid="new-client-secret"
            onClick={() => run(async () => { if (confirm("Generate a new client secret? The current secret stops working.")) setSecret((await appsApi.newSecret(app.slug)).client_secret); })}>
            Generate a new client secret
          </button>
        </div>
      </section>

      <form className="box" onSubmit={save} data-testid="app-general-form">
        <div className="box-header"><h2 className="h5">Basic information</h2></div>
        <div className="box-body">
          <div className="form-group"><label htmlFor="ag-name">GitHub App name</label><input id="ag-name" className="input" required maxLength={34} value={f.name} onChange={set("name")} /></div>
          <div className="form-group"><label htmlFor="ag-desc">Description</label><textarea id="ag-desc" className="textarea" rows={3} value={f.description} onChange={set("description")} /></div>
          <div className="form-group"><label htmlFor="ag-url">Homepage URL</label><input id="ag-url" className="input" type="url" required value={f.url} onChange={set("url")} /></div>
          <h3 className="h5">Identifying and authorizing users</h3>
          <div className="form-group"><label htmlFor="ag-cb">Callback URLs (one per line)</label><textarea id="ag-cb" className="textarea" rows={2} value={f.callbacks} onChange={set("callbacks")} /></div>
          <label className="checkbox form-group"><input type="checkbox" checked={f.request_oauth} onChange={set("request_oauth")} /> Request user authorization (OAuth) during installation</label>
          <h3 className="h5">Post installation</h3>
          <div className="form-group"><label htmlFor="ag-setup">Setup URL</label><input id="ag-setup" className="input" type="url" value={f.setup_url} onChange={set("setup_url")} /></div>
          <label className="checkbox form-group"><input type="checkbox" checked={f.setup_on_update} onChange={set("setup_on_update")} /> Redirect on update</label>
          <h3 className="h5">Webhook</h3>
          <label className="checkbox form-group"><input type="checkbox" checked={f.hook_active} onChange={set("hook_active")} /> Active</label>
          <div className="form-group"><label htmlFor="ag-hook">Webhook URL</label><input id="ag-hook" className="input" type="url" value={f.hook_url} onChange={set("hook_url")} data-testid="app-hook-url" /></div>
          <div className="form-group">
            <label htmlFor="ag-secret">Webhook secret</label>
            <input id="ag-secret" className="input" type="password" autoComplete="new-password" value={f.hook_secret} onChange={set("hook_secret")}
              placeholder={app.webhook?.has_secret ? "•••••••• (unchanged)" : "No secret set"} />
          </div>
          <button type="submit" className="btn btn-primary" disabled={busy} data-testid="app-save">{busy ? "Saving…" : "Save changes"}</button>
        </div>
      </form>

      <section className="box" data-testid="app-keys">
        <div className="box-header"><h2 className="h5">Private keys</h2></div>
        <div className="box-body">
          <p className="small muted">You need a private key to sign access token requests (a JWT with the app ID as issuer, signed RS256).</p>
          {pem && <PemDownload pem={pem} slug={app.slug} />}
        </div>
        <ul className="list">
          {(app.keys || []).map((k) => (
            <li key={k.id} className="list-row" data-testid="app-key-row">
              <div className="grow"><span className="mono small">{k.fingerprint}</span><div className="small muted">Added <RelTime iso={k.created_at} /></div></div>
              <button type="button" className="btn btn-sm btn-danger-outline" disabled={(app.keys || []).length <= 1} data-testid="delete-key"
                onClick={() => run(async () => {
                  if (!confirm("Delete this private key? JWTs signed with it stop working.")) return;
                  await appsApi.deleteKey(app.slug, k.id);
                  setApp({ ...app, keys: (app.keys || []).filter((x) => x.id !== k.id) });
                })}>Delete</button>
            </li>
          ))}
        </ul>
        <div className="box-footer">
          <button type="button" className="btn btn-sm" data-testid="generate-key" onClick={() => run(async () => {
            const k = await appsApi.addKey(app.slug);
            setPem(k.pem);
            setApp(await appsApi.manage(app.slug));
          })}>Generate a private key</button>
        </div>
      </section>
    </div>
  );
}

function AppPermissions({ app, setApp }: { app: ManagedApp; setApp: (a: ManagedApp) => void }) {
  const [perms, setPerms] = useState<Record<string, string>>(app.permissions || { metadata: "read" });
  const [events, setEvents] = useState<string[]>(app.events || []);
  const [err, setErr] = useState<unknown>();
  const [msg, setMsg] = useState("");
  return (
    <form data-testid="app-permissions-form" onSubmit={async (e) => {
      e.preventDefault();
      setErr(undefined);
      try {
        setApp(await appsApi.update(app.slug, { default_permissions: perms, default_events: events }));
        setMsg("Permissions saved. Installations keep their current permissions until their owners accept the update.");
      } catch (e2) {
        setErr(e2);
      }
    }}>
      {msg && <Flash kind="success">{msg}</Flash>}
      {err !== undefined && <ErrorBox error={err} />}
      <PermissionsEditor perms={perms} setPerms={setPerms} events={events} setEvents={setEvents} />
      <button type="submit" className="btn btn-primary" data-testid="save-permissions">Save changes</button>
    </form>
  );
}

/** Accounts the viewer can install apps on: themselves and organizations they own. */
export function useInstallAccounts() {
  const { user } = useAuth();
  return useAsync(async () => {
    if (!user) return [] as User[];
    const mships = await appsApi.myOrgs().catch(() => [] as User[]);
    const owned: User[] = [];
    for (const o of mships) {
      const m = await appsApi.membership(o.login).catch(() => null);
      if (m?.role === "admin") owned.push({ ...o, type: "Organization" });
    }
    return [user, ...owned];
  }, [user?.login]);
}

function AppInstallTab({ app }: { app: ManagedApp }) {
  const accounts = useInstallAccounts();
  const inst = useAsync(() => appsApi.myInstallations(), []);
  return (
    <section className="box" data-testid="app-install-tab">
      <div className="box-header"><h2 className="h5">Install {app.name}</h2></div>
      <p className="box-body small muted">Choose an account to install this GitHub App on.{!app.public && " This app is private: it can only be installed on the account that owns it."}</p>
      <Async state={accounts}>
        {(list) => (
          <ul className="list">
            {list.map((a) => {
              const existing = inst.data?.installations.find((i) => i.app_slug === app.slug && i.account.login === a.login);
              const allowed = app.public || a.login === app.owner.login;
              return (
                <li key={a.login} className="list-row" data-testid="install-account-row">
                  <Avatar user={a} size={24} />
                  <strong className="grow">{a.login}</strong>
                  {existing ? <Link className="btn btn-sm" to={installationPath(existing)}>Configure</Link>
                    : allowed ? <Link className="btn btn-sm btn-primary" to={`/apps/${app.slug}/installations/new/permissions?target_id=${a.id}`} data-testid="install-on-account">Install</Link>
                      : <span className="muted small">Not installable</span>}
                </li>
              );
            })}
          </ul>
        )}
      </Async>
    </section>
  );
}

function AppAdvanced({ app, owner, setApp }: { app: ManagedApp; owner: AppOwner; setApp: (a: ManagedApp) => void }) {
  const nav = useNavigate();
  const [err, setErr] = useState<unknown>();
  const [confirmText, setConfirmText] = useState("");
  return (
    <div data-testid="app-advanced">
      {err !== undefined && <ErrorBox error={err} />}
      <h2 className="h3 c-danger">Danger zone</h2>
      <section className="box danger-zone">
        <div className="box-body pkg-danger-row">
          <div className="grow">
            <h3 className="h5">Make this GitHub App {app.public ? "private" : "public"}</h3>
            <p className="small muted">{app.public ? "Only the owning account will be able to install it." : "Any account will be able to install it."}</p>
          </div>
          <button type="button" className="btn btn-danger-outline" data-testid="toggle-public" onClick={async () => {
            setErr(undefined);
            try { setApp(await appsApi.update(app.slug, { public: !app.public })); } catch (e) { setErr(e); }
          }}>Make {app.public ? "private" : "public"}</button>
        </div>
        <form className="box-body" onSubmit={async (e) => {
          e.preventDefault();
          setErr(undefined);
          try { await appsApi.remove(app.slug); nav(`${owner.root}/apps`); } catch (e2) { setErr(e2); }
        }}>
          <h3 className="h5">Delete GitHub App</h3>
          <p className="small muted">This deletes the app, its bot account, its installations and every token it issued.</p>
          <div className="form-group">
            <label htmlFor="app-del">To confirm, type <strong>{app.name}</strong></label>
            <input id="app-del" className="input" autoComplete="off" value={confirmText} onChange={(e) => setConfirmText(e.target.value)} />
          </div>
          <button type="submit" className="btn btn-danger" disabled={confirmText !== app.name} data-testid="delete-app">Delete GitHub App</button>
        </form>
      </section>
    </div>
  );
}

// ---------------------------------------------------------------------------------------------
// Public app page and install flow

export const installationPath = (i: Pick<Installation, "id" | "account" | "target_type">) =>
  i.target_type === "Organization" ? `/organizations/${i.account.login}/settings/installations/${i.id}` : `/settings/installations/${i.id}`;

/** `/apps/:slug` */
export function AppPublicPage() {
  const { slug = "" } = useParams();
  const { user } = useAuth();
  const st = useAsync(() => appsApi.publicApp(slug), [slug]);
  useTitle(st.data?.name || "GitHub App");
  return (
    <div className="container app-public" data-testid="app-public">
      <Async state={st}>
        {(app) => (
          <div className="layout-sidebar">
            <div className="layout-main">
              <div className="app-public-head">
                <div className="app-logo" aria-hidden="true">{app.name.slice(0, 1).toUpperCase()}</div>
                <div>
                  <h1 className="h2">{app.name}</h1>
                  <p className="muted small">GitHub App by <Link to={`/${app.owner.login}`}>{app.owner.login}</Link></p>
                </div>
              </div>
              {app.description ? <p>{app.description}</p> : <p className="muted"><em>No description.</em></p>}
              <section className="box">
                <div className="box-header"><h2 className="h5">Permissions</h2></div>
                <div className="box-body"><PermissionList perms={app.permissions} events={app.events} /></div>
              </section>
            </div>
            <aside className="layout-aside">
              {user ? <Link to={`/apps/${app.slug}/installations/new`} className="btn btn-primary btn-block" data-testid="app-install">Install</Link>
                : <Link to={`/login?return_to=${encodeURIComponent(`/apps/${app.slug}`)}`} className="btn btn-primary btn-block">Sign in to install</Link>}
              <ul className="pkg-details small">
                <li>Homepage: <a href={app.external_url} rel="nofollow noopener">{app.external_url}</a></li>
                <li>{app.installations_count} installation{app.installations_count === 1 ? "" : "s"}</li>
                <li>Created <RelTime iso={app.created_at} /></li>
              </ul>
            </aside>
          </div>
        )}
      </Async>
    </div>
  );
}

/** `/apps/:slug/installations/new` (pick an account) and `.../new/permissions?target_id=` (pick repositories). */
export function AppInstallPage({ step }: { step: "account" | "permissions" }) {
  const { slug = "" } = useParams();
  const { user, loading } = useAuth();
  const [sp] = useSearchParams();
  const app = useAsync(() => appsApi.publicApp(slug), [slug]);
  const accounts = useInstallAccounts();
  const inst = useAsync(() => (user ? appsApi.myInstallations() : Promise.resolve({ total_count: 0, installations: [] as Installation[] })), [user?.login]);
  useTitle(app.data ? `Install ${app.data.name}` : "Install");
  if (loading) return <Spinner />;
  if (!user) return <NotFoundView />;
  return (
    <div className="container narrow app-install" data-testid="app-install-page">
      <Async state={app}>
        {(a) => (
          <Async state={accounts}>
            {(list) => {
              if (step === "account") {
                return (
                  <section className="box">
                    <div className="box-header"><h1 className="h4">Install {a.name}</h1></div>
                    <p className="box-body small muted">Where do you want to install {a.name}?</p>
                    <ul className="list">
                      {list.map((acc) => {
                        const existing = inst.data?.installations.find((i) => i.app_slug === a.slug && i.account.login === acc.login);
                        return (
                          <li key={acc.login} className="list-row" data-testid="install-account-row">
                            <Avatar user={acc} size={24} />
                            <strong className="grow">{acc.login}</strong>
                            {existing ? <Link className="btn btn-sm" to={installationPath(existing)}>Configure</Link>
                              : <Link className="btn btn-sm btn-primary" to={`/apps/${a.slug}/installations/new/permissions?target_id=${acc.id}`} data-testid="install-on-account">Install</Link>}
                          </li>
                        );
                      })}
                    </ul>
                  </section>
                );
              }
              const target = list.find((x) => String(x.id) === sp.get("target_id"));
              if (!target) return <NotFoundView />;
              return <InstallForm app={a} account={target} />;
            }}
          </Async>
        )}
      </Async>
    </div>
  );
}

function RepoPicker({ repos, selected, setSelected }: { repos: Repo[]; selected: number[]; setSelected: (ids: number[]) => void }) {
  const [q, setQ] = useState("");
  const shown = repos.filter((r) => r.name.toLowerCase().includes(q.toLowerCase()));
  return (
    <div className="repo-picker" data-testid="repo-picker">
      <input className="input input-sm" type="search" placeholder="Search for a repository" aria-label="Search for a repository" value={q} onChange={(e) => setQ(e.target.value)} />
      <ul className="list repo-picker-list">
        {shown.map((r) => (
          <li key={r.id} className="list-row">
            <label className="checkbox grow">
              <input type="checkbox" checked={selected.includes(r.id)} data-testid={`repo-pick-${r.name}`}
                onChange={(e) => setSelected(e.target.checked ? [...selected, r.id] : selected.filter((x) => x !== r.id))} /> {r.full_name}
            </label>
          </li>
        ))}
        {shown.length === 0 && <li className="list-row muted small">No repositories found.</li>}
      </ul>
      <p className="small muted">Selected {selected.length} of {repos.length} repositories.</p>
    </div>
  );
}

function InstallForm({ app, account }: { app: { slug: string; name: string; permissions: Record<string, string>; events: string[] }; account: User }) {
  const nav = useNavigate();
  const repos = useAsync(() => appsApi.accountRepos(account), [account.login]);
  const [mode, setMode] = useState<"all" | "selected">("all");
  const [selected, setSelected] = useState<number[]>([]);
  const [err, setErr] = useState<unknown>();
  const [busy, setBusy] = useState(false);
  return (
    <form className="box" data-testid="install-form" onSubmit={async (e) => {
      e.preventDefault();
      setBusy(true);
      setErr(undefined);
      try {
        const i = await appsApi.install(app.slug, { account: account.login, repository_selection: mode, repository_ids: mode === "selected" ? selected : [] });
        nav(installationPath(i), { state: { installed: true } });
      } catch (e2) {
        setErr(e2);
      } finally {
        setBusy(false);
      }
    }}>
      <div className="box-header"><h1 className="h4">Install {app.name} on <Avatar user={account} size={20} /> {account.login}</h1></div>
      <div className="box-body">
        {err !== undefined && <ErrorBox error={err} />}
        <fieldset className="form-group">
          <legend>Repository access</legend>
          <label className="radio"><input type="radio" name="sel" checked={mode === "all"} onChange={() => setMode("all")} /> <strong>All repositories</strong>
            <span className="small muted"> — this applies to current and future repositories owned by {account.login}.</span></label>
          <label className="radio"><input type="radio" name="sel" checked={mode === "selected"} onChange={() => setMode("selected")} data-testid="select-repos" /> <strong>Only select repositories</strong></label>
          {mode === "selected" && (
            <Async state={repos}>{(rs) => <RepoPicker repos={rs.filter((r) => r.owner.login === account.login)} selected={selected} setSelected={setSelected} />}</Async>
          )}
        </fieldset>
        <h2 className="h5">with these permissions:</h2>
        <PermissionList perms={app.permissions} events={app.events} />
        <button type="submit" className="btn btn-primary" disabled={busy || (mode === "selected" && selected.length === 0)} data-testid="confirm-install">
          {busy ? "Installing…" : "Install"}
        </button>
      </div>
    </form>
  );
}

// ---------------------------------------------------------------------------------------------
// Installed GitHub Apps (user Settings → Applications, org Settings → GitHub Apps)

export function InstallationsList({ load, empty, base }: { load: () => Promise<Installation[]>; empty: ReactNode; base: string }) {
  const st = useAsync(load, [base]);
  return (
    <Async state={st}>
      {(list) => list.length === 0 ? <Blankslate title="No installed GitHub Apps" icon={<AppsIcon size={24} />}>{empty}</Blankslate> : (
        <section className="box" data-testid="installations-list">
          <ul className="list">
            {list.map((i) => (
              <li key={i.id} className="list-row" data-testid="installation-row">
                <AppsIcon className="icon muted" />
                <div className="grow">
                  <strong>{i.app_slug}</strong>{i.suspended_at && <span className="badge badge-warn">Suspended</span>}
                  <div className="small muted">On {i.account.login} · {i.repository_selection === "all" ? "All repositories" : "Selected repositories"} · Installed <RelTime iso={i.created_at} /></div>
                </div>
                <Link className="btn btn-sm" to={`${base}/${i.id}`} data-testid="configure-installation">Configure</Link>
              </li>
            ))}
          </ul>
        </section>
      )}
    </Async>
  );
}

/** Installation settings: `/settings/installations/{id}` and the org equivalent. */
export function InstallationPage({ id, back }: { id: number; back: string }) {
  const st = useAsync(() => appsApi.installation(id), [id]);
  const loc = useLocation();
  const installed = (loc.state as { installed?: boolean } | null)?.installed;
  useTitle("Installed GitHub App");
  return (
    <Async state={st}>
      {(i) => <InstallationView i={i} setI={st.setData} back={back} installed={!!installed} />}
    </Async>
  );
}

function InstallationView({ i, setI, back, installed }: { i: InstallationDetails; setI: (x: InstallationDetails) => void; back: string; installed: boolean }) {
  const nav = useNavigate();
  const repos = useAsync(() => appsApi.accountRepos(i.account), [i.account.login]);
  const [mode, setMode] = useState(i.repository_selection);
  const [selected, setSelected] = useState<number[]>(i.repositories.map((r) => r.id));
  const [err, setErr] = useState<unknown>();
  const [msg, setMsg] = useState(installed ? `${i.app.name} was installed on ${i.account.login}.` : "");
  const run = async (fn: () => Promise<void>) => {
    setErr(undefined);
    setMsg("");
    try { await fn(); } catch (e) { setErr(e); }
  };
  const can = i.viewer_can_manage;
  return (
    <div data-testid="installation-page">
      {msg && <Flash kind="success">{msg}</Flash>}
      {err !== undefined && <ErrorBox error={err} />}
      <div className="toolbar">
        <div className="app-logo" aria-hidden="true">{i.app.name.slice(0, 1).toUpperCase()}</div>
        <div className="grow">
          <h1 className="h2">{i.app.name}</h1>
          <p className="small muted">Installed <RelTime iso={i.created_at} /> on {i.account.login} · Developed by <Link to={`/${i.app.owner.login}`}>{i.app.owner.login}</Link> · <Link to={`/apps/${i.app.slug}`}>App settings page</Link></p>
        </div>
      </div>
      {i.suspended_at && <Flash kind="warn">This installation was suspended <RelTime iso={i.suspended_at} />{i.suspended_by && <> by {i.suspended_by.login}</>}. The app can't access {i.account.login}'s resources.</Flash>}
      {i.pending_update && (
        <section className="box" data-testid="pending-permissions">
          <div className="box-header"><h2 className="h5">{i.app.name} is requesting updated permissions</h2></div>
          <div className="box-body">
            <PermissionList perms={i.pending_update.permissions} events={i.pending_update.events} />
            {can && <button type="button" className="btn btn-primary btn-sm" data-testid="accept-permissions"
              onClick={() => run(async () => { setI(await appsApi.acceptPermissions(i.id)); setMsg("New permissions accepted."); })}>Accept new permissions</button>}
          </div>
        </section>
      )}
      <section className="box">
        <div className="box-header"><h2 className="h5">Permissions</h2></div>
        <div className="box-body"><PermissionList perms={i.permissions} events={i.events} /></div>
      </section>
      <form className="box" data-testid="installation-repos" onSubmit={(e) => {
        e.preventDefault();
        run(async () => { setI(await appsApi.updateInstallation(i.id, { repository_selection: mode, repository_ids: mode === "selected" ? selected : [] })); setMsg("Repository access saved."); });
      }}>
        <div className="box-header"><h2 className="h5">Repository access</h2></div>
        <div className="box-body">
          <label className="radio"><input type="radio" name="isel" disabled={!can} checked={mode === "all"} onChange={() => setMode("all")} data-testid="inst-all" /> <strong>All repositories</strong></label>
          <label className="radio"><input type="radio" name="isel" disabled={!can} checked={mode === "selected"} onChange={() => setMode("selected")} data-testid="inst-selected" /> <strong>Only select repositories</strong></label>
          {mode === "selected" && (can
            ? <Async state={repos}>{(rs) => <RepoPicker repos={rs.filter((r) => r.owner.login === i.account.login)} selected={selected} setSelected={setSelected} />}</Async>
            : <ul className="small">{i.repositories.map((r) => <li key={r.id}>{r.full_name}</li>)}</ul>)}
          {can && <button type="submit" className="btn btn-primary btn-sm" disabled={mode === "selected" && selected.length === 0} data-testid="save-installation">Save</button>}
        </div>
      </form>
      {can && (
        <>
          <h2 className="h3 c-danger">Danger zone</h2>
          <section className="box danger-zone">
            <div className="box-body pkg-danger-row">
              <div className="grow">
                <h3 className="h5">{i.suspended_at ? "Unsuspend" : "Suspend"} your installation</h3>
                <p className="small muted">{i.suspended_at ? "Give the app its access back." : "Block the app's access to your resources while keeping its settings."}</p>
              </div>
              <button type="button" className="btn btn-danger-outline" data-testid="toggle-suspend" onClick={() => run(async () => {
                if (i.suspended_at) await appsApi.unsuspend(i.id);
                else if (confirm(`Suspend ${i.app.name}?`)) await appsApi.suspend(i.id);
                else return;
                setI(await appsApi.installation(i.id));
              })}>{i.suspended_at ? "Unsuspend" : "Suspend"}</button>
            </div>
            <div className="box-body pkg-danger-row">
              <div className="grow"><h3 className="h5">Uninstall "{i.app.name}"</h3><p className="small muted">This removes the app's access and revokes its tokens.</p></div>
              <button type="button" className="btn btn-danger" data-testid="uninstall" onClick={() => run(async () => {
                if (!confirm(`Uninstall ${i.app.name}?`)) return;
                await appsApi.uninstall(i.id);
                nav(back);
              })}>Uninstall</button>
            </div>
          </section>
        </>
      )}
    </div>
  );
}
