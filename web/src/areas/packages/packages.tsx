// Packages: account / organization / repository package lists, the package page (install
// instructions, README, versions), version pages, package settings (visibility, linked
// repository, access) and the "deleted packages" restore lists.
import { useState, type FormEvent, type ReactNode } from "react";
import { Link, Navigate, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { api, errorMessage, type User } from "../../api";
import { useAuth } from "../../auth";
import { Async, Avatar, Blankslate, CodeLine, ErrorBox, Flash, NotFoundView, RelTime, Spinner } from "../../components/common";
import { LockIcon, RepoIcon, TagIcon } from "../../components/icons";
import type { RepoCtx } from "../../components/RepoLayout";
import { useRepo } from "../../components/RepoLayout";
import { useAsync, useTitle } from "../../hooks";
import { Markdown } from "../../markdown";
import { humanSize } from "../../util";
import { PACKAGE_TYPES, packagePath, pkgApi, type Package, type PackageDetails, type PackageVersion } from "./api";
import { PackageIcon } from "./icons";

const TYPE_LABEL: Record<string, string> = {
  container: "Container", docker: "Docker", npm: "npm", maven: "Apache Maven", rubygems: "RubyGems", nuget: "NuGet",
};
export const typeLabel = (t: string) => TYPE_LABEL[t] || t;

const shortDigest = (d: string) => (d.startsWith("sha256:") ? d.slice(0, 19) : d);

/** `HOST/owner/name` for container images (owner lowercased, like ghcr.io). */
export const imageRef = (p: Pick<Package, "owner" | "name">) => `${location.host}/${p.owner.login.toLowerCase()}/${p.name}`;
export const npmName = (p: Pick<Package, "owner" | "name">) => `@${p.owner.login.toLowerCase()}/${p.name}`;

function VisibilityBadge({ v }: { v: string }) {
  return <span className="badge" data-testid="pkg-visibility">{v === "public" ? "Public" : v === "internal" ? "Internal" : <><LockIcon size={12} /> Private</>}</span>;
}

// ---------------------------------------------------------------------------------------------
// Lists

export function PackageRows({ packages, showOwner }: { packages: Package[]; showOwner?: boolean }) {
  return (
    <ul className="list pkg-list" data-testid="package-list">
      {packages.map((p) => (
        <li key={p.id} className="list-row pkg-row">
          <PackageIcon className="icon muted" />
          <div className="grow">
            <h3 className="h4 pkg-row-title">
              <Link to={packagePath(p)} data-testid="package-link">{showOwner ? `${p.owner.login}/${p.name}` : p.name}</Link>{" "}
              <VisibilityBadge v={p.visibility} />
            </h3>
            <div className="muted small pkg-row-meta">
              <span>{typeLabel(p.package_type)}</span>
              <span>Published <RelTime iso={p.updated_at} /></span>
              <span>{p.version_count} version{p.version_count === 1 ? "" : "s"}</span>
              {p.repository && <span><RepoIcon size={12} /> <Link to={`/${p.repository.full_name}`}>{p.repository.full_name}</Link></span>}
            </div>
          </div>
        </li>
      ))}
    </ul>
  );
}

/** Packages of a user or organization, with type filter and search (profile tab and /orgs/:org/packages). */
export function OwnerPackages({ owner }: { owner: User }) {
  const [sp, setSp] = useSearchParams();
  const type = sp.get("package_type") || sp.get("ecosystem") || "";
  const q = sp.get("q") || "";
  const [text, setText] = useState(q);
  const st = useAsync(() => pkgApi.listAll(owner.login, { package_type: type || undefined, q: q || undefined }), [owner.login, type, q]);
  const set = (k: string, v: string) => {
    const n = new URLSearchParams(sp);
    if (v) n.set(k, v);
    else n.delete(k);
    setSp(n);
  };
  return (
    <div className="pkg-owner">
      <form className="toolbar pkg-filters" onSubmit={(e) => { e.preventDefault(); set("q", text.trim()); }}>
        <input className="input grow" type="search" placeholder="Search packages…" aria-label="Search packages" value={text}
          onChange={(e) => setText(e.target.value)} data-testid="pkg-search" />
        <select className="input input-auto" aria-label="Type" value={type} onChange={(e) => set("package_type", e.target.value)} data-testid="pkg-type-filter">
          <option value="">All types</option>
          {PACKAGE_TYPES.filter((t) => t !== "docker").map((t) => <option key={t} value={t}>{typeLabel(t)}</option>)}
        </select>
      </form>
      <Async state={st}>
        {(list) => list.length === 0 ? (
          <Blankslate title={q || type ? "No packages match your filters." : `${owner.login} has no packages yet.`} icon={<PackageIcon size={24} />}>
            <p>Publish container images with <code>docker push {location.host}/{owner.login.toLowerCase()}/IMAGE</code> or npm packages to <code>{location.origin}/npm/</code>.</p>
          </Blankslate>
        ) : (
          <section className="box"><PackageRows packages={list} /></section>
        )}
      </Async>
    </div>
  );
}

export function ProfilePackagesTab({ user }: { user: User }) {
  return <OwnerPackages owner={user} />;
}

/** `/orgs/:org/packages` */
export function OrgPackagesPage() {
  const { org = "" } = useParams();
  const st = useAsync(() => api.user(org), [org]);
  useTitle(`Packages · ${org}`);
  return (
    <div className="container pkg-page">
      <Async state={st}>
        {(o) => (
          <>
            <div className="pkg-org-head">
              <Avatar user={o} size={48} />
              <div>
                <h1 className="h2">{o.name || o.login}</h1>
                <nav className="muted small"><Link to={`/${o.login}`}>Overview</Link> · <Link to={`/orgs/${o.login}/packages`} aria-current="page"><strong>Packages</strong></Link></nav>
              </div>
            </div>
            <h2 className="h3">Packages</h2>
            <OwnerPackages owner={o} />
          </>
        )}
      </Async>
    </div>
  );
}

/** `/users/:owner/packages` → the profile's Packages tab. */
export function UserPackagesRedirect() {
  const { owner = "" } = useParams();
  return <Navigate to={`/${owner}?tab=packages`} replace />;
}

/** `/{o}/{r}/packages` */
export function RepoPackagesPage() {
  const { owner, name, repo } = useRepo();
  useTitle(`Packages · ${owner}/${name}`);
  const st = useAsync(() => pkgApi.repoPackages(owner, name), [owner, name]);
  return (
    <div className="pkg-repo">
      <h2 className="h3">Packages</h2>
      <Async state={st}>
        {(list) => list.length === 0 ? (
          <Blankslate title="No packages published" icon={<PackageIcon size={24} />}>
            <p>Packages are linked to this repository when they are published with its URL: the
              <code> org.opencontainers.image.source</code> label for container images, or the <code>repository</code> field of
              <code> package.json</code> for npm, pointing at <code>{location.origin}/{repo.full_name}</code>.</p>
          </Blankslate>
        ) : <section className="box"><PackageRows packages={list} showOwner /></section>}
      </Async>
    </div>
  );
}

/** "Packages" box on the repository Code page sidebar. */
export function RepoPackagesSidebar({ ctx }: { ctx: RepoCtx; refName: string }) {
  const st = useAsync(() => pkgApi.repoPackages(ctx.owner, ctx.name), [ctx.owner, ctx.name]);
  const list = st.data || [];
  return (
    <section className="sidebar-section pkg-sidebar" data-testid="repo-packages-sidebar">
      <h2 className="h4"><Link to={`${ctx.base}/packages`} className="muted-link">Packages</Link>{list.length > 0 && <span className="counter">{list.length}</span>}</h2>
      {st.loading && !st.data ? null : list.length === 0 ? (
        <p className="muted small">No packages published</p>
      ) : (
        <ul className="pkg-sidebar-list">
          {list.slice(0, 3).map((p) => (
            <li key={p.id}><PackageIcon size={14} /> <Link to={packagePath(p)}>{p.name}</Link></li>
          ))}
          {list.length > 3 && <li><Link to={`${ctx.base}/packages`} className="small">+ {list.length - 3} packages</Link></li>}
        </ul>
      )}
    </section>
  );
}

// ---------------------------------------------------------------------------------------------
// Package pages: /{users|orgs}/:owner/packages/:ptype/*

/** Routes `package/NAME`, `NAME/versions`, `NAME/settings`, `NAME/{versionId}`. */
export function PackageRoute() {
  const { owner = "", ptype = "", "*": splat = "" } = useParams();
  const parts = splat.split("/").filter(Boolean).map((s) => decodeURIComponent(s));
  let view: "package" | "versions" | "settings" | "version" = "package";
  let name = "";
  let versionId = 0;
  if (parts[0] === "package" && parts.length > 1) {
    name = parts.slice(1).join("/");
  } else if (parts.length >= 2) {
    const last = parts[parts.length - 1];
    name = parts.slice(0, -1).join("/");
    if (last === "versions") view = "versions";
    else if (last === "settings") view = "settings";
    else if (/^\d+$/.test(last)) { view = "version"; versionId = Number(last); }
    else name = "";
  }
  if (!name || !PACKAGE_TYPES.includes(ptype as never)) return <NotFoundView />;
  return <PackageShell key={`${owner}/${ptype}/${name}`} owner={owner} ptype={ptype} name={name} view={view} versionId={versionId} />;
}

function PackageShell({ owner, ptype, name, view, versionId }: { owner: string; ptype: string; name: string; view: string; versionId: number }) {
  const st = useAsync(() => pkgApi.details(owner, ptype, name), [owner, ptype, name]);
  useTitle(`Package ${name}`);
  return (
    <div className="container pkg-page">
      <Async state={st}>
        {(d) => (
          <>
            <PackageHeader d={d} view={view} />
            {view === "package" && <PackageOverview d={d} />}
            {view === "versions" && <VersionsPage d={d} />}
            {view === "version" && <VersionPage d={d} id={versionId} />}
            {view === "settings" && (d.viewer_permission === "admin" ? <PackageSettings d={d} reload={st.reload} /> : <NotFoundView />)}
          </>
        )}
      </Async>
    </div>
  );
}

function PackageHeader({ d, view }: { d: PackageDetails; view: string }) {
  const p = d.package;
  const tab = (key: string, to: string, label: ReactNode) => (
    <Link to={to} className={"tabnav-tab" + (view === key ? " selected" : "")} data-testid={`pkg-tab-${key}`}>{label}</Link>
  );
  return (
    <header className="pkg-header">
      <div className="pkg-title-row">
        <PackageIcon size={24} />
        <h1 className="h2 pkg-title">
          <Link to={p.owner.type === "Organization" ? `/orgs/${p.owner.login}/packages` : `/${p.owner.login}?tab=packages`} className="muted-link">{p.owner.login}</Link>
          <span className="slash">/</span>
          <Link to={packagePath(p)} data-testid="pkg-name">{p.name}</Link>
        </h1>
        <VisibilityBadge v={d.visibility} />
      </div>
      {d.repository && <p className="muted small"><RepoIcon size={12} /> Linked to <Link to={`/${d.repository}`} data-testid="pkg-linked-repo">{d.repository}</Link></p>}
      <nav className="tabnav" aria-label="Package">
        {tab("package", packagePath(p), "Package")}
        {tab("versions", packagePath(p, "versions"), <>Versions <span className="counter">{p.version_count}</span></>)}
        {d.viewer_permission === "admin" && tab("settings", packagePath(p, "settings"), "Package settings")}
      </nav>
    </header>
  );
}

function InstallBox({ d, version }: { d: PackageDetails; version?: { name: string; tags: string[] } | null }) {
  const p = d.package;
  if (p.package_type === "container" || p.package_type === "docker") {
    const tag = version ? (version.tags[0] ? `:${version.tags[0]}` : `@${version.name}`) : d.latest_version?.tags.includes("latest") ? ":latest" : d.latest_version?.tags[0] ? `:${d.latest_version.tags[0]}` : "";
    return (
      <section className="box pkg-install" data-testid="pkg-install">
        <div className="box-header"><h2 className="h5">Install from the command line</h2></div>
        <div className="box-body">
          <CodeLine text={`docker pull ${imageRef(p)}${tag}`} label="docker pull command" />
          <p className="small muted">Sign in first with <code>docker login {location.host} -u USERNAME</code> and a personal access token as the password (private images).</p>
        </div>
      </section>
    );
  }
  if (p.package_type === "npm") {
    const v = version?.name || d.latest_version?.name;
    const reg = `${location.origin}/npm/`;
    return (
      <section className="box pkg-install" data-testid="pkg-install">
        <div className="box-header"><h2 className="h5">Install from the command line</h2></div>
        <div className="box-body">
          <CodeLine text={`npm install ${npmName(p)}${v ? `@${v}` : ""}`} label="npm install command" />
          <p className="small muted">Point the <code>@{p.owner.login.toLowerCase()}</code> scope at this registry in your project's <code>.npmrc</code>:</p>
          <CodeLine text={`@${p.owner.login.toLowerCase()}:registry=${reg}`} label=".npmrc registry line" />
          <p className="small muted">Install via package.json: <code>"{npmName(p)}": "{v ? `^${v}` : "*"}"</code></p>
        </div>
      </section>
    );
  }
  return null;
}

function PackageOverview({ d }: { d: PackageDetails }) {
  const p = d.package;
  const versions = useAsync(() => pkgApi.versions(p.owner, p.package_type, p.name), [p.id]);
  return (
    <div className="layout-sidebar">
      <div className="layout-main">
        <InstallBox d={d} />
        <section className="box readme pkg-readme" data-testid="pkg-readme">
          <div className="box-header"><h2 className="h5">{d.readme ? "README.md" : "About this package"}</h2></div>
          <div className="box-body">
            {d.readme ? <Markdown source={d.readme} /> : d.description ? <p>{d.description}</p> : (
              <p className="muted"><em>This package has no README. {p.package_type === "npm" ? "Add a README.md to the published tarball." : "Add an org.opencontainers.image.description label to describe it."}</em></p>
            )}
          </div>
        </section>
        <section className="box">
          <div className="box-header"><h2 className="h5">Recent versions</h2></div>
          <Async state={versions}>
            {(vs) => vs.length === 0 ? <Blankslate title="No versions" /> : (
              <ul className="list">
                {vs.slice(0, 5).map((v) => <VersionRowView key={v.id} d={d} v={v} />)}
              </ul>
            )}
          </Async>
          <div className="box-footer"><Link to={packagePath(p, "versions")} data-testid="pkg-all-versions">View all versions</Link></div>
        </section>
      </div>
      <aside className="layout-aside pkg-aside">
        <section className="sidebar-section">
          <h2 className="h5">Details</h2>
          <ul className="pkg-details small">
            <li><Avatar user={p.owner} size={16} /> <Link to={`/${p.owner.login}`}>{p.owner.login}</Link></li>
            <li>{typeLabel(p.package_type)}</li>
            {d.latest_version && <li>Latest version <Link to={packagePath(p, d.latest_version.id)}>{p.package_type === "npm" ? d.latest_version.name : d.latest_version.tags[0] || shortDigest(d.latest_version.name)}</Link></li>}
            <li>{p.version_count} version{p.version_count === 1 ? "" : "s"}</li>
            <li>Published <RelTime iso={p.updated_at} /></li>
            {d.latest_version?.license && <li>License {d.latest_version.license}</li>}
            {d.repository && <li><RepoIcon size={12} /> <Link to={`/${d.repository}`}>{d.repository}</Link></li>}
          </ul>
        </section>
        {d.viewer_permission === "admin" && <Link to={packagePath(p, "settings")} className="btn btn-sm" data-testid="pkg-settings-link">Package settings</Link>}
      </aside>
    </div>
  );
}

function VersionRowView({ d, v, actions }: { d: PackageDetails; v: PackageVersion; actions?: ReactNode }) {
  const p = d.package;
  const tags = v.metadata.container?.tags || [];
  const container = p.package_type === "container" || p.package_type === "docker";
  return (
    <li className="list-row pkg-version-row" data-testid="pkg-version-row">
      <TagIcon className="icon muted" />
      <div className="grow">
        <Link to={packagePath(p, v.id)} className="pkg-version-name" data-testid="pkg-version-link">{container ? (tags[0] || shortDigest(v.name)) : v.name}</Link>
        {container && tags.slice(container ? 1 : 0).map((t) => <span key={t} className="badge pkg-tag">{t}</span>)}
        <div className="muted small">
          {container && <span className="mono">{shortDigest(v.name)} · </span>}
          {v.deleted_at ? <>Deleted <RelTime iso={v.deleted_at} /></> : <>Published <RelTime iso={v.created_at} /></>}
        </div>
      </div>
      {actions}
    </li>
  );
}

function VersionsPage({ d }: { d: PackageDetails }) {
  const p = d.package;
  const admin = d.viewer_permission === "admin";
  const [sp, setSp] = useSearchParams();
  const state = admin && sp.get("state") === "deleted" ? "deleted" : "active";
  const st = useAsync(() => pkgApi.versions(p.owner, p.package_type, p.name, state), [p.id, state]);
  const [err, setErr] = useState<unknown>();
  const [msg, setMsg] = useState("");
  const act = async (v: PackageVersion, restore: boolean) => {
    const label = p.package_type === "npm" ? v.name : (v.metadata.container?.tags[0] || shortDigest(v.name));
    if (!restore && !confirm(`Delete version ${label}? You can restore it for 30 days.`)) return;
    setErr(undefined);
    try {
      if (restore) await pkgApi.restoreVersion(p.owner, p.package_type, p.name, v.id);
      else await pkgApi.deleteVersion(p.owner, p.package_type, p.name, v.id);
      setMsg(restore ? `Version ${label} restored.` : `Version ${label} deleted.`);
      st.reload();
    } catch (e) {
      setErr(e);
    }
  };
  return (
    <section>
      {msg && <Flash kind="success">{msg}</Flash>}
      {err !== undefined && <ErrorBox error={err} />}
      <div className="toolbar">
        <h2 className="h4">{state === "deleted" ? "Deleted versions" : "Active versions"}</h2>
        <span className="spacer" />
        {admin && (
          <div className="segmented" role="group" aria-label="Version state">
            <button type="button" aria-pressed={state === "active"} onClick={() => setSp({})} data-testid="versions-active">Active</button>
            <button type="button" aria-pressed={state === "deleted"} onClick={() => setSp({ state: "deleted" })} data-testid="versions-deleted">Deleted</button>
          </div>
        )}
      </div>
      <Async state={st}>
        {(vs) => vs.length === 0 ? <Blankslate title={state === "deleted" ? "No deleted versions" : "No versions"} /> : (
          <section className="box">
            <ul className="list">
              {vs.map((v) => (
                <VersionRowView key={v.id} d={d} v={v} actions={admin ? (
                  state === "deleted"
                    ? <button type="button" className="btn btn-sm" onClick={() => act(v, true)} data-testid="version-restore">Restore</button>
                    : <button type="button" className="btn btn-sm btn-danger-outline" onClick={() => act(v, false)} data-testid="version-delete">Delete</button>
                ) : undefined} />
              ))}
            </ul>
          </section>
        )}
      </Async>
    </section>
  );
}

function VersionPage({ d, id }: { d: PackageDetails; id: number }) {
  const p = d.package;
  const nav = useNavigate();
  const st = useAsync(() => pkgApi.versionDetails(p.owner.login, p.package_type, p.name, id), [p.id, id]);
  const [err, setErr] = useState<unknown>();
  const container = p.package_type === "container" || p.package_type === "docker";
  return (
    <Async state={st}>
      {(v) => (
        <div className="layout-sidebar">
          <div className="layout-main">
            <h2 className="h3 pkg-version-title" data-testid="version-title">
              {container ? (v.tags[0] || shortDigest(v.name)) : v.name}
              {v.deleted_at && <span className="badge badge-removed">Deleted</span>}
            </h2>
            {!v.deleted_at && <InstallBox d={d} version={v} />}
            <section className="box">
              <div className="box-header"><h2 className="h5">Version details</h2></div>
              <dl className="box-body pkg-dl">
                <dt>Digest</dt><dd className="mono" data-testid="version-digest">{v.digest}</dd>
                {container && <><dt>Tags</dt><dd>{v.tags.length ? v.tags.map((t) => <span key={t} className="badge pkg-tag">{t}</span>) : <span className="muted">untagged</span>}</dd></>}
                <dt>Size</dt><dd>{humanSize(v.size)}</dd>
                <dt>Media type</dt><dd className="mono small">{v.media_type}</dd>
                <dt>Published</dt><dd><RelTime iso={v.created_at} />{v.publisher && <> by <Link to={`/${v.publisher}`}>{v.publisher}</Link></>}</dd>
                {v.license && <><dt>License</dt><dd>{v.license}</dd></>}
                {typeof v.metadata.config_digest === "string" && <><dt>Config</dt><dd className="mono small">{v.metadata.config_digest}</dd></>}
                {typeof v.metadata.integrity === "string" && <><dt>Integrity</dt><dd className="mono small pkg-wrap">{v.metadata.integrity}</dd></>}
              </dl>
            </section>
            {v.readme && <section className="box readme"><div className="box-header"><h2 className="h5">README.md</h2></div><div className="box-body"><Markdown source={v.readme} /></div></section>}
          </div>
          <aside className="layout-aside">
            {err !== undefined && <ErrorBox error={err} />}
            {v.viewer_permission === "admin" && (v.deleted_at ? (
              <button type="button" className="btn btn-sm" data-testid="version-restore" onClick={async () => {
                try { await pkgApi.restoreVersion(p.owner, p.package_type, p.name, v.id); st.reload(); } catch (e) { setErr(e); }
              }}>Restore version</button>
            ) : (
              <button type="button" className="btn btn-sm btn-danger" data-testid="version-delete" onClick={async () => {
                if (!confirm("Delete this version? You can restore it for 30 days.")) return;
                try { await pkgApi.deleteVersion(p.owner, p.package_type, p.name, v.id); nav(packagePath(p, "versions")); } catch (e) { setErr(e); }
              }}>Delete version</button>
            ))}
            <p className="small"><Link to={packagePath(p, "versions")}>All versions</Link></p>
          </aside>
        </div>
      )}
    </Async>
  );
}

// ---------------------------------------------------------------------------------------------
// Package settings

function PackageSettings({ d, reload }: { d: PackageDetails; reload: () => void }) {
  const p = d.package;
  const nav = useNavigate();
  const [repo, setRepo] = useState(d.repository || "");
  const [inherit, setInherit] = useState(d.inherit_access);
  const [vis, setVis] = useState<string>(d.visibility);
  const [err, setErr] = useState<unknown>();
  const [msg, setMsg] = useState("");
  const [confirmName, setConfirmName] = useState("");
  const save = async (b: { visibility?: string; repository?: string; inherit_access?: boolean }, ok: string) => {
    setErr(undefined);
    setMsg("");
    try {
      await pkgApi.update(p.owner.login, p.package_type, p.name, b);
      setMsg(ok);
      reload();
    } catch (e) {
      setErr(e);
    }
  };
  const del = async (e: FormEvent) => {
    e.preventDefault();
    setErr(undefined);
    try {
      await pkgApi.deletePackage(p.owner, p.package_type, p.name);
      nav(p.owner.type === "Organization" ? `/orgs/${p.owner.login}/packages` : `/${p.owner.login}?tab=packages`);
    } catch (e2) {
      setErr(e2);
    }
  };
  return (
    <div className="pkg-settings" data-testid="pkg-settings">
      {msg && <Flash kind="success">{msg}</Flash>}
      {err !== undefined && <ErrorBox error={err} />}
      <section className="box">
        <div className="box-header"><h2 className="h5">Repository source</h2></div>
        <form className="box-body" onSubmit={(e) => { e.preventDefault(); save({ repository: repo.trim() }, repo.trim() ? `Linked to ${repo.trim()}.` : "Repository unlinked."); }}>
          <p className="small muted">Link the package to a repository of <strong>{p.owner.login}</strong>: it is listed on the repository, and can inherit its access.</p>
          <div className="form-row">
            <div className="form-group grow">
              <label htmlFor="pkg-repo">Repository</label>
              <input id="pkg-repo" className="input" placeholder={`${p.owner.login}/repository`} value={repo} onChange={(e) => setRepo(e.target.value)} />
            </div>
          </div>
          <button type="submit" className="btn" data-testid="pkg-link-save">{repo.trim() ? "Link repository" : "Unlink repository"}</button>
        </form>
      </section>

      <section className="box">
        <div className="box-header"><h2 className="h5">Manage access</h2></div>
        <div className="box-body">
          <label className="checkbox">
            <input type="checkbox" checked={inherit} disabled={!d.repository} data-testid="pkg-inherit"
              onChange={(e) => { setInherit(e.target.checked); save({ inherit_access: e.target.checked }, e.target.checked ? "Access is inherited from the repository." : "Access is no longer inherited."); }} />
            {" "}Inherit access from source repository{d.repository ? <> (<code>{d.repository}</code>)</> : <span className="muted"> (link a repository first)</span>}
          </label>
        </div>
        <AccessList d={d} />
      </section>

      <h2 className="h3 c-danger">Danger Zone</h2>
      <section className="box danger-zone">
        <form className="box-body pkg-danger-row" onSubmit={(e) => { e.preventDefault(); if (vis !== d.visibility && confirm(`Change the visibility of ${p.name} to ${vis}?`)) save({ visibility: vis }, `Package visibility changed to ${vis}.`); }}>
          <div className="grow">
            <h3 className="h5">Change package visibility</h3>
            <p className="small muted">This package is currently <strong>{d.visibility}</strong>. Public packages can be read by anyone, without signing in.</p>
          </div>
          <select className="input input-auto" aria-label="Visibility" value={vis} onChange={(e) => setVis(e.target.value)} data-testid="pkg-visibility-select">
            <option value="public">Public</option>
            <option value="private">Private</option>
            {p.owner.type === "Organization" && <option value="internal">Internal</option>}
          </select>
          <button type="submit" className="btn btn-danger-outline" disabled={vis === d.visibility} data-testid="pkg-visibility-save">Change visibility</button>
        </form>
        <form className="box-body pkg-danger-delete" onSubmit={del}>
          <h3 className="h5">Delete this package</h3>
          <p className="small muted">Deleting the package deletes all its versions. You can restore it for 30 days from the {p.owner.type === "Organization" ? "organization's" : "your"} Packages settings.</p>
          <div className="form-group">
            <label htmlFor="pkg-del-confirm">To confirm, type <strong>{p.name}</strong> in the box below</label>
            <input id="pkg-del-confirm" className="input" value={confirmName} autoComplete="off" onChange={(e) => setConfirmName(e.target.value)} />
          </div>
          <button type="submit" className="btn btn-danger" disabled={confirmName !== p.name} data-testid="pkg-delete">I understand the consequences, delete this package</button>
        </form>
      </section>
    </div>
  );
}

function AccessList({ d }: { d: PackageDetails }) {
  const p = d.package;
  const st = useAsync(() => pkgApi.access(p.owner.login, p.package_type, p.name), [p.id]);
  const [login, setLogin] = useState("");
  const [role, setRole] = useState("read");
  const [err, setErr] = useState<unknown>();
  const grant = async (who: string, r: string) => {
    setErr(undefined);
    try {
      await pkgApi.grant(p.owner.login, p.package_type, p.name, who, r);
      setLogin("");
      st.reload();
    } catch (e) {
      setErr(e);
    }
  };
  return (
    <div className="pkg-access" data-testid="pkg-access">
      {err !== undefined && <ErrorBox error={err} />}
      <form className="box-body form-row pkg-access-form" onSubmit={(e) => { e.preventDefault(); if (login.trim()) grant(login.trim(), role); }}>
        <input className="input grow" placeholder="Username" aria-label="Username to invite" value={login} onChange={(e) => setLogin(e.target.value)} data-testid="pkg-access-login" />
        <select className="input input-auto" aria-label="Role" value={role} onChange={(e) => setRole(e.target.value)}>
          <option value="read">Read</option>
          <option value="write">Write</option>
          <option value="admin">Admin</option>
        </select>
        <button type="submit" className="btn" disabled={!login.trim()} data-testid="pkg-access-add">Add</button>
      </form>
      <Async state={st}>
        {(list) => list.length === 0 ? <p className="box-body muted small">No people have been granted access to this package directly.</p> : (
          <ul className="list">
            {list.map((g) => (
              <li key={g.user.login} className="list-row pkg-access-row" data-testid="pkg-access-row">
                <Avatar user={g.user} size={20} />
                <Link to={`/${g.user.login}`} className="grow">{g.user.login}</Link>
                <select className="input input-sm input-auto" aria-label={`Role of ${g.user.login}`} value={g.role} onChange={(e) => grant(g.user.login, e.target.value)}>
                  <option value="read">Read</option>
                  <option value="write">Write</option>
                  <option value="admin">Admin</option>
                </select>
                <button type="button" className="btn btn-sm btn-danger-outline" onClick={async () => {
                  try { await pkgApi.revoke(p.owner.login, p.package_type, p.name, g.user.login); st.reload(); } catch (e) { setErr(e); }
                }}>Remove</button>
              </li>
            ))}
          </ul>
        )}
      </Async>
    </div>
  );
}

// ---------------------------------------------------------------------------------------------
// Settings → Packages (deleted packages, restore)

export function DeletedPackagesSection({ owner }: { owner: User }) {
  const st = useAsync(() => pkgApi.listDeleted(owner.login), [owner.login]);
  const [err, setErr] = useState<unknown>();
  const [msg, setMsg] = useState("");
  return (
    <div data-testid="deleted-packages">
      <h1 className="h2">Packages</h1>
      {msg && <Flash kind="success">{msg}</Flash>}
      {err !== undefined && <ErrorBox error={err} />}
      <section className="box">
        <div className="box-header"><h2 className="h5">Deleted packages</h2></div>
        <p className="box-body small muted">Packages deleted in the last 30 days can be restored, with their versions.</p>
        <Async state={st}>
          {(list) => list.length === 0 ? <Blankslate title="No deleted packages" /> : (
            <ul className="list">
              {list.map((p) => (
                <li key={p.id} className="list-row" data-testid="deleted-package-row">
                  <PackageIcon className="icon muted" />
                  <div className="grow">
                    <strong>{p.name}</strong> <span className="muted small">{typeLabel(p.package_type)}</span>
                    <div className="muted small">Deleted <RelTime iso={p.deleted_at} />{p.name_in_use && " · the name is in use by a newer package"}</div>
                  </div>
                  <button type="button" className="btn btn-sm" disabled={p.name_in_use} data-testid="package-restore" onClick={async () => {
                    setErr(undefined);
                    try {
                      await pkgApi.restorePackage(owner, p.package_type, p.name);
                      setMsg(`Package ${p.name} restored.`);
                      st.reload();
                    } catch (e) {
                      setErr(e);
                    }
                  }}>Restore</button>
                </li>
              ))}
            </ul>
          )}
        </Async>
      </section>
    </div>
  );
}

export function UserPackagesSettings() {
  const { user } = useAuth();
  if (!user) return <Spinner />;
  return <DeletedPackagesSection owner={user} />;
}

export { errorMessage };
