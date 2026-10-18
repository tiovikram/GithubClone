// Rulesets at repository, organization and enterprise level, with rule insights (rule suites)
// and push bypass requests. One component tree, parameterised by `RulesLevel`.
//   <root>                      rulesets list          (root = /{o}/{r}/settings/rules, …)
//   <root>/new?target=branch    new ruleset
//   <root>/<id>                 edit ruleset (+ history)
//   <root>/insights[/<suite>]   rule insights
//   <root>/bypass-requests      push bypass requests
import { useEffect, useMemo, useState, type FormEvent, type ReactNode } from "react";
import { Link, useLocation, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { Async, Blankslate, NotFoundView, RelTime, Spinner } from "../../components/common";
import { useAsync, useTitle } from "../../hooks";
import { shortSha } from "../../util";
import {
  ra, type BypassActor, type BypassRequest, type CustomRole, type PropertyDef, type Rule, type RuleSuite, type Ruleset,
  type RulesetConditions, type RulesetInput,
} from "./api";
import { ACTOR_LABEL, MODES, ROLE_IDS, RULES, defaultParams, ruleLabel, type ParamKind, type RuleMeta } from "./rules-meta";
import { ListInput, Pill, SectionHead, Status, split, useAction } from "./ui";

export interface RulesLevel {
  kind: "repo" | "org" | "enterprise";
  /** API base: /repos/{o}/{r}, /orgs/{org} or /enterprises/{e}. */
  api: string;
  /** UI path of the rules section. */
  root: string;
  /** Owner org (org-owned repository, or the organization itself): teams, roles, installations. */
  org?: string;
  owner?: string;
  repo?: string;
  repoId?: number;
}

/** Splat after the section path ("rules/12" → "12"). */
function useRest(section: string) {
  const { "*": splat = "" } = useParams();
  const p = splat.replace(/^\/+|\/+$/g, "");
  return p === section ? "" : p.startsWith(`${section}/`) ? p.slice(section.length + 1) : p;
}

export function RulesSection({ level, section = "rules", tabs = true }: { level: RulesLevel; section?: string; tabs?: boolean }) {
  const rest = useRest(section);
  let body: ReactNode;
  let tab = "rulesets";
  if (rest === "") body = <RulesetList level={level} />;
  else if (rest === "new") body = <RulesetEditor level={level} />;
  else if (rest === "insights") { tab = "insights"; body = <Insights level={level} />; }
  else if (rest.startsWith("insights/")) { tab = "insights"; body = <SuiteDetail level={level} id={Number(rest.slice(9))} />; }
  else if (rest === "bypass-requests") { tab = "bypass"; body = <BypassList level={level} />; }
  else if (/^\d+$/.test(rest)) body = <RulesetEditor level={level} id={Number(rest)} />;
  else if (/^\d+\/history$/.test(rest)) body = <RulesetHistory level={level} id={Number(rest.split("/")[0])} />;
  else body = <NotFoundView />;
  return (
    <div className="settings ra-rules">
      {tabs && (
        <nav className="tabnav ra-tabnav" aria-label="Rules">
          <Link className={`tabnav-tab${tab === "rulesets" ? " selected" : ""}`} to={level.root}>Rulesets</Link>
          {level.kind !== "enterprise" && <Link className={`tabnav-tab${tab === "insights" ? " selected" : ""}`} to={`${level.root}/insights`}>Insights</Link>}
          <Link className={`tabnav-tab${tab === "bypass" ? " selected" : ""}`} to={`${level.root}/bypass-requests`}>Bypass requests</Link>
        </nav>
      )}
      {body}
    </div>
  );
}

export const enforcementPill = (e: string) =>
  e === "active" ? <Pill kind="ok">Active</Pill> : e === "evaluate" ? <Pill kind="warn">Evaluate</Pill> : <Pill>Disabled</Pill>;

function RulesetList({ level, target }: { level: RulesLevel; target?: string }) {
  useTitle("Rulesets");
  const st = useAsync(() => ra.rulesets(level.api), [level.api]);
  const act = useAction();
  const [importing, setImporting] = useState(false);
  const nav = useNavigate();
  const list = (st.data || []).filter((r) => !target || r.target === target);
  return (
    <>
      <SectionHead title={target === "tag" ? "Tag rulesets" : "Rulesets"}
        actions={
          <>
            <button type="button" className="btn btn-sm" onClick={() => setImporting(!importing)}>Import a ruleset</button>
            {(!target || target === "branch") && <Link className="btn btn-sm btn-primary" to={`${level.root}/new?target=branch`}>New branch ruleset</Link>}
            {(!target || target === "tag") && <Link className={`btn btn-sm${target === "tag" ? " btn-primary" : ""}`} to={`${level.root}/new?target=tag`}>New tag ruleset</Link>}
            {!target && <Link className="btn btn-sm" to={`${level.root}/new?target=push`}>New push ruleset</Link>}
          </>
        }>
        Rulesets define whether collaborators can delete or force push and set requirements for any pushes, such as passing status checks or a linear commit history.
      </SectionHead>
      {importing && (
        <ImportBox onImport={async (r) => {
          const out = await act.run(() => ra.createRuleset(level.api, r), "Ruleset imported.");
          if (out) nav(`${level.root}/${out.id}`);
        }} />
      )}
      <Status state={act.state} />
      <Async state={st}>
        {() =>
          list.length === 0 ? (
            <div className="box"><Blankslate title="You haven't created any rulesets">Define whether collaborators can delete or force push and set requirements for any pushes.</Blankslate></div>
          ) : (
            <ul className="box ra-list" data-testid="rulesets">
              {list.map((r) => {
                const own = isOwn(level, r);
                return (
                  <li key={r.id} className="ra-row" data-testid={`ruleset-${r.name}`}>
                    <div className="ra-row-main">
                      {own ? <Link className="ra-strong" to={`${level.root}/${r.id}`}>{r.name}</Link> : <span className="ra-strong">{r.name}</span>}{" "}
                      {enforcementPill(r.enforcement)} <Pill>{r.target || "branch"}</Pill>
                      <div className="small muted">
                        {own ? "" : `Managed by ${r.source_type === "Organization" ? "organization" : "enterprise"} ${r.source} · `}
                        {r.rules ? `${r.rules.length} rules · ` : ""}{r.updated_at && <>Updated <RelTime iso={r.updated_at} /></>}
                      </div>
                    </div>
                    {own ? (
                      <div className="ra-row-actions">
                        <Link className="btn btn-sm" to={`${level.root}/${r.id}`}>Edit</Link>
                        <button type="button" className="btn btn-sm btn-danger" disabled={act.busy} aria-label={`Delete ruleset ${r.name}`}
                          onClick={async () => {
                            if (!confirm(`Delete ruleset “${r.name}”? This cannot be undone.`)) return;
                            await act.run(() => ra.deleteRuleset(level.api, r.id), `Ruleset ${r.name} deleted.`);
                            st.reload();
                          }}>Delete</button>
                      </div>
                    ) : (
                      r.source_type === "Organization" && <Link className="btn btn-sm" to={`/organizations/${r.source}/settings/rules/${r.id}`}>View</Link>
                    )}
                  </li>
                );
              })}
            </ul>
          )
        }
      </Async>
    </>
  );
}

const isOwn = (level: RulesLevel, r: Ruleset) =>
  !r.source_type || (level.kind === "repo" ? r.source_type === "Repository" : level.kind === "org" ? r.source_type === "Organization" : r.source_type === "Enterprise");

/** Tags section: the tag-targeting rulesets of the repository. */
export function TagRulesets({ level }: { level: RulesLevel }) {
  return (
    <div className="settings ra-rules">
      <RulesetList level={level} target="tag" />
      <p className="small muted">Tag protection rules have been replaced by tag rulesets. Rules here apply to tags matching the ruleset's target patterns.</p>
    </div>
  );
}

function ImportBox({ onImport }: { onImport: (r: RulesetInput) => void }) {
  const [text, setText] = useState("");
  const [err, setErr] = useState("");
  return (
    <div className="box box-body">
      <label htmlFor="ruleset-import" className="label-text">Ruleset JSON (as exported)</label>
      <textarea id="ruleset-import" className="input textarea mono" rows={6} value={text} onChange={(e) => setText(e.target.value)} />
      {err && <div className="flash flash-error">{err}</div>}
      <button type="button" className="btn btn-sm" onClick={() => {
        try {
          const j = JSON.parse(text);
          setErr("");
          onImport({ name: j.name, target: j.target || "branch", enforcement: j.enforcement || "disabled", bypass_actors: j.bypass_actors || [], conditions: j.conditions || null, rules: j.rules || [] });
        } catch (e) {
          setErr(`Invalid JSON: ${(e as Error).message}`);
        }
      }}>Import</button>
    </div>
  );
}

// ---------- editor ----------

interface RefTarget { kind: "include" | "exclude"; value: string }

const refPrefix = (t: string) => (t === "tag" ? "refs/tags/" : "refs/heads/");

function toRefTargets(c: RulesetConditions | null | undefined): RefTarget[] {
  const out: RefTarget[] = [];
  for (const v of c?.ref_name?.include || []) out.push({ kind: "include", value: v });
  for (const v of c?.ref_name?.exclude || []) out.push({ kind: "exclude", value: v });
  return out;
}

function refLabel(v: string) {
  if (v === "~DEFAULT_BRANCH") return "Default branch";
  if (v === "~ALL") return "All branches / tags";
  return v.replace(/^refs\/(heads|tags)\//, "");
}

type RepoTargeting = "all" | "name" | "property" | "ids";

function RulesetEditor({ level, id }: { level: RulesLevel; id?: number }) {
  const [sp] = useSearchParams();
  const nav = useNavigate();
  const existing = useAsync(() => (id ? ra.ruleset(level.api, id) : Promise.resolve(null)), [level.api, id]);
  const [name, setName] = useState("");
  const [target, setTarget] = useState(sp.get("target") || "branch");
  const [enforcement, setEnforcement] = useState("active");
  const [bypass, setBypass] = useState<BypassActor[]>([]);
  const [refs, setRefs] = useState<RefTarget[]>([]);
  const [rules, setRules] = useState<Rule[]>([]);
  const [repoT, setRepoT] = useState<RepoTargeting>("all");
  const [repoInc, setRepoInc] = useState<string[]>([]);
  const [repoExc, setRepoExc] = useState<string[]>([]);
  const [repoProtected, setRepoProtected] = useState(false);
  const [repoIds, setRepoIds] = useState<string[]>([]);
  const [propInc, setPropInc] = useState<{ name: string; property_values: string[]; source?: string }[]>([]);
  const [propExc, setPropExc] = useState<{ name: string; property_values: string[]; source?: string }[]>([]);
  const [orgInc, setOrgInc] = useState<string[]>(["~ALL"]);
  const [orgExc, setOrgExc] = useState<string[]>([]);
  const act = useAction();
  useTitle(id ? "Edit ruleset" : "New ruleset");

  useEffect(() => {
    const r = existing.data;
    if (!r) {
      if (!id) {
        const t = sp.get("target") || "branch";
        setRules(RULES.filter((m) => m.defaultOn?.includes(t as "branch")).map((m) => ({ type: m.type, ...(defaultParams(m) ? { parameters: defaultParams(m) } : {}) })));
        setRefs(t === "branch" ? [{ kind: "include", value: "~DEFAULT_BRANCH" }] : []);
      }
      return;
    }
    setName(r.name);
    setTarget(r.target || "branch");
    setEnforcement(r.enforcement);
    setBypass(r.bypass_actors || []);
    setRefs(toRefTargets(r.conditions));
    setRules(r.rules || []);
    const c = r.conditions || {};
    if (c.repository_id) { setRepoT("ids"); setRepoIds(c.repository_id.repository_ids.map(String)); }
    else if (c.repository_property) { setRepoT("property"); setPropInc(c.repository_property.include || []); setPropExc(c.repository_property.exclude || []); }
    else if (c.repository_name) {
      const all = c.repository_name.include.length === 1 && c.repository_name.include[0] === "~ALL" && !c.repository_name.exclude.length && !c.repository_name.protected;
      setRepoT(all ? "all" : "name");
      setRepoInc(c.repository_name.include);
      setRepoExc(c.repository_name.exclude);
      setRepoProtected(!!c.repository_name.protected);
    }
    if (c.organization_name) { setOrgInc(c.organization_name.include); setOrgExc(c.organization_name.exclude); }
  }, [existing.data, id, sp]);

  if (id && existing.loading) return <Spinner />;
  if (id && existing.error) return <Async state={existing}>{() => null}</Async>;

  const conditions = (): RulesetConditions | null => {
    const c: RulesetConditions = {};
    if (target !== "push") {
      c.ref_name = { include: refs.filter((r) => r.kind === "include").map((r) => r.value), exclude: refs.filter((r) => r.kind === "exclude").map((r) => r.value) };
    }
    if (level.kind !== "repo") {
      if (repoT === "all") c.repository_name = { include: ["~ALL"], exclude: [] };
      else if (repoT === "name") c.repository_name = { include: repoInc, exclude: repoExc, protected: repoProtected };
      else if (repoT === "ids") c.repository_id = { repository_ids: repoIds.map(Number).filter(Boolean) };
      else c.repository_property = { include: propInc, exclude: propExc };
    }
    if (level.kind === "enterprise") c.organization_name = { include: orgInc, exclude: orgExc };
    return Object.keys(c).length ? c : null;
  };
  const input = (): RulesetInput => ({ name: name.trim(), target, enforcement, bypass_actors: bypass, conditions: conditions(), rules });
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const out = await act.run(() => (id ? ra.updateRuleset(level.api, id, input()) : ra.createRuleset(level.api, input())), id ? "Ruleset updated." : "Ruleset created.");
    if (out && !id) nav(`${level.root}/${out.id}`, { replace: true });
    if (out && id) existing.setData(out);
  };
  const exportJson = () => {
    const blob = new Blob([JSON.stringify({ id, ...input(), source_type: existing.data?.source_type, source: existing.data?.source }, null, 2)], { type: "application/json" });
    const a = document.createElement("a");
    a.href = URL.createObjectURL(blob);
    a.download = `${name || "ruleset"}.json`;
    a.click();
    URL.revokeObjectURL(a.href);
  };
  const metas = RULES.filter((m) => m.targets.includes(target as "branch"));
  return (
    <form onSubmit={submit} className="ra-ruleset-editor" data-testid="ruleset-editor">
      <div className="ra-crumbs small"><Link to={level.root}>Rulesets</Link> / {id ? existing.data?.name : `New ${target} ruleset`}</div>
      <SectionHead title={id ? existing.data?.name || "Ruleset" : `New ${target} ruleset`}
        actions={id ? (
          <>
            <Link className="btn btn-sm" to={`${level.root}/${id}/history`}>History</Link>
            <button type="button" className="btn btn-sm" onClick={exportJson}>Export ruleset</button>
          </>
        ) : undefined} />
      <Status state={act.state} />

      <section className="box box-body">
        <div className="form-group">
          <label htmlFor="rs-name">Ruleset Name</label>
          <input id="rs-name" className="input" required value={name} onChange={(e) => setName(e.target.value)} />
        </div>
        <div className="form-row ra-form-row">
          <div className="form-group">
            <label htmlFor="rs-enforcement">Enforcement status</label>
            <select id="rs-enforcement" className="input input-auto" value={enforcement} onChange={(e) => setEnforcement(e.target.value)}>
              <option value="active">Active</option>
              <option value="evaluate">Evaluate</option>
              <option value="disabled">Disabled</option>
            </select>
            <p className="small muted">{enforcement === "active" ? "This ruleset will be enforced." : enforcement === "evaluate" ? "Rules are evaluated and recorded in rule insights, but not enforced." : "This ruleset will not be enforced."}</p>
          </div>
          {!id && (
            <div className="form-group">
              <label htmlFor="rs-target">Target</label>
              <select id="rs-target" className="input input-auto" value={target} onChange={(e) => {
                const t = e.target.value;
                setTarget(t);
                setRules((rs) => rs.filter((r) => RULES.find((m) => m.type === r.type)?.targets.includes(t as "branch")));
                if (t === "push") setRefs([]);
              }}>
                <option value="branch">Branches</option>
                <option value="tag">Tags</option>
                <option value="push">Push</option>
              </select>
            </div>
          )}
        </div>
      </section>

      <BypassEditor level={level} value={bypass} onChange={setBypass} />

      <section className="box">
        <div className="box-header"><h3 className="h5">Targets</h3></div>
        <div className="box-body">
          {level.kind === "enterprise" && (
            <fieldset className="form-group">
              <legend>Target organizations</legend>
              <div className="form-row ra-form-row">
                <div className="form-group"><label htmlFor="rs-org-inc">Include (names or patterns, <code>~ALL</code> for all)</label><ListInput id="rs-org-inc" value={orgInc} onChange={setOrgInc} rows={2} /></div>
                <div className="form-group"><label htmlFor="rs-org-exc">Exclude</label><ListInput id="rs-org-exc" value={orgExc} onChange={setOrgExc} rows={2} /></div>
              </div>
            </fieldset>
          )}
          {level.kind !== "repo" && (
            <RepoTargetEditor level={level} t={repoT} setT={setRepoT} inc={repoInc} setInc={setRepoInc} exc={repoExc} setExc={setRepoExc}
              prot={repoProtected} setProt={setRepoProtected} ids={repoIds} setIds={setRepoIds} propInc={propInc} setPropInc={setPropInc}
              propExc={propExc} setPropExc={setPropExc} />
          )}
          {target === "push" ? (
            <p className="muted small">Push rulesets apply to the whole repository and its forks' pushes, regardless of the ref.</p>
          ) : (
            <RefTargetEditor target={target} value={refs} onChange={setRefs} />
          )}
        </div>
      </section>

      <section className="box">
        <div className="box-header"><h3 className="h5">{target === "push" ? "Push rules" : target === "tag" ? "Tag rules" : "Branch rules"}</h3></div>
        <div className="box-body ra-rule-list">
          {metas.map((m) => (
            <RuleEditor key={m.type} meta={m} level={level} rule={rules.find((r) => r.type === m.type)}
              onChange={(r) => setRules((rs) => (r ? [...rs.filter((x) => x.type !== m.type), r] : rs.filter((x) => x.type !== m.type)))} />
          ))}
        </div>
      </section>

      <div className="form-actions">
        <button type="submit" className="btn btn-primary" disabled={act.busy || !name.trim()}>{id ? "Save changes" : "Create"}</button>
        <Link className="btn" to={level.root}>Cancel</Link>
      </div>
    </form>
  );
}

function RefTargetEditor({ target, value, onChange }: { target: string; value: RefTarget[]; onChange: (v: RefTarget[]) => void }) {
  const [pat, setPat] = useState("");
  const [kind, setKind] = useState<"include" | "exclude">("include");
  const noun = target === "tag" ? "tags" : "branches";
  const add = (t: RefTarget) => {
    if (!value.some((v) => v.kind === t.kind && v.value === t.value)) onChange([...value, t]);
  };
  return (
    <fieldset className="form-group" data-testid="ref-targets">
      <legend>Target {noun}</legend>
      <p className="small muted">Which {noun} do you want to make a ruleset for?</p>
      {value.length === 0 ? <p className="muted small">{noun[0].toUpperCase() + noun.slice(1)} targeting has not been configured.</p> : (
        <ul className="ra-chips">
          {value.map((v, i) => (
            <li key={i} className="ra-chip">
              <span className="small muted">{v.kind === "include" ? "Include" : "Exclude"}:</span> <span className="mono">{refLabel(v.value)}</span>
              <button type="button" className="btn-link ra-chip-x" aria-label={`Remove ${refLabel(v.value)}`} onClick={() => onChange(value.filter((_, j) => j !== i))}>×</button>
            </li>
          ))}
        </ul>
      )}
      <div className="ra-ref-add">
        {target === "branch" && <button type="button" className="btn btn-sm" onClick={() => add({ kind: "include", value: "~DEFAULT_BRANCH" })}>Include default branch</button>}
        <button type="button" className="btn btn-sm" onClick={() => add({ kind: "include", value: "~ALL" })}>Include all {noun}</button>
        <div className="input-group ra-pattern-add">
          <select className="input input-auto input-sm" value={kind} aria-label="Include or exclude" onChange={(e) => setKind(e.target.value as "include")}>
            <option value="include">Include by pattern</option>
            <option value="exclude">Exclude by pattern</option>
          </select>
          <input className="input input-sm mono" placeholder={target === "tag" ? "v*" : "release/*"} value={pat} aria-label={`${noun} name pattern`}
            onChange={(e) => setPat(e.target.value)} onKeyDown={(e) => {
              if (e.key === "Enter") { e.preventDefault(); if (pat.trim()) { add({ kind, value: refPrefix(target) + pat.trim() }); setPat(""); } }
            }} />
          <button type="button" className="btn btn-sm" disabled={!pat.trim()} onClick={() => { add({ kind, value: refPrefix(target) + pat.trim() }); setPat(""); }}>Add target</button>
        </div>
      </div>
    </fieldset>
  );
}

function RepoTargetEditor(p: {
  level: RulesLevel; t: RepoTargeting; setT: (v: RepoTargeting) => void; inc: string[]; setInc: (v: string[]) => void; exc: string[]; setExc: (v: string[]) => void;
  prot: boolean; setProt: (v: boolean) => void; ids: string[]; setIds: (v: string[]) => void;
  propInc: { name: string; property_values: string[]; source?: string }[]; setPropInc: (v: { name: string; property_values: string[]; source?: string }[]) => void;
  propExc: { name: string; property_values: string[]; source?: string }[]; setPropExc: (v: { name: string; property_values: string[]; source?: string }[]) => void;
}) {
  const props = useAsync(() => (p.level.kind === "org" ? ra.propertySchema(p.level.api).catch(() => [] as PropertyDef[]) : Promise.resolve([] as PropertyDef[])), [p.level.api]);
  return (
    <fieldset className="form-group" data-testid="repo-targets">
      <legend>Target repositories</legend>
      <select className="input input-auto" aria-label="Repository targeting" value={p.t} onChange={(e) => p.setT(e.target.value as RepoTargeting)}>
        <option value="all">All repositories</option>
        <option value="name">Dynamic list by name</option>
        {p.level.kind === "org" && <option value="property">Dynamic list by property</option>}
        <option value="ids">Select repositories (by id)</option>
      </select>
      {p.t === "name" && (
        <div className="form-row ra-form-row">
          <div className="form-group"><label htmlFor="rs-repo-inc">Include repositories by pattern</label><ListInput id="rs-repo-inc" value={p.inc} onChange={p.setInc} rows={2} placeholder="api-*" /></div>
          <div className="form-group"><label htmlFor="rs-repo-exc">Exclude repositories by pattern</label><ListInput id="rs-repo-exc" value={p.exc} onChange={p.setExc} rows={2} /></div>
          <label className="check-row"><input type="checkbox" checked={p.prot} onChange={(e) => p.setProt(e.target.checked)} /> Prevent renaming of target repositories</label>
        </div>
      )}
      {p.t === "ids" && (
        <div className="form-group"><label htmlFor="rs-repo-ids">Repository ids (one per line)</label><ListInput id="rs-repo-ids" value={p.ids} onChange={p.setIds} rows={2} /></div>
      )}
      {p.t === "property" && (
        <div className="ra-props-target">
          {(["include", "exclude"] as const).map((k) => {
            const list = k === "include" ? p.propInc : p.propExc;
            const setList = k === "include" ? p.setPropInc : p.setPropExc;
            return (
              <div key={k} className="form-group">
                <span className="label-text">{k === "include" ? "Include" : "Exclude"} repositories with property</span>
                {list.map((x, i) => (
                  <div key={i} className="input-group ra-prop-row">
                    <select className="input input-sm input-auto" aria-label="Property" value={x.name}
                      onChange={(e) => setList(list.map((y, j) => (j === i ? { ...y, name: e.target.value } : y)))}>
                      <option value="">Property…</option>
                      {(props.data || []).map((d) => <option key={d.property_name} value={d.property_name}>{d.property_name}</option>)}
                    </select>
                    <input className="input input-sm" aria-label="Values (comma separated)" placeholder="values, comma separated" value={x.property_values.join(", ")}
                      onChange={(e) => setList(list.map((y, j) => (j === i ? { ...y, property_values: split(e.target.value) } : y)))} />
                    <button type="button" className="btn btn-sm" onClick={() => setList(list.filter((_, j) => j !== i))}>Remove</button>
                  </div>
                ))}
                <button type="button" className="btn btn-sm" onClick={() => setList([...list, { name: "", property_values: [], source: "custom" }])}>Add a target</button>
              </div>
            );
          })}
        </div>
      )}
    </fieldset>
  );
}

function BypassEditor({ level, value, onChange }: { level: RulesLevel; value: BypassActor[]; onChange: (v: BypassActor[]) => void }) {
  const org = level.org;
  const teams = useAsync(() => (org ? ra.orgTeams(org).catch(() => []) : Promise.resolve([])), [org]);
  const roles = useAsync(() => (org ? ra.customRoles(org).catch(() => [] as CustomRole[]) : Promise.resolve([] as CustomRole[])), [org]);
  const [pick, setPick] = useState("");
  const [appId, setAppId] = useState("");
  const options = useMemo(() => {
    const o: [string, string][] = ROLE_IDS.map(([id, n]) => [`RepositoryRole:${id}`, `Role: ${n}`]);
    for (const r of roles.data || []) o.push([`RepositoryRole:${r.id}`, `Role: ${r.name}`]);
    if (level.kind !== "repo" || org) o.push(["OrganizationAdmin:1", "Organization admin"]);
    o.push(["EnterpriseOwner:1", "Enterprise owner"]);
    if (level.kind === "repo") o.push(["DeployKey:", "Deploy keys"]);
    for (const t of teams.data || []) o.push([`Team:${t.id}`, `Team: ${t.name}`]);
    return o;
  }, [roles.data, teams.data, level.kind, org]);
  const name = (b: BypassActor) => {
    const key = `${b.actor_type}:${b.actor_id ?? ""}`;
    const o = options.find((x) => x[0] === key);
    if (o) return o[1];
    if (b.actor_type === "Integration") return `App #${b.actor_id}`;
    return `${ACTOR_LABEL[b.actor_type] || b.actor_type}${b.actor_id ? ` #${b.actor_id}` : ""}`;
  };
  const add = (type: string, id: number | null) => {
    if (value.some((v) => v.actor_type === type && (v.actor_id ?? null) === id)) return;
    onChange([...value, { actor_type: type, actor_id: id, bypass_mode: "always" }]);
  };
  return (
    <section className="box" data-testid="bypass-list">
      <div className="box-header"><h3 className="h5">Bypass list</h3></div>
      <div className="box-body">
        <p className="small muted">Exempt roles, teams, or apps from this ruleset by adding them to the bypass list.</p>
        {value.length === 0 ? <p className="muted">Bypass list is empty</p> : (
          <ul className="ra-list ra-list-bordered">
            {value.map((b, i) => (
              <li key={i} className="ra-row">
                <div className="ra-row-main"><strong>{name(b)}</strong></div>
                <div className="ra-row-actions">
                  <select className="input input-sm input-auto" aria-label={`Bypass mode for ${name(b)}`} value={b.bypass_mode}
                    onChange={(e) => onChange(value.map((x, j) => (j === i ? { ...x, bypass_mode: e.target.value } : x)))}>
                    {MODES.map(([v, l]) => <option key={v} value={v}>{l}</option>)}
                  </select>
                  <button type="button" className="btn btn-sm" aria-label={`Remove ${name(b)} from bypass list`} onClick={() => onChange(value.filter((_, j) => j !== i))}>Remove</button>
                </div>
              </li>
            ))}
          </ul>
        )}
        <div className="input-group ra-bypass-add">
          <select className="input input-sm" aria-label="Add bypass" value={pick} onChange={(e) => setPick(e.target.value)}>
            <option value="">Add bypass…</option>
            {options.map(([v, l]) => <option key={v} value={v}>{l}</option>)}
          </select>
          <button type="button" className="btn btn-sm" disabled={!pick} onClick={() => {
            const [t, id] = pick.split(":");
            add(t, id ? Number(id) : null);
            setPick("");
          }}>Add</button>
        </div>
        <div className="input-group ra-bypass-add">
          <input className="input input-sm" inputMode="numeric" placeholder="App id" aria-label="App id to bypass" value={appId} onChange={(e) => setAppId(e.target.value.replace(/\D/g, ""))} />
          <button type="button" className="btn btn-sm" disabled={!appId} onClick={() => { add("Integration", Number(appId)); setAppId(""); }}>Add app</button>
        </div>
      </div>
    </section>
  );
}

function RuleEditor({ meta, rule, onChange, level }: { meta: RuleMeta; rule?: Rule; onChange: (r: Rule | null) => void; level: RulesLevel }) {
  const on = !!rule;
  const params = (rule?.parameters || {}) as Record<string, unknown>;
  const setP = (k: string, v: unknown) => onChange({ type: meta.type, parameters: { ...params, [k]: v } });
  return (
    <div className={`ra-rule${on ? " on" : ""}`} data-testid={`rule-${meta.type}`}>
      <label className="check-row">
        <input type="checkbox" checked={on} onChange={(e) => {
          const dp = defaultParams(meta);
          onChange(e.target.checked ? { type: meta.type, ...(dp ? { parameters: dp } : {}) } : null);
        }} />
        <span><strong>{meta.label}</strong><span className="ra-check-desc small muted">{meta.desc}</span></span>
      </label>
      {on && meta.params && (
        <div className="ra-check-children">
          {meta.params.map((p, i) => <ParamEditor key={i} p={p} type={meta.type} params={params} setP={setP} level={level} />)}
        </div>
      )}
    </div>
  );
}

function ParamEditor({ p, type, params, setP, level }: { p: ParamKind; type: string; params: Record<string, unknown>; setP: (k: string, v: unknown) => void; level: RulesLevel }) {
  const id = (k: string) => `rp-${type}-${k}`;
  switch (p.kind) {
    case "bool":
      return <label className="check-row"><input id={id(p.key)} type="checkbox" checked={!!params[p.key]} onChange={(e) => setP(p.key, e.target.checked)} /> {p.label}</label>;
    case "number":
      return (
        <div className="form-group">
          <label htmlFor={id(p.key)}>{p.label}</label>
          <input id={id(p.key)} type="number" className="input input-auto" min={p.min} max={p.max} value={Number(params[p.key] ?? p.default)}
            onChange={(e) => setP(p.key, Number(e.target.value))} />
        </div>
      );
    case "select":
      return (
        <div className="form-group">
          <label htmlFor={id(p.key)}>{p.label}</label>
          <select id={id(p.key)} className="input input-auto" value={String(params[p.key] ?? p.default)} onChange={(e) => setP(p.key, e.target.value)}>
            {p.options.map(([v, l]) => <option key={v} value={v}>{l}</option>)}
          </select>
        </div>
      );
    case "multi": {
      const cur = (params[p.key] as string[] | undefined) || p.default;
      return (
        <fieldset className="form-group">
          <legend>{p.label}</legend>
          {p.options.map(([v, l]) => (
            <label key={v} className="check-row">
              <input type="checkbox" checked={cur.includes(v)} onChange={(e) => setP(p.key, e.target.checked ? [...cur, v] : cur.filter((x) => x !== v))} /> {l}
            </label>
          ))}
        </fieldset>
      );
    }
    case "list":
      return (
        <div className="form-group">
          <label htmlFor={id(p.key)}>{p.label}</label>
          <ListInput id={id(p.key)} value={(params[p.key] as string[]) || []} onChange={(v) => setP(p.key, v)} placeholder={p.placeholder} />
        </div>
      );
    case "text":
      return (
        <div className="form-group">
          <label htmlFor={id(p.key)}>{p.label}</label>
          <input id={id(p.key)} className="input" value={String(params[p.key] ?? "")} placeholder={p.placeholder} onChange={(e) => setP(p.key, e.target.value)} />
        </div>
      );
    case "checks": {
      const list = (params.required_status_checks as { context: string; integration_id?: number }[]) || [];
      return (
        <div className="form-group">
          <label htmlFor={id("contexts")}>Status checks that are required (one per line)</label>
          <ListInput id={id("contexts")} value={list.map((c) => c.context)} placeholder="ci/build"
            onChange={(v) => setP("required_status_checks", v.map((context) => ({ context, ...(list.find((c) => c.context === context)?.integration_id ? { integration_id: list.find((c) => c.context === context)!.integration_id } : {}) })))} />
        </div>
      );
    }
    case "workflows": {
      const list = (params.workflows as { path: string; repository_id: number; ref?: string }[]) || [];
      return (
        <div className="form-group">
          <label htmlFor={id("workflows")}>Workflow files (<code>path@ref</code>, one per line; in this repository{level.kind !== "repo" ? " — use repo-id:path for others" : ""})</label>
          <ListInput id={id("workflows")} value={list.map((w) => `${level.repoId && w.repository_id !== level.repoId ? `${w.repository_id}:` : ""}${w.path}${w.ref ? `@${w.ref}` : ""}`)}
            placeholder=".github/workflows/ci.yml@refs/heads/main"
            onChange={(v) => setP("workflows", v.map((s) => {
              const m = s.match(/^(?:(\d+):)?([^@]+)(?:@(.+))?$/);
              return { path: m?.[2] || s, repository_id: m?.[1] ? Number(m[1]) : level.repoId || 0, ...(m?.[3] ? { ref: m[3] } : {}) };
            }))} />
        </div>
      );
    }
    case "tools": {
      const list = (params.code_scanning_tools as { tool: string; alerts_threshold: string; security_alerts_threshold: string }[]) || [];
      const upd = (i: number, k: string, v: string) => setP("code_scanning_tools", list.map((t, j) => (j === i ? { ...t, [k]: v } : t)));
      return (
        <div className="form-group">
          <span className="label-text">Required tools and alert thresholds</span>
          {list.map((t, i) => (
            <div key={i} className="input-group ra-prop-row">
              <input className="input input-sm" aria-label="Tool" value={t.tool} onChange={(e) => upd(i, "tool", e.target.value)} />
              <select className="input input-sm input-auto" aria-label="Alerts" value={t.alerts_threshold} onChange={(e) => upd(i, "alerts_threshold", e.target.value)}>
                {["none", "errors", "errors_and_warnings", "all"].map((x) => <option key={x} value={x}>Alerts: {x}</option>)}
              </select>
              <select className="input input-sm input-auto" aria-label="Security alerts" value={t.security_alerts_threshold} onChange={(e) => upd(i, "security_alerts_threshold", e.target.value)}>
                {["none", "critical", "high_or_higher", "medium_or_higher", "all"].map((x) => <option key={x} value={x}>Security: {x}</option>)}
              </select>
              <button type="button" className="btn btn-sm" onClick={() => setP("code_scanning_tools", list.filter((_, j) => j !== i))}>Remove</button>
            </div>
          ))}
          <button type="button" className="btn btn-sm" onClick={() => setP("code_scanning_tools", [...list, { tool: "", alerts_threshold: "errors", security_alerts_threshold: "high_or_higher" }])}>Add tool</button>
        </div>
      );
    }
  }
}

function RulesetHistory({ level, id }: { level: RulesLevel; id: number }) {
  const st = useAsync(() => ra.rulesetHistory(level.api, id), [level.api, id]);
  return (
    <>
      <div className="ra-crumbs small"><Link to={level.root}>Rulesets</Link> / <Link to={`${level.root}/${id}`}>Ruleset</Link> / History</div>
      <SectionHead title="Ruleset history" />
      <Async state={st}>
        {(list) =>
          !list || list.length === 0 ? <div className="box"><Blankslate title="No history recorded" /></div> : (
            <ul className="box ra-list" data-testid="ruleset-history">
              {list.map((v) => (
                <li key={v.version_id} className="ra-row">
                  <div className="ra-row-main">
                    <strong>Version {v.version_id}</strong>
                    <div className="small muted">Updated <RelTime iso={v.updated_at} />{v.actor ? ` by ${v.actor.type} #${v.actor.id}` : ""}</div>
                  </div>
                </li>
              ))}
            </ul>
          )
        }
      </Async>
    </>
  );
}

// ---------- insights ----------

export const resultPill = (r: string | null) =>
  r === "pass" ? <Pill kind="ok">Pass</Pill> : r === "fail" ? <Pill kind="bad">Fail</Pill> : r === "bypass" ? <Pill kind="warn">Bypass</Pill> : <Pill>{r || "—"}</Pill>;

function Insights({ level }: { level: RulesLevel }) {
  useTitle("Rule insights");
  const [sp, setSp] = useSearchParams();
  const q = {
    ref: sp.get("ref") || undefined,
    time_period: sp.get("time_period") || "day",
    actor_name: sp.get("actor_name") || undefined,
    rule_suite_result: sp.get("rule_suite_result") || "all",
    repository_name: sp.get("repository_name") || undefined,
    page: Number(sp.get("page") || 1),
  };
  const st = useAsync(() => ra.ruleSuites(level.api, q), [level.api, sp.toString()]);
  const set = (k: string, v: string) => {
    const n = new URLSearchParams(sp);
    if (v) n.set(k, v); else n.delete(k);
    n.delete("page");
    setSp(n, { replace: true });
  };
  return (
    <>
      <SectionHead title="Rule insights">Every evaluated push or ref update is recorded with its outcome.</SectionHead>
      <div className="toolbar ra-filters" role="search">
        <select className="input input-sm input-auto" aria-label="Time period" value={q.time_period} onChange={(e) => set("time_period", e.target.value)}>
          <option value="hour">Last hour</option><option value="day">Last 24 hours</option><option value="week">Last 7 days</option><option value="month">Last 30 days</option>
        </select>
        <select className="input input-sm input-auto" aria-label="Status" value={q.rule_suite_result} onChange={(e) => set("rule_suite_result", e.target.value)}>
          <option value="all">All statuses</option><option value="pass">Pass</option><option value="fail">Fail</option><option value="bypass">Bypass</option>
        </select>
        <input className="input input-sm ra-filter-input" placeholder="Actor" aria-label="Actor" defaultValue={q.actor_name} onBlur={(e) => set("actor_name", e.target.value.trim())} />
        {level.kind === "repo" ? (
          <input className="input input-sm ra-filter-input" placeholder="Branch or tag" aria-label="Ref" defaultValue={q.ref} onBlur={(e) => set("ref", e.target.value.trim())} />
        ) : (
          <input className="input input-sm ra-filter-input" placeholder="Repository" aria-label="Repository" defaultValue={q.repository_name} onBlur={(e) => set("repository_name", e.target.value.trim())} />
        )}
      </div>
      <Async state={st}>
        {(pg) =>
          pg.items.length === 0 ? <div className="box"><Blankslate title="No rule evaluations">Nothing matched these filters.</Blankslate></div> : (
            <>
              <ul className="box ra-list" data-testid="rule-suites">
                {pg.items.map((s) => (
                  <li key={s.id} className="ra-row" data-testid="rule-suite">
                    {resultPill(s.result)}
                    <div className="ra-row-main">
                      <Link to={`${level.root}/insights/${s.id}`} className="ra-strong">{s.ref.replace(/^refs\/(heads|tags)\//, "")}</Link>
                      <div className="small muted">
                        {s.actor_name || "unknown"} pushed {shortSha(s.after_sha)} {level.kind !== "repo" && <>to {s.repository_name} </>}<RelTime iso={s.pushed_at} />
                        {s.evaluation_result && s.evaluation_result !== s.result && <> · evaluate: {s.evaluation_result}</>}
                      </div>
                    </div>
                  </li>
                ))}
              </ul>
              <div className="pagination">
                <button type="button" className="btn btn-sm" disabled={!pg.prev} onClick={() => { const n = new URLSearchParams(sp); n.set("page", String(pg.prev)); setSp(n); }}>‹ Previous</button>
                <button type="button" className="btn btn-sm" disabled={!pg.next} onClick={() => { const n = new URLSearchParams(sp); n.set("page", String(pg.next)); setSp(n); }}>Next ›</button>
              </div>
            </>
          )
        }
      </Async>
    </>
  );
}

function SuiteDetail({ level, id }: { level: RulesLevel; id: number }) {
  const st = useAsync(() => ra.ruleSuite(level.api, id), [level.api, id]);
  return (
    <Async state={st}>
      {(s: RuleSuite) => (
        <div data-testid="rule-suite-detail">
          <div className="ra-crumbs small"><Link to={`${level.root}/insights`}>Rule insights</Link> / {id}</div>
          <SectionHead title={<>{resultPill(s.result)} {s.ref}</>}>
            {s.actor_name || "unknown"} pushed to {s.repository_name} <RelTime iso={s.pushed_at} /> · <span className="mono">{shortSha(s.before_sha)}…{shortSha(s.after_sha)}</span>
          </SectionHead>
          <ul className="box ra-list">
            {(s.rule_evaluations || []).length === 0 && <li className="ra-row muted">No rules were evaluated.</li>}
            {(s.rule_evaluations || []).map((e, i) => (
              <li key={i} className="ra-row">
                {resultPill(e.result)}
                <div className="ra-row-main">
                  <strong>{ruleLabel(e.rule_type)}</strong> <Pill>{e.enforcement}</Pill>
                  <div className="small muted">{e.rule_source.type} ruleset {e.rule_source.name || `#${e.rule_source.id}`}{e.details ? ` · ${e.details}` : ""}</div>
                </div>
              </li>
            ))}
          </ul>
        </div>
      )}
    </Async>
  );
}

// ---------- bypass requests ----------

export const bypassPill = (s: string) =>
  s === "approved" || s === "completed" ? <Pill kind="ok">{s}</Pill> : s === "denied" || s === "cancelled" || s === "expired" ? <Pill kind="bad">{s}</Pill> : <Pill kind="warn">{s}</Pill>;

export function BypassList({ level, title = "Push bypass requests" }: { level: RulesLevel; title?: string }) {
  const [status, setStatus] = useState("all");
  const st = useAsync(() => ra.pushBypassRequests(level.api, status), [level.api, status]);
  const loc = useLocation();
  return (
    <>
      <SectionHead title={title}>
        When a push is blocked by a push ruleset with bypass actors, the pusher can request a bypass. Bypass actors approve or deny it here; an approved commit can then be pushed.
      </SectionHead>
      <div className="toolbar">
        <select className="input input-sm input-auto" aria-label="Request status" value={status} onChange={(e) => setStatus(e.target.value)}>
          {["all", "open", "approved", "denied", "completed", "cancelled", "expired"].map((s) => <option key={s} value={s}>{s === "all" ? "All statuses" : s}</option>)}
        </select>
      </div>
      <Async state={st}>
        {(list) =>
          list.length === 0 ? <div className="box"><Blankslate title="No bypass requests" /></div> : (
            <ul className="box ra-list" data-testid="bypass-requests">
              {list.map((b) => (
                <li key={b.id} className="ra-row" data-testid={`bypass-${b.number}`}>
                  {bypassPill(b.status)}
                  <div className="ra-row-main">
                    <Link className="ra-strong" to={`/${b.repository.full_name}/exemptions/${b.number}`} state={{ from: loc.pathname }}>
                      #{b.number} {(b.data || []).map((d) => d.ruleset_name).join(", ") || "bypass request"}
                    </Link>
                    <div className="small muted">
                      {b.requester?.actor_name} requested <RelTime iso={b.created_at} /> · {b.repository.full_name} · commit <span className="mono">{shortSha(b.resource_identifier)}</span>
                    </div>
                  </div>
                </li>
              ))}
            </ul>
          )
        }
      </Async>
    </>
  );
}

/** `/{o}/{r}/exemptions/:number`: one push bypass request; approve / deny / cancel. */
export function BypassRequestPage({ owner, repo, canReview }: { owner: string; repo: string; canReview: boolean }) {
  const { number = "" } = useParams();
  const n = Number(number);
  useTitle(`Bypass request #${n}`);
  const st = useAsync(() => ra.pushBypassRequest(owner, repo, n), [owner, repo, n]);
  const act = useAction();
  const [msg, setMsg] = useState("");
  const review = async (status: "approve" | "deny") => {
    const out = await act.run(() => ra.reviewPushBypass(owner, repo, n, status, msg || undefined), status === "approve" ? "Bypass request approved." : "Bypass request denied.");
    if (out) st.setData(out);
  };
  return (
    <div className="ra-exemption">
      <Async state={st}>
        {(b: BypassRequest) => (
          <div data-testid="bypass-request">
            <div className="ra-crumbs small"><Link to={`/${owner}/${repo}/exemptions`}>Bypass requests</Link> / #{b.number}</div>
            <SectionHead title={<>Bypass request #{b.number} {bypassPill(b.status)}</>}>
              {b.requester?.actor_name} wants to push commit <span className="mono">{shortSha(b.resource_identifier)}</span>. Expires <RelTime iso={b.expires_at} />.
            </SectionHead>
            <Status state={act.state} />
            <div className="box">
              <div className="box-header"><h3 className="h5">Rule violations</h3></div>
              <ul className="ra-list">
                {(b.data || []).map((d, i) => (
                  <li key={i} className="ra-row"><div className="ra-row-main"><strong>{d.ruleset_name}</strong> <span className="muted small">{ruleLabel(d.rule_type)}</span></div></li>
                ))}
              </ul>
              {b.requester_comment && <div className="box-body"><strong>Comment:</strong> {b.requester_comment}</div>}
            </div>
            {(b.responses || []).length > 0 && (
              <div className="box">
                <div className="box-header"><h3 className="h5">Reviews</h3></div>
                <ul className="ra-list">
                  {(b.responses || []).map((r) => (
                    <li key={r.id} className="ra-row">{bypassPill(r.status)}<div className="ra-row-main">{r.reviewer?.actor_name} <RelTime iso={r.created_at} /></div></li>
                  ))}
                </ul>
              </div>
            )}
            {b.status === "pending" || b.status === "open" ? (
              <div className="box box-body">
                {canReview && (
                  <>
                    <label htmlFor="bypass-msg" className="label-text">Review comment (optional)</label>
                    <textarea id="bypass-msg" className="input textarea" rows={2} value={msg} onChange={(e) => setMsg(e.target.value)} />
                  </>
                )}
                <div className="form-actions">
                  {canReview && <button type="button" className="btn btn-primary" disabled={act.busy} onClick={() => review("approve")}>Approve bypass</button>}
                  {canReview && <button type="button" className="btn btn-danger" disabled={act.busy} onClick={() => review("deny")}>Deny</button>}
                  <button type="button" className="btn" disabled={act.busy} onClick={async () => {
                    const ok = await act.run(() => ra.cancelPushBypass(owner, repo, n).then(() => true), "Request cancelled.");
                    if (ok) st.reload();
                  }}>Cancel request</button>
                </div>
              </div>
            ) : null}
          </div>
        )}
      </Async>
    </div>
  );
}
