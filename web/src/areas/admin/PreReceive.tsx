// Pre-receive hooks: environments and hooks (site admin), plus the organization and repository
// settings sections where owners/admins override the enforcement when the hook allows it.
import { useEffect, useState, type FormEvent } from "react";
import { Link, useNavigate, useParams } from "react-router-dom";
import type { OrgCtx } from "../types";
import type { RepoCtx } from "../../components/RepoLayout";
import { Async, Blankslate, ErrorBox, Flash, RelTime } from "../../components/common";
import { useAsync } from "../../hooks";
import { adminApi, type Enforcement, type PreReceiveEnv, type PreReceiveHook, type ScopedPreReceiveHook } from "./api";
import { PageHead, Pill, useAction } from "./ui";

const ENFORCEMENTS: [Enforcement, string][] = [
  ["enabled", "Enabled — reject pushes the script declines"],
  ["testing", "Testing — run the script and show its output, never reject"],
  ["disabled", "Disabled"],
];

// ---------------------------------------------------------------------------------------------
// Environments

export function PreReceiveEnvsSection() {
  const envs = useAsync(() => adminApi.envs(), []);
  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  const act = useAction();
  const anyDownloading = envs.data?.some((e) => e.download.state === "in_progress");
  useEffect(() => {
    if (!anyDownloading) return;
    const t = setTimeout(envs.reload, 2000);
    return () => clearTimeout(t);
  }, [anyDownloading, envs.data, envs.reload]);
  const create = async (e: FormEvent) => {
    e.preventDefault();
    if (await act.run(() => adminApi.createEnv({ name: name.trim(), image_url: url.trim() }), "Environment added; its image is downloading.")) {
      setName("");
      setUrl("");
      envs.reload();
    }
  };
  return (
    <div data-testid="sa-prenvs">
      <PageHead title="Pre-receive hook environments"
        desc="An environment is a root filesystem (a .tar.gz image) that hook scripts run in. The default environment uses the server's own tools." />
      {act.status}
      <Async state={envs}>
        {(list) => (
          <ul className="box list" aria-label="Environments">
            {list.map((env) => <EnvRow key={env.id} env={env} onChange={envs.reload} />)}
          </ul>
        )}
      </Async>
      <form className="box box-body" onSubmit={create} aria-label="Add environment">
        <h3 className="h5">Add environment</h3>
        <div className="form-row">
          <div className="form-group">
            <label htmlFor="sa-env-name">Name</label>
            <input id="sa-env-name" className="input" required value={name} onChange={(e) => setName(e.target.value)} />
          </div>
          <div className="form-group grow">
            <label htmlFor="sa-env-url">Image URL (.tar.gz)</label>
            <input id="sa-env-url" className="input" type="url" required placeholder="https://example.com/env.tar.gz" value={url} onChange={(e) => setUrl(e.target.value)} />
          </div>
        </div>
        <button type="submit" className="btn btn-primary" disabled={act.busy}>Add environment</button>
      </form>
    </div>
  );
}

