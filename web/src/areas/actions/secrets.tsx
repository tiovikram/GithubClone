// Secrets and variables for Actions: repository, environment and organization scopes.
// Secret values are sealed in the browser with the scope's public key (see sealed.ts).
import { useState, type FormEvent } from "react";
import { Link, NavLink, useNavigate } from "react-router-dom";
import type { Repo } from "../../api";
import { Async, Blankslate, ErrorBox, Flash, RelTime } from "../../components/common";
import { useAsync } from "../../hooks";
import { actionsApi as A, type Scope, type Secret, type Variable } from "./api";

const NAME_RE = /^[A-Za-z_][A-Za-z0-9_]*$/;

function nameError(n: string): string | null {
  if (!n) return "Name is required.";
  if (!NAME_RE.test(n)) return "Secret and variable names can only contain alphanumeric characters ([a-z], [A-Z], [0-9]) or underscores (_), and must start with a letter or underscore.";
  if (/^GITHUB_/i.test(n)) return "Names must not start with GITHUB_.";
  return null;
}

/** Tabs between the Secrets and Variables pages of one scope. */
export function SecretsTabs({ root }: { root: string }) {
  return (
    <nav className="tabnav" aria-label="Secrets and variables">
      <NavLink className={({ isActive }) => "tabnav-tab" + (isActive ? " selected" : "")} to={`${root}/secrets/actions`} end>Secrets</NavLink>
      <NavLink className={({ isActive }) => "tabnav-tab" + (isActive ? " selected" : "")} to={`${root}/variables/actions`} end>Variables</NavLink>
    </nav>
  );
}

const scopeWord = (s: Scope) => (s.kind === "org" ? "organization" : s.kind === "env" ? "environment" : "repository");
const cap = (s: string) => s[0].toUpperCase() + s.slice(1);

/** List + delete of one scope's secrets; "New … secret" links to `newPath`. */
export function SecretsList({ scope, newPath, editPath, canEdit = true }: { scope: Scope; newPath?: string; editPath?: (name: string) => string; canEdit?: boolean }) {
  const st = useAsync(() => A.secrets(scope).then((r) => r.secrets), [JSON.stringify(scope)]);
  const [err, setErr] = useState<unknown>();
  const word = scopeWord(scope);
  return (
    <div className="box" data-testid={`${scope.kind}-secrets`}>
      <div className="box-header act-box-head">
        <strong>{cap(word)} secrets</strong>
        {canEdit && newPath && <Link className="btn btn-sm btn-primary" to={newPath} data-testid={`new-${scope.kind}-secret`}>New {word} secret</Link>}
      </div>
      {err !== undefined && <ErrorBox error={err} />}
      <Async state={st}>
        {(list) => list.length === 0 ? <div className="box-body muted small">This {word} has no secrets.</div> : (
          <ul className="list">
            {list.map((s) => (
              <li key={s.name} className="list-row act-secret-row" data-testid={`secret-${s.name}`}>
                <span className="mono grow act-break"><strong>{s.name}</strong></span>
                {s.visibility && <span className="badge">{s.visibility === "all" ? "All repositories" : s.visibility === "private" ? "Private repositories" : "Selected repositories"}</span>}
                <span className="muted small">Updated <RelTime iso={s.updated_at} /></span>
                {canEdit && editPath && <Link className="btn btn-sm" to={editPath(s.name)} aria-label={`Update ${s.name}`}>Update</Link>}
                {canEdit && (
                  <button type="button" className="btn btn-sm btn-danger-outline" aria-label={`Remove ${s.name}`} data-testid={`delete-secret-${s.name}`}
                    onClick={async () => {
                      if (!confirm(`Are you sure you want to delete ${s.name}?`)) return;
                      try {
                        await A.deleteSecret(scope, s.name);
                        st.reload();
                      } catch (e) {
                        setErr(e);
                      }
                    }}>Remove</button>
                )}
              </li>
            ))}
          </ul>
        )}
      </Async>
    </div>
  );
}

