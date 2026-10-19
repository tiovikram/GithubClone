// Organization settings → Private registries: list, add, edit, delete. Secrets are sealed in
// the browser against the organization's registry public key (libsodium sealed box), like
// GitHub; the server never returns them.
import { useSettingsSplat } from "./splat";
import { useState, type FormEvent } from "react";
import { Link, useNavigate } from "react-router-dom";
import { Async, Blankslate, ErrorBox, Flash, NotFoundView, RelTime } from "../../components/common";
import { useAsync, useTitle } from "../../hooks";
import { appsApi, registriesApi, type PrivateRegistry } from "./api";
import { PackageIcon } from "./icons";
import { sealBase64 } from "./sealed";

export const REGISTRY_TYPES: [string, string][] = [
  ["npm_registry", "npm registry"], ["docker_registry", "Docker registry"], ["maven_repository", "Maven repository"],
  ["nuget_feed", "NuGet feed"], ["rubygems_server", "RubyGems server"], ["python_index", "Python index"],
  ["cargo_registry", "Cargo registry"], ["composer_repository", "Composer repository"], ["goproxy_server", "Go proxy server"],
  ["git_source", "Git source"], ["helm_registry", "Helm registry"], ["hex_organization", "Hex organization"],
  ["hex_repository", "Hex repository"], ["pub_repository", "Pub repository"], ["terraform_registry", "Terraform registry"],
];
const typeName = (t: string) => REGISTRY_TYPES.find(([k]) => k === t)?.[1] || t;

const AUTH: [string, string, string[]][] = [
  ["token", "Token", []],
  ["username_password", "Username and password", []],
  ["oidc_azure", "OIDC: Azure", ["tenant_id", "client_id"]],
  ["oidc_aws", "OIDC: AWS CodeArtifact", ["aws_region", "account_id", "role_name", "domain", "domain_owner"]],
  ["oidc_jfrog", "OIDC: JFrog", ["jfrog_oidc_provider_name", "audience", "identity_mapping_name"]],
  ["oidc_cloudsmith", "OIDC: Cloudsmith", ["namespace", "service_slug", "api_host"]],
  ["oidc_gcp", "OIDC: Google Cloud", ["workload_identity_provider", "service_account"]],
];

/** Section element: `private-registries`, `private-registries/new`, `private-registries/{name}`. */
export function PrivateRegistriesSection({ org, isOwner }: { org: string; isOwner: boolean }) {
  const splat = useSettingsSplat();
  const parts = splat.split("/").filter(Boolean);
  if (!isOwner) return <NotFoundView />;
  if (parts.length === 1) return <RegistryList org={org} />;
  if (parts[1] === "new") return <RegistryForm org={org} />;
  return <RegistryEdit org={org} name={decodeURIComponent(parts[1])} />;
}

const root = (org: string) => `/organizations/${org}/settings/private-registries`;

function RegistryList({ org }: { org: string }) {
  useTitle("Private registries");
  const st = useAsync(() => registriesApi.list(org), [org]);
  return (
    <div data-testid="private-registries">
      <div className="toolbar">
        <h1 className="h2">Private registries</h1>
        <span className="spacer" />
        <Link to={`${root(org)}/new`} className="btn btn-primary btn-sm" data-testid="new-registry">New private registry</Link>
      </div>
      <p className="muted small">Give Dependabot and code scanning access to package registries that need credentials. Secrets are encrypted in your browser before they are sent.</p>
      <Async state={st}>
        {(r) => r.configurations.length === 0 ? <Blankslate title="No private registries" icon={<PackageIcon size={24} />} /> : (
          <section className="box">
            <ul className="list">
              {r.configurations.map((c) => (
                <li key={c.name} className="list-row" data-testid="registry-row">
                  <PackageIcon className="icon muted" />
                  <div className="grow">
                    <Link to={`${root(org)}/${encodeURIComponent(c.name)}`}><strong className="mono">{c.name}</strong></Link>
                    <div className="small muted">{typeName(c.registry_type)} · {c.url} · {c.visibility === "all" ? "All repositories" : c.visibility === "private" ? "Private and internal repositories" : "Selected repositories"} · Updated <RelTime iso={c.updated_at} /></div>
                  </div>
                </li>
              ))}
            </ul>
          </section>
        )}
      </Async>
    </div>
  );
}

function RegistryEdit({ org, name }: { org: string; name: string }) {
  const st = useAsync(() => registriesApi.get(org, name), [org, name]);
  return <Async state={st}>{(r) => <RegistryForm org={org} existing={r} />}</Async>;
}

