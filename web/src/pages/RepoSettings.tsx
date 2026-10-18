import { useEffect, useState, type FormEvent } from "react";
import { Navigate, useNavigate } from "react-router-dom";
import { api } from "../api";
import { BranchSelect } from "../components/BranchSelect";
import { ErrorBox, Flash } from "../components/common";
import { useRepo } from "../components/RepoLayout";
import { useAsync, useTitle } from "../hooks";
import { repoSettings, Slot } from "../areas";
import type { SettingsSection } from "../areas/types";
import type { RepoCtx } from "../components/RepoLayout";
import { SettingsShell } from "../components/SettingsShell";

const general: SettingsSection<RepoCtx> = { key: "general", path: "general", label: "General", order: 0, element: GeneralSettings };
export const allRepoSettings = [general, ...repoSettings].sort((a, b) => a.order - b.order);

/** `/{o}/{r}/settings/*`: sidebar of core + area sections. */
export function RepoSettingsPage() {
  const ctx = useRepo();
  useTitle(`Settings · ${ctx.repo.full_name}`);
  if (!ctx.isAdmin) return <Navigate to={ctx.base} replace />;
  return <SettingsShell root={`${ctx.base}/settings`} sections={allRepoSettings} ctx={ctx} label="Repository settings" container={false} />;
}

function GeneralSettings() {
  const ctx = useRepo();
  const { repo, owner, name, base, isAdmin, setRepo } = ctx;
  const nav = useNavigate();
  const branches = useAsync(() => api.branches(owner, name), [owner, name]);
  const [description, setDescription] = useState(repo.description || "");
  const [homepage, setHomepage] = useState(repo.homepage || "");
  const [newName, setNewName] = useState(repo.name);
  const [defBranch, setDefBranch] = useState(repo.default_branch);
  const [priv, setPriv] = useState(repo.private);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>();
  const [saved, setSaved] = useState(false);
  const [confirmText, setConfirmText] = useState("");
  const [delErr, setDelErr] = useState<unknown>();
  const [deleting, setDeleting] = useState(false);
  useEffect(() => {
    setDescription(repo.description || "");
    setDefBranch(repo.default_branch);
    setPriv(repo.private);
    setNewName(repo.name);
    setHomepage(repo.homepage || "");
  }, [repo]);
  if (!isAdmin) return <Navigate to={base} replace />;

  const save = async (e: FormEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(undefined);
    setSaved(false);
    try {
      const patch: Parameters<typeof api.updateRepo>[2] = { description, homepage };
      if (newName !== repo.name) patch.name = newName;
      if (defBranch !== repo.default_branch) patch.default_branch = defBranch;
      if (priv !== repo.private) patch.private = priv;
      const r = await api.updateRepo(owner, name, patch);
      setSaved(true);
      if (r.name !== repo.name) nav(`/${r.full_name}/settings`, { replace: true });
      else setRepo(r);
    } catch (err) {
      setError(err);
    } finally {
      setBusy(false);
    }
  };
  const del = async (e: FormEvent) => {
    e.preventDefault();
    setDeleting(true);
    setDelErr(undefined);
    try {
      await api.deleteRepo(owner, name);
      nav(`/`, { replace: true });
    } catch (err) {
      setDelErr(err);
      setDeleting(false);
    }
  };
  const names = (branches.data || []).map((b) => b.name);
  return (
    <div className="settings">
      <h2 className="h3">General</h2>
      <form className="box box-body" onSubmit={save}>
        {saved && <Flash kind="success">Repository settings saved.</Flash>}
        {error !== undefined && <ErrorBox error={error} />}
        <div className="form-group">
          <label htmlFor="set-name">Repository name</label>
          <input id="set-name" className="input" required value={newName} onChange={(e) => setNewName(e.target.value)} />
        </div>
        <div className="form-group">
          <label htmlFor="set-desc">Description</label>
          <input id="set-desc" className="input" value={description} onChange={(e) => setDescription(e.target.value)} />
        </div>
        <div className="form-group">
          <label htmlFor="set-home">Website</label>
          <input id="set-home" type="url" className="input" value={homepage} placeholder="https://"
            onChange={(e) => setHomepage(e.target.value)} />
        </div>
        <div className="form-group">
          <label htmlFor="set-branch">Default branch</label>
          {names.length ? (
            <BranchSelect id="set-branch" branches={names} value={defBranch} onChange={setDefBranch} label="Default branch" />
          ) : (
            <p className="muted small">{branches.loading ? "Loading branches…" : "Push a branch to choose a default branch."}</p>
          )}
        </div>
        <div className="form-group">
          <label className="check-row">
            <input type="checkbox" checked={priv} onChange={(e) => setPriv(e.target.checked)} /> Private repository
          </label>
        </div>
        <button type="submit" className="btn btn-primary" disabled={busy}>{busy ? "Saving…" : "Save changes"}</button>
      </form>
      <Slot name="repo.settings.general" ctx={ctx} />

      <h2 className="h3 c-danger">Danger Zone</h2>
      <Slot name="repo.settings.danger" ctx={ctx} />
      <form className="box danger-zone box-body" onSubmit={del}>
        <h3 className="h5">Delete this repository</h3>
        <p className="muted small">Once you delete a repository, there is no going back. Please be certain.</p>
        {delErr !== undefined && <ErrorBox error={delErr} />}
        <div className="form-group">
          <label htmlFor="del-confirm">To confirm, type <strong>{repo.full_name}</strong> in the box below</label>
          <input id="del-confirm" className="input" value={confirmText} autoComplete="off"
            onChange={(e) => setConfirmText(e.target.value)} />
        </div>
        <button type="submit" className="btn btn-danger" disabled={deleting || confirmText !== repo.full_name}>
          {deleting ? "Deleting…" : "Delete this repository"}
        </button>
      </form>
    </div>
  );
}
