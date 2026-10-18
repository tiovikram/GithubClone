// Deploy keys, autolink references, GitHub Pages, custom properties (repository values and
// organization / enterprise definitions), and the "Use this template" page.
import { useEffect, useState, type FormEvent } from "react";
import { Link, Navigate, useNavigate } from "react-router-dom";
import { useAuth } from "../../auth";
import { Async, Blankslate, RelTime } from "../../components/common";
import { useRepo, type RepoCtx } from "../../components/RepoLayout";
import { useAsync, useTitle } from "../../hooks";
import { ra, type PropertyDef, type PropertyValue, type RepoFull } from "./api";
import { Pill, SectionHead, Status, split, useAction } from "./ui";

// ---------- deploy keys ----------

export function DeployKeys({ ctx }: { ctx: RepoCtx }) {
  useTitle(`Deploy keys · ${ctx.repo.full_name}`);
  const st = useAsync(() => ra.deployKeys(ctx.owner, ctx.name), [ctx.owner, ctx.name]);
  const [adding, setAdding] = useState(false);
  const [title, setTitle] = useState("");
  const [key, setKey] = useState("");
  const [write, setWrite] = useState(false);
  const act = useAction();
  const add = async (e: FormEvent) => {
    e.preventDefault();
    const out = await act.run(() => ra.addDeployKey(ctx.owner, ctx.name, { title: title.trim(), key: key.trim(), read_only: !write }), "Deploy key added.");
    if (out) {
      setAdding(false); setTitle(""); setKey(""); setWrite(false);
      st.reload();
    }
  };
  return (
    <div className="settings ra-keys">
      <SectionHead title="Deploy keys" actions={!adding && <button type="button" className="btn btn-sm btn-primary" onClick={() => setAdding(true)}>Add deploy key</button>}>
        Deploy keys are SSH keys that grant access to a single repository. Read-only keys can clone; keys with write access can also push.
      </SectionHead>
      <Status state={act.state} />
      {adding && (
        <form className="box box-body" onSubmit={add} aria-label="Add deploy key">
          <div className="form-group">
            <label htmlFor="dk-title">Title</label>
            <input id="dk-title" className="input" required value={title} onChange={(e) => setTitle(e.target.value)} />
          </div>
          <div className="form-group">
            <label htmlFor="dk-key">Key</label>
            <textarea id="dk-key" className="input textarea mono" rows={4} required value={key} placeholder="Begins with 'ssh-rsa', 'ecdsa-sha2-nistp256', 'ecdsa-sha2-nistp384', 'ecdsa-sha2-nistp521', 'ssh-ed25519', 'sk-ecdsa-sha2-nistp256@openssh.com', or 'sk-ssh-ed25519@openssh.com'"
              onChange={(e) => setKey(e.target.value)} />
          </div>
          <label className="check-row"><input id="dk-write" type="checkbox" checked={write} onChange={(e) => setWrite(e.target.checked)} />
            <span><strong>Allow write access</strong><span className="ra-check-desc small muted">Can this key be used to push to this repository? Deploy keys always have pull access.</span></span></label>
          <div className="form-actions">
            <button type="submit" className="btn btn-primary" disabled={act.busy}>Add key</button>
            <button type="button" className="btn" onClick={() => setAdding(false)}>Cancel</button>
          </div>
        </form>
      )}
      <Async state={st}>
        {(list) =>
          list.length === 0 ? <div className="box"><Blankslate title="There are no deploy keys for this repository" /></div> : (
            <ul className="box ra-list" data-testid="deploy-keys">
              {list.map((k) => (
                <li key={k.id} className="ra-row" data-testid={`deploy-key-${k.title}`}>
                  <div className="ra-row-main">
                    <strong>{k.title}</strong> <Pill kind={k.read_only ? "" : "warn"}>{k.read_only ? "Read-only" : "Read/write"}</Pill>
                    <div className="mono small muted ra-break">{k.key.slice(0, 60)}…</div>
                    <div className="small muted">Added <RelTime iso={k.created_at} />{k.added_by ? ` by ${k.added_by}` : ""} · {k.last_used ? <>Last used <RelTime iso={k.last_used} /></> : "Never used"}</div>
                  </div>
                  <div className="ra-row-actions">
                    <button type="button" className="btn btn-sm btn-danger" disabled={act.busy} aria-label={`Delete deploy key ${k.title}`}
                      onClick={async () => {
                        if (!confirm(`Delete deploy key “${k.title}”?`)) return;
                        await act.run(() => ra.deleteDeployKey(ctx.owner, ctx.name, k.id), "Deploy key deleted.");
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

// ---------- autolinks ----------

export function Autolinks({ ctx }: { ctx: RepoCtx }) {
  useTitle(`Autolink references · ${ctx.repo.full_name}`);
  const st = useAsync(() => ra.autolinks(ctx.owner, ctx.name), [ctx.owner, ctx.name]);
  const [adding, setAdding] = useState(false);
  const [prefix, setPrefix] = useState("");
  const [tmpl, setTmpl] = useState("");
  const [alnum, setAlnum] = useState(true);
  const act = useAction();
  const add = async (e: FormEvent) => {
    e.preventDefault();
    const out = await act.run(() => ra.addAutolink(ctx.owner, ctx.name, { key_prefix: prefix.trim(), url_template: tmpl.trim(), is_alphanumeric: alnum }), "Autolink reference added.");
    if (out) { setAdding(false); setPrefix(""); setTmpl(""); st.reload(); }
  };
  return (
    <div className="settings ra-autolinks">
      <SectionHead title="Autolink references" actions={!adding && <button type="button" className="btn btn-sm btn-primary" onClick={() => setAdding(true)}>Add autolink reference</button>}>
        Autolinks automatically turn references to external resources, like JIRA issues or Zendesk tickets, into links.
      </SectionHead>
      <Status state={act.state} />
      {adding && (
        <form className="box box-body" onSubmit={add} aria-label="Add autolink reference">
          <fieldset className="form-group">
            <legend>Format</legend>
            <label className="radio-row"><input type="radio" name="al-kind" checked={alnum} onChange={() => setAlnum(true)} /> Alphanumeric <span className="muted small">(matches A-Z, 0-9, and -)</span></label>
            <label className="radio-row"><input type="radio" name="al-kind" checked={!alnum} onChange={() => setAlnum(false)} /> Numeric <span className="muted small">(matches 0-9)</span></label>
          </fieldset>
          <div className="form-group">
            <label htmlFor="al-prefix">Reference prefix</label>
            <input id="al-prefix" className="input" required value={prefix} placeholder="TICKET-" onChange={(e) => setPrefix(e.target.value)} />
            <p className="small muted">This prefix appended by a string will generate a link any time it is found in an issue, pull request, or commit.</p>
          </div>
          <div className="form-group">
            <label htmlFor="al-url">Target URL</label>
            <input id="al-url" className="input" required value={tmpl} placeholder="https://example.com/TICKET?query=<num>" onChange={(e) => setTmpl(e.target.value)} />
            <p className="small muted">The URL must contain <code>&lt;num&gt;</code> for the reference number.</p>
          </div>
          {prefix && tmpl.includes("<num>") && (
            <p className="small">Preview: <code>{prefix}123</code> → <code>{tmpl.replace("<num>", "123")}</code></p>
          )}
          <div className="form-actions">
            <button type="submit" className="btn btn-primary" disabled={act.busy}>Add autolink reference</button>
            <button type="button" className="btn" onClick={() => setAdding(false)}>Cancel</button>
          </div>
        </form>
      )}
      <Async state={st}>
        {(list) =>
          list.length === 0 ? <div className="box"><Blankslate title="No autolink references" /></div> : (
            <ul className="box ra-list" data-testid="autolinks">
              {list.map((a) => (
                <li key={a.id} className="ra-row" data-testid={`autolink-${a.key_prefix}`}>
                  <div className="ra-row-main">
                    <code>{a.key_prefix}&lt;num&gt;</code> → <span className="ra-break">{a.url_template}</span>
                    <div className="small muted">{a.is_alphanumeric ? "Alphanumeric" : "Numeric"}</div>
                  </div>
                  <div className="ra-row-actions">
                    <button type="button" className="btn btn-sm btn-danger" disabled={act.busy} aria-label={`Delete autolink ${a.key_prefix}`}
                      onClick={async () => {
                        await act.run(() => ra.deleteAutolink(ctx.owner, ctx.name, a.id), "Autolink reference deleted.");
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

// ---------- pages ----------

export function PagesSettings({ ctx }: { ctx: RepoCtx }) {
  useTitle(`Pages · ${ctx.repo.full_name}`);
  const site = useAsync(() => ra.pages(ctx.owner, ctx.name), [ctx.owner, ctx.name]);
  const builds = useAsync(() => (site.data ? ra.pagesBuilds(ctx.owner, ctx.name).catch(() => []) : Promise.resolve([])), [site.data]);
  const branches = useAsync(() => ra.branches(ctx.owner, ctx.name), [ctx.owner, ctx.name]);
  const [buildType, setBuildType] = useState("legacy");
  const [branch, setBranch] = useState("");
  const [path, setPath] = useState("/");
  const [cname, setCname] = useState("");
  const [https, setHttps] = useState(false);
  const act = useAction();
  useEffect(() => {
    const s = site.data;
    if (s) {
      setBuildType(s.build_type || "legacy");
      setBranch(s.source?.branch || ctx.repo.default_branch);
      setPath(s.source?.path || "/");
      setCname(s.cname || "");
      setHttps(!!s.https_enforced);
    } else setBranch(ctx.repo.default_branch);
  }, [site.data, ctx.repo.default_branch]);
  const s = site.data;
  const save = async (e: FormEvent) => {
    e.preventDefault();
    const source = { branch, path };
    if (!s) {
      const out = await act.run(() => ra.createPages(ctx.owner, ctx.name, buildType === "legacy" ? { build_type: "legacy", source } : { build_type: "workflow" }), "GitHub Pages source saved.");
      if (out) site.reload();
    } else {
      const out = await act.run(() => ra.updatePages(ctx.owner, ctx.name, { build_type: buildType, ...(buildType === "legacy" ? { source } : {}) }).then(() => true), "GitHub Pages source saved.");
      if (out) site.reload();
    }
  };
  const siteUrl = s?.html_url ? new URL(s.html_url, location.origin) : null;
  const localUrl = siteUrl ? siteUrl.pathname : "";
  return (
    <div className="settings ra-pages">
      <SectionHead title="GitHub Pages">GitHub Pages is designed to host your personal, organization, or project pages from a GitHub repository.</SectionHead>
      <Status state={act.state} />
      <Async state={site}>
        {() => (
          <>
            {s && (
              <div className="box box-body ra-pages-live" data-testid="pages-live">
                <div>
                  {s.status === "built" ? "Your site is live at " : `Your site is ${s.status || "not built yet"}: `}
                  <a href={localUrl || s.html_url} target="_blank" rel="noreferrer" className="ra-strong ra-break" data-testid="pages-url">{s.html_url}</a>
                </div>
                <div className="small muted">Build type: {s.build_type} {s.source && <>· from <code>{s.source.branch}</code> <code>{s.source.path}</code></>}</div>
                <div className="form-actions">
                  <a className="btn btn-sm" href={localUrl || s.html_url} target="_blank" rel="noreferrer">Visit site</a>
                  {s.build_type === "legacy" && (
                    <button type="button" className="btn btn-sm" disabled={act.busy} onClick={async () => { await act.run(() => ra.requestPagesBuild(ctx.owner, ctx.name), "Build requested."); setTimeout(() => { site.reload(); builds.reload(); }, 1000); }}>Request build</button>
                  )}
                  <button type="button" className="btn btn-sm btn-danger" disabled={act.busy} onClick={async () => {
                    if (!confirm("Unpublish this GitHub Pages site?")) return;
                    await act.run(() => ra.deletePages(ctx.owner, ctx.name), "GitHub Pages site unpublished.");
                    site.reload();
                  }}>Unpublish site</button>
                </div>
              </div>
            )}
            <form className="box" onSubmit={save} aria-label="Build and deployment">
              <div className="box-header"><h3 className="h5">Build and deployment</h3></div>
              <div className="box-body">
                <div className="form-group">
                  <label htmlFor="pages-source">Source</label>
                  <select id="pages-source" className="input input-auto" value={buildType} onChange={(e) => setBuildType(e.target.value)}>
                    <option value="legacy">Deploy from a branch</option>
                    <option value="workflow">GitHub Actions</option>
                  </select>
                </div>
                {buildType === "legacy" ? (
                  <div className="form-group">
                    <span className="label-text">Branch</span>
                    <p className="small muted">Your GitHub Pages site is built from this branch and folder.</p>
                    <div className="input-group ra-pages-branch">
                      <select id="pages-branch" className="input input-auto" aria-label="Branch" value={branch} onChange={(e) => setBranch(e.target.value)}>
                        {(branches.data || [{ name: ctx.repo.default_branch }]).map((b) => <option key={b.name} value={b.name}>{b.name}</option>)}
                      </select>
                      <select id="pages-path" className="input input-auto" aria-label="Folder" value={path} onChange={(e) => setPath(e.target.value)}>
                        <option value="/">/ (root)</option>
                        <option value="/docs">/docs</option>
                      </select>
                    </div>
                  </div>
                ) : (
                  <p className="small muted">Use a workflow (e.g. <code>actions/deploy-pages</code>) to build and deploy your site.</p>
                )}
                <button type="submit" className="btn btn-primary" disabled={act.busy}>Save</button>
              </div>
            </form>
            {s && (
              <form className="box" aria-label="Custom domain" onSubmit={async (e) => {
                e.preventDefault();
                const ok = await act.run(() => ra.updatePages(ctx.owner, ctx.name, { cname: cname.trim() || null }).then(() => true), cname.trim() ? "Custom domain saved." : "Custom domain removed.");
                if (ok) site.reload();
              }}>
                <div className="box-header"><h3 className="h5">Custom domain</h3></div>
                <div className="box-body">
                  <div className="input-group">
                    <input id="pages-cname" className="input" aria-label="Custom domain" placeholder="www.example.com" value={cname} onChange={(e) => setCname(e.target.value)} />
                    <button type="submit" className="btn" disabled={act.busy}>Save</button>
                    {s.cname && <button type="button" className="btn" disabled={act.busy} onClick={async () => { setCname(""); await act.run(() => ra.updatePages(ctx.owner, ctx.name, { cname: null }), "Custom domain removed."); site.reload(); }}>Remove</button>}
                  </div>
                  <label className="check-row">
                    <input type="checkbox" checked={https} disabled={act.busy} onChange={async (e) => {
                      const v = e.target.checked;
                      setHttps(v);
                      const ok = await act.run(() => ra.updatePages(ctx.owner, ctx.name, { https_enforced: v }).then(() => true), v ? "HTTPS enforced." : "HTTPS no longer enforced.");
                      if (!ok) setHttps(!v);
                    }} />
                    <span><strong>Enforce HTTPS</strong><span className="ra-check-desc small muted">HTTPS provides a layer of encryption that prevents others from snooping on or tampering with traffic to your site.</span></span>
                  </label>
                </div>
              </form>
            )}
            {s && (
              <div className="box">
                <div className="box-header"><h3 className="h5">Recent builds</h3></div>
                <Async state={builds}>
                  {(list) => list.length === 0 ? <p className="box-body muted">No builds yet.</p> : (
                    <ul className="ra-list" data-testid="pages-builds">
                      {list.map((b, i) => (
                        <li key={i} className="ra-row">
                          <Pill kind={b.status === "built" ? "ok" : b.status === "errored" ? "bad" : "warn"}>{b.status}</Pill>
                          <div className="ra-row-main">
                            <span className="mono small">{b.commit?.slice(0, 7)}</span> {b.error?.message && <span className="c-danger small">{b.error.message}</span>}
                            <div className="small muted">{b.pusher?.login} · <RelTime iso={b.created_at} /> · {b.duration} ms</div>
                          </div>
                        </li>
                      ))}
                    </ul>
                  )}
                </Async>
              </div>
            )}
          </>
        )}
      </Async>
    </div>
  );
}

// ---------- custom properties ----------

function PropValueInput({ def, value, onChange, id }: { def: PropertyDef; value: PropertyValue["value"]; onChange: (v: PropertyValue["value"]) => void; id: string }) {
  switch (def.value_type) {
    case "single_select":
      return (
        <select id={id} className="input input-auto" value={(value as string) || ""} onChange={(e) => onChange(e.target.value || null)}>
          <option value="">{def.required ? "Choose…" : "(unset)"}</option>
          {(def.allowed_values || []).map((v) => <option key={v} value={v}>{v}</option>)}
        </select>
      );
    case "multi_select": {
      const cur = Array.isArray(value) ? value : value ? [value] : [];
      return (
        <div id={id} className="ra-multi">
          {(def.allowed_values || []).map((v) => (
            <label key={v} className="check-row"><input type="checkbox" checked={cur.includes(v)} onChange={(e) => onChange(e.target.checked ? [...cur, v] : cur.filter((x) => x !== v))} /> {v}</label>
          ))}
        </div>
      );
    }
    case "true_false":
      return (
        <select id={id} className="input input-auto" value={(value as string) || ""} onChange={(e) => onChange(e.target.value || null)}>
          <option value="">(unset)</option><option value="true">true</option><option value="false">false</option>
        </select>
      );
    default:
      return <input id={id} className="input" type={def.value_type === "url" ? "url" : "text"} value={(value as string) || ""} onChange={(e) => onChange(e.target.value || null)} />;
  }
}

export function RepoProperties({ ctx }: { ctx: RepoCtx }) {
  useTitle(`Custom properties · ${ctx.repo.full_name}`);
  const isOrg = ctx.repo.owner.type === "Organization";
  const st = useAsync(async () => {
    if (!isOrg) return { defs: [] as PropertyDef[], values: [] as PropertyValue[] };
    const [defs, values] = await Promise.all([ra.propertySchema(`/orgs/${encodeURIComponent(ctx.owner)}`).catch(() => [] as PropertyDef[]), ra.repoProperties(ctx.owner, ctx.name)]);
    return { defs, values };
  }, [ctx.owner, ctx.name]);
  const [vals, setVals] = useState<Record<string, PropertyValue["value"]>>({});
  const act = useAction();
  useEffect(() => {
    if (st.data) setVals(Object.fromEntries(st.data.values.map((v) => [v.property_name, v.value])));
  }, [st.data]);
  return (
    <div className="settings ra-props">
      <SectionHead title="Custom properties">Custom properties allow you to decorate your repository with information such as compliance frameworks, data sensitivity, or project details.</SectionHead>
      {!isOrg ? (
        <div className="box"><Blankslate title="Custom properties are defined by organizations">This repository is owned by a user account, so no custom properties apply.</Blankslate></div>
      ) : (
        <Async state={st}>
          {(d) => d.defs.length === 0 ? (
            <div className="box"><Blankslate title="No custom properties">Organization owners can define custom properties in the <Link to={`/organizations/${ctx.owner}/settings/custom-properties`}>organization settings</Link>.</Blankslate></div>
          ) : (
            <form className="box box-body" data-testid="repo-properties" onSubmit={async (e) => {
              e.preventDefault();
              const props = d.defs.filter((def) => def.values_editable_by === "org_and_repo_actors" || true).map((def) => ({ property_name: def.property_name, value: vals[def.property_name] ?? null }));
              await act.run(() => ra.setRepoProperties(ctx.owner, ctx.name, props), "Custom properties saved.");
              st.reload();
            }}>
              <Status state={act.state} />
              {d.defs.map((def) => (
                <div key={def.property_name} className="form-group">
                  <label htmlFor={`prop-${def.property_name}`}>{def.property_name} {def.required && <Pill kind="info">Required</Pill>}</label>
                  {def.description && <p className="small muted">{def.description}</p>}
                  <PropValueInput id={`prop-${def.property_name}`} def={def} value={vals[def.property_name] ?? null} onChange={(v) => setVals({ ...vals, [def.property_name]: v })} />
                </div>
              ))}
              <button type="submit" className="btn btn-primary" disabled={act.busy}>Save changes</button>
            </form>
          )}
        </Async>
      )}
    </div>
  );
}

/** Custom property definitions of an organization (`/orgs/{org}`) or the enterprise (`/enterprises/{e}`). */
export function PropertyDefinitions({ api, title = "Custom properties", canEdit = true }: { api: string; title?: string; canEdit?: boolean }) {
  useTitle(title);
  const st = useAsync(() => ra.propertySchema(api), [api]);
  const [editing, setEditing] = useState<PropertyDef | "new" | null>(null);
  const act = useAction();
  return (
    <div className="settings ra-propdefs">
      <SectionHead title={title} actions={canEdit && !editing && <button type="button" className="btn btn-sm btn-primary" onClick={() => setEditing("new")}>New property</button>}>
        Custom properties allow you to decorate your repositories with information such as compliance frameworks, data sensitivity, or project details. Use them to target rulesets.
      </SectionHead>
      <Status state={act.state} />
      {editing && (
        <PropertyForm def={editing === "new" ? undefined : editing} onCancel={() => setEditing(null)} onSave={async (name, body) => {
          const ok = await act.run(() => ra.putProperty(api, name, body), `Property ${name} saved.`);
          if (ok) { setEditing(null); st.reload(); }
        }} />
      )}
      <Async state={st}>
        {(list) => list.length === 0 ? <div className="box"><Blankslate title="No custom properties yet" /></div> : (
          <ul className="box ra-list" data-testid="property-defs">
            {list.map((d) => (
              <li key={d.property_name} className="ra-row" data-testid={`propdef-${d.property_name}`}>
                <div className="ra-row-main">
                  <strong>{d.property_name}</strong> <Pill>{d.value_type}</Pill> {d.required && <Pill kind="info">Required</Pill>}
                  {d.source_type && d.source_type !== "organization" && api.startsWith("/orgs/") && <Pill>{d.source_type}</Pill>}
                  <div className="small muted">
                    {d.description || ""}{d.allowed_values?.length ? ` · ${d.allowed_values.join(", ")}` : ""}{d.default_value ? ` · default ${Array.isArray(d.default_value) ? d.default_value.join(", ") : d.default_value}` : ""}
                  </div>
                </div>
                {canEdit && (
                  <div className="ra-row-actions">
                    <button type="button" className="btn btn-sm" onClick={() => setEditing(d)}>Edit</button>
                    <button type="button" className="btn btn-sm btn-danger" disabled={act.busy} aria-label={`Delete property ${d.property_name}`} onClick={async () => {
                      if (!confirm(`Delete property ${d.property_name}? Values set on repositories are removed.`)) return;
                      await act.run(() => ra.deleteProperty(api, d.property_name), `Property ${d.property_name} deleted.`);
                      st.reload();
                    }}>Delete</button>
                  </div>
                )}
              </li>
            ))}
          </ul>
        )}
      </Async>
    </div>
  );
}

function PropertyForm({ def, onSave, onCancel }: { def?: PropertyDef; onSave: (name: string, b: Omit<PropertyDef, "property_name">) => void; onCancel: () => void }) {
  const [name, setName] = useState(def?.property_name || "");
  const [type, setType] = useState(def?.value_type || "string");
  const [desc, setDesc] = useState(def?.description || "");
  const [required, setRequired] = useState(!!def?.required);
  const [allowed, setAllowed] = useState((def?.allowed_values || []).join(", "));
  const [dflt, setDflt] = useState(Array.isArray(def?.default_value) ? def!.default_value.join(", ") : (def?.default_value as string) || "");
  const [editable, setEditable] = useState(def?.values_editable_by === "org_and_repo_actors");
  const select = type.endsWith("_select");
  return (
    <form className="box box-body" aria-label="Custom property" data-testid="property-form" onSubmit={(e) => {
      e.preventDefault();
      onSave(name.trim(), {
        value_type: type,
        required,
        description: desc || null,
        allowed_values: select ? split(allowed) : null,
        default_value: dflt ? (type === "multi_select" ? split(dflt) : dflt) : null,
        values_editable_by: editable ? "org_and_repo_actors" : "org_actors",
      });
    }}>
      <div className="form-row ra-form-row">
        <div className="form-group">
          <label htmlFor="pd-name">Name</label>
          <input id="pd-name" className="input" required value={name} readOnly={!!def} onChange={(e) => setName(e.target.value)} />
        </div>
        <div className="form-group">
          <label htmlFor="pd-type">Type</label>
          <select id="pd-type" className="input input-auto" value={type} onChange={(e) => setType(e.target.value)}>
            <option value="string">Text</option><option value="single_select">Single select</option><option value="multi_select">Multi select</option>
            <option value="true_false">True/false</option><option value="url">URL</option>
          </select>
        </div>
      </div>
      <div className="form-group">
        <label htmlFor="pd-desc">Description</label>
        <input id="pd-desc" className="input" value={desc} onChange={(e) => setDesc(e.target.value)} />
      </div>
      {select && (
        <div className="form-group">
          <label htmlFor="pd-allowed">Options (comma separated)</label>
          <input id="pd-allowed" className="input" required value={allowed} onChange={(e) => setAllowed(e.target.value)} />
        </div>
      )}
      <label className="check-row"><input type="checkbox" checked={required} onChange={(e) => setRequired(e.target.checked)} /> <span><strong>Require this property for all repositories</strong></span></label>
      {required && (
        <div className="form-group">
          <label htmlFor="pd-default">Default value</label>
          <input id="pd-default" className="input" required value={dflt} onChange={(e) => setDflt(e.target.value)} />
        </div>
      )}
      <label className="check-row"><input type="checkbox" checked={editable} onChange={(e) => setEditable(e.target.checked)} /> <span><strong>Allow repository actors to set this property</strong></span></label>
      <div className="form-actions">
        <button type="submit" className="btn btn-primary">Save property</button>
        <button type="button" className="btn" onClick={onCancel}>Cancel</button>
      </div>
    </form>
  );
}

/** Org settings: set property values for repositories (bulk). */
export function OrgRepoPropertyValues({ org }: { org: string }) {
  const st = useAsync(async () => {
    const [defs, rows] = await Promise.all([ra.propertySchema(`/orgs/${encodeURIComponent(org)}`), ra.orgRepoValues(org)]);
    return { defs, rows };
  }, [org]);
  const [repo, setRepo] = useState("");
  const [prop, setProp] = useState("");
  const [value, setValue] = useState<PropertyValue["value"]>(null);
  const act = useAction();
  return (
    <section className="box" data-testid="org-property-values">
      <div className="box-header"><h3 className="h5">Repository values</h3></div>
      <Async state={st}>
        {(d) => (
          <>
            <ul className="ra-list">
              {d.rows.map((r) => (
                <li key={r.repository_id} className="ra-row">
                  <div className="ra-row-main">
                    <Link to={`/${r.repository_full_name}`} className="ra-strong">{r.repository_name}</Link>
                    <div className="small muted">{r.properties.filter((p) => p.value !== null).map((p) => `${p.property_name}: ${Array.isArray(p.value) ? p.value.join(", ") : p.value}`).join(" · ") || "No values"}</div>
                  </div>
                </li>
              ))}
            </ul>
            {d.defs.length > 0 && (
              <form className="box-body ra-inline-form" onSubmit={async (e) => {
                e.preventDefault();
                await act.run(() => ra.setOrgRepoValues(org, [repo], [{ property_name: prop, value }]), "Values updated.");
                st.reload();
              }}>
                <Status state={act.state} />
                <select className="input input-sm input-auto" aria-label="Repository" required value={repo} onChange={(e) => setRepo(e.target.value)}>
                  <option value="">Repository…</option>
                  {d.rows.map((r) => <option key={r.repository_id} value={r.repository_name}>{r.repository_name}</option>)}
                </select>
                <select className="input input-sm input-auto" aria-label="Property" required value={prop} onChange={(e) => { setProp(e.target.value); setValue(null); }}>
                  <option value="">Property…</option>
                  {d.defs.map((x) => <option key={x.property_name} value={x.property_name}>{x.property_name}</option>)}
                </select>
                {prop && <PropValueInput id="org-prop-value" def={d.defs.find((x) => x.property_name === prop)!} value={value} onChange={setValue} />}
                <button type="submit" className="btn btn-sm" disabled={act.busy || !repo || !prop}>Set value</button>
              </form>
            )}
          </>
        )}
      </Async>
    </section>
  );
}

// ---------- use this template ----------

/** `/{o}/{r}/generate`: create a new repository from this template. */
export function GeneratePage() {
  const ctx = useRepo();
  const repo = ctx.repo as RepoFull;
  const { user } = useAuth();
  const nav = useNavigate();
  const orgs = useAsync(() => (user ? ra.myOrgs().catch(() => []) : Promise.resolve([])), [user?.login]);
  const [owner, setOwner] = useState(user?.login || "");
  const [name, setName] = useState("");
  const [desc, setDesc] = useState("");
  const [priv, setPriv] = useState(false);
  const [all, setAll] = useState(false);
  const act = useAction();
  useTitle(`Create a new repository from ${repo.full_name}`);
  if (!user) return <Navigate to={`/login?return_to=${encodeURIComponent(`${ctx.base}/generate`)}`} replace />;
  if (!repo.is_template) return <Navigate to={ctx.base} replace />;
  return (
    <form className="narrow ra-generate" data-testid="generate-form" onSubmit={async (e) => {
      e.preventDefault();
      const out = await act.run(() => ra.generate(ctx.owner, ctx.name, { owner, name: name.trim(), description: desc || undefined, private: priv, include_all_branches: all }));
      if (out) nav(`/${out.full_name}`);
    }}>
      <SectionHead title="Create a new repository">Repository template: <Link to={ctx.base}>{repo.full_name}</Link></SectionHead>
      <Status state={act.state} />
      <div className="form-row ra-form-row">
        <div className="form-group">
          <label htmlFor="gen-owner">Owner</label>
          <select id="gen-owner" className="input input-auto" value={owner} onChange={(e) => setOwner(e.target.value)}>
            <option value={user.login}>{user.login}</option>
            {(orgs.data || []).map((o) => <option key={o.login} value={o.login}>{o.login}</option>)}
          </select>
        </div>
        <div className="form-group">
          <label htmlFor="gen-name">Repository name</label>
          <input id="gen-name" className="input" required value={name} onChange={(e) => setName(e.target.value)} />
        </div>
      </div>
      <div className="form-group">
        <label htmlFor="gen-desc">Description (optional)</label>
        <input id="gen-desc" className="input" value={desc} onChange={(e) => setDesc(e.target.value)} />
      </div>
      <fieldset className="form-group">
        <legend>Visibility</legend>
        <label className="radio-row"><input type="radio" name="gen-vis" checked={!priv} onChange={() => setPriv(false)} /> Public</label>
        <label className="radio-row"><input type="radio" name="gen-vis" checked={priv} onChange={() => setPriv(true)} /> Private</label>
      </fieldset>
      <label className="check-row"><input type="checkbox" checked={all} onChange={(e) => setAll(e.target.checked)} /> <span><strong>Include all branches</strong><span className="ra-check-desc small muted">Copy all branches from {repo.full_name} and not just the default branch.</span></span></label>
      <button type="submit" className="btn btn-primary" disabled={act.busy || !name.trim()}>{act.busy ? "Creating…" : "Create repository"}</button>
    </form>
  );
}