/** Create or update a secret (`existing` = update; the name is then fixed). */
export function SecretForm({ scope, existing, back, orgRepos }: { scope: Scope; existing?: string; back: string; orgRepos?: boolean }) {
  const nav = useNavigate();
  const [name, setName] = useState(existing || "");
  const [value, setValue] = useState("");
  const [vis, setVis] = useState<"all" | "private" | "selected">("all");
  const [repoIds, setRepoIds] = useState<number[]>([]);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string>();
  const cur = useAsync(
    () => (existing && scope.kind === "org"
      ? Promise.all([A.secrets(scope).then((r) => r.secrets.find((s) => s.name === existing)), A.orgItemRepos(scope.org, "secrets", existing).catch(() => ({ repositories: [] as Repo[] }))])
      : Promise.resolve(undefined)),
    [existing, JSON.stringify(scope)],
  );
  const [loaded, setLoaded] = useState(false);
  if (cur.data && !loaded) {
    setLoaded(true);
    const [s, repos] = cur.data;
    if (s?.visibility) setVis(s.visibility);
    setRepoIds(repos.repositories.map((r) => r.id));
  }
  const word = scopeWord(scope);
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const n = name.trim().toUpperCase();
    const ne = nameError(n);
    if (ne) return setErr(ne);
    if (!value) return setErr("Secret value is required.");
    setBusy(true);
    setErr(undefined);
    try {
      await A.putSecret(scope, n, value, scope.kind === "org" ? { visibility: vis, ...(vis === "selected" ? { selected_repository_ids: repoIds } : {}) } : {});
      nav(back);
    } catch (e2) {
      setErr(String((e2 as Error).message || e2));
    } finally {
      setBusy(false);
    }
  };
  return (
    <form className="act-form" onSubmit={submit} data-testid="secret-form">
      <h2 className="h3">{existing ? `Update secret` : `New ${word} secret`}</h2>
      <div className="form-group">
        <label htmlFor="secret-name">Name <span className="c-danger">*</span></label>
        <input id="secret-name" className="input mono" value={name} disabled={!!existing} onChange={(e) => setName(e.target.value)} placeholder="YOUR_SECRET_NAME" autoComplete="off" />
      </div>
      <div className="form-group">
        <label htmlFor="secret-value">{existing ? "Value" : "Secret"} <span className="c-danger">*</span></label>
        <textarea id="secret-value" className="input textarea mono" rows={5} value={value} onChange={(e) => setValue(e.target.value)} autoComplete="off" spellCheck={false} />
        {existing && <p className="muted small">Secret values are encrypted and cannot be displayed; enter a new value to replace it.</p>}
      </div>
      {scope.kind === "org" && orgRepos && <RepoAccess org={scope.org} vis={vis} setVis={setVis} ids={repoIds} setIds={setRepoIds} what="secret" />}
      {err && <Flash kind="error">{err}</Flash>}
      <div className="form-actions act-left">
        <button type="submit" className="btn btn-primary" disabled={busy} data-testid="secret-submit">{busy ? "Saving…" : existing ? "Update secret" : "Add secret"}</button>
        <Link className="btn" to={back}>Cancel</Link>
      </div>
    </form>
  );
}