function EnvRow({ env, onChange }: { env: PreReceiveEnv; onChange: () => void }) {
  const [editing, setEditing] = useState(false);
  const [name, setName] = useState(env.name);
  const [url, setUrl] = useState(env.image_url);
  const act = useAction();
  return (
    <li className="list-row sa-row" data-testid={`sa-env-${env.id}`}>
      <div className="grow sa-row-main">
        <div className="sa-row-title">
          <strong>{env.name}</strong>
          {env.default_environment && <Pill>default</Pill>}
          {!env.default_environment && <Pill kind={env.download.state === "success" ? "success" : env.download.state}>{env.download.state.replace("_", " ")}</Pill>}
        </div>
        <div className="muted small sa-break">
          {env.default_environment ? "Built in" : env.image_url} · {env.hooks_count} hook{env.hooks_count === 1 ? "" : "s"} · added <RelTime iso={env.created_at} />
          {env.download.downloaded_at && <> · downloaded <RelTime iso={env.download.downloaded_at} /></>}
        </div>
        {env.download.message && <div className="small c-danger">{env.download.message}</div>}
        {act.status}
        {editing && (
          <form className="sa-inline-form" onSubmit={async (e) => { e.preventDefault(); if (await act.run(() => adminApi.updateEnv(env.id, { name: name.trim(), image_url: url.trim() }))) { setEditing(false); onChange(); } }}>
            <label htmlFor={`sa-env-n-${env.id}`} className="sr-only">Name</label>
            <input id={`sa-env-n-${env.id}`} className="input input-sm" value={name} onChange={(e) => setName(e.target.value)} />
            <label htmlFor={`sa-env-u-${env.id}`} className="sr-only">Image URL</label>
            <input id={`sa-env-u-${env.id}`} className="input input-sm grow" type="url" value={url} onChange={(e) => setUrl(e.target.value)} />
            <button type="submit" className="btn btn-sm" disabled={act.busy}>Save</button>
          </form>
        )}
      </div>
      {!env.default_environment && (
        <div className="sa-row-actions">
          <button type="button" className="btn btn-sm" disabled={act.busy || env.download.state === "in_progress"}
            onClick={async () => { if (await act.run(() => adminApi.downloadEnv(env.id))) onChange(); }}>Download again</button>
          <button type="button" className="btn btn-sm" onClick={() => setEditing(!editing)}>Edit</button>
          <button type="button" className="btn btn-sm btn-danger-outline" disabled={act.busy || env.hooks_count > 0}
            title={env.hooks_count > 0 ? "Environments with hooks cannot be deleted" : undefined}
            onClick={async () => { if (window.confirm(`Delete environment “${env.name}”?`) && await act.run(() => adminApi.deleteEnv(env.id))) onChange(); }}>Delete</button>
        </div>
      )}
    </li>
  );
}

// ---------------------------------------------------------------------------------------------
// Hooks

export function PreReceiveHooksSection() {
  const splat = useParams()["*"] || "";
  const sub = splat.split("/")[1];
  if (sub === "new") return <HookForm />;
  if (sub) return <HookLoad id={Number(sub)} />;
  return <HookList />;
}

function HookList() {
  const hooks = useAsync(() => adminApi.preHooks(), []);
  const act = useAction();
  return (
    <div data-testid="sa-prehooks">
      <PageHead title="Pre-receive hooks" desc="Scripts that run on every push, before refs are updated; an enabled hook that exits non-zero rejects the push.">
        <Link className="btn btn-primary btn-sm" to="/site-admin/pre-receive-hooks/new" data-testid="sa-prehook-new">Add pre-receive hook</Link>
      </PageHead>
      {act.status}
      <Async state={hooks}>
        {(list) => list.length === 0 ? <Blankslate title="No pre-receive hooks" /> : (
          <ul className="box list" aria-label="Pre-receive hooks">
            {list.map((h) => (
              <li key={h.id} className="list-row sa-row" data-testid={`sa-prehook-${h.name}`}>
                <div className="grow sa-row-main">
                  <div className="sa-row-title"><Link to={`/site-admin/pre-receive-hooks/${h.id}`}><strong>{h.name}</strong></Link><Pill>{h.enforcement}</Pill>{h.allow_downstream_configuration && <Pill>configurable</Pill>}</div>
                  <div className="muted small sa-break"><code>{h.script}</code> in <Link to={`/${h.script_repository.full_name}`}>{h.script_repository.full_name}</Link> · environment {h.environment.name}</div>
                </div>
                <div className="sa-row-actions">
                  <Link className="btn btn-sm" to={`/site-admin/pre-receive-hooks/${h.id}`}>Edit</Link>
                  <button type="button" className="btn btn-sm btn-danger-outline" disabled={act.busy}
                    onClick={async () => { if (window.confirm(`Delete pre-receive hook “${h.name}”?`) && await act.run(() => adminApi.deletePreHook(h.id), "Hook deleted.")) hooks.reload(); }}>Delete</button>
                </div>
              </li>
            ))}
          </ul>
        )}
      </Async>
    </div>
  );
}

function HookLoad({ id }: { id: number }) {
  const hooks = useAsync(() => adminApi.preHooks(), [id]);
  return <Async state={hooks}>{(list) => {
    const h = list.find((x) => x.id === id);
    return h ? <HookForm hook={h} /> : <Blankslate title="Hook not found" />;
  }}</Async>;
}