function RegistryForm({ org, existing }: { org: string; existing?: PrivateRegistry }) {
  useTitle(existing ? existing.name : "New private registry");
  const nav = useNavigate();
  const repos = useAsync(() => appsApi.accountRepos({ login: org, type: "Organization" } as never), [org]);
  const [f, setF] = useState({
    registry_type: existing?.registry_type || "npm_registry",
    url: existing?.url || "",
    auth_type: existing?.auth_type || "token",
    username: (existing?.username as string) || "",
    secret: "",
    replaces_base: !!existing?.replaces_base,
    visibility: existing?.visibility || "all",
  });
  const [oidc, setOidc] = useState<Record<string, string>>(() =>
    Object.fromEntries(AUTH.flatMap(([, , fields]) => fields).filter((k) => typeof existing?.[k] === "string").map((k) => [k, existing![k] as string])));
  const [selected, setSelected] = useState<number[]>(existing?.selected_repository_ids || []);
  const [err, setErr] = useState<unknown>();
  const [msg, setMsg] = useState("");
  const [busy, setBusy] = useState(false);
  const set = (k: keyof typeof f) => (e: { target: { value: string; checked?: boolean; type?: string } }) =>
    setF({ ...f, [k]: e.target.type === "checkbox" ? !!e.target.checked : e.target.value });
  const oidcFields = AUTH.find(([k]) => k === f.auth_type)?.[2] || [];
  const needsSecret = f.auth_type === "token" || f.auth_type === "username_password";
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setErr(undefined);
    setMsg("");
    try {
      const body: Record<string, unknown> = {
        registry_type: f.registry_type,
        url: f.url.trim(),
        auth_type: f.auth_type,
        replaces_base: f.replaces_base,
        visibility: f.visibility,
      };
      if (f.auth_type === "username_password") body.username = f.username.trim();
      else if (existing?.username) body.username = null;
      if (f.visibility === "selected") body.selected_repository_ids = selected;
      for (const k of oidcFields) if (oidc[k]?.trim()) body[k] = oidc[k].trim();
      if (needsSecret && f.secret) {
        const key = await registriesApi.publicKey(org);
        body.encrypted_value = sealBase64(key.key, f.secret);
        body.key_id = key.key_id;
      }
      if (existing) {
        await registriesApi.update(org, existing.name, body);
        setMsg("Private registry updated.");
        setF({ ...f, secret: "" });
      } else {
        await registriesApi.create(org, body);
        nav(root(org));
      }
    } catch (e2) {
      setErr(e2);
    } finally {
      setBusy(false);
    }
  };
  return (
    <form onSubmit={submit} className="registry-form" data-testid="registry-form">
      <div className="toolbar">
        <h1 className="h2 grow">{existing ? <span className="mono">{existing.name}</span> : "New private registry"}</h1>
        <Link to={root(org)} className="btn btn-sm">All registries</Link>
      </div>
      {msg && <Flash kind="success">{msg}</Flash>}
      {err !== undefined && <ErrorBox error={err} />}
      <div className="form-group">
        <label htmlFor="reg-type">Registry type</label>
        <select id="reg-type" className="input" value={f.registry_type} onChange={set("registry_type")}>
          {REGISTRY_TYPES.map(([k, l]) => <option key={k} value={k}>{l}</option>)}
        </select>
      </div>
      <div className="form-group"><label htmlFor="reg-url">URL</label><input id="reg-url" className="input" type="url" required value={f.url} onChange={set("url")} placeholder="https://registry.example.com" /></div>
      <div className="form-group">
        <label htmlFor="reg-auth">Authentication</label>
        <select id="reg-auth" className="input" value={f.auth_type} onChange={set("auth_type")}>
          {AUTH.map(([k, l]) => <option key={k} value={k}>{l}</option>)}
        </select>
      </div>
      {f.auth_type === "username_password" && (
        <div className="form-group"><label htmlFor="reg-user">Username</label><input id="reg-user" className="input" required value={f.username} onChange={set("username")} /></div>
      )}
      {needsSecret && (
        <div className="form-group">
          <label htmlFor="reg-secret">{f.auth_type === "token" ? "Token" : "Password"}</label>
          <input id="reg-secret" className="input" type="password" autoComplete="new-password" required={!existing} value={f.secret} onChange={set("secret")}
            placeholder={existing ? "Leave empty to keep the current secret" : ""} />
        </div>
      )}
      {oidcFields.map((k) => (
        <div key={k} className="form-group">
          <label htmlFor={`reg-${k}`}>{k.replace(/_/g, " ")}</label>
          <input id={`reg-${k}`} className="input" value={oidc[k] || ""} onChange={(e) => setOidc({ ...oidc, [k]: e.target.value })} />
        </div>
      ))}
      <label className="checkbox form-group"><input type="checkbox" checked={f.replaces_base} onChange={set("replaces_base")} /> Replace the default public registry for this ecosystem</label>
      <fieldset className="form-group">
        <legend>Repository access</legend>
        {(["all", "private", "selected"] as const).map((v) => (
          <label key={v} className="radio"><input type="radio" name="reg-vis" value={v} checked={f.visibility === v} onChange={set("visibility")} />{" "}
            {v === "all" ? "All repositories" : v === "private" ? "Private and internal repositories" : "Selected repositories"}</label>
        ))}
        {f.visibility === "selected" && (
          <Async state={repos}>
            {(rs) => (
              <ul className="list repo-picker-list">
                {rs.map((r) => (
                  <li key={r.id} className="list-row"><label className="checkbox grow">
                    <input type="checkbox" checked={selected.includes(r.id)} onChange={(e) => setSelected(e.target.checked ? [...selected, r.id] : selected.filter((x) => x !== r.id))} /> {r.full_name}
                  </label></li>
                ))}
                {rs.length === 0 && <li className="list-row muted small">This organization has no repositories.</li>}
              </ul>
            )}
          </Async>
        )}
      </fieldset>
      <div className="form-actions">
        <button type="submit" className="btn btn-primary" disabled={busy} data-testid="save-registry">{busy ? "Saving…" : existing ? "Update registry" : "Add registry"}</button>
        {existing && (
          <button type="button" className="btn btn-danger-outline" data-testid="delete-registry" onClick={async () => {
            if (!confirm(`Delete ${existing.name}?`)) return;
            try { await registriesApi.remove(org, existing.name); nav(root(org)); } catch (e2) { setErr(e2); }
          }}>Delete</button>
        )}
      </div>
    </form>
  );
}