/** Organization secret/variable repository access: all, private, or selected repositories. */
export function RepoAccess({ org, vis, setVis, ids, setIds, what }: {
  org: string; vis: "all" | "private" | "selected"; setVis: (v: "all" | "private" | "selected") => void; ids: number[]; setIds: (ids: number[]) => void; what: string;
}) {
  const repos = useAsync(() => A.orgRepos(org), [org]);
  return (
    <fieldset className="form-group">
      <legend>Repository access</legend>
      <label className="radio-row"><input type="radio" name="vis" checked={vis === "all"} onChange={() => setVis("all")} /> <span>Public repositories<br /><span className="muted small">This {what} may be used by any public repository in the organization.</span></span></label>
      <label className="radio-row"><input type="radio" name="vis" checked={vis === "private"} onChange={() => setVis("private")} /> <span>Private repositories<br /><span className="muted small">This {what} may be used by private and internal repositories in the organization.</span></span></label>
      <label className="radio-row"><input type="radio" name="vis" checked={vis === "selected"} onChange={() => setVis("selected")} data-testid="vis-selected" /> <span>Selected repositories<br /><span className="muted small">This {what} may only be used by specifically selected repositories.</span></span></label>
      {vis === "selected" && (
        <div className="box act-repo-pick" data-testid="repo-picker">
          <div className="box-header"><strong>{ids.length} selected repositor{ids.length === 1 ? "y" : "ies"}</strong></div>
          {repos.error !== undefined && <ErrorBox error={repos.error} />}
          {(repos.data || []).length === 0 && <div className="box-body muted small">No repositories in this organization.</div>}
          {(repos.data || []).map((r) => (
            <label key={r.id} className="check-row act-pick-row">
              <input type="checkbox" checked={ids.includes(r.id)} onChange={(e) => setIds(e.target.checked ? [...ids, r.id] : ids.filter((x) => x !== r.id))} />
              <span>{r.full_name}</span>
            </label>
          ))}
        </div>
      )}
    </fieldset>
  );
}