function HookForm({ hook }: { hook?: PreReceiveHook }) {
  const nav = useNavigate();
  const envs = useAsync(() => adminApi.envs(), []);
  const [name, setName] = useState(hook?.name || "");
  const [repo, setRepo] = useState(hook?.script_repository.full_name || "");
  const [script, setScript] = useState(hook?.script || "");
  const [envId, setEnvId] = useState<number | undefined>(hook?.environment.id);
  const [enforcement, setEnforcement] = useState<Enforcement>(hook?.enforcement || "enabled");
  const [downstream, setDownstream] = useState(hook?.allow_downstream_configuration ?? false);
  const act = useAction();
  useEffect(() => {
    if (envId === undefined && envs.data?.length) setEnvId((envs.data.find((e) => e.default_environment) || envs.data[0]).id);
  }, [envs.data, envId]);
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const body = {
      name: name.trim(), script: script.trim(), script_repository: { full_name: repo.trim() },
      environment: { id: envId! }, enforcement, allow_downstream_configuration: downstream,
    };
    if (await act.run(() => (hook ? adminApi.updatePreHook(hook.id, body) : adminApi.createPreHook(body)))) nav("/site-admin/pre-receive-hooks");
  };
  return (
    <div data-testid="sa-prehook-form">
      <div className="sa-crumbs small"><Link to="/site-admin/pre-receive-hooks">Pre-receive hooks</Link> / {hook ? hook.name : "New hook"}</div>
      <PageHead title={hook ? "Edit pre-receive hook" : "Add pre-receive hook"} />
      <form className="box box-body" onSubmit={submit} aria-label="Pre-receive hook">
        {act.status}
        <div className="form-group">
          <label htmlFor="sa-ph-name">Hook name</label>
          <input id="sa-ph-name" className="input" required value={name} onChange={(e) => setName(e.target.value)} />
        </div>
        <div className="form-row">
          <div className="form-group grow">
            <label htmlFor="sa-ph-repo">Script repository</label>
            <input id="sa-ph-repo" className="input" required placeholder="owner/repository" value={repo} onChange={(e) => setRepo(e.target.value)} />
          </div>
          <div className="form-group grow">
            <label htmlFor="sa-ph-script">Script path</label>
            <input id="sa-ph-script" className="input" required placeholder="hooks/check.sh" value={script} onChange={(e) => setScript(e.target.value)} />
          </div>
        </div>
        <p className="muted small">The script is read from the repository's default branch. It gets the pushed refs on stdin (<code>old new ref</code> per line) and runs with a 5 second limit.</p>
        <div className="form-group">
          <label htmlFor="sa-ph-env">Environment</label>
          <select id="sa-ph-env" className="input input-auto" value={envId ?? ""} onChange={(e) => setEnvId(Number(e.target.value))}>
            {(envs.data || []).map((env) => <option key={env.id} value={env.id}>{env.name}{env.default_environment ? " (default)" : ""}</option>)}
          </select>
        </div>
        <fieldset className="form-group">
          <legend>Enforcement</legend>
          {ENFORCEMENTS.map(([v, label]) => (
            <label key={v} className="radio-row"><input type="radio" name="sa-ph-enf" value={v} checked={enforcement === v} onChange={() => setEnforcement(v)} /> {label}</label>
          ))}
        </fieldset>
        <label className="check-row"><input id="sa-ph-downstream" type="checkbox" checked={downstream} onChange={(e) => setDownstream(e.target.checked)} /> Allow organization owners and repository admins to change the enforcement</label>
        <div className="form-actions">
          <button type="submit" className="btn btn-primary" disabled={act.busy || envId === undefined} data-testid="sa-prehook-save">{hook ? "Update hook" : "Add pre-receive hook"}</button>
        </div>
      </form>
    </div>
  );
}

// ---------------------------------------------------------------------------------------------
// Organization and repository settings ("Hooks")

function ScopedList({ load, update, reset, scope, testid, downstreamLabel }: {
  load: () => Promise<ScopedPreReceiveHook[]>;
  update: (id: number, enforcement: Enforcement, downstream?: boolean) => Promise<unknown>;
  reset: (id: number) => Promise<unknown>;
  scope: "org" | "repo";
  testid: string;
  downstreamLabel?: string;
}) {
  const hooks = useAsync(load, []);
  return (
    <div data-testid={testid}>
      <PageHead title="Pre-receive hooks"
        desc={scope === "org"
          ? "Hooks configured by the site administrators. When a hook allows it, you can change its enforcement for this organization's repositories."
          : "Hooks configured by the site administrators or the organization. When a hook allows it, you can change its enforcement for this repository."} />
      <Async state={hooks}>
        {(list) => list.length === 0 ? <Blankslate title="No pre-receive hooks">There are no pre-receive hooks on this instance.</Blankslate> : (
          <ul className="box list" aria-label="Pre-receive hooks">
            {list.map((h) => <ScopedRow key={h.id} h={h} scope={scope} update={update} reset={reset} onChange={hooks.reload} downstreamLabel={downstreamLabel} />)}
          </ul>
        )}
      </Async>
    </div>
  );
}

