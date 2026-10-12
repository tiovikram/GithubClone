import { useEffect, useState, type FormEvent } from "react";
import { Navigate, useNavigate } from "react-router-dom";
import { api } from "../api";
import { BranchSelect } from "../components/BranchSelect";
import { ErrorBox, Flash } from "../components/common";
import { useRepo } from "../components/RepoLayout";
import { useAsync, useTitle } from "../hooks";

export function RepoSettingsPage() {
  const { repo, owner, name, base, isAdmin, setRepo } = useRepo();
  const nav = useNavigate();
  useTitle(`Settings · ${repo.full_name}`);
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
    <div className="narrow-form settings">
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

      <h2 className="h3 c-danger">Danger Zone</h2>
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