/** List + inline edit/delete of one scope's variables. */
export function VariablesList({ scope, newPath, editPath, canEdit = true }: { scope: Scope; newPath?: string; editPath?: (name: string) => string; canEdit?: boolean }) {
  const st = useAsync(() => A.variables(scope).then((r) => r.variables), [JSON.stringify(scope)]);
  const [err, setErr] = useState<unknown>();
  const word = scopeWord(scope);
  return (
    <div className="box" data-testid={`${scope.kind}-variables`}>
      <div className="box-header act-box-head">
        <strong>{cap(word)} variables</strong>
        {canEdit && newPath && <Link className="btn btn-sm btn-primary" to={newPath} data-testid={`new-${scope.kind}-variable`}>New {word} variable</Link>}
      </div>
      {err !== undefined && <ErrorBox error={err} />}
      <Async state={st}>
        {(list) => list.length === 0 ? <div className="box-body muted small">This {word} has no variables.</div> : (
          <table className="act-table">
            <thead><tr><th>Name</th><th>Value</th><th>Last updated</th><th><span className="sr-only">Actions</span></th></tr></thead>
            <tbody>
              {list.map((v) => (
                <tr key={v.name} data-testid={`variable-${v.name}`}>
                  <td className="mono act-break"><strong>{v.name}</strong></td>
                  <td className="mono act-break act-var-value">{v.value}</td>
                  <td className="muted small"><RelTime iso={v.updated_at} /></td>
                  <td className="act-right act-nowrap">
                    {canEdit && editPath && <Link className="btn btn-sm" to={editPath(v.name)} aria-label={`Edit ${v.name}`}>Edit</Link>}{" "}
                    {canEdit && (
                      <button type="button" className="btn btn-sm btn-danger-outline" aria-label={`Delete ${v.name}`} data-testid={`delete-variable-${v.name}`}
                        onClick={async () => {
                          if (!confirm(`Are you sure you want to delete ${v.name}?`)) return;
                          try {
                            await A.deleteVariable(scope, v.name);
                            st.reload();
                          } catch (e) {
                            setErr(e);
                          }
                        }}>Delete</button>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </Async>
    </div>
  );
}

export function VariableForm({ scope, existing, back, orgRepos }: { scope: Scope; existing?: string; back: string; orgRepos?: boolean }) {
  const nav = useNavigate();
  const cur = useAsync(
    () => (existing ? Promise.all([
      A.variables(scope).then((r) => r.variables.find((v) => v.name === existing) as Variable | undefined),
      scope.kind === "org" ? A.orgItemRepos(scope.org, "variables", existing).then((r) => r.repositories).catch(() => [] as Repo[]) : Promise.resolve([] as Repo[]),
    ]) : Promise.resolve(undefined)),
    [existing, JSON.stringify(scope)],
  );
  const [name, setName] = useState(existing || "");
  const [value, setValue] = useState("");
  const [vis, setVis] = useState<"all" | "private" | "selected">("all");
  const [repoIds, setRepoIds] = useState<number[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string>();
  if (cur.data && !loaded) {
    setLoaded(true);
    const [v, repos] = cur.data;
    if (v) {
      setValue(v.value);
      if (v.visibility) setVis(v.visibility);
    }
    setRepoIds(repos.map((r) => r.id));
  }
  const word = scopeWord(scope);
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const n = name.trim().toUpperCase();
    const ne = nameError(n);
    if (ne) return setErr(ne);
    if (!value) return setErr("Value is required.");
    setBusy(true);
    setErr(undefined);
    const extra = scope.kind === "org" ? { visibility: vis, ...(vis === "selected" ? { selected_repository_ids: repoIds } : {}) } : {};
    try {
      if (existing) await A.updateVariable(scope, existing, { name: n !== existing ? n : undefined, value, ...extra });
      else await A.createVariable(scope, { name: n, value, ...extra });
      nav(back);
    } catch (e2) {
      setErr(String((e2 as Error).message || e2));
    } finally {
      setBusy(false);
    }
  };
  return (
    <form className="act-form" onSubmit={submit} data-testid="variable-form">
      <h2 className="h3">{existing ? "Update variable" : `New ${word} variable`}</h2>
      <p className="muted">Note: Variable values are exposed as plain text. If you need to encrypt and mask sensitive information, create a secret instead.</p>
      <div className="form-group">
        <label htmlFor="var-name">Name <span className="c-danger">*</span></label>
        <input id="var-name" className="input mono" value={name} onChange={(e) => setName(e.target.value)} placeholder="YOUR_VARIABLE_NAME" autoComplete="off" />
      </div>
      <div className="form-group">
        <label htmlFor="var-value">Value <span className="c-danger">*</span></label>
        <textarea id="var-value" className="input textarea mono" rows={4} value={value} onChange={(e) => setValue(e.target.value)} spellCheck={false} />
      </div>
      {scope.kind === "org" && orgRepos && <RepoAccess org={scope.org} vis={vis} setVis={setVis} ids={repoIds} setIds={setRepoIds} what="variable" />}
      {err && <Flash kind="error">{err}</Flash>}
      <div className="form-actions act-left">
        <button type="submit" className="btn btn-primary" disabled={busy} data-testid="variable-submit">{busy ? "Saving…" : existing ? "Update variable" : "Add variable"}</button>
        <Link className="btn" to={back}>Cancel</Link>
      </div>
    </form>
  );
}

/** Read-only list of organization secrets/variables a repository can use. */
export function InheritedList({ title, load, empty }: { title: string; load: () => Promise<(Secret | Variable)[]>; empty: string }) {
  const st = useAsync(load, []);
  if (!st.data?.length) return st.error !== undefined ? null : (
    <div className="box"><div className="box-header"><strong>{title}</strong></div><div className="box-body muted small">{empty}</div></div>
  );
  return (
    <div className="box">
      <div className="box-header"><strong>{title}</strong></div>
      <ul className="list">
        {st.data.map((s) => (
          <li key={s.name} className="list-row">
            <span className="mono grow"><strong>{s.name}</strong></span>
            {"value" in s && <span className="mono muted act-break">{s.value}</span>}
            <span className="muted small">Updated <RelTime iso={s.updated_at} /></span>
          </li>
        ))}
      </ul>
    </div>
  );
}

export function NoAccess() {
  return <Blankslate title="You don't have access to these settings." />;
}