function ScopedRow({ h, scope, update, reset, onChange, downstreamLabel }: {
  h: ScopedPreReceiveHook; scope: "org" | "repo"; onChange: () => void; downstreamLabel?: string;
  update: (id: number, enforcement: Enforcement, downstream?: boolean) => Promise<unknown>;
  reset: (id: number) => Promise<unknown>;
}) {
  const own = scope === "org" ? h.configuration_url.includes("/orgs/") : h.configuration_url.includes("/repos/");
  const level = h.configuration_url.includes("/repos/") ? "repository" : h.configuration_url.includes("/orgs/") ? "organization" : "enterprise";
  const [enf, setEnf] = useState<Enforcement>(h.enforcement);
  const [down, setDown] = useState(!!h.allow_downstream_configuration);
  const act = useAction();
  useEffect(() => { setEnf(h.enforcement); setDown(!!h.allow_downstream_configuration); }, [h]);
  const dirty = enf !== h.enforcement || (scope === "org" && down !== !!h.allow_downstream_configuration);
  return (
    <li className="list-row sa-row" data-testid={`hooks-row-${h.name}`}>
      <form className="grow sa-row-main" onSubmit={async (e) => { e.preventDefault(); if (await act.run(() => update(h.id, enf, scope === "org" ? down : undefined), "Enforcement updated.")) onChange(); }}>
        <div className="sa-row-title"><strong>{h.name}</strong><Pill>{h.enforcement}</Pill><span className="muted small">set at the {level} level</span></div>
        {act.status}
        <div className="sa-inline-form">
          <label htmlFor={`hk-${h.id}`} className="sr-only">Enforcement for {h.name}</label>
          <select id={`hk-${h.id}`} className="input input-sm input-auto" value={enf} onChange={(e) => setEnf(e.target.value as Enforcement)}>
            {ENFORCEMENTS.map(([v]) => <option key={v} value={v}>{v}</option>)}
          </select>
          {scope === "org" && (
            <label className="check-row small"><input type="checkbox" checked={down} onChange={(e) => setDown(e.target.checked)} /> {downstreamLabel}</label>
          )}
          <button type="submit" className="btn btn-sm" disabled={act.busy || !dirty}>Save</button>
          {own && <button type="button" className="btn btn-sm" disabled={act.busy}
            onClick={async () => { if (await act.run(() => reset(h.id), "Using the inherited enforcement.")) onChange(); }}>Use inherited setting</button>}
        </div>
      </form>
    </li>
  );
}

export function OrgHooksSection({ ctx }: { ctx: OrgCtx }) {
  if (!ctx.isOwner) return <Flash kind="warn">Only organization owners can configure pre-receive hooks.</Flash>;
  return (
    <ScopedList testid="org-prehooks" scope="org"
      load={() => adminApi.orgPreHooks(ctx.login)}
      update={(id, enforcement, allow) => adminApi.updateOrgPreHook(ctx.login, id, { enforcement, allow_downstream_configuration: allow })}
      reset={(id) => adminApi.resetOrgPreHook(ctx.login, id)}
      downstreamLabel="Allow repository admins to change it" />
  );
}

export function RepoHooksSection({ ctx }: { ctx: RepoCtx }) {
  if (!ctx.isAdmin) return <ErrorBox error={new Error("Only repository admins can configure pre-receive hooks.")} />;
  return (
    <ScopedList testid="repo-prehooks" scope="repo"
      load={() => adminApi.repoPreHooks(ctx.owner, ctx.name)}
      update={(id, enforcement) => adminApi.updateRepoPreHook(ctx.owner, ctx.name, id, enforcement)}
      reset={(id) => adminApi.resetRepoPreHook(ctx.owner, ctx.name, id)} />
  );
}
